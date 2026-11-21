//! Shell behaviour on the compositor state: the ways the person asks for the overview, which go to the overview
//! component of the output (`overview_v1.rs`), the dock's intents (workspaces, named actions, tile commands), and the
//! hot corners.

use cupertino_protocols::server::cupertino_dock_v1::{AppCommand, Corner};
use cupertino_protocols::server::overview::cupertino_overview_v1::Mode as OverviewMode;
use cupertino_wm::{Command, OutputId, WindowId, WorkspaceId};
use smithay::utils::{Logical, Point};

use crate::input::{translate_action, Action};
use crate::state::Cupertino;

impl Cupertino {
    /// The overview shortcut, its named action and gesture: the overview component of the active output opens or leaves
    /// (`app_mode`: the front application's windows, all of them when no application is in front).
    pub fn toggle_overview(&mut self, app_mode: bool) {
        let Some(output) = self.active_output() else {
            return;
        };
        let app = app_mode
            .then(|| self.wm.front_app())
            .flatten()
            .and_then(|app| self.app_identity_of(app))
            .filter(|identity| !identity.is_empty());
        match app {
            Some(app_id) => self.ask_overview(output, OverviewMode::App, &app_id),
            None => self.ask_overview(output, OverviewMode::All, ""),
        }
    }

    /// Ask the overview component of `output` to open in `mode` (`app_id` the application of the app mode, empty for
    /// all), or to leave when it is open: the component decides. Nothing without a component on that output.
    pub fn ask_overview(&self, output: OutputId, mode: OverviewMode, app_id: &str) {
        if !self.toggle_overview_v1(output, mode, app_id) {
            tracing::debug!(
                output = output.0,
                "no overview component on the output to open the overview"
            );
        }
    }

    /// `switch_workspace`: go to the workspace.
    pub fn dock_switch_workspace(&mut self, output: OutputId, id: u32) {
        let workspace = WorkspaceId(u64::from(id));
        if !self.wm.workspaces(output).iter().any(|w| w.id == workspace) {
            return;
        }
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
                let app_id = self
                    .app_identity_of(app)
                    .unwrap_or_else(|| app_id.to_owned());
                self.ask_overview(output, OverviewMode::App, &app_id);
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
