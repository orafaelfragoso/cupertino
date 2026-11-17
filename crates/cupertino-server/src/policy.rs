//! Window policy glue: pure helpers translating between protocol events and `cupertino-wm`
//! commands/effects. Policy itself (focus, lifecycle, workspaces, snapping) lives in the model;
//! this module converts representations and places new windows ([`Cascade`]), which the model
//! receives as their first frame.

use std::collections::HashMap;

use cupertino_wm::{Edges, ToplevelStates, WindowId};
use cupertino_wm::{Point, Rect, Size};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{ResizeEdge, State};

use crate::config::Config;

/// Protocol states for a configure: the model's states plus the activated flag.
pub fn xdg_states(states: &ToplevelStates, activated: bool) -> Vec<State> {
    let mut v = Vec::new();
    if activated {
        v.push(State::Activated);
    }
    if states.maximized {
        v.push(State::Maximized);
    }
    if states.fullscreen {
        v.push(State::Fullscreen);
    }
    if states.resizing {
        v.push(State::Resizing);
    }
    if states.tiled.left {
        v.push(State::TiledLeft);
    }
    if states.tiled.right {
        v.push(State::TiledRight);
    }
    if states.tiled.top {
        v.push(State::TiledTop);
    }
    if states.tiled.bottom {
        v.push(State::TiledBottom);
    }
    v
}

/// All states the model controls (everything except activation is owned by the model).
pub const MODEL_STATES: [State; 7] = [
    State::Maximized,
    State::Fullscreen,
    State::Resizing,
    State::TiledLeft,
    State::TiledRight,
    State::TiledTop,
    State::TiledBottom,
];

pub fn resize_edges(edge: ResizeEdge) -> Edges {
    let (l, r, t, b) = match edge {
        ResizeEdge::Top => (false, false, true, false),
        ResizeEdge::Bottom => (false, false, false, true),
        ResizeEdge::Left => (true, false, false, false),
        ResizeEdge::Right => (false, true, false, false),
        ResizeEdge::TopLeft => (true, false, true, false),
        ResizeEdge::TopRight => (false, true, true, false),
        ResizeEdge::BottomLeft => (true, false, false, true),
        ResizeEdge::BottomRight => (false, true, false, true),
        _ => (false, false, false, false),
    };
    Edges {
        left: l,
        right: r,
        top: t,
        bottom: b,
    }
}

/// Size a new window opens at: the client's when it has one, otherwise a fraction of `v`; at most
/// the size of `v` (approximate stand-in for the design's constraint to the screen).
pub fn new_window_size(v: Rect, size: Option<Size>, cfg: &Config) -> Size {
    let size = size.filter(|s| s.w > 0.0 && s.h > 0.0).unwrap_or_else(|| {
        Size::new(
            (v.w * cfg.default_window_fraction).round(),
            (v.h * cfg.default_window_fraction).round(),
        )
    });
    Size::new(size.w.min(v.w), size.h.min(v.h))
}

/// `x` rounded to the nearest integer when within 1/8 of it, else rounded down (the design's
/// pixel snapping of placement values, the strict 1/8 bound, cmpltsd at both sites).
fn snap_floor(x: f64) -> f64 {
    if (x - x.round()).abs() < 0.125 {
        x.round()
    } else {
        x.floor()
    }
}

/// A window of `size` centred in the visible frame `v`: horizontally centred,
/// a quarter of the vertical slack above it and three quarters below, its top on `v`'s top when it
/// is taller. The design's bottom-up `y = minY + min(dy, ⌊0.75·dy⌋) in top-down coordinates.
pub fn centred(v: Rect, size: Size) -> Rect {
    let (dx, dy) = (v.w - size.w, v.h - size.h);
    Rect::new(
        v.x + snap_floor(0.5 * dx),
        v.y + dy - dy.min(snap_floor(0.75 * dy)),
        size.w,
        size.h,
    )
}

/// Cascade state of one application: the wrap offsets, statics of the design's application
/// process, and the point each of its windows left for the next one.
#[derive(Debug, Default)]
struct AppCascade {
    x_offset: f64,
    y_offset: f64,
    /// Oldest first, so the last entry on an output is the newest window there.
    next: Vec<(WindowId, Point)>,
}

impl AppCascade {
    /// The design's in top-down coordinates,
    /// for a window at `frame` in the visible frame `v` of the output `screen`, cascading from
    /// `from` (`None`: the zero point, which leaves the window where it is and skips the horizontal
    /// test). Returns the window's top-left and the point the next window cascades from.
    fn cascade(
        &mut self,
        frame: Rect,
        from: Option<Point>,
        v: Rect,
        screen: Rect,
        step: f64,
    ) -> (Point, Point) {
        let (w, h) = (frame.w, frame.h);
        let leaves_x = |x: f64| x < v.x || x + w > v.right();
        let x = match from {
            None => frame.x,
            Some(p) if leaves_x(p.x) => {
                self.x_offset += 0.5 * step;
                if leaves_x(v.x + self.x_offset) {
                    self.x_offset = 0.0;
                }
                v.x + self.x_offset
            }
            Some(p) => p.x,
        };
        let leaves_y = |top: f64| top + h > v.bottom() || top < v.y;
        // the design compares its bottom-up top edge with 4 steps: within 4 steps of the bottom
        // of the screen (the active output's bottom stands for the main screen's)
        let near_bottom = |top: f64| screen.bottom() - top < 4.0 * step;
        let mut top = from.map_or(frame.y, |p| p.y);
        if leaves_y(top) {
            self.y_offset += snap_floor(0.5 * step);
            top = v.y + self.y_offset;
            if leaves_y(top) || near_bottom(top) {
                self.y_offset = 0.0;
                top = v.y;
            }
        }
        (Point::new(x, top), Point::new(x + step, top + step))
    }
}

/// Where new windows open: the first window of an application is centred
/// ([`centred`]); each later one cascades `cascade_step` right and down from the point its newest
/// window on the same output left, wrapping inside the visible frame. That the design's window
/// controller class maps onto the application identity is.
#[derive(Debug, Default)]
pub struct Cascade {
    apps: HashMap<String, AppCascade>,
}

impl Cascade {
    /// Forget the windows `alive` rejects (closed windows no longer lead a cascade).
    pub fn retain(&mut self, alive: impl Fn(WindowId) -> bool) {
        for app in self.apps.values_mut() {
            app.next.retain(|(id, _)| alive(*id));
        }
    }

    /// Frame of the new window `window` of application `app`, of `size`, on the output `screen`
    /// whose visible frame (usable area, or the stage frame) is `v`; `step` is the cascade step.
    pub fn place(
        &mut self,
        app: &str,
        window: WindowId,
        size: Size,
        v: Rect,
        screen: Rect,
        step: f64,
    ) -> Rect {
        let state = self.apps.entry(app.to_string()).or_default();
        let from = state
            .next
            .iter()
            .rev()
            .map(|(_, p)| *p)
            .find(|p| screen.contains(*p));
        let start = centred(v, size);
        let (top_left, next) = state.cascade(start, from, v, screen, step);
        state.next.push((window, next));
        Rect::new(top_left.x, top_left.y, size.w, size.h)
    }
}

/// Application identity string for grouping: the toolkit application id, falling back to the title.
pub fn app_identity(app_id: Option<&str>, title: Option<&str>) -> String {
    app_id
        .filter(|s| !s.is_empty())
        .or(title.filter(|s| !s.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

/// Seconds since an arbitrary epoch from the presentation clock (monotonic milliseconds).
pub fn clock_seconds(now: std::time::Duration) -> f64 {
    now.as_secs_f64()
}

/// Pointer position in model coordinates.
pub fn point_of(x: f64, y: f64) -> Point {
    Point::new(x, y)
}
