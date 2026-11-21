use super::*;

impl Cupertino {
    /// Drop the bars whose connection no longer holds the MenuBar role.
    pub(crate) fn release_revoked_menu_bars(&mut self) {
        let revoked: Vec<_> = self
            .menu_bars
            .bars
            .iter()
            .filter(|b| {
                b.resource
                    .client()
                    .is_some_and(|c| !authorized(&c, ShellRole::MenuBar))
            })
            .map(|b| (b.resource.clone(), b.output))
            .collect();
        for (resource, output) in revoked {
            self.menu_bars.bars.retain(|b| b.resource != resource);
            self.menu_bar_gone(output);
        }
    }

    /// `cupertino_shell_v1.get_menu_bar`: validate the role, register the bar and send its first group.
    pub fn create_menu_bar(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::MenuBar>>,
        id: New<CupertinoMenuBarV1>,
        surface: WlSurface,
        output: &WlOutput,
        init: &mut DataInit<'_, Self>,
    ) {
        if permit.is_none() {
            init.init(id, MenuBarData { output: None });
            refuse(shell, ShellRole::MenuBar);
            return;
        }
        let target = Output::from_resource(output)
            .and_then(|o| self.output_by_wl(&o).map(|e| (e.id, e.output.clone())));
        let rejection = match &target {
            None => Some("the output is gone".to_string()),
            Some((_, o))
                if layer_map_for_output(o)
                    .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                    .is_none() =>
            {
                Some("the surface is not a layer surface of the output".to_string())
            }
            Some(_) if self.menu_bars.has_surface(&surface) || self.docks.has_surface(&surface) => {
                Some("the surface already has a shell role".to_string())
            }
            Some((out, _)) if self.menu_bars.has_bar(*out) => {
                Some("the output already has a menu bar".to_string())
            }
            Some(_) => None,
        };
        if let Some(reason) = rejection {
            init.init(id, MenuBarData { output: None });
            shell.post_error(ShellError::Role, reason);
            return;
        }
        if shell.version() < MENU_BAR_MESSAGES_SINCE {
            tracing::warn!(
                version = shell.version(),
                since = MENU_BAR_MESSAGES_SINCE,
                "a menu bar bound below the shell version of this server's menu bar messages is left inert"
            );
            init.init(id, MenuBarData { output: None });
            return;
        }
        let (out, _) = target.expect("a rejected target returned above");
        let resource = init.init(id, MenuBarData { output: Some(out) });
        self.menu_bars.bars.push(Bar {
            resource,
            surface,
            output: out,
            sent: None,
            capture_sent: None,
            shortcuts_sent: None,
            backdrop_sent: None,
            bands_sent: None,
        });
        self.refresh_usable_area(out);
        self.refresh_menu_bars();
        self.refresh_capture_indicator();
        self.refresh_menu_bar_shortcuts();
        self.refresh_backdrop();
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
        if self.menu_bars.bars.len() == 1 {
            self.check_menu_reader();
        }
    }

    /// The layer surface of a bar went away: the role ends with it.
    pub fn menu_bar_surface_gone(&mut self, surface: &WlSurface) {
        let outputs: Vec<OutputId> = self
            .menu_bars
            .bars
            .iter()
            .filter(|b| &b.surface == surface)
            .map(|b| b.output)
            .collect();
        self.menu_bars.bars.retain(|b| &b.surface != surface);
        for output in outputs {
            self.menu_bar_gone(output);
        }
    }

    /// A bar of `output` went away: a menu it had open closes with it, so its reveal does not
    /// outlive it, and its reservation goes.
    fn menu_bar_gone(&mut self, output: OutputId) {
        let now = self.now();
        self.menu_bars.set.set_menu_open(output, false, now);
        self.refresh_usable_area(output);
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
    }

    /// Tell the windows' menu objects whether a bar shows their menus, when that changed: while any bar runs it
    /// reads the address of the active window.
    pub fn refresh_app_menus(&mut self) {
        if self.menu_bars.is_empty() {
            self.check_menu_reader();
        }
        self.menu_bars.send_shown();
    }

    /// Follow the owner of [`MENU_BAR_BUS_NAME`] on the D-Bus worker: every change checks the reader again. Call once,
    /// when the worker starts.
    pub fn watch_menu_reader(&mut self) {
        let Some(dbus) = &self.dbus else { return };
        let (changed, rx) = channel::channel::<()>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let ChannelEvent::Msg(()) = event {
                state.check_menu_reader();
            }
        });
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "cannot follow the menu bar's bus name");
            return;
        }
        // the loop is gone only while the server shuts down
        dbus.watch_owner(
            MENU_BAR_BUS_NAME,
            Box::new(move || {
                let _ = changed.send(());
            }),
        );
    }

    /// Check that the menu bar process owns [`MENU_BAR_BUS_NAME`]; with no bar there is no reader.
    fn check_menu_reader(&mut self) {
        if let Some(token) = self.menu_bars.reader_retry.take() {
            self.loop_handle.remove(token);
        }
        let client = self.menu_bars.bars.first().and_then(|b| b.surface.client());
        match client {
            Some(client) => self.check_owner(OwnerCheck {
                key: OwnerKey::MenuReader,
                name: MENU_BAR_BUS_NAME.to_owned(),
                client,
            }),
            None => {
                self.cancel_owner_check(&OwnerKey::MenuReader);
                self.menu_bars.reader_backoff = READER_RETRY_FIRST;
                self.set_menu_reader(None);
            }
        }
    }

    /// The answer to the check of the reader. A refusal for a passing reason (the identification threads are in use, or
    /// the bus is slow) keeps the reader so far and asks again after a wait that doubles; any other names no reader.
    pub(crate) fn menu_reader_checked(&mut self, verdict: Result<String, OwnerRefusal>) {
        match verdict {
            Ok(reader) => {
                self.menu_bars.reader_backoff = READER_RETRY_FIRST;
                self.set_menu_reader(Some(reader));
            }
            Err(
                refusal @ (OwnerRefusal::Busy | OwnerRefusal::TimedOut | OwnerRefusal::Stalled),
            ) => {
                let wait = self.menu_bars.reader_backoff;
                self.menu_bars.reader_backoff = (wait * 2).min(READER_RETRY_MAX);
                tracing::debug!(
                    ?refusal,
                    ?wait,
                    "the menu bar's bus name is checked again later"
                );
                let timer =
                    self.loop_handle
                        .insert_source(Timer::from_duration(wait), |_, _, state| {
                            state.menu_bars.reader_retry = None;
                            state.check_menu_reader();
                            TimeoutAction::Drop
                        });
                match timer {
                    Ok(token) => self.menu_bars.reader_retry = Some(token),
                    Err(e) => {
                        tracing::warn!(error = %e, "cannot time the next check of the menu bar's bus name")
                    }
                }
            }
            Err(refusal) => {
                tracing::info!(?refusal, "the menu bar's bus name is not the menu bar process's: the windows' menus answer no one");
                self.set_menu_reader(None);
            }
        }
    }

    /// Tell the version 3 menu objects the reader, when it changed: a new reader before `menu_bar(1)`, so a window
    /// hides its own menus only once its menus answer the bar, and `menu_bar(0)` before the reader goes. A new reader
    /// also sends every bar its group again, unchanged: a bar whose load an application refused while it had no reader
    /// loads again once the group announces the menus again.
    fn set_menu_reader(&mut self, reader: Option<String>) {
        if reader == self.menu_bars.reader {
            return;
        }
        if reader.is_none() {
            self.menu_bars.reader = None;
            self.menu_bars.send_shown();
        }
        let name = reader.clone().unwrap_or_default();
        for menu in &self.menu_bars.app_menus {
            if menu.resource.version() >= 3 {
                menu.resource.menu_reader(name.clone());
            }
        }
        if reader.is_some() {
            self.menu_bars.reader = reader;
            self.menu_bars.send_shown();
            for bar in &mut self.menu_bars.bars {
                bar.sent = None;
            }
            self.refresh_menu_bars();
        }
    }

    /// The group the bars show now: the key window and the menu it exports. `client` is what [`Self::window_process`]
    /// says of the key window's connection.
    fn menu_feed(&self, client: (u32, bool)) -> Feed {
        let window = self.wm.key_window().and_then(|id| self.wm.window(id));
        let Some(window) = window else {
            return Feed {
                app_id: String::new(),
                title: String::new(),
                menu: None,
                can_zoom: false,
                can_minimize: false,
                full_screen: false,
                workspace: 0,
                workspaces: 0,
                windows: Vec::new(),
                client: (0, false),
                keys: ClientClass::Other,
            };
        };
        let workspaces = self.wm.workspaces(window.output);
        let windows = self
            .listed_windows(window.app)
            .take(MAX_APP_WINDOWS)
            .map(|w| {
                let mut flags = AppWindowFlags::empty();
                flags.set(AppWindowFlags::Active, w.id == window.id);
                flags.set(AppWindowFlags::Minimized, w.state().is_minimized());
                AppWindow {
                    id: w.id.0 as u32,
                    title: cut_text(&w.title).to_string(),
                    flags,
                }
            })
            .collect();
        Feed {
            app_id: self
                .wm
                .app(window.app)
                .map(|a| a.identity.clone())
                .unwrap_or_default(),
            // the key window's, as the chord rewrite classes it
            keys: self.key_client_class(),
            title: window.title.clone(),
            menu: self.menu_bars.address(window.id.0).cloned(),
            can_zoom: window.can_zoom(),
            can_minimize: window.can_minimize(),
            full_screen: window.state().is_fullscreen(),
            workspace: workspaces
                .iter()
                .position(|w| w.id == window.workspace)
                .map_or(0, |i| i as u32 + 1),
            workspaces: workspaces.len() as u32,
            windows,
            client,
        }
    }

    /// The process behind the connection of window `id` (0 when unknown) and whether it is the X11 bridge, which shows
    /// the windows of every X11 program: the one the server started, or one started apart whose program only the
    /// system can have installed. Read once per connection and remembered.
    fn window_process(&mut self, id: WindowId) -> (u32, bool) {
        let Some(client) = self.entry(id).and_then(|e| e.surface().client()) else {
            return (0, false);
        };
        let clients = &mut self.menu_bars.clients;
        if let Some(i) = clients.iter().position(|(c, _)| *c == client.id()) {
            let remembered = clients.remove(i);
            let process = remembered.1;
            clients.push(remembered);
            return process;
        }
        // a process not known yet is asked for again with the next group
        let Some((pid, _)) = (self.process_of)(&self.display_handle, &client) else {
            return (0, false);
        };
        let started = self
            .xwayland
            .as_ref()
            .and_then(crate::xwayland::XWayland::pid);
        let bridge = started == Some(pid)
            || (self.executable_of)(&self.display_handle, &client)
                .is_some_and(|program| is_bridge_program(&program));
        let process = (pid, bridge);
        let clients = &mut self.menu_bars.clients;
        if clients.len() >= MAX_REMEMBERED_CLIENTS {
            clients.remove(0);
        }
        clients.push((client.id(), process));
        process
    }

    /// The windows of `app` the window list names, top of the stack first: the ones the dock lists.
    fn listed_windows(&self, app: AppId) -> impl Iterator<Item = &Window> {
        self.wm
            .stacking_order()
            .iter()
            .rev()
            .filter_map(|id| self.wm.window(*id))
            .filter(move |w| {
                w.app == app
                    && !matches!(
                        w.state(),
                        LifecycleState::Created
                            | LifecycleState::Closing
                            | LifecycleState::Destroyed
                    )
            })
    }

    /// Send the current group to every bar that has not seen it. Call after anything the group
    /// derives from may have changed: focus, titles, window state, workspaces, menu addresses.
    pub fn refresh_menu_bars(&mut self) {
        if self.menu_bars.bars.is_empty() {
            return;
        }
        let client = self
            .wm
            .key_window()
            .map_or((0, false), |id| self.window_process(id));
        let feed = self.menu_feed(client);
        // a bar before version 10 has no window list: a change of the list alone is nothing new for it
        let listless = Feed {
            windows: Vec::new(),
            ..feed.clone()
        };
        for bar in &mut self.menu_bars.bars {
            let feed = if bar.resource.version() >= 10 {
                &feed
            } else {
                &listless
            };
            if bar.sent.as_ref() != Some(feed) {
                feed.send(&bar.resource);
                bar.sent = Some(feed.clone());
            }
        }
    }

    /// The chords of the bound actions as the bindings hold them now, in the physical keys of the person. An action
    /// bound to several chords shows the one bound last: a `keybindings` entry of the configuration over the shortcut
    /// tables of the profile.
    fn menu_bar_shortcuts(&self) -> Shortcuts {
        let profile = self.config.keyboard.modifier_profile();
        let entries = self.key_router.bindings.entries();
        let chords = BOUND_ACTIONS
            .iter()
            .map(|(bound, action)| {
                let chord = entries
                    .iter()
                    .rev()
                    .find(|(_, a)| a == action || runs_as(*action, *a))
                    .map(|(c, _)| physical_chord(*c))
                    .unwrap_or_default();
                (*bound, chord)
            })
            .collect();
        let primary = if profile.primary() == Physical::Ctrl {
            ModifierKey::Ctrl
        } else {
            ModifierKey::Super
        };
        Shortcuts { chords, primary }
    }

    /// Tell the bars of version 10 or later the chords of the bound actions and the primary modifier, each one when
    /// that changed for it. Call after the key bindings or the modifier profile changed.
    pub fn refresh_menu_bar_shortcuts(&mut self) {
        if self.menu_bars.bars.is_empty() {
            return;
        }
        let shortcuts = self.menu_bar_shortcuts();
        for bar in
            self.menu_bars.bars.iter_mut().filter(|b| {
                b.resource.version() >= 10 && b.shortcuts_sent.as_ref() != Some(&shortcuts)
            })
        {
            shortcuts.send(&bar.resource);
            bar.shortcuts_sent = Some(shortcuts.clone());
        }
    }

    /// Tell the bars of version 3 or later whether a capture session runs or a desktop-control run may capture, each one
    /// when that changed for it.
    pub fn refresh_capture_indicator(&mut self) {
        let active = self.capture_running() || self.control_capturing();
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= 3 && b.capture_sent != Some(active))
        {
            bar.resource.screen_capture(u32::from(active));
            bar.resource.done();
            bar.capture_sent = Some(active);
        }
    }

    /// The colour of the wallpaper under the bar of `output`, band by band across it (at most [`BACKDROP_BANDS_MAX`]
    /// bands). Bands equal to the stored ones do nothing. `None` forgets the output's measure when the output goes and
    /// sends nothing: a value sent to a bar is never withdrawn.
    pub fn set_backdrop_bands(&mut self, output: OutputId, bands: Option<BarBands>) {
        let Some(mut bands) = bands else {
            self.menu_bars.backdrops.remove(&output);
            return;
        };
        bands.0.truncate(BACKDROP_BANDS_MAX);
        if self.menu_bars.backdrops.get(&output) != Some(&bands) {
            self.menu_bars.backdrops.insert(output, bands);
            self.refresh_backdrop();
        }
    }

    /// The colour of the wallpaper under the bar of `output`, as last measured.
    pub fn backdrop_bands(&self, output: OutputId) -> Option<&BarBands> {
        self.menu_bars.backdrops.get(&output)
    }

    /// The lightness of the wallpaper under the bar of `output` (CIE L*/100 of its mean colour, in thousandths), as
    /// last measured: what a bar older than the bands is told.
    pub fn backdrop_lightness(&self, output: OutputId) -> Option<u16> {
        self.menu_bars
            .backdrops
            .get(&output)
            .map(|b| b.lightness().min(BACKDROP_LIGHTNESS_MAX))
    }

    /// Tell each bar the wallpaper under it: a bar of version 21 or later its bands, the first time and whenever they
    /// change; a bar of version 14 to 20 its lightness, the first time and when it moved by more than
    /// [`BACKDROP_LIGHTNESS_STEP`] since.
    fn refresh_backdrop(&mut self) {
        let backdrops = &self.menu_bars.backdrops;
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= 14)
        {
            let Some(bands) = backdrops.get(&bar.output) else {
                continue;
            };
            if bar.resource.version() >= BACKDROP_BANDS_VERSION {
                if bar.bands_sent.as_ref() == Some(bands) {
                    continue;
                }
                let bytes = bands
                    .0
                    .iter()
                    .flat_map(|band| band.iter().flat_map(|c| c.to_le_bytes()))
                    .collect();
                bar.resource.backdrop_bands(bytes);
                bar.resource.done();
                bar.bands_sent = Some(bands.clone());
                continue;
            }
            let lightness = bands.lightness().min(BACKDROP_LIGHTNESS_MAX);
            if bar
                .backdrop_sent
                .is_some_and(|sent| sent.abs_diff(lightness) <= BACKDROP_LIGHTNESS_STEP)
            {
                continue;
            }
            bar.resource.backdrop_luminance(u32::from(lightness));
            bar.resource.done();
            bar.backdrop_sent = Some(lightness);
        }
    }

    /// The menu-bar shortcut: the bar of the active output opens its first menu.
    pub fn focus_menu_bar(&mut self) {
        let Some(active) = self.menu_bars.set.active_output() else {
            return;
        };
        // the bar answers by asking for exclusive keyboard interactivity, which gives it the
        // keyboard on its next commit (`layer_keyboard_exclusivity`), even when a press released it
        self.released_layer = None;
        for bar in self.menu_bars.bars.iter().filter(|b| b.output == active) {
            bar.resource.engage();
            bar.resource.done();
        }
    }

    /// The Help search shortcut: the bar of the active output opens its Help menu with the search field focused, taking
    /// the keyboard as for `engage`. Bars below version 12 are not told; nothing is sent while the session is locked or
    /// a lock is pending, whatever asks for it.
    pub fn focus_help_search(&mut self) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(active) = self.menu_bars.set.active_output() else {
            return;
        };
        self.released_layer = None;
        for bar in self
            .menu_bars
            .bars
            .iter()
            .filter(|b| b.output == active && b.resource.version() >= 12)
        {
            bar.resource.engage_help();
            bar.resource.done();
        }
    }

    /// The output under the pointer is the active one; the other bars are dimmed.
    pub fn track_active_menu_bar(
        &mut self,
        pos: smithay::utils::Point<f64, smithay::utils::Logical>,
        now: f64,
    ) {
        let Some(active) = self.output_at(pos).map(|o| o.id) else {
            return;
        };
        if self.menu_bars.set.active_output() != Some(active) {
            self.menu_bars.set.set_active_output(Some(active), now);
            self.request_redraw();
        }
    }

    /// Timers of the bars: full-screen state per output and the animations.
    pub fn tick_menu_bars(&mut self, now: f64) {
        let ids: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        for id in ids {
            let fullscreen = self
                .wm
                .windows()
                .any(|w| w.output == id && w.state() == LifecycleState::FullScreen);
            if !self
                .menu_bars
                .set
                .set_fullscreen(id, fullscreen, now)
                .is_empty()
            {
                self.refresh_usable_area(id);
            }
        }
        self.menu_bars.set.tick(now);
        if let Some(ev) = self.bar_edge.tick(now) {
            self.apply_bar_edge(ev, now);
        }
        self.tick_menu_bar_edit(now);
    }

    /// Reveal or hide the auto-hidden bar after the pointer dwelled at (or left) the top edge.
    pub fn apply_bar_edge(&mut self, ev: cupertino_input::EdgeEvent, now: f64) {
        let (idx, shown) = match ev {
            cupertino_input::EdgeEvent::Reveal { output } => (output, true),
            cupertino_input::EdgeEvent::Hide { output } => (output, false),
        };
        if let Some(id) = self.outputs.get(idx).map(|o| o.id) {
            self.menu_bars.set.set_revealed(id, shown, now);
            self.request_redraw();
        }
    }

    pub fn menu_bars_want_tick(&self, now: f64) -> bool {
        self.outputs.iter().any(|o| {
            self.menu_bars
                .set
                .presentation(o.id, now)
                .is_some_and(|p| p.animating)
        }) || self.bar_edge.next_deadline().is_some()
    }

    /// Run a window-management command of the fallback menus on the active application or window; ignored while
    /// the session is locked. False when the server does not know the command, or the bar's `version` does not have it.
    fn menu_bar_window_command(&mut self, command: BarCommand, version: u32) -> bool {
        // the entries from toggle_fullscreen on came with version 10
        if version < 10 && u32::from(command) >= u32::from(BarCommand::ToggleFullscreen) {
            return false;
        }
        if self.session_lock.is_active() {
            return true;
        }
        let ctx = self.action_context();
        let window = ctx.key_window;
        match command {
            BarCommand::HideApp => {
                if let Some(app) = ctx.front_app {
                    self.command(Command::HideApp { app });
                }
            }
            BarCommand::HideOthers => {
                if let Some(app) = ctx.front_app {
                    self.command(Command::HideOthers { app });
                }
            }
            BarCommand::ShowAll => self.command(Command::ShowAll),
            BarCommand::QuitApp => {
                for window in ctx.front_app_windows {
                    self.command(Command::RequestClose { window });
                }
            }
            BarCommand::Minimize => {
                if let Some(window) = window {
                    let target = self.minimize_target(window);
                    self.command(Command::Minimize { window, target });
                }
            }
            BarCommand::Zoom => {
                if let Some(window) = window {
                    self.command(Command::ToggleZoom { window });
                }
            }
            BarCommand::Close => {
                if let Some(window) = window {
                    self.command(Command::RequestClose { window });
                }
            }
            BarCommand::ToggleFullscreen => {
                let fullscreen = window
                    .and_then(|w| self.wm.window(w))
                    .is_some_and(|w| w.state().is_fullscreen());
                match window {
                    Some(window) if fullscreen => self.command(Command::ExitFullscreen { window }),
                    _ => self.execute_plan(translate_action(Action::ToggleFullscreen, &ctx)),
                }
            }
            BarCommand::TileLeft => self.execute_plan(translate_action(Action::TileLeft, &ctx)),
            BarCommand::TileRight => self.execute_plan(translate_action(Action::TileRight, &ctx)),
            BarCommand::TileFill => self.execute_plan(translate_action(Action::TileFill, &ctx)),
            BarCommand::BringAllToFront | BarCommand::CascadeAll => {
                if let Some(app) = ctx.front_app {
                    let cascade = command == BarCommand::CascadeAll;
                    self.command(Command::BringAllToFront { app, cascade });
                }
            }
            BarCommand::MinimizeAll => {
                for window in ctx.front_app_windows {
                    if self
                        .wm
                        .window(window)
                        .is_some_and(|w| !w.state().is_minimized() && !w.state().is_dead())
                    {
                        self.command(Command::Minimize {
                            window,
                            target: None,
                        });
                    }
                }
            }
            BarCommand::ZoomAll => {
                for window in ctx.front_app_windows {
                    let zoom = self.wm.window(window).is_some_and(|w| {
                        w.can_zoom()
                            && !matches!(
                                w.state(),
                                LifecycleState::Zooming | LifecycleState::Zoomed
                            )
                    });
                    if zoom {
                        self.command(Command::ToggleZoom { window });
                    }
                }
            }
            BarCommand::ForceQuitDialog => {
                self.execute_plan(translate_action(Action::ForceQuit, &ctx))
            }
            _ => return false,
        }
        true
    }

    /// `activate_window`: show the window `id` names when it is a window of the active application, as the window
    /// list names them now; anything else, and every request while the session is locked, is ignored.
    fn menu_bar_activate_window(&mut self, id: u32) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(app) = self
            .wm
            .key_window()
            .and_then(|k| self.wm.window(k))
            .map(|w| w.app)
        else {
            return;
        };
        let window = WindowId(u64::from(id));
        if self.listed_windows(app).any(|w| w.id == window) {
            // unminimizes it, switches to its workspace, raises and focuses it
            self.focus_window_by_input(window);
        }
    }

    /// Move the key window to workspace `n` (1-based) of its output and switch there; ignored while the session is
    /// locked.
    fn menu_bar_move_to_workspace(&mut self, n: u32) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(window) = self.wm.key_window() else {
            return;
        };
        let Some(output) = self.wm.window(window).map(|w| w.output) else {
            return;
        };
        let target = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.wm.workspaces(output).get(i).map(|w| w.id));
        if let Some(workspace) = target {
            self.command(Command::MoveWindowToWorkspace { window, workspace });
            self.command(Command::SwitchWorkspace { output, workspace });
        }
    }

    /// `force_quit_app` of the bar: force quit the active application as the dock's Force Quit dialog does, only when
    /// `app_id` names it now. Anything else, and every request while the session is locked or a lock is pending, is
    /// ignored, so a focus change between the bar's label and the request ends nothing.
    fn menu_bar_force_quit_app(&mut self, app_id: &str) {
        if self.session_lock.is_active() {
            tracing::info!("menu bar force quit: ignored while the session is locked");
            return;
        }
        if app_id.len() > MAX_APPLICATION_ID_BYTES {
            tracing::warn!(
                len = app_id.len(),
                "menu bar force quit: the application id is too long, ignored"
            );
            return;
        }
        let active = self.key_app_id();
        if active.is_empty() || active != app_id {
            tracing::info!(
                app = app_id,
                active,
                "menu bar force quit: not the active application, ignored"
            );
            return;
        }
        self.force_quit_app(app_id);
    }

    /// The key window when an edit may go to it: the xdg toplevel of a client of the public socket, while no lock
    /// (active or pending), application switcher or move or resize is under way. The server runs no
    /// Xwayland: every window it manages is an xdg toplevel (`WindowEntry`), so no X11 window is ever returned, nor a
    /// layer, lock or shell surface.
    fn edit_target(&self) -> Option<WindowId> {
        if self.session_lock.is_active()
            || self.wm.switcher().is_some()
            || self.pointer_mode != PointerMode::Normal
        {
            return None;
        }
        let window = self.wm.key_window()?;
        let client = self.entry(window)?.surface().client()?;
        capabilities(&client).is_empty().then_some(window)
    }

    /// `edit` of the bar on `bar`: kept for the key window when the request comes from a choice in the bar's open
    /// menu (its popup grab is active); ignored otherwise. A newer request replaces a pending one.
    fn menu_bar_edit(&mut self, bar: &WlSurface, action: EditAction) {
        if self.popup_grab_root() != Some(bar) {
            return;
        }
        let Some(window) = self.edit_target() else {
            return;
        };
        self.menu_bars.edit = Some(PendingEdit {
            action,
            bar: bar.clone(),
            window,
            since: self.now(),
            returned: false,
        });
    }

    /// The keyboard went to the menu bar: its layer surface (the bar's keyboard mode) or one of its menus. The window
    /// whose menus the bar shows is still the window in use, so it keeps its activated state, and since its client heard
    /// the keyboard leave it is told so again: an application whose menu is open in the bar keeps its active look. The
    /// keyboard coming back, or another window becoming the key window, changes nothing here.
    pub(crate) fn keep_activated_under_menu_bar(&mut self, focused: Option<&WlSurface>) {
        let on_bar =
            focused.is_some_and(|f| self.menu_bars.has_surface(&self.shell_surface_root(f)));
        // moving between the bar and its menus is no new leave for the window
        if std::mem::replace(&mut self.menu_bars.holds_keyboard, on_bar) || !on_bar {
            return;
        }
        if let Some(entry) = self.activated_window().and_then(|w| self.entry(w)) {
            entry.restate_activated();
        }
    }

    /// The keyboard focus moved to `focused`. The first move after the bar's popup grab ended decides on the pending
    /// edit: it goes out at the next tick when the keyboard came back to its window, and is dropped when it went
    /// anywhere else, so it never reaches whatever has the keyboard later.
    pub(crate) fn menu_bar_edit_focus(&mut self, focused: Option<&WlSurface>) {
        let Some(edit) = &self.menu_bars.edit else {
            return;
        };
        if edit.returned || self.popup_grab_root() == Some(&edit.bar) {
            return;
        }
        let back =
            focused.is_some_and(|s| self.entry(edit.window).is_some_and(|e| e.has_surface(s)));
        match self.menu_bars.edit.as_mut() {
            Some(edit) if back => edit.returned = true,
            _ => self.menu_bars.edit = None,
        }
    }

    /// Send the pending edit once the keyboard is back on its window, checking every guard again; drop it
    /// [`EDIT_WAIT`] after its arrival.
    fn tick_menu_bar_edit(&mut self, now: f64) {
        let Some(edit) = &self.menu_bars.edit else {
            return;
        };
        if now - edit.since > EDIT_WAIT {
            self.menu_bars.edit = None;
            return;
        }
        if !edit.returned {
            return;
        }
        let Some(edit) = self.menu_bars.edit.take() else {
            return;
        };
        let keyboard = self.seat.get_keyboard().unwrap();
        let on_window = keyboard
            .current_focus()
            .is_some_and(|f| self.entry(edit.window).is_some_and(|e| e.has_surface(&f)));
        if !on_window
            || self.edit_target() != Some(edit.window)
            || self.popup_grab_root().is_some()
            || keyboard.is_grabbed()
            || self.pointer.is_grabbed()
        {
            return;
        }
        let kb = &self.config.keyboard;
        let app = self.key_app_id();
        if let Some((mods, sym)) = edit_chord(
            edit.action,
            self.key_client_class(),
            kb.rewrite_opted_out(&app),
            &kb.modifier_profile(),
        ) {
            self.forward_chord(sym, mods);
        }
    }
}

impl Dispatch<CupertinoMenuBarV1, MenuBarData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoMenuBarV1,
        request: cupertino_menu_bar_v1::Request,
        data: &MenuBarData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(output) = data.output else { return };
        if !require(client, ShellRole::MenuBar, resource) {
            return;
        }
        match request {
            cupertino_menu_bar_v1::Request::SetMenuOpen { open } => {
                let now = state.now();
                state.menu_bars.set.set_menu_open(output, open != 0, now);
                state.request_redraw();
            }
            cupertino_menu_bar_v1::Request::WindowCommand { command } => {
                let version = resource.version();
                let known =
                    matches!(command, WEnum::Value(c) if state.menu_bar_window_command(c, version));
                if !known {
                    resource.post_error(
                        BarError::InvalidCommand,
                        format!("{command:?} is not a window command"),
                    );
                }
            }
            cupertino_menu_bar_v1::Request::MoveToWorkspace { workspace } => {
                state.menu_bar_move_to_workspace(workspace)
            }
            cupertino_menu_bar_v1::Request::ActivateWindow { id } => {
                state.menu_bar_activate_window(id)
            }
            // a bar below version 11 cannot send it: the wire rejects a request newer than the object
            cupertino_menu_bar_v1::Request::Edit { action } => match action {
                WEnum::Value(action) => {
                    let bar = state
                        .menu_bars
                        .bars
                        .iter()
                        .find(|b| &b.resource == resource)
                        .map(|b| b.surface.clone());
                    if let Some(bar) = bar {
                        state.menu_bar_edit(&bar, action);
                    }
                }
                WEnum::Unknown(_) => resource.post_error(
                    BarError::InvalidEditAction,
                    format!("{action:?} is not an edit action"),
                ),
            },
            // a bar below version 13 cannot send it: the wire rejects a request newer than the object
            cupertino_menu_bar_v1::Request::ForceQuitApp { app_id } => {
                state.menu_bar_force_quit_app(&app_id)
            }
            cupertino_menu_bar_v1::Request::SessionAction { action } => {
                if action != WEnum::Value(SessionAction::RefreshCapabilities)
                    && !state.control_session_action_allowed(client)
                {
                    return;
                }
                // the panels and what the machine allows are the session agent's; sleep, the lock and the switch to the
                // login window the server's own
                match action {
                    WEnum::Value(SessionAction::Sleep) => state.sleep_session(),
                    WEnum::Value(SessionAction::Restart) => {
                        state.ask_session_agent(AgentRequest::Restart)
                    }
                    WEnum::Value(SessionAction::ShutDown) => {
                        state.ask_session_agent(AgentRequest::ShutDown)
                    }
                    WEnum::Value(SessionAction::Lock) => state.lock_session_now(),
                    WEnum::Value(SessionAction::LogOut) => state.log_out_session(),
                    WEnum::Value(SessionAction::SwitchUser) => state.switch_to_login_window(),
                    WEnum::Value(SessionAction::RefreshCapabilities) => {
                        state.ask_session_agent(AgentRequest::RefreshCapabilities)
                    }
                    WEnum::Value(_) | WEnum::Unknown(_) => resource.post_error(
                        BarError::InvalidSessionAction,
                        format!("{action:?} is not a session action"),
                    ),
                }
            }
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The role ends with the object; the bar's reservation goes with it.
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoMenuBarV1, data: &MenuBarData) {
        let Some(output) = data.output else { return };
        state.menu_bars.bars.retain(|b| &b.resource != resource);
        state.menu_bar_gone(output);
    }
}

/// The longest bus name, unique or well-known, the D-Bus specification allows.
pub const MAX_BUS_NAME_BYTES: usize = 255;

/// The longest application id a window's menu announcement may carry.
pub const MAX_APPLICATION_ID_BYTES: usize = 255;

/// Whether `program`, a program only the system can have installed ([`crate::process::trusted_executable`]), is the X11
/// bridge.
fn is_bridge_program(program: &std::path::Path) -> bool {
    program.file_name() == Some(std::ffi::OsStr::new(crate::xwayland::SATELLITE))
}

/// Whether `name` is a unique bus name such as `:1.42`.
pub(crate) fn is_unique_bus_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(':') else {
        return false;
    };
    name.len() <= MAX_BUS_NAME_BYTES
        && rest.split('.').count() >= 2
        && rest.split('.').all(|e| {
            !e.is_empty()
                && e.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// The longest object path a window's menu announcement may carry: the server forwards the four paths, the bus name and
/// an application id in one `menu_gtk` event, and four paths of this length still fit the 4096-byte message limit.
pub const MAX_OBJECT_PATH_BYTES: usize = 512;

/// Whether `name` is a bus name, unique or well-known, no longer than [`MAX_BUS_NAME_BYTES`].
pub(super) fn is_bus_name(name: &str) -> bool {
    name.len() <= MAX_BUS_NAME_BYTES && zbus::names::BusName::try_from(name).is_ok()
}

/// Whether `path` is a D-Bus object path no longer than [`MAX_OBJECT_PATH_BYTES`].
pub(super) fn is_object_path(path: &str) -> bool {
    path.len() <= MAX_OBJECT_PATH_BYTES
        && (path == "/"
            || path.strip_prefix('/').is_some_and(|rest| {
                rest.split('/').all(|e| {
                    !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            }))
}

/// Per-object data of a window's menu object; `surface` is `None` for an object the server rejected with a protocol
/// error.
pub struct AppMenuData {
    pub surface: Option<WlSurface>,
}

impl Dispatch<CupertinoAppMenuV1, AppMenuData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoAppMenuV1,
        request: cupertino_app_menu_v1::Request,
        data: &AppMenuData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(surface) = &data.surface else { return };
        if let cupertino_app_menu_v1::Request::SetAddress {
            bus_name,
            application_object_path,
            window_object_path,
            menubar_object_path,
            app_menu_object_path,
        } = request
        {
            let paths = [
                &application_object_path,
                &window_object_path,
                &menubar_object_path,
            ];
            if !is_unique_bus_name(&bus_name)
                || !paths.iter().all(|p| is_object_path(p))
                || !(app_menu_object_path.is_empty() || is_object_path(&app_menu_object_path))
            {
                resource.post_error(
                    AppMenuError::InvalidAddress,
                    "a bus name or object path is malformed",
                );
                return;
            }
            let props = GtkMenuProps {
                bus_name,
                application_id: None,
                application_object_path: Some(application_object_path),
                window_object_path: Some(window_object_path),
                menubar_object_path: Some(menubar_object_path),
                app_menu_object_path: (!app_menu_object_path.is_empty())
                    .then_some(app_menu_object_path),
            };
            if let Some(menu) = state
                .menu_bars
                .app_menus
                .iter_mut()
                .find(|m| &m.resource == resource)
            {
                menu.announced = true;
            }
            state.announce_menu_address(surface, MenuAddress::Gtk(props));
        }
    }

    /// The announcement ends with the object.
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoAppMenuV1, data: &AppMenuData) {
        let Some(i) = state
            .menu_bars
            .app_menus
            .iter()
            .position(|m| &m.resource == resource)
        else {
            return;
        };
        let menu = state.menu_bars.app_menus.remove(i);
        if let (true, Some(surface)) = (menu.announced, &data.surface) {
            if surface.is_alive() {
                state.withdraw_menu_address(surface, MenuKind::Gtk);
            }
        }
    }
}
