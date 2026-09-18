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
    NSButton, NSColor, NSFont, NSFontWeightSemibold, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSLayoutPriorityDefaultLow, NSLineBreakMode, NSPopUpButton,
    NSProgressIndicator, NSProgressIndicatorStyle, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController,
};
use objc2_foundation::{NSEdgeInsets, NSInteger, NSString, ns_string};
use turnstile_core::gamedata::OriginalGame;

use crate::actions::Actions;
use crate::strings;

/// The controls `apply` needs a handle on. This type exists only to get them
/// all out of `build` in one call.
pub struct Detail {
    pub installed_popup: Retained<NSPopUpButton>,
    pub play_button: Retained<NSButton>,
    pub remove_button: Retained<NSButton>,
    pub releases_popup: Retained<NSPopUpButton>,
    pub download_button: Retained<NSButton>,
    pub progress: Retained<NSProgressIndicator>,
    pub develop_check: Retained<NSButton>,
    pub auto_update_check: Retained<NSButton>,
    pub title_label: Retained<NSTextField>,
    pub version_label: Retained<NSTextField>,
    /// One row per original game, in `OriginalGame::ALL` order. All three are
    /// built once and hidden or shown, because rebuilding a row is how a
    /// button loses its target.
    pub game_data: Vec<GameDataRow>,
}

/// The state of one original game's data, and the button that changes it.
pub struct GameDataRow {
    pub game: OriginalGame,
    /// Hidden when the row is about a game other than the selected one.
    pub container: Retained<NSStackView>,
    pub path: Retained<NSTextField>,
    pub button: Retained<NSButton>,
}

/// Builds the detail pane: a vertical `NSStackView` of horizontal rows.
/// Returns the view controller
/// `NSSplitViewItem::splitViewItemWithViewController` needs, and every control
/// `apply` writes to.
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

    let title_label = label(mtm, "");
    // SAFETY: reading this extern weight constant is always sound.
    let semibold = unsafe { NSFontWeightSemibold };
    title_label.setFont(Some(&NSFont::systemFontOfSize_weight(24.0, semibold)));

    let spacer = NSView::new(mtm);
    // An empty view has no intrinsic size to hug, but its hugging priority
    // still defaults to the labels' value; lowering it is what makes it the
    // one that stretches.
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
    // The window's default button: Return activates it whatever has focus.
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
    // **100.0**, while every value written to it is the 0.0-to-1.0 fraction
    // `download.rs` reports. Left at the default, the bar fills at most one
    // percent of its width over a whole download.
    progress.setMinValue(0.0);
    progress.setMaxValue(1.0);
    progress.setHidden(true);
    root.addArrangedSubview(&progress);

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

    root.addArrangedSubview(&label(mtm, &strings::get("GameData")));

    let game_data: Vec<GameDataRow> = OriginalGame::ALL
        .into_iter()
        .enumerate()
        .map(|(index, game)| {
            let title = label(mtm, title_of(game));
            let path = small_secondary_label(mtm, "");
            path.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);

            // SAFETY: `target` outlives the button, and
            // `installGameDataClicked:` is a real selector `Actions` defines.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&strings::get("InstallGameData")),
                    Some(target),
                    Some(sel!(installGameDataClicked:)),
                    mtm,
                )
            };
            // Which game the button means, from the same list it was built
            // from rather than written out again.
            button.setTag(NSInteger::try_from(index).unwrap_or(0));

            let text = NSStackView::new(mtm);
            text.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            text.setSpacing(0.0);
            text.setAlignment(NSLayoutAttribute::Leading);
            text.addArrangedSubview(&title);
            text.addArrangedSubview(&path);

            let container = row(mtm);
            container.addArrangedSubview(&text);
            container.addArrangedSubview(&button);
            root.addArrangedSubview(&container);

            GameDataRow {
                game,
                container,
                path,
                button,
            }
        })
        .collect();

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
            title_label,
            version_label,
            game_data,
        },
    )
}

/// A horizontal row for the stack view above.
fn row(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setSpacing(8.0);
    stack
}

pub fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&NSString::from_str(text), mtm)
}

fn secondary(field: &NSTextField) {
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
}

/// A small caption above the row it introduces, styled to read as secondary
/// rather than as a field label demanding equal weight with the controls.
pub(crate) fn small_secondary_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let field = label(mtm, text);
    field.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    secondary(&field);
    field
}

/// The name of an original game. Not in the string table: a translation of
/// "RollerCoaster Tycoon 2" would be a mistranslation.
const fn title_of(game: OriginalGame) -> &'static str {
    match game {
        OriginalGame::RollerCoasterTycoon1 => "RollerCoaster Tycoon",
        OriginalGame::RollerCoasterTycoon2 => "RollerCoaster Tycoon 2",
        OriginalGame::Locomotion => "Chris Sawyer's Locomotion",
    }
}
