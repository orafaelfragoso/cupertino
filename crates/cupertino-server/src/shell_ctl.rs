//! Shell behaviour on the compositor state: opening and closing the overview, the dock's intents
//! for it (target frames, selection, workspaces, named actions, tile commands), the server-side
//! fallbacks that work without a dock (click, Escape, workspace switch), and the hot corners.

use cupertino_overview::{OverviewConfig, OverviewMode};
use cupertino_protocols::server::cupertino_dock_v1::{AppCommand, Corner};
use cupertino_wm::{Command, OutputId, Point as MPoint, WindowId, WorkspaceId};
use smithay::utils::{Logical, Point};

use crate::input::{translate_action, Action};
use crate::shell::{self, OverviewSession};
use crate::state::Cupertino;

impl Cupertino {
    /// The overview shortcut: open on the active output (`app_mode`: the front application's
    /// windows), or leave an open one.
    pub fn toggle_overview(&mut self, app_mode: bool) {
        if self.shell.overview_open() {
            self.close_overview();
            return;
        }
        let Some(output) = self.active_output() else {
            return;
        };
        let mode = match (app_mode, self.wm.front_app()) {
            (true, Some(app)) => OverviewMode::AppOverview(app),
            _ => OverviewMode::Overview,
        };
        self.open_overview(output, mode);
    }

    /// Open the overview on `output`; a closing one is replaced.
    pub fn open_overview(&mut self, output: OutputId, mode: OverviewMode) {
        if self.shell.overview_open() {
            return;
        }
        let Some(rect) = self.output_entry(output).map(|o| o.rect) else {
            return;
        };
        let now = self.now();
        // the dock lays the windows out from the output's top (the menu bar gives way) down to the usable area's
        // bottom (clear of the dock)
        let usable = self.wm.output(output).map(|o| o.usable).unwrap_or(rect);
        let area = shell::overview_area(rect, usable);
        let windows = shell::overview_windows(&self.wm, output, now, self.dock_rect(output));
        let mut session =
            OverviewSession::new(&OverviewConfig::default(), output, area, mode, windows);
        session.opened_at = now;
        session.reduce_motion = self.display_options().reduce_motion;
        session.transition.toggle(true, now);
        tracing::info!(
            output = output.0,
            x = area.x,
            y = area.y,
            width = area.w,
            height = area.h,
            windows = session.windows.len(),
            "overview opened"
        );
        self.shell.overview = Some(session);
        self.send_overview_to_docks();
        self.request_redraw();
    }

    /// Leave the overview: the windows glide back to their desktop frames, the docks drop their chrome.
    pub fn close_overview(&mut self) {
        let now = self.now();
        if let Some(s) = self.shell.overview.as_mut().filter(|s| !s.closing) {
            s.close(now);
            let output = s.output;
            tracing::info!(output = output.0, "overview closed");
            self.docks.overview_closed(output);
        }
        self.request_redraw();
    }

    /// Leave the overview activating `window` (a minimized one is restored). False when the window
    /// is not in the overview.
    pub fn overview_select(&mut self, window: WindowId) -> bool {
        if !self
            .shell
            .overview
            .as_ref()
            .is_some_and(|s| !s.closing && s.contains(window))
        {
            return false;
        }
        let minimized = self
            .wm
            .window(window)
            .is_some_and(|w| w.state().is_minimized());
        self.close_overview();
        self.as_input(|s| {
            s.command(if minimized {
                Command::Restore { window }
            } else {
                Command::FocusWindow { window }
            })
        });
        true
    }

    /// A press while the overview is open that no shell surface took: the window under the pointer
    /// is selected, an empty spot leaves. Works without a dock. False when no overview is open.
    pub fn overview_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let now = self.now();
        let Some(s) = self.shell.overview.as_ref().filter(|s| !s.closing) else {
            return false;
        };
        let hit = s.hit(MPoint::new(pos.x, pos.y), self.wm.stacking_order(), now);
        match hit {
            Some(window) => self.overview_select(window),
            None => {
                self.close_overview();
                true
            }
        }
    }

    /// `overview_frame`: the dock laid a window out; it glides there.
    pub fn dock_overview_frame(&mut self, output: OutputId, id: u32, rect: cupertino_wm::Rect) {
        let now = self.now();
        let Some(s) = self
            .shell
            .overview
            .as_mut()
            .filter(|s| !s.closing && s.output == output)
        else {
            return;
        };
        if s.set_target(WindowId(u64::from(id)), rect, now) {
            tracing::info!(
                id,
                x = rect.x,
                y = rect.y,
                width = rect.w,
                height = rect.h,
                "overview frame"
            );
            self.request_redraw();
        }
    }

    /// `overview_drag`: the person drags a window of the overview; it is drawn where the pointer took it at once.
    pub fn dock_overview_drag(&mut self, output: OutputId, id: u32, rect: cupertino_wm::Rect) {
        let Some(s) = self
            .shell
            .overview
            .as_mut()
            .filter(|s| !s.closing && s.output == output)
        else {
            return;
        };
        if s.set_drag(WindowId(u64::from(id)), rect) {
            self.request_redraw();
        }
    }

    /// `overview_move_window`: a window of the overview was dropped on another regular workspace of its output; it
    /// moves there and leaves the overview (the window set is sent again).
    pub fn dock_overview_move_window(&mut self, output: OutputId, id: u32, workspace: u32) {
        let window = WindowId(u64::from(id));
        let workspace = WorkspaceId(u64::from(workspace));
        let in_overview = self
            .shell
            .overview_on(output)
            .is_some_and(|s| s.contains(window));
        let target = self
            .wm
            .workspaces(output)
            .iter()
            .find(|w| w.id == workspace);
        let regular = target.is_some_and(|w| w.kind == cupertino_wm::WorkspaceKind::Regular);
        if !in_overview || !regular || self.wm.current_workspace(output) == Some(workspace) {
            return;
        }
        tracing::info!(
            id,
            workspace = workspace.0,
            "overview window moved to a workspace"
        );
        self.command(Command::MoveWindowToWorkspace { window, workspace });
        self.request_redraw();
    }

    /// `overview_workspace_frame`: where the live picture of a workspace of `output` is drawn while its overview is
    /// open; an empty rectangle removes it.
    pub fn dock_overview_picture(
        &mut self,
        output: OutputId,
        workspace: u32,
        rect: cupertino_wm::Rect,
    ) {
        let workspace = WorkspaceId(u64::from(workspace));
        if !self.wm.workspaces(output).iter().any(|w| w.id == workspace) {
            return;
        }
        if let Some(s) = self
            .shell
            .overview
            .as_mut()
            .filter(|s| !s.closing && s.output == output)
        {
            s.set_picture(workspace, rect);
            self.request_redraw();
        }
    }

    /// `switch_workspace`: leave an open overview and go to the workspace.
    pub fn dock_switch_workspace(&mut self, output: OutputId, id: u32) {
        let workspace = WorkspaceId(u64::from(id));
        if !self.wm.workspaces(output).iter().any(|w| w.id == workspace) {
            return;
        }
        self.close_overview();
        self.command(Command::SwitchWorkspace { output, workspace });
    }

    /// `run_action`: a named action of the shortcut file, as its chord would run it.
    pub fn dock_run_action(&mut self, name: &str) {
        let Some(action) = Action::from_name(name) else {
            tracing::warn!(name, "dock asked for an unknown action");
            return;
        };
        let ctx = self.action_context();
        if ctx.locked {
            return;
        }
        self.execute_plan(translate_action(action, &ctx));
    }

    /// `app_command`: a tile's context menu item for the application.
    pub fn dock_app_command(&mut self, output: OutputId, app_id: &str, command: AppCommand) {
        let Some(app) = self.wm.app_for_identity(app_id) else {
            return;
        };
        match command {
            AppCommand::ShowWindows => {
                if self.shell.overview_open() {
                    self.close_overview();
                }
                self.open_overview(output, OverviewMode::AppOverview(app));
            }
            AppCommand::Hide => self.command(Command::HideApp { app }),
            AppCommand::Quit => {
                let windows: Vec<WindowId> = self
                    .wm
                    .app(app)
                    .map(|a| a.windows_mru.clone())
                    .unwrap_or_default();
                self.commands(
                    windows
                        .into_iter()
                        .map(|window| Command::RequestClose { window })
                        .collect(),
                );
            }
            // the generated enum is non-exhaustive; the dispatcher already rejected unknown values
            _ => tracing::warn!(
                ?command,
                "dock sent an app command this server does not know"
            ),
        }
    }

    fn input_rect(r: cupertino_wm::Rect) -> cupertino_input::Rect {
        cupertino_input::Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }

    /// Feed the pointer to the edge reveal (full-screen menu bar) and the hot corners.
    pub fn update_input_pointer(&mut self, pos: Point<f64, Logical>, now: f64) {
        let ip = cupertino_input::Point::new(pos.x, pos.y);
        let outputs: Vec<cupertino_input::Rect> = self
            .outputs
            .iter()
            .map(|o| Self::input_rect(o.rect))
            .collect();
        if let Some(o) = self.output_at(pos).map(|o| (o.id, o.rect)) {
            let shown = self
                .menu_bars
                .set
                .presentation(o.0, now)
                .map_or(1.0, |b| b.visible);
            // the title bar revealed below a full-screen window's bar keeps it, so the pointer can reach its buttons
            let titlebar_bottom = self
                .windows
                .iter()
                .filter(|e| self.wm.window(e.id).is_some_and(|w| w.output == o.0))
                .filter_map(|e| {
                    e.revealed_titlebar(
                        &self.menu_bars,
                        &self.decor,
                        self.config.window.decorations,
                        o.0,
                        now,
                    )
                })
                .map(|r| r.y + r.h - o.1.y)
                .fold(0.0, f64::max);
            let bar = cupertino_wm::Rect::new(
                o.1.x,
                o.1.y,
                o.1.w,
                (self.config.shell.menu_bar_height * shown.max(0.5)).max(titlebar_bottom),
            );
            self.bar_edge.set_hold_region(Some(Self::input_rect(bar)));
        }
        if let Some(ev) = self.bar_edge.update(ip, &outputs, now) {
            self.apply_bar_edge(ev, now);
        }
        let m = self.current_mods();
        let mods = cupertino_input::Modifiers::new(m.shift, m.ctrl, m.alt, m.logo);
        for ev in self.hot_corners.update(ip, mods, &outputs, now) {
            self.hot_corner_event(ev);
        }
    }

    pub fn tick_input_timers(&mut self, now: f64) {
        for ev in self.hot_corners.tick(now) {
            self.hot_corner_event(ev);
        }
    }

    /// A corner fired: the dock of that output maps it to its action. The binding's own action is
    /// only what arms the corner (`config.rs`).
    fn hot_corner_event(&mut self, ev: cupertino_input::CornerEvent) {
        let cupertino_input::CornerEvent::Triggered { corner, output, .. } = ev else {
            return;
        };
        if self.session_lock.is_active() {
            return;
        }
        let Some(output) = self.outputs.get(output).map(|o| o.id) else {
            return;
        };
        let corner = match corner {
            cupertino_input::Corner::TopLeft => Corner::TopLeft,
            cupertino_input::Corner::TopRight => Corner::TopRight,
            cupertino_input::Corner::BottomLeft => Corner::BottomLeft,
            cupertino_input::Corner::BottomRight => Corner::BottomRight,
        };
        self.docks.hot_corner(output, corner);
    }
}
