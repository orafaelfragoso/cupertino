use super::*;

impl Cupertino {
    // ---- outputs -------------------------------------------------------------------------

    /// Register `output` in its current state, which [`new_output`] set, and announce its global. Returns its model id.
    /// Outputs are created for the enabled heads of `crate::displays`, once their backend shows them.
    pub fn add_output(&mut self, output: Output) -> OutputId {
        let global = output.create_global::<Self>(&self.display_handle);
        let mode = output.current_mode().expect("a new output has a mode");
        let scale = output.current_scale().fractional_scale();
        let transform = output.current_transform();
        let position = output.current_location();
        let id = OutputId(self.next_output);
        self.next_output += 1;
        let rect = output_rect(mode, scale, transform, (position.x, position.y));
        let tracker = OutputDamageTracker::from_output(&output);
        self.outputs.push(OutputEntry {
            output: output.clone(),
            id,
            rect,
            scale,
            render_scale: scale,
            scale_generation: 0,
            tracker,
            cache: ElementCache::default(),
            global: Some(global),
            lock: Default::default(),
            scheduler: Default::default(),
            identity: None,
        });
        let now = self.now();
        self.menu_bars.set.add_output(id, rect, now);
        let usable = self.usable_area(id);
        let fx = self.wm.handle(
            Command::AddOutput {
                output: id,
                rect,
                usable,
            },
            now,
        );
        self.apply_effects(fx);
        self.refresh_usable_area(id);
        self.refresh_render_scales();
        id
    }

    /// Work out again the scale each output is drawn at from its scale and `fractional_scale_mode`; an output whose
    /// render scale changed is drawn again whole and its surfaces are told the scale to draw at.
    pub fn refresh_render_scales(&mut self) {
        let modes = &self.config.fractional_scale_mode;
        let mut changed = false;
        for o in &mut self.outputs {
            let name = o.output.name();
            let mode = modes.of(std::iter::once(name.as_str()).chain(o.identity.as_deref()));
            let render = crate::render::supersample::render_scale(o.scale, mode);
            if render != o.render_scale {
                tracing::info!(output = %name, scale = o.scale, render, "the output's render scale changed");
                o.render_scale = render;
                o.scale_generation += 1;
                o.cache.supersample.reset();
                o.scheduler.damage();
                changed = true;
            }
        }
        if changed {
            self.refresh_surface_outputs();
            self.request_redraw();
        }
    }

    /// Whether output `idx` is drawn at a whole scale above its own and shrunk to its pixels.
    pub fn supersampled(&self, idx: usize) -> bool {
        self.outputs
            .get(idx)
            .is_some_and(|o| o.render_scale != o.scale)
    }

    /// Remove an output (hot-unplug): its windows move to the first remaining output and its layer surfaces let go of
    /// the input ([`Cupertino::release_output_layers`]). What needs the person stays reachable
    /// (`crate::displays::recovery`): a menu of a window on it closes, an overview on it ends, the pointer comes onto a
    /// remaining output and a locked session's keyboard onto a remaining lock surface.
    pub fn remove_output(&mut self, id: OutputId) {
        let Some(idx) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        if let Some(identity) = self.outputs[idx].identity.clone() {
            let from = self.outputs[idx].rect;
            self.note_departures(id, &identity, from);
        }
        self.end_popup_grab_on(id);
        self.overviews.remove_output(id);
        self.output_screenshots.retain(|(o, _)| *o != id);
        // told before the global goes, so a client hears `leave` for an output it can still name
        self.leave_departing_output(idx);
        let entry = self.outputs.remove(idx);
        self.release_output_layers(&entry.output);
        let now = self.now();
        self.menu_bars.remove_output(id, now);
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
        self.docks.remove_output(id);
        self.window_managers.remove_output(id);
        if let Some(g) = entry.global {
            self.retire_global(g);
        }
        let now = self.now();
        let fx = self.wm.handle(Command::RemoveOutput { output: id }, now);
        self.apply_effects(fx);
        self.note_placements();
        self.bring_pointer_onto_outputs();
        self.refocus_lock_keyboard();
        self.refresh_surface_outputs();
        self.request_redraw();
    }

    /// The layer surfaces of an output that goes leave its layer map: a popup grab of one of them ends, and one that
    /// holds the keyboard exclusively gives it back to the key window. Their clients destroy them later, after the
    /// `closed` that [`Cupertino::drop_head_output`] sent, or never; meanwhile no output shows them and none of them
    /// keeps the keyboard or the pointer.
    fn release_output_layers(&mut self, output: &Output) {
        let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
        if layers.is_empty() {
            return;
        }
        if self
            .popup_grab_root()
            .is_some_and(|root| layers.iter().any(|l| l.wl_surface() == root))
        {
            self.end_popup_grab(
                SERIAL_COUNTER.next_serial(),
                std::time::Duration::from(self.clock.now()).as_millis() as u32,
            );
        }
        for layer in &layers {
            self.layer_keyboard_exclusivity(layer.wl_surface(), false);
        }
        {
            let mut map = layer_map_for_output(output);
            for layer in &layers {
                map.unmap_layer(layer);
            }
        }
        self.refresh_holds();
    }

    /// Take a global away without disconnecting a client that binds it before it hears that it is gone: clients are
    /// told now, and the global is destroyed after [`RETIRED_GLOBAL_LIFETIME`].
    fn retire_global(&mut self, global: GlobalId) {
        self.display_handle.disable_global::<Self>(global.clone());
        let timer = self.loop_handle.insert_source(
            Timer::from_duration(RETIRED_GLOBAL_LIFETIME),
            move |_, _, state| {
                state.display_handle.remove_global::<Self>(global.clone());
                TimeoutAction::Drop
            },
        );
        if let Err(e) = timer {
            tracing::warn!(error = %e, "cannot time the removal of a global; it stays disabled");
        }
    }

    /// Give an output a new mode, scale, transform or position (a nested window resized, an
    /// output configuration applied): its layer surfaces, lock surface, menu bar and windows follow.
    pub fn configure_output(
        &mut self,
        id: OutputId,
        mode: OutputMode,
        scale: f64,
        transform: Transform,
        position: (i32, i32),
    ) {
        let Some(idx) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        let entry = &mut self.outputs[idx];
        entry.output.change_current_state(
            Some(mode),
            Some(transform),
            Some(OutputScale::Fractional(scale)),
            Some(position.into()),
        );
        entry.output.set_preferred(mode);
        if entry.scale != scale {
            entry.scale_generation += 1;
            tracing::debug!(output = ?id, from = entry.scale, to = scale, "output scale changed");
        }
        entry.scale = scale;
        entry.rect = output_rect(mode, scale, transform, position);
        entry.tracker = OutputDamageTracker::from_output(&entry.output);
        entry.scheduler.damage();
        let rect = entry.rect;
        layer_map_for_output(&entry.output).arrange();
        self.refresh_render_scales();
        self.configure_lock_surface(idx);
        self.menu_bars.set.set_output_rect(id, rect);
        let usable = self.usable_area(id);
        self.command(Command::SetOutputRect {
            output: id,
            rect,
            usable,
        });
        self.refresh_usable_area(id);
        self.request_redraw();
    }

    /// The output's quantities kept apart: its mode in panel pixels, how it is turned, its scale and its logical
    /// extent. `None` for an unknown output or one without a mode.
    pub fn output_geometry(&self, id: OutputId) -> Option<cupertino_displays::OutputGeometry> {
        let entry = self.output_entry(id)?;
        let mode = entry.output.current_mode()?;
        let extent = cupertino_displays::Extent::new(
            u32::try_from(mode.size.w).ok()?,
            u32::try_from(mode.size.h).ok()?,
        );
        let turned = matches!(
            entry.output.current_transform(),
            Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
        );
        cupertino_displays::OutputGeometry::new(extent, entry.scale, turned).ok()
    }

    /// A position in an output's turned panel pixels (a pointer that reports pixels) in the global logical space.
    pub fn output_pixels_to_global(
        &self,
        id: OutputId,
        pixels: (f64, f64),
    ) -> Option<Point<f64, Logical>> {
        let entry = self.output_entry(id)?;
        let (x, y) = self.output_geometry(id)?.to_points(pixels);
        Some(Point::from((entry.rect.x + x, entry.rect.y + y)))
    }

    pub fn output_entry(&self, id: OutputId) -> Option<&OutputEntry> {
        self.outputs.iter().find(|o| o.id == id)
    }

    pub fn output_by_wl(&self, output: &Output) -> Option<&OutputEntry> {
        self.outputs.iter().find(|o| &o.output == output)
    }

    /// Output containing `p` (global logical coordinates), else the nearest one.
    pub fn output_at(&self, p: Point<f64, Logical>) -> Option<&OutputEntry> {
        let pt = MPoint::new(p.x, p.y);
        self.outputs
            .iter()
            .find(|o| o.rect.contains(pt))
            .or_else(|| {
                self.outputs.iter().min_by(|a, b| {
                    let d = |o: &OutputEntry| {
                        let dx = (pt.x - pt.x.clamp(o.rect.x, o.rect.right())).abs();
                        let dy = (pt.y - pt.y.clamp(o.rect.y, o.rect.bottom())).abs();
                        dx + dy
                    };
                    d(a).total_cmp(&d(b))
                })
            })
    }

    pub fn primary_output(&self) -> Option<&OutputEntry> {
        self.outputs.first()
    }

    /// Area available to windows on `id` (menu bar and layer-shell zones removed; the dock's
    /// exclusive zone is one of them).
    pub fn usable_area(&self, id: OutputId) -> Rect {
        let Some(o) = self.output_entry(id) else {
            return Rect::default();
        };
        let menu = if self.config.shell.show_menu_bar {
            self.menu_bars.reserved_height(id)
        } else {
            0.0
        };
        let mut zone = {
            let map = layer_map_for_output(&o.output);
            let z = map.non_exclusive_zone();
            (z.size.w > 0 && z.size.h > 0).then(|| {
                Rect::new(
                    o.rect.x + z.loc.x as f64,
                    o.rect.y + z.loc.y as f64,
                    z.size.w as f64,
                    z.size.h as f64,
                )
            })
        };
        if let Some(held) = self.hosts.held_non_exclusive_zone(id, o.rect) {
            zone = Some(match zone {
                Some(z) => {
                    let x0 = z.x.max(held.x);
                    let y0 = z.y.max(held.y);
                    let x1 = (z.x + z.w).min(held.x + held.w);
                    let y1 = (z.y + z.h).min(held.y + held.h);
                    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
                }
                None => held,
            });
        }
        shell::usable_area(o.rect, menu, zone)
    }

    pub fn refresh_usable_area(&mut self, id: OutputId) {
        let usable = self.usable_area(id);
        let now = self.now();
        let fx = self
            .wm
            .handle(Command::SetUsableArea { output: id, usable }, now);
        self.apply_effects(fx);
        self.refresh_overview_v1();
        let fx = self.wm.handle(
            Command::SetDockZone {
                output: id,
                zone: self.dock_rect(id),
            },
            now,
        );
        self.apply_effects(fx);
        self.refresh_window_managers();
    }

    // ---- windows -------------------------------------------------------------------------

    pub fn entry(&self, id: WindowId) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Send the configures that shrinking frame animations held back once they end.
    pub(super) fn send_deferred_configures(&mut self, now: f64) {
        let key = self.activated_window();
        for e in &mut self.windows {
            let Shrink::Deferred { frame, states } = e.shrink else {
                continue;
            };
            if self
                .wm
                .window(e.id)
                .is_some_and(|w| w.lifecycle.is_animating(now))
            {
                continue;
            }
            e.shrink = e
                .configure(frame, &states, key == Some(e.id))
                .map_or(Shrink::None, Shrink::Awaiting);
        }
    }

    pub fn entry_for_surface(&self, surface: &WlSurface) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| w.has_surface(surface))
    }

    pub fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.entry_for_surface(surface).map(|w| w.id)
    }

    /// Toplevel root of any surface in a window's tree (subsurface or popup child).
    pub fn root_window_surface(&self, surface: &WlSurface) -> Option<WlSurface> {
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        if self.entry_for_surface(&root).is_some() {
            return Some(root);
        }
        self.popups
            .find_popup(surface)
            .and_then(|p| smithay::desktop::find_popup_root_surface(&p).ok())
    }

    /// Called on a toplevel commit: sends the initial configure, maps the window once it has a
    /// buffer and tracks unmapping.
    pub fn toplevel_committed(&mut self, surface: &WlSurface) {
        let Some(idx) = self.windows.iter().position(|w| w.has_surface(surface)) else {
            return;
        };
        if crate::toplevel_icon::icon_committed(surface) && self.windows[idx].mapped {
            self.refresh_docks();
        }
        let initial_sent = with_states(surface, |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        if !initial_sent {
            use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
            let t = &self.windows[idx].toplevel;
            if self.windows[idx].server_side {
                t.with_pending_state(|st| st.decoration_mode = Some(Mode::ServerSide));
            }
            t.send_configure();
            return;
        }
        let has_buffer =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| {
                s.buffer().is_some()
            })
            .unwrap_or(false);
        let entry = &mut self.windows[idx];
        if has_buffer && !entry.server_side && entry.toplevel.wl_surface() == surface {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
            // the states the client drew this commit in: the last configure it acknowledged
            let states = entry.toplevel.current_state().states;
            let filling = [
                State::Maximized,
                State::Fullscreen,
                State::TiledLeft,
                State::TiledRight,
                State::TiledTop,
                State::TiledBottom,
            ]
            .into_iter()
            .any(|s| states.contains(s));
            entry.frame = crate::frame_fill::opaque_frame(
                surface,
                entry.window.geometry(),
                self.config.window.corner_radius,
                filling,
            );
        }
        let (mapped, id) = (self.windows[idx].mapped, self.windows[idx].id);
        match (mapped, has_buffer) {
            (false, true) => self.map_window(idx),
            (true, false) => {
                self.windows[idx].mapped = false;
                self.windows[idx].shrink = Shrink::None;
                // a window mapped again is judged from its new frames alone
                self.windows[idx].frame = crate::frame_fill::Frame::UNSEEN;
                let now = self.now();
                let fx = self.wm.handle(Command::Destroy { window: id }, now);
                self.apply_effects(fx);
            }
            (true, true) => {
                self.windows[idx].window.on_commit();
                if let Shrink::Awaiting(sent) = self.windows[idx].shrink {
                    let acked = with_states(surface, |s| {
                        s.data_map
                            .get::<XdgToplevelSurfaceData>()
                            .and_then(|d| d.lock().unwrap().current_serial)
                    });
                    if acked.is_some_and(|serial| serial.is_no_older_than(&sent)) {
                        self.windows[idx].shrink = Shrink::None;
                    }
                }
                self.refresh_snapshot(id);
                let (min, max) = frame_limits(self.windows[idx].insets(), size_hints(surface));
                let now = self.now();
                let fx = self.wm.handle(
                    Command::SetSizeHints {
                        window: id,
                        min,
                        max,
                        content: None,
                    },
                    now,
                );
                self.apply_effects(fx);
                let disabled = cupertino_wm::DisabledControls::from(
                    crate::app_effects_v1::disabled_controls(surface),
                );
                if self.wm.window(id).is_some_and(|w| w.disabled != disabled) {
                    let fx = self.wm.handle(
                        Command::SetDisabledControls {
                            window: id,
                            disabled,
                        },
                        now,
                    );
                    self.apply_effects(fx);
                    self.refresh_menu_bars();
                }
            }
            (false, false) => {}
        }
        self.request_redraw_for(surface);
    }

    pub(crate) fn map_window(&mut self, idx: usize) {
        let surface = self.windows[idx].surface().clone();
        let id = self.windows[idx].id;
        let (app_id, title) = with_states(&surface, |s| {
            let d = s
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            (d.app_id.clone(), d.title.clone())
        });
        let launch = self.take_launch(&surface);
        let identity = match app_id.as_deref().filter(|a| !a.is_empty()) {
            Some(app_id) => app_id.to_string(),
            None => self
                .client_identity(&surface, launch.as_ref())
                .unwrap_or_else(|| policy::app_identity(None, title.as_deref())),
        };
        self.windows[idx].window.on_commit();
        let geo = self.windows[idx].window.geometry();
        let (min, max) = frame_limits(self.windows[idx].insets(), size_hints(&surface));
        // the protocol facts the window kind is chosen from (`cupertino_wm::window_kind`): parent and
        // modality; Wayland has no utility-window signal
        let parent = self.windows[idx]
            .toplevel
            .parent()
            .and_then(|p| self.window_id_for_surface(&p));
        let Some(output) = self.active_output() else {
            return;
        };
        let (screen, usable) = self
            .wm
            .output(output)
            .map(|o| (o.rect, o.usable))
            .unwrap_or_default();
        // the visible frame new windows are placed in: the stage frame while the stage is on
        let visible = if self.stage_active(output) {
            cupertino_wm::stage::StageConfig::default().stage_frame(usable)
        } else {
            usable
        };
        let ins = self.windows[idx].insets();
        let size = (geo.size.w > 0 && geo.size.h > 0).then(|| {
            Size::new(
                geo.size.w as f64 + ins.horizontal(),
                geo.size.h as f64 + ins.vertical(),
            )
        });
        let size = policy::new_window_size(visible, size, &self.config);
        let frame = if parent.is_some() {
            // the model places child windows itself (sheets, dialogs)
            policy::centred(visible, size)
        } else {
            self.cascade.retain(|w| self.wm.window(w).is_some());
            self.cascade.place(
                &identity,
                id,
                size,
                visible,
                screen,
                self.config.cascade_step,
            )
        };
        let mut params = MapParams::new(id, &identity, frame);
        params.title = title.unwrap_or_default();
        params.min_size = min;
        params.max_size = max;
        params.disabled = cupertino_wm::DisabledControls::from(
            crate::app_effects_v1::disabled_controls(&surface),
        );
        params.parent = parent;
        params.modal = self.windows[idx].modal;
        if self.windows[idx].server_side {
            params.sheet_top = self.decor.sheet_top(self.windows[idx].titlebar);
        }
        // the model turns a modal child into a sheet and places dialogs
        params.kind = cupertino_wm::window_kind(params.modal, false);
        params.output = Some(output);
        params.activation = cupertino_wm::Activation::Policy(self.map_focus(&surface, launch));
        self.windows[idx].mapped = true;
        self.apply_pending_menu_addresses(id.0, &surface);
        let handle = self
            .foreign_toplevel_state
            .new_toplevel::<Self>(cut_text(&params.title), cut_text(&identity));
        self.windows[idx].foreign = Some(handle);
        let now = self.now();
        let fx = self.wm.handle(Command::Map(params), now);
        self.apply_effects(fx);
    }

    /// The grant of the launch behind a toplevel that maps, used up: the token it presented for its surface, else the
    /// `XDG_ACTIVATION_TOKEN` its process was started with, for a program that never presents it.
    fn take_launch(&mut self, surface: &WlSurface) -> Option<crate::handlers::TokenGrant> {
        if let Some(grant) = self.pending_activations.remove(&surface.id()) {
            return Some(grant);
        }
        let client = surface.client()?;
        let token = smithay::wayland::xdg_activation::XdgActivationToken::from((self
            .launch_token_of)(
            &self.display_handle,
            &client,
        )?);
        let grant = self
            .xdg_activation_state
            .data_for_token(&token)?
            .user_data
            .get::<crate::handlers::TokenGrant>()
            .cloned();
        self.xdg_activation_state.remove_token(&token);
        grant
    }

    /// The application of a toplevel that maps without an application id (Chromium's first-run dialog), from its client:
    /// the application of another mapped window of that client, else the application its launch named, else the desktop
    /// entry of the program it runs when the system installed that program ([`crate::process::trusted_executable`]). A
    /// client cannot name an application this way that it could not name with `set_app_id`.
    fn client_identity(
        &self,
        surface: &WlSurface,
        launch: Option<&crate::handlers::TokenGrant>,
    ) -> Option<String> {
        let client = surface.client()?;
        let sibling = self.windows.iter().filter(|e| e.mapped).find_map(|e| {
            let same = e.surface().client().is_some_and(|c| c.id() == client.id());
            same.then(|| self.wm.window(e.id).and_then(|w| self.wm.app(w.app)))
                .flatten()
                .map(|a| a.identity.clone())
        });
        sibling
            .or_else(|| launch.and_then(|l| l.app_id.clone()))
            .or_else(|| {
                let program = (self.executable_of)(&self.display_handle, &client)?;
                self.desktop.for_program(&program).map(|e| e.id.clone())
            })
    }

    /// Forgets the first-window grant of processes whose last client that mapped a toplevel has disconnected, and the
    /// resident applications of the clients that disconnected (the docks hear of it).
    pub fn forget_gone_processes(&mut self) {
        let gone = std::mem::take(
            &mut *self
                .gone_clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if gone.is_empty() {
            return;
        }
        let mut residents_changed = false;
        for id in &gone {
            self.mapped_clients.remove(id);
            residents_changed |= self.docks.client_gone(id);
            self.host_client_disconnected(id);
            let now = self.now();
            self.transactions
                .client_disconnected(id, self.hosts.held_surfaces(), now);
            self.overviews.client_disconnected(id);
        }
        let mapped = &self.mapped_clients;
        self.mapped_processes
            .retain(|p| mapped.values().any(|q| q == p));
        if residents_changed {
            self.refresh_docks();
        }
    }

    /// The facts the focus policy needs about a window that maps (`cupertino_wm::MapFocus`): whether it is its client's
    /// first toplevel, whether the person has given input elsewhere since the client connected, and the grant of its launch
    /// ([`Self::take_launch`]).
    fn map_focus(
        &mut self,
        surface: &WlSurface,
        launch: Option<crate::handlers::TokenGrant>,
    ) -> cupertino_wm::MapFocus {
        use std::sync::atomic::Ordering;
        let (first_of_client, input_elsewhere) = match surface.client() {
            Some(client) => match client.get_data::<ClientState>() {
                Some(data) => {
                    let id = client.id();
                    let first_of_connection = !data.mapped_toplevel.swap(true, Ordering::SeqCst);
                    // a process that cannot be named gets no first-window grant
                    let process = (self.process_of)(&self.display_handle, &client);
                    if let Some(process) = process {
                        self.mapped_clients.insert(id.clone(), process);
                        data.gone.get_or_init(|| self.gone_clients.clone());
                    }
                    let first_of_process =
                        process.is_some_and(|process| self.mapped_processes.insert(process));
                    // nobody is at the screen while the session is locked: what comes up then waits behind
                    let first =
                        first_of_connection && first_of_process && !self.session_lock.is_active();
                    let elsewhere = self.last_input.as_ref().is_some_and(|(at, who)| {
                        *at > data.connected_at && who.as_ref() != Some(&id)
                    });
                    (first, elsewhere)
                }
                None => (false, false),
            },
            None => (false, false),
        };
        let grant = launch.map(|g| cupertino_wm::Grant {
            created: g.created,
            valid: g.valid,
        });
        cupertino_wm::MapFocus {
            first_of_client,
            input_elsewhere,
            grant,
            last_user_event: self.last_user_event,
        }
    }

    /// Note that a key or button press went to `focus` (its client, if any).
    pub fn note_input(&mut self, focus: Option<&WlSurface>) {
        self.last_input = Some((
            std::time::Instant::now(),
            focus.and_then(|s| s.client()).map(|c| c.id()),
        ));
    }

    /// Insets for a window decorated by the compositor with a title bar of `style`.
    pub fn decoration_insets(
        &self,
        style: cupertino_decorations::TitlebarStyle,
    ) -> crate::config::Insets {
        if self.config.window.decorations {
            self.decor.insets(style)
        } else {
            self.config.window.ssd_insets
        }
    }

    /// The cursor of the resize zone `edge` of window `id` at its size now, `None` where it cannot resize that way
    /// (`decor::resize_cursor`).
    pub fn resize_cursor(
        &self,
        id: WindowId,
        edge: cupertino_decorations::ResizeEdge,
    ) -> Option<smithay::input::pointer::CursorIcon> {
        let w = self.wm.window(id)?;
        let (min, max) = w.size_limits();
        let f = w.geometry();
        crate::decor::resize_cursor(
            edge,
            crate::decor::AxisFreedom::of(f.w, min.w, max.w),
            crate::decor::AxisFreedom::of(f.h, min.h, max.h),
        )
    }

    /// The windows on the output under `pos` that take input, topmost first, each with its plan; `None` while the
    /// session is locked or off every output. Hidden windows and the windows holes claim (the stage strip's, the
    /// overview's) take no input of their clients.
    fn input_windows(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<Vec<(WindowId, &WindowEntry, crate::render::scene::WindowPlan)>> {
        if self.session_lock.is_active() {
            return None;
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let list = crate::render::scene::windows_on_output(
            &self.wm,
            out.id,
            out.rect.w,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            now,
        );
        Some(
            list.into_iter()
                .rev()
                .filter(|(id, _, _)| {
                    !self.window_managers.passes_input(*id)
                        && !self.transactions.is_window_claimed(*id)
                })
                .filter_map(|(id, slide, _)| {
                    let entry = self.entry(id)?;
                    let (plan, _) = crate::render::scene::plan_for(
                        &self.wm,
                        entry,
                        slide,
                        now,
                        &self.fx,
                        self.display_options(),
                    )?;
                    Some((id, entry, plan))
                })
                .collect(),
        )
    }

    /// The surface of one of `entry`'s popups under `pos` (global logical), every surface of a popup's tree:
    /// `Window::surface_under` with `POPUP` alone finds only the popups' roots.
    fn popup_of_window_under(
        id: WindowId,
        entry: &WindowEntry,
        plan: &crate::render::scene::WindowPlan,
        pos: Point<f64, Logical>,
    ) -> Option<WindowTarget> {
        let geo = entry.window.geometry();
        let ins = entry.insets();
        let origin = Point::<f64, Logical>::from((
            plan.visual.x + ins.left - geo.loc.x as f64,
            plan.visual.y + ins.top - geo.loc.y as f64,
        ));
        let root = entry.window.toplevel().map(|t| t.wl_surface().clone())?;
        for (popup, location) in PopupManager::popups_for_surface(&root) {
            let offset = geo.loc + location - popup.geometry().loc;
            if let Some((surface, loc)) = smithay::desktop::utils::under_from_surface_tree(
                popup.wl_surface(),
                pos - origin,
                offset,
                WindowSurfaceType::ALL,
            ) {
                return Some(WindowTarget::Surface {
                    id,
                    surface,
                    origin: origin + loc.to_f64(),
                });
            }
        }
        None
    }

    /// The popup surface under `pos` (global logical) of the window whose popups are drawn above every window and the
    /// bars (`SceneLayer::Popups`, [`crate::render::scene::raised_popups_window`]), so the pointer finds them before
    /// either. The popups of the other windows are part of their window ([`Self::window_target_under`]).
    /// The window target under `pos` when nothing drawn above the windows (an overlay or top layer surface such as the
    /// dock, a bar's popup, a lock surface) takes the point first: what hover cursors and presses on the decorations
    /// act on, so a resize band under the dock neither changes the cursor nor starts a resize.
    pub fn window_target_on_top(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        let target = self.window_target_under(pos)?;
        let above = self.surface_under(pos);
        let on_top = match (&target, &above) {
            (WindowTarget::Decoration { .. }, None) => true,
            (WindowTarget::Surface { surface, .. }, Some((s, _))) => s == surface,
            _ => false,
        };
        on_top.then_some(target)
    }

    pub fn window_popup_under(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        let out = self.output_at(pos)?;
        let raised = crate::render::scene::raised_popups_window(&self.wm, out.id)?;
        let (id, entry, plan) = self
            .input_windows(pos)?
            .into_iter()
            .find(|(id, _, _)| *id == raised)?;
        Self::popup_of_window_under(id, entry, &plan, pos)
    }

    /// What is under `pos` (global logical), from the top down: the raised popups of the key window
    /// ([`Self::window_popup_under`]), then per window, in this order: the window control buttons of a unified bar, the
    /// window's own popups (any other window's), the resize zones (3 pt inside to 4 pt outside the frame, also for a
    /// client-decorated window, measured from its window geometry), the client's surface inside the frame, the title
    /// bar and buttons of a server-decorated window. Outside the frame and its resize band a window takes no input: a
    /// client-drawn shadow passes the pointer to what lies below.
    pub fn window_target_under(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        if let Some(popup) = self.window_popup_under(pos) {
            return Some(popup);
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let raised = crate::render::scene::raised_popups_window(&self.wm, out.id);
        for (id, entry, plan) in self.input_windows(pos)? {
            let geo = entry.window.geometry();
            let ins = entry.insets();
            let origin = Point::<f64, Logical>::from((
                plan.visual.x + ins.left - geo.loc.x as f64,
                plan.visual.y + ins.top - geo.loc.y as f64,
            ));
            let style = entry.titlebar;
            let decorated = entry.server_side
                && !entry.fullscreen
                && self.config.window.decorations
                && plan.exact
                && (ins.top > 0.0 || style.client_drawn());
            let decoration_hit = |resize: bool| {
                let content = (f64::from(geo.size.w), f64::from(geo.size.h));
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                (
                    self.decor.hit_test(style, content, local, resize),
                    content,
                    local,
                )
            };
            // the window control buttons of a unified bar lie over the client's surface and take the pointer first
            if decorated && style.client_drawn() {
                let (hit, content, local) = decoration_hit(false);
                if matches!(hit, cupertino_decorations::Hit::Button(_)) {
                    return Some(WindowTarget::Decoration {
                        id,
                        style,
                        hit,
                        content,
                        local,
                    });
                }
            }
            // the popups of a window other than the one whose popups are raised lie above it and its resize band
            if Some(id) != raised {
                if let Some(popup) = Self::popup_of_window_under(id, entry, &plan, pos) {
                    return Some(popup);
                }
            }
            // the title bar revealed with the menu bar over a full-screen window: its buttons act, the rest takes the
            // pointer from the client
            if let Some(strip) = entry
                .revealed_titlebar(
                    &self.menu_bars,
                    &self.decor,
                    self.config.window.decorations,
                    out.id,
                    now,
                )
                .filter(|r| plan.exact && r.contains(MPoint::new(pos.x, pos.y)))
            {
                let content = (strip.w, strip.h);
                let local = (pos.x - strip.x, pos.y - strip.y);
                let style = WindowEntry::REVEALED_TITLEBAR;
                let hit = match self.decor.hit_test(style, content, local, false) {
                    hit @ cupertino_decorations::Hit::Button(_) => hit,
                    _ => cupertino_decorations::Hit::None,
                };
                return Some(WindowTarget::Decoration {
                    id,
                    style,
                    hit,
                    content,
                    local,
                });
            }
            // the resize zones, where the window can resize that way (the cursor follows; dropping the zone with the cursor is)
            let edge = if decorated {
                match decoration_hit(true).0 {
                    cupertino_decorations::Hit::Resize(edge) => Some(edge),
                    _ => None,
                }
            } else if plan.exact
                && (!entry.server_side || !self.config.window.decorations || ins.top == 0.0)
            {
                // a client-decorated toplevel draws its own title bar: titled, the band reaches outside its window
                // geometry; a window of ours without a title bar is untitled and has the inside band only
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                self.decor
                    .resize_hit((plan.visual.w, plan.visual.h), local, !entry.server_side)
            } else {
                None
            };
            if let Some(edge) = edge.filter(|e| self.resize_cursor(id, *e).is_some()) {
                let content = (f64::from(geo.size.w), f64::from(geo.size.h));
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                return Some(WindowTarget::Decoration {
                    id,
                    style,
                    hit: cupertino_decorations::Hit::Resize(edge),
                    content,
                    local,
                });
            }
            let v = plan.visual;
            let in_frame = pos.x >= v.x && pos.x < v.x + v.w && pos.y >= v.y && pos.y < v.y + v.h;
            if in_frame {
                if let Some((surface, loc)) = entry.window.surface_under(
                    pos - origin,
                    WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
                ) {
                    return Some(WindowTarget::Surface {
                        id,
                        surface,
                        origin: origin + loc.to_f64(),
                    });
                }
            }
            if decorated {
                let (hit, content, local) = decoration_hit(false);
                if matches!(
                    hit,
                    cupertino_decorations::Hit::Button(_) | cupertino_decorations::Hit::Titlebar
                ) {
                    return Some(WindowTarget::Decoration {
                        id,
                        style,
                        hit,
                        content,
                        local,
                    });
                }
            }
        }
        None
    }

    /// Window id under `pos` (global logical), with the surface and its origin, from the topmost
    /// window down. Popups of a window are part of it.
    pub fn window_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WindowId, WlSurface, Point<f64, Logical>)> {
        match self.window_target_under(pos)? {
            WindowTarget::Surface {
                id,
                surface,
                origin,
            } => Some((id, surface, origin)),
            WindowTarget::Decoration { .. } => None,
        }
    }

    /// Pointer focus target under `pos`: overlay layer surfaces, the popups of the other layer surfaces and of the key
    /// window, top layer surfaces, windows (with the popups of the others), layer surfaces below (the drawing order).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.session_lock.is_active() {
            return self.lock_surface_under(pos);
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let rel = pos - Point::from((out.rect.x, out.rect.y));
        let origin = Point::<f64, Logical>::from((out.rect.x, out.rect.y));
        let covered = crate::render::scene::top_layer_covered(&self.wm, out.id);
        let layer_hit = |layers: &[Layer]| {
            let map = layer_map_for_output(&out.output);
            // every surface of a layer, topmost first: one whose box holds the point but whose input region does not
            // (an overlay strip with an empty region) must not hide the surfaces below it
            for l in layers {
                for surface in crate::authorization::layers_stacked(&map, *l).rev() {
                    if surface
                        .wl_surface()
                        .client()
                        .is_some_and(|c| self.hosts.is_client_unresponsive(&c))
                    {
                        continue;
                    }
                    if *l == Layer::Top
                        && covered
                        && crate::render::scene::gives_way(surface, &self.menu_bars)
                    {
                        continue;
                    }
                    let geo = map.layer_geometry(surface)?;
                    // a menu bar is offset while it hides or reveals
                    let shift = Point::<f64, Logical>::from((
                        0.0,
                        self.menu_bars
                            .look(surface.wl_surface(), out.rect.y, now)
                            .map_or(0.0, |l| l.dy),
                    ));
                    if let Some((s, loc)) = surface
                        .surface_under(rel - geo.loc.to_f64() - shift, WindowSurfaceType::ALL)
                    {
                        return Some((s, origin + (geo.loc + loc).to_f64() + shift));
                    }
                }
            }
            None
        };
        if let Some(hit) = layer_hit(&[Layer::Overlay]) {
            return Some(hit);
        }
        // the popups of the bars and of the key window are drawn above every window and bar (`SceneLayer::Popups`): a
        // menu over the dock takes the pointer there
        let layer_popup_hit = || {
            let map = layer_map_for_output(&out.output);
            for l in [Layer::Top, Layer::Bottom, Layer::Background] {
                for surface in crate::authorization::layers_stacked(&map, l).rev() {
                    if surface
                        .wl_surface()
                        .client()
                        .is_some_and(|c| self.hosts.is_client_unresponsive(&c))
                    {
                        continue;
                    }
                    if l == Layer::Top
                        && covered
                        && crate::render::scene::gives_way(surface, &self.menu_bars)
                    {
                        continue;
                    }
                    let Some(geo) = map.layer_geometry(surface) else {
                        continue;
                    };
                    let shift = Point::<f64, Logical>::from((
                        0.0,
                        self.menu_bars
                            .look(surface.wl_surface(), out.rect.y, now)
                            .map_or(0.0, |l| l.dy),
                    ));
                    let base = geo.loc.to_f64() + shift;
                    for (popup, location) in PopupManager::popups_for_surface(surface.wl_surface())
                    {
                        let offset = location - popup.geometry().loc;
                        if let Some((s, loc)) = smithay::desktop::utils::under_from_surface_tree(
                            popup.wl_surface(),
                            rel - base,
                            offset,
                            WindowSurfaceType::ALL,
                        ) {
                            return Some((s, origin + base + loc.to_f64()));
                        }
                    }
                }
            }
            None
        };
        if let Some(hit) = layer_popup_hit() {
            return Some(hit);
        }
        if let Some(WindowTarget::Surface {
            surface, origin, ..
        }) = self.window_popup_under(pos)
        {
            return Some((surface, origin));
        }
        if let Some(hit) = layer_hit(&[Layer::Top]) {
            return Some(hit);
        }
        match self.window_target_under(pos) {
            Some(WindowTarget::Surface {
                surface, origin, ..
            }) => return Some((surface, origin)),
            Some(WindowTarget::Decoration { .. }) => return None,
            None => {}
        }
        layer_hit(&[Layer::Bottom, Layer::Background])
    }
}
