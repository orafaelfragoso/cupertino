//! Shell: the overview session the server owns (its transition, the windows it shows and their
//! frames), and the usable area. The overview's chrome (workspaces bar, highlight, titles) and the
//! layout are the dock process's (`dock.rs` tells it the windows and takes their target frames);
//! the application switcher's panel is the dock's too.

use cupertino_animation::{AnimatedValue, Curve};
use cupertino_wm::overview::{OverviewConfig, OverviewMode, OverviewWindow, Transition};
use cupertino_wm::{OutputId, Point, Rect, WindowId, Wm, WorkspaceId};

/// The menu bar gives way to an opening overview over this many seconds, about three frames at 60 Hz, and comes back
/// at once when it closes.
pub const MENU_BAR_FADE: f64 = 0.05;

/// Under Reduce Motion the overview crossfades in and out over this many seconds instead of moving the windows.
pub const REDUCED_CROSSFADE: f64 = 0.25;

/// A window of the open overview: it flies from `from` (its desktop frame, or the dock tile of a
/// minimized window) to `target`, which the dock sets and which starts at the desktop frame.
pub struct OverviewWindowState {
    pub id: WindowId,
    pub from: Rect,
    /// The frame on the desktop (for a minimized window: before it minimized); what the dock lays out.
    pub frame: Rect,
    pub minimized: bool,
    target: AnimatedValue<[f64; 4]>,
}

/// A window's place on screen at one moment of the overview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedFrame {
    pub id: WindowId,
    pub frame: Rect,
}

/// An open overview on one output.
pub struct OverviewSession {
    pub output: OutputId,
    pub mode: OverviewMode,
    /// The area the dock lays the windows out in (global logical coordinates).
    pub area: Rect,
    pub transition: Transition,
    pub windows: Vec<OverviewWindowState>,
    /// Closing: removed once the transition came to rest.
    pub closing: bool,
    reflow: Curve,
    /// When it opened, for the menu bar's fade.
    pub opened_at: f64,
    /// When it started to close.
    pub closed_at: Option<f64>,
    /// Under Reduce Motion the windows do not move: the desktop and the overview crossfade.
    pub reduce_motion: bool,
    /// The window the person drags: drawn at its target at once, above the dock's overview surface.
    pub dragged: Option<WindowId>,
    /// Where the dock wants each workspace's live picture (global logical coordinates).
    pub pictures: Vec<(WorkspaceId, Rect)>,
}

/// Whether `mode` shows `w` (the all mode skips minimized windows, the app mode keeps one application).
fn shown(mode: OverviewMode, w: &OverviewWindow) -> bool {
    match mode {
        OverviewMode::Overview => !w.minimized,
        OverviewMode::AppOverview(app) => w.app == app,
    }
}

impl OverviewSession {
    pub fn new(
        cfg: &OverviewConfig,
        output: OutputId,
        area: Rect,
        mode: OverviewMode,
        windows: Vec<OverviewWindow>,
    ) -> Self {
        let mut session = Self {
            output,
            mode,
            area,
            transition: Transition::new(cfg),
            windows: Vec::new(),
            closing: false,
            reflow: cfg.reflow,
            opened_at: 0.0,
            closed_at: None,
            reduce_motion: false,
            dragged: None,
            pictures: Vec::new(),
        };
        session.windows = windows
            .into_iter()
            .filter(|w| shown(mode, w))
            .map(|w| OverviewWindowState {
                id: w.id,
                from: w.from,
                frame: w.size,
                minimized: w.minimized,
                target: AnimatedValue::new(w.from.to_array()),
            })
            .collect();
        session
    }

    /// Replace the window set: windows that stay keep their target, new ones start at their
    /// desktop frame. True when the set of ids changed.
    pub fn set_windows(&mut self, windows: Vec<OverviewWindow>) -> bool {
        let before: Vec<WindowId> = self.windows.iter().map(|w| w.id).collect();
        let mut next = Vec::new();
        for w in windows.into_iter().filter(|w| shown(self.mode, w)) {
            match self.windows.iter().position(|s| s.id == w.id) {
                Some(i) => next.push(self.windows.swap_remove(i)),
                None => next.push(OverviewWindowState {
                    id: w.id,
                    from: w.from,
                    frame: w.size,
                    minimized: w.minimized,
                    target: AnimatedValue::new(w.from.to_array()),
                }),
            }
        }
        self.windows = next;
        let after: Vec<WindowId> = self.windows.iter().map(|w| w.id).collect();
        before != after
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.windows.iter().any(|w| w.id == id)
    }

    /// The dock laid window `id` out at `rect`: glide there with the reflow spring. False for a
    /// window that is not in the overview.
    pub fn set_target(&mut self, id: WindowId, rect: Rect, now: f64) -> bool {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        w.target.animate_to(rect.to_array(), self.reflow, now);
        if self.dragged == Some(id) {
            self.dragged = None;
        }
        true
    }

    /// The person drags window `id`: it is drawn at `rect` at once, above the overview's chrome, until a new target
    /// takes it back. False for a window that is not in the overview.
    pub fn set_drag(&mut self, id: WindowId, rect: Rect) -> bool {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        w.target.set_immediately(rect.to_array());
        self.dragged = Some(id);
        true
    }

    /// Where the live picture of `workspace` is drawn; an empty `rect` removes it.
    pub fn set_picture(&mut self, workspace: WorkspaceId, rect: Rect) {
        self.pictures.retain(|p| p.0 != workspace);
        if rect.w > 0.0 && rect.h > 0.0 {
            self.pictures.push((workspace, rect));
        }
    }

    /// Start closing at `now`: the windows go back, the pictures and a drag end.
    pub fn close(&mut self, now: f64) {
        self.transition.toggle(false, now);
        self.closing = true;
        self.closed_at = Some(now);
        self.pictures.clear();
        self.dragged = None;
    }

    /// Opacity of the menu bar of the overview's output at `now`: it fades out over [`MENU_BAR_FADE`] as the overview
    /// opens and is back whole as soon as it starts to close.
    pub fn menu_bar_alpha(&self, now: f64) -> f64 {
        if self.closing {
            return 1.0;
        }
        (1.0 - (now - self.opened_at) / MENU_BAR_FADE).clamp(0.0, 1.0)
    }

    /// Under Reduce Motion, how far the overview has faded in at `now`: from 0 to 1 over [`REDUCED_CROSSFADE`] after
    /// it opened, back to 0 over the same time once it closes.
    pub fn crossfade(&self, now: f64) -> f64 {
        match self.closed_at {
            Some(t) => {
                let shown = ((t - self.opened_at) / REDUCED_CROSSFADE).clamp(0.0, 1.0);
                (shown - (now - t) / REDUCED_CROSSFADE).clamp(0.0, 1.0)
            }
            None => ((now - self.opened_at) / REDUCED_CROSSFADE).clamp(0.0, 1.0),
        }
    }

    /// The frames the windows are heading for (the dock's layout; the desktop frame until it answers).
    pub fn targets(&self) -> Vec<PlacedFrame> {
        self.windows
            .iter()
            .map(|w| PlacedFrame {
                id: w.id,
                frame: Rect::from_array(w.target.target()),
            })
            .collect()
    }

    /// Where every window is at `now`: between its origin and its target by the transition's progress.
    pub fn frames(&self, now: f64) -> Vec<PlacedFrame> {
        let p = self.transition.progress_at(now);
        self.windows
            .iter()
            .map(|w| PlacedFrame {
                id: w.id,
                frame: Rect::lerp(w.from, Rect::from_array(w.target.value(now)), p),
            })
            .collect()
    }

    /// The window whose target frame holds `p`: where a click lands once the overview is open,
    /// the topmost of `stacking` (bottom first) winning.
    pub fn hit(&self, p: Point, stacking: &[WindowId], now: f64) -> Option<WindowId> {
        stacking.iter().rev().copied().find(|id| {
            self.windows
                .iter()
                .any(|w| w.id == *id && Rect::from_array(w.target.value(now)).contains(p))
        })
    }

    pub fn is_animating(&self, now: f64) -> bool {
        if self.reduce_motion {
            let c = self.crossfade(now);
            if (self.closing && c > 0.0) || (!self.closing && c < 1.0) {
                return true;
            }
        }
        if !self.closing && self.menu_bar_alpha(now) > 0.0 {
            return true;
        }
        self.transition.is_animating(now) || self.windows.iter().any(|w| w.target.is_animating(now))
    }

    /// Whether a closing overview has come to rest and goes.
    pub fn is_finished(&self, now: f64) -> bool {
        self.closing
            && if self.reduce_motion {
                self.crossfade(now) <= 0.0
            } else {
                !self.transition.is_animating(now)
            }
    }
}

#[derive(Default)]
pub struct Shell {
    pub overview: Option<OverviewSession>,
}

impl Shell {
    pub fn overview_open(&self) -> bool {
        self.overview.as_ref().is_some_and(|o| !o.closing)
    }

    /// The open (not closing) overview of `output`.
    pub fn overview_on(&self, output: OutputId) -> Option<&OverviewSession> {
        self.overview
            .as_ref()
            .filter(|o| !o.closing && o.output == output)
    }

    /// Any component that needs frames at `now`.
    pub fn wants_tick(&self, now: f64) -> bool {
        self.overview
            .as_ref()
            .is_some_and(|o| o.is_animating(now) || o.closing)
    }
}

/// Area available to windows: the output minus the menu bar at the top, intersected with the
/// layer-shell exclusive zones (`layer_usable`, in the same space; the dock's is one of them).
pub fn usable_area(output: Rect, menu_bar: f64, layer_usable: Option<Rect>) -> Rect {
    let base = Rect::new(
        output.x,
        output.y + menu_bar,
        output.w,
        (output.h - menu_bar).max(1.0),
    );
    match layer_usable {
        Some(l) => base.intersection(&l).unwrap_or(base),
        None => base,
    }
}

/// The area the overview lays its windows out in: the output from its top (the menu bar gives way to the overview's
/// desktops band) down to the bottom of the `usable` area (clear of the dock), across the usable width.
pub fn overview_area(output: Rect, usable: Rect) -> Rect {
    Rect::new(
        usable.x,
        output.y,
        usable.w,
        (usable.bottom() - output.y).max(1.0),
    )
}

/// Overview entries for the windows of `output`: visible ones with their desktop frames, minimized
/// ones with their real size and the dock tile they fly in from (`dock_rect`).
pub fn overview_windows(
    wm: &Wm,
    output: OutputId,
    now: f64,
    dock_rect: Option<Rect>,
) -> Vec<OverviewWindow> {
    let mut out = Vec::new();
    let current = wm.current_workspace(output);
    for id in wm.stacking_order() {
        let Some(w) = wm.window(*id) else { continue };
        if w.output != output || w.state().is_dead() {
            continue;
        }
        let frame = wm
            .presentation(*id, now)
            .map(|p| p.frame)
            .unwrap_or_else(|| w.geometry());
        let on_ws = w.sticky || Some(w.workspace) == current;
        if !on_ws {
            continue;
        }
        if w.state().is_minimized() {
            let d = dock_rect.unwrap_or(Rect::new(frame.cx(), frame.bottom(), 1.0, 1.0));
            out.push(OverviewWindow::minimized(
                *id,
                w.app,
                &w.title,
                w.geometry(),
                d,
            ));
        } else {
            out.push(OverviewWindow::new(*id, w.app, &w.title, frame));
        }
    }
    out
}
