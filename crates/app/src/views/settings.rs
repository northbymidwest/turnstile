//! The Settings window, reached from the menu bar with Command-comma.
//!
//! It holds the settings that are about the application. The two that stay in
//! the main window are there on purpose: "automatically install updates" is
//! stored per game, and "show development versions" changes what the list in
//! front of you displays.
//!
//! One window, created once and reused. `NSWindow` released when closed would
//! leave the controller holding a freed pointer, so this hides it instead.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSLayoutAttribute, NSLineBreakMode, NSStackView,
    NSTextField, NSUserInterfaceLayoutOrientation, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize, NSString};

use crate::actions::Actions;
use crate::strings;
use crate::views::detail::{label, small_secondary_label};

pub struct Settings {
    pub window: Retained<NSWindow>,
    pub multi_version_check: Retained<NSButton>,
    pub check_updates_check: Retained<NSButton>,
    /// Shows the chosen directory, or the word for "default". Not a text
    /// field: a path somebody typed would have to be checked, created and
    /// explained, and the panel does all three.
    pub install_root_label: Retained<NSTextField>,
}

pub fn build(mtm: MainThreadMarker, actions: &Actions) -> Settings {
    let target = &**actions;

    // SAFETY: `target` outlives the button, and `multiVersionToggled:` is a
    // real selector `Actions` defines.
    let multi_version_check = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(&strings::get("KeepMultipleVersions")),
            Some(target),
            Some(sel!(multiVersionToggled:)),
            mtm,
        )
    };
    let multi_version_detail =
        small_secondary_label(mtm, &strings::get("KeepMultipleVersionsDetail"));
    multi_version_detail.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    multi_version_detail.setMaximumNumberOfLines(0);
    multi_version_detail.setPreferredMaxLayoutWidth(360.0);

    // SAFETY: as above, for `checkForUpdatesToggled:`.
    let check_updates_check = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(&strings::get("CheckForUpdates")),
            Some(target),
            Some(sel!(checkForUpdatesToggled:)),
            mtm,
        )
    };
    let check_updates_detail = small_secondary_label(mtm, &strings::get("CheckForUpdatesDetail"));
    check_updates_detail.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    check_updates_detail.setMaximumNumberOfLines(0);
    check_updates_detail.setPreferredMaxLayoutWidth(360.0);

    // SAFETY: as above, for `chooseInstallRootClicked:`.
    let choose_root = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(&strings::get("ChangeGameData")),
            Some(target),
            Some(sel!(chooseInstallRootClicked:)),
            mtm,
        )
    };
    // SAFETY: as above, for `useDefaultInstallRootClicked:`.
    let use_default = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(&strings::get("UseDefault")),
            Some(target),
            Some(sel!(useDefaultInstallRootClicked:)),
            mtm,
        )
    };

    let install_root_title = label(mtm, &strings::get("InstallDirectory"));
    let install_root_label = small_secondary_label(mtm, &strings::get("DefaultDirectory"));
    install_root_label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    install_root_label.setPreferredMaxLayoutWidth(360.0);

    let install_root_detail = small_secondary_label(mtm, &strings::get("InstallDirectoryDetail"));
    install_root_detail.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    install_root_detail.setMaximumNumberOfLines(0);
    install_root_detail.setPreferredMaxLayoutWidth(360.0);

    let root = NSStackView::new(mtm);
    root.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    root.setSpacing(14.0);
    root.setAlignment(NSLayoutAttribute::Leading);
    root.setEdgeInsets(NSEdgeInsets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });

    let versions_column = NSStackView::new(mtm);
    versions_column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    versions_column.setSpacing(2.0);
    versions_column.setAlignment(NSLayoutAttribute::Leading);
    versions_column.addArrangedSubview(&multi_version_check);
    versions_column.addArrangedSubview(&multi_version_detail);
    root.addArrangedSubview(&versions_column);

    let updates_column = NSStackView::new(mtm);
    updates_column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    updates_column.setSpacing(2.0);
    updates_column.setAlignment(NSLayoutAttribute::Leading);
    updates_column.addArrangedSubview(&check_updates_check);
    updates_column.addArrangedSubview(&check_updates_detail);
    root.addArrangedSubview(&updates_column);

    let buttons = NSStackView::new(mtm);
    buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    buttons.setSpacing(8.0);
    buttons.addArrangedSubview(&choose_root);
    buttons.addArrangedSubview(&use_default);

    let directory_column = NSStackView::new(mtm);
    directory_column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    directory_column.setSpacing(2.0);
    directory_column.setAlignment(NSLayoutAttribute::Leading);
    directory_column.addArrangedSubview(&install_root_title);
    directory_column.addArrangedSubview(&install_root_label);
    directory_column.addArrangedSubview(&install_root_detail);
    directory_column.addArrangedSubview(&buttons);
    root.addArrangedSubview(&directory_column);

    // No Resizable and no Miniaturizable: a settings sheet of two checkboxes
    // has one correct size, and a minimised settings window is a way to lose it.
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 330.0));
    // SAFETY: the designated initialiser for a window created from code.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(&NSString::from_str(&strings::get("Settings")));
    window.setContentView(Some(&root));
    // SAFETY: the window must outlive its own close, because `Views` holds it
    // for the life of the process and shows it again on the next
    // Command-comma. Released on close, that second show would message freed
    // memory.
    unsafe { window.setReleasedWhenClosed(false) };
    window.center();

    Settings {
        window,
        multi_version_check,
        check_updates_check,
        install_root_label,
    }
}

/// Brings the window to the front, creating nothing: there is one.
pub fn show(settings: &Settings, mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    settings.window.makeKeyAndOrderFront(None);
}
