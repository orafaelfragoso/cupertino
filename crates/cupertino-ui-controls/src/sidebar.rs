//! Sidebar: a scroll view around the sections, each a heading over its rows. The rows are one roving group, keyed by
//! their items' keys; the model keeps the selection, counted over every section, which keys and clicks report.

use crate::icon;
use crate::look::{self, paint_focus_ring};
use crate::scroll;
use crate::{Handler, Scroller, Sidebar, SidebarAccessory, SidebarItem};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Environment, Event, EventCx,
    FocusTarget, Key, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    SizeSpec,
};
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent};
use cupertino_ui_render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::rc::Rc;

const HOME: Keysym = Keysym(0xff50);
const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const END: Keysym = Keysym(0xff57);

const ROW_HEIGHT: f64 = 28.0;
const HEADING_HEIGHT: f64 = 19.0;
const HEADING_SIZE: f32 = 11.0;
const HEADING_WEIGHT: u16 = 600;
/// Above the first section and below the last.
const TOP: f64 = 10.0;
const SECTION_GAP: f64 = 13.0;
/// Between the sidebar's edges and a row's selection.
const ROW_INSET: f64 = 10.0;
const SELECTION_RADIUS: f64 = 5.0;
/// Between a selection's leading edge and the icon, and the heading's text.
const INDENT: f64 = 6.0;
const ICON_BOX: f64 = 17.0;
const ICON: f64 = 16.0;
/// From the icon's leading edge to the label's.
const LABEL_X: f64 = 22.0;
const LABEL_TRAILING: f64 = 4.0;
/// A row's trailing button: its symbol centred in a slot that ends [`LABEL_TRAILING`] before the row's edge, and the
/// press taken up to that edge.
const ACCESSORY_SLOT: f64 = 24.0;
const ACCESSORY_SYMBOL: f64 = 14.0;
/// A busy row's activity indicator in the accessory slot: its spokes, their length and width from a circle of
/// [`ACCESSORY_SYMBOL`] across, the opacity of the faintest spoke, and one turn of the brightest spoke.
const ACTIVITY_SPOKES: usize = 8;
const ACTIVITY_SPOKE_LENGTH: f64 = 3.5;
const ACTIVITY_SPOKE_WIDTH: f64 = 1.5;
const ACTIVITY_FAINTEST: f32 = 0.25;
const ACTIVITY_TURN_NS: u64 = 800_000_000;

type Select<M> = Option<Rc<Handler<usize, M>>>;
type Accessory<M> = Option<Rc<Handler<Key, M>>>;

impl<M: 'static> From<Sidebar<M>> for Element<M> {
    fn from(sidebar: Sidebar<M>) -> Self {
        let count = sidebar.sections.iter().map(|s| s.items.len()).sum();
        let selected = sidebar.selected.filter(|&s| s < count);
        let cursor = (count > 0).then(|| selected.unwrap_or(0));
        let on_select = sidebar.on_select.map(Rc::new);
        let on_accessory = sidebar.on_accessory.map(Rc::new);
        let mut keys = Vec::with_capacity(count);
        let children = sidebar
            .sections
            .into_iter()
            .map(|section| {
                let rows = section
                    .items
                    .into_iter()
                    .map(|item| {
                        let index = keys.len();
                        let key = item.key.clone();
                        keys.push(key.clone());
                        let selected = selected == Some(index);
                        let children = item
                            .accessory
                            .clone()
                            .filter(|_| !item.busy)
                            .map(|accessory| {
                                Element::new(AccessoryButton {
                                    accessory,
                                    row: key.clone(),
                                    selected,
                                    on_accessory: on_accessory.clone(),
                                })
                            })
                            .into_iter()
                            .collect();
                        Element::new(Row {
                            index,
                            item,
                            selected,
                            cursor: cursor == Some(index),
                            on_select: on_select.clone(),
                            children,
                        })
                        .key(key)
                    })
                    .collect();
                Element::new(Section {
                    title: section.title,
                    rows,
                })
            })
            .collect();
        let rows = Rows {
            label: sidebar.label,
            keys,
            selected,
            on_select,
            children,
        };
        scroll::frame(
            Element::new(rows),
            sidebar.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(sidebar.on_scroll),
        )
    }
}

fn select<M>(on_select: &Select<M>, index: usize, cx: &mut EventCx<'_, M>) {
    if let Some(on_select) = on_select {
        cx.emit(on_select(index));
    }
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn column(insets: Insets, spacing: f64) -> LayoutSpec {
    LayoutSpec {
        insets,
        width: SizeSpec::Fill,
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// The content of the sidebar's scroll view: the sections, and the keys that move the selection across them.
struct Rows<M> {
    label: String,
    keys: Vec<Key>,
    selected: Option<usize>,
    on_select: Select<M>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Rows<M> {
    fn key(&self, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(last) = self.keys.len().checked_sub(1) else {
            return Propagation::Continue;
        };
        let mods = key.mods;
        if [Mods::CTRL, Mods::ALT, Mods::PRIMARY, Mods::SHIFT]
            .iter()
            .any(|&m| mods.contains(m))
        {
            return Propagation::Continue;
        }
        let to = match (key.keysym, self.selected) {
            (UP, Some(s)) => s.saturating_sub(1),
            (DOWN, Some(s)) => (s + 1).min(last),
            (UP | DOWN | HOME, None) | (HOME, Some(_)) => 0,
            (END, _) => last,
            _ => return Propagation::Continue,
        };
        if self.selected != Some(to) {
            select(&self.on_select, to, cx);
        }
        cx.focus_item(FocusTarget::Key(self.keys[to].clone()));
        Propagation::Stop
    }
}

impl<M: 'static> Widget<M> for Rows<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(
            Insets {
                top: TOP,
                left: 0.0,
                bottom: TOP,
                right: 0.0,
            },
            SECTION_GAP,
        )
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.keys.len() != old.keys.len() {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(key, _) if key.pressed && cx.phase() != Phase::Capture => self.key(key, cx),
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(self.label.clone());
        cx.item_count(self.keys.len());
        cx.roving_group();
    }
}

/// A heading, when the section has one, over its rows.
struct Section<M> {
    title: Option<String>,
    rows: Vec<Element<M>>,
}

fn heading(env: &Environment, title: &str) -> Option<ParagraphLayout> {
    let style = TextStyle {
        weight: FontWeight(HEADING_WEIGHT),
        ..TextStyle::new(HEADING_SIZE)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(title, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

impl<M: 'static> Widget<M> for Section<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let top = if self.title.is_some() {
            HEADING_HEIGHT
        } else {
            0.0
        };
        column(
            Insets {
                top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            0.0,
        )
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.rows)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.title.is_some() != old.title.is_some() {
            Change::Layout
        } else if self.title != old.title {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(title) = &self.title else {
            return;
        };
        let env = cx.env();
        let Some(layout) = heading(env, title) else {
            return;
        };
        let width = cx.size().width;
        let text = layout.size().width;
        let x = match env.direction {
            LayoutDirection::Ltr => ROW_INSET + INDENT,
            LayoutDirection::Rtl => width - ROW_INSET - INDENT - text,
        };
        let size = f64::from(HEADING_SIZE);
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let baseline = HEADING_HEIGHT / 2.0 + size * 0.36;
        let color = env
            .theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL);
        look::glyphs(cx, &layout, Point::new(x, baseline - first), color);
    }

    /// A section without a heading has no node of its own: its rows sit in the list.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some(title) = &self.title {
            cx.role(Role::Group);
            cx.name(title.clone());
        }
    }
}

/// One row: its selection inset from the sidebar's edges, its icon and its label, its accessory at the trailing edge,
/// and the focus at the cursor.
struct Row<M> {
    index: usize,
    item: SidebarItem,
    selected: bool,
    /// The sidebar's one Tab stop.
    cursor: bool,
    on_select: Select<M>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Row<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ROW_HEIGHT),
            insets: Insets {
                top: 0.0,
                left: ROW_INSET,
                bottom: 0.0,
                right: ROW_INSET,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::End,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.index, &self.item, self.selected, self.cursor)
            != (old.index, &old.item, old.selected, old.cursor)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, ROW_HEIGHT),
            baseline: None,
        }
    }

    /// A press focuses the row without showing the ring and selects it.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                cx.request_focus();
                if !self.selected {
                    select(&self.on_select, self.index, cx);
                }
                Propagation::Stop
            }
            Event::Action(request) if cx.node() == cx.target() => match request.action {
                Action::Click => {
                    if !self.selected {
                        select(&self.on_select, self.index, cx);
                    }
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            Event::AnimationFrame(_) if self.item.busy => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let chrome = Rect::new(
            ROW_INSET,
            0.0,
            (size.width - 2.0 * ROW_INSET).max(0.0),
            size.height,
        );
        let theme = cx.env().theme.clone();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let selected = ControlState {
            enabled: true,
            selected: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        if self.selected {
            let fill = look::color(&theme, ColorRole::ControlFill, &selected);
            look::fill(cx, chrome, SELECTION_RADIUS, fill);
        }
        // a selected row's icon and label take the selected text colour; another row's icon is the accent
        let text = if self.selected {
            look::color(&theme, ColorRole::ControlText, &selected)
        } else {
            theme.colors.get(ColorRole::Text, StateVariant::NORMAL)
        };
        let label_color = if self.item.busy && !self.selected {
            theme
                .colors
                .get(ColorRole::SecondaryText, StateVariant::NORMAL)
        } else {
            text
        };
        let leading = |offset: f64, width: f64| {
            if rtl {
                chrome.max_x() - offset - width
            } else {
                chrome.min_x() + offset
            }
        };
        if let Some(name) = &self.item.icon {
            // in a window that is not active another row's icon is no longer the accent: it takes the secondary label
            // colour, which keeps its contrast against the sidebar where the inactive accent would fade into it
            let tint = if self.selected {
                text
            } else if !cx.window_active() {
                theme
                    .colors
                    .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            } else {
                look::color(&theme, ColorRole::ControlFill, &selected)
            };
            if let Some(tint) = tint {
                let area = Rect::new(
                    leading(INDENT, ICON_BOX),
                    (size.height - ICON_BOX) / 2.0,
                    ICON_BOX,
                    ICON_BOX,
                );
                icon::symbol(cx, name, ICON, tint, false, area);
            }
        }
        if let Some(layout) = look::label(cx.env(), &self.item.label) {
            let start = INDENT + LABEL_X;
            // a busy row's activity indicator takes the accessory's slot
            let trailing = if self.item.accessory.is_some() || self.item.busy {
                ACCESSORY_SLOT + LABEL_TRAILING
            } else {
                LABEL_TRAILING
            };
            let room = (chrome.width() - start - trailing).max(0.0);
            let clip = Rect::new(leading(start, room), 0.0, room, size.height);
            let x = if rtl {
                clip.max_x() - layout.size().width
            } else {
                clip.min_x()
            };
            let origin = Point::new(x, look::top(&layout, chrome, cx.env()));
            cx.push(PaintOp::PushClip(Shape::Rect(clip)));
            look::glyphs(cx, &layout, origin, label_color);
            cx.push(PaintOp::PopClip);
        }
        if self.item.busy {
            let slot = Rect::new(
                leading(
                    chrome.width() - LABEL_TRAILING - ACCESSORY_SLOT,
                    ACCESSORY_SLOT,
                ),
                0.0,
                ACCESSORY_SLOT,
                size.height,
            );
            if let Some(color) = label_color {
                paint_activity(cx, slot.center(), color);
            }
        }
        if cx.is_focused() {
            paint_focus_ring(cx, chrome, SELECTION_RADIUS);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.item.label.clone());
        if let Some(description) = &self.item.description {
            cx.description(description.clone());
        }
        cx.states(AccessibleStates {
            selected: self.selected,
            busy: self.item.busy,
            ..AccessibleStates::default()
        });
        cx.item_index(self.index);
        cx.action(Action::Click);
        cx.roving_item();
        if self.cursor {
            cx.focusable();
        }
    }
}

/// The spokes of an activity indicator centred on `centre`, the brightest turning around once in
/// [`ACTIVITY_TURN_NS`] and the others fading behind it; still when motion is reduced.
fn paint_activity(cx: &mut PaintCx<'_>, centre: Point, color: cupertino_ui_theme::Color) {
    let lead = if cx.env().theme.motion.reduce {
        0
    } else {
        cx.request_animation_frame();
        let phase = (cx.now().0 % ACTIVITY_TURN_NS) as f64 / ACTIVITY_TURN_NS as f64;
        (phase * ACTIVITY_SPOKES as f64) as usize % ACTIVITY_SPOKES
    };
    let outer = ACCESSORY_SYMBOL / 2.0;
    let inner = outer - ACTIVITY_SPOKE_LENGTH;
    for spoke in 0..ACTIVITY_SPOKES {
        let angle = std::f64::consts::TAU * spoke as f64 / ACTIVITY_SPOKES as f64;
        let (sin, cos) = angle.sin_cos();
        let behind = (lead + ACTIVITY_SPOKES - spoke) % ACTIVITY_SPOKES;
        let fade = 1.0 - (1.0 - ACTIVITY_FAINTEST) * behind as f32 / (ACTIVITY_SPOKES - 1) as f32;
        cx.push(PaintOp::Stroke {
            shape: Shape::Path {
                verbs: vec![
                    PathVerb::MoveTo(Point::new(centre.x + inner * sin, centre.y - inner * cos)),
                    PathVerb::LineTo(Point::new(centre.x + outer * sin, centre.y - outer * cos)),
                ],
                fill_rule: FillRule::NonZero,
            },
            width: ACTIVITY_SPOKE_WIDTH,
            paint: Paint::Solid(cupertino_ui_theme::Color {
                a: color.a * fade,
                ..color
            }),
        });
    }
}

/// A row's trailing button. It acts on press, like the row, and shows no hover or pressed look; it is no Tab stop, so
/// the keyboard reaches its action through the application's own command.
struct AccessoryButton<M> {
    accessory: SidebarAccessory,
    row: Key,
    selected: bool,
    on_accessory: Accessory<M>,
}

impl<M: 'static> AccessoryButton<M> {
    fn press(&self, cx: &mut EventCx<'_, M>) {
        if let Some(on_accessory) = &self.on_accessory {
            cx.emit(on_accessory(self.row.clone()));
        }
    }
}

impl<M: 'static> Widget<M> for AccessoryButton<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(ACCESSORY_SLOT + LABEL_TRAILING),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.accessory, self.selected) != (&old.accessory, old.selected) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(ACCESSORY_SLOT + LABEL_TRAILING, ROW_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                self.press(cx);
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                self.press(cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    /// The symbol in the slot at the node's leading side; the rest of the node, up to the row's edge, takes presses.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let x = match cx.env().direction {
            LayoutDirection::Ltr => 0.0,
            LayoutDirection::Rtl => size.width - ACCESSORY_SLOT,
        };
        let slot = Rect::new(x, 0.0, ACCESSORY_SLOT, size.height);
        let state = ControlState {
            enabled: true,
            selected: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let color = if self.selected {
            look::color(&theme, ColorRole::ControlText, &state)
        } else {
            theme
                .colors
                .get(ColorRole::SecondaryText, StateVariant::NORMAL)
        };
        if let Some(color) = color {
            icon::symbol(
                cx,
                &self.accessory.symbol,
                ACCESSORY_SYMBOL,
                color,
                false,
                slot,
            );
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.accessory.label.clone());
        // without a handler a press does nothing, so the button offers no action and says it is unavailable
        if self.on_accessory.is_some() {
            cx.action(Action::Click);
        } else {
            cx.states(AccessibleStates {
                disabled: true,
                ..AccessibleStates::default()
            });
        }
    }
}
