//! What the source is told of the menus that show: every public call ends by comparing the menus open with those the
//! source was told of, and reports the difference as [`Effect::MenuClosed`] (the deepest first) and
//! [`Effect::MenuOpened`] (the shallowest first), whatever closed or opened them: a key, a click outside, another
//! title, a choice, the item gone from the content. The effects carry the time of the input clock, advanced from the
//! latest input by the controller's own time. A source's request to open a menu is [`MenuController::open_item`].

use super::*;

impl MenuController {
    /// The input about to be handled happened at `millis`, milliseconds of the window system's input clock, at the
    /// controller's time `now` (seconds). The effects that carry a time are stamped from it.
    pub fn set_input_time(&mut self, millis: u32, now: f64) {
        self.input = (millis, now);
        self.clock = now;
    }

    /// The input clock's time now: the latest input's, advanced by the controller's time since it came (wrapping).
    pub(super) fn time(&self) -> u32 {
        let (millis, at) = self.input;
        let since = ((self.clock - at).max(0.0) * 1000.0).round() as u32;
        millis.wrapping_add(since)
    }

    fn at(&mut self, now: f64) {
        self.clock = now;
    }

    /// `out` with the opening and closing of the menus since the source was last told.
    pub(super) fn synced(&mut self, mut out: Outcome) -> Outcome {
        let showing: Vec<(SlotKey, ItemId)> = match &self.open_slot {
            Some(slot) => self
                .levels
                .iter()
                .map(|l| (slot.clone(), l.menu.id))
                .collect(),
            None => Vec::new(),
        };
        let kept = self
            .told
            .iter()
            .zip(&showing)
            .take_while(|(told, now)| told == now)
            .count();
        let time = self.time();
        for (slot, item) in self.told.drain(kept..).rev() {
            out.effects.push(Effect::MenuClosed { slot, item, time });
        }
        for (slot, item) in showing[kept..].iter().cloned() {
            out.effects.push(Effect::MenuOpened { slot, item, time });
        }
        self.told = showing;
        out
    }

    /// Replace the bar content (focused app changed, menus updated, status items changed). Open menus are kept, with the
    /// rows of the new content, when their items still exist: under the same id, or, for an exporter that rebuilt its
    /// tree under new ids, at the same place under the same label ([`rematch`]); such a menu still shows and the source
    /// is not told again. A menu whose item is gone closes.
    pub fn set_content(&mut self, content: BarContent) -> Outcome {
        let out = self.content_set(content);
        // the levels kept are the first of those shown before, found again in the new content
        if let Some(slot) = &self.open_slot {
            for (told, level) in self.told.iter_mut().zip(&self.levels) {
                *told = (slot.clone(), level.menu.id);
            }
        }
        self.synced(out)
    }

    /// Alt went down or up: the open menus show their items' alternates while it is held.
    pub fn set_alt(&mut self, held: bool) -> Outcome {
        let out = self.alt_set(held);
        self.synced(out)
    }

    pub fn pointer_move(&mut self, pos: Point, now: f64) -> Outcome {
        self.at(now);
        let out = self.moved(pos, now);
        self.synced(out)
    }

    pub fn pointer_press(&mut self, pos: Point, now: f64) -> Outcome {
        self.at(now);
        let out = self.pressed(pos, now);
        self.synced(out)
    }

    /// A press opened a menu from its title or pressed in an open one: what its release does is
    /// [`Tracking::release`]. A drag onto a row chooses it, a quick click on the title leaves the menu open.
    pub fn pointer_release(&mut self, pos: Point, now: f64) -> Outcome {
        self.at(now);
        let out = self.released(pos, now);
        self.synced(out)
    }

    /// Advance timers (a submenu opening, the end of the way into a submenu).
    pub fn tick(&mut self, now: f64) -> Outcome {
        self.at(now);
        let out = self.ticked(now);
        self.synced(out)
    }

    /// The "focus the menu bar" shortcut: highlight the first title for keyboard navigation (a
    /// second call leaves keyboard focus).
    pub fn focus_bar(&mut self) -> Outcome {
        let out = self.bar_focused();
        self.synced(out)
    }

    pub fn key(&mut self, key: Key, now: f64) -> Outcome {
        self.at(now);
        let out = self.keyed(key, now);
        self.synced(out)
    }

    /// The Help search shortcut: open the Help menu with the keyboard in its search field. `has_keyboard` says whether
    /// the bar holds the keyboard already; without it the Help title takes keyboard focus as for the menu-bar shortcut,
    /// which asks for the keyboard, and the menu opens when it arrives ([`MenuController::keyboard_arrived`]). With the
    /// Help menu open already, only its submenus close. Not consumed when the bar shows no Help menu.
    pub fn focus_help_search(&mut self, has_keyboard: bool) -> Outcome {
        let out = self.help_search_focused(has_keyboard);
        self.synced(out)
    }

    /// The keyboard came to the bar: the Help menu the Help search shortcut asked for opens.
    pub fn keyboard_arrived(&mut self) -> Outcome {
        let out = self.arrived();
        self.synced(out)
    }

    /// A key press while the Help menu shows its search field, with the menu key it maps to (`None` for the typing
    /// keys). The field takes the keys that edit text: the arrows, Home and End only while it holds text, Space
    /// always; Up, Down, Return and Esc stay the menu's ([`MenuController::key`]). `None` when the field does not take
    /// the key. The bar has no clipboard, so copy, cut and paste only change the text where they edit it.
    pub fn search_key(
        &mut self,
        menu_key: Option<Key>,
        keymap: &mut Keymap,
        input: KeyInput,
        at: Instant,
    ) -> Option<Outcome> {
        let out = self.search_keyed(menu_key, keymap, input, at)?;
        Some(self.synced(out))
    }

    /// The source asks for the menu of `item` to be opened (DBusMenu `ItemActivationRequested`): the title's menu that
    /// holds it opens as if clicked, with the submenus on the way to it open and their rows selected; a submenu's own
    /// menu opens too, an item without one is selected. Nothing happens for an item no menu of the bar holds, a
    /// disabled one on the way, or while a chosen row flashes.
    pub fn open_item(&mut self, item: ItemId) -> Outcome {
        let mut out = Outcome::default();
        if self.flash.is_some() {
            return out;
        }
        let found = self.menu_slots().into_iter().find_map(|key| {
            let root = self.root_item(&key)?;
            if root.id == item && matches!(key, SlotKey::Menu(_)) {
                return Some((key, Vec::new()));
            }
            Some((key, root.path_to(item)?))
        });
        let Some((key, path)) = found else {
            return out;
        };
        out.consumed = true;
        if self.open_slot.as_ref() == Some(&key) {
            self.levels.truncate(1);
            self.levels[0].selected = None;
        } else {
            self.open_title(key, &mut out);
        }
        self.tracking = None;
        self.keyboard_origin = false;
        self.mode = Mode::Sticky;
        for (level, id) in path.into_iter().enumerate() {
            let Some(row) = self
                .levels
                .get(level)
                .and_then(|l| l.layout.index_of(id))
                .filter(|row| self.levels[level].layout.rows[*row].item.is_selectable())
            else {
                break;
            };
            self.levels[level].selected = Some(row);
            if !self.levels[level].layout.rows[row].item.has_submenu {
                break;
            }
            self.open_submenu(level, false, &mut out);
        }
        out.effects.push(Effect::Redraw);
        self.synced(out)
    }
}
