//! File names of screenshots: `Screenshot <date> at <time>.png` (pattern), with the date as
//! `YYYY-MM-dd` and the time as `h-mm-ss AM` from the design's single format `YYYY-MM-dd--h-mm-ss-a`, split at its
//! `--` into the two parts the pattern takes (split; the `- before the day period becoming a space is
//! approximate). `YYYY` is the calendar year here; the design's format letter is the week-based year, which differs
//! only in the last and first days of a year (approximate).

use std::path::{Path, PathBuf};

use crate::values::FILE_PREFIX;

/// A local wall-clock time, as the name shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    pub day: u32,
    /// 0 to 23.
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// The name without its extension, for example `Screenshot 2026-10-25 at 3-04-05 PM`.
pub fn base_name(t: &LocalTime) -> String {
    let hour12 = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    let period = if t.hour < 12 { "AM" } else { "PM" };
    format!(
        "{FILE_PREFIX} {:04}-{:02}-{:02} at {hour12}-{:02}-{:02} {period}",
        t.year, t.month, t.day, t.minute, t.second
    )
}

/// The first free path for `base` in `dir`: `<base>.png`, then `<base> (2).png`, `<base> (3).png` and so on, the
/// numbering being approximate. Several screens captured at once get one name each this way. `exists` tells whether a
/// path is taken.
pub fn free_path(dir: &Path, base: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{base}.png"));
    if !exists(&first) {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{base} ({n}).png")))
        .find(|p| !exists(p))
        .expect("some number is free")
}
