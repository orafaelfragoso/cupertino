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
