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
pub const MODEL_STATES: [State; 7] =
    [State::Maximized, State::Fullscreen, State::Resizing, State::TiledLeft, State::TiledRight, State::TiledTop, State::TiledBottom];

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
    Edges { left: l, right: r, top: t, bottom: b }
}

/// Size a new window opens at: the client's when it has one, otherwise a fraction of `v`; at most
/// the size of `v` (approximate stand-in for the design's constraint to the screen).
pub fn new_window_size(v: Rect, size: Option<Size>, cfg: &Config) -> Size {
    let size = size.filter(|s| s.w > 0.0 && s.h > 0.0).unwrap_or_else(|| {
        Size::new((v.w * cfg.default_window_fraction).round(), (v.h * cfg.default_window_fraction).round())
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
    Rect::new(v.x + snap_floor(0.5 * dx), v.y + dy - dy.min(snap_floor(0.75 * dy)), size.w, size.h)
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
    fn cascade(&mut self, frame: Rect, from: Option<Point>, v: Rect, screen: Rect, step: f64) -> (Point, Point) {
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
    pub fn place(&mut self, app: &str, window: WindowId, size: Size, v: Rect, screen: Rect, step: f64) -> Rect {
        let state = self.apps.entry(app.to_string()).or_default();
        let from = state.next.iter().rev().map(|(_, p)| *p).find(|p| screen.contains(*p));
        let start = centred(v, size);
        let (top_left, next) = state.cascade(start, from, v, screen, step);
        state.next.push((window, next));
        Rect::new(top_left.x, top_left.y, size.w, size.h)
    }
}

/// Application identity string for grouping: the toolkit application id, falling back to the title.
pub fn app_identity(app_id: Option<&str>, title: Option<&str>) -> String {
    app_id.filter(|s| !s.is_empty()).or(title.filter(|s| !s.is_empty())).unwrap_or("unknown").to_string()
}

/// Seconds since an arbitrary epoch from the presentation clock (monotonic milliseconds).
pub fn clock_seconds(now: std::time::Duration) -> f64 {
    now.as_secs_f64()
}

/// Pointer position in model coordinates.
pub fn point_of(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_map_one_to_one() {
        let s = ToplevelStates { maximized: true, tiled: Edges { left: true, ..Edges::NONE }, ..Default::default() };
        let v = xdg_states(&s, true);
        assert_eq!(v, vec![State::Activated, State::Maximized, State::TiledLeft]);
        assert!(xdg_states(&ToplevelStates::default(), false).is_empty());
        let fs = ToplevelStates { fullscreen: true, resizing: true, ..Default::default() };
        assert_eq!(xdg_states(&fs, false), vec![State::Fullscreen, State::Resizing]);
    }

    #[test]
    fn resize_edges_translate() {
        assert_eq!(resize_edges(ResizeEdge::BottomRight), Edges { right: true, bottom: true, ..Edges::NONE });
        assert_eq!(resize_edges(ResizeEdge::Left), Edges { left: true, ..Edges::NONE });
        assert!(!resize_edges(ResizeEdge::None).any());
    }

    const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    /// 1920 x 1080 output under a 24 pt menu bar.
    const V: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);
    const SIZE: Size = Size::new(800.0, 600.0);

    /// Frames of `n` new windows of one application.
    fn cascade_of(c: &mut Cascade, app: &str, first_id: u64, n: usize, size: Size) -> Vec<Rect> {
        (0..n).map(|i| c.place(app, WindowId(first_id + i as u64), size, V, SCREEN, 29.0)).collect()
    }

    #[test]
    fn a_first_window_is_centred_with_a_quarter_of_the_slack_above() {
        // slack 1120 x 456: x = 560, 114 above (24 + 114 = 138) and 342 below
        assert_eq!(centred(V, SIZE), Rect::new(560.0, 138.0, 800.0, 600.0));
        assert_eq!(centred(V, Size::new(801.0, 601.0)), Rect::new(559.0, 138.0, 801.0, 601.0), "slack 1119 x 455: 559.5 and 341.25 round down");
        assert_eq!(centred(V, Size::new(800.0, 1200.0)), Rect::new(560.0, 24.0, 800.0, 1200.0), "taller than the area: top on its top");
    }

    #[test]
    fn later_windows_of_the_app_cascade_29_pt_right_and_down() {
        let mut c = Cascade::default();
        let frames = cascade_of(&mut c, "org.example.notes", 1, 3, SIZE);
        assert_eq!(frames, vec![Rect::new(560.0, 138.0, 800.0, 600.0), Rect::new(589.0, 167.0, 800.0, 600.0), Rect::new(618.0, 196.0, 800.0, 600.0)]);
    }

    #[test]
    fn another_app_s_first_window_is_centred_not_cascaded() {
        let mut c = Cascade::default();
        cascade_of(&mut c, "org.example.notes", 1, 2, SIZE);
        assert_eq!(c.place("org.example.editor", WindowId(10), SIZE, V, SCREEN, 29.0), Rect::new(560.0, 138.0, 800.0, 600.0));
        assert_eq!(c.place("org.example.notes", WindowId(11), SIZE, V, SCREEN, 29.0), Rect::new(618.0, 196.0, 800.0, 600.0), "the first app goes on from its newest window");
    }

    #[test]
    fn an_overflowing_cascade_wraps_14_pt_down_then_14_5_pt_right() {
        let mut c = Cascade::default();
        let frames = cascade_of(&mut c, "org.example.notes", 1, 22, SIZE);
        // the 13th window's top (138 + 12 * 29 = 486) would end below 1080: a new row 14 pt below the top
        assert_eq!(frames[11], Rect::new(879.0, 457.0, 800.0, 600.0));
        assert_eq!(frames[12], Rect::new(908.0, 38.0, 800.0, 600.0));
        // the 21st window's left (560 + 20 * 29 = 1140) would end past 1920: a new column 14.5 pt in
        assert_eq!(frames[19], Rect::new(1111.0, 241.0, 800.0, 600.0));
        assert_eq!(frames[20], Rect::new(14.5, 270.0, 800.0, 600.0));
        assert_eq!(frames[21], Rect::new(43.5, 299.0, 800.0, 600.0));
    }

    #[test]
    fn a_wrap_that_still_does_not_fit_resets_to_the_area_s_edge() {
        let mut c = Cascade::default();
        let wide = Size::new(1910.0, 600.0);
        let frames = cascade_of(&mut c, "org.example.notes", 1, 2, wide);
        // centred at 5; the next start 34 + 1910 leaves the area, 14.5 + 1910 too: back to 0
        assert_eq!((frames[0].x, frames[1].x), (5.0, 0.0));
        assert_eq!(c.apps["org.example.notes"].x_offset, 0.0);
        let tall = Size::new(800.0, 1045.0);
        let frames = cascade_of(&mut c, "org.example.viewer", 10, 2, tall);
        // centred at 24 + 11 - 8 = 27; the next top 56 + 1045 ends below 1080, the wrapped 38 + 1045 too: back to 24
        assert_eq!((frames[0].y, frames[1].y), (27.0, 24.0));
    }

    #[test]
    fn a_wrapped_row_that_starts_within_4_steps_of_the_screen_bottom_resets() {
        let mut c = Cascade::default();
        // a visible frame ending 20 pt above the screen bottom (a bottom dock), its top 120 pt above it
        let low = Rect::new(0.0, 960.0, 1920.0, 100.0);
        let frames: Vec<(f64, f64)> =
            (1..=4).map(|i| c.place("org.example.notes", WindowId(i), Size::new(400.0, 10.0), low, SCREEN, 29.0)).map(|r| (r.x, r.y)).collect();
        // the 4th top 1070 leaves the area; the wrapped top 974 fits, but 1080 - 974 = 106 < 116: back to 960
        assert_eq!(frames, vec![(760.0, 983.0), (789.0, 1012.0), (818.0, 1041.0), (847.0, 960.0)]);
        assert_eq!(c.apps["org.example.notes"].y_offset, 0.0);
    }

    #[test]
    fn a_closed_window_or_one_on_another_output_does_not_lead_the_cascade() {
        let mut c = Cascade::default();
        cascade_of(&mut c, "org.example.notes", 1, 2, SIZE);
        c.retain(|id| id == WindowId(1));
        assert_eq!(c.place("org.example.notes", WindowId(3), SIZE, V, SCREEN, 29.0), Rect::new(589.0, 167.0, 800.0, 600.0), "from the first window, the second closed");
        let other = Rect::new(1920.0, 0.0, 1280.0, 800.0);
        let v2 = Rect::new(1920.0, 24.0, 1280.0, 776.0);
        assert_eq!(c.place("org.example.notes", WindowId(4), SIZE, v2, other, 29.0), centred(v2, SIZE), "no window of the app on that output yet");
    }

    #[test]
    fn a_new_window_without_a_size_opens_at_a_fraction_of_the_area_and_never_larger() {
        let cfg = Config::default();
        assert_eq!(new_window_size(V, None, &cfg), Size::new((1920.0 * cfg.default_window_fraction).round(), (1056.0 * cfg.default_window_fraction).round()));
        assert_eq!(new_window_size(V, Some(Size::new(5000.0, 5000.0)), &cfg), Size::new(1920.0, 1056.0));
    }

    #[test]
    fn identity_falls_back_to_title() {
        assert_eq!(app_identity(Some("org.example.app"), Some("t")), "org.example.app");
        assert_eq!(app_identity(Some(""), Some("t")), "t");
        assert_eq!(app_identity(None, None), "unknown");
    }
}
