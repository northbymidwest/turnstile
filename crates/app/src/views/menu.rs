//! The application's main menu bar.
//!
//! Not optional: without one, Command-Q does not quit and Command-C does not
//! work in a text field, because both route through standard responder
//! selectors that only exist once a menu is installed.

use std::ffi::CString;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

use crate::strings;

pub fn install(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let main = NSMenu::new(mtm);
    let name = "Turnstile";

    main.addItem(&submenu(
        mtm,
        name,
        &[
            (
                strings::format1("MenuAbout", name),
                "orderFrontStandardAboutPanel:",
                "",
            ),
            (SEPARATOR.into(), "", ""),
            // No target, so this travels the responder chain to the
            // application delegate, which implements it.
            (strings::get("MenuSettings"), "showSettings:", ","),
            (SEPARATOR.into(), "", ""),
            (strings::format1("MenuHide", name), "hide:", "h"),
            (strings::format1("MenuQuit", name), "terminate:", "q"),
        ],
    ));

    main.addItem(&submenu(
        mtm,
        &strings::get("MenuEdit"),
        &[
            (strings::get("MenuUndo"), "undo:", "z"),
            (strings::get("MenuRedo"), "redo:", "Z"),
            (SEPARATOR.into(), "", ""),
            (strings::get("MenuCut"), "cut:", "x"),
            (strings::get("MenuCopy"), "copy:", "c"),
            (strings::get("MenuPaste"), "paste:", "v"),
            (strings::get("MenuSelectAll"), "selectAll:", "a"),
        ],
    ));

    let window_menu = submenu(
        mtm,
        &strings::get("MenuWindow"),
        &[
            (strings::get("MenuMinimize"), "performMiniaturize:", "m"),
            (strings::get("MenuClose"), "performClose:", "w"),
        ],
    );
    main.addItem(&window_menu);

    app.setMainMenu(Some(&main));
    if let Some(sub) = window_menu.submenu() {
        app.setWindowsMenu(Some(&sub));
    }
}

const SEPARATOR: &str = "-";

/// Builds a top-level menu bar item carrying a submenu. `SEPARATOR` becomes a
/// separator; every other entry becomes a titled item whose action is resolved
/// by name and sent up the responder chain, so whichever object implements the
/// selector handles it.
fn submenu(
    mtm: MainThreadMarker,
    title: &str,
    entries: &[(String, &str, &str)],
) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for (label, selector, key) in entries {
        if label == SEPARATOR {
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

/// Resolves a selector by name: the dynamic counterpart of `objc2::sel!`,
/// needed because the names above come from a runtime table.
fn sel_name(name: &str) -> Sel {
    let c_name = CString::new(name).expect("selector name must not contain NUL bytes");
    Sel::register(&c_name)
}
