//! ListView and TableView: a scroll view around one content node as tall as every row, which builds only the rows near
//! the viewport plus the cursor row, each at its index times the row height. The model keeps the offset and the
//! selection; keys and clicks report a new selection and move the focus to the cursor row, which the runtime scrolls
//! into view.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::scroll::{self, Frame};
use crate::{
    Column, ColumnWidth, Handler, Icon, IconDirection, ListView, Scroller, Selection, SortOrder,
    TableView, Text, TextRole,
};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Event, EventCx, FocusTarget, Key,
    MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, ItemMetrics, Justify, LayoutDirection,
    LayoutSpec, Measured, ScrollState, SizeSpec, VirtualList,
};
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::ops::Range;
use std::rc::Rc;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const HOME: Keysym = Keysym(0xff50);
const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const END: Keysym = Keysym(0xff57);
const A: Keysym = Keysym(0x61);
const SHIFTED_A: Keysym = Keysym(0x41);

/// Built rows beyond the viewport on each side, in viewports.
const OVERSCAN: f64 = 0.5;
/// Between a row's edges and its selection.
const SELECTION_INSET: f64 = 4.0;
const SELECTION_RADIUS: f64 = 5.0;
/// Between a row's edges and its content; an approximate design value.
const CONTENT_INSET: f64 = 10.0;
/// Between a cell's edges and its content, so neighbouring columns keep apart; an approximate design value.
const CELL_INSET: f64 = 4.0;
const HEADER_HEIGHT: f64 = 28.0;
const HEADER_TEXT_SIZE: f32 = 11.0;
const HEADER_TEXT_WEIGHT: u16 = 600;
const CHEVRON: f64 = 9.0;
const CHEVRON_GAP: f64 = 4.0;
/// How long typed characters keep adding to one search.
const TYPE_SELECT_NS: u64 = 1_000_000_000;

impl Selection {
    /// The rows in `ranges`, in any order and overlapping or not, with the cursor and the anchor: what an application
    /// builds after it moved its rows.
    pub fn new(
        ranges: impl IntoIterator<Item = Range<usize>>,
        cursor: Option<usize>,
        anchor: Option<usize>,
    ) -> Self {
        let mut selection = Self {
            ranges: Vec::new(),
            cursor,
            anchor,
        };
        for range in ranges.into_iter().filter(|r| r.start < r.end) {
            selection.insert(range);
        }
        selection
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub fn contains(&self, index: usize) -> bool {
        let at = self.ranges.partition_point(|r| r.end <= index);
        self.ranges.get(at).is_some_and(|r| r.contains(&index))
    }

    /// How many rows are selected.
    pub fn len(&self) -> usize {
        self.ranges.iter().map(|r| r.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Selects `index` alone and puts the cursor and the anchor on it.
    pub fn select_only(&mut self, index: usize) {
        self.ranges.clear();
        self.ranges.push(index..index + 1);
        self.cursor = Some(index);
        self.anchor = Some(index);
    }

    /// Selects from the anchor to `index` and moves the cursor there. The rows the last extension from the anchor
    /// selected are given up; rows selected otherwise stay.
    pub fn extend_to(&mut self, index: usize) {
        let anchor = self.anchor.or(self.cursor).unwrap_or(index);
        if let Some(cursor) = self.cursor {
            self.remove(anchor.min(cursor)..anchor.max(cursor) + 1);
        }
        self.insert(anchor.min(index)..anchor.max(index) + 1);
        self.cursor = Some(index);
        self.anchor = Some(anchor);
    }

    /// Selects `index` if it was not, and the other way round; the cursor and the anchor move to it.
    pub fn toggle(&mut self, index: usize) {
        if self.contains(index) {
            self.remove(index..index + 1);
        } else {
            self.insert(index..index + 1);
        }
        self.cursor = Some(index);
        self.anchor = Some(index);
    }

    /// Selects all `count` rows; the cursor stays.
    pub fn select_all(&mut self, count: usize) {
        self.ranges.clear();
        if count > 0 {
            self.ranges.push(0..count);
        }
    }

    /// Drops what lies past the last of `count` rows.
    pub fn clamp(&mut self, count: usize) {
        self.ranges.retain_mut(|r| {
            r.end = r.end.min(count);
            r.start < r.end
        });
        self.cursor = self.cursor.filter(|&c| c < count);
        self.anchor = self.anchor.filter(|&a| a < count);
    }

    fn insert(&mut self, range: Range<usize>) {
        let start = self.ranges.partition_point(|r| r.end < range.start);
        let end = self.ranges.partition_point(|r| r.start <= range.end);
        let merged = self.ranges[start..end]
            .iter()
            .fold(range, |m, r| m.start.min(r.start)..m.end.max(r.end));
        self.ranges.splice(start..end, [merged]);
    }

    fn remove(&mut self, range: Range<usize>) {
        let start = self.ranges.partition_point(|r| r.end <= range.start);
        let end = self.ranges.partition_point(|r| r.start < range.end);
        let rest: Vec<Range<usize>> = self.ranges[start..end]
            .iter()
            .flat_map(|r| {
                [
                    r.start..range.start.max(r.start),
                    range.end.min(r.end)..r.end,
                ]
            })
            .filter(|r| !r.is_empty())
            .collect();
        self.ranges.splice(start..end, rest);
    }
}

/// What the rows are to accessibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    List,
    Table,
}

impl<M: 'static> From<ListView<M>> for Element<M> {
    fn from(list: ListView<M>) -> Self {
        let rows = Rows {
            kind: Kind::List,
            label: list.label,
            count: list.count,
            row_height: list.row_height,
            scroll: list.scroll,
            selection: list.selection,
            multiple: list.multiple,
            row: list.row,
            type_select: list.type_select,
            on_select: list.on_select,
            on_activate: list.on_activate,
        };
        scroll::frame(
            Element::new(rows),
            list.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(list.on_scroll),
        )
    }
}

impl<M: 'static> From<TableView<M>> for Element<M> {
    fn from(table: TableView<M>) -> Self {
        let columns: Rc<[Column]> = table.columns.into();
        let cell = table.cell;
        let widths: Rc<[ColumnWidth]> = columns.iter().map(|c| c.width).collect();
        let row: Box<dyn Fn(usize, bool) -> Element<M>> = Box::new(move |index, selected| {
            let cells = widths
                .iter()
                .enumerate()
                .map(|(column, &width)| {
                    Element::new(Cell {
                        width,
                        header: None,
                        content: vec![cell(index, column, selected)],
                    })
                })
                .collect();
            Element::new(Cells { cells })
        });
        let rows = Rows {
            kind: Kind::Table,
            label: table.label,
            count: table.count,
            row_height: table.row_height,
            scroll: table.scroll,
            selection: table.selection,
            multiple: table.multiple,
            row,
            type_select: table.type_select,
            on_select: table.on_select,
            on_activate: table.on_activate,
        };
        let on_sort = table.on_sort.map(Rc::new);
        let header = Element::new(Header {
            cells: columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    let order = table.sort.filter(|&(c, _)| c == index).map(|(_, o)| o);
                    let last = index + 1 == columns.len();
                    header_cell(index, last, column, order, on_sort.clone())
                })
                .collect(),
        });
        let body = scroll::frame(
            Element::new(rows),
            table.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(table.on_scroll),
        );
        Element::new(Frame {
            kind: Some(Axis::Vertical),
            children: vec![header, body],
        })
    }
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// The content of a list's scroll view: every row's height, holding the rows that are built.
struct Rows<M> {
    kind: Kind,
    label: String,
    count: usize,
    row_height: f64,
    scroll: ScrollState,
    selection: Selection,
    multiple: bool,
    row: Box<dyn Fn(usize, bool) -> Element<M>>,
    type_select: Option<Box<dyn Fn(usize) -> String>>,
    on_select: Option<Handler<Selection, M>>,
    on_activate: Option<Handler<usize, M>>,
}

/// The node state of a list. Opaque.
#[derive(Clone, Debug, Default)]
pub struct ListState {
    /// What was typed to select, and when that search ends.
    typed: String,
    typed_until: Option<Timestamp>,
}

impl<M: 'static> Rows<M> {
    /// The row the keyboard is on: the selection's cursor, else the first row in view.
    fn cursor(&self) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let top = (self.scroll.offset.y / self.row_height).floor();
        let first = if top.is_finite() {
            top.max(0.0) as usize
        } else {
            0
        };
        Some(
            self.selection
                .cursor
                .filter(|&c| c < self.count)
                .unwrap_or(first.min(self.count - 1)),
        )
    }

    /// The rows to build, in order: those near the viewport and the cursor row.
    fn built(&self) -> Vec<usize> {
        let Ok(mut list) = VirtualList::new(self.count, ItemMetrics::Fixed(self.row_height)) else {
            return Vec::new();
        };
        let window = list.window(self.scroll.offset.y, self.scroll.viewport.height, OVERSCAN);
        let mut rows: Vec<usize> = (window.first..window.last).collect();
        match self.cursor() {
            Some(c) if c < window.first => rows.insert(0, c),
            Some(c) if c >= window.last => rows.push(c),
            _ => {}
        }
        rows
    }

    /// The row under a pointer at `position`, in window coordinates.
    fn index_at(&self, position: Point, cx: &EventCx<'_, M>) -> Option<usize> {
        let y = position.y - cx.bounds()?.min_y();
        let index = (y / self.row_height).floor();
        (index >= 0.0 && index < self.count as f64).then_some(index as usize)
    }

    fn select(&self, next: Selection, cx: &mut EventCx<'_, M>) {
        if next != self.selection {
            if let Some(on_select) = &self.on_select {
                cx.emit(on_select(next));
            }
        }
    }

    /// Moves the cursor to `index`, selecting it alone or extending to it, and the focus with it.
    fn move_to(&self, index: usize, extend: bool, cx: &mut EventCx<'_, M>) {
        let mut next = self.selection.clone();
        if extend && self.multiple {
            next.extend_to(index);
        } else {
            next.select_only(index);
        }
        self.select(next, cx);
        cx.focus_item(FocusTarget::Key(Key::Int(index as u64)));
    }

    fn key(&self, state: &mut ListState, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(at) = self.cursor() else {
            return Propagation::Continue;
        };
        let mods = key.mods;
        let plain = !mods.contains(Mods::CTRL)
            && !mods.contains(Mods::ALT)
            && !mods.contains(Mods::PRIMARY);
        let shift = mods.contains(Mods::SHIFT);
        let last = self.count - 1;
        let page = ((self.scroll.viewport.height / self.row_height).floor() as usize).max(1);
        let cursor = self.selection.cursor.filter(|&c| c < self.count);
        let to = match key.keysym {
            UP if plain => cursor.map_or(at, |c| c.saturating_sub(1)),
            DOWN if plain => cursor.map_or(at, |c| (c + 1).min(last)),
            HOME if plain => 0,
            END if plain => last,
            PAGE_UP if plain => at.saturating_sub(page),
            PAGE_DOWN if plain => (at + page).min(last),
            A | SHIFTED_A if self.multiple && mods.contains(Mods::PRIMARY) => {
                let mut next = self.selection.clone();
                next.select_all(self.count);
                self.select(next, cx);
                return Propagation::Stop;
            }
            RETURN | KP_ENTER if plain => {
                return match (cursor, &self.on_activate) {
                    (Some(c), Some(on_activate)) => {
                        cx.emit(on_activate(c));
                        Propagation::Stop
                    }
                    _ => Propagation::Continue,
                };
            }
            _ if plain => return self.type_select(state, key, cursor, cx),
            _ => return Propagation::Continue,
        };
        self.move_to(to, shift, cx);
        Propagation::Stop
    }

    /// Selects the next row whose text starts with what was typed in the last second: after the cursor for a new
    /// search, from the cursor while it goes on, wrapping around.
    fn type_select(
        &self,
        state: &mut ListState,
        key: &KeyEvent,
        cursor: Option<usize>,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let (Some(text_of), Some(c)) = (&self.type_select, key.text) else {
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
        let start = match cursor {
            Some(c) if state.typed.chars().count() == 1 => c + 1,
            Some(c) => c,
            None => 0,
        };
        let found = (0..self.count)
            .map(|k| (start + k) % self.count)
            .find(|&i| text_of(i).to_lowercase().starts_with(&state.typed));
        if let Some(index) = found {
            self.move_to(index, false, cx);
        }
        Propagation::Stop
    }
}

impl<M: 'static> Widget<M> for Rows<M> {
    type State = ListState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.count as f64 * self.row_height),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let cursor = self.cursor();
        self.built()
            .into_iter()
            .map(|index| {
                let selected = self.selection.contains(index);
                Element::new(ListRow {
                    kind: self.kind,
                    index,
                    row_height: self.row_height,
                    selected,
                    cursor: cursor == Some(index),
                    content: Some((self.row)(index, selected)),
                })
                .key(Key::Int(index as u64))
            })
            .collect()
    }

    fn state(&self) -> ListState {
        ListState::default()
    }

    fn update(&self, old: &Self, _: &mut ListState) -> Change {
        if (self.count, self.row_height) != (old.count, old.row_height) {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ListState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, state: &mut ListState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
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
                let Some(index) = self.index_at(position, cx) else {
                    return Propagation::Continue;
                };
                let mut next = self.selection.clone();
                if self.multiple && mods.contains(Mods::SHIFT) {
                    next.extend_to(index);
                } else if self.multiple && mods.contains(Mods::PRIMARY) {
                    next.toggle(index);
                } else {
                    next.select_only(index);
                }
                self.select(next, cx);
                Propagation::Stop
            }
            Event::Key(key, _) if key.pressed => self.key(state, &key, cx),
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &ListState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &ListState, cx: &mut SemanticsCx<'_>) {
        cx.role(match self.kind {
            Kind::List => Role::List,
            Kind::Table => Role::Table,
        });
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            multiselectable: self.multiple,
            ..AccessibleStates::default()
        });
        cx.roving_group();
    }
}

/// One built row: its selection, the focus at the cursor, and its content inset from the edges.
struct ListRow<M> {
    kind: Kind,
    index: usize,
    row_height: f64,
    selected: bool,
    /// The list's one Tab stop.
    cursor: bool,
    content: Option<Element<M>>,
}

impl<M: 'static> Widget<M> for ListRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CONTENT_INSET,
                bottom: 0.0,
                right: CONTENT_INSET,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.row_height),
            position: Point::new(0.0, self.index as f64 * self.row_height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        self.content.take().into_iter().collect()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.index, self.row_height) != (old.index, old.row_height) {
            Change::Layout
        } else if (self.selected, self.cursor) != (old.selected, old.cursor) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    /// A press focuses the row without showing the ring or scrolling, then goes on to the list, which selects.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
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
        let size = cx.size();
        let chrome = Rect::new(
            SELECTION_INSET,
            0.0,
            (size.width - 2.0 * SELECTION_INSET).max(0.0),
            size.height,
        );
        if self.selected {
            let state = ControlState {
                enabled: true,
                selected: true,
                window_active: cx.window_active(),
                ..ControlState::default()
            };
            let color = look::color(&cx.env().theme, ColorRole::ControlFill, &state);
            look::fill(cx, chrome, SELECTION_RADIUS, color);
        }
        if cx.is_focused() {
            paint_focus_ring(cx, chrome, SELECTION_RADIUS);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(match self.kind {
            Kind::List => Role::ListItem,
            Kind::Table => Role::Row,
        });
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.roving_item();
        if self.cursor {
            cx.focusable();
        }
    }
}

/// A table row's cells, across its width.
struct Cells<M> {
    cells: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Cells<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..row_of_cells()
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.cells)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

fn row_of_cells() -> LayoutSpec {
    LayoutSpec::new(ContainerKind::Stack {
        axis: Axis::Horizontal,
        spacing: 0.0,
        align: Align::Center,
        justify: Justify::Start,
    })
}

/// A column's sorting header: what a click on its title asks for.
struct Sort<M> {
    index: usize,
    /// The last column draws no divider after it.
    last: bool,
    order: Option<SortOrder>,
    on_sort: Option<Rc<Handler<(usize, SortOrder), M>>>,
}

/// One column's share of a row or of the header, with its content inset from the column's edges.
struct Cell<M> {
    width: ColumnWidth,
    /// Set on the header's cells.
    header: Option<Sort<M>>,
    content: Vec<Element<M>>,
}

fn header_cell<M: 'static>(
    index: usize,
    last: bool,
    column: &Column,
    order: Option<SortOrder>,
    on_sort: Option<Rc<Handler<(usize, SortOrder), M>>>,
) -> Element<M> {
    let style = TextStyle {
        weight: FontWeight(HEADER_TEXT_WEIGHT),
        ..TextStyle::new(HEADER_TEXT_SIZE)
    };
    let mut content = vec![Element::new(Text {
        content: AttributedText::plain(&column.title, style),
        role: TextRole::Caption,
        wrap: false,
        secondary: false,
        selected: false,
    })];
    if let Some(order) = order {
        let name = match order {
            SortOrder::Ascending => "symbol.chevron.up",
            SortOrder::Descending => "symbol.chevron.down",
        };
        content.push(Element::new(Icon {
            name: name.to_owned(),
            size: CHEVRON,
            direction: IconDirection::Fixed,
            label: None,
            tint: Some(ColorRole::SecondaryText),
        }));
    }
    Element::new(Cell {
        width: column.width,
        header: Some(Sort {
            index,
            last,
            order,
            on_sort,
        }),
        content,
    })
}

impl<M: 'static> Widget<M> for Cell<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        let (width, flex) = match self.width {
            ColumnWidth::Fixed(w) => (SizeSpec::Fixed(w), Flex::NONE),
            ColumnWidth::Fill => (SizeSpec::Fit, Flex::grow(1.0)),
        };
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CELL_INSET,
                bottom: 0.0,
                right: CELL_INSET,
            },
            width,
            height: SizeSpec::Fill,
            flex,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CHEVRON_GAP,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.content)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        let order = |c: &Self| c.header.as_ref().map(|h| h.order);
        if self.width != old.width {
            Change::Layout
        } else if order(self) != order(old) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(sort) = &self.header else {
            return Propagation::Continue;
        };
        match press.event(event, sort.on_sort.is_some(), Keys::Space, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => Propagation::Stop,
            Outcome::Activated(_) => {
                let order = match sort.order {
                    Some(SortOrder::Ascending) => SortOrder::Descending,
                    Some(SortOrder::Descending) | None => SortOrder::Ascending,
                };
                if let Some(on_sort) = &sort.on_sort {
                    cx.emit(on_sort((sort.index, order)));
                }
                Propagation::Stop
            }
        }
    }

    /// A header cell other than the last draws the divider at its trailing edge.
    fn paint(&self, _: &Press, cx: &mut PaintCx<'_>) {
        if self.header.as_ref().is_none_or(|h| h.last) {
            return;
        }
        let env = cx.env();
        let Some(color) = env
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL)
        else {
            return;
        };
        let size = cx.size();
        let x = match env.direction {
            LayoutDirection::Ltr => size.width - 1.0,
            LayoutDirection::Rtl => 0.0,
        };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(x, 0.0, 1.0, size.height)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        match &self.header {
            Some(sort) => {
                cx.role(Role::ColumnHeader);
                if sort.on_sort.is_some() {
                    cx.action(Action::Click);
                }
            }
            None => cx.role(Role::Cell),
        }
    }
}

/// A table's column titles, over its scroll view and inset like its rows, with a line under them.
struct Header<M> {
    cells: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Header<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CONTENT_INSET,
                bottom: 1.0,
                right: CONTENT_INSET,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(HEADER_HEIGHT),
            ..row_of_cells()
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.cells)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env();
        let Some(color) = env
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL)
        else {
            return;
        };
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, size.height - 1.0, size.width, 1.0)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Row);
    }
}
