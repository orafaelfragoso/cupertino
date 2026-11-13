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

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect::new(300.0, 100.0, 800.0, 500.0)
    }

    fn slot() -> Rect {
        Rect::new(1200.0, 1000.0, 64.0, 64.0)
    }

    #[test]
    fn timing_formula_values() {
        let p = WarpParams::default();
        // s = 600, |dxL| + |dxR| = 300, L = 1100 (window 500 + slide 600)
        let t = WarpTiming::compute(600.0, 300.0, 1100.0, 500.0, &p);
        assert_eq!(t.slide, 300.0, "trunc(ln(601)) = 6 units");
        assert_eq!(t.stretch, 300.0, "trunc(ln(551)) = 6 units");
    }

    #[test]
    fn timing_delay_follows_the_acos_overlap_rule() {
        let p = WarpParams::default();
        let t = WarpTiming::compute(600.0, 300.0, 1100.0, 500.0, &p);
        let u = (1.0 - 2.0 * 600.0 / 1100.0f64).acos() / std::f64::consts::PI * t.stretch;
        assert!((t.delay - (t.slide - u).max(0.0)).abs() < 1e-9);
        assert!((t.total() - (t.delay + t.stretch)).abs() < 1e-12);
    }

    #[test]
    fn phases_start_at_zero_and_end_at_one() {
        let g = WarpGeometry::new(
            window(),
            slot(),
            WarpDirection::Down,
            &WarpParams::default(),
        );
        assert_eq!(g.timing.phases(0.0), (0.0, 0.0));
        assert_eq!(g.timing.phases(1.0), (1.0, 1.0));
        let (e1, e2) = g.timing.phases(0.3);
        assert!(e1 > e2, "the slide phase leads the stretch phase");
    }

    #[test]
    fn mesh_at_zero_is_the_window() {
        for dir in [
            WarpDirection::Down,
            WarpDirection::Up,
            WarpDirection::Right,
            WarpDirection::Left,
        ] {
            let m = warp_mesh(window(), slot(), dir, 0.0, &WarpParams::default());
            assert!(m.bounds().size.width > 0.0);
            let b = m.bounds();
            assert!(
                (b.min_x() - 300.0).abs() < 1e-3 && (b.max_x() - 1100.0).abs() < 1e-3,
                "{dir:?} {b:?}"
            );
            assert!(
                (b.min_y() - 100.0).abs() < 1e-3 && (b.max_y() - 600.0).abs() < 1e-3,
                "{dir:?} {b:?}"
            );
        }
    }

    #[test]
    fn mesh_at_zero_maps_texture_uniformly_to_window() {
        let m = warp_mesh(
            window(),
            slot(),
            WarpDirection::Down,
            0.0,
            &WarpParams::default(),
        );
        for v in &m.vertices {
            let x = 300.0 + v.uv[0] as f64 * 800.0;
            let y = 100.0 + v.uv[1] as f64 * 500.0;
            assert!(
                (v.position[0] as f64 - x).abs() < 1e-3 && (v.position[1] as f64 - y).abs() < 1e-3
            );
        }
    }

    #[test]
    fn mesh_at_one_is_the_slot_with_icon_rect_landing() {
        for (dir, win, slot) in [
            (WarpDirection::Down, window(), slot()),
            (
                WarpDirection::Up,
                Rect::new(300.0, 900.0, 800.0, 400.0),
                Rect::new(200.0, 300.0, 64.0, 64.0),
            ),
            (
                WarpDirection::Right,
                window(),
                Rect::new(1800.0, 400.0, 64.0, 64.0),
            ),
            (
                WarpDirection::Left,
                window(),
                Rect::new(20.0, 300.0, 64.0, 64.0),
            ),
        ] {
            let m = warp_mesh(win, slot, dir, 1.0, &WarpParams::default());
            let b = m.bounds();
            assert!(
                (b.min_x() - slot.min_x()).abs() < 1e-3,
                "{dir:?} {b:?} vs {slot:?}"
            );
            assert!((b.max_x() - slot.max_x()).abs() < 1e-3, "{dir:?} {b:?}");
            assert!((b.min_y() - slot.min_y()).abs() < 1e-3, "{dir:?} {b:?}");
            assert!((b.max_y() - slot.max_y()).abs() < 1e-3, "{dir:?} {b:?}");
        }
    }

    #[test]
    fn uv_corners_land_on_matching_slot_corners() {
        let m = warp_mesh(
            window(),
            slot(),
            WarpDirection::Down,
            1.0,
            &WarpParams::default(),
        );
        let corner = |u: f32, v: f32| m.vertices.iter().find(|x| x.uv == [u, v]).unwrap().position;
        let s = slot();
        assert!(
            (corner(0.0, 0.0)[0] as f64 - s.min_x()).abs() < 1e-3
                && (corner(0.0, 0.0)[1] as f64 - s.min_y()).abs() < 1e-3
        );
        assert!(
            (corner(1.0, 1.0)[0] as f64 - s.max_x()).abs() < 1e-3
                && (corner(1.0, 1.0)[1] as f64 - s.max_y()).abs() < 1e-3
        );
    }

    #[test]
    fn literal_landing_collapses_onto_the_near_edge() {
        let p = WarpParams {
            landing: Landing::IconEdge,
            ..Default::default()
        };
        let m = warp_mesh(window(), slot(), WarpDirection::Down, 1.0, &p);
        let b = m.bounds();
        assert!(b.height() < 1e-3, "{b:?}");
        assert!((b.min_y() - slot().min_y()).abs() < 1e-3);
        let m0 = warp_mesh(window(), slot(), WarpDirection::Down, 0.0, &p);
        assert!((m0.bounds().height() - 500.0).abs() < 1e-3);
    }

    #[test]
    fn mesh_is_finite_and_within_the_vertex_budget_everywhere() {
        let p = WarpParams::default();
        for dir in [
            WarpDirection::Down,
            WarpDirection::Up,
            WarpDirection::Right,
            WarpDirection::Left,
        ] {
            let g = WarpGeometry::new(window(), slot(), dir, &p);
            for k in 0..=200 {
                let m = g.mesh(k as f64 / 200.0);
                assert!(m.is_finite());
                assert!(m.vertex_count() <= 800, "{}", m.vertex_count());
                assert!(m.cols == 2 || m.rows == 2);
            }
        }
    }

    #[test]
    fn travel_ends_move_monotonically() {
        let g = WarpGeometry::new(
            window(),
            slot(),
            WarpDirection::Down,
            &WarpParams::default(),
        );
        let (mut far, mut near) = (-1.0, -1.0);
        for k in 0..=400 {
            let s = g.state(k as f64 / 400.0);
            assert!(s.far >= far - 1e-9 && s.near >= near - 1e-9, "k={k}");
            assert!(s.far <= s.near + 1e-9, "no inversion");
            far = s.far;
            near = s.near;
        }
    }

    #[test]
    fn funnel_narrows_towards_the_near_end() {
        let g = WarpGeometry::new(
            window(),
            slot(),
            WarpDirection::Down,
            &WarpParams::default(),
        );
        let m = g.mesh(0.4);
        let first = m.vertex(1, 0).position[0] - m.vertex(0, 0).position[0];
        let last = m.vertex(1, m.rows - 1).position[0] - m.vertex(0, m.rows - 1).position[0];
        assert!(
            last < first,
            "near end {last} narrower than far end {first}"
        );
    }

    #[test]
    fn subdivision_rules() {
        let p = WarpParams::default();
        assert_eq!(
            subdivide(7.0, 100.0, &p),
            vec![0.0, 1.0],
            "shorter than 8 pt: a single segment"
        );
        let flat = subdivide(100.0, 0.0, &p);
        assert_eq!(flat, vec![0.0, 1.0], "short and flat: a single segment");
        let long = subdivide(1000.0, 0.0, &p);
        assert!(long.len() > 8, "{}", long.len());
        assert!(
            long.windows(2)
                .all(|w| (w[1] - w[0]) * 1000.0 < 128.0 + 1e-9),
            "never longer than 128 pt"
        );
        let curvy = subdivide(120.0, 400.0, &p);
        assert!(curvy.len() > 2, "short but strongly curved subdivides");
        for v in [flat, long, curvy] {
            assert_eq!(v[0], 0.0);
            assert_eq!(*v.last().unwrap(), 1.0);
            assert!(v.windows(2).all(|w| w[0] < w[1]));
        }
    }

    #[test]
    fn subdivision_respects_the_sample_budget() {
        let p = WarpParams {
            max_depth: 30,
            ..Default::default()
        };
        let v = subdivide(100_000.0, 5_000.0, &p);
        assert!(v.len() <= 400, "{}", v.len());
        let tiny = WarpParams {
            max_samples: 5,
            ..Default::default()
        };
        assert!(subdivide(100_000.0, 5_000.0, &tiny).len() <= 5);
    }

    #[test]
    fn direction_selection() {
        let w = window();
        assert_eq!(
            WarpDirection::for_dock(DockEdge::Bottom, w, slot()),
            WarpDirection::Down
        );
        assert_eq!(
            WarpDirection::for_dock(
                DockEdge::Bottom,
                Rect::new(0.0, 1100.0, 100.0, 100.0),
                slot()
            ),
            WarpDirection::Up
        );
        assert_eq!(
            WarpDirection::for_dock(DockEdge::Right, w, slot()),
            WarpDirection::Right
        );
        assert_eq!(
            WarpDirection::for_dock(DockEdge::Left, w, slot()),
            WarpDirection::Left
        );
    }

    #[test]
    fn overlapping_slot_does_not_produce_nan() {
        let win = Rect::new(100.0, 900.0, 300.0, 300.0);
        let sl = Rect::new(150.0, 1000.0, 64.0, 64.0);
        let g = WarpGeometry::new(win, sl, WarpDirection::Down, &WarpParams::default());
        assert_eq!(g.slide, 0.0);
        for k in 0..=50 {
            assert!(g.mesh(k as f64 / 50.0).is_finite());
        }
    }

    #[test]
    fn degenerate_inputs_stay_finite() {
        let g = WarpGeometry::new(
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Rect::new(0.0, 0.0, 0.0, 0.0),
            WarpDirection::Down,
            &WarpParams::default(),
        );
        for k in 0..=10 {
            assert!(g.mesh(k as f64 / 10.0).is_finite());
        }
    }
}
