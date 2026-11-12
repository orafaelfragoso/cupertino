//! The battery's charge over the last days: a sample every few minutes, kept for ten days and written to a file so a
//! restart keeps it. [`History`] is pure: it takes the time and the reading as arguments and encodes itself as text.

use std::path::Path;

use crate::supply::State;

/// How long a sample is kept, in seconds: ten days.
pub const RETENTION: i64 = 10 * 24 * 60 * 60;
/// The shortest time between two samples of an unchanged state, in seconds.
pub const INTERVAL: i64 = 5 * 60;

/// One reading: when, how full the battery was (0 to 100) and what it was doing (UPower's `State` value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub time: i64,
    pub percent: f64,
    pub state: u32,
}

/// The recorded samples, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    samples: Vec<Sample>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the reading `percent` in `state` at `now`, when it is due: the state changed or [`INTERVAL`] has passed
    /// since the last sample. Samples older than [`RETENTION`] go. Returns whether a sample was added.
    pub fn record(&mut self, now: i64, percent: f64, state: State) -> bool {
        let state = state as u32;
        let due = match self.samples.last() {
            Some(last) => now < last.time || last.state != state || now - last.time >= INTERVAL,
            None => true,
        };
        // a clock that went back makes older samples unreachable by time: keep only what is not newer than now
        self.samples
            .retain(|s| s.time <= now && now - s.time <= RETENTION);
        if !due {
            return false;
        }
        self.samples.push(Sample {
            time: now,
            percent: percent.clamp(0.0, 100.0),
            state,
        });
        true
    }

    /// The samples of the last `timespan` seconds before `now`, thinned to at most one per `resolution` seconds (the
    /// last of each slot); `resolution` 0 keeps them all.
    pub fn query(&self, now: i64, timespan: u32, resolution: u32) -> Vec<Sample> {
        let from = now - i64::from(timespan);
        let mut out: Vec<Sample> = Vec::new();
        for s in self.samples.iter().filter(|s| s.time >= from) {
            match out.last_mut() {
                Some(last)
                    if resolution > 0
                        && s.time / i64::from(resolution) == last.time / i64::from(resolution)
                        && s.state == last.state =>
                {
                    *last = *s
                }
                _ => out.push(*s),
            }
        }
        out
    }

    /// The history as the file holds it: a line `time percent state` for each sample.
    pub fn encode(&self) -> String {
        self.samples
            .iter()
            .map(|s| format!("{} {:.1} {}\n", s.time, s.percent, s.state))
            .collect()
    }

    /// The history of `text`; a line that is not a sample is skipped.
    pub fn decode(text: &str) -> Self {
        let samples = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split_whitespace();
                Some(Sample {
                    time: f.next()?.parse().ok()?,
                    percent: f.next()?.parse().ok()?,
                    state: f.next()?.parse().ok()?,
                })
            })
            .collect();
        Self { samples }
    }

    /// Read the file at `path`; a missing or unreadable file is an empty history.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::decode(&t))
            .unwrap_or_default()
    }

    /// Write the history to `path` through a temporary file beside it, so a crash leaves the old file whole.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = std::path::PathBuf::from(temporary);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&temporary, self.encode())?;
        std::fs::rename(&temporary, path)
    }
}
