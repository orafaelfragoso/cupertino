//! What the desktop draws on each output, and what its presses mean. The main display shows the items of the desktop
//! folder in their cells ([`crate::desktop`]): the icon, on a grey backdrop when selected, and the name under it in
//! white with a soft shadow, on an accent capsule when selected.
//!
//! * A press on an item selects it alone; with Shift or Primary held it toggles the item. A plain press on a selected
//!   item keeps the selection for a drag and selects the item alone on release. The second press on the same item within
//!   the double-click interval opens the selection, and a press on the name of the only selected item renames it once
//!   the interval passes without another press.
//! * A press on no item clears the selection (unless Shift or Primary is held) and a drag from it draws a rubber band
//!   that selects the items whose icon or name it touches, fading in as it starts and out once it is let go.
//! * Dragging items moves them by whole cells, or into the folder they are let go on.
//! * A secondary press (or a press with Ctrl held) selects the item under it unless it is selected, and opens the
//!   context menu of the selection, or of the desktop on no item.

use std::path::PathBuf;
use std::rc::Rc;

use cupertino_kit::controls::{Image, ImageFit, ImageSource};
use cupertino_kit::core::{
    AccessibleStates, Change, Element, Environment, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_kit::render::{Effect, Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, ParagraphLayout, TextStyle};
use cupertino_kit::theme::{Color, ColorRole, InteractionVariant, StateVariant};

use super::Message;
use crate::desktop::{backdrop_radius, item_frames};
use crate::kit::grid::laid_out_name;
use crate::metrics::desktop as m;
use crate::metrics::{drag, timing::DOUBLE_CLICK_INTERVAL};
use crate::rename::{RenameEvent, RenameTrigger};

/// The grey of the backdrop behind a selected icon, before its opacity.
const BACKDROP_GREY: f32 = 0.35;

fn seconds(at: Timestamp) -> f64 {
    at.0 as f64 / 1e9
}

fn rect(r: crate::geom::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn selected_variant() -> StateVariant {
    StateVariant {
        interaction: InteractionVariant::Normal,
        selected: true,
        window_active: true,
    }
}

/// An item as the desktop shows it.
#[derive(Clone)]
pub(super) struct ItemView {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Its cell, in the window.
    pub cell: Rect,
    pub icon: Vec<ImageSource>,
    pub selected: bool,
    /// A context menu is open on it.
    pub ringed: bool,
}

/// The rubber band as it shows: where, and when it started and was let go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BandLook {
    pub rect: Rect,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
}

impl BandLook {
    /// How opaque the band is at `now`: fading in from its start, and out from where it was when let go.
    fn opacity(&self, now: Timestamp) -> (f64, bool) {
        let fade_in = |at: Timestamp| {
            ((seconds(at) - seconds(self.started)) / m::BAND_FADE_IN).clamp(0.0, 1.0)
        };
        match self.ended {
            None => {
                let shown = fade_in(now);
                (shown, shown < 1.0)
            }
            Some(end) => {
                let out = ((seconds(now) - seconds(end)) / m::BAND_FADE_OUT).clamp(0.0, 1.0);
                (fade_in(end) * (1.0 - out), out < 1.0)
            }
        }
    }
}

/// The text of an item's name.
fn name_text(name: &str) -> AttributedText {
    AttributedText::plain(name.to_owned(), TextStyle::new(m::TEXT_SIZE as f32))
}

/// An item's name laid out as the desktop shows it.
fn laid_out(env: &Environment, name: &str) -> Option<ParagraphLayout> {
    laid_out_name(env, &name_text(name), Some(m::LABEL_LINES), m::LABEL_WIDTH)
}

/// The desktop of one output: its items, the band and the icons a drag moves.
pub(super) struct Desk {
    pub output: u32,
    pub label: String,
    pub items: Rc<[ItemView]>,
    pub band: Option<BandLook>,
    /// How far a drag moved the selected items.
    pub dragged: Option<Point>,
    /// The item being renamed, and the field it is renamed in.
    pub renaming: Option<(usize, Element<Message>)>,
}

/// A press held on an item.
struct Press {
    index: usize,
    at: Point,
    dragging: bool,
    /// The press kept the selection: the release selects the item alone.
    collapse: bool,
}

/// A band being drawn: where it started, and the selection to toggle against when Shift or Primary was held.
struct Banding {
    origin: Point,
    base: Option<Vec<usize>>,
}

#[derive(Default)]
pub(super) struct DeskState {
    press: Option<Press>,
    band: Option<Banding>,
    /// The item last pressed, and when.
    last: Option<(usize, f64)>,
    rename: RenameTrigger,
}

impl Desk {
    fn selected(&self) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&i| self.items[i].selected)
            .collect()
    }

    fn paths(&self, indices: impl IntoIterator<Item = usize>) -> Vec<PathBuf> {
        indices
            .into_iter()
            .filter_map(|i| self.items.get(i).map(|item| item.path.clone()))
            .collect()
    }

    fn select(&self, indices: Vec<usize>, cx: &mut EventCx<'_, Message>) {
        if indices != self.selected() {
            cx.emit(Message::Select(self.paths(indices)));
        }
    }

    /// Where item `index`'s name is, in the window, with the room of its capsule.
    fn label_rect(&self, index: usize, env: &Environment) -> Rect {
        let item = &self.items[index];
        let frames = item_frames(crate::geom::Rect::new(
            item.cell.min_x(),
            item.cell.min_y(),
            item.cell.width(),
            item.cell.height(),
        ));
        let size = laid_out(env, &item.name).map_or(Size::ZERO, |l| l.size());
        let (px, py) = m::LABEL_PAD;
        Rect::new(
            frames.label_center - size.width / 2.0 - px,
            frames.label_top,
            size.width + 2.0 * px,
            size.height + 2.0 * py,
        )
    }

    fn icon_rect(&self, index: usize) -> Rect {
        let cell = self.items[index].cell;
        rect(
            item_frames(crate::geom::Rect::new(
                cell.min_x(),
                cell.min_y(),
                cell.width(),
                cell.height(),
            ))
            .backdrop,
        )
    }

    /// The item whose icon or name is under `position`, and whether it was the name.
    fn hit(&self, position: Point, env: &Environment) -> Option<(usize, bool)> {
        (0..self.items.len()).rev().find_map(|i| {
            if self.icon_rect(i).contains(position) {
                Some((i, false))
            } else {
                self.label_rect(i, env)
                    .contains(position)
                    .then_some((i, true))
            }
        })
    }

    /// The items whose icon or name `band` touches.
    fn hits(&self, band: Rect, env: &Environment) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&i| {
                band.intersects(&self.icon_rect(i)) || band.intersects(&self.label_rect(i, env))
            })
            .collect()
    }

    fn local(&self, position: Point, cx: &EventCx<'_, Message>) -> Point {
        let origin = cx.bounds().map_or(Point::ZERO, |b| b.origin);
        Point::new(position.x - origin.x, position.y - origin.y)
    }

    fn press(
        &self,
        state: &mut DeskState,
        position: Point,
        mods: Mods,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        cx.emit(Message::Pressed);
        let toggles = mods.contains(Mods::SHIFT) || mods.contains(Mods::PRIMARY);
        let selected = self.selected();
        let Some((index, on_name)) = self.hit(position, cx.env()) else {
            state.last = None;
            state.rename.cancel();
            if !toggles {
                self.select(Vec::new(), cx);
            }
            state.band = Some(Banding {
                origin: position,
                base: toggles.then_some(selected),
            });
            cx.capture_pointer();
            return Propagation::Stop;
        };
        let now = seconds(cx.now());
        if toggles {
            state.last = None;
            state.rename.cancel();
            let mut next = selected;
            match next.iter().position(|&i| i == index) {
                Some(at) => {
                    next.remove(at);
                }
                None => {
                    next.push(index);
                    next.sort_unstable();
                }
            }
            self.select(next, cx);
            return Propagation::Stop;
        }
        let double = state
            .last
            .is_some_and(|(i, t)| i == index && now - t <= DOUBLE_CLICK_INTERVAL);
        if double {
            state.last = None;
            state.rename.cancel();
            cx.emit(Message::Open(self.paths(selected_or(&selected, index))));
            return Propagation::Stop;
        }
        state.last = Some((index, now));
        let sole = selected == [index];
        state.rename.click(index, on_name, sole, now);
        if let Some(deadline) = state.rename.deadline() {
            cx.wake_at(Timestamp((deadline * 1e9) as u64 + 1));
        }
        let keep = self.items[index].selected;
        if !keep {
            self.select(vec![index], cx);
        }
        state.press = Some(Press {
            index,
            at: position,
            dragging: false,
            collapse: keep && selected.len() > 1,
        });
        cx.capture_pointer();
        Propagation::Stop
    }

    fn secondary(&self, state: &mut DeskState, position: Point, cx: &mut EventCx<'_, Message>) {
        cx.emit(Message::Pressed);
        state.last = None;
        state.rename.cancel();
        match self.hit(position, cx.env()) {
            Some((index, _)) if !self.items[index].selected => self.select(vec![index], cx),
            Some(_) => {}
            None => self.select(Vec::new(), cx),
        }
        cx.emit(Message::ContextMenu {
            output: self.output,
            at: position,
        });
    }
}

/// The items an open acts on: the selection, or the item pressed when it is not selected.
fn selected_or(selected: &[usize], index: usize) -> Vec<usize> {
    if selected.contains(&index) {
        selected.to_vec()
    } else {
        vec![index]
    }
}

impl Widget<Message> for Desk {
    type State = DeskState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        let mut renaming = self.renaming.take();
        let mut children: Vec<Element<Message>> = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            let field = if renaming.as_ref().is_some_and(|(at, _)| *at == index) {
                renaming.take().map(|(_, field)| field)
            } else {
                None
            };
            children.push(
                cell(item, field, 1.0, item.cell).key(format!("item/{}", item.path.display())),
            );
        }
        if let Some(delta) = self.dragged {
            for item in self.items.iter().filter(|item| item.selected) {
                let moved = Rect::new(
                    item.cell.min_x() + delta.x,
                    item.cell.min_y() + delta.y,
                    item.cell.width(),
                    item.cell.height(),
                );
                children.push(
                    cell(item, None, m::DRAGGED_ALPHA, moved)
                        .key(format!("dragged/{}", item.path.display())),
                );
            }
        }
        if let Some(band) = self.band {
            children.push(Element::new(BandView { look: band }).key("band"));
        }
        children
    }

    fn state(&self) -> DeskState {
        DeskState::default()
    }

    fn update(&self, _: &Self, _: &mut DeskState) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &DeskState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        state: &mut DeskState,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => {
                let position = self.local(position, cx);
                if mods.contains(Mods::CTRL) {
                    // Ctrl with a press is the secondary click
                    self.secondary(state, position, cx);
                    return Propagation::Stop;
                }
                self.press(state, position, mods, cx)
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                let position = self.local(position, cx);
                self.secondary(state, position, cx);
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let position = self.local(position, cx);
                if let Some(band) = &state.band {
                    let area = Rect::new(
                        band.origin.x.min(position.x),
                        band.origin.y.min(position.y),
                        (band.origin.x - position.x).abs(),
                        (band.origin.y - position.y).abs(),
                    );
                    let hits = self.hits(area, cx.env());
                    let next = match &band.base {
                        None => hits,
                        Some(base) => {
                            let mut out: Vec<usize> = base
                                .iter()
                                .copied()
                                .filter(|i| !hits.contains(i))
                                .chain(hits.iter().copied().filter(|i| !base.contains(i)))
                                .collect();
                            out.sort_unstable();
                            out
                        }
                    };
                    cx.emit(Message::Band(self.output, Some(area), cx.now()));
                    self.select(next, cx);
                    return Propagation::Stop;
                }
                let Some(press) = state.press.as_mut() else {
                    return Propagation::Continue;
                };
                let delta = Point::new(position.x - press.at.x, position.y - press.at.y);
                if !press.dragging && delta.x.hypot(delta.y) < drag::START_DISTANCE {
                    return Propagation::Stop;
                }
                press.dragging = true;
                state.rename.cancel();
                state.last = None;
                cx.emit(Message::Drag(Some(delta)));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                let position = self.local(position, cx);
                if state.band.take().is_some() {
                    cx.release_pointer();
                    cx.emit(Message::Band(self.output, None, cx.now()));
                    return Propagation::Stop;
                }
                let Some(press) = state.press.take() else {
                    return Propagation::Continue;
                };
                cx.release_pointer();
                if press.dragging {
                    let delta = Point::new(position.x - press.at.x, position.y - press.at.y);
                    // a folder that is not dragged takes what is let go on it
                    let onto = self
                        .hit(position, cx.env())
                        .map(|(i, _)| i)
                        .filter(|&i| self.items[i].is_dir && !self.items[i].selected)
                        .map(|i| self.items[i].path.clone());
                    cx.emit(Message::Drop { delta, onto });
                    return Propagation::Stop;
                }
                if let Some(deadline) = state.rename.deadline() {
                    cx.wake_at(Timestamp((deadline * 1e9) as u64 + 1));
                }
                if press.collapse {
                    self.select(vec![press.index], cx);
                }
                Propagation::Stop
            }
            Event::Cancel => {
                if state.press.take().is_some_and(|p| p.dragging) {
                    cx.emit(Message::Drag(None));
                }
                state.rename.cancel();
                if state.band.take().is_some() {
                    cx.emit(Message::Band(self.output, None, cx.now()));
                }
                Propagation::Continue
            }
            Event::Wake(now) => {
                if state.press.is_none() {
                    if let Some(RenameEvent::Begin(index)) = state.rename.poll(seconds(now)) {
                        if let Some(item) = self.items.get(index) {
                            cx.emit(Message::BeginRename(item.path.clone()));
                        }
                    }
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &DeskState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &DeskState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            multiselectable: true,
            ..AccessibleStates::default()
        });
        cx.item_count(self.items.len());
    }
}

/// Item `item` in `frame`, at `opacity`; its name, or `field` while it is renamed.
fn cell(
    item: &ItemView,
    field: Option<Element<Message>>,
    opacity: f32,
    frame: Rect,
) -> Element<Message> {
    let name = field.unwrap_or_else(|| {
        Element::new(Label {
            name: item.name.clone(),
            selected: item.selected,
        })
    });
    Element::new(Cell {
        frame,
        name: item.name.clone(),
        selected: item.selected,
        ringed: item.ringed,
        opacity,
        children: vec![
            Element::new(Image {
                sources: item.icon.clone(),
                size: Size::new(m::ICON_SIZE, m::ICON_SIZE),
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            }),
            name,
        ],
    })
}

/// One item: its icon over its name, on the backdrop when selected.
struct Cell {
    /// In the window.
    frame: Rect,
    name: String,
    selected: bool,
    ringed: bool,
    opacity: f32,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Cell {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: m::ICON_TOP + m::BACKDROP_PAD,
                ..Insets::ZERO
            },
            width: SizeSpec::Fixed(self.frame.width()),
            height: SizeSpec::Fixed(self.frame.height()),
            position: self.frame.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: m::BACKDROP_PAD + m::LABEL_GAP,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if (self.selected, self.ringed, self.opacity)
            != (old.selected, old.ringed, old.opacity)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.opacity < 1.0 {
            cx.set_group(self.opacity, Vec::new());
        }
        let side = m::ICON_SIZE + 2.0 * m::BACKDROP_PAD;
        let backdrop = Rect::new((self.frame.width() - side) / 2.0, m::ICON_TOP, side, side);
        let radius = backdrop_radius();
        if self.selected {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: backdrop,
                    radius,
                },
                paint: Paint::Solid(Color::new(
                    BACKDROP_GREY,
                    BACKDROP_GREY,
                    BACKDROP_GREY,
                    m::BACKDROP_ALPHA,
                )),
            });
        }
        if self.ringed {
            let theme = cx.env().theme.clone();
            if let Some(accent) = theme.colors.get(ColorRole::ControlFill, selected_variant()) {
                let ring = crate::metrics::selection::CONTEXT_RING;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: backdrop.outset(-ring / 2.0),
                        radius: (radius - ring / 2.0).max(0.0),
                    },
                    paint: Paint::Solid(accent),
                    width: ring,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.name.clone());
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
    }
}

/// An item's name: white with a soft shadow, or on the accent capsule when selected.
struct Label {
    name: String,
    selected: bool,
}

impl Widget<Message> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name {
            Change::Layout
        } else if self.selected != old.selected {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let (px, py) = m::LABEL_PAD;
        match laid_out(cx.env(), &self.name) {
            Some(layout) => {
                let size = layout.size();
                Measured {
                    size: Size::new(size.width + 2.0 * px, size.height + 2.0 * py),
                    baseline: None,
                }
            }
            None => no_measure(),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = laid_out(cx.env(), &self.name) else {
            return;
        };
        let size = cx.size();
        let (px, py) = m::LABEL_PAD;
        let colors = cx.env().theme.colors.clone();
        let text = if self.selected {
            if let Some(accent) = colors.get(ColorRole::ControlFill, selected_variant()) {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(0.0, 0.0, size.width, size.height),
                        radius: (size.height / 2.0).min(m::LABEL_RADIUS_MAX),
                    },
                    paint: Paint::Solid(accent),
                });
            }
            colors
                .get(ColorRole::ControlText, selected_variant())
                .unwrap_or(Color::new(1.0, 1.0, 1.0, 1.0))
        } else {
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![Effect::DropShadow {
                    offset: Point::new(0.0, m::LABEL_SHADOW_OFFSET),
                    blur: m::LABEL_SHADOW_BLUR,
                    color: Color::new(0.0, 0.0, 0.0, m::LABEL_SHADOW_ALPHA),
                }],
            });
            Color::new(1.0, 1.0, 1.0, 1.0)
        };
        // the lines are centred in the widest a name runs; the label is as wide as its longest line
        let x = px - (m::LABEL_WIDTH - layout.size().width) / 2.0;
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: Point::new(x, py) + run.origin,
                    run: run.run.clone(),
                    color: text,
                });
            }
        }
        if !self.selected {
            cx.push(PaintOp::PopLayer);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The rubber band: a translucent fill inside a one-point border, fading in and out.
struct BandView {
    look: BandLook,
}

impl Widget<Message> for BandView {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.look.rect.width()),
            height: SizeSpec::Fixed(self.look.rect.height()),
            position: self.look.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.look.rect != old.look.rect {
            Change::Layout
        } else if self.look != old.look {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let (opacity, moving) = self.look.opacity(cx.now());
        if moving {
            cx.request_animation_frame();
        }
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let colors = cx.env().theme.colors.clone();
        let faded = |c: Color| Color {
            a: c.a * opacity as f32,
            ..c
        };
        if let Some(fill) = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(area),
                paint: Paint::Solid(faded(fill)),
            });
        }
        let width = crate::metrics::marquee::BORDER_WIDTH;
        let border = colors.get(ColorRole::PlaceholderText, StateVariant::NORMAL);
        if let Some(border) = border.filter(|_| size.width >= width && size.height >= width) {
            cx.push(PaintOp::Stroke {
                shape: Shape::Rect(Rect::new(
                    width / 2.0,
                    width / 2.0,
                    size.width - width,
                    size.height - width,
                )),
                paint: Paint::Solid(faded(border)),
                width,
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
