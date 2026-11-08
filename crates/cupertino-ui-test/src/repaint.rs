//! What each frame of an interaction costs: the wall time from the input to the presented frame (event, update, view,
//! layout, paint, raster and the copy of the damage), the pixels it repainted, and whether it repainted the whole
//! window. A [`FrameMeter`] steps a [`TestDriver`] one frame period at a time; a [`Summary`] gives the percentiles and
//! the frames over a budget, so a slow full repaint is never hidden behind an average.

use crate::{HeadlessWindowSystem, TestDriver};
use cupertino_ui_core::Application;
use cupertino_ui_platform::{Timestamp, WindowId};
use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// The time one frame may take: half of a 60 Hz frame, which leaves the rest to the compositor.
pub const FRAME_BUDGET: Duration = Duration::from_millis(8);

/// The time between two frames the meter steps: one frame of a 60 Hz display.
pub const FRAME_PERIOD: Duration = Duration::from_nanos(16_666_667);

/// The runtime turns one step may take before the meter gives up on it.
const STEP_TURNS: u32 = 256;

/// One presented frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSample {
    /// The wall time of the step that presented it.
    pub nanos: u64,
    /// The physical pixels it repainted.
    pub damaged: u64,
    /// The window's physical pixels.
    pub area: u64,
    /// How many rectangles it repainted.
    pub rects: usize,
    /// It repainted the whole window.
    pub full: bool,
}

/// Steps a driven window one frame period at a time and keeps a [`FrameSample`] of each frame it presents.
pub struct FrameMeter {
    window: WindowId,
    now: Timestamp,
    samples: Vec<FrameSample>,
}

impl FrameMeter {
    /// Meters `window` from `now`, the time the driver's clock shows.
    pub fn new(window: WindowId, now: Timestamp) -> FrameMeter {
        FrameMeter {
            window,
            now,
            samples: Vec::new(),
        }
    }

    /// The time of the last step.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// Moves the clock one frame period on, lets `input` queue the period's input, and answers each frame the
    /// window asks for until the runtime is idle. Everything after `input` is timed. Returns the frame it presented,
    /// if any; two frames in one step are taken as one, with their damage added.
    pub fn step<A: Application>(
        &mut self,
        driver: &mut TestDriver<A>,
        input: impl FnOnce(&mut HeadlessWindowSystem, Timestamp),
    ) -> Option<FrameSample> {
        self.now = Timestamp(self.now.0 + FRAME_PERIOD.as_nanos() as u64);
        driver
            .advance_to(self.now)
            .expect("the meter's clock only moves on");
        input(driver.window_system_mut(), self.now);
        let before = driver
            .runtime()
            .window_system()
            .frames(self.window)
            .map_or(0, <[_]>::len);
        let turns = NonZeroU32::new(STEP_TURNS).expect("not zero");
        let start = Instant::now();
        for _ in 0..4 {
            let system = driver.window_system_mut();
            if system.frame_requested(self.window).unwrap_or(false) {
                system
                    .frame_ready(self.window, self.now)
                    .expect("a frame was asked for");
            }
            driver.run_until_idle(turns).expect("the step runs");
            let system = driver.runtime().window_system();
            let presented = system.frames(self.window).map_or(0, <[_]>::len) > before;
            if presented || !system.frame_requested(self.window).unwrap_or(false) {
                break;
            }
        }
        let nanos = start.elapsed().as_nanos() as u64;
        let frames = driver.runtime().window_system().frames(self.window).ok()?;
        let new = frames.get(before..).filter(|f| !f.is_empty())?;
        let mut sample = FrameSample {
            nanos,
            damaged: 0,
            area: 0,
            rects: 0,
            full: false,
        };
        for frame in new {
            let area = u64::from(frame.width) * u64::from(frame.height);
            let damaged: u64 = frame
                .damage
                .iter()
                .map(|r| u64::from(r.width) * u64::from(r.height))
                .sum();
            sample.area = area;
            sample.damaged += damaged;
            sample.rects += frame.damage.len();
            sample.full |= damaged >= area;
        }
        sample.damaged = sample.damaged.min(sample.area);
        self.samples.push(sample);
        Some(sample)
    }

    /// The frames since the last [`take`](Self::take).
    pub fn samples(&self) -> &[FrameSample] {
        &self.samples
    }

    /// The frames since the last call, which starts a new interaction.
    pub fn take(&mut self) -> Vec<FrameSample> {
        std::mem::take(&mut self.samples)
    }
}

/// The distribution of an interaction's frame times and damage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Frames that took longer than [`FRAME_BUDGET`].
    pub over_budget: usize,
    /// Frames that repainted the whole window.
    pub full: usize,
    /// The median and largest share of the window a frame repainted, from 0 to 1.
    pub p50_damage: f64,
    pub max_damage: f64,
    /// The window's physical pixels.
    pub area: u64,
    /// The wall time per repainted pixel over all frames, in nanoseconds.
    pub ns_per_px: f64,
}

/// The value at quantile `q` of sorted `values`, by the nearest rank.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

impl Summary {
    pub fn of(samples: &[FrameSample]) -> Summary {
        let mut ms: Vec<f64> = samples.iter().map(|s| s.nanos as f64 / 1e6).collect();
        ms.sort_by(f64::total_cmp);
        let area = samples.iter().map(|s| s.area).max().unwrap_or(0);
        let mut share: Vec<f64> = samples
            .iter()
            .map(|s| s.damaged as f64 / s.area.max(1) as f64)
            .collect();
        share.sort_by(f64::total_cmp);
        let nanos: u64 = samples.iter().map(|s| s.nanos).sum();
        let damaged: u64 = samples.iter().map(|s| s.damaged).sum();
        let budget = FRAME_BUDGET.as_nanos() as u64;
        Summary {
            frames: samples.len(),
            p50_ms: quantile(&ms, 0.5),
            p95_ms: quantile(&ms, 0.95),
            p99_ms: quantile(&ms, 0.99),
            max_ms: ms.last().copied().unwrap_or(0.0),
            over_budget: samples.iter().filter(|s| s.nanos > budget).count(),
            full: samples.iter().filter(|s| s.full).count(),
            p50_damage: quantile(&share, 0.5),
            max_damage: share.last().copied().unwrap_or(0.0),
            area,
            ns_per_px: if damaged == 0 {
                0.0
            } else {
                nanos as f64 / damaged as f64
            },
        }
    }

    /// The header of [`Self::markdown_row`]'s table.
    pub fn markdown_header() -> &'static str {
        "| interaction | frames | p50 ms | p95 ms | p99 ms | max ms | > 8 ms | full | damage p50 | damage max | ns/px |\n\
         |---|---|---|---|---|---|---|---|---|---|---|"
    }

    /// One row of a markdown table, named `name`.
    pub fn markdown_row(&self, name: &str) -> String {
        format!(
            "| {name} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {:.1}% | {:.1}% | {:.2} |",
            self.frames,
            self.p50_ms,
            self.p95_ms,
            self.p99_ms,
            self.max_ms,
            self.over_budget,
            self.full,
            self.p50_damage * 100.0,
            self.max_damage * 100.0,
            self.ns_per_px,
        )
    }
}

/// Every frame of each named interaction as CSV: interaction, frame, milliseconds, damaged and window pixels,
/// rectangles and whether the frame was whole.
pub fn csv<'a>(runs: impl IntoIterator<Item = (&'a str, &'a [FrameSample])>) -> String {
    let mut out = String::from("interaction,frame,ms,damaged_px,window_px,rects,full\n");
    for (name, samples) in runs {
        for (i, s) in samples.iter().enumerate() {
            let _ = writeln!(
                out,
                "{name},{i},{:.3},{},{},{},{}",
                s.nanos as f64 / 1e6,
                s.damaged,
                s.area,
                s.rects,
                s.full
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: u64, damaged: u64) -> FrameSample {
        FrameSample {
            nanos: ms * 1_000_000,
            damaged,
            area: 100,
            rects: 1,
            full: damaged == 100,
        }
    }

    #[test]
    fn the_summary_counts_every_slow_and_whole_frame_and_takes_ranks_not_averages() {
        let mut samples: Vec<FrameSample> = (1..=99).map(|_| sample(1, 10)).collect();
        samples.push(sample(30, 100));
        let summary = Summary::of(&samples);
        assert_eq!(summary.frames, 100);
        assert_eq!(summary.p50_ms, 1.0);
        assert_eq!(summary.p99_ms, 1.0);
        assert_eq!(summary.max_ms, 30.0);
        assert_eq!(summary.over_budget, 1);
        assert_eq!(summary.full, 1);
        assert_eq!(summary.p50_damage, 0.1);
        assert_eq!(summary.max_damage, 1.0);
    }

    #[test]
    fn an_empty_interaction_summarises_to_nothing() {
        let summary = Summary::of(&[]);
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.max_ms, 0.0);
        assert_eq!(summary.ns_per_px, 0.0);
    }

    #[test]
    fn the_csv_has_a_line_per_frame() {
        let samples = [sample(2, 10), sample(3, 100)];
        let out = csv([("scroll", &samples[..])]);
        assert_eq!(
            out,
            "interaction,frame,ms,damaged_px,window_px,rects,full\n\
             scroll,0,2.000,10,100,1,false\n\
             scroll,1,3.000,100,100,1,true\n"
        );
    }
}
