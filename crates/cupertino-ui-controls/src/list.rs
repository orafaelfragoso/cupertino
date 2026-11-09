//! ListView, TableView and TreeView: a scroll view around one content node as tall as every row, which builds only the rows near
//! the viewport plus the cursor row, each at its index times the row height. The model keeps the offset and the
//! selection; keys and clicks report a new selection and move the focus to the cursor row, which the runtime scrolls
//! into view.

use crate::disclosure;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::scroll;
use crate::{
    Column, ColumnWidth, Handler, Icon, IconDirection, ListView, OutlineTable, RowKeyHandler,
    Scroller, Selection, SortOrder, TableView, Text, TextRole, TreeItem, TreeToggle, TreeView,
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
const LEFT: Keysym = Keysym(0xff51);
const RIGHT: Keysym = Keysym(0xff53);
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
/// A tree row's indent per level, and the column its disclosure triangle sits in. Approximate design values.
const INDENT: f64 = 15.0;
const TRIANGLE_COLUMN: f64 = 16.0;
const TRIANGLE: f64 = 9.0;
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
    Tree,
}

/// A tree's rows and the requests to open and close them, shared by its rows.
struct Outline<M> {
    item: Box<dyn Fn(usize) -> TreeItem>,
    on_toggle: Option<Handler<TreeToggle, M>>,
}

impl<M> Outline<M> {
    fn toggle(&self, toggle: TreeToggle, cx: &mut EventCx<'_, M>) {
        if let Some(on_toggle) = &self.on_toggle {
            cx.emit(on_toggle(toggle));
        }
    }
}

impl<M: 'static> From<ListView<M>> for Element<M> {
    fn from(list: ListView<M>) -> Self {
        list.inset_element(SELECTION_INSET)
    }
}

impl<M: 'static> ListView<M> {
    /// The list's element with each row's selection and focus ring drawn `selection_inset` points in from the row's
    /// sides, in place of the standard inset `Element::from` gives.
    pub fn inset_element(self, selection_inset: f64) -> Element<M> {
        self.under_element(selection_inset, 0.0)
    }

    /// [`inset_element`](Self::inset_element) for a list that fills a pane under a toolbar row `lead` points tall
    /// ([`crate::unobscured_insets`]): its rows start below the row and scroll under it.
    pub fn under_element(self, selection_inset: f64, lead: f64) -> Element<M> {
        let list = self;
        let rows = Rows {
            kind: Kind::List,
            label: list.label,
            count: list.count,
            row_height: list.row_height,
            scroll: list.scroll,
            selection: list.selection,
            multiple: list.multiple,
            initial: list.initial,
            row: list.row,
            key: list.key,
            type_select: list.type_select,
            on_select: list.on_select.map(Rc::new),
            on_activate: list.on_activate,
            on_row_key: None,
            outline: None,
            selection_inset,
            lead,
        };
        scroll::frame(
            led(Element::new(rows), lead),
            list.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(list.on_scroll),
        )
    }
}

impl<M: 'static> From<TreeView<M>> for Element<M> {
    fn from(tree: TreeView<M>) -> Self {
        let rows = Rows {
            kind: Kind::Tree,
            label: tree.label,
            count: tree.count,
            row_height: tree.row_height,
            scroll: tree.scroll,
            selection: tree.selection,
            multiple: tree.multiple,
            initial: false,
            row: tree.row,
            key: None,
            type_select: tree.type_select,
            on_select: tree.on_select.map(Rc::new),
            on_activate: tree.on_activate,
            on_row_key: None,
            outline: Some(Rc::new(Outline {
                item: tree.item,
                on_toggle: tree.on_toggle,
            })),
            selection_inset: SELECTION_INSET,
            lead: 0.0,
        };
        scroll::frame(
            Element::new(rows),
            tree.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(tree.on_scroll),
        )
    }
}

impl<M: 'static> From<TableView<M>> for Element<M> {
    fn from(table: TableView<M>) -> Self {
        table_element(table, None, None)
    }
}

impl<M: 'static> From<OutlineTable<M>> for Element<M> {
    fn from(outline: OutlineTable<M>) -> Self {
        let outline_rows = Outline {
            item: outline.item,
            on_toggle: outline.on_toggle,
        };
        table_element(outline.table, Some(Rc::new(outline_rows)), outline.key)
    }
}

fn table_element<M: 'static>(
    table: TableView<M>,
    outline: Option<Rc<Outline<M>>>,
    key: Option<Box<dyn Fn(usize) -> Key>>,
) -> Element<M> {
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
                    column,
                    header: None,
                    content: vec![cell(index, column, selected)],
                })
            })
            .collect();
        Element::new(Cells { cells })
    });
    let rows = Rows {
        kind: Kind::Table,
        label: table.label.clone(),
        count: table.count,
        row_height: table.row_height,
        scroll: table.scroll,
        selection: table.selection,
        multiple: table.multiple,
        initial: false,
        row,
        key,
        type_select: table.type_select,
        on_select: table.on_select.map(Rc::new),
        on_activate: table.on_activate,
        on_row_key: table.on_row_key,
        outline,
        selection_inset: SELECTION_INSET,
        lead: table.header_top + HEADER_HEIGHT * f64::from(u8::from(table.header_top > 0.0)),
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
    // the rows start below the header, which lies over the view's top with what covers it
    let lead = rows.lead;
    let rows = led(Element::new(rows), lead);
    // a header over the rows' scrolled top holds text outside glass: the edge under it is solid
    let body = if table.header_top > 0.0 {
        scroll::pinned
    } else {
        scroll::frame
    }(
        rows,
        table.scroll,
        (false, true),
        Scroller::Overlay,
        Rc::new(table.on_scroll),
    );
    let header = if table.header_top > 0.0 {
        Element::new(RaisedHeader {
            top: table.header_top,
            children: vec![header],
        })
    } else {
        header
    };
    Element::new(TableFrame {
        label: table.label,
        count: table.count,
        multiple: table.multiple,
        under: table.header_top > 0.0,
        children: vec![header, body],
    })
}

/// `rows` `lead` points down a scroll view's content, the part what covers the view's top hides at rest.
fn led<M: 'static>(rows: Element<M>, lead: f64) -> Element<M> {
    if lead <= 0.0 {
        return rows;
    }
    Element::new(crate::Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: lead,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![rows],
    })
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
    initial: bool,
    row: Box<dyn Fn(usize, bool) -> Element<M>>,
    key: Option<Box<dyn Fn(usize) -> Key>>,
    type_select: Option<Box<dyn Fn(usize) -> String>>,
    on_select: Option<Rc<Handler<Selection, M>>>,
    on_activate: Option<Handler<usize, M>>,
    on_row_key: Option<RowKeyHandler<M>>,
    outline: Option<Rc<Outline<M>>>,
    /// From a row's sides to its selection.
    selection_inset: f64,
    /// How far above the first row the scroll view's content starts: the part raised nodes over the view's top cover
    /// at rest, which the rows scroll under.
    lead: f64,
}

/// What an assistive Click on row `index` reports: the row selected alone, unless it already is.
type Pick<M> = Rc<dyn Fn(usize) -> Option<M>>;

/// The node state of a list. Opaque.
#[derive(Clone, Debug, Default)]
pub struct ListState {
    /// What was typed to select, and when that search ends.
    typed: String,
    typed_until: Option<Timestamp>,
}

impl<M: 'static> Rows<M> {
    fn row_key(&self, index: usize) -> Key {
        self.key
            .as_ref()
            .map_or(Key::Int(index as u64), |key| key(index))
    }

    fn pick(&self) -> Option<Pick<M>> {
        let on_select = Rc::clone(self.on_select.as_ref()?);
        let selection = self.selection.clone();
        Some(Rc::new(move |index| {
            let mut next = selection.clone();
            next.select_only(index);
            (next != selection).then(|| on_select(next))
        }))
    }

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
        // the rows under what covers the view's top show too: from `lead` above the first row in view
        let window = list.window(
            (self.scroll.offset.y - self.lead).max(0.0),
            self.scroll.viewport.height,
            OVERSCAN,
        );
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
        cx.focus_item(FocusTarget::Key(self.row_key(index)));
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
        let shown = self.scroll.viewport.height - self.lead;
        let page = ((shown / self.row_height).floor() as usize).max(1);
        let cursor = self.selection.cursor.filter(|&c| c < self.count);
        if let Some(outline) = &self.outline {
            if let Some(propagation) = self.tree_key(outline, at, key, cx) {
                return propagation;
            }
        }
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

    /// A key the table did not take, offered to the application with the row the keyboard is on.
    fn offer_key(&self, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        let (Some(on_row_key), Some(at)) = (&self.on_row_key, self.cursor()) else {
            return Propagation::Continue;
        };
        match on_row_key(at, key) {
            Some(message) => {
                cx.emit(message);
                Propagation::Stop
            }
            None => Propagation::Continue,
        }
    }

    /// Left and Right in a tree: open or close the cursor row, or move to its first child or its parent.
    fn tree_key(
        &self,
        outline: &Outline<M>,
        at: usize,
        key: &KeyEvent,
        cx: &mut EventCx<'_, M>,
    ) -> Option<Propagation> {
        let (forward, back) = match cx.env().direction {
            LayoutDirection::Ltr => (RIGHT, LEFT),
            LayoutDirection::Rtl => (LEFT, RIGHT),
        };
        let open = match key.keysym {
            k if k == forward => true,
            k if k == back => false,
            _ => return None,
        };
        let mods = key.mods;
        if mods.contains(Mods::CTRL) || mods.contains(Mods::ALT) || mods.contains(Mods::SHIFT) {
            return None;
        }
        let item = (outline.item)(at);
        let toggle = |expanded, recursive| TreeToggle {
            index: at,
            expanded,
            recursive,
        };
        if mods.contains(Mods::PRIMARY) {
            if item.expanded.is_some() {
                outline.toggle(toggle(open, true), cx);
            }
            return Some(Propagation::Stop);
        }
        match (open, item.expanded) {
            (true, Some(false)) | (false, Some(true)) => outline.toggle(toggle(open, false), cx),
            (true, Some(true))
                if at + 1 < self.count && (outline.item)(at + 1).level > item.level =>
            {
                self.move_to(at + 1, false, cx)
            }
            (false, _) if item.level > 0 => {
                let parent = (0..at)
                    .rev()
                    .find(|&i| (outline.item)(i).level < item.level);
                if let Some(parent) = parent {
                    self.move_to(parent, false, cx);
                }
            }
            _ => {}
        }
        Some(Propagation::Stop)
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
        let pick = self.pick();
        self.built()
            .into_iter()
            .map(|index| {
                let selected = self.selection.contains(index);
                let item = self.outline.as_ref().map(|o| (o.item)(index));
                let mut content = Vec::with_capacity(2);
                if let (Some(outline), Some(item)) = (&self.outline, item) {
                    content.push(Element::new(Twisty {
                        index,
                        item,
                        selected,
                        outline: Rc::clone(outline),
                    }));
                }
                content.push((self.row)(index, selected));
                Element::new(ListRow {
                    kind: self.kind,
                    index,
                    row_height: self.row_height,
                    selected,
                    cursor: cursor == Some(index),
                    item,
                    outline: self.outline.clone(),
                    pick: pick.clone(),
                    selection_inset: self.selection_inset,
                    content,
                })
                .key(self.row_key(index))
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
            Event::Key(key, _) if key.pressed => match self.key(state, &key, cx) {
                // what the table leaves goes to the application; the header's keys never pass the rows
                Propagation::Continue => self.offer_key(&key, cx),
                taken => taken,
            },
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &ListState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &ListState, cx: &mut SemanticsCx<'_>) {
        // A table's rows sit under its frame, with its header
        // a tree's set sizes are per parent, on its items
        if self.kind != Kind::Table {
            cx.role(match self.kind {
                Kind::Tree => Role::Tree,
                _ => Role::List,
            });
            cx.name(self.label.clone());
            cx.states(AccessibleStates {
                multiselectable: self.multiple,
                ..AccessibleStates::default()
            });
            if self.kind == Kind::List {
                cx.item_count(self.count);
            }
        }
        cx.roving_group();
        if self.initial {
            cx.initial_focus();
        }
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
    item: Option<TreeItem>,
    outline: Option<Rc<Outline<M>>>,
    /// A row's assistive Click.
    pick: Option<Pick<M>>,
    /// From the row's sides to its selection.
    selection_inset: f64,
    content: Vec<Element<M>>,
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
        std::mem::take(&mut self.content)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.index, self.row_height) != (old.index, old.row_height) {
            Change::Layout
        } else if (self.selected, self.cursor, self.item) != (old.selected, old.cursor, old.item)
            || self.selection_inset != old.selection_inset
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    /// A press focuses the row without showing the ring or scrolling, then goes on to the list, which selects.
    /// Assistive technology selects a row of a list, table or tree with its Click action, and opens and closes a tree
    /// row with its Expand and Collapse actions.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                let Some(pick) = &self.pick else {
                    return Propagation::Continue;
                };
                if let Some(message) = pick(self.index) {
                    cx.emit(message);
                }
                return Propagation::Stop;
            }
            Event::Action(request) if cx.node() == cx.target() => {
                let (Some(outline), Some(expanded)) =
                    (&self.outline, self.item.and_then(|i| i.expanded))
                else {
                    return Propagation::Continue;
                };
                let open = match request.action {
                    Action::Expand => true,
                    Action::Collapse => false,
                    _ => return Propagation::Continue,
                };
                if open != expanded {
                    outline.toggle(
                        TreeToggle {
                            index: self.index,
                            expanded: open,
                            recursive: false,
                        },
                        cx,
                    );
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
        let size = cx.size();
        let chrome = Rect::new(
            self.selection_inset,
            0.0,
            (size.width - 2.0 * self.selection_inset).max(0.0),
            size.height,
        );
        let theme = cx.env().theme.clone();
        let selected = ControlState {
            enabled: true,
            selected: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        if self.selected {
            let color = look::color(&theme, ColorRole::ControlFill, &selected);
            look::fill(cx, chrome, SELECTION_RADIUS, color);
        }
        if cx.is_focused() && cx.focus_visible() && cx.window_active() {
            let inner = chrome.inset(Insets::uniform(theme.focus.outset + theme.focus.width));
            let radius =
                (SELECTION_RADIUS - theme.focus.width / 2.0 - theme.focus.radius_extra).max(0.0);
            let (rect, radius) = look::focus_ring(inner, radius, &theme.focus);
            let color = if self.selected {
                look::color(&theme, ColorRole::ControlText, &selected)
            } else {
                theme
                    .colors
                    .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            };
            look::stroke(cx, rect, radius, theme.focus.width, color);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(match self.kind {
            Kind::List => Role::ListItem,
            Kind::Table => Role::Row,
            Kind::Tree => Role::TreeItem,
        });
        cx.states(AccessibleStates {
            selected: self.selected,
            expanded: self.item.and_then(|i| i.expanded),
            ..AccessibleStates::default()
        });
        match self.item {
            Some(item) => {
                cx.level(item.level + 1);
                cx.item_index(item.position);
                cx.item_count(item.siblings);
                if let Some(expanded) = item.expanded {
                    cx.action(if expanded {
                        Action::Collapse
                    } else {
                        Action::Expand
                    });
                }
            }
            None => cx.item_index(self.index),
        }
        if self.pick.is_some() {
            cx.action(Action::Click);
        }
        cx.roving_item();
        if self.cursor {
            cx.focusable();
        }
    }
}

/// A tree row's indent and its disclosure triangle, before its content; a press on it opens or closes the row.
struct Twisty<M> {
    index: usize,
    item: TreeItem,
    selected: bool,
    outline: Rc<Outline<M>>,
}

impl<M: 'static> Widget<M> for Twisty<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.item.level as f64 * INDENT + TRIANGLE_COLUMN),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.item.level != old.item.level {
            Change::Layout
        } else if (self.item.expanded, self.selected) != (old.item.expanded, old.selected) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match (event, self.item.expanded) {
            (
                Event::Pointer(
                    PointerEvent::Button {
                        button: MouseButton::Left,
                        pressed: true,
                        mods,
                        ..
                    },
                    _,
                ),
                Some(expanded),
            ) if cx.node() == cx.target() => {
                let toggle = TreeToggle {
                    index: self.index,
                    expanded: !expanded,
                    recursive: mods.contains(Mods::PRIMARY),
                };
                self.outline.toggle(toggle, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(open) = self.item.expanded else {
            return;
        };
        let size = cx.size();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let column = if rtl {
            0.0
        } else {
            size.width - TRIANGLE_COLUMN
        };
        let area = Rect::new(
            column + (TRIANGLE_COLUMN - TRIANGLE) / 2.0,
            (size.height - TRIANGLE) / 2.0,
            TRIANGLE,
            TRIANGLE,
        );
        let points = disclosure::place([(0.25, 0.15), (0.8, 0.5), (0.25, 0.85)], area, open, rtl);
        let state = ControlState {
            enabled: true,
            selected: self.selected,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let role = if self.selected {
            ColorRole::ControlText
        } else {
            ColorRole::SecondaryText
        };
        if let Some(color) = look::color(&cx.env().theme, role, &state) {
            cx.push(PaintOp::Fill {
                shape: disclosure::path(&points, true),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
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
    title: String,
    /// The last column draws no divider after it.
    last: bool,
    order: Option<SortOrder>,
    on_sort: Option<Rc<Handler<(usize, SortOrder), M>>>,
}

/// One column's share of a row or of the header, with its content inset from the column's edges.
struct Cell<M> {
    width: ColumnWidth,
    column: usize,
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
        column: index,
        header: Some(Sort {
            index,
            title: column.title.clone(),
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
        match press.event(event, sort.on_sort.is_some(), Keys::SpaceAndReturn, cx) {
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

    /// A header cell other than the last draws the divider at its trailing edge; a sorting one shows the keyboard focus.
    fn paint(&self, _: &Press, cx: &mut PaintCx<'_>) {
        let Some(sort) = &self.header else {
            return;
        };
        if sort.on_sort.is_some() {
            let size = cx.size();
            paint_focus_ring(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0);
        }
        if sort.last {
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
                cx.name(sort.title.clone());
                if sort.on_sort.is_some() {
                    cx.focusable();
                    cx.action(Action::Click);
                }
                if let Some(order) = sort.order {
                    cx.sort(order);
                }
            }
            None => cx.role(Role::Cell),
        }
        cx.column_index(self.column);
    }
}

/// A table's header over its scroll view: the one table assistive technology reads, its header row first.
struct TableFrame<M> {
    label: String,
    count: usize,
    multiple: bool,
    /// The table fills a pane under a toolbar row: its header lies over its scroll view, which runs to its top.
    under: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for TableFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let kind = if self.under {
            ContainerKind::Overlay
        } else {
            ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            }
        };
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(kind)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.under != old.under {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Table);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            multiselectable: self.multiple,
            ..AccessibleStates::default()
        });
        cx.item_count(self.count);
    }
}

/// A table's header below the toolbar row over the table's top: raised with the row, over the rows that scroll under
/// both. It takes no point above the header, which belongs to the row.
struct RaisedHeader<M> {
    top: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for RaisedHeader<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: self.top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.top + HEADER_HEIGHT),
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

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.top != old.top {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn hit(&self, _: &(), point: Point) -> bool {
        point.y >= self.top
    }

    fn raised(&self, _: &()) -> bool {
        true
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
