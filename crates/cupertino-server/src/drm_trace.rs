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
///
/// Each output reports its own input-to-flip latency: an input is waiting on every output until a frame of that output
/// is queued, which takes it; the first flip of that output that shows that frame or a later one reports it. A frame
/// whose flip never comes (refused, dropped with its CRTC's state, or parked behind a failed one) leaves its input to the
/// next flip of the output that does.
#[derive(Debug, Default)]
pub struct DrmTrace {
    /// By output name, the earliest input no queued frame of the output has taken yet.
    waiting: Vec<(String, Option<Instant>)>,
    /// The inputs queued frames took, by output and the frame's [`DrmTrace::queued`] number, not reported yet.
    in_flight: Vec<(String, u64, Instant)>,
    /// The number of the last frame queued.
    frames: u64,
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

    /// An input arrived at `at`: it waits on every output for the output's next frame.
    pub fn input(&mut self, at: Instant) {
        for (_, waiting) in &mut self.waiting {
            waiting.get_or_insert(at);
        }
    }

    /// The earliest input waiting on `output`'s next frame.
    pub fn waiting(&self, output: &str) -> Option<Instant> {
        self.waiting
            .iter()
            .find(|(name, _)| name == output)
            .and_then(|(_, at)| *at)
    }

    /// A frame of `output` is being queued: it takes the input waiting on the output, if any. Its number, for
    /// [`DrmTrace::flipped`].
    pub fn queued(&mut self, output: &str) -> u64 {
        self.frames += 1;
        let taken = match self.waiting.iter_mut().find(|(name, _)| name == output) {
            Some((_, waiting)) => waiting.take(),
            None => {
                // an output starts waiting for inputs at its first frame
                self.waiting.push((output.to_owned(), None));
                None
            }
        };
        if let Some(at) = taken {
            self.in_flight.push((output.to_owned(), self.frames, at));
        }
        self.frames
    }

    /// The earliest input a flip of `output` showing the frame numbered `frame` shows first: the input of that frame or
    /// of any earlier frame of the output whose flip never came.
    fn shown_input(&mut self, output: &str, frame: u64) -> Option<Instant> {
        let mut earliest: Option<Instant> = None;
        self.in_flight.retain(|(name, number, at)| {
            let shown = name == output && *number <= frame;
            if shown {
                earliest = Some(earliest.map_or(*at, |e| e.min(*at)));
            }
            !shown
        });
        earliest
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

    /// A flip of `output` showing the frame numbered `frame` (from [`DrmTrace::queued`]) was shown at `flip`, its time
    /// from `branch`, `late_periods` refreshes after it was due. The lines to log: the branch when it changed, a missed
    /// flip, the latency from the earliest input it is the first to show.
    pub fn flipped(
        &mut self,
        output: &str,
        branch: Branch,
        late_periods: u32,
        frame: u64,
        flip: Instant,
    ) -> Vec<String> {
        let input = self.shown_input(output, frame);
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
        let first = t.flipped("DP-1", Branch::FlipTimestamp, 0, 0, now);
        assert_eq!(
            first,
            ["presentation-time output=DP-1 branch=flip-timestamp"]
        );
        assert!(t
            .flipped("DP-1", Branch::FlipTimestamp, 0, 0, now)
            .is_empty());
        assert_eq!(t.flipped("DP-2", Branch::OwnClock, 0, 0, now).len(), 1);
        assert_eq!(t.flipped("DP-1", Branch::OwnClock, 0, 0, now).len(), 1);
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

    fn us(n: u64) -> Duration {
        Duration::from_micros(n)
    }

    #[test]
    fn an_input_ends_at_the_flip_of_the_first_frame_queued_after_it_and_late_flips_count_as_missed()
    {
        let mut t = DrmTrace::new();
        let t0 = Instant::now();
        t.queued("DP-1");
        let before = t.flipped("DP-1", Branch::FlipTimestamp, 0, 1, t0);
        assert_eq!(before.len(), 1, "the branch only: no input yet");
        t.input(t0);
        t.input(t0 + us(3_000));
        assert_eq!(t.waiting("DP-1"), Some(t0), "the earliest input");
        let frame = t.queued("DP-1");
        assert_eq!(t.waiting("DP-1"), None, "the frame took it");
        let later = t.queued("DP-1");
        let lines = t.flipped("DP-1", Branch::FlipTimestamp, 2, frame, t0 + us(21_500));
        assert_eq!(
            lines,
            [
                "missed output=DP-1 late_periods=2 flips=2 missed_total=2".to_owned(),
                "input-to-flip output=DP-1 latency_us=21500".to_owned()
            ]
        );
        assert!(
            t.flipped("DP-1", Branch::FlipTimestamp, 0, later, t0 + us(30_000))
                .is_empty(),
            "a later frame carries none"
        );
        assert_eq!((t.flips, t.missed), (3, 2));
    }

    #[test]
    fn each_output_reports_the_input_at_its_own_first_flip_after_it() {
        let mut t = DrmTrace::new();
        let t0 = Instant::now();
        for output in ["DP-1", "DP-2"] {
            let frame = t.queued(output);
            t.flipped(output, Branch::FlipTimestamp, 0, frame, t0);
        }
        t.input(t0);
        // the first output queues first; the second, where the input shows, queues later and still reports it
        let first = t.queued("DP-1");
        let second = t.queued("DP-2");
        assert_eq!(
            t.flipped("DP-1", Branch::FlipTimestamp, 0, first, t0 + us(8_000)),
            ["input-to-flip output=DP-1 latency_us=8000"]
        );
        assert_eq!(
            t.flipped("DP-2", Branch::FlipTimestamp, 0, second, t0 + us(12_000)),
            ["input-to-flip output=DP-2 latency_us=12000"]
        );
    }

    #[test]
    fn an_input_carried_by_a_dropped_flip_is_reported_at_the_next_flip_of_its_output() {
        let mut t = DrmTrace::new();
        let t0 = Instant::now();
        let shown_before = t.queued("DP-1");
        t.input(t0);
        // this frame's flip never comes
        let _dropped = t.queued("DP-1");
        // a flip of a frame queued before the input does not show it
        assert!(t
            .flipped("DP-1", Branch::OwnClock, 0, shown_before, t0 + us(1_000))
            .iter()
            .all(|line| !line.starts_with("input-to-flip")));
        let next = t.queued("DP-1");
        assert_eq!(
            t.flipped("DP-1", Branch::OwnClock, 0, next, t0 + us(40_000)),
            ["input-to-flip output=DP-1 latency_us=40000"]
        );
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
