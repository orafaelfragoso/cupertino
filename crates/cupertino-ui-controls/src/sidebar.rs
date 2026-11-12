//! Sidebar: a scroll view around the sections, each a heading over its rows. The rows are one roving group, keyed by
//! their items' keys; the model keeps the selection, counted over every section, which keys and clicks report.

use crate::icon;
use crate::look::{self, paint_focus_ring};
use crate::scroll;
use crate::{Handler, Scroller, Sidebar, SidebarAccessory, SidebarItem, SidebarLook};
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

/// The geometry of a [`SidebarLook`], in points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    row: f64,
    heading: f64,
    /// Above the first section and below the last, and between two sections.
    top: f64,
    section_gap: f64,
    selection_radius: f64,
    /// From a selection's leading edge to the icon's box, the box and the symbol drawn in it.
    icon_x: f64,
    icon_box: f64,
    icon: f64,
    /// From a selection's leading edge to the label's and to the heading's text.
    label_x: f64,
    heading_x: f64,
    /// The label's weight in the selected row, `None` for the body weight.
    selected_weight: Option<u16>,
}

/// [`SidebarLook::Accent`].
const ACCENT: Metrics = Metrics {
    row: 28.0,
    heading: 19.0,
    top: 10.0,
    section_gap: 13.0,
    selection_radius: 5.0,
    icon_x: 6.0,
    icon_box: 17.0,
    icon: 16.0,
    label_x: 28.0,
    heading_x: 6.0,
    selected_weight: None,
};

/// [`SidebarLook::Neutral`]: 32 pt rows whose 22 pt symbol box is centred 18.75 pt in from the selection's edge, the
/// label 36.5 pt in, a 17 pt heading 7 pt in, 15 pt between sections and an 8 pt selection radius.
const NEUTRAL: Metrics = Metrics {
    row: 32.0,
    heading: 17.0,
    top: 2.5,
    section_gap: 15.0,
    selection_radius: 8.0,
    icon_x: 7.75,
    icon_box: 22.0,
    icon: 20.0,
    label_x: 36.5,
    heading_x: 7.0,
    selected_weight: Some(600),
};

impl SidebarLook {
    fn metrics(&self) -> &'static Metrics {
        match self {
            SidebarLook::Accent => &ACCENT,
            SidebarLook::Neutral { .. } => &NEUTRAL,
        }
    }
}

const HEADING_SIZE: f32 = 11.0;
const HEADING_WEIGHT: u16 = 600;
/// Between the sidebar's edges and a row's selection.
const ROW_INSET: f64 = 10.0;
/// The width of the accent ring round the row a context menu is open on, inside the selection's shape.
const MENU_RING: f64 = 2.0;
const LABEL_TRAILING: f64 = 4.5;
/// A row's trailing button: its symbol centred in a slot that ends [`LABEL_TRAILING`] before the row's edge, and the
/// press taken up to that edge.
const ACCESSORY_SLOT: f64 = 24.0;
const ACCESSORY_SYMBOL: f64 = 11.0;
/// The width of a busy row's activity indicator.
const ACTIVITY_DIAMETER: f64 = 14.0;
/// A busy row's activity indicator in the accessory slot: its spokes, their length and width from a circle of
/// [`ACTIVITY_DIAMETER`] across, the opacity of the faintest spoke, and one turn of the brightest spoke.
const ACTIVITY_SPOKES: usize = 8;
const ACTIVITY_SPOKE_LENGTH: f64 = 3.5;
const ACTIVITY_SPOKE_WIDTH: f64 = 1.5;
const ACTIVITY_FAINTEST: f32 = 0.25;
const ACTIVITY_TURN_NS: u64 = 800_000_000;

type Select<M> = Option<Rc<Handler<usize, M>>>;
type Accessory<M> = Option<Rc<Handler<Key, M>>>;

impl<M: 'static> From<Sidebar<M>> for Element<M> {
    fn from(sidebar: Sidebar<M>) -> Self {
        sidebar.with_look(SidebarLook::Accent)
    }
}

impl<M: 'static> Sidebar<M> {
    /// The sidebar's element, its rows drawn in `look`.
    pub fn with_look(self, look: SidebarLook) -> Element<M> {
        self.with_context(look, None, None)
    }

    /// [`with_look`](Self::with_look) for a sidebar whose rows have context menus: `ringed` is the row a context
    /// menu is open on, which wears an accent ring, and a secondary press on a row sends `on_context` with its
    /// index, without selecting it; the press goes on to what holds the sidebar, which opens the menu.
    pub fn with_context(
        self,
        look: SidebarLook,
        ringed: Option<usize>,
        on_context: Option<Handler<usize, M>>,
    ) -> Element<M> {
        let sidebar = self;
        let on_context = on_context.map(Rc::new);
        let metrics = look.metrics();
        let count = sidebar.sections.iter().map(|s| s.items.len()).sum();
        let selected = sidebar.selected.filter(|&s| s < count);
        let cursor = (count > 0).then(|| selected.unwrap_or(0));
        let on_select = sidebar.on_select.map(Rc::new);
        let on_accessory = sidebar.on_accessory.map(Rc::new);
        let mut keys = Vec::with_capacity(count);
        let children = sidebar
            .sections
            .into_iter()
            .enumerate()
            .map(|(number, section)| {
                let style = Style {
                    metrics,
                    neutral: matches!(look, SidebarLook::Neutral { .. }),
                    accent: match &look {
                        SidebarLook::Accent => true,
                        SidebarLook::Neutral { accent } => {
                            accent.get(number).copied().unwrap_or(false)
                        }
                    },
                };
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
                                    style,
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
                            ringed: ringed == Some(index),
                            style,
                            on_select: on_select.clone(),
                            on_context: on_context.clone(),
                            children,
                        })
                        .key(key)
                    })
                    .collect();
                Element::new(Section {
                    title: section.title,
                    metrics,
                    rows,
                })
            })
            .collect();
        let rows = Rows {
            label: sidebar.label,
            metrics,
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

/// How one section's rows are drawn: the look's metrics, whether the selection is the neutral grey, and whether the
/// rows' symbols take the accent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Style {
    metrics: &'static Metrics,
    neutral: bool,
    accent: bool,
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
    metrics: &'static Metrics,
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
                top: self.metrics.top,
                left: 0.0,
                bottom: self.metrics.top,
                right: 0.0,
            },
            self.metrics.section_gap,
        )
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.keys.len() != old.keys.len() || self.metrics != old.metrics {
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
    metrics: &'static Metrics,
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
            self.metrics.heading
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
        if self.title.is_some() != old.title.is_some() || self.metrics != old.metrics {
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
            LayoutDirection::Ltr => ROW_INSET + self.metrics.heading_x,
            LayoutDirection::Rtl => width - ROW_INSET - self.metrics.heading_x - text,
        };
        let size = f64::from(HEADING_SIZE);
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let baseline = self.metrics.heading / 2.0 + size * 0.36;
        // in a window that is not active the heading recedes with the rows' labels
        let role = if cx.window_active() {
            ColorRole::SecondaryText
        } else {
            ColorRole::InactiveSidebarText
        };
        let color = env.theme.colors.get(role, StateVariant::NORMAL);
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
    /// A context menu is open on the row: it wears an accent ring.
    ringed: bool,
    style: Style,
    on_select: Select<M>,
    on_context: Select<M>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Row<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.style.metrics.row),
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
        if self.style.metrics != old.style.metrics {
            Change::Layout
        } else if (
            self.index,
            &self.item,
            self.selected,
            self.cursor,
            self.ringed,
            self.style,
        ) != (
            old.index,
            &old.item,
            old.selected,
            old.cursor,
            old.ringed,
            old.style,
        ) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.style.metrics.row),
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
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                select(&self.on_context, self.index, cx);
                Propagation::Continue
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
        let m = self.style.metrics;
        let neutral = self.style.neutral;
        let active = cx.window_active();
        // the accent look's selection in an active window is the accent; the neutral look's, and any selection in a
        // window that is not active, is the sidebar's neutral selection laid over its material, fainter when inactive
        let accented = self.selected && !neutral && active;
        if self.selected {
            let fill = if accented {
                look::color(&theme, ColorRole::ControlFill, &selected)
            } else {
                look::color(&theme, ColorRole::SidebarSelection, &selected)
            };
            look::fill(cx, chrome, m.selection_radius, fill);
        }
        if self.ringed {
            look::stroke(
                cx,
                chrome.inset(cupertino_geometry::Insets::uniform(MENU_RING / 2.0)),
                (m.selection_radius - MENU_RING / 2.0).max(0.0),
                MENU_RING,
                look::color(&theme, ColorRole::ControlFill, &selected),
            );
        }
        // a row on the accent takes the selected text colour; on the neutral selection it keeps its own colours; in a
        // window that is not active every label is the inactive sidebar text, the selected row's too
        let text = if accented {
            look::color(&theme, ColorRole::ControlText, &selected)
        } else if !active {
            theme
                .colors
                .get(ColorRole::InactiveSidebarText, StateVariant::NORMAL)
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
            // in a window that is not active no symbol is the accent: each takes the label's inactive sidebar text,
            // which keeps its contrast against the sidebar and the neutral selection where the inactive accent would
            // fade into them; in an active window the neutral look's selected row keeps the accent symbol, and its
            // sections without the accent the secondary
            let accent = look::color(&theme, ColorRole::ControlFill, &selected);
            let secondary = theme
                .colors
                .get(ColorRole::SecondaryText, StateVariant::NORMAL);
            let tint = if accented || !active {
                text
            } else if self.selected || self.style.accent {
                accent
            } else {
                secondary
            };
            if let Some(tint) = tint {
                let area = Rect::new(
                    leading(m.icon_x, m.icon_box),
                    (size.height - m.icon_box) / 2.0,
                    m.icon_box,
                    m.icon_box,
                );
                icon::symbol(cx, name, m.icon, tint, false, area);
            }
        }
        let weight = m.selected_weight.filter(|_| self.selected);
        if let Some(layout) = row_label(cx.env(), &self.item.label, weight) {
            let start = m.label_x;
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
            paint_focus_ring(cx, chrome, m.selection_radius);
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

/// A row's label in the theme's body type, in `weight` when there is one.
fn row_label(env: &Environment, text: &str, weight: Option<u16>) -> Option<ParagraphLayout> {
    let Some(weight) = weight else {
        return look::label(env, text);
    };
    if text.is_empty() {
        return None;
    }
    let body = env.theme.type_scale.body;
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(body.size)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
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
    let outer = ACTIVITY_DIAMETER / 2.0;
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
    style: Style,
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
        if (&self.accessory, self.selected, self.style) != (&old.accessory, old.selected, old.style)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(ACCESSORY_SLOT + LABEL_TRAILING, self.style.metrics.row),
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
        // in a window that is not active the accessory recedes with the labels
        let color = if self.selected && !self.style.neutral && cx.window_active() {
            look::color(&theme, ColorRole::ControlText, &state)
        } else if !cx.window_active() {
            theme
                .colors
                .get(ColorRole::InactiveSidebarText, StateVariant::NORMAL)
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
