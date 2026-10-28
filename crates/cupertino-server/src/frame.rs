//! Frame orchestration shared by the backends: scene construction for an output, frame callbacks
//! and presentation feedback after a frame was shown.

use std::time::Duration;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::layer_map_for_output;
use smithay::desktop::utils::{
    send_frames_surface_tree, take_presentation_feedback_surface_tree, OutputPresentationFeedback,
};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::with_states;
use smithay::wayland::presentation::Refresh;

use crate::render::scene::{build_scene, CursorPlan, SceneParams, SessionOverlay};
use crate::render::CupertinoElement;
use crate::state::Cupertino;
use cupertino_scene::SceneKind;

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
    /// what the client under the pointer asked for.
    pub fn cursor_choice(&self) -> CursorChoice {
        match (self.decor.cursor, self.cursor_status.clone()) {
            (Some(icon), _) => CursorChoice::Icon(icon),
            (None, _) if self.pointer_over_unresponsive() => {
                CursorChoice::Icon(smithay::input::pointer::CursorIcon::Wait)
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
        let cursor = self.cursor_plan(self.outputs[idx].scale);
        self.with_scene_and_cursor(renderer, idx, cursor, f)
    }

    /// [`Cupertino::with_scene`] with the pointer drawn as `cursor`.
    fn with_scene_and_cursor<R>(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        cursor: CursorPlan,
        f: impl FnOnce(&mut GlesRenderer, SceneParams<'_>) -> R,
    ) -> Option<R> {
        let scale = self.outputs[idx].scale;
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
        Some(f(renderer, params))
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

    /// The element list (front to back) of output `idx`, the pointer left out with `paint_cursor` false;
    /// `None` while the render resources are not loaded. With `CUPERTINO_SCENE_ORACLE=1` at build time every list is
    /// compared with the one the previous builder gives.
    pub fn scene_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        paint_cursor: bool,
    ) -> Option<Vec<CupertinoElement>> {
        let build = |r: &mut GlesRenderer, mut p: SceneParams<'_>| {
            if !paint_cursor {
                p.cursor = CursorPlan::Hidden;
            }
            build_scene(r, p)
        };
        #[cfg(not(scene_oracle))]
        let elements = self.with_scene(renderer, idx, build)?;
        #[cfg(scene_oracle)]
        let elements = {
            use crate::render::legacy_scene;
            // both builders see the same instant and the same cursor frame
            let pinned = self.fixed_time.is_none().then(|| self.now());
            self.fixed_time = self.fixed_time.or(pinned);
            let cursor = self.cursor_plan(self.outputs[idx].scale);
            let elements = self.with_scene_and_cursor(renderer, idx, cursor.clone(), build);
            let legacy = elements.as_ref().and_then(|_| {
                self.with_scene_and_cursor(renderer, idx, cursor, |r, mut p| {
                    if !paint_cursor {
                        p.cursor = CursorPlan::Hidden;
                    }
                    legacy_scene::build_scene(r, p)
                })
            });
            if pinned.is_some() {
                self.fixed_time = None;
            }
            let (elements, legacy) = (elements?, legacy?);
            let out = &mut self.outputs[idx];
            legacy_scene::assert_parity(&mut out.cache, &legacy, &elements, out.scale);
            legacy_scene::record(legacy);
            elements
        };
        Some(elements)
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
        let elements = self.build_output_elements(renderer, idx);
        let out = &self.outputs[idx];
        let size = out
            .output
            .current_mode()
            .map(|m| m.size)
            .unwrap_or_default();
        match crate::render::screenshot::render_to_image(
            renderer,
            &elements,
            size,
            out.scale,
            SCREENSHOT_CLEAR,
        ) {
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

    /// Send frame callbacks for output `idx`; windows that are not on screen are throttled.
    pub fn send_frames(&mut self, idx: usize, time: Duration) {
        let output = self.outputs[idx].output.clone();
        let out = &output;
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
                    Some(out.clone())
                });
            }
            self.outputs[idx].redraw = false;
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
                Some(out.clone())
            });
        }
        if let Some((s, _)) = &self.drag_icon {
            send_frames_surface_tree(s, out, time, Some(HIDDEN_FRAME_THROTTLE), move |_, _| {
                Some(out.clone())
            });
        }
        self.outputs[idx].redraw = false;
        self.popups.cleanup();
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
