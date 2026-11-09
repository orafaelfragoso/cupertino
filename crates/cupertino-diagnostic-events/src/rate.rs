//! The per-id rate limit: at most [`RATE_PER_ID`] events of one id in a [`RATE_WINDOW`]; the rest are counted and written
//! as one summary when the window closes (or the process ends).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::event::Record;

/// How many events of one id a window writes.
pub const RATE_PER_ID: usize = 10;
/// How long a window lasts, from the first event of the id in it.
pub const RATE_WINDOW: Duration = Duration::from_secs(10);

struct Window {
    start: Instant,
    written: usize,
    suppressed: u64,
    /// The last suppressed record: the summary carries its fields and time.
    last: Option<Record>,
}

/// The windows of every id a process has used.
pub struct RateLimiter {
    per_id: usize,
    window: Duration,
    ids: HashMap<String, Window>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        RateLimiter::new(RATE_PER_ID, RATE_WINDOW)
    }
}

impl RateLimiter {
    pub fn new(per_id: usize, window: Duration) -> Self {
        RateLimiter {
            per_id,
            window,
            ids: HashMap::new(),
        }
    }

    /// Offers `record` at `now`: what to write, in order. That is the summary of the id's last window when it closed,
    /// then the record when its window has room; nothing when it is suppressed.
    pub fn offer(&mut self, record: Record, now: Instant) -> Vec<Record> {
        let mut out = Vec::new();
        let window = self.window;
        let entry = self.ids.entry(record.id.clone()).or_insert(Window {
            start: now,
            written: 0,
            suppressed: 0,
            last: None,
        });
        if now.saturating_duration_since(entry.start) >= window {
            out.extend(summary(entry));
            *entry = Window {
                start: now,
                written: 0,
                suppressed: 0,
                last: None,
            };
        }
        if entry.written < self.per_id {
            entry.written += 1;
            out.push(record);
        } else {
            entry.suppressed += 1;
            entry.last = Some(record);
        }
        out
    }

    /// The summaries of the windows that closed by `now`.
    pub fn due(&mut self, now: Instant) -> Vec<Record> {
        let window = self.window;
        let mut out = Vec::new();
        for entry in self.ids.values_mut() {
            if entry.suppressed > 0 && now.saturating_duration_since(entry.start) >= window {
                out.extend(summary(entry));
            }
        }
        out
    }

    /// Every summary still held, for the end of the process.
    pub fn drain(&mut self) -> Vec<Record> {
        self.ids.values_mut().filter_map(summary).collect()
    }

    /// When the next window with suppressed events closes.
    pub fn next_due(&self) -> Option<Instant> {
        self.ids
            .values()
            .filter(|w| w.suppressed > 0)
            .map(|w| w.start + self.window)
            .min()
    }
}

fn summary(window: &mut Window) -> Option<Record> {
    let mut last = window.last.take()?;
    last.suppressed = std::mem::take(&mut window.suppressed);
    Some(last)
}
