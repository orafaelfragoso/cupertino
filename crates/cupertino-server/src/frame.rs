//! Frame orchestration shared by the backends: scene construction for an output, frame callbacks
//! and presentation feedback after a frame was shown.

use std::time::Duration;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::utils::{
    send_frames_surface_tree, take_presentation_feedback_surface_tree, OutputPresentationFeedback,
};
use smithay::desktop::{find_popup_root_surface, layer_map_for_output, WindowSurfaceType};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::{get_parent, with_states};
use smithay::wayland::presentation::Refresh;

use crate::render::compose::Composed;
use crate::render::gles::{Gles, GlesResources};
use crate::render::scene::{
    compose_scene, CursorPlan, RenderResources, SceneParams, SessionOverlay,
};
use crate::render::shader_rect::ElementCache;
use crate::render::CupertinoElement;
use crate::state::Cupertino;
use cupertino_render_api::{Failure, RenderFailure};
use cupertino_scene::SceneKind;
use smithay::backend::renderer::damage::OutputDamageTracker;

/// Frame callbacks are throttled to this interval for surfaces that are not on screen.
pub const HIDDEN_FRAME_THROTTLE: Duration = Duration::from_secs(1);

/// Background of offscreen output renderings (`--screenshot`, screen capture) where nothing is drawn.
pub const SCREENSHOT_CLEAR: smithay::backend::renderer::Color32F =
    smithay::backend::renderer::Color32F::new(0.05, 0.05, 0.07, 1.0);

/// What the pointer shows, before it is drawn.
#[derive(Clone, Debug)]
pub enum CursorChoice {
    Hidden,
    /// A client's cursor surface.
    Surface(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface),
    /// A named cursor of the theme.
    Icon(smithay::input::pointer::CursorIcon),
}

impl Cupertino {
    /// What the pointer shows: the cursor of a server decoration zone (a resize edge) first, then the busy cursor over a
    /// window whose client does not answer its pings (it cannot change its cursor, and the one it set is stale), else
    /// what the client under the pointer asked for, except that a window the person released from holding the pointer
    /// cannot hide it.
    pub fn cursor_choice(&self) -> CursorChoice {
        match (self.decor.cursor, self.cursor_status.clone()) {
            (Some(icon), _) => CursorChoice::Icon(icon),
            (None, _) if self.pointer_over_unresponsive() => {
                CursorChoice::Icon(smithay::input::pointer::CursorIcon::Wait)
            }
            (None, CursorImageStatus::Hidden) if self.cursor_forced_visible() => {
                CursorChoice::Icon(smithay::input::pointer::CursorIcon::Default)
            }
            (None, CursorImageStatus::Named(icon)) => CursorChoice::Icon(icon),
            (None, CursorImageStatus::Hidden) => CursorChoice::Hidden,
            (None, CursorImageStatus::Surface(surface)) => CursorChoice::Surface(surface),
        }
    }

    /// What to draw for the pointer on an output of `scale`. Server-drawn cursors take the cursor
    /// scale (user size times shake); a client's cursor surface is drawn at the user size already
    /// (`XCURSOR_SIZE`), so it takes only the shake multiplier.
    fn cursor_plan(&mut self, scale: f64) -> CursorPlan {
        let icon = match self.cursor_choice() {
            CursorChoice::Icon(icon) => icon,
            CursorChoice::Hidden => return CursorPlan::Hidden,
            CursorChoice::Surface(surface) => {
                let hotspot = with_states(&surface, |s| {
                    s.data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|d| d.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                return CursorPlan::Surface {
                    surface,
                    hotspot,
                    scale: self.shake_multiplier(),
                };
            }
        };
        let t = Duration::from(self.clock.now());
        let size = f64::from(self.cursor_loader.size()) * self.cursor_scale();
        let frame = self
            .cursor_loader
            .frame(icon, (size * scale).ceil() as u32, t);
        CursorPlan::Themed { frame, size }
    }

    /// Run `f` with the scene parameters of output `idx` (renderer, model, shell and caches).
    pub fn with_scene<R>(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        f: impl FnOnce(&mut GlesRenderer, SceneParams<'_>) -> R,
    ) -> Option<R> {
        let scale = self.outputs[idx].scale;
        let cursor = self.cursor_plan(scale);
        let pointer: Point<f64, Logical> = self.pointer.current_location();
        let now = self.now();
        let drag_icon = self.drag_icon.clone();
        let lock = self.lock_view(idx);
        let session_overlays = [
            self.activation_overlay_alpha().map(|alpha| SessionOverlay {
                kind: SceneKind::ActivationFade,
                alpha,
            }),
            self.power_overlay_alpha().map(|alpha| SessionOverlay {
                kind: SceneKind::PowerOverlay,
                alpha,
            }),
        ];
        let res = self.render.as_ref()?;
        let out = &mut self.outputs[idx];
        let mode = out
            .output
            .current_mode()
            .map(|m| m.size)
            .unwrap_or_default();
        let params = SceneParams {
            output: &out.output,
            output_id: out.id,
            rect: out.rect,
            scale,
            output_px: (mode.w, mode.h),
            now,
            wm: &self.wm,
            windows: &self.windows,
            config: &self.config,
            display: self.accessibility.settings.display,
            display_filter: self.accessibility.display_filter(),
            shell: &mut self.shell,
            decor: &mut self.decor,
            menu_bars: &self.menu_bars,
            window_managers: &self.window_managers,
            overlays: &mut self.overlays,
            fx: &self.fx,
            res,
            cache: &mut out.cache,
            cursor,
            pointer,
            drag_icon,
            lock,
            session_overlays,
        };
        let result = f(renderer, params);
        // a new image was put up, perhaps for another output: that output draws it, and the lightness under its bar,
        // measured once with the decode, goes to the bar
        for (output, lightness) in self.overlays.wallpaper.take_bar_lightness() {
            let id = cupertino_wm::OutputId(output);
            if let Some(out) = self.outputs.iter_mut().find(|o| o.id == id) {
                out.scheduler.damage();
            }
            self.set_backdrop_lightness(id, Some(lightness));
        }
        Some(result)
    }

    /// Element list (front to back) of output `idx` at the current model time.
    pub fn build_output_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
    ) -> Vec<CupertinoElement> {
        self.note_scene_built(idx);
        self.scene_elements(renderer, idx, true).unwrap_or_default()
    }

    /// The element list (front to back) of output `idx`, the pointer left out with `paint_cursor` false; `None` while
    /// the render resources are not loaded. The lowering alone, for tests and tools that look at the elements.
    pub fn scene_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        paint_cursor: bool,
    ) -> Option<Vec<CupertinoElement>> {
        let composed = self.compose_output(renderer, idx, paint_cursor)?;
        let (res, cache, _) = self.gles_parts(idx)?;
        let mut resources = GlesResources::new(res, cache, composed.resources);
        Gles::lower(renderer, &composed.frame, &mut resources)
            .map_err(|e| tracing::warn!(error = %e, "lowering the scene failed"))
            .ok()
    }

    /// Compose output `idx` into its scene frame, the pointer left out with `paint_cursor` false; `None` while the
    /// render resources are not loaded.
    pub fn compose_output(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        paint_cursor: bool,
    ) -> Option<Composed> {
        self.with_scene(renderer, idx, |r, mut p| {
            if !paint_cursor {
                p.cursor = CursorPlan::Hidden;
            }
            compose_scene(r, &mut p)
        })
    }

    /// The frame of output `idx` for presentation: the session lock's built-frame mark is set as for every frame
    /// shown.
    pub fn compose_for_present(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
    ) -> Option<Composed> {
        self.note_scene_built(idx);
        let (now, refresh) = (self.frame_now(), self.refresh_of(idx));
        self.outputs[idx].scheduler.cycle(now, refresh);
        let composed = self.compose_output(renderer, idx, true);
        if composed.is_some() {
            self.outputs[idx].scheduler.composed();
        }
        composed
    }

    /// Draw output `idx` to a fresh image, the pointer left out with `paint_cursor` false. The built-frame mark of the
    /// session lock is set with `note_built`, for frames that stand for what the screen shows.
    pub fn draw_output_image(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        paint_cursor: bool,
        note_built: bool,
    ) -> anyhow::Result<image::RgbaImage> {
        use anyhow::Context;
        use cupertino_render_api::Renderer;
        if note_built {
            self.note_scene_built(idx);
        }
        let composed = self
            .compose_output(renderer, idx, paint_cursor)
            .context("the render resources are not loaded")?;
        let (res, cache, _) = self.gles_parts(idx).context("the output is gone")?;
        let mut resources = GlesResources::new(res, cache, composed.resources);
        match Gles.render(
            &composed.frame,
            &mut resources,
            crate::render::gles::GlesTarget::Image {
                renderer,
                clear: SCREENSHOT_CLEAR,
            },
        )? {
            crate::render::gles::GlesCompletion::Image(image) => Ok(image),
            _ => anyhow::bail!("the image target gave no image"),
        }
    }

    /// What the GLES renderer draws output `idx` with: the shared resources, the element cache and the damage
    /// tracker of the output.
    pub fn gles_parts(
        &mut self,
        idx: usize,
    ) -> Option<(
        &RenderResources,
        &mut ElementCache,
        &mut OutputDamageTracker,
    )> {
        let res = self.render.as_ref()?;
        let out = self.outputs.get_mut(idx)?;
        Some((res, &mut out.cache, &mut out.tracker))
    }

    /// Render window `id` at its natural size with its shadow into a texture (`None` when it is not
    /// drawn right now). Returns the texture, the captured area and the window frame's origin.
    fn capture_snapshot(
        &mut self,
        id: cupertino_wm::WindowId,
    ) -> Option<(
        crate::render::snapshots::SnapshotOwner,
        cupertino_wm::Rect,
        (f64, f64),
    )> {
        let w = self.wm.window(id)?;
        if !self.wm.window_on_screen(id) || w.state().is_minimized() || self.shell.overview_open() {
            return None;
        }
        let output = w.output;
        let idx = self.outputs.iter().position(|o| o.id == output)?;
        let mut backend = self.backend.take()?;
        let width = self.outputs[idx].rect.w;
        let active = self.wm.key_window() == Some(id);
        let now = self.now();
        let shadow_reach = {
            let g = self.decor.shadow_params(active).gpu(
                cupertino_geometry::Size::new(100.0, 100.0),
                self.config.window.corner_radius,
                1.0,
            );
            f64::from(g.radius_px) + f64::from(g.offset_px[1])
        };
        let slide = crate::render::scene::windows_on_output(
            &self.wm,
            output,
            width,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            now,
        )
        .into_iter()
        .find(|(w, _, _)| *w == id)
        .map(|(_, s, _)| s);
        let captured = slide.and_then(|slide| {
            self.with_scene(backend.renderer(), idx, |renderer, mut p| {
                let windows = p.windows;
                let entry = windows.iter().find(|w| w.id == id)?;
                let (plan, _) =
                    crate::render::scene::plan_for(p.wm, entry, slide, p.now, p.fx, p.display)?;
                let geo = entry.window.geometry();
                let ins = entry.insets;
                let natural = cupertino_wm::Rect::new(
                    plan.visual.x,
                    plan.visual.y,
                    f64::from(geo.size.w) + ins.horizontal(),
                    f64::from(geo.size.h) + ins.vertical(),
                );
                let (tex, area) = crate::render::scene::capture_window(
                    renderer,
                    &mut p,
                    entry,
                    natural,
                    active,
                    shadow_reach,
                    true,
                )?;
                Some((p.res.snapshots.insert(tex), area, (natural.x, natural.y)))
            })
            .flatten()
        });
        self.backend = Some(backend);
        captured
    }

    /// Remember the current look of a window (throttled) so that it can fade out after its surface
    /// is gone.
    pub fn refresh_snapshot(&mut self, id: cupertino_wm::WindowId) {
        let now = self.now();
        let animates_out = self
            .wm
            .window(id)
            .is_some_and(|w| self.close_transform(w.lifecycle.kind()).is_some());
        if !animates_out || self.fx.snapshot_age(id, now) < crate::fx::SNAPSHOT_INTERVAL {
            return;
        }
        if self
            .wm
            .window(id)
            .is_some_and(|w| w.lifecycle.is_animating(now))
        {
            return;
        }
        if let Some((owner, area, origin)) = self.capture_snapshot(id) {
            self.fx.store_snapshot(id, owner, area, origin, now);
        }
    }

    /// Keep a snapshot of a window that is about to disappear and fade it out (closing animation).
    /// The live window is captured while its surface still has a buffer, else the last stored
    /// snapshot is used.
    pub fn snapshot_closing(&mut self, id: cupertino_wm::WindowId) {
        if self.fx.has_ghost(id) {
            return;
        }
        let Some(w) = self.wm.window(id) else { return };
        let Some(kind) = self.close_transform(w.lifecycle.kind()) else {
            return;
        };
        let output = w.output;
        let now = self.now();
        let live = self.entry(id).is_some_and(|e| {
            smithay::backend::renderer::utils::with_renderer_surface_state(e.surface(), |st| {
                st.buffer().is_some()
            })
            .unwrap_or(false)
        });
        let frame_origin = {
            let f = w.geometry();
            (f.x, f.y)
        };
        let captured = if live {
            self.capture_snapshot(id)
        } else {
            None
        };
        let captured = captured.or_else(|| self.fx.stored_snapshot(id, frame_origin));
        if let Some((snapshot, rect, _)) = captured {
            self.fx.start_close(id, output, rect, snapshot, kind, now);
            self.request_redraw();
        }
    }

    /// `snapshot_closing` for the window owning `surface` (called before an unmapping commit).
    pub fn snapshot_closing_surface(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        if let Some(id) = self
            .entry_for_surface(surface)
            .filter(|e| e.mapped)
            .map(|e| e.id)
        {
            self.snapshot_closing(id);
        }
    }

    /// Render pending screenshot requests of output `idx` to PNG files (offscreen, full frame), and copy the
    /// screen-capture frames waiting on it ([`Cupertino::process_captures`]).
    pub fn process_screenshots(&mut self, renderer: &mut GlesRenderer, idx: usize) {
        if idx >= self.outputs.len() {
            return;
        }
        self.process_captures(renderer, idx);
        if self.screenshot_requests.is_empty() {
            return;
        }
        let paths = std::mem::take(&mut self.screenshot_requests);
        match self.draw_output_image(renderer, idx, true, true) {
            Ok(img) => {
                for p in paths {
                    if self.exit_after_screenshot {
                        self.running = false;
                        self.loop_signal.stop();
                    }
                    match crate::render::screenshot::save_png(&img, &p) {
                        Ok(()) => tracing::info!(path = %p.display(), "screenshot written"),
                        Err(e) => tracing::error!(error = %e, "writing screenshot failed"),
                    }
                }
            }
            Err(e) => tracing::error!(error = %e, "screenshot render failed"),
        }
    }

    /// Windows that are drawn on output `idx` right now (for frame callback decisions).
    fn visible_window_ids(&self, idx: usize) -> Vec<cupertino_wm::WindowId> {
        let out = &self.outputs[idx];
        let now = self.now();
        if self
            .shell
            .overview
            .as_ref()
            .is_some_and(|s| s.output == out.id)
        {
            return self
                .wm
                .windows()
                .filter(|w| w.output == out.id && !w.state().is_dead())
                .map(|w| w.id)
                .collect();
        }
        crate::render::scene::windows_on_output(
            &self.wm,
            out.id,
            out.rect.w,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            now,
        )
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
    }

    /// Take the presentation feedback callbacks of everything shown on output `idx`; call
    /// `presented` on the result once the frame reached the screen.
    pub fn collect_feedback(&mut self, idx: usize) -> OutputPresentationFeedback {
        let output = self.outputs[idx].output.clone();
        let visible = self.visible_window_ids(idx);
        let mut feedback = OutputPresentationFeedback::new(&output);
        let out = &output;
        if let Some(lock) = self.lock_surface_of(idx) {
            take_presentation_feedback_surface_tree(
                &lock,
                &mut feedback,
                move |_, _| Some(out.clone()),
                |_, _| wp_presentation_feedback::Kind::Vsync,
            );
            return feedback;
        }
        if self.session_lock.is_active() {
            return feedback;
        }
        for e in &self.windows {
            if e.mapped && visible.contains(&e.id) {
                e.window.take_presentation_feedback(
                    &mut feedback,
                    move |_, _| Some(out.clone()),
                    |_, _| wp_presentation_feedback::Kind::Vsync,
                );
            }
        }
        let map = layer_map_for_output(&output);
        for l in map.layers() {
            l.take_presentation_feedback(
                &mut feedback,
                move |_, _| Some(out.clone()),
                |_, _| wp_presentation_feedback::Kind::Vsync,
            );
        }
        if let Some((s, _)) = &self.drag_icon {
            take_presentation_feedback_surface_tree(
                s,
                &mut feedback,
                move |_, _| Some(out.clone()),
                |_, _| wp_presentation_feedback::Kind::Vsync,
            );
        }
        feedback
    }

    /// Send frame callbacks for output `idx`; windows that are not on screen are throttled, and so are the cursor and a
    /// drag icon on every output but the pointer's.
    pub fn send_frames(&mut self, idx: usize, time: Duration) {
        let output = self.outputs[idx].output.clone();
        let out = &output;
        let pointer = self.pointer_output_idx() == Some(idx);
        if self.session_lock.is_active() {
            // only the lock surface is on screen; everything else waits for the unlock
            if let Some(lock) = self.lock_surface_of(idx) {
                send_frames_surface_tree(
                    &lock,
                    out,
                    time,
                    Some(HIDDEN_FRAME_THROTTLE),
                    move |_, _| Some(out.clone()),
                );
            }
            if let CursorImageStatus::Surface(s) = &self.cursor_status {
                send_frames_surface_tree(s, out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                    pointer.then(|| out.clone())
                });
            }
            self.outputs[idx].scheduler.called_back();
            return;
        }
        let visible = self.visible_window_ids(idx);
        for e in &self.windows {
            if !e.mapped {
                continue;
            }
            let on_screen = visible.contains(&e.id);
            e.window
                .send_frame(out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                    on_screen.then(|| out.clone())
                });
        }
        {
            let map = layer_map_for_output(&output);
            for l in map.layers() {
                l.send_frame(out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                    Some(out.clone())
                });
            }
        }
        if let CursorImageStatus::Surface(s) = &self.cursor_status {
            send_frames_surface_tree(s, out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                pointer.then(|| out.clone())
            });
        }
        if let Some((s, _)) = &self.drag_icon {
            send_frames_surface_tree(s, out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                pointer.then(|| out.clone())
            });
        }
        self.outputs[idx].scheduler.called_back();
        self.popups.cleanup();
    }

    /// The tail of a frame of output `idx` that failed to draw. The screen still shows the frame before it, so nothing
    /// is reported as shown, but no client waits on a frame that never comes, and the output stays due for another try
    /// one `refresh` later: a failing output is retried, and its clients called back, at the display rate. A lost
    /// rendering context ends the session instead, so the session manager can start a new one.
    pub fn frame_failed(
        &mut self,
        idx: usize,
        time: Duration,
        refresh: Duration,
        error: &dyn RenderFailure,
    ) {
        if error.failure() == Failure::ContextLost {
            tracing::error!(error = %error, "the rendering context is lost: ending the session");
            self.rendering_lost = Some(error.to_string());
            self.running = false;
            return;
        }
        self.send_frames(idx, time);
        let count = self.outputs[idx].scheduler.failed(time, refresh);
        if count == 1 {
            tracing::warn!(error = %error, "rendering the frame failed");
        } else {
            tracing::debug!(error = %error, count, "rendering the frame failed again");
        }
    }

    /// Whether output `idx` may draw at `time`: not before the retry time of a frame that failed.
    pub fn retry_due(&self, idx: usize, time: Duration) -> bool {
        self.outputs[idx].scheduler.retry_due(time)
    }

    /// The clock frames are scheduled on: a test's fixed model time, otherwise the presentation clock, unscaled.
    pub fn frame_now(&self) -> Duration {
        self.fixed_time.map_or_else(
            || Duration::from(self.clock.now()),
            |t| Duration::from_secs_f64(t.max(0.0)),
        )
    }

    /// One refresh period of output `idx`'s mode.
    pub fn refresh_of(&self, idx: usize) -> Duration {
        let mhz = self.outputs[idx]
            .output
            .current_mode()
            .map_or(60_000, |m| m.refresh);
        Duration::from_secs_f64(1000.0 / f64::from(mhz.max(1000)))
    }

    /// Whether output `idx` draws a frame at `now`.
    pub fn frame_due(&self, idx: usize, now: Duration) -> bool {
        self.outputs[idx].scheduler.due(now, self.animating())
    }

    /// When output `idx` is next due, if anything wants a frame of it.
    pub fn frame_deadline(&self, idx: usize, now: Duration) -> Option<Duration> {
        self.outputs[idx].scheduler.deadline(now, self.animating())
    }

    /// Mark for a frame the outputs that show `surface`, all of them when none is known.
    pub fn request_redraw_for(&mut self, surface: &WlSurface) {
        match self.outputs_showing(surface) {
            Some(shown) => {
                for idx in shown {
                    self.outputs[idx].scheduler.damage();
                }
            }
            None => self.request_redraw(),
        }
    }

    /// The outputs `surface` is drawn on, by the surface it belongs to, as the scene places it: a window's (popups and
    /// subsurfaces follow their root), a layer surface's, a lock surface's, and the pointer's for the cursor, a drag
    /// icon and a window shown nowhere. `None` for anything else.
    fn outputs_showing(&self, surface: &WlSurface) -> Option<Vec<usize>> {
        let root = self.root_surface(surface);
        let pointer_surface = matches!(&self.cursor_status, CursorImageStatus::Surface(s) if *s == root)
            || self.drag_icon.as_ref().is_some_and(|(s, _)| *s == root);
        if pointer_surface {
            return self.pointer_output_idx().map(|idx| vec![idx]);
        }
        if let Some(id) = self
            .windows
            .iter()
            .find(|e| e.has_surface(&root))
            .map(|e| e.id)
        {
            let shown: Vec<usize> = (0..self.outputs.len())
                .filter(|&idx| self.visible_window_ids(idx).contains(&id))
                .collect();
            if shown.is_empty() {
                // its callbacks are throttled on whichever output sends them; one output is enough to send them
                return self.pointer_output_idx().map(|idx| vec![idx]);
            }
            return Some(shown);
        }
        if let Some(idx) = self.outputs.iter().position(|o| {
            layer_map_for_output(&o.output)
                .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                .is_some()
        }) {
            return Some(vec![idx]);
        }
        (0..self.outputs.len())
            .find(|&idx| self.lock_surface_of(idx).as_ref() == Some(&root))
            .map(|idx| vec![idx])
    }

    /// The surface `surface` belongs to: the top of its subsurface tree, through a popup to the popup's root.
    fn root_surface(&self, surface: &WlSurface) -> WlSurface {
        let top = |s: &WlSurface| {
            let mut root = s.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            root
        };
        let root = top(surface);
        match self
            .popups
            .find_popup(&root)
            .and_then(|popup| find_popup_root_surface(&popup).ok())
        {
            Some(parent) => top(&parent),
            None => root,
        }
    }

    /// The output the cursor and a drag icon are paced by: the one under the pointer, or the first powered output when
    /// that one is off, so an animated cursor keeps animating.
    pub fn pointer_output_idx(&self) -> Option<usize> {
        let powered = |idx: &usize| self.displays.powered(self.outputs[*idx].id);
        self.output_at(self.pointer.current_location())
            .and_then(|o| self.outputs.iter().position(|e| e.id == o.id))
            .filter(powered)
            .or_else(|| (0..self.outputs.len()).find(powered))
    }

    /// A frame of output `idx` was drawn, which ends a run of failed frames.
    pub fn frame_drawn(&mut self, idx: usize) {
        if let Some(failed) = self.outputs[idx].scheduler.drawn() {
            tracing::info!(failed, "rendering the frame works again");
        }
    }

    /// Frame callbacks plus an immediate presentation report (backends without a scan-out event).
    pub fn post_repaint(
        &mut self,
        idx: usize,
        time: Duration,
        refresh: Option<Duration>,
        presented: bool,
    ) {
        // these backends swap synchronously: a frame with no damage is already on the screen
        self.note_frame_shown(idx);
        self.send_frames(idx, time);
        if presented {
            let mut feedback = self.collect_feedback(idx);
            let refresh = refresh.map(Refresh::fixed).unwrap_or(Refresh::Unknown);
            feedback.presented(
                self.clock.now(),
                refresh,
                0,
                wp_presentation_feedback::Kind::Vsync,
            );
        }
    }
}
