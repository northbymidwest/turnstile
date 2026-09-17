//! The inline error banner (`AlertView`), plus the two modal dialogs that
//! are not tied to `UiState` at all: `confirm_remove` and
//! `show_disable_report` both block on `NSAlert::runModal`, called directly
//! from `Actions` rather than routed through the reducer, because a modal
//! answer is needed before the corresponding `Msg` can even be constructed.
//!
//! `AlertView` is a reusable component rather than inlined into `apply`'s
//! caller because the deferred launcher-update banner will want exactly
//! this shape: an icon, a bold title, and a wrapping message.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSColor, NSFont, NSImage, NSImageView,
    NSLayoutAttribute, NSLineBreakMode, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation,
};
use objc2_foundation::{NSEdgeInsets, NSString, ns_string};
use turnstile_core::store::DisableReport;

use crate::strings;

/// A horizontal banner: a warning triangle beside a bold title and a
/// wrapping message. Hidden by default; `apply` calls `show`/`hide` once
/// per reduce, same as every other control.
pub struct AlertView {
    pub view: Retained<NSStackView>,
    title_label: Retained<NSTextField>,
    message_label: Retained<NSTextField>,
}

impl AlertView {
    pub fn new(mtm: MainThreadMarker) -> AlertView {
        let icon = NSImageView::new(mtm);
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("exclamationmark.triangle.fill"),
            None,
        );
        icon.setImage(symbol.as_deref());
        icon.setContentTintColor(Some(&NSColor::systemOrangeColor()));

        let title_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        title_label.setFont(Some(
            &NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
        ));

        let message_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        // `maximumNumberOfLines(0)` alone does not wrap: word-wrapping is
        // what actually turns a long message into multiple lines within
        // `preferredMaxLayoutWidth`, as Task 18 found for the multi-version
        // detail label.
        message_label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        message_label.setMaximumNumberOfLines(0);
        message_label.setPreferredMaxLayoutWidth(420.0);

        let text = NSStackView::new(mtm);
        text.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        text.setSpacing(2.0);
        text.addArrangedSubview(&title_label);
        text.addArrangedSubview(&message_label);

        let root = NSStackView::new(mtm);
        root.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        root.setSpacing(8.0);
        // Top-aligns the icon with the title rather than centering it
        // against the whole (potentially multi-line) text column.
        root.setAlignment(NSLayoutAttribute::Top);
        root.setEdgeInsets(NSEdgeInsets {
            top: 12.0,
            left: 16.0,
            bottom: 12.0,
            right: 16.0,
        });
        root.addArrangedSubview(&icon);
        root.addArrangedSubview(&text);
        root.setHidden(true);

        AlertView {
            view: root,
            title_label,
            message_label,
        }
    }

    pub fn show(&self, title: &str, message: &str) {
        self.title_label.setStringValue(&NSString::from_str(title));
        self.message_label
            .setStringValue(&NSString::from_str(message));
        self.view.setHidden(false);
    }

    pub fn hide(&self) {
        self.view.setHidden(true);
    }
}

/// Blocks on a modal. Removal is not reversible, which is the whole reason
/// this exists: switching versions has no confirmation because it is one
/// atomic rename and is undone by picking the other entry again.
pub fn confirm_remove(mtm: MainThreadMarker, tag: &str) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(&strings::format1(
        "RemoveConfirmTitle",
        tag,
    )));
    alert.setInformativeText(&NSString::from_str(&strings::get("RemoveConfirmMessage")));

    let remove = alert.addButtonWithTitle(&NSString::from_str(&strings::get("Remove")));
    let cancel = alert.addButtonWithTitle(&NSString::from_str(&strings::get("Cancel")));
    // The reasoning here has been wrong twice, so what follows is measured
    // rather than reasoned. A throwaway probe built this same alert, ran it
    // modally, pressed keys at `kCGHIDEventTap`, and read the key
    // equivalents back as byte values:
    //
    //                                      Remove  Cancel  Escape      Return
    //   after addButtonWithTitle, English  [13]    [27]    cancels     removes
    //   after addButtonWithTitle, German   [13]    []      nothing     removes
    //   Remove "", Cancel "\r"             []      [13]    nothing     cancels
    //   Remove "", Cancel "\033"           []      [27]    cancels     nothing
    //
    // Three things follow, and only the third could not have been reasoned
    // out. `NSAlert`'s automatic Escape goes only to a button whose title is
    // literally "Cancel", so in the eight non-English locales this app ships
    // it never existed, and reordering the buttons to inherit it would fix
    // English alone. A button holds exactly one key equivalent, so
    // `setKeyEquivalent` replaces rather than adds: the previous version of
    // this code gave Cancel `"\r"` and by doing so took Escape away from the
    // one locale that had it, leaving no button in the alert carrying it.
    // And with no button bound to Escape, the panel does **not** dismiss by
    // itself -- the probe pressed Escape against a real modal and `runModal`
    // did not return, so `cancelOperation:` reaching the panel is not a way
    // out of this dialog.
    //
    // Escape is therefore the binding worth having: it is the reflex for
    // backing out, and backing out of this particular dialog is the whole
    // reason it exists. Return is deliberately left bound to nothing, a
    // downgrade from the previous round's "Return dismisses" that is forced
    // by one button holding one equivalent. Space was probed too and
    // activates nothing.
    //
    // The invariant any future edit here must preserve: **no keystroke may
    // reach the Remove button.** Clearing its equivalent is what does that,
    // and it held in every row above.
    remove.setKeyEquivalent(&NSString::from_str(""));
    cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));

    alert.runModal() == NSAlertFirstButtonReturn
}

/// Informational, not a confirmation: nothing was destroyed. It exists so a
/// user who turns the setting off knows their other builds are still on
/// disk and how to get them back, rather than assuming they were deleted.
pub fn show_disable_report(mtm: MainThreadMarker, report: &DisableReport) {
    if report.retained.is_empty() {
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Informational);
    // `kept` is `None` when there was nothing linked to collapse, which is
    // reachable whenever `bin` is absent or points outside `versions/`.
    // "Now using {0}" has nothing to name in that case, and rendering it
    // with an empty argument produced a title ending in mid-sentence.
    let title = match &report.kept {
        Some(kept) => strings::format1("MultiVersionDisabledTitle", kept),
        None => strings::get("MultiVersionDisabledNoneTitle"),
    };
    alert.setMessageText(&NSString::from_str(&title));
    // A separate key for one, rather than "1 other versions are still on
    // disk". The singular takes only the size, since a count of one is what
    // the wording already says; a language that needs the number can put
    // its own literal in, and one that inflects differently for one is what
    // this key exists for.
    let bytes = human_bytes(report.retained_bytes);
    let message = if report.retained.len() == 1 {
        strings::format1("MultiVersionDisabledMessageOne", &bytes)
    } else {
        strings::format2(
            "MultiVersionDisabledMessage",
            &report.retained.len().to_string(),
            &bytes,
        )
    };
    alert.setInformativeText(&NSString::from_str(&message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.runModal();
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counts_are_reported_in_readable_units() {
        assert_eq!(human_bytes(512), "512 bytes");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn zero_bytes_does_not_produce_a_fractional_unit() {
        assert_eq!(human_bytes(0), "0 bytes");
    }
}
