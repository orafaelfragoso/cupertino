//! Pointer acceleration, scroll carry, axis input, mouse identity, and drag-threshold models.

use std::collections::BTreeMap;

use cupertino_input::{MouseAccel, PointerAccel, ScrollAxisAccel, ScrollPhase};
use cupertino_wm::{Command, Point as MPoint};
use smithay::backend::input::{Axis, AxisRelativeDirection, AxisSource};
use smithay::utils::{Logical, Point};

/// Tracking-speed, acceleration, and per-mouse DPI settings consumed by [`PointerDevice`].
pub trait PointerAccelConfig {
    fn mouse_speed(&self) -> f64;
    fn trackpad_speed(&self) -> f64;
    fn mouse_dpi(&self, key: &str) -> Option<u32>;
    fn mouse_acceleration(&self) -> bool;
    fn trackpad_acceleration(&self) -> bool;
}

/// Standalone pointer acceleration settings (without cursor-rendering fields).
#[derive(Clone, Debug, PartialEq)]
pub struct PointerAccelSettings {
    pub mouse_speed: f64,
    pub trackpad_speed: f64,
    pub mouse_dpi: BTreeMap<String, u32>,
    pub mouse_acceleration: bool,
    pub trackpad_acceleration: bool,
}

impl Default for PointerAccelSettings {
    fn default() -> Self {
        Self {
            mouse_speed: 0.6875,
            trackpad_speed: 0.6875,
            mouse_dpi: BTreeMap::new(),
            mouse_acceleration: true,
            trackpad_acceleration: true,
        }
    }
}

impl PointerAccelConfig for PointerAccelSettings {
    fn mouse_speed(&self) -> f64 {
        self.mouse_speed
    }

    fn trackpad_speed(&self) -> f64 {
        self.trackpad_speed
    }

    fn mouse_dpi(&self, key: &str) -> Option<u32> {
        self.mouse_dpi.get(key).copied()
    }

    fn mouse_acceleration(&self) -> bool {
        self.mouse_acceleration
    }

    fn trackpad_acceleration(&self) -> bool {
        self.trackpad_acceleration
    }
}

/// One connected mouse as the `Mice` property lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseEntry {
    /// `cupertino_defaults::compositor::mouse_dpi_key` of the device.
    pub key: String,
    pub name: String,
    /// The resolution in use, in dots per inch.
    pub dpi: u32,
    /// The user stored the resolution (`mouse_dpi`), as opposed to the hardware database or the default.
    pub stored: bool,
}

/// Which kind of gesture was forwarded to the client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureSource {
    #[default]
    Swipe,
    Pinch,
}

/// How a device's motion is accelerated.
#[derive(Clone, Debug)]
enum Accel {
    Pad(PointerAccel),
    Mouse(MouseAccel),
}

/// What tells a mouse's resolution: the key of the user's stored value and the hardware database's `MOUSE_DPI`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MouseIdentity {
    /// `mouse_dpi_key` of the device.
    pub key: Option<String>,
    /// The device name.
    pub name: String,
    /// The `MOUSE_DPI` property of the device node.
    pub hwdb_dpi: Option<String>,
}

/// Acceleration state of one relative pointing device.
#[derive(Clone, Debug)]
pub struct PointerDevice {
    pub touchpad: bool,
    identity: MouseIdentity,
    accel: Accel,
    last_usec: Option<u64>,
    /// Scroll accelerators (horizontal, vertical) of a device with a scroll resolution: only the
    /// touchpad. A wheel has none: the design gives a generic wheel, high-resolution or
    /// not, no scroll resolution and so no scroll acceleration.
    scroll: Option<[ScrollAxisAccel; 2]>,
    /// Scroll count of the touchpad's flick series.
    scroll_count: cupertino_input::ScrollCounter,
    /// A finger scroll gesture is under way: its next event is a `Changed`, not a `Began`.
    scrolling: bool,
    /// The device has moved, scrolled or clicked: a keyboard's or a receiver's unused pointer interface never does.
    used: bool,
}

impl PointerDevice {
    /// Touchpads use the touchpad curve family and report rate, taking the touchpad model's motion
    /// (`MM_PER_COUNT` per count). Mice use the default acceleration table on the speed of the hand
    /// in inches per second, from counts, the resolution of the device and the event timestamps. Touchpad
    /// finger scrolling is accelerated with the trackpad scroll curves.
    pub fn new(touchpad: bool, cfg: &impl PointerAccelConfig) -> Self {
        Self::with_identity(touchpad, cfg, MouseIdentity::default())
    }

    /// Like [`PointerDevice::new`] for a mouse that `identity` tells its resolution of.
    pub fn with_identity(
        touchpad: bool,
        cfg: &impl PointerAccelConfig,
        identity: MouseIdentity,
    ) -> Self {
        let (accel, scroll) = if touchpad {
            let pad = cupertino_input::TouchpadSettings {
                tracking_speed: cfg.trackpad_speed(),
                acceleration: cfg.trackpad_acceleration(),
                ..cupertino_input::TouchpadSettings::default()
            };
            (
                Accel::Pad(pad.pointer_accel()),
                pad.scroll_accel().map(|axis| [axis.clone(), axis]),
            )
        } else {
            let stored = identity.key.as_deref().and_then(|key| cfg.mouse_dpi(key));
            let dpi = cupertino_input::resolve_dpi(stored, identity.hwdb_dpi.as_deref());
            let mouse = if cfg.mouse_acceleration() {
                let curve = cupertino_input::TableCurve::new(
                    &cupertino_input::AccelTable::default_table(),
                    cfg.mouse_speed(),
                );
                MouseAccel::new(curve.map(cupertino_input::AccelCurve::Table), dpi)
            } else {
                MouseAccel::linear(cfg.mouse_speed(), dpi)
            };
            (Accel::Mouse(mouse), None)
        };
        Self {
            touchpad,
            identity,
            accel,
            last_usec: None,
            scroll,
            scroll_count: cupertino_input::ScrollCounter::default(),
            scrolling: false,
            used: false,
        }
    }

    /// Note that the device moved, scrolled or clicked; true the first time.
    pub fn note_use(&mut self) -> bool {
        !std::mem::replace(&mut self.used, true)
    }

    /// A mouse (not a touchpad) that has moved, scrolled or clicked.
    pub fn is_used_mouse(&self) -> bool {
        !self.touchpad && self.used
    }

    /// The `Mice` entry of this device, for a used mouse that has a key.
    pub fn mouse_entry(&self, cfg: &impl PointerAccelConfig) -> Option<MouseEntry> {
        if self.touchpad || !self.used {
            return None;
        }
        let key = self.identity.key.clone()?;
        let stored = cfg.mouse_dpi(&key);
        let dpi = cupertino_input::resolve_dpi(stored, self.identity.hwdb_dpi.as_deref());
        Some(MouseEntry {
            key,
            name: self.identity.name.clone(),
            dpi: dpi.round() as u32,
            stored: stored.is_some(),
        })
    }

    /// Take new tracking settings: the device starts over as [`PointerDevice::new`] made it.
    pub fn reconfigure(&mut self, cfg: &impl PointerAccelConfig) {
        *self = Self {
            used: self.used,
            ..Self::with_identity(self.touchpad, cfg, self.identity.clone())
        };
    }

    /// Accelerated delta for unaccelerated `delta` at `usec`.
    pub fn accelerate(&mut self, delta: Point<f64, Logical>, usec: u64) -> Point<f64, Logical> {
        let dt_ms = self
            .last_usec
            .map_or(0.0, |last| usec.saturating_sub(last) as f64 / 1000.0);
        self.last_usec = Some(usec);
        match &mut self.accel {
            Accel::Pad(pad) => pad.accelerate(delta.x, delta.y, dt_ms).into(),
            Accel::Mouse(mouse) => mouse.accelerate(delta.x, delta.y, usec).into(),
        }
    }

    /// Points to scroll for `counts` raw scroll counts on `axis` at `usec`: the accelerator's
    /// lines, coalesced as the design's event translator does, at 10 points each;
    /// zero while the event is held back, `None` for a device without a scroll
    /// resolution. The coalescing already quantises to whole points.
    pub fn scroll_points(&mut self, axis: Axis, counts: f64, usec: u64) -> Option<f64> {
        let scroll = self.scroll.as_mut()?;
        let accel = match axis {
            Axis::Horizontal => &mut scroll[0],
            Axis::Vertical => &mut scroll[1],
        };
        Some(accel.scroll_lines(counts, usec as f64 / 1000.0) * cupertino_input::POINTS_PER_LINE)
    }

    /// The scroll count after a finger scroll event at `usec` that sends `points` (horizontal, vertical),
    /// or after the end of the gesture when `stop`: the coalescing's held-back events (no points) and
    /// the end of a gesture have nothing to count and give `None`, as does a device without a scroll
    /// resolution (a wheel has no gesture phases, so no count).
    pub fn scroll_count(&mut self, points: (f64, f64), stop: bool, usec: u64) -> Option<u32> {
        self.scroll.as_ref()?;
        let ms = usec as f64 / 1000.0;
        if stop {
            if std::mem::take(&mut self.scrolling) {
                self.scroll_count.update(ScrollPhase::Ended, 0.0, 0.0, ms);
            }
            return None;
        }
        if points == (0.0, 0.0) {
            return None;
        }
        let phase = if std::mem::replace(&mut self.scrolling, true) {
            ScrollPhase::Changed
        } else {
            ScrollPhase::Began
        };
        Some(self.scroll_count.update(phase, points.0, points.1, ms))
    }
}

/// A pointer axis event as the server reads it. `amount` is the backend's value (touchpad counts for finger
/// scrolling) and `v120` the wheel's high-resolution steps; the arrays hold the horizontal axis first.
#[derive(Clone, Debug)]
pub struct AxisInput {
    /// Identifier of the pointing device.
    pub device: String,
    pub source: AxisSource,
    /// Event time in microseconds.
    pub usec: u64,
    pub amount: [Option<f64>; 2],
    pub v120: [Option<f64>; 2],
    pub relative_direction: [AxisRelativeDirection; 2],
}

impl AxisInput {
    /// The axes this event stops, horizontal first: a finger scroll stops an axis with an amount of exactly zero.
    pub fn stopped_axes(&self) -> [bool; 2] {
        self.amount
            .map(|a| self.source == AxisSource::Finger && a == Some(0.0))
    }

    /// Whether this event ends a touchpad gesture: a finger scroll where no axis moves and one is exactly zero.
    pub fn ends_gesture(&self) -> bool {
        self.source == AxisSource::Finger
            && self.amount.contains(&Some(0.0))
            && self.amount.iter().all(|a| a.is_none_or(|a| a == 0.0))
    }
}

/// `wl_pointer.axis` carries a fixed-point value in 1/256 steps. A value is sent rounded to that step and what the
/// rounding lost is added to the next value of the same axis, source and direction, so a run of small scrolls (a
/// high-resolution wheel's fractions of a notch) adds up to the distance the device moved. A stop, another source or
/// a reversal starts the axis over.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisCarry {
    carried: [f64; 2],
    /// Whether the last value of each axis was positive.
    positive: [bool; 2],
    source: Option<AxisSource>,
}

/// Steps of the protocol's fixed-point numbers per unit.
const FIXED_STEPS: f64 = 256.0;

impl AxisCarry {
    /// The value to send on axis `i` for `value` of `source`: `value` plus what earlier values lost, rounded to the
    /// fixed-point step.
    pub fn send(&mut self, i: usize, source: AxisSource, value: f64) -> f64 {
        if self.source != Some(source) {
            *self = AxisCarry {
                source: Some(source),
                ..AxisCarry::default()
            };
        }
        let positive = value > 0.0;
        let carried = &mut self.carried[i];
        if self.positive[i] != positive {
            self.positive[i] = positive;
            *carried = 0.0;
        }
        let total = value + *carried;
        let sent = (total * FIXED_STEPS).round() / FIXED_STEPS;
        *carried = total - sent;
        sent
    }

    /// The axis `i` stopped: nothing carries into the next gesture.
    pub fn stop(&mut self, i: usize) {
        self.carried[i] = 0.0;
    }
}

pub fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}

/// A press the server took for a window move or resize, which starts only once the pointer leaves
/// the drag-start box around the press (5 pt), so a click with a little jitter stays a
/// click.
#[derive(Clone, Debug)]
pub struct PendingDrag {
    pub button: u32,
    pub press: MPoint,
    /// The `BeginMove` or `BeginResize`, anchored at the press point.
    pub command: Command,
}

/// Linux evdev button code for the primary (left) mouse button.
pub const BTN_LEFT: u32 = 0x110;
/// Linux evdev button code for the secondary (right) mouse button.
pub const BTN_RIGHT: u32 = 0x111;
