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
    let msec = if changed { HOLD_CHANGED_MSEC } else { HOLD_UNCHANGED_MSEC };
    f64::from(msec / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_steps_from_a_grid_level_move_one_segment() {
        assert_eq!(step_level(0.5, 1, false), 0.5625);
        assert_eq!(step_level(0.5, -1, false), 0.4375);
        assert_eq!(step_level(0.0, 1, false), 0.0625);
    }

    #[test]
    fn coarse_steps_from_an_off_grid_level_snap_first() {
        // 0.30 snaps to 19/64 = 0.296875: the next segment boundary is 0.3125 above and 0.25 below
        assert_eq!(step_level(0.30, 1, false), 0.3125);
        assert_eq!(step_level(0.30, -1, false), 0.25);
        // 0.01 snaps to 1/64: down reaches 0
        assert_eq!(step_level(0.01, -1, false), 0.0);
    }

    #[test]
    fn fine_steps_move_a_sixty_fourth_from_the_snapped_level() {
        assert_eq!(step_level(0.5, 1, true), 0.515625);
        assert_eq!(step_level(0.5, -1, true), 0.484375);
        // 0.30 snaps to 0.296875
        assert_eq!(step_level(0.30, 1, true), 0.3125);
        assert_eq!(step_level(0.30, -1, true), 0.28125);
    }

    #[test]
    fn steps_clamp_at_zero_and_one() {
        assert_eq!(step_level(1.0, 1, false), 1.0);
        assert_eq!(step_level(0.99, 1, false), 1.0);
        assert_eq!(step_level(1.0, 1, true), 1.0);
        assert_eq!(step_level(0.0, -1, false), 0.0);
        assert_eq!(step_level(0.0, -1, true), 0.0);
        assert_eq!(step_level(0.984375, 1, true), 1.0);
    }

    #[test]
    fn the_hold_is_whole_seconds_by_whether_the_press_changed_anything() {
        assert_eq!(hold_seconds(true), 4.0);
        assert_eq!(hold_seconds(false), 1.0);
    }
}
