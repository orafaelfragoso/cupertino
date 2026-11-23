use super::*;

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// Lays out, rebuilds semantics and focus, and paints and presents every window that is due. A window that fails
    /// is reported and tries again whole on its next change or configure, whatever failed it: the renderer gives the same
    /// error for a target and for content too large to draw. The same error again is not reported again, and the
    /// window's animations ask for no frames until a frame presents. Only a lost connection ends the application.
    pub(super) fn frame(&mut self) -> Result<(), UiError> {
        let relayout = self.tree.take_layout_dirty();
        let paint_dirty = self.tree.take_paint_dirty();
        let now = self.options.clock.now();
        for i in 0..self.windows.len() {
            match self.frame_window(i, relayout, &paint_dirty, now) {
                Ok(()) => {}
                Err(error @ UiError::Platform(PlatformError::Disconnected(_))) => {
                    return Err(error)
                }
                Err(error) => {
                    let w = &mut self.windows[i];
                    w.invalidate();
                    w.dirty = false;
                    w.invalidate_damage();
                    if w.failed.as_ref() != Some(&error) {
                        if let Some(report) = &mut self.options.on_window_error {
                            report(w.id, &error);
                        }
                    }
                    w.failed = Some(error);
                }
            }
        }
        self.follow_anchors();
        self.follow_grow_origins();
        if !self.awaiting_anchor.is_empty() {
            self.awaiting_anchor.retain(|(_, asked)| !asked);
            for (_, asked) in &mut self.awaiting_anchor {
                *asked = true;
                self.view_dirty = true;
            }
        }
        if !self.awaiting_layout.is_empty() {
            let before = self.awaiting_layout.len();
            let windows = &self.windows;
            self.awaiting_layout.retain(|key| {
                windows
                    .iter()
                    .any(|w| &w.key == key && w.geometry.is_empty())
            });
            if self.awaiting_layout.len() < before {
                self.view_dirty = true;
            }
        }
        tracing::trace_span!("ui.accessibility").in_scope(|| self.sync_accessibility());
        tracing::trace_span!("ui.semantics").in_scope(|| self.sync_semantics());
        self.send_presented();
        Ok(())
    }

    /// Sends the messages waiting for windows that have now presented a frame.
    pub(super) fn send_presented(&mut self) {
        let windows = &self.windows;
        let (ready, waiting) = std::mem::take(&mut self.on_presented)
            .into_iter()
            .partition(|(key, _)| windows.iter().any(|w| &w.key == key && w.presented));
        self.on_presented = waiting;
        let ready: Vec<(WindowKey, A::Message)> = ready;
        self.messages.extend(ready.into_iter().map(|(_, m)| m));
    }

    /// Moves each open popup whose anchor moved or whose content changed size, once the parents are laid out.
    pub(super) fn follow_anchors(&mut self) {
        for i in 0..self.windows.len() {
            let Some(state) = self.windows[i]
                .popup
                .as_ref()
                .filter(|_| self.windows[i].fade.is_none())
            else {
                continue;
            };
            let Some(placed) = self
                .windows
                .iter()
                .find(|w| w.key == state.parent)
                .and_then(|parent| {
                    place(
                        &self.tree,
                        parent,
                        &state.anchor,
                        state.placed.grab,
                        state.size,
                    )
                })
                .filter(|placed| *placed != state.placed)
            else {
                continue;
            };
            let parent = state.parent.clone();
            let w = &mut self.windows[i];
            let _ = self
                .system
                .request(w.id, WindowRequest::Reposition(placed.clone()));
            if let Some(state) = &mut w.popup {
                state.placed = placed;
            }
            let key = w.key.clone();
            self.forget_placement(&parent, &key);
        }
    }

    pub(super) fn frame_window(
        &mut self,
        i: usize,
        relayout: bool,
        paint_dirty: &BTreeSet<NodeId>,
        now: Timestamp,
    ) -> Result<(), UiError> {
        let w = &mut self.windows[i];
        let Some(root) = w.root else {
            return Ok(());
        };
        if relayout {
            w.invalidate();
        } else if paint_dirty.iter().any(|n| w.geometry.contains_key(n)) {
            w.repaint();
        }
        // every control may show the window's active state: one painted in the other state paints whole
        let active = w.active || w.keyboard_in_popup;
        if w.painted_active.is_some_and(|p| p != active) {
            w.invalidate();
            w.invalidate_damage();
        }
        if !w.dirty {
            return Ok(());
        }
        let _window =
            tracing::trace_span!("ui.window", window = tracing::field::debug(w.id)).entered();
        let mut focus_effects = Effects::default();
        let mut hold = false;
        let laid_out = !w.prepared;
        let laying_out = mark(&self.frame_timing);
        let layout_span = tracing::trace_span!("ui.layout", whole = !w.prepared).entered();
        if !w.prepared {
            let result = match w.laid_out.take() {
                Some(result) => result,
                None => {
                    let available = Constraints::tight(w.size).map_err(UiError::Layout)?;
                    // a node measures in its environment's type and pixels, none of which its generation covers
                    if !w
                        .measured_in
                        .as_ref()
                        .is_some_and(|env| same_environment(env, &w.env))
                    {
                        w.layout_cache.clear();
                        w.placements.clear();
                        w.semantics.clear();
                        w.measured_in = Some(w.env.clone());
                    }
                    let view = TreeView::new(&self.tree, &w.env);
                    let result = layout_cached(
                        &view,
                        root,
                        available,
                        &LayoutOptions {
                            direction: w.env.direction,
                            node_limit: self.options.node_limit,
                        },
                        &mut w.layout_cache,
                    )
                    .map_err(UiError::Layout)?;
                    w.idle.extend(view.idle.into_inner());
                    w.layout = result.stats;
                    result
                }
            };
            // scroll offsets and hit targets are the nodes' own state, so the placements are made again
            w.geometry = placements_kept(&self.tree, root, &result, &w.env, &[], &mut w.placements);
            w.layouts += 1;
            w.raised = raised_nodes(&self.tree, root, &w.geometry);
            w.obscuring = w
                .raised
                .iter()
                .filter(|(id, _)| {
                    self.tree
                        .get(*id)
                        .is_some_and(|n| n.widget.obscures(&*n.state))
                })
                .copied()
                .collect();
            // raised nodes that draw in place leave what scrolls under them out: it would cover the glass under them
            if !w.obscuring.is_empty() && !self.system.capabilities().raised_layer {
                w.geometry = placements_kept(
                    &self.tree,
                    root,
                    &result,
                    &w.env,
                    &w.obscuring,
                    &mut w.placements,
                );
            }
            // In a layer every node's paint lies over every material of the layer, so a raised node a floating one
            // lies across would show through the floating one's glass: it draws with the content instead, under that
            // glass, and still covers what scrolls under it.
            if self.system.capabilities().raised_layer && w.obscuring.len() < w.raised.len() {
                let floating: Vec<Rect> = w
                    .raised
                    .iter()
                    .filter(|r| !w.obscuring.contains(r))
                    .map(|(_, rect)| *rect)
                    .collect();
                let obscuring = &w.obscuring;
                w.raised.retain(|r| {
                    !obscuring.contains(r) || !floating.iter().any(|f| f.intersects(&r.1))
                });
            }
            w.laid_out = Some(result);

            let semantics = build_kept(
                &self.tree,
                root,
                &w.geometry,
                w.focus.node(),
                w.env.direction,
                w.env.scale,
                &w.obscuring,
                &mut w.semantics,
            );
            w.focusable = semantics.focusable;
            w.roving_groups = semantics.roving_groups;
            w.roving_items = semantics.roving_items;
            let requested = match w
                .focus_request
                .take()
                .and_then(|r| resolve(&self.tree, &w.roving_groups, &w.roving_items, r))
            {
                Some(item) if w.focusable.contains(&item) => {
                    w.focus.set(&self.tree, Some(item), true)
                }
                _ => None,
            };
            let change = w.focus.sync(
                &self.tree,
                &semantics.scopes,
                &w.focusable,
                &semantics.hidden,
            );
            let refocused = requested.is_some() || change.is_some();
            focus_effects =
                Self::focus_events(&mut self.tree, w, requested, &mut self.messages, now);
            focus_effects.absorb(Self::focus_events(
                &mut self.tree,
                w,
                change,
                &mut self.messages,
                now,
            ));
            let (nodes, accessibility_focus) = if refocused {
                let pass = build_kept(
                    &self.tree,
                    root,
                    &w.geometry,
                    w.focus.node(),
                    w.env.direction,
                    w.env.scale,
                    &w.obscuring,
                    &mut w.semantics,
                );
                (pass.nodes, pass.focus)
            } else {
                (semantics.nodes, semantics.focus)
            };
            if let Some(adapter) = &mut self.options.accessibility {
                if adapter.is_active() && w.accessible {
                    let (changed, removed) = diff(&w.accessibility, &nodes);
                    if !changed.is_empty() || !removed.is_empty() {
                        adapter.update(&AccessibilityUpdate {
                            window: w.id,
                            root,
                            focus: accessibility_focus,
                            nodes: changed,
                            removed,
                            full: false,
                        });
                    }
                }
            }
            w.accessibility = nodes;
            w.accessibility_focus = accessibility_focus;
            w.prepared = true;

            let mut scroll = geometry_events(
                &self.tree,
                root,
                &w.geometry,
                w.env.direction,
                w.env.scale,
                &mut w.scroll_heard,
            );
            if let Some((target, part)) = w.reveal.take() {
                scroll.extend(reveal_events(
                    &self.tree,
                    &w.geometry,
                    &w.obscuring,
                    w.env.direction,
                    w.env.scale,
                    target,
                    part,
                ));
            }
            let mut scroll_effects = Effects::default();
            let mut route = route(&mut self.tree, w, now, &mut scroll_effects);
            for (node, event) in &scroll {
                route.notify(*node, event);
            }
            // the model takes the new geometry before this frame paints, so a list never shows the rows of an old size
            hold = !scroll_effects.messages.is_empty() && w.held_paints < HELD_PAINTS;
            focus_effects.absorb(scroll_effects);
        }
        drop(layout_span);
        lap(&mut self.frame_timing, Phase::Layout, laying_out);
        self.apply(i, focus_effects);
        // what lies under a drag that stood still may have moved
        if laid_out {
            self.drop_over(i);
        }
        if self.pins_seen {
            self.refresh_pinned();
        }
        let w = &mut self.windows[i];
        if hold {
            w.held_paints += 1;
            return Ok(());
        }

        // a window whose last frame is still being drawn or committed away from the UI thread draws no other
        if !w.may_draw() {
            return Ok(());
        }
        if let (WindowBackground::Solid(color), false) = (w.background, w.solid_refused) {
            w.dirty = false;
            w.held_paints = 0;
            // the compositor still shows this colour at this size, and no configure waits for an answer
            if w.ack.is_none() && w.solid_shown == Some((w.size, color)) {
                return Ok(());
            }
            // the content paints nothing, so the glass it laid under itself goes with the colour's commit; drawn
            // again, the window asks for it anew
            if !w.materials.is_empty() {
                let _ = self
                    .system
                    .request(w.id, WindowRequest::Materials(Vec::new()));
                w.materials.clear();
                w.painter.tones.regions.clear();
            }
            // and the raised layer goes with it, its subsurface hidden with the colour's commit
            if w.layer.shown.is_some() {
                self.system
                    .present_layer(w.id, None)
                    .map_err(UiError::Platform)?;
                w.layer.shown = None;
                w.layer.damage.invalidate();
            }
            w.surface_layers.hide(&mut self.system, w.id)?;
            let committing = mark(&self.frame_timing);
            let solid = tracing::trace_span!("ui.present", solid = true)
                .in_scope(|| self.system.present_solid(w.id, color, w.ack));
            lap(&mut self.frame_timing, Phase::Commit, committing);
            match solid {
                Ok(PresentOutcome::Presented) => {
                    if let Some(t) = &mut self.frame_timing {
                        t.presented();
                    }
                    w.failed = None;
                    w.solid_shown = Some((w.size, color));
                    w.ack = None;
                    w.frame_ready = false;
                    w.committed = true;
                    self.system.request_frame(w.id);
                    return Ok(());
                }
                Ok(PresentOutcome::NotReady) => {
                    w.failed = None;
                    w.frame_ready = false;
                    w.dirty = true;
                    self.system.request_frame(w.id);
                    return Ok(());
                }
                // drawn instead, filled with the colour, from now on
                Err(PlatformError::Unsupported) => {
                    w.solid_refused = true;
                    w.invalidate_damage();
                }
                Err(e) => return Err(UiError::Platform(e)),
            }
        }
        self.begin_grow(i, now);
        let w = &mut self.windows[i];
        let capabilities = self.system.capabilities();
        let effects = capabilities.app_effects;
        let layered = capabilities.raised_layer;
        let surface_layered = capabilities.surface_layers;
        let glass = crate::paint::GlassSupport::of(&capabilities);
        let paint = |w: &mut WindowState<A::Message>,
                     tree: &Tree<A::Message>,
                     pinned: &[Option<Arc<ThemeTokens>>; 2]| {
            if w.activated_at.is_some() {
                // the colours are crossing: each node's last paint is of other colours, and the crossing is not part
                // of what a node's cached paint is keyed on
                w.painter.forget_nodes();
            }
            let activation = w.activation_progress(now);
            let mut painting = w.painter.paint(
                tree,
                root,
                &w.geometry,
                &PaintInput {
                    env: &w.env,
                    focus: w.focus.node(),
                    focus_visible: w.focus.visible(),
                    window_active: w.active || w.keyboard_in_popup,
                    background: w.background,
                    backdrop: w.backdrop.is_some(),
                    materials: effects && !w.materials_refused,
                    now,
                    raised: &w.raised,
                    obscuring: &w.obscuring,
                    layer: layered,
                    surface_layers: surface_layered,
                    glass,
                    pinned,
                    activation,
                },
            );
            if let Some(fade) = w.fade {
                fade::dim(&mut painting.list, fade.opacity);
                if let Some(into) = fade.into {
                    morph::apply(
                        &mut painting.list,
                        &mut painting.materials,
                        into,
                        fade.scale,
                    );
                }
                // the glass the content declares fades with it, a popover's and a menu's that draws its own
                morph::fade_materials(&mut painting.materials, fade.opacity);
            } else if let Some(scale) = w.grow.and_then(|g| g.scale(now)) {
                let origin = w.grow.map_or(Point::ZERO, |g| g.origin);
                morph::apply(&mut painting.list, &mut painting.materials, origin, scale);
            }
            painting
        };
        let painting_since = mark(&self.frame_timing);
        let paint_span = tracing::trace_span!("ui.paint").entered();
        let mut painting = paint(w, &self.tree, &self.pinned);
        w.painted_active = Some(w.active || w.keyboard_in_popup);
        if painting.pins && !self.pins_seen {
            // the first subtree pinned to an appearance: make the themes it draws with and paint it in them at once
            self.pins_seen = true;
            self.pinned_from = Some(Arc::clone(&self.options.theme));
            self.pinned = pinned_themes(&mut self.theme_for, &self.options.theme);
            if self.pinned.iter().any(Option::is_some) {
                painting = paint(w, &self.tree, &self.pinned);
            }
        }
        // the shapes take effect with the frame's commit, so the frame commits even when it repaints no pixel: the
        // compositor draws the glass, and a window's own pixels under a region that moved, grew or faded are the same,
        // so they are not drawn again (a glass panel that opens or fades repaints only what its content changed)
        let mut materials_changed = false;
        if painting.materials != w.materials {
            let request = WindowRequest::Materials(painting.materials.clone());
            if self.system.request(w.id, request).is_ok() {
                w.materials = std::mem::take(&mut painting.materials);
                materials_changed = true;
            } else {
                // its nodes paint their own fill from now on
                w.materials_refused = true;
                w.materials.clear();
                painting = paint(w, &self.tree, &self.pinned);
                w.invalidate_damage();
            }
            // regions past the new count lose their backdrops; one made again later starts unmeasured
            let kept = w.materials.len();
            w.painter.tones.regions.truncate(kept);
        }
        // the marked subtrees leave the window's paint for their surface layers, marked once the nodes painted so an
        // overlay's layer lies where its paint put it this frame
        let marks = if surface_layered {
            surface_layers::marked(&self.tree, root, &w.geometry, w.size)
        } else {
            Vec::new()
        };
        let surface = w
            .surface_layers
            .prepare(surface_layers::split(&mut painting, &marks));
        // the layer's height in whole points, from the window's top edge, as far down as its nodes draw
        let layer_height = painting
            .layer
            .as_ref()
            .map(|l| (l.bottom.min(w.size.height).ceil().max(1.0)) as u32);
        if layer_height != w.layer.shown {
            w.layer.damage.invalidate();
        }
        let damage = w.damage.damage(&painting);
        let layer_damage = painting.layer.as_ref().map(|l| {
            w.layer
                .damage
                .damage_of(&l.list, &l.own, &l.blank, &Default::default())
        });
        w.paint_animate.extend(painting.animating.iter().copied());
        w.idle.extend(painting.idle.iter().copied());
        w.animate.extend(painting.animating);
        w.display_list = Arc::new(painting.list);
        w.resources = Arc::new(painting.resources);
        w.dirty = false;
        w.held_paints = 0;
        let layer_changed = layer_height != w.layer.shown
            || layer_damage
                .as_ref()
                .is_some_and(|d| !matches!(d, DamageRegion::Rects(r) if r.is_empty()))
            || (painting.layer.is_some() && w.layer.damage.repainted());
        // a toplevel says which of its parts its pixels cover whole, with the next frame: the compositor leaves out what lies
        // under them and tells a window they cover all of that it is not seen
        if w.popup.is_none() {
            let opaque = crate::opaque::region(&w.display_list, w.size, &w.materials);
            if opaque != w.opaque
                && self
                    .system
                    .request(w.id, WindowRequest::SetOpaqueRegion(opaque.clone()))
                    .is_ok()
            {
                w.opaque = opaque;
            }
        }
        drop(paint_span);
        lap(&mut self.frame_timing, Phase::Paint, painting_since);
        // a frame that repainted nothing sends nothing, unless a configure waits for its acknowledgement or a frame
        // the window system was not ready for waits to be shown; one that repainted no pixel commits with no damage
        if matches!(&damage, DamageRegion::Rects(r) if r.is_empty())
            && !w.damage.repainted()
            && !layer_changed
            && !surface.changed
            && !materials_changed
            && w.ack.is_none()
            && !w.owed
        {
            return Ok(());
        }
        let scale = w.env.scale;
        let width = physical(w.size.width, scale);
        let height = physical(w.size.height, scale);
        // the surface layers and the raised layer go with the frame, drawn and shown before it
        let surface_layers = surface.changed.then(|| {
            w.surface_layers
                .submitted(surface, scale, &mut self.options.renderers)
        });
        let raised = match (layer_changed, painting.layer, layer_height, layer_damage) {
            (false, ..) => RaisedChange::Kept,
            (true, Some(layer), Some(layer_height), Some(damage)) => {
                w.layer.list = Arc::new(layer.list);
                let renderers = &mut self.options.renderers;
                let renderer = w
                    .layer
                    .renderer
                    .get_or_insert_with(|| Arc::new(Mutex::new(renderers())));
                RaisedChange::Drawn {
                    height: layer_height,
                    layer: LayerList {
                        renderer: Arc::clone(renderer),
                        list: Arc::clone(&w.layer.list),
                        width,
                        height: physical(f64::from(layer_height), scale),
                        damage,
                    },
                }
            }
            (true, ..) => RaisedChange::Hidden,
        };
        // the frame drawn and committed away from the UI thread where the window system takes it that way
        let submitting = mark(&self.frame_timing);
        let submission = Submission {
            renderer: Arc::clone(&w.renderer),
            list: Arc::clone(&w.display_list),
            resources: Arc::clone(&w.resources),
            scale,
            width,
            height,
            damage: damage.clone(),
            raised,
            surface_layers,
        };
        let submitted = tracing::trace_span!("ui.submit")
            .in_scope(|| self.system.submit(w.id, &submission))
            .map_err(UiError::Platform)?;
        match submitted {
            Submitted::Taken => {
                lap(&mut self.frame_timing, Phase::Commit, submitting);
                w.in_flight = Some(InFlight {
                    ack: w.ack,
                    raised: layer_changed.then_some(layer_height),
                    surface_layers: submission
                        .surface_layers
                        .as_deref()
                        .map(surface_layers::shows),
                });
                w.frame_ready = false;
                return Ok(());
            }
            // nothing was drawn, and the trackers have already taken the frame's damage
            Submitted::NotReady => {
                w.damage.invalidate();
                w.layer.damage.invalidate();
                w.surface_layers.not_shown();
                w.frame_ready = false;
                w.dirty = true;
                w.owed = true;
                self.system.request_frame(w.id);
                return Ok(());
            }
            Submitted::Declined => {}
        }
        // declined: the surface layers drawn here first, then the raised layer; both show with the content's commit
        if let Some(layers) = &submission.surface_layers {
            let rastering = mark(&self.frame_timing);
            let _surface_layers = tracing::trace_span!("ui.surface_layers").entered();
            let shown =
                w.surface_layers
                    .present(layers, &mut self.system, w.id, &w.resources, scale)?;
            lap(&mut self.frame_timing, Phase::Raster, rastering);
            if shown == PresentOutcome::NotReady {
                w.frame_ready = false;
                w.dirty = true;
                w.invalidate_damage();
                self.system.request_frame(w.id);
                return Ok(());
            }
        }
        let shown = match &submission.raised {
            RaisedChange::Kept => None,
            RaisedChange::Hidden => Some(
                self.system
                    .present_layer(w.id, None)
                    .map_err(UiError::Platform)?,
            ),
            RaisedChange::Drawn {
                height: layer_height,
                layer,
            } => {
                let rastering = mark(&self.frame_timing);
                let raster_span = tracing::trace_span!("ui.raster", layer = "raised").entered();
                let mut renderer = held(&layer.renderer);
                let id = renderer
                    .render(&submission.layer_request(layer))
                    .map_err(UiError::Render)?;
                let frame = renderer.frame(id).map_err(UiError::Render)?;
                drop(raster_span);
                lap(&mut self.frame_timing, Phase::Raster, rastering);
                let committing = mark(&self.frame_timing);
                let raised = RaisedLayer {
                    frame,
                    height: *layer_height,
                };
                let shown = tracing::trace_span!("ui.present", layer = "raised")
                    .in_scope(|| self.system.present_layer(w.id, Some(raised)))
                    .map_err(UiError::Platform)?;
                handed_over(&mut self.frame_timing, &mut self.system, committing);
                Some(shown)
            }
        };
        drop(submission);
        match shown {
            Some(PresentOutcome::NotReady) => {
                w.frame_ready = false;
                w.dirty = true;
                w.invalidate_damage();
                w.layer.shown = None;
                self.system.request_frame(w.id);
                return Ok(());
            }
            Some(PresentOutcome::Presented) => w.layer.shown = layer_height,
            None => {}
        }
        let rastering = mark(&self.frame_timing);
        let mut renderer = held(&w.renderer);
        let request = RenderRequest {
            list: &w.display_list,
            resources: &w.resources,
            scale,
            width,
            height,
            damage: &damage,
        };
        // the frame drawn straight into a buffer the window system lends, where it lends one; else drawn into the
        // renderer's own image and copied
        let mut lent = None;
        if renderer.draws_into_memory() {
            let changes = match &damage {
                DamageRegion::Full => vec![PixelRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }],
                DamageRegion::Rects(rects) => damage_rects(rects, scale, width, height),
            };
            let timing = &mut self.frame_timing;
            let (mut failed, mut drawn, mut committing) = (None, None, None);
            let _present = tracing::trace_span!("ui.present", lent = true).entered();
            let outcome = self
                .system
                .present_into(w.id, (width, height), w.ack, &changes, &mut |target| {
                    let into = tracing::trace_span!("ui.raster", layer = "content")
                        .in_scope(|| renderer.render_into(&request, target));
                    lap(timing, Phase::Raster, rastering);
                    committing = mark(timing);
                    match into {
                        Ok((id, damage)) => {
                            drawn = Some(id);
                            Some((id, damage))
                        }
                        Err(e) => {
                            failed = Some(e);
                            None
                        }
                    }
                })
                .map_err(UiError::Platform)?;
            if let Some(e) = failed {
                return Err(UiError::Render(e));
            }
            if let Lent::Drawn(outcome) = outcome {
                if drawn.is_none() {
                    // no buffer was free and the renderer never saw the frame, whose damage the tracker has already
                    // taken: the next frame draws it all, so what the renderer keeps between frames (its scroll
                    // images among it) is painted again where this one would have changed it
                    w.damage.invalidate();
                }
                handed_over(&mut self.frame_timing, &mut self.system, committing);
                lent = Some((outcome, drawn));
            }
        }
        let (outcome, id) = match lent {
            Some(lent) => lent,
            None => {
                let raster_span = tracing::trace_span!("ui.raster", layer = "content").entered();
                let id = renderer.render(&request).map_err(UiError::Render)?;
                let frame = renderer.frame(id).map_err(UiError::Render)?;
                drop(raster_span);
                lap(&mut self.frame_timing, Phase::Raster, rastering);
                let committing = mark(&self.frame_timing);
                let outcome = tracing::trace_span!("ui.present")
                    .in_scope(|| self.system.present(w.id, frame, w.ack))
                    .map_err(UiError::Platform)?;
                handed_over(&mut self.frame_timing, &mut self.system, committing);
                (outcome, Some(id))
            }
        };
        w.failed = None;
        match outcome {
            PresentOutcome::Presented => {
                if let Some(t) = &mut self.frame_timing {
                    t.presented();
                }
                w.ack = None;
                w.owed = false;
                w.frame_ready = false;
                w.committed = true;
                self.system.request_frame(w.id);
                if let (Some(timing), Some(id)) = (&mut self.scroll_timing, id) {
                    timing.submitted(w.id, id, self.options.clock.now());
                }
            }
            // the frame is drawn (or, into a lent buffer, not drawn) and the window system keeps its damage for the next
            // present, which only draws what changes after it
            PresentOutcome::NotReady => {
                w.frame_ready = false;
                w.dirty = true;
                w.owed = true;
                self.system.request_frame(w.id);
            }
        }
        Ok(())
    }

    /// Gives an adapter that has just become active, or a window it has not seen yet, the whole tree; later frames
    /// send only what changed.
    pub(super) fn sync_accessibility(&mut self) {
        let Some(adapter) = &mut self.options.accessibility else {
            return;
        };
        let active = adapter.is_active();
        for w in &mut self.windows {
            if !active {
                w.accessible = false;
                continue;
            }
            if w.fade.is_some() {
                continue;
            }
            let Some(root) = w.root else { continue };
            if w.accessible || !w.prepared {
                continue;
            }
            adapter.window_title(w.id, &w.spec.title);
            adapter.update(&AccessibilityUpdate {
                window: w.id,
                root,
                focus: w.accessibility_focus,
                nodes: w
                    .accessibility
                    .iter()
                    .map(|(&id, node)| (id, node.clone()))
                    .collect(),
                removed: Vec::new(),
                full: true,
            });
            w.accessible = true;
        }
    }

    pub(super) fn accessibility_behind(&self) -> bool {
        self.options.accessibility.as_ref().is_some_and(|adapter| {
            let active = adapter.is_active();
            self.windows.iter().any(|w| {
                w.fade.is_none() && w.root.is_some() && w.prepared && w.accessible != active
            })
        })
    }

    /// The title window `window` gives assistive technology: its spec's, a popup's too, which the window system is
    /// not told.
    pub fn window_title(&self, window: WindowId) -> Option<&str> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        Some(&w.spec.title)
    }

    /// Whether the current focus protects input. This reads widget semantics, including nodes without a role.
    pub fn protected_focus(&self, window: WindowId) -> Option<bool> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        Some(
            w.focus
                .node()
                .is_some_and(|node| protected(&self.tree, node)),
        )
    }

    /// A read-only view of one window after the last frame, for tests and the inspector.
    pub fn snapshot(&self, window: WindowId) -> Option<FrameSnapshot> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        let root = w.root?;
        let nodes = self
            .tree
            .preorder(root)
            .into_iter()
            .filter_map(|id| {
                let node = self.tree.get(id)?;
                let placement = w.geometry.get(&id);
                Some(NodeSnapshot {
                    id,
                    parent: node.parent,
                    children: node.children.to_vec(),
                    widget: node.widget.type_name(),
                    key: node.key.clone(),
                    rect: placement.map_or_else(Default::default, |p| p.rect),
                    clip: placement.and_then(|p| p.clip),
                })
            })
            .collect();
        Some(FrameSnapshot {
            window,
            nodes,
            focus: w.focus.node(),
            pointer_capture: w.pointer.capture(),
            text_input: w.text_input,
            accessibility: w
                .accessibility
                .iter()
                .map(|(&id, n)| (id, n.clone()))
                .collect(),
            accessibility_focus: w.accessibility_focus,
            display_list: (*w.display_list).clone(),
            pending: self.pending(),
            layout: w.layout,
        })
    }

    /// What `window`'s raised layer drew last: the raised nodes ([`Widget::raised`]) the window system shows over
    /// the window's content; empty while it shows no layer.
    pub fn layer_display_list(&self, window: WindowId) -> Option<DisplayList> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        Some((*w.layer.list).clone())
    }

    /// The windows the application has open, in its view's order.
    pub fn windows(&self) -> Vec<(WindowKey, WindowId)> {
        self.windows
            .iter()
            .filter(|w| w.fade.is_none())
            .map(|w| (w.key.clone(), w.id))
            .collect()
    }

    /// The node under `point` in `window`, from the last layout.
    pub fn node_at(&self, window: WindowId, point: Point) -> Option<NodeId> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        hit_test(&self.tree, w.root?, &w.geometry, point)?
            .last()
            .copied()
    }

    /// The node with `key` in `window`, searched in reading order.
    pub fn find(&self, window: WindowId, key: &Key) -> Option<NodeId> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        self.tree.preorder(w.root?).into_iter().find(|&n| {
            self.tree
                .get(n)
                .is_some_and(|n| n.key.as_ref() == Some(key))
        })
    }
}
