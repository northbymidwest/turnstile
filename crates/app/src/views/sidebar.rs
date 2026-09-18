//! The sidebar: two fixed rows, OpenRCT2 and OpenLoco, each showing an icon
//! and a name. `NSTableViewStyle::SourceList` is what gives the rows the
//! translucent, inset look; the row content itself comes from whatever is
//! installed as the table's data source and delegate (`Actions`, built in
//! `actions.rs`).

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSImage, NSImageView, NSScrollView, NSTableCellView, NSTableColumn, NSTableView,
    NSTableViewStyle, NSTextField, NSViewController,
};
use objc2_foundation::{NSBundle, NSString, ns_string};
use turnstile_core::game::GameId;

/// A fixed order, independent of `GameId::ALL`'s: a future reordering there
/// must not silently reorder the sidebar out from under a saved selection.
const ROWS: [GameId; 2] = [GameId::OpenRCT2, GameId::OpenLoco];

pub fn row_count() -> isize {
    ROWS.len() as isize
}

pub fn game_for_row(row: isize) -> Option<GameId> {
    usize::try_from(row).ok().and_then(|i| ROWS.get(i).copied())
}

/// The inverse of `game_for_row`. A click already leaves the table selection
/// where the user put it, but restoring the last-selected game from
/// preferences at startup does not go through a click at all.
pub fn row_for_game(game: GameId) -> Option<usize> {
    ROWS.iter().position(|&g| g == game)
}

/// Builds the sidebar pane: a single-column table inside a scroll view,
/// wrapped in the view controller
/// `NSSplitViewItem::sidebarWithViewController` needs. Returns that controller
/// and the raw table view, so its data source and delegate can be attached
/// once `Actions` exists.
pub fn build(mtm: MainThreadMarker) -> (Retained<NSViewController>, Retained<NSTableView>) {
    let table = NSTableView::new(mtm);
    table.setStyle(NSTableViewStyle::SourceList);
    table.setHeaderView(None);
    table.setAllowsEmptySelection(false);
    table.setAllowsMultipleSelection(false);

    let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("game"));
    table.addTableColumn(&column);

    let scroll = NSScrollView::new(mtm);
    scroll.setHasVerticalScroller(true);
    // Lets the sidebar's translucent material show through instead of a flat
    // table background painted on top of it.
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(&table));

    let controller = NSViewController::new(mtm);
    controller.setView(&scroll);

    (controller, table)
}

/// One sidebar row: a 20x20 icon beside the game's name, both vertically
/// centered via Auto Layout. A freshly created `NSTableCellView` starts at zero
/// size, so anchoring rather than fixed frames is what keeps them positioned
/// once the table gives the cell its real frame.
pub fn make_row_view(mtm: MainThreadMarker, game: GameId) -> Retained<NSTableCellView> {
    let cell = NSTableCellView::new(mtm);

    let image_view = NSImageView::new(mtm);
    image_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    if let Some(image) = load_icon(game) {
        image_view.setImage(Some(&image));
    }
    cell.addSubview(&image_view);

    let label = NSTextField::labelWithString(&NSString::from_str(game.display_name()), mtm);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    cell.addSubview(&label);

    image_view
        .leadingAnchor()
        .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 8.0)
        .setActive(true);
    image_view
        .centerYAnchor()
        .constraintEqualToAnchor(&cell.centerYAnchor())
        .setActive(true);
    image_view
        .widthAnchor()
        .constraintEqualToConstant(20.0)
        .setActive(true);
    image_view
        .heightAnchor()
        .constraintEqualToConstant(20.0)
        .setActive(true);

    label
        .leadingAnchor()
        .constraintEqualToAnchor_constant(&image_view.trailingAnchor(), 8.0)
        .setActive(true);
    label
        .trailingAnchor()
        .constraintLessThanOrEqualToAnchor(&cell.trailingAnchor())
        .setActive(true);
    label
        .centerYAnchor()
        .constraintEqualToAnchor(&cell.centerYAnchor())
        .setActive(true);

    // SAFETY: both views were just added as subviews above, which retains
    // them, so they outlive these unretained property assignments.
    unsafe {
        cell.setImageView(Some(&image_view));
        cell.setTextField(Some(&label));
    }

    cell
}

fn load_icon(game: GameId) -> Option<Retained<NSImage>> {
    let base_name = match game {
        GameId::OpenRCT2 => "icon-openrct2",
        GameId::OpenLoco => "icon-openloco",
    };
    let path = icon_path(base_name)?;
    NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&path))
}

/// Resolves an icon's path from the app bundle, or straight from the checkout
/// during `cargo run`, which has no bundle for `NSBundle` to find anything in.
fn icon_path(base_name: &str) -> Option<String> {
    let bundle = NSBundle::mainBundle();
    let name = NSString::from_str(base_name);
    if let Some(path) = bundle.pathForResource_ofType(Some(&name), Some(ns_string!("png"))) {
        return Some(path.to_string());
    }
    let dev_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../resources")
        .join(format!("{base_name}.png"));
    dev_path
        .exists()
        .then(|| dev_path.to_string_lossy().into_owned())
}
