//! Warp minimize: a window funnels into its dock slot in two overlapping phases.
//!
//! * two phases with virtual durations `T_slide = trunc(ln(max(2|s|, (|dxL|+|dxR|)/8)·0.5 + 1))·50`
//!   and `T_stretch = trunc(ln(|L|·0.5 + 1))·50` (only their ratio matters; `s` is how far the
//!   window slides until its near edge reaches the slot, / the cross-axis offsets between
//!   window and slot edges, `L` the stretch length); the stretch is delayed by
//!   `max(T_slide − u, 0)` with `u = acos(1 − 2|L − L0|/|L|)/π · T_stretch`; both phases use the
//!   ease `0.5 − 0.5·cos(π·x)`; the virtual time is `p · (delay + T_stretch).
//! * the mesh is two vertices wide and N vertices along the travel axis, N from recursive
//!   midpoint subdivision of the S-curve that stops when a sub-interval is shorter than 8 pt, or
//!   shorter than 128 pt with a chord error below 0.6 pt, with at most 400 samples.
//! * the sides are displaced by `dx · e1 · S(y)` with the S-curve `S(y) = 0.5 − 0.5·cos(π·y),
//!   the near end moves by `slide · e1` and the far end by `L · e2`.
//! * how `L` and `L0` relate to the rectangles (here `L` is the distance the far end
//!   travels to reach the slot and `L0` the window length along the travel axis), the mapping of
//!   mesh vertices to window texture coordinates (uniform along the travel axis), the direction
//!   selection, the maximum subdivision depth (8), a floor of one time unit (50 ms virtual) for
//!   degenerate phases, and evaluating the chord error at the quarter points as well as the
//!   midpoint (the midpoint error of the full S-curve is zero by symmetry).
//! * landing, [`Landing::IconRect`] (default): the phases above collapse the window
//!   onto an edge of the slot at `p = 1`; the extension lets the near end continue to the far edge
//!   of the slot and blends the side offset from `e2` at the far end to `e1` at the near end, so the
//!   mesh at `p = 1` is exactly the slot rectangle. [`Landing::IconEdge`] stops on the slot's edge.

use crate::ease::cosine;
use crate::mesh::{Mesh, Vertex};
use cupertino_geometry::Rect;

/// Side of the screen the dock sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockEdge {
    Bottom,
    Left,
    Right,
}

/// Direction the window travels towards its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpDirection {
    Down,
    Up,
    Right,
    Left,
}

impl WarpDirection {
    /// Bottom dock: down, or up if the window lies below the slot; right dock: right; left dock:
    /// left (derived from the four direction codes of the design).
    pub fn for_dock(edge: DockEdge, window: Rect, slot: Rect) -> Self {
        match edge {
            DockEdge::Bottom if window.center().y > slot.center().y => WarpDirection::Up,
            DockEdge::Bottom => WarpDirection::Down,
            DockEdge::Right => WarpDirection::Right,
            DockEdge::Left => WarpDirection::Left,
        }
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, WarpDirection::Right | WarpDirection::Left)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// The mesh ends exactly on the slot rectangle (extension).
    IconRect,
    /// Literal formulas: the window collapses onto the near edge of the slot.
    IconEdge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpParams {
    /// Sub-intervals shorter than this stop subdividing (8 pt).
    pub min_segment: f64,
    /// Sub-intervals shorter than this stop when flat enough (128 pt).
    pub flat_segment: f64,
    /// Chord error below which a sub-interval is flat (0.6 pt).
    pub chord_tolerance: f64,
    /// Maximum recursion depth (8; the design limit is only known as a buffer size).
    pub max_depth: u32,
    /// Sample budget of the design buffer (400).
    pub max_samples: usize,
    /// Virtual time unit of the phase durations (50).
    pub time_unit: f64,
    pub landing: Landing,
}

impl Default for WarpParams {
    fn default() -> Self {
        Self {
            min_segment: 8.0,
            flat_segment: 128.0,
            chord_tolerance: 0.6,
            max_depth: 8,
            max_samples: 400,
            time_unit: 50.0,
            landing: Landing::IconRect,
        }
    }
}

/// Phase timing in virtual milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpTiming {
    pub slide: f64,
    pub stretch: f64,
    pub delay: f64,
}

impl WarpTiming {
    /// `slide_dist = |s|`, `cross = |dxL| + |dxR|`, `stretch_len = |L|`, `window_len = L0`.
    pub fn compute(
        slide_dist: f64,
        cross: f64,
        stretch_len: f64,
        window_len: f64,
        params: &WarpParams,
    ) -> Self {
        let unit = params.time_unit;
        let slide = ((slide_dist * 2.0).max(cross / 8.0) * 0.5 + 1.0)
            .ln()
            .trunc()
            * unit;
        let stretch = (stretch_len.abs() * 0.5 + 1.0).ln().trunc() * unit;
        let (slide, stretch) = (slide.max(unit), stretch.max(unit));
        let ratio = if stretch_len.abs() > 0.0 {
            (stretch_len - window_len).abs() / stretch_len.abs()
        } else {
            0.0
        };
        let u = (1.0 - 2.0 * ratio).clamp(-1.0, 1.0).acos() / std::f64::consts::PI * stretch;
        let delay = (slide - u).max(0.0);
        Self {
            slide,
            stretch,
            delay,
        }
    }

    pub fn total(&self) -> f64 {
        self.delay + self.stretch
    }

    /// Eased phase progress `(e1, e2)` (slide, stretch) at overall progress `p`.
    pub fn phases(&self, p: f64) -> (f64, f64) {
        let t = p.clamp(0.0, 1.0) * self.total();
        let x1 = (t / self.slide).clamp(0.0, 1.0);
        let x2 = ((t - self.delay) / self.stretch).clamp(0.0, 1.0);
        (cosine(x1), cosine(x2))
    }
}

/// Sorted fractions along the window axis (0 = far end, 1 = near end), including both ends.
///
/// `extent` is the current length of the window along the travel axis in points and `amplitude`
/// the peak side displacement in points; the S-curve `0.5 − 0.5·cos(π·v)` is the shape whose chord
/// error is tested.
pub fn subdivide(extent: f64, amplitude: f64, params: &WarpParams) -> Vec<f64> {
    let cap_depth = ((params.max_samples.max(2) - 1) as f64).log2().floor() as u32;
    let depth_limit = params.max_depth.min(cap_depth);
    let mut out = vec![0.0];
    split(
        0.0,
        1.0,
        0,
        extent.abs(),
        amplitude.abs(),
        depth_limit,
        params,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn split(
    v0: f64,
    v1: f64,
    depth: u32,
    extent: f64,
    amp: f64,
    limit: u32,
    p: &WarpParams,
    out: &mut Vec<f64>,
) {
    let len = (v1 - v0) * extent;
    let curve = |v: f64| amp * cosine(v);
    let vm = 0.5 * (v0 + v1);
    let chord_error = [0.25, 0.5, 0.75]
        .iter()
        .map(|&t| (curve(v0 + t * (v1 - v0)) - (curve(v0) + t * (curve(v1) - curve(v0)))).abs())
        .fold(0.0, f64::max);
    let leaf = depth >= limit
        || len < p.min_segment
        || (len < p.flat_segment && chord_error < p.chord_tolerance);
    if leaf {
        out.push(v1);
    } else {
        split(v0, vm, depth + 1, extent, amp, limit, p, out);
        split(vm, v1, depth + 1, extent, amp, limit, p, out);
    }
}

/// Window and slot expressed along the travel axis.
#[derive(Clone, Copy, Debug)]
pub struct WarpGeometry {
    pub direction: WarpDirection,
    origin: [f64; 2],
    axis: [f64; 2],
    cross: [f64; 2],
    /// Window length along the travel axis (`L0`).
    pub length: f64,
    /// Window length across the travel axis.
    pub width: f64,
    /// Distance the window must slide until its near edge reaches the slot, floored at 0 (`s`).
    pub slide: f64,
    far_target: f64,
    near_target: f64,
    /// Cross-axis offset between the low (left/top) edges of slot and window.
    pub dx_low: f64,
    /// Cross-axis offset between the high (right/bottom) edges of slot and window.
    pub dx_high: f64,
    pub timing: WarpTiming,
    params: WarpParams,
}

impl WarpGeometry {
    pub fn new(window: Rect, slot: Rect, direction: WarpDirection, params: &WarpParams) -> Self {
        let (origin, axis, cross, length, width, icon_near, icon_far, low, high) = match direction {
            WarpDirection::Down => (
                [window.min_x(), window.min_y()],
                [0.0, 1.0],
                [1.0, 0.0],
                window.height(),
                window.width(),
                slot.min_y() - window.min_y(),
                slot.max_y() - window.min_y(),
                slot.min_x() - window.min_x(),
                slot.max_x() - window.min_x(),
            ),
            WarpDirection::Up => (
                [window.min_x(), window.max_y()],
                [0.0, -1.0],
                [1.0, 0.0],
                window.height(),
                window.width(),
                window.max_y() - slot.max_y(),
                window.max_y() - slot.min_y(),
                slot.min_x() - window.min_x(),
                slot.max_x() - window.min_x(),
            ),
            WarpDirection::Right => (
                [window.min_x(), window.min_y()],
                [1.0, 0.0],
                [0.0, 1.0],
                window.width(),
                window.height(),
                slot.min_x() - window.min_x(),
                slot.max_x() - window.min_x(),
                slot.min_y() - window.min_y(),
                slot.max_y() - window.min_y(),
            ),
            WarpDirection::Left => (
                [window.max_x(), window.min_y()],
                [-1.0, 0.0],
                [0.0, 1.0],
                window.width(),
                window.height(),
                window.max_x() - slot.max_x(),
                window.max_x() - slot.min_x(),
                slot.min_y() - window.min_y(),
                slot.max_y() - window.min_y(),
            ),
        };
        let slide = (icon_near - length).max(0.0);
        let far_target = icon_near.max(0.0);
        let near_target = match params.landing {
            Landing::IconRect => icon_far.max(far_target),
            Landing::IconEdge => (length + slide).max(far_target),
        };
        let dx_low = low;
        let dx_high = high - width;
        let timing = WarpTiming::compute(
            slide,
            dx_low.abs() + dx_high.abs(),
            far_target,
            length,
            params,
        );
        Self {
            direction,
            origin,
            axis,
            cross,
            length,
            width,
            slide,
            far_target,
            near_target,
            dx_low,
            dx_high,
            timing,
            params: *params,
        }
    }

    fn uv(&self, fa: f64, fb: f64) -> [f32; 2] {
        let (u, v) = match self.direction {
            WarpDirection::Down => (fb, fa),
            WarpDirection::Up => (fb, 1.0 - fa),
            WarpDirection::Right => (fa, fb),
            WarpDirection::Left => (1.0 - fa, fb),
        };
        [u as f32, v as f32]
    }

    fn point(&self, a: f64, b: f64) -> [f32; 2] {
        [
            (self.origin[0] + self.axis[0] * a + self.cross[0] * b) as f32,
            (self.origin[1] + self.axis[1] * a + self.cross[1] * b) as f32,
        ]
    }

    /// Positions of the far and near end along the travel axis (from the far end's start) and the
    /// phase progress at `p`.
    pub fn state(&self, p: f64) -> WarpState {
        let (e1, e2) = self.timing.phases(p);
        let near = self.length + (self.near_target - self.length) * e1;
        let far = (self.far_target * e2).min(near);
        WarpState { e1, e2, far, near }
    }

    /// Mesh at overall progress `p ∈ [0, 1]` (values outside are clamped).
    pub fn mesh(&self, p: f64) -> Mesh {
        let st = self.state(p);
        let peak = self.dx_low.abs().max(self.dx_high.abs());
        let amplitude = match self.params.landing {
            Landing::IconEdge => peak * st.e1,
            Landing::IconRect => peak * (st.e1 - st.e2).abs(),
        };
        let vs = subdivide(st.near - st.far, amplitude, &self.params);
        let offset = |dx: f64, v: f64| -> f64 {
            let s = cosine(v);
            match self.params.landing {
                Landing::IconEdge => dx * st.e1 * s,
                Landing::IconRect => dx * (st.e2 + (st.e1 - st.e2) * s),
            }
        };
        let make = |v: f64, high: bool| -> Vertex {
            let a = st.far + v * (st.near - st.far);
            let b = if high {
                self.width + offset(self.dx_high, v)
            } else {
                offset(self.dx_low, v)
            };
            Vertex {
                position: self.point(a, b),
                uv: self.uv(v, if high { 1.0 } else { 0.0 }),
            }
        };
        let n = vs.len();
        let mut vertices = Vec::with_capacity(2 * n);
        if self.direction.is_horizontal() {
            for high in [false, true] {
                for &v in &vs {
                    vertices.push(make(v, high));
                }
            }
            Mesh::new(n, 2, vertices)
        } else {
            for &v in &vs {
                vertices.push(make(v, false));
                vertices.push(make(v, true));
            }
            Mesh::new(2, n, vertices)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpState {
    pub e1: f64,
    pub e2: f64,
    /// Far end position along the travel axis, measured from the window's far edge.
    pub far: f64,
    /// Near end position along the travel axis, measured from the window's far edge.
    pub near: f64,
}

/// One-shot convenience: the mesh of a warp minimize at progress `p`.
pub fn warp_mesh(
    window: Rect,
    slot: Rect,
    direction: WarpDirection,
    p: f64,
    params: &WarpParams,
) -> Mesh {
    WarpGeometry::new(window, slot, direction, params).mesh(p)
}
