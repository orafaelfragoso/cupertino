use super::*;

impl Cupertino {
    /// Register a new host connection (shared or separate), returning the client-end Wayland socket fd.
    pub fn attach_host_connection(
        &mut self,
        kind: HostKind,
        components: Vec<&'static str>,
        holder: Holder,
    ) -> std::io::Result<OwnedFd> {
        let (server_end, client_end) = UnixStream::pair()?;
        let state = ClientState {
            capabilities: ClientCapabilitySet::of([ShellRole::Host]),
            ..ClientState::default()
        };
        let process = match &holder {
            Holder::Process(p) => p.try_clone().ok(),
            Holder::Detached(_) => ProcessHandle::open(std::process::id()).ok(),
        };
        let start_ticks = process
            .as_ref()
            .and_then(cupertino_peer::component::process_start_ticks);
        let _ = state.holder.set(holder);
        let _ = state.gone.set(self.gone_clients.clone());
        state.look_up_process();
        let client = crate::state::insert_client_handle(&self.display_handle, server_end, state)?;
        let client_id = client.id();
        let host_id = self.hosts.next_host_id;
        self.hosts.next_host_id += 1;
        self.hosts.host_by_client.insert(client_id.clone(), host_id);
        self.hosts.hosts.insert(
            host_id,
            HostEntry {
                id: host_id,
                kind,
                components,
                resource: None,
                component_objects: HashMap::new(),
                connect_pending: HashSet::new(),
                restart_requested_at: None,
                expected_exit_kill_at: None,
                kill_sent: false,
                process,
                start_ticks,
                last_bridge_at: None,
                bridge_retry_at: None,
            },
        );
        Ok(OwnedFd::from(client_end))
    }

    /// Replace the component list of `host_id` and resend `component`…`done` if bound.
    pub fn set_host_components(&mut self, host_id: u64, components: Vec<&'static str>) {
        let now = Instant::now();
        let Some(host) = self.hosts.hosts.get_mut(&host_id) else {
            return;
        };
        host.components = components;
        if let Some(res) = host.resource.clone() {
            for &name in &host.components {
                res.component(name.to_string());
            }
            res.done();
        }
        if matches!(host.kind, HostKind::Separate(_)) && host.components.is_empty() {
            host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
        }
        // Any inert `connect` that just entered the list can now be answered.
        let pending: Vec<&'static str> = host.connect_pending.iter().copied().collect();
        for name in pending {
            self.try_answer_component_connect(host_id, name, now);
        }
    }

    /// Ask to restart `component` (`cupertino shell restart NAME`): sends `restart` if running, or clears `gave_up`
    /// and answers a pending `connect` if stopped.
    pub fn request_component_restart(
        &mut self,
        component: &'static str,
        waiter: Option<RestartWaiter>,
    ) -> bool {
        let now = Instant::now();
        let Some(rec) = self.hosts.components.get_mut(component) else {
            return false;
        };
        rec.separate_for_session = false;
        if let Some(cb) = waiter {
            rec.restart_waiters.push((now + HOLD_READY_TIMEOUT, cb));
        }
        if rec.gave_up {
            rec.gave_up = false;
            rec.failures.clear();
            rec.consecutive_failures = 0;
            rec.backoff_until = None;
            // Find the host waiting on this component and answer connect.
            let host_id = self
                .hosts
                .hosts
                .values()
                .find(|h| h.components.contains(&component))
                .map(|h| h.id);
            if let Some(hid) = host_id {
                self.try_answer_component_connect(hid, component, now);
            }
            return true;
        }
        if let Some((_, host_id)) = rec.active {
            if let Some(obj) = self
                .hosts
                .hosts
                .get(&host_id)
                .and_then(|h| h.component_objects.get(component))
            {
                obj.restart();
                return true;
            }
        }
        false
    }

    /// Verify `org.cupertino.Session1.AnnounceComponent(nonce)` from `sender` / `caller` (D20).
    pub fn verify_component_bus_nonce(
        &mut self,
        sender: &str,
        caller: &Caller,
        nonce: &str,
    ) -> Result<(), Denial> {
        let now = Instant::now();
        let caller_proc = match caller {
            Caller::Process(id) => Some(&id.process),
            Caller::Unclassified(p) => Some(p),
            Caller::Anonymous => None,
        };

        // Locate the component that holds this pending nonce (single-use regardless of outcome).
        let target_comp = self.hosts.components.iter().find_map(|(&name, rec)| {
            rec.pending_nonce
                .as_ref()
                .is_some_and(|p| p.nonce == nonce)
                .then_some(name)
        });
        let Some(comp_name) = target_comp else {
            return Err(Denial::Untrusted);
        };
        let pending = self
            .hosts
            .components
            .get_mut(comp_name)
            .and_then(|r| r.pending_nonce.take())
            .expect("pending nonce was just matched");

        let fresh = now.saturating_duration_since(pending.issued_at) <= BUS_NONCE_TTL;
        let sender_matches = pending.name == sender;
        let host_matches = self
            .hosts
            .components
            .get(comp_name)
            .and_then(|r| r.active.as_ref().map(|(_, hid)| *hid))
            .and_then(|hid| self.hosts.hosts.get(&hid))
            .is_some_and(|host| match (&host.process, caller_proc) {
                (Some(host_proc), Some(caller_proc)) => host_proc.is(caller_proc),
                _ => false,
            });
        let duplicate = self.hosts.components.iter().any(|(&other, rec)| {
            other != comp_name && rec.accepted_bus_name.as_deref() == Some(sender)
        });

        if fresh && sender_matches && host_matches && !duplicate {
            if let Some(rec) = self.hosts.components.get_mut(comp_name) {
                rec.accepted_bus_name = Some(pending.name.clone());
            }
            pending
                .resource
                .bus_name(pending.name, BusNameResult::Accepted);
            Ok(())
        } else {
            pending
                .resource
                .bus_name(pending.name, BusNameResult::Refused);
            Err(Denial::Untrusted)
        }
    }

    /// Periodic host housekeeping: answer due backoffs, expire nonces and holds, answer quiet-moment restarts,
    /// create throttled bridges, and kill overdue expected-exit hosts.
    pub fn tick_hosts(&mut self, now: Instant) {
        // 1. Reset failure counters for components running stably for >= 60s.
        for rec in self.hosts.components.values_mut() {
            if rec.active.is_some()
                && rec
                    .started_at
                    .is_some_and(|s| now.saturating_duration_since(s) >= FAILURE_RESET_AFTER)
            {
                rec.consecutive_failures = 0;
                rec.failures.clear();
            }
        }

        // 2. Expire overdue bus nonces (5s).
        for rec in self.hosts.components.values_mut() {
            if rec
                .pending_nonce
                .as_ref()
                .is_some_and(|p| now.saturating_duration_since(p.issued_at) > BUS_NONCE_TTL)
            {
                if let Some(p) = rec.pending_nonce.take() {
                    p.resource.bus_name(p.name, BusNameResult::Refused);
                }
            }
        }

        // 3. Expire un-taken-over holds after 10s without `ready`, and fail timed-out restart waiters.
        let mut expired_hold_components = Vec::new();
        for (&name, rec) in &mut self.hosts.components {
            if rec.hold_expire_at.is_some_and(|at| now >= at) {
                rec.hold_expire_at = None;
                expired_hold_components.push(name);
            }
            let mut keep = Vec::new();
            for (deadline, cb) in std::mem::take(&mut rec.restart_waiters) {
                if now >= deadline {
                    cb(Err(format!("{name}: started and not ready")));
                } else {
                    keep.push((deadline, cb));
                }
            }
            rec.restart_waiters = keep;
        }
        for name in expired_hold_components {
            self.release_component_holds(name);
        }

        // 4. Answer due pending `connect` requests once backoff elapses.
        let pending_connects: Vec<(u64, &'static str)> = self
            .hosts
            .hosts
            .values()
            .flat_map(|h| h.connect_pending.iter().map(move |&c| (h.id, c)))
            .collect();
        for (hid, comp) in pending_connects {
            self.try_answer_component_connect(hid, comp, now);
        }

        // 5. Answer shared-host `request_restart` once quiet for 2s (and no grabs/transients) or 60s max.
        let idle_ok = now.saturating_duration_since(self.hosts.last_input_at) >= RESTART_QUIET_IDLE;
        let restart_ready: Vec<u64> = self
            .hosts
            .hosts
            .values()
            .filter_map(|h| {
                let req = h.restart_requested_at?;
                let max_hit = now.saturating_duration_since(req) >= RESTART_QUIET_MAX;
                let quiet = match h.kind {
                    HostKind::Separate(_) => true,
                    HostKind::Shared => idle_ok && !self.host_has_active_grab_or_transient(h.id),
                };
                (quiet || max_hit).then_some(h.id)
            })
            .collect();
        for hid in restart_ready {
            if let Some(host) = self.hosts.hosts.get_mut(&hid) {
                host.restart_requested_at = None;
                host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                if let Some(res) = &host.resource {
                    res.restart();
                }
            }
        }

        // 6. Kill expected-exit hosts past their 2s grace, once. The deadline stays set: the exit that follows is
        //    still expected, not a crash (D16). A host whose holder is detached names the server's own process.
        for host in self.hosts.hosts.values_mut() {
            if !host.kill_sent && host.expected_exit_kill_at.is_some_and(|at| now >= at) {
                host.kill_sent = true;
                if let Some(proc) = &host.process {
                    if !proc.has_exited() && proc.pid != std::process::id() {
                        tracing::warn!(
                            pid = proc.pid,
                            "killing shell host that did not exit in time"
                        );
                        let _ = proc.send(libc::SIGKILL);
                    }
                }
            }
        }

        // 7. Create throttled host-bus bridges whose retry time has arrived.
        let due_bridges: Vec<u64> = self
            .hosts
            .hosts
            .values()
            .filter_map(|h| {
                h.bridge_retry_at
                    .is_some_and(|at| now >= at)
                    .then_some(h.id)
            })
            .collect();
        for hid in due_bridges {
            if let Some(h) = self.hosts.hosts.get_mut(&hid) {
                h.bridge_retry_at = None;
            }
            self.create_bridge_for_separate_host(hid, now);
        }
    }

    /// True if any component running in `host_id` currently has a mapped transient layer surface or popup grab.
    fn host_has_active_grab_or_transient(&self, host_id: u64) -> bool {
        let host_clients: HashSet<ClientId> = self
            .hosts
            .components
            .values()
            .filter_map(|r| {
                r.active
                    .as_ref()
                    .filter(|(_, hid)| *hid == host_id)
                    .map(|(cid, _)| cid.clone())
            })
            .collect();
        if host_clients.is_empty() {
            return false;
        }
        // Check popup grabs.
        if let Some(ptr) = self.seat.get_pointer() {
            if ptr.is_grabbed() {
                return true;
            }
        }
        // Check mapped non-persisted (transient) layer surfaces belonging to this host's components.
        for o in &self.outputs {
            let map = layer_map_for_output(&o.output);
            for layer in map.layers() {
                let surf = layer.wl_surface();
                if let Some(client) = surf.client() {
                    if host_clients.contains(&client.id())
                        && !self.hosts.persisted.contains_key(&surf.id())
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Called from `CompositorHandler::commit` for every surface: a persisted surface's first buffer commit takes its
    /// hold over atomically. Any other surface costs one lookup; no picture is taken on a commit.
    pub fn host_surface_committed(&mut self, surface: &WlSurface) {
        let surface_id = surface.id();
        if !self.hosts.persisted.contains_key(&surface_id) {
            return;
        }
        let has_buffer = with_states(surface, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|d| d.lock().ok().map(|s| s.buffer().is_some()))
                .unwrap_or(false)
                || states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer
                    .is_some()
        });
        if !has_buffer {
            return;
        }
        let takeover = {
            let Some(persisted) = self.hosts.persisted.get_mut(&surface_id) else {
                return;
            };
            if !persisted.mapped {
                persisted.mapped = true;
                Some((persisted.component, persisted.output, persisted.key.clone()))
            } else {
                None
            }
        };
        if let Some((component, output, key)) = takeover {
            let before = self.hosts.holds.len();
            self.hosts
                .holds
                .retain(|h| !(h.component == component && h.output == output && h.key == key));
            if self.hosts.holds.len() != before {
                tracing::debug!(component, %key, "held surface taken over atomically on first buffer commit");
                if component == "windowmanager" {
                    self.take_over_wm_placements(output);
                }
                self.request_redraw();
            }
        }
    }

    /// Called from `WlrLayerShellHandler::layer_destroyed` BEFORE unmapping `layer` from `output`:
    /// if the surface was persisted and its component is not stopped, capture it into a [`HeldSurface`] so its
    /// reserved space and last picture stay until takeover or `ready`/timeout.
    pub fn host_layer_destroyed(
        &mut self,
        output: &smithay::output::Output,
        layer: &DesktopLayerSurface,
    ) {
        let surface = layer.wl_surface();
        let surface_id = surface.id();
        let Some(mut persisted) = self.hosts.persisted.remove(&surface_id) else {
            return;
        };
        let gave_up = self
            .hosts
            .components
            .get(persisted.component)
            .is_some_and(|r| r.gave_up);
        if gave_up {
            return;
        }
        // the picture taken as the `wl_surface` went, or, while it still shows, one taken now
        let picture = persisted
            .last_picture
            .take()
            .or_else(|| self.capture_held_picture(surface));
        let Some(output_id) = self.output_by_wl(output).map(|o| o.id) else {
            return;
        };
        let map = layer_map_for_output(output);
        let Some(geometry) = map.layer_geometry(layer) else {
            return;
        };
        let (wlr_layer, anchor, exclusive_zone, margin) = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
            let cur = guard.current();
            (cur.layer, cur.anchor, cur.exclusive_zone, cur.margin)
        });
        // Replace any older hold with the same (component, output, key).
        self.hosts.holds.retain(|h| {
            !(h.component == persisted.component && h.output == output_id && h.key == persisted.key)
        });
        self.hosts.holds.push(HeldSurface {
            component: persisted.component,
            key: persisted.key,
            output: output_id,
            layer: wlr_layer,
            geometry,
            anchor,
            exclusive_zone,
            margin,
            picture,
            held_since: Instant::now(),
        });
        if persisted.component == "windowmanager" {
            self.hosts.hold_wm_placements(output_id);
        }
        self.request_redraw();
    }

    /// Release all holds owned by `component` (`ready`, the hold timeout, give-up) and refresh usable areas on
    /// affected outputs. For the window manager, placements held on an output no surface took over end with them.
    pub fn release_component_holds(&mut self, component: &'static str) {
        let mut outputs = HashSet::new();
        self.hosts.holds.retain(|h| {
            if h.component == component {
                outputs.insert(h.output);
                false
            } else {
                true
            }
        });
        if component == "windowmanager" {
            let held: Vec<OutputId> = self
                .hosts
                .held_wm_outputs
                .iter()
                .filter(|(_, s)| **s == WmPlacementHold::Held)
                .map(|(o, _)| *o)
                .collect();
            for out in held {
                self.end_held_wm_placements(out);
            }
        }
        for out in outputs {
            self.refresh_usable_area(out);
        }
        self.request_redraw();
    }

    /// Dismiss all non-persisted (transient) layer surfaces belonging to `client_id`, release any grabs it holds,
    /// and restore keyboard focus to the key window if `client_id` held focus.
    pub fn dismiss_component_transients_and_grabs(&mut self, client_id: &ClientId) {
        // Release pointer/keyboard grabs if held by this client (popup grab or similar).
        let pointer_on_client = self
            .seat
            .get_pointer()
            .and_then(|p| p.current_focus())
            .and_then(|s| s.client())
            .is_some_and(|c| c.id() == *client_id);
        if pointer_on_client {
            if let Some(ptr) = self.seat.get_pointer() {
                ptr.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), 0);
            }
        }
        let kbd_on_client = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|s| s.client())
            .is_some_and(|c| c.id() == *client_id);
        if kbd_on_client {
            if let Some(kbd) = self.seat.get_keyboard() {
                kbd.unset_grab(self);
            }
        }

        // Dismiss all transient (non-persisted) layer surfaces of `client_id`.
        let mut to_dismiss: Vec<(OutputId, smithay::output::Output, DesktopLayerSurface)> =
            Vec::new();
        for o in &self.outputs {
            let map = layer_map_for_output(&o.output);
            for layer in map.layers() {
                let surf = layer.wl_surface();
                if surf.client().is_some_and(|c| c.id() == *client_id)
                    && !self.hosts.persisted.contains_key(&surf.id())
                {
                    to_dismiss.push((o.id, o.output.clone(), layer.clone()));
                }
            }
        }
        let mut touched_outputs = HashSet::new();
        for (oid, output, layer) in to_dismiss {
            layer_map_for_output(&output).unmap_layer(&layer);
            self.layer_keyboard_exclusivity(layer.wl_surface(), false);
            layer.layer_surface().send_close();
            touched_outputs.insert(oid);
        }
        for oid in touched_outputs {
            self.refresh_usable_area(oid);
        }
        if kbd_on_client {
            self.restore_keyboard_focus();
        }
        self.refocus_pointer();
        self.request_redraw();
    }

    /// Clean up host or component state when `client_id` disconnects.
    pub fn host_client_disconnected(&mut self, client_id: &ClientId) {
        let now = Instant::now();

        // Case 1: Host connection ended.
        if let Some(host_id) = self.hosts.host_by_client.remove(client_id) {
            if let Some(host) = self.hosts.hosts.remove(&host_id) {
                let expected = host.expected_exit_kill_at.is_some();
                match host.kind {
                    HostKind::Separate(comp) => {
                        if let Some(shared) = self.shared_host_resource() {
                            shared.bus_peer_gone(comp.to_string());
                        }
                    }
                    HostKind::Shared => {
                        if !expected {
                            self.hosts.shared_host_crashes.retain(|t| {
                                now.saturating_duration_since(*t) <= SHARED_HOST_CRASH_WINDOW
                            });
                            self.hosts.shared_host_crashes.push(now);
                            if self.hosts.shared_host_crashes.len()
                                >= SHARED_HOST_CRASH_ISOLATE_COUNT
                            {
                                tracing::warn!(
                                    "shared shell host crashed {} times within 60s: isolating components in separate hosts for this session",
                                    self.hosts.shared_host_crashes.len()
                                );
                                self.hosts.all_separate_for_session = true;
                            }
                        }
                    }
                }
            }
            return;
        }

        // Case 2: Component connection ended.
        let comp_name = self.hosts.components.iter().find_map(|(&name, rec)| {
            rec.active
                .as_ref()
                .is_some_and(|(cid, _)| cid == client_id)
                .then_some(name)
        });
        if let Some(name) = comp_name {
            self.dismiss_component_transients_and_grabs(client_id);
            if let Some(rec) = self.hosts.components.get_mut(name) {
                rec.active = None;
                rec.component_resource = None;
                rec.liveness = Liveness::Responsive;
                rec.accepted_bus_name = None;
                rec.pending_nonce = None;
                rec.ready = false;
            }
        }
    }

    fn shared_host_resource(&self) -> Option<CupertinoHostV1> {
        self.hosts
            .hosts
            .values()
            .find(|h| h.kind == HostKind::Shared)
            .and_then(|h| h.resource.clone())
    }

    fn create_bridge_for_separate_host(&mut self, separate_host_id: u64, now: Instant) {
        let Some(shared_res) = self.shared_host_resource() else {
            return;
        };
        let Some(sep) = self.hosts.hosts.get_mut(&separate_host_id) else {
            return;
        };
        let HostKind::Separate(comp) = sep.kind else {
            return;
        };
        let Some(sep_res) = sep.resource.clone() else {
            return;
        };
        if let Some(last) = sep.last_bridge_at {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < BRIDGE_THROTTLE {
                sep.bridge_retry_at = Some(last + BRIDGE_THROTTLE);
                return;
            }
        }
        match bridge_socketpair() {
            Ok((shared_fd, sep_fd)) => {
                sep.last_bridge_at = Some(now);
                shared_res.bus_peer(comp.to_string(), shared_fd.as_fd());
                sep_res.bus(sep_fd.as_fd());
            }
            Err(e) => {
                tracing::warn!(component = comp, error = %e, "creating host bus bridge socketpair failed");
            }
        }
    }

    fn record_component_failure(&mut self, component: &'static str, lost: bool, now: Instant) {
        let mut gave_up_now = false;
        if let Some(rec) = self.hosts.components.get_mut(component) {
            // Loss window rule (D16): exclude the run duration of an instance lost within 60s of start so 5 consecutive
            // startup hangs still trigger give-up within the rolling 60s window.
            let stamp = if lost {
                match rec.started_at {
                    Some(started) if now.saturating_duration_since(started) < GIVE_UP_WINDOW => {
                        started
                    }
                    _ => now,
                }
            } else {
                now
            };
            rec.failures
                .retain(|t| stamp.saturating_duration_since(*t) <= GIVE_UP_WINDOW);
            rec.failures.push(stamp);
            rec.consecutive_failures = rec.consecutive_failures.saturating_add(1);
            if rec.failures.len() >= GIVE_UP_FAILURES {
                rec.gave_up = true;
                rec.backoff_until = None;
                gave_up_now = true;
            } else {
                rec.backoff_until = Some(now + backoff_for(rec.consecutive_failures));
            }
            if lost {
                rec.separate_for_session = true;
            }
        }
        if gave_up_now {
            tracing::warn!(component, "component hit 5 failures within 60s: giving up");
            self.release_component_holds(component);
            if component == "windowmanager" {
                // stopped: no instance takes anything over, so placements awaiting a first commit end too
                let outputs: Vec<OutputId> = self.hosts.held_wm_outputs.keys().copied().collect();
                for out in outputs {
                    self.end_held_wm_placements(out);
                }
            }
            if let Some(shared) = self.shared_host_resource() {
                shared.component_stopped(component.to_string());
            }
        }
    }

    fn try_answer_component_connect(
        &mut self,
        host_id: u64,
        component: &'static str,
        now: Instant,
    ) {
        let (in_list, obj, host_holder) = {
            let Some(host) = self.hosts.hosts.get(&host_id) else {
                return;
            };
            if !host.connect_pending.contains(component) {
                return;
            }
            let in_list = host.components.contains(&component);
            let obj = host.component_objects.get(component).cloned();
            let holder = match &host.process {
                Some(p) => p.try_clone().ok().map(Holder::Process),
                None => Some(Holder::Detached(
                    now + crate::authorization::DETACHED_LIFETIME,
                )),
            };
            (in_list, obj, holder)
        };
        if !in_list {
            return;
        }
        let Some(obj) = obj else {
            return;
        };
        let (gave_up, backoff_until) = match self.hosts.components.get(component) {
            Some(rec) => (rec.gave_up, rec.backoff_until),
            None => return,
        };
        if gave_up {
            if let Some(host) = self.hosts.hosts.get_mut(&host_id) {
                host.connect_pending.remove(component);
            }
            obj.stopped(StopReason::GaveUp);
            return;
        }
        if backoff_until.is_some_and(|at| now < at) {
            return;
        }

        // Close any prior active connection for this component so roles live in one host at a time.
        let prior_client = self
            .hosts
            .components
            .get(component)
            .and_then(|r| r.active.as_ref().map(|(cid, _)| cid.clone()));
        if let Some(old_cid) = prior_client {
            self.display_handle.backend_handle().kill_client(
                old_cid.clone(),
                smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
            );
            self.host_client_disconnected(&old_cid);
        }

        let Some(spec) = component_spec(component) else {
            return;
        };
        let Ok((server_end, client_end)) = UnixStream::pair() else {
            tracing::warn!(component, "creating component Wayland socketpair failed");
            return;
        };
        let instance = {
            let rec = self
                .hosts
                .components
                .get_mut(component)
                .expect("component exists in table");
            rec.instance = rec.instance.saturating_add(1);
            rec.instance
        };
        let state = ClientState {
            capabilities: ClientCapabilitySet::of(spec.roles.iter().copied()),
            component: Some(ComponentConnection {
                name: component,
                instance,
                host_id,
            }),
            ..ClientState::default()
        };
        if let Some(holder) = host_holder {
            let _ = state.holder.set(holder);
        }
        let _ = state.gone.set(self.gone_clients.clone());
        state.look_up_process();
        let Ok(client) =
            crate::state::insert_client_handle(&self.display_handle, server_end, state)
        else {
            tracing::warn!(component, "inserting component Wayland client failed");
            return;
        };
        let client_id = client.id();
        if let Some(host) = self.hosts.hosts.get_mut(&host_id) {
            host.connect_pending.remove(component);
        }
        if let Some(rec) = self.hosts.components.get_mut(component) {
            rec.active = Some((client_id, host_id));
            rec.started_at = Some(now);
            rec.liveness = Liveness::Responsive;
            rec.ready = false;
            rec.hold_expire_at = Some(now + HOLD_READY_TIMEOUT);
        }
        let client_fd = OwnedFd::from(client_end);
        obj.connection(client_fd.as_fd());
    }
}

impl GlobalDispatch<CupertinoHostV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoHostV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let Some(&host_id) = state.hosts.host_by_client.get(&client.id()) else {
            return;
        };
        let host_obj = init.init(resource, HostGlobalData { host_id });
        let (kind, components) = {
            let Some(host) = state.hosts.hosts.get_mut(&host_id) else {
                return;
            };
            host.resource = Some(host_obj.clone());
            (host.kind, host.components.clone())
        };
        host_obj.kind(match kind {
            HostKind::Shared => WireHostKind::Shared,
            HostKind::Separate(_) => WireHostKind::Separate,
        });
        for name in components {
            host_obj.component(name.to_string());
        }
        host_obj.done();

        let now = Instant::now();
        match kind {
            HostKind::Separate(_) => state.create_bridge_for_separate_host(host_id, now),
            HostKind::Shared => {
                let separate_ids: Vec<u64> = state
                    .hosts
                    .hosts
                    .values()
                    .filter(|h| matches!(h.kind, HostKind::Separate(_)) && h.resource.is_some())
                    .map(|h| h.id)
                    .collect();
                for sid in separate_ids {
                    state.create_bridge_for_separate_host(sid, now);
                }
            }
        }
    }

    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::Host)
    }
}

impl Dispatch<CupertinoHostV1, HostGlobalData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoHostV1,
        request: cupertino_host_v1::Request,
        data: &HostGlobalData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        match request {
            cupertino_host_v1::Request::Destroy => {
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.resource = None;
                }
            }
            cupertino_host_v1::Request::GetComponent { id, name } => {
                let Some(spec) = component_spec(&name) else {
                    resource.post_error(HostError::Name, format!("unknown component name: {name}"));
                    return;
                };
                let already = state
                    .hosts
                    .hosts
                    .get(&data.host_id)
                    .is_some_and(|h| h.component_objects.contains_key(spec.name));
                if already {
                    resource.post_error(
                        HostError::Component,
                        format!("host already holds an object for {}", spec.name),
                    );
                    return;
                }
                let obj = init.init(
                    id,
                    HostComponentData {
                        host_id: data.host_id,
                        component: spec.name,
                    },
                );
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.component_objects.insert(spec.name, obj);
                }
            }
            cupertino_host_v1::Request::RequestRestart => {
                let Some(host) = state.hosts.hosts.get_mut(&data.host_id) else {
                    return;
                };
                match host.kind {
                    HostKind::Separate(_) => {
                        host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                        resource.restart();
                    }
                    HostKind::Shared => {
                        if host.restart_requested_at.is_none() {
                            host.restart_requested_at = Some(now);
                        }
                    }
                }
            }
            cupertino_host_v1::Request::BusDropped { component } => {
                let Some(spec) = component_spec(&component) else {
                    resource.post_error(
                        HostError::Name,
                        format!("unknown component name: {component}"),
                    );
                    return;
                };
                let Some(host) = state.hosts.hosts.get(&data.host_id) else {
                    return;
                };
                match host.kind {
                    HostKind::Separate(own) if own != spec.name => {
                        resource.post_error(
                            HostError::Kind,
                            format!(
                                "separate host for {own} cannot report bus_dropped for {}",
                                spec.name
                            ),
                        );
                    }
                    HostKind::Separate(_) => {
                        state.create_bridge_for_separate_host(data.host_id, now);
                    }
                    HostKind::Shared => {
                        let sep_id = state
                            .hosts
                            .hosts
                            .values()
                            .find(|h| h.kind == HostKind::Separate(spec.name))
                            .map(|h| h.id);
                        if let Some(sid) = sep_id {
                            state.create_bridge_for_separate_host(sid, now);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, HostComponentData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoHostComponentV1,
        request: cupertino_host_component_v1::Request,
        data: &HostComponentData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        let component = data.component;
        match request {
            cupertino_host_component_v1::Request::Destroy => {
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.component_objects.remove(component);
                    host.connect_pending.remove(component);
                }
                let active_here = state.hosts.components.get(component).and_then(|r| {
                    r.active
                        .as_ref()
                        .filter(|(_, hid)| *hid == data.host_id)
                        .map(|(cid, _)| cid.clone())
                });
                if let Some(cid) = active_here {
                    state.display_handle.backend_handle().kill_client(
                        cid.clone(),
                        smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
                    );
                    state.host_client_disconnected(&cid);
                }
            }
            cupertino_host_component_v1::Request::Connect => {
                // Close any existing connection for this component in this host first.
                let active_here = state.hosts.components.get(component).and_then(|r| {
                    r.active
                        .as_ref()
                        .filter(|(_, hid)| *hid == data.host_id)
                        .map(|(cid, _)| cid.clone())
                });
                if let Some(cid) = active_here {
                    state.display_handle.backend_handle().kill_client(
                        cid.clone(),
                        smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
                    );
                    state.host_client_disconnected(&cid);
                }
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    if !host.connect_pending.insert(component) {
                        // Already outstanding: idempotent.
                        return;
                    }
                }
                state.try_answer_component_connect(data.host_id, component, now);
            }
            cupertino_host_component_v1::Request::Busy => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Busy;
                    }
                }
            }
            cupertino_host_component_v1::Request::Unresponsive => {
                let active_cid = state.hosts.components.get_mut(component).and_then(|rec| {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Unresponsive;
                        rec.active.as_ref().map(|(cid, _)| cid.clone())
                    } else {
                        None
                    }
                });
                if let Some(cid) = active_cid {
                    state.dismiss_component_transients_and_grabs(&cid);
                }
            }
            cupertino_host_component_v1::Request::Responsive => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Responsive;
                    }
                }
                state.refocus_pointer();
            }
            cupertino_host_component_v1::Request::Lost => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    rec.liveness = Liveness::Lost;
                }
                state.record_component_failure(component, true, now);
                let kind = state.hosts.hosts.get(&data.host_id).map(|h| h.kind);
                match kind {
                    Some(HostKind::Shared) => {
                        let new_list: Vec<&'static str> = state
                            .hosts
                            .hosts
                            .get(&data.host_id)
                            .map(|h| {
                                h.components
                                    .iter()
                                    .copied()
                                    .filter(|&c| c != component)
                                    .collect()
                            })
                            .unwrap_or_default();
                        state.set_host_components(data.host_id, new_list);
                        if !state.hosts.component_gave_up(component) {
                            state.hosts.requested_separate_hosts.push(component);
                        }
                    }
                    Some(HostKind::Separate(_)) => {
                        if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                            host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                        }
                        if !state.hosts.component_gave_up(component) {
                            state.hosts.requested_separate_hosts.push(component);
                        }
                    }
                    None => {}
                }
            }
            cupertino_host_component_v1::Request::Failed { cause } => {
                let WEnum::Value(cause) = cause else {
                    resource.post_error(HostComponentError::Cause, "invalid failure cause");
                    return;
                };
                tracing::warn!(component, ?cause, "shell component failed");
                state.record_component_failure(component, false, now);
            }
            cupertino_host_component_v1::Request::Unavailable { reason } => {
                let WEnum::Value(reason) = reason else {
                    resource.post_error(HostComponentError::Reason, "invalid unavailable reason");
                    return;
                };
                tracing::warn!(component, ?reason, "shell component unavailable in host");
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.connect_pending.remove(component);
                }
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<CupertinoComponentV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoComponentV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let Some(conn) = client.get_data::<ClientState>().and_then(|s| s.component) else {
            return;
        };
        let obj = init.init(
            resource,
            ComponentGlobalData {
                component: conn.name,
                instance: conn.instance,
            },
        );
        if let Some(rec) = state.hosts.components.get_mut(conn.name) {
            rec.component_resource = Some(obj.clone());
        }
        obj.component(conn.name.to_string(), conn.instance);
        let keys: Vec<String> = state
            .hosts
            .holds
            .iter()
            .filter(|h| h.component == conn.name)
            .map(|h| h.key.clone())
            .collect();
        for key in keys {
            obj.hold(key);
        }
        obj.done();
    }

    fn can_view(client: Client, _: &()) -> bool {
        client
            .get_data::<ClientState>()
            .is_some_and(|s| s.component.is_some())
    }
}

impl Dispatch<CupertinoComponentV1, ComponentGlobalData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoComponentV1,
        request: cupertino_component_v1::Request,
        data: &ComponentGlobalData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        match request {
            cupertino_component_v1::Request::Destroy => {
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    if rec
                        .component_resource
                        .as_ref()
                        .is_some_and(|r| r == resource)
                    {
                        rec.component_resource = None;
                    }
                }
            }
            cupertino_component_v1::Request::AnnounceBusName { name } => {
                if !is_unique_bus_name(&name) {
                    resource.post_error(
                        ComponentError::Name,
                        format!("not a valid unique bus name: {name}"),
                    );
                    return;
                }
                let nonce = generate_nonce();
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    // Re-announcing immediately revokes any prior accepted bus name.
                    rec.accepted_bus_name = None;
                    rec.pending_nonce = None;
                }
                let Some(nonce) = nonce else {
                    // fail closed: without a random nonce the name cannot be verified
                    resource.bus_name(name, BusNameResult::Refused);
                    return;
                };
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    rec.pending_nonce = Some(PendingBusNonce {
                        name,
                        nonce: nonce.clone(),
                        issued_at: now,
                        resource: resource.clone(),
                    });
                }
                resource.bus_nonce(nonce);
            }
            cupertino_component_v1::Request::Persist { surface, key } => {
                if !surface.is_alive() || surface.client().as_ref() != Some(client) {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "surface does not belong to this client or is gone",
                    );
                    return;
                }
                if get_role(&surface) != Some(LAYER_SURFACE_ROLE) {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "surface is not a layer shell surface",
                    );
                    return;
                }
                let already_mapped = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<RendererSurfaceStateUserData>()
                        .and_then(|d| d.lock().ok().map(|s| s.buffer().is_some()))
                        .unwrap_or(false)
                        || states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer
                            .is_some()
                });
                if already_mapped {
                    resource.post_error(
                        ComponentError::Mapped,
                        "persist must be sent before the surface commits its first buffer",
                    );
                    return;
                }
                if key.is_empty() || key.len() > MAX_PERSIST_KEY_BYTES {
                    resource.post_error(ComponentError::Key, "persist key must be 1..=64 bytes");
                    return;
                }
                // Find the output this layer surface is mapped on.
                let output_id = state.outputs.iter().find_map(|o| {
                    let map = layer_map_for_output(&o.output);
                    let found = map.layers().any(|l| l.wl_surface() == &surface);
                    found.then_some(o.id)
                });
                let Some(output_id) = output_id else {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "layer surface is not mapped on any output",
                    );
                    return;
                };
                let client_id = client.id();
                let dup = state.hosts.persisted.values().any(|p| {
                    p.client_id == client_id
                        && p.output == output_id
                        && p.key == key
                        && p.surface_id != surface.id()
                });
                if dup {
                    resource.post_error(
                        ComponentError::Key,
                        format!("duplicate persist key `{key}` on the same output"),
                    );
                    return;
                }
                state.hosts.persisted.insert(
                    surface.id(),
                    PersistedSurface {
                        component: data.component,
                        client_id,
                        surface_id: surface.id(),
                        key,
                        output: output_id,
                        mapped: false,
                        last_picture: None,
                    },
                );
            }
            cupertino_component_v1::Request::Ready => {
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    if rec.instance == data.instance {
                        rec.ready = true;
                        rec.hold_expire_at = None;
                        for (_, cb) in std::mem::take(&mut rec.restart_waiters) {
                            cb(Ok(()));
                        }
                    }
                }
                state.release_component_holds(data.component);
            }
            _ => {}
        }
    }
}
