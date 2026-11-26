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
//! design. Releasing a press, the timings and the safe triangle are the ones every menu shares
//! ([`cupertino_menu_model::tracking`]); see [`ControllerConfig`].

use crate::fallback::{self, HELP_MENU};
use crate::geom::{Point, Rect};
use crate::item::{ItemId, MenuItem};
use crate::layout::{
    layout_bar, layout_popup, Anchor, BarContent, BarLayout, BarMetrics, MenuMetrics, PopupLayout,
    Slot, SlotKey, TextMeasure, SYSTEM_MARK_PT,
};
use crate::search::{self, Match};
use cupertino_kit::motion::Timing;
use cupertino_kit::text::{KeyInput, Keymap, Response, TextBuffer, TextFieldKeys};
use cupertino_kit::typeahead::TypeAhead;
use cupertino_menu_model::tracking::{
    Edge, Release, SubmenuAim, Tracking, Under, SUBMENU_OPEN_DELAY,
};
use std::time::Instant;

/// How long the highlight of a chosen row stays off, then on, before the choice is made: the catalogue's motions.
const FLASH_OFF: Timing = Timing::MENU_CHOICE_OFF;
const FLASH_ON: Timing = Timing::MENU_CHOICE_ON;

/// Identifier of the synthetic root item of the app menu.
pub const APP_MENU_ROOT: ItemId = ItemId(i32::MIN);
/// The root of the system menu.
pub const SYSTEM_MENU_ROOT: ItemId = ItemId(i32::MIN + 1);

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
    /// The bar's first menu opened (the bar holds a menu open from now on).
    Opened {
        slot: SlotKey,
    },
    /// All menus closed.
    Closed,
    /// The menu of `item` (a title's root, or a submenu) shows from now on: DBusMenu `opened`. Every menu that shows
    /// gets one, the shallowest first, and a [`Effect::MenuClosed`] when it no longer shows, however it closed; a menu
    /// whose item the exporter renumbered while it showed keeps showing under the new id and gets neither.
    MenuOpened {
        slot: SlotKey,
        item: ItemId,
        /// When, in milliseconds of the input clock ([`MenuController::set_input_time`]).
        time: u32,
    },
    /// The menu of `item` no longer shows: DBusMenu `closed`, the deepest first. The item may be gone from the content.
    MenuClosed {
        slot: SlotKey,
        item: ItemId,
        time: u32,
    },
    /// The user chose an item, at `time` (milliseconds of the input clock of the input that chose it).
    Activate {
        slot: SlotKey,
        item: ItemId,
        time: u32,
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
    /// Hover time on a submenu row before its popup opens.
    pub submenu_open_delay: f64,
    /// Keep a submenu open while the pointer heads into it ([`SubmenuAim`]).
    pub safe_triangle: bool,
    /// Releasing the button over the open title within the tracking interval of the press leaves the menu open
    /// (click-sticky); a release there after a longer hold, or a drag out and back, closes it.
    pub release_on_title_keeps_open: bool,
    /// A click outside the menus closes them and is also delivered to the client below.
    pub click_outside_passthrough: bool,
    /// Hovering another title while a menu is open switches to it.
    pub hover_switch: bool,
    /// A chosen row flashes ([`Timing::MENU_CHOICE_OFF`], [`Timing::MENU_CHOICE_ON`]) before the choice is made; off, it is made at once.
    pub flash_choice: bool,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            submenu_open_delay: SUBMENU_OPEN_DELAY,
            safe_triangle: true,
            release_on_title_keeps_open: true,
            click_outside_passthrough: false,
            hover_switch: true,
            flash_choice: true,
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

/// A chosen row flashing before the choice is made.
#[derive(Clone, Debug)]
struct Flash {
    level: usize,
    /// The row's index in its level's layout.
    row: usize,
    /// What choosing it does.
    activation: Option<Effect>,
    start: f64,
}

impl Flash {
    fn lights(&self) -> f64 {
        self.start + FLASH_OFF.duration()
    }

    fn ends(&self) -> f64 {
        self.start + FLASH_OFF.duration() + FLASH_ON.duration()
    }
}

/// The submenu `open` of the selected row of `level` opens at `due`, unless the pointer moves on first.
#[derive(Clone, Copy, Debug)]
struct Pending {
    level: usize,
    open: ItemId,
    due: f64,
}

/// What an open popup showed, to find it again in new content: its menu's id and label, its selected row's id, index
/// and label, and how far it was scrolled.
struct Shown {
    menu: (ItemId, String),
    selected: Option<(ItemId, usize, String)>,
    scroll: f64,
}

/// Where the item that was at `index` of a list, with `id` and `label`, is among `items` (id and label) now: under the
/// same id and label; else, for an exporter that rebuilt its tree under new ids (Chromium's renumbers every item when
/// a menu is about to show) or reused its ids for other items, at the same place under the same label, then under the
/// label elsewhere; else under the same id, an item that was renamed. `None` when it is gone.
fn rematch<'a>(
    items: impl Iterator<Item = (ItemId, &'a str)> + Clone,
    id: ItemId,
    index: usize,
    label: &str,
) -> Option<usize> {
    let items = items.enumerate();
    let by = |found: &dyn Fn(usize, ItemId, &str) -> bool| {
        items
            .clone()
            .find(|(i, (item, text))| found(*i, *item, text))
            .map(|(i, _)| i)
    };
    let labelled = !label.is_empty();
    by(&|_, item, text| item == id && text == label)
        .or_else(|| by(&|i, _, text| labelled && i == index && text == label))
        .or_else(|| by(&|_, _, text| labelled && text == label))
        .or_else(|| by(&|_, item, _| item == id))
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
    typing: TypeAhead,
    /// The menu the typed text searches; another menu starts a new search.
    typed_in: Option<ItemId>,
    /// The press held since it opened a menu or pressed in one.
    tracking: Option<Tracking>,
    /// Whether the pointer heads from a row into the submenu it opened.
    aim: SubmenuAim,
    pending: Option<Pending>,
    keyboard_origin: bool,
    /// The Help menu's search field and what its text finds.
    search: TextBuffer,
    hits: Vec<Match>,
    /// The Help search shortcut focused the Help title and asked for the keyboard: the Help menu opens with its field
    /// focused once the keyboard is the bar's ([`MenuController::keyboard_arrived`]).
    help_awaits_keyboard: bool,
    /// Alt is held: the open menus show the alternates of their items.
    alt: bool,
    /// The row chosen and flashing, until its choice is made and the menus close.
    flash: Option<Flash>,
    /// The menus the source was told show ([`Effect::MenuOpened`]), the bar's first menu first.
    told: Vec<(SlotKey, ItemId)>,
    /// The latest input's time in milliseconds of the input clock, and the controller's time it came at.
    input: (u32, f64),
    /// The controller's latest time, in seconds.
    clock: f64,
}

mod keyboard;
mod shown;

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
            typing: TypeAhead::default(),
            typed_in: None,
            tracking: None,
            aim: SubmenuAim::default(),
            pending: None,
            keyboard_origin: false,
            search: TextBuffer::new(),
            hits: Vec::new(),
            help_awaits_keyboard: false,
            alt: false,
            flash: None,
            told: Vec::new(),
            input: (0, 0.0),
            clock: 0.0,
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

    /// The Help menu's search field while the Help menu is open.
    pub fn search_field(&self) -> Option<&TextBuffer> {
        self.searching().then_some(&self.search)
    }

    /// Time of the next pending timer, if any.
    pub fn next_deadline(&self) -> Option<f64> {
        let flash = self.flash.as_ref().map(|f| {
            if self
                .levels
                .get(f.level)
                .is_some_and(|l| l.selected.is_none())
            {
                f.lights()
            } else {
                f.ends()
            }
        });
        [self.pending.map(|p| p.due), self.aim.deadline(), flash]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    // ---- content -------------------------------------------------------------------------

    pub fn set_output(&mut self, output: Rect) {
        self.output = output;
        self.relayout();
        self.rebuild_levels();
    }

    /// Replace the bar content (focused app changed, menus updated, status items changed). Open menus are kept, with the
    /// rows of the new content, when their items still exist: under the same id, or, for an exporter that rebuilt its
    /// tree under new ids, at the same place under the same label ([`rematch`]). A menu whose item is gone closes.
    fn content_set(&mut self, content: BarContent) -> Outcome {
        let old = std::mem::replace(&mut self.content, content);
        self.hits = search::search(&self.content, self.search.text());
        self.relayout();
        self.open_slot = self.open_slot.take().map(|k| self.carried(&old, k));
        self.title_focus = self.title_focus.take().map(|k| self.carried(&old, k));
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

    /// The slot of the new content that stands for `key` of the `old`: an application's menu the exporter gave a new id
    /// has it under that id, and the other slots keep their keys.
    fn carried(&self, old: &BarContent, key: SlotKey) -> SlotKey {
        let SlotKey::Menu(id) = key else {
            return key;
        };
        let Some((index, was)) = old.menus.iter().enumerate().find(|(_, m)| m.id == id) else {
            return key;
        };
        let menus = self.content.menus.iter().map(|m| (m.id, m.label.as_str()));
        match rematch(menus, id, index, &was.label) {
            Some(i) => SlotKey::Menu(self.content.menus[i].id),
            None => key,
        }
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
            SlotKey::System => Some(MenuItem {
                id: SYSTEM_MENU_ROOT,
                has_submenu: true,
                children: self.content.system_menu.clone(),
                label: "System".to_string(),
                enabled: true,
                visible: true,
                ..Default::default()
            }),
            SlotKey::AppMenu => Some(MenuItem {
                id: APP_MENU_ROOT,
                has_submenu: true,
                children: self.content.app_menu.clone(),
                label: self.content.app_name.clone(),
                enabled: true,
                visible: true,
                ..Default::default()
            }),
            SlotKey::Menu(id) => {
                let menu = self.content.menus.iter().find(|m| m.id == *id)?;
                Some(if *id == HELP_MENU {
                    search::help_menu(menu, self.search.text(), &self.hits)
                } else {
                    menu.clone()
                })
            }
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

    /// Alt went down or up: the open menus show their items' alternates while it is held.
    fn alt_set(&mut self, held: bool) -> Outcome {
        let mut out = Outcome::default();
        if self.alt != held {
            self.alt = held;
            if self.is_open() {
                self.rebuild_levels();
                out.effects.push(Effect::Redraw);
            }
        }
        out
    }

    /// What a slot's menu hangs from: the slot's rectangle, moved so the menu's labels line up with the slot's text (a
    /// title, or the system menu's mark).
    fn menu_anchor(&self, slot: &Slot) -> Rect {
        let text_x = match slot.key {
            SlotKey::AppMenu | SlotKey::Menu(_) => slot.rect.x + self.bar_metrics.title_padding,
            SlotKey::System => slot.rect.x + (slot.rect.w - SYSTEM_MARK_PT) / 2.0,
            // an item of the status area's menu or panel lines up with its highlight, which reaches past the slot
            SlotKey::Status(_) | SlotKey::ControlPanel | SlotKey::Clock => {
                let (dx, w) = self.bar_metrics.highlight_span(&slot.key, slot.rect.w);
                return Rect::new(slot.rect.x + dx, slot.rect.y, w, slot.rect.h);
            }
        };
        Rect::new(
            text_x - self.menu_metrics.padding_x,
            slot.rect.y,
            slot.rect.w,
            slot.rect.h,
        )
    }

    /// The popup of `menu`, with its items' alternates in their place while Alt is held.
    fn popup(&self, menu: &MenuItem, anchor: Anchor, scroll: f64) -> PopupLayout {
        let shown;
        let menu = if self.alt {
            shown = menu.alternates_shown();
            &shown
        } else {
            menu
        };
        layout_popup(
            menu,
            anchor,
            self.output,
            scroll,
            &self.menu_metrics,
            self.measure.as_ref(),
        )
    }

    fn rebuild_levels(&mut self) {
        let Some(key) = self.open_slot.clone() else {
            return;
        };
        let Some(root) = self.root_item(&key) else {
            self.levels.clear();
            return;
        };
        let Some(slot_rect) = self.layout.slot(&key).map(|s| self.menu_anchor(s)) else {
            self.levels.clear();
            return;
        };
        let old: Vec<Shown> = self
            .levels
            .iter()
            .map(|l| Shown {
                menu: (l.menu.id, l.menu.label.clone()),
                selected: l.selected.and_then(|row| {
                    let item = &l.layout.rows.get(row)?.item;
                    Some((item.id, row, item.label.clone()))
                }),
                scroll: l.layout.scroll,
            })
            .collect();
        // a submenu waiting to open from the selected row opens from that row in the new content
        let pending = self.pending.filter(|p| {
            old.get(p.level)
                .and_then(|l| l.selected.as_ref())
                .is_some_and(|(id, _, _)| *id == p.open)
        });
        let mut new_levels: Vec<Level> = Vec::new();
        for (i, shown) in old.iter().enumerate() {
            let (menu_id, label) = &shown.menu;
            // the submenu of the row selected in its parent, which is the row it opened from
            let menu = if i == 0 {
                Some(root.clone())
            } else {
                new_levels[i - 1]
                    .selected_item()
                    .and_then(|r| new_levels[i - 1].menu.find(r.id))
                    .filter(|m| m.has_submenu && (m.id == *menu_id || m.label == *label))
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
            let layout = self.popup(&menu, anchor, shown.scroll);
            let selected = shown.selected.as_ref().and_then(|(id, row, label)| {
                let rows = layout
                    .rows
                    .iter()
                    .map(|r| (r.item.id, r.item.label.as_str()));
                // an alternate shown or hidden as Alt changes keeps its row
                rematch(rows, *id, *row, label).or_else(|| layout.index_of(*id))
            });
            new_levels.push(Level {
                menu,
                layout,
                selected,
            });
        }
        self.levels = new_levels;
        if let Some(p) = pending {
            let open = self
                .levels
                .get(p.level)
                .and_then(Level::selected_item)
                .filter(|item| item.has_submenu)
                .map(|item| item.id);
            self.pending = open.map(|open| Pending { open, ..p });
        }
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
        self.clear_search();
        let Some(root) = self.root_item(&key) else {
            return;
        };
        let Some(slot_rect) = self.layout.slot(&key).map(|s| self.menu_anchor(s)) else {
            return;
        };
        let layout = self.popup(&root, Anchor::BelowSlot(slot_rect), 0.0);
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
        self.aim.reset();
        out.effects.push(Effect::Opened { slot: key });
        out.effects.push(Effect::Redraw);
    }

    /// Chooses `item`, the selected row of `level`: its row flashes, then the choice is made and the menus close.
    fn choose(&mut self, level: usize, item: ItemId, now: f64, out: &mut Outcome) {
        let activation = self.activation(item);
        // the switch and the disclosure of a status panel act at once and leave the panel open
        if self
            .levels
            .get(level)
            .and_then(|l| l.menu.find(item))
            .is_some_and(|i| i.panel.as_ref().is_some_and(|p| p.keeps_open()))
        {
            out.effects.extend(activation);
            self.mode = Mode::Sticky;
            out.effects.push(Effect::Redraw);
            return;
        }
        let row = self.levels.get(level).and_then(|l| l.selected);
        let Some(row) = row.filter(|_| self.cfg.flash_choice && self.flash.is_none()) else {
            out.effects.extend(activation);
            self.close_all(out);
            return;
        };
        self.levels[level].selected = None;
        self.flash = Some(Flash {
            level,
            row,
            activation,
            start: now,
        });
        self.pending = None;
        self.tracking = None;
        self.aim.reset();
        self.mode = Mode::Sticky;
        out.effects.push(Effect::Redraw);
    }

    /// The flash's next step: the row lights again, then the choice is made and the menus close.
    fn flash_step(&mut self, now: f64, out: &mut Outcome) {
        let Some(flash) = self.flash.clone() else {
            return;
        };
        if now >= flash.ends() {
            self.close_all(out);
        } else if now >= flash.lights() {
            if let Some(level) = self.levels.get_mut(flash.level) {
                if level.selected.is_none() {
                    level.selected = Some(flash.row);
                    out.effects.push(Effect::Redraw);
                }
            }
        }
    }

    fn close_all(&mut self, out: &mut Outcome) {
        // a choice made is not lost with the menus
        if let Some(flash) = self.flash.take() {
            out.effects.extend(flash.activation);
        }
        let had = self.is_open() || self.mode != Mode::Idle;
        self.levels.clear();
        self.open_slot = None;
        self.pending = None;
        self.tracking = None;
        self.aim.reset();
        self.title_focus = None;
        self.mode = Mode::Idle;
        self.keyboard_origin = false;
        self.help_awaits_keyboard = false;
        self.clear_search();
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
        let layout = self.popup(
            &child,
            Anchor::BesideRow {
                row: row_rect,
                parent: parent_rect,
            },
            0.0,
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

    /// The near edge of the submenu open from the menu at `level`, where the pointer comes into it.
    fn submenu_edge(&self, level: usize) -> Option<Edge> {
        if !self.cfg.safe_triangle || level + 1 >= self.levels.len() {
            return None;
        }
        let child = &self.levels[level + 1].layout;
        let x = if child.placed_left {
            child.rect.right()
        } else {
            child.rect.x
        };
        Some(Edge {
            x,
            top: child.rect.y,
            bottom: child.rect.bottom(),
        })
    }

    fn moved(&mut self, pos: Point, now: f64) -> Outcome {
        self.last_pointer = Some(pos);
        let mut out = Outcome::default();
        if self.flash.is_some() {
            out.consumed = true;
            return out;
        }
        if !self.is_open() {
            return out;
        }
        out.consumed = true;
        if self.mode == Mode::Keyboard {
            self.mode = Mode::Sticky;
        }
        let hit = self.hit(pos);
        // the menu under the pointer, or off the menus the parent of the deepest, whose submenu it may head into
        let aimed = match hit {
            Hit::Popup { level, .. } | Hit::PopupPadding { level } => Some(level),
            Hit::Bar | Hit::Outside => self.levels.len().checked_sub(2),
            Hit::Slot => None,
        };
        let edge = aimed.and_then(|level| self.submenu_edge(level));
        let heading = self.aim.moved((pos.x, pos.y), now, edge);
        match hit {
            Hit::Popup { level, row } if !heading => self.hover_row(level, row, now, &mut out),
            Hit::PopupPadding { level } if !heading => self.select(level, None, &mut out),
            Hit::Slot => {
                let key = self.hit_slot.clone().expect("slot hit");
                if self.cfg.hover_switch
                    && self.open_slot.as_ref() != Some(&key)
                    && self.slot_has_menu(&key)
                {
                    self.open_title(key, &mut out);
                }
            }
            Hit::Bar | Hit::Outside if !heading && self.pending.is_none() => {
                let deepest = self.levels.len() - 1;
                self.select(deepest, None, &mut out);
            }
            _ => {}
        }
        out
    }

    /// The pointer stopped heading into a submenu: the row under it takes over.
    fn pointer_rested(&mut self, now: f64, out: &mut Outcome) {
        let Some(pos) = self.last_pointer else {
            return;
        };
        match self.hit(pos) {
            Hit::Popup { level, row } => self.hover_row(level, row, now, out),
            Hit::PopupPadding { level } => self.select(level, None, out),
            _ => {}
        }
    }

    fn hover_row(&mut self, level: usize, row: usize, now: f64, out: &mut Outcome) {
        let item = self.levels[level].layout.rows[row].item.clone();
        let target = item.is_selectable().then_some(row);
        if let Some(tracking) = self.tracking.as_mut().filter(|_| target.is_some()) {
            tracking.reach();
        }
        let changed = self.levels[level].selected != target;
        if changed {
            self.select(level, target, out);
        }
        let opens = target.is_some() && item.has_submenu;
        let open_already = self
            .levels
            .get(level + 1)
            .is_some_and(|child| opens && child.menu.id == item.id);
        if self.levels.len() > level + 1 && !open_already && (changed || target.is_none()) {
            // the pointer came onto another row, not on its way into the open submenu: it closes now
            self.levels.truncate(level + 1);
            out.effects.push(Effect::Redraw);
        }
        if opens && !open_already {
            self.pending = Some(Pending {
                level,
                open: item.id,
                due: now + self.cfg.submenu_open_delay,
            });
        } else if changed || open_already {
            self.pending = None;
        }
    }

    fn pressed(&mut self, pos: Point, now: f64) -> Outcome {
        self.last_pointer = Some(pos);
        let mut out = Outcome::default();
        if self.flash.is_some() {
            out.consumed = true;
            return out;
        }
        let hit = self.hit(pos);
        if !self.is_open() {
            match hit {
                Hit::Slot => {
                    let key = self.hit_slot.clone().expect("slot hit");
                    out.consumed = true;
                    if self.slot_has_menu(&key) {
                        self.open_title(key, &mut out);
                        self.tracking = Some(Tracking::from_title(now));
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
                    self.tracking = Some(Tracking::from_title(now));
                    self.mode = Mode::Tracking;
                } else {
                    self.close_all(&mut out);
                }
            }
            Hit::Popup { level, row } => {
                let selectable = self.levels[level].layout.rows[row].item.is_selectable();
                self.tracking = Some(Tracking::in_menu(now));
                self.levels.truncate(level + 1);
                self.select(level, selectable.then_some(row), &mut out);
                self.mode = Mode::Tracking;
                if selectable && self.levels[level].layout.rows[row].item.has_submenu {
                    self.open_submenu(level, false, &mut out);
                }
            }
            Hit::PopupPadding { .. } => {
                self.tracking = Some(Tracking::in_menu(now));
                self.mode = Mode::Tracking;
            }
            Hit::Bar | Hit::Outside => {
                self.close_all(&mut out);
                out.consumed = !self.cfg.click_outside_passthrough;
            }
        }
        out
    }

    /// A press opened a menu from its title or pressed in an open one: what its release does is
    /// [`Tracking::release`]. A drag onto a row chooses it, a quick click on the title leaves the menu open.
    fn released(&mut self, pos: Point, now: f64) -> Outcome {
        self.last_pointer = Some(pos);
        let mut out = Outcome::default();
        if self.flash.is_some() {
            out.consumed = true;
            return out;
        }
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
        let tracking = self.tracking.take().unwrap_or(Tracking::in_menu(now));
        let (under, item) = match hit {
            Hit::Popup { level, row } => {
                let item = &self.levels[level].layout.rows[row].item;
                if item.is_selectable() && !item.has_submenu {
                    (Under::Item, Some(item.id))
                } else {
                    (Under::Menu, None)
                }
            }
            Hit::PopupPadding { .. } => (Under::Menu, None),
            Hit::Slot if self.hit_slot == self.open_slot => (Under::Title, None),
            _ => (Under::Outside, None),
        };
        let release = match tracking.release(now, under) {
            Release::Stay if under == Under::Title && !self.cfg.release_on_title_keeps_open => {
                Release::Close
            }
            release => release,
        };
        match (release, item) {
            (Release::Choose, Some(item)) => {
                let level = match hit {
                    Hit::Popup { level, .. } => level,
                    _ => self.levels.len() - 1,
                };
                self.choose(level, item, now, &mut out);
            }
            (Release::Close, _) => self.close_all(&mut out),
            _ => self.mode = Mode::Sticky,
        }
        out
    }

    /// Advance timers (a submenu opening, the end of the way into a submenu).
    fn ticked(&mut self, now: f64) -> Outcome {
        let mut out = Outcome::default();
        if self.flash.is_some() {
            self.flash_step(now, &mut out);
            return out;
        }
        if self.aim.expired(now) {
            self.pointer_rested(now, &mut out);
        }
        if let Some(p) = self.pending {
            if now >= p.due {
                self.pending = None;
                if self
                    .levels
                    .get(p.level)
                    .and_then(|l| l.selected_item())
                    .is_some_and(|i| i.id == p.open)
                {
                    self.open_submenu(p.level, false, &mut out);
                }
            }
        }
        out
    }

    /// Close the menus when `slot`'s menu is the open one (its content went away); `None` when another menu, or none, is
    /// open.
    pub fn close_slot(&mut self, slot: &SlotKey) -> Option<Outcome> {
        (self.open_slot.as_ref() == Some(slot)).then(|| self.close())
    }

    /// Close everything (focus lost, the app changed, the compositor needs the pointer).
    pub fn close(&mut self) -> Outcome {
        let mut out = Outcome::default();
        self.close_all(&mut out);
        self.synced(out)
    }
}

/// Whether `p` lies inside (or on the edge of) the triangle `(t0, t1, t2)`.
pub fn point_in_triangle(p: Point, t0: Point, t1: Point, t2: Point) -> bool {
    cupertino_menu_model::tracking::point_in_triangle(
        (p.x, p.y),
        (t0.x, t0.y),
        (t1.x, t1.y),
        (t2.x, t2.y),
    )
}
