//! Menus in popup windows, the regions that open a context menu, and pop-up buttons. A menu's enabled items are one
//! roving group: the arrows and typed characters move the focus between them, and the focused item is drawn
//! highlighted. Each open submenu is a popup of the menu before it, beside its item.
//!
//! Each level's panel follows the pointer as every menu does ([`cupertino_menu_model::tracking`]): the pointer
//! highlights the row under it, a submenu opens after the pointer rests on its row and closes after it rests on
//! another, and while the pointer heads from a row into its open submenu the rows it crosses wait. A press that opened
//! the menu (a secondary press, a press on a pop-up button) and is let go on a row chooses it once the pointer reached
//! a row or the press was held; a quick click leaves the menu open.

use crate::button::ButtonLook;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{
    ButtonRole, ButtonStyle, CalloutPointer, ContextMenu, Handler, InlinePopUpButton, Menu,
    MenuAnchor, MenuItem, MenuStyle, MenuTail, PopUpButton, PullDownButton,
};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_menu_model::tracking::{
    Edge, Release, SubmenuAim, Tracking, Under, SUBMENU_CLOSE_DELAY, SUBMENU_OPEN_DELAY,
    TRACKING_INTERVAL,
};
use cupertino_theme::ControlFamily;
use cupertino_typeahead::TypeAhead;
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActivationSource, Change, Checked, CommandRegistry,
    ControlState, Element, Environment, Event, EventCx, FocusTarget, Glass, Key, MeasureCx,
    PaintCx, Phase, Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::{
    DecorationMode, Keysym, Mods, MouseButton, PointerEvent, Timestamp, TouchEvent, WindowSpec,
};
use cupertino_ui_render::{Paint, PaintOp, Sampling, Shape};
use cupertino_ui_theme::{Color, ColorRole, StateVariant, Surface};
use std::cell::RefCell;
use std::rc::Rc;

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);
const ESCAPE: Keysym = Keysym(0xff1b);
const MENU: Keysym = Keysym(0xff67);
const F10: Keysym = Keysym(0xffc7);
const RETURN: Keysym = Keysym(0xff0d);
const SPACE: Keysym = Keysym(0x20);

/// The panel's corner radius.
pub(crate) const PANEL_RADIUS: f64 = 11.5;
/// The panel's own edge, drawn only over the opaque fill a window without the compositor's material paints.
const EDGE: f64 = 1.0;
/// Above the first row and below the last.
const PADDING_Y: f64 = 5.0;
const ROW_HEIGHT: f64 = 24.0;
const SEPARATOR_HEIGHT: f64 = 11.0;
/// From the leading edge to the labels of a menu without check marks.
const LABEL_LEADING: f64 = 16.5;
/// From the leading edge to the labels of a menu with a check mark on any row: the marks get a column of their own.
const MARKED_LABEL_LEADING: f64 = 30.5;
/// From the leading edge to the middle of a check mark.
const MARK_CENTRE: f64 = 17.0;
/// From the trailing edge to the end of a shortcut or a submenu's chevron.
const TRAILING: f64 = 20.0;
/// From the trailing edge to the end of a separator line; it starts under the labels.
const SEPARATOR_TRAILING: f64 = 16.5;
/// A separator line over the glass in the dark appearance: white at 14 %.
const GLASS_SEPARATOR_DARK: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.14,
};
/// A separator line over the glass in the light appearance: black at 10 %.
const GLASS_SEPARATOR_LIGHT: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.1,
};
/// The width of a submenu's chevron.
const CHEVRON_WIDTH: f64 = 5.0;
pub(crate) const MIN_WIDTH: f64 = 120.0;
/// The least room between a row's label and the shortcuts and chevrons of a menu that has any.
const SHORTCUT_GAP: f64 = 23.0;
/// Between a highlight and the panel's sides.
const HIGHLIGHT_INSET: f64 = 5.5;
const HIGHLIGHT_RADIUS: f64 = 6.75;
/// The share of the label's opacity a shortcut keeps on an enabled row, and on a disabled one (whose label is dimmed
/// already).
const SHORTCUT_ALPHA: f32 = 0.28;
const DISABLED_SHORTCUT_ALPHA: f32 = 0.7;
/// How long the highlight of a chosen row stays off, then on, before the choice is sent, in seconds.
const FLASH_OFF: f64 = 0.034;
const FLASH_ON: f64 = 0.067;
const SYMBOL: f64 = 10.0;
/// How far a submenu overlaps the menu it opens from: the highlight's right edge.
const SUBMENU_OVERLAP: f64 = 5.5;
/// Between a pop-up button and its menu.
const GAP: f64 = 4.0;
/// The most a menu takes; a taller one is cut off.
const MAX: Size = Size {
    width: 480.0,
    height: 720.0,
};
/// Between a pop-up button's label and its chevrons.
const CHEVRONS_GAP: f64 = 6.0;
/// A pop-up button's chevrons: their width, each one's distance from the middle, and their stroke. Approximate
/// design values.
const POPUP_CHEVRON: f64 = 6.0;
const POPUP_CHEVRON_GAP: f64 = 2.75;
const POPUP_CHEVRON_STROKE: f64 = 1.25;
/// An inline pop-up's disc across, and the space between its value and the disc.
const INLINE_DISC: f64 = 20.0;
const INLINE_GAP: f64 = 12.5;
/// An inline pop-up's chevrons: their width, and each one's distance from the middle of the disc.
const INLINE_CHEVRON: f64 = 7.5;
const INLINE_CHEVRON_GAP: f64 = 3.375;
/// Between a pop-up button's sides and its label or chevrons; its menu lines its rows' labels up with the label.
const LABEL_INSET: f64 = 10.0;

/// The columns every row of one menu lines up on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Columns {
    /// From the leading edge to the labels: further in when any row has a check mark.
    label: f64,
    /// Some row has a shortcut or opens a submenu, so every row leaves room for them after its label.
    trailing: bool,
    /// The least width of a row, and so of the panel.
    min_width: f64,
}

impl Columns {
    fn of<M>(items: &[MenuItem<M>], min_width: f64) -> Self {
        let marks = items.iter().any(|item| {
            matches!(
                item,
                MenuItem::Item { checked: true, .. } | MenuItem::Shortcut { checked: true, .. }
            )
        });
        Columns {
            label: if marks {
                MARKED_LABEL_LEADING
            } else {
                LABEL_LEADING
            },
            trailing: items
                .iter()
                .any(|item| matches!(item, MenuItem::Shortcut { .. } | MenuItem::Submenu { .. })),
            min_width,
        }
    }
}

/// A row that was chosen, flashing before its messages are sent: its highlight is off for [`FLASH_OFF`], then on for
/// [`FLASH_ON`]. The menu hears nothing else meanwhile.
struct Flash<M> {
    /// The row's place among the menu's items.
    row: usize,
    start: Timestamp,
    /// The panel sends the messages, not the row: the choice was made on the panel.
    by_panel: bool,
    then: Vec<M>,
}

/// What a menu level's panel and rows share: the choice flashing now, if any.
type Flashing<M> = Rc<RefCell<Option<Flash<M>>>>;

impl<M> Flash<M> {
    fn ends(&self) -> Timestamp {
        after(self.start, FLASH_OFF + FLASH_ON)
    }

    /// The row's highlight shows: not in the first moments of the flash.
    fn lit(&self, now: Timestamp) -> bool {
        now >= after(self.start, FLASH_OFF)
    }
}

/// Starts the flash of row `row` that sends `then` when it ends, or sends them at once with Reduce Motion or while
/// another choice flashes.
fn choose<M: Clone>(
    flash: &Flashing<M>,
    row: usize,
    by_panel: bool,
    then: Vec<M>,
    cx: &mut EventCx<'_, M>,
) {
    if flash.borrow().is_some() {
        return;
    }
    if cx.env().theme.preferences.reduce_motion {
        for message in then {
            cx.emit(message);
        }
        return;
    }
    let start = cx.now();
    *flash.borrow_mut() = Some(Flash {
        row,
        start,
        by_panel,
        then,
    });
    cx.wake_at(after(start, FLASH_OFF));
    cx.request_paint();
}

/// A wake of the node that started the flash: sends the choice once the flash is over, else waits for its end.
/// Whether the wake was the flash's.
fn flash_woke<M: Clone>(
    flash: &Flashing<M>,
    mine: impl Fn(&Flash<M>) -> bool,
    now: Timestamp,
    cx: &mut EventCx<'_, M>,
) -> bool {
    let mut slot = flash.borrow_mut();
    let Some(current) = slot.as_ref().filter(|f| mine(f)) else {
        return false;
    };
    if now >= current.ends() {
        if let Some(done) = slot.take() {
            for message in done.then {
                cx.emit(message);
            }
        }
    } else {
        cx.wake_at(current.ends());
        cx.request_paint();
    }
    true
}

/// The key of the window of the submenu `depth` levels below the menu in `key`.
fn level_key(key: &WindowKey, depth: usize) -> WindowKey {
    if depth == 0 {
        return key.clone();
    }
    let base = match &key.0 {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => s.to_string(),
    };
    WindowKey(Key::from(format!("{base}/{depth}")))
}

impl<M: Clone + 'static> Menu<M> {
    /// The popup windows that show the menu and its open submenus.
    pub fn windows(self, key: WindowKey) -> Vec<Window<M>> {
        self.windows_styled(key, MenuStyle::default())
    }

    /// [`Menu::windows`] with panels of the corner radius and least width of `style`.
    pub fn windows_styled(self, key: WindowKey, style: MenuStyle) -> Vec<Window<M>> {
        let Menu {
            parent,
            anchor,
            label,
            mut items,
            current,
            open,
            on_open,
            on_close,
        } = self;
        let mut parent = parent;
        // the menu opens with its current item over the pointer that pressed the button
        let over = matches!(anchor, MenuAnchor::Over(_));
        // a context menu opens at the pointer, which a menu slid to fit can leave over a row
        let at_pointer = matches!(anchor, MenuAnchor::At(_));
        let mut anchor = match anchor {
            MenuAnchor::Below(key) => PopupAnchor::Below { key, gap: GAP },
            MenuAnchor::At(point) => PopupAnchor::At(point),
            MenuAnchor::Above { key, gap, dx } => PopupAnchor::Above { key, gap, dx },
            MenuAnchor::Over(key) => {
                let above: f64 = items[..current.unwrap_or(0).min(items.len())]
                    .iter()
                    .map(|item| match item {
                        MenuItem::Separator => SEPARATOR_HEIGHT,
                        _ => ROW_HEIGHT,
                    })
                    .sum();
                PopupAnchor::Over {
                    key,
                    offset: Point::new(
                        LABEL_INSET - Columns::of(&items, style.min_width).label,
                        -(PADDING_Y + above + ROW_HEIGHT / 2.0),
                    ),
                }
            }
        };
        let mut label = label;
        let mut windows = Vec::new();
        for depth in 0..=open.len() {
            let path = &open[..depth];
            let next = open.get(depth).copied();
            let submenu = match next.and_then(|i| items.get_mut(i)) {
                Some(MenuItem::Submenu {
                    label,
                    enabled: true,
                    items,
                }) => Some((label.clone(), std::mem::take(items))),
                _ => None,
            };
            let back = (depth > 0).then(|| on_open(open[..depth - 1].to_vec()));
            let typing = Rc::new(RefCell::new(TypeAhead::default()));
            let flash: Flashing<M> = Rc::new(RefCell::new(None));
            let panel = Panel {
                label: label.clone(),
                rows: items
                    .iter()
                    .filter(|i| !matches!(i, MenuItem::Separator))
                    .count(),
                current: current
                    .filter(|_| depth == 0)
                    .and_then(|i| enabled_position(&items, i)),
                escape: back.clone().unwrap_or_else(|| on_close.clone()),
                back,
                slots: slots(&items, path, &on_open, &on_close),
                open: next
                    .zip(submenu.as_ref())
                    .map(|(index, (_, sub))| (index, height(sub))),
                submenu: submenu.as_ref().map(|_| level_key(&key, depth + 1)),
                collapse: on_open(path.to_vec()),
                start: current.filter(|_| depth == 0 && over),
                at_pointer: depth == 0 && at_pointer,
                typing: Rc::clone(&typing),
                flash: Rc::clone(&flash),
                radius: style.radius,
                tailed: depth == 0 && style.tail.is_some(),
                children: rows(
                    &items,
                    style.min_width,
                    path,
                    current.filter(|_| depth == 0),
                    next.filter(|_| submenu.is_some()),
                    &on_open,
                    &on_close,
                    &typing,
                    &flash,
                ),
            };
            let key = level_key(&key, depth);
            let tailed = depth == 0 && style.tail.is_some();
            windows.push(Window {
                key: key.clone(),
                spec: WindowSpec {
                    title: label,
                    app_id: String::new(),
                    size: MAX,
                    min_size: None,
                    max_size: None,
                    decorations: DecorationMode::None,
                    foreign_parent: None,
                    input_region: None,
                },
                shortcuts: CommandRegistry::new(),
                on_close: None,
                background: if tailed {
                    WindowBackground::None
                } else {
                    WindowBackground::Surface {
                        surface: Surface::Menu,
                        radius: style.radius,
                        alpha: 1.0,
                    }
                },
                content: match style.tail.filter(|_| tailed) {
                    Some(tail) => Element::new(Callout {
                        children: vec![
                            Element::new(panel).key("panel"),
                            Element::new(TailRow { tail }).key("tail"),
                        ],
                    }),
                    None => Element::new(panel),
                },
                popup: Some(Popup {
                    parent,
                    anchor,
                    grab: true,
                    on_dismiss: on_close.clone(),
                }),
                menus: None,
            });
            let (Some(index), Some((sub_label, sub_items))) = (next, submenu) else {
                break;
            };
            parent = key;
            anchor = PopupAnchor::Beside {
                key: Key::from(index as u64),
                offset: Point::new(-SUBMENU_OVERLAP, -PADDING_Y),
            };
            label = sub_label;
            items = sub_items;
        }
        windows
    }
}

/// The rows of the menu at `path`, keyed by their index; `current` is the item that takes the focus when it opens,
/// and `open` the item whose submenu is open.
fn rows<M: Clone + 'static>(
    items: &[MenuItem<M>],
    min_width: f64,
    path: &[usize],
    current: Option<usize>,
    open: Option<usize>,
    on_open: &Handler<Vec<usize>, M>,
    on_close: &M,
    typing: &Rc<RefCell<TypeAhead>>,
    flash: &Flashing<M>,
) -> Vec<Element<M>> {
    let columns = Columns::of(items, min_width);
    let labels: Rc<[String]> = items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Item {
                label,
                enabled: true,
                ..
            }
            | MenuItem::Shortcut {
                label,
                enabled: true,
                ..
            }
            | MenuItem::Submenu {
                label,
                enabled: true,
                ..
            } => Some(label.clone()),
            _ => None,
        })
        .collect();
    let mut position = 0;
    let mut place = 0;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let shortcut = match item {
                MenuItem::Shortcut { shortcut, .. } => Some(shortcut.clone()),
                _ => None,
            };
            let (label, enabled, checked, action) = match item {
                MenuItem::Separator => {
                    return Element::new(Separator {
                        leading: columns.label,
                    })
                    .key(index as u64)
                }
                MenuItem::Item {
                    label,
                    enabled,
                    checked,
                    on_select,
                }
                | MenuItem::Shortcut {
                    label,
                    enabled,
                    checked,
                    on_select,
                    ..
                } => (
                    label,
                    *enabled,
                    *checked,
                    RowAction::Select {
                        on_select: on_select.clone(),
                        on_close: on_close.clone(),
                    },
                ),
                MenuItem::Submenu { label, enabled, .. } => {
                    let mut opened = path.to_vec();
                    opened.push(index);
                    (
                        label,
                        *enabled,
                        false,
                        RowAction::Open {
                            on_open: on_open(opened),
                            open: open == Some(index),
                        },
                    )
                }
            };
            let row = Row {
                label: label.clone(),
                shortcut,
                enabled,
                checked,
                action,
                columns,
                labels: Rc::clone(&labels),
                typing: Rc::clone(typing),
                flash: Rc::clone(flash),
                index,
                position,
                place,
                current: current == Some(index),
            };
            if enabled {
                position += 1;
            }
            place += 1;
            Element::new(row).key(index as u64)
        })
        .collect()
}

/// What the panel of the menu at `path` needs to follow the pointer over `items`: each row's height, its place among
/// the enabled rows and what choosing it does.
fn slots<M: Clone>(
    items: &[MenuItem<M>],
    path: &[usize],
    on_open: &Handler<Vec<usize>, M>,
    on_close: &M,
) -> Vec<Slot<M>> {
    let mut position = 0;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let action = match item {
                MenuItem::Item {
                    enabled: true,
                    on_select,
                    ..
                }
                | MenuItem::Shortcut {
                    enabled: true,
                    on_select,
                    ..
                } => Some(RowAction::Select {
                    on_select: on_select.clone(),
                    on_close: on_close.clone(),
                }),
                MenuItem::Submenu { enabled: true, .. } => {
                    let mut opened = path.to_vec();
                    opened.push(index);
                    Some(RowAction::Open {
                        on_open: on_open(opened),
                        open: false,
                    })
                }
                _ => None,
            };
            let place = action.is_some().then(|| {
                position += 1;
                position - 1
            });
            Slot {
                height: row_height(item),
                position: place,
                action,
            }
        })
        .collect()
}

fn row_height<M>(item: &MenuItem<M>) -> f64 {
    match item {
        MenuItem::Separator => SEPARATOR_HEIGHT,
        _ => ROW_HEIGHT,
    }
}

/// The height of the panel of `items`.
fn height<M>(items: &[MenuItem<M>]) -> f64 {
    let rows: f64 = items.iter().map(row_height).sum();
    (2.0 * PADDING_Y + rows).min(MAX.height)
}

/// The place of item `index` among the enabled items, if it is one.
fn enabled_position<M>(items: &[MenuItem<M>], index: usize) -> Option<usize> {
    let enabled = |item: &MenuItem<M>| {
        matches!(
            item,
            MenuItem::Item { enabled: true, .. }
                | MenuItem::Shortcut { enabled: true, .. }
                | MenuItem::Submenu { enabled: true, .. }
        )
    };
    enabled(items.get(index)?).then(|| items[..index].iter().filter(|i| enabled(i)).count())
}

/// One level of a menu: its rows on the panel, and the messages that close it.
struct Panel<M> {
    label: String,
    /// How many rows the menu holds, separators not counted.
    rows: usize,
    /// The place among the enabled rows of the row that takes the focus when the menu opens.
    current: Option<usize>,
    /// Sent on Escape: closes a submenu, or the whole menu.
    escape: M,
    /// Sent on the arrow toward the leading side in a submenu.
    back: Option<M>,
    /// The rows in order, separators included.
    slots: Vec<Slot<M>>,
    /// The item whose submenu is open, and the submenu's height.
    open: Option<(usize, f64)>,
    /// The window of the open submenu.
    submenu: Option<WindowKey>,
    /// Closes the submenus of this menu.
    collapse: M,
    /// The item over the pointer as the menu opens: a pop-up button's current option.
    start: Option<usize>,
    /// The menu opened at the pointer: the row the pointer is first found on is the one it was on as the menu opened.
    at_pointer: bool,
    /// What was typed to find a row, which the rows share.
    typing: Rc<RefCell<TypeAhead>>,
    /// The choice flashing now.
    flash: Flashing<M>,
    /// The panel's corner radius.
    radius: f64,
    /// The panel is glass of its own, joined to its pointer's ([`MenuTail`]); the window has no background.
    tailed: bool,
    children: Vec<Element<M>>,
}

/// A row of a panel as the pointer sees it.
struct Slot<M> {
    height: f64,
    /// Its place among the enabled rows, if it is one.
    position: Option<usize>,
    /// What choosing it does, if it is enabled.
    action: Option<RowAction<M>>,
}

/// A submenu opening or closing once the pointer rests long enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Open(usize),
    Collapse,
}

#[derive(Default)]
struct PanelState {
    /// The focus came in.
    entered: bool,
    /// When the panel first heard of anything: what press-and-hold times from when the runtime does not know the press
    /// that opened the menu.
    opened: Option<Timestamp>,
    /// A press that went down on this panel.
    press: Option<Tracking>,
    /// The touch the panel follows, as the pointer, and where it last was: one put down on it, or the one that opened
    /// the menu, which it hears move on from the opener. Another finger meanwhile is not followed.
    touch: Option<(i32, Point)>,
    /// The pointer came onto a row that can be chosen, other than the one it was on as the menu opened.
    reached: bool,
    pointer: Option<Point>,
    /// The row, of any kind, the pointer was last found on outside the way into a submenu.
    hovered: Option<usize>,
    /// The pointer has been over the panel.
    visited: bool,
    /// The row, of any kind, the pointer was first found on.
    first: Option<Option<usize>>,
    aim: SubmenuAim,
    pending: Option<(Timestamp, Pending)>,
}

fn seconds(t: Timestamp) -> f64 {
    t.0 as f64 / 1e9
}

fn timestamp(seconds: f64) -> Timestamp {
    Timestamp((seconds * 1e9).ceil().max(0.0) as u64)
}

fn after(t: Timestamp, seconds: f64) -> Timestamp {
    Timestamp(t.0 + (seconds * 1e9).round() as u64)
}

impl<M: Clone + 'static> Panel<M> {
    /// The row under `p`, in window coordinates, with the panel at `bounds`.
    fn row_at(&self, p: Point, bounds: Rect) -> Option<usize> {
        if !bounds.contains(p) {
            return None;
        }
        let mut y = bounds.min_y() + PADDING_Y;
        for (index, slot) in self.slots.iter().enumerate() {
            if p.y >= y && p.y < y + slot.height {
                return Some(index);
            }
            y += slot.height;
        }
        None
    }

    /// The near edge of the open submenu: of the window where the compositor placed it, on whichever side of the panel
    /// it went, or, until it is placed, beside its row on the trailing side.
    fn submenu_edge(
        &self,
        bounds: Rect,
        direction: LayoutDirection,
        placed: Option<Rect>,
    ) -> Option<Edge> {
        let (index, height) = self.open?;
        if let Some(placed) = placed {
            let right =
                placed.min_x() + placed.width() / 2.0 >= bounds.min_x() + bounds.width() / 2.0;
            return Some(Edge {
                x: if right {
                    placed.min_x()
                } else {
                    placed.max_x()
                },
                top: placed.min_y(),
                bottom: placed.max_y(),
            });
        }
        let above: f64 = self.slots[..index.min(self.slots.len())]
            .iter()
            .map(|s| s.height)
            .sum();
        let top = bounds.min_y() + above;
        let x = match direction {
            LayoutDirection::Ltr => bounds.max_x() - SUBMENU_OVERLAP,
            LayoutDirection::Rtl => bounds.min_x() + SUBMENU_OVERLAP,
        };
        Some(Edge {
            x,
            top,
            bottom: top + height,
        })
    }

    /// The pointer is at `p`: unless it heads into the open submenu, the row under it takes the highlight.
    fn pointed(&self, state: &mut PanelState, p: Point, cx: &mut EventCx<'_, M>) {
        let Some(bounds) = cx.bounds() else {
            return;
        };
        state.pointer = Some(p);
        state.visited |= bounds.contains(p);
        let placed = self
            .submenu
            .as_ref()
            .and_then(|key| cx.popup_placement(key));
        let edge = self.submenu_edge(bounds, cx.env().direction, placed);
        if !state.aim.moved((p.x, p.y), seconds(cx.now()), edge) {
            self.hover(state, p, bounds, cx);
        }
        self.schedule(state, cx);
    }

    /// The row under `p` takes the highlight, and its submenu opens, or the open one closes, once the pointer rests.
    /// On a row that cannot be chosen the open submenu closes too; on the panel's padding it stays.
    fn hover(&self, state: &mut PanelState, p: Point, bounds: Rect, cx: &mut EventCx<'_, M>) {
        let row = self.row_at(p, bounds);
        let position = row.and_then(|i| self.slots[i].position);
        let first = *state.first.get_or_insert(row);
        let start = if self.at_pointer { first } else { self.start };
        if position.is_some() && row != start {
            state.reached = true;
        }
        if row == state.hovered {
            return;
        }
        state.hovered = row;
        let Some(index) = row else {
            // off the rows a closing submenu still closes, an opening one does not open
            state.pending = state.pending.filter(|(_, p)| *p == Pending::Collapse);
            return;
        };
        if let Some(position) = position {
            cx.focus_item(FocusTarget::Index(position));
        }
        let now = cx.now();
        let open = self.open.map(|(i, _)| i);
        state.pending = match &self.slots[index].action {
            Some(RowAction::Open { .. }) if open == Some(index) => None,
            Some(RowAction::Open { .. }) => {
                Some((after(now, SUBMENU_OPEN_DELAY), Pending::Open(index)))
            }
            _ if open.is_some() => Some((after(now, SUBMENU_CLOSE_DELAY), Pending::Collapse)),
            _ => None,
        };
    }

    /// Wakes the panel for the earliest of its pending submenu and the end of the pointer's way into one.
    fn schedule(&self, state: &PanelState, cx: &mut EventCx<'_, M>) {
        let aim = state.aim.deadline().map(timestamp);
        let pending = state.pending.map(|(at, _)| at);
        if let Some(at) = aim.into_iter().chain(pending).min() {
            cx.wake_at(at);
        }
    }

    fn woke(&self, state: &mut PanelState, now: Timestamp, cx: &mut EventCx<'_, M>) {
        if state.aim.expired(seconds(now) + 1e-9) {
            if let (Some(p), Some(bounds)) = (state.pointer, cx.bounds()) {
                self.hover(state, p, bounds, cx);
            }
        }
        if let Some((_, pending)) = state.pending.filter(|(at, _)| *at <= now) {
            state.pending = None;
            match pending {
                Pending::Collapse => cx.emit(self.collapse.clone()),
                Pending::Open(index) => {
                    if let Some(RowAction::Open { on_open, .. }) = &self.slots[index].action {
                        cx.emit(on_open.clone());
                    }
                }
            }
        }
        self.schedule(state, cx);
    }

    /// A button went down or up at `p` over this panel.
    fn button(&self, state: &mut PanelState, p: Point, pressed: bool, cx: &mut EventCx<'_, M>) {
        let now = cx.now();
        let row = cx.bounds().and_then(|bounds| self.row_at(p, bounds));
        let action = row.and_then(|i| self.slots[i].action.as_ref());
        if pressed {
            state.press = Some(Tracking::in_menu(seconds(now)));
            if let Some(bounds) = cx.bounds() {
                self.hover(state, p, bounds, cx);
            }
            state.pending = None;
            if let Some(RowAction::Open { on_open, .. }) = action {
                if self.open.map(|(i, _)| i) != row {
                    cx.emit(on_open.clone());
                }
            }
            return;
        }
        let inside = cx.bounds().is_some_and(|bounds| bounds.contains(p));
        // a release without a press here ends the press that opened the menu
        let tracking = state.press.take().unwrap_or_else(|| {
            let pressed = cx.opening_press().or(state.opened).unwrap_or(now);
            let mut tracking = Tracking::from_title(seconds(pressed));
            if state.reached {
                tracking.reach();
            }
            tracking
        });
        if !inside {
            // the press was let go away from every menu: dragged through the menus, it chooses nothing and closes
            // them, which the outermost one does; one that never came over the menus leaves them open
            let close = tracking.release(seconds(now), Under::Outside) == Release::Close;
            if close && state.visited && self.back.is_none() {
                cx.emit(self.escape.clone());
            }
            return;
        }
        // let go on a panel, a release never closes the menu without choosing
        let under = match action {
            Some(RowAction::Select { .. }) => Under::Item,
            _ => Under::Menu,
        };
        match (tracking.release(seconds(now), under), action) {
            (
                Release::Choose,
                Some(RowAction::Select {
                    on_select,
                    on_close,
                }),
            ) => {
                if let Some(row) = row {
                    choose(
                        &self.flash,
                        row,
                        true,
                        vec![on_select.clone(), on_close.clone()],
                        cx,
                    );
                }
            }
            (_, Some(RowAction::Open { on_open, .. })) if self.open.map(|(i, _)| i) != row => {
                state.pending = None;
                cx.emit(on_open.clone());
            }
            _ => {}
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Panel<M> {
    type State = PanelState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::new(PADDING_Y, 0.0, PADDING_Y, 0.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> PanelState {
        PanelState::default()
    }

    fn update(&self, old: &Self, _: &mut PanelState) -> Change {
        if !Rc::ptr_eq(&self.typing, &old.typing) {
            let typed = old.typing.borrow().clone();
            *self.typing.borrow_mut() = typed;
        }
        if self.label != old.label || self.radius != old.radius {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &PanelState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, state: &mut PanelState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        state.opened.get_or_insert(cx.now());
        // a choice that flashes is made: the menu takes no more of the pointer or the keys
        if cx.phase() == Phase::Capture
            && self.flash.borrow().is_some()
            && matches!(
                event,
                Event::Pointer(PointerEvent::Button { .. } | PointerEvent::Motion { .. }, _)
                    | Event::Touch(..)
                    | Event::Key(..)
            )
        {
            return Propagation::Stop;
        }
        // the panel follows the pointer over all its rows: as they hear it, or over its own padding
        let mine = cx.phase() == Phase::Capture || cx.node() == cx.target();
        match *event {
            Event::Pointer(PointerEvent::Enter { position }, _) if cx.node() == cx.target() => {
                self.pointed(state, position, cx);
                return Propagation::Continue;
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if mine => {
                self.pointed(state, position, cx);
                return Propagation::Continue;
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.pointer = None;
                state.hovered = None;
                state.pending = None;
                state.aim.reset();
                return Propagation::Continue;
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left | MouseButton::Right,
                    pressed,
                    ..
                },
                _,
            ) if mine => {
                self.button(state, position, pressed, cx);
                return Propagation::Stop;
            }
            Event::Wake(now) if cx.node() == cx.target() => {
                if !flash_woke(&self.flash, |f| f.by_panel, now, cx) {
                    self.woke(state, now, cx);
                }
                return Propagation::Stop;
            }
            // a finger is the pointer while it is down: put down on a row it presses it, moved it points, lifted it
            // lets go where it last was
            Event::Touch(TouchEvent::Down { id, position }, _) if mine => {
                if state.touch.is_none() {
                    state.touch = Some((id, position));
                    self.button(state, position, true, cx);
                }
                return Propagation::Stop;
            }
            Event::Touch(TouchEvent::Motion { id, position }, _) if mine => {
                // a touch not seen going down is the one that opened the menu, coming on from its opener
                if state.touch.is_none_or(|(followed, _)| followed == id) {
                    state.touch = Some((id, position));
                    self.pointed(state, position, cx);
                }
                return Propagation::Stop;
            }
            Event::Touch(TouchEvent::Up { id }, _) if mine => {
                if let Some((_, at)) = state.touch.filter(|(followed, _)| *followed == id) {
                    state.touch = None;
                    self.button(state, at, false, cx);
                }
                return Propagation::Stop;
            }
            // the touch was taken away: its press ends with nothing chosen, and the menu stays as it is
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel if state.touch.is_some() => {
                state.touch = None;
                state.press = None;
                return Propagation::Stop;
            }
            _ => {}
        }
        if let Event::FocusWithin { gained: true, .. } = event {
            if !std::mem::replace(&mut state.entered, true) {
                if let Some(index) = self.current {
                    cx.focus_item(FocusTarget::Index(index));
                }
            }
            return Propagation::Continue;
        }
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || !key.mods.is_empty() || cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let leading = match cx.env().direction {
            LayoutDirection::Ltr => LEFT,
            LayoutDirection::Rtl => RIGHT,
        };
        let message = match key.keysym {
            ESCAPE => Some(&self.escape),
            k if k == leading => self.back.as_ref(),
            _ => None,
        };
        match message {
            Some(message) => {
                cx.emit(message.clone());
                Propagation::Stop
            }
            None => Propagation::Continue,
        }
    }

    /// The window paints the glass or, without the compositor's material, the opaque fill under it. The compositor
    /// draws the glass's edge; over the opaque fill the panel draws its own.
    fn paint(&self, _: &PanelState, cx: &mut PaintCx<'_>) {
        if self.tailed {
            let size = cx.size();
            let rect = Rect::new(0.0, 0.0, size.width, size.height);
            if cx.glass(tail_glass().radius(self.radius)) {
                return;
            }
            // without the compositor's material the panel paints the menu's opaque fill itself
            let fill = cx.env().theme.surface(Surface::Menu).fill;
            look::fill(cx, rect, self.radius, fill);
        } else if cx.window_backdrop() {
            return;
        }
        let size = cx.size();
        let theme = &cx.env().theme;
        let edge = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            self.radius - EDGE / 2.0,
            EDGE,
            edge,
        );
    }

    fn semantics(&self, _: &PanelState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Menu);
        cx.name(self.label.clone());
        cx.item_count(self.rows);
        cx.focus_scope();
        cx.roving_group();
    }
}

#[derive(Clone)]
enum RowAction<M> {
    Select { on_select: M, on_close: M },
    Open { on_open: M, open: bool },
}

struct Row<M> {
    label: String,
    /// The keys that choose the row, shown at its trailing edge.
    shortcut: Option<String>,
    enabled: bool,
    checked: bool,
    action: RowAction<M>,
    columns: Columns,
    /// The labels of the menu's enabled rows, in order, for typing to move.
    labels: Rc<[String]>,
    /// What was typed so far, shared by the menu's rows.
    typing: Rc<RefCell<TypeAhead>>,
    /// The choice flashing now, which this row's may be.
    flash: Flashing<M>,
    /// This row's place among all the menu's items.
    index: usize,
    /// This row's place among them, if enabled.
    position: usize,
    /// This row's place among all the menu's rows, separators not counted, which a screen reader reads out.
    place: usize,
    /// Highlighted while it has the focus, also before the pointer or a key moves it.
    current: bool,
}

#[derive(Default)]
struct RowState {
    press: Press,
    /// The pointer moved onto the row.
    pointed: bool,
}

impl<M> Row<M> {
    fn open(&self) -> bool {
        matches!(self.action, RowAction::Open { open: true, .. })
    }

    /// Whether the row shows as the menu's selection when it has the focus: a menu opened with the pointer focuses
    /// its first row but selects nothing until the pointer or a key moves.
    fn selected(&self, state: &RowState, focus_visible: bool) -> bool {
        self.open() || focus_visible || state.pointed || self.current
    }
}

impl<M: Clone + 'static> Row<M> {
    fn activate(&self, cx: &mut EventCx<'_, M>) {
        match &self.action {
            RowAction::Select {
                on_select,
                on_close,
            } => choose(
                &self.flash,
                self.index,
                false,
                vec![on_select.clone(), on_close.clone()],
                cx,
            ),
            RowAction::Open { on_open, .. } => cx.emit(on_open.clone()),
        }
    }

    /// The enabled row that what was typed, with `c` typed at `now`, finds: one character the next row it starts
    /// after this one (from this one when it is not `selected`), more characters the first row from this one that
    /// they start, wrapping around.
    fn typed(&self, c: char, selected: bool, now: f64) -> Option<usize> {
        let count = self.labels.len();
        let titles: Vec<Option<&str>> = self.labels.iter().map(|l| Some(l.as_str())).collect();
        let from = if selected {
            self.position
        } else {
            (self.position + count - 1) % count
        };
        self.typing.borrow_mut().find(c, now, from, &titles)
    }

    fn key(
        &self,
        key: &cupertino_ui_platform::KeyEvent,
        selected: bool,
        cx: &mut EventCx<'_, M>,
    ) -> bool {
        let trailing = match cx.env().direction {
            LayoutDirection::Ltr => RIGHT,
            LayoutDirection::Rtl => LEFT,
        };
        let target = match key.keysym {
            RETURN | SPACE if !selected => return true,
            DOWN if !selected => FocusTarget::First,
            UP if !selected => FocusTarget::Last,
            DOWN => FocusTarget::Next,
            UP => FocusTarget::Previous,
            HOME => FocusTarget::First,
            END => FocusTarget::Last,
            k if k == trailing && selected && matches!(self.action, RowAction::Open { .. }) => {
                self.activate(cx);
                return true;
            }
            _ => {
                let plain = !key.mods.contains(Mods::CTRL)
                    && !key.mods.contains(Mods::ALT)
                    && !key.mods.contains(Mods::PRIMARY);
                match key.text.filter(|c| plain && !c.is_control() && *c != ' ') {
                    Some(c) => match self.typed(c, selected, seconds(cx.now())) {
                        Some(index) => FocusTarget::Index(index),
                        None => return true,
                    },
                    None => return false,
                }
            }
        };
        cx.focus_item(target);
        true
    }
}

/// Draws the symbol `name`, `SYMBOL` points square, centred at `centre`.
fn symbol(cx: &mut PaintCx<'_>, name: &str, centre: Point, color: Option<Color>, mirrored: bool) {
    let Some(color) = color else {
        return;
    };
    let scale = cx.env().scale;
    let px = (SYMBOL * scale).round().max(1.0) as u32;
    let icon = cx.env().icons.symbol(name, px, color, mirrored);
    let (width, height) = (
        f64::from(icon.image.width) / scale,
        f64::from(icon.image.height) / scale,
    );
    let dst = Rect::new(
        centre.x - width / 2.0,
        centre.y - height / 2.0,
        width,
        height,
    );
    cx.image(icon.id, &icon.image, dst, Sampling::Linear);
}

impl<M: Clone + 'static> Widget<M> for Row<M> {
    type State = RowState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> RowState {
        RowState::default()
    }

    fn update(&self, old: &Self, _: &mut RowState) -> Change {
        if self.label != old.label
            || self.shortcut != old.shortcut
            || self.columns != old.columns
            || matches!(self.action, RowAction::Open { .. })
                != matches!(old.action, RowAction::Open { .. })
        {
            Change::Layout
        } else if self.enabled != old.enabled
            || self.checked != old.checked
            || self.current != old.current
            || self.open() != old.open()
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &RowState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let label = look::label(cx.env(), &self.label).map_or(0.0, |l| l.size().width);
        let shortcut = self
            .shortcut
            .as_deref()
            .and_then(|s| look::label(cx.env(), s))
            .map_or(0.0, |l| l.size().width);
        // the shortcuts and chevrons keep a column at the trailing edge when the menu has any
        let trailing = if self.columns.trailing {
            SHORTCUT_GAP + shortcut.max(CHEVRON_WIDTH)
        } else {
            0.0
        };
        let width = self.columns.label + label + trailing + TRAILING;
        Measured {
            size: Size::new(width.max(self.columns.min_width), ROW_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, state: &mut RowState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // the panel moves the focus to the row the pointer is on
            Event::Pointer(PointerEvent::Enter { .. }, _)
                if self.enabled && cx.node() == cx.target() =>
            {
                state.pointed = true;
                cx.request_paint();
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.pointed = false;
                cx.request_paint();
            }
            Event::Focus { .. } if cx.node() == cx.target() => cx.request_paint(),
            Event::Wake(now)
                if cx.node() == cx.target()
                    && flash_woke(
                        &self.flash,
                        |f| !f.by_panel && f.row == self.index,
                        *now,
                        cx,
                    ) =>
            {
                return Propagation::Stop;
            }
            Event::AnimationFrame(_) if cx.node() == cx.target() => cx.request_paint(),
            Event::Key(key, _)
                if self.enabled
                    && key.pressed
                    && cx.node() == cx.target()
                    && cx.phase() != Phase::Capture
                    && self.key(key, self.selected(state, cx.focus_visible()), cx) =>
            {
                return Propagation::Stop;
            }
            _ => {}
        }
        match state
            .press
            .event(event, self.enabled, Keys::SpaceAndReturn, cx)
        {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                self.activate(cx);
                Propagation::Stop
            }
        }
    }

    fn paint(&self, state: &RowState, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let size = cx.size();
        let mut highlighted = self.enabled
            && (self.open() || cx.is_focused() && self.selected(state, cx.focus_visible()));
        // a choice flashes: its row's highlight goes off, then on again, until the choice is sent
        if let Some(flash) = self.flash.borrow().as_ref() {
            cx.request_animation_frame();
            if flash.row == self.index {
                highlighted = flash.lit(cx.now());
            }
        }
        if highlighted {
            let rect = Rect::new(
                HIGHLIGHT_INSET,
                0.0,
                size.width - 2.0 * HIGHLIGHT_INSET,
                size.height,
            );
            look::fill(
                cx,
                rect,
                HIGHLIGHT_RADIUS,
                theme
                    .colors
                    .get(ColorRole::SelectedControlFill, StateVariant::NORMAL),
            );
        }
        let role = if !self.enabled {
            ColorRole::DisabledControlText
        } else if highlighted {
            ColorRole::SelectedControlText
        } else {
            ColorRole::Text
        };
        let color = theme.colors.get(role, StateVariant::NORMAL);
        let rtl = env.direction == LayoutDirection::Rtl;
        let leading = |x: f64, width: f64| if rtl { size.width - x - width } else { x };
        let middle = size.height / 2.0;
        if self.checked {
            let x = leading(MARK_CENTRE, 0.0);
            symbol(cx, "symbol.checkmark", Point::new(x, middle), color, false);
        }
        if let Some(label) = look::label(&env, &self.label) {
            let width = label.size().width;
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let origin = Point::new(
                leading(self.columns.label, width),
                look::top(&label, row, &env),
            );
            look::glyphs(cx, &label, origin, color);
        }
        if let Some(shortcut) = self.shortcut.as_deref().and_then(|s| look::label(&env, s)) {
            // the keys keep to the trailing edge, in the secondary colour unless the row is highlighted or disabled
            let keys = match (self.enabled, highlighted) {
                (_, true) => color,
                (true, false) => color.map(|c| Color {
                    a: c.a * SHORTCUT_ALPHA,
                    ..c
                }),
                (false, false) => color.map(|c| Color {
                    a: c.a * DISABLED_SHORTCUT_ALPHA,
                    ..c
                }),
            };
            let width = shortcut.size().width;
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let origin = Point::new(
                leading(size.width - TRAILING - width, width),
                look::top(&shortcut, row, &env),
            );
            look::glyphs(cx, &shortcut, origin, keys);
        }
        if matches!(self.action, RowAction::Open { .. }) {
            let x = leading(size.width - TRAILING - CHEVRON_WIDTH / 2.0, 0.0);
            symbol(
                cx,
                "symbol.chevron.right",
                Point::new(x, middle),
                color,
                rtl,
            );
        }
    }

    fn semantics(&self, _: &RowState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::MenuItem);
        cx.name(self.label.clone());
        if let Some(shortcut) = &self.shortcut {
            cx.description(shortcut.clone());
        }
        cx.item_index(self.place);
        if self.checked {
            cx.value(AccessibleValue::Toggle(Checked::True));
        }
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: matches!(self.action, RowAction::Open { .. }).then(|| self.open()),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.roving_item();
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// A line between groups of rows, under the labels: from the label column to near the trailing edge.
struct Separator {
    /// From the leading edge to the menu's labels.
    leading: f64,
}

impl<M: 'static> Widget<M> for Separator {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.leading != old.leading {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, SEPARATOR_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let appearance = theme.preferences.appearance;
        // over the glass a translucent line that takes its colour; over the opaque fill and with Increase Contrast the
        // theme's separator
        let color = if cx.window_backdrop() && !appearance.is_high_contrast() {
            Some(if appearance.is_dark() {
                GLASS_SEPARATOR_DARK
            } else {
                GLASS_SEPARATOR_LIGHT
            })
        } else {
            theme.colors.get(ColorRole::Separator, StateVariant::NORMAL)
        };
        let Some(color) = color else {
            return;
        };
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let length = (size.width - self.leading - SEPARATOR_TRAILING).max(0.0);
        let x = if rtl {
            SEPARATOR_TRAILING
        } else {
            self.leading
        };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(x, (size.height - EDGE) / 2.0, length, EDGE)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Separator);
    }
}

/// How long a finger rests on a context menu's region before its menu opens there, in seconds: a finger has no
/// secondary button. As long as a press is held before a menu tracks it as held.
const TOUCH_HOLD: f64 = TRACKING_INTERVAL;

/// How far, in points, a resting finger may stray before it is a drag or a scroll rather than a hold.
const TOUCH_SLOP: f64 = 8.0;

/// A context menu's region: the finger resting on it, if one is, with where it went down and when its rest opens the
/// menu. Opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextMenuState {
    hold: Option<(i32, Point, Timestamp)>,
}

impl<M: Clone + 'static> Widget<M> for ContextMenu<M> {
    type State = ContextMenuState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![std::mem::replace(
            &mut self.content,
            Element::new(crate::Stack {
                axis: Axis::Vertical,
                padding: Insets::uniform(0.0),
                spacing: 0.0,
                align: Align::Start,
                children: Vec::new(),
            }),
        )]
    }

    fn state(&self) -> ContextMenuState {
        ContextMenuState::default()
    }

    fn update(&self, _: &Self, _: &mut ContextMenuState) -> Change {
        Change::None
    }

    fn measure(&self, _: &ContextMenuState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut ContextMenuState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let Some(on_open) = &self.on_open else {
            return Propagation::Continue;
        };
        // a finger that rests on the region opens the menu where it went down, and the menu follows it from there; it
        // is watched on its way in, so the content under it still takes it as a press
        match *event {
            Event::Touch(TouchEvent::Down { id, position }, _)
                if cx.phase() == Phase::Capture && state.hold.is_none() =>
            {
                let at = after(cx.now(), TOUCH_HOLD);
                state.hold = Some((id, position, at));
                cx.wake_at(at);
            }
            Event::Touch(TouchEvent::Motion { id, position }, _)
                if cx.phase() == Phase::Capture =>
            {
                if let Some((_, from, _)) = state.hold.filter(|(held, ..)| *held == id) {
                    if (position.x - from.x).hypot(position.y - from.y) > TOUCH_SLOP {
                        state.hold = None;
                    }
                }
            }
            Event::Touch(TouchEvent::Up { id }, _) if cx.phase() == Phase::Capture => {
                state.hold = state.hold.filter(|(held, ..)| *held != id);
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => state.hold = None,
            Event::Wake(now) if cx.node() == cx.target() => {
                if let Some((_, at, _)) = state.hold.filter(|(.., due)| *due <= now) {
                    state.hold = None;
                    cx.emit(on_open(at));
                    return Propagation::Stop;
                }
            }
            _ => {}
        }
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let at = match event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => Some(*position),
            Event::Key(key, _)
                if key.pressed
                    && (key.keysym == MENU && key.mods.is_empty()
                        || key.keysym == F10 && key.mods == Mods::SHIFT) =>
            {
                cx.bounds().map(|b| match cx.env().direction {
                    LayoutDirection::Ltr => b.origin,
                    LayoutDirection::Rtl => Point::new(b.max_x(), b.min_y()),
                })
            }
            _ => None,
        };
        match at {
            Some(at) => {
                cx.emit(on_open(at));
                Propagation::Stop
            }
            None => Propagation::Continue,
        }
    }

    fn paint(&self, _: &ContextMenuState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &ContextMenuState, _: &mut SemanticsCx<'_>) {}
}

impl<M: Clone + 'static> PopUpButton<M> {
    /// The menu of the options over the button, which has the key `anchor` in `parent`, the chosen one's label on the
    /// button's: choosing one sends `on_select` with its index, and the menu closes with `on_close`.
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        let items = self
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| MenuItem::Item {
                label: label.clone(),
                enabled: true,
                checked: index == self.selected,
                on_select: on_select(index),
            })
            .collect();
        let close = on_close.clone();
        Menu {
            parent,
            anchor: MenuAnchor::Over(anchor),
            label: self.label.clone(),
            items,
            current: Some(self.selected),
            open: Vec::new(),
            on_open: Box::new(move |_| close.clone()),
            on_close,
        }
    }

    fn chosen(&self) -> &str {
        self.options.get(self.selected).map_or("", String::as_str)
    }
}

impl<M: Clone + 'static> Widget<M> for PopUpButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.options != old.options {
            Change::Layout
        } else if self.selected != old.selected
            || self.enabled != old.enabled
            || self.open != old.open
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    /// Wide enough for the widest option, so choosing another never moves what follows.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        menu_button_measure(cx.env(), &self.options)
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        menu_button_event(press, event, cx, self.enabled, self.on_open.as_ref())
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        paint_menu_button(press, cx, self.enabled, self.open, self.chosen(), false);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Text {
            value: self.chosen().to_owned(),
            editing: None,
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.open),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}

/// A pop-up or pull-down button's size: wide enough for the widest of `labels`, so another never moves what follows.
fn menu_button_measure(env: &Environment, labels: &[String]) -> Measured {
    let metrics = env.theme.metrics;
    let widest = labels
        .iter()
        .filter_map(|o| look::label(env, o))
        .map(|l| l.size().width)
        .fold(0.0, f64::max);
    let height = look::geometry(&env.theme, ControlFamily::PopUp).height;
    let width = LABEL_INSET + widest + CHEVRONS_GAP + SYMBOL + LABEL_INSET;
    Measured {
        size: Size::new(
            width.max(metrics.min_target.width),
            height.max(metrics.min_target.height),
        ),
        baseline: None,
    }
}

/// A pop-up or pull-down button's events: its menu opens on a press or a finger put down, so the pointer or the finger
/// can drag to a row and let go there, and on Space, Return, Up or Down.
fn menu_button_event<M: Clone + 'static>(
    press: &mut Press,
    event: &Event,
    cx: &mut EventCx<'_, M>,
    enabled: bool,
    on_open: Option<&M>,
) -> Propagation {
    let Some(on_open) = on_open.filter(|_| enabled) else {
        return Propagation::Continue;
    };
    if let Event::Key(key, _) = event {
        if key.pressed
            && (key.keysym == UP || key.keysym == DOWN)
            && key.mods.is_empty()
            && cx.node() == cx.target()
        {
            cx.emit(on_open.clone());
            return Propagation::Stop;
        }
    }
    // the menu opens on the press, so the pointer can drag to an option and let go there
    let pressed = cx.phase() != Phase::Capture && opens_on_press(event, press);
    if pressed {
        cx.emit(on_open.clone());
    }
    match press.event(event, true, Keys::SpaceAndReturn, cx) {
        Outcome::Ignored if pressed => Propagation::Stop,
        Outcome::Ignored => Propagation::Continue,
        Outcome::Changed
        | Outcome::Activated(ActivationSource::Pointer | ActivationSource::Touch) => {
            cx.request_paint();
            Propagation::Stop
        }
        Outcome::Activated(_) => {
            cx.request_paint();
            cx.emit(on_open.clone());
            Propagation::Stop
        }
    }
}

/// Whether `event` is a press that opens a pop-up or pull-down button's menu, or a combo box's: the primary button
/// going down, or a finger put down while no other holds the control (`press`, before it hears the event).
pub(crate) fn opens_on_press(event: &Event, press: &Press) -> bool {
    match event {
        Event::Pointer(
            PointerEvent::Button {
                button: MouseButton::Left,
                pressed: true,
                ..
            },
            _,
        ) => true,
        Event::Touch(TouchEvent::Down { .. }, _) => !press.armed(),
        _ => false,
    }
}

/// A pop-up or pull-down button: an ordinary button's quiet bezel with `text_label` and opposing chevrons, or a
/// single downward one when `pull_down`.
fn paint_menu_button(
    press: &Press,
    cx: &mut PaintCx<'_>,
    enabled: bool,
    open: bool,
    text_label: &str,
    pull_down: bool,
) {
    let env = cx.env().clone();
    let theme = &env.theme;
    let state = ControlState {
        enabled,
        hovered: press.hovered(),
        pressed: press.pressed() || open,
        focused: cx.is_focused(),
        focus_visible: cx.focus_visible(),
        window_active: cx.window_active(),
        ..ControlState::default()
    };
    let size = cx.size();
    let geometry = look::geometry(theme, ControlFamily::PopUp);
    let height = geometry.height;
    let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
    let radius = geometry.corners.radius(height);
    // an ordinary button's quiet bezel
    let bezel = ButtonLook::of(ButtonRole::Normal, ButtonStyle::Bordered, theme, &state);
    look::fill(cx, chrome, radius, bezel.fill);
    look::stroke(
        cx,
        chrome.inset(Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        bezel.border,
    );
    let text = bezel.text;
    let inset = LABEL_INSET;
    let rtl = env.direction == LayoutDirection::Rtl;
    if let Some(label) = look::label(&env, text_label) {
        let width = label.size().width;
        let x = if rtl {
            chrome.max_x() - inset - width
        } else {
            chrome.min_x() + inset
        };
        look::glyphs(
            cx,
            &label,
            Point::new(x, look::top(&label, chrome, &env)),
            text,
        );
    }
    let x = if rtl {
        chrome.min_x() + inset + SYMBOL / 2.0
    } else {
        chrome.max_x() - inset - SYMBOL / 2.0
    };
    // opposing chevrons: a choice among values; a pull-down's single one points down at its actions
    if let Some(color) = text {
        let mid = chrome.min_y() + chrome.height() / 2.0;
        let chevrons: &[bool] = if pull_down { &[false] } else { &[true, false] };
        for &up in chevrons {
            let offset = match (pull_down, up) {
                (true, _) => 0.0,
                (false, true) => -POPUP_CHEVRON_GAP,
                (false, false) => POPUP_CHEVRON_GAP,
            };
            cx.push(PaintOp::Stroke {
                shape: look::chevron(Point::new(x, mid + offset), POPUP_CHEVRON, up),
                width: POPUP_CHEVRON_STROKE,
                paint: Paint::Solid(color),
            });
        }
    }
    paint_focus_ring(cx, chrome, radius);
}

impl<M: Clone + 'static> InlinePopUpButton<M> {
    /// The menu of the options over the button ([`PopUpButton::menu`]).
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        self.0.menu(parent, anchor, on_select, on_close)
    }
}

impl<M: Clone + 'static> Widget<M> for InlinePopUpButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, state: &mut Press) -> Change {
        self.0.update(&old.0, state)
    }

    /// Wide enough for the widest option and the disc, so choosing another never moves what follows.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let env = cx.env();
        let metrics = env.theme.metrics;
        let widest = self
            .0
            .options
            .iter()
            .filter_map(|o| look::label(env, o))
            .map(|l| l.size().width)
            .fold(0.0, f64::max);
        let height = look::geometry(&env.theme, ControlFamily::PopUp).height;
        Measured {
            size: Size::new(
                (widest + INLINE_GAP + INLINE_DISC).max(metrics.min_target.width),
                height.max(metrics.min_target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.0.event(press, event, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let popup = &self.0;
        let state = ControlState {
            enabled: popup.enabled,
            hovered: press.hovered(),
            pressed: press.pressed() || popup.open,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let height = look::geometry(theme, ControlFamily::PopUp).height;
        let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
        let rtl = env.direction == LayoutDirection::Rtl;
        let disc = Rect::new(
            if rtl {
                chrome.min_x()
            } else {
                chrome.max_x() - INLINE_DISC
            },
            chrome.min_y() + (chrome.height() - INLINE_DISC) / 2.0,
            INLINE_DISC,
            INLINE_DISC,
        );
        look::fill(
            cx,
            disc,
            INLINE_DISC / 2.0,
            Some(look::quiet(theme, look::DISC, &state)),
        );
        let bezel = ButtonLook::of(ButtonRole::Normal, ButtonStyle::Bordered, theme, &state);
        if theme.preferences.appearance.is_high_contrast() {
            look::stroke(
                cx,
                disc.inset(Insets::uniform(0.5)),
                INLINE_DISC / 2.0 - 0.5,
                1.0,
                bezel.border,
            );
        }
        let text = bezel.text;
        // the value, against the disc
        if let Some(label) = look::label(&env, popup.chosen()) {
            let width = label.size().width;
            let x = if rtl {
                disc.max_x() + INLINE_GAP
            } else {
                disc.min_x() - INLINE_GAP - width
            };
            look::glyphs(
                cx,
                &label,
                Point::new(x, look::top(&label, chrome, &env)),
                text,
            );
        }
        if let Some(color) = text {
            let mid = disc.min_y() + disc.height() / 2.0;
            let x = disc.min_x() + disc.width() / 2.0;
            for up in [true, false] {
                let offset = if up {
                    -INLINE_CHEVRON_GAP
                } else {
                    INLINE_CHEVRON_GAP
                };
                cx.push(PaintOp::Stroke {
                    shape: look::chevron(Point::new(x, mid + offset), INLINE_CHEVRON, up),
                    width: POPUP_CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            }
        }
        paint_focus_ring(cx, chrome, height / 2.0);
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.0.semantics(press, cx);
    }
}

impl<M: Clone + 'static> PullDownButton<M> {
    /// The menu of the actions below the button, which has the key `anchor` in `parent`: choosing one sends
    /// `on_select` with its index, and the menu closes with `on_close`. No action is marked: none is chosen for good.
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        let items = self
            .actions
            .iter()
            .enumerate()
            .map(|(index, label)| MenuItem::Item {
                label: label.clone(),
                enabled: true,
                checked: false,
                on_select: on_select(index),
            })
            .collect();
        let close = on_close.clone();
        Menu {
            parent,
            anchor: MenuAnchor::Below(anchor),
            label: self.label.clone(),
            items,
            current: None,
            open: Vec::new(),
            on_open: Box::new(move |_| close.clone()),
            on_close,
        }
    }
}

impl<M: Clone + 'static> Widget<M> for PullDownButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.label != old.label {
            Change::Layout
        } else if self.enabled != old.enabled || self.open != old.open {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        menu_button_measure(cx.env(), std::slice::from_ref(&self.label))
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        menu_button_event(press, event, cx, self.enabled, self.on_open.as_ref())
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        paint_menu_button(press, cx, self.enabled, self.open, &self.label, true);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.open),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}

/// The container the panel and its pointer share as glass, and how far their outlines blend into each other.
const TAIL_CONTAINER: u32 = 1;
const TAIL_BLEND: f64 = 8.0;

fn tail_glass() -> Glass {
    Glass::regular().container(TAIL_CONTAINER, TAIL_BLEND)
}

/// The content of a menu window with a pointer: the panel, and under it the row that holds the pointer.
struct Callout<M> {
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Callout<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
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

/// The row under the panel that holds its pointer at [`MenuTail::from_leading`].
struct TailRow {
    tail: MenuTail,
}

impl<M: Clone + 'static> Widget<M> for TailRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::new(
                0.0,
                self.tail.from_leading - self.tail.width / 2.0,
                0.0,
                0.0,
            ),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![CalloutPointer {
            width: self.tail.width,
            depth: self.tail.depth,
            glass: tail_glass(),
            surface: Surface::Menu,
            fills_without_glass: true,
        }
        .element()
        .key("shape")]
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.tail != old.tail {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
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
