//! The folder as a grid of icons with their names under them, laid out by [`crate::layout::icon`]. It builds only the
//! cells near the viewport plus the cursor's. The model keeps the selection: clicks and keys report a new one and move
//! the focus to the cursor's cell, which the runtime scrolls into view. Arrows go to the nearest cell that way
//! ([`IconLayout::neighbour`], mirrored in right-to-left layouts), Page Up and Page Down by the rows in view, Home and
//! End to the first and last item, Return opens, and typing selects by name. Read as a list of items.

use std::rc::Rc;

use cupertino_kit::controls::{Icon, IconDirection, Selection, Text, TextRole};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, FocusTarget, Key, MeasureCx,
    PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, ScrollState, SizeSpec,
};
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{ColorRole, InteractionVariant, StateVariant};

use super::Message;
use crate::geom::{self, Direction};
use crate::layout::icon::{IconLayout, IconParams};

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

/// How far past the viewport cells are built, in points.
const OVERSCAN: f64 = 200.0;
/// How long typed letters add to one search (1 s).
const TYPE_SELECT_NS: u64 = 1_000_000_000;
/// Corner radius of the highlight behind a selected icon and behind its name (approximate).
const ICON_RADIUS: f64 = 5.0;
const NAME_RADIUS: f64 = 4.0;
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

/// The item `step` moves the keyboard to from item `from` of a non-empty `layout`, `rows` rows making a page. A move
/// past the edge stays.
pub(super) fn target(layout: &IconLayout, from: usize, step: Step, rows: usize) -> usize {
    let last = layout.cells.len().saturating_sub(1);
    let page = rows.max(1) * layout.columns;
    match step {
        Step::Toward(direction) => layout.neighbour(from, direction).unwrap_or(from),
        Step::Home => 0,
        Step::End => last,
        Step::PageUp => from.saturating_sub(page),
        Step::PageDown => (from + page).min(last),
    }
}

/// An item the grid shows: its name and the symbol of its icon.
pub(super) struct Item {
    pub name: String,
    pub symbol: &'static str,
}

/// The content of the grid's scroll view: as tall as every cell, holding the cells that are built.
pub(super) struct IconGrid {
    /// The accessible name.
    pub label: String,
    pub layout: Rc<IconLayout>,
    pub params: IconParams,
    pub items: Rc<[Item]>,
    pub scroll: ScrollState,
    pub selection: Selection,
    /// What sits under item `index`'s icon, given as `(index, selected)`: its [`name`], or the field it is renamed in.
    pub name: Box<dyn Fn(usize, bool) -> Element<Message>>,
}

/// The node state of the grid: what was typed to select, and when that search ends.
#[derive(Default)]
pub(super) struct TypeSelect {
    typed: String,
    typed_until: Option<Timestamp>,
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
            let top = self.scroll.offset.y;
            let cells = &self.layout.cells;
            let first = cells.iter().position(|c| c.frame.y + c.frame.h > top);
            first.or(cells.len().checked_sub(1))
        })
    }

    /// Rows of cells a page moves by: those the viewport shows.
    fn page_rows(&self) -> usize {
        let pitch = self.layout.cell.h + self.layout.spacing.1;
        (self.scroll.viewport.height / pitch).floor().max(1.0) as usize
    }

    /// The cells to build, in order: those near the viewport and the keyboard's.
    fn built(&self) -> Vec<usize> {
        let top = self.scroll.offset.y - OVERSCAN;
        let bottom = self.scroll.offset.y + self.scroll.viewport.height + OVERSCAN;
        let at = self.at();
        self.layout
            .cells
            .iter()
            .filter(|c| {
                (c.frame.y + c.frame.h >= top && c.frame.y <= bottom) || Some(c.index) == at
            })
            .map(|c| c.index)
            .collect()
    }

    /// The item whose icon or name is under a pointer at `position`, in window coordinates.
    fn index_at(&self, position: Point, cx: &EventCx<'_, Message>) -> Option<usize> {
        let bounds = cx.bounds()?;
        let x = position.x - bounds.min_x();
        let x = match cx.env().direction {
            LayoutDirection::Ltr => x,
            LayoutDirection::Rtl => bounds.size.width - x,
        };
        self.layout.hit(geom::Point {
            x,
            y: position.y - bounds.min_y(),
        })
    }

    fn pick(&self) -> Pick {
        let selection = self.selection.clone();
        Rc::new(move |index| {
            let mut next = selection.clone();
            next.select_only(index);
            (next != selection).then_some(Message::Select(next))
        })
    }

    fn select(&self, next: Selection, cx: &mut EventCx<'_, Message>) {
        if next != self.selection {
            cx.emit(Message::Select(next));
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
        cx.focus_item(FocusTarget::Key(Key::Int(index as u64)));
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
            RETURN | KP_ENTER if plain => {
                return match self.cursor() {
                    Some(c) => {
                        cx.emit(Message::Open(c));
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                };
            }
            _ if plain => return self.type_select(state, key, cx),
            _ => return Propagation::Continue,
        };
        let to = match self.cursor() {
            Some(c) => target(&self.layout, c, step, self.page_rows()),
            None => at,
        };
        self.move_to(to, mods.contains(Mods::SHIFT), cx);
        Propagation::Stop
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
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.layout.content.h),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        let at = self.at();
        let pick = self.pick();
        let side = self.params.icon_box().w;
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
                    Element::new(Icon {
                        name: self.items[index].symbol.into(),
                        size: self.params.icon_size,
                        direction: IconDirection::Fixed,
                        label: None,
                        tint: None,
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
                    stop: at == Some(index),
                    name: self.items[index].name.clone(),
                    pick: Rc::clone(&pick),
                    children,
                })
                .key(Key::Int(index as u64))
            })
            .collect()
    }

    fn state(&self) -> TypeSelect {
        TypeSelect::default()
    }

    fn update(&self, old: &Self, _: &mut TypeSelect) -> Change {
        if self.layout != old.layout {
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
                let mut next = self.selection.clone();
                match self.index_at(position, cx) {
                    Some(index) if mods.contains(Mods::SHIFT) => next.extend_to(index),
                    Some(index) if mods.contains(Mods::PRIMARY) => next.toggle(index),
                    Some(index) => next.select_only(index),
                    None if mods.contains(Mods::SHIFT) || mods.contains(Mods::PRIMARY) => {}
                    None => next = Selection::new([], None, None),
                }
                self.select(next, cx);
                Propagation::Stop
            }
            Event::Key(key, _) if key.pressed => self.key(state, &key, cx),
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
        } else if (self.selected, self.stop, &self.name) != (old.selected, old.stop, &old.name) {
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
        if self.selected {
            let variant = StateVariant {
                window_active: cx.window_active(),
                ..StateVariant::NORMAL
            };
            if let Some(color) = theme
                .colors
                .get(ColorRole::SecondarySelectedControlFill, variant)
            {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: self.icon,
                        radius: ICON_RADIUS,
                    },
                    paint: Paint::Solid(color),
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
                        rect: self.icon.outset(ring.outset + ring.width / 2.0),
                        radius: ICON_RADIUS + ring.radius_extra,
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

/// An item's name in `text_size` points, wrapping in its cell, on the selected control colour when `selected`.
pub(super) fn name(text: String, text_size: f64, selected: bool) -> Element<Message> {
    Element::new(Highlight {
        selected,
        children: vec![Element::new(Text {
            content: AttributedText::plain(text, TextStyle::new(text_size as f32)),
            role: TextRole::Body,
            wrap: true,
            secondary: false,
            selected,
        })],
    })
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
        assert_eq!(target(&g, 1, Step::PageDown, 2), 9);
        assert_eq!(target(&g, 1, Step::PageDown, 1), 5);
        assert_eq!(target(&g, 9, Step::PageUp, 1), 5);
        assert_eq!(target(&g, 6, Step::PageUp, 2), 0);
        assert_eq!(
            target(&g, 6, Step::PageUp, 0),
            2,
            "a page is at least a row"
        );
    }
}
