//! Workspaces: per-output ordered
//! lists, sticky windows, full-screen workspaces inserted after the current one and removed on exit,
//! optional MRU reordering, gesture-driven switching with rubber band, app → workspace assignment.
//!
//! The slide itself is the 60 Hz solver (`crate::slide`). The finger-tracking
//! rubber band, the velocity projection and the swipe jump limit are approximate.

use cupertino_effects::SlideStart;

use crate::command::Effect;
use crate::ids::{AppId, OutputId, WindowId, WorkspaceId};
use crate::slide::SlidePosition;
use crate::window::Assignment;
use crate::wm::Wm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    Regular,
    /// Dedicated workspace of a full-screen window; `origin` is where the window returns to.
    FullScreen {
        window: WindowId,
        origin: WorkspaceId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub kind: WorkspaceKind,
}

pub struct OutputWorkspaces {
    pub list: Vec<Workspace>,
    pub current: WorkspaceId,
    /// Fractional index into `list` (the slide position).
    pub(crate) position: SlidePosition,
    pub(crate) gesture_start: Option<usize>,
    pub(crate) pending_removal: Vec<WorkspaceId>,
    pub(crate) pending_reorder: bool,
}

impl OutputWorkspaces {
    pub fn index_of(&self, id: WorkspaceId) -> Option<usize> {
        self.list.iter().position(|w| w.id == id)
    }
    pub(crate) fn is_animating(&self, now: f64) -> bool {
        self.position.is_animating(now)
            || self.gesture_start.is_some()
            || !self.pending_removal.is_empty()
            || self.pending_reorder
    }
}

fn rubber(x: f64, c: f64) -> f64 {
    1.0 - 1.0 / (x * c + 1.0)
}

impl Wm {
    pub fn workspaces(&self, output: OutputId) -> &[Workspace] {
        self.workspaces.get(&output).map_or(&[], |w| &w.list)
    }
    pub fn current_workspace(&self, output: OutputId) -> Option<WorkspaceId> {
        self.workspaces.get(&output).map(|w| w.current)
    }
    /// Slide position in workspace-index units (0.0 = first workspace fully shown).
    pub fn workspace_position(&self, output: OutputId, now: f64) -> f64 {
        self.workspaces
            .get(&output)
            .map_or(0.0, |w| w.position.value(now))
    }
    pub fn windows_in_workspace(&self, ws: WorkspaceId) -> Vec<WindowId> {
        let mut v: Vec<WindowId> = self
            .stack
            .iter()
            .copied()
            .filter(|id| self.windows[id].workspace == ws)
            .collect();
        v.retain(|id| !self.windows[id].lifecycle.state().is_dead());
        v
    }

    /// Width of `output` in points; workspaces exist only for registered outputs.
    fn output_width(&self, output: OutputId) -> f64 {
        self.output(output)
            .expect("workspaces exist only for registered outputs")
            .rect
            .w
    }

    fn alloc_workspace(&mut self) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace);
        self.next_workspace += 1;
        id
    }

    pub(crate) fn create_output_workspaces(&mut self, output: OutputId, out: &mut Vec<Effect>) {
        let id = self.alloc_workspace();
        self.workspaces.insert(
            output,
            OutputWorkspaces {
                list: vec![Workspace {
                    id,
                    kind: WorkspaceKind::Regular,
                }],
                current: id,
                position: SlidePosition::new(0.0),
                gesture_start: None,
                pending_removal: Vec::new(),
                pending_reorder: false,
            },
        );
        out.push(Effect::WorkspaceCreated {
            output,
            workspace: id,
            index: 0,
        });
    }

    pub(crate) fn add_workspace(
        &mut self,
        output: OutputId,
        out: &mut Vec<Effect>,
    ) -> Option<WorkspaceId> {
        if !self.workspaces.contains_key(&output) {
            return None;
        }
        let id = self.alloc_workspace();
        let ws = self.workspaces.get_mut(&output).unwrap();
        ws.list.push(Workspace {
            id,
            kind: WorkspaceKind::Regular,
        });
        out.push(Effect::WorkspaceCreated {
            output,
            workspace: id,
            index: ws.list.len() - 1,
        });
        Some(id)
    }

    pub(crate) fn output_of_workspace(&self, id: WorkspaceId) -> Option<OutputId> {
        self.workspaces
            .iter()
            .find(|(_, w)| w.list.iter().any(|x| x.id == id))
            .map(|(o, _)| *o)
    }

    /// Remove a regular workspace; its windows move to the previous (else next) regular one.
    pub(crate) fn remove_workspace(&mut self, id: WorkspaceId, now: f64, out: &mut Vec<Effect>) {
        let Some(output) = self.output_of_workspace(id) else {
            return;
        };
        let ws = &self.workspaces[&output];
        let idx = ws.index_of(id).unwrap();
        if ws.list[idx].kind != WorkspaceKind::Regular {
            return;
        }
        let regulars: Vec<usize> = (0..ws.list.len())
            .filter(|i| ws.list[*i].kind == WorkspaceKind::Regular)
            .collect();
        if regulars.len() < 2 {
            return;
        }
        let dest = regulars
            .iter()
            .rev()
            .find(|i| **i < idx)
            .or_else(|| regulars.iter().find(|i| **i > idx))
            .copied()
            .unwrap();
        let dest_id = ws.list[dest].id;
        for w in self.windows.values_mut() {
            if w.workspace == id {
                w.workspace = dest_id;
            }
        }
        for a in self.apps.values_mut() {
            if a.assignment == Assignment::Workspace(id) {
                a.assignment = Assignment::None;
            }
        }
        if self.workspaces[&output].current == id {
            self.switch_workspace(output, dest_id, true, now, out);
        }
        let width = self.output_width(output);
        let ws = self.workspaces.get_mut(&output).unwrap();
        let cur_idx_before = ws.index_of(ws.current).unwrap();
        let was = ws.list.remove(idx);
        let cur_idx = ws.index_of(ws.current).unwrap();
        if cur_idx != cur_idx_before && !ws.position.is_animating(now) {
            ws.position.set_immediately(cur_idx as f64);
        } else if cur_idx != cur_idx_before {
            ws.position
                .slide_to(cur_idx, ws.list.len(), width, SlideStart::Programmatic, now);
        }
        out.push(Effect::WorkspaceRemoved {
            output,
            workspace: was.id,
        });
    }

    /// Move a regular workspace to `index` among its output's list (clamped to the end); the slide
    /// position follows the current workspace so nothing moves on screen.
    pub(crate) fn move_workspace(
        &mut self,
        id: WorkspaceId,
        index: usize,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(output) = self.output_of_workspace(id) else {
            return;
        };
        let ws = self.workspaces.get_mut(&output).unwrap();
        let from = ws.index_of(id).unwrap();
        if ws.list[from].kind != WorkspaceKind::Regular {
            return;
        }
        let to = index.min(ws.list.len() - 1);
        if from == to {
            return;
        }
        let moved = ws.list.remove(from);
        ws.list.insert(to, moved);
        let cur = ws.index_of(ws.current).unwrap();
        if !ws.position.is_animating(now) {
            ws.position.set_immediately(cur as f64);
        }
        out.push(Effect::WorkspacesReordered { output });
    }

    pub(crate) fn switch_workspace_relative(
        &mut self,
        output: OutputId,
        delta: i32,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(ws) = self.workspaces.get(&output) else {
            return;
        };
        let idx = ws.index_of(ws.current).unwrap() as i64;
        let n = ws.list.len() as i64;
        let target = (idx + delta as i64).clamp(0, n - 1) as usize;
        let id = ws.list[target].id;
        self.switch_workspace(output, id, true, now, out);
    }

    /// Make `to` the current workspace of `output` with the slide animation.
    pub(crate) fn switch_workspace(
        &mut self,
        output: OutputId,
        to: WorkspaceId,
        refocus: bool,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let mru = self.cfg.workspaces.mru_reorder;
        let width = self.output_width(output);
        let Some(ws) = self.workspaces.get_mut(&output) else {
            return;
        };
        let Some(idx) = ws.index_of(to) else { return };
        let from = ws.current;
        ws.gesture_start = None;
        ws.current = to;
        ws.position
            .slide_to(idx, ws.list.len(), width, SlideStart::Programmatic, now);
        if mru {
            ws.pending_reorder = true;
        }
        if from != to {
            out.push(Effect::WorkspaceSwitched { output, from, to });
            if refocus {
                self.focus_after_switch(output, now, out);
            }
        }
    }

    /// After a switch: keep the key window if it is still on screen, else focus the most recently
    /// used window that is (activating its app), else clear the key window.
    fn focus_after_switch(&mut self, output: OutputId, now: f64, out: &mut Vec<Effect>) {
        if self.focus.key_window.is_some_and(|k| self.focusable(k)) {
            return;
        }
        let mru = self.focus.app_mru.clone();
        for app in mru {
            if let Some(w) = self.apps.get(&app).and_then(|a| {
                a.windows_mru
                    .iter()
                    .copied()
                    .find(|w| self.focusable(*w) && self.windows[w].output == output)
            }) {
                let w = self.blocking_modal(w).unwrap_or(w);
                self.activate_front(app, now, out);
                self.raise(w, out);
                self.set_key(Some(w), out);
                return;
            }
        }
        self.set_key(None, out);
    }

    /// Bring the window onto the screen of its output: switch to its workspace
    /// (`switch_on_activate`) or move it to the current workspace.
    pub(crate) fn ensure_on_screen(&mut self, window: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get(&window) else {
            return;
        };
        let (output, ws, sticky) = (w.output, w.workspace, w.sticky);
        let Some(cur) = self.workspaces.get(&output).map(|o| o.current) else {
            return;
        };
        if sticky || cur == ws {
            return;
        }
        if self.cfg.workspaces.switch_on_activate {
            self.switch_workspace(output, ws, false, now, out);
        } else {
            self.windows.get_mut(&window).unwrap().workspace = cur;
        }
    }

    pub(crate) fn move_window_to_workspace(
        &mut self,
        window: WindowId,
        ws: WorkspaceId,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(output) = self.output_of_workspace(ws) else {
            return;
        };
        let Some(w) = self.windows.get_mut(&window) else {
            return;
        };
        if w.lifecycle.state().is_fullscreen() {
            return;
        }
        w.output = output;
        w.workspace = ws;
        w.sticky = false;
        w.sticky_from_app = false;
        if self.focus.key_window == Some(window) && !self.focusable(window) {
            self.release_focus(window, now, out);
        }
    }

    pub(crate) fn set_sticky(&mut self, window: WindowId, sticky: bool, from_app: bool) {
        if let Some(w) = self.windows.get_mut(&window) {
            if w.lifecycle.state().is_fullscreen() {
                return;
            }
            w.sticky = sticky;
            w.sticky_from_app = sticky && from_app;
        }
    }

    pub(crate) fn assign_app(&mut self, app: AppId, assignment: Assignment) {
        let Some(a) = self.apps.get_mut(&app) else {
            return;
        };
        let previous = a.assignment;
        a.assignment = assignment;
        let ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.app == app)
            .map(|w| w.id)
            .collect();
        for id in ids {
            match assignment {
                Assignment::AllWorkspaces => self.set_sticky(id, true, true),
                _ => {
                    if previous == Assignment::AllWorkspaces && self.windows[&id].sticky_from_app {
                        self.set_sticky(id, false, false);
                    }
                }
            }
        }
    }

    pub(crate) fn fullscreen_workspace_of(
        &self,
        window: WindowId,
    ) -> Option<(OutputId, WorkspaceId)> {
        self.workspaces.iter().find_map(|(o, ws)| {
            ws.list
                .iter()
                .find(|w| {
                    matches!(w.kind, WorkspaceKind::FullScreen { window: x, .. } if x == window)
                        && !ws.pending_removal.contains(&w.id)
                })
                .map(|w| (*o, w.id))
        })
    }

    /// Full-screen windows get their own workspace inserted after the current one.
    pub(crate) fn enter_fullscreen_workspace(
        &mut self,
        window: WindowId,
        output: OutputId,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let reuse = self.workspaces.get(&output).and_then(|ws| {
            ws.list
                .iter()
                .find(|w| matches!(w.kind, WorkspaceKind::FullScreen { window: x, .. } if x == window))
                .map(|w| w.id)
        });
        let id = match reuse {
            Some(id) => {
                self.workspaces
                    .get_mut(&output)
                    .unwrap()
                    .pending_removal
                    .retain(|w| *w != id);
                id
            }
            None => {
                let id = self.alloc_workspace();
                let ws = self.workspaces.get_mut(&output).unwrap();
                let origin = ws.current;
                let idx = ws.index_of(origin).unwrap() + 1;
                ws.list.insert(
                    idx,
                    Workspace {
                        id,
                        kind: WorkspaceKind::FullScreen { window, origin },
                    },
                );
                out.push(Effect::WorkspaceCreated {
                    output,
                    workspace: id,
                    index: idx,
                });
                // `position` is expressed in index units: keep the current workspace where it is.
                let cur = ws.index_of(ws.current).unwrap();
                let at = ws.position.value(now);
                if cur as f64 != at && !ws.position.is_animating(now) {
                    ws.position.set_immediately(cur as f64);
                }
                id
            }
        };
        self.windows.get_mut(&window).unwrap().workspace = id;
        self.switch_workspace(output, id, false, now, out);
    }

    /// Return the window to its origin workspace; the full-screen workspace is removed once the
    /// slide has settled.
    pub(crate) fn leave_fullscreen_workspace(
        &mut self,
        window: WindowId,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some((output, fs)) = self.fullscreen_workspace_of(window) else {
            return;
        };
        let ws = &self.workspaces[&output];
        let origin = match ws.list.iter().find(|w| w.id == fs).map(|w| w.kind) {
            Some(WorkspaceKind::FullScreen { origin, .. }) if ws.index_of(origin).is_some() => {
                origin
            }
            _ => ws
                .list
                .iter()
                .find(|w| w.kind == WorkspaceKind::Regular)
                .map(|w| w.id)
                .unwrap(),
        };
        let mut others: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.workspace == fs)
            .map(|w| w.id)
            .collect();
        others.sort();
        for w in others {
            self.windows.get_mut(&w).unwrap().workspace = origin;
        }
        let ws = self.workspaces.get_mut(&output).unwrap();
        ws.pending_removal.push(fs);
        let was_current = ws.current == fs;
        if was_current {
            self.switch_workspace(output, origin, false, now, out);
        }
    }

    pub(crate) fn tick_workspaces(&mut self, now: f64, out: &mut Vec<Effect>) {
        let mut outputs: Vec<OutputId> = self.workspaces.keys().copied().collect();
        outputs.sort();
        for output in outputs {
            let ws = self.workspaces.get_mut(&output).unwrap();
            if ws.position.is_animating(now) || ws.gesture_start.is_some() {
                continue;
            }
            let pending = std::mem::take(&mut ws.pending_removal);
            for id in pending {
                if ws.current == id {
                    ws.pending_removal.push(id);
                    continue;
                }
                if let Some(i) = ws.index_of(id) {
                    ws.list.remove(i);
                    out.push(Effect::WorkspaceRemoved {
                        output,
                        workspace: id,
                    });
                }
            }
            if ws.pending_reorder {
                ws.pending_reorder = false;
                let cur = ws.index_of(ws.current).unwrap();
                if ws.list[cur].kind == WorkspaceKind::Regular && cur != 0 {
                    let mut end = cur + 1;
                    while end < ws.list.len()
                        && matches!(ws.list[end].kind, WorkspaceKind::FullScreen { .. })
                    {
                        end += 1;
                    }
                    let group: Vec<Workspace> = ws.list.drain(cur..end).collect();
                    for (k, g) in group.into_iter().enumerate() {
                        ws.list.insert(k, g);
                    }
                    out.push(Effect::WorkspacesReordered { output });
                }
            }
            let cur = ws.index_of(ws.current).unwrap() as f64;
            if ws.position.value(now) != cur {
                ws.position.set_immediately(cur);
            }
        }
    }

    pub(crate) fn workspace_gesture_begin(&mut self, output: OutputId, now: f64) {
        let Some(ws) = self.workspaces.get_mut(&output) else {
            return;
        };
        let p = ws.position.value(now);
        let start = ws.index_of(ws.current).unwrap();
        ws.gesture_start = Some(start);
        ws.position.track(p, now);
    }

    pub(crate) fn workspace_gesture_update(&mut self, output: OutputId, fraction: f64, now: f64) {
        let c = self.cfg.workspaces.rubber_band;
        let Some(ws) = self.workspaces.get_mut(&output) else {
            return;
        };
        let Some(start) = ws.gesture_start else {
            return;
        };
        let max = (ws.list.len() - 1) as f64;
        let raw = start as f64 - fraction;
        let p = if raw < 0.0 {
            -rubber(-raw, c)
        } else if raw > max {
            max + rubber(raw - max, c)
        } else {
            raw
        };
        ws.position.track(p, now);
    }

    pub(crate) fn workspace_gesture_end(
        &mut self,
        output: OutputId,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let cfg = self.cfg.workspaces.clone();
        let width = self.output_width(output);
        let Some(ws) = self.workspaces.get_mut(&output) else {
            return;
        };
        let Some(start) = ws.gesture_start.take() else {
            return;
        };
        let p = ws.position.value(now);
        let v = ws.position.velocity(now);
        let max = ws.list.len() - 1;
        let projected = p + v * cfg.projection_s;
        let lo = start.saturating_sub(cfg.max_swipe_jump);
        let hi = (start + cfg.max_swipe_jump).min(max);
        let target = (projected.round().max(0.0) as usize).clamp(lo, hi);
        let id = ws.list[target].id;
        let from = ws.current;
        ws.current = id;
        ws.position
            .slide_to(target, ws.list.len(), width, SlideStart::Release, now);
        if cfg.mru_reorder {
            ws.pending_reorder = true;
        }
        if from != id {
            out.push(Effect::WorkspaceSwitched {
                output,
                from,
                to: id,
            });
            self.focus_after_switch(output, now, out);
        }
    }
}
