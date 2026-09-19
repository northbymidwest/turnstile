//! One lock that every store operation is run under, so two workers can never
//! be inside the same `bin` at once.
//!
//! This is the third mechanism guarding concurrent store work, and the only
//! one that touches the filesystem question. `reduce`'s `!is_busy()` guards
//! keep the *interface* coherent but are advisory for the disk, since
//! `bump_generation` clears `busy` unconditionally; the generation gate keeps
//! *replies* coherent. Neither is weakened by this, and neither is replaced.
//!
//! A lock rather than another counter: every attempt to close this class with
//! a flag or a count added a new way to wedge the app, because a clear missed
//! on some path left every control disabled. A guard has no "must always
//! clear" obligation to miss.
//!
//! **Acquired on the worker, never on the main thread** (with one startup
//! exception, noted in `controller::start`). Blocking the main thread would
//! stop the replies that are the only thing able to release this lock.
//!
//! **One lock for every game, not one per game.** `Effect::SetMode` is issued
//! once per game from a single `Msg::ToggleMultiVersion`, so a per-game lock
//! would not serialize the two `SetMode`s against a per-game operation at all.
//!
//! **Two locks, taken in this order: the process's, then the machine's.** The
//! mutex keeps this process's own workers apart; an `flock` on a lock file
//! keeps a second copy of Turnstile out of the same directories. Taking the
//! mutex first means only one thread here ever queues on the file.
//!
//! The file lock cannot constrain OpenLauncher, which shares this layout and
//! knows nothing about it. It covers the case that is ours to cover: the same
//! application running twice, where both copies run the same recovery and
//! could otherwise race the two renames an install swaps with.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::Mutex;

use turnstile_core::dirs::Dirs;

use crate::mainqueue;
use crate::state::Msg;

/// `Mutex<()>`: there is no in-memory data to guard, the resource is the
/// store's directories.
static STORE: Mutex<()> = Mutex::new(());

/// Beside Turnstile's own directory rather than inside a game's: the game
/// directories are the layout OpenLauncher shares, and this file is not its
/// business.
fn lock_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = LOCK_PATH.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(path);
    }
    Some(Dirs::system()?.app_support.join("Turnstile/.store.lock"))
}

/// Where the tests point `lock_path`, so they never touch the real one.
#[cfg(test)]
static LOCK_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

fn lock_file() -> Option<File> {
    let path = lock_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    // Never truncated: the file carries no contents, only the lock.
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .ok()
}

/// Excludes a second copy of Turnstile for as long as the returned handle
/// lives. The kernel drops an `flock` when the holder's file closes, so a
/// process that died mid-install cannot leave the store locked against the
/// next run -- which is the whole reason this is a lock file and not a pid
/// file.
///
/// `None` means the lock could not be taken: no home directory, an unwritable
/// directory, or a filesystem without `flock`. That degrades to the in-process
/// lock alone, because refusing to install anything is a worse answer than
/// the protection this instance already had.
fn machine_lock() -> Option<File> {
    let file = lock_file()?;
    file.lock().ok()?;
    Some(file)
}

/// Runs `work` with the store lock held, blocking until it is free.
///
/// **A poisoned lock is recovered from, not propagated.** The payload is `()`
/// and cannot be inconsistent, so there is nothing for a caller to observe
/// half-written. Propagating would instead permanently disable installing,
/// switching, removing and mode changes with no way back but a restart, and
/// the store already repairs debris left by a process that died mid-operation.
pub fn serialized<T>(work: impl FnOnce() -> T) -> T {
    // The guards are bound rather than dropped: `let _ = ...` would release
    // them immediately and silently undo this whole module.
    let _process = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _machine = machine_lock();
    work()
}

/// Like `serialized`, but gives up rather than waiting when another copy of
/// Turnstile holds the store.
///
/// For the startup repair alone, which runs on the main thread: blocking there
/// would leave the window unbuilt for as long as the other copy's install
/// takes. `None` is not a failure to report, because the copy holding the lock
/// runs the same repair on its own way in.
pub fn try_serialized<T>(work: impl FnOnce() -> T) -> Option<T> {
    let _process = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _machine = match lock_file() {
        Some(file) => {
            file.try_lock().ok()?;
            Some(file)
        }
        // Nothing to take, so nothing is holding it either; same degradation
        // as `serialized`.
        None => None,
    };
    Some(work())
}

/// Runs a store operation on a background thread and posts the `Msg` it
/// produces, with the store lock held for the whole of it.
///
/// The lock's scope is the whole closure by construction, which is what makes
/// multi-phase operations safe: `Effect::SwitchThenRemove` activates and
/// *then* deletes, and both phases have to be inside one acquisition. Do not
/// acquire inside a phase instead.
///
/// The reply is posted after the guard has dropped, so the main thread is
/// never delivered a reply while the worker that produced it holds the lock.
pub fn spawn<F>(work: F)
where
    F: FnOnce() -> Msg + Send + 'static,
{
    mainqueue::spawn(move || serialized(work));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use super::*;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The second thread announces itself *before* it asks for the lock, so
    /// the negative assertion below cannot pass merely because the thread had
    /// not started yet.
    #[test]
    fn a_second_store_operation_cannot_start_while_the_first_is_running() {
        let _guard = lock_tests();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (asking_tx, asking_rx) = mpsc::channel();
        let (second_tx, second_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(move || {
                serialized(|| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
            });
            entered_rx.recv().unwrap();

            scope.spawn(move || {
                asking_tx.send(()).unwrap();
                serialized(|| second_tx.send(()).unwrap())
            });
            asking_rx.recv().unwrap();

            let got_in_early = second_rx.recv_timeout(Duration::from_millis(250));

            // Released *before* the assertion: `thread::scope` joins while
            // unwinding, so a failure here with the first thread still parked
            // inside the lock would hang the run rather than fail it.
            release_tx.send(()).unwrap();

            assert!(
                matches!(got_in_early, Err(RecvTimeoutError::Timeout)),
                "the second operation got in while the first was still inside the store"
            );
            second_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("the second operation must run once the first releases");
        });
    }

    /// Points `lock_path` at a file of this test's own, and hands back the
    /// path. Cleared by the next test to call this, which is safe because
    /// every test in this module holds `TEST_LOCK`.
    fn use_a_private_lock_file(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("turnstile-storelock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        *LOCK_PATH.lock().unwrap() = Some(path.clone());
        path
    }

    fn stop_using_a_private_lock_file() {
        *LOCK_PATH.lock().unwrap() = None;
    }

    /// Stands in for a second copy of Turnstile. `flock` is held per open
    /// file, not per process, so a separate handle contends exactly as another
    /// process would -- confirmed by probing `File::try_lock` across two
    /// handles to one path.
    fn another_copy_holds(path: &std::path::Path) -> std::fs::File {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .unwrap();
        file.lock().unwrap();
        file
    }

    #[test]
    fn the_startup_repair_gives_up_rather_than_waiting_for_another_copy() {
        let _guard = lock_tests();
        let path = use_a_private_lock_file("startup");

        let held = another_copy_holds(&path);
        assert!(
            try_serialized(|| ()).is_none(),
            "the startup repair must not run while another copy holds the store"
        );

        drop(held);
        assert!(
            try_serialized(|| 7) == Some(7),
            "and must run once that copy has let go"
        );
        stop_using_a_private_lock_file();
    }

    #[test]
    fn a_store_operation_waits_for_another_copy_to_finish() {
        let _guard = lock_tests();
        let path = use_a_private_lock_file("waits");
        let held = another_copy_holds(&path);

        let (asking_tx, asking_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            asking_tx.send(()).unwrap();
            serialized(|| done_tx.send(()).unwrap());
        });
        asking_rx.recv().unwrap();

        let got_in_early = done_rx.recv_timeout(Duration::from_millis(250));
        // Released before the assertion, so a failure cannot hang the run.
        drop(held);

        assert!(
            matches!(got_in_early, Err(RecvTimeoutError::Timeout)),
            "the operation ran while another copy still held the store"
        );
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the operation must run once the other copy releases");
        worker.join().unwrap();
        stop_using_a_private_lock_file();
    }

    #[test]
    fn work_still_runs_when_no_lock_file_can_be_made() {
        let _guard = lock_tests();
        *LOCK_PATH.lock().unwrap() = Some(std::path::PathBuf::from(
            "/turnstile-nonexistent-root/x.lock",
        ));

        assert_eq!(
            serialized(|| 7),
            7,
            "an unusable lock file must not stop an install"
        );
        assert_eq!(try_serialized(|| 7), Some(7));
        stop_using_a_private_lock_file();
    }

    /// The panic below is expected and its message appears in the test output;
    /// the hook is not suppressed, since `set_hook` is process-wide and would
    /// hide a real panic from whatever test ran beside this one.
    #[test]
    fn a_panic_inside_the_lock_does_not_disable_later_operations() {
        let _guard = lock_tests();
        let died = std::thread::spawn(|| serialized(|| panic!("a worker died holding the lock")));
        assert!(died.join().is_err(), "the worker was supposed to panic");

        assert_eq!(serialized(|| 7), 7, "a later operation must still run");
    }
}
