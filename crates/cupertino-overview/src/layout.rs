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
    /// The display rectangle is inset by this much before packing (16 pt).
    pub inset: f64,
    /// Space kept between two windows (14 pt).
    pub gap: f64,
    /// Windows are placed this much above their scaled centre (20 pt).
    pub lift: f64,
    /// Packing stops when every overlap is at most this and the largest move is below it (2 pt).
    pub tolerance: f64,
    /// Shrink constant `s = s·(r + shrink·(1 − r)) (0.9).
    pub shrink: f64,
    /// Pass limit (499).
    pub max_passes: usize,
    /// Windows are never drawn larger than this fraction of their size (1, the design's `min(1, …)).
    pub max_scale: f64,
    /// Space kept under every window for its title, counted as part of the window when windows are kept apart and
    /// inside the work area, so a title never lies over another window or outside the area (0: no titles).
    pub title_space: f64,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            inset: 16.0,
            gap: 14.0,
            lift: 20.0,
            tolerance: 2.0,
            shrink: 0.9,
            max_passes: 499,
            max_scale: 1.0,
            title_space: 0.0,
        }
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

/// What a window takes in the arrangement: its rectangle and the title space under it.
fn footprint(r: &Rect, title_space: f64) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h + title_space)
}

/// Rect of size `s·size` whose footprint (with `title_space` under it) is centred on `c`, kept inside `work` (a
/// window larger than the area sits at its top-left corner).
fn placed(c: (f64, f64), size: Rect, s: f64, work: Rect, title_space: f64) -> Rect {
    let (w, h) = (size.w * s, size.h * s);
    let fh = h + title_space;
    let x = (c.0 - w / 2.0).clamp(work.x, (work.right() - w).max(work.x));
    let y = (c.1 - fh / 2.0).clamp(work.y, (work.bottom() - fh).max(work.y));
    Rect::new(x, y, w, h)
}

/// Pack `windows` into `area`: every window keeps its aspect ratio and a common scale; see the
/// module notes for the algorithm.
pub fn pack(windows: &[Rect], area: Rect, p: &LayoutParams) -> Packing {
    let n = windows.len();
    if n == 0 {
        return Packing {
            rects: Vec::new(),
            passes: 0,
        };
    }
    let work = area.inset(p.inset);
    let bbox = bounds(windows).expect("n > 0");
    // 1. start scale: the arrangement as a whole fits the work area
    let mut s = p
        .max_scale
        .min(work.w / bbox.w.max(1.0))
        .min((work.h - p.title_space).max(0.0) / bbox.h.max(1.0));
    // 2. scaled centres about the work area's centre, lifted
    let mut centres: Vec<(f64, f64)> = windows
        .iter()
        .map(|w| {
            (
                work.cx() + (w.cx() - work.cx()) * s,
                work.cy() + (w.cy() - work.cy()) * s - p.lift,
            )
        })
        .collect();
    let mut passes = 0;
    let rects = loop {
        let rects: Vec<Rect> = windows
            .iter()
            .zip(&centres)
            .map(|(w, c)| placed(*c, *w, s, work, p.title_space))
            .collect();
        let feet: Vec<Rect> = rects.iter().map(|r| footprint(r, p.title_space)).collect();
        centres = feet.iter().map(|r| (r.cx(), r.cy())).collect();
        // 3. overlaps (with the gap between windows) and the push that resolves each
        let mut push = vec![(0.0, 0.0); n];
        let mut overlap: f64 = 0.0;
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (feet[i].inset(-p.gap / 2.0), feet[j].inset(-p.gap / 2.0));
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
                let exit = [(ox, dx), (oy, dy)]
                    .into_iter()
                    .filter(|(_, d)| d.abs() > 1e-9)
                    .map(|(o, d)| o / d.abs())
                    .fold(f64::INFINITY, f64::min);
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
        let used = bounds(&feet).expect("n > 0");
        let room = ((work.w - used.w) / used.w.max(1.0))
            .max((work.h - used.h) / used.h.max(1.0))
            .clamp(0.0, 1.0);
        s *= room + p.shrink * (1.0 - room);
        let damping = 1.0 / (max_move / 10.0 + 1.0);
        for (c, (dx, dy)) in centres.iter_mut().zip(&push) {
            c.0 += dx * damping;
            c.1 += dy * damping;
        }
    };
    let rects = centred(separated(rects, windows, work, p), work, p);
    Packing { rects, passes }
}

/// Smallest space between two packed windows: the gap less the tolerance the packing stops at.
fn min_space(p: &LayoutParams) -> f64 {
    (p.gap - p.tolerance).max(0.0)
}

/// Whether the footprints of `a` and `b` are closer than `space` on both axes.
fn too_close(a: &Rect, b: &Rect, space: f64, title_space: f64) -> bool {
    let (a, b) = (footprint(a, title_space), footprint(b, title_space));
    let (a, b) = (a.inset(-space / 2.0), b.inset(-space / 2.0));
    a.right().min(b.right()) - a.x.max(b.x) > 1e-9
        && a.bottom().min(b.bottom()) - a.y.max(b.y) > 1e-9
}

/// The packing stopped at the pass limit with windows still closer than the gap less the
/// tolerance: put them in a grid of the work area instead, in the reading order of where the
/// packing left them, at one common scale, each centred in its cell. A settled packing is
/// returned as it is.
fn separated(rects: Vec<Rect>, windows: &[Rect], work: Rect, p: &LayoutParams) -> Vec<Rect> {
    let space = min_space(p);
    let n = rects.len();
    let crowded =
        (0..n).any(|i| (i + 1..n).any(|j| too_close(&rects[i], &rects[j], space, p.title_space)));
    if !crowded {
        return rects;
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let cell_w = ((work.w - p.gap * (cols - 1) as f64) / cols as f64).max(1.0);
    let cell_h = ((work.h - p.gap * (rows - 1) as f64) / rows as f64).max(1.0);
    // the window's part of the cell, above its title space
    let window_h = (cell_h - p.title_space).max(1.0);
    let s = windows.iter().fold(p.max_scale, |s, w| {
        s.min(cell_w / w.w.max(1.0)).min(window_h / w.h.max(1.0))
    });
    let mut out = vec![Rect::default(); n];
    for (k, i) in reading_order(&rects).into_iter().enumerate() {
        let (col, row) = (k % cols, k / cols);
        let cx = work.x + col as f64 * (cell_w + p.gap) + cell_w / 2.0;
        let cy = work.y + row as f64 * (cell_h + p.gap) + cell_h / 2.0;
        let (w, h) = (windows[i].w * s, windows[i].h * s);
        out[i] = Rect::new(cx - w / 2.0, cy - (h + p.title_space) / 2.0, w, h);
    }
    out
}

/// Move the arrangement as a whole so its bounding box, titles included, is centred across `work` and the lift above
/// its middle, kept inside `work`: the windows keep their places relative to each other, and the arrangement is
/// balanced in the area whatever the windows' desktop places were.
fn centred(rects: Vec<Rect>, work: Rect, p: &LayoutParams) -> Vec<Rect> {
    let lift = p.lift;
    let feet: Vec<Rect> = rects.iter().map(|r| footprint(r, p.title_space)).collect();
    let Some(used) = bounds(&feet) else {
        return rects;
    };
    let x = (work.cx() - used.w / 2.0).clamp(work.x, (work.right() - used.w).max(work.x));
    let y = (work.cy() - lift - used.h / 2.0).clamp(work.y, (work.bottom() - used.h).max(work.y));
    let (dx, dy) = (x - used.x, y - used.y);
    rects.into_iter().map(|r| r.translate(dx, dy)).collect()
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
