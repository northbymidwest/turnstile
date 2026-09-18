//! Installing the original games' data out of a GOG.com installer.
//!
//! OpenRCT2 and OpenLoco reimplement the game engines, not the games: both
//! need the graphics, sounds and scenarios from the original release before
//! they will run at all. The originals were never released for the Mac, so the
//! data has to come out of the Windows installer people already own.
//!
//! Those installers are Inno Setup executables, and GOG's split every file
//! into parts, compress them again, and store them under the MD5 of their
//! contents, with the real name recorded in the installer's own script. The
//! `inno` crate does that reassembly; this module decides which of those files
//! belong in a game directory and puts them there.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::download::{Progress, Status};
use crate::error::CoreError;

/// An original game whose data one of the reimplementations needs.
///
/// Not [`GameId`]: that is a game Turnstile installs, of which there are two,
/// while there are three of these. RollerCoaster Tycoon 1's data is extra
/// content for OpenRCT2 rather than a game in its own right.
///
/// [`GameId`]: crate::game::GameId
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OriginalGame {
    RollerCoasterTycoon1,
    RollerCoasterTycoon2,
    Locomotion,
}

impl OriginalGame {
    pub const ALL: [Self; 3] = [
        Self::RollerCoasterTycoon1,
        Self::RollerCoasterTycoon2,
        Self::Locomotion,
    ];

    /// The directory this game's data is installed into, under whichever root
    /// is configured. Stable, because a game's configuration file points at it
    /// and renaming this would strand the install.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::RollerCoasterTycoon1 => "rct1",
            Self::RollerCoasterTycoon2 => "rct2",
            Self::Locomotion => "locomotion",
        }
    }

    /// The executable that identifies an installer as holding this game. The
    /// data files cannot: RollerCoaster Tycoon 2's `Data/g1.dat` and
    /// Locomotion's `Data/g1.DAT` are the same name on a Mac.
    #[must_use]
    const fn executable(self) -> &'static str {
        match self {
            Self::RollerCoasterTycoon1 => "RCT.EXE",
            Self::RollerCoasterTycoon2 => "RCT2.EXE",
            Self::Locomotion => "LOCO.EXE",
        }
    }

    /// The file a reimplementation looks for to decide whether a directory
    /// really holds this game, checked after installing rather than reporting
    /// a success the game will reject.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::RollerCoasterTycoon1 => "Data/csg1.dat",
            Self::RollerCoasterTycoon2 => "Data/g1.dat",
            Self::Locomotion => "Data/g1.DAT",
        }
    }
}

/// Whether `directory` holds this game's data already.
#[must_use]
pub fn installed_at(game: OriginalGame, directory: &Path) -> bool {
    directory.join(game.marker()).exists()
}

/// Reads an installer and says which game's data it holds.
///
/// Returns `Ok(None)` for an installer this does not recognise, which is an
/// ordinary thing for somebody to pick by mistake.
///
/// # Errors
///
/// Returns [`CoreError::ExtractionFailed`] if the file cannot be read as an
/// Inno Setup installer at all.
pub fn identify(installer: &Path) -> Result<Option<OriginalGame>, CoreError> {
    let inno = open(installer)?;
    let files = inno::gog::files(inno.file_entries());

    Ok(OriginalGame::ALL.into_iter().find(|game| {
        files.iter().any(|file| {
            is_install_file(file) && file.path().eq_ignore_ascii_case(game.executable())
        })
    }))
}

/// Installs the game data in `installer` into `destination`.
///
/// Everything is extracted to a staging directory beside the destination and
/// moved into place with one rename, for the same reason [`install`] does it:
/// a failure part way through must not leave a directory that looks like a
/// game and is not one.
///
/// [`install`]: crate::install::install
///
/// # Errors
///
/// Returns [`CoreError::ExtractionFailed`] if the installer cannot be read, if
/// a file fails its recorded checksum, or if what was extracted does not
/// contain the game's marker file.
pub fn install_game_data(
    installer: &Path,
    game: OriginalGame,
    destination: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    let parent = destination.parent().ok_or_else(|| {
        CoreError::ExtractionFailed(format!("{} has no parent directory", destination.display()))
    })?;
    std::fs::create_dir_all(parent).map_err(io)?;

    let staging = parent.join(format!(".staging-{}", game.directory()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(io)?;

    let result = extract_into(installer, &staging, progress, cancel);

    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    if !installed_at(game, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(CoreError::ExtractionFailed(format!(
            "the installer did not contain {}",
            game.marker()
        )));
    }

    // Move an older copy aside rather than deleting it first, so a failure
    // here leaves the old data in place.
    let previous = parent.join(format!(".previous-{}", game.directory()));
    let _ = std::fs::remove_dir_all(&previous);
    let had_previous = destination.exists();
    if had_previous {
        std::fs::rename(destination, &previous).map_err(io)?;
    }

    if let Err(error) = std::fs::rename(&staging, destination) {
        if had_previous {
            let _ = std::fs::rename(&previous, destination);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(io(error));
    }

    let _ = std::fs::remove_dir_all(&previous);

    Ok(())
}

/// Reads every file the installer means to put in the game directory and
/// writes it under `destination`.
fn extract_into(
    installer: &Path,
    destination: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    let mut inno = open(installer)?;

    let files: Vec<_> = inno::gog::files(inno.file_entries())
        .into_iter()
        .filter(is_install_file)
        .collect();

    if files.is_empty() {
        return Err(CoreError::ExtractionFailed(
            "the installer holds no game files".into(),
        ));
    }

    // One stored location can be the source for more than one file: an
    // installer holding the same bytes twice stores them once. Keyed by
    // location alone, the second file is silently never written.
    let mut destinations: BTreeMap<u32, Vec<(usize, usize)>> = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        for (part, entry) in file.parts().iter().enumerate() {
            let Some(location) = inno
                .file_entries()
                .get(*entry)
                .map(inno::entry::File::location)
            else {
                continue;
            };
            destinations
                .entry(location)
                .or_default()
                .push((index, part));
        }
    }

    let total = files.len();
    let mut written = 0_usize;
    let mut pending: BTreeMap<usize, Vec<Option<Vec<u8>>>> = BTreeMap::new();

    let wanted: Vec<u32> = destinations.keys().copied().collect();
    for result in inno.filtered_files(|entry| wanted.binary_search(&entry.location_index()).is_ok())
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }

        let (entry, data) = result.map_err(|error| {
            CoreError::ExtractionFailed(format!("could not read the installer: {error}"))
        })?;

        let Some(targets) = destinations.get(&entry.location_index()) else {
            continue;
        };

        for (index, part) in targets.clone() {
            let file = &files[index];
            let slots = pending
                .entry(index)
                .or_insert_with(|| vec![None; file.parts().len()]);
            slots[part] = Some(data.clone());

            if slots.iter().all(Option::is_some) {
                let parts: Vec<Vec<u8>> = pending
                    .remove(&index)
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect();

                let bytes = file.assemble(&parts).map_err(|error| {
                    CoreError::ExtractionFailed(format!("{}: {error}", file.path()))
                })?;

                write_file(destination, file.path(), &bytes)?;
                written += 1;

                #[allow(clippy::cast_precision_loss)]
                progress(Progress {
                    status: Status::Extracting,
                    value: Some(written as f64 / total as f64),
                });
            }
        }
    }

    if written < total {
        return Err(CoreError::ExtractionFailed(format!(
            "only {written} of {total} files could be read out of the installer"
        )));
    }

    Ok(())
}

/// Whether a file the installer produces belongs in the game directory.
///
/// An installer writes into several places, named by the directory constants
/// it uses: `tmp` for the wizard's own images, `commonappdata` for a support
/// uninstaller. Files recovered from their parts carry no constant at all,
/// because the script names them relative to where the installer is unpacking,
/// and those are the bulk of the game.
fn is_install_file(file: &inno::gog::InstallerFile) -> bool {
    matches!(file.root(), None | Some("app"))
}

/// Writes one extracted file, refusing any path that would leave the
/// destination. The paths come out of the installer, so they are not to be
/// trusted with a `..` or a leading separator.
fn write_file(destination: &Path, path: &str, bytes: &[u8]) -> Result<(), CoreError> {
    let mut target = destination.to_path_buf();

    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(CoreError::ExtractionFailed(format!(
                "the installer names a file outside the game directory: {path}"
            )));
        }
        target.push(component);
    }

    if target == destination {
        return Err(CoreError::ExtractionFailed(format!(
            "the installer names a file with no name: {path}"
        )));
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }

    std::fs::write(&target, bytes).map_err(io)
}

fn open(installer: &Path) -> Result<inno::Inno<std::io::BufReader<std::fs::File>>, CoreError> {
    inno::Inno::open(installer).map_err(|error| {
        CoreError::ExtractionFailed(format!(
            "{} is not an installer this can read: {error}",
            installer.display()
        ))
    })
}

fn io(error: std::io::Error) -> CoreError {
    CoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{OriginalGame, installed_at, write_file};

    #[test]
    fn every_game_has_a_directory_and_a_marker_of_its_own() {
        let mut directories: Vec<_> = OriginalGame::ALL.iter().map(|g| g.directory()).collect();
        directories.sort_unstable();
        directories.dedup();
        assert_eq!(directories.len(), OriginalGame::ALL.len());
    }

    #[test]
    fn the_two_games_that_share_a_marker_name_are_told_apart_by_something_else() {
        let rct2 = OriginalGame::RollerCoasterTycoon2;
        let loco = OriginalGame::Locomotion;
        assert!(rct2.marker().eq_ignore_ascii_case(loco.marker()));
        assert!(!rct2.executable().eq_ignore_ascii_case(loco.executable()));
    }

    #[test]
    fn a_directory_without_the_marker_is_not_an_install() {
        let directory = tempfile::tempdir().unwrap();
        assert!(!installed_at(
            OriginalGame::RollerCoasterTycoon2,
            directory.path()
        ));

        std::fs::create_dir_all(directory.path().join("Data")).unwrap();
        std::fs::write(directory.path().join("Data/g1.dat"), b"x").unwrap();
        assert!(installed_at(
            OriginalGame::RollerCoasterTycoon2,
            directory.path()
        ));
    }

    #[test]
    fn a_file_is_written_under_the_destination() {
        let directory = tempfile::tempdir().unwrap();
        write_file(directory.path(), "Data/g1.dat", b"contents").unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("Data/g1.dat")).unwrap(),
            b"contents"
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_destination_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let error = write_file(directory.path(), "../escaped.txt", b"x").unwrap_err();
        assert!(error.to_string().contains("outside"), "{error}");
        assert!(!directory.path().join("../escaped.txt").exists());
    }

    #[test]
    fn a_leading_separator_does_not_make_the_path_absolute() {
        let directory = tempfile::tempdir().unwrap();
        write_file(directory.path(), "/etc/passwd", b"x").unwrap();
        assert!(directory.path().join("etc/passwd").exists());
    }

    #[test]
    fn a_path_that_names_nothing_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        assert!(write_file(directory.path(), "/", b"x").is_err());
    }

    /// The installers are hundreds of megabytes and nobody's to redistribute,
    /// so they cannot live in the repository. Point this at a directory
    /// holding the GOG installers to run it:
    ///
    /// ```text
    /// TURNSTILE_INSTALLERS=~/Downloads cargo test -p turnstile-core --ignored
    /// ```
    #[test]
    #[ignore = "needs GOG installers; set TURNSTILE_INSTALLERS and run with --ignored"]
    fn real_installers_are_identified_and_installed() {
        use std::sync::atomic::AtomicBool;

        let directory = std::env::var("TURNSTILE_INSTALLERS")
            .expect("set TURNSTILE_INSTALLERS to a directory holding the installers");

        let mut found = Vec::new();
        for entry in std::fs::read_dir(&directory).expect("read the installer directory") {
            let path = entry.expect("read an entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("exe") {
                continue;
            }

            let Ok(Some(game)) = super::identify(&path) else {
                continue;
            };

            let into = tempfile::tempdir().expect("temp dir");
            let destination = into.path().join(game.directory());

            super::install_game_data(
                &path,
                game,
                &destination,
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

            assert!(
                installed_at(game, &destination),
                "{} installed without {}",
                path.display(),
                game.marker()
            );
            found.push(game);
        }

        found.sort_unstable();
        found.dedup();
        assert_eq!(
            found,
            OriginalGame::ALL.to_vec(),
            "expected one installer for each game in {directory}"
        );
    }
}
