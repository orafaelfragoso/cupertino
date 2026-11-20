use super::*;

impl WindowSystem for WaylandWindowSystem {
    fn semantics_active(&self, window: WindowId) -> bool {
        self.semantic_surfaces
            .get(&window)
            .is_some_and(|s| s.active())
    }
    fn publish_semantics(
        &mut self,
        window: WindowId,
        snapshot: &cupertino_ui_core::platform::semantics::Snapshot,
    ) {
        if let Some(s) = self.semantic_surfaces.get(&window) {
            s.publish(snapshot);
        }
    }
    fn semantic_requests(
        &mut self,
    ) -> Vec<(WindowId, cupertino_ui_core::platform::semantics::ActionCall)> {
        let requests: Vec<_> = self
            .semantic_surfaces
            .iter()
            .flat_map(|(w, s)| s.requests().into_iter().map(|r| (*w, r)))
            .collect();
        requests
            .into_iter()
            .map(|(window, request)| {
                if let Some(serial) = request.grab_serial {
                    let root = self.root(window);
                    self.action_grabs
                        .record(window, root, request.call.id, serial);
                }
                (window, request.call)
            })
            .collect()
    }
    fn semantic_finished(
        &mut self,
        window: WindowId,
        call: &cupertino_ui_core::platform::semantics::ActionCall,
        outcome: cupertino_ui_core::platform::semantics::Outcome,
        revision: u64,
    ) {
        // answered as applied, the action's work may still show a popup a moment later; any other answer ends the grab
        let applied = outcome == cupertino_ui_core::platform::semantics::Outcome::Applied;
        self.action_grabs
            .answered(window, call.id, applied, Instant::now());
        if let Some(s) = self.semantic_surfaces.get(&window) {
            s.complete(call, outcome, revision);
        }
    }

    fn capabilities(&self) -> Capabilities {
        let has = |c| self.conn.capabilities().contains(c);
        Capabilities {
            fractional_scale: has(Capability::FractionalScale),
            presentation_time: has(Capability::Presentation),
            text_input: has(Capability::TextInput),
            touch: false,
            cursor_shape: has(Capability::CursorShape),
            primary_selection: false,
            drag_and_drop: has(Capability::Clipboard) && has(Capability::Seat),
            server_decorations: has(Capability::ServerDecoration),
            activation: has(Capability::Activation),
            app_effects: self.effects.is_some(),
            raised_layer: layers_drawn(&self.conn, self.effects.as_ref().map(|(e, _)| e)),
            present_animation: self.presentations.available() && self.roles.is_some(),
            present_popups: self.presentations.popups(),
            surface_layers: self.surface_layers_drawn(),
            surface_layer_edges: self.surface_layers_drawn()
                && self
                    .presentations
                    .layers()
                    .is_some_and(cupertino_wayland_client::app::Layers::draws_glass_edges),
        }
    }

    fn opening(&mut self, key: &WindowKey) {
        if self.roles.is_some() {
            self.opening_key = Some(key.clone());
        }
    }

    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError> {
        let key = self.opening_role();
        if let Some(surface) = self
            .roles
            .as_mut()
            .and_then(|roles| roles.create(&mut self.conn, spec))
        {
            let surface = surface.map_err(platform_error)?;
            let id = self.add(
                key,
                surface,
                Kind::External,
                None,
                convert::whole(spec.size),
            );
            self.window_mut(id)?.input_region = spec.input_region.clone().map(Some);
            return Ok(id);
        }
        let toplevel = self
            .conn
            .toplevel(&ToplevelConfig {
                app_id: spec.app_id.clone(),
                title: spec.title.clone(),
                min_size: spec.min_size.map(convert::whole),
                max_size: spec.max_size.map(convert::whole),
                decorations: convert::decorations(spec.decorations),
                foreign_parent: spec.foreign_parent.as_ref().map(convert::foreign_parent),
            })
            .map_err(platform_error)?;
        // the style goes with the surface's first commit, before any buffer
        if let (Some(style), Some((effects, _))) = (
            convert::window_style(spec.decorations),
            self.effects.as_ref(),
        ) {
            effects
                .set_window_style(&self.conn, toplevel.surface(), style)
                .map_err(|e| PlatformError::Protocol(e.to_string()))?;
        }
        let id = self.add(
            key,
            toplevel.surface(),
            Kind::Toplevel(toplevel),
            None,
            convert::whole(spec.size),
        );
        let w = self.window_mut(id)?;
        (w.min_size, w.max_size) = (spec.min_size, spec.max_size);
        w.app_id = spec.app_id.clone();
        w.input_region = spec.input_region.clone().map(Some);
        Ok(id)
    }

    fn can_grab(&self, parent: WindowId) -> bool {
        self.conn.input_serial().is_some() || self.action_grab(parent).is_some()
    }

    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError> {
        let key = self.opening_role();
        let surface = self.window(parent)?.surface;
        let placement = convert::placement(spec);
        // a menu a desktop-control action shows grabs with the serial the action came with, not a stale or missing one
        let action = self.action_grab(parent).filter(|_| spec.grab);
        let popup = match action {
            Some(serial) => self
                .conn
                .popup_with_grab_serial(surface, &placement, serial),
            None => self.conn.popup(surface, &placement),
        }
        .map_err(platform_error)?;
        // the compositor takes it once, for the menu that opens from a window; the menu's own popups grow its grab
        if let Some(serial) =
            action.filter(|_| self.window(parent).is_ok_and(|w| w.parent.is_none()))
        {
            self.action_grabs.spend(serial);
        }
        Ok(self.add(
            key,
            popup.surface(),
            Kind::Popup(popup),
            Some(parent),
            convert::whole(spec.size),
        ))
    }

    fn destroy(&mut self, window: WindowId) {
        self.semantic_surfaces.remove(&window);
        self.action_grabs.forget(window);
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        // the announcement goes before its surface
        if let Ok(w) = self.window_mut(window) {
            w.menus = None;
        }
        let mut gone_surfaces = vec![surface];
        let mut parents = vec![window];
        while let Some(child) = self
            .windows
            .iter()
            .find(|w| w.parent.is_some_and(|p| parents.contains(&p)) && !parents.contains(&w.id))
        {
            parents.push(child.id);
            gone_surfaces.push(child.surface);
        }
        // and the subsurfaces of their raised layers
        gone_surfaces.extend(
            self.windows
                .iter()
                .filter(|w| parents.contains(&w.id))
                .filter_map(|w| w.layer.as_ref().map(|l| l.surface)),
        );
        for s in gone_surfaces {
            self.gpu.forget(&mut self.conn, s);
        }
        // the presentation and the layers go before its surface
        self.presentations.forget(window);
        for s in self.surface_layers.forget(window) {
            self.gpu.forget(&mut self.conn, s);
            let _ = self.conn.destroy(s);
        }
        // the transport destroys a window's popups with it
        let _ = self.conn.destroy(surface);
        if let Some(roles) = self.roles.as_mut() {
            roles.destroyed(surface);
        }
        let mut gone = vec![window];
        while let Some(child) = self
            .windows
            .iter()
            .find(|w| w.parent.is_some_and(|p| gone.contains(&p)) && !gone.contains(&w.id))
        {
            gone.push(child.id);
        }
        self.windows.retain(|w| !gone.contains(&w.id));
    }

    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError> {
        let result = match request {
            WindowRequest::Backdrop(backdrop) => return self.set_backdrop(window, backdrop),
            WindowRequest::Materials(regions) => return self.set_materials(window, regions),
            WindowRequest::Menus(menus) => return self.set_menus(window, menus),
            WindowRequest::SetInputRegion(region) => {
                self.window_mut(window)?.input_region = Some(region);
                return Ok(());
            }
            WindowRequest::SetOpaqueRegion(region) => {
                self.window_mut(window)?.opaque_region = Some(region);
                return Ok(());
            }
            WindowRequest::Present(changes) => return self.present_changes(window, &changes),
            WindowRequest::Layer { key, changes } => {
                return self.change_surface_layer(window, key, changes)
            }
            WindowRequest::SetTitle(title) => {
                let t = self.toplevel(window)?;
                self.conn.set_title(t, &title)
            }
            WindowRequest::SetMinSize(size) => {
                self.window_mut(window)?.min_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::SetMaxSize(size) => {
                self.window_mut(window)?.max_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::SetSize(size) => {
                let w = self.window_mut(window)?;
                if w.free_size {
                    w.size = convert::whole(size);
                }
                return Ok(());
            }
            WindowRequest::Move => {
                let t = self.toplevel(window)?;
                self.conn.start_move(t)
            }
            WindowRequest::Resize(edge) => {
                let t = self.toplevel(window)?;
                self.conn.start_resize(t, convert::resize_edge(edge))
            }
            WindowRequest::ShowWindowMenu(at) => {
                let t = self.toplevel(window)?;
                self.conn.show_window_menu(t, at)
            }
            WindowRequest::Maximize(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_maximized(t, on)
            }
            WindowRequest::Fullscreen(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_fullscreen(t, on)
            }
            WindowRequest::Minimize => {
                let t = self.toplevel(window)?;
                self.conn.minimize(t)
            }
            WindowRequest::Activate(None) => {
                let t = self.toplevel(window)?;
                self.conn.activate(t)
            }
            WindowRequest::Activate(Some(token)) => {
                let t = self.toplevel(window)?;
                self.conn.activate_with_token(t, &token)
            }
            WindowRequest::Reposition(spec) => {
                let Kind::Popup(p) = self.window(window)?.kind else {
                    return Err(PlatformError::Unsupported);
                };
                self.window_mut(window)?.size = convert::whole(spec.size);
                self.conn
                    .reposition_popup(p, &convert::placement(&spec))
                    .map(drop)
            }
        };
        result.map_err(platform_error)
    }

    fn set_text_input(&mut self, window: WindowId, state: Option<&TextInputState>) {
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        let state = state.map(convert::text_input);
        // without the protocol the field types through key events alone
        let _ = self.conn.set_text_input(surface, state.as_ref());
    }

    fn set_cursor(&mut self, _window: WindowId, shape: Option<CursorShape>) {
        // before the pointer enters the compositor's cursor stays
        let _ = match shape {
            Some(shape) => self.conn.set_cursor(convert::cursor(shape)),
            None => self.conn.hide_cursor(),
        };
    }

    /// The drag answers the button press that started the implicit grab still held in `window`'s client (the first of
    /// the buttons down); one the transport refuses (no button held) fails here, and one the compositor ignores ends as [`DragProgress::Cancelled`]. A drag started while
    /// another runs replaces it, which ends as cancelled. An image the transport cannot show is left out.
    fn start_drag(
        &mut self,
        window: WindowId,
        request: DragRequest,
    ) -> Result<DragToken, PlatformError> {
        let surface = self.window(window)?.surface;
        // an image the transport cannot show (a side over its limit at this scale) leaves the drag without one
        let icon = request.image.as_ref().and_then(|image| {
            drag_icon(image)
                .inspect_err(|e| {
                    eprintln!("cupertino-ui-wayland: the drag goes without its image: {e:?}")
                })
                .ok()
        });
        let actions = drag_actions(&request.actions);
        let contents = request
            .data
            .into_iter()
            .map(|d| (d.mime, d.bytes))
            .collect();
        let source = self
            .conn
            .start_drag_with_icon(surface, contents, actions, icon.as_ref())
            .map_err(platform_error)?;
        self.next_drag += 1;
        let token = DragToken(self.next_drag);
        if let Some((_, replaced)) = self.drag.replace((source, token)) {
            self.queued.push(PlatformEvent::Drag {
                token: replaced,
                progress: DragProgress::Cancelled,
            });
        }
        Ok(token)
    }

    /// Accepts the drag in the answer's type, or refuses it, and names the actions and the preferred one; the
    /// compositor's choice arrives as [`DropEvent::Action`] (a compositor older than drag actions copies).
    fn answer_drop(&mut self, token: DropToken, answer: &DropAnswer) -> Result<(), PlatformError> {
        let offer = match &self.hovering {
            Some(drop) if drop.token == token => drop.offer.clone(),
            _ => return Err(PlatformError::Protocol("no such drag over a window".into())),
        };
        self.conn
            .accept_drag(&offer, answer.mime.as_deref())
            .map_err(platform_error)?;
        let preferred = drag_actions(answer.preferred.as_ref());
        self.conn
            .set_drag_actions(&offer, drag_actions(&answer.actions), preferred)
            .map_err(platform_error)?;
        self.conn.flush().map_err(platform_error)
    }

    /// The read runs in the transport's loop like a clipboard read, with its limits.
    fn read_drop(&mut self, token: DropToken, mime: &str) -> Result<ClipboardToken, PlatformError> {
        let offer = self
            .landed
            .iter()
            .find(|d| d.token == token)
            .map(|d| d.offer.clone())
            .ok_or_else(|| PlatformError::Protocol("no such drop".into()))?;
        let id = self
            .conn
            .receive_drop(&offer, mime)
            .map_err(platform_error)?;
        self.next_clipboard += 1;
        let read = ClipboardToken(self.next_clipboard);
        self.reads.push((id, read, mime.to_owned()));
        Ok(read)
    }

    /// The source is told the action the drop was taken with just before it finishes, whatever was chosen during the
    /// drag (an ask, or a move where the window copied), so a source never removes data the window only copied. One
    /// taken as an ask copies; one taken with an action the source does not allow is refused, and the source keeps its
    /// data.
    fn finish_drop(&mut self, token: DropToken, action: Option<DropAction>) {
        let Some(at) = self.landed.iter().position(|d| d.token == token) else {
            return;
        };
        let drop = self.landed.remove(at);
        let last = action.map(|action| match action {
            DropAction::Ask => DropAction::Copy,
            other => other,
        });
        let allowed =
            last.filter(|last| drop.offer.source_actions().contains(drag_actions([last])));
        // a drop whose surface or offer is gone is already over
        let _ = match allowed {
            Some(last) => {
                // a compositor tells the source only a change of action, so this costs nothing when it agrees
                let bits = drag_actions([&last]);
                let _ = self.conn.set_drag_actions(&drop.offer, bits, bits);
                self.conn.finish_drag(&drop.offer)
            }
            None => self.conn.refuse_drop(&drop.offer),
        };
    }

    fn set_clipboard(&mut self, kind: ClipboardKind, data: ClipboardData) {
        if kind == ClipboardKind::Clipboard {
            let _ = self.conn.set_selection(vec![(data.mime, data.bytes)]);
        }
    }

    /// The read runs in the transport's loop while dispatch goes on; its data arrives with the dispatch it ends in (the
    /// transport gives up on a source silent for a second, or after ten seconds). One that can't start answers `None`
    /// with the next dispatch.
    fn read_clipboard(&mut self, kind: ClipboardKind, mime: &str) -> ClipboardToken {
        self.next_clipboard += 1;
        let token = ClipboardToken(self.next_clipboard);
        let started = match (&self.selection, kind) {
            (Some(offer), ClipboardKind::Clipboard) => {
                let offer = offer.clone();
                self.conn.receive_async(&offer, mime).ok()
            }
            _ => None,
        };
        match started {
            Some(id) => self.reads.push((id, token, mime.to_owned())),
            None => self
                .queued
                .push(PlatformEvent::Clipboard { token, data: None }),
        }
        token
    }

    fn request_activation_token(
        &mut self,
        window: WindowId,
        app_id: Option<&str>,
    ) -> ActivationRequest {
        self.next_activation += 1;
        let request = ActivationRequest(self.next_activation);
        let surface = self.window(window).map(|w| w.surface).ok();
        let asked =
            surface.and_then(|surface| self.conn.request_activation_token(surface, app_id).ok());
        match asked {
            Some(wire) => self.tokens.push((wire, request)),
            None => self.queued.push(PlatformEvent::ActivationToken {
                request,
                token: None,
            }),
        }
        request
    }

    /// With no frame callback outstanding and no present waiting for a configure or a free buffer nothing paces the
    /// window, so the frame is due at once.
    fn activated(&self, window: WindowId) -> Option<bool> {
        self.window(window).ok().and_then(|w| w.activated)
    }

    fn request_frame(&mut self, window: WindowId) {
        let now = self.now();
        let Ok(w) = self.window_mut(window) else {
            return;
        };
        if w.frame_pending || w.away || w.stalled.is_some() {
            w.frame_wanted = true;
        } else {
            let id = w.id;
            self.queued.push(PlatformEvent::Window {
                window: id,
                event: WindowEvent::Frame(now),
            });
        }
    }

    /// A frame with a surface on the GPU is drawn on the render thread into free buffers; one all in shared memory is
    /// drawn on the CPU pool into the buffers it leases. Its surface layers and raised layer are drawn with it, each into
    /// a buffer of its own surface ([`hand_over`]). The thread that drew the frame commits the layers and then the
    /// window, acknowledging the configure that was the latest when the frame was submitted. No frame is declined.
    fn submit(&mut self, window: WindowId, frame: &Submission) -> Result<Submitted, PlatformError> {
        let (surface, logical) = self.before_present(window)?;
        // what applies with the window's commit: the surface layers' values and the raised layer placed
        if let Some(layers) = &frame.surface_layers {
            let wanted: Vec<_> = layers.iter().map(|l| (l.key, l.rect, l.look)).collect();
            self.sync_surface_layers(window, &wanted)?;
        }
        let raised = match &frame.raised {
            RaisedChange::Kept | RaisedChange::Hidden => None,
            RaisedChange::Drawn { height, layer } => {
                let layer_surface = self.layer_surface(window)?;
                let width = self.window(window)?.size.0;
                self.conn
                    .place_subsurface(layer_surface, (0, 0), (width, *height))
                    .map_err(platform_error)?;
                Some((layer_surface, (width, *height), layer.clone()))
            }
        };
        // the surfaces the frame draws, in the order they commit: the surface layers, the raised layer, the window
        let mut drawn: Vec<(Of, SurfaceId, (u32, u32), LayerList)> = Vec::new();
        for l in frame.surface_layers.iter().flatten() {
            let Some(list) = &l.layer else { continue };
            let shown = self.surface_layers.shown_mut(window);
            if let Some(state) = shown.iter().find(|s| s.key == l.key) {
                drawn.push((Of::Layer(l.key), state.surface, state.size(), list.clone()));
            }
        }
        if let Some((layer_surface, size, list)) = raised {
            drawn.push((Of::Raised, layer_surface, size, list));
        }
        drawn.push((
            Of::Window,
            surface,
            logical,
            LayerList {
                renderer: Arc::clone(&frame.renderer),
                list: Arc::clone(&frame.list),
                width: frame.width,
                height: frame.height,
                damage: frame.damage.clone(),
            },
        ));
        // the frame waits until every surface it draws has a buffer, so no layer commits a frame its window cannot
        // follow with its own
        let busy: Vec<SurfaceId> = drawn
            .iter()
            .map(|d| d.1)
            .filter(|&s| !self.surface_can_present(s))
            .collect();
        if !busy.is_empty() {
            let w = self.window_mut(window)?;
            w.stalled = Some(Stall::Buffer);
            w.busy_layers = busy.into_iter().filter(|&s| s != surface).collect();
            return Ok(Submitted::NotReady);
        }
        // the window's buffer first: a configure of another size stops the frame before any layer's is taken
        let mut targets = Vec::with_capacity(drawn.len());
        for (of, s, preferred, list) in drawn.iter().rev() {
            let pixels = (list.width, list.height);
            let resized = self.shown_size(window, *of) != Some(pixels);
            let target = match self.prepare_target(*s, *preferred, pixels, resized) {
                Ok(Ok(target)) => Ok(target),
                Ok(Err(stall)) => {
                    self.window_mut(window)?.stalled = stall;
                    Err(Submitted::NotReady)
                }
                Err(e) => {
                    for (s, target) in targets {
                        self.release_target(s, target);
                    }
                    return Err(e);
                }
            };
            match target {
                Ok(target) => targets.push((*s, target)),
                Err(submitted) => {
                    for (s, target) in targets {
                        self.release_target(s, target);
                    }
                    return Ok(submitted);
                }
            }
        }
        let mut parts = Vec::with_capacity(drawn.len());
        for ((of, s, _, list), (_, target)) in drawn.into_iter().zip(targets.into_iter().rev()) {
            let pixels = (list.width, list.height);
            let resized = self.shown_size(window, of) != Some(pixels);
            parts.push(Part {
                of,
                surface: s,
                list,
                resized,
                target,
            });
        }
        // the raised layer hidden as the frame is handed over: after every commit of the window's frame before, which
        // were sent before it could draw this one, and before this frame's, whose window commit applies it
        if matches!(frame.raised, RaisedChange::Hidden) {
            if let Err(e) = self.hide_layer(window) {
                for part in parts {
                    self.release_target(part.surface, part.target);
                }
                return Err(e);
            }
        }
        self.role_commit(window, true);
        let on_render_thread = parts.iter().any(|p| matches!(p.target, Target::Gpu(_)));
        let (tx, waker) = (self.finished.0.clone(), self.conn.waker());
        // what the frame records, a panic too, is the submitting instance's
        let scope = diag::Scope::current();
        let submission = frame.clone();
        let job = move || {
            let _scope = scope.as_ref().map(diag::Scope::enter);
            let drawing = hand_over::draw(parts, &submission);
            let _ = tx.send(FromJob::Finished(Finished {
                window,
                backs: drawing.backs,
                panic: drawing.panic,
            }));
            // a commit that cannot be flushed fails the window system's next read of the connection
            let _ = drawing.commits.send();
            let _ = tx.send(FromJob::Sent(window));
            waker.wake();
        };
        if on_render_thread {
            render_thread::hand_over(job);
        } else {
            // the window's frames on one thread, where what its renderer uses stays warm; a frame in shared memory
            // waits for nothing the render thread does, such as the programs of a device being made
            cupertino_ui_core::pool::cpu_for(u64::from(window.0), job);
        }
        self.submitted(window)
    }

    /// No frame is presented from the UI thread: [`WindowSystem::submit`] takes every one.
    fn present(
        &mut self,
        _window: WindowId,
        _frame: &CpuFrame,
        _configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Hides the layer's subsurface with the window's next commit, a solid colour's; a layer to show goes with a
    /// submitted frame ([`Submission::raised`]).
    fn present_layer(
        &mut self,
        window: WindowId,
        layer: Option<RaisedLayer<'_>>,
    ) -> Result<PresentOutcome, PlatformError> {
        if layer.is_some() {
            return Err(PlatformError::Unsupported);
        }
        self.hide_layer(window)?;
        Ok(PresentOutcome::Presented)
    }

    /// Takes every layer away with the window's next commit, a solid colour's; layers to show go with a submitted
    /// frame ([`Submission::surface_layers`]).
    fn present_surface_layers(
        &mut self,
        window: WindowId,
        layers: &[cupertino_ui_core::platform::SurfaceLayer<'_>],
    ) -> Result<PresentOutcome, PlatformError> {
        if !layers.is_empty() {
            return Err(PlatformError::Unsupported);
        }
        self.sync_surface_layers(window, &[])?;
        Ok(PresentOutcome::Presented)
    }

    /// One pixel of `color` the compositor stretches over the window ([`Connection::present_solid`]), with the input
    /// region waiting to be sent; refused where the compositor has no one-pixel buffers. The next frame presented is
    /// whole.
    fn present_solid(
        &mut self,
        window: WindowId,
        color: Color,
        _configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        if !self.conn.capabilities().contains(Capability::SolidColor) {
            return Err(PlatformError::Unsupported);
        }
        if self.roles.is_some() {
            let w = self.window(window)?;
            match self.conn.frame_size(w.surface, w.size) {
                Ok(_) => {}
                Err(Error::NotConfigured) => {
                    self.window_mut(window)?.stalled = Some(Stall::Configure);
                    return Ok(PresentOutcome::NotReady);
                }
                Err(e) => return Err(platform_error(e)),
            }
        }
        let w = self.window_mut(window)?;
        let (surface, size) = (w.surface, w.size);
        if let Some(b) = w.backdrop.as_mut().filter(|b| b.shaped != Some(size)) {
            let bounds = Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
            if b.material.set_shape(&bounds, b.radius).is_ok() {
                b.shaped = Some(size);
            }
        }
        if let Some(region) = w.input_region.take() {
            self.conn
                .set_input_region(surface, region.as_deref())
                .map_err(platform_error)?;
        }
        self.role_commit(window, true);
        match self.conn.present_solid(surface, size, color.to_array()) {
            Ok(()) => {
                self.await_frame_callback(window)?;
                self.window_mut(window)?.shown = None;
                Ok(PresentOutcome::Presented)
            }
            Err(Error::NotConfigured) => {
                self.window_mut(window)?.stalled = Some(Stall::Configure);
                Ok(PresentOutcome::NotReady)
            }
            Err(Error::Unsupported(_)) => Err(PlatformError::Unsupported),
            Err(e) => Err(platform_error(e)),
        }
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        if let Some(roles) = self.roles.as_mut() {
            roles.prepare(&mut self.conn);
        }
        let ready = !self.queued.is_empty() || self.woken.load(Ordering::Acquire);
        events.append(&mut self.queued);
        // a release point signals through an eventfd the connection does not poll: look again soon, while the
        // compositor can draw the window and so release a buffer by itself
        let gpu_stalled = self
            .windows
            .iter()
            .filter(|w| w.stalled == Some(Stall::Buffer))
            .filter(|w| {
                self.gpu.draws(w.surface) || w.busy_layers.iter().any(|&s| self.gpu.draws(s))
            })
            .any(|w| {
                let on_output = self
                    .conn
                    .surface_outputs(w.surface)
                    .is_ok_and(|o| !o.is_empty());
                polls_release(!self.conn.outputs().is_empty(), on_output)
            });
        let warm_up = self
            .gpu
            .deadline()
            .map(|d| d.saturating_duration_since(Instant::now()));
        let timeout = wait_timeout(timeout, gpu_stalled, warm_up);
        let wire = self
            .conn
            .wait(if ready { Some(Duration::ZERO) } else { timeout })
            .map_err(platform_error)?;
        // the frames drawn away from the UI thread, before what the compositor said of them
        while let Ok(back) = self.finished.1.try_recv() {
            match back {
                FromJob::Finished(finished) => self.finish(finished)?,
                FromJob::Sent(window) => self.sent(window, events)?,
            }
        }
        for event in wire {
            self.translate(event, events);
        }
        // the windows whose layers' last animation ended in what was read
        events.extend(
            self.surface_layers
                .take_settled()
                .into_iter()
                .map(|window| PlatformEvent::Window {
                    window,
                    event: WindowEvent::LayersSettled,
                }),
        );
        for w in &self.windows {
            if let Some(ShownMenus {
                published: Published::Shown(menus),
                ..
            }) = &w.menus
            {
                events.extend(menus.activations().map(|a| PlatformEvent::Window {
                    window: w.id,
                    event: WindowEvent::MenuActivated(a.action),
                }));
            }
        }
        self.gpu.poll(&mut self.conn, None);
        let changes = self.gpu.take_events();
        self.renderer_changed(&changes);
        events.extend(changes);
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            if w.stalled != Some(Stall::Buffer) {
                continue;
            }
            // the window's surface, its raised layer's and its surface layers' that had none, which present with it
            let (surface, layer) = (w.surface, w.layer.as_ref().map(|l| l.surface));
            let busy = w.busy_layers.clone();
            let free = self.surface_can_present(surface)
                && layer.is_none_or(|l| self.surface_can_present(l))
                && busy.into_iter().all(|l| self.surface_can_present(l));
            if free && self.resume(i) {
                events.push(PlatformEvent::Window {
                    window: self.windows[i].id,
                    event: WindowEvent::Frame(self.now()),
                });
            }
        }
        if self.woken.swap(false, Ordering::AcqRel) {
            events.push(PlatformEvent::Wake);
        }
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::new(WakeHandle {
            waker: self.conn.waker(),
            woken: Arc::clone(&self.woken),
        }))
    }
}

/// Bring the published items from `was` disabled to those `now` disables, leaving out what its bar does not show.
pub(super) fn set_enabled(menus: &WindowMenus, was: &BTreeSet<ActionName>, now: &MenuDescription) {
    let shown = actions(&now.bar);
    for action in was.union(&now.disabled).filter(|a| shown.contains(a)) {
        menus.set_enabled(action, !now.disabled.contains(action));
    }
}

/// The actions of the items `bar` publishes.
fn actions(bar: &MenuBar) -> BTreeSet<ActionName> {
    fn walk(menu: &Menu, out: &mut BTreeSet<ActionName>) {
        for entry in menu.sections.iter().flatten() {
            match entry {
                Entry::Item(item) => {
                    if let Command::Action(action) = &item.command {
                        out.insert(action.clone());
                    }
                }
                Entry::Submenu(menu) => walk(menu, out),
            }
        }
    }
    let mut out = BTreeSet::new();
    for menu in std::iter::once(&bar.app).chain(&bar.menus) {
        walk(menu, &mut out);
    }
    out
}
