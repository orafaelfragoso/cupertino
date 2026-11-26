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
/// Diameter of the screen-capture indicator dot, the right-most thing on the bar while a capture runs.
pub const PRIVACY_DOT_PT: f64 = 9.0;
/// Room between the indicator dot's right edge and the output's right edge.
pub const PRIVACY_DOT_MARGIN: f64 = 7.5;
/// Room between the clock's text and the indicator dot right of it.
pub const PRIVACY_DOT_GAP: f64 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub struct BarMetrics {
    /// Bar height: 33 pt, which leaves 5.5 pt above and below an item's 22 pt frame.
    pub height: f64,
    /// Space between the output edge and the first / last slot.
    pub edge_inset: f64,
    /// Horizontal padding inside a title slot (each side): half the space between two titles' text.
    pub title_padding: f64,
    /// Room on each side of the content of an item of the status area (status items, the control panel, the clock)
    /// inside its slot. The slots meet, so two contents are `2 * status_frame_padding` apart.
    pub status_frame_padding: f64,
    /// How far the highlight of an item of the status area reaches past its slot on each side, over the edge of its
    /// neighbour's: its capsule is the content and `status_frame_padding + status_highlight_overhang` on each side.
    pub status_highlight_overhang: f64,
    /// How far the highlight of a title or the system mark reaches past its slot on each side.
    pub title_highlight_overhang: f64,
    /// Width of the system menu slot.
    pub system_width: f64,
    /// Width of the control panel slot: its glyph and the frame's padding on each side.
    pub control_panel_width: f64,
    /// Padding of the clock text (each side): the frame's padding.
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
            status_frame_padding: 10.5,
            status_highlight_overhang: 2.0,
            title_highlight_overhang: 3.875,
            system_width: SYSTEM_MARK_PT + 2.0 * SYSTEM_MARK_PADDING,
            control_panel_width: STATUS_ICON_PT + 2.0 * 10.5,
            clock_padding: 10.5,
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
    /// side, an item of the status area's [`BarMetrics::status_highlight_overhang`].
    pub fn highlight_span(&self, key: &SlotKey, width: f64) -> (f64, f64) {
        let overhang = match key {
            SlotKey::System | SlotKey::AppMenu | SlotKey::Menu(_) => self.title_highlight_overhang,
            SlotKey::Status(_) | SlotKey::ControlPanel | SlotKey::Clock => {
                self.status_highlight_overhang
            }
        };
        (-overhang, width + 2.0 * overhang)
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
    /// The indicator dot laid out right of the clock, at the bar's right end (its width is the dot's diameter).
    pub indicator: Option<StatusEntry>,
    pub clock: String,
    pub control_panel: bool,
    /// The application's menus ask to be noticed (its DBusMenu exporter's `Status` is `notice`): the bold application
    /// name shows a dot after it.
    pub notice: bool,
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
    let mut clock_w = measure.width(&content.clock, false) + 2.0 * metrics.clock_padding;
    if let Some(dot) = &content.indicator {
        // the dot centred in its slot, `PRIVACY_DOT_MARGIN` from the output's edge; the slot reaches half the gap to
        // the clock's text, and the clock keeps the other half on its right
        let half_gap = PRIVACY_DOT_GAP / 2.0;
        let w = dot.width + 2.0 * half_gap;
        let x = bar.right() - PRIVACY_DOT_MARGIN - dot.width - half_gap;
        right_slots.push(Slot {
            key: SlotKey::Status(dot.id.clone()),
            rect: Rect::new(x, bar.y, w, bar.h),
            bold: false,
        });
        right = x;
        clock_w += half_gap - metrics.clock_padding;
    }
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
        let w = s.width + 2.0 * metrics.status_frame_padding;
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
    /// A heading over the rows below it, and the row of the network joined or of a Bluetooth device.
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
        Some(PanelRow::Current { .. } | PanelRow::Device { .. }) => panel::CURRENT,
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
