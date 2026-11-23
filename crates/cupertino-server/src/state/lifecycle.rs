use super::*;

impl Cupertino {
    // ---- model effects ---------------------------------------------------------------------

    pub fn command(&mut self, cmd: Command) {
        // the model may change a window's buttons: its title, its state, what it can do
        self.button_access.invalidate();
        let now = self.now();
        // a minimize without a target (key binding, action) flies into the dock's slot
        let cmd = match cmd {
            Command::Minimize {
                window,
                target: None,
            } => Command::Minimize {
                window,
                target: self.minimize_target(window),
            },
            other => other,
        };
        let fx = self.wm.handle(cmd, now);
        self.apply_effects(fx);
    }

    pub fn commands(&mut self, cmds: Vec<Command>) {
        for c in cmds {
            self.command(c);
        }
    }

    /// Advance the model to now and apply the resulting effects.
    pub fn tick(&mut self) {
        let _tick = tracing::trace_span!("server.tick").entered();
        self.flush_publications();
        self.refresh_control();
        self.forget_gone_processes();
        let wall_now = std::time::Instant::now();
        self.tick_hosts(wall_now);
        self.tick_transactions(wall_now);
        let now = self.now();
        self.tick_session_lock();
        self.idle.refresh_inhibition();
        self.tick_display_power();
        self.tick_quit();
        crate::acquire_waits::sweep(&mut self.acquire_registry, &self.loop_handle);
        self.tick_hung(now);
        self.tick_force_quit(now);
        let fx = self.wm.tick(now);
        self.apply_effects(fx);
        self.send_deferred_configures(now);
        // a drag that ended elsewhere than at a button (the session locked, the source went away)
        self.drag_ended();
        // the cursor, a drag icon and an input method's popup follow the pointer and the text cursor
        self.refresh_loose_surface_outputs();
        if self.fx.tick(now) {
            // an effect that ended leaves the screen; its last frame may still show it
            self.request_redraw();
        }
        self.tick_window_managers(now);
        self.tick_menu_bars(now);
        self.tick_input_timers(now);
        self.tick_overlays();
        self.tick_layers(now);
        // after the layers' finished: a transaction whose motion came to rest ends, and says so after them
        self.end_transactions_at_rest(wall_now);
        if self.popup_linger.tick(now) {
            // a dismissed popup reached its limit: the next frame no longer draws it
            self.request_redraw();
        }
        self.control_release_observations(now);
        self.sync_button_access();
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_shortcut_inhibitors(focus.as_ref());
        self.refresh_pointer_constraints(focus.as_ref());
        // windows without a close transform are skipped by `refresh_snapshot`
        let missing: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|e| e.mapped && self.fx.snapshot_age(e.id, now).is_infinite())
            .map(|e| e.id)
            .collect();
        for id in missing {
            self.refresh_snapshot(id);
        }
    }

    /// The earliest model time at which [`Cupertino::tick`] has time-driven work that no event and no frame brings
    /// about: a ping or the pong timeout, a promised SIGKILL, the idle countdowns, the end of the wait for lock
    /// surfaces, a hot corner's dwell, the menu bar edge's reveal or hide, a stale menu edit, and the keyboard filters'
    /// repeat, slow keys and mouse keys, pending native-control action replies, and the limit of a dismissed popup still
    /// drawn. Animation is not here: it draws frames ([`Cupertino::animating`]).
    pub fn next_tick_deadline(&self) -> Option<f64> {
        let filters = &self.key_filters;
        let ms = |t: u64| t as f64 / 1000.0;
        [
            self.control_deadline(),
            self.transaction_deadline(),
            self.hung.next_deadline(),
            self.force_quits.next_deadline(),
            self.display_power_deadline(),
            self.lock_wait_deadline(),
            self.hot_corners.next_deadline(),
            self.bar_edge.next_deadline(),
            self.menu_bars.set.next_deadline(),
            self.menu_bars.edit_deadline(),
            self.popup_linger.next_deadline(),
            filters.slow.as_ref().and_then(|s| s.deadline()).map(ms),
            filters.mouse.deadline().map(ms),
            filters.repeat.next_deadline().map(ms),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }

    /// Whether something animates: every output then draws a frame per its own refresh.
    pub fn animating(&self) -> bool {
        let now = self.now();
        self.wm.wants_tick(now)
            || self.fx.wants_tick(now)
            || self.window_managers.wants_tick(now)
            || self.menu_bars_want_tick(now)
            || self.overlays.wants_tick()
            || self.power.policy().animating(now)
            || self.layers.wants_tick(now)
    }

    pub fn wants_frames(&self) -> bool {
        self.animating() || self.outputs.iter().any(|o| o.scheduler.damaged())
    }

    pub fn apply_effects(&mut self, effects: Vec<Effect>) {
        let mut redraw = false;
        for fx in effects {
            redraw = true;
            match fx {
                Effect::Configure {
                    window,
                    size,
                    states,
                } => {
                    let activated = self.activated_window() == Some(window);
                    let now = self.now();
                    let (pos, animating) = self
                        .wm
                        .window(window)
                        .map(|w| (w.geometry(), w.lifecycle.is_animating(now)))
                        .unwrap_or_default();
                    if let Some(e) = self.windows.iter_mut().find(|e| e.id == window) {
                        let frame = Rect::new(pos.x, pos.y, size.w, size.h);
                        let (w, h) = e.committed_size();
                        if animating && e.mapped && (frame.w < w || frame.h < h) {
                            let covering =
                                Rect::new(frame.x, frame.y, frame.w.max(w), frame.h.max(h));
                            e.configure(covering, &states, activated);
                            e.shrink = Shrink::Deferred { frame, states };
                        } else {
                            e.configure(frame, &states, activated);
                            e.shrink = Shrink::None;
                        }
                    }
                }
                // a window that moved is told its outputs and scale below
                Effect::Place { .. } => {}
                Effect::RequestClose { window } => {
                    if let Some(e) = self.entry(window) {
                        e.close();
                    }
                }
                Effect::Focus { window } => {
                    // an exclusive layer surface keeps the keyboard; the key window gets it when
                    // the layer lets go (`layer_keyboard_exclusivity`)
                    if self.exclusive_layer.is_none() {
                        let surface = window
                            .and_then(|w| self.entry(w))
                            .map(|e| e.surface().clone());
                        self.set_keyboard_focus(surface);
                    }
                    // focus moved elsewhere: the accent popup loses its reason to stay open
                    self.accent_dismiss();
                    self.refresh_activation();
                    if let Some(window) = window {
                        self.recheck_menu_owners(window);
                        self.follow_focus_to_output(window);
                    }
                }
                Effect::WindowDestroyed { window } => {
                    self.transactions.notify_window_closed(window);
                    if let Some(h) = self.entry(window).and_then(|e| e.foreign.clone()) {
                        h.send_closed();
                    }
                }
                Effect::SwitcherChanged { apps, selected } => {
                    let names = apps
                        .iter()
                        .filter_map(|a| self.wm.app(*a))
                        .map(|a| a.identity.clone())
                        .collect();
                    self.switcher_changed(names, selected);
                }
                Effect::AppAttention { app } => {
                    if let Some(a) = self.wm.app(app) {
                        self.docks.attention(&a.identity);
                    }
                }
                Effect::SwitcherClosed { committed } => self.docks.switcher_closed(committed),
                Effect::WorkspaceRemoved { workspace, .. } => {
                    self.transactions.notify_workspace_removed(workspace);
                }
                Effect::WindowStateChanged { window, from, to } => {
                    if from == cupertino_wm::LifecycleState::Minimized
                        && to != cupertino_wm::LifecycleState::Minimized
                    {
                        self.transactions.notify_window_unminimized(window);
                    }
                }
                Effect::Activate { .. }
                | Effect::AppRegistered { .. }
                | Effect::Attention { .. }
                | Effect::Raise { .. }
                | Effect::SetVisible { .. }
                | Effect::StartAnimation { .. }
                | Effect::SnapPreview { .. }
                | Effect::WorkspaceSwitched { .. }
                | Effect::WorkspaceCreated { .. }
                | Effect::WorkspacesReordered { .. } => {}
            }
        }
        // the outputs each window is on and the scale it draws at follow its frame (`displays::surface_output`)
        self.refresh_window_outputs(false);
        if redraw {
            self.suspension.invalidate();
        }
        // a window nobody sees is told so (`suspended`)
        self.refresh_suspended();
        if redraw {
            self.request_redraw();
            self.refresh_overview_v1();
            self.refresh_menu_bars();
            self.refresh_docks();
            self.refresh_window_managers();
        }
    }

    /// The toplevel whose `activated` state is on: the model's key window, and none while the session is locked or about to
    /// be, so no application behaves as the one in front behind the lock screen.
    pub fn activated_window(&self) -> Option<WindowId> {
        self.wm
            .key_window()
            .filter(|_| !self.session_lock.is_active())
    }

    /// Make every toplevel's `activated` state match [`Self::activated_window`].
    pub fn refresh_activation(&mut self) {
        let key = self.activated_window();
        for e in &self.windows {
            e.set_activated(Some(e.id) == key);
        }
    }

    /// Give the keyboard to `surface`. While the session is locked only a lock surface gets it, and while the
    /// authentication prompt is shown only the prompt. A popup grab whose chain `surface` is outside of ends first, its
    /// menus dismissed ([`Cupertino::end_popup_grab_for_focus`]).
    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        if surface
            .as_ref()
            .and_then(|s| s.client())
            .is_some_and(|c| self.hosts.is_client_unresponsive(&c))
        {
            return;
        }
        if surface
            .as_ref()
            .is_some_and(|s| self.control_sensitive_focus(s))
        {
            self.suspend_controls(crate::control::PauseReason::ProtectedFocus);
        }
        let allowed = if self.session_lock.is_active() {
            surface.as_ref().is_some_and(|s| self.is_lock_surface(s))
        } else {
            !self.prompt_shown()
                || surface
                    .as_ref()
                    .is_some_and(crate::authorization::is_prompt)
        };
        if !allowed {
            return;
        }
        self.end_popup_grab_for_focus(surface.as_ref());
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    /// Give the keyboard back to the key window.
    pub fn restore_keyboard_focus(&mut self) {
        let surface = self
            .wm
            .key_window()
            .and_then(|w| self.entry(w))
            .map(|e| e.surface().clone());
        self.set_keyboard_focus(surface);
    }

    /// A layer surface above the windows that asks for exclusive keyboard interactivity takes the
    /// keyboard; when it stops asking or goes away the key window gets it back. A dock bar or menu bar a press outside
    /// it released ([`Cupertino::release_engaged_layer`]) is not given it again until it stops asking.
    ///
    /// While the session is locked only a shell process can take it: an application's layer surface must not collect
    /// what is typed at the lock screen, nor wait to take the keyboard at the unlock. The authentication prompt keeps its
    /// claim under the lock, which holds the keyboard, and gets it back at the unlock. While the authentication prompt is
    /// shown only the prompt can take it, and an open menu of another surface is closed as it does.
    pub fn layer_keyboard_exclusivity(&mut self, surface: &WlSurface, exclusive: bool) {
        if exclusive && self.control_sensitive_focus(surface) {
            self.suspend_controls(crate::control::PauseReason::ProtectedFocus);
        }
        if !exclusive && self.released_layer.as_ref() == Some(surface) {
            // the layer let go after a press outside it: its next claim is a new one
            self.released_layer = None;
        }
        let prompt = crate::authorization::is_prompt(surface);
        let exclusive = exclusive
            && self.released_layer.as_ref() != Some(surface)
            && (!self.session_lock.is_active()
                || prompt
                || surface
                    .client()
                    .is_some_and(|c| crate::authorization::capabilities(&c).holds_shell_role()))
            && (prompt || !self.prompt_shown());
        let holds = self.exclusive_layer.as_ref() == Some(surface);
        if exclusive && !holds {
            self.exclusive_layer = Some(surface.clone());
            self.set_keyboard_focus(Some(surface.clone()));
        } else if !exclusive && holds {
            self.exclusive_layer = None;
            // another surface may have taken the focus since: leave it there
            if self.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(surface) {
                self.restore_keyboard_focus();
            }
        }
    }

    /// Whether `surface` is a layer surface of the session agent's panels ([`SESSION_PANEL_NAMESPACE`]).
    pub fn is_session_panel(&self, surface: &WlSurface) -> bool {
        self.outputs.iter().any(|o| {
            layer_map_for_output(&o.output)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_some_and(|l| l.namespace() == SESSION_PANEL_NAMESPACE)
        })
    }

    /// A press at `pos` outside the dock bar or menu bar that holds the keyboard exclusively, which it asks for only
    /// while engaged (Focus Dock, the menu bar's keyboard mode), or outside a session panel, which asks for it when it
    /// appears ([`SESSION_PANEL_NAMESPACE`]), ends that: the layer loses the keyboard, which goes back to the key window
    /// (and to the window pressed), and the layer answers the leave by letting go. A press on the layer or on one of its
    /// popups keeps it. Other exclusive layers (the authentication prompt, the launcher, the overview) are left as they
    /// are, and the lock screen never gets here.
    ///
    /// A menu open from the layer closes first, so the keyboard is back on the layer when it leaves and the layer is
    /// told; the press is then consumed, as any press that closes a menu is. True when it was.
    pub fn release_engaged_layer(
        &mut self,
        pos: Point<f64, Logical>,
        serial: Serial,
        time: u32,
    ) -> bool {
        let Some(layer) = self.exclusive_layer.clone().filter(|l| {
            self.docks.has_surface(l) || self.menu_bars.has_surface(l) || self.is_session_panel(l)
        }) else {
            return false;
        };
        if self
            .surface_under(pos)
            .is_some_and(|(surface, _)| self.shell_surface_root(&surface) == layer)
        {
            return false;
        }
        let menus = self.popup_grab_root() == Some(&layer)
            && self.dismiss_popup_grab_for_press(pos, serial, time);
        self.exclusive_layer = None;
        self.released_layer = Some(layer);
        self.restore_keyboard_focus();
        menus
    }

    /// The surface whose tree `surface` is in: the parent of a subsurface, the surface a popup chain grows from.
    pub(crate) fn shell_surface_root(&self, surface: &WlSurface) -> WlSurface {
        let top = |s: &WlSurface| {
            let mut top = s.clone();
            while let Some(parent) = smithay::wayland::compositor::get_parent(&top) {
                top = parent;
            }
            top
        };
        let surface = top(surface);
        self.popups
            .find_popup(&surface)
            .and_then(|p| smithay::desktop::find_popup_root_surface(&p).ok())
            .map_or(surface, |root| top(&root))
    }

    // ---- actions ---------------------------------------------------------------------------

    pub fn action_context(&self) -> ActionContext {
        let output = self.active_output();
        let (workspaces, current) = output
            .map(|o| {
                let list: Vec<_> = self.wm.workspaces(o).iter().map(|w| w.id).collect();
                let cur = self
                    .wm
                    .current_workspace(o)
                    .and_then(|c| list.iter().position(|w| *w == c))
                    .unwrap_or(0);
                (list, cur)
            })
            .unwrap_or_default();
        let front_app = self.wm.front_app();
        let front_app_windows = front_app
            .and_then(|a| self.wm.app(a))
            .map(|a| a.windows_mru.clone())
            .unwrap_or_default();
        ActionContext {
            key_window: self.wm.key_window(),
            front_app,
            front_app_windows,
            output,
            switcher_open: self.wm.switcher().is_some(),
            workspaces,
            current_workspace: current,
            locked: self.session_lock.is_active(),
            input_held: self.shortcut_inhibit.holding(),
            pointer_held: self.pointer_held(),
            key_window_fullscreen: self
                .wm
                .key_window()
                .and_then(|w| self.wm.window(w))
                .is_some_and(|w| w.state().is_fullscreen()),
            close_goes_to_app: self.key_window_has_keyboard()
                && self
                    .config
                    .keyboard
                    .close_goes_to_app(&self.key_app_id(), self.key_client_class()),
        }
    }

    /// Run what a key, a gesture or a menu asked for: the keyboard focus it gives is the person's own input.
    pub fn execute_plan(&mut self, plan: ActionPlan) {
        self.as_input(|s| s.run_plan(plan));
    }

    fn run_plan(&mut self, plan: ActionPlan) {
        self.commands(plan.commands);
        for op in plan.ops {
            match op {
                ShellOp::ToggleOverview => self.toggle_overview(false),
                ShellOp::ToggleAppOverview => self.toggle_overview(true),
                ShellOp::SpawnTerminal => self.spawn_terminal(),
                ShellOp::Screenshot(request) => self.start_screenshot(request),
                ShellOp::Quit => self.quit_compositor(),
                ShellOp::SwitchVt(n) => {
                    if let Some(b) = self.backend.as_mut() {
                        b.switch_vt(n);
                    }
                }
                ShellOp::ToggleShowDesktop => {
                    let now = self.now();
                    self.fx.toggle_show_desktop(now);
                    self.request_redraw();
                }
                ShellOp::ToggleAppGrid => self.toggle_applications(),
                ShellOp::OpenForceQuit => self.docks.force_quit_dialog(),
                ShellOp::ToggleLauncher => self.toggle_launcher(),
                ShellOp::ToggleNotificationCenter => self.toggle_notification_center(),
                ShellOp::ToggleControlPanel => self.toggle_control_panel(),
                ShellOp::ToggleAppearance => self.toggle_appearance(),
                ShellOp::ToggleDisplayOption(option) => self.toggle_display_option(option),
                ShellOp::Osd { key, fine } => self.osd_key(key, fine),
                ShellOp::InputSource { menu, step } => self.input_source_step(menu, step),
                ShellOp::FocusMenuBar => self.focus_menu_bar(),
                ShellOp::HelpSearch => self.focus_help_search(),
                ShellOp::FocusDock => self.focus_dock(),
                ShellOp::ToggleDockAutoHide => self.docks.toggle_auto_hide(),
                ShellOp::ToggleStage => self.window_managers.toggle_stage(),
                ShellOp::Exec(i) => {
                    if let Some(cmd) = self.key_router.bindings.exec_command(i).map(str::to_string)
                    {
                        self.spawn_command_line(&cmd);
                    }
                }
                ShellOp::Lock => self.lock_session_now(),
                ShellOp::LogOut => self.log_out_session(),
                ShellOp::LogOutNow => self.log_out_session_now(),
                ShellOp::PowerDialog => self.power_dialog(),
                ShellOp::ReleaseShortcuts => {
                    self.release_shortcuts();
                }
                ShellOp::PowerKey { pressed } => self.power_key(pressed),
                ShellOp::GlobalShortcut { key, pressed } => self.global_shortcut(key, pressed),
            }
        }
    }

    /// End the server: the event loop stops after this iteration.
    pub fn quit_compositor(&mut self) {
        self.running = false;
        self.loop_signal.stop();
    }

    pub fn spawn_terminal(&mut self) {
        let candidates = self.config.terminals.clone();
        match cupertino_defaults::terminal::resolve_installed(&candidates) {
            Some(terminal) => self.spawn(&terminal, &[]),
            None => tracing::warn!(?candidates, "no terminal emulator found"),
        }
    }

    /// Start a client on this compositor's socket. The application receives an activation token
    /// so its first window takes focus, and the docks learn which application is launching when
    /// the program has a desktop entry.
    pub fn spawn(&mut self, program: &str, args: &[&str]) {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        let token = self.server_activation_token();
        cmd.env("XDG_ACTIVATION_TOKEN", &token)
            .env("DESKTOP_STARTUP_ID", &token);
        self.spawn_prepared(cmd);
    }

    /// Start a prepared command on this compositor's socket.
    pub fn spawn_prepared(&mut self, mut cmd: std::process::Command) {
        if self.login_screen {
            tracing::warn!(program = %cmd.get_program().to_string_lossy(), "the login screen starts no programs");
            return;
        }
        if let Some(s) = &self.socket_name {
            cmd.env("WAYLAND_DISPLAY", s);
        }
        cmd.env("XDG_SESSION_TYPE", "wayland")
            .env("XDG_CURRENT_DESKTOP", CURRENT_DESKTOP);
        // clients that draw their own cursors draw them in the asset theme (the only one on their path) at the user cursor
        // size; without a theme they keep the cursor theme of their environment
        cmd.env(
            "XCURSOR_SIZE",
            self.cursor_loader
                .client_size(self.config.pointer.cursor_scale)
                .to_string(),
        );
        if let Some(theme) = self.cursor_loader.theme() {
            cmd.env("XCURSOR_THEME", &theme.name)
                .env("XCURSOR_PATH", &theme.search_dir);
        }
        crate::xwayland::set_client_env(&mut cmd, self.xwayland.as_ref().map(|x| x.display()));
        cmd.env_remove("WAYLAND_SOCKET");
        let program = cmd.get_program().to_string_lossy().into_owned();
        // the docks key tiles by desktop entry id; a program without an entry the desktop shows (a script, a shell, a
        // helper with a hidden entry) is not an application launch, and neither is a name several entries run
        let path = std::path::Path::new(&program);
        let app_id = self.desktop_lookup(|desktop| {
            desktop
                .for_program(path)
                .map(|e| desktop.shown(e))
                .filter(|e| !e.no_display)
                .map(|e| e.id.clone())
        });
        crate::file_limit::restore_in_child(&mut cmd);
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(app_id) = app_id {
                    self.docks.launching(&app_id);
                }
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => tracing::warn!(program, error = %e, "spawning failed"),
        }
    }

    /// What `look` finds in the desktop entries; when it finds nothing, the entries are read again first if they
    /// changed since they were read ([`crate::desktop::DesktopIndex::refresh`]), so an application installed or an
    /// entry written after the server started is found.
    pub(crate) fn desktop_lookup<T>(
        &mut self,
        look: impl Fn(&crate::desktop::DesktopIndex) -> Option<T>,
    ) -> Option<T> {
        look(&self.desktop).or_else(|| {
            self.desktop
                .refresh()
                .then(|| look(&self.desktop))
                .flatten()
        })
    }

    /// The desktop file ID of the application the desktop shows that `identity` names ([`Self::desktop_lookup`]).
    pub(crate) fn shown_application(&mut self, identity: &str) -> Option<String> {
        self.desktop_lookup(|desktop| {
            desktop
                .find(identity)
                .filter(|e| !e.no_display)
                .map(|e| e.id.clone())
        })
    }

    /// Run a command line (program and arguments split at whitespace).
    pub fn spawn_command_line(&mut self, line: &str) {
        let parts: Vec<String> = line.split_whitespace().map(String::from).collect();
        if let Some((prog, args)) = parts.split_first() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            self.spawn(prog, &args);
        }
    }

    pub fn app_identity_of(&self, app: AppId) -> Option<String> {
        self.wm.app(app).map(|a| a.identity.clone())
    }

    /// Serial helper.
    pub fn next_serial(&self) -> Serial {
        SERIAL_COUNTER.next_serial()
    }

    pub fn rect_to_logical(r: Rect) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (r.x.round() as i32, r.y.round() as i32).into(),
            (r.w.round() as i32, r.h.round() as i32).into(),
        )
    }

    /// The primary output's scale. A surface is told its own output's (`Cupertino::surface_scale`).
    pub fn preferred_scale(&self) -> f64 {
        self.outputs.first().map_or(1.0, |o| o.scale)
    }
}
