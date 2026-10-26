//! Menu interaction state machine: pointer press-drag-release, click-sticky menus, hover switching
//! between open menus, keyboard navigation and submenus with a safe triangle.
//!
//! The controller owns the bar content and the layouts, and is driven with absolute timestamps in
//! seconds. Every call returns an [`Outcome`] with the effects the compositor (or the D-Bus layer)
//! must perform; popups to draw are read from [`MenuController::levels`].
//!
//! the interaction model (press-drag-release selects in one gesture, a click opens
//! a sticky menu, hovering across titles switches menus while one is open, arrows navigate,
//! type-ahead selects, Esc closes, a shortcut focuses the bar) is as designed behaviour of the
//! design. The timings and the safe-triangle rule are
//! approximate; see [`ControllerConfig`].

use crate::geom::{Point, Rect};
use crate::item::{ItemId, MenuItem};
use crate::layout::{
    layout_bar, layout_popup, Anchor, BarContent, BarLayout, BarMetrics, MenuMetrics, PopupLayout,
    SlotKey, TextMeasure,
};

/// Identifier of the synthetic root item of the app menu.
pub const APP_MENU_ROOT: ItemId = ItemId(i32::MIN);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Escape,
    Home,
    End,
    Char(char),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Redraw,
    /// A menu is about to be shown: let the source refresh or lazily load `item` (DBusMenu
    /// `AboutToShow`) and report new content through [`MenuController::set_content`].
    AboutToShow {
        slot: SlotKey,
        item: ItemId,
    },
    Opened {
        slot: SlotKey,
    },
    /// All menus closed.
    Closed,
    /// The user chose an item.
    Activate {
        slot: SlotKey,
        item: ItemId,
    },
    /// A slot without a menu (clock, control panel, status item) was clicked.
    SlotActivated {
        slot: SlotKey,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub effects: Vec<Effect>,
    /// The event was used by the menu system and must not reach other clients.
    pub consumed: bool,
}

#[derive(Clone, Debug)]
pub struct ControllerConfig {
    /// Hover time on a submenu row before its popup opens. approximate.
    pub submenu_open_delay: f64,
    /// Time before the popup of a submenu closes after the pointer moved to another row outside the
    /// safe triangle. approximate.
    pub submenu_close_delay: f64,
    /// Keep a submenu open while the pointer travels towards it inside a triangle. approximate.
    pub safe_triangle: bool,
    /// Seconds after which the type-ahead buffer restarts. approximate.
    pub type_ahead_timeout: f64,
    /// Releasing the button over the open title after a press leaves the menu open (click-sticky).
    pub release_on_title_keeps_open: bool,
    /// A click outside the menus closes them and is also delivered to the client below.
    pub click_outside_passthrough: bool,
    /// Hovering another title while a menu is open switches to it.
    pub hover_switch: bool,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            submenu_open_delay: 0.2,
            submenu_close_delay: 0.3,
            safe_triangle: true,
            type_ahead_timeout: 1.0,
            release_on_title_keeps_open: true,
            click_outside_passthrough: false,
            hover_switch: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    /// The bar has keyboard focus on a title; no menu is open.
    TitleFocus,
    /// A button is held down; releasing it chooses.
    Tracking,
    /// Menus stay open after a click.
    Sticky,
    /// Menus are open and driven by the keyboard.
    Keyboard,
}

/// One open popup.
#[derive(Clone, Debug)]
pub struct Level {
    /// The menu item whose children this popup shows.
    pub menu: MenuItem,
    pub layout: PopupLayout,
    /// Index of the highlighted row in `layout.rows`.
    pub selected: Option<usize>,
}

impl Level {
    pub fn selected_item(&self) -> Option<&MenuItem> {
        self.selected
            .and_then(|i| self.layout.rows.get(i))
            .map(|r| &r.item)
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    level: usize,
    open: Option<ItemId>,
    due: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Popup { level: usize, row: usize },
    PopupPadding { level: usize },
    Slot,
    Bar,
    Outside,
}

pub struct MenuController {
    pub cfg: ControllerConfig,
    pub bar_metrics: BarMetrics,
    pub menu_metrics: MenuMetrics,
    measure: Box<dyn TextMeasure>,
    output: Rect,
    content: BarContent,
    layout: BarLayout,
    mode: Mode,
    open_slot: Option<SlotKey>,
    levels: Vec<Level>,
    title_focus: Option<SlotKey>,
    pressed_button: Option<SlotKey>,
    hit_slot: Option<SlotKey>,
    last_pointer: Option<Point>,
    type_ahead: String,
    type_ahead_at: f64,
    pending: Option<Pending>,
    keyboard_origin: bool,
}

impl MenuController {
    pub fn new(output: Rect, measure: Box<dyn TextMeasure>) -> Self {
        let mut c = Self {
            cfg: ControllerConfig::default(),
            bar_metrics: BarMetrics::default(),
            menu_metrics: MenuMetrics::default(),
            measure,
            output,
            content: BarContent::default(),
            layout: BarLayout {
                bar: Rect::default(),
                slots: Vec::new(),
                hidden_menus: Vec::new(),
                hidden_status: Vec::new(),
            },
            mode: Mode::Idle,
            open_slot: None,
            levels: Vec::new(),
            title_focus: None,
            pressed_button: None,
            hit_slot: None,
            last_pointer: None,
            type_ahead: String::new(),
            type_ahead_at: 0.0,
            pending: None,
            keyboard_origin: false,
        };
        c.relayout();
        c
    }

    // ---- accessors -----------------------------------------------------------------------

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn layout(&self) -> &BarLayout {
        &self.layout
    }

    pub fn content(&self) -> &BarContent {
        &self.content
    }

    pub fn levels(&self) -> &[Level] {
        &self.levels
    }

    pub fn is_open(&self) -> bool {
        !self.levels.is_empty()
    }

    pub fn open_slot(&self) -> Option<&SlotKey> {
        self.open_slot.as_ref()
    }

    /// The slot to draw highlighted: the open title, or the keyboard-focused title.
    pub fn highlighted_slot(&self) -> Option<&SlotKey> {
        self.open_slot.as_ref().or(self.title_focus.as_ref())
    }

    /// Time of the next pending timer, if any.
    pub fn next_deadline(&self) -> Option<f64> {
        self.pending.map(|p| p.due)
    }

    // ---- content -------------------------------------------------------------------------

    pub fn set_output(&mut self, output: Rect) {
        self.output = output;
        self.relayout();
        self.rebuild_levels();
    }

    /// Replace the bar content (focused app changed, menus updated, status items changed). Open
    /// menus are kept when their items still exist.
    pub fn set_content(&mut self, content: BarContent) -> Outcome {
        self.content = content;
        self.relayout();
        let mut out = Outcome::default();
        if let Some(k) = &self.open_slot {
            if self.layout.slot(k).is_none() || self.root_item(k).is_none() {
                self.close_all(&mut out);
            }
        }
        if self
            .title_focus
            .as_ref()
            .is_some_and(|k| self.layout.slot(k).is_none())
        {
            self.title_focus = None;
            if self.mode == Mode::TitleFocus {
                self.mode = Mode::Idle;
            }
        }
        self.rebuild_levels();
        out.effects.push(Effect::Redraw);
        out
    }

    fn relayout(&mut self) {
        self.layout = layout_bar(
            self.output,
            &self.content,
            &self.bar_metrics,
            self.measure.as_ref(),
        );
    }

    fn root_item(&self, key: &SlotKey) -> Option<MenuItem> {
        match key {
            SlotKey::AppMenu => Some(MenuItem {
                id: APP_MENU_ROOT,
                has_submenu: true,
                children: self.content.app_menu.clone(),
                label: self.content.app_name.clone(),
                enabled: true,
                visible: true,
                ..Default::default()
            }),
            SlotKey::Menu(id) => self.content.menus.iter().find(|m| m.id == *id).cloned(),
            SlotKey::Status(id) => self
                .content
                .status
                .iter()
                .find(|s| &s.id == id)
                .and_then(|s| s.menu.clone()),
            SlotKey::ControlPanel | SlotKey::Clock => None,
        }
    }

    fn slot_has_menu(&self, key: &SlotKey) -> bool {
        self.root_item(key).is_some()
    }

    fn menu_slots(&self) -> Vec<SlotKey> {
        self.layout
            .slots
            .iter()
            .filter(|s| self.slot_has_menu(&s.key))
            .map(|s| s.key.clone())
            .collect()
    }

    fn rebuild_levels(&mut self) {
        let Some(key) = self.open_slot.clone() else {
            return;
        };
        let Some(root) = self.root_item(&key) else {
            self.levels.clear();
            return;
        };
        let Some(slot_rect) = self.layout.slot(&key).map(|s| s.rect) else {
            self.levels.clear();
            return;
        };
        let old: Vec<(ItemId, Option<ItemId>, f64)> = self
            .levels
            .iter()
            .map(|l| (l.menu.id, l.selected_item().map(|i| i.id), l.layout.scroll))
            .collect();
        let mut new_levels: Vec<Level> = Vec::new();
        for (i, (menu_id, sel, scroll)) in old.iter().enumerate() {
            let menu = if i == 0 {
                Some(root.clone())
            } else {
                new_levels[i - 1]
                    .selected_item()
                    .and_then(|r| new_levels[i - 1].menu.find(r.id))
                    .filter(|m| m.id == *menu_id)
                    .cloned()
            };
            let Some(menu) = menu else { break };
            let anchor = if i == 0 {
                Anchor::BelowSlot(slot_rect)
            } else {
                let p = &new_levels[i - 1];
                let row = p
                    .selected
                    .map(|s| p.layout.rows[s].rect)
                    .unwrap_or_default();
                Anchor::BesideRow {
                    row,
                    parent: p.layout.rect,
                }
            };
            let layout = layout_popup(
                &menu,
                anchor,
                self.output,
                *scroll,
                &self.menu_metrics,
                self.measure.as_ref(),
            );
            let selected = sel.and_then(|id| layout.index_of(id));
            new_levels.push(Level {
                menu,
                layout,
                selected,
            });
        }
        self.levels = new_levels;
        if self.levels.is_empty() {
            self.open_slot = None;
            if self.mode != Mode::TitleFocus {
                self.mode = Mode::Idle;
            }
        }
    }

    // ---- hit testing ---------------------------------------------------------------------

    fn hit(&mut self, p: Point) -> Hit {
        self.hit_slot = None;
        for (i, l) in self.levels.iter().enumerate().rev() {
            if l.layout.rect.contains(p) {
                return match l.layout.row_at(p) {
                    Some(row) => Hit::Popup { level: i, row },
                    None => Hit::PopupPadding { level: i },
                };
            }
        }
        if let Some(s) = self.layout.hit(p) {
            self.hit_slot = Some(s.key.clone());
            return Hit::Slot;
        }
        if self.layout.bar.contains(p) {
            Hit::Bar
        } else {
            Hit::Outside
        }
    }

    // ---- opening and closing -------------------------------------------------------------

    fn open_title(&mut self, key: SlotKey, out: &mut Outcome) {
        let Some(root) = self.root_item(&key) else {
            return;
        };
        let Some(slot_rect) = self.layout.slot(&key).map(|s| s.rect) else {
            return;
        };
        let layout = layout_popup(
            &root,
            Anchor::BelowSlot(slot_rect),
            self.output,
            0.0,
            &self.menu_metrics,
            self.measure.as_ref(),
        );
        out.effects.push(Effect::AboutToShow {
            slot: key.clone(),
            item: root.id,
        });
        self.levels = vec![Level {
            menu: root,
            layout,
            selected: None,
        }];
        self.open_slot = Some(key.clone());
        self.title_focus = None;
        self.pending = None;
        out.effects.push(Effect::Opened { slot: key });
        out.effects.push(Effect::Redraw);
    }

    fn close_all(&mut self, out: &mut Outcome) {
        let had = self.is_open() || self.mode != Mode::Idle;
        self.levels.clear();
        self.open_slot = None;
        self.pending = None;
        self.title_focus = None;
        self.mode = Mode::Idle;
        self.keyboard_origin = false;
        if had {
            out.effects.push(Effect::Closed);
            out.effects.push(Effect::Redraw);
        }
    }

    /// Open the submenu of the selected row in `level`.
    fn open_submenu(&mut self, level: usize, select_first: bool, out: &mut Outcome) {
        let Some(slot) = self.open_slot.clone() else {
            return;
        };
        let (child, row_rect, parent_rect) = {
            let l = &self.levels[level];
            let Some(sel) = l.selected else { return };
            let row = &l.layout.rows[sel];
            if !row.item.has_submenu {
                return;
            }
            let Some(child) = l.menu.find(row.item.id).cloned() else {
                return;
            };
            (child, row.rect, l.layout.rect)
        };
        self.levels.truncate(level + 1);
        let layout = layout_popup(
            &child,
            Anchor::BesideRow {
                row: row_rect,
                parent: parent_rect,
            },
            self.output,
            0.0,
            &self.menu_metrics,
            self.measure.as_ref(),
        );
        let selected = if select_first {
            layout.rows.iter().position(|r| r.item.is_selectable())
        } else {
            None
        };
        out.effects.push(Effect::AboutToShow {
            slot,
            item: child.id,
        });
        self.levels.push(Level {
            menu: child,
            layout,
            selected,
        });
        self.pending = None;
        out.effects.push(Effect::Redraw);
    }

    fn select(&mut self, level: usize, row: Option<usize>, out: &mut Outcome) {
        if self.levels[level].selected != row {
            self.levels[level].selected = row;
            out.effects.push(Effect::Redraw);
        }
    }

    // ---- pointer -------------------------------------------------------------------------

    fn in_safe_triangle(&self, level: usize, from: Point, to: Point) -> bool {
        if !self.cfg.safe_triangle || level + 1 >= self.levels.len() {
            return false;
        }
        let parent = &self.levels[level].layout;
        let child = &self.levels[level + 1].layout;
        if !parent.rect.contains(from) {
            return false;
        }
        let edge_x = if child.placed_left {
            child.rect.right()
        } else {
            child.rect.x
        };
        let (a, b) = (
            Point::new(edge_x, child.rect.y),
            Point::new(edge_x, child.rect.bottom()),
        );
        point_in_triangle(to, from, a, b)
    }

    pub fn pointer_move(&mut self, pos: Point, now: f64) -> Outcome {
        let prev = self.last_pointer.replace(pos);
        let mut out = Outcome::default();
        if !self.is_open() {
            return out;
        }
        out.consumed = true;
        if self.mode == Mode::Keyboard {
            self.mode = Mode::Sticky;
        }
        match self.hit(pos) {
            Hit::Popup { level, row } => self.hover_row(level, row, prev, pos, now, &mut out),
            Hit::PopupPadding { level } => {
                if !prev.is_some_and(|p| self.in_safe_triangle(level, p, pos)) {
                    self.select(level, None, &mut out);
                }
            }
            Hit::Slot => {
                let key = self.hit_slot.clone().expect("slot hit");
                if self.cfg.hover_switch
                    && self.open_slot.as_ref() != Some(&key)
                    && self.slot_has_menu(&key)
                {
                    self.open_title(key, &mut out);
                }
            }
            Hit::Bar | Hit::Outside => {
                let deepest = self.levels.len() - 1;
                let keeps_child = self.pending.is_some();
                if !keeps_child
                    && !prev
                        .is_some_and(|p| deepest > 0 && self.in_safe_triangle(deepest - 1, p, pos))
                {
                    self.select(deepest, None, &mut out);
                }
            }
        }
        out
    }

    fn hover_row(
        &mut self,
        level: usize,
        row: usize,
        prev: Option<Point>,
        pos: Point,
        now: f64,
        out: &mut Outcome,
    ) {
        if prev.is_some_and(|p| self.in_safe_triangle(level, p, pos)) {
            return;
        }
        let item = self.levels[level].layout.rows[row].item.clone();
        let target = item.is_selectable().then_some(row);
        let changed = self.levels[level].selected != target;
        if changed {
            self.select(level, target, out);
        }
        let has_child = self.levels.len() > level + 1;
        if let Some(it) = target.map(|_| &item).filter(|it| it.has_submenu) {
            let open_already = has_child && self.levels[level + 1].menu.id == it.id;
            if !open_already {
                self.pending = Some(Pending {
                    level,
                    open: Some(it.id),
                    due: now + self.cfg.submenu_open_delay,
                });
            } else {
                self.pending = None;
            }
        } else if has_child && (changed || (target.is_none() && self.pending.is_none())) {
            self.pending = Some(Pending {
                level,
                open: None,
                due: now + self.cfg.submenu_close_delay,
            });
        }
    }

    pub fn pointer_press(&mut self, pos: Point, _now: f64) -> Outcome {
        self.last_pointer = Some(pos);
        let mut out = Outcome::default();
        let hit = self.hit(pos);
        if !self.is_open() {
            match hit {
                Hit::Slot => {
                    let key = self.hit_slot.clone().expect("slot hit");
                    out.consumed = true;
                    if self.slot_has_menu(&key) {
                        self.open_title(key, &mut out);
                        self.mode = Mode::Tracking;
                        self.keyboard_origin = false;
                    } else {
                        self.pressed_button = Some(key);
                    }
                }
                Hit::Bar => out.consumed = true,
                _ => {
                    if self.mode == Mode::TitleFocus {
                        self.close_all(&mut out);
                    }
                }
            }
            return out;
        }
        out.consumed = true;
        match hit {
            Hit::Slot => {
                let key = self.hit_slot.clone().expect("slot hit");
                if self.open_slot.as_ref() == Some(&key) {
                    self.close_all(&mut out);
                } else if self.slot_has_menu(&key) {
                    self.open_title(key, &mut out);
                    self.mode = Mode::Tracking;
                } else {
                    self.close_all(&mut out);
                }
            }
            Hit::Popup { level, row } => {
                let selectable = self.levels[level].layout.rows[row].item.is_selectable();
                self.levels.truncate(level + 1);
                self.select(level, selectable.then_some(row), &mut out);
                self.mode = Mode::Tracking;
                if selectable && self.levels[level].layout.rows[row].item.has_submenu {
                    self.open_submenu(level, false, &mut out);
                }
            }
            Hit::PopupPadding { .. } => self.mode = Mode::Tracking,
            Hit::Bar | Hit::Outside => {
                self.close_all(&mut out);
                out.consumed = !self.cfg.click_outside_passthrough;
            }
        }
        out
    }

    pub fn pointer_release(&mut self, pos: Point, _now: f64) -> Outcome {
        self.last_pointer = Some(pos);
        let mut out = Outcome::default();
        let hit = self.hit(pos);
        if let Some(button) = self.pressed_button.take() {
            if hit == Hit::Slot && self.hit_slot.as_ref() == Some(&button) {
                out.effects.push(Effect::SlotActivated { slot: button });
            }
            out.consumed = true;
            return out;
        }
        if self.mode != Mode::Tracking {
            return out;
        }
        out.consumed = true;
        match hit {
            Hit::Popup { level, row } => {
                let item = self.levels[level].layout.rows[row].item.clone();
                if item.is_selectable() && !item.has_submenu {
                    let slot = self.open_slot.clone().expect("open slot");
                    out.effects.push(Effect::Activate {
                        slot,
                        item: item.id,
                    });
                    self.close_all(&mut out);
                } else {
                    self.mode = Mode::Sticky;
                }
            }
            Hit::Slot
                if self.cfg.release_on_title_keeps_open && self.hit_slot == self.open_slot =>
            {
                self.mode = Mode::Sticky
            }
            Hit::PopupPadding { .. } => self.mode = Mode::Sticky,
            _ => self.close_all(&mut out),
        }
        out
    }

    /// Advance timers (submenu open / close).
    pub fn tick(&mut self, now: f64) -> Outcome {
        let mut out = Outcome::default();
        if let Some(p) = self.pending {
            if now >= p.due {
                self.pending = None;
                if p.level < self.levels.len() {
                    self.levels.truncate(p.level + 1);
                    out.effects.push(Effect::Redraw);
                    if let Some(id) = p.open {
                        if self.levels[p.level]
                            .selected_item()
                            .is_some_and(|i| i.id == id)
                        {
                            self.open_submenu(p.level, false, &mut out);
                        }
                    }
                }
            }
        }
        out
    }

    // ---- keyboard ------------------------------------------------------------------------

    /// The "focus the menu bar" shortcut: highlight the first title for keyboard navigation (a
    /// second call leaves keyboard focus).
    pub fn focus_bar(&mut self) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        if self.mode == Mode::TitleFocus || self.is_open() {
            self.close_all(&mut out);
            return out;
        }
        if let Some(first) = self.menu_slots().into_iter().next() {
            self.title_focus = Some(first);
            self.mode = Mode::TitleFocus;
            out.effects.push(Effect::Redraw);
        } else {
            out.consumed = false;
        }
        out
    }

    pub fn key(&mut self, key: Key, now: f64) -> Outcome {
        match self.mode {
            Mode::Idle => Outcome::default(),
            Mode::TitleFocus => self.key_title_focus(key),
            _ => self.key_menu(key, now),
        }
    }

    fn step_title(&self, from: &SlotKey, delta: isize) -> Option<SlotKey> {
        let slots = self.menu_slots();
        let i = slots.iter().position(|s| s == from)? as isize;
        let n = slots.len() as isize;
        slots.get((i + delta).rem_euclid(n) as usize).cloned()
    }

    fn key_title_focus(&mut self, key: Key) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        let Some(cur) = self.title_focus.clone() else {
            return out;
        };
        match key {
            Key::Left => self.title_focus = self.step_title(&cur, -1),
            Key::Right => self.title_focus = self.step_title(&cur, 1),
            Key::Home => self.title_focus = self.menu_slots().first().cloned(),
            Key::End => self.title_focus = self.menu_slots().last().cloned(),
            Key::Down | Key::Enter | Key::Space | Key::Up => {
                self.open_title(cur, &mut out);
                self.mode = Mode::Keyboard;
                self.keyboard_origin = true;
                let rows = &self.levels[0].layout.rows;
                let pick = if key == Key::Up {
                    rows.iter().rposition(|r| r.item.is_selectable())
                } else {
                    rows.iter().position(|r| r.item.is_selectable())
                };
                self.levels[0].selected = pick;
                return out;
            }
            Key::Escape => {
                self.close_all(&mut out);
                return out;
            }
            Key::Char(_) => {}
        }
        out.effects.push(Effect::Redraw);
        out
    }

    fn move_selection(&mut self, level: usize, delta: isize, out: &mut Outcome) {
        let rows = &self.levels[level].layout.rows;
        let n = rows.len() as isize;
        if n == 0 {
            return;
        }
        let start = match self.levels[level].selected {
            Some(s) => s as isize,
            None => {
                if delta > 0 {
                    -1
                } else {
                    n
                }
            }
        };
        let mut i = start;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if rows[i as usize].item.is_selectable() {
                self.select(level, Some(i as usize), out);
                self.ensure_visible(level);
                return;
            }
        }
    }

    fn ensure_visible(&mut self, level: usize) {
        let l = &self.levels[level];
        let Some(sel) = l.selected else { return };
        if !l.layout.clipped {
            return;
        }
        let row = l.layout.rows[sel].rect;
        let top = l.layout.rect.y + self.menu_metrics.padding_y;
        let bottom = l.layout.rect.bottom() - self.menu_metrics.padding_y;
        let mut scroll = l.layout.scroll;
        if row.y < top {
            scroll -= top - row.y;
        } else if row.bottom() > bottom {
            scroll += row.bottom() - bottom;
        } else {
            return;
        }
        let menu = l.menu.clone();
        let anchor = self.level_anchor(level);
        let layout = layout_popup(
            &menu,
            anchor,
            self.output,
            scroll,
            &self.menu_metrics,
            self.measure.as_ref(),
        );
        self.levels[level].layout = layout;
    }

    fn level_anchor(&self, level: usize) -> Anchor {
        if level == 0 {
            let key = self.open_slot.clone().unwrap_or(SlotKey::AppMenu);
            Anchor::BelowSlot(self.layout.slot(&key).map(|s| s.rect).unwrap_or_default())
        } else {
            let p = &self.levels[level - 1];
            let row = p
                .selected
                .map(|s| p.layout.rows[s].rect)
                .unwrap_or_default();
            Anchor::BesideRow {
                row,
                parent: p.layout.rect,
            }
        }
    }

    fn switch_title(&mut self, delta: isize, out: &mut Outcome) {
        let Some(cur) = self.open_slot.clone() else {
            return;
        };
        if let Some(next) = self.step_title(&cur, delta) {
            self.open_title(next, out);
            let first = self.levels[0]
                .layout
                .rows
                .iter()
                .position(|r| r.item.is_selectable());
            self.levels[0].selected = first;
        }
    }

    fn key_menu(&mut self, key: Key, now: f64) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        if !self.is_open() {
            return out;
        }
        if self.mode != Mode::Tracking {
            self.mode = Mode::Keyboard;
        }
        let level = self.levels.len() - 1;
        match key {
            Key::Down => self.move_selection(level, 1, &mut out),
            Key::Up => self.move_selection(level, -1, &mut out),
            Key::Home => {
                let first = self.levels[level]
                    .layout
                    .rows
                    .iter()
                    .position(|r| r.item.is_selectable());
                self.select(level, first, &mut out);
            }
            Key::End => {
                let last = self.levels[level]
                    .layout
                    .rows
                    .iter()
                    .rposition(|r| r.item.is_selectable());
                self.select(level, last, &mut out);
            }
            Key::Right => {
                if self.levels[level]
                    .selected_item()
                    .is_some_and(|i| i.has_submenu)
                {
                    self.open_submenu(level, true, &mut out);
                } else {
                    self.switch_title(1, &mut out);
                }
            }
            Key::Left => {
                if level > 0 {
                    self.levels.truncate(level);
                    out.effects.push(Effect::Redraw);
                } else {
                    self.switch_title(-1, &mut out);
                }
            }
            Key::Enter | Key::Space => {
                if let Some(item) = self.levels[level].selected_item().cloned() {
                    if item.has_submenu {
                        self.open_submenu(level, true, &mut out);
                    } else {
                        let slot = self.open_slot.clone().expect("open slot");
                        out.effects.push(Effect::Activate {
                            slot,
                            item: item.id,
                        });
                        self.close_all(&mut out);
                    }
                }
            }
            Key::Escape => {
                if level > 0 {
                    self.levels.truncate(level);
                    out.effects.push(Effect::Redraw);
                } else if self.keyboard_origin {
                    let slot = self.open_slot.clone();
                    self.close_all(&mut out);
                    self.title_focus = slot;
                    self.mode = Mode::TitleFocus;
                    self.keyboard_origin = true;
                } else {
                    self.close_all(&mut out);
                }
            }
            Key::Char(c) => self.type_ahead(level, c, now, &mut out),
        }
        out
    }

    fn type_ahead(&mut self, level: usize, c: char, now: f64, out: &mut Outcome) {
        if c.is_control() {
            return;
        }
        if now - self.type_ahead_at > self.cfg.type_ahead_timeout {
            self.type_ahead.clear();
        }
        self.type_ahead_at = now;
        self.type_ahead.extend(c.to_lowercase());
        let buf: Vec<char> = self.type_ahead.chars().collect();
        let repeated = buf.len() > 1 && buf.iter().all(|x| *x == buf[0]);
        let (prefix, advance) = if repeated {
            (buf[0].to_string(), true)
        } else {
            (self.type_ahead.clone(), buf.len() == 1)
        };
        let rows = &self.levels[level].layout.rows;
        let n = rows.len();
        if n == 0 {
            return;
        }
        let cur = self.levels[level].selected;
        let start = match (cur, advance) {
            (Some(i), true) => i + 1,
            (Some(i), false) => i,
            (None, _) => 0,
        };
        for k in 0..n {
            let i = (start + k) % n;
            let r = &rows[i];
            if r.item.is_selectable() && r.item.label.to_lowercase().starts_with(&prefix) {
                self.select(level, Some(i), out);
                self.ensure_visible(level);
                return;
            }
        }
    }

    /// Close everything (focus lost, the app changed, the compositor needs the pointer).
    pub fn close(&mut self) -> Outcome {
        let mut out = Outcome::default();
        self.close_all(&mut out);
        out
    }
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Whether `p` lies inside (or on the edge of) the triangle `(t0, t1, t2)`.
pub fn point_in_triangle(p: Point, t0: Point, t1: Point, t2: Point) -> bool {
    let d1 = cross(p, t0, t1);
    let d2 = cross(p, t1, t2);
    let d3 = cross(p, t2, t0);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}
