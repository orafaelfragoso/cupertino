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

/// Dock size and magnification: the slider value v in 0…1 is `(px − 16) / 112`.
pub const DOCK_MIN_PX: f64 = 16.0;
pub const DOCK_SPAN_PX: f64 = 112.0;
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

/// The magnification slider at `v`: off at 0, else on with the magnified size of `v` (mapping). The
/// size is clamped by the dock's own rule when it applies it.
pub fn magnification(v: f64) -> (bool, Option<f64>) {
    if v <= 0.0 {
        (false, None)
    } else {
        (true, Some(dock_px(v)))
    }
}

/// The magnification slider's value for a stored state.
pub fn magnification_value(on: bool, large_px: f64) -> f64 {
    if on {
        dock_value(large_px)
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

/// The first level whose value is ≤ `units`, 0 when none is (the walk).
fn walk(levels: &[f64], units: f64) -> usize {
    levels.iter().position(|l| *l <= units).unwrap_or(0)
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

/// The delay position of a stored delay: the walk, clamped into the slider's 1…6 (approximate clamp, §6.3).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_stops_round_trip_and_the_default_is_stop_three() {
        for (i, v) in TRACKING_STOPS.iter().enumerate() {
            assert_eq!(tracking_stop(*v), i);
        }
        assert_eq!(tracking_stop(TRACKING_DEFAULT), 3);
        assert_eq!(
            tracking_stop(0.7),
            3,
            "a stored value between stops shows the nearest"
        );
    }

    #[test]
    fn the_dock_sliders_map_pixels_and_marks() {
        assert_eq!(
            (dock_px(0.0), dock_px(1.0), dock_px(0.5)),
            (16.0, 128.0, 72.0)
        );
        assert_eq!(
            (dock_value(16.0), dock_value(128.0), dock_value(48.0)),
            (0.0, 1.0, 32.0 / 112.0)
        );
        for px in [16.0, 48.0, 64.0, 128.0] {
            assert_eq!(dock_px(dock_value(px)), px);
        }
        assert_eq!(magnification(0.0), (false, None));
        assert_eq!(magnification(0.125), (true, Some(30.0)), "the Small mark");
        assert_eq!(magnification(1.0), (true, Some(128.0)));
        assert_eq!(magnification_value(false, 128.0), 0.0);
        assert_eq!(magnification_value(true, 128.0), 1.0);
    }

    #[test]
    fn rate_positions_write_the_reference_intervals_and_off() {
        let intervals: Vec<u32> = (1..RATE_POSITIONS).map(rate_interval_ms).collect();
        assert_eq!(intervals, [2000, 1500, 1000, 500, 200, 83, 33]);
        assert_eq!(rate_write(0, 4, 83), (0, 83), "off keeps the interval");
        assert_eq!(rate_write(6, 4, 0), (500, 83));
    }

    #[test]
    fn delay_positions_write_the_reference_delays() {
        let delays: Vec<u32> = (DELAY_FIRST..=DELAY_LAST).map(delay_ms).collect();
        assert_eq!(delays, [2000, 1567, 1133, 500, 417, 250]);
    }

    #[test]
    fn reading_walks_to_the_first_level_at_or_below_the_value() {
        assert_eq!(
            (rate_position(500, 83), delay_position(500)),
            (RATE_DEFAULT, DELAY_DEFAULT),
            "the defaults"
        );
        assert_eq!(rate_position(500, 90), 6, "90 ms = 5.4 units");
        assert_eq!(rate_position(500, 40), 7, "40 ms = 2.4 units");
        assert_eq!(rate_position(0, 83), 0, "a delay of 0 is repeat off");
        assert_eq!(delay_position(600), 4, "600 ms = 36 units");
        assert_eq!(
            delay_position(100),
            DELAY_FIRST,
            "no level qualifies: the walk's 0 clamps to 1"
        );
        assert_eq!(
            delay_position(5_000_000),
            DELAY_FIRST,
            "level 0 is not on the slider"
        );
        for p in 1..RATE_POSITIONS {
            assert_eq!(
                rate_position(500, rate_interval_ms(p)),
                p,
                "every written interval reads back as its position"
            );
        }
        for p in DELAY_FIRST..=DELAY_LAST {
            assert_eq!(delay_position(delay_ms(p)), p);
        }
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

#[cfg(test)]
mod timer_tests {
    use super::*;

    #[test]
    fn sleep_minutes_and_seconds_round_trip_and_an_unlisted_value_is_an_extra_item() {
        let c = timer_choices(&SLEEP_CHOICES, Some(600));
        assert_eq!(
            (c.labels[c.selected].as_str(), c.labels.len()),
            ("10 minutes", 13)
        );
        assert_eq!(c.labels[0], "1 minute");
        assert_eq!(c.labels[7], "1 hour");
        assert_eq!(c.labels[8], "90 minutes");
        assert_eq!(c.labels[11], "3 hours");
        assert_eq!(c.labels[12], "Never");
        let seven = timer_choices(&SLEEP_CHOICES, Some(420));
        assert_eq!(
            (seven.selected, seven.labels[13].as_str(), seven.values[13]),
            (13, "7 minutes", Some(420)),
            "7 minutes is kept as an extra item"
        );
        let never = timer_choices(&PASSWORD_CHOICES, None);
        assert_eq!(
            (never.labels.len(), never.labels[8].as_str()),
            (9, "Never"),
            "never shows only when stored"
        );
        let five = timer_choices(&PASSWORD_CHOICES, Some(300));
        assert_eq!(five.labels[five.selected], "5 minutes", "the default 300 s");
        assert_eq!(
            timer_choices(&PASSWORD_CHOICES, Some(0)).labels[0],
            "Immediately"
        );
    }
}
