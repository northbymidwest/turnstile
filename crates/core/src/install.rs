use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::download::{Progress, Status, download_to_temp};
use crate::error::CoreError;
use crate::extract::extract;
use crate::game::{GameId, sanitize_tag, version_file_in};
use crate::release::Download;
use crate::store::{Mode, VersionStore};

/// Installs `tag` from `download`.
///
/// Staging is the whole point: extraction, the slow and failure-prone part,
/// happens somewhere harmless, and the destination is only ever touched by two
/// instant renames.
pub fn install(
    store: &VersionStore,
    mode: Mode,
    tag: &str,
    download: &Download,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    // A crash on either side of the two renames below can leave a build
    // orphaned in `.trash-<name>`. Sweep regardless of which tag is being
    // installed now: nothing else ever looks for it again.
    recover_stray_trash(store, mode);

    let safe_tag = sanitize_tag(tag)?;
    let game_path = store.bin_path().parent().unwrap().to_path_buf();

    let (staging_parent, destination, name) = match mode {
        Mode::MultiVersion => {
            let name = resolve_multi_version_name(store, &safe_tag, tag);
            let destination = store.versions_path().join(&name);
            (store.versions_path(), destination, name)
        }
        // One shared destination regardless of tag, so no collision to resolve.
        Mode::Compatible => (game_path.clone(), store.bin_path(), safe_tag.clone()),
    };
    std::fs::create_dir_all(&staging_parent).map_err(|e| CoreError::Io(e.to_string()))?;

    let staging = staging_parent.join(format!(".staging-{name}"));
    let (trash, marker) = trash_and_marker(&staging_parent, &name);
    let temp_archive = staging_parent.join(format!(".download-{name}"));

    // Fail fast, before paying for a download and an extraction: the sweep
    // above just ran, so anything still occupying this trash name belongs to
    // something else. Checked again below, since the download takes real time.
    if trash_name_is_occupied(&destination, &trash) {
        return Err(occupied_trash_error(&trash));
    }

    let result = (|| -> Result<(), CoreError> {
        let _ = std::fs::remove_dir_all(&staging);

        download_to_temp(&download.url, &temp_archive, progress, cancel)?;

        progress(Progress {
            status: Status::Extracting,
            value: None,
        });
        std::fs::create_dir_all(&staging).map_err(|e| CoreError::Io(e.to_string()))?;
        let url_path = download.url.split('?').next().unwrap_or(&download.url);
        extract(&temp_archive, url_path, &staging)?;

        std::fs::write(staging.join(".version"), tag).map_err(|e| CoreError::Io(e.to_string()))?;

        let had_previous = std::fs::symlink_metadata(&destination).is_ok();
        if had_previous {
            if trash_name_is_occupied(&destination, &trash) {
                return Err(occupied_trash_error(&trash));
            }
            // Record intent before the rename that needs it, so a trash this
            // code creates always has a marker first. The marker holds the tag
            // being installed, so a later sweep can confirm the swap actually
            // completed rather than infer it from the destination existing.
            std::fs::write(&marker, tag.as_bytes()).map_err(|e| CoreError::Io(e.to_string()))?;
            std::fs::rename(&destination, &trash).map_err(|e| CoreError::Io(e.to_string()))?;
        }

        match std::fs::rename(&staging, &destination) {
            Ok(()) => {
                // Only once the trash it describes is actually gone. If the
                // removal fails, leave both: `recover_stray_trash` retries on
                // the next install of the *same* tag, since only then can it
                // re-confirm the marker against the destination.
                if had_previous && std::fs::remove_dir_all(&trash).is_ok() {
                    let _ = std::fs::remove_file(&marker);
                }
                progress(Progress {
                    status: Status::Extracting,
                    value: Some(1.0),
                });
                Ok(())
            }
            Err(e) => {
                // Put the previous installation back before reporting, and only
                // then clear the marker; if the restore itself fails, both
                // survive for `recover_stray_trash` to retry.
                if had_previous && std::fs::rename(&trash, &destination).is_ok() {
                    let _ = std::fs::remove_file(&marker);
                }
                Err(CoreError::Io(e.to_string()))
            }
        }
    })();

    let _ = std::fs::remove_file(&temp_archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Multi-version installs must never let two different tags collide on one
/// sanitized directory name: `sanitize_tag` maps every character outside
/// `[A-Za-z0-9._-]` to `-`, so `develop/2024-01-01` and `develop-2024-01-01`
/// sanitize alike. Mirrors `VersionStore::adopt_dir`'s free-name walk, except
/// that reinstalling the same tag overwrites in place.
fn resolve_multi_version_name(store: &VersionStore, safe_tag: &str, tag: &str) -> String {
    let versions = store.versions_path();
    let mut candidate = safe_tag.to_string();
    let mut n = 2;
    loop {
        let dir = versions.join(&candidate);
        // `symlink_metadata`, not `exists`: a dangling symlink already
        // occupying a candidate name still counts as taken.
        if std::fs::symlink_metadata(&dir).is_err() {
            return candidate;
        }
        let existing_tag = std::fs::read_to_string(version_file_in(&dir))
            .ok()
            .map(|s| s.trim().to_string());
        if existing_tag.as_deref() == Some(tag) {
            return candidate; // Reinstalling the same tag: overwrite in place.
        }
        candidate = format!("{safe_tag}-{n}");
        n += 1;
    }
}

/// Does `path` resolve to a real directory? Follows symlinks, unlike
/// `symlink_metadata`, which only answers "is this name taken".
fn resolves(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// Is `path` a marker this code wrote? `is_file` rather than mere existence,
/// so a directory can never be mistaken for a marker.
fn is_marker_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// The trash a `name` lands in while `install` swaps it out, and the marker
/// recording that this code is what put it there.
///
/// The marker uses a distinct *prefix* (`.trashmark-`) rather than a suffix on
/// the trash name, because a suffix is ambiguous: `sanitize_tag` preserves `.`,
/// so a tag ending in `.pending` would produce a trash directory a suffix
/// scheme could not tell apart from a marker.
fn trash_and_marker(staging_parent: &Path, name: &str) -> (PathBuf, PathBuf) {
    (
        staging_parent.join(format!(".trash-{name}")),
        staging_parent.join(format!(".trashmark-{name}")),
    )
}

/// Does `marker` prove that `destination` now holds the build `install` moved
/// `trash` aside to make room for? If the marker's tag matches the one
/// recorded at `destination`, the swap completed and the trash is safe to
/// delete. Anything else means something else populated `destination` since
/// (`activate`, `reconcile_compatible`, `disable_multi_version` and `adopt_dir`
/// all can), so it must not be deleted.
fn marker_confirms_superseded(destination: &Path, marker: &Path) -> bool {
    let Ok(marker_tag) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(destination_tag) = std::fs::read_to_string(version_file_in(destination)) else {
        return false;
    };
    marker_tag.trim() == destination_tag.trim()
}

/// Is `path` shaped like something this code could have put in `.trash-*`? A
/// trash is either a directory or the symlink that used to occupy a
/// compatible-mode `bin`, which can legitimately be dangling. Deliberately not
/// `resolves()`: that would reject a dangling-symlink trash and orphan a real
/// build. A plain file at this name is foreign debris.
fn is_trash_like(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir() || m.is_symlink())
        .unwrap_or(false)
}

/// Restores `trash` to `destination` when `destination` does not resolve.
/// `rename` refuses to replace an existing non-directory with a directory, so
/// a dangling symlink occupying the name has to be unlinked first; nothing
/// resolves through it, so nothing is destroyed.
///
/// Refuses to move anything not `is_trash_like`. This is the one place that
/// moves a candidate onto a destination, so the guard belongs here rather than
/// only where candidates are built.
fn restore_over(trash: &Path, destination: &Path) {
    if !is_trash_like(trash) {
        return;
    }
    if std::fs::symlink_metadata(destination).is_ok() {
        let _ = std::fs::remove_file(destination);
    }
    let _ = std::fs::rename(trash, destination);
}

/// Is `trash`'s name already occupied by something this install did not put
/// there? Only meaningful after `recover_stray_trash` has run: anything found
/// past that point cannot be claimed or cleared without guessing.
fn trash_name_is_occupied(destination: &Path, trash: &Path) -> bool {
    std::fs::symlink_metadata(destination).is_ok() && std::fs::symlink_metadata(trash).is_ok()
}

fn occupied_trash_error(trash: &Path) -> CoreError {
    CoreError::Io(format!(
        "{} already exists and is not this install's to replace",
        trash.display()
    ))
}

/// Where `install` puts its own working files: `versions/` when each build has
/// its own destination, the game directory when they all share `bin`.
fn staging_parent(store: &VersionStore, mode: Mode) -> Option<PathBuf> {
    match mode {
        Mode::MultiVersion => Some(store.versions_path()),
        Mode::Compatible => store.bin_path().parent().map(|p| p.to_path_buf()),
    }
}

/// Repairs the debris a crashed or force-quit install left behind, for a
/// caller that is not itself starting an install.
///
/// `install` sweeps trash on its own way in, but a crash between its two
/// renames leaves the build in `.trash-<name>` with nothing at the
/// destination, and until some later install ran the app reported that build
/// as not installed -- exactly the state in which a user has no reason to
/// start one. `controller::start` calls this at launch.
pub fn recover_debris(store: &VersionStore, mode: Mode) {
    recover_stray_trash(store, mode);
    sweep_stale_staging(store, mode, std::time::SystemTime::now());
}

/// How long a `.staging-*` or `.download-*` entry must sit untouched before it
/// is treated as debris. The margin is not for timing: nothing stops a second
/// copy of the app running against the same directories.
const STALE_DEBRIS_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Deletes staging directories and partial downloads that no install is using
/// any more. `install` only ever clears them under the name it is installing,
/// so a crash leaks a whole build's worth of disk under the tag that crashed.
///
/// Nothing deleted here is a game build: a staging directory holds an extract
/// no destination has ever pointed at, and a `.download-` file is a partial
/// archive. Both are reproducible by downloading again, which is why this may
/// delete on a time bound while the trash table may not.
///
/// Not called from `install`: two installs can briefly overlap in one process,
/// and a sweep from inside one could delete the other's live staging.
fn sweep_stale_staging(store: &VersionStore, mode: Mode, now: std::time::SystemTime) {
    let Some(staging_parent) = staging_parent(store, mode) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&staging_parent) else {
        return;
    };
    for entry in entries.flatten() {
        let filename = entry.file_name().to_string_lossy().into_owned();
        if !filename.starts_with(".staging-") && !filename.starts_with(".download-") {
            continue;
        }
        // `DirEntry::metadata` does not follow symlinks, so a symlink parked
        // at one of these names is left alone: this code never creates one.
        let Ok(meta) = entry.metadata() else { continue };
        // A timestamp in the future reads as "not old enough", which is the
        // safe direction for a delete.
        let stale = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_DEBRIS_AGE);
        if !stale {
            continue;
        }
        if meta.is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if meta.is_file() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Heals whatever `.trash-<name>` / `.trashmark-<name>` debris a previous
/// `install` left behind. Every trash this code creates gets a marker first,
/// so marker presence, trash presence, whether the destination resolves, and
/// whether the marker's tag matches the destination's fully determine what
/// happened:
///
/// | marker | trash | destination | verdict |
/// |--------|-------|-------------|---------|
/// | yes | no  | any | crashed before the trash rename, or after it was removed. Clear the marker. |
/// | yes | yes | not resolving | crashed between the two renames. Restore the trash. |
/// | yes | yes | resolving, tag confirmed | the swap completed; only cleanup was interrupted. Delete the trash. |
/// | yes | yes | resolving, tag unconfirmed | something else may have repopulated the destination. Leave it. |
/// | no  | yes | any | not produced by this code. Restore it if the destination does not resolve, otherwise leave it. |
///
/// Named generically rather than by the tag being installed, and swept
/// unconditionally, so a retry under any tag finds it.
fn recover_stray_trash(store: &VersionStore, mode: Mode) {
    let Some(staging_parent) = staging_parent(store, mode) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&staging_parent) else {
        return;
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    for entry in entries.flatten() {
        let filename = entry.file_name().to_string_lossy().into_owned();
        // Both prefixes: dropping the marker arm would lose the
        // `(marker, no trash)` row.
        if let Some(suffix) = filename.strip_prefix(".trashmark-") {
            names.insert(suffix.to_string());
        } else if let Some(suffix) = filename.strip_prefix(".trash-") {
            names.insert(suffix.to_string());
        }
    }
    if names.is_empty() {
        return;
    }

    match mode {
        // Each entry owns its destination alone, so each can be judged
        // independently against the table above.
        Mode::MultiVersion => {
            for name in &names {
                recover_one(&staging_parent, name, &staging_parent.join(name));
            }
        }
        // All entries share the single `bin` destination, which the per-entry
        // table cannot answer safely: restoring one would make `bin` resolve
        // for the next entry judged in the same pass.
        Mode::Compatible => {
            recover_sharing_one_destination(&staging_parent, &names, &store.bin_path());
        }
    }
}

/// Applies the marker table to one `.trash-<name>` with a destination of its
/// own (multi-version mode).
fn recover_one(staging_parent: &Path, name: &str, destination: &Path) {
    let (trash, marker) = trash_and_marker(staging_parent, name);
    let has_marker = is_marker_file(&marker);
    let has_trash = is_trash_like(&trash);

    match (has_marker, has_trash) {
        (true, false) => {
            let _ = std::fs::remove_file(&marker);
        }
        (true, true) if !resolves(destination) => {
            restore_over(&trash, destination);
            let _ = std::fs::remove_file(&marker);
        }
        (true, true) => {
            if marker_confirms_superseded(destination, &marker)
                && std::fs::remove_dir_all(&trash).is_ok()
            {
                let _ = std::fs::remove_file(&marker);
            }
            // Else: unconfirmed, or the delete failed. Leave both.
        }
        (false, true) => {
            if !resolves(destination) {
                restore_over(&trash, destination);
            }
            // Else: not produced by this code, and the destination holds
            // something real. Never delete.
        }
        (false, false) => {}
    }
}

/// Applies the marker table to every `.trash-*` in compatible mode, where all
/// entries share the single `bin` destination.
///
/// Whether `bin` already held something real is decided exactly once, before
/// touching anything: judging a later entry against a destination an earlier
/// entry in this pass just repopulated is how a build got destroyed in review.
fn recover_sharing_one_destination(
    staging_parent: &Path,
    names: &BTreeSet<String>,
    destination: &Path,
) {
    if resolves(destination) {
        for name in names {
            let (trash, marker) = trash_and_marker(staging_parent, name);
            let has_trash = is_trash_like(&trash);
            if is_marker_file(&marker) {
                if has_trash {
                    if marker_confirms_superseded(destination, &marker)
                        && std::fs::remove_dir_all(&trash).is_ok()
                    {
                        let _ = std::fs::remove_file(&marker);
                    }
                    // Else: unconfirmed, or the delete failed. Leave both.
                } else {
                    let _ = std::fs::remove_file(&marker);
                }
            }
            // Marker-less trash with a destination that already resolves: not
            // produced by this code. Never delete.
        }
        return;
    }

    // `bin` does not resolve. Markers with no trash are cleared outright.
    // Among the rest, at most one is genuinely the build that used to be at
    // `bin`; restore the newest by modification time. The others cannot be
    // proven superseded, so their markers are stripped rather than the
    // directories deleted, and a later sweep reads them as ordinary foreign
    // debris that is never deleted automatically.
    //
    // The losers are demoted before the winner is restored, and the winner's
    // marker is cleared only after, so a crash anywhere in between converges:
    // the next sweep either re-derives the same winner from the same mtimes,
    // or finds it as marker-with-no-trash and closes it cleanly.
    let mut candidates: Vec<(std::time::SystemTime, PathBuf, PathBuf)> = Vec::new();
    for name in names {
        let (trash, marker) = trash_and_marker(staging_parent, name);
        match std::fs::symlink_metadata(&trash) {
            Ok(meta) if is_trash_like(&trash) => candidates.push((
                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                trash,
                marker,
            )),
            // Either nothing is here, or something not trash-shaped is, and
            // neither is restorable. A marker beside it describes a trash that
            // does not exist, so it is cleared.
            _ => {
                let _ = std::fs::remove_file(&marker);
            }
        }
    }

    let winner = candidates
        .iter()
        .enumerate()
        .max_by_key(|(_, (mtime, ..))| *mtime)
        .map(|(i, _)| i);
    for (i, (_, _, marker)) in candidates.iter().enumerate() {
        if Some(i) != winner {
            let _ = std::fs::remove_file(marker);
        }
    }
    if let Some(i) = winner {
        let (_, trash, marker) = &candidates[i];
        restore_over(trash, destination);
        let _ = std::fs::remove_file(marker);
    }
}

/// Spawns the game and reports failure only if it has already given up. A
/// short wait catches an immediate failure (missing data files, a bad binary)
/// without holding the caller for the game's whole lifetime.
///
/// Deliberately does not verify a code signature: both games ship unsigned
/// macOS builds, so a check would make every launch fail.
pub fn launch(game: GameId, dir: &Path) -> Result<(), CoreError> {
    let exe: PathBuf = game.executable_in(dir);
    if !exe.exists() {
        return Err(CoreError::LaunchFailed(format!(
            "{} is not installed here",
            exe.display()
        )));
    }

    let mut child = std::process::Command::new(&exe)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::LaunchFailed(e.to_string()))?;

    std::thread::sleep(std::time::Duration::from_millis(500));

    match child.try_wait() {
        Ok(Some(status)) if !status.success() => {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            Err(CoreError::LaunchFailed(if stderr.trim().is_empty() {
                format!("the game exited immediately with {status}")
            } else {
                stderr.trim().to_string()
            }))
        }
        _ => {
            // Still running, or we could not tell. Move the child and its
            // stderr pipe into a detached thread: dropping `child` here would
            // close the pipe's read end, and std resets SIGPIPE to its default
            // in the child, so its next write to stderr would kill it. The
            // thread drains stderr so the game can never block on a full pipe,
            // then waits on the child so it is reaped rather than left a zombie.
            let stderr = child.stderr.take();
            std::thread::spawn(move || {
                if let Some(mut pipe) = stderr {
                    // Into `io::sink`, not a buffer: a buffer would grow for
                    // the game's entire lifetime.
                    let _ = std::io::copy(&mut pipe, &mut std::io::sink());
                }
                let _ = child.wait();
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Dirs;
    use crate::game::GameId;
    use crate::release::Download;
    use crate::store::{Mode, VersionStore};
    use std::sync::atomic::AtomicBool;

    struct Scratch {
        root: std::path::PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir()
                .join(format!("turnstile-install-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Scratch { root }
        }
        fn store(&self) -> VersionStore {
            VersionStore::new(Dirs::at(&self.root), GameId::OpenRCT2)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dl(url: &str) -> Download {
        Download {
            url: url.to_string(),
            bytes: 0,
        }
    }

    /// Serves a real zip containing a launchable OpenRCT2.app.
    fn serve_game_zip(scratch: &Scratch) -> String {
        let src = scratch.root.join("payload");
        let exe_dir = src.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        let zip = scratch.root.join("payload.zip");
        assert!(
            std::process::Command::new("/usr/bin/ditto")
                .args(["-c", "-k"])
                .arg(&src)
                .arg(&zip)
                .status()
                .unwrap()
                .success()
        );
        serve_file(std::fs::read(&zip).unwrap())
    }

    fn serve_file(body: Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/game-macos.zip", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut discard = [0u8; 1024];
                let _ = stream.read(&mut discard);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        url
    }

    #[test]
    fn installing_in_multi_version_mode_lands_in_the_store_and_activates() {
        let s = Scratch::new("multi");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        store.activate(Mode::MultiVersion, "v1").unwrap();

        assert!(s.root.join("OpenRCT2/versions/v1/OpenRCT2.app").exists());
        assert_eq!(
            std::fs::read_to_string(s.root.join("OpenRCT2/versions/v1/.version"))
                .unwrap()
                .trim(),
            "v1"
        );
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
    }

    #[test]
    fn installing_in_compatible_mode_produces_a_plain_bin_directory() {
        let s = Scratch::new("compat");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::Compatible,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let bin = s.root.join("OpenRCT2/bin");
        assert!(
            std::fs::symlink_metadata(&bin)
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert!(bin.join("OpenRCT2.app").exists());
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v1"
        );
        assert!(
            !s.root.join("OpenRCT2/versions").exists(),
            "no store in compatible mode"
        );
    }

    #[test]
    fn installing_twice_in_compatible_mode_replaces_cleanly() {
        let s = Scratch::new("twice");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::Compatible,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        install(
            &store,
            Mode::Compatible,
            "v2",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let bin = s.root.join("OpenRCT2/bin");
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v2"
        );
        assert!(
            std::fs::symlink_metadata(&bin)
                .unwrap()
                .file_type()
                .is_dir()
        );
        let leftovers: Vec<_> = std::fs::read_dir(s.root.join("OpenRCT2"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    #[test]
    fn a_failed_install_leaves_the_existing_version_intact_and_active() {
        let s = Scratch::new("failed");
        let store = s.store();
        let good = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&good),
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        store.activate(Mode::MultiVersion, "v1").unwrap();

        let bad = serve_file(b"not a zip at all".to_vec());
        let err = install(
            &store,
            Mode::MultiVersion,
            "v2",
            &dl(&bad),
            &mut |_| {},
            &cancel,
        );
        assert!(err.is_err());

        assert!(s.root.join("OpenRCT2/versions/v1/OpenRCT2.app").exists());
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
        assert!(!s.root.join("OpenRCT2/versions/.staging-v2").exists());
        assert!(!s.root.join("OpenRCT2/versions/v2").exists());
    }

    #[test]
    fn progress_reports_downloading_then_extracting() {
        let s = Scratch::new("progress");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);
        let mut statuses = Vec::new();

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |p| statuses.push(p.status),
            &cancel,
        )
        .unwrap();

        assert_eq!(statuses.first(), Some(&Status::Downloading));
        assert_eq!(statuses.last(), Some(&Status::Extracting));
    }

    #[test]
    fn a_hostile_tag_cannot_write_outside_the_store() {
        let s = Scratch::new("hostile");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "../escaped",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        assert!(!s.root.join("OpenRCT2/escaped").exists());
        assert!(s.root.join("OpenRCT2/versions/-.-escaped").is_dir());
    }

    #[test]
    fn a_crash_between_the_two_renames_is_recovered_by_a_later_install() {
        let s = Scratch::new("crashwindow");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::Compatible,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let bin = s.root.join("OpenRCT2/bin");
        let trash = s.root.join("OpenRCT2/.trash-v2");
        let marker = s.root.join("OpenRCT2/.trashmark-v2");
        std::fs::write(&marker, b"v2").unwrap();
        std::fs::rename(&bin, &trash).unwrap();
        assert!(!bin.exists());

        install(
            &store,
            Mode::Compatible,
            "v3",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        assert!(bin.join("OpenRCT2.app").exists());
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v3"
        );
        assert!(
            !trash.exists(),
            "the crash-orphaned build must not be left behind"
        );
        assert!(!marker.exists());
    }

    #[test]
    fn startup_recovery_restores_a_crash_orphaned_build_with_no_install() {
        let s = Scratch::new("startuprecover");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::Compatible,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let bin = s.root.join("OpenRCT2/bin");
        let trash = s.root.join("OpenRCT2/.trash-v2");
        let marker = s.root.join("OpenRCT2/.trashmark-v2");
        std::fs::write(&marker, b"v2").unwrap();
        std::fs::rename(&bin, &trash).unwrap();
        assert!(
            store.installed(Mode::Compatible).unwrap().is_empty(),
            "the app would say this"
        );

        recover_debris(&store, Mode::Compatible);

        assert!(
            bin.join("OpenRCT2.app").exists(),
            "the build is back where it belongs"
        );
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v1"
        );
        assert!(!trash.exists());
        assert!(!marker.exists());
        assert_eq!(store.installed(Mode::Compatible).unwrap().len(), 1);
    }

    #[test]
    fn startup_recovery_restores_a_crash_orphaned_build_in_multi_version_mode() {
        let s = Scratch::new("startuprecovermulti");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let dest = s.root.join("OpenRCT2/versions/v1");
        let trash = s.root.join("OpenRCT2/versions/.trash-v1");
        let marker = s.root.join("OpenRCT2/versions/.trashmark-v1");
        std::fs::write(&marker, b"v1").unwrap();
        std::fs::rename(&dest, &trash).unwrap();
        assert!(store.installed(Mode::MultiVersion).unwrap().is_empty());

        recover_debris(&store, Mode::MultiVersion);

        assert!(dest.join("OpenRCT2.app").exists());
        assert!(!trash.exists());
        assert!(!marker.exists());
    }

    #[test]
    fn stale_staging_debris_is_swept_and_work_in_progress_is_not() {
        let s = Scratch::new("stagingsweep");
        let store = s.store();
        let versions = s.root.join("OpenRCT2/versions");
        std::fs::create_dir_all(versions.join(".staging-v9/OpenRCT2.app")).unwrap();
        std::fs::write(versions.join(".download-v9"), b"partial").unwrap();
        std::fs::create_dir_all(versions.join("v1")).unwrap();

        sweep_stale_staging(&store, Mode::MultiVersion, std::time::SystemTime::now());
        assert!(
            versions.join(".staging-v9").is_dir(),
            "a live install must be left alone"
        );
        assert!(versions.join(".download-v9").exists());

        let later =
            std::time::SystemTime::now() + STALE_DEBRIS_AGE + std::time::Duration::from_secs(60);
        sweep_stale_staging(&store, Mode::MultiVersion, later);

        assert!(
            !versions.join(".staging-v9").exists(),
            "stale staging is debris"
        );
        assert!(
            !versions.join(".download-v9").exists(),
            "so is a partial download"
        );
        assert!(
            versions.join("v1").is_dir(),
            "and a real build is never touched"
        );
    }

    #[test]
    fn the_staging_sweep_never_touches_trash_or_its_marker() {
        let s = Scratch::new("stagingsweeptrash");
        let store = s.store();
        let game = s.root.join("OpenRCT2");
        std::fs::create_dir_all(game.join(".trash-v1")).unwrap();
        std::fs::write(game.join(".trashmark-v1"), b"v2").unwrap();
        std::fs::create_dir_all(game.join(".staging-v2")).unwrap();

        let later =
            std::time::SystemTime::now() + STALE_DEBRIS_AGE + std::time::Duration::from_secs(60);
        sweep_stale_staging(&store, Mode::Compatible, later);

        assert!(game.join(".trash-v1").is_dir());
        assert!(game.join(".trashmark-v1").exists());
        assert!(!game.join(".staging-v2").exists());
    }

    #[test]
    fn a_crash_between_the_two_renames_is_recovered_in_multi_version_mode_too() {
        let s = Scratch::new("crashwindowmulti");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let dest = s.root.join("OpenRCT2/versions/v1");
        let trash = s.root.join("OpenRCT2/versions/.trash-v1");
        let marker = s.root.join("OpenRCT2/versions/.trashmark-v1");
        std::fs::write(&marker, b"v1").unwrap();
        std::fs::rename(&dest, &trash).unwrap();

        install(
            &store,
            Mode::MultiVersion,
            "v2",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        assert!(
            dest.join("OpenRCT2.app").exists(),
            "v1 was recovered from the crash"
        );
        assert!(!trash.exists());
        assert!(!marker.exists());
    }

    #[test]
    fn a_stale_trash_with_a_marker_is_cleaned_up_once_the_destination_resolves_again() {
        let s = Scratch::new("stale-trash-marked");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");

        let exe_dir = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin.join(".version"), b"v2").unwrap();

        let stale_trash = game_dir.join(".trash-stale");
        let stale_marker = game_dir.join(".trashmark-stale");
        std::fs::create_dir_all(&stale_trash).unwrap();
        std::fs::write(&stale_marker, b"v2").unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        assert!(
            !stale_trash.exists(),
            "trash confirmed superseded by the destination it created must not linger"
        );
        assert!(!stale_marker.exists());
    }

    #[test]
    fn a_marker_that_does_not_match_the_destinations_tag_is_never_treated_as_confirmed() {
        let s = Scratch::new("stale-trash-unconfirmed");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");

        let exe_dir = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin.join(".version"), b"v3").unwrap();

        let trash = game_dir.join(".trash-stale");
        let marker = game_dir.join(".trashmark-stale");
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(&marker, b"v2").unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        assert!(trash.exists(), "an unconfirmed trash must never be deleted");
        assert!(marker.exists());
    }

    #[test]
    fn is_marker_file_rejects_a_directory_shaped_like_a_marker() {
        let s = Scratch::new("pending-collision");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");

        let exe_dir = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();

        let genuine = game_dir.join(".trash-genuine");
        std::fs::create_dir_all(genuine.join("OpenRCT2.app/Contents/MacOS")).unwrap();

        let impersonator = game_dir.join(".trash-genuine.pending");
        std::fs::create_dir_all(&impersonator).unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        assert!(
            genuine.exists(),
            "a marker-less trash must never be deleted, regardless of a same-shaped neighbor"
        );
        assert!(
            impersonator.exists(),
            "the impersonating directory is not a marker and must be left alone too"
        );
    }

    #[test]
    fn a_marker_less_trash_is_never_deleted_even_once_the_destination_resolves() {
        let s = Scratch::new("foreign-trash");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::Compatible,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let foreign = s.root.join("OpenRCT2/.trash-foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("marker"), b"unrelated").unwrap();

        install(
            &store,
            Mode::Compatible,
            "v2",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        assert!(
            foreign.exists(),
            "a trash directory this code did not create must never be deleted"
        );
    }

    #[test]
    fn a_dangling_destination_symlink_is_not_mistaken_for_a_superseding_build() {
        let s = Scratch::new("danglingsweep");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");
        std::fs::create_dir_all(&game_dir).unwrap();

        let trash = game_dir.join(".trash-v1");
        let marker = game_dir.join(".trashmark-v1");
        let exe = trash.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(&marker, b"v1").unwrap();
        std::os::unix::fs::symlink(s.root.join("nonexistent-target"), &bin).unwrap();
        assert!(
            std::fs::symlink_metadata(&bin).is_ok(),
            "bin exists as a symlink"
        );
        assert!(!bin.exists(), "but it is dangling");

        recover_stray_trash(&store, Mode::Compatible);

        assert!(
            bin.join("OpenRCT2.app").exists(),
            "the orphaned build must be restored"
        );
        assert!(!trash.exists());
        assert!(!marker.exists());
    }

    #[test]
    fn two_orphaned_trash_directories_are_never_resolved_by_deleting_either() {
        let s = Scratch::new("doubletrash");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        std::fs::create_dir_all(&game_dir).unwrap();
        let bin = game_dir.join("bin"); // absent: the destination does not resolve

        let make_trash = |name: &str| -> std::path::PathBuf {
            let trash = game_dir.join(format!(".trash-{name}"));
            let exe = trash.join("OpenRCT2.app/Contents/MacOS");
            std::fs::create_dir_all(&exe).unwrap();
            std::fs::write(exe.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::write(game_dir.join(format!(".trashmark-{name}")), name).unwrap();
            trash
        };

        let older = make_trash("older");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = make_trash("newer");

        recover_stray_trash(&store, Mode::Compatible);

        assert!(bin.join("OpenRCT2.app").exists());
        assert!(!newer.exists());
        assert!(
            older.exists(),
            "an ambiguous second trash must never be deleted"
        );
        assert!(
            !game_dir.join(".trashmark-older").exists(),
            "its marker is stripped so a later sweep reads it as foreign debris, not as provably superseded"
        );

        recover_stray_trash(&store, Mode::Compatible);
        assert!(older.exists(), "must still not be deleted on a later sweep");
    }

    #[test]
    fn a_trash_directory_that_cannot_be_deleted_keeps_its_marker_for_retry() {
        let s = Scratch::new("stuckdelete");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");
        let exe = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin.join(".version"), b"stuck").unwrap();

        let trash = game_dir.join(".trash-stuck");
        let locked = trash.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), b"x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::write(game_dir.join(".trashmark-stuck"), b"stuck").unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        assert!(trash.exists(), "the undeletable trash is still there");
        assert!(
            game_dir.join(".trashmark-stuck").exists(),
            "its marker must survive so a later sweep retries the delete"
        );

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn colliding_sanitized_tags_do_not_overwrite_each_others_builds() {
        let s = Scratch::new("collision");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "develop/2024-01-01",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        install(
            &store,
            Mode::MultiVersion,
            "develop-2024-01-01",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let installed = store.installed(Mode::MultiVersion).unwrap();
        assert_eq!(
            installed.len(),
            2,
            "both tags must survive as separate installs"
        );
        let mut tags: Vec<_> = installed.iter().map(|i| i.tag.clone()).collect();
        tags.sort();
        assert_eq!(tags, ["develop-2024-01-01", "develop/2024-01-01"]);
    }

    #[test]
    fn reinstalling_the_same_tag_still_overwrites_in_place() {
        let s = Scratch::new("sametag");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let installed = store.installed(Mode::MultiVersion).unwrap();
        assert_eq!(
            installed.len(),
            1,
            "reinstalling the same tag must not create a second directory"
        );
        assert_eq!(installed[0].name, "v1");
    }

    #[test]
    fn a_plain_file_at_a_trash_name_is_never_elected_or_restored() {
        let s = Scratch::new("trash-file-junk");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        std::fs::create_dir_all(&game_dir).unwrap();
        let bin = game_dir.join("bin"); // destination missing

        let junk = game_dir.join(".trash-junk");
        std::fs::write(&junk, b"not a build").unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        assert!(
            !bin.exists(),
            "a plain file must never be elected and restored onto the destination"
        );
        assert!(junk.exists(), "the foreign file itself is left untouched");
    }

    #[test]
    fn install_refuses_to_overwrite_a_trash_name_it_does_not_recognize() {
        let s = Scratch::new("trashcollision");
        let store = s.store();
        let url = serve_game_zip(&s);
        let cancel = AtomicBool::new(false);

        install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| {},
            &cancel,
        )
        .unwrap();

        let foreign = s.root.join("OpenRCT2/versions/.trash-v1");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("marker"), b"unrelated").unwrap();

        let mut progress_calls = 0;
        let err = install(
            &store,
            Mode::MultiVersion,
            "v1",
            &dl(&url),
            &mut |_| progress_calls += 1,
            &cancel,
        );

        assert!(
            err.is_err(),
            "must fail rather than guess what to do with foreign debris"
        );
        assert!(
            foreign.exists(),
            "the foreign debris must survive untouched"
        );
        assert!(foreign.join("marker").exists());
        assert!(s.root.join("OpenRCT2/versions/v1/OpenRCT2.app").exists());
        assert_eq!(progress_calls, 0, "must fail before downloading anything");
    }

    #[test]
    fn launching_a_missing_executable_is_an_error_not_a_panic() {
        let s = Scratch::new("nolaunch");
        let err = launch(GameId::OpenRCT2, &s.root.join("nowhere")).unwrap_err();
        assert!(matches!(err, CoreError::LaunchFailed(_)));
    }

    #[test]
    fn launching_a_working_executable_succeeds() {
        let s = Scratch::new("launch");
        let dir = s.root.join("v1");
        let exe_dir = dir.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("OpenRCT2");
        std::fs::write(&exe, b"#!/bin/sh\nsleep 5\n").unwrap();
        set_executable(&exe);

        launch(GameId::OpenRCT2, &dir).unwrap();
    }

    #[test]
    fn launching_something_that_exits_immediately_reports_its_stderr() {
        let s = Scratch::new("launchfail");
        let dir = s.root.join("v1");
        let exe_dir = dir.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("OpenRCT2");
        std::fs::write(&exe, b"#!/bin/sh\necho 'missing data files' >&2\nexit 3\n").unwrap();
        set_executable(&exe);

        let err = launch(GameId::OpenRCT2, &dir).unwrap_err();
        match err {
            CoreError::LaunchFailed(msg) => assert!(msg.contains("missing data files"), "{msg}"),
            other => panic!("expected LaunchFailed, got {other:?}"),
        }
    }

    #[test]
    fn launch_does_not_kill_a_game_that_keeps_writing_to_stderr() {
        let s = Scratch::new("nokill");
        let dir = s.root.join("v1");
        let exe_dir = dir.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("OpenRCT2");
        let marker = s.root.join("survived");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nfor i in $(seq 1 10); do echo tick >&2; sleep 0.1; done\ntouch {}\n",
                marker.display()
            ),
        )
        .unwrap();
        set_executable(&exe);

        launch(GameId::OpenRCT2, &dir).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(2000));
        assert!(
            marker.exists(),
            "the game did not survive long enough to finish"
        );
    }

    fn set_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}
