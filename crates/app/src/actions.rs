//! `Actions`, the single `NSObject` subclass every control's target and the
//! sidebar's data source/delegate point at.
//!
//! `Actions` holds no controller reference and, apart from one suppression
//! flag described below, no state. Its selectors just post a `Msg` onto the
//! main queue (`mainqueue::post`), which delivers to whatever
//! `mainqueue::register` installed -- the same path a background worker's
//! reply takes. That is what lets `Actions` be built, and the sidebar show
//! its two rows and respond to selection, before any controller exists:
//! there is no `attach_target` step, because there is nothing to attach.
//!
//! The one hazard: a message posted before `mainqueue::register` runs is
//! silently dropped (see `mainqueue::post`). Every *button* selector waits
//! on user input, so none of them can fire that early. The sidebar is the
//! exception, because `tableViewSelectionDidChange:` is a notification
//! rather than an action and AppKit sends it for a programmatic selection
//! too -- including the one `views::apply` makes. `select_sidebar_row` is
//! the only supported way to move that selection from code, and it holds
//! `suppress_sidebar` for the duration so nothing is posted; see
//! `table_view_selection_did_change` for why that is required rather than
//! merely tidy.

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

/// Almost a pure event-to-message translator. The one field is the flag
/// that tells `tableViewSelectionDidChange:` a selection came from
/// `views::apply` rather than from the user. `Cell` rather than anything
/// synchronised because `Actions` is `MainThreadOnly`, so both the write
/// and the notification it guards happen on the same thread.
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
    // returning a row count consistent with what the delegate below hands
    // back for each row, which the fixed, two-entry `sidebar::ROWS` table
    // guarantees.
    unsafe impl NSTableViewDataSource for Actions {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows_in_table_view(&self, _table_view: &NSTableView) -> NSInteger {
            sidebar::row_count()
        }
    }

    // SAFETY: `NSControlTextEditingDelegate` has no safety requirements.
    // Required by `NSTableViewDelegate` below; it has no methods of its
    // own that apply here.
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
        /// alike.
        ///
        /// This replaced a `sidebarChanged:` target/action, which the table
        /// fires only on a click: arrow keys moved `NSTableView`'s own
        /// selection and redrew the highlight but never reached the
        /// reducer, so a keyboard-only user could not change game at all.
        /// The two are not combined -- the action was removed rather than
        /// kept alongside this -- because a click fires both, and posting
        /// `SelectGame` twice would bump the generation twice and refetch
        /// the release list for nothing.
        ///
        /// AppKit sends this for a programmatic selection as well as a
        /// user's, and `views::apply` sets the selection on every single
        /// render. Unguarded, the startup restore of a persisted game --
        /// the one case where `apply` really does move the row -- would
        /// post a `SelectGame` out of the middle of a render, bumping the
        /// generation and refetching the release list it had just asked
        /// for. Whether that settles after one extra round or keeps going
        /// depends on AppKit staying silent when a programmatic selection
        /// does not actually change, which is not something to build on:
        /// the flag makes the question moot. `select_sidebar_row` is the
        /// only thing that sets it, and AppKit posts this notification
        /// synchronously from inside
        /// `selectRowIndexes:byExtendingSelection:`, which is what makes a
        /// plain flag sufficient.
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
        // One selector per control.
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
    }
);

/// `indexOfSelectedItem` returns `-1` when nothing is selected; that has no
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

    /// Moves the sidebar's selection from code without the delegate
    /// mistaking it for the user's doing. Every programmatic selection must
    /// go through here: `views::apply` calls it on every render, and a bare
    /// `selectRowIndexes:byExtendingSelection:` would post a `SelectGame`
    /// straight back into the render that made it.
    ///
    /// The flag is cleared unconditionally afterwards rather than only on
    /// the way out of a successful call, because leaving it set would
    /// silently deafen the sidebar for the rest of the session.
    pub fn select_sidebar_row(&self, table: &NSTableView, row: usize) {
        self.ivars().suppress_sidebar.set(true);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        self.ivars().suppress_sidebar.set(false);
    }

    /// No controller reference: `mainqueue::post` delivers to whatever
    /// `mainqueue::register` installed. One runloop hop, and every message
    /// reaches the controller by the same path whether it came from a
    /// worker thread or a control.
    fn send(&self, msg: Msg) {
        mainqueue::post(msg);
    }
}

/// Builds the app's `Actions` instance and wires it in as `table`'s data
/// source and delegate, so the sidebar shows its two rows and responds to
/// selection.
///
/// Deliberately no `setTarget`/`setAction` here: selection is handled by
/// `tableViewSelectionDidChange:` alone, which covers the keyboard as well
/// as the mouse. Adding the action back would double every click.
pub fn install(mtm: MainThreadMarker, table: &NSTableView) -> Retained<Actions> {
    let actions = Actions::new(mtm);
    // SAFETY: `actions` is retained by `Views` for the app's lifetime,
    // which outlives `table`.
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&*actions)));
        table.setDelegate(Some(ProtocolObject::from_ref(&*actions)));
    }
    actions
}
