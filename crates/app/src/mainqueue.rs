//! Bridges a `Msg` produced off the main thread back onto it.
//!
//! `Controller` is `Rc`-based and confined to the app's main thread, so worker
//! threads cannot capture it: a worker computes a `Msg`, which is `Send`, and
//! posts it here instead.
//!
//! Soundness is main-thread-only **by construction**: `HANDLER` holds an `Rc`,
//! which is not `Send`, so it is only ever reachable through a
//! `thread_local!`. Do not "fix" this into a shared `static` to make it
//! reachable from a test's private queue -- see `HANDLER`'s doc comment.

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
    /// Only ever touched on the main thread. An `Rc` is sound here because in
    /// production `register` and every later `deliver` run on the app's single
    /// real main OS thread.
    ///
    /// That stops being true under `cargo test`, which never runs on the main
    /// thread, so the tests substitute a private serial queue (see
    /// `target_queue`) -- and a private queue does not pin its blocks to the
    /// thread that called `register`, confirmed by probing `dispatch2`.
    ///
    /// An earlier version "fixed" that with a `static` behind a `Mutex` and an
    /// `unsafe impl Sync`. That was unsound: the mutex serializes access to
    /// this cell, but not to the `Rc`'s own refcount, which every clone and
    /// drop of every handle touches, including ones kept outside this module.
    /// It also risked dropping the closure on a non-main thread. So this stays
    /// a `thread_local!`, and the tests go through `spy` instead.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Registers the handler every posted `Msg` is delivered to. Called once, at
/// startup, from the main thread.
pub fn register(handler: Rc<dyn Fn(Msg)>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

#[cfg(test)]
pub fn clear_handler() {
    HANDLER.with(|h| *h.borrow_mut() = None);
}

/// A test-only observation hook for `deliver`, standing in for `register`.
/// Unlike `HANDLER` it holds only a `Send + Sync` closure, with no non-atomic
/// refcount hidden inside, so a plain `Mutex` is genuinely enough.
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

    // Clone the `Rc` out before calling, so a handler that itself posts does
    // not re-enter a live borrow.
    let handler = HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(msg);
    }
}

/// The queue `post` delivers onto: `DispatchQueue::main()` in production.
///
/// Test builds substitute a private serial queue, because `cargo test` runs on
/// a worker thread and a block posted to the real main queue there never runs:
/// there is no `NSApplication` run loop pumping it. `#[ignore]` does not
/// rescue this either. A private serial queue sidesteps the run loop entirely
/// while still giving the one-at-a-time, FIFO ordering the main queue would.
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

    /// Everything from `post` through `exec_async` down to `deliver` runs for
    /// real here.
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
        // `post` enqueues synchronously, so a trailing synchronous
        // submission to the same serial queue is a reliable barrier.
        target_queue().exec_sync(|| {});

        assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3, 4]);
        clear_spy();
    }

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

        // `spawn`'s thread races this one, so there is no deterministic
        // barrier to wait on. Poll instead.
        let start = Instant::now();
        while seen.lock().unwrap().is_empty() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(*seen.lock().unwrap(), vec![42]);
        clear_spy();
    }

    /// `HANDLER`'s own lookup is deliberately not exercised: it is a
    /// `thread_local!` reachable only from whichever thread called `register`,
    /// which under `cargo test` is never the thread the queue delivers on. Its
    /// failure mode in production is unmissable -- the app never responds to
    /// anything -- so this only proves the empty-handler path does not panic.
    #[test]
    fn posting_without_a_registered_handler_does_not_panic() {
        let _guard = lock_tests();
        clear_handler();
        clear_spy();
        post(Msg::ClickPlay);
        target_queue().exec_sync(|| {});
    }
}
