mod actions;
mod appdelegate;
mod controller;
mod mainqueue;
mod prefs;
mod state;
mod storelock;
mod strings;
mod views;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

fn main() {
    let mtm = MainThreadMarker::new().expect("main() must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Held for the process's lifetime: NSApplication's delegate property is
    // unretained, so dropping this would leave AppKit messaging freed memory.
    let _delegate = appdelegate::install(mtm);

    views::menu::install(mtm);

    // The controller owns the views and drives them from state for the rest
    // of the process's life; see controller::start.
    controller::start(mtm);

    app.activate();
    app.run();
}
