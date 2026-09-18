//! The two open panels: one for an installer, one for a directory.
//!
//! Both run modally and return what was chosen, or nothing when the panel was
//! cancelled. Cancelling is the ordinary way out of a panel, so it is not an
//! error and produces no message beyond the one saying so.

use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSModalResponse, NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{NSArray, NSString};

use crate::strings;

/// Asks for a GOG installer.
///
/// The panel allows any file rather than only `.exe`. What makes a file
/// readable here is what is inside it, which is checked when it is read, and
/// a filter that hid the file somebody is looking at would be worse than one
/// that lets them pick the wrong thing and be told.
pub fn choose_installer(mtm: MainThreadMarker) -> Option<PathBuf> {
    let panel = open_panel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setMessage(Some(&NSString::from_str(&strings::get("ChooseInstaller"))));
    panel.setPrompt(Some(&NSString::from_str(&strings::get("Install"))));

    run(&panel, mtm)
}

/// Asks for the directory game data should be unpacked into.
pub fn choose_directory(mtm: MainThreadMarker) -> Option<PathBuf> {
    let panel = open_panel(mtm);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    panel.setMessage(Some(&NSString::from_str(&strings::get(
        "ChooseInstallDirectory",
    ))));
    panel.setPrompt(Some(&NSString::from_str(&strings::get("Choose"))));

    run(&panel, mtm)
}

fn open_panel(mtm: MainThreadMarker) -> objc2::rc::Retained<NSOpenPanel> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(false);
    panel
}

/// Runs the panel and returns the single chosen path.
fn run(panel: &NSOpenPanel, mtm: MainThreadMarker) -> Option<PathBuf> {
    // Without this the panel can open behind the window when Turnstile is not
    // the frontmost application, which looks like nothing happened.
    NSApplication::sharedApplication(mtm).activate();

    let response: NSModalResponse = panel.runModal();
    if response != NSModalResponseOK {
        return None;
    }

    let urls: objc2::rc::Retained<NSArray<objc2_foundation::NSURL>> = panel.URLs();
    let url = urls.firstObject()?;
    url.path().map(|path| PathBuf::from(path.to_string()))
}
