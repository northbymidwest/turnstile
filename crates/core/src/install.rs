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
/// happens somewhere harmless, and the destination is only ever touched by
/// two instant renames. Extracting directly over the destination would mean
/// a failure mid-extraction leaves the user with no game until a rollback
/// completes.
pub fn install(
    store: &VersionStore,
    mode: Mode,
    tag: &str,
    download: &Download,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), CoreError> {
    // A crash on either side of the two renames below, in an earlier
    // install, can leave a build orphaned in `.trash-<name>`. Recover any
    // such leftovers before doing anything else, regardless of which tag is
    // being installed now: otherwise that build would be permanently
    // orphaned the moment a *different* tag is installed next, since
    // nothing else ever looks for it again.
    recover_stray_trash(store, mode);

    let safe_tag = sanitize_tag(tag)?;
    let game_path = store.bin_path().parent().unwrap().to_path_buf();

    let (staging_parent, destination, name) = match mode {
        Mode::MultiVersion => {
            let name = resolve_multi_version_name(store, &safe_tag, tag);
            let destination = store.versions_path().join(&name);
            (store.versions_path(), destination, name)
        }
        // One shared destination regardless of tag, so there is no
        // directory-name collision to resolve here.
        Mode::Compatible => (game_path.clone(), store.bin_path(), safe_tag.clone()),
    };
    std::fs::create_dir_all(&staging_parent).map_err(|e| CoreError::Io(e.to_string()))?;

    let staging = staging_parent.join(format!(".staging-{name}"));
    let (trash, marker) = trash_and_marker(&staging_parent, &name);
    let temp_archive = staging_parent.join(format!(".download-{name}"));

    // Fail fast, before paying for a download and an extraction: the sweep
    // above just ran, so anything still occupying this exact trash name
    // belongs to something else (undeletable debris the sweep correctly
    // declined to touch, most likely), and this install cannot proceed.
    // Checked again just before the rename that needs it below, since the
    // download and extraction take real time and something could occupy
    // the name in the meantime.
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
            // Record intent before the rename that needs it: a trash this
            // code creates always has a marker first, which is what makes
            // `recover_stray_trash` provably correct rather than guessing
            // from filesystem shape alone. The marker holds the tag being
            // installed, not just the destination path, so a later sweep
            // can confirm the swap actually completed (row 3) instead of
            // inferring it from the destination merely existing again.
            std::fs::write(&marker, tag.as_bytes()).map_err(|e| CoreError::Io(e.to_string()))?;
            std::fs::rename(&destination, &trash).map_err(|e| CoreError::Io(e.to_string()))?;
        }

        match std::fs::rename(&staging, &destination) {
            Ok(()) => {
                // Only clear the marker once the trash it describes is
                // actually gone. If the removal fails (for example a
                // read-only subdirectory inside the old build), leave both
                // in place: `recover_stray_trash` retries the deletion on
                // the next install of the *same* tag, since only then can
                // it re-confirm the marker against the destination (row 3).
                // In compatible mode, installing a *different* tag next
                // repopulates the destination with that tag instead, so
                // confirmation can never succeed again and this trash is
                // orphaned permanently rather than retried -- the accepted
                // cost of making row 3 provable instead of inferred, not a
                // silent loss (nothing is deleted; disk space is spent, not
                // a build). Multi-version mode is unaffected: each
                // destination keeps its own tag indefinitely.
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
                // Put the previous installation back before reporting, and
                // only then clear the marker; if the restore itself fails,
                // both survive for `recover_stray_trash` to retry.
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
/// `[A-Za-z0-9._-]` to `-`, so for example `develop/2024-01-01` and
/// `develop-2024-01-01` both sanitize to the same string. Mirrors
/// `VersionStore`'s own `adopt_dir` free-name walk, with one refinement:
/// reinstalling the same tag over itself must still overwrite in place
/// rather than pile up a new directory on every retry.
fn resolve_multi_version_name(store: &VersionStore, safe_tag: &str, tag: &str) -> String {
    let versions = store.versions_path();
    let mut candidate = safe_tag.to_string();
    let mut n = 2;
    loop {
        let dir = versions.join(&candidate);
        // `symlink_metadata`, not `exists`: a dangling symlink already
        // occupying a candidate name must still count as taken, the same
        // reasoning `adopt_dir` documents in store.rs.
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

/// Does `path` currently resolve to a real directory? Follows a symlink
/// (unlike `symlink_metadata`, which only answers "is this name taken");
/// a dangling symlink or a plain file both report `false`.
fn resolves(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// Is `path` a marker this code wrote? Checking `is_file` rather than mere
/// existence means a directory can never be mistaken for a marker, which
/// matters because `.trashmark-<name>` is still, in principle, a name a
/// hostile or coincidental tag could produce a trash directory at (see
/// `trash_and_marker`).
fn is_marker_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// The trash a `name` lands in while `install` swaps it out, and the
/// marker recording that this code, specifically, is the one that put it
/// there.
///
/// The marker uses a distinct *prefix* (`.trashmark-`), not a suffix on the
/// trash name (`.trash-<name>.pending`), because a suffix is ambiguous:
/// `sanitize_tag` preserves `.`, so a tag ending in `.pending` produces a
/// trash directory whose name a suffix-based scheme cannot distinguish from
/// a marker. `.trash-` and `.trashmark-` diverge at a fixed index (the
/// eighth character) that no tag content can reach, since the tag only ever
/// occupies the position after the prefix, so no sanitized name can ever
/// make one prefix parse as the other.
fn trash_and_marker(staging_parent: &Path, name: &str) -> (PathBuf, PathBuf) {
    (
        staging_parent.join(format!(".trash-{name}")),
        staging_parent.join(format!(".trashmark-{name}")),
    )
}

/// Does `marker` prove that `destination` now holds the build that
/// `install` moved `trash` aside to make room for? The marker holds the
/// tag `install` was installing when it wrote it; if that matches the tag
/// recorded at `destination` now, the swap that created this trash
/// demonstrably completed and the trash is safe to delete. If the read
/// fails or the tags differ, something else populated `destination` since
/// (see `install`'s callers: `activate`, `reconcile_compatible`,
/// `disable_multi_version` and `adopt_dir` in store.rs all can), and this
/// trash cannot be proven superseded, so it must not be deleted.
fn marker_confirms_superseded(destination: &Path, marker: &Path) -> bool {
    let Ok(marker_tag) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(destination_tag) = std::fs::read_to_string(version_file_in(destination)) else {
        return false;
    };
    marker_tag.trim() == destination_tag.trim()
}

/// Is `path` shaped like something this code could have put in `.trash-*`?
/// A trash is either a directory, or the symlink that used to occupy a
/// compatible-mode `bin` (renaming `bin` moves the link itself, and that
/// link can legitimately be dangling if its target was itself mid-swap).
/// Deliberately not `resolves()`: that would reject a dangling-symlink
/// trash and orphan a real build. A plain regular file at this name was
/// never put there by this code, most likely foreign debris, and must
/// never be elected a restore candidate or moved onto a destination.
fn is_trash_like(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir() || m.is_symlink())
        .unwrap_or(false)
}

/// Restores `trash` to `destination` when `destination` does not resolve.
/// `rename` refuses to replace an existing non-directory (including a
/// symlink) with a directory, so a dangling symlink occupying the name has
/// to be unlinked first, exactly as `reconcile_compatible` does in
/// store.rs. Unlinking it destroys nothing, since by definition nothing
/// resolves through it.
///
/// Refuses to move anything not `is_trash_like`: this is the one place
/// that actually moves a candidate onto a destination, so guarding here
/// means a future call site cannot forget the check the way it could if
/// eligibility were only filtered where candidates are built.
fn restore_over(trash: &Path, destination: &Path) {
    if !is_trash_like(trash) {
        return;
    }
    if std::fs::symlink_metadata(destination).is_ok() {
        let _ = std::fs::remove_file(destination);
    }
    let _ = std::fs::rename(trash, destination);
}

/// Is `trash`'s name already occupied by something this install did not
/// put there? Only meaningful once `had_previous` is true (there is a
/// build to displace) and after `recover_stray_trash` has already run:
/// anything found here past that point cannot be claimed or cleared
/// without guessing.
fn trash_name_is_occupied(destination: &Path, trash: &Path) -> bool {
    std::fs::symlink_metadata(destination).is_ok() && std::fs::symlink_metadata(trash).is_ok()
}

fn occupied_trash_error(trash: &Path) -> CoreError {
    CoreError::Io(format!(
        "{} already exists and is not this install's to replace",
        trash.display()
    ))
}

/// The directory `install` puts its own working files in, for `mode`:
/// `versions/` when each build has its own destination, the game directory
/// when they all share `bin`.
fn staging_parent(store: &VersionStore, mode: Mode) -> Option<PathBuf> {
    match mode {
        Mode::MultiVersion => Some(store.versions_path()),
        Mode::Compatible => store.bin_path().parent().map(|p| p.to_path_buf()),
    }
}

/// Repairs the debris a crashed or force-quit install left behind, for a
/// caller that is not itself starting an install.
///
/// `install` runs the trash sweep on its own way in, and for a long time
/// that was the *only* thing that ever ran it. A crash between install's
/// two renames leaves the user's build in `.trash-<name>` with nothing at
/// the destination, and until some later install happened to run, the app
/// reported that build as not installed -- which is exactly the state in
/// which a user has no reason to start an install, and every reason to
/// delete the mysterious `.trash-v0.5.5` sitting beside their saved games
/// and make the loss real. `controller::start` calls this at launch, before
/// `VersionStore::reconcile` and before anything reads `installed`.
///
/// Also sweeps stale staging debris, which `install` deliberately does not:
/// see `sweep_stale_staging`.
pub fn recover_debris(store: &VersionStore, mode: Mode) {
    recover_stray_trash(store, mode);
    sweep_stale_staging(store, mode, std::time::SystemTime::now());
}

/// How long a `.staging-*` or `.download-*` entry must have sat untouched
/// before it is treated as debris rather than as work in progress.
///
/// An install writes to both continuously and finishes in seconds: a real
/// OpenRCT2 release is 117 MB, and the slowest plausible download and
/// extraction together are orders of magnitude inside this. The threshold
/// is not there for a timing margin, it is there because nothing stops a
/// second copy of the app from running against the same directories.
/// Within one process this is unreachable by construction -- `start` runs
/// this before any install can have begun.
const STALE_DEBRIS_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Deletes staging directories and partial downloads that no install is
/// using any more.
///
/// `install` removes its own `.staging-<name>` and `.download-<name>` on
/// every path out, but only under the name it is installing: a crash leaves
/// one behind under the tag that crashed, and the next install of a
/// *different* tag never looks at it again, so it is a permanent leak of
/// however large that build was.
///
/// Nothing deleted here is a game build. A staging directory holds a
/// freshly extracted archive that no destination has ever pointed at -- the
/// two renames in `install` are what promote it, and the build it would
/// have displaced is in `.trash-<name>`, which this does not touch and
/// `recover_stray_trash` restores. A `.download-` file is a partial
/// archive. Both are reproducible by downloading again, which is why this
/// may delete on a time bound while the trash table may not delete on
/// anything short of proof.
///
/// Not called from `install`, only from `recover_debris`. Two installs can
/// briefly overlap inside one process (cancellation is cooperative, so a
/// cancelled worker may still be extracting), and a sweep from inside one
/// of them could delete the other's live staging directory.
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
        // `DirEntry::metadata` does not follow symlinks, so a symlink
        // parked at one of these names is judged as itself and, being
        // neither a directory nor a regular file, is left alone: this code
        // never creates one there.
        let Ok(meta) = entry.metadata() else { continue };
        // A timestamp in the future makes `duration_since` fail, which
        // reads here as "not old enough" and leaves the entry alone. That
        // is the safe direction: this is a delete.
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
/// `install` left behind, before this call touches anything else.
///
/// Every trash this code creates gets a marker file first (see `install`),
/// so the marker's presence, together with whether the trash still exists,
/// whether its destination currently resolves, and (for the resolving case)
/// whether the marker's recorded tag matches what is actually at the
/// destination now, fully determines what happened without guessing from
/// filesystem shape alone:
///
/// | marker | trash | destination    | verdict |
/// |--------|-------|----------------|---------|
/// | yes    | no    | any            | crashed before the trash rename, or after the trash was already removed. Clear the marker. |
/// | yes    | yes   | not resolving  | crashed between the two renames; the swap never completed. Restore the trash. |
/// | yes    | yes   | resolving, tag confirmed   | the swap that created this trash demonstrably completed; only this call's own cleanup was interrupted. Delete the trash. |
/// | yes    | yes   | resolving, tag not confirmed | the marker proves this code created the trash, but not that *this* swap is what repopulated the destination (`activate`, `reconcile_compatible`, `disable_multi_version` and `adopt_dir` all can too). Treat like the row below: never delete, leave it. |
/// | no     | yes   | any            | not produced by this code (for example `VersionStore::adopt_dir` minting the same name independently). Never delete: restore it if the destination does not resolve, otherwise leave it. |
///
/// Named generically, not by the tag currently being installed, and swept
/// unconditionally so a retry under any tag finds it, exactly as
/// `VersionStore::reconcile` recovers a stray `.bin.incoming` left by an
/// interrupted collapse.
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
        // Both prefixes are enumerated: dropping the marker arm would
        // silently lose the `(marker, no trash)` row, which is what cleans
        // up after a crash between writing the marker and renaming the
        // trash into place.
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
        // Each entry owns its destination alone (`versions/<name>`), so
        // every entry can be judged independently against the table above.
        Mode::MultiVersion => {
            for name in &names {
                recover_one(&staging_parent, name, &staging_parent.join(name));
            }
        }
        // All entries share the single `bin` destination, which the
        // per-entry table cannot answer safely: restoring one entry would
        // make `bin` resolve for the next entry judged in the same pass,
        // which is exactly how review found a build get destroyed.
        Mode::Compatible => {
            recover_sharing_one_destination(&staging_parent, &names, &store.bin_path());
        }
    }
}

/// Applies the marker table to one `.trash-<name>` whose destination is not
/// shared with any other entry (multi-version mode).
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
            // Else: not confirmed that the swap which created this trash
            // is what repopulated the destination, or the delete failed.
            // Leave both in place rather than guess.
        }
        (false, true) => {
            if !resolves(destination) {
                restore_over(&trash, destination);
            }
            // Else: not produced by this code, and the destination already
            // holds something real. Never delete; leave it on disk.
        }
        (false, false) => {}
    }
}

/// Applies the marker table to every `.trash-*` in compatible mode, where
/// all entries share the single `bin` destination.
///
/// Whether `bin` already held something real is decided exactly once,
/// before touching anything, and that decision must never depend on what
/// this same pass has already restored: judging a later entry against a
/// destination an earlier entry in this pass just repopulated is how a
/// build got destroyed in review.
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
                    // Else: not confirmed, or the delete failed. Leave both.
                } else {
                    let _ = std::fs::remove_file(&marker);
                }
            }
            // Marker-less trash with a destination that already resolves:
            // not produced by this code. Never delete; leave it.
        }
        return;
    }

    // `bin` does not resolve. Markers with no trash behind them are cleared
    // outright, since there is nothing to weigh them against. Among the
    // rest, at most one is genuinely the build that used to be at `bin`;
    // restore the newest by modification time. The others cannot be proven
    // superseded by a destination this pass is about to create, so their
    // markers are stripped rather than the directories deleted: a later
    // sweep then reads them as ordinary foreign debris (the `(false, true)`
    // row) and never deletes them automatically, which is the direction to
    // guess wrong in.
    //
    // The losers are demoted (marker stripped) *before* the winner is
    // restored, and the winner's own marker is cleared only after that
    // restore succeeds, so a crash at any point in between still converges:
    // a crash before the restore leaves every loser already marker-less
    // (row 4, never deleted) and the winner untouched (still marker-with-
    // trash, still not resolving), so the next sweep re-derives the same
    // winner from the same mtimes. A crash after the restore leaves the
    // winner as marker-with-no-trash, which is row 1, cleanly closed by
    // clearing the marker.
    let mut candidates: Vec<(std::time::SystemTime, PathBuf, PathBuf)> = Vec::new();
    for name in names {
        let (trash, marker) = trash_and_marker(staging_parent, name);
        match std::fs::symlink_metadata(&trash) {
            Ok(meta) if is_trash_like(&trash) => candidates.push((
                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                trash,
                marker,
            )),
            // Either nothing is here, or something not trash-shaped is (a
            // plain regular file, never put there by this code): neither is
            // a candidate this code can restore. A marker beside it
            // describes a trash that does not exist, so it is cleared; a
            // foreign file, if any, is left untouched.
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

/// Spawns the game and reports failure only if it has already given up.
///
/// A short wait after spawning is enough to catch an immediate failure
/// (missing data files, a bad binary) without holding the caller for the
/// game's whole lifetime.
///
/// Deliberately does not verify a code signature: both games ship unsigned
/// macOS builds, so a signature check would make every launch fail.
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
            // stderr pipe into a detached thread for the rest of its life:
            // dropping `child` here would close the pipe's read end, and
            // std resets SIGPIPE to its default disposition in the child,
            // so its next write to stderr would kill it. The thread drains
            // stderr to EOF, so the game can never block on a full pipe
            // either, and then waits on the child so it is reaped instead
            // of left a zombie for the rest of the launcher's life.
            let stderr = child.stderr.take();
            std::thread::spawn(move || {
                if let Some(mut pipe) = stderr {
                    // `io::copy` into `io::sink` drains identically to
                    // reading into a buffer, but retains nothing: a buffer
                    // here would grow for the game's entire lifetime,
                    // leaking the launcher's memory by total stderr volume
                    // over a long session.
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
        // No debris left behind.
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

        // A URL that says .zip but serves rubbish.
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

        // The playable build is untouched and still active.
        assert!(s.root.join("OpenRCT2/versions/v1/OpenRCT2.app").exists());
        assert_eq!(store.active(Mode::MultiVersion).unwrap(), Some("v1".into()));
        // The failed install left no staging directory.
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
        // sanitize_tag maps '/' to '-', then strips the resulting leading
        // dot so the name cannot land in this module's reserved namespace.
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

        // Simulate a crash exactly between the two renames of a later
        // install: the marker was written, the destination has already
        // been moved aside under the new tag's trash name, and the new
        // build never arrived.
        let bin = s.root.join("OpenRCT2/bin");
        let trash = s.root.join("OpenRCT2/.trash-v2");
        let marker = s.root.join("OpenRCT2/.trashmark-v2");
        std::fs::write(&marker, b"v2").unwrap();
        std::fs::rename(&bin, &trash).unwrap();
        assert!(!bin.exists());

        // A later install, even of a different tag, must recover the
        // orphaned build rather than leaving it stranded forever.
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

    /// I2: the recovery used to be reachable only from `install`, so after
    /// a crash the user's intact build sat in `.trash-<tag>` while the app
    /// reported nothing installed -- and a user told nothing is installed
    /// has no reason to start the install that would have repaired it.
    /// `controller::start` now runs this at every launch.
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

        // The crash window: marker written, destination already moved
        // aside, the replacement never placed.
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

    /// The same, in the mode where each build has its own destination.
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

    /// Deferred item 5, closed by the same startup pass: `install` clears
    /// only the staging name it is using, so debris under any other tag was
    /// never swept and leaked a whole build's worth of disk.
    #[test]
    fn stale_staging_debris_is_swept_and_work_in_progress_is_not() {
        let s = Scratch::new("stagingsweep");
        let store = s.store();
        let versions = s.root.join("OpenRCT2/versions");
        std::fs::create_dir_all(versions.join(".staging-v9/OpenRCT2.app")).unwrap();
        std::fs::write(versions.join(".download-v9"), b"partial").unwrap();
        std::fs::create_dir_all(versions.join("v1")).unwrap();

        // Nothing is old enough yet, which is the case that protects a
        // second copy of the app mid-install.
        sweep_stale_staging(&store, Mode::MultiVersion, std::time::SystemTime::now());
        assert!(
            versions.join(".staging-v9").is_dir(),
            "a live install must be left alone"
        );
        assert!(versions.join(".download-v9").exists());

        // Judged from far enough in the future that both are debris. The
        // clock is the parameter rather than the files' timestamps so the
        // test never has to backdate anything on disk.
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

    /// The sweep must never take a build the trash table is responsible
    /// for, whatever their relative ages: the staging directory is the one
    /// that is reproducible by downloading again, the trash is the user's.
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

        // Installing a different tag must not leave v1's build stranded.
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

        // The destination really does hold "v2" now, exactly as if this
        // install's own final rename had already succeeded and only its
        // post-rename cleanup of the now-superseded trash was interrupted.
        // The marker records the same tag, so the sweep can confirm the
        // swap that created this trash is what is at the destination now.
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

        // The destination resolves, but to a different tag than the marker
        // claims: `activate`, `reconcile_compatible`, `disable_multi_version`
        // or `adopt_dir` in store.rs could all have repopulated it, not the
        // install that created this trash. The marker proves this code made
        // the trash; it does not prove *this* swap is what is at the
        // destination now.
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
        // Names what this test actually pins: `is_marker_file`'s `is_file`
        // check, not the `.trash-` / `.trashmark-` prefix split. The
        // `.trash-<name>.pending` suffix scheme this crate shipped with
        // briefly was ambiguous (`sanitize_tag` preserves '.', so a release
        // tag ending in `.pending` produced a trash directory a suffix
        // scheme could not tell apart from a marker), but once markers are
        // required to be regular files, a trash -- always a directory or a
        // symlink to one, never a plain file -- can no longer satisfy that
        // check under any name, prefixed or not. The distinct `.trashmark-`
        // prefix is defense in depth on top of that, not a separately
        // reachable fix: with `is_marker_file` in place, the naming
        // collision this test used to reproduce is no longer independently
        // triggerable.
        let s = Scratch::new("pending-collision");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");

        // The destination already resolves to a real, current build.
        let exe_dir = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();

        // A genuine, marker-less trash directory (row 4: never delete)...
        let genuine = game_dir.join(".trash-genuine");
        std::fs::create_dir_all(genuine.join("OpenRCT2.app/Contents/MacOS")).unwrap();

        // ...sitting beside a directory at the exact name a marker for it
        // would use under the old suffix scheme. It is a directory, not a
        // marker file, so `is_marker_file` must reject it regardless of how
        // closely its name resembles a marker's.
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

        // A `.trash-*` directory with no marker was not produced by this
        // code (for example a name `VersionStore::adopt_dir` happened to
        // mint independently). The destination already resolves.
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

        // Exactly the state Critical 1 in review described: a trash+marker
        // pair left by a crashed install, and `bin` now a dangling symlink
        // (as if a separate multi-version reinstall crashed and moved the
        // link's target away).
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

        // The old build must be restored, not deleted, because the symlink
        // never resolved to anything real: `symlink_metadata` alone would
        // have read the occupied name as a superseding build.
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

        // The newer one is restored...
        assert!(bin.join("OpenRCT2.app").exists());
        assert!(!newer.exists());
        // ...and the older one survives untouched, rather than being
        // guessed wrong and deleted (this is Critical 2 from review: two
        // trash directories sharing one destination must never be resolved
        // by deleting one of them).
        assert!(
            older.exists(),
            "an ambiguous second trash must never be deleted"
        );
        assert!(
            !game_dir.join(".trashmark-older").exists(),
            "its marker is stripped so a later sweep reads it as foreign debris, not as provably superseded"
        );

        // A later sweep, now that bin resolves again, must still not delete
        // the demoted trash.
        recover_stray_trash(&store, Mode::Compatible);
        assert!(older.exists(), "must still not be deleted on a later sweep");
    }

    #[test]
    fn a_trash_directory_that_cannot_be_deleted_keeps_its_marker_for_retry() {
        let s = Scratch::new("stuckdelete");
        let store = s.store();
        let game_dir = s.root.join("OpenRCT2");
        let bin = game_dir.join("bin");
        // A real, resolving destination already in place, holding the tag
        // the marker below will confirm, so the sweep actually attempts the
        // delete rather than stopping short at "not confirmed".
        let exe = bin.join("OpenRCT2.app/Contents/MacOS");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join("OpenRCT2"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin.join(".version"), b"stuck").unwrap();

        // A trash+marker pair whose deletion will fail: a read-only
        // subdirectory blocks unlinking its contents, just like a real
        // archive that ships restrictive permissions.
        let trash = game_dir.join(".trash-stuck");
        let locked = trash.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), b"x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::write(game_dir.join(".trashmark-stuck"), b"stuck").unwrap();

        recover_stray_trash(&store, Mode::Compatible);

        // The delete failed, so the marker must survive for the next sweep
        // to retry: losing it would mean this trash is never looked at
        // again (Important 1 in review).
        assert!(trash.exists(), "the undeletable trash is still there");
        assert!(
            game_dir.join(".trashmark-stuck").exists(),
            "its marker must survive so a later sweep retries the delete"
        );

        // Restore permissions so Drop can remove the scratch directory.
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

        // A plain regular file named like a trash entry: foreign debris,
        // never something this code creates (a trash is always a
        // directory, or the symlink that used to occupy `bin`).
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

        // Foreign debris sitting exactly where a reinstall of v1 would need
        // to put its own trash: no marker, so `recover_stray_trash`
        // correctly left it alone (the destination already resolves).
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
        // And the version that would have been replaced is untouched too.
        assert!(s.root.join("OpenRCT2/versions/v1/OpenRCT2.app").exists());
        // The check runs before paying for a download: no progress at all
        // was ever reported.
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
        // Critical 3 in review: `Stdio::piped()` plus dropping `Child` on
        // return closes the pipe's read end, and std resets SIGPIPE to
        // default in the child, so its very next stderr write kills it.
        // OpenRCT2 logs to stderr, so this is the ordinary path, not an
        // edge case.
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

        // The game needs about a second past the 500ms check to finish all
        // ten iterations and touch the marker. If launch's stderr handling
        // killed it via SIGPIPE, as it did before this fix, the marker
        // would never appear.
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
