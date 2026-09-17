//! The application delegate, which exists for one answer.
//!
//! Without a delegate, AppKit's default is that closing the last window
//! leaves the process running. For a document-based application that is
//! right: the Dock icon reopens a window. Turnstile has exactly one window
//! and no way to ask for another, so the default left it running with
//! nothing on screen and no way back, and the only way out was Force Quit
//! or Command-Q while it still had focus.
//!
//! `applicationShouldTerminateAfterLastWindowClosed:` returning true is the
//! whole fix. Closing the window now quits, which is what a single-window
//! utility should do.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol};

use crate::state::Msg;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnstileAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }
    }

    impl AppDelegate {
        /// The Settings menu item, which carries no target and so arrives
        /// here along the responder chain. The menu is built before
        /// `Actions` exists, so it cannot point at that object directly;
        /// posting the same message reaches the same controller by the same
        /// path either way.
        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&NSObject>) {
            crate::mainqueue::post(Msg::ShowSettings);
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // `set_ivars(())` because this class declares none: it takes the
        // allocation from `Allocated` to `PartialInit`, which is what `init`
        // can be sent through.
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: the signature of `NSObject`'s `init` is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Installs the delegate and returns it.
///
/// `NSApplication`'s `delegate` is an unretained property, so the returned
/// value has to outlive the application. `main` keeps it for the process's
/// lifetime; dropping it would leave AppKit messaging freed memory.
#[must_use]
pub fn install(mtm: MainThreadMarker) -> Retained<AppDelegate> {
    let delegate = AppDelegate::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    delegate
}
