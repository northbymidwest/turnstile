//! The strip that appears when a newer Turnstile has been published.
//!
//! Hidden unless there is something to say, like `AlertView` above it, so the
//! window looks exactly as it did before when there is no update and when the
//! check is turned off.
//!
//! Two buttons rather than one. Following the notice and dismissing it are
//! different intentions, and a banner that can only be acted on is a banner
//! that stays until you act.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSButton, NSColor, NSImage, NSImageView, NSLayoutAttribute, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSString, ns_string};

use crate::actions::Actions;
use crate::strings;

pub struct UpdateBanner {
    pub view: Retained<NSStackView>,
    message: Retained<NSTextField>,
}

impl UpdateBanner {
    pub fn new(mtm: MainThreadMarker, actions: &Actions) -> UpdateBanner {
        let target = &**actions;

        let message = NSTextField::labelWithString(&NSString::from_str(""), mtm);

        // SAFETY: `target` outlives the button and `openUpdatePage:` is a
        // real selector `Actions` defines.
        let open = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&strings::get("Update")),
                Some(target),
                Some(sel!(openUpdatePage:)),
                mtm,
            )
        };

        // SAFETY: as above, for `dismissUpdate:`.
        let dismiss = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&strings::get("Dismiss")),
                Some(target),
                Some(sel!(dismissUpdate:)),
                mtm,
            )
        };

        // Grows to fill whatever is left, which is what pushes the two
        // buttons to the trailing edge.
        let spacer = NSView::new(mtm);

        let icon = NSImageView::new(mtm);
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("arrow.down.circle.fill"),
            None,
        );
        icon.setImage(symbol.as_deref());
        icon.setContentTintColor(Some(&NSColor::controlAccentColor()));

        let view = NSStackView::new(mtm);
        view.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        view.setSpacing(8.0);
        view.setAlignment(NSLayoutAttribute::CenterY);
        view.setEdgeInsets(NSEdgeInsets {
            top: 10.0,
            left: 16.0,
            bottom: 10.0,
            right: 16.0,
        });
        view.addArrangedSubview(&icon);
        view.addArrangedSubview(&message);
        view.addArrangedSubview(&spacer);
        view.addArrangedSubview(&open);
        view.addArrangedSubview(&dismiss);
        view.setHidden(true);

        UpdateBanner { view, message }
    }

    pub fn show(&self, version: &str) {
        self.message
            .setStringValue(&NSString::from_str(&strings::format1(
                "UpdateAvailable",
                version,
            )));
        self.view.setHidden(false);
    }

    pub fn hide(&self) {
        self.view.setHidden(true);
    }
}
