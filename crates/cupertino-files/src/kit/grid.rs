//! The folder as a grid of icons with their names under them, laid out by [`crate::layout::icon`]. It builds only the
//! cells near the viewport plus the cursor's. The model keeps the selection: clicks and keys report a new one and move
//! the focus to the cursor's cell, which the runtime scrolls into view. Arrows go to the nearest cell that way
//! ([`IconLayout::neighbour`], mirrored in right-to-left layouts), Page Up and Page Down by the rows in view, Home and
//! End to the first and last item, Return opens, typing selects by name, and Tab and Shift+Tab select the next and the
//! previous item by name, and past the last (or before the first) move the keyboard on to the next control. A press on empty space draws a rubber band ([`super::band`]); a double click opens an item, a
//! press on the name of the only selected item renames it after the double-click interval ([`super::clicks`]), and a
//! press with Ctrl held opens the context menu. Read as a list of items.

use std::rc::Rc;

use cupertino_kit::controls::{Image, ImageFit, ImageSource, Selection, Text, TextRole};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Environment, Event, EventCx, FocusTarget, Key,
    MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, ScrollState, SizeSpec,
};
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, InteractionVariant, StateVariant};

use super::band::{self, Band};
use super::clicks::Clicks;
use super::dragged::Grab;
use super::{Event as WindowEvent, Message};
use crate::geom::{self, Direction};
use crate::layout::icon::{IconLayout, IconParams};
use crate::metrics::{drag, selection as sel};

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const HOME: Keysym = Keysym(0xff50);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const END: Keysym = Keysym(0xff57);
const A: Keysym = Keysym(0x61);
const SHIFTED_A: Keysym = Keysym(0x41);
const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);

/// How far past the viewport cells are built, in points.
const OVERSCAN: f64 = 200.0;
/// How long typed letters add to one search (1 s).
const TYPE_SELECT_NS: u64 = 1_000_000_000;
/// Corner radius of the highlight behind a selected name (approximate).
const NAME_RADIUS: f64 = 4.0;
/// The body role's size at the usual text size: a name follows the role's changes from it.
const USUAL_BODY_SIZE: f32 = 13.0;
/// Room between a selected name and the ends of its highlight (approximate).
const NAME_INSETS: Insets = Insets {
    top: 0.0,
    left: 3.0,
    bottom: 0.0,
    right: 3.0,
};

/// A keyboard move in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// To the nearest cell on that side, in the layout's own (left-to-right) terms.
    Toward(Direction),
    Home,
    End,
    PageUp,
    PageDown,
}

/// The item `step` moves the keyboard to from item `from` of a non-empty `layout`, `page` items making a page (one at
/// least). A move past the edge stays.
pub(super) fn target(layout: &IconLayout, from: usize, step: Step, page: usize) -> usize {
    let last = layout.cells.len().saturating_sub(1);
    let page = page.max(1);
    match step {
        Step::Toward(direction) => layout.neighbour(from, direction).unwrap_or(from),
        Step::Home => 0,
        Step::End => last,
        Step::PageUp => from.saturating_sub(page),
        Step::PageDown => (from + page).min(last),
    }
}

/// The item after (or before, `back`) item `from` in the order of `names`, by name: the first (or last) without one, and
/// the same at the end.
pub(super) fn by_name<S: AsRef<str>>(
    names: &[S],
    from: Option<usize>,
    back: bool,
) -> Option<usize> {
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by(|&a, &b| crate::natural::compare_names(names[a].as_ref(), names[b].as_ref()));
    let at = from.and_then(|f| order.iter().position(|&i| i == f));
    let next = match (at, back) {
        (None, false) => 0,
        (None, true) => order.len().checked_sub(1)?,
        (Some(at), false) => (at + 1).min(order.len() - 1),
        (Some(at), true) => at.saturating_sub(1),
    };
    order.get(next).copied()
}

/// The key of item `index`'s cell in the grid of window `id`: one per window, as focus goes to the first window holding
/// the key.
pub(super) fn cell_key(id: u64, index: usize) -> Key {
    Key::from(format!("icon/{id}/{index}"))
}

/// The index of the item whose cell [`cell_key`] keyed `key`.
pub(super) fn cell_index(key: &Key) -> Option<u64> {
    let Key::Str(key) = key else {
        return None;
    };
    let (_, index) = key.strip_prefix("icon/")?.split_once('/')?;
    index.parse().ok()
}

/// An item the grid shows: its name and the renditions of its icon or thumbnail.
pub(super) struct Item {
    pub name: String,
    pub icon: Vec<ImageSource>,
}

/// The content of the grid's scroll view: as tall as every cell, holding the cells that are built.
pub(super) struct IconGrid {
    /// The id of the window the grid's messages are for.
    pub window: u64,
    /// The accessible name.
    pub label: String,
    pub layout: Rc<IconLayout>,
    pub params: IconParams,
    pub items: Rc<[Item]>,
    pub scroll: ScrollState,
    pub selection: Selection,
    /// The items a context menu is open on, which wear an accent ring.
    pub ringed: Selection,
    /// The rubber band being drawn, in the grid.
    pub band: Option<Rect>,
    /// How far down the scroll view's content the grid starts: the part of the view the toolbar row covers at rest,
    /// which the grid scrolls under.
    pub lead: f64,
    /// What sits under item `index`'s icon, given as `(index, selected)`: its [`name`], or the field it is renamed in.
    pub name: Box<dyn Fn(usize, bool) -> Element<Message>>,
    /// One row scrolled sideways ([`crate::layout::icon::strip`]), as wide as its cells and as tall as the view: a page
    /// is the pictures in view, and a press between them draws no band.
    pub strip: bool,
}

/// The node state of the grid: what was typed to select, and when that search ends; the press that may become a drag,
/// the band being drawn and what the presses mean.
#[derive(Default)]
pub(super) struct TypeSelect {
    typed: String,
    typed_until: Option<Timestamp>,
    press: Option<Press>,
    band: Option<Band>,
    clicks: Clicks,
}

/// A press on an item, held: moving [`drag::START_DISTANCE`] drags the selection, a release without moving
/// selects the item alone, with the cursor and the anchor on it, when the press kept the selection.
struct Press {
    index: usize,
    /// In window coordinates.
    at: Point,
    /// The selection the drag carries, the pressed item first.
    dragged: Vec<usize>,
    collapse: bool,
}

/// What an assistive Click on item `index` reports: the item selected alone, unless it already is.
type Pick = Rc<dyn Fn(usize) -> Option<Message>>;

fn rect(r: geom::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn selected_variant(window_active: bool) -> StateVariant {
    StateVariant {
        interaction: InteractionVariant::Normal,
        selected: true,
        window_active,
    }
}

impl IconGrid {
    fn count(&self) -> usize {
        self.layout.cells.len()
    }

    fn cursor(&self) -> Option<usize> {
        self.selection.cursor().filter(|&c| c < self.count())
    }

    /// The item the keyboard is on: the selection's cursor, else the first item in view.
    fn at(&self) -> Option<usize> {
        self.cursor().or_else(|| {
            let cells = &self.layout.cells;
            let first = match self.strip {
                true => cells
                    .iter()
                    .position(|c| c.frame.x + c.frame.w > self.scroll.offset.x),
                false => cells
                    .iter()
                    .position(|c| c.frame.y + c.frame.h > self.scroll.offset.y),
            };
            first.or(cells.len().checked_sub(1))
        })
    }

    /// Items a page moves by: the rows the viewport shows below the toolbar row, or the pictures a strip shows.
    fn page(&self) -> usize {
        let (room, pitch, per) = match self.strip {
            true => (
                self.scroll.viewport.width,
                self.layout.cell.w + self.layout.spacing.0,
                1,
            ),
            false => (
                self.scroll.viewport.height - self.lead,
                self.layout.cell.h + self.layout.spacing.1,
                self.layout.columns,
            ),
        };
        (room / pitch).floor().max(1.0) as usize * per
    }

    /// The cells to build, in order: those near the viewport and the keyboard's.
    fn built(&self) -> Vec<usize> {
        let at = self.at();
        let near: Box<dyn Fn(&crate::layout::icon::IconCell) -> bool> = match self.strip {
            true => {
                let left = self.scroll.offset.x - OVERSCAN;
                let right = self.scroll.offset.x + self.scroll.viewport.width + OVERSCAN;
                Box::new(move |c| c.frame.x + c.frame.w >= left && c.frame.x <= right)
            }
            false => {
                // in the grid, which starts `lead` down the content: what shows under the toolbar row too
                let top = self.scroll.offset.y - self.lead - OVERSCAN;
                let bottom =
                    self.scroll.offset.y - self.lead + self.scroll.viewport.height + OVERSCAN;
                Box::new(move |c| c.frame.y + c.frame.h >= top && c.frame.y <= bottom)
            }
        };
        self.layout
            .cells
            .iter()
            .filter(|c| near(c) || Some(c.index) == at)
            .map(|c| c.index)
            .collect()
    }

    /// `position` in the window as a point of the layout, which is laid out left to right.
    fn layout_point(&self, position: Point, cx: &EventCx<'_, Message>) -> Option<geom::Point> {
        let bounds = cx.bounds()?;
        let x = position.x - bounds.min_x();
        let x = match cx.env().direction {
            LayoutDirection::Ltr => x,
            LayoutDirection::Rtl => bounds.size.width - x,
        };
        Some(geom::Point {
            x,
            y: position.y - bounds.min_y(),
        })
    }

    /// The item whose icon or name is under a pointer at `position`, in window coordinates.
    fn index_at(&self, position: Point, cx: &EventCx<'_, Message>) -> Option<usize> {
        self.layout.hit(self.layout_point(position, cx)?)
    }

    /// Whether `position` in the window is on the name of item `index`.
    fn on_name(&self, index: usize, position: Point, cx: &EventCx<'_, Message>) -> bool {
        let (Some(at), Some(cell)) = (
            self.layout_point(position, cx),
            self.layout.cells.get(index),
        ) else {
            return false;
        };
        cell.label_frame.contains(at)
    }

    /// The part of the grid the scroll view shows below the toolbar row, in the window.
    fn shown(&self, cx: &EventCx<'_, Message>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        let viewport = self.scroll.viewport;
        Some(Rect::new(
            bounds.min_x(),
            bounds.min_y() + self.scroll.offset.y,
            viewport.width.max(bounds.size.width),
            (viewport.height - self.lead).max(0.0),
        ))
    }

    /// Show `band` and select what it touches: the items whose icon or name it meets. The window hears of the band
    /// before its selection, so it holds the selection's count until the band is let go.
    fn draw_band(&self, band: &Band, cx: &mut EventCx<'_, Message>) {
        let Some(bounds) = cx.bounds() else {
            return;
        };
        let rect = band.rect(bounds.origin);
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let x = if rtl {
            bounds.size.width - rect.max_x()
        } else {
            rect.min_x()
        };
        let hits = self.layout.hits_in_rect(&geom::Rect {
            x,
            y: rect.min_y(),
            w: rect.width(),
            h: rect.height(),
        });
        cx.emit(Message::Window(self.window, WindowEvent::Band(Some(rect))));
        self.select(band.select(&hits), cx);
    }

    fn pick(&self) -> Pick {
        let (window, selection) = (self.window, self.selection.clone());
        Rc::new(move |index| {
            let mut next = selection.clone();
            next.select_only(index);
            (next != selection).then_some(Message::Window(window, WindowEvent::Select(next)))
        })
    }

    fn select(&self, next: Selection, cx: &mut EventCx<'_, Message>) {
        if next != self.selection {
            cx.emit(Message::Window(self.window, WindowEvent::Select(next)));
        }
    }

    /// Moves the cursor to `index`, selecting it alone or extending to it, and the focus with it.
    fn move_to(&self, index: usize, extend: bool, cx: &mut EventCx<'_, Message>) {
        let mut next = self.selection.clone();
        if extend {
            next.extend_to(index);
        } else {
            next.select_only(index);
        }
        self.select(next, cx);
        cx.focus_item(FocusTarget::Key(cell_key(self.window, index)));
    }

    fn key(
        &self,
        state: &mut TypeSelect,
        key: &KeyEvent,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        let Some(at) = self.at() else {
            return Propagation::Continue;
        };
        let mods = key.mods;
        let plain = !mods.contains(Mods::CTRL)
            && !mods.contains(Mods::ALT)
            && !mods.contains(Mods::PRIMARY);
        let (forward, back) = match cx.env().direction {
            LayoutDirection::Ltr => (Direction::Right, Direction::Left),
            LayoutDirection::Rtl => (Direction::Left, Direction::Right),
        };
        let step = match key.keysym {
            LEFT if plain => Step::Toward(back),
            RIGHT if plain => Step::Toward(forward),
            UP if plain => Step::Toward(Direction::Up),
            DOWN if plain => Step::Toward(Direction::Down),
            HOME if plain => Step::Home,
            END if plain => Step::End,
            PAGE_UP if plain => Step::PageUp,
            PAGE_DOWN if plain => Step::PageDown,
            A | SHIFTED_A if mods.contains(Mods::PRIMARY) => {
                let mut next = self.selection.clone();
                next.select_all(self.count());
                self.select(next, cx);
                return Propagation::Stop;
            }
            TAB | LEFT_TAB if plain => {
                let back = key.keysym == LEFT_TAB || mods.contains(Mods::SHIFT);
                let names: Vec<&str> = self.items.iter().map(|i| i.name.as_str()).collect();
                let cursor = self.cursor();
                // past the last item by name (or before the first) the keyboard goes on to the next control
                return match by_name(&names, cursor, back).filter(|&to| Some(to) != cursor) {
                    Some(to) => {
                        self.move_to(to, false, cx);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                };
            }
            RETURN | KP_ENTER if plain => {
                return match self.cursor() {
                    Some(c) => {
                        cx.emit(Message::Window(self.window, WindowEvent::Open(c)));
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                };
            }
            _ if plain => return self.type_select(state, key, cx),
            _ => return Propagation::Continue,
        };
        let to = match self.cursor() {
            Some(c) => target(&self.layout, c, step, self.page()),
            None => at,
        };
        self.move_to(to, mods.contains(Mods::SHIFT), cx);
        Propagation::Stop
    }

    /// What a drag of `press` carries, and how the pressed item's cell is laid out for its image.
    fn grab(&self, press: &Press, cx: &EventCx<'_, Message>) -> Option<Grab> {
        let bounds = cx.bounds()?;
        let cell = self.layout.cells.get(press.index)?;
        let (frame, icon, label) = (
            rect(cell.frame),
            rect(cell.icon_frame),
            rect(cell.label_frame),
        );
        // the folder may have changed under a held press
        if press.dragged.iter().any(|&i| i >= self.items.len()) {
            return None;
        }
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let left = if rtl {
            bounds.size.width - frame.max_x()
        } else {
            frame.min_x()
        };
        Some(Grab {
            indices: press.dragged.clone(),
            name: self.items[press.index].name.clone(),
            icons: press
                .dragged
                .iter()
                .map(|&i| self.items[i].icon.clone())
                .collect(),
            offset: Point::new(
                press.at.x - bounds.min_x() - left,
                press.at.y - bounds.min_y() - frame.min_y(),
            ),
            rtl,
            cell: frame.size,
            icon_top: icon.min_y() - frame.min_y(),
            icon_size: self.params.icon_size,
            gap: label.min_y() - icon.max_y(),
            label: label.size,
            text_size: self.params.text_size,
        })
    }

    /// Selects the next item whose name starts with what was typed in the last second: after the cursor for a new
    /// search, from the cursor while it goes on, wrapping around.
    fn type_select(
        &self,
        state: &mut TypeSelect,
        key: &KeyEvent,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        let Some(c) = key.text else {
            return Propagation::Continue;
        };
        let now = cx.now();
        if state.typed_until.is_none_or(|until| now >= until) {
            state.typed.clear();
        }
        if c.is_control() || (c == ' ' && state.typed.is_empty()) {
            return Propagation::Continue;
        }
        state.typed.extend(c.to_lowercase());
        state.typed_until = Some(Timestamp(now.0 + TYPE_SELECT_NS));
        let count = self.count();
        let start = match self.cursor() {
            Some(c) if state.typed.chars().count() == 1 => c + 1,
            Some(c) => c,
            None => 0,
        };
        let found = (0..count)
            .map(|k| (start + k) % count)
            .find(|&i| self.items[i].name.to_lowercase().starts_with(&state.typed));
        if let Some(index) = found {
            self.move_to(index, false, cx);
        }
        Propagation::Stop
    }
}

impl Widget<Message> for IconGrid {
    type State = TypeSelect;

    fn layout(&self) -> LayoutSpec {
        if self.strip {
            return LayoutSpec {
                width: SizeSpec::Fixed(self.layout.content.w.max(self.scroll.viewport.width)),
                height: SizeSpec::Fixed(self.layout.content.h),
                ..LayoutSpec::new(ContainerKind::Absolute)
            };
        }
        LayoutSpec {
            width: SizeSpec::Fill,
            // as tall as the view at least, so a press under the last row starts a band
            height: SizeSpec::Fixed(
                self.layout
                    .content
                    .h
                    .max(self.scroll.viewport.height - self.lead),
            ),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        let at = self.at();
        let pick = self.pick();
        let side = self.params.icon_box().w;
        let band = self.band.map(band::view);
        self.built()
            .into_iter()
            .map(|index| {
                let cell = &self.layout.cells[index];
                let selected = self.selection.contains(index);
                let frame = rect(cell.frame);
                let icon = rect(cell.icon_frame);
                let label = rect(cell.label_frame);
                let (x, y) = (icon.min_x() - frame.min_x(), icon.min_y() - frame.min_y());
                let children = vec![
                    Element::new(Image {
                        sources: self.items[index].icon.clone(),
                        size: Size::new(self.params.icon_size, self.params.icon_size),
                        fit: ImageFit::Contain,
                        radius: 0.0,
                        label: None,
                    }),
                    Element::new(NameBox {
                        size: label.size,
                        children: vec![(self.name)(index, selected)],
                    }),
                ];
                Element::new(Cell {
                    index,
                    frame,
                    icon: Rect::new(
                        x + (icon.size.width - side) / 2.0,
                        y + (icon.size.height - side) / 2.0,
                        side,
                        side,
                    ),
                    icon_top: y,
                    gap: label.min_y() - icon.max_y(),
                    selected,
                    ringed: self.ringed.contains(index),
                    stop: at == Some(index),
                    name: self.items[index].name.clone(),
                    pick: Rc::clone(&pick),
                    children,
                })
                .key(cell_key(self.window, index))
            })
            .chain(band)
            .collect()
    }

    fn state(&self) -> TypeSelect {
        TypeSelect::default()
    }

    fn update(&self, old: &Self, _: &mut TypeSelect) -> Change {
        if self.layout != old.layout || self.scroll.viewport != old.scroll.viewport {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &TypeSelect, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        state: &mut TypeSelect,
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
                if mods.contains(Mods::CTRL) {
                    // Ctrl with a press is the secondary click
                    state.clicks.cancel();
                    let index = self.index_at(position, cx);
                    if index.is_some() {
                        cx.emit(Message::Window(
                            self.window,
                            WindowEvent::SecondaryPress(index),
                        ));
                    }
                    cx.emit(Message::Window(
                        self.window,
                        WindowEvent::SecondaryPress(None),
                    ));
                    cx.emit(Message::Window(
                        self.window,
                        WindowEvent::ContextMenu(position),
                    ));
                    return Propagation::Stop;
                }
                let mut next = self.selection.clone();
                let index = self.index_at(position, cx);
                let Some(pressed) = index else {
                    state.clicks.press_empty();
                    if self.strip {
                        // between a strip's pictures: the item shown stays
                        return Propagation::Stop;
                    }
                    if !mods.contains(Mods::SHIFT) && !mods.contains(Mods::PRIMARY) {
                        next = Selection::new([], None, None);
                    }
                    self.select(next, cx);
                    let origin = cx.bounds().map_or(Point::ZERO, |b| {
                        Point::new(position.x - b.min_x(), position.y - b.min_y())
                    });
                    state.band = Some(Band::start(origin, position, mods, &self.selection));
                    cx.capture_pointer();
                    return Propagation::Stop;
                };
                let sole = self.selection.len() == 1 && self.selection.contains(pressed);
                let on_name = self.on_name(pressed, position, cx);
                let plain = !mods.contains(Mods::SHIFT) && !mods.contains(Mods::PRIMARY);
                if state.clicks.press(pressed, on_name, sole, plain, cx.now()) {
                    cx.emit(Message::Window(self.window, WindowEvent::Open(pressed)));
                    return Propagation::Stop;
                }
                state.clicks.arm(cx);
                // a plain press on a selected item keeps the selection for a drag; the release selects it alone
                let keep = !mods.contains(Mods::SHIFT)
                    && !mods.contains(Mods::PRIMARY)
                    && index.is_some_and(|i| self.selection.contains(i));
                match index {
                    _ if keep => {}
                    Some(index) if mods.contains(Mods::SHIFT) => next.extend_to(index),
                    Some(index) if mods.contains(Mods::PRIMARY) => next.toggle(index),
                    Some(index) => next.select_only(index),
                    None if mods.contains(Mods::SHIFT) || mods.contains(Mods::PRIMARY) => {}
                    None => next = Selection::new([], None, None),
                }
                state.press = index.filter(|&i| next.contains(i)).map(|index| {
                    let rest = next.ranges().iter().flat_map(Clone::clone);
                    Press {
                        index,
                        at: position,
                        dragged: std::iter::once(index)
                            .chain(rest.filter(|&i| i != index && i < self.count()))
                            .collect(),
                        collapse: keep,
                    }
                });
                if state.press.is_some() {
                    cx.capture_pointer();
                }
                self.select(next, cx);
                Propagation::Stop
            }
            // the context menu opens from what holds the grid; it learns which item was pressed first
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                if let Some(index) = self.index_at(position, cx) {
                    cx.emit(Message::Window(
                        self.window,
                        WindowEvent::SecondaryPress(Some(index)),
                    ));
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                if let Some(band) = state.band.as_mut() {
                    band.pointer = position;
                    let band = band.clone();
                    self.draw_band(&band, cx);
                    if let Some(shown) = self.shown(cx) {
                        band::follow(self.window, self.scroll, position, shown, cx);
                    }
                    return Propagation::Stop;
                }
                let Some(press) = state.press.as_ref() else {
                    return Propagation::Continue;
                };
                let moved = (position.x - press.at.x).hypot(position.y - press.at.y);
                if moved < drag::START_DISTANCE {
                    return Propagation::Stop;
                }
                let press = state.press.take().expect("held above");
                state.clicks.cancel();
                cx.release_pointer();
                if let Some(grab) = self.grab(&press, cx) {
                    cx.emit(Message::Window(self.window, WindowEvent::DragOut(grab)));
                }
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                if state.band.take().is_some() {
                    cx.release_pointer();
                    cx.emit(Message::Window(self.window, WindowEvent::Band(None)));
                    return Propagation::Stop;
                }
                let Some(press) = state.press.take() else {
                    return Propagation::Continue;
                };
                cx.release_pointer();
                // a rename that came due while the button was held starts now
                state.clicks.arm(cx);
                if press.collapse {
                    let mut next = self.selection.clone();
                    next.select_only(press.index);
                    self.select(next, cx);
                }
                Propagation::Stop
            }
            Event::Cancel => {
                state.press = None;
                state.clicks.cancel();
                if state.band.take().is_some() {
                    cx.emit(Message::Window(self.window, WindowEvent::Band(None)));
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Scroll { .. }, _) => {
                state.clicks.cancel();
                Propagation::Continue
            }
            Event::Wake(now) => {
                let held = state.press.is_some();
                state.clicks.wake(self.window, now, held, cx);
                if let Some(band) = state.band.clone() {
                    // the content moved under a still pointer: the band follows, and the scroll goes on
                    self.draw_band(&band, cx);
                    if let Some(shown) = self.shown(cx) {
                        band::follow(self.window, self.scroll, band.pointer, shown, cx);
                    }
                }
                Propagation::Continue
            }
            Event::Key(key, _) if key.pressed => {
                state.clicks.cancel();
                self.key(state, &key, cx)
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &TypeSelect, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &TypeSelect, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            multiselectable: true,
            ..AccessibleStates::default()
        });
        cx.item_count(self.count());
        cx.roving_group();
    }
}

/// One built cell: its icon over its name, the highlight of a selected icon, and the focus at the keyboard's item.
struct Cell {
    index: usize,
    /// In the grid.
    frame: Rect,
    /// The icon with its padding, in the cell.
    icon: Rect,
    /// From the top of the cell to the icon's.
    icon_top: f64,
    /// Between the icon and its name.
    gap: f64,
    selected: bool,
    /// A context menu is open on the item: its selection's shape wears an accent ring.
    ringed: bool,
    /// The grid's one Tab stop.
    stop: bool,
    name: String,
    pick: Pick,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Cell {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: self.icon_top,
                ..Insets::ZERO
            },
            width: SizeSpec::Fixed(self.frame.size.width),
            height: SizeSpec::Fixed(self.frame.size.height),
            position: self.frame.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: self.gap,
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
        if (self.frame, self.icon, self.icon_top, self.gap)
            != (old.frame, old.icon, old.icon_top, old.gap)
        {
            Change::Layout
        } else if (self.selected, self.ringed, self.stop, &self.name)
            != (old.selected, old.ringed, old.stop, &old.name)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    /// A press focuses the cell without showing the ring or scrolling, then goes on to the grid, which selects.
    /// Assistive technology selects a cell with its Click action.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        match event {
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                if let Some(message) = (self.pick)(self.index) {
                    cx.emit(message);
                }
                return Propagation::Stop;
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => cx.request_focus(),
            Event::Focus { .. } if cx.node() == cx.target() => cx.request_paint(),
            _ => {}
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        // the bubble: the icon grown by its padding, in the same colour in an active and an inactive window, with a
        // white border
        let longest = self.icon.size.width.max(self.icon.size.height);
        let pad = longest / sel::BUBBLE_DIVISOR;
        let round = longest * sel::BUBBLE_RADIUS_RATIO;
        let bubble = self.icon.outset(pad);
        if self.selected {
            if let Some(color) = theme
                .colors
                .get(ColorRole::QuaternaryLabel, StateVariant::NORMAL)
            {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: bubble,
                        radius: round,
                    },
                    paint: Paint::Solid(color),
                });
            }
            let border = pad.trunc().min(sel::BUBBLE_BORDER_MAX);
            if border >= 1.0 {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: bubble.outset(-border / 2.0),
                        radius: (round - border / 2.0).max(0.0),
                    },
                    paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, sel::BUBBLE_BORDER_ALPHA)),
                    width: border,
                });
            }
        }
        if self.ringed {
            if let Some(accent) = theme
                .colors
                .get(ColorRole::ControlFill, selected_variant(cx.window_active()))
            {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: bubble.outset(-sel::CONTEXT_RING / 2.0),
                        radius: (round - sel::CONTEXT_RING / 2.0).max(0.0),
                    },
                    paint: Paint::Solid(accent),
                    width: sel::CONTEXT_RING,
                });
            }
        }
        if cx.is_focused() && cx.focus_visible() && cx.window_active() {
            let ring = &theme.focus;
            if let Some(color) = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: bubble.outset(ring.outset + ring.width / 2.0),
                        radius: round + ring.radius_extra,
                    },
                    paint: Paint::Solid(color),
                    width: ring.width,
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
        cx.item_index(self.index);
        cx.action(Action::Click);
        cx.roving_item();
        if self.stop {
            cx.focusable();
        }
    }
}

/// The room of a cell's name: its width, and as many lines as the layout gives it; the name is centred in it.
struct NameBox {
    size: Size,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for NameBox {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
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
        if self.size != old.size {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// An item's name in `text_size` points, wrapping in its cell, on the selected control colour when `selected`. With
/// `max_lines` it keeps that many lines at most, the last ending in an ellipsis when the name goes on past it.
pub(super) fn name(
    text: String,
    text_size: f64,
    selected: bool,
    max_lines: Option<u32>,
) -> Element<Message> {
    Element::new(Highlight {
        selected,
        children: vec![Element::new(Name {
            text: Text {
                content: AttributedText::plain(text, TextStyle::new(text_size as f32)),
                role: TextRole::Body,
                wrap: true,
                secondary: false,
                selected,
            },
            max_lines,
        })],
    })
}

/// A wrapping body-text name, drawn as [`Text`] draws it, whose lines stop at `max_lines` with an ellipsis.
struct Name {
    text: Text,
    max_lines: Option<u32>,
}

impl Name {
    /// The name in the view's text size (scaled with the body role) and the body weight, each line centred, laid out within `width`; `None` if the fonts
    /// cannot shape it. A name that runs past `max_lines` keeps its first lines and has the rest cut in the middle to
    /// what the last line holds, so its end (the extension) stays in view: "report with a…middle.xlsx".
    fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        let role = env.theme.type_scale.body;
        let mut content = self.text.content.clone();
        for (_, style) in &mut content.runs {
            // the view's text size, grown or shrunk as the body role is against its usual size
            style.size *= role.size / USUAL_BODY_SIZE;
            if style.weight == FontWeight::REGULAR {
                style.weight = FontWeight(role.weight);
            }
        }
        let wrap = |wrap| ParagraphStyle {
            align: TextAlign::Center,
            wrap,
            ..ParagraphStyle::default()
        };
        let room = width.is_finite().then_some(width as f32);
        let full = env
            .text
            .paragraph(&content, &wrap(Wrap::Word))
            .ok()?
            .layout(room);
        let Some(max) = self.max_lines.map(|n| n.max(1) as usize) else {
            return Some(full);
        };
        if full.lines().len() <= max || room.is_none() {
            return Some(full);
        }
        let text = &content.text;
        let cut = full.lines()[max - 1].range.0.start;
        let (head, rest) = text.split_at(cut);
        let style = content
            .runs
            .first()
            .map(|(_, style)| style.clone())
            .unwrap_or_else(|| TextStyle::new(role.size));
        let fits = |candidate: &str| {
            env.text
                .paragraph(
                    &AttributedText::plain(candidate, style.clone()),
                    &wrap(Wrap::None),
                )
                .is_ok_and(|p| p.layout(None).size().width <= width)
        };
        let chars: Vec<char> = rest.trim_start().chars().collect();
        let middle = |keep: usize| -> String {
            let front = keep.div_ceil(2);
            let back = keep - front;
            chars[..front]
                .iter()
                .chain(std::iter::once(&'\u{2026}'))
                .chain(&chars[chars.len() - back..])
                .collect()
        };
        // the most characters that fit around the ellipsis on one line
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(&middle(mid)) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let shown = format!("{}\n{}", head.trim_end(), middle(lo));
        let paragraph = env
            .text
            .paragraph(&AttributedText::plain(shown, style), &wrap(Wrap::Word))
            .ok()?;
        Some(paragraph.layout(room))
    }
}

impl Widget<Message> for Name {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        // as for a text, the colours are paint; what is set, in which role, how it wraps and on how many lines is size
        let (new, was) = (&self.text, &old.text);
        if new.content != was.content
            || new.role != was.role
            || new.wrap != was.wrap
            || self.max_lines != old.max_lines
        {
            Change::Layout
        } else if new != was {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        match self.laid_out(cx.env(), constraints.width.max()) {
            Some(layout) => Measured {
                size: layout.size(),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => Measured {
                size: Size::ZERO,
                baseline: None,
            },
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let width = cx.size().width;
        let Some(layout) = self.laid_out(cx.env(), width) else {
            return;
        };
        let (role, variant) = if self.text.selected {
            (ColorRole::ControlText, selected_variant(cx.window_active()))
        } else {
            (ColorRole::Text, StateVariant::NORMAL)
        };
        let Some(color) = cx.env().theme.colors.get(role, variant) else {
            return;
        };
        // each line is centred in the layout's width, which is the name's
        let x = 0.0;
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: Point::new(x, 0.0) + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.content.text.clone());
    }
}

/// Fits its text and paints the selection behind it.
struct Highlight {
    selected: bool,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Highlight {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: NAME_INSETS,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
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
        if self.selected != old.selected {
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
        if !self.selected {
            return;
        }
        let size = cx.size();
        let variant = selected_variant(cx.window_active());
        if let Some(color) = cx.env().theme.colors.get(ColorRole::ControlFill, variant) {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: NAME_RADIUS,
                },
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cells_key_names_its_window_and_gives_its_index_back() {
        assert_ne!(cell_key(1, 4), cell_key(2, 4));
        assert_eq!(cell_index(&cell_key(1, 4)), Some(4));
        assert_eq!(cell_index(&Key::Int(4)), None);
        assert_eq!(cell_index(&Key::from("row/1/x")), None);
    }

    #[test]
    fn selecting_a_name_repaints_it_and_a_new_name_or_line_count_lays_it_out() {
        let name = |text: &str, selected: bool, max_lines: Option<u32>| Name {
            text: Text {
                content: AttributedText::plain(text.to_owned(), TextStyle::new(12.0)),
                role: TextRole::Body,
                wrap: true,
                secondary: false,
                selected,
            },
            max_lines,
        };
        let mut state = ();
        let shown = name("a.txt", false, Some(2));
        let update =
            |new: &Name, old: &Name, state: &mut ()| Widget::<Message>::update(new, old, state);
        assert_eq!(update(&shown, &shown, &mut state), Change::None);
        assert_eq!(
            update(&name("a.txt", true, Some(2)), &shown, &mut state),
            Change::Paint
        );
        assert_eq!(
            update(&name("b.txt", false, Some(2)), &shown, &mut state),
            Change::Layout
        );
        assert_eq!(
            update(&name("a.txt", false, Some(3)), &shown, &mut state),
            Change::Layout
        );
    }
    use crate::layout::icon::{self, Flow, FontMetrics, IconParams, SectionSpec};

    /// Ten items of 64 pt icons in a grid four cells wide: rows of 0-3, 4-7 and 8-9.
    fn grid() -> IconLayout {
        let params = IconParams::new(64, 54, 12, true, false, FontMetrics::estimate(12.0));
        let width = 4.0 * 112.0 + 2.0 * icon::MARGIN;
        let layout = icon::layout(
            &params,
            width,
            &[SectionSpec {
                title: None,
                count: 10,
            }],
            Flow::Leading,
        );
        assert_eq!(layout.columns, 4);
        layout
    }

    #[test]
    fn arrows_go_to_the_next_cell_that_way_and_stay_at_the_edges() {
        let g = grid();
        let toward = |from, d| target(&g, from, Step::Toward(d), 1);
        assert_eq!(toward(5, Direction::Right), 6);
        assert_eq!(toward(5, Direction::Left), 4);
        assert_eq!(toward(5, Direction::Up), 1);
        assert_eq!(toward(5, Direction::Down), 9);
        assert_eq!(toward(3, Direction::Right), 3);
        assert_eq!(toward(4, Direction::Left), 4);
        assert_eq!(toward(1, Direction::Up), 1);
        assert_eq!(toward(9, Direction::Down), 9);
    }

    #[test]
    fn down_from_over_the_short_last_row_goes_to_its_nearest_item() {
        let g = grid();
        assert_eq!(target(&g, 7, Step::Toward(Direction::Down), 1), 9);
    }

    #[test]
    fn home_end_and_pages_of_rows() {
        let g = grid();
        assert_eq!(target(&g, 6, Step::Home, 1), 0);
        assert_eq!(target(&g, 1, Step::End, 1), 9);
        // a page of two rows, then one, of four items
        assert_eq!(target(&g, 1, Step::PageDown, 8), 9);
        assert_eq!(target(&g, 1, Step::PageDown, 4), 5);
        assert_eq!(target(&g, 9, Step::PageUp, 4), 5);
        assert_eq!(target(&g, 6, Step::PageUp, 8), 0);
        assert_eq!(
            target(&g, 6, Step::PageUp, 0),
            5,
            "a page is at least an item"
        );
    }
}
