//! The detail pane: the selected game's header, its installed and available
//! versions, download progress, and the three preference checkboxes.
//!
//! This module only builds the controls and wires their target/action to
//! `Actions`' selectors -- it holds no state and decides nothing about what
//! any control shows. That is `apply`'s job, in `mod.rs`.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSFontWeightSemibold, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultLow, NSLineBreakMode, NSPopUpButton, NSProgressIndicator,
    NSProgressIndicatorStyle, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
    NSViewController,
};
use objc2_foundation::{NSEdgeInsets, NSString, ns_string};

use crate::actions::Actions;
use crate::strings;

/// The controls `apply` needs a handle on, in the shape `Views` wants them
/// destructured into. This type exists only to get all eleven out of `build`
/// in one call; nothing keeps it around afterward.
pub struct Detail {
    pub installed_popup: Retained<NSPopUpButton>,
    pub play_button: Retained<NSButton>,
    pub remove_button: Retained<NSButton>,
    pub releases_popup: Retained<NSPopUpButton>,
    pub download_button: Retained<NSButton>,
    pub progress: Retained<NSProgressIndicator>,
    pub develop_check: Retained<NSButton>,
    pub auto_update_check: Retained<NSButton>,
    pub multi_version_check: Retained<NSButton>,
    pub title_label: Retained<NSTextField>,
    pub version_label: Retained<NSTextField>,
}

/// Builds the detail pane: a vertical `NSStackView` of horizontal rows, each
/// created inline below because none of them is reused. Returns the view
/// controller `NSSplitViewItem::splitViewItemWithViewController` needs, and
/// every control `apply` writes to.
pub fn build(mtm: MainThreadMarker, actions: &Actions) -> (Retained<NSViewController>, Detail) {
    // SAFETY: `actions` is retained by `Views` for the app's lifetime, which
    // outlives every control built below.
    let target: &AnyObject = actions;

    let root = NSStackView::new(mtm);
    root.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    root.setEdgeInsets(NSEdgeInsets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });
    root.setSpacing(12.0);

    // Row 1: header -- game name, a spacer, then the launcher version.
    let title_label = label(mtm, "");
    // SAFETY: reading this extern weight constant is always sound; it is an
    // immutable value AppKit provides, not one that can be misused.
    let semibold = unsafe { NSFontWeightSemibold };
    title_label.setFont(Some(&NSFont::systemFontOfSize_weight(24.0, semibold)));

    let spacer = NSView::new(mtm);
    // An empty view has no intrinsic size to hug, but its hugging priority
    // still defaults to the same value as the labels either side of it;
    // lowering it explicitly is what makes it the one that stretches.
    spacer.setContentHuggingPriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );

    let version_label = label(mtm, &format!("v{}", env!("CARGO_PKG_VERSION")));
    secondary(&version_label);

    let header_row = row(mtm);
    header_row.addArrangedSubview(&title_label);
    header_row.addArrangedSubview(&spacer);
    header_row.addArrangedSubview(&version_label);
    root.addArrangedSubview(&header_row);

    // Row 2-3: installed versions.
    let installed_label = small_secondary_label(mtm, &strings::get("InstalledLabel"));
    root.addArrangedSubview(&installed_label);

    let installed_popup = NSPopUpButton::new(mtm);
    // SAFETY: `target` outlives `installed_popup`, and `installedChanged:`
    // is a real selector `Actions` defines below.
    unsafe {
        installed_popup.setTarget(Some(target));
        installed_popup.setAction(Some(sel!(installedChanged:)));
    }

    // SAFETY: `target` outlives `play_button`, and `playClicked:` is a real
    // selector `Actions` already defines.
    let play_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(&strings::get("Play")),
            Some(target),
            Some(sel!(playClicked:)),
            mtm,
        )
    };
    // The window's default button: Return activates it regardless of which
    // control has focus.
    play_button.setKeyEquivalent(ns_string!("\r"));

    // SAFETY: `target` outlives `remove_button`, and `removeClicked:` is a
    // real selector `Actions` already defines.
    let remove_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(&strings::get("Remove")),
            Some(target),
            Some(sel!(removeClicked:)),
            mtm,
        )
    };

    let installed_row = row(mtm);
    installed_row.addArrangedSubview(&installed_popup);
    installed_row.addArrangedSubview(&play_button);
    installed_row.addArrangedSubview(&remove_button);
    root.addArrangedSubview(&installed_row);

    // Row 4-6: available releases, plus the progress bar beneath them.
    let available_label = small_secondary_label(mtm, &strings::get("AvailableLabel"));
    root.addArrangedSubview(&available_label);

    let releases_popup = NSPopUpButton::new(mtm);
    // SAFETY: `target` outlives `releases_popup`, and `buildChanged:` is a
    // real selector `Actions` defines below.
    unsafe {
        releases_popup.setTarget(Some(target));
        releases_popup.setAction(Some(sel!(buildChanged:)));
    }

    // SAFETY: `target` outlives `download_button`, and `downloadClicked:`
    // is a real selector `Actions` already defines.
    let download_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(&strings::get("Download")),
            Some(target),
            Some(sel!(downloadClicked:)),
            mtm,
        )
    };

    let releases_row = row(mtm);
    releases_row.addArrangedSubview(&releases_popup);
    releases_row.addArrangedSubview(&download_button);
    root.addArrangedSubview(&releases_row);

    let progress = NSProgressIndicator::new(mtm);
    progress.setStyle(NSProgressIndicatorStyle::Bar);
    // Set explicitly because `NSProgressIndicator`'s defaults are 0.0 and
    // **100.0**, not 0.0 and 1.0, while every value written to it is the
    // 0.0-to-1.0 fraction `download.rs` reports. Left at the default the
    // bar fills at most one percent of its width over a whole download,
    // which is indistinguishable from a bar that never moves at all -- the
    // symptom Task 22's smoke test recorded as 76 byte-identical frames.
    // Measured, not assumed: a bare `[[NSProgressIndicator alloc] init]`
    // reports `maxValue == 100`, and its accessibility value is
    // `doubleValue / maxValue`, which is why the fractions the smoke test
    // read back off the control were a hundredth of the ones being written.
    progress.setMinValue(0.0);
    progress.setMaxValue(1.0);
    progress.setHidden(true);
    root.addArrangedSubview(&progress);

    // Row 7: the three preference checkboxes.
    // SAFETY: `target` outlives `develop_check`, and `developToggled:` is a
    // real selector `Actions` defines below.
    let develop_check = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(&strings::get("ShowDevelopmentVersions")),
            Some(target),
            Some(sel!(developToggled:)),
            mtm,
        )
    };
    root.addArrangedSubview(&develop_check);

    // SAFETY: `target` outlives `auto_update_check`, and
    // `autoUpdateToggled:` is a real selector `Actions` defines below.
    let auto_update_check = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(&strings::get("AutoUpdateGame")),
            Some(target),
            Some(sel!(autoUpdateToggled:)),
            mtm,
        )
    };
    root.addArrangedSubview(&auto_update_check);

    // SAFETY: `target` outlives `multi_version_check`, and
    // `multiVersionToggled:` is a real selector `Actions` defines below.
    let multi_version_check = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(&strings::get("KeepMultipleVersions")),
            Some(target),
            Some(sel!(multiVersionToggled:)),
            mtm,
        )
    };
    // "Changes how builds are stored on disk" is not guessable from the
    // checkbox's own label, so it gets a line of its own underneath.
    let multi_version_detail =
        small_secondary_label(mtm, &strings::get("KeepMultipleVersionsDetail"));
    // `maximumNumberOfLines(0)` alone does not wrap: `labelWithString`'s
    // default line-break mode is truncating, which just lets a single line
    // run past the window's edge instead. Word-wrapping is what actually
    // turns that into multiple lines within `preferredMaxLayoutWidth`.
    multi_version_detail.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    multi_version_detail.setMaximumNumberOfLines(0);
    multi_version_detail.setPreferredMaxLayoutWidth(420.0);

    let multi_version_column = NSStackView::new(mtm);
    multi_version_column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    multi_version_column.setSpacing(2.0);
    multi_version_column.addArrangedSubview(&multi_version_check);
    multi_version_column.addArrangedSubview(&multi_version_detail);
    root.addArrangedSubview(&multi_version_column);

    let controller = NSViewController::new(mtm);
    controller.setView(&root);

    (
        controller,
        Detail {
            installed_popup,
            play_button,
            remove_button,
            releases_popup,
            download_button,
            progress,
            develop_check,
            auto_update_check,
            multi_version_check,
            title_label,
            version_label,
        },
    )
}

/// A horizontal row for the stack view above -- just a container, so its
/// only settings are orientation and inter-item spacing.
fn row(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setSpacing(8.0);
    stack
}

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&NSString::from_str(text), mtm)
}

fn secondary(field: &NSTextField) {
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
}

/// `InstalledLabel` and `AvailableLabel` are both this: a small caption
/// above the row it introduces, styled to read as secondary rather than
/// as a field label demanding equal weight with the controls below it.
fn small_secondary_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let field = label(mtm, text);
    field.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    secondary(&field);
    field
}
