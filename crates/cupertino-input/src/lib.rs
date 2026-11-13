//! Input model for the Cupertino compositor (no windowing-library dependency).
//!
//! * [`gesture`]: touchpad swipe / pinch / hold recognizer → workspace switch, overview, app
//!   overview, app grid and show-desktop gestures with progress and velocity.
//! * [`hotcorner`]: output-corner dwell triggers with modifier requirements.
//! * [`cursor`]: the accessibility cursor scale and the shake-to-locate detector.
//! * [`edge`]: screen-edge dwell for revealing the dock or a fullscreen menu bar.
//! * [`top_zone`]: a window dragged up into the top of the screen opens the overview.
//! * [`shortcuts`] and [`keysym`]: configurable key combinations → [`Action`]s.
//! * [`pointer`]: pointer acceleration curves (curve family and formula)
//!   and the fresh-account defaults.
//! * [`mouse_accel`]: mouse acceleration on physical speed (device resolution, event timestamps).
//! * [`table`]: table acceleration and the embedded default table.
//! * [`scroll_accel`]: per-axis scroll acceleration.
//! * [`scroll_count`]: the scroll count of repeated flicks.
//! * [`scroll`]: kinetic scrolling and rubber banding built on `cupertino-animation`.
//! * [`touchpad`]: touchpad behaviour (taps, tap-drag, clicks, scroll, swipes, pinch, thumb, palm, force click) from
//!   contact frames.
//! * [`velocity`]: sliding-window velocity estimation.
//! * [`geom`]: shared geometry, and the 5 pt drag-start threshold.
//!
//! the momentum and elastic constants used by [`scroll`] are as designed;
//! the dock reveal delay is as designed;
//! the hot corner action delay and the fullscreen menu bar reveal delay are unknown;
//! all gesture distances, thresholds and finger counts are approximate. Every such value is
//! configurable.

pub mod cursor;
pub mod edge;
pub mod geom;
pub mod gesture;
pub mod hotcorner;
pub mod keysym;
pub mod mouse_accel;
pub mod pointer;
pub use cupertino_animation::scroll;
pub mod scroll_accel;
pub mod scroll_count;
pub mod shortcuts;
pub mod table;
pub mod top_zone;
pub mod touchpad;
pub use cupertino_animation::velocity;

pub use cursor::{
    clamp_cursor_scale, ShakeDetector, CURSOR_SCALE_LIMIT, CURSOR_SCALE_MAX, CURSOR_SCALE_MIN,
    SHAKE_EVALUATION_INTERVAL_S, SHAKE_SCALE_MAX,
};
pub use edge::{Edge, EdgeConfig, EdgeEvent, EdgeReveal};
pub use geom::{drag_started, Modifiers, Point, Rect, DRAG_START_THRESHOLD};
pub use gesture::{
    fluid_swipe_commits, Direction, Disposition, GestureConfig, GestureEvent, GestureKind,
    GestureOutput, GestureRecognizer, Outcome,
};
pub use hotcorner::{
    Corner, CornerAction, CornerBinding, CornerEvent, HotCornerConfig, HotCorners,
};
pub use mouse_accel::{parse_mouse_dpi, resolve_dpi, MouseAccel, DEFAULT_MOUSE_DPI};
pub use pointer::{
    double_click_space, scroll_point_delta, wheel_points, AccelCurve, CurveParams, ParametricCurve,
    PointerAccel, DEFAULT_ACCELERATION, DEFAULT_DOUBLE_CLICK_TIME, DEFAULT_SCROLLING_SPEED,
    DEFAULT_TRACKING_SPEED, MOUSE_CURVES, POINTS_PER_LINE, TRACKING_SPEED_STOPS, TRACKPAD_CURVES,
    TRACKPAD_REPORT_RATE_HZ, TRACKPAD_SCROLL_CURVES, WHEEL_LINES_PER_NOTCH, WHEEL_LINE_POINTS,
};
pub use scroll::{KineticScroller, MomentumKind, ScrollConfig};
pub use scroll_accel::{
    ScrollAxisAccel, ScrollConsume, DEFAULT_SCROLL_REPORT_RATE_HZ, SCROLL_OUTPUT_SCALE,
    TRACKPAD_SCROLL_RESOLUTION,
};
pub use scroll_count::{ScrollCounter, ScrollPhase};
pub use shortcuts::{
    names_super, Action, ConfigReport, KeyCombo, ParseError, ScreenshotRequest, ShortcutMap,
};
pub use table::{AccelTable, TableCurve, TableEntry, TableError, DEFAULT_TABLE};
pub use top_zone::{TopZone, TopZoneConfig};
pub use touchpad::{
    ClickPressure, CornerClick, SwipeAxis, TouchContact, Touchpad, TouchpadButton, TouchpadEvent,
    TouchpadFrame, TouchpadGeometry, TouchpadSettings, MM_PER_COUNT, SWIPE_COMPLETE_MM,
};
pub use velocity::VelocityTracker;
