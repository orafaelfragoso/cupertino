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
use crate::typeahead::TypeAhead;
use crate::{
    ButtonRole, ButtonStyle, CalloutPointer, ContextMenu, Handler, InlinePopUpButton, Menu,
    MenuAnchor, MenuItem, MenuStyle, MenuTail, PopUpButton, PullDownButton,
};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_menu_model::tracking::{
    Edge, Release, SubmenuAim, Tracking, Under, SUBMENU_OPEN_DELAY, TRACKING_INTERVAL,
};
use cupertino_theme::ui::{Color, ColorRole, StateVariant, Surface};
use cupertino_theme::ControlFamily;
use cupertino_ui_core::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_core::platform::{
    DecorationMode, Keysym, Mods, MouseButton, PointerEvent, Timestamp, TouchEvent, WindowSpec,
};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActivationSource, Change, Checked, CommandRegistry,
    ControlState, Element, Environment, Event, EventCx, FocusTarget, Glass, Key, MeasureCx,
    PaintCx, Phase, Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_render::{Paint, PaintOp, Sampling, Shape};
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
const TRAILING: f64 = 19.0;
/// From the trailing edge to the end of a separator line; it starts under the labels.
const SEPARATOR_TRAILING: f64 = 16.5;
/// The middle of the shortcut key column and of a submenu chevron, from the trailing edge.
const KEY_CENTRE: f64 = 24.0;
/// Where the modifier symbols of a shortcut end, from the trailing edge.
const KEY_CELL_LEADING: f64 = 32.5;
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
/// A menu with a pointer ([`MenuStyle::pointed`]): its panel's corner radius and least width, and its pointer's width
/// where it leaves the panel and its depth below it.
pub(crate) const POINTED_RADIUS: f64 = 14.5;
pub(crate) const POINTED_MIN_WIDTH: f64 = 110.0;
pub(crate) const TAIL_WIDTH: f64 = 18.0;
pub(crate) const TAIL_DEPTH: f64 = 10.0;
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

/// What a menu sends as a row is chosen, a submenu opens or it closes: a message of the application's, or, for a menu a
/// control opened for itself ([`EventCx::open_popup`]), the row's number to that control and the popup's end.
#[derive(Clone)]
pub(crate) enum Out<M> {
    Message(M),
    Answer(u32),
    Close,
    /// Nothing: such a menu has no submenus to open.
    Nothing,
}

impl<M: Clone> Out<M> {
    fn send(&self, cx: &mut EventCx<'_, M>) {
        match self {
            Out::Message(message) => cx.emit(message.clone()),
            Out::Answer(row) => cx.answer_popup(*row),
            Out::Close => cx.close_popup(),
            Out::Nothing => {}
        }
    }
}

/// `items` as the menu's levels hold them.
fn outward<M>(items: Vec<MenuItem<M>>) -> Vec<MenuItem<Out<M>>> {
    items
        .into_iter()
        .map(|item| match item {
            MenuItem::Item {
                label,
                enabled,
                checked,
                on_select,
            } => MenuItem::Item {
                label,
                enabled,
                checked,
                on_select: Out::Message(on_select),
            },
            MenuItem::Shortcut {
                label,
                shortcut,
                enabled,
                checked,
                on_select,
            } => MenuItem::Shortcut {
                label,
                shortcut,
                enabled,
                checked,
                on_select: Out::Message(on_select),
            },
            MenuItem::Submenu {
                label,
                enabled,
                items,
            } => MenuItem::Submenu {
                label,
                enabled,
                items: outward(items),
            },
            MenuItem::Separator => MenuItem::Separator,
        })
        .collect()
}

/// A row that was chosen, flashing before its messages are sent: its highlight is off for [`FLASH_OFF`], then on for
/// [`FLASH_ON`]. The menu hears nothing else meanwhile.
struct Flash<M> {
    /// The row's place among the menu's items.
    row: usize,
    start: Timestamp,
    /// The panel sends the messages, not the row: the choice was made on the panel.
    by_panel: bool,
    then: Vec<Out<M>>,
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
    then: Vec<Out<M>>,
    cx: &mut EventCx<'_, M>,
) {
    if flash.borrow().is_some() {
        return;
    }
    if cx.env().theme.preferences.reduce_motion {
        for message in then {
            message.send(cx);
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
                message.send(cx);
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
            items,
            current,
            open,
            on_open,
            on_close,
        } = self;
        let dismiss = on_close.clone();
        levels(
            Levels {
                parent,
                anchor,
                label,
                items: outward(items),
                current,
                open,
                on_open: &|path| Out::Message(on_open(path)),
                on_close: Out::Message(on_close),
            },
            key,
            style,
        )
        .into_iter()
        .map(|level| Window {
            key: level.key,
            spec: level.spec,
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: level.background,
            content: level.content,
            popup: Some(Popup {
                parent: level.parent,
                anchor: level.anchor,
                grab: true,
                on_dismiss: dismiss.clone(),
            }),
            menus: None,
        })
        .collect()
    }
}

/// A menu as its levels are built from it: a [`Menu`] with what it sends as [`Out`].
pub(crate) struct Levels<'a, M> {
    pub(crate) parent: WindowKey,
    pub(crate) anchor: MenuAnchor,
    pub(crate) label: String,
    pub(crate) items: Vec<MenuItem<Out<M>>>,
    pub(crate) current: Option<usize>,
    pub(crate) open: Vec<usize>,
    pub(crate) on_open: &'a dyn Fn(Vec<usize>) -> Out<M>,
    pub(crate) on_close: Out<M>,
}

/// One popup window of a menu: the menu itself or an open submenu, and where it opens in which window.
pub(crate) struct Level<M> {
    pub(crate) key: WindowKey,
    pub(crate) spec: WindowSpec,
    pub(crate) background: WindowBackground,
    pub(crate) content: Element<M>,
    pub(crate) parent: WindowKey,
    pub(crate) anchor: PopupAnchor,
}

/// The popup windows that show `menu` and its open submenus, with panels of the corner radius and least width of
/// `style`.
pub(crate) fn levels<M: Clone + 'static>(
    menu: Levels<'_, M>,
    key: WindowKey,
    style: MenuStyle,
) -> Vec<Level<M>> {
    let Levels {
        parent,
        anchor,
        label,
        mut items,
        current,
        open,
        on_open,
        on_close,
    } = menu;
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
    let mut levels = Vec::new();
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
            slots: slots(&items, path, on_open, &on_close),
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
                on_open,
                &on_close,
                &typing,
                &flash,
            ),
        };
        let key = level_key(&key, depth);
        let tailed = depth == 0 && style.tail.is_some();
        let name = label.clone();
        levels.push(Level {
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
            background: if tailed {
                WindowBackground::None
            } else {
                WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: style.radius,
                    alpha: 1.0,
                    glass: Some(widgets::MENU_GLASS),
                }
            },
            content: match style.tail.filter(|_| tailed) {
                Some(tail) => Element::new(Callout {
                    name,
                    children: vec![
                        Element::new(panel).key("panel"),
                        Element::new(TailRow { tail }).key("tail"),
                    ],
                }),
                None => Element::new(panel),
            },
            parent,
            anchor,
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
    levels
}

/// The rows of the menu at `path`, keyed by their index; `current` is the item that takes the focus when it opens,
/// and `open` the item whose submenu is open.
#[allow(clippy::too_many_arguments)]
fn rows<M: Clone + 'static>(
    items: &[MenuItem<Out<M>>],
    min_width: f64,
    path: &[usize],
    current: Option<usize>,
    open: Option<usize>,
    on_open: &dyn Fn(Vec<usize>) -> Out<M>,
    on_close: &Out<M>,
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
    items: &[MenuItem<Out<M>>],
    path: &[usize],
    on_open: &dyn Fn(Vec<usize>) -> Out<M>,
    on_close: &Out<M>,
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
    escape: Out<M>,
    /// Sent on the arrow toward the leading side in a submenu.
    back: Option<Out<M>>,
    /// The rows in order, separators included.
    slots: Vec<Slot<M>>,
    /// The item whose submenu is open, and the submenu's height.
    open: Option<(usize, f64)>,
    /// The window of the open submenu.
    submenu: Option<WindowKey>,
    /// Closes the submenus of this menu.
    collapse: Out<M>,
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
    /// The submenu of this row opens at this time, unless the pointer moves on first.
    pending: Option<(Timestamp, usize)>,
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

    /// The row under `p` takes the highlight, the open submenu of another row closes at once, and the row's own
    /// submenu opens after [`SUBMENU_OPEN_DELAY`]. On a row that cannot be chosen the open submenu closes too; on the
    /// panel's padding it stays.
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
            // off the rows a submenu still to open does not open
            state.pending = None;
            return;
        };
        if let Some(position) = position {
            cx.focus_item(FocusTarget::Index(position));
        }
        let open = self.open.map(|(i, _)| i);
        if open.is_some_and(|open| open != index) {
            // the pointer came onto another row, not on its way into the open submenu: it closes now
            self.collapse.send(cx);
        }
        let opens = matches!(self.slots[index].action, Some(RowAction::Open { .. }));
        state.pending =
            (opens && open != Some(index)).then(|| (after(cx.now(), SUBMENU_OPEN_DELAY), index));
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
        if let Some((_, index)) = state.pending.filter(|(at, _)| *at <= now) {
            state.pending = None;
            if let Some(RowAction::Open { on_open, .. }) = &self.slots[index].action {
                on_open.send(cx);
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
                    on_open.send(cx);
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
                self.escape.send(cx);
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
                on_open.send(cx);
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
        // a choice still flashing when the application builds its menu again (a pane that refreshes as it shows a
        // level) carries on in the new menu, so it is still sent when the flash ends
        if !Rc::ptr_eq(&self.flash, &old.flash) {
            if let Some(flash) = old.flash.borrow_mut().take() {
                *self.flash.borrow_mut() = Some(flash);
            }
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
                message.send(cx);
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

mod widgets;
pub(crate) use widgets::opens_on_press;
use widgets::{tail_glass, Callout, Row, RowAction, Separator, TailRow};
