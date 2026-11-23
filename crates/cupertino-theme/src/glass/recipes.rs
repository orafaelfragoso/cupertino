//! How a mode resolves to a recipe: the variant's base and stages, then the inactive branch, the window shadow and the
//! display options in the order the system lays them.

use super::accessibility::{flatten, legibility, reduced_motion, regular_reduced};
use super::clear::{clear_base, clear_reduced, control, dock};
use super::fitted::{control_center, search_results_reduced};
use super::regular::{contrast_face, inactive, regular_base, regular_variant, window_shadow};
use super::*;

impl GlassParams {
    /// The recipe of a shape in `mode` whose shorter side is `short_side` points: resolved at the size its modifiers
    /// name ([`GlassSize`]), else at that side.
    pub fn new(mode: GlassMode, short_side: f32) -> Self {
        let side = if short_side.is_finite() {
            short_side.max(0.0)
        } else {
            0.0
        };
        let s = mode.modifiers.size.side(side);
        let (dark, variant, m) = (mode.dark, mode.variant, mode.modifiers);
        let (ic, rt) = (mode.increase_contrast, mode.reduce_transparency);
        let Some(base) = variant.base() else {
            return Self::identity();
        };
        let mut p = match variant {
            GlassVariant::Clear | GlassVariant::AppIcons => {
                let mut p = clear_base(s);
                if rt {
                    clear_reduced(&mut p, dark);
                }
                p
            }
            GlassVariant::Dock => dock(dark, s, false, ic, rt),
            GlassVariant::AppSwitcher => dock(dark, s, true, ic, rt),
            GlassVariant::ControlCenter => control_center(mode, s),
            GlassVariant::Control | GlassVariant::Slider | GlassVariant::Loupe => {
                control(variant, dark, s)
            }
            _ => {
                let mut p = regular_base(dark, s);
                if ic {
                    contrast_face(&mut p.face, dark);
                }
                regular_variant(&mut p, variant, m, dark, ic);
                if !m.active && variant.has_inactive_branch() {
                    inactive(&mut p, variant, dark);
                }
                if matches!(variant, GlassVariant::GlassWindow | GlassVariant::Menu) {
                    window_shadow(&mut p, m, dark);
                }
                if rt {
                    regular_reduced(&mut p, dark, ic);
                    if variant == GlassVariant::SearchResults && dark && !ic {
                        search_results_reduced(&mut p);
                    }
                }
                p
            }
        };
        if rt && variant == GlassVariant::Clear {
            flatten(&mut p);
        }
        if mode.reduce_motion {
            reduced_motion(&mut p, variant);
        }
        if !rt && mode.density > 0 {
            legibility(&mut p, base, dark, legibility_amount(mode.density));
        }
        if m.exclude_shadow {
            p.shadow.opacity = 0.0;
        }
        p
    }

    /// No glass: the backdrop read at the output's scale through a face that leaves it as it is, with nothing at its
    /// edge, so it draws what lies behind it unchanged.
    pub fn identity() -> Self {
        let mut p = clear_base(0.0);
        p.blur = GlassBlur {
            radius: 0.0,
            level_opacity: [0.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        };
        p.face = GlassFace {
            saturation: 1.0,
            white: 1.0,
            black: 0.0,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        };
        p.highlight.amount = 0.0;
        p.shadow.amount = 0.0;
        p.shadow.height = 0.0;
        p.capture_scale = TRACK_CAPTURE_SCALE;
        p
    }

    /// The Gaussian deviation of the backdrop's blur, in points: the radius scaled by the first level's opacity, twice
    /// over, at most [`BLUR_SIGMA_MAX`].
    pub fn blur_sigma(&self) -> f32 {
        (BLUR_SIGMA_PER_RADIUS * self.blur.radius * self.blur.level_opacity[0].clamp(0.0, 1.0))
            .clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// Whether the blur fill shows.
    pub fn fill_shows(&self) -> bool {
        self.blur.fill_darken + self.blur.fill_normal + self.blur.fill_lighten > 0.0
    }

    /// The Gaussian deviation of the blur fill, in points, at most [`BLUR_SIGMA_MAX`].
    pub fn fill_sigma(&self) -> f32 {
        (FILL_SIGMA_PER_RADIUS * self.blur.fill_radius).clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// How far past the shape the backdrop is read, in points: three deviations of the widest blur it reads (the
    /// backdrop's, or the blur fill's when it shows), or the reach of a shown drop shadow.
    pub fn reach(&self) -> f32 {
        let fill = if self.fill_shows() {
            self.fill_sigma()
        } else {
            0.0
        };
        // a Gaussian is read three deviations out; the refraction reads the face further inside the shape, never past
        // its edge
        let read = 3.0 * self.blur_sigma().max(fill);
        let line = if self.shadow.edge_line > 0.0 {
            EDGE_LINE_REACH
        } else {
            0.0
        };
        read.max(self.shadow_reach()).max(line)
    }

    /// How far past the shape a shown drop shadow reaches, in points: its offset and half its smooth edge of three
    /// blur radii; 0 when it does not show.
    pub fn shadow_reach(&self) -> f32 {
        let s = &self.shadow;
        if s.opacity * s.amount * s.fill[3] <= 0.0 {
            return 0.0;
        }
        s.offset[0].abs().max(s.offset[1].abs()) + 1.5 * s.height
    }
}

/// How far past the shape the edge line ([`GlassShadow::edge_line`]) needs the glass drawn, in points: its pixel and the
/// half pixel its band fades over, at a scale of 1 or more.
const EDGE_LINE_REACH: f32 = 2.0;

/// The refraction's offset at the edge, as a share of its strength.
pub const REFRACTION_EDGE_SHARE: f32 = 0.9;
/// How fast the refraction's offset decays across its band: `e^(−REFRACTION_DECAY · t)`.
pub const REFRACTION_DECAY: f32 = 4.2;

/// The refraction's offset `t` of its band deep (0 at the edge, 1 at the band's inner side), as a share of its strength:
/// `REFRACTION_EDGE_SHARE · e^(−REFRACTION_DECAY · t) · (1 − t⁴)`, nothing from the band's inner side on. A pixel reads
/// the face `strength · refraction_profile(t)` points along the edge's outward normal: a negative strength reads it
/// further inside, so the edge magnifies what lies just inside it.
pub fn refraction_profile(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return if t < 0.0 { REFRACTION_EDGE_SHARE } else { 0.0 };
    }
    let t2 = t * t;
    REFRACTION_EDGE_SHARE * (-REFRACTION_DECAY * t).exp() * (1.0 - t2 * t2)
}
