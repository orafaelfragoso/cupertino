//! Layout of the bar and of pull-down menus.
//!
//! Text measurement is abstracted behind [`TextMeasure`] so the compositor can plug in its text
//! renderer. Every metric is approximate unless a doc comment says otherwise.

use crate::fallback::HELP_SEARCH;
use crate::geom::{Point, Rect};
use crate::item::{ItemId, MenuItem, ShortcutGlyphs, ShortcutStyle};

/// Measures the advance width of a run of text in the bar / menu font.
pub trait TextMeasure {
    fn width(&self, text: &str, bold: bool) -> f64;
}

/// A deterministic fixed-advance measurer for tests and headless use.
#[derive(Clone, Copy, Debug)]
pub struct MonoMeasure {
    pub advance: f64,
    pub bold_extra: f64,
}

impl Default for MonoMeasure {
    fn default() -> Self {
        Self {
            advance: 7.0,
            bold_extra: 0.5,
        }
    }
}

impl TextMeasure for MonoMeasure {
    fn width(&self, text: &str, bold: bool) -> f64 {
        text.chars().count() as f64 * (self.advance + if bold { self.bold_extra } else { 0.0 })
    }
}

/// Edge of a status item icon in points. approximate.
pub const STATUS_ICON_PT: f64 = 16.0;
/// Edge of the system menu's mark in points: the grid it is drawn on, so its strokes fall on whole pixels.
pub const SYSTEM_MARK_PT: f64 = 16.0;
/// Room on each side of the system menu's mark in its slot.
pub const SYSTEM_MARK_PADDING: f64 = 10.875;
/// Diameter of the privacy indicator dot attached to a menu-bar item (6 pt).
pub const PRIVACY_DOT_PT: f64 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct BarMetrics {
    /// Bar height: 33 pt, which leaves 5.5 pt above and below an item's 22 pt frame.
    pub height: f64,
    /// Space between the output edge and the first / last slot.
    pub edge_inset: f64,
    /// Horizontal padding inside a title slot (each side): half the space between two titles' text.
    pub title_padding: f64,
    /// Room on each side of a status item's content inside its frame, the shape its highlight fills.
    pub status_frame_padding: f64,
    /// Space between the frames of two items of the status area (status items, the control panel, the clock). Each
    /// slot reaches half of it past its frame, so the slots meet and two contents are `2 * status_frame_padding +
    /// status_spacing` apart.
    pub status_spacing: f64,
    /// How far the highlight of a title or the system mark reaches past its slot on each side.
    pub title_highlight_overhang: f64,
    /// Width of the system menu slot.
    pub system_width: f64,
    /// Width of the control panel slot: its glyph, the frame's padding and the spacing.
    pub control_panel_width: f64,
    /// Padding of the clock text (each side): the frame's padding and half the spacing.
    pub clock_padding: f64,
    /// Smallest gap kept between the menus and the status area before items are dropped.
    pub min_gap: f64,
    /// Corner radius of the selection highlight: a capsule when it is at least half the highlight's height.
    pub selection_corner_radius: f64,
    /// Vertical inset of the selection highlight inside the bar: the 22 pt item frame centred in the bar's height.
    pub selection_inset_y: f64,
}

impl Default for BarMetrics {
    fn default() -> Self {
        Self {
            height: 33.0,
            edge_inset: 6.0,
            title_padding: 10.0,
            status_frame_padding: 5.0,
            status_spacing: 6.0,
            title_highlight_overhang: 3.875,
            system_width: SYSTEM_MARK_PT + 2.0 * SYSTEM_MARK_PADDING,
            control_panel_width: 32.0,
            clock_padding: 8.0,
            min_gap: 24.0,
            selection_corner_radius: 11.0,
            selection_inset_y: (33.0 - Self::ITEM_FRAME_HEIGHT) / 2.0,
        }
    }
}

impl BarMetrics {
    /// Height of an item's frame (a title's, a status item's), the shape its highlight fills.
    pub const ITEM_FRAME_HEIGHT: f64 = 22.0;

    /// How far an item's frame sits below the top of a bar `height` points tall: the frame centred, never above the
    /// top when the bar is shorter than the frame.
    pub fn item_inset_y(height: f64) -> f64 {
        ((height - Self::ITEM_FRAME_HEIGHT) / 2.0).max(0.0)
    }

    /// Where the highlight of the slot of `key`, `width` points wide, lies along it: its left edge from the slot's and
    /// its width. A title's and the system mark's reach [`BarMetrics::title_highlight_overhang`] past the slot on each
    /// side; an item of the status area's fills its frame, the slot less half the spacing on each side.
    pub fn highlight_span(&self, key: &SlotKey, width: f64) -> (f64, f64) {
        match key {
            SlotKey::System | SlotKey::AppMenu | SlotKey::Menu(_) => (
                -self.title_highlight_overhang,
                width + 2.0 * self.title_highlight_overhang,
            ),
            SlotKey::Status(_) | SlotKey::ControlPanel | SlotKey::Clock => {
                let inset = self.status_spacing / 2.0;
                (inset, (width - 2.0 * inset).max(0.0))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SlotKey {
    /// The system menu at the left end of the bar.
    System,
    /// The bold app-name menu.
    AppMenu,
    Menu(ItemId),
    Status(String),
    ControlPanel,
    Clock,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusEntry {
    pub id: String,
    /// Icon width.
    pub width: f64,
    /// The context menu (a submenu item whose children are the entries), if the item has one.
    pub menu: Option<MenuItem>,
}

/// Everything the bar shows for one output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarContent {
    pub app_name: String,
    /// The entries of the system menu; none leaves the slot out.
    pub system_menu: Vec<MenuItem>,
    /// Items of the app menu opened by the bold app name.
    pub app_menu: Vec<MenuItem>,
    /// Top-level menus of the focused app: submenu items whose label is the title.
    pub menus: Vec<MenuItem>,
    /// Status items, nearest to the control panel first.
    pub status: Vec<StatusEntry>,
    pub clock: String,
    pub control_panel: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub key: SlotKey,
    pub rect: Rect,
    pub bold: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub bar: Rect,
    pub slots: Vec<Slot>,
    /// Menus that did not fit and are not shown.
    pub hidden_menus: Vec<ItemId>,
    /// Status items that did not fit.
    pub hidden_status: Vec<String>,
}

impl BarLayout {
    pub fn slot(&self, key: &SlotKey) -> Option<&Slot> {
        self.slots.iter().find(|s| &s.key == key)
    }

    pub fn hit(&self, p: Point) -> Option<&Slot> {
        self.bar
            .contains(p)
            .then(|| self.slots.iter().find(|s| s.rect.contains(p)))
            .flatten()
    }
}

/// Lay out the bar for an output `rect` (the bar occupies its top `metrics.height` points).
pub fn layout_bar(
    rect: Rect,
    content: &BarContent,
    metrics: &BarMetrics,
    measure: &dyn TextMeasure,
) -> BarLayout {
    let bar = Rect::new(rect.x, rect.y, rect.w, metrics.height);
    let mut slots = Vec::new();
    let mut x = bar.x + metrics.edge_inset;
    let push_left = |slots: &mut Vec<Slot>, key: SlotKey, text: &str, bold: bool, x: &mut f64| {
        let w = measure.width(text, bold) + 2.0 * metrics.title_padding;
        slots.push(Slot {
            key,
            rect: Rect::new(*x, bar.y, w, bar.h),
            bold,
        });
        *x += w;
    };
    if !content.system_menu.is_empty() {
        slots.push(Slot {
            key: SlotKey::System,
            rect: Rect::new(x, bar.y, metrics.system_width, bar.h),
            bold: false,
        });
        x += metrics.system_width;
    }
    push_left(
        &mut slots,
        SlotKey::AppMenu,
        &content.app_name,
        true,
        &mut x,
    );
    let left_menu_start = slots.len();
    let mut menu_ends = Vec::new();
    for m in &content.menus {
        push_left(&mut slots, SlotKey::Menu(m.id), &m.label, false, &mut x);
        menu_ends.push(x);
    }

    let mut right = bar.right() - metrics.edge_inset;
    let mut right_slots: Vec<Slot> = Vec::new();
    let clock_w = measure.width(&content.clock, false) + 2.0 * metrics.clock_padding;
    if !content.clock.is_empty() {
        right -= clock_w;
        right_slots.push(Slot {
            key: SlotKey::Clock,
            rect: Rect::new(right, bar.y, clock_w, bar.h),
            bold: false,
        });
    }
    if content.control_panel {
        right -= metrics.control_panel_width;
        right_slots.push(Slot {
            key: SlotKey::ControlPanel,
            rect: Rect::new(right, bar.y, metrics.control_panel_width, bar.h),
            bold: false,
        });
    }
    let fixed_right = right;
    let mut status_slots: Vec<Slot> = Vec::new();
    for s in &content.status {
        let w = s.width + 2.0 * metrics.status_frame_padding + metrics.status_spacing;
        right -= w;
        status_slots.push(Slot {
            key: SlotKey::Status(s.id.clone()),
            rect: Rect::new(right, bar.y, w, bar.h),
            bold: false,
        });
    }

    let mut hidden_menus = Vec::new();
    let mut hidden_status = Vec::new();
    let menus_end = |count: usize, app_end: f64, ends: &[f64]| {
        if count == 0 {
            app_end
        } else {
            ends[count - 1]
        }
    };
    let app_end = slots[left_menu_start - 1].rect.right();
    let mut menu_count = content.menus.len();
    let mut status_count = status_slots.len();
    let status_start = |n: usize, status: &[Slot]| {
        if n == 0 {
            fixed_right
        } else {
            status[n - 1].rect.x
        }
    };
    while menus_end(menu_count, app_end, &menu_ends) + metrics.min_gap
        > status_start(status_count, &status_slots)
    {
        if status_count > 0 {
            status_count -= 1;
            hidden_status.push(content.status[status_count].id.clone());
        } else if menu_count > 0 {
            menu_count -= 1;
            hidden_menus.push(content.menus[menu_count].id);
        } else {
            break;
        }
    }
    slots.truncate(left_menu_start + menu_count);
    status_slots.truncate(status_count);
    hidden_menus.reverse();
    hidden_status.reverse();
    slots.extend(status_slots.into_iter().rev());
    slots.extend(right_slots.into_iter().rev());
    BarLayout {
        bar,
        slots,
        hidden_menus,
        hidden_status,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuMetrics {
    pub row_height: f64,
    pub separator_height: f64,
    /// From the menu's edge to the labels, and to the ends of the separator lines.
    pub padding_x: f64,
    pub padding_y: f64,
    /// From the menu's trailing edge to the end of a shortcut or a submenu's chevron.
    pub trailing_inset: f64,
    /// Column reserved for check / radio marks, counted from `padding_x`, when any item has one.
    pub mark_column: f64,
    /// Column an icon takes before the label of its row.
    pub icon_column: f64,
    /// Height of the row of the Help search field, which holds the field with room above and below.
    pub search_row_height: f64,
    /// Gap between the label and the shortcut text.
    pub shortcut_gap: f64,
    /// Width reserved for the submenu arrow.
    pub arrow_width: f64,
    pub min_width: f64,
    /// Margin kept to the output edges.
    pub screen_margin: f64,
    /// Horizontal overlap of a submenu with its parent popup.
    pub submenu_overlap: f64,
    /// Gap between the label and the secondary text after it.
    pub detail_gap: f64,
    /// Smallest width of a menu with a search field, so its width holds while results come and go.
    pub search_width: f64,
    /// How the shortcuts are drawn: the Mac glyphs or the PC key names.
    pub shortcut_style: ShortcutStyle,
}

impl Default for MenuMetrics {
    fn default() -> Self {
        Self {
            row_height: 24.0,
            separator_height: 11.0,
            padding_x: 16.5,
            padding_y: 5.0,
            trailing_inset: 18.5,
            mark_column: 14.0,
            icon_column: 23.0,
            search_row_height: 35.0,
            shortcut_gap: 24.0,
            arrow_width: 10.0,
            min_width: 120.0,
            screen_margin: 4.0,
            submenu_overlap: 5.5,
            detail_gap: 8.0,
            search_width: 361.0,
            shortcut_style: ShortcutStyle::Glyphs,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupRow {
    /// A copy of the item without its children.
    pub item: MenuItem,
    pub rect: Rect,
    /// The shortcut's modifier glyphs and key, drawn in two columns.
    pub shortcut: Option<ShortcutGlyphs>,
    /// The secondary text, cut at its end with an ellipsis to the room left between the label and the shortcut.
    pub detail: Option<String>,
    /// Where the label starts, from the menu's leading edge: after the mark column when any row has a mark, and after
    /// the icon column on a row that has an icon (the rows without one are not indented).
    pub label_x: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupLayout {
    pub rect: Rect,
    pub rows: Vec<PopupRow>,
    /// The menu is taller than the room on the output; rows beyond `rect` are clipped and
    /// `scroll` moves them.
    pub clipped: bool,
    pub content_height: f64,
    pub scroll: f64,
    /// Submenu popups only: placed to the left of the parent because the right side had no room.
    pub placed_left: bool,
    /// The width of the column the shortcuts' keys stand in, right of their modifier glyphs.
    pub shortcut_key_width: f64,
    /// A status panel: its rows are laid out and drawn in the panel's style.
    pub panel: bool,
}

impl PopupLayout {
    pub fn row_at(&self, p: Point) -> Option<usize> {
        if !self.rect.contains(p) {
            return None;
        }
        self.rows
            .iter()
            .position(|r| r.rect.contains(p) && self.rect.contains(Point::new(p.x, p.y)))
    }

    /// The row that shows item `id`, or that shows its alternate in its place.
    pub fn index_of(&self, id: ItemId) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.item.id == id || r.item.alternate.as_ref().is_some_and(|a| a.id == id))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Drop down from a bar slot.
    BelowSlot(Rect),
    /// Open beside a row of a parent popup.
    BesideRow { row: Rect, parent: Rect },
}

/// Lay out the popup for the visible children of `menu`.
pub fn layout_popup(
    menu: &MenuItem,
    anchor: Anchor,
    screen: Rect,
    scroll: f64,
    m: &MenuMetrics,
    measure: &dyn TextMeasure,
) -> PopupLayout {
    let children = menu.visible_children();
    if children.iter().any(|c| c.panel.is_some()) {
        return layout_panel(menu, anchor, screen, m);
    }
    let any_mark = children
        .iter()
        .any(|c| c.toggle != crate::item::Toggle::None);
    let base = m.padding_x + if any_mark { m.mark_column } else { 0.0 };
    // a row with an icon starts its label after the icon column; the rows without one are not indented
    let lead_of = |c: &MenuItem| {
        base + if c.icon_name.is_some() || c.icon_data.is_some() {
            m.icon_column
        } else {
            0.0
        }
    };
    let mut label_w: f64 = 0.0;
    let mut modifiers_w: f64 = 0.0;
    let mut key_w: f64 = 0.0;
    let mut any_arrow = false;
    // a row is as wide as the wider of its item and the alternate shown in its place while Alt is held
    for c in children
        .iter()
        .flat_map(|c| std::iter::once(*c).chain(c.alternate.as_deref()))
    {
        if c.separator {
            continue;
        }
        label_w = label_w.max(lead_of(c) - base + measure.width(&c.label, false));
        if let Some(s) = &c.shortcut {
            let glyphs = s.shown(m.shortcut_style);
            modifiers_w = modifiers_w.max(measure.width(&glyphs.modifiers, false));
            key_w = key_w.max(measure.width(&glyphs.key, false));
        }
        any_arrow |= c.has_submenu;
    }
    let trailing = m.trailing_inset
        + if modifiers_w + key_w > 0.0 {
            m.shortcut_gap + modifiers_w + key_w
        } else {
            0.0
        }
        + if any_arrow { m.arrow_width } else { 0.0 };
    let has_field = children.iter().any(|c| c.id == HELP_SEARCH);
    let w = (base + label_w + trailing)
        .max(m.min_width)
        .max(if has_field { m.search_width } else { 0.0 });
    let height_of = |c: &MenuItem| {
        if c.separator {
            m.separator_height
        } else if c.id == HELP_SEARCH {
            m.search_row_height
        } else {
            m.row_height
        }
    };
    let content_h: f64 = 2.0 * m.padding_y + children.iter().map(|c| height_of(c)).sum::<f64>();

    let (mut x, anchor_y, placed_left) = match anchor {
        Anchor::BelowSlot(slot) => (slot.x, slot.bottom(), false),
        Anchor::BesideRow { row, parent } => {
            let right_x = parent.right() - m.submenu_overlap;
            if right_x + w <= screen.right() - m.screen_margin {
                (right_x, row.y - m.padding_y, false)
            } else {
                (parent.x - w + m.submenu_overlap, row.y - m.padding_y, true)
            }
        }
    };
    if !placed_left {
        x = x.min(screen.right() - m.screen_margin - w);
    }
    x = x.max(screen.x + m.screen_margin);
    let room_bottom = screen.bottom() - m.screen_margin;
    let mut y = anchor_y;
    let mut h = content_h;
    let clipped = y + h > room_bottom;
    if clipped {
        if matches!(anchor, Anchor::BesideRow { .. }) {
            y = (room_bottom - h).max(screen.y + m.screen_margin);
        }
        h = h.min(room_bottom - y);
    }
    let max_scroll = (content_h - h).max(0.0);
    let scroll = scroll.clamp(0.0, max_scroll);
    let rect = Rect::new(x, y, w, h);
    let mut ry = y + m.padding_y - scroll;
    let rows = children
        .iter()
        .map(|c| {
            let rh = height_of(c);
            let lead = lead_of(c);
            let r = Rect::new(x, ry, w, rh);
            ry += rh;
            let mut item = (*c).clone();
            item.children = Vec::new();
            let shortcut = c.shortcut.as_ref().map(|s| s.shown(m.shortcut_style));
            let detail = c.detail.as_deref().and_then(|detail| {
                let chord = shortcut.as_ref().map_or(0.0, |s| {
                    m.shortcut_gap + measure.width(&s.modifiers, false) + key_w
                });
                let arrow = if c.has_submenu { m.arrow_width } else { 0.0 };
                let room = w
                    - lead
                    - measure.width(&c.label, false)
                    - m.detail_gap
                    - chord
                    - arrow
                    - m.trailing_inset;
                fit_tail(detail, room, measure)
            });
            PopupRow {
                label_x: lead,
                shortcut,
                detail,
                item,
                rect: r,
            }
        })
        .collect();
    PopupLayout {
        rect,
        rows,
        clipped,
        content_height: content_h,
        scroll,
        placed_left,
        shortcut_key_width: key_w,
        panel: false,
    }
}

/// Sizes of a status panel, in points.
pub mod panel {
    /// The panel's width.
    pub const WIDTH: f64 = 308.5;
    /// Space between the bar and the panel.
    pub const GAP: f64 = 1.5;
    /// Corner radius.
    pub const RADIUS: f64 = 14.0;
    /// The row with the title and the switch.
    pub const HEADER: f64 = 41.5;
    /// A heading over the rows below it, and the row of the network joined.
    pub const SECTION: f64 = 24.0;
    pub const CURRENT: f64 = 40.0;
    /// The row that shows or hides the list, a network of the list, and a line of plain text.
    pub const DISCLOSURE: f64 = 32.0;
    pub const NETWORK: f64 = 32.0;
    pub const NOTE: f64 = 32.0;
    /// A plain text row that opens something.
    pub const LINK: f64 = 33.5;
    /// A dividing line, and its distance from the panel's sides.
    pub const SEPARATOR: f64 = 1.0;
    pub const SEPARATOR_INSET: f64 = 14.5;
    /// From the panel's edge to the text of its rows.
    pub const TEXT_INSET: f64 = 15.0;
    /// The switch of the header: its size and its distance from the trailing edge.
    pub const SWITCH_WIDTH: f64 = 54.0;
    pub const SWITCH_HEIGHT: f64 = 24.0;
    pub const SWITCH_INSET: f64 = 14.0;
    /// The knob of the switch.
    pub const KNOB_WIDTH: f64 = 32.0;
    pub const KNOB_HEIGHT: f64 = 20.0;
    /// The disc of the joined network, and where its name starts after it.
    pub const DISC: f64 = 26.0;
    pub const DISC_INSET: f64 = 14.5;
    pub const DISC_GAP: f64 = 9.0;
    /// From the trailing edge to the end of a lock.
    pub const LOCK_INSET: f64 = 16.5;
}

/// The height of a panel row.
fn panel_height(item: &MenuItem) -> f64 {
    use crate::item::PanelRow;
    if item.separator {
        return panel::SEPARATOR;
    }
    match &item.panel {
        Some(PanelRow::Header { .. }) | None => panel::HEADER,
        Some(PanelRow::Section { .. }) => panel::SECTION,
        Some(PanelRow::Current { .. }) => panel::CURRENT,
        Some(PanelRow::Disclosure { .. }) => panel::DISCLOSURE,
        Some(PanelRow::Network { .. }) => panel::NETWORK,
        Some(PanelRow::Note { .. }) => panel::NOTE,
        Some(PanelRow::Link { .. }) => panel::LINK,
    }
}

/// Lay out `menu` as a status panel: a fixed width, its trailing edge level with the item's, a little below the bar.
fn layout_panel(menu: &MenuItem, anchor: Anchor, screen: Rect, m: &MenuMetrics) -> PopupLayout {
    let children = menu.visible_children();
    let w = panel::WIDTH;
    let content_h: f64 = children.iter().map(|c| panel_height(c)).sum();
    let (right, top) = match anchor {
        Anchor::BelowSlot(slot) => (slot.right(), slot.bottom() + panel::GAP),
        Anchor::BesideRow { row, parent } => (parent.right() + w, row.y),
    };
    let x = (right - w)
        .min(screen.right() - m.screen_margin - w)
        .max(screen.x + m.screen_margin);
    let room = (screen.bottom() - m.screen_margin - top).max(0.0);
    let h = content_h.min(room);
    let mut y = top;
    let rows = children
        .iter()
        .map(|c| {
            let rh = panel_height(c);
            let rect = Rect::new(x, y, w, rh);
            y += rh;
            let mut item = (*c).clone();
            item.children = Vec::new();
            PopupRow {
                item,
                rect,
                shortcut: None,
                detail: None,
                label_x: panel::TEXT_INSET,
            }
        })
        .collect();
    PopupLayout {
        rect: Rect::new(x, top, w, h),
        rows,
        clipped: content_h > h,
        content_height: content_h,
        scroll: 0.0,
        placed_left: false,
        shortcut_key_width: 0.0,
        panel: true,
    }
}

/// `text`, or as much of its start as fits in `room` with an ellipsis after it; `None` when not even the ellipsis fits.
fn fit_tail(text: &str, room: f64, measure: &dyn TextMeasure) -> Option<String> {
    if measure.width(text, false) <= room {
        return Some(text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .rev()
        .map(|n| format!("{}…", chars[..n].iter().collect::<String>().trim_end()))
        .find(|cut| measure.width(cut, false) <= room)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Shortcut;

    const OUT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    fn content() -> BarContent {
        BarContent {
            app_name: "Files".into(),
            system_menu: vec![],
            app_menu: vec![],
            menus: vec![
                MenuItem::submenu(1, "File", vec![]),
                MenuItem::submenu(2, "Edit", vec![]),
                MenuItem::submenu(3, "View", vec![]),
            ],
            status: vec![
                StatusEntry {
                    id: "net".into(),
                    width: 16.0,
                    menu: None,
                },
                StatusEntry {
                    id: "vol".into(),
                    width: 16.0,
                    menu: None,
                },
            ],
            clock: "Tue 10:42".into(),
            control_panel: true,
        }
    }

    /// The bar is 33 points tall and an item's 22 point frame sits in its middle, 5.5 points from the top and the
    /// bottom, in the defaults and for any other height the theme gives.
    #[test]
    fn items_are_centred_in_the_bar_with_room_above_and_below() {
        let m = BarMetrics::default();
        assert_eq!(m.height, 33.0);
        assert_eq!(m.selection_inset_y, 5.5);
        assert_eq!(
            m.height - 2.0 * m.selection_inset_y,
            BarMetrics::ITEM_FRAME_HEIGHT
        );
        assert_eq!(BarMetrics::item_inset_y(24.0), 1.0);
        assert_eq!(BarMetrics::item_inset_y(20.0), 0.0);
    }

    #[test]
    fn slots_flow_left_and_right_without_overlap() {
        let l = layout_bar(
            OUT,
            &content(),
            &BarMetrics::default(),
            &MonoMeasure::default(),
        );
        assert_eq!(l.bar, Rect::new(0.0, 0.0, 1920.0, 33.0));
        let keys: Vec<&SlotKey> = l.slots.iter().map(|s| &s.key).collect();
        assert_eq!(
            keys,
            vec![
                &SlotKey::AppMenu,
                &SlotKey::Menu(ItemId(1)),
                &SlotKey::Menu(ItemId(2)),
                &SlotKey::Menu(ItemId(3)),
                &SlotKey::Status("vol".into()),
                &SlotKey::Status("net".into()),
                &SlotKey::ControlPanel,
                &SlotKey::Clock,
            ]
        );
        for w in l.slots.windows(2) {
            assert!(
                w[0].rect.right() <= w[1].rect.x + 1e-9,
                "{:?} overlaps {:?}",
                w[0],
                w[1]
            );
        }
        let app = l.slot(&SlotKey::AppMenu).unwrap();
        assert!(app.bold);
        assert_eq!(app.rect.x, 6.0);
        let metrics = BarMetrics::default();
        assert!((app.rect.w - (5.0 * 7.5 + 2.0 * metrics.title_padding)).abs() < 1e-9);
        let clock = l.slot(&SlotKey::Clock).unwrap();
        assert!((clock.rect.right() - (1920.0 - 6.0)).abs() < 1e-9);
        assert!(l.hidden_menus.is_empty() && l.hidden_status.is_empty());
    }

    /// A title is its text and 10 points on each side; the items of the status area are 16 points apart from content to
    /// content, each highlight is its frame (the content and 5 points on each side) with no overhang, and two frames
    /// are 6 points apart.
    #[test]
    fn titles_and_status_items_are_spaced_by_their_frames() {
        let metrics = BarMetrics::default();
        let measure = MonoMeasure::default();
        let l = layout_bar(OUT, &content(), &metrics, &measure);
        let file = l.slot(&SlotKey::Menu(ItemId(1))).unwrap().rect;
        assert!((file.w - (measure.width("File", false) + 10.0 + 10.0)).abs() < 1e-9);
        let rect = |key: SlotKey| l.slot(&key).unwrap().rect;
        let (vol, net) = (
            rect(SlotKey::Status("vol".into())),
            rect(SlotKey::Status("net".into())),
        );
        let content_x = |r: Rect| r.x + (r.w - 16.0) / 2.0;
        assert!((content_x(net) - content_x(vol) - (16.0 + 16.0)).abs() < 1e-9);
        let frame = |key: SlotKey, r: Rect| {
            let (dx, w) = metrics.highlight_span(&key, r.w);
            (r.x + dx, w)
        };
        let (vol_x, vol_w) = frame(SlotKey::Status("vol".into()), vol);
        let (net_x, net_w) = frame(SlotKey::Status("net".into()), net);
        assert!((vol_w - (16.0 + 10.0)).abs() < 1e-9 && (net_w - 26.0).abs() < 1e-9);
        assert!(
            (net_x - (vol_x + vol_w) - 6.0).abs() < 1e-9,
            "frames 6 apart"
        );
        let cp = rect(SlotKey::ControlPanel);
        let clock = rect(SlotKey::Clock);
        let (cp_x, cp_w) = frame(SlotKey::ControlPanel, cp);
        let (clock_x, clock_w) = frame(SlotKey::Clock, clock);
        assert!((cp_x - (net_x + net_w) - 6.0).abs() < 1e-9);
        assert!((clock_x - (cp_x + cp_w) - 6.0).abs() < 1e-9);
        assert!((clock_w - (measure.width("Tue 10:42", false) + 10.0)).abs() < 1e-9);
        // the highlight lies inside the item's own slot: no overhang
        for (key, r) in [
            (SlotKey::Status("vol".into()), vol),
            (SlotKey::ControlPanel, cp),
            (SlotKey::Clock, clock),
        ] {
            let (dx, w) = metrics.highlight_span(&key, r.w);
            assert!(dx >= 0.0 && dx + w <= r.w, "{key:?}");
        }
        // a title's highlight still reaches past its slot
        assert!(metrics.highlight_span(&SlotKey::Menu(ItemId(1)), file.w).0 < 0.0);
    }

    #[test]
    fn status_items_are_ordered_from_the_control_panel_outwards() {
        let l = layout_bar(
            OUT,
            &content(),
            &BarMetrics::default(),
            &MonoMeasure::default(),
        );
        let cp = l.slot(&SlotKey::ControlPanel).unwrap().rect;
        let net = l.slot(&SlotKey::Status("net".into())).unwrap().rect;
        let vol = l.slot(&SlotKey::Status("vol".into())).unwrap().rect;
        assert!((net.right() - cp.x).abs() < 1e-9);
        assert!(vol.right() <= net.x + 1e-9);
    }

    #[test]
    fn narrow_bars_drop_status_items_then_menus() {
        let narrow = Rect::new(0.0, 0.0, 300.0, 400.0);
        let l = layout_bar(
            narrow,
            &content(),
            &BarMetrics::default(),
            &MonoMeasure::default(),
        );
        assert!(!l.hidden_status.is_empty() || !l.hidden_menus.is_empty());
        for w in l.slots.windows(2) {
            assert!(w[0].rect.right() <= w[1].rect.x + 1e-9);
        }
        assert!(
            l.slot(&SlotKey::AppMenu).is_some(),
            "the app menu always stays"
        );
        let tiny = Rect::new(0.0, 0.0, 120.0, 400.0);
        let l = layout_bar(
            tiny,
            &content(),
            &BarMetrics::default(),
            &MonoMeasure::default(),
        );
        assert_eq!(l.hidden_status.len(), 2);
        assert_eq!(l.hidden_menus, vec![ItemId(1), ItemId(2), ItemId(3)]);
        assert!(l.slot(&SlotKey::AppMenu).is_some());
    }

    #[test]
    fn hit_testing() {
        let l = layout_bar(
            OUT,
            &content(),
            &BarMetrics::default(),
            &MonoMeasure::default(),
        );
        let file = l.slot(&SlotKey::Menu(ItemId(1))).unwrap().rect;
        assert_eq!(
            l.hit(Point::new(file.x + 1.0, 10.0)).unwrap().key,
            SlotKey::Menu(ItemId(1))
        );
        assert!(
            l.hit(Point::new(file.x + 1.0, 40.0)).is_none(),
            "below the bar"
        );
        assert!(l.hit(Point::new(900.0, 10.0)).is_none(), "gap");
    }

    fn file_menu() -> MenuItem {
        use crate::item::{Shortcut, Toggle};
        MenuItem::submenu(
            1,
            "File",
            vec![
                MenuItem::new(10, "New Window")
                    .with_shortcut(Shortcut::from_tokens(&["Control", "N"]).unwrap()),
                MenuItem::separator(11),
                MenuItem::new(12, "Show Hidden").with_toggle(Toggle::Check(true)),
                MenuItem::submenu(13, "Open Recent", vec![MenuItem::new(14, "a.txt")]),
                MenuItem::new(15, "Quit").disabled(),
            ],
        )
    }

    #[test]
    fn popup_rows_have_the_right_sizes_and_positions() {
        let slot = Rect::new(60.0, 0.0, 40.0, 24.0);
        let p = layout_popup(
            &file_menu(),
            Anchor::BelowSlot(slot),
            OUT,
            0.0,
            &MenuMetrics::default(),
            &MonoMeasure::default(),
        );
        assert_eq!(p.rows.len(), 5);
        assert_eq!(p.rect.x, 60.0);
        assert_eq!(p.rect.y, 24.0);
        let m = MenuMetrics::default();
        assert!(
            (p.rect.h - (2.0 * m.padding_y + 4.0 * m.row_height + m.separator_height)).abs() < 1e-9
        );
        assert_eq!(p.rows[0].rect.y, 24.0 + m.padding_y);
        assert_eq!(p.rows[1].rect.h, m.separator_height);
        assert_eq!(
            p.rows[0].shortcut,
            Some(ShortcutGlyphs {
                modifiers: "\u{2303}".into(),
                key: "N".into()
            })
        );
        assert_eq!(p.shortcut_key_width, 7.0, "one character of the key column");
        let min_w = m.padding_x
            + m.mark_column
            + 11.0 * 7.0
            + m.trailing_inset
            + m.shortcut_gap
            + 2.0 * 7.0
            + m.arrow_width;
        assert!((p.rect.w - min_w.max(m.min_width)).abs() < 1e-9);
        assert!(p.rows.iter().all(|r| r.item.children.is_empty()));
        assert!(!p.clipped);
        assert_eq!(p.row_at(Point::new(70.0, p.rows[2].rect.y + 1.0)), Some(2));
        assert_eq!(p.row_at(Point::new(5.0, 30.0)), None);
        assert_eq!(p.index_of(ItemId(13)), Some(3));
    }

    #[test]
    fn a_row_is_sized_for_the_wider_of_its_item_and_its_alternate() {
        let menu = MenuItem::submenu(
            1,
            "Window",
            vec![
                MenuItem::new(10, "Zoom").with_alternate(
                    MenuItem::new(11, "Zoom Every Window There Is")
                        .with_shortcut(Shortcut::from_tokens(&["Alt", "Super", "Z"]).unwrap()),
                ),
                MenuItem::new(12, "Close"),
            ],
        );
        let slot = Anchor::BelowSlot(Rect::new(60.0, 0.0, 40.0, 24.0));
        let (m, measure) = (MenuMetrics::default(), MonoMeasure::default());
        let plain = layout_popup(&menu, slot, OUT, 0.0, &m, &measure);
        let alt = layout_popup(&menu.alternates_shown(), slot, OUT, 0.0, &m, &measure);
        assert_eq!(plain.rows[0].item.id, ItemId(10));
        assert_eq!(alt.rows[0].item.id, ItemId(11));
        assert_eq!(alt.rows[1].item.id, ItemId(12));
        assert_eq!(
            plain.rect, alt.rect,
            "Alt swaps a row without resizing the menu"
        );
        assert_eq!(plain.shortcut_key_width, alt.shortcut_key_width);
        let alternate = m.padding_x + 26.0 * 7.0 + m.trailing_inset + m.shortcut_gap + 3.0 * 7.0;
        assert!((plain.rect.w - alternate).abs() < 1e-9);
        assert_eq!(plain.index_of(ItemId(11)), Some(0));
        assert_eq!(alt.index_of(ItemId(10)), Some(0));
    }

    #[test]
    fn popup_is_clamped_to_the_output() {
        let slot = Rect::new(1900.0, 0.0, 20.0, 24.0);
        let p = layout_popup(
            &file_menu(),
            Anchor::BelowSlot(slot),
            OUT,
            0.0,
            &MenuMetrics::default(),
            &MonoMeasure::default(),
        );
        assert!(p.rect.right() <= 1920.0 - 4.0 + 1e-9);
    }

    #[test]
    fn secondary_text_is_cut_at_its_end_before_the_shortcut_and_the_label_never_is() {
        let (m, measure) = (MenuMetrics::default(), MonoMeasure::default());
        let path = "Format ▸ Paragraph ▸ Alignment ▸ Justification ▸ Options";
        let result = |id: i32, label: &str, detail: &str, key: &str| MenuItem {
            detail: Some(detail.into()),
            ..MenuItem::new(id, label)
                .with_shortcut(Shortcut::from_tokens(&["Control", key]).unwrap())
        };
        let long_label = "A label far too long to share its row with any path at all";
        let help = MenuItem::submenu(
            HELP_SEARCH.0 - 1,
            "Help",
            vec![
                MenuItem::new(HELP_SEARCH.0, "Search").disabled(),
                result(1, "Save", "File", "S"),
                result(2, "Justify", path, "J"),
            ],
        );
        let slot = Anchor::BelowSlot(Rect::new(60.0, 0.0, 40.0, 24.0));
        let p = layout_popup(&help, slot, OUT, 0.0, &m, &measure);
        assert_eq!(
            p.rect.w, m.search_width,
            "a menu with the field keeps its width"
        );
        assert_eq!(p.rows[1].detail.as_deref(), Some("File"));

        let row = &p.rows[2];
        let cut = row.detail.as_deref().unwrap();
        assert_eq!(row.item.label, "Justify");
        assert!(
            cut.ends_with('…') && path.starts_with(cut.trim_end_matches('…')),
            "{cut}"
        );
        let glyphs = row.shortcut.as_ref().unwrap();
        let chord_x = p.rect.right()
            - m.padding_x
            - p.shortcut_key_width
            - measure.width(&glyphs.modifiers, false)
            - m.shortcut_gap;
        let detail_end = p.rect.x
            + m.padding_x
            + measure.width("Justify", false)
            + m.detail_gap
            + measure.width(cut, false);
        assert!(detail_end <= chord_x + 1e-9, "{detail_end} > {chord_x}");
        assert!(
            detail_end + measure.advance > chord_x,
            "it is cut no shorter than it must"
        );

        let mut wide = help.clone();
        wide.children.push(result(3, long_label, path, "L"));
        let p = layout_popup(&wide, slot, OUT, 0.0, &m, &measure);
        assert!(
            p.rect.w > m.search_width,
            "the menu widens for a long label"
        );
        assert_eq!(p.rows[3].item.label, long_label);
        assert_eq!(p.rows[3].detail, None, "no room is left for the path");
    }

    #[test]
    fn submenu_flips_to_the_left_when_there_is_no_room() {
        let m = MenuMetrics::default();
        let parent = Rect::new(1700.0, 24.0, 200.0, 120.0);
        let row = Rect::new(1700.0, 60.0, 200.0, 22.0);
        let sub = file_menu();
        let p = layout_popup(
            &sub,
            Anchor::BesideRow { row, parent },
            OUT,
            0.0,
            &m,
            &MonoMeasure::default(),
        );
        assert!(p.placed_left);
        assert!(p.rect.right() <= parent.right());
        assert_eq!(p.rect.y, 60.0 - m.padding_y);
        let parent = Rect::new(100.0, 24.0, 200.0, 120.0);
        let p = layout_popup(
            &sub,
            Anchor::BesideRow {
                row: Rect::new(100.0, 60.0, 200.0, 22.0),
                parent,
            },
            OUT,
            0.0,
            &m,
            &MonoMeasure::default(),
        );
        assert!(!p.placed_left);
        assert_eq!(p.rect.x, parent.right() - m.submenu_overlap);
    }

    #[test]
    fn tall_menus_are_clipped_and_scroll() {
        let items: Vec<MenuItem> = (0..100)
            .map(|i| MenuItem::new(i + 1, &format!("item {i}")))
            .collect();
        let menu = MenuItem::submenu(0, "Long", items);
        let screen = Rect::new(0.0, 0.0, 800.0, 400.0);
        let m = MenuMetrics::default();
        let p = layout_popup(
            &menu,
            Anchor::BelowSlot(Rect::new(10.0, 0.0, 40.0, 24.0)),
            screen,
            0.0,
            &m,
            &MonoMeasure::default(),
        );
        assert!(p.clipped);
        assert!(p.rect.bottom() <= 400.0 - m.screen_margin + 1e-9);
        let q = layout_popup(
            &menu,
            Anchor::BelowSlot(Rect::new(10.0, 0.0, 40.0, 24.0)),
            screen,
            100.0,
            &m,
            &MonoMeasure::default(),
        );
        assert_eq!(q.scroll, 100.0);
        assert!((q.rows[0].rect.y - (p.rows[0].rect.y - 100.0)).abs() < 1e-9);
        let r = layout_popup(
            &menu,
            Anchor::BelowSlot(Rect::new(10.0, 0.0, 40.0, 24.0)),
            screen,
            1e9,
            &m,
            &MonoMeasure::default(),
        );
        assert!((r.scroll - (r.content_height - r.rect.h)).abs() < 1e-9);
    }

    #[test]
    fn hidden_items_and_edge_separators_do_not_take_room() {
        let menu = MenuItem::submenu(
            0,
            "m",
            vec![
                MenuItem::separator(1),
                MenuItem::new(2, "only"),
                MenuItem {
                    visible: false,
                    ..MenuItem::new(3, "hidden")
                },
                MenuItem::separator(4),
            ],
        );
        let p = layout_popup(
            &menu,
            Anchor::BelowSlot(Rect::new(0.0, 0.0, 10.0, 24.0)),
            OUT,
            0.0,
            &MenuMetrics::default(),
            &MonoMeasure::default(),
        );
        assert_eq!(p.rows.len(), 1);
    }

    #[test]
    fn the_system_menu_slot_is_the_first_slot_and_leaves_the_app_menu_after_it() {
        let mut with = content();
        with.system_menu = vec![MenuItem::new(1, "Sleep")];
        let metrics = BarMetrics::default();
        let measure = MonoMeasure::default();
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let plain = layout_bar(rect, &content(), &metrics, &measure);
        let l = layout_bar(rect, &with, &metrics, &measure);
        assert_eq!(l.slots[0].key, SlotKey::System);
        assert_eq!(l.slots[0].rect.x, metrics.edge_inset);
        assert_eq!(l.slots[0].rect.w, metrics.system_width);
        assert_eq!(l.slots[1].key, SlotKey::AppMenu);
        assert_eq!(
            l.slots[1].rect.x,
            plain.slots[0].rect.x + metrics.system_width,
            "the app menu moves right by the mark's width"
        );
        assert_eq!(
            plain.slots[0].key,
            SlotKey::AppMenu,
            "without entries no slot"
        );
    }

    #[test]
    fn the_names_style_lays_out_the_pc_names_of_the_keys() {
        let menu = MenuItem::submenu(
            0,
            "",
            vec![MenuItem::new(1, "Close Window")
                .with_shortcut(Shortcut::from_tokens(&["Alt", "F4"]).unwrap())],
        );
        let mut m = MenuMetrics::default();
        let measure = MonoMeasure::default();
        let shown = |m: &MenuMetrics| {
            layout_popup(
                &menu,
                Anchor::BelowSlot(Rect::new(0.0, 0.0, 40.0, 24.0)),
                OUT,
                0.0,
                m,
                &measure,
            )
            .rows[0]
                .shortcut
                .clone()
                .unwrap()
        };
        assert_eq!(shown(&m).modifiers, "\u{2325}", "glyphs by default");
        m.shortcut_style = ShortcutStyle::Names;
        assert_eq!(
            shown(&m),
            ShortcutGlyphs {
                modifiers: "Alt+".into(),
                key: "F4".into()
            }
        );
    }
}
