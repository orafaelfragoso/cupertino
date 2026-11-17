//! What the design overlay's sender does with a key press, in one place: the level steps and the hold times,
//! and the one
//! platform deviation of the prototype.

/// Steps of a normal press: 16, step 1/16, one meter segment.
pub const COARSE_STEPS: u32 = 16;

/// Steps of a fine press (Shift and Alt held): 64, step 1/64, a quarter segment.
pub const FINE_STEPS: u32 = 64;

/// The current level is snapped to the nearest 1/64 before a press: `q = floor((v + 1/128) * 64) / 64`.
const SNAP_STEPS: f64 = 64.0;
const SNAP_OFFSET: f64 = 1.0 / 128.0;

/// the sender passes when the press changed the level.
pub const HOLD_CHANGED_MSEC: u32 = 4000;

/// the sender passes when the press was refused or changed nothing, for example up at 1.0.
pub const HOLD_UNCHANGED_MSEC: u32 = 1000;

/// Lowest raw display brightness a press reaches. The design requests down to 0.0, but a Linux
/// backlight at raw 0 can blank the panel, so the prototype never goes below raw 1: a platform deviation (approximate).
pub const DISPLAY_FLOOR_RAW: u32 = 1;

/// The level one press away from `level` (0 to 1) in the sign of `direction`, clamped to 0 to 1:
/// `q = floor((level + 1/128) * 64) / 64`, then up `floor(n (q + 1/n)) / n` or down `ceil(n (q - 1/n)) / n`, with
/// `n` 16 for a normal press and 64 for a fine one. A direction of 0 returns `level` unchanged.
pub fn step_level(level: f64, direction: i32, fine: bool) -> f64 {
    let steps = f64::from(if fine { FINE_STEPS } else { COARSE_STEPS });
    let snapped = ((level + SNAP_OFFSET) * SNAP_STEPS).floor() / SNAP_STEPS;
    let next = match direction.signum() {
        1 => (steps * (snapped + 1.0 / steps)).floor() / steps,
        -1 => (steps * (snapped - 1.0 / steps)).ceil() / steps,
        _ => return level,
    };
    next.clamp(0.0, 1.0)
}

/// Whole seconds the overlay holds at full opacity before its fade: `floor(msecUntilFade / 1000),
/// with the sender's 4000 ms after a change and 1000 ms otherwise.
pub fn hold_seconds(changed: bool) -> f64 {
    let msec = if changed {
        HOLD_CHANGED_MSEC
    } else {
        HOLD_UNCHANGED_MSEC
    };
    f64::from(msec / 1000)
}
