//! The dock's windows as CupertinoKit draws them: the bar, the tile held by the pointer, the context menus, the Force
//! Quit dialog and the edge strips that move the dock to another display; the switcher's
//! panel and the overview's chrome are [`crate::switcher_kit`] and [`crate::overview_kit`]. Each window is a pure
//! function of what [`crate::kit::Model`] holds; the geometry is the model's (`Dock::layout`,
//! `force_quit::Layout`), so a node is placed where the model put it and reports the pointer and the keys back as an
//! [`Input`].

pub mod bar;
pub mod confirm;
pub mod drag;
pub mod edge;
pub mod fan;
pub mod force_quit;
pub mod label;
pub mod menu;
pub(crate) mod paint;
pub mod stack;

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Key, WindowKey};
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{KeyEvent, Mods};

use crate::overview_kit::OverviewInput;
use crate::switcher_kit::SwitcherInput;

/// The `app_id` prefixes the process's windows open with, each followed by `@` and the key of the output it is on;
/// the surface roles give each its layer surface by them. The context menus are popups of the bar.
pub const BAR_APP_ID: &str = "cupertino-dock";
pub const DRAG_APP_ID: &str = "cupertino-dock-drag";
pub const FORCE_QUIT_APP_ID: &str = "cupertino-force-quit";
pub const EDGE_APP_ID: &str = "cupertino-dock-edge";
pub const SWITCHER_APP_ID: &str = crate::switcher_kit::APP_ID;
pub const OVERVIEW_APP_ID: &str = crate::overview_kit::APP_ID;

/// What assistive technology calls the bar.
pub const BAR_TITLE: &str = "Dock";

/// The bar's window on the output with key `output`: a new key, a new surface, so the dock moves by changing it, and
/// a surface the compositor closed is replaced by taking the next `generation`.
pub fn bar_key(output: u32, generation: u64) -> WindowKey {
    WindowKey(Key::from(format!("bar-{output}-{generation}")))
}

/// The context menu's window.
pub fn menu_key() -> WindowKey {
    WindowKey(Key::from("menu"))
}

/// What the pointer did, in the window's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pointer {
    Move(Point),
    Leave,
    /// The left button went down with these modifiers held.
    Press(Point, Mods),
    /// The left button came up.
    Release(Point),
    /// The right button went down.
    Secondary(Point),
    /// The wheel or the touchpad moved by `delta` points.
    Scroll(Point, Point),
}

/// What the user did on one of the windows.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Bar(Pointer),
    /// Return, Space or assistive technology's click on the tile with this id (`TileId.0`).
    Activate(u64),
    /// The Menu key, Shift+F10 or assistive technology's show-menu on the tile with this id.
    ShowMenu(u64),
    /// Escape in the bar, or the keyboard left it: the bar gives the keyboard back.
    LeaveKeyboard,
    /// The context menu's item at this index was chosen.
    MenuItem(usize),
    /// The context menu closed: an item was chosen, Escape, or a press outside it.
    MenuClosed,
    ForceQuit(Pointer),
    ForceQuitKey(KeyEvent),
    /// Assistive technology's click on a row or a button of the dialog.
    ForceQuitRow(usize),
    ForceQuitButton(crate::force_quit::Button),
    /// The pointer came onto (`true`) or left the edge strip of the output with this key.
    Edge(u32, bool),
    /// The switcher's panel on the output with this key.
    Switcher(u32, SwitcherInput),
    /// The overview's chrome on the output with this key.
    Overview(u32, OverviewInput),
    /// The compositor closed this window-manager surface of the output with this key.
    WmClosed(u32, Wm),
    /// The tile with this id (`TileId.0`) took the bar's keyboard focus.
    TileFocused(u64),
    /// A window whose content moves by itself got a frame from the compositor.
    Frame,
    /// The compositor closed a window the model still shows: its output went away.
    Closed(WindowKey),
    /// The pointer on the open folder stack.
    Stack(Pointer),
    /// A key on the open folder stack that none of its parts took.
    StackKey(KeyEvent),
    /// Assistive technology's click on the stack's entry at this index.
    StackEntry(usize),
    /// The stack's button that opens its folder.
    StackOpenFolder,
    /// The stack closed: Escape, or a press outside it.
    StackClosed,
    /// The question of a tile's menu was answered yes: its button was pressed.
    ConfirmDone,
    /// The question closed: its other button, Escape, or a press outside it.
    ConfirmClosed,
    /// The compositor took the hovered tile's label down; the next pointer motion shows it again.
    LabelGone,
}

/// The window-manager surfaces of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wm {
    Switcher,
    Overview,
}

/// The model's rectangle as the toolkit's.
pub fn kit_rect(r: &cupertino_wm::geometry::Rect) -> cupertino_kit::geometry::Rect {
    cupertino_kit::geometry::Rect::new(r.x, r.y, r.w, r.h)
}

/// The toolkit's point as the model's.
pub fn wm_point(p: Point) -> cupertino_wm::geometry::Point {
    cupertino_wm::geometry::Point::new(p.x, p.y)
}

/// Renditions of one icon at several device-pixel sizes; the painter takes the one that best covers its box.
pub type IconSet = Vec<ImageSource>;
