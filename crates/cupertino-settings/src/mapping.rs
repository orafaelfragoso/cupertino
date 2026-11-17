//! Between control positions and stored values (§6.3).

/// Tracking speed stops (Mouse, Trackpad): slider positions 0…9; the default 0.6875 is stop 3
/// (settled).
pub const TRACKING_STOPS: [f64; 10] = [0.0, 0.125, 0.5, 0.6875, 0.875, 1.0, 1.5, 2.0, 2.5, 3.0];
pub const TRACKING_DEFAULT: f64 = 0.6875;

/// The stop of a stored tracking speed: the nearest one.
pub fn tracking_stop(speed: f64) -> usize {
    nearest(&TRACKING_STOPS, speed)
}

fn nearest(stops: &[f64], v: f64) -> usize {
    stops
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs()))
        .map(|(i, _)| i)
        .expect("the stop table is not empty")
}

/// Dock size: the slider value v in 0…1 is `(px − 16) / 112`.
pub const DOCK_MIN_PX: f64 = 16.0;
pub const DOCK_SPAN_PX: f64 = 112.0;
/// The magnified size at the magnification slider's Large end (the size slider's Large end too).
pub const MAGNIFIED_MAX_PX: f64 = DOCK_MIN_PX + DOCK_SPAN_PX;
/// The magnification slider's marks: Off 0, Small 0.125, Large 1.
pub const MAGNIFICATION_MARKS: [f64; 3] = [0.0, 0.125, 1.0];
/// The dock size slider's marks: Small 0, Large 1.
pub const DOCK_SIZE_MARKS: [f64; 2] = [0.0, 1.0];

pub fn dock_px(v: f64) -> f64 {
    (DOCK_MIN_PX + DOCK_SPAN_PX * v.clamp(0.0, 1.0)).round()
}

pub fn dock_value(px: f64) -> f64 {
    ((px - DOCK_MIN_PX) / DOCK_SPAN_PX).clamp(0.0, 1.0)
}

/// The magnification slider at `v` over tiles of `tile_px`: off at 0, else on with a magnified size
/// from just above the tile to [`MAGNIFIED_MAX_PX`] (the tile's own size when it is larger), so every
/// position past Off magnifies. The dock clamps the size by its own rule when it applies it.
pub fn magnification(v: f64, tile_px: f64) -> (bool, Option<f64>) {
    if v <= 0.0 {
        return (false, None);
    }
    let top = MAGNIFIED_MAX_PX.max(tile_px);
    (true, Some((tile_px + (top - tile_px) * v.min(1.0)).ceil()))
}

/// The magnification slider's value for a stored state over tiles of `tile_px`: Off while the
/// magnified size is no larger than the tile, which the dock does not magnify.
pub fn magnification_value(on: bool, large_px: f64, tile_px: f64) -> f64 {
    let top = MAGNIFIED_MAX_PX.max(tile_px);
    if on && top > tile_px {
        ((large_px - tile_px) / (top - tile_px)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Key repeat levels in 1/60 s units: the rate's seven levels and the delay's seven, level
/// 0 of the delay (300000) being unreachable from its slider.
pub const RATE_UNITS: [f64; 7] = [120.0, 90.0, 60.0, 30.0, 12.0, 5.0, 2.0];
pub const DELAY_UNITS: [f64; 7] = [300_000.0, 120.0, 94.0, 68.0, 30.0, 25.0, 15.0];
/// Milliseconds per 1/60 s unit.
const UNIT_MS: f64 = 1000.0 / 60.0;

/// The rate slider runs 0…7: 0 is repeat off, p ≥ 1 is rate level p − 1. Default 6 (5 units, 83 ms).
pub const RATE_POSITIONS: usize = 8;
pub const RATE_DEFAULT: usize = 6;
/// The delay slider runs 1…6 over the delay levels. Default 4 (30 units, 500 ms).
pub const DELAY_FIRST: usize = 1;
pub const DELAY_LAST: usize = 6;
pub const DELAY_DEFAULT: usize = 4;

fn ms(units: f64) -> u32 {
    (units * UNIT_MS).round() as u32
}

/// The repeat interval of rate position `p` ≥ 1, in ms.
pub fn rate_interval_ms(p: usize) -> u32 {
    ms(RATE_UNITS[p - 1])
}

/// The delay of delay position `p` (1…6), in ms.
pub fn delay_ms(p: usize) -> u32 {
    ms(DELAY_UNITS[p])
}

/// The first level whose value is ≤ `units` (the walk). The levels run from longest to shortest, so a value below every
/// level is past the shortest one and reads as the last level, the slider's short (fast) end.
fn walk(levels: &[f64], units: f64) -> usize {
    levels
        .iter()
        .position(|l| *l <= units)
        .unwrap_or(levels.len() - 1)
}

/// A stored value in units. `keyboard.conf` keeps whole milliseconds, rounded from the units the slider wrote (83 ms for
/// 5 units is 4.98 units), so half a millisecond is added back before the walk (Original: the design stores units).
fn units(ms: u32) -> f64 {
    (f64::from(ms) + 0.5) / UNIT_MS
}

/// The rate position of a stored state: 0 when the delay is 0 (repeat off), else 1 + the walk of the interval.
pub fn rate_position(delay_ms: u32, interval_ms: u32) -> usize {
    if delay_ms == 0 {
        0
    } else {
        1 + walk(&RATE_UNITS, units(interval_ms))
    }
}

/// The delay position of a stored delay: the walk, clamped into the slider's 1…6 (approximate clamp, §6.3). A delay
/// longer than the longest step shows at the Long end, one shorter than the shortest step at the Short end.
pub fn delay_position(delay_ms: u32) -> usize {
    walk(&DELAY_UNITS, units(delay_ms)).clamp(DELAY_FIRST, DELAY_LAST)
}

/// What a rate position writes: `(repeat_delay, repeat_interval)`; position 0 sets the delay to 0, the server's
/// switch-off value, and keeps the interval.
pub fn rate_write(p: usize, delay_position: usize, interval_ms: u32) -> (u32, u32) {
    if p == 0 {
        (0, interval_ms)
    } else {
        (delay_ms(delay_position), rate_interval_ms(p))
    }
}

/// Choices of a timer pop-up: a list of durations in seconds (`None`: never) with their labels.
pub struct TimerChoices {
    pub labels: Vec<String>,
    /// The duration of each listed label; an extra last label (a stored value outside the list) has none.
    pub values: Vec<Option<u64>>,
    pub selected: usize,
}

fn duration_label(seconds: u64) -> String {
    match seconds {
        0 => "Immediately".to_string(),
        s if s < 60 => format!("{s} seconds"),
        60 => "1 minute".to_string(),
        s if s < 3600 || s % 3600 != 0 => format!("{} minutes", s / 60),
        3600 => "1 hour".to_string(),
        s => format!("{} hours", s / 3600),
    }
}

/// The choices of `listed` (seconds, `None` for Never) with `stored` selected; a stored value outside the list shows as
/// an extra selected item (§6.3, ST-18), and `never` shows only when listed or stored.
pub fn timer_choices(listed: &[Option<u64>], stored: Option<u64>) -> TimerChoices {
    let mut values: Vec<Option<u64>> = listed.to_vec();
    let mut labels: Vec<String> = values
        .iter()
        .map(|v| v.map_or_else(|| "Never".to_string(), duration_label))
        .collect();
    let selected = match values.iter().position(|v| *v == stored) {
        Some(i) => i,
        None => {
            labels.push(stored.map_or_else(|| "Never".to_string(), duration_label));
            values.push(stored);
            values.len() - 1
        }
    };
    TimerChoices {
        labels,
        values,
        selected,
    }
}

/// Display and system sleep: 1, 2, 3, 5, 10, 20, 30, 60, 90, 120, 150, 180 minutes and Never.
pub const SLEEP_CHOICES: [Option<u64>; 13] = [
    Some(60),
    Some(120),
    Some(180),
    Some(300),
    Some(600),
    Some(1200),
    Some(1800),
    Some(3600),
    Some(5400),
    Some(7200),
    Some(9000),
    Some(10800),
    None,
];

/// Require the password after display sleep: Immediately, 5 seconds, 1, 5, 15 minutes, 1, 4, 8 hours; `never` is not offered.
pub const PASSWORD_CHOICES: [Option<u64>; 8] = [
    Some(0),
    Some(5),
    Some(60),
    Some(300),
    Some(900),
    Some(3600),
    Some(14400),
    Some(28800),
];
