//! Overview configuration: the open/close spring (critically damped, response 0.25 s, 2.0 s in slow motion), the
//! shortened response after a fast trackpad release, the desktops bar's metrics ([`BarConfig`]) and the packing's
//! constants ([`LayoutParams`]).

use cupertino_animation::{Curve, SpringParams};

use crate::bar::BarConfig;
use crate::layout::LayoutParams;

/// Window titles fade in near the end of the open transition (approximate schedule: they start
/// to appear once progress exceeds `start` and are fully visible at `end`).
#[derive(Clone, Copy, Debug)]
pub struct TitleFade {
    pub start: f64,
    pub end: f64,
}

impl Default for TitleFade {
    fn default() -> Self {
        Self {
            start: 0.8,
            end: 1.0,
        }
    }
}

impl TitleFade {
    /// Opacity of the titles at transition progress `p` (smoothstep between `start` and `end`).
    pub fn opacity(&self, p: f64) -> f64 {
        if self.end <= self.start {
            return if p >= self.end { 1.0 } else { 0.0 };
        }
        let t = ((p - self.start) / (self.end - self.start)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Clone, Debug)]
pub struct HighlightConfig {
    /// The highlight follows the window frame grown by this many points: 0, the ring is drawn round the frame.
    pub inset: f64,
    pub fade_s: f64,
    pub move_curve: Curve,
}

impl Default for HighlightConfig {
    fn default() -> Self {
        Self {
            inset: 0.0,
            fade_s: 0.12,
            move_curve: Curve::Spring(SpringParams::from_perceptual(0.2, 0.0)),
        }
    }
}

/// Single-app mode: the app's windows above, its minimized windows in a lower row.
#[derive(Clone, Debug)]
pub struct AppOverviewConfig {
    /// Height of the minimized row as a fraction of the area height. approximate.
    pub row_height_fraction: f64,
    pub row_gap: f64,
    /// Largest scale of a minimized thumbnail relative to the window size. approximate.
    pub row_max_scale: f64,
    /// Space between the window area and the minimized row.
    pub row_margin: f64,
}

impl Default for AppOverviewConfig {
    fn default() -> Self {
        Self {
            row_height_fraction: 0.2,
            row_gap: 16.0,
            row_max_scale: 0.5,
            row_margin: 24.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OverviewConfig {
    pub layout: LayoutParams,
    pub app_overview: AppOverviewConfig,
    pub title_fade: TitleFade,
    pub highlight: HighlightConfig,
    pub bar: BarConfig,
    /// Distance of a full-progress swipe in trackpad units.
    pub swipe_distance: f64,
    /// Open/close transition duration. 0.25 s.
    pub animation_duration: f64,
    /// Duration while the slow-motion modifier is held. 2.0 s.
    pub slow_motion_duration: f64,
    /// Open/close spring: critically damped, response `animation_duration`.
    pub spring: SpringParams,
    /// A trackpad release faster than this (progress per second) settles on a shorter spring (5).
    pub fast_release_speed: f64,
    /// The response of that shorter spring as a fraction of the open/close one (0.6).
    pub fast_release_scale: f64,
    /// Progress damping beyond [0, 1].
    pub rubber_band: f64,
    /// Velocity projection window used to decide on release.
    pub projection_s: f64,
    /// Spring used to move windows to a new layout (window closed, mode switched).
    pub reflow: Curve,
}

impl OverviewConfig {
    /// Open/close spring for the slow-motion duration (bounce 0).
    pub fn slow_motion_spring(&self) -> SpringParams {
        SpringParams::from_perceptual(self.slow_motion_duration, 0.0)
    }
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            layout: LayoutParams::default(),
            app_overview: AppOverviewConfig::default(),
            title_fade: TitleFade::default(),
            highlight: HighlightConfig::default(),
            bar: BarConfig::default(),
            swipe_distance: 300.0,
            animation_duration: 0.25,
            slow_motion_duration: 2.0,
            spring: SpringParams::from_ratio(0.25, 1.0),
            fast_release_speed: 5.0,
            fast_release_scale: 0.6,
            rubber_band: 0.15,
            projection_s: 0.15,
            reflow: Curve::Spring(SpringParams::from_perceptual(0.25, 0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_durations() {
        let c = OverviewConfig::default();
        assert_eq!((c.animation_duration, c.slow_motion_duration), (0.25, 2.0));
        assert!((c.spring.perceptual_duration() - 0.25).abs() < 1e-12);
        assert!((c.slow_motion_spring().perceptual_duration() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_bar_band_is_72pt_closed_and_165pt_open() {
        let bar = BarConfig::default();
        assert_eq!(
            (bar.band_height(false), bar.band_height(true)),
            (72.0, 165.0)
        );
    }

    #[test]
    fn the_spring_is_critically_damped() {
        let c = OverviewConfig::default();
        assert!(c.spring.bounce().abs() < 1e-9);
        assert_eq!((c.fast_release_speed, c.fast_release_scale), (5.0, 0.6));
    }
}
