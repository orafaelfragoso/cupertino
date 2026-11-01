//! Menus in popup windows, the regions that open a context menu, and pop-up buttons. A menu's enabled items are one
//! roving group: the arrows and typed characters move the focus between them, and the focused item is drawn
//! highlighted. Each open submenu is a popup of the menu before it, beside its item.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{ContextMenu, Handler, Menu, MenuAnchor, MenuItem, PopUpButton};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::metrics::Control;
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, CommandRegistry, ControlState,
    Element, Event, EventCx, FocusTarget, Key, MeasureCx, PaintCx, Phase, Popup, PopupAnchor,
    Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::{DecorationMode, Keysym, Mods, MouseButton, PointerEvent, WindowSpec};
use cupertino_ui_render::{Paint, PaintOp, Sampling, Shape};
use cupertino_ui_theme::{Color, ColorRole, StateVariant, Surface};
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

const PANEL_RADIUS: f64 = 8.0;
const EDGE: f64 = 1.0;
/// Above the first row and below the last.
const PADDING_Y: f64 = 5.0;
const ROW_HEIGHT: f64 = 22.0;
const SEPARATOR_HEIGHT: f64 = 9.0;
const PADDING_X: f64 = 10.0;
/// Before the label, for the check mark.
const MARK_COLUMN: f64 = 18.0;
/// After the label, for a submenu's chevron.
const ARROW_COLUMN: f64 = 14.0;
const MIN_WIDTH: f64 = 120.0;
/// Between a highlight and the panel's sides.
const HIGHLIGHT_INSET: f64 = 5.0;
const HIGHLIGHT_RADIUS: f64 = 4.0;
const SYMBOL: f64 = 10.0;
/// How far a submenu overlaps the menu it opens from.
const SUBMENU_OVERLAP: f64 = 2.0;
/// Between a pop-up button and its menu.
const GAP: f64 = 4.0;
/// The most a menu takes; a taller one is cut off.
const MAX: Size = Size {
    width: 480.0,
    height: 720.0,
};
/// Between a pop-up button's label and its chevrons.
const CHEVRONS_GAP: f64 = 6.0;
/// Between a pop-up button's sides and its label or chevrons; its menu lines its rows' labels up with the label.
const LABEL_INSET: f64 = 10.0;

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
        let mut anchor = match anchor {
            MenuAnchor::Below(key) => PopupAnchor::Below { key, gap: GAP },
            MenuAnchor::At(point) => PopupAnchor::At(point),
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
                        LABEL_INSET - PADDING_X - MARK_COLUMN,
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
            let panel = Panel {
                label: label.clone(),
                current: current
                    .filter(|_| depth == 0)
                    .and_then(|i| enabled_position(&items, i)),
                escape: back.clone().unwrap_or_else(|| on_close.clone()),
                back,
                children: rows(
                    &items,
                    path,
                    current.filter(|_| depth == 0),
                    next.filter(|_| submenu.is_some()),
                    &on_open,
                    &on_close,
                ),
            };
            let key = level_key(&key, depth);
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
                },
                shortcuts: CommandRegistry::new(),
                on_close: None,
                background: WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: PANEL_RADIUS,
                    alpha: 1.0,
                },
                content: Element::new(panel),
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
    path: &[usize],
    current: Option<usize>,
    open: Option<usize>,
    on_open: &Handler<Vec<usize>, M>,
    on_close: &M,
) -> Vec<Element<M>> {
    let labels: Rc<[String]> = items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Item {
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
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (label, enabled, checked, action) = match item {
                MenuItem::Separator => return Element::new(Separator).key(index as u64),
                MenuItem::Item {
                    label,
                    enabled,
                    checked,
                    on_select,
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
                enabled,
                checked,
                action,
                labels: Rc::clone(&labels),
                position,
                current: current == Some(index),
            };
            if enabled {
                position += 1;
            }
            Element::new(row).key(index as u64)
        })
        .collect()
}

/// The place of item `index` among the enabled items, if it is one.
fn enabled_position<M>(items: &[MenuItem<M>], index: usize) -> Option<usize> {
    let enabled = |item: &MenuItem<M>| {
        matches!(
            item,
            MenuItem::Item { enabled: true, .. } | MenuItem::Submenu { enabled: true, .. }
        )
    };
    enabled(items.get(index)?).then(|| items[..index].iter().filter(|i| enabled(i)).count())
}

/// One level of a menu: its rows on the panel, and the messages that close it.
struct Panel<M> {
    label: String,
    /// The place among the enabled rows of the row that takes the focus when the menu opens.
    current: Option<usize>,
    /// Sent on Escape: closes a submenu, or the whole menu.
    escape: M,
    /// Sent on the arrow toward the leading side in a submenu.
    back: Option<M>,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Panel<M> {
    /// Whether the focus came in yet.
    type State = bool;

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

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, _: &mut bool) -> Change {
        if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, entered: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::FocusWithin { gained: true, .. } = event {
            if !std::mem::replace(entered, true) {
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

    /// The window paints the material or the opaque fill under it; the panel adds its edge.
    fn paint(&self, _: &bool, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let edge = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            PANEL_RADIUS - EDGE / 2.0,
            EDGE,
            edge,
        );
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Menu);
        cx.name(self.label.clone());
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
    enabled: bool,
    checked: bool,
    action: RowAction<M>,
    /// The labels of the menu's enabled rows, in order, for typing to move.
    labels: Rc<[String]>,
    /// This row's place among them, if enabled.
    position: usize,
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
            } => {
                cx.emit(on_select.clone());
                cx.emit(on_close.clone());
            }
            RowAction::Open { on_open, .. } => cx.emit(on_open.clone()),
        }
    }

    /// The next enabled row after this one whose label starts with `c`, wrapping around; from this one when it is
    /// not `selected`.
    fn typed(&self, c: char, selected: bool) -> Option<usize> {
        let typed: String = c.to_lowercase().collect();
        let count = self.labels.len();
        (usize::from(selected)..count + usize::from(selected))
            .map(|k| (self.position + k) % count)
            .find(|&i| self.labels[i].to_lowercase().starts_with(&typed))
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
                    Some(c) => match self.typed(c, selected) {
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
        let width = PADDING_X + MARK_COLUMN + label + ARROW_COLUMN + PADDING_X;
        Measured {
            size: Size::new(width.max(MIN_WIDTH), ROW_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, state: &mut RowState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Enter { .. }, _)
                if self.enabled && cx.node() == cx.target() =>
            {
                state.pointed = true;
                cx.request_focus();
                cx.request_paint();
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.pointed = false;
                cx.request_paint();
            }
            Event::Focus { .. } if cx.node() == cx.target() => cx.request_paint(),
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
        let highlighted = self.enabled
            && (self.open() || cx.is_focused() && self.selected(state, cx.focus_visible()));
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
            let x = leading(PADDING_X, MARK_COLUMN) + MARK_COLUMN / 2.0;
            symbol(cx, "symbol.checkmark", Point::new(x, middle), color, false);
        }
        if let Some(label) = look::label(&env, &self.label) {
            let width = label.size().width;
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let origin = Point::new(
                leading(PADDING_X + MARK_COLUMN, width),
                look::top(&label, row, &env),
            );
            look::glyphs(cx, &label, origin, color);
        }
        if matches!(self.action, RowAction::Open { .. }) {
            let x =
                leading(size.width - PADDING_X - ARROW_COLUMN, ARROW_COLUMN) + ARROW_COLUMN / 2.0;
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

struct Separator;

impl<M: 'static> Widget<M> for Separator {
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
            size: Size::new(0.0, SEPARATOR_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let Some(color) = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL)
        else {
            return;
        };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(
                PADDING_X,
                (size.height - EDGE) / 2.0,
                size.width - 2.0 * PADDING_X,
                EDGE,
            )),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Separator);
    }
}

impl<M: Clone + 'static> Widget<M> for ContextMenu<M> {
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(on_open) = &self.on_open else {
            return Propagation::Continue;
        };
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
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
        let env = cx.env();
        let metrics = env.theme.metrics;
        let widest = self
            .options
            .iter()
            .filter_map(|o| look::label(env, o))
            .map(|l| l.size().width)
            .fold(0.0, f64::max);
        let height = look::metric(Control::PopUpButton, &env.theme);
        let width = LABEL_INSET + widest + CHEVRONS_GAP + SYMBOL + LABEL_INSET;
        Measured {
            size: Size::new(
                width.max(metrics.min_target.width),
                height.max(metrics.min_target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(on_open) = self.on_open.as_ref().filter(|_| self.enabled) else {
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
        match press.event(event, true, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
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

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed() || self.open,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let height = look::metric(Control::PopUpButton, theme);
        let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
        let radius = theme.metrics.button_radius;
        look::fill(
            cx,
            chrome,
            radius,
            look::color(theme, ColorRole::ControlFill, &state),
        );
        look::stroke(
            cx,
            chrome.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            look::color(theme, ColorRole::ControlBorder, &state),
        );
        let text = look::color(theme, ColorRole::ControlText, &state);
        let inset = LABEL_INSET;
        let rtl = env.direction == LayoutDirection::Rtl;
        if let Some(label) = look::label(&env, self.chosen()) {
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
        symbol(
            cx,
            "symbol.chevron.up.down",
            Point::new(x, chrome.min_y() + chrome.height() / 2.0),
            text,
            false,
        );
        paint_focus_ring(cx, chrome, radius);
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
