use std::path::PathBuf;

use crate::dirs::Dirs;
use crate::error::{StoreError, TagError};
use crate::game::{GameId, version_file_in};

/// Tag used when an installation has no `.version` file to identify it.
pub const UNKNOWN_TAG: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Upstream's layout: `bin/` is a directory holding one installation.
    Compatible,
    /// `versions/<tag>/` with `bin` as a symlink into it.
    MultiVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The `.version` file's contents, for display only. Two rows can
    /// share a tag: `adopt_dir`'s free-name walk keeps the original
    /// `.version` when it resolves a collision by renaming the directory,
    /// not the tag inside it. Never pass this to `activate`/`remove`.
    pub tag: String,
    /// The on-disk directory name under `versions/`. What `activate` and
    /// `remove` take, and what `active(MultiVersion)` reports: always
    /// unique, unlike `tag`.
    pub name: String,
    pub dir: PathBuf,
    pub can_launch: bool,
}

pub struct VersionStore {
    dirs: Dirs,
    game: GameId,
}

#[derive(Debug, Clone)]
pub struct DisableReport {
    /// The version that became the real `bin/`, or `None` when there was
    /// nothing linked to collapse. An `Option` rather than an empty string:
    /// the view renders this into "Now using {0}", and the empty-string
    /// convention rendered that as "Now using " with nothing after it.
    pub kept: Option<String>,
    pub retained: Vec<Installed>,
    pub retained_bytes: u64,
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}

/// Rejects anything that is not exactly one ordinary path component.
/// `activate`/`remove` take a directory name that must already exist under
/// `versions/`; a name shaped like a path (a separator, `.`, `..`, empty,
/// absolute) could retarget the operation at a directory it was never meant
/// to reach. Rejecting rather than `sanitize_tag`-style rewriting matters
/// here: rewriting a name that must already exist can silently make it name
/// a different, real directory instead, which for `remove` means deleting a
/// build the caller never named.
fn validate_name(name: &str) -> Result<(), StoreError> {
    let mut components = std::path::Path::new(name).components();
    let Some(std::path::Component::Normal(_)) = components.next() else {
        return Err(StoreError::Tag(TagError::Reserved));
    };
    if components.next().is_some() {
        return Err(StoreError::Tag(TagError::Reserved));
    }
    Ok(())
}

impl VersionStore {
    pub fn new(dirs: Dirs, game: GameId) -> VersionStore {
        VersionStore { dirs, game }
    }

    pub fn game(&self) -> GameId {
        self.game
    }

    pub fn bin_path(&self) -> PathBuf {
        self.game.bin_path(&self.dirs)
    }

    pub fn versions_path(&self) -> PathBuf {
        self.game.versions_path(&self.dirs)
    }

    fn describe(&self, dir: PathBuf) -> Installed {
        let tag = std::fs::read_to_string(version_file_in(&dir))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| UNKNOWN_TAG.to_string());
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let can_launch = self.game.executable_in(&dir).exists();
        Installed {
            tag,
            name,
            dir,
            can_launch,
        }
    }

    pub fn installed(&self, mode: Mode) -> Result<Vec<Installed>, StoreError> {
        match mode {
            Mode::Compatible => {
                let bin = self.bin_path();
                // `exists()` follows symlinks; in compatible mode a symlink
                // means an interrupted collapse, which reconcile() repairs.
                if bin.is_dir() {
                    Ok(vec![self.describe(bin)])
                } else {
                    Ok(Vec::new())
                }
            }
            Mode::MultiVersion => self.installed_multi(),
        }
    }

    pub fn active(&self, mode: Mode) -> Result<Option<String>, StoreError> {
        match mode {
            Mode::Compatible => Ok(self.installed(mode)?.into_iter().next().map(|i| i.tag)),
            Mode::MultiVersion => self.active_multi(),
        }
    }

    fn installed_multi(&self) -> Result<Vec<Installed>, StoreError> {
        let versions = self.versions_path();
        if !versions.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&versions).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().to_string();
            // `.staging-*` and `.trash-*` are ours, not installations.
            // `file_type()` does not follow symlinks (unlike `Path::is_dir`),
            // so a symlink placed in `versions/` is not listed as one
            // either: `remove`'s `remove_dir_all` cannot delete a symlink,
            // so listing one here would report an entry `remove` could
            // never act on.
            let Ok(ft) = entry.file_type() else { continue };
            if name.starts_with('.') || !ft.is_dir() {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let mut installed = self.describe(entry.path());
            if installed.tag == UNKNOWN_TAG {
                installed.tag = name;
            }
            out.push((modified, installed));
        }
        out.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        Ok(out.into_iter().map(|(_, i)| i).collect())
    }

    fn active_multi(&self) -> Result<Option<String>, StoreError> {
        let bin = self.bin_path();
        let Ok(target) = std::fs::read_link(&bin) else {
            return Ok(None);
        };
        // Only a link into `versions/` counts as an active version.
        let mut components = target.components().rev();
        let Some(last) = components.next() else {
            return Ok(None);
        };
        let is_in_versions = components
            .next()
            .map(|c| c.as_os_str() == "versions")
            .unwrap_or(false);
        if !is_in_versions {
            return Ok(None);
        }
        Ok(Some(last.as_os_str().to_string_lossy().to_string()))
    }

    /// `name` is a version's on-disk directory name under `versions/`, as
    /// `Installed::name` reports it, never the display `tag`, which two
    /// versions can share (see `Installed`).
    pub fn activate(&self, mode: Mode, name: &str) -> Result<(), StoreError> {
        if mode != Mode::MultiVersion {
            return Err(StoreError::RequiresMultiVersion);
        }
        validate_name(name)?;
        // `validate_name` proves the name is a safe single path component; it
        // says nothing about whether that directory is actually there. Without
        // this check, activating a name that no longer exists succeeds and
        // leaves `bin` dangling -- which is not merely a broken launch. A
        // dangling `bin` is the exact state behind this project's worst data
        // loss: an earlier sweep treated "symlink_metadata succeeds" as "the
        // destination is occupied", so a dangling link made it delete the only
        // real build. Refusing here keeps that state unreachable from the one
        // write path that could still create it.
        if !self.game.version_path(&self.dirs, name).is_dir() {
            return Err(StoreError::NoSuchVersion(name.to_string()));
        }
        let game_path = self.game.game_path(&self.dirs);
        std::fs::create_dir_all(&game_path).map_err(io)?;

        let tmp = game_path.join(".bin.tmp");
        let _ = std::fs::remove_file(&tmp);
        // Relative target, so the whole tree stays relocatable.
        std::os::unix::fs::symlink(PathBuf::from("versions").join(name), &tmp).map_err(io)?;
        // Renaming onto the existing link is atomic: there is no instant at
        // which `bin` is missing or half-written.
        std::fs::rename(&tmp, self.bin_path()).map_err(io)?;
        Ok(())
    }

    /// `name` is a version's on-disk directory name under `versions/`, as
    /// `Installed::name` reports it, never the display `tag`, which two
    /// versions can share (see `Installed`). Using `tag` here would let a
    /// caller aim the deletion at a directory they never named.
    pub fn remove(&self, mode: Mode, name: &str) -> Result<(), StoreError> {
        if mode != Mode::MultiVersion {
            return Err(StoreError::RequiresMultiVersion);
        }
        validate_name(name)?;
        if self.active(mode)?.as_deref() == Some(name) {
            return Err(StoreError::RemoveActive);
        }
        let dir = self.game.version_path(&self.dirs, name);
        // Redundant with `validate_name` today: a single validated path
        // component always resolves under `versions_path()`. Kept anyway as
        // the last check before an irreversible delete, so it still holds if
        // `version_path` or the validation above ever change independently.
        if dir.parent() != Some(self.versions_path().as_path()) {
            return Err(StoreError::Tag(TagError::Reserved));
        }
        // Reports rather than quietly succeeding, and symmetrically with
        // `activate` above (closed by 0f42dcf). `describe` builds
        // `Installed::name` with `to_string_lossy`, so a directory whose
        // name is not valid UTF-8 yields a name full of U+FFFD that matches
        // nothing on disk. Returning `Ok(())` for it told the UI the build
        // had been deleted, the reload put it straight back, and no
        // sequence of clicks could ever remove it.
        if !dir.exists() {
            return Err(StoreError::NoSuchVersion(name.to_string()));
        }
        std::fs::remove_dir_all(&dir).map_err(io)?;
        Ok(())
    }

    /// Repairs whatever layout is found. Never deletes a game build.
    pub fn reconcile(&self, mode: Mode) -> Result<(), StoreError> {
        match mode {
            Mode::MultiVersion => self.reconcile_multi(),
            Mode::Compatible => self.reconcile_compatible(),
        }
    }

    fn reconcile_multi(&self) -> Result<(), StoreError> {
        // A collapse that crashed after moving the active version out but
        // before putting it back in place leaves it here, invisible to
        // installed_multi (it isn't under versions/) and unreachable by any
        // other repair path in this mode: it would stay a permanent, silent
        // leak once bin no longer points at it. Fold it back into the store
        // first, under a free name, so the rest of this function treats it
        // like any other version.
        let incoming = self.game.game_path(&self.dirs).join(".bin.incoming");
        if incoming.is_dir() {
            self.adopt_dir(&incoming)?;
        }

        let bin = self.bin_path();
        let meta = std::fs::symlink_metadata(&bin);

        match meta {
            // A real directory: adopt it into the store.
            Ok(m) if m.file_type().is_dir() => {
                self.adopt_bin()?;
            }
            // A symlink: healthy if it resolves, otherwise re-point it.
            Ok(m) if m.file_type().is_symlink() => {
                if !bin.exists() {
                    std::fs::remove_file(&bin).map_err(io)?;
                    self.link_to_newest()?;
                }
            }
            Ok(_) => {}
            // Missing entirely.
            Err(_) => {
                self.link_to_newest()?;
            }
        }
        Ok(())
    }

    fn reconcile_compatible(&self) -> Result<(), StoreError> {
        let game_path = self.game.game_path(&self.dirs);
        let incoming = game_path.join(".bin.incoming");
        let bin = self.bin_path();

        // A collapse that died after moving the version out but before
        // putting it in place.
        if incoming.is_dir() {
            if std::fs::symlink_metadata(&bin)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                std::fs::remove_file(&bin).map_err(io)?;
            }
            if !bin.exists() {
                std::fs::rename(&incoming, &bin).map_err(io)?;
                return Ok(());
            }
        }

        let is_link = std::fs::symlink_metadata(&bin)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        // A dangling link: whatever it pointed at is gone (deleted or moved
        // outside the app, e.g. versions/<tag> removed by hand), so nothing
        // is behind it and unlinking destroys nothing. Mirrors the identical
        // case in reconcile_multi. Without this, disable_multi_version below
        // would try to resolve a version that no longer exists and fail
        // every time reconcile runs, which would mean the app fails to
        // start correctly on every launch in this state.
        if is_link && !bin.exists() {
            std::fs::remove_file(&bin).map_err(io)?;
            return Ok(());
        }

        // The user turned the mode off, or a collapse never started.
        if is_link {
            self.disable_multi_version()?;
        }
        Ok(())
    }

    fn link_to_newest(&self) -> Result<(), StoreError> {
        if let Some(newest) = self.installed_multi()?.into_iter().next() {
            self.activate(Mode::MultiVersion, &newest.name)?;
        }
        Ok(())
    }

    /// Moves a real `bin/` into the store under a free name and links to it.
    fn adopt_bin(&self) -> Result<String, StoreError> {
        let name = self.adopt_dir(&self.bin_path())?;
        self.activate(Mode::MultiVersion, &name)?;
        Ok(name)
    }

    /// Moves a real directory into the store under a free name. Never
    /// overwrites: walks to the first free `<tag>`, `<tag>-2`, ... name.
    fn adopt_dir(&self, src: &std::path::Path) -> Result<String, StoreError> {
        let versions = self.versions_path();
        std::fs::create_dir_all(&versions).map_err(io)?;

        let described = self.describe(src.to_path_buf());
        let base =
            crate::game::sanitize_tag(&described.tag).unwrap_or_else(|_| UNKNOWN_TAG.to_string());

        let mut name = base.clone();
        let mut n = 2;
        // `symlink_metadata`, not `exists`: a dangling symlink at this name
        // must still count as occupied, or the rename below fails with
        // ENOTDIR after this loop reports the name as free.
        while std::fs::symlink_metadata(versions.join(&name)).is_ok() {
            name = format!("{base}-{n}");
            n += 1;
        }

        std::fs::rename(src, versions.join(&name)).map_err(io)?;
        Ok(name)
    }

    pub fn enable_multi_version(&self) -> Result<(), StoreError> {
        let bin = self.bin_path();
        let is_real_dir = std::fs::symlink_metadata(&bin)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        if is_real_dir {
            self.adopt_bin()?;
        } else {
            self.reconcile_multi()?;
        }
        Ok(())
    }

    /// Collapses back to upstream's layout. The active version becomes a real
    /// `bin/`; every other version stays in `versions/` untouched.
    ///
    /// Ordered so an interruption is recoverable: move the version out to
    /// `.bin.incoming`, drop the link, then move it into place. A crash at
    /// any point leaves something `reconcile_compatible` can finish.
    pub fn disable_multi_version(&self) -> Result<DisableReport, StoreError> {
        let game_path = self.game.game_path(&self.dirs);
        let bin = self.bin_path();
        let incoming = game_path.join(".bin.incoming");

        // Checked before anything else, including the no-active-link branch
        // below: if a previous collapse crashed before finishing, this
        // directory currently holds that moved-but-not-yet-placed game
        // build, and by the time `bin` is inspected the crash can look
        // exactly like "nothing is linked" (a dangling symlink resolves to
        // `false` just like a missing one). Deleting it, or reporting
        // success while it sits there, would both misrepresent or destroy
        // it. reconcile(Compatible) is what finishes an interrupted
        // collapse, so ask for that instead.
        if incoming.exists() {
            return Err(StoreError::Io(format!(
                "{} exists from an interrupted operation; run reconcile first",
                incoming.display()
            )));
        }

        let Some(active_tag) = self.active_multi()? else {
            // Nothing is linked. Remove a stray link and leave the store be.
            if std::fs::symlink_metadata(&bin)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                std::fs::remove_file(&bin).map_err(io)?;
            }
            // Measured, not assumed to be zero. `retained` here is every
            // build in the store, and the whole point of the dialog this
            // feeds is to tell the user how much disk those builds occupy
            // so they can decide whether to go back and delete some.
            // Reporting "0 bytes" beside a non-empty list said there was
            // nothing to reclaim.
            let retained = self.installed_multi()?;
            let retained_bytes = retained.iter().map(|i| dir_size(&i.dir)).sum();
            return Ok(DisableReport {
                kept: None,
                retained,
                retained_bytes,
            });
        };

        let active_dir = self.game.version_path(&self.dirs, &active_tag);
        std::fs::rename(&active_dir, &incoming).map_err(io)?;
        if std::fs::symlink_metadata(&bin)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            std::fs::remove_file(&bin).map_err(io)?;
        }
        std::fs::rename(&incoming, &bin).map_err(io)?;

        let retained = self.installed_multi()?;
        let retained_bytes = retained.iter().map(|i| dir_size(&i.dir)).sum();

        Ok(DisableReport {
            kept: Some(active_tag),
            retained,
            retained_bytes,
        })
    }
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&p) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type()` does not follow symlinks (unlike `metadata()`).
            // macOS `.app` bundles routinely contain framework symlinks, so
            // following them would double-count bytes in every retained
            // build; an upward-pointing symlink would make this walk never
            // terminate.
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            } else if ft.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Dirs;
    use crate::game::GameId;

    pub(crate) struct Scratch {
        pub root: std::path::PathBuf,
    }

    impl Scratch {
        pub fn new(name: &str) -> Scratch {
            let root =
                std::env::temp_dir().join(format!("turnstile-store-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Scratch { root }
        }

        pub fn store(&self) -> VersionStore {
            VersionStore::new(Dirs::at(&self.root), GameId::OpenRCT2)
        }

        /// Creates a launchable installation directory for OpenRCT2.
        pub fn make_install(&self, dir: &std::path::Path, tag: Option<&str>) {
            let exe = dir.join("OpenRCT2.app/Contents/MacOS");
            std::fs::create_dir_all(&exe).unwrap();
            std::fs::write(exe.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
            if let Some(t) = tag {
                std::fs::write(dir.join(".version"), t).unwrap();
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn an_empty_tree_has_nothing_installed() {
        let s = Scratch::new("empty");
        let store = s.store();
        assert!(store.installed(Mode::Compatible).unwrap().is_empty());
        assert_eq!(store.active(Mode::Compatible).unwrap(), None);
    }

    #[test]
    fn a_real_bin_directory_is_the_single_installed_version() {
        let s = Scratch::new("single");
        let store = s.store();
        let bin = s.root.join("OpenRCT2/bin");
        s.make_install(&bin, Some("v0.5.5"));

        let installed = store.installed(Mode::Compatible).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].tag, "v0.5.5");
        assert_eq!(installed[0].dir, bin);
        assert!(installed[0].can_launch);
        assert_eq!(
            store.active(Mode::Compatible).unwrap(),
            Some("v0.5.5".into())
        );
    }

    #[test]
    fn a_bin_directory_without_a_version_file_reports_the_unknown_tag() {
        let s = Scratch::new("noversion");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), None);
        let installed = store.installed(Mode::Compatible).unwrap();
        assert_eq!(installed[0].tag, UNKNOWN_TAG);
        assert!(installed[0].can_launch);
    }

    #[test]
    fn a_bin_directory_without_an_executable_is_listed_but_not_launchable() {
        let s = Scratch::new("broken");
        let store = s.store();
        let bin = s.root.join("OpenRCT2/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(".version"), "v0.5.5").unwrap();
        let installed = store.installed(Mode::Compatible).unwrap();
        assert_eq!(installed.len(), 1);
        assert!(!installed[0].can_launch);
    }

    #[test]
    fn compatible_mode_ignores_a_versions_directory_entirely() {
        let s = Scratch::new("ignores");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        let installed = store.installed(Mode::Compatible).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].tag, "v1");
    }

    #[test]
    fn switching_operations_are_rejected_in_compatible_mode() {
        let s = Scratch::new("reject");
        let store = s.store();
        assert!(matches!(
            store.activate(Mode::Compatible, "v1"),
            Err(StoreError::RequiresMultiVersion)
        ));
        assert!(matches!(
            store.remove(Mode::Compatible, "v1"),
            Err(StoreError::RequiresMultiVersion)
        ));
    }

    #[test]
    fn versions_in_the_store_are_listed_newest_first() {
        let s = Scratch::new("listmulti");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v0.4.28"), Some("v0.4.28"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.make_install(&s.root.join("OpenRCT2/versions/v0.4.29"), Some("v0.4.29"));

        let installed = store.installed(Mode::MultiVersion).unwrap();
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0].tag, "v0.4.29", "newest first");
        assert_eq!(installed[1].tag, "v0.4.28");
    }

    #[test]
    fn dot_prefixed_directories_are_not_versions() {
        let s = Scratch::new("dotdirs");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/.staging-v2"), Some("v2"));
        s.make_install(&s.root.join("OpenRCT2/versions/.trash-v3"), Some("v3"));

        let tags: Vec<_> = store
            .installed(Mode::MultiVersion)
            .unwrap()
            .into_iter()
            .map(|i| i.tag)
            .collect();
        assert_eq!(tags, ["v1"]);
    }

    #[test]
    fn a_directory_name_is_the_fallback_tag_when_version_is_missing() {
        let s = Scratch::new("fallback");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v9.9.9"), None);
        let installed = store.installed(Mode::MultiVersion).unwrap();
        assert_eq!(
            installed[0].tag, "v9.9.9",
            "falls back to the directory name"
        );
    }

    #[test]
    fn activate_points_bin_at_the_requested_version() {
        let s = Scratch::new("activate");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));

        store.activate(Mode::MultiVersion, "v1").unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
        store.activate(Mode::MultiVersion, "v2").unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v2".into()));
    }

    #[test]
    fn the_bin_symlink_is_relative_so_the_tree_can_be_moved() {
        let s = Scratch::new("relative");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        let target = std::fs::read_link(store.bin_path()).unwrap();
        assert!(target.is_relative(), "symlink target was {target:?}");
        assert_eq!(target, std::path::Path::new("versions/v1"));
    }

    #[test]
    fn activating_over_an_existing_link_leaves_no_intermediate_state() {
        let s = Scratch::new("atomic");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        store.activate(Mode::MultiVersion, "v1").unwrap();
        store.activate(Mode::MultiVersion, "v2").unwrap();

        // No temporary link is left behind.
        assert!(!s.root.join("OpenRCT2/.bin.tmp").exists());
        assert!(store.bin_path().join("OpenRCT2.app").exists());
    }

    #[test]
    fn the_executable_resolves_through_the_symlink() {
        let s = Scratch::new("resolve");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();
        let exe = GameId::OpenRCT2.executable_in(&store.bin_path());
        assert!(
            exe.exists(),
            "executable did not resolve through the symlink"
        );
    }

    #[test]
    fn removing_the_active_version_is_refused() {
        let s = Scratch::new("rmactive");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();
        assert!(matches!(
            store.remove(Mode::MultiVersion, "v1"),
            Err(StoreError::RemoveActive)
        ));
        assert!(
            s.root.join("OpenRCT2/versions/v1").exists(),
            "nothing was deleted"
        );
    }

    #[test]
    fn removing_an_inactive_version_deletes_only_that_directory() {
        let s = Scratch::new("rmother");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        store.remove(Mode::MultiVersion, "v2").unwrap();
        assert!(!s.root.join("OpenRCT2/versions/v2").exists());
        assert!(s.root.join("OpenRCT2/versions/v1").exists());
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
    }

    #[test]
    fn active_is_none_when_bin_is_absent() {
        let s = Scratch::new("noactive");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), None);
    }

    #[test]
    fn reconcile_adopts_a_real_bin_directory_into_the_store() {
        let s = Scratch::new("adopt");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v0.5.5"));

        store.reconcile(Mode::MultiVersion).unwrap();

        assert!(s.root.join("OpenRCT2/versions/v0.5.5").is_dir());
        assert_eq!(
            store.active(Mode::MultiVersion).unwrap(),
            Some("v0.5.5".into())
        );
        assert!(GameId::OpenRCT2.executable_in(&store.bin_path()).exists());
    }

    #[test]
    fn adoption_uses_the_unknown_tag_when_no_version_file_exists() {
        let s = Scratch::new("adoptunknown");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), None);

        store.reconcile(Mode::MultiVersion).unwrap();

        assert!(s.root.join("OpenRCT2/versions/unknown").is_dir());
        assert_eq!(
            store.active(Mode::MultiVersion).unwrap(),
            Some("unknown".into())
        );
    }

    #[test]
    fn adoption_never_overwrites_an_existing_version_directory() {
        let s = Scratch::new("adoptcollide");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        std::fs::write(s.root.join("OpenRCT2/versions/v1/marker"), b"original").unwrap();
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v1"));

        store.reconcile(Mode::MultiVersion).unwrap();

        // The original is untouched and the adopted one landed beside it.
        assert_eq!(
            std::fs::read(s.root.join("OpenRCT2/versions/v1/marker")).unwrap(),
            b"original"
        );
        assert!(s.root.join("OpenRCT2/versions/v1-2").is_dir());
        assert_eq!(
            store.active(Mode::MultiVersion).unwrap(),
            Some("v1-2".into())
        );
    }

    #[test]
    fn reconcile_links_bin_to_the_newest_version_when_bin_is_missing() {
        let s = Scratch::new("missingbin");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));

        store.reconcile(Mode::MultiVersion).unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v2".into()));
    }

    #[test]
    fn reconcile_does_nothing_when_bin_is_missing_and_the_store_is_empty() {
        let s = Scratch::new("emptystore");
        let store = s.store();
        store.reconcile(Mode::MultiVersion).unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), None);
        assert!(!store.bin_path().exists());
    }

    #[test]
    fn reconcile_repairs_a_dangling_symlink() {
        let s = Scratch::new("dangling");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        std::os::unix::fs::symlink("versions/gone", s.root.join("OpenRCT2/bin")).unwrap();

        store.reconcile(Mode::MultiVersion).unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
    }

    #[test]
    fn reconcile_removes_a_dangling_symlink_when_the_store_is_empty() {
        let s = Scratch::new("danglingempty");
        let store = s.store();
        std::fs::create_dir_all(s.root.join("OpenRCT2")).unwrap();
        std::os::unix::fs::symlink("versions/gone", s.root.join("OpenRCT2/bin")).unwrap();

        store.reconcile(Mode::MultiVersion).unwrap();
        assert!(
            std::fs::symlink_metadata(store.bin_path()).is_err(),
            "link should be gone"
        );
    }

    #[test]
    fn reconcile_leaves_a_healthy_symlink_alone() {
        let s = Scratch::new("healthy");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        store.reconcile(Mode::MultiVersion).unwrap();
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
    }

    #[test]
    fn enabling_then_disabling_round_trips_and_keeps_everything() {
        let s = Scratch::new("roundtrip");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v1"));

        store.enable_multi_version().unwrap();
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        store.activate(Mode::MultiVersion, "v2").unwrap();

        let report = store.disable_multi_version().unwrap();

        assert_eq!(report.kept.as_deref(), Some("v2"));
        assert_eq!(report.retained.len(), 1);
        assert_eq!(report.retained[0].tag, "v1");
        assert!(report.retained_bytes > 0);

        // bin is a real directory again, holding what was active.
        let bin = store.bin_path();
        assert!(bin.is_dir());
        assert!(
            std::fs::symlink_metadata(&bin)
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v2"
        );
        assert_eq!(store.active(Mode::Compatible).unwrap(), Some("v2".into()));

        // Nothing was deleted.
        assert!(s.root.join("OpenRCT2/versions/v1").is_dir());

        // Re-enabling brings both back.
        store.enable_multi_version().unwrap();
        let tags: Vec<_> = store
            .installed(Mode::MultiVersion)
            .unwrap()
            .into_iter()
            .map(|i| i.tag)
            .collect();
        assert!(tags.contains(&"v1".to_string()));
        assert!(tags.contains(&"v2".to_string()));
    }

    #[test]
    fn enabling_with_nothing_installed_is_a_no_op() {
        let s = Scratch::new("enableempty");
        let store = s.store();
        store.enable_multi_version().unwrap();
        assert!(store.installed(Mode::MultiVersion).unwrap().is_empty());
    }

    #[test]
    fn compatible_reconcile_finishes_an_interrupted_collapse_from_a_symlink() {
        let s = Scratch::new("collapse1");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        store.reconcile(Mode::Compatible).unwrap();

        let bin = store.bin_path();
        assert!(
            std::fs::symlink_metadata(&bin)
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert_eq!(
            std::fs::read_to_string(bin.join(".version"))
                .unwrap()
                .trim(),
            "v1"
        );
    }

    #[test]
    fn compatible_reconcile_rescues_a_stray_incoming_directory() {
        let s = Scratch::new("collapse2");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/.bin.incoming"), Some("v1"));

        store.reconcile(Mode::Compatible).unwrap();

        assert!(store.bin_path().is_dir());
        assert!(!s.root.join("OpenRCT2/.bin.incoming").exists());
        assert_eq!(store.active(Mode::Compatible).unwrap(), Some("v1".into()));
    }

    #[test]
    fn compatible_mode_never_creates_a_symlink() {
        let s = Scratch::new("nosymlink");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v1"));
        store.reconcile(Mode::Compatible).unwrap();

        assert!(
            std::fs::symlink_metadata(store.bin_path())
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert!(!s.root.join("OpenRCT2/versions").exists());
    }

    // --- Critical 1: `Installed.name` vs `Installed.tag` -----------------

    #[test]
    fn remove_and_the_active_guard_key_on_directory_name_not_display_tag() {
        let s = Scratch::new("nameidentity");
        let store = s.store();
        // adopt_dir's free-name walk produces exactly this state on a
        // collision: two directories, one shared `.version` tag.
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/bin"), Some("v1"));
        store.reconcile(Mode::MultiVersion).unwrap();

        let installed = store.installed(Mode::MultiVersion).unwrap();
        assert_eq!(installed.len(), 2);
        assert!(
            installed.iter().all(|i| i.tag == "v1"),
            "both rows share a display tag"
        );
        let mut names: Vec<_> = installed.iter().map(|i| i.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["v1", "v1-2"], "but `name` distinguishes them");

        assert_eq!(
            store.active(Mode::MultiVersion).unwrap(),
            Some("v1-2".into())
        );

        // Keyed on name, the active guard refuses the row that is really
        // active...
        assert!(matches!(
            store.remove(Mode::MultiVersion, "v1-2"),
            Err(StoreError::RemoveActive)
        ));
        // ...and removing the other row by its real name deletes only that
        // one, leaving the active copy untouched.
        store.remove(Mode::MultiVersion, "v1").unwrap();
        assert!(!s.root.join("OpenRCT2/versions/v1").exists());
        assert!(
            s.root.join("OpenRCT2/versions/v1-2").is_dir(),
            "the active copy survives"
        );
    }

    #[test]
    fn activate_rejects_a_name_that_is_not_a_single_path_component() {
        let s = Scratch::new("activatetraversal");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));

        assert!(matches!(
            store.activate(Mode::MultiVersion, ".."),
            Err(StoreError::Tag(_))
        ));
        assert!(matches!(
            store.activate(Mode::MultiVersion, "../evil"),
            Err(StoreError::Tag(_))
        ));
        assert!(matches!(
            store.activate(Mode::MultiVersion, ""),
            Err(StoreError::Tag(_))
        ));
        // Nothing was linked by any of the rejected calls.
        assert!(std::fs::symlink_metadata(store.bin_path()).is_err());
    }

    #[test]
    fn activate_refuses_a_name_with_no_directory_rather_than_dangling_bin() {
        let s = Scratch::new("activatemissing");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        // The name is a perfectly valid single path component, so
        // `validate_name` passes it; only the existence check stops it.
        assert!(matches!(
            store.activate(Mode::MultiVersion, "v2"),
            Err(StoreError::NoSuchVersion(_))
        ));
        // The refusal left the previously working link alone rather than
        // replacing it with a dangling one: a failed switch must not cost the
        // user the build they already had.
        assert_eq!(
            std::fs::read_link(store.bin_path()).unwrap(),
            PathBuf::from("versions/v1")
        );
    }

    #[test]
    fn remove_rejects_a_traversal_string_smuggled_through_a_version_file() {
        let s = Scratch::new("removetraversal");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        // A hostile or corrupted `.version` file must not double as a path:
        // `tag` is display-only precisely because of input like this.
        s.make_install(&s.root.join("OpenRCT2/versions/evil"), Some("../../etc"));

        let installed = store.installed(Mode::MultiVersion).unwrap();
        let evil = installed.iter().find(|i| i.name == "evil").unwrap();
        assert_eq!(evil.tag, "../../etc");

        assert!(matches!(
            store.remove(Mode::MultiVersion, &evil.tag),
            Err(StoreError::Tag(_))
        ));
        // Nothing was deleted: not `versions/v1`, and not `evil` itself.
        assert!(s.root.join("OpenRCT2/versions/v1").is_dir());
        assert!(s.root.join("OpenRCT2/versions/evil").is_dir());
    }

    // --- Important 1: the stray-`.bin.incoming` guard must cover both
    // branches of `disable_multi_version` --------------------------------

    #[test]
    fn disabling_over_a_stray_incoming_directory_is_refused() {
        let s = Scratch::new("strayincoming");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();
        s.make_install(&s.root.join("OpenRCT2/.bin.incoming"), Some("v0"));

        assert!(store.disable_multi_version().is_err());
        assert!(
            s.root.join("OpenRCT2/.bin.incoming").is_dir(),
            "nothing was deleted"
        );
        assert!(
            s.root.join("OpenRCT2/versions/v1").is_dir(),
            "nothing was moved"
        );
    }

    #[test]
    fn disabling_over_a_stray_incoming_directory_is_refused_even_with_no_active_link() {
        let s = Scratch::new("strayincomingnolink");
        let store = s.store();
        // bin is absent, which is exactly what a dangling link left behind
        // by a crashed disable_multi_version also looks like through
        // active_multi. The old no-active branch reported success here
        // without ever looking at incoming.
        s.make_install(&s.root.join("OpenRCT2/.bin.incoming"), Some("v1"));

        assert!(store.disable_multi_version().is_err());
        assert!(
            s.root.join("OpenRCT2/.bin.incoming").is_dir(),
            "nothing was deleted"
        );
    }

    /// I3: the branch taken when `bin` is absent or points outside
    /// `versions/` used to hard-code zero bytes and an empty `kept`, so the
    /// dialog told the user their retained builds occupied "0 bytes" -- the
    /// one number the dialog exists to give them, and the one that says
    /// there is nothing to reclaim.
    #[test]
    fn disabling_with_nothing_linked_still_measures_the_builds_it_retains() {
        let s = Scratch::new("disablenolink");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/versions/v2"), Some("v2"));
        std::fs::write(s.root.join("OpenRCT2/versions/v1/payload"), vec![0u8; 4096]).unwrap();
        // No `bin` at all: reachable after a manual `rm`, and as the tail of
        // an interrupted mode change.
        assert!(std::fs::symlink_metadata(store.bin_path()).is_err());

        let report = store.disable_multi_version().unwrap();

        assert_eq!(
            report.kept, None,
            "nothing was linked, so nothing became bin"
        );
        assert_eq!(report.retained.len(), 2);
        assert!(
            report.retained_bytes >= 4096,
            "the retained builds are on disk and must be measured: {}",
            report.retained_bytes
        );
        assert!(
            s.root.join("OpenRCT2/versions/v1").is_dir(),
            "and nothing was deleted"
        );
        assert!(s.root.join("OpenRCT2/versions/v2").is_dir());
    }

    /// Deferred item 1's `remove` half, now symmetrical with `activate`.
    /// `Installed::name` comes from `to_string_lossy`, so a directory whose
    /// name is not valid UTF-8 produces a name full of U+FFFD that matches
    /// nothing on disk. Reporting success for it told the UI the build was
    /// deleted while the next reload put it straight back, and no sequence
    /// of clicks could ever remove it.
    #[test]
    fn removing_a_name_with_no_directory_reports_rather_than_silently_succeeding() {
        let s = Scratch::new("removeghost");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        store.activate(Mode::MultiVersion, "v1").unwrap();

        assert!(matches!(
            store.remove(Mode::MultiVersion, "v\u{fffd}2"),
            Err(StoreError::NoSuchVersion(_))
        ));
        assert!(
            s.root.join("OpenRCT2/versions/v1").is_dir(),
            "and nothing else was touched"
        );
    }

    // --- Important 2: compatible-mode reconcile must repair a dangling
    // symlink instead of erroring forever ---------------------------------

    #[test]
    fn compatible_reconcile_repairs_a_dangling_symlink() {
        let s = Scratch::new("compatdangling");
        let store = s.store();
        std::fs::create_dir_all(s.root.join("OpenRCT2")).unwrap();
        std::os::unix::fs::symlink("versions/gone", s.root.join("OpenRCT2/bin")).unwrap();

        store.reconcile(Mode::Compatible).unwrap();

        assert!(
            std::fs::symlink_metadata(store.bin_path()).is_err(),
            "dangling link should be gone"
        );
    }

    // --- Important 3: multi-version reconcile must adopt a stray
    // `.bin.incoming` instead of leaving it invisible ----------------------

    #[test]
    fn reconcile_multi_adopts_a_stray_incoming_directory() {
        let s = Scratch::new("multiincoming");
        let store = s.store();
        s.make_install(&s.root.join("OpenRCT2/versions/v1"), Some("v1"));
        s.make_install(&s.root.join("OpenRCT2/.bin.incoming"), Some("v2"));

        store.reconcile(Mode::MultiVersion).unwrap();

        assert!(s.root.join("OpenRCT2/versions/v2").is_dir());
        assert!(!s.root.join("OpenRCT2/.bin.incoming").exists());
        let tags: Vec<_> = store
            .installed(Mode::MultiVersion)
            .unwrap()
            .into_iter()
            .map(|i| i.tag)
            .collect();
        assert!(tags.contains(&"v1".to_string()));
        assert!(tags.contains(&"v2".to_string()));
    }

    // --- Important 4: `dir_size` must not follow symlinks -----------------

    #[test]
    fn dir_size_does_not_follow_symlinks() {
        let s = Scratch::new("dirsize");
        let dir = s.root.join("payload");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.bin"), vec![0u8; 100]).unwrap();
        // An upward-pointing symlink: following it would recurse forever.
        std::os::unix::fs::symlink(&s.root, dir.join("loop")).unwrap();

        assert_eq!(dir_size(&dir), 100);
    }
}
