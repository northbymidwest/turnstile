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

use std::sync::Mutex;

use crate::mainqueue;
use crate::state::Msg;

/// `Mutex<()>`: there is no in-memory data to guard, the resource is the
/// store's directories.
static STORE: Mutex<()> = Mutex::new(());

/// Runs `work` with the store lock held, blocking until it is free.
///
/// **A poisoned lock is recovered from, not propagated.** The payload is `()`
/// and cannot be inconsistent, so there is nothing for a caller to observe
/// half-written. Propagating would instead permanently disable installing,
/// switching, removing and mode changes with no way back but a restart, and
/// the store already repairs debris left by a process that died mid-operation.
pub fn serialized<T>(work: impl FnOnce() -> T) -> T {
    // The guard is bound rather than dropped: `let _ = ...` would release it
    // immediately and silently undo this whole module.
    let _guard = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    work()
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
