//! Per-frame timings of an offscreen output, for the frame-rate benchmark (`--frame-stats PATH`, `scripts/dev/bench`).
//!
//! Each frame drawn for presentation writes one line: when it was shown, how long composing the scene and rendering it
//! took (the GPU waited for, so the time covers the GPU's work too), how much of that the glass passes and their
//! backdrop blurs took, how many glass backdrops were blurred again or reused, and the input-to-present latency of the
//! frame when it is the first shown after an input.
//!
//! Latency is counted two ways. `input` ends at the first frame shown after the input: what the server itself moves (a
//! dragged window, the pointer) shows there. `app` ends at the first frame shown after a client committed in answer
//! to it: a scrolled list or a magnified dock shows there. A commit later than [`APP_ANSWER_WITHIN`] is not taken as an
//! answer.

use std::io::Write;
use std::time::{Duration, Instant};

/// The longest a client commit may follow an input and still count as its answer.
pub const APP_ANSWER_WITHIN: Duration = Duration::from_millis(250);

/// The first frame shown after an input, and the first after a client answered it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// The earliest input no frame has shown yet.
    input: Option<Instant>,
    /// The earliest input no client answer has shown yet.
    app: Option<Instant>,
    /// A client committed after [`Latency::app`].
    answered: bool,
}

impl Latency {
    /// An input arrived at `at`.
    pub fn input(&mut self, at: Instant) {
        self.input.get_or_insert(at);
        if self.app.is_none() {
            self.app = Some(at);
            self.answered = false;
        }
    }

    /// A client committed at `at`.
    pub fn commit(&mut self, at: Instant) {
        match self.app {
            Some(from) if at.saturating_duration_since(from) > APP_ANSWER_WITHIN => {
                self.app = None;
                self.answered = false;
            }
            Some(_) => self.answered = true,
            None => {}
        }
    }

    /// A frame was shown at `at`: the input latency and the client-answer latency it ends, if any.
    pub fn presented(&mut self, at: Instant) -> (Option<Duration>, Option<Duration>) {
        let input = self.input.take().map(|t| at.saturating_duration_since(t));
        let app = match self.app {
            Some(t) if self.answered => {
                self.app = None;
                self.answered = false;
                Some(at.saturating_duration_since(t))
            }
            _ => None,
        };
        (input, app)
    }
}

/// The time the glass passes took within one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlassTime {
    /// Capturing and blurring backdrops, GPU included.
    pub blur: Duration,
    /// The glass draws, blurs included.
    pub total: Duration,
}

/// One frame shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameRecord {
    /// When it was shown, from the server's start.
    pub shown: Duration,
    pub compose: Duration,
    pub render: Duration,
    pub glass: GlassTime,
    /// Glass backdrops blurred again and reused.
    pub blurred: u64,
    pub reused: u64,
    pub latency: (Option<Duration>, Option<Duration>),
}

impl FrameRecord {
    /// The record as one line: times in microseconds, `-` for a latency the frame does not end.
    pub fn line(&self) -> String {
        let us = |d: Duration| d.as_micros().to_string();
        let opt = |d: Option<Duration>| d.map_or_else(|| "-".into(), us);
        format!(
            "{} {} {} {} {} {} {} {} {}",
            us(self.shown),
            us(self.compose),
            us(self.render),
            us(self.glass.total),
            us(self.glass.blur),
            self.blurred,
            self.reused,
            opt(self.latency.0),
            opt(self.latency.1),
        )
    }
}

/// The recorder: the latency bookkeeping and the file the lines go to.
pub struct FrameStats {
    pub start: Instant,
    pub latency: Latency,
    /// The material counters when the frame being drawn began.
    before: crate::render::material::MaterialStats,
    out: std::io::BufWriter<std::fs::File>,
}

/// The first line of the file: the column names.
pub const HEADER: &str =
    "# shown_us compose_us render_us glass_us blur_us blurred reused input_lat_us app_lat_us";

impl FrameStats {
    /// Write to `path`, naming the device that renders (`renderer`) on the first line; times count from `start`.
    pub fn create(
        path: &std::path::Path,
        renderer: &str,
        refresh: Duration,
        start: Instant,
    ) -> std::io::Result<Self> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "# renderer {renderer}")?;
        writeln!(out, "# refresh_us {}", refresh.as_micros())?;
        writeln!(out, "{HEADER}")?;
        Ok(Self {
            start,
            latency: Latency::default(),
            before: Default::default(),
            out,
        })
    }

    pub fn write(&mut self, record: &FrameRecord) {
        let _ = writeln!(self.out, "{}", record.line());
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// Before a frame is composed: the glass passes time themselves from here on, and their counters are noted.
pub fn start_glass_timing(state: &mut crate::state::Cupertino) {
    let Some(stats) = state.frame_stats.as_mut() else {
        return;
    };
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        let mut m = material.borrow_mut();
        m.timing = true;
        m.glass_time = GlassTime::default();
        stats.before = m.stats;
    }
}

/// A frame was rendered and its GPU work finished: write its line.
pub fn frame_rendered(state: &mut crate::state::Cupertino, compose: Duration, render: Duration) {
    let material = state
        .render
        .as_ref()
        .and_then(|r| r.material.as_ref())
        .map(|m| {
            let m = m.borrow();
            (m.glass_time, m.stats)
        });
    let Some(stats) = state.frame_stats.as_mut() else {
        return;
    };
    let now = Instant::now();
    let (glass, counts) = material.unwrap_or((GlassTime::default(), stats.before));
    let record = FrameRecord {
        shown: now.saturating_duration_since(stats.start),
        compose,
        render,
        glass,
        blurred: counts.backdrop_misses - stats.before.backdrop_misses,
        reused: counts.backdrop_hits - stats.before.backdrop_hits,
        latency: stats.latency.presented(now),
    };
    stats.write(&record);
}

/// An input arrived (benchmark runs only).
pub fn note_input(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.input(Instant::now());
    }
}

/// A client committed (benchmark runs only).
pub fn note_commit(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.commit(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn an_input_ends_at_the_next_frame_and_its_answer_at_the_frame_after_the_commit() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.input(t0);
        l.input(t0 + ms(2));
        assert_eq!(l.presented(t0 + ms(5)), (Some(ms(5)), None));
        l.commit(t0 + ms(9));
        assert_eq!(l.presented(t0 + ms(13)), (None, Some(ms(13))));
        assert_eq!(l.presented(t0 + ms(20)), (None, None));
    }

    #[test]
    fn a_commit_long_after_an_input_is_not_its_answer() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.input(t0);
        l.presented(t0 + ms(1));
        l.commit(t0 + APP_ANSWER_WITHIN + ms(1));
        assert_eq!(l.presented(t0 + ms(400)), (None, None));
        l.input(t0 + ms(500));
        l.commit(t0 + ms(504));
        assert_eq!(l.presented(t0 + ms(508)), (Some(ms(8)), Some(ms(8))));
    }

    #[test]
    fn a_line_names_every_column() {
        let r = FrameRecord {
            shown: ms(1),
            latency: (Some(ms(3)), None),
            ..Default::default()
        };
        assert_eq!(
            r.line().split(' ').count(),
            HEADER.trim_start_matches("# ").split(' ').count()
        );
        assert!(r.line().ends_with("3000 -"));
    }
}
