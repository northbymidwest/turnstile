//! The view layer: a window with a translucent two-row sidebar beside the
//! detail pane, plus the app's main menu. `apply` is the only place that
//! reads `UiState` and pushes it into these controls.

mod detail;
pub mod picker;
pub mod settings;
pub(crate) mod sidebar;
pub mod updatebanner;
mod window;

pub mod alert;
pub mod menu;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSButton, NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton, NSProgressIndicator,
    NSStackView, NSTableView, NSTextField, NSUserInterfaceLayoutOrientation, NSViewController,
    NSWindow,
};
use objc2_foundation::NSString;

use crate::actions::{self, Actions};
use crate::state::UiState;
use crate::strings;
use crate::views::alert::AlertView;

/// Everything the rest of the app needs a handle on. Two fields are held
/// rather than read: `window`, because dropping it would tear down a live
/// control tree, and `version_label`, because it is set once at build time
/// and never changes with `UiState`. `actions` is both: `apply` calls
/// `select_sidebar_row` on it, and it has to stay alive regardless, since
/// `NSTableView`'s data source/delegate and every control's target are
/// unretained properties pointing at it.
pub struct Views {
    #[allow(dead_code)]
    pub window: Retained<NSWindow>,
    pub sidebar_table: Retained<NSTableView>,
    pub actions: Retained<Actions>,
    pub installed_popup: Retained<NSPopUpButton>,
    pub play_button: Retained<NSButton>,
    pub remove_button: Retained<NSButton>,
    pub releases_popup: Retained<NSPopUpButton>,
    pub download_button: Retained<NSButton>,
    pub progress: Retained<NSProgressIndicator>,
    pub develop_check: Retained<NSButton>,
    pub game_data: Vec<detail::GameDataRow>,
    pub auto_update_check: Retained<NSButton>,
    pub settings: settings::Settings,
    pub title_label: Retained<NSTextField>,
    #[allow(dead_code)]
    pub version_label: Retained<NSTextField>,
    pub alert: AlertView,
    pub update_banner: updatebanner::UpdateBanner,
}

/// Builds every view: the window, the split view joining the sidebar to the
/// detail pane, and `Actions`, wired in as the target/action or
/// data-source/delegate of every control so they show something and respond
/// to input.
pub fn build(mtm: MainThreadMarker) -> Views {
    let (sidebar_controller, sidebar_table) = sidebar::build(mtm);
    let actions = actions::install(mtm, &sidebar_table);

    let (detail_controller, detail) = detail::build(mtm, &actions);
    let settings = settings::build(mtm, &actions);

    let alert = AlertView::new(mtm);
    let update_banner = updatebanner::UpdateBanner::new(mtm, &actions);

    // The banner sits above the detail pane rather than inside it, so
    // neither `detail.rs` nor `window.rs` needs to know it exists: this
    // wraps detail's already-built view controller in a plain vertical
    // stack with the (hidden) banner as its first row, and hands that
    // wrapper to the split view in its place.
    let content = NSStackView::new(mtm);
    content.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    content.setSpacing(0.0);
    content.addArrangedSubview(&update_banner.view);
    content.addArrangedSubview(&alert.view);
    content.addArrangedSubview(&detail_controller.view());

    let content_controller = NSViewController::new(mtm);
    content_controller.setView(&content);

    let window = window::build(mtm, &sidebar_controller, &content_controller);

    Views {
        window,
        settings,
        sidebar_table,
        actions,
        installed_popup: detail.installed_popup,
        play_button: detail.play_button,
        remove_button: detail.remove_button,
        releases_popup: detail.releases_popup,
        download_button: detail.download_button,
        progress: detail.progress,
        develop_check: detail.develop_check,
        game_data: detail.game_data,
        auto_update_check: detail.auto_update_check,
        title_label: detail.title_label,
        version_label: detail.version_label,
        alert,
        update_banner,
    }
}

/// Pushes the whole state into the controls. Called after every reduce.
///
/// This is deliberately not incremental: at this control count, re-setting
/// everything is free, and it means there is exactly one place where a
/// control's appearance is decided, rather than the scattered assignments
/// and `_isBusy` bookkeeping upstream needs.
pub fn apply(state: &UiState, views: &Views) {
    views
        .title_label
        .setStringValue(&NSString::from_str(state.selected_game.display_name()));

    // Keeps the sidebar's own selection in step with `state.selected_game`.
    // A click already leaves it there on its own, but restoring the
    // last-selected game from preferences at startup never goes through a
    // click, so without this the sidebar would show the wrong row
    // highlighted the moment anything but OpenRCT2 was persisted.
    //
    // Through `Actions` rather than straight at the table: the sidebar's
    // delegate now answers `tableViewSelectionDidChange:`, which AppKit
    // sends for a programmatic selection too, so a bare `selectRowIndexes`
    // here would post a `SelectGame` out of the middle of a render and
    // never stop. `select_sidebar_row` suppresses that for the one call.
    if let Some(row) = sidebar::row_for_game(state.selected_game) {
        views.actions.select_sidebar_row(&views.sidebar_table, row);
    }

    // Installed versions.
    //
    // Which entry is "active" is resolved once, through `active_entry`, the
    // one place that knows `VersionStore::active` does not report the same
    // kind of value in both modes: `Installed::name` in multi-version mode,
    // but `Installed::tag` in compatible mode, where the sole entry's
    // `name` is always the literal string "bin". Comparing against the
    // wrong one of the two would either mislabel every compatible-mode
    // install as inactive, or (given a collision) mislabel a multi-version
    // entry that merely shares a display tag with the active one.
    let active_entry = state.active_entry();
    views.installed_popup.removeAllItems();
    for entry in &state.installed {
        let label = if active_entry.map(|e| e.name.as_str()) == Some(entry.name.as_str()) {
            strings::format1("ActiveVersion", &entry.tag)
        } else {
            entry.tag.clone()
        };
        views
            .installed_popup
            .addItemWithTitle(&NSString::from_str(&label));
    }
    if !state.installed.is_empty() {
        views
            .installed_popup
            .selectItemAtIndex(state.selected_installed as isize);
    }
    views
        .installed_popup
        .setEnabled(!state.is_busy() && !state.installed.is_empty());

    views.play_button.setEnabled(state.play_enabled());
    // Hidden rather than disabled: a control that can never become usable in
    // this mode is noise.
    views.remove_button.setHidden(!state.multi_version);
    views.remove_button.setEnabled(state.remove_enabled());

    // Available releases.
    views.releases_popup.removeAllItems();
    for row in &state.releases {
        let label = match row.age {
            Some(age) => strings::format2("BuildListing", &row.tag, &strings::age(age)),
            None => row.tag.clone(),
        };
        views
            .releases_popup
            .addItemWithTitle(&NSString::from_str(&label));
    }
    if !state.releases.is_empty() {
        views
            .releases_popup
            .selectItemAtIndex(state.selected_release as isize);
    }
    views
        .releases_popup
        .setEnabled(state.popup_enabled() && !state.releases.is_empty());
    views.download_button.setEnabled(state.download_enabled());

    // Progress.
    match state.busy {
        Some(busy) => {
            views.progress.setHidden(false);
            match busy.value {
                Some(v) => {
                    views.progress.setIndeterminate(false);
                    views.progress.setDoubleValue(v);
                }
                None => {
                    views.progress.setIndeterminate(true);
                    // SAFETY: a `None` sender has no type of its own to get
                    // wrong; `startAnimation:` has no further requirements.
                    unsafe { views.progress.startAnimation(None) };
                }
            }
        }
        None => {
            views.progress.setHidden(true);
            // SAFETY: a `None` sender has no type of its own to get wrong;
            // `stopAnimation:` has no further requirements.
            unsafe { views.progress.stopAnimation(None) };
        }
    }

    // The button says what is happening only while something is actually
    // reporting a phase. `busy.status` is `None` between initiating an
    // operation and its first progress report, and for the whole of an
    // operation that reports none (switching version, changing mode,
    // removing), so the button keeps its ordinary title -- disabled
    // throughout -- rather than claiming a download that is not running.
    let download_title = match state.busy.and_then(|busy| busy.status) {
        Some(status) => strings::status(status),
        None => strings::get("Download"),
    };
    views
        .download_button
        .setTitle(&NSString::from_str(&download_title));

    // Checkboxes.
    set_check(&views.develop_check, state.show_develop, !state.is_busy());
    set_check(
        &views.auto_update_check,
        state.auto_update(),
        !state.is_busy(),
    );
    set_check(
        &views.settings.multi_version_check,
        state.multi_version,
        !state.is_busy(),
    );
    set_check(
        &views.settings.check_updates_check,
        state.check_for_updates,
        true,
    );

    // Game data. Which rows are relevant follows the selected game, not the
    // list of original games: RollerCoaster Tycoon 1 and 2 are both read by
    // OpenRCT2, and showing Locomotion's row beside them would offer somebody
    // an install that the game in front of them cannot use.
    for row in &views.game_data {
        let relevant = row.game.configured_in() == state.selected_game;
        row.container.setHidden(!relevant);
        if !relevant {
            continue;
        }

        let installed = state.game_data(row.game);

        row.path
            .setStringValue(&NSString::from_str(&match installed {
                Some(path) => path.to_string_lossy().into_owned(),
                // An optional one says so, because "not installed" next to a
                // game that will not start without it means something different
                // from the same words next to extra content.
                None if optional(row.game) => format!(
                    "{} ({})",
                    strings::get("GameDataNotInstalled"),
                    strings::get("GameDataOptional")
                ),
                None => strings::get("GameDataNotInstalled"),
            }));

        row.button
            .setTitle(&NSString::from_str(&strings::get(if installed.is_some() {
                // Not "Change": this asks for an installer and unpacks it
                // again. Where the data goes is a setting, not a per-game
                // choice, and the path shown is read from the game's own
                // configuration.
                "ReinstallGameData"
            } else {
                "InstallGameData"
            })));
        row.button.setEnabled(!state.is_busy());
    }

    views
        .settings
        .install_root_label
        .setStringValue(&NSString::from_str(&match &state.install_root {
            Some(path) => path.to_string_lossy().into_owned(),
            None => strings::get("DefaultDirectory"),
        }));

    // The banner, which is hidden whenever there is nothing to say: no update
    // found, one dismissed, or the check turned off.
    match &state.update {
        Some(update) => views.update_banner.show(&update.version),
        None => views.update_banner.hide(),
    }

    // Error banner. Added here rather than in Task 18 because AlertView is
    // created by this task.
    match &state.error {
        Some(alert) => {
            // `title_arg` is `Some` only for `FailedToLaunchGame`, the one
            // error title with a `{0}` in it; every other title is looked
            // up verbatim. `state.rs` stays free of `strings.rs`, so it
            // hands over the raw argument rather than a formatted string.
            let title = match &alert.title_arg {
                Some(arg) => strings::format1(&alert.title, arg),
                None => strings::get(&alert.title),
            };
            // The message is a worker's own words in every case but one --
            // the store's, GitHub's or the OS's, none of which is
            // localizable -- so it is shown verbatim unless the reducer
            // said otherwise. It says otherwise only for an error it raised
            // itself, which has no reported text to show and names a key
            // instead. See `Alert::message_is_key`.
            let message = if alert.message_is_key {
                strings::get(&alert.message)
            } else {
                alert.message.clone()
            };
            views.alert.show(&title, &message);
        }
        None => views.alert.hide(),
    }
}

fn set_check(button: &NSButton, checked: bool, enabled: bool) {
    button.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    button.setEnabled(enabled);
}

/// Whether a game's data is extra content rather than something the
/// reimplementation cannot start without. Only RollerCoaster Tycoon 1 is:
/// OpenRCT2 runs without it and gains its scenarios and objects when it is
/// there.
const fn optional(game: turnstile_core::gamedata::OriginalGame) -> bool {
    matches!(
        game,
        turnstile_core::gamedata::OriginalGame::RollerCoasterTycoon1
    )
}
