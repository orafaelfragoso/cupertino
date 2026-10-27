//! Context menus of the browser window: what a row is and does, where the rows and submenus go, and the hover,
//! delay, modifier and keyboard state of an open menu. What each menu offers is [`crate::menu_spec`]; drawing is
//! `WindowPainter::paint_menu`; the process maps one popup per open level (`app`).
//!
//! * The metrics are in [`metrics::menu`]. Submenus open after a 0.2 s timer and no arrow key wraps.
//!   The design's safe-triangle tracking is replaced by that timer alone: the pointer may cross a
//!   neighbouring row on its way to the submenu without closing it (approximate).
//! * A row can carry alternates: rows that take the place of the row while the modifiers they name are held, as the
//!   reference's descriptor table marks them with a key-equivalent modifier mask and the alternate flag. The
//!   menu keeps its size when they swap.
//! * Two rows are views in the design and are drawn by the window painter from the geometry here: the tag row (a
//!   row of seven colour dots, `TLabelViewForMenu`) and the row of the four view modes.

use crate::geom::{Point, Rect};
use crate::metrics::menu as m;
use crate::sort::SortKey;
use crate::tags::TagColor;
use crate::window::ViewMode;
use std::collections::HashMap;

/// The modifier keys that select an alternate row. The design's masks are Alt (0x80000), Control (0x40000) and
/// Shift + Alt (0xa0000).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Mods {
    pub const ALT: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: true,
    };
    pub const CTRL: Mods = Mods {
        shift: false,
        ctrl: true,
        alt: false,
    };
    pub const SHIFT_ALT: Mods = Mods {
        shift: true,
        ctrl: false,
        alt: true,
    };
}

/// What a picked row asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the selection with its default applications (folders: enter the first).
    Open,
    /// Open the selection with the application of this desktop entry id.
    OpenWith(String),
    /// Open each selected folder in a window of its own.
    OpenInNewWindow,
    /// Show the target of the selected link in its folder.
    ShowOriginal,
    /// Put the selected items of the trash back where they came from.
    PutBack,
    MoveToTrash,
    /// Edit the name of the selected item in place.
    Rename,
    /// Create an empty folder in the open one and edit its name.
    NewFolder,
    /// Create a folder next to the selection, move the selection into it and edit its name.
    NewFolderWithSelection,
    /// Copy the selected items next to themselves.
    Duplicate,
    /// Make a link to the selected item next to it.
    MakeAlias,
    /// Order the folder by this key, ascending or not.
    SortBy(SortKey, bool),
    /// Add the tag of this colour to the selection, or remove it when every item has it.
    ToggleTag(TagColor),
    SetView(ViewMode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Command(Command),
    Submenu(Vec<MenuItem>),
    Separator,
    /// A row the window cannot do anything for yet: drawn disabled, never picked.
    Unavailable,
    /// The row of colour dots; `on` are the colours every selected item has.
    TagRow {
        on: Vec<TagColor>,
    },
    /// The row of the four view modes, the current one marked.
    ViewRow {
        current: ViewMode,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub kind: ItemKind,
    pub enabled: bool,
    /// The state column shows a check mark: the current choice of a list of choices.
    pub checked: bool,
    /// Rows that replace this one while exactly these modifiers are held.
    pub alternates: Vec<(Mods, MenuItem)>,
}

impl MenuItem {
    pub fn command(label: &str, command: Command, enabled: bool) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Command(command),
            enabled,
            checked: false,
            alternates: Vec::new(),
        }
    }

    /// A row without a backend: disabled.
    pub fn unavailable(label: &str) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Unavailable,
            enabled: false,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn submenu(label: &str, items: Vec<MenuItem>, enabled: bool) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Submenu(items),
            enabled,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        MenuItem {
            label: String::new(),
            kind: ItemKind::Separator,
            enabled: false,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// This row, replaced by `alternate` while `mods` are held.
    pub fn with_alternate(mut self, mods: Mods, alternate: MenuItem) -> Self {
        self.alternates.push((mods, alternate));
        self
    }

    /// The row can be highlighted, picked and reached with the arrow keys.
    pub fn is_selectable(&self) -> bool {
        self.enabled && matches!(self.kind, ItemKind::Command(_) | ItemKind::Submenu(_))
    }

    /// The pointer can act on the row: a selectable row, or one of the two view rows.
    fn is_hit(&self) -> bool {
        self.is_selectable()
            || matches!(
                self.kind,
                ItemKind::TagRow { .. } | ItemKind::ViewRow { .. }
            )
    }

    fn height(&self) -> f64 {
        match self.kind {
            ItemKind::Separator => m::SEPARATOR_HEIGHT,
            ItemKind::TagRow { .. } => m::TAG_ROW_HEIGHT,
            ItemKind::ViewRow { .. } => m::VIEW_ROW_HEIGHT,
            _ => m::ITEM_HEIGHT,
        }
    }

    /// This row as it shows while `mods` are held: the alternate that names exactly them, else the row itself.
    fn resolved(&self, mods: Mods) -> MenuItem {
        let mut item = self
            .alternates
            .iter()
            .find(|(m, _)| *m == mods)
            .map_or_else(|| self.clone(), |(_, alt)| alt.clone());
        item.alternates.clear();
        if let ItemKind::Submenu(sub) = &mut item.kind {
            *sub = sub.iter().map(|i| i.resolved(mods)).collect();
        }
        item
    }
}

/// The colours of the tag row, in the order of the dots: red, orange, yellow, green, blue, purple, gray (the order of
/// the design's default favourite tags).
pub const TAG_ROW_ORDER: [TagColor; 7] = [
    TagColor::Red,
    TagColor::Orange,
    TagColor::Yellow,
    TagColor::Green,
    TagColor::Blue,
    TagColor::Purple,
    TagColor::Gray,
];

/// The view modes in the order of the row's segments.
pub const VIEW_ROW_ORDER: [ViewSegment; 4] = [
    ViewSegment::Icons,
    ViewSegment::List,
    ViewSegment::Columns,
    ViewSegment::Gallery,
];

/// A segment of the view row; only the first two have a view behind them in this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewSegment {
    Icons,
    List,
    Columns,
    Gallery,
}

impl ViewSegment {
    pub fn mode(self) -> Option<ViewMode> {
        match self {
            ViewSegment::Icons => Some(ViewMode::Icons),
            ViewSegment::List => Some(ViewMode::List),
            ViewSegment::Columns | ViewSegment::Gallery => None,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            ViewSegment::Icons => "symbol.grid",
            ViewSegment::List => "symbol.list",
            ViewSegment::Columns => "symbol.columns",
            ViewSegment::Gallery => "symbol.gallery",
        }
    }
}

/// The dots of the tag row `row`: seven equal slots across the 192 pt of the design's dot view, centred in the row.
pub fn tag_dots(row: &Rect) -> [(TagColor, Rect); 7] {
    let width = m::TAG_ROW_WIDTH - 24.0;
    let slot = width / 7.0;
    let left = row.x + (row.w - width) / 2.0;
    TAG_ROW_ORDER.map(|color| {
        let i = TAG_ROW_ORDER
            .iter()
            .position(|c| *c == color)
            .expect("the colour is in its own order") as f64;
        (
            color,
            Rect::new(
                left + slot * i + (slot - m::TAG_DOT_DIAMETER) / 2.0,
                row.y + m::TAG_DOT_TOP,
                m::TAG_DOT_DIAMETER,
                m::TAG_DOT_DIAMETER,
            ),
        )
    })
}

/// The segments of the view row `row`.
pub fn view_segments(row: &Rect) -> [(ViewSegment, Rect); 4] {
    let inner = row.inset(m::VIEW_ROW_INSET, 3.0);
    let width = (inner.w - 3.0 * m::VIEW_SEGMENT_GAP) / 4.0;
    VIEW_ROW_ORDER.map(|segment| {
        let i = VIEW_ROW_ORDER
            .iter()
            .position(|s| *s == segment)
            .expect("the segment is in its own order") as f64;
        (
            segment,
            Rect::new(
                inner.x + i * (width + m::VIEW_SEGMENT_GAP),
                inner.y,
                width,
                inner.h,
            ),
        )
    })
}

/// An application that can open the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppChoice {
    /// Desktop entry id.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// The applications every list offers, in the order of the first list (the design intersects the binding lists of a
/// multi-item selection). An empty slice of lists offers none.
pub fn common_apps(lists: &[Vec<AppChoice>]) -> Vec<AppChoice> {
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .filter(|a| rest.iter().all(|l| l.iter().any(|b| b.id == a.id)))
        .cloned()
        .collect()
}

/// One laid-out level: its rectangle and one rectangle per row, in the window's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rect: Rect,
    pub rows: Vec<Rect>,
}

/// Size of a level whose rows are `items`, wide enough for the labels (and alternates' labels) of `widest_of`.
fn level_size(
    items: &[MenuItem],
    widest_of: &[MenuItem],
    measure: &dyn Fn(&str) -> f64,
) -> (f64, f64) {
    let labels = |i: &MenuItem| {
        std::iter::once(i.label.clone())
            .chain(i.alternates.iter().map(|(_, a)| a.label.clone()))
            .collect::<Vec<_>>()
    };
    let widest = widest_of
        .iter()
        .filter(|i| i.kind != ItemKind::Separator)
        .flat_map(labels)
        .map(|l| measure(&l))
        .fold(0.0, f64::max);
    let arrow = if widest_of.iter().chain(items).any(|i| {
        matches!(i.kind, ItemKind::Submenu(_))
            || i.alternates
                .iter()
                .any(|(_, a)| matches!(a.kind, ItemKind::Submenu(_)))
    }) {
        m::SUBMENU_ARROW
    } else {
        0.0
    };
    let mut w = (m::TITLE_OFFSET + widest + arrow + m::TRAILING_PADDING).max(m::MIN_WIDTH);
    if items
        .iter()
        .any(|i| matches!(i.kind, ItemKind::TagRow { .. }))
    {
        w = w.max(m::TAG_ROW_WIDTH);
    }
    let h = items.iter().map(MenuItem::height).sum::<f64>() + 2.0 * m::EDGE_PADDING;
    (w.ceil(), h)
}

fn rows_of(items: &[MenuItem], rect: &Rect) -> Vec<Rect> {
    let mut top = rect.y + m::EDGE_PADDING;
    items
        .iter()
        .map(|i| {
            let row = Rect::new(rect.x, top, rect.w, i.height());
            top += i.height();
            row
        })
        .collect()
}

/// Keep `rect` inside `bounds`, less the margin.
fn keep_inside(rect: Rect, bounds: &Rect) -> Rect {
    let inner = bounds.inset(m::SCREEN_MARGIN, m::SCREEN_MARGIN);
    let x = rect.x.min(inner.right() - rect.w).max(inner.x);
    let y = rect.y.min(inner.bottom() - rect.h).max(inner.y);
    Rect::new(x.round(), y.round(), rect.w, rect.h)
}

/// A root level whose top-left corner is at `at`.
fn layout_root(
    items: &[MenuItem],
    widest_of: &[MenuItem],
    at: Point,
    bounds: &Rect,
    measure: &dyn Fn(&str) -> f64,
) -> Layout {
    let (w, h) = level_size(items, widest_of, measure);
    let rect = keep_inside(Rect::new(at.x, at.y, w, h), bounds);
    Layout {
        rows: rows_of(items, &rect),
        rect,
    }
}

/// A submenu of the row `row` of the level `parent`: to the right with a 5 pt overlap, else to the left, with its first
/// row level with the parent row.
fn layout_submenu(
    items: &[MenuItem],
    parent: &Rect,
    row: &Rect,
    bounds: &Rect,
    measure: &dyn Fn(&str) -> f64,
) -> Layout {
    let (w, h) = level_size(items, items, measure);
    let inner = bounds.inset(m::SCREEN_MARGIN, m::SCREEN_MARGIN);
    let right = parent.right() - m::SUBMENU_OVERLAP;
    let x = if right + w <= inner.right() {
        right
    } else {
        parent.x + m::SUBMENU_OVERLAP - w
    };
    let rect = keep_inside(Rect::new(x, row.y - m::SUBMENU_TOP_OFFSET, w, h), bounds);
    Layout {
        rows: rows_of(items, &rect),
        rect,
    }
}

/// A key that moves through an open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    /// Into the submenu of the highlighted row.
    Right,
    /// Out of the deepest submenu.
    Left,
    /// Pick the highlighted row.
    Enter,
}

/// A key of an open menu: a move, or Space (picks the highlighted row as Return does) or the cancel chords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Nav(Nav),
    Space,
    /// Escape or Super+. : the menu closes and nothing runs.
    Cancel,
}

/// What a press or key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The menu closes and nothing runs.
    Close,
    /// Run the command; the menu closes.
    Run(Command),
    /// The menu changed (a submenu opened or closed, the highlight moved): draw it again.
    Changed,
    Nothing,
}

/// An open menu: the root level and the submenus open under it, each with its highlighted row.
#[derive(Clone, Debug)]
pub struct Menu {
    root: Vec<MenuItem>,
    /// `root` as it shows under the modifiers held: alternates swapped in. Levels below the root are read from here.
    shown: Vec<MenuItem>,
    mods: Mods,
    bounds: Rect,
    /// Width of every label of every level, measured when the menu opened.
    widths: HashMap<String, f64>,
    /// Row of each open level except the deepest that holds the next level's submenu.
    path: Vec<usize>,
    layouts: Vec<Layout>,
    hovered: Vec<Option<usize>>,
    /// A row the pointer rests on whose submenu opens (or whose siblings' submenu closes) at the time: `(level, row, time)`.
    pending: Option<(usize, usize, f64)>,
}

fn measure_all(items: &[MenuItem], measure: &dyn Fn(&str) -> f64, into: &mut HashMap<String, f64>) {
    for item in items {
        if item.kind != ItemKind::Separator {
            into.insert(item.label.clone(), measure(&item.label));
        }
        if let ItemKind::Submenu(sub) = &item.kind {
            measure_all(sub, measure, into);
        }
        for (_, alt) in &item.alternates {
            measure_all(std::slice::from_ref(alt), measure, into);
        }
    }
}

impl Menu {
    /// Open `items` with the top-left corner at `at` inside `bounds`; `measure` gives a label's width in points.
    pub fn open(
        items: Vec<MenuItem>,
        at: Point,
        bounds: Rect,
        measure: &dyn Fn(&str) -> f64,
    ) -> Menu {
        let mut widths = HashMap::new();
        measure_all(&items, measure, &mut widths);
        let mods = Mods::default();
        let shown: Vec<MenuItem> = items.iter().map(|i| i.resolved(mods)).collect();
        let layouts = vec![layout_root(&shown, &items, at, &bounds, &|l| widths[l])];
        Menu {
            root: items,
            shown,
            mods,
            bounds,
            widths,
            path: Vec::new(),
            layouts,
            hovered: vec![None],
            pending: None,
        }
    }

    /// The modifiers held changed: rows with an alternate for them swap (the layout does not change). The submenus close,
    /// because the row that holds one may have changed. `true` when anything changed.
    pub fn set_mods(&mut self, mods: Mods) -> bool {
        if mods == self.mods {
            return false;
        }
        self.mods = mods;
        self.shown = self.root.iter().map(|i| i.resolved(mods)).collect();
        self.layouts.truncate(1);
        self.hovered.truncate(1);
        self.path.clear();
        self.pending = None;
        true
    }

    /// Number of open levels (at least one).
    pub fn levels(&self) -> usize {
        self.layouts.len()
    }

    pub fn layout(&self, level: usize) -> &Layout {
        &self.layouts[level]
    }

    /// The rectangle of `level` relative to the top-left corner of the level below it (of the window for the root).
    pub fn rect_in_parent(&self, level: usize) -> Rect {
        let rect = self.layouts[level].rect;
        match level.checked_sub(1) {
            Some(parent) => rect.offset(-self.layouts[parent].rect.x, -self.layouts[parent].rect.y),
            None => rect,
        }
    }

    pub fn items(&self, level: usize) -> &[MenuItem] {
        let mut items = self.shown.as_slice();
        for row in &self.path[..level] {
            let ItemKind::Submenu(sub) = &items[*row].kind else {
                unreachable!("the path leads through submenu rows")
            };
            items = sub;
        }
        items
    }

    pub fn hovered(&self, level: usize) -> Option<usize> {
        self.hovered[level]
    }

    /// The selectable row of `level` under `p` (window coordinates).
    pub fn hit(&self, level: usize, p: Point) -> Option<usize> {
        let items = self.items(level);
        self.layouts[level]
            .rows
            .iter()
            .position(|r| r.contains(p))
            .filter(|i| items[*i].is_hit())
    }

    /// The time at which [`Menu::poll`] has something to do.
    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, _, at)| at)
    }

    /// The pointer is over `row` of `level` (`None`: over no selectable row) at `now` seconds. A row other than the open
    /// submenu's takes effect after the submenu delay; `true` when the highlight moved.
    pub fn hover(&mut self, level: usize, row: Option<usize>, now: f64) -> bool {
        // the pointer reached a deeper level: the wait on a shallower one is over
        if self.pending.is_some_and(|(l, ..)| l < level) {
            self.pending = None;
        }
        if self.hovered[level] == row {
            return false;
        }
        self.hovered[level] = row;
        if let Some(row) = row {
            let opens = matches!(self.items(level)[row].kind, ItemKind::Submenu(_));
            let is_open = self.path.get(level) == Some(&row);
            let closes = self.layouts.len() > level + 1;
            self.pending =
                (!is_open && (opens || closes)).then_some((level, row, now + m::SUBMENU_DELAY));
        }
        true
    }

    /// Apply the delayed open or close that is due at `now`; `true` when the levels changed.
    pub fn poll(&mut self, now: f64) -> bool {
        let Some((level, row, at)) = self.pending else {
            return false;
        };
        if now < at {
            return false;
        }
        self.pending = None;
        // the pointer may have left the row since
        self.hovered[level] == Some(row) && self.show_submenu_of(level, row)
    }

    /// Close the levels below `level`, then open the submenu of `row` if it has an enabled one; `true` when the levels changed.
    fn show_submenu_of(&mut self, level: usize, row: usize) -> bool {
        let before = self.layouts.len();
        self.layouts.truncate(level + 1);
        self.hovered.truncate(level + 1);
        self.path.truncate(level);
        let item = &self.items(level)[row];
        if let (ItemKind::Submenu(sub), true) = (&item.kind, item.enabled) {
            if !sub.is_empty() {
                let layout = layout_submenu(
                    sub,
                    &self.layouts[level].rect,
                    &self.layouts[level].rows[row],
                    &self.bounds,
                    &|l| self.widths[l],
                );
                self.path.push(row);
                self.layouts.push(layout);
                self.hovered.push(None);
                return true;
            }
        }
        before != self.layouts.len()
    }

    /// A press on `row` of `level`: a command runs, a submenu opens at once.
    pub fn press(&mut self, level: usize, row: usize) -> Outcome {
        let item = &self.items(level)[row];
        if !item.is_selectable() {
            return Outcome::Nothing;
        }
        match &item.kind {
            ItemKind::Command(command) => Outcome::Run(command.clone()),
            ItemKind::Submenu(_) => {
                self.pending = None;
                if self.show_submenu_of(level, row) {
                    Outcome::Changed
                } else {
                    Outcome::Nothing
                }
            }
            ItemKind::Separator
            | ItemKind::Unavailable
            | ItemKind::TagRow { .. }
            | ItemKind::ViewRow { .. } => Outcome::Nothing,
        }
    }

    /// A press at `p` (window coordinates) on `row` of `level`: like [`Menu::press`], and on the tag row and the view row
    /// the dot or segment under `p` runs its command.
    pub fn press_at(&mut self, level: usize, row: usize, p: Point) -> Outcome {
        let rect = self.layouts[level].rows[row];
        match &self.items(level)[row].kind {
            ItemKind::TagRow { .. } => tag_dots(&rect)
                .into_iter()
                .find(|(_, r)| r.contains(p))
                .map_or(Outcome::Nothing, |(color, _)| {
                    Outcome::Run(Command::ToggleTag(color))
                }),
            ItemKind::ViewRow { .. } => view_segments(&rect)
                .into_iter()
                .find(|(_, r)| r.contains(p))
                .and_then(|(segment, _)| segment.mode())
                .map_or(Outcome::Nothing, |mode| {
                    Outcome::Run(Command::SetView(mode))
                }),
            _ => self.press(level, row),
        }
    }

    /// A navigation key. Up and Down move the highlight of the deepest level over selectable rows and stop at the ends;
    /// from no highlight they start at the first and the last row.
    pub fn press_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Nav(nav) => self.navigate(nav),
            Key::Space => self.navigate(Nav::Enter),
            Key::Cancel => Outcome::Close,
        }
    }

    pub fn navigate(&mut self, key: Nav) -> Outcome {
        let level = self.layouts.len() - 1;
        let items = self.items(level);
        let current = self.hovered[level];
        match key {
            Nav::Up | Nav::Down => {
                let selectable = |i: &usize| items[*i].is_selectable();
                let next = match (key, current) {
                    (Nav::Down, Some(c)) => (c + 1..items.len()).find(selectable),
                    (Nav::Down, None) => (0..items.len()).find(selectable),
                    (_, Some(c)) => (0..c).rev().find(selectable),
                    (_, None) => (0..items.len()).rev().find(selectable),
                };
                match next {
                    Some(row) => {
                        self.hovered[level] = Some(row);
                        self.pending = None;
                        Outcome::Changed
                    }
                    None => Outcome::Nothing,
                }
            }
            Nav::Right => match current {
                Some(row) if matches!(items[row].kind, ItemKind::Submenu(_)) => {
                    self.enter(level, row)
                }
                _ => Outcome::Nothing,
            },
            Nav::Left if level > 0 => {
                // the parent row stays highlighted
                self.layouts.truncate(level);
                self.hovered.truncate(level);
                self.path.truncate(level - 1);
                Outcome::Changed
            }
            Nav::Left => Outcome::Nothing,
            Nav::Enter => match current {
                Some(row) => match &items[row].kind {
                    ItemKind::Submenu(_) => self.enter(level, row),
                    _ => self.press(level, row),
                },
                None => Outcome::Nothing,
            },
        }
    }

    /// Open the submenu of `row` and highlight its first selectable row.
    fn enter(&mut self, level: usize, row: usize) -> Outcome {
        self.pending = None;
        if !self.show_submenu_of(level, row) {
            return Outcome::Nothing;
        }
        self.hovered[level + 1] = self
            .items(level + 1)
            .iter()
            .position(MenuItem::is_selectable);
        Outcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu_spec::{item_menu, Subject};

    fn subject() -> Subject {
        Subject {
            count: 1,
            name: "a.txt".into(),
            all_folders: true,
            single_link: false,
            in_trash: false,
            tags_on: vec![TagColor::Red],
        }
    }

    fn apps() -> Vec<AppChoice> {
        vec![
            AppChoice {
                id: "viewer.desktop".into(),
                name: "Viewer".into(),
                is_default: true,
            },
            AppChoice {
                id: "editor.desktop".into(),
                name: "Editor".into(),
                is_default: false,
            },
        ]
    }

    fn open(items: Vec<MenuItem>) -> Menu {
        Menu::open(
            items,
            Point { x: 100.0, y: 80.0 },
            Rect::new(0.0, 0.0, 600.0, 600.0),
            &|l| l.len() as f64 * 7.0,
        )
    }

    fn row_of(menu: &Menu, label: &str) -> usize {
        menu.items(0)
            .iter()
            .position(|i| i.label == label)
            .unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn rows_stack_at_the_measured_heights_and_the_menu_stays_in_bounds() {
        let menu = Menu::open(
            item_menu(&subject(), &apps()),
            Point { x: 590.0, y: 595.0 },
            Rect::new(0.0, 0.0, 600.0, 600.0),
            &|l| l.len() as f64 * 7.0,
        );
        let layout = menu.layout(0);
        let tag = row_of(&menu, "Tags…");
        assert_eq!(layout.rows[tag].h, 49.0);
        assert_eq!(layout.rows[0].h, 22.0);
        assert_eq!(layout.rows[0].y, layout.rect.y + 5.0);
        let total: f64 = layout.rows.iter().map(|r| r.h).sum::<f64>() + 10.0;
        assert_eq!(layout.rect.h, total);
        assert!(layout.rect.w >= 216.0, "the tag row's view is 216 wide");
        assert!(
            layout.rect.right() <= 596.0 && layout.rect.bottom() <= 596.0,
            "{:?}",
            layout.rect
        );
    }

    #[test]
    fn holding_alt_swaps_rows_without_moving_them_and_closes_submenus() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let before = menu.layout(0).clone();
        let with = row_of(&menu, "Open With");
        menu.hover(0, Some(with), 0.0);
        menu.poll(0.2);
        assert_eq!(menu.levels(), 2);
        assert!(!menu.set_mods(Mods::default()), "no change, no work");
        assert!(menu.set_mods(Mods::ALT));
        assert_eq!(menu.levels(), 1);
        assert_eq!(menu.items(0)[with].label, "Always Open With");
        assert_eq!(
            menu.items(0)[row_of(&menu, "Open in New Window")].kind,
            ItemKind::Command(Command::OpenInNewWindow)
        );
        assert_eq!(menu.layout(0), &before, "the menu keeps its size and rows");
        assert!(menu.set_mods(Mods::default()));
        assert_eq!(menu.items(0)[with].label, "Open With");
    }

    #[test]
    fn only_the_exact_modifiers_of_an_alternate_swap_it_in() {
        let mut menu = open(item_menu(&subject(), &apps()));
        menu.set_mods(Mods {
            shift: true,
            ctrl: false,
            alt: true,
        });
        assert!(menu.items(0).iter().any(|i| i.label == "Duplicate Exactly"));
        assert!(
            !menu
                .items(0)
                .iter()
                .any(|i| i.label == "Open in New Window"),
            "Shift + Alt is not Alt"
        );
        menu.set_mods(Mods::CTRL);
        assert!(menu.items(0).iter().any(|i| i.label == "Get Summary Info"));
    }

    #[test]
    fn a_dot_of_the_tag_row_runs_its_colour_and_a_gap_does_nothing() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let tag = row_of(&menu, "Tags…");
        let row = menu.layout(0).rows[tag];
        let dots = tag_dots(&row);
        assert_eq!(dots.map(|(c, _)| c), TAG_ROW_ORDER);
        let (blue, rect) = dots[4];
        assert_eq!(blue, TagColor::Blue);
        assert_eq!(menu.hit(0, rect.center()), Some(tag));
        assert_eq!(
            menu.press_at(0, tag, rect.center()),
            Outcome::Run(Command::ToggleTag(TagColor::Blue))
        );
        assert_eq!(
            menu.press_at(
                0,
                tag,
                Point {
                    x: row.x + 1.0,
                    y: row.bottom() - 1.0
                }
            ),
            Outcome::Nothing
        );
        assert_eq!(
            menu.press(0, tag),
            Outcome::Nothing,
            "the keyboard cannot pick a dot"
        );
    }

    #[test]
    fn view_segments_without_a_view_do_nothing() {
        use crate::menu_spec::{background_menu, Background};
        use crate::sort::SortSpec;
        let mut menu = open(background_menu(&Background {
            view: ViewMode::Icons,
            sort: SortSpec::default(),
            in_trash: false,
        }));
        let view = row_of(&menu, "As Icons");
        let segments = view_segments(&menu.layout(0).rows[view]);
        assert_eq!(
            menu.press_at(0, view, segments[1].1.center()),
            Outcome::Run(Command::SetView(ViewMode::List))
        );
        assert_eq!(
            menu.press_at(0, view, segments[3].1.center()),
            Outcome::Nothing
        );
    }

    #[test]
    fn a_submenu_opens_after_the_delay_level_with_its_row_and_overlapping_its_parent() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let with = row_of(&menu, "Open With");
        assert!(menu.hover(0, Some(with), 10.0));
        assert!(!menu.poll(10.1), "before the 0.2 s delay");
        assert!(menu.poll(10.2));
        assert_eq!(menu.levels(), 2);
        let (parent, row, sub) = (menu.layout(0), menu.layout(0).rows[with], menu.layout(1));
        assert_eq!(sub.rect.x, parent.rect.right() - 5.0);
        assert_eq!(sub.rect.y, row.y - 5.0);
        assert_eq!(
            sub.rows[0].y, row.y,
            "the first row is level with the parent's"
        );
        assert_eq!(menu.rect_in_parent(1).x, parent.rect.w - 5.0);
    }

    #[test]
    fn a_submenu_that_does_not_fit_on_the_right_opens_on_the_left() {
        let mut menu = Menu::open(
            item_menu(&subject(), &apps()),
            Point { x: 420.0, y: 80.0 },
            Rect::new(0.0, 0.0, 600.0, 600.0),
            &|l| l.len() as f64 * 7.0,
        );
        let with = row_of(&menu, "Open With");
        menu.hover(0, Some(with), 0.0);
        menu.poll(0.2);
        assert_eq!(menu.layout(1).rect.right(), menu.layout(0).rect.x + 5.0);
    }

    #[test]
    fn moving_to_another_row_closes_the_submenu_after_the_delay_but_reaching_it_keeps_it() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let with = row_of(&menu, "Open With");
        menu.hover(0, Some(with), 0.0);
        menu.poll(0.2);
        // crossing the row below on the way to the submenu
        menu.hover(0, Some(with + 2), 1.0);
        menu.hover(1, Some(0), 1.05);
        assert!(!menu.poll(1.3), "the pointer reached the submenu");
        assert_eq!(menu.levels(), 2);
        // resting on another row of the root
        menu.hover(0, Some(1), 2.0);
        assert!(menu.poll(2.2));
        assert_eq!(menu.levels(), 1);
    }

    #[test]
    fn arrow_keys_stop_at_the_ends_skip_separators_and_disabled_rows_and_enter_submenus() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let rows = menu.items(0).len();
        let selectable: Vec<usize> = (0..rows)
            .filter(|i| menu.items(0)[*i].is_selectable())
            .collect();
        assert_eq!(menu.navigate(Nav::Up), Outcome::Changed);
        assert_eq!(
            menu.hovered(0),
            selectable.last().copied(),
            "from nothing, Up starts at the last selectable row"
        );
        assert_eq!(menu.navigate(Nav::Down), Outcome::Nothing, "no wrap");
        menu.navigate(Nav::Up);
        assert_eq!(menu.hovered(0), Some(selectable[selectable.len() - 2]));
        let with = row_of(&menu, "Open With");
        menu.hover(0, Some(with), 0.0);
        assert_eq!(menu.navigate(Nav::Right), Outcome::Changed);
        assert_eq!((menu.levels(), menu.hovered(1)), (2, Some(0)));
        assert_eq!(
            menu.navigate(Nav::Enter),
            Outcome::Run(Command::OpenWith("viewer.desktop".into()))
        );
        assert_eq!(menu.navigate(Nav::Left), Outcome::Changed);
        assert_eq!((menu.levels(), menu.hovered(0)), (1, Some(with)));
    }

    #[test]
    fn space_picks_the_highlighted_row_like_return_and_cancel_closes_without_running() {
        let mut menu = open(item_menu(&subject(), &apps()));
        assert_eq!(
            menu.press_key(Key::Space),
            Outcome::Nothing,
            "nothing is highlighted yet"
        );
        let open_row = row_of(&menu, "Open");
        menu.hover(0, Some(open_row), 0.0);
        assert_eq!(menu.press_key(Key::Space), Outcome::Run(Command::Open));
        let with = row_of(&menu, "Open With");
        menu.hover(0, Some(with), 0.0);
        assert_eq!(menu.press_key(Key::Space), Outcome::Changed);
        assert_eq!(menu.levels(), 2, "Space on a submenu row opens it");
        assert_eq!(menu.press_key(Key::Cancel), Outcome::Close);
    }

    #[test]
    fn disabled_rows_cannot_be_hit_or_pressed() {
        let mut menu = open(item_menu(&subject(), &apps()));
        let info = row_of(&menu, "Get Info");
        let rect = menu.layout(0).rows[info];
        assert_eq!(menu.hit(0, rect.center()), None);
        assert_eq!(menu.press(0, info), Outcome::Nothing);
        let open_row = row_of(&menu, "Open");
        assert_eq!(menu.press(0, open_row), Outcome::Run(Command::Open));
    }
}
