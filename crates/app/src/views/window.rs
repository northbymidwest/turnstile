//! The main window: a translucent sidebar beside the detail pane, joined by
//! an `NSSplitViewController`.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSSplitViewController, NSSplitViewItem, NSViewController, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, ns_string};

/// Builds the window around `sidebar` and `detail`, the already-constructed
/// view controllers for the two panes.
pub fn build(
    mtm: MainThreadMarker,
    sidebar: &NSViewController,
    detail: &NSViewController,
) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(720.0, 460.0));
    // SAFETY: `content_rect` and `style` describe an ordinary titled,
    // resizable window; nothing here has further safety requirements beyond
    // being called on the main thread, which `mtm` proves.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content_rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setContentMinSize(NSSize::new(560.0, 400.0));
    window.setTitle(ns_string!("Turnstile"));

    let split = NSSplitViewController::new(mtm);

    let sidebar_item = NSSplitViewItem::sidebarWithViewController(sidebar);
    // The single call that supplies translucency, the inset titlebar, and
    // correct collapse behavior.
    sidebar_item.setMinimumThickness(180.0);
    sidebar_item.setMaximumThickness(240.0);
    split.addSplitViewItem(&sidebar_item);

    let detail_item = NSSplitViewItem::splitViewItemWithViewController(detail);
    split.addSplitViewItem(&detail_item);

    window.setContentViewController(Some(&split));

    // Must come after the content view controller is set: the autosaved frame
    // this restores is meaningless without content to size around.
    window.center();
    window.setFrameAutosaveName(ns_string!("MainWindow"));

    window.makeKeyAndOrderFront(None);
    window
}
