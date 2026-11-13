//! What the DRM backend does with each frame, logged on request (`CUPERTINO_DRM_TRACE=1`): which presentation time a
//! flip reports (the driver's flip timestamp, or the server's own clock when the driver gives none), flips shown late
//! (missed refreshes), which planes a frame used (the cursor plane, direct scanout of a client buffer on the primary
//! plane, overlay planes) and the time from an input to the flip that showed its first frame.
//!
//! The lines go to the server's log (`~/.local/state/cupertino/server.log` on the TTY) under the target
//! `cupertino_server::drm_trace`, at the info level the default filter keeps. Without the variable nothing is kept and
//! nothing is logged: the backend only looks whether [`DrmTrace`] is there.

use std::time::{Duration, Instant};

/// Turns the trace on when set to anything but empty or `0`.
pub const ENV: &str = "CUPERTINO_DRM_TRACE";

/// Whether the trace was asked for, from the variable's value.
pub fn requested(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Where a flip's presentation time came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// The driver's timestamp of the flip (`CLOCK_MONOTONIC`).
    FlipTimestamp,
    /// The server's clock when the flip event arrived: the driver gave no usable timestamp.
    OwnClock,
}

impl Branch {
    pub fn name(self) -> &'static str {
        match self {
            Branch::FlipTimestamp => "flip-timestamp",
            Branch::OwnClock => "own-clock",
        }
    }
}

/// The planes a frame used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Planes {
    /// A client buffer was scanned out on the primary plane instead of a composed frame.
    pub direct_scanout: bool,
    /// The cursor was on the cursor plane.
    pub cursor_plane: bool,
    /// Elements on overlay planes.
    pub overlays: usize,
}

/// The trace of one server run.
#[derive(Debug, Default)]
pub struct DrmTrace {
    /// The earliest input no queued frame has carried yet.
    input: Option<Instant>,
    /// The last branch and planes logged, by output name, so only changes are logged.
    branches: Vec<(String, Branch)>,
    planes: Vec<(String, Planes)>,
    pub flips: u64,
    pub missed: u64,
}

impl DrmTrace {
    pub fn new() -> DrmTrace {
        DrmTrace::default()
    }

    /// An input arrived at `at`.
    pub fn input(&mut self, at: Instant) {
        self.input.get_or_insert(at);
    }

    /// A frame is being queued: it carries the earliest input not shown yet, if any.
    pub fn queued(&mut self) -> Option<Instant> {
        self.input.take()
    }

    /// The planes of a frame of `output`: the line to log when they differ from the last frame's.
    pub fn planes(&mut self, output: &str, planes: Planes) -> Option<String> {
        changed(&mut self.planes, output, planes).then(|| {
            format!(
                "planes output={output} direct_scanout={} cursor_plane={} overlays={}",
                planes.direct_scanout, planes.cursor_plane, planes.overlays
            )
        })
    }

    /// A flip of `output` was shown, its time from `branch`, `late_periods` refreshes after it was due; it carried the
    /// input of `input`, shown at `flip` (on the same clock). The lines to log: the branch when it changed, a missed
    /// flip, the input-to-flip latency.
    pub fn flipped(
        &mut self,
        output: &str,
        branch: Branch,
        late_periods: u32,
        input: Option<Instant>,
        flip: Instant,
    ) -> Vec<String> {
        self.flips += 1;
        let mut lines = Vec::new();
        if changed(&mut self.branches, output, branch) {
            lines.push(format!(
                "presentation-time output={output} branch={}",
                branch.name()
            ));
        }
        if late_periods > 0 {
            self.missed += u64::from(late_periods);
            lines.push(format!(
                "missed output={output} late_periods={late_periods} flips={} missed_total={}",
                self.flips, self.missed
            ));
        }
        if let Some(input) = input {
            let latency = flip.saturating_duration_since(input);
            lines.push(format!(
                "input-to-flip output={output} latency_us={}",
                latency.as_micros()
            ));
        }
        lines
    }
}

/// Records `value` for `output`; whether it differs from the one recorded before.
fn changed<T: PartialEq + Copy>(seen: &mut Vec<(String, T)>, output: &str, value: T) -> bool {
    match seen.iter_mut().find(|(name, _)| name == output) {
        Some((_, last)) if *last == value => false,
        Some((_, last)) => {
            *last = value;
            true
        }
        None => {
            seen.push((output.to_owned(), value));
            true
        }
    }
}

/// The `Instant` of a `CLOCK_MONOTONIC` time `t`, where `now` is the same moment as `now_mono`.
pub fn instant_of(t: Duration, now: Instant, now_mono: Duration) -> Instant {
    if t <= now_mono {
        now.checked_sub(now_mono - t).unwrap_or(now)
    } else {
        now + (t - now_mono)
    }
}

/// Logs `line` to the trace's target.
pub fn log(line: &str) {
    tracing::info!(target: "cupertino_server::drm_trace", "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_is_on_for_any_value_but_empty_or_zero() {
        assert!(!requested(None));
        assert!(!requested(Some("")));
        assert!(!requested(Some("0")));
        assert!(requested(Some("1")));
        assert!(requested(Some("yes")));
    }

    #[test]
    fn the_branch_and_planes_are_logged_when_they_change_per_output() {
        let mut t = DrmTrace::new();
        let now = Instant::now();
        let first = t.flipped("DP-1", Branch::FlipTimestamp, 0, None, now);
        assert_eq!(
            first,
            ["presentation-time output=DP-1 branch=flip-timestamp"]
        );
        assert!(t
            .flipped("DP-1", Branch::FlipTimestamp, 0, None, now)
            .is_empty());
        assert_eq!(t.flipped("DP-2", Branch::OwnClock, 0, None, now).len(), 1);
        assert_eq!(t.flipped("DP-1", Branch::OwnClock, 0, None, now).len(), 1);
        let planes = Planes {
            cursor_plane: true,
            ..Planes::default()
        };
        assert_eq!(
            t.planes("DP-1", planes).as_deref(),
            Some("planes output=DP-1 direct_scanout=false cursor_plane=true overlays=0")
        );
        assert_eq!(t.planes("DP-1", planes), None);
        assert!(t.planes("DP-1", Planes::default()).is_some());
    }

    #[test]
    fn an_input_ends_at_the_flip_of_the_first_frame_queued_after_it_and_late_flips_count_as_missed()
    {
        let mut t = DrmTrace::new();
        let t0 = Instant::now();
        assert_eq!(t.queued(), None);
        t.input(t0);
        t.input(t0 + Duration::from_millis(3));
        let carried = t.queued();
        assert_eq!(carried, Some(t0), "the earliest input");
        assert_eq!(t.queued(), None, "a later frame carries none");
        let lines = t.flipped(
            "DP-1",
            Branch::FlipTimestamp,
            2,
            carried,
            t0 + Duration::from_micros(21_500),
        );
        assert_eq!(
            lines[1..],
            [
                "missed output=DP-1 late_periods=2 flips=1 missed_total=2".to_owned(),
                "input-to-flip output=DP-1 latency_us=21500".to_owned()
            ]
        );
        assert_eq!((t.flips, t.missed), (1, 2));
    }

    #[test]
    fn a_monotonic_time_maps_onto_instants_either_side_of_now() {
        let now = Instant::now();
        let mono = Duration::from_secs(100);
        assert_eq!(
            instant_of(Duration::from_secs(99), now, mono),
            now - Duration::from_secs(1)
        );
        assert_eq!(
            instant_of(Duration::from_secs(101), now, mono),
            now + Duration::from_secs(1)
        );
    }
}
