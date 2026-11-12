//! Reduce Motion in the server's drawing: what replaces each animation the server owns. The
//! reference has no global rule; each animation owner decides. Replacements here:

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_effects::{MinimizeKind, TransformKind, WindowKind};
use cupertino_wm::LifecycleState;

/// How the windows of an output follow its workspace position during a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceTransition {
    /// Each workspace slides by its distance from the position (`cupertino_effects::WorkspaceSlide`).
    Slide,
    /// The windows stay in place; a workspace is drawn at the opacity of its nearness to the position.
    CrossFade,
}

impl WorkspaceTransition {
    pub fn for_options(options: DisplayOptions) -> Self {
        if options.reduce_motion {
            WorkspaceTransition::CrossFade
        } else {
            WorkspaceTransition::Slide
        }
    }
}

/// Opacity of the workspace at `index` when the output's workspace position is `position` (in
/// index units) under [`WorkspaceTransition::CrossFade`]: 1 on it, 0 one workspace or more away.
pub fn cross_fade_opacity(index: usize, position: f64) -> f64 {
    (1.0 - (position - index as f64).abs()).clamp(0.0, 1.0)
}

/// Whether a window in `state` is drawn at its target frame instead of the animated one: the
/// full-screen frame zoom is dropped under Reduce Motion.
pub fn skips_frame_animation(state: LifecycleState, options: DisplayOptions) -> bool {
    options.reduce_motion
        && matches!(
            state,
            LifecycleState::EnterFullScreen | LifecycleState::ExitFullScreen
        )
}

/// Whether a window of `kind` in `state` is drawn at its resting frame while it keeps its opacity: under Reduce
/// Motion an opening sheet fades in where it rests instead of sliding down into place.
pub fn skips_sheet_slide(kind: WindowKind, state: LifecycleState, options: DisplayOptions) -> bool {
    options.reduce_motion && kind == WindowKind::Sheet && state == LifecycleState::Opening
}

/// Whether a window opening with the small scale-in is drawn at its full size instead: under Reduce Motion
/// it simply appears.
pub fn skips_open_scale(state: LifecycleState, options: DisplayOptions) -> bool {
    options.reduce_motion && state == LifecycleState::Opening
}

/// The close transform of a window of `kind` given its normal one: a sheet and an ordinary window close at once
/// under Reduce Motion.
pub fn close_transform(
    kind: WindowKind,
    normal: Option<TransformKind>,
    options: DisplayOptions,
) -> Option<TransformKind> {
    if options.reduce_motion
        && (kind == WindowKind::Sheet || normal == Some(TransformKind::WindowClose))
    {
        return None;
    }
    normal
}

/// The minimize effect drawn for the configured `effect`: under Reduce Motion the scale, the shortest and the one that
/// moves least, stands in for the warp and the funnel. No effect stays none.
pub fn minimize_kind(
    effect: Option<MinimizeKind>,
    options: DisplayOptions,
) -> Option<MinimizeKind> {
    effect.map(|kind| {
        if options.reduce_motion {
            MinimizeKind::Scale
        } else {
            kind
        }
    })
}

/// The opacity a minimizing or restoring window is drawn at, at `progress` (0 the window, 1 the dock slot): under
/// Reduce Motion the plain scale that stands in for the bend also fades, out towards the slot and back in on the way
/// out of it; otherwise the window stays opaque.
pub fn minimize_opacity(progress: f64, options: DisplayOptions) -> f64 {
    if options.reduce_motion {
        1.0 - progress.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_motion_minimizes_with_the_scale() {
        for kind in MinimizeKind::ALL {
            assert_eq!(
                minimize_kind(Some(kind), DisplayOptions::default()),
                Some(kind)
            );
            assert_eq!(
                minimize_kind(Some(kind), reduced()),
                Some(MinimizeKind::Scale)
            );
        }
        assert_eq!(minimize_kind(None, reduced()), None);
    }

    #[test]
    fn under_reduce_motion_the_scale_fades_as_it_goes_and_comes_back() {
        for progress in [0.0, 0.3, 1.0] {
            assert_eq!(minimize_opacity(progress, DisplayOptions::default()), 1.0);
        }
        assert_eq!(
            minimize_opacity(0.0, reduced()),
            1.0,
            "the window at its place"
        );
        assert!((minimize_opacity(0.25, reduced()) - 0.75).abs() < 1e-12);
        assert_eq!(minimize_opacity(1.0, reduced()), 0.0, "gone at the slot");
        assert_eq!(minimize_opacity(1.5, reduced()), 0.0);
    }

    /// The window model's progress is linear over the drawn effect's duration each way: the bend takes 0.45 s to the
    /// slot and 0.42 s back out of it; the scale that stands in for it under Reduce Motion takes a fifth of a second
    /// both ways.
    #[test]
    fn the_minimize_timing_is_the_drawn_effects_duration_both_ways() {
        use cupertino_animation::{CubicBezier, Curve};
        // the duration of a linear timed curve; `None` for any other
        let linear = |c: Curve| match c {
            Curve::Timed { duration, bezier } if bezier == CubicBezier::LINEAR => Some(duration),
            _ => None,
        };
        let timing = |effect: Option<MinimizeKind>, options: DisplayOptions| {
            let mut cfg = cupertino_wm::WmConfig::default();
            let effects = crate::fx::EffectsConfig {
                minimize: minimize_kind(effect, options),
                ..crate::fx::EffectsConfig::default()
            };
            crate::state::set_minimize_timing(&mut cfg, effects);
            (
                linear(cfg.animation.minimizing),
                linear(cfg.animation.restoring),
            )
        };
        assert_eq!(
            timing(Some(MinimizeKind::Warp), DisplayOptions::default()),
            (Some(0.45), Some(0.42))
        );
        assert_eq!(
            timing(Some(MinimizeKind::Warp), reduced()),
            (Some(0.2), Some(0.2))
        );
        assert_eq!(
            timing(Some(MinimizeKind::Funnel), DisplayOptions::default()),
            (Some(0.4), Some(0.4))
        );
        let defaults = cupertino_wm::WmConfig::default().animation;
        assert_eq!(
            timing(None, reduced()),
            (linear(defaults.minimizing), linear(defaults.restoring)),
            "no effect keeps the model's own curves"
        );
    }

    fn reduced() -> DisplayOptions {
        DisplayOptions {
            reduce_motion: true,
            ..DisplayOptions::default()
        }
    }

    #[test]
    fn reduce_motion_picks_the_cross_fade() {
        assert_eq!(
            WorkspaceTransition::for_options(DisplayOptions::default()),
            WorkspaceTransition::Slide
        );
        assert_eq!(
            WorkspaceTransition::for_options(reduced()),
            WorkspaceTransition::CrossFade
        );
    }

    #[test]
    fn the_two_workspaces_of_a_switch_share_the_opacity() {
        assert_eq!(
            (cross_fade_opacity(0, 0.0), cross_fade_opacity(1, 0.0)),
            (1.0, 0.0)
        );
        assert_eq!(
            (cross_fade_opacity(0, 0.25), cross_fade_opacity(1, 0.25)),
            (0.75, 0.25)
        );
        assert_eq!(
            (cross_fade_opacity(2, 3.0), cross_fade_opacity(3, 3.0)),
            (0.0, 1.0)
        );
        assert_eq!(
            cross_fade_opacity(1, 1.002),
            0.998,
            "the spring's overshoot fades the target a little"
        );
        assert_eq!(
            cross_fade_opacity(0, -0.1),
            0.9,
            "rubber band at the first workspace"
        );
    }

    #[test]
    fn only_the_full_screen_transitions_lose_their_frame_animation() {
        for state in [
            LifecycleState::EnterFullScreen,
            LifecycleState::ExitFullScreen,
        ] {
            assert!(skips_frame_animation(state, reduced()));
            assert!(!skips_frame_animation(state, DisplayOptions::default()));
        }
        for state in [
            LifecycleState::Zooming,
            LifecycleState::Minimizing,
            LifecycleState::Opening,
            LifecycleState::FullScreen,
        ] {
            assert!(!skips_frame_animation(state, reduced()), "{state:?}");
        }
    }

    #[test]
    fn under_reduce_motion_only_an_opening_sheet_loses_its_slide() {
        assert!(skips_sheet_slide(
            WindowKind::Sheet,
            LifecycleState::Opening,
            reduced()
        ));
        assert!(!skips_sheet_slide(
            WindowKind::Sheet,
            LifecycleState::Opening,
            DisplayOptions::default()
        ));
        assert!(!skips_sheet_slide(
            WindowKind::Sheet,
            LifecycleState::Normal,
            reduced()
        ));
        assert!(!skips_sheet_slide(
            WindowKind::Alert,
            LifecycleState::Opening,
            reduced()
        ));
    }

    #[test]
    fn a_sheet_closes_at_once_and_other_kinds_keep_their_transform() {
        assert_eq!(
            close_transform(WindowKind::Sheet, Some(TransformKind::SheetOut), reduced()),
            None
        );
        assert_eq!(
            close_transform(
                WindowKind::Sheet,
                Some(TransformKind::SheetOut),
                DisplayOptions::default()
            ),
            Some(TransformKind::SheetOut)
        );
        assert_eq!(
            close_transform(WindowKind::Utility, Some(TransformKind::FadeOut), reduced()),
            Some(TransformKind::FadeOut)
        );
    }
}
