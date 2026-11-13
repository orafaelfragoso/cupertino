//! Interactive move / resize / snap.
//!
//! ```text
//! Idle ─begin move─► Moving ─hot zone + dwell (or option accelerator)─► SnapPreview(kind) ─release─► Tiling
//! Moving ─edge dwell─► workspace switch, move continues      SnapPreview ─leave zone─► Moving
//! Idle ─begin resize─► Resizing(edges) ─release─► Idle
//! ```
//! The hard hot-zone delay (0 s), the soft hot zone (20 pt, 0.2 s) and the tiled window spacing (8 pt)
//! are as designed, as are the 20 pt corner zones, the 15 pt preview outset, the
//! 0.2268 s preview fade spring and the 5 pt dwell stillness; the Alt side extent and
//! the workspace-edge dwell are approximate / unknown and configurable. Moved and resized edges stick to other windows and the
//! usable screen rectangle through [`crate::magnet`] (thresholds).

use cupertino_animation::AnimatedValue;
use cupertino_input::drag_started;

use crate::command::{AnimationKind, Effect, Modifiers};
use crate::config::SnapConfig;
use crate::geometry::{Edges, Point, Rect};
use crate::ids::{OutputId, WindowId};
use crate::lifecycle::LifecycleState;
use crate::magnet::{Guide, Magnet, MagnetStep};
use crate::pieces::window_guides;
use crate::tiling::{hot_zone_hit, is_still, preview_frame, tile_rect, TileKind};
use crate::wm::Wm;
use crate::workspace::WorkspaceKind;

#[derive(Clone, Copy, Debug)]
struct SnapCandidate {
    kind: TileKind,
    since: f64,
    /// Where the dwell started: the zone entry, or where the pointer last left the stillness
    /// tolerance.
    anchor: Point,
    immediate: bool,
    delay: f64,
}

struct Preview {
    kind: TileKind,
    immediate: bool,
    frame: AnimatedValue<[f64; 4]>,
    opacity: AnimatedValue<f64>,
}

#[derive(Clone, Copy)]
struct EdgeDwell {
    side: i8,
    since: f64,
}

pub(crate) struct MoveState {
    pub(crate) window: WindowId,
    grab: Point,
    start_pointer: Point,
    pub(crate) start_frame: Rect,
    pulled_out: bool,
    pub(crate) output: OutputId,
    snap: Option<SnapCandidate>,
    preview: Option<Preview>,
    edge: Option<EdgeDwell>,
    pointer: Point,
    /// Created on the first step after the window is pulled out.
    magnet: Option<Magnet>,
}

pub(crate) struct ResizeState {
    pub(crate) window: WindowId,
    edges: Edges,
    start_pointer: Point,
    start_frame: Rect,
    magnet: Magnet,
    pointer: Point,
    /// Dragged edges resting near the usable area's edges and since when (size snapping).
    rest: Option<(Edges, f64)>,
}

pub(crate) enum Interaction {
    Idle,
    Moving(Box<MoveState>),
    Resizing(Box<ResizeState>),
}

impl Interaction {
    pub(crate) fn wants_tick(&self, _now: f64) -> bool {
        match self {
            Interaction::Moving(m) => m.snap.is_some() || m.edge.is_some() || m.preview.is_some(),
            Interaction::Resizing(r) => r.rest.is_some(),
            Interaction::Idle => false,
        }
    }
}

/// Presentation of the tiling preview rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewPresentation {
    pub window: WindowId,
    /// The animated tile rectangle the preview shows.
    pub rect: Rect,
    /// `rect` outset by `SnapConfig::preview_outset` on every side: the preview's own frame
    /// (15 pt, that the margin holds its shadow is ).
    pub frame: Rect,
    pub opacity: f64,
    pub kind: TileKind,
}

/// Whether the pointer has left the toolkit drag-start box around the press point.
fn left_press_box(press: Point, pointer: Point) -> bool {
    drag_started(
        cupertino_input::Point::new(press.x, press.y),
        cupertino_input::Point::new(pointer.x, pointer.y),
    )
}

impl Wm {
    pub fn is_interacting(&self) -> bool {
        !matches!(self.interaction, Interaction::Idle)
    }

    /// Whether a window is being moved (not resized).
    pub fn is_moving(&self) -> bool {
        matches!(self.interaction, Interaction::Moving(_))
    }

    pub fn interacting_window(&self) -> Option<WindowId> {
        match &self.interaction {
            Interaction::Idle => None,
            Interaction::Moving(m) => Some(m.window),
            Interaction::Resizing(r) => Some(r.window),
        }
    }

    pub fn snap_preview(&self, now: f64) -> Option<PreviewPresentation> {
        match &self.interaction {
            Interaction::Moving(m) => m.preview.as_ref().map(|p| {
                let rect = Rect::from_array(p.frame.value(now));
                PreviewPresentation {
                    window: m.window,
                    rect,
                    frame: preview_frame(rect, self.cfg.snap.preview_outset),
                    opacity: p.opacity.value(now),
                    kind: p.kind,
                }
            }),
            _ => None,
        }
    }

    pub(crate) fn begin_move(
        &mut self,
        window: WindowId,
        pointer: Point,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        self.cancel_interaction(now, out);
        let Some(w) = self.windows.get(&window) else {
            return;
        };
        let s = w.lifecycle.state();
        if s.is_fullscreen() || s.is_minimized() || s.is_dead() || s == LifecycleState::Created {
            return;
        }
        let frame = w.lifecycle.presentation(now).frame;
        let output = w.output;
        let normal = matches!(s, LifecycleState::Normal | LifecycleState::Opening);
        self.focus_window(window, now, out);
        self.interaction = Interaction::Moving(Box::new(MoveState {
            window,
            grab: Point::new(pointer.x - frame.x, pointer.y - frame.y),
            start_pointer: pointer,
            start_frame: self.windows[&window].lifecycle.normal_frame(),
            pulled_out: normal,
            output,
            snap: None,
            preview: None,
            edge: None,
            pointer,
            magnet: None,
        }));
    }

    pub(crate) fn begin_resize(
        &mut self,
        window: WindowId,
        edges: Edges,
        pointer: Point,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        self.cancel_interaction(now, out);
        if !edges.any() {
            return;
        }
        let Some(w) = self.windows.get_mut(&window) else {
            return;
        };
        let s = w.lifecycle.state();
        if s.is_fullscreen() || s.is_minimized() || s.is_dead() || s == LifecycleState::Created {
            return;
        }
        if !matches!(s, LifecycleState::Normal | LifecycleState::Opening) {
            w.lifecycle.drag_out(None, now);
            let f = w.lifecycle.normal_frame();
            w.lifecycle.set_normal_frame(f);
        }
        let start_frame = w.lifecycle.normal_frame();
        self.focus_window(window, now, out);
        let magnet = Magnet::new(pointer, start_frame, now);
        self.interaction = Interaction::Resizing(Box::new(ResizeState {
            window,
            edges,
            start_pointer: pointer,
            start_frame,
            magnet,
            pointer,
            rest: None,
        }));
        if let Some(c) = self.configure_effect(window, true) {
            out.push(c);
        }
    }

    pub(crate) fn pointer_motion(
        &mut self,
        pointer: Point,
        modifiers: Modifiers,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        match std::mem::replace(&mut self.interaction, Interaction::Idle) {
            Interaction::Moving(mut m) => {
                self.move_step(&mut m, pointer, modifiers, now, out);
                self.interaction = Interaction::Moving(m);
            }
            Interaction::Resizing(mut r) => {
                self.resize_step(&mut r, pointer, modifiers, now, out);
                self.interaction = Interaction::Resizing(r);
            }
            Interaction::Idle => {}
        }
    }

    fn move_step(
        &mut self,
        m: &mut MoveState,
        pointer: Point,
        modifiers: Modifiers,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let cfg: SnapConfig = self.cfg.snap.clone();
        m.pointer = pointer;
        if !self.windows.contains_key(&m.window) {
            return;
        }
        if let Some(o) = self.outputs.iter().find(|o| o.rect.contains(pointer)) {
            m.output = o.id;
        }
        if !m.pulled_out && !left_press_box(m.start_pointer, pointer) {
            return;
        }
        let usable = self.output(m.output).map_or(Rect::default(), |o| o.usable);
        let guides = self.magnet_guides(m.window, m.output, true, now);
        let mcfg = &self.cfg.magnet;
        let w = self.windows.get_mut(&m.window).unwrap();
        let normal_size = w.lifecycle.normal_frame().size();
        if !m.pulled_out {
            m.pulled_out = true;
            let cur = w.lifecycle.presentation(now).frame;
            let frac = ((m.start_pointer.x - cur.x) / cur.w.max(1.0)).clamp(0.0, 1.0);
            let gy = (m.start_pointer.y - cur.y).clamp(0.0, cfg.titlebar_height);
            m.grab = Point::new(frac * normal_size.w, gy);
        }
        let mut frame = Rect::from_origin_size(
            Point::new(pointer.x - m.grab.x, pointer.y - m.grab.y),
            normal_size,
        );
        if cfg.keep_titlebar_visible && usable.h > 0.0 {
            frame.y = frame
                .y
                .max(usable.y)
                .min(usable.bottom() - cfg.titlebar_height);
        }
        let step = match m.magnet.as_mut() {
            Some(magnet) => {
                let enabled = mcfg.enabled && !(mcfg.disable_with_alt && modifiers.option);
                let may_snap = !w.lifecycle.is_catching_up(now);
                magnet.step(
                    mcfg,
                    pointer,
                    frame,
                    Edges::ALL,
                    &guides,
                    enabled,
                    may_snap,
                    now,
                )
            }
            None => {
                m.magnet = Some(Magnet::new(pointer, frame, now));
                MagnetStep {
                    frame,
                    jumped: false,
                }
            }
        };
        let frame = step.frame;
        let before = w.lifecycle.normal_frame();
        let was_tiled = !matches!(
            w.lifecycle.state(),
            LifecycleState::Normal | LifecycleState::Opening
        );
        if was_tiled {
            w.lifecycle.drag_out(Some(frame), now);
        } else if step.jumped && !w.lifecycle.presentation(now).frame.contains(pointer) {
            w.lifecycle
                .catch_up_to(frame, mcfg.catch_up_duration, mcfg.catch_up_curve, now);
        } else {
            w.lifecycle.set_normal_frame(frame);
        }
        if w.output != m.output {
            w.output = m.output;
            if !w.sticky {
                if let Some(c) = self.workspaces.get(&m.output).map(|o| o.current) {
                    self.windows.get_mut(&m.window).unwrap().workspace = c;
                }
            }
        }
        if was_tiled {
            self.emit_layout(m.window, AnimationKind::DragOut, out);
        } else {
            out.push(Effect::Place {
                window: m.window,
                rect: frame,
            });
        }
        self.move_children(m.window, frame.x - before.x, frame.y - before.y, out);

        let (out_rect, dock_zone) = self
            .output(m.output)
            .map_or((Rect::default(), None), |o| (o.rect, o.dock_zone));
        // a window that cannot zoom is not offered the fill zone, which it would be refused
        let can_zoom = self.windows[&m.window].can_zoom();
        let hit = hot_zone_hit(&cfg, out_rect, usable, pointer, modifiers.option, dock_zone)
            .filter(|hit| hit.kind != TileKind::Fill || can_zoom);
        match hit {
            None => self.clear_preview(m, out),
            Some(hit) => {
                if m.snap.map(|s| s.kind) != Some(hit.kind) {
                    m.snap = Some(SnapCandidate {
                        kind: hit.kind,
                        since: now,
                        anchor: pointer,
                        immediate: hit.immediate,
                        delay: hit.delay,
                    });
                } else if let Some(s) = m.snap.as_mut() {
                    s.immediate |= hit.immediate;
                    s.delay = s.delay.min(hit.delay);
                    // a dwell that has not shown the preview yet starts again once the pointer moves
                    // past the stillness tolerance
                    if m.preview.is_none()
                        && !s.immediate
                        && !is_still(s.kind, s.anchor, pointer, cfg.dwell_stillness)
                    {
                        s.since = now;
                        s.anchor = pointer;
                    }
                }
                self.update_preview(m, now, out);
            }
        }
        self.edge_step(m, now, out);
    }

    fn clear_preview(&mut self, m: &mut MoveState, out: &mut Vec<Effect>) {
        m.snap = None;
        if m.preview.take().is_some() {
            out.push(Effect::SnapPreview {
                window: m.window,
                rect: None,
                immediate: false,
            });
        }
    }

    fn update_preview(&mut self, m: &mut MoveState, now: f64, out: &mut Vec<Effect>) {
        let cfg = &self.cfg.snap;
        let Some(s) = m.snap else { return };
        if !(s.immediate || now - s.since >= s.delay) {
            return;
        }
        let Some(usable) = self.output(m.output).map(|o| o.usable) else {
            return;
        };
        let target = tile_rect(s.kind, usable, cfg.tiled_margin);
        let (frame_curve, opacity_curve) = (cfg.preview_frame_curve, cfg.preview_opacity_curve);
        let opacity = cfg.preview_opacity;
        match m.preview.as_mut() {
            None => {
                let from = self.windows[&m.window].lifecycle.presentation(now).frame;
                let mut frame = AnimatedValue::new(from.to_array());
                frame.animate_to(target.to_array(), frame_curve, now);
                let mut op = AnimatedValue::new(0.0);
                op.animate_to(opacity, opacity_curve, now);
                m.preview = Some(Preview {
                    kind: s.kind,
                    immediate: s.immediate,
                    frame,
                    opacity: op,
                });
                out.push(Effect::SnapPreview {
                    window: m.window,
                    rect: Some(target),
                    immediate: s.immediate,
                });
            }
            Some(p) if p.kind != s.kind => {
                p.kind = s.kind;
                p.frame.animate_to(target.to_array(), frame_curve, now);
                out.push(Effect::SnapPreview {
                    window: m.window,
                    rect: Some(target),
                    immediate: p.immediate,
                });
            }
            Some(_) => {}
        }
    }

    fn edge_step(&mut self, m: &mut MoveState, now: f64, out: &mut Vec<Effect>) {
        let cfg = &self.cfg.snap;
        if !cfg.workspace_edge_switch {
            return;
        }
        let Some(r) = self.output(m.output).map(|o| o.rect) else {
            return;
        };
        let side: i8 = if m.pointer.x - r.x <= cfg.edge_threshold {
            -1
        } else if r.right() - m.pointer.x <= cfg.edge_threshold {
            1
        } else {
            0
        };
        let dwell = cfg.workspace_edge_dwell;
        match (side, m.edge) {
            (0, _) => m.edge = None,
            (s, Some(e)) if e.side == s => {
                if now - e.since >= dwell && self.edge_switch(m, s, now, out) {
                    m.edge = Some(EdgeDwell {
                        side: s,
                        since: now,
                    });
                }
            }
            (s, _) => {
                m.edge = Some(EdgeDwell {
                    side: s,
                    since: now,
                })
            }
        }
    }

    /// Move the window to the adjacent regular workspace in direction `side` and switch to it.
    fn edge_switch(
        &mut self,
        m: &mut MoveState,
        side: i8,
        now: f64,
        out: &mut Vec<Effect>,
    ) -> bool {
        let Some(ws) = self.workspaces.get(&m.output) else {
            return false;
        };
        let Some(cur) = ws.index_of(ws.current) else {
            return false;
        };
        let range: Vec<usize> = if side < 0 {
            (0..cur).rev().collect()
        } else {
            (cur + 1..ws.list.len()).collect()
        };
        let Some(target) = range
            .into_iter()
            .find(|i| ws.list[*i].kind == WorkspaceKind::Regular)
        else {
            return false;
        };
        let target = ws.list[target].id;
        self.clear_preview(m, out);
        let mut moved = vec![m.window];
        moved.extend(self.descendants(m.window));
        for id in moved {
            if let Some(w) = self.windows.get_mut(&id) {
                if !w.sticky {
                    w.workspace = target;
                }
            }
        }
        self.switch_workspace(m.output, target, false, now, out);
        true
    }

    pub(crate) fn descendants(&self, id: WindowId) -> Vec<WindowId> {
        let mut result = Vec::new();
        let mut frontier = vec![id];
        while let Some(p) = frontier.pop() {
            let mut kids: Vec<WindowId> = self
                .windows
                .values()
                .filter(|w| w.parent == Some(p))
                .map(|w| w.id)
                .collect();
            kids.sort();
            for k in kids {
                if !result.contains(&k) && k != id {
                    result.push(k);
                    frontier.push(k);
                }
            }
        }
        result
    }

    /// Children (sheets, dialogs) follow their parent while it is being moved.
    fn move_children(&mut self, parent: WindowId, dx: f64, dy: f64, out: &mut Vec<Effect>) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        for c in self.descendants(parent) {
            let Some(w) = self.windows.get_mut(&c) else {
                continue;
            };
            if matches!(
                w.lifecycle.state(),
                LifecycleState::Normal | LifecycleState::Opening
            ) {
                let f = w.lifecycle.normal_frame().translate(dx, dy);
                w.lifecycle.set_normal_frame(f);
                out.push(Effect::Place { window: c, rect: f });
            }
        }
    }

    /// Snapping guides for `window` on `output` (selection): other
    /// on-screen windows drawn at an opacity of at least the guide minimum, without a scale or
    /// minimize transform, outside the window's movement group (itself and its children), walked
    /// front to back so that a partly covered window guides only with its visible pieces
    /// ([`crate::pieces`]); and with `screen` the usable rectangle (the session rect
    /// excludes the menu bar and dock), added last.
    fn magnet_guides(
        &self,
        window: WindowId,
        output: OutputId,
        screen: bool,
        now: f64,
    ) -> Vec<Guide> {
        let cfg = &self.cfg.magnet;
        let group = self.descendants(window);
        let front_to_back: Vec<Rect> = self
            .visible_windows(output)
            .into_iter()
            .rev()
            .filter(|id| *id != window && !group.contains(id))
            .map(|id| self.windows[&id].lifecycle.presentation(now))
            .filter(|p| {
                p.opacity >= cfg.guide_min_opacity && p.scale == 1.0 && p.minimize_progress == 0.0
            })
            .map(|p| p.frame)
            .collect();
        let mut guides = window_guides(&front_to_back, cfg.min_piece);
        if let Some(u) = self
            .output(output)
            .map(|o| o.usable)
            .filter(|u| screen && u.w > cfg.screen_guide_min && u.h > cfg.screen_guide_min)
        {
            guides.push(Guide::Screen(u));
        }
        guides
    }

    fn resize_step(
        &mut self,
        r: &mut ResizeState,
        pointer: Point,
        modifiers: Modifiers,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        // sheets and child windows do not snap while resizing
        let guides = match self.windows.get(&r.window) {
            Some(w) if w.parent.is_none() => self.magnet_guides(r.window, w.output, false, now),
            _ => Vec::new(),
        };
        let mcfg = &self.cfg.magnet;
        let Some(w) = self.windows.get_mut(&r.window) else {
            return;
        };
        let (dx, dy) = (pointer.x - r.start_pointer.x, pointer.y - r.start_pointer.y);
        let s = r.start_frame;
        let (min, max) = w.size_limits();
        let mut f = s;
        if r.edges.right {
            f.w = (s.w + dx).clamp(min.w, max.w);
        }
        if r.edges.left {
            f.w = (s.w - dx).clamp(min.w, max.w);
            f.x = s.right() - f.w;
        }
        if r.edges.bottom {
            f.h = (s.h + dy).clamp(min.h, max.h);
        }
        if r.edges.top {
            f.h = (s.h - dy).clamp(min.h, max.h);
            f.y = s.bottom() - f.h;
        }
        let enabled = mcfg.enabled && !(mcfg.disable_with_alt && modifiers.option);
        // a snap the size limits reject is not kept: the magnet only advances its pointer history
        let mut trial = r.magnet.clone();
        let snapped = trial
            .step(mcfg, pointer, f, r.edges, &guides, enabled, true, now)
            .frame;
        if (min.w..=max.w).contains(&snapped.w) && (min.h..=max.h).contains(&snapped.h) {
            f = snapped;
            r.magnet = trial;
        } else {
            r.magnet
                .step(mcfg, pointer, f, r.edges, &guides, false, true, now);
        }
        w.lifecycle.set_normal_frame(f);
        let output = w.output;
        r.pointer = pointer;
        // every step restarts the settle timer of the edges that rest near the screen's edges
        r.rest = self
            .output(output)
            .filter(|_| self.cfg.snap.snap_size_to_screen_edges)
            .and_then(|o| {
                let near = near_edges(f, o.usable, r.edges, self.cfg.snap.snap_size_delta);
                near.any().then_some((near, now))
            });
        out.push(Effect::Place {
            window: r.window,
            rect: f,
        });
        if let Some(c) = self.configure_effect(r.window, true) {
            out.push(c);
        }
    }

    /// A dragged edge rested near the screen edge for the snap delay: the window takes the whole
    /// usable extent on that axis, and the resize goes on from the new frame.
    fn resize_settle(&mut self, r: &mut ResizeState, now: f64, out: &mut Vec<Effect>) {
        let Some((near, since)) = r.rest else { return };
        if now - since < self.cfg.snap.snap_size_delay {
            return;
        }
        r.rest = None;
        let axes = Edges {
            left: near.left || near.right,
            right: near.left || near.right,
            top: near.top || near.bottom,
            bottom: near.top || near.bottom,
        };
        let Some(f) = self.screen_snapped_frame(r.window, axes) else {
            return;
        };
        if self
            .windows
            .get_mut(&r.window)
            .is_some_and(|w| w.lifecycle.animate_normal_frame(f, now))
        {
            r.start_frame = f;
            r.start_pointer = r.pointer;
            r.magnet = Magnet::new(r.pointer, f, now);
            out.push(Effect::Place {
                window: r.window,
                rect: f,
            });
            if let Some(c) = self.configure_effect(r.window, true) {
                out.push(c);
            }
        }
    }

    /// The normal frame of `window` with each of `edges` moved to its output's usable edge; an axis
    /// whose new size breaks the size limits keeps its edges. `None` without a change.
    fn screen_snapped_frame(&self, window: WindowId, edges: Edges) -> Option<Rect> {
        let w = self.windows.get(&window)?;
        let u = self.output(w.output)?.usable;
        let f0 = w.lifecycle.normal_frame();
        let (min, max) = (w.min_size, w.max_size);
        let fits = |len: f64, lo: Option<f64>, hi: Option<f64>| {
            lo.is_none_or(|v| len >= v) && hi.filter(|v| *v > 0.0).is_none_or(|v| len <= v)
        };
        let (mut x0, mut x1) = (
            if edges.left { u.x } else { f0.x },
            if edges.right { u.right() } else { f0.right() },
        );
        if !fits(x1 - x0, min.map(|m| m.w), max.map(|m| m.w)) {
            (x0, x1) = (f0.x, f0.right());
        }
        let (mut y0, mut y1) = (
            if edges.top { u.y } else { f0.y },
            if edges.bottom {
                u.bottom()
            } else {
                f0.bottom()
            },
        );
        if !fits(y1 - y0, min.map(|m| m.h), max.map(|m| m.h)) {
            (y0, y1) = (f0.y, f0.bottom());
        }
        let f = Rect::new(x0, y0, x1 - x0, y1 - y0);
        (f != f0).then_some(f)
    }

    /// Double click on a resize edge or corner: the hit edges, with `opposite` also the
    /// opposite ones, move to the usable area's edges, animated. Off while full screen and when
    /// the setting is off; only a window in the normal layout snaps.
    pub(crate) fn snap_edges_to_screen(
        &mut self,
        window: WindowId,
        edges: Edges,
        opposite: bool,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        if !self.cfg.snap.snap_size_on_double_click || self.is_interacting() {
            return;
        }
        if !self
            .windows
            .get(&window)
            .is_some_and(|w| matches!(w.state(), LifecycleState::Normal | LifecycleState::Opening))
        {
            return;
        }
        let e = if opposite {
            Edges {
                left: edges.left || edges.right,
                right: edges.left || edges.right,
                top: edges.top || edges.bottom,
                bottom: edges.top || edges.bottom,
            }
        } else {
            edges
        };
        let Some(f) = self.screen_snapped_frame(window, e) else {
            return;
        };
        if self
            .windows
            .get_mut(&window)
            .is_some_and(|w| w.lifecycle.animate_normal_frame(f, now))
        {
            if let Some(c) = self.configure_effect(window, false) {
                out.push(c);
            }
            out.push(Effect::Place { window, rect: f });
        }
    }

    pub(crate) fn pointer_release(&mut self, now: f64, out: &mut Vec<Effect>) {
        match std::mem::replace(&mut self.interaction, Interaction::Idle) {
            Interaction::Moving(mut m) => {
                let kind = m.preview.as_ref().map(|p| p.kind);
                let window = m.window;
                self.clear_preview(&mut m, out);
                if let Some(kind) = kind {
                    self.tile_window(window, kind, now, out);
                }
            }
            Interaction::Resizing(r) => {
                if let Some(c) = self.configure_effect(r.window, false) {
                    out.push(c);
                }
            }
            Interaction::Idle => {}
        }
    }

    pub(crate) fn cancel_interaction(&mut self, _now: f64, out: &mut Vec<Effect>) {
        match std::mem::replace(&mut self.interaction, Interaction::Idle) {
            Interaction::Moving(mut m) => {
                self.clear_preview(&mut m, out);
                if let Some(w) = self.windows.get_mut(&m.window) {
                    if m.pulled_out && w.lifecycle.set_normal_frame(m.start_frame) {
                        out.push(Effect::Place {
                            window: m.window,
                            rect: m.start_frame,
                        });
                    }
                }
            }
            Interaction::Resizing(r) => {
                if let Some(w) = self.windows.get_mut(&r.window) {
                    w.lifecycle.set_normal_frame(r.start_frame);
                    out.push(Effect::Place {
                        window: r.window,
                        rect: r.start_frame,
                    });
                }
                if let Some(c) = self.configure_effect(r.window, false) {
                    out.push(c);
                }
            }
            Interaction::Idle => {}
        }
    }

    /// Drop the interaction without touching geometry (the window is going away or minimizing).
    pub(crate) fn cancel_interaction_for(
        &mut self,
        window: WindowId,
        _now: f64,
        out: &mut Vec<Effect>,
    ) {
        if self.interacting_window() != Some(window) {
            return;
        }
        if let Interaction::Moving(m) = &self.interaction {
            if m.preview.is_some() {
                out.push(Effect::SnapPreview {
                    window,
                    rect: None,
                    immediate: false,
                });
            }
        }
        self.interaction = Interaction::Idle;
    }

    pub(crate) fn tick_interaction(&mut self, now: f64, out: &mut Vec<Effect>) {
        match std::mem::replace(&mut self.interaction, Interaction::Idle) {
            Interaction::Moving(mut m) => {
                if m.pulled_out {
                    self.update_preview(&mut m, now, out);
                    self.edge_step(&mut m, now, out);
                }
                self.interaction = Interaction::Moving(m);
            }
            Interaction::Resizing(mut r) => {
                self.resize_settle(&mut r, now, out);
                self.interaction = Interaction::Resizing(r);
            }
            Interaction::Idle => {}
        }
    }
}

/// The dragged `edges` of `frame` that lie within `delta` of the matching edge of `usable` (the
/// per-axis edge-delta threshold of, comparison).
fn near_edges(frame: Rect, usable: Rect, edges: Edges, delta: f64) -> Edges {
    Edges {
        left: edges.left && (frame.x - usable.x).abs() <= delta,
        right: edges.right && (usable.right() - frame.right()).abs() <= delta,
        top: edges.top && (frame.y - usable.y).abs() <= delta,
        bottom: edges.bottom && (usable.bottom() - frame.bottom()).abs() <= delta,
    }
}
