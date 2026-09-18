//! The application delegate, which exists for one answer.
//!
//! Without a delegate, AppKit's default is that closing the last window leaves
//! the process running. Turnstile has exactly one window and no way to ask for
//! another, so that left it running with nothing on screen and no way back.

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
        /// The Settings menu item carries no target, so it arrives here along
        /// the responder chain: the menu is built before `Actions` exists.
        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&NSObject>) {
            crate::mainqueue::post(Msg::ShowSettings);
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // `set_ivars(())` because this class declares none: it takes the
        // allocation from `Allocated` to `PartialInit`.
        // SAFETY: the signature of `NSObject`'s `init` is correct.
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: the signature of `NSObject`'s `init` is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Installs the delegate and returns it. `NSApplication`'s `delegate` is an
/// unretained property, so the returned value has to outlive the application.
#[must_use]
pub fn install(mtm: MainThreadMarker) -> Retained<AppDelegate> {
    let delegate = AppDelegate::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    delegate
}
