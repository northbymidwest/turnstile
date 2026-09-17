//! Bridges a `Msg` produced off the main thread back onto it.
//!
//! `Controller` (Task 17) is `Rc`-based and confined to the app's main
//! thread, so worker threads cannot capture it: a worker computes a `Msg`,
//! which is `Send`, and posts it here instead. The registered handler is
//! expected to recover the controller from its own `thread_local!`
//! `Weak<Controller>` once it starts running on the main thread; `reduce`
//! stays pure, and this module's job stops at getting the `Msg` there.
//!
//! This module's own soundness is main-thread-only **by construction**,
//! not by convention: `HANDLER` holds an `Rc`, which is not `Send`, so it
//! is only ever reachable through a `thread_local!`. That is correct and
//! sufficient in production, where `register` and every delivery run on
//! the same, single OS thread: the app's real main thread, the only
//! thread `DispatchQueue::main()` ever executes blocks on. There is no
//! cross-thread access to `HANDLER` at all in production. Do not "fix"
//! this into a shared `static` to make it reachable from a test's private
//! queue: see `HANDLER`'s doc comment for why that was tried, why it was
//! unsound, and what tests use instead.

use std::cell::RefCell;
use std::rc::Rc;
#[cfg(test)]
use std::sync::Mutex;

use dispatch2::DispatchQueue;

use crate::state::Msg;

type Handler = Rc<dyn Fn(Msg)>;
#[cfg(test)]
type SpyHook = Box<dyn Fn(&Msg) + Send + Sync>;

thread_local! {
    /// Only ever touched on the main thread. An `Rc` is sound here for
    /// exactly one reason: in production, `register` and every later
    /// `deliver` run on the app's single, real main OS thread, so there is
    /// no cross-thread access to this cell at all.
    ///
    /// That stops being true under `cargo test`, which runs on worker
    /// threads, never the main thread, and this module's tests must
    /// substitute a private serial queue for `DispatchQueue::main()` (see
    /// `target_queue` below) since a block posted to the real main queue
    /// there would never run. A private queue does not pin its blocks to
    /// the thread that submitted them, or to the thread that called
    /// `register`, confirmed by probing `dispatch2` directly: five threads
    /// submitting to one private queue all ran on a sixth. Registering
    /// from *within* a block already running on that queue does not
    /// reliably fix it either. A further probe showed GCD does not
    /// guarantee it keeps reusing the same worker thread for a serial
    /// queue once real time passes between submissions; a delay as small
    /// as 20ms was enough to break it in every one of 30 trials.
    ///
    /// An earlier version of this module "fixed" that by replacing this
    /// `thread_local!` with a `static` behind a `Mutex` and an
    /// `unsafe impl Sync`. That was unsound: the mutex serializes access
    /// to *this cell*, but not to the `Rc`'s own refcount, which lives in
    /// its allocation and is touched by every clone/drop of every handle
    /// to it, including ones a caller might keep outside this module
    /// entirely: an `unsafe impl Sync` has to hold for every possible
    /// safe use of the type, not just the one call shape this module
    /// happens to use today. It also risked dropping the closure, and
    /// anything it captures, on a non-main thread. So: this stays a
    /// `thread_local!`, and this module's tests exercise `post`/`spawn`
    /// through `spy` (below) instead of through `register`.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Registers the handler every posted `Msg` is delivered to. Called once,
/// at startup, from the main thread.
pub fn register(handler: Rc<dyn Fn(Msg)>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

#[cfg(test)]
pub fn clear_handler() {
    HANDLER.with(|h| *h.borrow_mut() = None);
}

/// A test-only observation hook for `deliver`, standing in for
/// `register`/`HANDLER` in tests. Unlike `HANDLER`, this is safe to reach
/// from any thread without any `unsafe`: it never holds an `Rc`, only a
/// `Send + Sync` closure, so a plain `Mutex` is genuinely enough. There is
/// no non-atomic refcount hidden inside it for the mutex to fail to guard.
#[cfg(test)]
static SPY: Mutex<Option<SpyHook>> = Mutex::new(None);

#[cfg(test)]
pub fn spy(hook: impl Fn(&Msg) + Send + Sync + 'static) {
    *SPY.lock().unwrap() = Some(Box::new(hook));
}

#[cfg(test)]
pub fn clear_spy() {
    *SPY.lock().unwrap() = None;
}

fn deliver(msg: Msg) {
    #[cfg(test)]
    if let Some(hook) = SPY.lock().unwrap().as_deref() {
        hook(&msg);
    }

    // Clone the `Rc` out before calling, so a handler that itself posts
    // does not re-enter a live borrow.
    let handler = HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(msg);
    }
}

/// The queue `post` delivers onto.
///
/// `DispatchQueue::main()` in production. Test builds substitute a private
/// serial queue instead: `cargo test` runs every test on a worker thread,
/// never the real main thread, so a block posted to the actual main queue
/// there never runs: there is no `NSApplication` run loop pumping it, and
/// pumping the *test* thread's run loop does not help, since that thread
/// is not the main thread either. `#[ignore]` does not rescue this: a run
/// with `--ignored` still executes on a non-main thread. A private serial
/// queue sidesteps the run loop entirely (GCD services it with its own
/// worker pool) while still giving the one-at-a-time, FIFO ordering the
/// real main queue would.
#[cfg(not(test))]
fn target_queue() -> &'static DispatchQueue {
    DispatchQueue::main()
}

#[cfg(test)]
fn target_queue() -> &'static DispatchQueue {
    static QUEUE: std::sync::OnceLock<dispatch2::DispatchRetained<DispatchQueue>> =
        std::sync::OnceLock::new();
    QUEUE.get_or_init(|| DispatchQueue::new("turnstile.mainqueue.test", None))
}

/// Hands `msg` to the target queue. Safe to call from any thread.
pub fn post(msg: Msg) {
    target_queue().exec_async(move || deliver(msg));
}

/// Runs `work` on a background thread and posts whatever it returns.
pub fn spawn<F>(work: F)
where
    F: FnOnce() -> Msg + Send + 'static,
{
    std::thread::spawn(move || post(work()));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;

    // `SPY` and `target_queue()`'s queue are both process-wide statics,
    // and `cargo test` runs tests on separate threads by default, so
    // without this lock the two tests below can interleave: one's
    // `clear_spy` can land between the other's `spy` and its delivery.
    // Each test holds this for its whole body. This is ordinary,
    // fully-safe test-code synchronization of test-owned state. Nothing
    // like the unsound production `static` this module used to have.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn progress(generation: u64) -> Msg {
        Msg::Progress {
            generation,
            status: turnstile_core::download::Status::Downloading,
            value: None,
        }
    }

    /// Proves the queue itself moves work posted through the real `post`
    /// entry point across threads, in FIFO order, and that `deliver`
    /// reaches whatever is observing it (here, `spy`). This is the part
    /// with real machinery: everything from `post` through `exec_async`
    /// down to `deliver` runs for real.
    #[test]
    fn messages_posted_directly_are_delivered_in_order() {
        let _guard = lock_tests();
        let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        spy(move |msg| {
            if let Msg::Progress { generation, .. } = msg {
                recorder.lock().unwrap().push(*generation);
            }
        });

        for i in 0..5 {
            post(progress(i));
        }
        // `post` enqueues synchronously before returning, so a trailing
        // synchronous submission to the same serial queue is a reliable
        // barrier: it cannot run until every earlier one has.
        target_queue().exec_sync(|| {});

        assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3, 4]);
        clear_spy();
    }

    /// Proves `spawn` runs `work` on a background thread and hands
    /// whatever it returns to `post`, the part of the bridge unique to
    /// `spawn`, as opposed to calling `post` directly above.
    #[test]
    fn spawn_posts_whatever_the_worker_returned() {
        let _guard = lock_tests();
        let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        spy(move |msg| {
            if let Msg::Progress { generation, .. } = msg {
                recorder.lock().unwrap().push(*generation);
            }
        });

        spawn(|| progress(42));

        // `spawn`'s background thread races this one, so there is no
        // single deterministic barrier to wait on (unlike the direct
        // `post` calls above). Poll instead.
        let start = Instant::now();
        while seen.lock().unwrap().is_empty() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(*seen.lock().unwrap(), vec![42]);
        clear_spy();
    }

    /// `HANDLER`'s own lookup is deliberately not exercised here: it is a
    /// `thread_local!`, reachable only from whichever thread called
    /// `register`, and under `cargo test` that is never the thread the
    /// target queue delivers on (see `HANDLER`'s doc comment). That is an
    /// acceptable, documented gap rather than an oversight: the lookup
    /// itself is three lines, its failure mode is "no handler, message
    /// silently dropped," and on the one call site that matters in
    /// production, that failure is immediately and unmissably visible
    /// (the app simply never responds to anything) the first time it
    /// runs. This test only proves the empty-handler path here does not
    /// panic.
    #[test]
    fn posting_without_a_registered_handler_does_not_panic() {
        let _guard = lock_tests();
        clear_handler();
        clear_spy();
        post(Msg::ClickPlay);
        target_queue().exec_sync(|| {});
    }
}
