//! Window packing for the overview modes: the design's iterative relaxation. Windows start at their
//! desktop places scaled into the work area and are pushed apart and shrunk until no pair overlaps
//! by more than the tolerance.

pub use cupertino_wm::geometry::Rect;

/// Constants of the packing.: the work-area inset, the gap argument, the
/// vertical offset, the stop tolerance, the shrink constant and the pass limit. the gap's
/// role (space kept between windows), the start scale from the windows' bounding box, the centre
/// as the point windows scale about, and the repulsion as the shallower axis of each overlap.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutParams {
    /// The display rectangle is inset by this much before packing (20 pt).
    pub inset: f64,
    /// Space kept between two windows (8 pt).
    pub gap: f64,
    /// Windows are placed this much above their scaled centre (20 pt).
    pub lift: f64,
    /// Packing stops when every overlap is at most this and the largest move is below it (4 pt).
    pub tolerance: f64,
    /// Shrink constant `s = s·(r + shrink·(1 − r)) (0.9).
    pub shrink: f64,
    /// Pass limit (499).
    pub max_passes: usize,
    /// Windows are never drawn larger than this fraction of their size (1, the design's `min(1, …)).
    pub max_scale: f64,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self { inset: 20.0, gap: 8.0, lift: 20.0, tolerance: 4.0, shrink: 0.9, max_passes: 499, max_scale: 1.0 }
    }
}

/// Result of one packing: the rectangles in input order and the passes it took.
#[derive(Clone, Debug, PartialEq)]
pub struct Packing {
    pub rects: Vec<Rect>,
    pub passes: usize,
}

/// Bounding box of `rects` (`None` for an empty list).
fn bounds(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(|a, b| a.union(&b))
}

/// Rect of size `s·size` centred on `c`, kept inside `work` (a window larger than the area sits at
/// its top-left corner).
fn placed(c: (f64, f64), size: Rect, s: f64, work: Rect) -> Rect {
    let (w, h) = (size.w * s, size.h * s);
    let x = (c.0 - w / 2.0).clamp(work.x, (work.right() - w).max(work.x));
    let y = (c.1 - h / 2.0).clamp(work.y, (work.bottom() - h).max(work.y));
    Rect::new(x, y, w, h)
}

/// Pack `windows` into `area`: every window keeps its aspect ratio and a common scale; see the
/// module notes for the algorithm.
pub fn pack(windows: &[Rect], area: Rect, p: &LayoutParams) -> Packing {
    let n = windows.len();
    if n == 0 {
        return Packing { rects: Vec::new(), passes: 0 };
    }
    let work = area.inset(p.inset);
    let bbox = bounds(windows).expect("n > 0");
    // 1. start scale: the arrangement as a whole fits the work area
    let mut s = p.max_scale.min(work.w / bbox.w.max(1.0)).min(work.h / bbox.h.max(1.0));
    // 2. scaled centres about the work area's centre, lifted
    let mut centres: Vec<(f64, f64)> = windows.iter().map(|w| (work.cx() + (w.cx() - work.cx()) * s, work.cy() + (w.cy() - work.cy()) * s - p.lift)).collect();
    let mut passes = 0;
    let rects = loop {
        let rects: Vec<Rect> = windows.iter().zip(&centres).map(|(w, c)| placed(*c, *w, s, work)).collect();
        centres = rects.iter().map(|r| (r.cx(), r.cy())).collect();
        // 3. overlaps (with the gap between windows) and the push that resolves each
        let mut push = vec![(0.0, 0.0); n];
        let mut overlap: f64 = 0.0;
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (rects[i].inset(-p.gap / 2.0), rects[j].inset(-p.gap / 2.0));
                let ox = a.right().min(b.right()) - a.x.max(b.x);
                let oy = a.bottom().min(b.bottom()) - a.y.max(b.y);
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                overlap = overlap.max(ox.min(oy));
                // the pair separates along the line through their centres (the same place: to the
                // sides, the later window to the right), each by half the move that clears them
                let (mut dx, mut dy) = (b.cx() - a.cx(), b.cy() - a.cy());
                let len = dx.hypot(dy);
                if len < 1e-9 {
                    (dx, dy) = (1.0, 0.0);
                } else {
                    (dx, dy) = (dx / len, dy / len);
                }
                let exit = [(ox, dx), (oy, dy)].into_iter().filter(|(_, d)| d.abs() > 1e-9).map(|(o, d)| o / d.abs()).fold(f64::INFINITY, f64::min);
                push[i].0 -= dx * exit / 2.0;
                push[i].1 -= dy * exit / 2.0;
                push[j].0 += dx * exit / 2.0;
                push[j].1 += dy * exit / 2.0;
            }
        }
        let max_move = push.iter().map(|(x, y)| x.hypot(*y)).fold(0.0, f64::max);
        // 4. stop when settled, or at the pass limit
        if (overlap <= p.tolerance && max_move < p.tolerance) || passes >= p.max_passes {
            break rects;
        }
        passes += 1;
        // 5. shrink by how little room is left, and move each window by its damped push
        let used = bounds(&rects).expect("n > 0");
        let room = ((work.w - used.w) / used.w.max(1.0)).max((work.h - used.h) / used.h.max(1.0)).clamp(0.0, 1.0);
        s *= room + p.shrink * (1.0 - room);
        let damping = 1.0 / (max_move / 10.0 + 1.0);
        for (c, (dx, dy)) in centres.iter_mut().zip(&push) {
            c.0 += dx * damping;
            c.1 += dy * damping;
        }
    };
    Packing { rects, passes }
}

/// The packed rectangles of `windows` in `area`, in input order.
pub fn layout(windows: &[Rect], area: Rect, p: &LayoutParams) -> Vec<Rect> {
    pack(windows, area, p).rects
}

/// Single centred row in input order (used for the minimized-window row). The common scale is
/// limited by the row height, the available width and `max_scale`.
pub fn layout_row(windows: &[Rect], area: Rect, gap: f64, max_scale: f64) -> Vec<Rect> {
    if windows.is_empty() {
        return vec![];
    }
    let total_w: f64 = windows.iter().map(|w| w.w).sum();
    let hmax = windows.iter().map(|w| w.h).fold(0.0, f64::max);
    let avail_w = area.w - gap * (windows.len() as f64 - 1.0);
    let scale = (avail_w / total_w).min(area.h / hmax).min(max_scale);
    let row_w = total_w * scale + gap * (windows.len() as f64 - 1.0);
    let mut x = area.x + (area.w - row_w) / 2.0;
    windows
        .iter()
        .map(|w| {
            let (rw, rh) = (w.w * scale, w.h * scale);
            let r = Rect::new(x, area.y + (area.h - rh) / 2.0, rw, rh);
            x += rw + gap;
            r
        })
        .collect()
}

/// Reading order of a set of rectangles (rows top to bottom, left to right inside a row). Rows
/// are formed by grouping rectangles whose centre-y differ by less than half the smallest height.
pub fn reading_order(rects: &[Rect]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..rects.len()).collect();
    idx.sort_by(|&a, &b| rects[a].cy().total_cmp(&rects[b].cy()));
    let tol = rects.iter().map(|r| r.h).fold(f64::INFINITY, f64::min) / 2.0;
    let mut out = Vec::with_capacity(idx.len());
    let mut row: Vec<usize> = Vec::new();
    let mut row_y = f64::NEG_INFINITY;
    for i in idx {
        if !row.is_empty() && rects[i].cy() - row_y > tol {
            row.sort_by(|&a, &b| rects[a].cx().total_cmp(&rects[b].cx()));
            out.append(&mut row);
        }
        if row.is_empty() {
            row_y = rects[i].cy();
        }
        row.push(i);
    }
    row.sort_by(|&a, &b| rects[a].cx().total_cmp(&rects[b].cx()));
    out.append(&mut row);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 90.0, 1920.0, 990.0);

    fn wins() -> Vec<Rect> {
        vec![
            Rect::new(0.0, 0.0, 1200.0, 800.0),
            Rect::new(600.0, 100.0, 900.0, 700.0),
            Rect::new(100.0, 500.0, 800.0, 500.0),
            Rect::new(1300.0, 600.0, 500.0, 400.0),
        ]
    }

    /// Every rect inside the inset work area, no pair closer than the gap minus the tolerance, no
    /// upscale, aspect kept and one common scale.
    fn check(windows: &[Rect], area: Rect) -> Packing {
        let p = LayoutParams::default();
        let packing = pack(windows, area, &p);
        let work = area.inset(p.inset);
        let r = &packing.rects;
        assert_eq!(r.len(), windows.len());
        let scale = r[0].w / windows[0].w;
        for (i, a) in r.iter().enumerate() {
            assert!(a.x >= work.x - 1e-6 && a.y >= work.y - 1e-6 && a.right() <= work.right() + 1e-6 && a.bottom() <= work.bottom() + 1e-6, "{a:?} outside {work:?}");
            assert!(a.w <= windows[i].w + 1e-9, "no upscale");
            assert!((a.w / a.h - windows[i].w / windows[i].h).abs() < 1e-6, "aspect preserved");
            assert!((a.w / windows[i].w - scale).abs() < 1e-6, "one common scale");
            for b in &r[i + 1..] {
                let (ga, gb) = (a.inset(-p.gap / 2.0), b.inset(-p.gap / 2.0));
                let ox = ga.right().min(gb.right()) - ga.x.max(gb.x);
                let oy = ga.bottom().min(gb.bottom()) - ga.y.max(gb.y);
                assert!(ox <= p.tolerance + 1e-6 || oy <= p.tolerance + 1e-6, "{a:?} and {b:?} overlap by {ox}×{oy}");
            }
        }
        assert!(packing.passes <= p.max_passes);
        packing
    }

    #[test]
    fn overlapping_windows_are_pushed_apart_inside_the_inset_work_area() {
        let packing = check(&wins(), AREA);
        assert!(packing.passes > 0, "the cascade overlaps and needs relaxation");
        assert!(packing.passes < LayoutParams::default().max_passes, "and settles before the pass limit");
    }

    #[test]
    fn a_lone_window_keeps_its_size_and_rises_by_the_lift() {
        let w = Rect::new(400.0, 300.0, 800.0, 600.0);
        let packing = check(&[w], AREA);
        let r = packing.rects[0];
        assert_eq!(packing.passes, 0);
        assert_eq!((r.w, r.h), (800.0, 600.0));
        assert!((r.cx() - w.cx()).abs() < 1e-9);
        assert!((r.cy() - (w.cy() - 20.0)).abs() < 1e-9, "20 pt above its centre: {r:?}");
    }

    #[test]
    fn a_window_larger_than_the_area_is_scaled_to_fit_without_a_pass() {
        let packing = check(&[Rect::new(-100.0, 0.0, 3000.0, 1500.0)], AREA);
        let work = AREA.inset(20.0);
        let r = packing.rects[0];
        assert_eq!(packing.passes, 0);
        assert!((r.w - work.w).abs() < 1e-6 || (r.h - work.h).abs() < 1e-6, "touches the work area on one axis: {r:?}");
    }

    #[test]
    fn two_full_screen_windows_end_side_by_side_at_half_width() {
        let full = Rect::new(0.0, 90.0, 1920.0, 990.0);
        let packing = check(&[full, full], AREA);
        let [a, b] = [packing.rects[0], packing.rects[1]];
        let work = AREA.inset(20.0);
        assert!(a.w <= work.w / 2.0 + 1e-6 && b.w <= work.w / 2.0 + 1e-6, "{a:?} {b:?}");
        assert!((a.x - b.x).abs() > a.w - 4.0, "apart horizontally: {a:?} {b:?}");
    }

    #[test]
    fn many_windows_still_settle_within_the_pass_limit() {
        let many: Vec<Rect> = (0..20).map(|i| Rect::new(60.0 * (i % 7) as f64, 90.0 + 50.0 * (i % 5) as f64, 900.0, 600.0)).collect();
        let packing = check(&many, AREA);
        assert!(packing.passes <= 499);
        assert!(packing.rects[0].w < 400.0, "twenty windows are small: {:?}", packing.rects[0]);
    }

    #[test]
    fn identical_windows_separate_deterministically() {
        let same = Rect::new(500.0, 300.0, 600.0, 400.0);
        let packing = check(&[same, same, same], AREA);
        let xs: Vec<f64> = packing.rects.iter().map(|r| r.x).collect();
        assert!(xs.windows(2).all(|p| (p[0] - p[1]).abs() > 1.0), "{xs:?}");
        assert_eq!(pack(&[same, same, same], AREA, &LayoutParams::default()), packing, "the same input packs the same way");
    }

    #[test]
    fn the_shrink_constant_applies_only_while_no_room_is_left() {
        // two windows that fit beside each other once pushed apart keep their scale
        let a = Rect::new(400.0, 300.0, 500.0, 400.0);
        let b = Rect::new(500.0, 300.0, 500.0, 400.0);
        let packing = check(&[a, b], AREA);
        assert!((packing.rects[0].w - 500.0).abs() < 1e-6, "room to spread: no shrink, got {:?}", packing.rects[0]);
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert_eq!(pack(&[], AREA, &LayoutParams::default()), Packing { rects: vec![], passes: 0 });
        assert!(layout(&[], AREA, &LayoutParams::default()).is_empty());
    }

    #[test]
    fn row_layout_is_ordered_scaled_and_centred() {
        let area = Rect::new(0.0, 800.0, 1000.0, 200.0);
        let w = vec![Rect::new(0.0, 0.0, 800.0, 600.0), Rect::new(0.0, 0.0, 400.0, 300.0), Rect::new(0.0, 0.0, 600.0, 600.0)];
        let r = layout_row(&w, area, 20.0, 0.5);
        assert!(r.windows(2).all(|p| p[0].right() + 20.0 - p[1].x < 1e-9 && p[0].x < p[1].x));
        assert!(r.iter().all(|x| x.h <= 200.0 + 1e-9 && x.y >= area.y && x.bottom() <= area.bottom()));
        let s = r[0].w / w[0].w;
        assert!(r.iter().zip(&w).all(|(a, b)| (a.w / b.w - s).abs() < 1e-9));
        assert!(s <= 0.5 + 1e-12);
        let used = r.last().unwrap().right() - r[0].x;
        assert!((r[0].x - (area.w - used) / 2.0).abs() < 1e-6);
        assert!(layout_row(&[], area, 1.0, 1.0).is_empty());
    }

    #[test]
    fn row_layout_shrinks_to_width() {
        let area = Rect::new(0.0, 0.0, 300.0, 400.0);
        let w = vec![Rect::new(0.0, 0.0, 400.0, 100.0); 3];
        let r = layout_row(&w, area, 10.0, 1.0);
        assert!(r.last().unwrap().right() <= 300.0 + 1e-9 && r[0].x >= -1e-9);
    }

    #[test]
    fn reading_order_groups_rows() {
        let r = vec![
            Rect::new(500.0, 10.0, 100.0, 100.0),
            Rect::new(100.0, 20.0, 100.0, 100.0),
            Rect::new(300.0, 300.0, 100.0, 100.0),
            Rect::new(100.0, 310.0, 100.0, 100.0),
        ];
        assert_eq!(reading_order(&r), vec![1, 0, 3, 2]);
        assert!(reading_order(&[]).is_empty());
    }
}
