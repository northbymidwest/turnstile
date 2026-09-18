//! `Actions`, the single `NSObject` subclass every control's target and the
//! sidebar's data source/delegate point at.
//!
//! It holds no controller reference and, apart from one suppression flag, no
//! state: its selectors just post a `Msg` onto the main queue, the same path a
//! background worker's reply takes. That is what lets `Actions` be built, and
//! the sidebar work, before any controller exists.
//!
//! The one hazard: a message posted before `mainqueue::register` runs is
//! silently dropped. Every *button* selector waits on user input, so none can
//! fire that early. The sidebar is the exception, because
//! `tableViewSelectionDidChange:` is a notification and AppKit sends it for a
//! programmatic selection too; `select_sidebar_row` is the only supported way
//! to move that selection from code.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSButton, NSControlStateValueOn, NSControlTextEditingDelegate, NSPopUpButton, NSTableColumn,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSView,
};
use objc2_foundation::{NSIndexSet, NSInteger, NSNotification, NSObjectProtocol};

use crate::mainqueue;
use crate::state::Msg;
use crate::views::sidebar;
use turnstile_core::gamedata::OriginalGame;

/// The one field is the flag that tells `tableViewSelectionDidChange:` a
/// selection came from `views::apply` rather than from the user. `Cell` rather
/// than anything synchronised because `Actions` is `MainThreadOnly`.
pub struct Ivars {
    suppress_sidebar: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct Actions;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for Actions {}

    // SAFETY: `NSTableViewDataSource` has no safety requirements beyond
    // returning a row count consistent with what the delegate hands back per
    // row, which the fixed `sidebar::ROWS` table guarantees.
    unsafe impl NSTableViewDataSource for Actions {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows_in_table_view(&self, _table_view: &NSTableView) -> NSInteger {
            sidebar::row_count()
        }
    }

    // SAFETY: `NSControlTextEditingDelegate` has no safety requirements. It is
    // required by `NSTableViewDelegate` below.
    unsafe impl NSControlTextEditingDelegate for Actions {}

    // SAFETY: `NSTableViewDelegate` has no safety requirements.
    unsafe impl NSTableViewDelegate for Actions {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn table_view_view_for_table_column_row(
            &self,
            _table_view: &NSTableView,
            _table_column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            sidebar::game_for_row(row).map(|game| sidebar::make_row_view(self.mtm(), game).into_super())
        }

        /// The sidebar's one selection hook, covering mouse and keyboard
        /// alike. This replaced a `sidebarChanged:` target/action, which the
        /// table fires only on a click, so arrow keys never reached the
        /// reducer. The two are not combined, because a click fires both and
        /// `SelectGame` twice would refetch the release list for nothing.
        ///
        /// AppKit sends this for a programmatic selection as well, and
        /// `views::apply` sets the selection on every render, so the startup
        /// restore of a persisted game would otherwise post a `SelectGame` out
        /// of the middle of a render. `select_sidebar_row` is the only thing
        /// that sets the flag, and AppKit posts this notification
        /// synchronously, which is what makes a plain flag sufficient.
        #[unsafe(method(tableViewSelectionDidChange:))]
        fn table_view_selection_did_change(&self, notification: &NSNotification) {
            if self.ivars().suppress_sidebar.get() {
                return;
            }
            let object = notification.object();
            let Some(table) = object.and_then(|o| o.downcast::<NSTableView>().ok()) else {
                return;
            };
            if let Some(game) = sidebar::game_for_row(table.selectedRow()) {
                self.send(Msg::SelectGame(game));
            }
        }
    }

    impl Actions {
        #[unsafe(method(playClicked:))]
        fn play_clicked(&self, _sender: Option<&NSObject>) {
            self.send(Msg::ClickPlay);
        }

        #[unsafe(method(downloadClicked:))]
        fn download_clicked(&self, _sender: Option<&NSObject>) {
            self.send(Msg::ClickDownload);
        }

        #[unsafe(method(removeClicked:))]
        fn remove_clicked(&self, _sender: Option<&NSObject>) {
            self.send(Msg::ClickRemove);
        }

        #[unsafe(method(installedChanged:))]
        fn installed_changed(&self, sender: Option<&NSPopUpButton>) {
            if let Some(index) = selected_index(sender) {
                self.send(Msg::SelectInstalled(index));
            }
        }

        #[unsafe(method(buildChanged:))]
        fn build_changed(&self, sender: Option<&NSPopUpButton>) {
            if let Some(index) = selected_index(sender) {
                self.send(Msg::SelectRelease(index));
            }
        }

        #[unsafe(method(developToggled:))]
        fn develop_toggled(&self, sender: Option<&NSButton>) {
            if let Some(on) = checkbox_state(sender) {
                self.send(Msg::ToggleDevelop(on));
            }
        }

        #[unsafe(method(autoUpdateToggled:))]
        fn auto_update_toggled(&self, sender: Option<&NSButton>) {
            if let Some(on) = checkbox_state(sender) {
                self.send(Msg::ToggleAutoUpdate(on));
            }
        }

        #[unsafe(method(checkForUpdatesToggled:))]
        fn check_for_updates_toggled(&self, sender: Option<&NSButton>) {
            if let Some(on) = checkbox_state(sender) {
                self.send(Msg::ToggleCheckForUpdates(on));
            }
        }

        #[unsafe(method(openUpdatePage:))]
        fn open_update_page(&self, _sender: Option<&NSObject>) {
            self.send(Msg::OpenUpdatePage);
        }

        #[unsafe(method(dismissUpdate:))]
        fn dismiss_update(&self, _sender: Option<&NSObject>) {
            self.send(Msg::DismissUpdate);
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&NSObject>) {
            self.send(Msg::ShowSettings);
        }

        #[unsafe(method(multiVersionToggled:))]
        fn multi_version_toggled(&self, sender: Option<&NSButton>) {
            if let Some(on) = checkbox_state(sender) {
                self.send(Msg::ToggleMultiVersion(on));
            }
        }

        /// One selector for three buttons, told apart by the sender's tag: the
        /// game's position in `OriginalGame::ALL`, set where the button is
        /// built from the same list, so the two cannot drift.
        #[unsafe(method(installGameDataClicked:))]
        fn install_game_data_clicked(&self, sender: Option<&NSButton>) {
            if let Some(game) = sender.and_then(game_for_tag) {
                self.send(Msg::ChooseInstaller(game));
            }
        }

        #[unsafe(method(chooseInstallRootClicked:))]
        fn choose_install_root_clicked(&self, _sender: Option<&NSObject>) {
            self.send(Msg::ChooseInstallRoot);
        }

        #[unsafe(method(useDefaultInstallRootClicked:))]
        fn use_default_install_root_clicked(&self, _sender: Option<&NSObject>) {
            self.send(Msg::InstallRootChosen(None));
        }
    }
);

/// `indexOfSelectedItem` returns `-1` when nothing is selected, which has no
/// `usize` equivalent, so it is treated the same as no sender at all.
fn selected_index(popup: Option<&NSPopUpButton>) -> Option<usize> {
    usize::try_from(popup?.indexOfSelectedItem()).ok()
}

fn checkbox_state(button: Option<&NSButton>) -> Option<bool> {
    Some(button?.state() == NSControlStateValueOn)
}

impl Actions {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            suppress_sidebar: Cell::new(false),
        });
        // SAFETY: The signature of `NSObject`'s `init` method is correct.
        unsafe { msg_send![super(this), init] }
    }

    /// Moves the sidebar's selection from code without the delegate mistaking
    /// it for the user's doing. Every programmatic selection must go through
    /// here: a bare `selectRowIndexes:byExtendingSelection:` would post a
    /// `SelectGame` straight back into the render that made it.
    ///
    /// The flag is cleared unconditionally afterwards, since leaving it set
    /// would silently deafen the sidebar for the rest of the session.
    pub fn select_sidebar_row(&self, table: &NSTableView, row: usize) {
        self.ivars().suppress_sidebar.set(true);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        self.ivars().suppress_sidebar.set(false);
    }

    fn send(&self, msg: Msg) {
        mainqueue::post(msg);
    }
}

/// Builds the app's `Actions` instance and wires it in as `table`'s data
/// source and delegate.
///
/// Deliberately no `setTarget`/`setAction`: selection is handled by
/// `tableViewSelectionDidChange:` alone, which covers the keyboard as well as
/// the mouse. Adding the action back would double every click.
pub fn install(mtm: MainThreadMarker, table: &NSTableView) -> Retained<Actions> {
    let actions = Actions::new(mtm);
    // SAFETY: `actions` is retained by `Views` for the app's lifetime, which
    // outlives `table`.
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&*actions)));
        table.setDelegate(Some(ProtocolObject::from_ref(&*actions)));
    }
    actions
}

/// The game a tagged button stands for. Out of range means a button somebody
/// tagged by hand, and doing nothing beats acting on whichever game sits at
/// index zero.
fn game_for_tag(sender: &NSButton) -> Option<OriginalGame> {
    let tag = usize::try_from(sender.tag()).ok()?;
    OriginalGame::ALL.get(tag).copied()
}
