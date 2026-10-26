//! Panel state: query editing, result selection, scrolling and the appear/disappear animation.
//!
//! The panel is a model. The compositor feeds it translated key events ([`Key`], [`Mods`]) and
//! pointer events and draws it with [`crate::render`]. Editing uses emacs-style bindings:
//!
//! | key | action |
//! |---|---|
//! | characters, Backspace, Delete, Left, Right, Home, End | edit the query |
//! | Ctrl+A / Ctrl+E | start / end of the query |
//! | Ctrl+B / Ctrl+F, Ctrl+Left / Ctrl+Right | character / word movement |
//! | Ctrl+U / Ctrl+K | delete to start / to end |
//! | Ctrl+W, Ctrl+Backspace | delete the previous word |
//! | Down / Ctrl+N, Up / Ctrl+P | next / previous result, no wrap-around; Up on the first result returns to the field |
//! | Primary+Down, Primary+Up | first result of the next / previous section |
//! | PageDown, PageUp | move by a page |
//! | Tab | in the field: complete the query to the selected result's title; in the list: back to the field |
//! | Enter | activate the selected result and close |
//! | Alt+1 … Alt+9 | activate the n-th result (authored extension) |
//! | Escape | list focus: back to the field; non-empty query: clear it; empty query: close ("Escape twice") |
//! | Primary+Enter / Primary+R, +B, +I, +Y, +K, +L, +D, +, +H | [`PanelCommand`] events for the host |
//! | Space with list focus | [`PanelCommand::Preview`] |
//!
//! Primary is the platform's "command" modifier (Super here). Keyboard semantics come from the design
//! desktop's launcher analysis,.
//!
//! Every query change re-runs [`Launcher::query`] synchronously (the matcher answers 5 000
//! applications in a few milliseconds) and selects the first result.

use crate::engine::{Activation, Launcher, ResultItem, Results};
use cupertino_animation::{AnimatedValue, Curve, SpringParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    PageUp,
    PageDown,
    Tab,
    Enter,
    Escape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The primary ("command") modifier, Super on this platform.
    pub primary: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        primary: false,
    };
    pub const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
        primary: false,
    };
    pub const ALT: Mods = Mods {
        ctrl: false,
        alt: true,
        shift: false,
        primary: false,
    };
    pub const PRIMARY: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        primary: true,
    };

    /// Primary held without Ctrl / Alt / Shift (the design test for panel shortcuts).
    pub fn only_primary(self) -> bool {
        self.primary && !self.ctrl && !self.alt && !self.shift
    }
}

/// Both clocks the panel needs: the monotonic frame clock (animations) and wall-clock Unix
/// seconds (frecency decay).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub mono: f64,
    pub unix: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelEvent {
    None,
    /// The query, selection or scroll position changed: redraw.
    Changed,
    /// Close the panel without activating anything.
    Close,
    /// An item was activated; the panel closes itself.
    Activate(Activation),
    /// A panel shortcut the host has to carry out (the panel stays open unless noted).
    Command(PanelCommand),
}

/// Shortcut commands of the panel. The selected item, when needed, is
/// [`Panel::selected_item`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelCommand {
    /// Show the selected item in the file manager.
    Reveal,
    /// Show the properties of the selected item.
    Properties,
    /// Quick preview of the selected item (does not close the panel).
    Preview,
    /// Search the web for the query.
    WebSearch,
    /// Open the design-article result.
    OpenReference,
    /// Select the dictionary result.
    SelectDefinition,
    /// Open the dictionary result.
    OpenDefinition,
    /// Open the launcher settings; the panel closes.
    OpenSettings,
    /// Open the launcher help; the panel closes.
    Help,
}

/// Which part of the panel owns the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The query field; typing edits it, the first result is highlighted (the "top hit").
    Field,
    /// The result list; printable keys are forwarded to the field.
    List,
}

pub struct Panel {
    query: String,
    cursor: usize,
    results: Results,
    selected: usize,
    scroll_row: usize,
    visible: bool,
    focus: Focus,
    /// The whole query is selected (after the invocation and after leaving the list upwards): typing replaces it.
    select_all: bool,
    appear: AnimatedValue<f64>,
    /// Rows (headers included) shown at once; keep equal to the style's `max_rows`.
    pub max_rows: usize,
    pub spring: SpringParams,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

fn is_sep(c: char) -> bool {
    c.is_whitespace()
}

impl Panel {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            results: Results::default(),
            selected: 0,
            scroll_row: 0,
            visible: false,
            focus: Focus::Field,
            select_all: false,
            appear: AnimatedValue::new(0.0),
            max_rows: 9,
            spring: SpringParams::from_perceptual(0.3, 0.0),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Cursor position in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn results(&self) -> &Results {
        &self.results
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_item(&self) -> Option<&ResultItem> {
        self.results.get(self.selected)
    }

    pub fn scroll_row(&self) -> usize {
        self.scroll_row
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// `true` while the whole query is selected (typing replaces it).
    pub fn query_selected(&self) -> bool {
        self.select_all
    }

    /// Appear progress 0…1 (the spring may overshoot slightly with a bouncy configuration).
    pub fn progress(&self, now: f64) -> f64 {
        self.appear.value(now)
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.appear.is_animating(now)
    }

    /// `true` while something has to be drawn: visible or still animating out.
    pub fn needs_draw(&self, now: f64) -> bool {
        self.visible || self.appear.is_animating(now) || self.appear.value(now) > 0.001
    }

    /// Open with an empty query (suggestions).
    pub fn show(&mut self, launcher: &Launcher, now: Now) {
        self.query.clear();
        self.cursor = 0;
        self.visible = true;
        self.focus = Focus::Field;
        self.select_all = false;
        self.appear
            .animate_to(1.0, Curve::Spring(self.spring), now.mono);
        self.requery(launcher, now.unix);
    }

    pub fn hide(&mut self, now: f64) {
        self.visible = false;
        self.focus = Focus::Field;
        self.appear.animate_to(0.0, Curve::Spring(self.spring), now);
    }

    pub fn toggle(&mut self, launcher: &Launcher, now: Now) {
        if self.visible {
            self.hide(now.mono);
        } else {
            self.show(launcher, now);
        }
    }

    pub fn set_query(&mut self, text: &str, launcher: &Launcher, now: Now) {
        self.query = text.to_string();
        self.cursor = self.query.chars().count();
        self.requery(launcher, now.unix);
    }

    /// Re-run the current query, keeping the selection index where possible (for results that
    /// arrive late, for example when the file index finishes).
    pub fn refresh(&mut self, launcher: &Launcher, now: Now) {
        let keep = self.selected;
        self.results = launcher.query(&self.query, now.unix);
        self.selected = keep.min(self.results.len().saturating_sub(1));
        self.clamp_scroll();
    }

    fn requery(&mut self, launcher: &Launcher, unix: f64) {
        self.results = launcher.query(&self.query, unix);
        self.selected = 0;
        self.scroll_row = 0;
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_index)
            .map_or(self.query.len(), |(b, _)| b)
    }

    fn chars_len(&self) -> usize {
        self.query.chars().count()
    }

    /// Row index of the flat result index (counting section headers).
    pub fn row_of(&self, flat: usize) -> Option<usize> {
        let (s, i) = self.results.locate(flat)?;
        let before: usize = self.results.sections[..s]
            .iter()
            .map(|x| x.items.len() + 1)
            .sum();
        Some(before + 1 + i)
    }

    pub fn total_rows(&self) -> usize {
        self.results
            .sections
            .iter()
            .map(|s| s.items.len() + 1)
            .sum()
    }

    fn clamp_scroll(&mut self) {
        let Some(row) = self.row_of(self.selected) else {
            self.scroll_row = 0;
            return;
        };
        let max = self.max_rows.max(1);
        let first_in_section = self
            .results
            .locate(self.selected)
            .is_some_and(|(_, i)| i == 0);
        let top = if first_in_section { row - 1 } else { row };
        if top < self.scroll_row {
            self.scroll_row = top;
        } else if row >= self.scroll_row + max {
            self.scroll_row = row + 1 - max;
        }
        self.scroll_row = self.scroll_row.min(self.total_rows().saturating_sub(max));
    }

    fn move_selection(&mut self, delta: isize, wrap: bool) -> PanelEvent {
        let n = self.results.len();
        if n == 0 {
            return PanelEvent::None;
        }
        let target = self.selected as isize + delta;
        let new = if wrap {
            target.rem_euclid(n as isize) as usize
        } else {
            target.clamp(0, n as isize - 1) as usize
        };
        if new == self.selected {
            return PanelEvent::None;
        }
        self.selected = new;
        self.clamp_scroll();
        PanelEvent::Changed
    }

    /// First result of the next (`forward`) or previous section (Primary+Down / Primary+Up).
    fn jump_section(&mut self, forward: bool) -> PanelEvent {
        let Some((s, _)) = self.results.locate(self.selected) else {
            return PanelEvent::None;
        };
        let target = if forward {
            s + 1
        } else {
            s.checked_sub(1).unwrap_or(usize::MAX)
        };
        let Some(section) = self.results.sections.get(target) else {
            return PanelEvent::None;
        };
        if section.items.is_empty() {
            return PanelEvent::None;
        }
        let flat: usize = self.results.sections[..target]
            .iter()
            .map(|x| x.items.len())
            .sum();
        self.focus = Focus::List;
        self.set_selected(flat);
        PanelEvent::Changed
    }

    /// Down / Up: moves through the list without wrapping; Up on the first result gives the keyboard back to the field.
    fn arrow(&mut self, down: bool) -> PanelEvent {
        if self.results.is_empty() {
            return PanelEvent::None;
        }
        if !down && self.selected == 0 {
            if self.focus == Focus::List || !self.select_all {
                self.focus = Focus::Field;
                self.select_all = !self.query.is_empty();
                return PanelEvent::Changed;
            }
            return PanelEvent::None;
        }
        self.focus = Focus::List;
        self.select_all = false;
        self.move_selection(if down { 1 } else { -1 }, false);
        PanelEvent::Changed
    }

    /// Select by flat index (pointer hover).
    pub fn set_selected(&mut self, flat: usize) -> PanelEvent {
        if flat >= self.results.len() || flat == self.selected {
            return PanelEvent::None;
        }
        self.selected = flat;
        self.clamp_scroll();
        PanelEvent::Changed
    }

    /// Activate the result at a flat index (click, Alt+digit) and close.
    pub fn activate_index(&mut self, flat: usize, launcher: &mut Launcher, now: Now) -> PanelEvent {
        let Some(item) = self.results.get(flat).cloned() else {
            return PanelEvent::None;
        };
        let activation = launcher.activate(&item, &self.query, now.unix);
        self.hide(now.mono);
        PanelEvent::Activate(activation)
    }

    fn word_start(&self, from: usize) -> usize {
        let chars: Vec<char> = self.query.chars().collect();
        let mut i = from;
        while i > 0 && is_sep(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && !is_sep(chars[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_end(&self, from: usize) -> usize {
        let chars: Vec<char> = self.query.chars().collect();
        let mut i = from;
        while i < chars.len() && is_sep(chars[i]) {
            i += 1;
        }
        while i < chars.len() && !is_sep(chars[i]) {
            i += 1;
        }
        i
    }

    fn delete_range(
        &mut self,
        from: usize,
        to: usize,
        launcher: &Launcher,
        now: Now,
    ) -> PanelEvent {
        if from >= to {
            return PanelEvent::None;
        }
        let (b0, b1) = (self.byte_at(from), self.byte_at(to));
        self.query.replace_range(b0..b1, "");
        self.cursor = from;
        self.requery(launcher, now.unix);
        PanelEvent::Changed
    }

    fn insert(&mut self, c: char, launcher: &Launcher, now: Now) -> PanelEvent {
        if c.is_control() {
            return PanelEvent::None;
        }
        if self.select_all {
            self.query.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        self.focus = Focus::Field;
        let b = self.byte_at(self.cursor);
        self.query.insert(b, c);
        self.cursor += 1;
        self.requery(launcher, now.unix);
        PanelEvent::Changed
    }

    fn move_cursor(&mut self, to: usize) -> PanelEvent {
        let to = to.min(self.chars_len());
        if to == self.cursor {
            return PanelEvent::None;
        }
        self.cursor = to;
        PanelEvent::Changed
    }

    /// Handle one key press. Returns what the host has to do.
    pub fn handle_key(
        &mut self,
        key: Key,
        mods: Mods,
        launcher: &mut Launcher,
        now: Now,
    ) -> PanelEvent {
        if !self.visible {
            return PanelEvent::None;
        }
        let page = self.max_rows.max(1) as isize;
        // Panel shortcuts: the primary modifier alone.
        if mods.only_primary() {
            let cmd = match key {
                Key::Enter | Key::Char('r') => Some(PanelCommand::Reveal),
                Key::Char('b') => Some(PanelCommand::WebSearch),
                Key::Char('i') => Some(PanelCommand::Properties),
                Key::Char('y') => Some(PanelCommand::Preview),
                Key::Char('k') => Some(PanelCommand::OpenReference),
                Key::Char('l') => Some(PanelCommand::SelectDefinition),
                Key::Char('d') => Some(PanelCommand::OpenDefinition),
                Key::Char(',') => Some(PanelCommand::OpenSettings),
                Key::Char('h') => Some(PanelCommand::Help),
                _ => None,
            };
            if let Some(cmd) = cmd {
                if matches!(cmd, PanelCommand::OpenSettings | PanelCommand::Help) {
                    self.hide(now.mono);
                }
                return PanelEvent::Command(cmd);
            }
            match key {
                Key::Down => return self.jump_section(true),
                Key::Up => return self.jump_section(false),
                _ => {}
            }
        }
        match (key, mods) {
            (Key::Escape, _) => {
                if self.focus == Focus::List {
                    self.focus = Focus::Field;
                    PanelEvent::Changed
                } else if !self.query.is_empty() {
                    self.set_query("", launcher, now);
                    self.select_all = false;
                    PanelEvent::Changed
                } else {
                    self.hide(now.mono);
                    PanelEvent::Close
                }
            }
            (Key::Enter, _) => self.activate_index(self.selected, launcher, now),
            (Key::Char(c @ '1'..='9'), m) if m.alt => {
                self.activate_index(c as usize - '1' as usize, launcher, now)
            }
            (Key::Char(' '), m) if self.focus == Focus::List && !m.ctrl && !m.alt && !m.primary => {
                PanelEvent::Command(PanelCommand::Preview)
            }
            (Key::Char('a'), m) if m.ctrl => self.move_cursor(0),
            (Key::Char('e'), m) if m.ctrl => self.move_cursor(self.chars_len()),
            (Key::Char('b'), m) if m.ctrl => self.move_cursor(self.cursor.saturating_sub(1)),
            (Key::Char('f'), m) if m.ctrl => self.move_cursor(self.cursor + 1),
            (Key::Char('u'), m) if m.ctrl => self.delete_range(0, self.cursor, launcher, now),
            (Key::Char('k'), m) if m.ctrl => {
                self.delete_range(self.cursor, self.chars_len(), launcher, now)
            }
            (Key::Char('w'), m) if m.ctrl => {
                self.delete_range(self.word_start(self.cursor), self.cursor, launcher, now)
            }
            (Key::Char('n'), m) if m.ctrl => self.arrow(true),
            (Key::Char('p'), m) if m.ctrl => self.arrow(false),
            (Key::Char(c), m) if !m.ctrl && !m.alt && !m.primary => self.insert(c, launcher, now),
            (Key::Char(_), _) => PanelEvent::None,
            // Delete and Tab in the list only return the keyboard to the field.
            (Key::Backspace | Key::Delete | Key::Tab, _) if self.focus == Focus::List => {
                self.focus = Focus::Field;
                PanelEvent::Changed
            }
            (Key::Backspace, m) if m.ctrl => {
                self.delete_range(self.word_start(self.cursor), self.cursor, launcher, now)
            }
            (Key::Backspace, _) if self.select_all => {
                self.select_all = false;
                self.set_query("", launcher, now);
                PanelEvent::Changed
            }
            (Key::Backspace, _) => {
                self.delete_range(self.cursor.saturating_sub(1), self.cursor, launcher, now)
            }
            (Key::Delete, _) => self.delete_range(
                self.cursor,
                (self.cursor + 1).min(self.chars_len()),
                launcher,
                now,
            ),
            (Key::Left, m) if m.ctrl => self.move_cursor(self.word_start(self.cursor)),
            (Key::Right, m) if m.ctrl => self.move_cursor(self.word_end(self.cursor)),
            (Key::Left, _) => self.move_cursor(self.cursor.saturating_sub(1)),
            (Key::Right, _) => self.move_cursor(self.cursor + 1),
            (Key::Home, _) => self.move_cursor(0),
            (Key::End, _) => self.move_cursor(self.chars_len()),
            (Key::Down, _) => self.arrow(true),
            (Key::Up, _) => self.arrow(false),
            (Key::PageDown, _) => self.move_selection(page, false),
            (Key::PageUp, _) => self.move_selection(-page, false),
            (Key::Tab, _) => match self.selected_item().map(|i| i.title.clone()) {
                Some(t) if t != self.query => {
                    self.set_query(&t, launcher, now);
                    PanelEvent::Changed
                }
                _ => PanelEvent::None,
            },
        }
    }
}

#[cfg(test)]
mod tests;
