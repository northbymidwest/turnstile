//! The application's main menu bar.
//!
//! Not optional. Without a main menu, Command-Q does not quit and Command-C
//! does not work in a text field, because both route through standard
//! responder selectors that only exist once a menu is installed. Avalonia
//! supplies this implicitly upstream; AppKit does not.

use std::ffi::CString;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

pub fn install(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let main = NSMenu::new(mtm);

    main.addItem(&submenu(
        mtm,
        "Turnstile",
        &[
            ("About Turnstile", "orderFrontStandardAboutPanel:", ""),
            ("-", "", ""),
            // No target, so this travels the responder chain to the
            // application delegate, which implements it. The menu is built
            // before `Actions` exists, so it cannot point at that directly.
            ("Settings...", "showSettings:", ","),
            ("-", "", ""),
            ("Hide Turnstile", "hide:", "h"),
            ("Quit Turnstile", "terminate:", "q"),
        ],
    ));

    main.addItem(&submenu(
        mtm,
        "Edit",
        &[
            ("Undo", "undo:", "z"),
            ("Redo", "redo:", "Z"),
            ("-", "", ""),
            ("Cut", "cut:", "x"),
            ("Copy", "copy:", "c"),
            ("Paste", "paste:", "v"),
            ("Select All", "selectAll:", "a"),
        ],
    ));

    let window_menu = submenu(
        mtm,
        "Window",
        &[
            ("Minimize", "performMiniaturize:", "m"),
            ("Close", "performClose:", "w"),
        ],
    );
    main.addItem(&window_menu);

    app.setMainMenu(Some(&main));
    if let Some(sub) = window_menu.submenu() {
        app.setWindowsMenu(Some(&sub));
    }
}

/// Builds a top-level menu bar item carrying a submenu of the given entries.
/// `"-"` becomes a separator; every other entry becomes a titled item whose
/// action is resolved by name and sent up the responder chain (no target is
/// set, so whichever object down the chain implements the selector handles
/// it -- which is the whole point of using AppKit's standard selectors here
/// rather than writing our own).
fn submenu(
    mtm: MainThreadMarker,
    title: &str,
    entries: &[(&str, &str, &str)],
) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for (label, selector, key) in entries {
        if *label == "-" {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        let action = if selector.is_empty() {
            None
        } else {
            Some(sel_name(selector))
        };
        // SAFETY: `action`, when present, names a real, argument-taking
        // Objective-C selector (`foo:`), which is exactly what
        // `initWithTitle:action:keyEquivalent:` requires.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                action,
                &NSString::from_str(key),
            )
        };
        menu.addItem(&item);
    }
    let top = NSMenuItem::new(mtm);
    top.setSubmenu(Some(&menu));
    top
}

/// Resolves a selector by name. The dynamic counterpart of `objc2::sel!`,
/// needed here because the selector names above come from a runtime table
/// rather than individual literals.
fn sel_name(name: &str) -> Sel {
    let c_name = CString::new(name).expect("selector name must not contain NUL bytes");
    Sel::register(&c_name)
}
