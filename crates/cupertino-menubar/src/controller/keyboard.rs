//! The controller's keyboard: the focus-bar shortcut, the keys of the titles and of open menus, type-ahead, and the
//! Help menu's search field.

use super::*;

impl MenuController {
    // ---- keyboard ------------------------------------------------------------------------

    /// The "focus the menu bar" shortcut: highlight the first title for keyboard navigation (a
    /// second call leaves keyboard focus).
    pub(super) fn bar_focused(&mut self) -> Outcome {
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

    pub(super) fn keyed(&mut self, key: Key, now: f64) -> Outcome {
        if self.flash.is_some() {
            return Outcome {
                consumed: true,
                ..Default::default()
            };
        }
        match self.mode {
            Mode::Idle => Outcome::default(),
            Mode::TitleFocus => self.key_title_focus(key),
            _ => self.key_menu(key, now),
        }
    }

    pub(super) fn step_title(&self, from: &SlotKey, delta: isize) -> Option<SlotKey> {
        let slots = self.menu_slots();
        let i = slots.iter().position(|s| s == from)? as isize;
        let n = slots.len() as isize;
        slots.get((i + delta).rem_euclid(n) as usize).cloned()
    }

    pub(super) fn key_title_focus(&mut self, key: Key) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        let Some(cur) = self.title_focus.clone() else {
            return out;
        };
        self.help_awaits_keyboard = false;
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

    pub(super) fn move_selection(&mut self, level: usize, delta: isize, out: &mut Outcome) {
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

    pub(super) fn ensure_visible(&mut self, level: usize) {
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
        let layout = self.popup(&menu, anchor, scroll);
        self.levels[level].layout = layout;
    }

    pub(super) fn level_anchor(&self, level: usize) -> Anchor {
        if level == 0 {
            let key = self.open_slot.clone().unwrap_or(SlotKey::AppMenu);
            Anchor::BelowSlot(
                self.layout
                    .slot(&key)
                    .map(|s| self.menu_anchor(s))
                    .unwrap_or_default(),
            )
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

    pub(super) fn switch_title(&mut self, delta: isize, out: &mut Outcome) {
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

    pub(super) fn key_menu(&mut self, key: Key, now: f64) -> Outcome {
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
                        self.choose(level, item.id, now, &mut out);
                    }
                }
            }
            Key::Escape => {
                if level > 0 {
                    self.levels.truncate(level);
                    out.effects.push(Effect::Redraw);
                } else if self.searching() && !self.search.is_empty() {
                    self.search.set_text("");
                    self.searched(&mut out);
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

    pub(super) fn type_ahead(&mut self, level: usize, c: char, now: f64, out: &mut Outcome) {
        if c.is_control() {
            return;
        }
        let Some(last) = self.levels[level].layout.rows.len().checked_sub(1) else {
            return;
        };
        let menu = self.levels[level].menu.id;
        if self.typed_in.replace(menu) != Some(menu) {
            self.typing = TypeAhead::default();
        }
        let rows = &self.levels[level].layout.rows;
        let titles: Vec<Option<&str>> = rows
            .iter()
            .map(|r| r.item.is_selectable().then_some(r.item.label.as_str()))
            .collect();
        // with nothing selected the search starts at the first row
        let from = self.levels[level].selected.unwrap_or(last);
        if let Some(i) = self.typing.find(c, now, from, &titles) {
            self.select(level, Some(i), out);
            self.ensure_visible(level);
        }
    }

    // ---- Help search ---------------------------------------------------------------------

    /// The Help search shortcut: open the Help menu with the keyboard in its search field. `has_keyboard` says whether
    /// the bar holds the keyboard already; without it the Help title takes keyboard focus as for the menu-bar shortcut,
    /// which asks for the keyboard, and the menu opens when it arrives ([`MenuController::keyboard_arrived`]). With the
    /// Help menu open already, only its submenus close. Not consumed when the bar shows no Help menu.
    pub(super) fn help_search_focused(&mut self, has_keyboard: bool) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        let help = SlotKey::Menu(HELP_MENU);
        if !self.menu_slots().contains(&help) {
            out.consumed = false;
            return out;
        }
        if self.is_open() || has_keyboard {
            self.open_help_field(&mut out);
        } else {
            self.title_focus = Some(help);
            self.mode = Mode::TitleFocus;
            self.help_awaits_keyboard = true;
            out.effects.push(Effect::Redraw);
        }
        out
    }

    /// The keyboard came to the bar: the Help menu the Help search shortcut asked for opens.
    pub(super) fn arrived(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if std::mem::take(&mut self.help_awaits_keyboard) && self.mode == Mode::TitleFocus {
            out.consumed = true;
            self.open_help_field(&mut out);
        }
        out
    }

    /// Open the Help menu, or keep it open with its submenus closed, driven by the keyboard with its field taking what
    /// is typed. Esc clears the field, then closes the menu.
    pub(super) fn open_help_field(&mut self, out: &mut Outcome) {
        let help = SlotKey::Menu(HELP_MENU);
        if self.open_slot.as_ref() == Some(&help) {
            self.levels.truncate(1);
            out.effects.push(Effect::Redraw);
        } else {
            self.open_title(help, out);
        }
        self.help_awaits_keyboard = false;
        self.keyboard_origin = false;
        if self.mode != Mode::Tracking {
            self.mode = Mode::Keyboard;
        }
    }

    /// The Help menu is open, so its search field shows.
    pub(super) fn searching(&self) -> bool {
        self.is_open() && self.open_slot == Some(SlotKey::Menu(HELP_MENU))
    }

    pub(super) fn clear_search(&mut self) {
        self.search = TextBuffer::new();
        self.hits.clear();
    }

    /// The effect of choosing the row `item` of the open menu. A search result stands for the item it found, by its
    /// position among the results: choosing it is choosing that item in its own menu. It carries the time of the
    /// input that chose it.
    pub(super) fn activation(&self, item: ItemId) -> Option<Effect> {
        let slot = self.open_slot.clone()?;
        let time = self.time();
        match fallback::search_result_row(item).filter(|_| self.searching()) {
            Some(row) => self.hits.get(row).map(|hit| Effect::Activate {
                slot: hit.slot.clone(),
                item: hit.item,
                time,
            }),
            None => Some(Effect::Activate { slot, item, time }),
        }
    }

    /// The field's text changed: search again and select the first result.
    pub(super) fn searched(&mut self, out: &mut Outcome) {
        self.hits = search::search(&self.content, self.search.text());
        self.rebuild_levels();
        if let Some(help) = self.levels.first_mut() {
            help.selected = help.layout.index_of(fallback::search_result(0));
        }
        out.effects.push(Effect::Redraw);
    }

    /// A key press while the Help menu shows its search field, with the menu key it maps to (`None` for the typing
    /// keys). The field takes the keys that edit text: the arrows, Home and End only while it holds text, Space
    /// always; Up, Down, Return and Esc stay the menu's ([`MenuController::key`]). `None` when the field does not take
    /// the key. The bar has no clipboard, so copy, cut and paste only change the text where they edit it.
    pub(super) fn search_keyed(
        &mut self,
        menu_key: Option<Key>,
        keymap: &mut Keymap,
        input: KeyInput,
        at: Instant,
    ) -> Option<Outcome> {
        if !self.searching() || self.levels.len() > 1 {
            return None;
        }
        let takes = match menu_key {
            None | Some(Key::Space | Key::Char(_)) => true,
            Some(Key::Left | Key::Right | Key::Home | Key::End) => !self.search.is_empty(),
            Some(Key::Up | Key::Down | Key::Enter | Key::Escape) => false,
        };
        if !takes {
            return None;
        }
        let response = self.search.key(keymap, input, at);
        if response == Response::Ignored {
            return None;
        }
        let mut out = Outcome {
            consumed: true,
            ..Default::default()
        };
        if self.mode != Mode::Tracking {
            self.mode = Mode::Keyboard;
        }
        match response {
            Response::Edited | Response::Cut(_) => self.searched(&mut out),
            _ => out.effects.push(Effect::Redraw),
        }
        Some(out)
    }
}
