//! The scroll edge: what a view that scrolls under raised nodes, such as a page under a toolbar row, draws over its
//! content where they cover it, so what passes under them recedes and their controls stay legible.
//!
//! The soft edge, shown only once the page is scrolled, veils the content with the window background, near-solid over
//! the top of the covered band and a little thinner towards its bottom ([`soft_edge_opacity`]), with a hairline along
//! the bottom, so content recedes as it passes under the row. The hard
//! edge fills the band with the window background and draws a separator along its bottom, so the row reads as a solid
//! bar: with Reduce Transparency and in high contrast. A band that holds text outside glass, a table's pinned header
//! under the row, is always the hard edge, its header drawing the line along its bottom. Where the window system has no raised layer nothing scrolls
//! under raised nodes (their scroll containers clip it away) and there is no edge.
//!
//! In a window that is not active the row recedes into a band of its own: the band is filled with the inactive title
//! bar colour, a little lighter (dark) or darker (light) than the window, with the hard edge's separator where the
//! edge is hard, so the content stops at the row as it does under a solid title bar.

use cupertino_geometry::{Point, Rect};
use cupertino_ui_core::{OverlayCx, PaintCx};
use cupertino_ui_render::{GradientStop, Paint};
use cupertino_ui_theme::{Color, ColorRole, StateVariant, ThemePreferences};

/// The soft edge's veil over the top of the band: this much of the window background shows over the content.
pub const SOFT_EDGE_MAX: f64 = 0.96;
/// The soft edge's veil at the bottom of the band, just above its hairline.
pub const SOFT_EDGE_MIN: f64 = 0.72;
/// How far down the band, as a share of its height, the veil keeps its full strength before it eases down.
pub const SOFT_EDGE_HOLD: f64 = 0.0;
/// Points at which the soft edge's gradient samples [`soft_edge_opacity`], from the band's top to its bottom.
pub const SOFT_EDGE_STOPS: usize = 9;
/// The soft edge's hairline along the bottom of the band, in points.
const HAIRLINE: f64 = 0.5;
/// The hairline's strength: black over a light window, white over a dark one.
const HAIRLINE_LIGHT: f32 = 0.11;
const HAIRLINE_DARK: f32 = 0.06;

/// How a scroll view's content is set off where raised nodes cover it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollEdge {
    /// A near-solid veil, a little thinner towards the band's bottom, with a hairline there, shown while the content
    /// is scrolled.
    Soft,
    /// The band filled, with a separator along its bottom.
    Hard,
}

/// The edge for the theme's preferences.
pub fn scroll_edge(preferences: &ThemePreferences) -> ScrollEdge {
    if preferences.reduce_transparency || preferences.appearance.is_high_contrast() {
        ScrollEdge::Hard
    } else {
        ScrollEdge::Soft
    }
}

/// The soft edge's veil, from [`SOFT_EDGE_MAX`] down to [`SOFT_EDGE_MIN`], `depth` points below the top of a covered
/// band `band` points tall: full over the band's top [`SOFT_EDGE_HOLD`], then thinning in a straight line to the
/// band's bottom, and nothing below it.
pub fn soft_edge_opacity(depth: f64, band: f64) -> f64 {
    if band.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || !depth.is_finite() {
        return 0.0;
    }
    if depth > band {
        return 0.0;
    }
    let t = depth / band;
    if t <= SOFT_EDGE_HOLD {
        return SOFT_EDGE_MAX;
    }
    let u = ((t - SOFT_EDGE_HOLD) / (1.0 - SOFT_EDGE_HOLD)).clamp(0.0, 1.0);
    SOFT_EDGE_MAX + (SOFT_EDGE_MIN - SOFT_EDGE_MAX) * u
}

/// The soft edge's gradient stops in `color`, from the band's top (offset 0) to its bottom (offset 1).
pub fn soft_edge_stops(color: Color) -> Vec<GradientStop> {
    (0..SOFT_EDGE_STOPS)
        .map(|i| {
            let t = i as f64 / (SOFT_EDGE_STOPS - 1) as f64;
            GradientStop {
                offset: t as f32,
                color: color.with_alpha(color.a * soft_edge_opacity(t, 1.0) as f32),
            }
        })
        .collect()
}

/// A scroll edge as a frame paints it: its kind, colours and the covered band's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EdgeLook {
    edge: ScrollEdge,
    background: Color,
    separator: Option<Color>,
    /// The soft edge's hairline.
    hairline: Option<Color>,
    /// Content is scrolled under the band: the soft edge shows only then.
    scrolled: bool,
    /// The band's fill in a window that is not active, which replaces the veil.
    inactive: Option<Color>,
    band: f64,
}

impl EdgeLook {
    /// Whether this is the soft edge, which shows only once the page is scrolled.
    pub(crate) fn is_soft(&self) -> bool {
        self.edge == ScrollEdge::Soft
    }

    /// The edge of a node `band` points of whose top raised nodes cover; none where they draw in place, with nothing
    /// scrolled under them. A `firm` band, which holds text outside glass such as a table's pinned header, is solid in
    /// every appearance, without a separator of its own: what it holds draws the line along its bottom.
    pub(crate) fn of(cx: &PaintCx<'_>, band: f64, firm: bool, scrolled: bool) -> Option<Self> {
        if !cx.raised_layer() {
            return None;
        }
        let theme = &cx.env().theme;
        let state = StateVariant {
            window_active: cx.window_active(),
            ..StateVariant::NORMAL
        };
        let edge = if firm {
            ScrollEdge::Hard
        } else {
            scroll_edge(&theme.preferences)
        };
        Some(Self {
            edge,
            background: theme
                .colors
                .get(ColorRole::WindowBackground, state)
                .unwrap_or(Color::new(0.0, 0.0, 0.0, 0.0)),
            inactive: (!firm && !cx.window_active())
                .then(|| {
                    theme
                        .colors
                        .get(ColorRole::InactiveTitlebar, StateVariant::NORMAL)
                })
                .flatten(),
            separator: match edge {
                ScrollEdge::Hard if !firm => theme.colors.get(ColorRole::Separator, state),
                _ => None,
            },
            hairline: (!firm && edge == ScrollEdge::Soft).then(|| {
                if theme.preferences.appearance.is_dark() {
                    Color::new(1.0, 1.0, 1.0, HAIRLINE_DARK)
                } else {
                    Color::new(0.0, 0.0, 0.0, HAIRLINE_LIGHT)
                }
            }),
            scrolled,
            band,
        })
    }

    pub(crate) fn paint(&self, cx: &mut OverlayCx<'_>) {
        let size = cx.size();
        let band = self.band.min(size.height);
        if band <= 0.0 {
            return;
        }
        let rect = Rect::new(0.0, 0.0, size.width, band);
        if let Some(fill) = self.inactive {
            cx.fill(rect, Paint::Solid(fill));
            if let Some(separator) = self.separator {
                let hair = 1.0 / cx.env().scale.max(1.0);
                cx.fill_pixels(Rect::new(0.0, band - hair, size.width, hair), separator);
            }
            return;
        }
        match self.edge {
            ScrollEdge::Soft => {
                // nothing lies under the band until the page is scrolled
                if !self.scrolled {
                    return;
                }
                // the toolbar row's own blurred band carries the veil when it is on show
                if !cx.band_shown() {
                    cx.fill(
                        rect,
                        Paint::Linear {
                            start: Point::new(0.0, 0.0),
                            end: Point::new(0.0, band),
                            stops: soft_edge_stops(self.background),
                        },
                    );
                }
                if let Some(hairline) = self.hairline {
                    cx.fill_pixels(
                        Rect::new(0.0, band - HAIRLINE, size.width, HAIRLINE),
                        hairline,
                    );
                }
            }
            ScrollEdge::Hard => {
                cx.fill(rect, Paint::Solid(self.background));
                if let Some(separator) = self.separator {
                    let hair = 1.0 / cx.env().scale.max(1.0);
                    cx.fill_pixels(Rect::new(0.0, band - hair, size.width, hair), separator);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_theme::Appearance;

    #[test]
    fn the_soft_veil_holds_over_the_top_then_eases_out_to_nothing_at_the_bottom() {
        let band = 52.0;
        assert_eq!(soft_edge_opacity(0.0, band), SOFT_EDGE_MAX);
        assert_eq!(
            soft_edge_opacity(band * SOFT_EDGE_HOLD, band),
            SOFT_EDGE_MAX
        );
        assert_eq!(soft_edge_opacity(band, band), 0.0);
        assert_eq!(
            soft_edge_opacity(band + 10.0, band),
            0.0,
            "nothing below the band"
        );
        assert_eq!(soft_edge_opacity(10.0, 0.0), 0.0, "no band, no veil");
        // never stronger further down, and smooth: no step larger than a smoothstep's steepest at this sampling
        let mut last = SOFT_EDGE_MAX;
        for i in 0..=520 {
            let o = soft_edge_opacity(f64::from(i) / 10.0, band);
            assert!(o <= last + 1e-12, "at {i}: {o} after {last}");
            assert!(last - o < 0.01, "at {i}: a step from {last} to {o}");
            last = o;
        }
        // half way down the easing it is half as strong
        let mid = band * (SOFT_EDGE_HOLD + 1.0) / 2.0;
        assert!((soft_edge_opacity(mid, band) - SOFT_EDGE_MAX / 2.0).abs() < 1e-9);
    }

    #[test]
    fn the_gradient_samples_the_veil_down_the_band() {
        let color = Color::new(0.1, 0.2, 0.3, 1.0);
        let stops = soft_edge_stops(color);
        assert_eq!(stops.len(), SOFT_EDGE_STOPS);
        assert_eq!(stops[0].offset, 0.0);
        assert_eq!(stops[SOFT_EDGE_STOPS - 1].offset, 1.0);
        for s in &stops {
            assert_eq!((s.color.r, s.color.g, s.color.b), (0.1, 0.2, 0.3));
            let want = soft_edge_opacity(f64::from(s.offset), 1.0) as f32;
            assert!((s.color.a - want).abs() < 1e-6);
        }
        assert_eq!(stops[SOFT_EDGE_STOPS - 1].color.a, 0.0);
    }

    #[test]
    fn reduce_transparency_and_high_contrast_give_a_solid_edge() {
        let prefs = |appearance, reduce_transparency| ThemePreferences {
            appearance,
            reduce_transparency,
            reduce_motion: false,
        };
        for appearance in [Appearance::Light, Appearance::Dark] {
            assert_eq!(scroll_edge(&prefs(appearance, false)), ScrollEdge::Soft);
            assert_eq!(scroll_edge(&prefs(appearance, true)), ScrollEdge::Hard);
        }
        for appearance in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
            assert_eq!(scroll_edge(&prefs(appearance, false)), ScrollEdge::Hard);
        }
    }
}
