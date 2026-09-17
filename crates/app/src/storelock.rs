//! One lock that every store operation is run under, so two workers can
//! never be inside the same `bin` at once.
//!
//! This is the third mechanism guarding concurrent store work, underneath
//! two that already exist, and it is deliberately the only one of the three
//! that touches the filesystem question:
//!
//! - `reduce`'s `!is_busy()` guards keep the *interface* coherent: a control
//!   that would start a second operation is refused, so the UI never claims
//!   two things are happening at once. They are advisory for the filesystem,
//!   because `bump_generation` clears `busy` unconditionally -- it has to,
//!   or a reply discarded as stale leaves every control disabled forever --
//!   and that re-opens every guard while the abandoned operation is still
//!   running.
//! - The generation gate keeps *replies* coherent: work from an abandoned
//!   generation is discarded rather than folded into the new state.
//! - This lock keeps the *disk* coherent, and it is the one that does not
//!   depend on either of the other two being right.
//!
//! Neither of the first two is weakened by this existing, and this does not
//! replace them: without the guards the UI would misreport, and without the
//! generation gate a stale reply would corrupt the state.
//!
//! **Why a lock and not another counter.** Every previous attempt to close
//! this class with a flag or a count added a new way to wedge the app,
//! because a clear that was missed on some path left the interface inert
//! with every control disabled. That happened on this branch. A guard has no
//! "must always clear" obligation to miss: it is released on drop, including
//! while a panic unwinds, so the only way to hold this lock forever is to
//! block forever inside the operation itself.
//!
//! **Acquired on the worker, never on the main thread** (with one startup
//! exception, noted in `controller::start`, where no worker exists yet and
//! the acquisition therefore cannot block). The main thread runs the UI and
//! the main-queue dispatch that delivers every reply, so blocking it would
//! stop the replies that are the only thing able to release this lock. A
//! worker blocking is ordinary and expected: it is why `spawn` below exists
//! and why the acquisition is inside the spawned closure rather than around
//! it.
//!
//! **One lock for every game, not one per game.** `Effect::SetMode` is
//! issued once per game from a single `Msg::ToggleMultiVersion`, so a
//! per-game lock would let a mode change for game A run beside an
//! `Activate` for game B -- and, worse, would not serialize the two
//! `SetMode`s against a per-game operation at all. The cost of a single lock
//! is that two operations on unrelated games queue instead of overlapping,
//! which is invisible: they are both already reported as one busy state.

use std::sync::Mutex;

use crate::mainqueue;
use crate::state::Msg;

/// The lock itself. `Mutex<()>`: there is no in-memory data to guard here,
/// the resource is the store's directories.
static STORE: Mutex<()> = Mutex::new(());

/// Runs `work` with the store lock held, blocking until it is free.
///
/// **A poisoned lock is recovered from, not propagated.** A worker that
/// panicked while holding this left the *filesystem* in whatever state it
/// reached, not this mutex's payload -- which is `()` and cannot be
/// inconsistent -- so there is nothing here for a caller to observe
/// half-written. Propagating the poison would instead panic every later
/// operation, which for a lock taken by every store operation in the app
/// means the first panicked worker permanently disables installing,
/// switching, removing and mode changes, with no error and no way back but
/// a restart. The store already has a repair path for debris left by a
/// process that died mid-operation (`install::recover_debris` and
/// `VersionStore::reconcile`, both run at startup), so carrying on is both
/// the recoverable direction and the one the rest of the app is built for.
pub fn serialized<T>(work: impl FnOnce() -> T) -> T {
    // `into_inner` on the poison, not `unwrap`: see above. The guard is
    // bound rather than dropped -- `let _ = ...` would release it
    // immediately and silently undo this whole module.
    let _guard = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    work()
}

/// Runs a store operation on a background thread and posts the `Msg` it
/// produces, exactly as `mainqueue::spawn` does, with the store lock held
/// for the whole of it.
///
/// The lock's scope is the whole closure by construction, which is what
/// makes multi-phase operations safe: `Effect::SwitchThenRemove` activates
/// and *then* deletes, and both phases have to be inside one acquisition or
/// the interleaving this exists to prevent is still reachable between them.
/// Do not acquire inside a phase instead.
///
/// The reply is posted after the guard has dropped: `work` returns the
/// `Msg`, `serialized` returns it, and only then does `mainqueue::spawn`
/// hand it to the queue. So the main thread is never delivered a reply while
/// the worker that produced it still holds the lock.
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

    // `STORE` is a process-wide static and `cargo test` runs tests
    // concurrently, so the two tests below would otherwise contend with
    // each other for the very thing they are measuring. Each holds this
    // for its whole body. Same pattern, and the same reason, as
    // `mainqueue`'s own `TEST_LOCK`.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The property the module exists for: two store operations issued at
    /// once do not overlap.
    ///
    /// The second thread announces itself *before* it asks for the lock, and
    /// the test waits for that announcement, so the negative assertion below
    /// cannot pass merely because the thread had not started yet -- which
    /// would be a confident answer to a different question. Past that point
    /// the only thing between the announcement and the acquisition is the
    /// call itself.
    #[test]
    fn a_second_store_operation_cannot_start_while_the_first_is_running() {
        let _guard = lock_tests();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (asking_tx, asking_rx) = mpsc::channel();
        let (second_tx, second_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            // `move` on both: an `mpsc::Receiver` is `Send` but not `Sync`,
            // so it has to be handed to the thread rather than borrowed by
            // it.
            scope.spawn(move || {
                serialized(|| {
                    entered_tx.send(()).unwrap();
                    // Stands in for a real operation's duration: a
                    // `remove_dir_all` of a 117 MB build, or the `dir_size`
                    // walk that ends a mode change.
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

            // Released *before* the assertion, deliberately. `thread::scope`
            // joins its threads while unwinding, so a failed assertion here
            // with the first thread still parked inside the lock would hang
            // the run rather than fail it -- which is what happened when
            // this was checked by taking the lock back out.
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

    /// Requirement 4: a worker that panics inside the lock must not disable
    /// every later store operation. The panic below is expected, and its
    /// message appears in the test output; the hook is deliberately not
    /// suppressed, since `set_hook` is process-wide and would hide a real
    /// panic from whatever test ran beside this one.
    #[test]
    fn a_panic_inside_the_lock_does_not_disable_later_operations() {
        let _guard = lock_tests();
        let died = std::thread::spawn(|| serialized(|| panic!("a worker died holding the lock")));
        assert!(died.join().is_err(), "the worker was supposed to panic");

        // The lock is poisoned at this point. This is the acquisition that
        // would panic, or hang, if the poison were propagated.
        assert_eq!(serialized(|| 7), 7, "a later operation must still run");
    }
}
