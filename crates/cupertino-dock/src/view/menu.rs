//! A tile's or the separator's context menu as a CupertinoKit menu: a popup of the bar with a grab, on the menu
//! material, whose enabled items are one roving group (the arrows, Home, End and typed characters move between them,
//! Return or Space chooses, Escape or a press outside closes).

use cupertino_kit::controls::{Menu, MenuAnchor, MenuItem as KitItem, MenuStyle};
use cupertino_kit::core::{
    Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Popup, PopupAnchor,
    Propagation, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::theme::Surface;

use super::bar::MENU_ANCHOR;
use super::{menu_key, Input};
use crate::config::Orientation;
use crate::menu::{MenuItem, GAP};

/// The corner radius of a tile menu's panel, and the least width it has.
pub const RADIUS: f64 = 14.5;
pub const MIN_WIDTH: f64 = 110.0;
/// How far a tile menu's panel stands above the bar's top edge, and how far its tail's tip does.
pub const PANEL_GAP: f64 = 12.5;
pub const TAIL_GAP: f64 = 3.0;
/// How far the panel's leading edge is to the leading side of the tile's middle, which is where the tail points: the
/// panel stands off to the trailing side of the icon.
pub const TAIL_FROM_EDGE: f64 = 27.5;
/// The tail: its width and its depth, which reaches up into the panel a little.
pub const TAIL_WIDTH: f64 = 22.0;
pub const TAIL_DEPTH: f64 = 10.0;

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
    let bottom = view.orientation == Orientation::Bottom;
    let anchor = match view.orientation {
        // above the bar, its leading edge to the leading side of the icon's middle
        Orientation::Bottom => MenuAnchor::Above {
            key: Key::from(MENU_ANCHOR),
            gap: PANEL_GAP,
            dx: -TAIL_FROM_EDGE,
        },
        Orientation::Left => MenuAnchor::At(Point::new(a.x() + a.width() + GAP, a.y())),
        // trailing has no room at the right edge, so the popup flips to the leading side of the point
        Orientation::Right => MenuAnchor::At(Point::new(a.x() - GAP, a.y())),
    };
    let parent = view.parent.clone();
    let mut windows = Menu {
        parent: view.parent,
        anchor,
        label: view.label,
        items,
        current: None,
        open: Vec::new(),
        on_open: Box::new(move |_| input(Input::MenuClosed)),
        on_close: input(Input::MenuClosed),
    }
    .windows_styled(
        menu_key(),
        MenuStyle {
            radius: RADIUS,
            min_width: MIN_WIDTH,
        },
    );
    if bottom {
        windows.push(tail(parent, input));
    }
    windows
}

/// The key of the tail's window.
fn tail_key() -> WindowKey {
    WindowKey(Key::from("menu-tail"))
}

/// The tail of a menu above the bar: a small piece of the menu's material standing under the panel's middle-left,
/// its tip a little above the bar, over the icon. A popup of the bar of its own, which takes no input and goes with the
/// menu.
fn tail<M: Clone + 'static>(parent: WindowKey, input: fn(Input) -> M) -> Window<M> {
    Window {
        key: tail_key(),
        spec: WindowSpec {
            title: String::new(),
            app_id: String::new(),
            size: Size::new(TAIL_WIDTH, TAIL_DEPTH),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: Some(Vec::new()),
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Surface {
            surface: Surface::Menu,
            radius: TAIL_DEPTH / 2.0,
            alpha: 1.0,
        },
        content: Element::new(TailNode),
        popup: Some(Popup {
            parent,
            anchor: PopupAnchor::Above {
                key: Key::from(MENU_ANCHOR),
                gap: TAIL_GAP,
                dx: -TAIL_WIDTH / 2.0,
            },
            grab: false,
            on_dismiss: input(Input::MenuClosed),
        }),
        menus: None,
    }
}

/// The tail's content: nothing but its size, the material is the window's.
struct TailNode;

impl<M: Clone + 'static> Widget<M> for TailNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(TAIL_WIDTH, TAIL_DEPTH),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}
