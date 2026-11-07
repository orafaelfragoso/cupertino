//! A tile's or the separator's context menu as a CupertinoKit menu: a popup of the bar with a grab, on the menu
//! material, whose enabled items are one roving group (the arrows, Home, End and typed characters move between them,
//! Return or Space chooses, Escape or a press outside closes).

use cupertino_kit::controls::{Menu, MenuAnchor, MenuItem as KitItem};
use cupertino_kit::core::{Key, Window, WindowKey};
use cupertino_kit::geometry::{Point, Rect};

use super::{menu_key, Input};
use crate::config::Orientation;
use crate::menu::{MenuItem, GAP};

/// An open context menu as the bar shows it.
pub struct MenuView<'a> {
    /// The bar's window, which the menu is a popup of.
    pub parent: WindowKey,
    pub items: &'a [MenuItem],
    /// What assistive technology calls it: the tile's name.
    pub label: String,
    /// The tile it belongs to (`TileId.0`) and where that tile is in the bar's window.
    pub tile: u64,
    pub anchor: Rect,
    pub orientation: Orientation,
}

/// The menu's windows: the menu itself (its Options are flat, so there are no submenus).
pub fn windows<M: Clone + 'static>(view: MenuView<'_>, input: fn(Input) -> M) -> Vec<Window<M>> {
    let items = view
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            if item.separator {
                KitItem::Separator
            } else {
                KitItem::Item {
                    label: item.label.clone(),
                    enabled: item.enabled,
                    checked: item.checked,
                    on_select: input(Input::MenuItem(i)),
                }
            }
        })
        .collect();
    let a = view.anchor;
    let anchor = match view.orientation {
        // centred over the tile, above it: below has no room at the bottom of the output, so the popup flips
        Orientation::Bottom => MenuAnchor::Below(Key::from(format!("tile-{}", view.tile))),
        Orientation::Left => MenuAnchor::At(Point::new(a.x() + a.width() + GAP, a.y())),
        // trailing has no room at the right edge, so the popup flips to the leading side of the point
        Orientation::Right => MenuAnchor::At(Point::new(a.x() - GAP, a.y())),
    };
    Menu {
        parent: view.parent,
        anchor,
        label: view.label,
        items,
        current: None,
        open: Vec::new(),
        on_open: Box::new(move |_| input(Input::MenuClosed)),
        on_close: input(Input::MenuClosed),
    }
    .windows(menu_key())
}
