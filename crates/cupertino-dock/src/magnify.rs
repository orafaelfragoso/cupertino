//! Magnification: ramp duration, pluggable profile and one-dimensional layout along the dock axis.
//!
//! *: the fisheye profile of [`FisheyeProfile`], the default: a half-sine
//!   displacement of tile *edges* over ±3 tile widths around the pointer. The tile under the
//!   pointer centre is exactly the large size; the dock grows by `2 · real_scale`.
//! * approximate: the raised-cosine [`CosineProfile`] (kept as an alternative) and the anchoring
//!   rule of the weight-based layout.

use crate::config::DockConfig;

/// Magnification ramp duration in ms (formula, unit). `current_fish` is the
/// current magnification amount in points (0 … large − tile).
pub fn fish_ramp_ms(cfg: &DockConfig, current_fish: f64, dragging: bool) -> f64 {
    if dragging {
        return cfg.move_speed_ms;
    }
    let d = (cfg.large_size - (cfg.tile_size + current_fish)).abs();
    ((d * 0.5 + 1.0).ln() * cfg.fish_speed + 1.0).trunc()
}

/// Ramp duration when the magnification shrinks back (approximate: the verified formula is only
/// known for the growing direction; the same expression is applied with the distance to the
/// resting size).
pub fn fish_ramp_out_ms(cfg: &DockConfig, current_fish: f64, dragging: bool) -> f64 {
    if dragging {
        return cfg.move_speed_ms;
    }
    ((current_fish.abs() * 0.5 + 1.0).ln() * cfg.fish_speed + 1.0).trunc()
}

/// Weight function of the magnification bump.
///
/// `x` is the signed distance between the tile centre and the cursor divided by the span
/// (`fish_span · tile_size`). Implementations return a weight in `[0, 1]`, 1 under the cursor and
/// 0 for `|x| ≥ 1`.
///
/// A profile that displaces tile edges instead of scaling tiles (like [`FisheyeProfile`])
/// overrides [`MagnificationProfile::layout`]; [`layout_items`] then uses it and `weight` is only
/// the equivalent per-tile weight for a tile of the nominal size.
pub trait MagnificationProfile {
    fn weight(&self, x: f64) -> f64;

    /// Complete layout (frames relative to the dock centre line) or `None` to fall back to the
    /// weight-based layout.
    fn layout(&self, _cfg: &DockConfig, _items: &[LayoutItem], _cursor: Option<f64>, _fish: f64) -> Option<Vec<TileFrame>> {
        None
    }
}

/// Fisheye size in tile widths (3.0).
pub const FISH_SIZE_TILES: f64 = 3.0;

/// Edge displacement of the fisheye:
/// `−s` for `d ≤ −F`, `+s` for `d ≥ F`, `s · sin(π·d/(2F))` between, with `s = real_scale` and
/// `F = fish_size`; `d` is the distance of the edge from the fish centre.
pub fn fisheye_edge_offset(d: f64, fish_size: f64, real_scale: f64) -> f64 {
    if d <= -fish_size {
        -real_scale
    } else if d >= fish_size {
        real_scale
    } else {
        real_scale * (d * std::f64::consts::PI / fish_size * 0.5).sin()
    }
}

/// `real_scale = ((large − tile)/2) / sin(π·tile/(4·fish_size)) · fish`; 0 when magnification is off or the large size does not exceed the tile size.
/// `fish` ∈ [0, 1] is the animated magnification amount.
pub fn fisheye_real_scale(cfg: &DockConfig, fish: f64) -> f64 {
    let t = cfg.tile_size;
    let fish_size = cfg.fish_span * t;
    if !cfg.magnification || cfg.large_size <= t || fish_size <= 0.0 {
        return 0.0;
    }
    (cfg.large_size - t) * 0.5 / (std::f64::consts::PI * t / (4.0 * fish_size)).sin() * fish.clamp(0.0, 1.0)
}

/// The half-sine fisheye. Default profile of the dock.
///
/// Both edges of every item at rest position `e` move to `e + offset(e − c)` where `c` is the
/// pointer position clamped to the dock. The layout is anchored so that the item under the
/// pointer stays under the pointer; with the pointer farther than the fisheye size from both
/// ends the dock grows by `real_scale` on each side.
#[derive(Clone, Copy, Debug, Default)]
pub struct FisheyeProfile;

impl FisheyeProfile {
    /// Width above the tile size of a nominal-size tile whose centre is `x` fisheye sizes from the
    /// pointer, as a fraction of `large − tile` (exact for tiles of the nominal size).
    fn width_weight(x: f64) -> f64 {
        let delta = 0.5 / FISH_SIZE_TILES;
        let sine = |y: f64| (std::f64::consts::FRAC_PI_2 * y.clamp(-1.0, 1.0)).sin();
        (sine(x + delta) - sine(x - delta)) / (2.0 * sine(delta))
    }
}

impl MagnificationProfile for FisheyeProfile {
    fn weight(&self, x: f64) -> f64 {
        Self::width_weight(x)
    }

    fn layout(&self, cfg: &DockConfig, items: &[LayoutItem], cursor: Option<f64>, fish: f64) -> Option<Vec<TileFrame>> {
        let c = cursor?;
        let s = fisheye_real_scale(cfg, fish);
        if s <= 0.0 || items.is_empty() {
            return None;
        }
        let fish_size = cfg.fish_span * cfg.tile_size;
        let total = base_length(cfg, items);
        let c = c.clamp(0.0, total);
        let gap = cfg.tile_size * cfg.spacing;
        let mut frames = Vec::with_capacity(items.len());
        let mut at = 0.0;
        for it in items {
            let (l, r) = (at, at + it.extent);
            let l2 = l + fisheye_edge_offset(l - c, fish_size, s);
            let r2 = r + fisheye_edge_offset(r - c, fish_size, s);
            frames.push(TileFrame { pos: -total / 2.0 + l2, size: (r2 - l2).max(0.0) });
            at = r + gap;
        }
        Some(frames)
    }
}

/// Raised-cosine bump (approximate alternative to [`FisheyeProfile`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct CosineProfile;

impl MagnificationProfile for CosineProfile {
    fn weight(&self, x: f64) -> f64 {
        profile(x)
    }
}

/// Profile f(x) ∈ [0,1], x = distance from cursor in tile units / span (approximate).
pub fn profile(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        0.0
    } else {
        0.5 * ((std::f64::consts::PI * x).cos() + 1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileFrame {
    /// position of the tile's leading edge along the dock axis, relative to the dock centre line
    pub pos: f64,
    pub size: f64,
}

/// One element of the flow: its unmagnified extent along the axis and whether it magnifies
/// (separators and placeholders do not).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutItem {
    pub extent: f64,
    pub magnify: bool,
}

/// Unmagnified length of the flow.
pub fn base_length(cfg: &DockConfig, items: &[LayoutItem]) -> f64 {
    let n = items.len();
    items.iter().map(|i| i.extent).sum::<f64>() + n.saturating_sub(1) as f64 * cfg.tile_size * cfg.spacing
}

/// Lay out a flow of items. `cursor` is the pointer position along the axis in the *unmagnified*
/// dock coordinate space (0 = leading edge) or `None`; `fish` ∈ [0,1] is the animated
/// magnification amount. Frames are relative to the dock centre line.
///
/// Anchoring (approximate): the point under the cursor keeps the same fractional position inside
/// its item and the same screen position, clamped so that the ends grow outwards.
pub fn layout_items(
    cfg: &DockConfig,
    items: &[LayoutItem],
    cursor: Option<f64>,
    fish: f64,
    profile: &dyn MagnificationProfile,
) -> Vec<TileFrame> {
    if let Some(frames) = profile.layout(cfg, items, cursor, fish) {
        return frames;
    }
    let n = items.len();
    let t = cfg.tile_size;
    let gap = t * cfg.spacing;
    let base_len = base_length(cfg, items);
    let amp = if cfg.magnification { (cfg.large_size - t).max(0.0) * fish.clamp(0.0, 1.0) } else { 0.0 };
    let mut pos = Vec::with_capacity(n);
    let mut p = 0.0;
    for it in items {
        pos.push(p);
        p += it.extent + gap;
    }
    let sizes: Vec<f64> = items
        .iter()
        .zip(&pos)
        .map(|(it, pos)| match cursor {
            Some(c) if amp > 0.0 && it.magnify => {
                let centre = pos + it.extent * 0.5;
                it.extent + amp * (it.extent / t) * profile.weight((centre - c) / (cfg.fish_span * t))
            }
            _ => it.extent,
        })
        .collect();
    let total: f64 = sizes.iter().sum::<f64>() + n.saturating_sub(1) as f64 * gap;
    let mut start = -total / 2.0;
    if let (Some(c), true) = (cursor, amp > 0.0 && n > 0) {
        let cc = c.clamp(0.0, base_len);
        let idx = (0..n).rev().find(|&i| pos[i] <= cc).unwrap_or(0);
        let frac = if items[idx].extent > 0.0 { ((cc - pos[idx]) / items[idx].extent).clamp(0.0, 1.0) } else { 0.0 };
        let before: f64 = sizes[..idx].iter().sum::<f64>() + idx as f64 * gap;
        let screen_c = cc - base_len / 2.0;
        let anchored = screen_c - before - frac * sizes[idx];
        let slack = total - base_len;
        start = anchored.clamp(-base_len / 2.0 - slack, -base_len / 2.0);
    }
    let mut out = Vec::with_capacity(n);
    let mut p = start;
    for s in sizes {
        out.push(TileFrame { pos: p, size: s });
        p += s + gap;
    }
    out
}

/// Lay out `n` identical magnifying tiles with the default (fisheye) profile.
pub fn layout(cfg: &DockConfig, n: usize, cursor: Option<f64>, fish: f64) -> Vec<TileFrame> {
    let items = vec![LayoutItem { extent: cfg.tile_size, magnify: true }; n];
    layout_items(cfg, &items, cursor, fish, &FisheyeProfile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> DockConfig {
        DockConfig { tile_size: 48.0, magnification: true, ..DockConfig::default() }
    }

    fn cfg64() -> DockConfig {
        DockConfig { magnification: true, ..DockConfig::default() }
    }

    #[test]
    fn ramp_duration_formula() {
        let c = cfg();
        assert_eq!(fish_ramp_ms(&c, 0.0, false), 156.0);
        assert_eq!(fish_ramp_ms(&c, 0.0, true), 270.0);
        assert_eq!(fish_ramp_ms(&c, 80.0, false), 1.0);
    }

    #[test]
    fn ramp_out_duration_grows_with_distance() {
        let c = cfg();
        assert_eq!(fish_ramp_out_ms(&c, 0.0, false), 1.0);
        assert_eq!(fish_ramp_out_ms(&c, 80.0, false), 156.0);
        assert_eq!(fish_ramp_out_ms(&c, 80.0, true), 270.0);
    }

    #[test]
    fn no_cursor_is_uniform_and_centred() {
        let f = layout(&cfg(), 5, None, 1.0);
        assert!(f.iter().all(|t| t.size == 48.0));
        assert!((f[0].pos + f[4].pos + 48.0).abs() < 1e-9);
    }

    #[test]
    fn peak_under_cursor_and_continuity() {
        let c = cfg();
        let n = 10;
        let f = layout(&c, n, Some(4.5 * 48.0), 1.0);
        assert!((f[4].size - c.large_size).abs() < 1.0);
        let mut prev = layout(&c, n, Some(0.0), 1.0);
        for k in 1..=480 {
            let cur = layout(&c, n, Some(k as f64), 1.0);
            for (a, b) in prev.iter().zip(&cur) {
                assert!((a.pos - b.pos).abs() < 6.0 && (a.size - b.size).abs() < 3.0);
            }
            prev = cur;
        }
    }

    #[test]
    fn cursor_stays_over_hovered_tile() {
        let c = cfg();
        let n = 10;
        for k in 0..480 {
            let cur = k as f64;
            let f = layout(&c, n, Some(cur), 1.0);
            let screen = cur - n as f64 * 48.0 / 2.0;
            let idx = (cur / 48.0) as usize;
            assert!(screen >= f[idx].pos - 1e-6 && screen <= f[idx].pos + f[idx].size + 1e-6, "k={k}");
        }
    }

    #[test]
    fn fish_zero_or_disabled_magnification_is_uniform() {
        let c = cfg();
        assert!(layout(&c, 6, Some(100.0), 0.0).iter().all(|t| t.size == 48.0));
        let off = DockConfig { magnification: false, ..cfg() };
        assert!(layout(&off, 6, Some(100.0), 1.0).iter().all(|t| t.size == 48.0));
    }

    #[test]
    fn spacing_and_mixed_items() {
        let c = DockConfig { spacing: 0.25, ..cfg() };
        let items = [
            LayoutItem { extent: 48.0, magnify: true },
            LayoutItem { extent: 12.0, magnify: false },
            LayoutItem { extent: 48.0, magnify: true },
        ];
        let f = layout_items(&c, &items, None, 0.0, &CosineProfile);
        assert_eq!(base_length(&c, &items), 48.0 + 12.0 + 48.0 + 24.0);
        assert!((f[1].pos - (f[0].pos + 48.0 + 12.0)).abs() < 1e-9);
        let m = layout_items(&c, &items, Some(24.0), 1.0, &CosineProfile);
        assert_eq!(m[1].size, 12.0, "separators never magnify");
        assert!(m[0].size > 100.0);
    }

    #[test]
    fn mixed_items_keep_cursor_over_hovered_item() {
        let c = cfg();
        let items: Vec<LayoutItem> = (0..8)
            .map(|i| LayoutItem { extent: if i == 3 { 12.0 } else { 48.0 }, magnify: i != 3 })
            .collect();
        let base = base_length(&c, &items);
        let mut starts = vec![0.0];
        for it in &items {
            starts.push(starts.last().unwrap() + it.extent);
        }
        for k in 0..(base as usize) {
            let cur = k as f64 + 0.5;
            let f = layout_items(&c, &items, Some(cur), 1.0, &CosineProfile);
            let idx = (0..items.len()).rev().find(|&i| starts[i] <= cur).unwrap();
            let screen = cur - base / 2.0;
            assert!(screen >= f[idx].pos - 1e-6 && screen <= f[idx].pos + f[idx].size + 1e-6, "k={k}");
        }
    }

    struct Linear;
    impl MagnificationProfile for Linear {
        fn weight(&self, x: f64) -> f64 {
            (1.0 - x.abs()).max(0.0)
        }
    }

    #[test]
    fn profile_is_pluggable() {
        let c = cfg();
        let items = vec![LayoutItem { extent: 48.0, magnify: true }; 7];
        let a = layout_items(&c, &items, Some(3.5 * 48.0), 1.0, &CosineProfile);
        let b = layout_items(&c, &items, Some(3.5 * 48.0), 1.0, &Linear);
        assert!((a[3].size - b[3].size).abs() < 1e-9, "both peak at the full size");
        assert!(a[2].size > b[2].size, "cosine bump is fatter than a linear one");
    }

    #[test]
    fn profile_properties() {
        assert_eq!(profile(0.0), 1.0);
        assert_eq!(profile(1.0), 0.0);
        assert!((profile(0.5) - 0.5).abs() < 1e-12);
        assert_eq!(profile(-2.0), 0.0);
    }

    fn centred_on_tile(cfg: &DockConfig, n: usize, k: usize) -> Vec<TileFrame> {
        let t = cfg.tile_size;
        layout(cfg, n, Some((k as f64 + 0.5) * t), 1.0)
    }

    #[test]
    fn fisheye_constants() {
        let c = cfg64();
        let s = fisheye_real_scale(&c, 1.0);
        assert!((s - 32.0 / (std::f64::consts::PI / 12.0).sin()).abs() < 1e-9);
        assert!((s - 123.64).abs() < 0.01);
        assert!((fisheye_real_scale(&c, 0.5) - s * 0.5).abs() < 1e-9);
        assert_eq!(fisheye_real_scale(&DockConfig::default(), 1.0), 0.0, "magnification is off by default");
        assert_eq!(fisheye_real_scale(&DockConfig { large_size: 64.0, ..cfg64() }, 1.0), 0.0);
    }

    #[test]
    fn fisheye_hovered_tile_is_exactly_the_large_size() {
        for (tile, large) in [(64.0, 128.0), (48.0, 128.0), (52.0, 200.0), (32.0, 96.0)] {
            let c = DockConfig { tile_size: tile, large_size: large, magnification: true, ..DockConfig::default() };
            for n in [1usize, 5, 11] {
                for k in 0..n {
                    let f = centred_on_tile(&c, n, k);
                    assert!((f[k].size - large).abs() < 1e-9, "tile {tile} large {large} n {n} k {k}: {}", f[k].size);
                }
            }
        }
    }

    #[test]
    fn fisheye_neighbour_sizes_match_recovered_values() {
        let c = cfg64();
        let f = centred_on_tile(&c, 15, 7);
        let r = |k: usize| f[k].size / 64.0;
        assert!((r(7) - 2.0).abs() < 1e-9);
        assert!((r(8) - 1.866).abs() < 0.001 && (r(6) - r(8)).abs() < 1e-9);
        assert!((r(9) - 1.5).abs() < 0.001);
        assert!((r(10) - 1.066).abs() < 0.002);
        assert!((r(11) - 1.0).abs() < 1e-9, "beyond three tile widths nothing grows");
        assert!((r(3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fisheye_growth_is_twice_the_real_scale() {
        let c = cfg64();
        let n = 21;
        let f = centred_on_tile(&c, n, 10);
        let len = |f: &[TileFrame]| f.last().unwrap().pos + f.last().unwrap().size - f[0].pos;
        let rest = layout(&c, n, None, 1.0);
        let grow = len(&f) - len(&rest);
        assert!((grow - 2.0 * fisheye_real_scale(&c, 1.0)).abs() < 1e-6, "{grow}");
        assert!((f[0].pos - (rest[0].pos - fisheye_real_scale(&c, 1.0))).abs() < 1e-9);
    }

    #[test]
    fn fisheye_sizes_fall_off_monotonically() {
        let c = cfg64();
        let f = centred_on_tile(&c, 21, 10);
        for k in 10..20 {
            assert!(f[k].size >= f[k + 1].size - 1e-9, "right side k={k}");
        }
        for k in (1..=10).rev() {
            assert!(f[k].size >= f[k - 1].size - 1e-9, "left side k={k}");
        }
    }

    #[test]
    fn fisheye_edge_displacement_is_clamped_and_odd() {
        let (fs, s) = (192.0, 100.0);
        assert_eq!(fisheye_edge_offset(-500.0, fs, s), -s);
        assert_eq!(fisheye_edge_offset(500.0, fs, s), s);
        assert_eq!(fisheye_edge_offset(0.0, fs, s), 0.0);
        assert!((fisheye_edge_offset(fs, fs, s) - s).abs() < 1e-12);
        for k in 0..100 {
            let d = k as f64 * 3.0;
            assert!((fisheye_edge_offset(d, fs, s) + fisheye_edge_offset(-d, fs, s)).abs() < 1e-9);
            assert!(fisheye_edge_offset(d, fs, s).abs() <= s + 1e-12);
        }
    }

    #[test]
    fn fisheye_keeps_the_pointer_over_its_tile_everywhere() {
        let c = cfg64();
        let n = 9;
        let len = n as f64 * 64.0;
        for k in 0..=(len as usize * 2) {
            let cur = k as f64 * 0.5;
            let f = layout(&c, n, Some(cur), 1.0);
            let screen = cur - len / 2.0;
            let idx = ((cur / 64.0) as usize).min(n - 1);
            assert!(screen >= f[idx].pos - 1e-9 && screen <= f[idx].pos + f[idx].size + 1e-9, "cursor {cur}");
        }
    }

    #[test]
    fn fisheye_frames_are_contiguous_and_continuous() {
        let c = cfg64();
        let n = 8;
        let mut prev = layout(&c, n, Some(0.0), 1.0);
        for k in 0..=(n * 64) {
            let cur = layout(&c, n, Some(k as f64), 1.0);
            for w in cur.windows(2) {
                assert!((w[0].pos + w[0].size - w[1].pos).abs() < 1e-9, "no gaps with zero spacing");
            }
            for (a, b) in prev.iter().zip(&cur) {
                assert!(a.pos.is_finite() && (a.pos - b.pos).abs() < 3.0 && (a.size - b.size).abs() < 3.0);
            }
            prev = cur;
        }
    }

    #[test]
    fn fisheye_weight_matches_the_layout_for_nominal_tiles() {
        let c = cfg64();
        let f = centred_on_tile(&c, 15, 7);
        for (k, frame) in f.iter().enumerate() {
            let x = (k as f64 - 7.0) * 64.0 / (c.fish_span * 64.0);
            let expected = 64.0 + 64.0 * FisheyeProfile.weight(x);
            assert!((frame.size - expected).abs() < 1e-9, "k={k}");
        }
        assert_eq!(FisheyeProfile.weight(0.0), 1.0);
        assert!(FisheyeProfile.weight(1.2).abs() < 1e-12);
    }

    #[test]
    fn fish_amount_scales_the_growth_linearly() {
        let c = cfg64();
        let f = layout(&c, 9, Some(4.5 * 64.0), 0.5);
        assert!((f[4].size - (64.0 + 32.0)).abs() < 1e-9);
        assert!(layout(&c, 9, Some(4.5 * 64.0), 0.0).iter().all(|t| t.size == 64.0));
    }

    #[test]
    fn default_config_does_not_magnify() {
        let f = layout(&DockConfig::default(), 5, Some(100.0), 1.0);
        assert!(f.iter().all(|t| t.size == 64.0));
    }
}
