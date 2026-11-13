//! Panel state: query editing, result selection, scrolling and the appear/disappear animation.
//!
//! The panel is a model. The compositor feeds it translated key events ([`Key`], [`Mods`]) and
//! pointer events and draws it with [`crate::render`]. The query is a shared text field (`cupertino-ui-text`'s `TextBuffer`):
//! every key the table below does not name is [`PanelEvent::Edit`], which the host passes to [`Panel::edit`].
//!
//! | key | action |
//! |---|---|
//! | Down / Ctrl+N, Up / Ctrl+P | next / previous result, no wrap-around; Up on the first result returns to the field |
//! | Primary+Down, Primary+Up | first result of the next / previous section |
//! | PageDown, PageUp | move by a page |
//! | Tab, Shift+Tab | next / previous action of the selected result, wrapping round ([`Launcher::actions`]) |
//! | Right, Left with list focus | next / previous action of the selected result, stopping at the ends |
//! | Enter | carry out the selected result's chosen action (its own activation unless Tab or the arrows chose another) and close |
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

use crate::engine::{Activation, ItemAction, Launcher, ResultItem, Results};
use crate::refine;
use cupertino_animation::{AnimatedValue, Curve, SpringParams};
use cupertino_ui_text::TextBuffer;

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
    /// A key of the query field: the host passes it to the field through [`Panel::edit`].
    Edit,
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

/// Whether `key` with `mods` is one of the panel's keys while `focus` owns the keyboard: [`Panel::handle_key`] acts on
/// it (or ignores it) instead of returning [`PanelEvent::Edit`]. A host that hands the field's keys to a text field of
/// its own asks this before the field sees the key.
pub fn takes_key(key: Key, mods: Mods, focus: Focus) -> bool {
    if mods.only_primary()
        && matches!(
            key,
            Key::Enter
                | Key::Up
                | Key::Down
                | Key::Char('r' | 'b' | 'i' | 'y' | 'k' | 'l' | 'd' | ',' | 'h')
        )
    {
        return true;
    }
    match (key, mods) {
        (Key::Escape | Key::Enter, _) => true,
        (Key::Left | Key::Right, _) if focus == Focus::List => true,
        (Key::Char('1'..='9'), m) if m.alt => true,
        (Key::Char(' '), m) if focus == Focus::List && !m.ctrl && !m.alt && !m.primary => true,
        (Key::Char('n' | 'p'), m) if m.ctrl => true,
        (Key::Backspace | Key::Delete, _) if focus == Focus::List => true,
        (Key::Down | Key::Up | Key::PageDown | Key::PageUp | Key::Tab, _) => true,
        _ => false,
    }
}

/// The most bytes the query holds.
pub const QUERY_MAX_LEN: usize = 1024;

pub struct Panel {
    field: TextBuffer,
    results: Results,
    selected: usize,
    /// The chosen action of the selected result, an index into its [`Launcher::actions`]; 0 is its own activation.
    action: usize,
    scroll_row: usize,
    visible: bool,
    focus: Focus,
    appear: AnimatedValue<f64>,
    /// Rows (headers included) shown at once; keep equal to the style's `max_rows`.
    pub max_rows: usize,
    pub spring: SpringParams,
    /// The searches that opened something, the latest first, at most [`HISTORY_LEN`]; and which of them Up put in
    /// the field, while it stands there unedited.
    history: Vec<String>,
    recalled: Option<usize>,
}

/// How many past searches Up can recall.
pub const HISTORY_LEN: usize = 20;

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            field: TextBuffer::with_max_len(QUERY_MAX_LEN),
            results: Results::default(),
            selected: 0,
            action: 0,
            scroll_row: 0,
            visible: false,
            focus: Focus::Field,
            appear: AnimatedValue::new(0.0),
            max_rows: 9,
            spring: SpringParams::from_perceptual(0.3, 0.0),
            history: Vec::new(),
            recalled: None,
        }
    }

    /// The searches Up recalls, the latest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Remember `query` as the latest search that opened something.
    fn remember(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.history.retain(|q| q != query);
        self.history.insert(0, query.to_owned());
        self.history.truncate(HISTORY_LEN);
    }

    /// Up in the field with nothing typed: the latest search comes back, and each further Up the one before it.
    /// `None` when there is nothing (more) to recall or something is typed.
    fn recall(&mut self, launcher: &Launcher, now: Now) -> Option<PanelEvent> {
        let shown = self
            .recalled
            .and_then(|i| self.history.get(i))
            .map(String::as_str);
        let next = match shown {
            Some(q) if q == self.field.text() => self.recalled? + 1,
            _ if self.field.text().is_empty() => 0,
            _ => return None,
        };
        let query = self.history.get(next)?.clone();
        self.set_query(&query, launcher, now);
        self.field.select_all();
        self.recalled = Some(next);
        Some(PanelEvent::Changed)
    }

    pub fn query(&self) -> &str {
        self.field.text()
    }

    /// The query's text field (caret, selection, scroll).
    pub fn field(&self) -> &TextBuffer {
        &self.field
    }

    /// Change the query field with `f`; when its text changes the query runs again and the field takes the keyboard.
    pub fn edit<R>(
        &mut self,
        launcher: &Launcher,
        now: Now,
        f: impl FnOnce(&mut TextBuffer) -> R,
    ) -> R {
        let before = self.field.text().to_string();
        let r = f(&mut self.field);
        if self.field.text() != before {
            self.focus = Focus::Field;
            self.requery(launcher, now.unix);
        }
        r
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

    /// The chosen action of the selected result: 0 for its own activation, else an index into its actions.
    pub fn action(&self) -> usize {
        self.action
    }

    /// Choose action `index` of the selected result; out of range chooses its own activation.
    pub fn set_action(&mut self, index: usize, launcher: &Launcher) -> PanelEvent {
        let count = self.action_count(launcher);
        let index = if index < count { index } else { 0 };
        if index == self.action {
            return PanelEvent::None;
        }
        self.action = index;
        PanelEvent::Changed
    }

    fn action_count(&self, launcher: &Launcher) -> usize {
        self.selected_item()
            .map_or(0, |item| launcher.actions(item).len())
    }

    /// Move the chosen action by `delta`, wrapping round the selected result's actions or stopping at their ends.
    fn step_action(&mut self, delta: isize, wrap: bool, launcher: &Launcher) -> PanelEvent {
        let count = self.action_count(launcher) as isize;
        if count <= 1 {
            return PanelEvent::None;
        }
        let target = self.action as isize + delta;
        let next = if wrap {
            target.rem_euclid(count)
        } else {
            target.clamp(0, count - 1)
        } as usize;
        if next == self.action {
            return PanelEvent::None;
        }
        self.action = next;
        PanelEvent::Changed
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

    /// Whether the query field's caret shows at `now`: the panel is open with the keyboard in the field and the blink
    /// is on. `steady` (Reduce Motion): it does not blink.
    pub fn caret_visible(&self, now: std::time::Instant, steady: bool) -> bool {
        self.visible
            && self.focus == Focus::Field
            && if steady {
                self.field.selection().is_empty()
            } else {
                self.field.caret_visible(now)
            }
    }

    /// Until the caret's next blink while it can show; never while it is `steady`.
    pub fn next_blink(&self, now: std::time::Instant, steady: bool) -> Option<std::time::Duration> {
        (self.visible && self.focus == Focus::Field && !steady)
            .then(|| self.field.next_blink(now))
            .flatten()
    }

    /// Scroll the query so that the caret at `caret_x` stays inside a field `width` points wide.
    pub fn follow_caret(&mut self, caret_x: f64, text_width: f64, width: f64) {
        self.field.follow_caret(caret_x, text_width, width);
    }

    /// A click in the query field gives it the keyboard.
    pub fn focus_field(&mut self) {
        self.focus = Focus::Field;
    }

    /// `true` while the whole query is selected (typing replaces it).
    fn query_selected(&self) -> bool {
        !self.field.is_empty() && self.field.selection() == (0..self.field.text().len())
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

    /// Open with the previous query kept and selected, so typing replaces it. A kept query is not searched until it
    /// is edited: nothing shows below the field for it. With no query, the suggestions.
    pub fn show(&mut self, launcher: &Launcher, now: Now) {
        self.visible = true;
        self.focus = Focus::Field;
        self.appear
            .animate_to(1.0, Curve::Spring(self.spring), now.mono);
        if self.field.is_empty() {
            self.requery(launcher, now.unix);
        } else {
            self.results = Results::default();
            self.selected = 0;
            self.action = 0;
            self.scroll_row = 0;
            self.field.select_all();
        }
    }

    /// Put the appear animation at its end at once (the shell client's fixed test clock).
    pub fn settle_appear(&mut self) {
        self.appear
            .set_immediately(if self.visible { 1.0 } else { 0.0 });
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
        self.field.set_text(text);
        self.requery(launcher, now.unix);
    }

    /// Re-run the current query, keeping the selected result (for results that arrive late, for example when the
    /// file index finishes).
    pub fn refresh(&mut self, launcher: &Launcher, now: Now) {
        self.set_results(launcher.query(self.field.text(), now.unix));
    }

    /// Show `results` for the current query (a slower provider answered): the selected result stays selected wherever
    /// it moved, with its chosen action; when it is gone, the selection stays at its place in the list.
    pub fn set_results(&mut self, results: Results) {
        let keep = self.selected_item().map(|item| item.id.clone());
        let at = self.selected;
        self.results = results;
        match keep.and_then(|id| self.results.iter().position(|item| item.id == id)) {
            Some(index) => self.selected = index,
            None => {
                self.selected = at.min(self.results.len().saturating_sub(1));
                self.action = 0;
            }
        }
        self.clamp_scroll();
    }

    /// Put `text` in the query field without searching (the application browser filters by it instead).
    pub fn set_text_quiet(&mut self, text: &str) {
        self.field.set_text(text);
        self.results = Results::default();
        self.selected = 0;
        self.action = 0;
        self.scroll_row = 0;
    }

    fn requery(&mut self, launcher: &Launcher, unix: f64) {
        self.results = launcher.query(self.field.text(), unix);
        self.selected = 0;
        self.action = 0;
        self.scroll_row = 0;
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
        self.action = 0;
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
            if self.focus == Focus::List || !self.query_selected() {
                self.focus = Focus::Field;
                self.field.select_all();
                return PanelEvent::Changed;
            }
            return PanelEvent::None;
        }
        self.focus = Focus::List;
        self.field.blur();
        self.move_selection(if down { 1 } else { -1 }, false);
        PanelEvent::Changed
    }

    /// Select by flat index (pointer hover).
    pub fn set_selected(&mut self, flat: usize) -> PanelEvent {
        if flat >= self.results.len() || flat == self.selected {
            return PanelEvent::None;
        }
        self.selected = flat;
        self.action = 0;
        self.clamp_scroll();
        PanelEvent::Changed
    }

    /// Carry out action `index` of the result at `flat` (a click on one of its action buttons, Enter on a chosen
    /// action) and close. Its own activation is [`Panel::activate_index`].
    pub fn activate_action(
        &mut self,
        flat: usize,
        index: usize,
        launcher: &mut Launcher,
        now: Now,
    ) -> PanelEvent {
        let Some(item) = self.results.get(flat).cloned() else {
            return PanelEvent::None;
        };
        let actions: Vec<ItemAction> = launcher.actions(&item);
        let Some(action) = actions.get(index) else {
            return PanelEvent::None;
        };
        let activation = launcher.activate_action(&item, action, self.field.text(), now.unix);
        let query = self.field.text().to_owned();
        self.remember(&query);
        self.hide(now.mono);
        PanelEvent::Activate(activation)
    }

    /// Activate the result at a flat index (click, Alt+digit) and close.
    pub fn activate_index(&mut self, flat: usize, launcher: &mut Launcher, now: Now) -> PanelEvent {
        let Some(item) = self.results.get(flat).cloned() else {
            return PanelEvent::None;
        };
        let activation = launcher.activate(&item, self.field.text(), now.unix);
        let query = self.field.text().to_owned();
        self.remember(&query);
        self.hide(now.mono);
        PanelEvent::Activate(activation)
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
                } else if !self.field.is_empty() {
                    self.set_query("", launcher, now);
                    PanelEvent::Changed
                } else {
                    self.hide(now.mono);
                    PanelEvent::Close
                }
            }
            (Key::Enter, _) if self.action > 0 => {
                self.activate_action(self.selected, self.action, launcher, now)
            }
            (Key::Enter, _) => self.activate_index(self.selected, launcher, now),
            (Key::Char(c @ '1'..='9'), m) if m.alt => {
                self.activate_index(c as usize - '1' as usize, launcher, now)
            }
            (Key::Char(' '), m) if self.focus == Focus::List && !m.ctrl && !m.alt && !m.primary => {
                PanelEvent::Command(PanelCommand::Preview)
            }
            (Key::Char('n'), m) if m.ctrl => self.arrow(true),
            (Key::Char('p'), m) if m.ctrl => self.arrow(false),
            // Delete in the list only returns the keyboard to the field.
            (Key::Backspace | Key::Delete, _) if self.focus == Focus::List => {
                self.focus = Focus::Field;
                PanelEvent::Changed
            }
            // a kind's name and Tab: the search is scoped to that kind, its token in the field
            (Key::Tab, m) if !m.shift && self.focus == Focus::Field => {
                match refine::scope_by_name(self.field.text()) {
                    Some(token) => {
                        self.set_query(&token, launcher, now);
                        PanelEvent::Changed
                    }
                    None => self.step_action(1, true, launcher),
                }
            }
            (Key::Tab, m) => self.step_action(if m.shift { -1 } else { 1 }, true, launcher),
            (Key::Right, _) if self.focus == Focus::List => self.step_action(1, false, launcher),
            (Key::Left, _) if self.focus == Focus::List => self.step_action(-1, false, launcher),
            (Key::Down, _) => self.arrow(true),
            (Key::Up, m) if self.focus == Focus::Field && m == Mods::NONE => self
                .recall(launcher, now)
                .unwrap_or_else(|| self.arrow(false)),
            (Key::Up, _) => self.arrow(false),
            (Key::PageDown, _) => self.move_selection(page, false),
            (Key::PageUp, _) => self.move_selection(-page, false),
            _ => PanelEvent::Edit,
        }
    }
}

#[cfg(test)]
mod tests;
