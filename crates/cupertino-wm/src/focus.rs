//! Focus model: application focus versus window focus,
//! app MRU plus per-app window MRU, hide / hide others, the app switcher and in-app window cycling,
//! modal blocking.

use crate::activation::{token_accepted, GRANT_LIFETIME, LATE_ACTIVATION, TYPING_QUIET};
use crate::command::Effect;
use crate::command::MapFocus;
use crate::ids::{AppId, WindowId};
use crate::wm::Wm;

/// Delay between the switcher opening and its panel appearing (150 ms). Until
/// then the Tab steps only move a counter ([`hidden_index`]).
pub const SWITCHER_SHOW_DELAY: f64 = 0.15;

/// How a switcher key moves the selection: Tab and the right arrow step forward, Shift-Tab,
/// backtick and the left arrow back, Home and Page Up go to the first entry, End and Page Down to
/// the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitcherStep {
    Next,
    Previous,
    First,
    Last,
}

/// The open application switcher: its entries, most recently used first, and the selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Switcher {
    pub items: Vec<AppId>,
    pub selected: usize,
    opened_at: f64,
    /// Net Tab steps (forward +1, backward −1) while the panel is not up yet; `None` once another
    /// key or a pick moved the selection.
    hidden_count: Option<i64>,
}

/// The entry a hidden switcher with the net step count `count` selects among `n`: a positive count clamps at the last entry, a negative one counts from the end and
/// clamps at the first, zero selects by the direction of the last step.
pub fn hidden_index(count: i64, n: usize, forward: bool) -> usize {
    let n = n as i64;
    let index = match count {
        c if c > 0 => c.min(n - 1),
        c if c < 0 => {
            if -c <= n {
                n + c
            } else {
                0
            }
        }
        _ if forward => i64::from(n > 1),
        _ => n - 1,
    };
    index as usize
}

/// The entry a visible switcher moves to: `wrap` (a fresh press) carries a step past either end
/// round to the other one; a repeat stops there.
fn stepped(selected: usize, n: usize, step: SwitcherStep, wrap: bool) -> usize {
    match step {
        SwitcherStep::Next if selected + 1 < n => selected + 1,
        SwitcherStep::Next if wrap => 0,
        SwitcherStep::Previous if selected > 0 => selected - 1,
        SwitcherStep::Previous if wrap => n - 1,
        SwitcherStep::Next | SwitcherStep::Previous => selected,
        SwitcherStep::First => 0,
        SwitcherStep::Last => n - 1,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CycleSession {
    list: Vec<WindowId>,
    index: usize,
}

#[derive(Default)]
pub struct FocusState {
    pub(crate) front_app: Option<AppId>,
    pub(crate) key_window: Option<WindowId>,
    /// Most recently used first.
    pub(crate) app_mru: Vec<AppId>,
    pub(crate) switcher: Option<Switcher>,
    pub(crate) cycle: Option<CycleSession>,
}

impl Wm {
    pub fn switcher(&self) -> Option<&Switcher> {
        self.focus.switcher.as_ref()
    }

    /// The modal dialog (deepest, topmost) that blocks input to `window`, if any.
    pub fn blocking_modal(&self, window: WindowId) -> Option<WindowId> {
        let mut cur = window;
        let mut found = None;
        for _ in 0..=self.windows.len() {
            let child = self.stack.iter().rev().copied().find(|id| {
                self.windows.get(id).is_some_and(|w| {
                    w.parent == Some(cur) && w.modal && !w.lifecycle.state().is_dead()
                })
            });
            match child {
                Some(c) => {
                    found = Some(c);
                    cur = c;
                }
                None => break,
            }
        }
        found
    }

    pub fn is_blocked(&self, window: WindowId) -> bool {
        self.blocking_modal(window).is_some()
    }

    /// Whether an activation token created at `token` (seconds, the model's clock) may bring
    /// `window` forward, given the last key or button press `last_user_event` ([`token_accepted`]
    /// against the last time the window's application became active) at `now`. A token more than [`LATE_ACTIVATION`]
    /// old is too old for a window that is already mapped. False for an unknown window.
    pub fn activation_allowed(
        &self,
        window: WindowId,
        token: f64,
        last_user_event: Option<f64>,
        now: f64,
    ) -> bool {
        if now - token > LATE_ACTIVATION {
            return false;
        }
        let Some(app) = self
            .windows
            .get(&window)
            .and_then(|w| self.apps.get(&w.app))
        else {
            return false;
        };
        token_accepted(token, app.activated_at, last_user_event)
    }

    /// Whether a window of `app` (a sheet or dialog of `parent`) that maps at `now` takes the keyboard. It does when nothing
    /// is focused (there is nothing to take it from), when it belongs to the key window, when its application is the
    /// front one, when its client presented a valid grant that is still fresh, and when it is the first window of a client
    /// the person has not left since it connected and is not typing at that moment ([`TYPING_QUIET`]). Otherwise a program that came up on its own would take the keystrokes
    /// of the person's work.
    pub(crate) fn takes_focus_on_map(
        &self,
        app: AppId,
        parent: Option<WindowId>,
        facts: &MapFocus,
        now: f64,
    ) -> bool {
        let Some(key) = self.focus.key_window else {
            return true;
        };
        if parent == Some(key) || self.focus.front_app == Some(app) {
            return true;
        }
        if let Some(grant) = facts.grant {
            let activated_at = self.apps.get(&app).and_then(|a| a.activated_at);
            if grant.valid
                && now - grant.created <= GRANT_LIFETIME
                && token_accepted(grant.created, activated_at, facts.last_user_event)
            {
                return true;
            }
        }
        let typing = facts
            .last_user_event
            .is_some_and(|event| now - event < TYPING_QUIET);
        facts.first_of_client && !facts.input_elsewhere && !typing
    }

    /// Put a window that maps without the focus just below the key window, so it neither covers it nor takes its place.
    pub(crate) fn place_behind_key_window(&mut self, window: WindowId) {
        let Some(key) = self.focus.key_window else {
            return;
        };
        let level = self.windows.get(&window).map(|w| w.level);
        let key_level = self.windows.get(&key).map(|w| w.level);
        if level.is_none() || level > key_level {
            return;
        }
        let Some(at) = self.stack.iter().position(|w| *w == key) else {
            return;
        };
        self.stack.retain(|w| *w != window);
        let at = self.stack.iter().position(|w| *w == key).unwrap_or(at);
        self.stack.insert(at, window);
    }

    /// The client of `window` asked for attention (an activation that was refused): its
    /// application's dock tile bounces, unless it is the active application (a request
    /// for attention does nothing while the application is active).
    pub(crate) fn demand_attention(&mut self, window: WindowId, out: &mut Vec<Effect>) {
        let Some(app) = self
            .windows
            .get(&window)
            .filter(|w| !w.lifecycle.state().is_dead())
            .map(|w| w.app)
        else {
            return;
        };
        if self.focus.front_app != Some(app) {
            out.push(Effect::AppAttention { app });
        }
    }

    fn promote_app(&mut self, app: AppId) {
        self.focus.app_mru.retain(|a| *a != app);
        self.focus.app_mru.insert(0, app);
    }

    fn promote_window(&mut self, window: WindowId) {
        let Some(app) = self.windows.get(&window).map(|w| w.app) else {
            return;
        };
        if let Some(a) = self.apps.get_mut(&app) {
            a.windows_mru.retain(|w| *w != window);
            a.windows_mru.insert(0, window);
        }
    }

    pub(crate) fn set_key(&mut self, window: Option<WindowId>, out: &mut Vec<Effect>) {
        let prev = self.focus.key_window;
        if prev == window {
            return;
        }
        self.focus.key_window = window;
        out.push(Effect::Focus { window });
        for id in prev.into_iter().chain(window) {
            if let Some(c) = self.configure_effect(id, false) {
                out.push(c);
            }
        }
    }

    pub(crate) fn activate_front(&mut self, app: AppId, now: f64, out: &mut Vec<Effect>) {
        if self.focus.front_app != Some(app) {
            self.focus.front_app = Some(app);
            if let Some(a) = self.apps.get_mut(&app) {
                a.activated_at = Some(now);
            }
            out.push(Effect::Activate { app: Some(app) });
        }
        self.promote_app(app);
        if self.apps.get(&app).is_some_and(|a| a.hidden) {
            self.unhide_app(app, out);
        }
    }

    /// First focusable window of `app` in MRU order, redirected to a blocking modal child.
    pub(crate) fn pick_key_window(&self, app: AppId) -> Option<WindowId> {
        let a = self.apps.get(&app)?;
        a.windows_mru
            .iter()
            .copied()
            .find(|w| self.focusable(*w))
            .map(|w| self.blocking_modal(w).unwrap_or(w))
    }

    /// Click / focus request on a window.
    ///
    /// * a window blocked by a modal child redirects focus to the child and pulses it;
    /// * a minimized window is restored; a window on another workspace switches workspace (or is
    ///   moved here when `switch_on_activate` is off);
    /// * if the owner is not the front app, the app is activated first (menu bar switches).
    pub(crate) fn focus_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if w.lifecycle.state().is_dead() {
            return;
        }
        let mut target = id;
        if let Some(m) = self.blocking_modal(id) {
            target = m;
            out.push(Effect::Attention { window: m });
        }
        if self.windows[&target].lifecycle.state().is_minimized() {
            self.restore_window(target, now, out);
        }
        self.ensure_on_screen(target, now, out);
        let app = self.windows[&target].app;
        self.activate_front(app, now, out);
        self.promote_window(target);
        self.raise(target, out);
        self.set_key(Some(target), out);
    }

    pub(crate) fn activate_app(&mut self, app: AppId, now: f64, out: &mut Vec<Effect>) {
        if !self.apps.contains_key(&app) {
            return;
        }
        self.activate_front(app, now, out);
        let mut key = self.pick_key_window(app);
        if key.is_none() {
            let mru = self.apps[&app].windows_mru.clone();
            let live = |wm: &Wm, w: &WindowId| {
                wm.windows
                    .get(w)
                    .is_some_and(|x| !x.lifecycle.state().is_dead())
            };
            let off_screen = mru.iter().copied().find(|w| {
                live(self, w)
                    && !self.windows[w].lifecycle.state().is_minimized()
                    && !self.window_on_screen(*w)
            });
            if let (Some(w), true) = (off_screen, self.cfg.workspaces.switch_on_activate) {
                self.ensure_on_screen(w, now, out);
                key = self.pick_key_window(app);
            }
            if key.is_none() && self.cfg.focus.restore_minimized_on_activate {
                if let Some(w) = mru
                    .iter()
                    .copied()
                    .find(|w| live(self, w) && self.windows[w].lifecycle.state().is_minimized())
                {
                    self.restore_window(w, now, out);
                    self.ensure_on_screen(w, now, out);
                    key = Some(w);
                }
            }
        }
        match key {
            Some(k) => {
                if self.cfg.focus.bring_all_to_front {
                    let mut others: Vec<WindowId> = self.apps[&app]
                        .windows_mru
                        .iter()
                        .copied()
                        .filter(|w| *w != k && self.focusable(*w))
                        .collect();
                    others.reverse();
                    for w in others {
                        self.raise(w, out);
                    }
                }
                self.promote_window(k);
                self.raise(k, out);
                self.set_key(Some(k), out);
            }
            None => {
                self.set_key(None, out);
            }
        }
    }

    /// Called when `id` stops being focusable (minimized, closing, destroyed).
    pub(crate) fn release_focus(&mut self, id: WindowId, _now: f64, out: &mut Vec<Effect>) {
        if let Some(c) = self.focus.cycle.as_mut() {
            c.list.retain(|w| *w != id);
        }
        if self.focus.key_window != Some(id) {
            return;
        }
        let Some(w) = self.windows.get(&id) else {
            self.set_key(None, out);
            return;
        };
        let (app, parent) = (w.app, w.parent);
        let next = parent
            .filter(|p| self.focusable(*p))
            .map(|p| self.blocking_modal(p).unwrap_or(p))
            .filter(|p| *p != id)
            .or_else(|| self.pick_key_window(app).filter(|p| *p != id));
        match next {
            Some(n) => {
                self.promote_window(n);
                self.set_key(Some(n), out);
            }
            None => self.set_key(None, out),
        }
    }

    pub(crate) fn hide_app(&mut self, app: AppId, now: f64, out: &mut Vec<Effect>) {
        self.hide_app_only(app, out);
        if self.focus.front_app == Some(app) {
            let next = self
                .focus
                .app_mru
                .iter()
                .copied()
                .filter(|a| *a != app)
                .find(|a| {
                    self.apps.get(a).is_some_and(|x| !x.hidden)
                        && self.pick_key_window(*a).is_some()
                });
            match next {
                Some(n) => self.activate_app(n, now, out),
                None => {
                    self.focus.front_app = None;
                    self.set_key(None, out);
                    out.push(Effect::Activate { app: None });
                }
            }
        }
    }

    fn hide_app_only(&mut self, app: AppId, out: &mut Vec<Effect>) {
        let Some(a) = self.apps.get_mut(&app) else {
            return;
        };
        if a.hidden {
            return;
        }
        a.hidden = true;
        let mut ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.app == app)
            .map(|w| w.id)
            .collect();
        ids.sort();
        for window in ids {
            out.push(Effect::SetVisible {
                window,
                visible: false,
            });
        }
    }

    pub(crate) fn unhide_app(&mut self, app: AppId, out: &mut Vec<Effect>) {
        let Some(a) = self.apps.get_mut(&app) else {
            return;
        };
        if !a.hidden {
            return;
        }
        a.hidden = false;
        let mut ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.app == app)
            .map(|w| w.id)
            .collect();
        ids.sort();
        for window in ids {
            let visible = self.windows[&window].lifecycle.state().is_visible();
            out.push(Effect::SetVisible { window, visible });
        }
    }

    pub(crate) fn hide_others(&mut self, app: AppId, now: f64, out: &mut Vec<Effect>) {
        let others: Vec<AppId> = self
            .focus
            .app_mru
            .iter()
            .copied()
            .filter(|a| *a != app)
            .collect();
        for a in others {
            self.hide_app_only(a, out);
        }
        self.activate_app(app, now, out);
    }

    pub(crate) fn show_all(&mut self, out: &mut Vec<Effect>) {
        let hidden: Vec<AppId> = self
            .focus
            .app_mru
            .iter()
            .copied()
            .filter(|a| self.apps.get(a).is_some_and(|x| x.hidden))
            .collect();
        for a in hidden {
            self.unhide_app(a, out);
        }
    }

    /// Open the switcher: the applications that have a window, most recently used
    /// first; the first press is the first step of the hidden counter, so forward selects the
    /// previous application and backward the least recent one.
    pub(crate) fn switcher_open(&mut self, backward: bool, now: f64, out: &mut Vec<Effect>) {
        if self.focus.switcher.is_some() {
            let step = if backward {
                SwitcherStep::Previous
            } else {
                SwitcherStep::Next
            };
            self.switcher_step(step, false, now, out);
            return;
        }
        let items: Vec<AppId> = self
            .focus
            .app_mru
            .iter()
            .copied()
            .filter(|a| self.apps.get(a).is_some_and(|x| !x.windows_mru.is_empty()))
            .collect();
        if items.is_empty() {
            return;
        }
        let count = if backward { -1 } else { 1 };
        let selected = hidden_index(count, items.len(), !backward);
        out.push(Effect::SwitcherChanged {
            apps: items.clone(),
            selected,
        });
        self.focus.switcher = Some(Switcher {
            items,
            selected,
            opened_at: now,
            hidden_count: Some(count),
        });
    }

    /// Move the selection. Before the panel is up ([`SWITCHER_SHOW_DELAY`]) Tab steps move the
    /// hidden counter, which clamps instead of wrapping; once it is up a fresh press wraps and a key
    /// repeat stops at the ends. Any other step, or a pick, ends the hidden counter.
    pub(crate) fn switcher_step(
        &mut self,
        step: SwitcherStep,
        repeat: bool,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(s) = self.focus.switcher.as_mut() else {
            return;
        };
        let n = s.items.len();
        let before = s.selected;
        let hidden = now - s.opened_at < SWITCHER_SHOW_DELAY;
        match (s.hidden_count, step) {
            (Some(count), SwitcherStep::Next | SwitcherStep::Previous) if hidden => {
                let forward = step == SwitcherStep::Next;
                let count = count + if forward { 1 } else { -1 };
                s.hidden_count = Some(count);
                s.selected = hidden_index(count, n, forward);
            }
            _ => {
                s.hidden_count = None;
                s.selected = stepped(s.selected, n, step, !repeat);
            }
        }
        if s.selected != before {
            out.push(Effect::SwitcherChanged {
                apps: s.items.clone(),
                selected: s.selected,
            });
        }
    }

    pub(crate) fn switcher_select(&mut self, index: usize, out: &mut Vec<Effect>) {
        let Some(s) = self
            .focus
            .switcher
            .as_mut()
            .filter(|s| index < s.items.len() && index != s.selected)
        else {
            return;
        };
        s.selected = index;
        s.hidden_count = None;
        out.push(Effect::SwitcherChanged {
            apps: s.items.clone(),
            selected: index,
        });
    }

    pub(crate) fn switcher_commit(&mut self, now: f64, out: &mut Vec<Effect>) {
        let Some(s) = self.focus.switcher.take() else {
            return;
        };
        out.push(Effect::SwitcherClosed { committed: true });
        if let Some(app) = s.items.get(s.selected).copied() {
            self.activate_app(app, now, out);
        }
    }

    pub(crate) fn switcher_cancel(&mut self, out: &mut Vec<Effect>) {
        if self.focus.switcher.take().is_some() {
            out.push(Effect::SwitcherClosed { committed: false });
        }
    }

    /// Q: ask every window of the selected application to close. The entry leaves the
    /// list when its last window is gone ([`Wm::switcher_forget`]). With a single entry nothing
    /// happens (the design refuses with the alert sound, which the model has no channel for).
    pub(crate) fn switcher_quit(&mut self, out: &mut Vec<Effect>) {
        let Some(s) = self.focus.switcher.as_ref().filter(|s| s.items.len() > 1) else {
            return;
        };
        let app = s.items[s.selected];
        let mut ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.app == app && !w.lifecycle.state().is_dead())
            .map(|w| w.id)
            .collect();
        ids.sort();
        out.extend(
            ids.into_iter()
                .map(|window| Effect::RequestClose { window }),
        );
    }

    /// H: hide the selected application, or show it again when it is hidden; the switcher stays
    /// open.
    pub(crate) fn switcher_toggle_hide(&mut self, now: f64, out: &mut Vec<Effect>) {
        let Some(app) = self.focus.switcher.as_ref().map(|s| s.items[s.selected]) else {
            return;
        };
        if self.apps.get(&app).is_some_and(|a| a.hidden) {
            self.unhide_app(app, out);
        } else {
            self.hide_app(app, now, out);
        }
    }

    /// `app` may have lost its last window: an open switcher drops its entry (the selection stays
    /// on the same place, or the new last entry) and ends when nothing is left.
    pub(crate) fn switcher_forget(&mut self, app: AppId, out: &mut Vec<Effect>) {
        if self
            .apps
            .get(&app)
            .is_some_and(|a| !a.windows_mru.is_empty())
        {
            return;
        }
        let Some(s) = self.focus.switcher.as_mut() else {
            return;
        };
        let Some(i) = s.items.iter().position(|a| *a == app) else {
            return;
        };
        s.items.remove(i);
        if s.items.is_empty() {
            self.switcher_cancel(out);
            return;
        }
        if i < s.selected || s.selected == s.items.len() {
            s.selected -= 1;
        }
        out.push(Effect::SwitcherChanged {
            apps: s.items.clone(),
            selected: s.selected,
        });
    }

    /// Window cycling inside the front app. The list is a snapshot taken at the first press; the
    /// MRU order is only updated when the session ends (any other command or `EndCycleWindows`).
    pub(crate) fn cycle_windows(&mut self, backward: bool, out: &mut Vec<Effect>) {
        let Some(app) = self.focus.front_app else {
            return;
        };
        if self.focus.cycle.is_none() {
            let list: Vec<WindowId> = self.apps[&app]
                .windows_mru
                .iter()
                .copied()
                .filter(|w| self.focusable(*w) && !self.is_blocked(*w))
                .collect();
            let index = self
                .focus
                .key_window
                .and_then(|k| list.iter().position(|w| *w == k))
                .unwrap_or(0);
            self.focus.cycle = Some(CycleSession { list, index });
        }
        let c = self.focus.cycle.as_mut().unwrap();
        if c.list.len() < 2 {
            return;
        }
        let n = c.list.len();
        c.index = if backward {
            (c.index + n - 1) % n
        } else {
            (c.index + 1) % n
        };
        let target = c.list[c.index];
        self.raise(target, out);
        self.set_key(Some(target), out);
    }

    pub(crate) fn finish_cycle(&mut self) {
        if self.focus.cycle.take().is_some() {
            if let Some(k) = self.focus.key_window {
                self.promote_window(k);
            }
        }
    }
}
