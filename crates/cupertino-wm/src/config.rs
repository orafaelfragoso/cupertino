//! Tunable parameters. Every value that is not is approximate or unknown and is
//! therefore configurable. Sources:,
//! the workspace.

use cupertino_animation::{CubicBezier, Curve, SpringParams};
use cupertino_effects::{Driver, TilingSprings};

use crate::tiling::TileKind;

/// Perceptual-duration spring curve (approximate values use it).
pub fn spring(duration: f64, bounce: f64) -> Curve {
    Curve::Spring(SpringParams::from_perceptual(duration, bounce))
}

/// The spring of a tiling driver as an animation curve; the tiling drivers are springs.
fn spring_curve(driver: Driver) -> Curve {
    let Driver::Spring(params) = driver else {
        panic!("tiling drivers are springs")
    };
    Curve::Spring(params)
}

/// Response of the preview fade spring, seconds. `SpringParams::from_ratio`
/// is the design constructor's form: stiffness `(2π/response)²`, damping `2·√stiffness·ratio`.
const PREVIEW_FADE_RESPONSE: f64 = 0.2268;

/// Curves of the lifecycle transitions.
///
/// Not configurable here because they are fixed functions of data: the per-kind
/// order-in / order-out transforms (`cupertino_effects::WindowKind`) and the zoom
/// duration `min(0.35, 181·ln(maxΔ/100 + 1)/750)` with the `sin²` ease.
#[derive(Clone, Debug)]
pub struct AnimationConfig {
    /// approximate: the warp-minimize speed key exists but its value is unknown;
    /// 0.5 s ease-in-ease-out is a stand-in.
    pub minimizing: Curve,
    pub restoring: Curve,
    /// 0.5 s with the bézier (0.25, 0, 0, 1).
    pub enter_fullscreen: Curve,
    /// the exit controller reads the same duration; the same curve is assumed.
    pub exit_fullscreen: Curve,
    /// critically damped spring, response 0.38 s.
    pub tiling: Curve,
    /// untiling is one of the three tiling-animation paths and takes the same
    /// spring.
    pub untile: Curve,
    /// Play tiling and untiling without animation. default `false`.
    pub disable_tiling: bool,
}

/// Full-screen transition duration (0.5 s).
pub const FULLSCREEN_DURATION: f64 = 0.5;
/// Full-screen frame curve ((0.25, 0, 0, 1)).
pub const FULLSCREEN_CURVE: CubicBezier = CubicBezier::new(0.25, 0.0, 0.0, 1.0);

impl Default for AnimationConfig {
    fn default() -> Self {
        let tiling = spring_curve(TilingSprings::default().frame);
        let fullscreen = Curve::Timed {
            duration: FULLSCREEN_DURATION,
            bezier: FULLSCREEN_CURVE,
        };
        Self {
            minimizing: Curve::Timed {
                duration: 0.5,
                bezier: CubicBezier::EASE_IN_EASE_OUT,
            },
            restoring: Curve::Timed {
                duration: 0.5,
                bezier: CubicBezier::EASE_IN_EASE_OUT,
            },
            enter_fullscreen: fullscreen,
            exit_fullscreen: fullscreen,
            tiling,
            untile: tiling,
            disable_tiling: false,
        }
    }
}

/// Magnetic window edges while moving and resizing (`crate::magnet`). Defaults the
/// window server's snapping preferences unless labelled otherwise.
#[derive(Clone, Debug)]
pub struct MagnetConfig {
    /// default on.
    pub enabled: bool,
    /// An edge snaps only if it was closer than this before the crossing step. 30 pt.
    pub snap_distance: f64,
    /// Fastest edge speed that still snaps. 400 pt/s.
    pub snap_velocity: f64,
    /// Edge speed above which a snapped edge lets go. 400 pt/s.
    pub unsnap_velocity: f64,
    /// Pull needed to unsnap when dragging along the edge normal. 35 pt.
    pub unsnap_distance_min: f64,
    /// Pull needed to unsnap when dragging at or beyond `unsnap_max_angle` from the normal.
    /// 140 pt.
    pub unsnap_distance_max: f64,
    /// 60°.
    pub unsnap_max_angle: f64,
    /// Travel from an unsnapped guide before the axis may snap again in the same direction.
    /// Value 55 pt; the guard semantics are derived from the resnap check.
    pub resnap_distance: f64,
    /// Release at once when pulling away within `instant_unsnap_angle` of the normal. on,
    /// 15°.
    pub instant_unsnap: bool,
    pub instant_unsnap_angle: f64,
    /// Space kept between facing edges of two windows. 1 pt.
    pub window_gap: f64,
    /// Guides must overlap the window along the other axis, or come within this margin.
    /// 1 pt.
    pub overlap_margin: f64,
    /// Windows below this opacity are not guides. 0.2.
    pub guide_min_opacity: f64,
    /// A visible piece of a partly covered window narrower or shorter than this (or than the
    /// window) guides only next to another piece. 20 pt.
    pub min_piece: f64,
    /// The usable screen rectangle is a guide when both sides exceed this. 4 pt.
    pub screen_guide_min: f64,
    /// When a snap or unsnap moves the window away from the pointer, the window catches up over
    /// this time. 0.15 s with the ease-out bézier (0, 0, 0.58, 1).
    pub catch_up_duration: f64,
    pub catch_up_curve: CubicBezier,
    /// Holding Alt disables snapping. derived from the flag.
    pub disable_with_alt: bool,
}

impl Default for MagnetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            snap_distance: 30.0,
            snap_velocity: 400.0,
            unsnap_velocity: 400.0,
            unsnap_distance_min: 35.0,
            unsnap_distance_max: 140.0,
            unsnap_max_angle: 60.0,
            resnap_distance: 55.0,
            instant_unsnap: true,
            instant_unsnap_angle: 15.0,
            window_gap: 1.0,
            overlap_margin: 1.0,
            guide_min_opacity: 0.2,
            min_piece: 20.0,
            screen_guide_min: 4.0,
            catch_up_duration: 0.15,
            catch_up_curve: CubicBezier::EASE_OUT,
            disable_with_alt: true,
        }
    }
}

/// Interactive move/resize/snap parameters.
#[derive(Clone, Debug)]
pub struct SnapConfig {
    /// Tiling by dragging to the left/right edge. Existence of the setting:.
    pub edge_drag_tiling: bool,
    /// Tiling by dragging to the top edge. Existence, default unknown.
    pub top_edge_tiling: bool,
    /// Immediate preview while the option accelerator modifier is held. Existence.
    pub option_accelerator: bool,
    /// Gap kept around and between tiled windows; 0 disables margins. default 8 pt with
    /// margins enabled.
    pub tiled_margin: f64,
    /// Ignore hot zones that overlap the dock hot zone. default `false`.
    pub avoid_dock_hot_zones: bool,
    /// Pointer distance from an output edge that counts as "at the edge" (spec: ≤ 1 px).
    pub edge_threshold: f64,
    /// Seconds the pointer must stay in the soft hot zone, or in a corner zone without Alt, before
    /// the preview appears. 0.2 s.
    pub edge_dwell: f64,
    /// Width of the soft hot zone along the pointer's edge distance, and side of the corner zones
    /// that select a quarter, with or without Alt. 20 pt.
    pub soft_hot_zone_size: f64,
    /// Seconds the pointer must stay in the hard (edge) hot zone. 0 s.
    pub hot_zone_delay: f64,
    /// Draw a border around tiled windows. default `false`.
    pub show_tiled_border: bool,
    /// Order the tiling preview above the target window. default `true`.
    pub order_preview_above_target: bool,
    /// Distance from the left, right and top edges that selects a half or the top tile while the
    /// option accelerator is held. approximate (the side zones were not re-read; the Alt corner
    /// zones are `soft_hot_zone_size`).
    pub option_edge_extent: f64,
    /// Tile applied by the top edge (spec: "full (or fill)"). approximate.
    pub top_edge_action: TileKind,
    /// Switch workspace after dwelling at the left/right output edge while moving a window.
    pub workspace_edge_switch: bool,
    /// Dwell before the edge workspace switch. The setting's existence is as designed,
    /// the value unknown.
    pub workspace_edge_dwell: f64,
    /// Keep the window title bar inside the usable area while moving. approximate.
    pub keep_titlebar_visible: bool,
    /// Title bar height used by `keep_titlebar_visible`. approximate.
    pub titlebar_height: f64,
    /// Preview rectangle spring: the tiling component's `frame` spring (role).
    pub preview_frame_curve: Curve,
    /// Preview fade spring: damping ratio 1.0, response 0.2268 s, the fade spring the hot-zone
    /// handler builds for the preview (fade-in versus fade-out direction ).
    pub preview_opacity_curve: Curve,
    /// The preview frame is the tile rectangle outset by this on every side. 15 pt
    /// (`ceil(4.0 × 2.8 + 3.0)); that the outset holds the preview's shadow is.
    pub preview_outset: f64,
    /// While a hot-zone dwell runs the pointer may move this far from where it entered the zone,
    /// inclusive; moving farther starts the dwell again from there. 5 pt and the per-zone
    /// metric ([`crate::tiling::is_still`]); the entry point as reference and the restart
    /// are.
    pub dwell_stillness: f64,
    /// Preview opacity (spec: 0.5, approximate).
    pub preview_opacity: f64,
    /// While resizing, a dragged edge that rests near the usable area's edge expands the window to
    /// the whole extent of that axis. default off.
    pub snap_size_to_screen_edges: bool,
    /// Seconds the edge must rest first. 0.4 s.
    pub snap_size_delay: f64,
    /// How close to the usable edge counts as resting on it. Value 50 pt, its use as a
    /// per-axis distance.
    pub snap_size_delta: f64,
    /// A double click on a resize edge or corner moves those edges to the usable area's edges.
    /// default on.
    pub snap_size_on_double_click: bool,
}

impl Default for SnapConfig {
    fn default() -> Self {
        let springs = TilingSprings::default();
        Self {
            edge_drag_tiling: true,
            top_edge_tiling: true,
            option_accelerator: true,
            tiled_margin: 8.0,
            avoid_dock_hot_zones: false,
            edge_threshold: 1.0,
            edge_dwell: 0.2,
            soft_hot_zone_size: 20.0,
            hot_zone_delay: 0.0,
            show_tiled_border: false,
            order_preview_above_target: true,
            option_edge_extent: 96.0,
            top_edge_action: TileKind::Fill,
            workspace_edge_switch: true,
            workspace_edge_dwell: 0.75,
            keep_titlebar_visible: true,
            titlebar_height: 28.0,
            preview_frame_curve: spring_curve(springs.frame),
            preview_opacity_curve: Curve::Spring(SpringParams::from_ratio(
                PREVIEW_FADE_RESPONSE,
                1.0,
            )),
            preview_outset: 15.0,
            dwell_stillness: 5.0,
            preview_opacity: 0.5,
            snap_size_to_screen_edges: false,
            snap_size_delay: 0.4,
            snap_size_delta: 50.0,
            snap_size_on_double_click: true,
        }
    }
}

/// Workspace parameters.
#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    /// Reorder workspaces by recent use. Existence, default unknown.
    pub mru_reorder: bool,
    /// Activating an app switches to a workspace holding its windows. Existence,
    /// default approximate.
    pub switch_on_activate: bool,
    /// Overscroll damping beyond the first/last workspace. approximate.
    pub rubber_band: f64,
    /// Velocity projection window used to pick the settle target on release. approximate.
    pub projection_s: f64,
    /// Largest number of workspaces a single swipe may traverse. approximate.
    pub max_swipe_jump: usize,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            mru_reorder: false,
            switch_on_activate: true,
            rubber_band: 0.55,
            projection_s: 0.15,
            max_swipe_jump: 1,
        }
    }
}

/// Focus and activation policy.
#[derive(Clone, Debug)]
pub struct FocusConfig {
    /// Activating an app raises all of its windows rather than just the key window.
    /// as an option of the design behaviour; default off.
    pub bring_all_to_front: bool,
    /// Activating an app that only has minimized windows restores the most recent one.
    /// approximate.
    pub restore_minimized_on_activate: bool,
    /// Modal children are placed under the parent title bar and move with it (sheet behaviour,
    /// ). approximate.
    pub place_modal_children_as_sheets: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            bring_all_to_front: false,
            restore_minimized_on_activate: true,
            place_modal_children_as_sheets: true,
        }
    }
}

/// Zoom (best-fit) parameters.
#[derive(Clone, Debug)]
pub struct ZoomConfig {
    /// Margin kept free around the zoomed window inside the usable area. approximate.
    pub margin: f64,
    /// Fraction of the usable area used when the client gave no size hint and no maximum.
    /// 1.0 equals maximize (nothing better is knowable without a content hint). approximate.
    pub default_fill: f64,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            margin: 0.0,
            default_fill: 1.0,
        }
    }
}

/// Show-desktop (desktop peek) motion.: a critically damped spring with a 0.3 s
/// response, where the response is the period of the undamped oscillation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopPeekConfig {
    pub damping_ratio: f64,
    pub response: f64,
}

impl Default for DesktopPeekConfig {
    fn default() -> Self {
        Self {
            damping_ratio: 1.0,
            response: 0.3,
        }
    }
}

impl DesktopPeekConfig {
    pub fn spring_params(&self) -> SpringParams {
        SpringParams::from_ratio(self.response, self.damping_ratio)
    }

    pub fn curve(&self) -> Curve {
        Curve::Spring(self.spring_params())
    }
}

#[derive(Clone, Debug, Default)]
pub struct WmConfig {
    pub animation: AnimationConfig,
    pub snap: SnapConfig,
    pub workspaces: WorkspaceConfig,
    pub focus: FocusConfig,
    pub zoom: ZoomConfig,
    pub desktop_peek: DesktopPeekConfig,
    pub magnet: MagnetConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_peek_is_critically_damped_with_300ms_response() {
        let c = DesktopPeekConfig::default();
        assert_eq!((c.damping_ratio, c.response), (1.0, 0.3));
        let p = c.spring_params();
        assert!((p.perceptual_duration() - 0.3).abs() < 1e-12);
        let ratio = p.damping / (2.0 * (p.stiffness * p.mass).sqrt());
        assert!((ratio - 1.0).abs() < 1e-12);
        assert!(matches!(
            WmConfig::default().desktop_peek.curve(),
            Curve::Spring(_)
        ));
    }

    #[test]
    fn tiling_animations_enabled_by_default() {
        assert!(!AnimationConfig::default().disable_tiling);
    }

    #[test]
    fn tiling_spring_is_critically_damped_with_380ms_response() {
        let Curve::Spring(p) = AnimationConfig::default().tiling else {
            panic!("spring expected")
        };
        let w0 = std::f64::consts::TAU / 0.38;
        assert_eq!((p.mass, p.stiffness, p.damping), (1.0, w0 * w0, 2.0 * w0));
    }

    #[test]
    fn fullscreen_is_half_a_second_on_the_verified_curve() {
        let Curve::Timed { duration, bezier } = AnimationConfig::default().enter_fullscreen else {
            panic!("timed expected")
        };
        assert_eq!(
            (duration, bezier),
            (0.5, CubicBezier::new(0.25, 0.0, 0.0, 1.0))
        );
    }
}
