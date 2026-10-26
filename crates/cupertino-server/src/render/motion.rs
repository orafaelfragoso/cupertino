//! Reduce Motion in the server's drawing: what replaces each animation the server owns. The
//! reference has no global rule; each animation owner decides. Replacements here:

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_effects::{TransformKind, WindowKind};
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

/// The close transform of a window of `kind` given its normal one: a sheet closes at once under
/// Reduce Motion.
pub fn close_transform(
    kind: WindowKind,
    normal: Option<TransformKind>,
    options: DisplayOptions,
) -> Option<TransformKind> {
    if options.reduce_motion && kind == WindowKind::Sheet {
        return None;
    }
    normal
}

#[cfg(test)]
mod tests {
    use super::*;

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
