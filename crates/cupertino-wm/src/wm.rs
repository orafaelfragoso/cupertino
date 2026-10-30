//! The window manager model: owns apps, windows, outputs, focus, workspaces and the pointer
//! interaction state. Driven through [`Wm::handle`] and [`Wm::tick`]; produces [`Effect`]s.

use std::collections::HashMap;

use crate::command::{Activation, AnimationKind, Command, Effect, MapParams};
use crate::config::WmConfig;
use crate::focus::FocusState;
use crate::geometry::Rect;
use crate::ids::{AppId, OutputId, WindowId};
use crate::interaction::Interaction;
use crate::kind;
use crate::lifecycle::{Lifecycle, LifecycleState, Presentation};
use crate::tiling::{tile_rect, TileKind};
use crate::window::{App, Window, WindowLevel};
use crate::workspace::OutputWorkspaces;
use crate::zoom::best_fit;
use cupertino_effects::WindowKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub id: OutputId,
    /// Full output rectangle in the global logical space.
    pub rect: Rect,
    /// Area available to windows (excludes menu bar and dock reservations).
    pub usable: Rect,
    /// Region owned by this output's dock; tiling hot zones inside it are ignored when configured.
    pub dock_zone: Option<Rect>,
}

pub struct Wm {
    pub(crate) cfg: WmConfig,
    pub(crate) apps: HashMap<AppId, App>,
    pub(crate) identities: HashMap<String, AppId>,
    pub(crate) next_app: u64,
    pub(crate) next_workspace: u64,
    pub(crate) windows: HashMap<WindowId, Window>,
    /// Bottom to top.
    pub(crate) stack: Vec<WindowId>,
    pub(crate) outputs: Vec<Output>,
    pub(crate) focus: FocusState,
    pub(crate) workspaces: HashMap<OutputId, OutputWorkspaces>,
    pub(crate) interaction: Interaction,
}

impl Wm {
    pub fn new(cfg: WmConfig) -> Self {
        Self {
            cfg,
            apps: HashMap::new(),
            identities: HashMap::new(),
            next_app: 1,
            next_workspace: 1,
            windows: HashMap::new(),
            stack: Vec::new(),
            outputs: Vec::new(),
            focus: FocusState::default(),
            workspaces: HashMap::new(),
            interaction: Interaction::Idle,
        }
    }

    pub fn config(&self) -> &WmConfig {
        &self.cfg
    }
    pub fn config_mut(&mut self) -> &mut WmConfig {
        &mut self.cfg
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }
    pub fn app(&self, id: AppId) -> Option<&App> {
        self.apps.get(&id)
    }
    pub fn app_for_identity(&self, identity: &str) -> Option<AppId> {
        self.identities.get(identity).copied()
    }
    pub fn output(&self, id: OutputId) -> Option<&Output> {
        self.outputs.iter().find(|o| o.id == id)
    }
    pub fn outputs(&self) -> &[Output] {
        &self.outputs
    }
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }
    pub fn apps(&self) -> impl Iterator<Item = &App> {
        self.apps.values()
    }
    pub fn key_window(&self) -> Option<WindowId> {
        self.focus.key_window
    }
    pub fn front_app(&self) -> Option<AppId> {
        self.focus.front_app
    }
    /// Applications, most recently used first.
    pub fn app_mru(&self) -> &[AppId] {
        &self.focus.app_mru
    }
    /// Window stacking order, bottom to top.
    pub fn stacking_order(&self) -> &[WindowId] {
        &self.stack
    }
    pub fn presentation(&self, id: WindowId, now: f64) -> Option<Presentation> {
        self.windows.get(&id).map(|w| w.lifecycle.presentation(now))
    }

    /// Opacity of the dimming over `parent` while sheets hang from it: how far
    /// its most shown sheet is shown; 0 without one.
    pub fn sheet_dimming(&self, parent: WindowId, now: f64) -> f64 {
        self.windows
            .values()
            .filter(|w| w.parent == Some(parent))
            .map(|w| w.lifecycle.sheet_shown(now))
            .fold(0.0, f64::max)
    }

    /// Whether the compositor should draw the window on its output right now (ignores animations
    /// in progress): lifecycle-visible, app not hidden, on the output's current workspace or sticky.
    pub fn window_on_screen(&self, id: WindowId) -> bool {
        let Some(w) = self.windows.get(&id) else {
            return false;
        };
        w.lifecycle.state().is_visible()
            && !self.apps.get(&w.app).is_some_and(|a| a.hidden)
            && (w.sticky
                || self
                    .workspaces
                    .get(&w.output)
                    .is_some_and(|ws| ws.current == w.workspace))
    }

    /// Windows to draw on `output`, bottom to top.
    pub fn visible_windows(&self, output: OutputId) -> Vec<WindowId> {
        self.stack
            .iter()
            .copied()
            .filter(|id| self.windows[id].output == output && self.window_on_screen(*id))
            .collect()
    }

    /// Whether a window may take input focus right now.
    pub fn focusable(&self, id: WindowId) -> bool {
        self.window_on_screen(id)
            && self.windows.get(&id).is_some_and(|w| {
                let s = w.lifecycle.state();
                !s.is_minimized() && !s.is_dead()
            })
    }

    /// True while any animation, dwell timer or gesture needs further `tick` calls.
    pub fn wants_tick(&self, now: f64) -> bool {
        self.windows.values().any(|w| w.lifecycle.is_animating(now))
            || self.interaction.wants_tick(now)
            || self.workspaces.values().any(|w| w.is_animating(now))
    }

    pub fn handle(&mut self, cmd: Command, now: f64) -> Vec<Effect> {
        let mut out = Vec::new();
        let o = &mut out;
        if !matches!(
            cmd,
            Command::CycleWindows { .. } | Command::PointerMotion { .. } | Command::PointerRelease
        ) {
            self.finish_cycle();
        }
        match cmd {
            Command::AddOutput {
                output,
                rect,
                usable,
            } => self.add_output(output, rect, usable, o),
            Command::RemoveOutput { output } => self.remove_output(output, now, o),
            Command::SetUsableArea { output, usable } => {
                if let Some(op) = self.outputs.iter_mut().find(|p| p.id == output) {
                    op.usable = usable;
                }
            }
            Command::SetOutputRect {
                output,
                rect,
                usable,
            } => self.set_output_rect(output, rect, usable, o),
            Command::SetDockZone { output, zone } => {
                if let Some(op) = self.outputs.iter_mut().find(|p| p.id == output) {
                    op.dock_zone = zone;
                }
            }
            Command::RegisterApp { identity } => {
                self.ensure_app(&identity, o);
            }
            Command::RemoveApp { app } => self.remove_app(app, now, o),
            Command::Map(p) => self.map_window(p, now, o),
            Command::Unmap { window } => self.close_window(window, now, o),
            Command::Destroy { window } => self.destroy_window(window, now, o),
            Command::SetTitle { window, title } => {
                if let Some(w) = self.windows.get_mut(&window) {
                    w.title = title;
                }
            }
            Command::SetDisabledControls { window, disabled } => {
                if let Some(w) = self.windows.get_mut(&window) {
                    w.disabled = disabled;
                }
            }
            Command::SetSizeHints {
                window,
                min,
                max,
                content,
            } => {
                if let Some(w) = self.windows.get_mut(&window) {
                    w.min_size = min;
                    w.max_size = max;
                    w.content_hint = content;
                }
            }
            Command::SetParent { window, parent } => self.set_parent(window, parent),
            Command::SetModal { window, modal } => {
                if let Some(w) = self.windows.get_mut(&window) {
                    w.modal = modal;
                }
            }
            Command::RequestClose { window } => {
                if self.windows.contains_key(&window) {
                    o.push(Effect::RequestClose { window });
                }
            }
            Command::Minimize { window, target } => self.minimize_window(window, target, now, o),
            Command::Restore { window } => {
                self.restore_window(window, now, o);
                self.focus_window(window, now, o);
            }
            Command::SetMinimizeTarget { window, target } => {
                if let Some(w) = self.windows.get_mut(&window) {
                    w.lifecycle.set_minimize_target(target);
                }
            }
            Command::ToggleZoom { window } => self.toggle_zoom(window, now, o),
            Command::Fullscreen { window, output } => {
                self.fullscreen_window(window, output, now, o)
            }
            Command::ExitFullscreen { window } => self.exit_fullscreen_window(window, now, o),
            Command::MinimizeOrRestore { window } => self.minimize_or_restore(window, now, o),
            Command::Tile { window, kind } => self.tile_window(window, kind, now, o),
            Command::Untile { window } => self.untile_window(window, now, o),
            Command::FocusWindow { window } => self.focus_window(window, now, o),
            Command::DemandAttention { window } => self.demand_attention(window, o),
            Command::ActivateApp { app } => self.activate_app(app, now, o),
            Command::HideApp { app } => self.hide_app(app, now, o),
            Command::UnhideApp { app } => {
                self.unhide_app(app, o);
                self.activate_app(app, now, o);
            }
            Command::HideOthers { app } => self.hide_others(app, now, o),
            Command::ShowAll => self.show_all(o),
            Command::BringAllToFront { app, cascade } => {
                self.bring_all_to_front(app, cascade, now, o)
            }
            Command::SwitcherOpen { backward } => self.switcher_open(backward, now, o),
            Command::SwitcherStep { step, repeat } => self.switcher_step(step, repeat, now, o),
            Command::SwitcherSelect { index } => self.switcher_select(index, o),
            Command::SwitcherCommit => self.switcher_commit(now, o),
            Command::SwitcherCancel => self.switcher_cancel(o),
            Command::SwitcherQuit => self.switcher_quit(o),
            Command::SwitcherToggleHide => self.switcher_toggle_hide(now, o),
            Command::CycleWindows { backward } => self.cycle_windows(backward, o),
            Command::EndCycleWindows => {}
            Command::BeginMove { window, pointer } => self.begin_move(window, pointer, now, o),
            Command::BeginResize {
                window,
                edges,
                pointer,
            } => self.begin_resize(window, edges, pointer, now, o),
            Command::SnapEdgesToScreen {
                window,
                edges,
                opposite,
            } => self.snap_edges_to_screen(window, edges, opposite, now, o),
            Command::PointerMotion { pointer, modifiers } => {
                self.pointer_motion(pointer, modifiers, now, o)
            }
            Command::PointerRelease => self.pointer_release(now, o),
            Command::CancelInteraction => self.cancel_interaction(now, o),
            Command::AddWorkspace { output } => {
                self.add_workspace(output, o);
            }
            Command::RemoveWorkspace { workspace } => self.remove_workspace(workspace, now, o),
            Command::MoveWorkspace { workspace, index } => {
                self.move_workspace(workspace, index, now, o)
            }
            Command::SwitchWorkspace { output, workspace } => {
                self.switch_workspace(output, workspace, true, now, o)
            }
            Command::SwitchWorkspaceRelative { output, delta } => {
                self.switch_workspace_relative(output, delta, now, o)
            }
            Command::MoveWindowToWorkspace { window, workspace } => {
                self.move_window_to_workspace(window, workspace, now, o)
            }
            Command::SetSticky { window, sticky } => self.set_sticky(window, sticky, false),
            Command::AssignApp { app, assignment } => self.assign_app(app, assignment),
            Command::WorkspaceGestureBegin { output } => self.workspace_gesture_begin(output, now),
            Command::WorkspaceGestureUpdate { output, fraction } => {
                self.workspace_gesture_update(output, fraction, now)
            }
            Command::WorkspaceGestureEnd { output } => self.workspace_gesture_end(output, now, o),
        }
        out
    }

    /// Advance animations, dwell timers and workspace settling to `now`.
    pub fn tick(&mut self, now: f64) -> Vec<Effect> {
        let mut out = Vec::new();
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort();
        for id in ids {
            let Some(w) = self.windows.get_mut(&id) else {
                continue;
            };
            if let Some((from, to)) = w.lifecycle.tick(now) {
                out.push(Effect::WindowStateChanged {
                    window: id,
                    from,
                    to,
                });
                if to == LifecycleState::Destroyed {
                    self.finalize_window(id, now, &mut out);
                }
            }
        }
        self.tick_interaction(now, &mut out);
        self.tick_workspaces(now, &mut out);
        out
    }

    fn add_output(&mut self, id: OutputId, rect: Rect, usable: Rect, out: &mut Vec<Effect>) {
        if self.outputs.iter().any(|o| o.id == id) {
            return;
        }
        self.outputs.push(Output {
            id,
            rect,
            usable,
            dock_zone: None,
        });
        self.create_output_workspaces(id, out);
    }

    /// Refit the windows of an output whose rectangle changed.
    fn set_output_rect(&mut self, id: OutputId, rect: Rect, usable: Rect, out: &mut Vec<Effect>) {
        let Some(op) = self.outputs.iter_mut().find(|o| o.id == id) else {
            return;
        };
        op.rect = rect;
        op.usable = usable;
        let mut ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.output == id)
            .map(|w| w.id)
            .collect();
        ids.sort();
        for wid in ids {
            let margin = self.cfg.snap.tiled_margin;
            let zoom_cfg = self.cfg.zoom.clone();
            let w = self.windows.get_mut(&wid).unwrap();
            let hints = w.hints();
            let changed = match w.lifecycle.state() {
                LifecycleState::Normal | LifecycleState::Opening => {
                    let f = w.lifecycle.normal_frame().clamp_into(&usable);
                    f != w.lifecycle.normal_frame() && w.lifecycle.set_normal_frame(f)
                }
                LifecycleState::FullScreen | LifecycleState::EnterFullScreen => {
                    w.lifecycle.refit(rect, rect)
                }
                LifecycleState::Zoomed => {
                    let f = best_fit(&zoom_cfg, usable, w.lifecycle.target_frame(), &hints);
                    w.lifecycle.refit(f, f)
                }
                LifecycleState::Tiled(kind) | LifecycleState::Tiling(kind) => {
                    w.lifecycle.refit(tile_rect(kind, usable, margin), rect)
                }
                _ => false,
            };
            if changed {
                if let Some(c) = self.configure_effect(wid, false) {
                    out.push(c);
                }
                if let Some(w) = self.windows.get(&wid) {
                    out.push(Effect::Place {
                        window: wid,
                        rect: w.geometry(),
                    });
                }
            }
        }
    }

    fn remove_output(&mut self, id: OutputId, now: f64, out: &mut Vec<Effect>) {
        let Some(pos) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        self.outputs.remove(pos);
        self.workspaces.remove(&id);
        let Some(dest) = self.outputs.first().copied() else {
            return;
        };
        let dest_ws = self.workspaces[&dest.id].current;
        let mut ids: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.output == id)
            .map(|w| w.id)
            .collect();
        ids.sort();
        for wid in ids {
            let w = self.windows.get_mut(&wid).unwrap();
            w.output = dest.id;
            w.workspace = dest_ws;
            if w.lifecycle.state() == LifecycleState::Normal {
                let f = w.lifecycle.normal_frame().clamp_into(&dest.usable);
                w.lifecycle.set_normal_frame(f);
                out.push(Effect::Place {
                    window: wid,
                    rect: f,
                });
            }
        }
        let _ = now;
    }

    pub(crate) fn ensure_app(&mut self, identity: &str, out: &mut Vec<Effect>) -> AppId {
        if let Some(id) = self.identities.get(identity) {
            return *id;
        }
        let id = AppId(self.next_app);
        self.next_app += 1;
        self.apps.insert(
            id,
            App {
                id,
                identity: identity.to_string(),
                hidden: false,
                windows_mru: Vec::new(),
                assignment: Default::default(),
                activated_at: None,
            },
        );
        self.identities.insert(identity.to_string(), id);
        self.focus.app_mru.push(id);
        out.push(Effect::AppRegistered {
            app: id,
            identity: identity.to_string(),
        });
        id
    }

    fn remove_app(&mut self, app: AppId, now: f64, out: &mut Vec<Effect>) {
        let Some(a) = self.apps.get(&app) else { return };
        let ids = a.windows_mru.clone();
        let mut all: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.app == app)
            .map(|w| w.id)
            .collect();
        all.sort();
        for w in ids.into_iter().chain(all) {
            self.destroy_window(w, now, out);
        }
        if let Some(a) = self.apps.remove(&app) {
            self.identities.remove(&a.identity);
        }
        self.focus.app_mru.retain(|x| *x != app);
        if self.focus.front_app == Some(app) {
            self.focus.front_app = None;
            self.focus.key_window = None;
            let next = self
                .focus
                .app_mru
                .iter()
                .copied()
                .find(|a| self.apps.get(a).is_some_and(|a| !a.hidden));
            match next {
                Some(n) => self.activate_app(n, now, out),
                None => {
                    out.push(Effect::Activate { app: None });
                    out.push(Effect::Focus { window: None });
                }
            }
        }
    }

    pub(crate) fn configure_effect(&self, id: WindowId, resizing: bool) -> Option<Effect> {
        let w = self.windows.get(&id)?;
        let mut states = w.lifecycle.toplevel_states();
        states.activated = self.focus.key_window == Some(id);
        states.resizing = resizing;
        Some(Effect::Configure {
            window: id,
            size: w.geometry().size(),
            states,
        })
    }

    pub(crate) fn emit_layout(&self, id: WindowId, kind: AnimationKind, out: &mut Vec<Effect>) {
        if let (Some(c), Some(w)) = (self.configure_effect(id, false), self.windows.get(&id)) {
            out.push(c);
            out.push(Effect::Place {
                window: id,
                rect: w.geometry(),
            });
            out.push(Effect::StartAnimation { window: id, kind });
        }
    }

    fn map_window(&mut self, p: MapParams, now: f64, out: &mut Vec<Effect>) {
        if self.windows.contains_key(&p.window) || self.outputs.is_empty() {
            return;
        }
        let parent = p.parent.filter(|id| self.windows.contains_key(id));
        let mut output = p
            .output
            .filter(|o| self.output(*o).is_some())
            .unwrap_or(self.outputs[0].id);
        let app = self.ensure_app(&p.app, out);
        let mut geometry = p.geometry;
        let mut kind = p.kind;
        if let Some(par) = parent {
            let pw = &self.windows[&par];
            output = pw.output;
            let pf = pw.geometry();
            if p.modal && self.cfg.focus.place_modal_children_as_sheets {
                kind = WindowKind::Sheet;
                geometry = kind::sheet_frame(pf, geometry.size(), self.cfg.snap.titlebar_height);
            } else {
                let usable = self
                    .output(output)
                    .expect("a window's output exists while outputs do")
                    .usable;
                geometry = kind::child_frame(pf, geometry.size(), usable);
            }
        } else if kind == WindowKind::AppModal {
            let o = *self.output(output).expect("the output was checked above");
            let previous = self
                .stack
                .iter()
                .rev()
                .filter_map(|id| self.windows.get(id))
                .find(|w| {
                    w.app == app
                        && w.parent.is_none()
                        && w.lifecycle.kind() == WindowKind::AppModal
                        && !w.lifecycle.state().is_dead()
                })
                .map(|w| (w.geometry().x, w.geometry().y));
            geometry = kind::modal_panel_frame(o.usable, o.rect, geometry.size(), previous);
        }
        let current = self.workspaces[&output].current;
        let (workspace, sticky, from_app) = match parent {
            Some(par) => (
                self.windows[&par].workspace,
                self.windows[&par].sticky,
                false,
            ),
            None => match self.apps[&app].assignment {
                crate::window::Assignment::AllWorkspaces => (current, true, true),
                crate::window::Assignment::Workspace(ws)
                    if self.workspaces[&output].list.iter().any(|w| w.id == ws) =>
                {
                    (ws, false, false)
                }
                _ => (current, false, false),
            },
        };
        let level = WindowLevel::for_kind(kind, parent.map(|par| self.windows[&par].level));
        let mut lifecycle = Lifecycle::new(self.cfg.animation.clone(), kind, geometry);
        lifecycle.map(now);
        self.windows.insert(
            p.window,
            Window {
                id: p.window,
                app,
                title: p.title,
                min_size: p.min_size,
                max_size: p.max_size,
                content_hint: None,
                parent,
                modal: p.modal,
                output,
                workspace,
                sticky,
                sticky_from_app: from_app,
                level,
                lifecycle,
                disabled: p.disabled,
            },
        );
        self.insert_in_stack(p.window);
        self.apps.get_mut(&app).unwrap().windows_mru.push(p.window);
        self.emit_layout(p.window, AnimationKind::Opening, out);
        let takes_focus = match p.activation {
            Activation::Always => true,
            Activation::Policy(facts) => self.takes_focus_on_map(app, parent, &facts, now),
        };
        if takes_focus {
            self.focus_window(p.window, now, out);
        } else {
            self.place_behind_key_window(p.window);
            self.demand_attention(p.window, out);
        }
    }

    fn set_parent(&mut self, window: WindowId, parent: Option<WindowId>) {
        if let Some(p) = parent {
            let mut cur = Some(p);
            let mut steps = 0;
            while let Some(c) = cur {
                if c == window || steps > self.windows.len() {
                    return;
                }
                cur = self.windows.get(&c).and_then(|w| w.parent);
                steps += 1;
            }
            if !self.windows.contains_key(&p) {
                return;
            }
        }
        if let Some(w) = self.windows.get_mut(&window) {
            w.parent = parent;
        }
    }

    /// Put `id` on top of the windows of its level, below any of a higher level.
    fn insert_in_stack(&mut self, id: WindowId) {
        self.stack.retain(|w| *w != id);
        let level = self
            .windows
            .get(&id)
            .map_or(WindowLevel::Normal, |w| w.level);
        let at = self
            .stack
            .iter()
            .rposition(|w| self.windows.get(w).is_some_and(|x| x.level <= level))
            .map_or(0, |i| i + 1);
        self.stack.insert(at, id);
    }

    pub(crate) fn raise(&mut self, id: WindowId, out: &mut Vec<Effect>) {
        self.insert_in_stack(id);
        out.push(Effect::Raise { window: id });
        let mut children: Vec<WindowId> = self
            .stack
            .iter()
            .copied()
            .filter(|c| self.windows.get(c).and_then(|w| w.parent) == Some(id))
            .collect();
        children.retain(|c| *c != id);
        for c in children {
            self.raise(c, out);
        }
    }

    fn close_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if !w.lifecycle.close(now) {
            return;
        }
        let to = w.lifecycle.state();
        out.push(Effect::StartAnimation {
            window: id,
            kind: AnimationKind::Close,
        });
        self.cancel_interaction_for(id, now, out);
        self.release_window(id, now, out);
        if to == LifecycleState::Destroyed {
            self.finalize_window(id, now, out);
        }
    }

    pub(crate) fn destroy_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        w.lifecycle.destroy();
        self.cancel_interaction_for(id, now, out);
        self.release_window(id, now, out);
        self.finalize_window(id, now, out);
    }

    /// Take a closing window out of focus, MRU and full-screen bookkeeping (idempotent).
    fn release_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        let app = w.app;
        if self.fullscreen_workspace_of(id).is_some() {
            self.leave_fullscreen_workspace(id, now, out);
        }
        if let Some(a) = self.apps.get_mut(&app) {
            a.windows_mru.retain(|x| *x != id);
        }
        self.release_focus(id, now, out);
        self.switcher_forget(app, out);
    }

    pub(crate) fn finalize_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        if !self.windows.contains_key(&id) {
            return;
        }
        self.release_window(id, now, out);
        let w = self.windows.remove(&id).unwrap();
        self.stack.retain(|x| *x != id);
        for c in self.windows.values_mut() {
            if c.parent == Some(id) {
                c.parent = w.parent;
            }
        }
        out.push(Effect::WindowDestroyed { window: id });
    }

    pub(crate) fn default_minimize_target(&self, output: OutputId) -> Rect {
        let r = self.output(output).map_or(Rect::default(), |o| o.rect);
        Rect::new(r.cx() - 24.0, r.bottom() - 48.0, 48.0, 48.0)
    }

    fn minimize_window(
        &mut self,
        id: WindowId,
        target: Option<Rect>,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if !w.can_minimize() {
            return;
        }
        let target = target.unwrap_or_else(|| self.default_minimize_target(w.output));
        let w = self.windows.get_mut(&id).unwrap();
        if !w.lifecycle.minimize(target, now) {
            return;
        }
        out.push(Effect::StartAnimation {
            window: id,
            kind: AnimationKind::Minimize,
        });
        self.cancel_interaction_for(id, now, out);
        self.release_focus(id, now, out);
    }

    pub(crate) fn restore_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if w.lifecycle.restore(now) {
            out.push(Effect::StartAnimation {
                window: id,
                kind: AnimationKind::Restore,
            });
        }
    }

    /// A tiled, filled or zoomed window goes back to its frame; any other window is minimized.
    fn minimize_or_restore(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        match w.state() {
            LifecycleState::Tiling(_) | LifecycleState::Tiled(_) => {
                self.untile_window(id, now, out)
            }
            LifecycleState::Zooming | LifecycleState::Zoomed => self.toggle_zoom(id, now, out),
            _ => self.minimize_window(id, None, now, out),
        }
    }

    fn toggle_zoom(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if matches!(w.state(), LifecycleState::Zooming | LifecycleState::Zoomed) {
            if self.windows.get_mut(&id).unwrap().lifecycle.unzoom(now) {
                self.emit_layout(id, AnimationKind::Unzoom, out);
            }
            return;
        }
        // unzooming above stays allowed, so disabling zoom cannot trap a zoomed window
        if !w.can_zoom() {
            return;
        }
        let Some(usable) = self.output(w.output).map(|o| o.usable) else {
            return;
        };
        let current = w.lifecycle.presentation(now).frame;
        let target = best_fit(&self.cfg.zoom, usable, current, &w.hints());
        if self
            .windows
            .get_mut(&id)
            .unwrap()
            .lifecycle
            .zoom(target, now)
        {
            self.emit_layout(id, AnimationKind::Zoom, out);
        }
    }

    fn fullscreen_window(
        &mut self,
        id: WindowId,
        output: Option<OutputId>,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        let output = output
            .filter(|o| self.output(*o).is_some())
            .unwrap_or(w.output);
        let Some(rect) = self.output(output).map(|o| o.rect) else {
            return;
        };
        let w = self.windows.get_mut(&id).unwrap();
        if !w.lifecycle.fullscreen(rect, now) {
            return;
        }
        w.sticky = false;
        w.sticky_from_app = false;
        if w.output != output {
            w.output = output;
            w.workspace = self.workspaces[&output].current;
        }
        self.enter_fullscreen_workspace(id, output, now, out);
        self.emit_layout(id, AnimationKind::EnterFullScreen, out);
        self.focus_window(id, now, out);
    }

    fn exit_fullscreen_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if w.lifecycle.exit_fullscreen(now) {
            self.leave_fullscreen_workspace(id, now, out);
            self.emit_layout(id, AnimationKind::ExitFullScreen, out);
        }
    }

    pub(crate) fn tile_window(
        &mut self,
        id: WindowId,
        kind: TileKind,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        let Some(usable) = self.output(w.output).map(|o| o.usable) else {
            return;
        };
        let rect = tile_rect(kind, usable, self.cfg.snap.tiled_margin);
        if self
            .windows
            .get_mut(&id)
            .unwrap()
            .lifecycle
            .tile(kind, rect, now)
        {
            self.emit_layout(id, AnimationKind::Tile(kind), out);
        }
    }

    fn untile_window(&mut self, id: WindowId, now: f64, out: &mut Vec<Effect>) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if w.lifecycle.drag_out(None, now) {
            self.emit_layout(id, AnimationKind::DragOut, out);
        }
    }
}
