//! Touchpad behaviour model: taps, tap-drag, physical clicks, two-finger scrolling, three- and
//! four-finger swipes, four-finger pinch, resting thumb and palm handling, and force click.
//!
//! Input is a stream of [`TouchpadFrame`]s (contacts in millimetres from the top-left corner of
//! the sensor, y growing downwards); output is [`TouchpadEvent`]s. Pointer motion leaves in
//! trackpad counts (`mm / 0.0635`) so the compositor can feed it to
//! [`PointerAccel`](crate::pointer::PointerAccel) unchanged; [`TouchpadSettings::pointer_accel`]
//! builds that accelerator. The model has no timers of its own: the compositor calls
//! [`Touchpad::tick`] at [`Touchpad::next_deadline`].
//!
//! timings, corner zones and
//! thumb geometry, motion sensitivities and swipe distances, force
//! ladder, axis dominance and swipe sets, commit rule ([`crate::gesture::fluid_swipe_commits`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::gesture::fluid_swipe_commits;
use crate::pointer::{
    AccelCurve, ParametricCurve, PointerAccel, DEFAULT_ACCELERATION, DEFAULT_RESOLUTION_DPI,
    DEFAULT_SCROLLING_SPEED, DEFAULT_TRACKING_SPEED, TRACKPAD_CURVES, TRACKPAD_REPORT_RATE_HZ,
    TRACKPAD_SCROLL_CURVES,
};
use crate::scroll_accel::{
    ScrollAxisAccel, DEFAULT_SCROLL_REPORT_RATE_HZ, TRACKPAD_SCROLL_RESOLUTION,
};
use crate::velocity::VelocityTracker;

/// Millimetres per trackpad count: `25.4 / 400 dpi` (the fallback
/// resolution of the pointer filter).
pub const MM_PER_COUNT: f64 = 25.4 / 400.0;

/// Longest touch that still counts as a tap, in seconds.
const TAP_MAX_DURATION_S: f64 = 0.25;
/// Largest spread between the touch-down times of the fingers of one tap: half the tap duration.
const TAP_MAX_LANDING_SPREAD_S: f64 = 0.5 * TAP_MAX_DURATION_S;
/// Fastest-finger speed above which a touch is not a tap, mm/s.
const TAP_MAX_HAND_SPEED_MM_S: f64 = 80.0;
/// Frames of excess motion that cancel a one-finger tap, and a tap of two or more fingers.
const TAP_EXCESS_FRAMES_ONE: u32 = 10;
const TAP_EXCESS_FRAMES_MULTI: u32 = 5;
/// A lone tap is held back this long, waiting for a second touch.
const TAP_HOLD_S: f64 = 0.20;
/// Window for the second tap of a tap-and-a-half drag, from the first lift.
const SECOND_TAP_WINDOW_S: f64 = 0.33;
/// Largest drift between the two taps, mm, for the built-in trackpad family.
const SECOND_TAP_DRIFT_MM: f64 = 10.0;
/// A lift shorter than this never ends a drag.
const DRAG_BRIDGE_S: f64 = 0.09;
/// A one-finger drag survives a lift this long after the later of the last touch-down and the last
/// motion.
const DRAG_GRACE_S: f64 = 0.5;

/// Fluid swipe start, complete distance and inertia for three and four fingers, mm, mm, s.
const SWIPE_START_MM: f64 = 2.0;
pub const SWIPE_COMPLETE_MM: f64 = 65.0;
/// Four-finger pinch and spread (the polar swipe): start, complete distance.
const PINCH_START_MM: f64 = 3.0;
const PINCH_COMPLETE_MM: f64 = 32.0;
/// Inertia time shared by the swipe and the pinch.
const FLUID_INERTIA_S: f64 = 0.5;

/// Axis dominance: the minor axis is zeroed when
/// `|v_major| · w > K · |v_minor| · w`. `K` is 2 for a resting trackpad hand (a cone of 26.6
/// degrees); the value 1 the design uses when a chord flag is set has an unknown trigger and is
/// not modelled. The weight is 0.45 on both axes (for a resting trackpad hand).
const AXIS_DOMINANCE_K: f64 = 2.0;
const AXIS_DOMINANCE_WEIGHT: f64 = 0.45;
/// Time constant of the velocity smoothing used by the axis dominance and the per-finger speed.
/// approximate: tau not known, the design reads it from a field that was not traced.
const SMOOTHING_TAU_S: f64 = 0.05;
/// Centroid travel before two fingers start a scroll, mm. approximate: the design's lock
/// thresholds (0.2 mm minimum, 3.0 mm lock override) have no scroll-start role.
const SCROLL_START_MM: f64 = 1.0;
/// Change of finger distance, or arc travelled by rotation (angle times half the distance), before
/// two fingers start a pinch or rotate, mm. approximate: the two-finger zoom sensitivity (400 dpi,
/// commit multiplier 0.066) records no start threshold, and its multiplier has no
/// role here.
const ZOOM_START_MM: f64 = 2.0;

/// Resting thumb: a contact that stays within this travel from touch-down for a tap's duration is
/// a resting thumb candidate, and a thumb is released again after this travel (the "activation"
/// and "release" distances 2.0 / 8.0 of the design, with an role,
/// ).
const THUMB_ACTIVATION_MM: f64 = 2.0;
const THUMB_RELEASE_MM: f64 = 8.0;
/// Thumb zone: `12 mm × ratio`, ratio `H / 76` (`sqrt(H / 76) below 76 mm).
const THUMB_ZONE_BASE_MM: f64 = 12.0;
const THUMB_ZONE_REFERENCE_HEIGHT_MM: f64 = 76.0;
/// Corner click zone: within this horizontal and vertical distance of a bottom corner and within
/// the radius.
const CORNER_WIDTH_MM: f64 = 31.5;
const CORNER_HEIGHT_MM: f64 = 22.8;
const CORNER_RADIUS_MM: f64 = 31.5;

/// Force click ladder in the firmware's raw units: activation and release of stage 2. The release is scaled by the same slider multiplier as the activation.
const FORCE_ACTIVATION: f64 = 300.0;
const FORCE_RELEASE: f64 = 255.0;

/// A speed-dependent gain for one kind of motion: `(speed / threshold)^power` above the
/// threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sensitivity {
    accel_thresh: f64,
    accel_power: f64,
}

/// Pointer movement: power 0, so counts are linear in finger travel.
const TRACKING: Sensitivity = Sensitivity {
    accel_thresh: 12.0,
    accel_power: 0.0,
};
/// Pointer movement while a thumb rests (use).
const RESTING_TRACKING: Sensitivity = Sensitivity {
    accel_thresh: 12.0,
    accel_power: 0.06,
};
/// Scrolling. Its commit multiplier 0.2 has no role and is not used.
const SCROLLING: Sensitivity = Sensitivity {
    accel_thresh: 12.0,
    accel_power: 0.0,
};

/// Gain applied to a motion delta at finger `speed` mm/s: the delta is multiplied by
/// `(speed / thresh)^power` only when both parameters are positive and the speed is above the
/// threshold.
fn axis_gain(speed: f64, s: Sensitivity) -> f64 {
    if s.accel_thresh > 0.0 && s.accel_power > 0.0 && speed > s.accel_thresh {
        (speed / s.accel_thresh).powf(s.accel_power)
    } else {
        1.0
    }
}

/// Size and kind of the sensor. Coordinates are millimetres from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchpadGeometry {
    pub width_mm: f64,
    pub height_mm: f64,
    /// The whole surface clicks (no separate buttons): enables the corner and two-finger click
    /// rules.
    pub clickpad: bool,
}

impl TouchpadGeometry {
    /// Height of the bottom band where a thumb rests.
    fn thumb_zone_height(&self) -> f64 {
        let h = self.height_mm;
        let ratio = if h < THUMB_ZONE_REFERENCE_HEIGHT_MM {
            (h / THUMB_ZONE_REFERENCE_HEIGHT_MM).sqrt()
        } else {
            h / THUMB_ZONE_REFERENCE_HEIGHT_MM
        };
        THUMB_ZONE_BASE_MM * ratio
    }

    /// Which bottom corner zone `pos` lies in, if any.
    fn corner_at(&self, pos: (f64, f64)) -> Option<CornerClick> {
        let dy = self.height_mm - pos.1;
        let in_zone = |dx: f64| {
            dx <= CORNER_WIDTH_MM && dy <= CORNER_HEIGHT_MM && dx.hypot(dy) <= CORNER_RADIUS_MM
        };
        if in_zone(pos.0) {
            Some(CornerClick::BottomLeft)
        } else if in_zone(self.width_mm - pos.0) {
            Some(CornerClick::BottomRight)
        } else {
            None
        }
    }
}

/// One finger on the sensor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchContact {
    pub id: u32,
    pub position_mm: (f64, f64),
    /// Pressure normalised to 0…1 against the device's reported range; `None` when the device
    /// reports none (force click is then disabled). See [`TouchpadSettings::pressure_full_scale_raw`].
    pub pressure: Option<f64>,
    /// The device flagged the contact as a palm (`MT_TOOL_PALM`); it is ignored for its lifetime.
    pub palm: bool,
}

/// All contacts at one instant, plus the physical button state of a clickpad.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchpadFrame {
    pub time_s: f64,
    pub contacts: Vec<TouchContact>,
    pub button_down: bool,
}

/// Which bottom corner turns a physical click into a secondary click. The preference value 1 is
/// the left corner and 2 the right one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CornerClick {
    Off,
    BottomLeft,
    BottomRight,
}

/// The click-pressure slider: scales the force ladder by 0.8, 1.0 or 1.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickPressure {
    Light,
    Medium,
    Firm,
}

impl ClickPressure {
    fn multiplier(self) -> f64 {
        match self {
            Self::Light => 0.8,
            Self::Medium => 1.0,
            Self::Firm => 1.2,
        }
    }
}

/// User-facing touchpad settings. [`Default`] is the fresh-account state.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchpadSettings {
    /// Tap to click (key `Clicking`, fresh value off).
    pub tap_to_click: bool,
    /// Tap-and-a-half drag (key `Dragging`, fresh value off). Needs `tap_to_click` here
    /// (approximate: the design reads the two keys independently).
    pub tap_drag: bool,
    /// Keep a drag until a tap ends it (key `DragLock`, fresh value off); only with `tap_drag`.
    pub drag_lock: bool,
    /// A physical click with two fingers down is a secondary click (key `TrackpadRightClick`,
    /// fresh value on).
    pub secondary_click: bool,
    /// Corner zone secondary click (key `TrackpadCornerSecondaryClick`, fresh value off).
    pub corner_click: CornerClick,
    /// Content follows the fingers (default on, /AGENTS settled values).
    pub natural_scrolling: bool,
    /// Two-finger scrolling (key `TrackpadScroll`, fresh value on).
    pub two_finger_scroll: bool,
    /// Report a release velocity so kinetic scrolling can continue (key `TrackpadMomentumScroll`,
    /// fresh value on).
    pub momentum_scroll: bool,
    /// Three- and four-finger swipes (fresh value 2: shell gestures).
    pub three_finger_swipe: bool,
    pub four_finger_swipe: bool,
    /// Four-finger pinch and spread (fresh value 2).
    pub four_finger_pinch: bool,
    /// Two-finger pinch (scale) and rotate (keys `TrackpadPinch` and `TrackpadRotate`, fresh
    /// values on). Both off leaves two fingers to scrolling.
    pub pinch: bool,
    pub rotate: bool,
    /// Force click (`ForceSuppressed` off). Only effective for contacts that report
    /// pressure.
    pub force_click: bool,
    pub click_pressure: ClickPressure,
    /// Raw firmware force reading that a normalised pressure of 1.0 stands for.
    ///
    /// approximate: the ladder thresholds are in the firmware's own unit and no host
    /// conversion exists, so the device range cannot be mapped
    /// exactly. 500 is above the largest threshold (360) so every stage is reachable; tune it per
    /// device.
    pub pressure_full_scale_raw: f64,
    /// Tracking speed, in the units of [`DEFAULT_TRACKING_SPEED`] (default).
    pub tracking_speed: f64,
    /// Pointer acceleration; off scales linearly by `tracking_speed` (default).
    pub acceleration: bool,
    /// Scroll acceleration setting (`HIDTrackpadScrollAcceleration`; fresh value 0.3125,
    /// ). The design has no control for it; negative turns scroll acceleration
    /// off.
    pub scrolling_speed: f64,
}

impl Default for TouchpadSettings {
    fn default() -> Self {
        Self {
            tap_to_click: false,
            tap_drag: false,
            drag_lock: false,
            secondary_click: true,
            corner_click: CornerClick::Off,
            natural_scrolling: true,
            two_finger_scroll: true,
            momentum_scroll: true,
            three_finger_swipe: true,
            four_finger_swipe: true,
            four_finger_pinch: true,
            pinch: true,
            rotate: true,
            force_click: true,
            click_pressure: ClickPressure::Medium,
            pressure_full_scale_raw: 500.0,
            tracking_speed: DEFAULT_TRACKING_SPEED,
            acceleration: DEFAULT_ACCELERATION,
            scrolling_speed: DEFAULT_SCROLLING_SPEED,
        }
    }
}

impl TouchpadSettings {
    /// The accelerator for [`TouchpadEvent::PointerMotion`]: the trackpad curves at the nominal
    /// trackpad report rate, or linear scaling when acceleration is
    /// off.
    pub fn pointer_accel(&self) -> PointerAccel {
        if self.acceleration {
            PointerAccel::new(
                &TRACKPAD_CURVES,
                self.tracking_speed,
                DEFAULT_RESOLUTION_DPI,
                TRACKPAD_REPORT_RATE_HZ,
            )
        } else {
            PointerAccel::linear(self.tracking_speed)
        }
    }

    /// The accelerator for one axis of [`TouchpadEvent::ScrollUpdate`] (whose counts are the
    /// scroll counts of the design's trackpads): the trackpad scroll curves at the trackpad
    /// scroll resolution and the fallback report rate.
    /// `None` when `scrolling_speed` is negative: deltas then pass through. Build one per axis.
    pub fn scroll_accel(&self) -> Option<ScrollAxisAccel> {
        let curve = ParametricCurve::new(&TRACKPAD_SCROLL_CURVES, self.scrolling_speed)?;
        Some(ScrollAxisAccel::new(
            AccelCurve::Parametric(curve),
            TRACKPAD_SCROLL_RESOLUTION,
            DEFAULT_SCROLL_REPORT_RATE_HZ,
        ))
    }

    fn swipe_enabled(&self, fingers: usize) -> bool {
        match fingers {
            3 => self.three_finger_swipe,
            4 => self.four_finger_swipe,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchpadButton {
    Left,
    Right,
}

/// Axis a swipe locked to (`LockOnFirstUntilPause`: the first decision is kept).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TouchpadEvent {
    /// Relative motion in trackpad counts, before pointer acceleration.
    PointerMotion {
        dx: f64,
        dy: f64,
    },
    Button {
        button: TouchpadButton,
        pressed: bool,
    },
    ScrollBegin,
    /// Content displacement in trackpad counts: with natural scrolling the content follows the
    /// fingers (fingers moving down give a positive `dy`).
    ScrollUpdate {
        dx: f64,
        dy: f64,
    },
    /// Release velocity in counts per second, zero when momentum scrolling is off; feed it to
    /// [`KineticScroller::end_drag`](crate::scroll::KineticScroller::end_drag).
    ScrollEnd {
        vx: f64,
        vy: f64,
    },
    SwipeBegin {
        fingers: u32,
        axis: SwipeAxis,
    },
    /// `progress` is the finger travel along `axis` over the complete distance (65 mm); positive
    /// is rightwards or downwards, unaffected by natural scrolling.
    SwipeUpdate {
        fingers: u32,
        progress: f64,
    },
    /// `committed` is the release decision of the commit rule.
    SwipeEnd {
        fingers: u32,
        committed: bool,
        progress: f64,
    },
    PinchBegin {
        fingers: u32,
    },
    /// `scale` is the current mean finger radius (distance for two fingers) over the value at the
    /// start. `rotation_deg` is the two-finger rotation since the start, positive clockwise on
    /// screen, 0 for four fingers. `progress` is the four-finger radius change over the complete
    /// distance (32 mm), positive when spreading, 0 for two fingers.
    PinchUpdate {
        fingers: u32,
        scale: f64,
        rotation_deg: f64,
        progress: f64,
    },
    /// The two-finger pinch has no commit rule: `committed` is always true for it.
    PinchEnd {
        fingers: u32,
        committed: bool,
        scale: f64,
    },
    /// The force ladder's second stage was reached (`active`) or released.
    ForceClick {
        active: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Thumb {
    /// Not classified yet.
    Unknown,
    Resting,
    /// Moved too far to be a resting thumb; an ordinary finger from now on.
    Released,
}

#[derive(Clone, Debug)]
struct Track {
    down_time: f64,
    down_pos: (f64, f64),
    pos: (f64, f64),
    max_travel: f64,
    /// Smoothed speed in mm/s.
    speed: f64,
    thumb: Thumb,
}

impl Track {
    fn counts(&self) -> bool {
        self.thumb != Thumb::Resting
    }
}

/// A touch from the first counted finger down to the last up.
#[derive(Clone, Debug)]
struct Tap {
    first_down: f64,
    last_down: f64,
    fingers: usize,
    excess_frames: u32,
    /// A scroll, swipe, pinch, click or palm made this touch something other than a tap.
    invalid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TapDrag {
    Idle,
    /// A lone tap lifted at `lifted_at`; its click is held back until `TAP_HOLD_S` passes or a
    /// second touch decides.
    Waiting {
        lifted_at: f64,
        pos: (f64, f64),
        click_sent: bool,
    },
    /// The button is down and a finger is on the pad. `fresh` is the touch that started the drag.
    Held {
        fresh: bool,
    },
    /// The finger lifted with the button still down; `release_at` is `None` under drag lock.
    Lifted {
        release_at: Option<f64>,
    },
}

/// A swipe or pinch being integrated: progress against its complete distance, the peak reached
/// and the filtered progress rate for the commit rule.
#[derive(Clone, Debug)]
struct Fluid {
    fingers: u32,
    complete_mm: f64,
    progress: f64,
    peak: f64,
    rate: VelocityTracker,
}

impl Fluid {
    fn new(fingers: usize, complete_mm: f64) -> Self {
        Self {
            fingers: fingers as u32,
            complete_mm,
            progress: 0.0,
            peak: 0.0,
            rate: VelocityTracker::default(),
        }
    }

    fn update(&mut self, t: f64, progress: f64) {
        self.progress = progress;
        // The peak is the largest magnitude in the current direction.
        if progress * self.peak <= 0.0 || progress.abs() > self.peak.abs() {
            self.peak = progress;
        }
        self.rate.push(t, progress);
    }

    fn commits(&self, t: f64) -> bool {
        fluid_swipe_commits(
            self.progress,
            self.peak,
            self.rate.velocity(t),
            FLUID_INERTIA_S,
        )
    }
}

#[derive(Clone, Debug)]
enum Mode {
    Idle,
    Pointer,
    ScrollPending,
    Scroll,
    SwipePending,
    Swipe(SwipeAxis, Fluid),
    Pinch(Fluid, f64),
    /// Two-finger pinch and rotate: scale, accumulated rotation (radians) and the last pair angle.
    Zoom {
        scale: f64,
        rotation: f64,
        last_angle: f64,
    },
}

/// Scroll integration state of the current two-finger touch.
#[derive(Clone, Debug)]
struct Scroll {
    pending_mm: (f64, f64),
    smoothed_mm_s: Option<(f64, f64)>,
    position: (f64, f64),
    vx: VelocityTracker,
    vy: VelocityTracker,
}

impl Scroll {
    fn new() -> Self {
        Self {
            pending_mm: (0.0, 0.0),
            smoothed_mm_s: None,
            position: (0.0, 0.0),
            vx: VelocityTracker::default(),
            vy: VelocityTracker::default(),
        }
    }
}

/// Geometry of the counted fingers when the current finger count began.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    centroid: (f64, f64),
    radius: f64,
    /// Distance and angle (radians, y down) between the first two fingers.
    spread: f64,
    angle: f64,
}

/// The touchpad model for one device.
#[derive(Clone, Debug)]
pub struct Touchpad {
    geometry: TouchpadGeometry,
    pub settings: TouchpadSettings,
    last_time: Option<f64>,
    tracks: BTreeMap<u32, Track>,
    /// Counted fingers (palms and resting thumbs excluded) in the last frame.
    fingers: usize,
    centroid: (f64, f64),
    anchor: Anchor,
    hand_speed: f64,
    /// Two or more fingers were down since the last time none were: the remaining finger does
    /// not move the pointer.
    multi: bool,
    mode: Mode,
    scroll: Scroll,
    tap: Option<Tap>,
    drag: TapDrag,
    /// Later of the last touch-down and the last pointer motion.
    drag_ref: f64,
    button: Option<TouchpadButton>,
    force_active: bool,
}

impl Touchpad {
    pub fn new(geometry: TouchpadGeometry, settings: TouchpadSettings) -> Self {
        Self {
            geometry,
            settings,
            last_time: None,
            tracks: BTreeMap::new(),
            fingers: 0,
            centroid: (0.0, 0.0),
            anchor: Anchor {
                centroid: (0.0, 0.0),
                radius: 0.0,
                spread: 0.0,
                angle: 0.0,
            },
            hand_speed: 0.0,
            multi: false,
            mode: Mode::Idle,
            scroll: Scroll::new(),
            tap: None,
            drag: TapDrag::Idle,
            drag_ref: 0.0,
            button: None,
            force_active: false,
        }
    }

    /// Advance the model by one frame. Panics when `frame.time_s` goes backwards.
    pub fn process_frame(&mut self, frame: &TouchpadFrame) -> Vec<TouchpadEvent> {
        let t = frame.time_s;
        assert!(
            self.last_time.is_none_or(|last| t >= last),
            "frame time went backwards"
        );
        let dt = self.last_time.map_or(0.0, |last| t - last);
        self.last_time = Some(t);
        let mut out = Vec::new();
        self.run_timers(t, &mut out);
        self.update_tracks(frame, t, dt);
        self.update_button(frame.button_down, &mut out);
        self.update_force(frame, &mut out);
        self.update_mode(t, dt, &mut out);
        out
    }

    /// Fire the timers that are due at `time_s` (a held-back tap click, a drag release).
    pub fn tick(&mut self, time_s: f64) -> Vec<TouchpadEvent> {
        let mut out = Vec::new();
        self.run_timers(time_s, &mut out);
        out
    }

    /// The next time [`tick`](Self::tick) has something to do, in the clock of the frames.
    pub fn next_deadline(&self) -> Option<f64> {
        match self.drag {
            TapDrag::Waiting {
                lifted_at,
                click_sent: false,
                ..
            } => Some(lifted_at + TAP_HOLD_S),
            TapDrag::Waiting { lifted_at, .. } => Some(lifted_at + SECOND_TAP_WINDOW_S),
            TapDrag::Lifted { release_at } => release_at,
            TapDrag::Idle | TapDrag::Held { .. } => None,
        }
    }

    fn run_timers(&mut self, t: f64, out: &mut Vec<TouchpadEvent>) {
        if let TapDrag::Waiting {
            lifted_at,
            pos,
            click_sent: false,
        } = self.drag
        {
            if t >= lifted_at + TAP_HOLD_S {
                click(TouchpadButton::Left, out);
                self.drag = TapDrag::Waiting {
                    lifted_at,
                    pos,
                    click_sent: true,
                };
            }
        }
        match self.drag {
            TapDrag::Waiting { lifted_at, .. } if t - lifted_at > SECOND_TAP_WINDOW_S => {
                self.drag = TapDrag::Idle
            }
            TapDrag::Lifted {
                release_at: Some(at),
            } if t >= at => {
                out.push(TouchpadEvent::Button {
                    button: TouchpadButton::Left,
                    pressed: false,
                });
                self.drag = TapDrag::Idle;
            }
            _ => {}
        }
    }

    /// Merge the frame's contacts into the tracks and classify resting thumbs.
    fn update_tracks(&mut self, frame: &TouchpadFrame, t: f64, dt: f64) {
        let alpha = 1.0 - (-dt / SMOOTHING_TAU_S).exp();
        let mut live = BTreeSet::new();
        for c in &frame.contacts {
            if c.palm {
                // A finger the device reclassifies as a palm vanishes and spoils the tap.
                if self.tracks.remove(&c.id).is_some() {
                    if let Some(tap) = &mut self.tap {
                        tap.invalid = true;
                    }
                }
                continue;
            }
            live.insert(c.id);
            match self.tracks.get_mut(&c.id) {
                Some(track) => {
                    let step = (c.position_mm.0 - track.pos.0, c.position_mm.1 - track.pos.1);
                    let inst = if dt > 0.0 {
                        step.0.hypot(step.1) / dt
                    } else {
                        0.0
                    };
                    track.speed += alpha * (inst - track.speed);
                    track.pos = c.position_mm;
                    track.max_travel = track.max_travel.max(
                        (track.pos.0 - track.down_pos.0).hypot(track.pos.1 - track.down_pos.1),
                    );
                }
                None => {
                    self.tracks.insert(
                        c.id,
                        Track {
                            down_time: t,
                            down_pos: c.position_mm,
                            pos: c.position_mm,
                            max_travel: 0.0,
                            speed: 0.0,
                            thumb: Thumb::Unknown,
                        },
                    );
                }
            }
        }
        self.tracks.retain(|id, _| live.contains(id));

        let zone_top = self.geometry.height_mm - self.geometry.thumb_zone_height();
        for track in self.tracks.values_mut() {
            track.thumb = match track.thumb {
                Thumb::Unknown if track.max_travel > THUMB_ACTIVATION_MM => Thumb::Released,
                // Resting: in the thumb zone since touch-down, held still for longer than a tap.
                Thumb::Unknown
                    if t - track.down_time >= TAP_MAX_DURATION_S
                        && track.down_pos.1 >= zone_top =>
                {
                    Thumb::Resting
                }
                Thumb::Resting if track.max_travel > THUMB_RELEASE_MM => Thumb::Released,
                other => other,
            };
        }
    }

    fn counted(&self) -> impl Iterator<Item = &Track> {
        self.tracks.values().filter(|tr| tr.counts())
    }

    /// Physical click: pick the button on press, repeat it on release.
    fn update_button(&mut self, down: bool, out: &mut Vec<TouchpadEvent>) {
        match (down, self.button) {
            (true, None) => {
                let button = self.click_button();
                self.button = Some(button);
                if let Some(tap) = &mut self.tap {
                    tap.invalid = true;
                }
                out.push(TouchpadEvent::Button {
                    button,
                    pressed: true,
                });
            }
            (false, Some(button)) => {
                self.button = None;
                out.push(TouchpadEvent::Button {
                    button,
                    pressed: false,
                });
            }
            _ => {}
        }
    }

    /// Secondary click when a finger is in the configured corner zone, or two fingers are down.
    /// A separate-button pad always reports its own left button.
    fn click_button(&self) -> TouchpadButton {
        if !self.geometry.clickpad {
            return TouchpadButton::Left;
        }
        let corner = self.settings.corner_click;
        let in_corner = corner != CornerClick::Off
            && self
                .counted()
                .any(|tr| self.geometry.corner_at(tr.pos) == Some(corner));
        let two_fingers = self.settings.secondary_click && self.counted().count() == 2;
        if in_corner || two_fingers {
            TouchpadButton::Right
        } else {
            TouchpadButton::Left
        }
    }

    /// Stage 2 of the force ladder, scaled by the pressure slider.
    fn update_force(&mut self, frame: &TouchpadFrame, out: &mut Vec<TouchpadEvent>) {
        let strongest = frame
            .contacts
            .iter()
            .filter(|c| !c.palm)
            .filter_map(|c| c.pressure)
            .reduce(f64::max);
        let raw = strongest
            .filter(|_| self.settings.force_click)
            .map(|p| p * self.settings.pressure_full_scale_raw);
        let m = self.settings.click_pressure.multiplier();
        let active = match raw {
            Some(raw) if self.force_active => raw >= FORCE_RELEASE * m,
            Some(raw) => raw >= FORCE_ACTIVATION * m,
            None => false,
        };
        if active != self.force_active {
            self.force_active = active;
            out.push(TouchpadEvent::ForceClick { active });
        }
    }

    fn update_mode(&mut self, t: f64, dt: f64, out: &mut Vec<TouchpadEvent>) {
        let positions: Vec<(f64, f64)> = self.counted().map(|tr| tr.pos).collect();
        let n = positions.len();
        let centroid = centroid_of(&positions);
        let radius = radius_of(&positions, centroid);
        if n > 0 {
            self.hand_speed = self.counted().map(|tr| tr.speed).fold(0.0, f64::max);
        }

        if n != self.fingers {
            self.end_mode(t, out);
            let previous = std::mem::replace(&mut self.fingers, n);
            let (spread, angle) = pair_geometry(&positions);
            self.anchor = Anchor {
                centroid,
                radius,
                spread,
                angle,
            };
            self.change_fingers(previous, n, centroid, t, out);
            self.centroid = centroid;
            return;
        }
        if n == 0 {
            return;
        }
        let delta = (centroid.0 - self.centroid.0, centroid.1 - self.centroid.1);
        self.centroid = centroid;
        self.latch_excess_motion();

        match self.mode {
            Mode::Pointer => self.pointer_motion(delta, t, dt, out),
            Mode::ScrollPending => self.two_finger_pending(delta, &positions, t, dt, out),
            Mode::Zoom { .. } => self.zoom_motion(&positions, out),
            Mode::Scroll => self.scroll_motion(delta, t, dt, out),
            Mode::SwipePending => self.start_fluid(centroid, radius, t, out),
            Mode::Swipe(..) | Mode::Pinch(..) => self.fluid_motion(centroid, radius, t, out),
            Mode::Idle => {}
        }
    }

    /// Two fingers have not decided yet: a pinch or rotate starts when the distance or angle
    /// changes more than the fingers travel together, otherwise it is a scroll.
    fn two_finger_pending(
        &mut self,
        delta: (f64, f64),
        positions: &[(f64, f64)],
        t: f64,
        dt: f64,
        out: &mut Vec<TouchpadEvent>,
    ) {
        self.scroll.pending_mm.0 += delta.0;
        self.scroll.pending_mm.1 += delta.1;
        let (px, py) = self.scroll.pending_mm;
        let travel = px.hypot(py);
        let (spread, angle) = pair_geometry(positions);
        let stretch = if self.settings.pinch {
            (spread - self.anchor.spread).abs()
        } else {
            0.0
        };
        let arc = if self.settings.rotate {
            wrap_angle(angle - self.anchor.angle).abs() * self.anchor.spread / 2.0
        } else {
            0.0
        };
        let zoom = stretch.max(arc);
        if zoom >= ZOOM_START_MM && zoom > travel && self.anchor.spread > 0.0 {
            self.invalidate_tap();
            self.mode = Mode::Zoom {
                scale: 1.0,
                rotation: 0.0,
                last_angle: self.anchor.angle,
            };
            out.push(TouchpadEvent::PinchBegin { fingers: 2 });
            self.zoom_motion(positions, out);
        } else if self.settings.two_finger_scroll && travel >= SCROLL_START_MM && travel >= zoom {
            self.invalidate_tap();
            self.mode = Mode::Scroll;
            self.scroll.pending_mm = (0.0, 0.0);
            out.push(TouchpadEvent::ScrollBegin);
            self.scroll_motion((px, py), t, dt, out);
        }
    }

    /// Scale against the distance at the start, and rotation accumulated frame by frame so it
    /// can pass 180 degrees.
    fn zoom_motion(&mut self, positions: &[(f64, f64)], out: &mut Vec<TouchpadEvent>) {
        let (spread, angle) = pair_geometry(positions);
        let (pinch, rotate, start) = (
            self.settings.pinch,
            self.settings.rotate,
            self.anchor.spread,
        );
        let Mode::Zoom {
            scale,
            rotation,
            last_angle,
        } = &mut self.mode
        else {
            return;
        };
        if pinch {
            *scale = spread / start;
        }
        if rotate {
            *rotation += wrap_angle(angle - *last_angle);
        }
        *last_angle = angle;
        out.push(TouchpadEvent::PinchUpdate {
            fingers: 2,
            scale: *scale,
            rotation_deg: rotation.to_degrees(),
            progress: 0.0,
        });
    }

    /// The counted finger count changed from `previous` to `n` (`self.centroid` still holds the
    /// previous frame's): start a touch, end one, or pick the mode of the new count.
    fn change_fingers(
        &mut self,
        previous: usize,
        n: usize,
        centroid: (f64, f64),
        t: f64,
        out: &mut Vec<TouchpadEvent>,
    ) {
        if n == 0 {
            self.multi = false;
            self.mode = Mode::Idle;
            self.finish_touch(t, out);
            return;
        }
        if previous == 0 {
            self.tap = Some(Tap {
                first_down: t,
                last_down: t,
                fingers: n,
                excess_frames: 0,
                invalid: false,
            });
            self.start_touch(t, n, centroid, out);
        } else if n > previous {
            if let Some(tap) = &mut self.tap {
                tap.last_down = t;
                tap.fingers = tap.fingers.max(n);
            }
            self.drag_ref = t;
            if matches!(self.drag, TapDrag::Held { .. }) {
                // A second finger turns the drag into a scroll or gesture.
                self.release_drag(out);
            }
        }
        if n >= 2 {
            self.multi = true;
        }
        self.mode = match n {
            1 if !self.multi => Mode::Pointer,
            2 if self.settings.two_finger_scroll || self.settings.pinch || self.settings.rotate => {
                self.scroll = Scroll::new();
                Mode::ScrollPending
            }
            3 | 4
                if self.settings.swipe_enabled(n)
                    || (n == 4 && self.settings.four_finger_pinch) =>
            {
                Mode::SwipePending
            }
            _ => Mode::Idle,
        };
    }

    /// First counted finger down: resolve a held-back tap into a click or a drag.
    fn start_touch(
        &mut self,
        t: f64,
        n: usize,
        centroid: (f64, f64),
        out: &mut Vec<TouchpadEvent>,
    ) {
        self.drag_ref = t;
        match self.drag {
            TapDrag::Waiting {
                lifted_at,
                pos,
                click_sent,
            } => {
                let near = (centroid.0 - pos.0).hypot(centroid.1 - pos.1) <= SECOND_TAP_DRIFT_MM;
                if !click_sent {
                    click(TouchpadButton::Left, out);
                }
                if t - lifted_at <= SECOND_TAP_WINDOW_S && near && n == 1 {
                    out.push(TouchpadEvent::Button {
                        button: TouchpadButton::Left,
                        pressed: true,
                    });
                    self.drag = TapDrag::Held { fresh: true };
                } else {
                    self.drag = TapDrag::Idle;
                }
            }
            TapDrag::Lifted { .. } if n == 1 => self.drag = TapDrag::Held { fresh: false },
            TapDrag::Lifted { .. } => self.release_drag(out),
            TapDrag::Idle | TapDrag::Held { .. } => {}
        }
    }

    /// Last counted finger up at `t`: decide the tap and the drag state.
    /// `self.centroid` is where the finger was in the previous frame.
    fn finish_touch(&mut self, t: f64, out: &mut Vec<TouchpadEvent>) {
        let Some(tap) = self.tap.take() else { return };
        let valid = self.settings.tap_to_click
            && !tap.invalid
            && t - tap.first_down < TAP_MAX_DURATION_S
            && tap.last_down - tap.first_down < TAP_MAX_LANDING_SPREAD_S
            && self.hand_speed <= TAP_MAX_HAND_SPEED_MM_S;
        let lock = self.settings.drag_lock;
        if let TapDrag::Held { fresh } = self.drag {
            self.drag = match (valid, lock) {
                (true, true) if fresh => TapDrag::Lifted { release_at: None },
                (true, _) => {
                    self.release_drag(out);
                    return;
                }
                (false, true) => TapDrag::Lifted { release_at: None },
                (false, false) => TapDrag::Lifted {
                    release_at: Some((self.drag_ref + DRAG_GRACE_S).max(t + DRAG_BRIDGE_S)),
                },
            };
            return;
        }
        if !valid {
            return;
        }
        match tap.fingers {
            1 if self.settings.tap_drag => {
                self.drag = TapDrag::Waiting {
                    lifted_at: t,
                    pos: self.centroid,
                    click_sent: false,
                }
            }
            1 => click(TouchpadButton::Left, out),
            2 => click(TouchpadButton::Right, out),
            _ => {}
        }
    }

    fn release_drag(&mut self, out: &mut Vec<TouchpadEvent>) {
        out.push(TouchpadEvent::Button {
            button: TouchpadButton::Left,
            pressed: false,
        });
        self.drag = TapDrag::Idle;
    }

    fn invalidate_tap(&mut self) {
        if let Some(tap) = &mut self.tap {
            tap.invalid = true;
        }
    }

    /// Excess motion cancels a tap after 10 frames, or 5 with two or more fingers.
    fn latch_excess_motion(&mut self) {
        if self.hand_speed <= TAP_MAX_HAND_SPEED_MM_S {
            return;
        }
        if let Some(tap) = &mut self.tap {
            tap.excess_frames += 1;
            let limit = if tap.fingers >= 2 {
                TAP_EXCESS_FRAMES_MULTI
            } else {
                TAP_EXCESS_FRAMES_ONE
            };
            if tap.excess_frames >= limit {
                tap.invalid = true;
            }
        }
    }

    fn pointer_motion(&mut self, delta: (f64, f64), t: f64, dt: f64, out: &mut Vec<TouchpadEvent>) {
        if delta == (0.0, 0.0) {
            return;
        }
        let speed = if dt > 0.0 {
            delta.0.hypot(delta.1) / dt
        } else {
            0.0
        };
        let thumb_resting = self.tracks.values().any(|tr| tr.thumb == Thumb::Resting);
        let gain = axis_gain(
            speed,
            if thumb_resting {
                RESTING_TRACKING
            } else {
                TRACKING
            },
        );
        self.drag_ref = t;
        out.push(TouchpadEvent::PointerMotion {
            dx: delta.0 / MM_PER_COUNT * gain,
            dy: delta.1 / MM_PER_COUNT * gain,
        });
    }

    /// Integrate one frame of two-finger scrolling: smooth the velocity, zero the minor axis when
    /// the other dominates, apply the gain and the scroll direction.
    fn scroll_motion(&mut self, delta: (f64, f64), t: f64, dt: f64, out: &mut Vec<TouchpadEvent>) {
        let raw = if dt > 0.0 {
            (delta.0 / dt, delta.1 / dt)
        } else {
            (0.0, 0.0)
        };
        let alpha = 1.0 - (-dt / SMOOTHING_TAU_S).exp();
        let smoothed = match self.scroll.smoothed_mm_s {
            None => raw,
            Some(prev) => (
                prev.0 + alpha * (raw.0 - prev.0),
                prev.1 + alpha * (raw.1 - prev.1),
            ),
        };
        self.scroll.smoothed_mm_s = Some(smoothed);
        let (ax, ay) = (
            smoothed.0.abs() * AXIS_DOMINANCE_WEIGHT,
            smoothed.1.abs() * AXIS_DOMINANCE_WEIGHT,
        );
        let gain = axis_gain(smoothed.0.hypot(smoothed.1), SCROLLING);
        let direction = if self.settings.natural_scrolling {
            1.0
        } else {
            -1.0
        };
        let dx = if ay > AXIS_DOMINANCE_K * ax {
            0.0
        } else {
            delta.0 / MM_PER_COUNT * gain * direction
        };
        let dy = if ax > AXIS_DOMINANCE_K * ay {
            0.0
        } else {
            delta.1 / MM_PER_COUNT * gain * direction
        };
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.scroll.position.0 += dx;
        self.scroll.position.1 += dy;
        self.scroll.vx.push(t, self.scroll.position.0);
        self.scroll.vy.push(t, self.scroll.position.1);
        out.push(TouchpadEvent::ScrollUpdate { dx, dy });
    }

    /// Three or four fingers have moved or spread enough: lock a swipe axis or a pinch.
    fn start_fluid(
        &mut self,
        centroid: (f64, f64),
        radius: f64,
        t: f64,
        out: &mut Vec<TouchpadEvent>,
    ) {
        let n = self.fingers;
        let d = (
            centroid.0 - self.anchor.centroid.0,
            centroid.1 - self.anchor.centroid.1,
        );
        let dr = radius - self.anchor.radius;
        let swipe = if self.settings.swipe_enabled(n) {
            d.0.hypot(d.1) / SWIPE_START_MM
        } else {
            0.0
        };
        let pinch = if n == 4 && self.settings.four_finger_pinch && self.anchor.radius > 0.0 {
            dr.abs() / PINCH_START_MM
        } else {
            0.0
        };
        if swipe < 1.0 && pinch < 1.0 {
            return;
        }
        self.invalidate_tap();
        if pinch > swipe {
            self.mode = Mode::Pinch(Fluid::new(n, PINCH_COMPLETE_MM), 1.0);
            out.push(TouchpadEvent::PinchBegin { fingers: n as u32 });
        } else {
            let axis = if d.0.abs() >= d.1.abs() {
                SwipeAxis::Horizontal
            } else {
                SwipeAxis::Vertical
            };
            self.mode = Mode::Swipe(axis, Fluid::new(n, SWIPE_COMPLETE_MM));
            out.push(TouchpadEvent::SwipeBegin {
                fingers: n as u32,
                axis,
            });
        }
        // Report the first update at once, with the travel that crossed the threshold.
        self.fluid_motion(centroid, radius, t, out);
    }

    /// Progress of the running swipe or pinch against the finger geometry at its start.
    fn fluid_motion(
        &mut self,
        centroid: (f64, f64),
        radius: f64,
        t: f64,
        out: &mut Vec<TouchpadEvent>,
    ) {
        let d = (
            centroid.0 - self.anchor.centroid.0,
            centroid.1 - self.anchor.centroid.1,
        );
        match &mut self.mode {
            Mode::Swipe(axis, fluid) => {
                let along = if *axis == SwipeAxis::Horizontal {
                    d.0
                } else {
                    d.1
                };
                fluid.update(t, along / fluid.complete_mm);
                out.push(TouchpadEvent::SwipeUpdate {
                    fingers: fluid.fingers,
                    progress: fluid.progress,
                });
            }
            Mode::Pinch(fluid, scale) => {
                fluid.update(t, (radius - self.anchor.radius) / fluid.complete_mm);
                *scale = radius / self.anchor.radius;
                out.push(TouchpadEvent::PinchUpdate {
                    fingers: fluid.fingers,
                    scale: *scale,
                    rotation_deg: 0.0,
                    progress: fluid.progress,
                });
            }
            _ => {}
        }
    }

    /// Close the running scroll, swipe or pinch at time `t`.
    fn end_mode(&mut self, t: f64, out: &mut Vec<TouchpadEvent>) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Scroll => {
                let (vx, vy) = if self.settings.momentum_scroll {
                    (self.scroll.vx.velocity(t), self.scroll.vy.velocity(t))
                } else {
                    (0.0, 0.0)
                };
                out.push(TouchpadEvent::ScrollEnd { vx, vy });
            }
            Mode::Swipe(_, fluid) => {
                out.push(TouchpadEvent::SwipeEnd {
                    fingers: fluid.fingers,
                    committed: fluid.commits(t),
                    progress: fluid.progress,
                });
            }
            Mode::Pinch(fluid, scale) => {
                out.push(TouchpadEvent::PinchEnd {
                    fingers: fluid.fingers,
                    committed: fluid.commits(t),
                    scale,
                });
            }
            Mode::Zoom { scale, .. } => out.push(TouchpadEvent::PinchEnd {
                fingers: 2,
                committed: true,
                scale,
            }),
            Mode::Idle | Mode::Pointer | Mode::ScrollPending | Mode::SwipePending => {}
        }
    }
}

/// A click: press and release at once.
fn click(button: TouchpadButton, out: &mut Vec<TouchpadEvent>) {
    out.push(TouchpadEvent::Button {
        button,
        pressed: true,
    });
    out.push(TouchpadEvent::Button {
        button,
        pressed: false,
    });
}

/// Distance and angle (radians, y down, so positive is clockwise on screen) from the first to the
/// second point; zero for fewer than two points.
fn pair_geometry(points: &[(f64, f64)]) -> (f64, f64) {
    match points {
        [a, b, ..] => ((b.0 - a.0).hypot(b.1 - a.1), (b.1 - a.1).atan2(b.0 - a.0)),
        _ => (0.0, 0.0),
    }
}

/// `angle` folded into `(-pi, pi]`.
fn wrap_angle(angle: f64) -> f64 {
    angle - std::f64::consts::TAU * (angle / std::f64::consts::TAU).round()
}

fn centroid_of(points: &[(f64, f64)]) -> (f64, f64) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let n = points.len() as f64;
    (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    )
}

/// Mean distance of the points from `centroid`.
fn radius_of(points: &[(f64, f64)], centroid: (f64, f64)) -> f64 {
    if points.is_empty() {
        return 0.0;
    }
    points
        .iter()
        .map(|p| (p.0 - centroid.0).hypot(p.1 - centroid.1))
        .sum::<f64>()
        / points.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEOMETRY: TouchpadGeometry = TouchpadGeometry {
        width_mm: 105.0,
        height_mm: 76.0,
        clickpad: true,
    };

    fn tapping() -> TouchpadSettings {
        TouchpadSettings {
            tap_to_click: true,
            ..TouchpadSettings::default()
        }
    }

    fn finger(id: u32, x: f64, y: f64) -> TouchContact {
        TouchContact {
            id,
            position_mm: (x, y),
            pressure: None,
            palm: false,
        }
    }

    fn frame(time_s: f64, contacts: Vec<TouchContact>) -> TouchpadFrame {
        TouchpadFrame {
            time_s,
            contacts,
            button_down: false,
        }
    }

    fn run(pad: &mut Touchpad, frames: &[TouchpadFrame]) -> Vec<TouchpadEvent> {
        frames.iter().flat_map(|f| pad.process_frame(f)).collect()
    }

    fn button(button: TouchpadButton, pressed: bool) -> TouchpadEvent {
        TouchpadEvent::Button { button, pressed }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    /// Fingers at `xs` (one row at `y`) shifted by `shift_x` and `shift_y` mm.
    fn row(t: f64, xs: &[f64], y: f64, shift_x: f64, shift_y: f64) -> TouchpadFrame {
        frame(
            t,
            xs.iter()
                .enumerate()
                .map(|(i, x)| finger(i as u32, x + shift_x, y + shift_y))
                .collect(),
        )
    }

    #[test]
    fn one_finger_tap_is_a_left_click() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let events = run(
            &mut pad,
            &[
                frame(0.0, vec![finger(0, 50.0, 30.0)]),
                frame(0.05, vec![finger(0, 50.0, 30.0)]),
                frame(0.1, vec![]),
            ],
        );
        assert_eq!(
            events,
            vec![
                button(TouchpadButton::Left, true),
                button(TouchpadButton::Left, false)
            ]
        );
    }

    #[test]
    fn two_finger_tap_is_a_right_click() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let two = || vec![finger(0, 40.0, 30.0), finger(1, 60.0, 30.0)];
        let events = run(
            &mut pad,
            &[frame(0.0, two()), frame(0.05, two()), frame(0.1, vec![])],
        );
        assert_eq!(
            events,
            vec![
                button(TouchpadButton::Right, true),
                button(TouchpadButton::Right, false)
            ]
        );
    }

    #[test]
    fn a_slow_press_is_not_a_tap() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let events = run(
            &mut pad,
            &[
                frame(0.0, vec![finger(0, 50.0, 30.0)]),
                frame(0.2, vec![finger(0, 50.0, 30.0)]),
                frame(0.4, vec![]),
            ],
        );
        assert_eq!(events, vec![]);
    }

    #[test]
    fn a_fast_flick_of_one_finger_is_not_a_tap() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        // 3 mm per 10 ms is 300 mm/s, far above the 80 mm/s limit.
        let mut frames: Vec<_> = (0..6)
            .map(|k| {
                frame(
                    f64::from(k) * 0.01,
                    vec![finger(0, 30.0 + 3.0 * f64::from(k), 30.0)],
                )
            })
            .collect();
        frames.push(frame(0.06, vec![]));
        let events = run(&mut pad, &frames);
        assert_eq!(events.len(), 5);
        assert!(
            events.iter().all(|e| *e
                == TouchpadEvent::PointerMotion {
                    dx: 3.0 / MM_PER_COUNT,
                    dy: 0.0
                }),
            "{events:?}"
        );
    }

    #[test]
    fn two_finger_vertical_scroll_locks_to_the_axis() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        // Each frame (10 ms): 2.0 mm down with 0.3 mm of sideways drift.
        let mut frames = vec![row(0.0, &[40.0, 60.0], 20.0, 0.0, 0.0)];
        frames.extend((1..=5).map(|k| {
            row(
                f64::from(k) * 0.01,
                &[40.0, 60.0],
                20.0,
                0.3 * f64::from(k),
                2.0 * f64::from(k),
            )
        }));
        frames.push(frame(0.06, vec![]));
        let events = run(&mut pad, &frames);
        let counts = 2.0 / MM_PER_COUNT;
        assert_eq!(events.len(), 7, "{events:?}");
        assert_eq!(events[0], TouchpadEvent::ScrollBegin);
        for e in &events[1..6] {
            match e {
                TouchpadEvent::ScrollUpdate { dx, dy } => {
                    assert_eq!(*dx, 0.0);
                    close(*dy, counts);
                }
                other => panic!("{other:?}"),
            }
        }
        match events[6] {
            TouchpadEvent::ScrollEnd { vx, vy } => {
                close(vx, 0.0);
                close(vy, counts / 0.01);
            }
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_diagonal_scroll_keeps_both_axes() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        let events = run(
            &mut pad,
            &[
                row(0.0, &[40.0, 60.0], 20.0, 0.0, 0.0),
                row(0.01, &[40.0, 60.0], 20.0, 1.5, 1.5),
            ],
        );
        match events[1] {
            TouchpadEvent::ScrollUpdate { dx, dy } => {
                close(dx, 1.5 / MM_PER_COUNT);
                close(dy, 1.5 / MM_PER_COUNT);
            }
            ref other => panic!("{other:?}"),
        }
    }

    /// A three-finger horizontal swipe: one frame per entry of `steps_mm`, 10 ms apart.
    fn swipe_frames(steps_mm: &[f64]) -> Vec<TouchpadFrame> {
        let mut x = 0.0;
        let mut frames = vec![row(0.0, &[20.0, 40.0, 60.0], 30.0, 0.0, 0.0)];
        for (k, s) in steps_mm.iter().enumerate() {
            x += s;
            frames.push(row(
                (k + 1) as f64 * 0.01,
                &[20.0, 40.0, 60.0],
                30.0,
                x,
                0.0,
            ));
        }
        frames
    }

    #[test]
    fn a_swipe_that_keeps_its_momentum_commits() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let mut frames = swipe_frames(&[5.0; 12]);
        frames.push(frame(0.13, vec![]));
        let events = run(&mut pad, &frames);
        assert_eq!(
            events[0],
            TouchpadEvent::SwipeBegin {
                fingers: 3,
                axis: SwipeAxis::Horizontal
            }
        );
        match events[1] {
            TouchpadEvent::SwipeUpdate {
                fingers: 3,
                progress,
            } => close(progress, 5.0 / 65.0),
            ref other => panic!("{other:?}"),
        }
        match events.last() {
            Some(TouchpadEvent::SwipeEnd {
                fingers: 3,
                committed: true,
                progress,
            }) => close(*progress, 60.0 / 65.0),
            other => panic!("{other:?}"),
        }
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, TouchpadEvent::Button { .. })),
            "a swipe is not a tap"
        );
    }

    #[test]
    fn a_swipe_that_falls_back_below_80_percent_of_its_peak_cancels() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        // Out to 60 mm (progress 0.923), back to 45 mm (0.692 < 0.8 * 0.923), then rest.
        let mut steps = vec![5.0; 12];
        steps.extend([-5.0; 3]);
        steps.extend([0.0; 20]);
        let mut frames = swipe_frames(&steps);
        let end = frames.len() as f64 * 0.01;
        frames.push(frame(end, vec![]));
        let events = run(&mut pad, &frames);
        match events.last() {
            Some(TouchpadEvent::SwipeEnd {
                fingers: 3,
                committed: false,
                progress,
            }) => close(*progress, 45.0 / 65.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_resting_thumb_does_not_move_the_pointer() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let thumb = || finger(7, 40.0, 70.0);
        let mut frames: Vec<_> = (0..=5)
            .map(|k| frame(f64::from(k) * 0.1, vec![thumb()]))
            .collect();
        // After 0.5 s the thumb has rested for longer than a tap; an index finger lands and slides.
        for k in 0..=3 {
            frames.push(frame(
                0.5 + f64::from(k) * 0.01,
                vec![thumb(), finger(1, 60.0 + f64::from(k), 30.0)],
            ));
        }
        let events = run(&mut pad, &frames);
        assert_eq!(events.len(), 3, "{events:?}");
        for e in events {
            match e {
                TouchpadEvent::PointerMotion { dx, dy } => {
                    // 1 mm in 10 ms is 100 mm/s: 15.748 counts times the resting-hand gain
                    // (100 / 12)^0.06.
                    close(dx, 17.88444222);
                    assert_eq!(dy, 0.0);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_palm_is_ignored() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let palm = |x: f64| TouchContact {
            palm: true,
            ..finger(3, x, 50.0)
        };
        let events = run(
            &mut pad,
            &[
                frame(0.0, vec![palm(30.0)]),
                frame(0.05, vec![palm(40.0)]),
                frame(0.1, vec![]),
            ],
        );
        assert_eq!(events, vec![]);
    }

    #[test]
    fn a_palm_beside_a_finger_neither_scrolls_nor_blocks_the_pointer() {
        let mut pad = Touchpad::new(GEOMETRY, tapping());
        let palm = || TouchContact {
            palm: true,
            ..finger(3, 90.0, 60.0)
        };
        let events = run(
            &mut pad,
            &[
                frame(0.0, vec![finger(0, 50.0, 30.0), palm()]),
                frame(0.01, vec![finger(0, 51.0, 30.0), palm()]),
            ],
        );
        assert_eq!(events.len(), 1);
        match events[0] {
            TouchpadEvent::PointerMotion { dx, .. } => close(dx, 1.0 / MM_PER_COUNT),
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_two_finger_physical_click_is_a_secondary_click() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        let two = |down| TouchpadFrame {
            button_down: down,
            ..frame(0.0, vec![finger(0, 40.0, 30.0), finger(1, 60.0, 30.0)])
        };
        let mut events = pad.process_frame(&two(true));
        events.extend(pad.process_frame(&two(false)));
        assert_eq!(
            events,
            vec![
                button(TouchpadButton::Right, true),
                button(TouchpadButton::Right, false)
            ]
        );
    }

    #[test]
    fn a_corner_click_is_secondary_only_when_enabled() {
        let corner = |down| TouchpadFrame {
            button_down: down,
            ..frame(0.0, vec![finger(0, 100.0, 70.0)])
        };
        let mut off = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        assert_eq!(
            off.process_frame(&corner(true)),
            vec![button(TouchpadButton::Left, true)]
        );
        let settings = TouchpadSettings {
            corner_click: CornerClick::BottomRight,
            ..TouchpadSettings::default()
        };
        let mut on = Touchpad::new(GEOMETRY, settings);
        assert_eq!(
            on.process_frame(&corner(true)),
            vec![button(TouchpadButton::Right, true)]
        );
        assert_eq!(
            on.process_frame(&corner(false)),
            vec![button(TouchpadButton::Right, false)]
        );
    }

    #[test]
    fn tap_and_a_half_drags_and_releases_after_the_grace() {
        let settings = TouchpadSettings {
            tap_drag: true,
            ..tapping()
        };
        let mut pad = Touchpad::new(GEOMETRY, settings);
        let at = |x: f64| vec![finger(0, x, 30.0)];
        // A lone tap is held back.
        assert_eq!(
            run(&mut pad, &[frame(0.0, at(50.0)), frame(0.05, vec![])]),
            vec![]
        );
        assert_eq!(pad.next_deadline(), Some(0.05 + TAP_HOLD_S));
        // A second touch within the window delivers the click, then presses and drags.
        let events = run(
            &mut pad,
            &[
                frame(0.15, at(50.0)),
                frame(0.2, at(51.0)),
                frame(0.25, at(52.0)),
            ],
        );
        assert_eq!(events.len(), 5, "{events:?}");
        assert_eq!(
            events[..3],
            [
                button(TouchpadButton::Left, true),
                button(TouchpadButton::Left, false),
                button(TouchpadButton::Left, true)
            ]
        );
        assert!(
            matches!(events[3], TouchpadEvent::PointerMotion { .. })
                && matches!(events[4], TouchpadEvent::PointerMotion { .. })
        );
        // Lifting does not release at once: 0.5 s after the last motion.
        assert_eq!(pad.process_frame(&frame(0.45, vec![])), vec![]);
        assert_eq!(pad.next_deadline(), Some(0.75));
        assert_eq!(pad.tick(0.74), vec![]);
        assert_eq!(pad.tick(0.75), vec![button(TouchpadButton::Left, false)]);
    }

    #[test]
    fn a_lone_tap_without_a_second_touch_clicks_after_the_hold() {
        let settings = TouchpadSettings {
            tap_drag: true,
            ..tapping()
        };
        let mut pad = Touchpad::new(GEOMETRY, settings);
        run(
            &mut pad,
            &[frame(0.0, vec![finger(0, 50.0, 30.0)]), frame(0.05, vec![])],
        );
        assert_eq!(
            pad.tick(0.25),
            vec![
                button(TouchpadButton::Left, true),
                button(TouchpadButton::Left, false)
            ]
        );
        assert_eq!(pad.next_deadline(), Some(0.05 + SECOND_TAP_WINDOW_S));
    }

    #[test]
    fn drag_lock_holds_until_a_tap() {
        let settings = TouchpadSettings {
            tap_drag: true,
            drag_lock: true,
            ..tapping()
        };
        let mut pad = Touchpad::new(GEOMETRY, settings);
        let at = || vec![finger(0, 50.0, 30.0)];
        let events = run(
            &mut pad,
            &[
                frame(0.0, at()),
                frame(0.05, vec![]),
                frame(0.15, at()),
                frame(0.6, vec![]),
            ],
        );
        assert_eq!(
            events,
            vec![
                button(TouchpadButton::Left, true),
                button(TouchpadButton::Left, false),
                button(TouchpadButton::Left, true)
            ]
        );
        assert_eq!(pad.next_deadline(), None);
        assert_eq!(pad.tick(100.0), vec![]);
        let events = run(&mut pad, &[frame(100.0, at()), frame(100.05, vec![])]);
        assert_eq!(events, vec![button(TouchpadButton::Left, false)]);
    }

    #[test]
    fn force_click_follows_the_slider_scaled_ladder() {
        let settings = TouchpadSettings {
            click_pressure: ClickPressure::Firm,
            ..TouchpadSettings::default()
        };
        let mut pad = Touchpad::new(GEOMETRY, settings);
        let pressed = |t: f64, p: f64| {
            frame(
                t,
                vec![TouchContact {
                    pressure: Some(p),
                    ..finger(0, 50.0, 30.0)
                }],
            )
        };
        // Firm: activation 360 and release 306 of 500 raw.
        assert_eq!(pad.process_frame(&pressed(0.0, 0.70)), vec![]);
        assert_eq!(
            pad.process_frame(&pressed(0.1, 0.73)),
            vec![TouchpadEvent::ForceClick { active: true }]
        );
        assert_eq!(pad.process_frame(&pressed(0.2, 0.62)), vec![]);
        assert_eq!(
            pad.process_frame(&pressed(0.3, 0.60)),
            vec![TouchpadEvent::ForceClick { active: false }]
        );
        // Without pressure the model never reports a force click.
        let mut plain = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        assert_eq!(
            plain.process_frame(&frame(0.0, vec![finger(0, 50.0, 30.0)])),
            vec![]
        );
    }

    /// Two fingers `half` mm either side of (50, 30), turned by `deg` degrees.
    fn pair(t: f64, half: f64, deg: f64) -> TouchpadFrame {
        let (s, c) = deg.to_radians().sin_cos();
        frame(
            t,
            vec![
                finger(0, 50.0 - half * c, 30.0 - half * s),
                finger(1, 50.0 + half * c, 30.0 + half * s),
            ],
        )
    }

    #[test]
    fn two_fingers_spreading_apart_pinch() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        // Spread 20 mm to 40 mm in 2 mm steps (1 mm per finger).
        let frames: Vec<_> = (0..=10)
            .map(|k| pair(f64::from(k) * 0.01, 10.0 + f64::from(k), 0.0))
            .collect();
        let events = run(&mut pad, &frames);
        assert_eq!(events[0], TouchpadEvent::PinchBegin { fingers: 2 });
        match events.last() {
            Some(TouchpadEvent::PinchUpdate {
                fingers: 2,
                scale,
                rotation_deg,
                ..
            }) => {
                close(*scale, 2.0);
                close(*rotation_deg, 0.0);
            }
            other => panic!("{other:?}"),
        }
        assert!(!events
            .iter()
            .any(|e| matches!(e, TouchpadEvent::ScrollBegin)));
    }

    #[test]
    fn two_fingers_moving_together_scroll() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        let events = run(
            &mut pad,
            &[
                row(0.0, &[40.0, 60.0], 30.0, 0.0, 0.0),
                row(0.01, &[40.0, 60.0], 30.0, 0.0, 3.0),
            ],
        );
        assert_eq!(events[0], TouchpadEvent::ScrollBegin);
        assert!(!events
            .iter()
            .any(|e| matches!(e, TouchpadEvent::PinchBegin { .. })));
    }

    #[test]
    fn two_fingers_rotating_in_place_report_the_angle() {
        let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
        let frames: Vec<_> = (0..=6)
            .map(|k| pair(f64::from(k) * 0.01, 10.0, 5.0 * f64::from(k)))
            .collect();
        let events = run(&mut pad, &frames);
        assert_eq!(events[0], TouchpadEvent::PinchBegin { fingers: 2 });
        match events.last() {
            Some(TouchpadEvent::PinchUpdate {
                scale,
                rotation_deg,
                ..
            }) => {
                close(*scale, 1.0);
                close(*rotation_deg, 30.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn axis_gain_applies_only_above_the_threshold() {
        close(axis_gain(24.0, RESTING_TRACKING), 2.0_f64.powf(0.06));
        assert_eq!(axis_gain(12.0, RESTING_TRACKING), 1.0);
        assert_eq!(axis_gain(500.0, TRACKING), 1.0);
    }

    #[test]
    fn the_thumb_zone_scales_with_the_sensor_height() {
        close(GEOMETRY.thumb_zone_height(), 12.0);
        close(
            TouchpadGeometry {
                height_mm: 114.0,
                ..GEOMETRY
            }
            .thumb_zone_height(),
            18.0,
        );
        close(
            TouchpadGeometry {
                height_mm: 19.0,
                ..GEOMETRY
            }
            .thumb_zone_height(),
            6.0,
        );
    }
}
