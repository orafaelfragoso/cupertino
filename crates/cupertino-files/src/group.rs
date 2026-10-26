//! Grouping of a sorted listing into sections ("arrange by"): kind, date buckets, size buckets, tags.
//!
//! Kind groups follow the type table of the design. The date bucket names
//! (Today, Yesterday, Previous 7 Days, Previous 30 Days, then months and years) and the size
//! buckets are as designed / approximate: the code that builds them is not known.

use crate::entry::Entry;
use crate::mime::{self, KindGroup};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GroupBy {
    None,
    Kind,
    DateModified,
    DateCreated,
    DateAccessed,
    Size,
    Tags,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    /// Indexes into the entry slice, in the order of the slice.
    pub items: Vec<usize>,
}

/// Seconds-resolution calendar needed for the date buckets (local time through `localtime_r`).
fn local_ymd(t: SystemTime) -> (i32, u32, u32) {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tt = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&tt, &mut tm);
    }
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn day_number(y: i32, m: u32, d: u32) -> i64 {
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Bucket title and sort rank (lower = nearer to now) of a date relative to `now`.
pub fn date_bucket(t: Option<SystemTime>, now: SystemTime) -> (String, i64) {
    let Some(t) = t else {
        return ("Unknown".into(), i64::MAX);
    };
    let (ty, tm, td) = local_ymd(t);
    let (ny, nm, nd) = local_ymd(now);
    let age = day_number(ny, nm, nd) - day_number(ty, tm, td);
    match age {
        i64::MIN..=0 => ("Today".into(), 0),
        1 => ("Yesterday".into(), 1),
        2..=7 => ("Previous 7 Days".into(), 2),
        8..=30 => ("Previous 30 Days".into(), 3),
        _ if ty == ny => (
            MONTHS[tm as usize - 1].to_string(),
            100 + (nm as i64 - tm as i64),
        ),
        _ => (ty.to_string(), 1000 + (ny - ty) as i64),
    }
}

/// Size buckets (approximate boundaries).
pub fn size_bucket(size: u64) -> (&'static str, u8) {
    const KB: u64 = 1000;
    match size {
        0 => ("Zero bytes", 0),
        1..=9_999 => ("Tiny (< 10 KB)", 1),
        10_000..=999_999 => ("Small (< 1 MB)", 2),
        1_000_000..=99_999_999 => ("Medium (< 100 MB)", 3),
        _ if size < 1000 * KB * KB * 10 => ("Large (< 10 GB)", 4),
        _ => ("Huge (10 GB and more)", 5),
    }
}

/// Group `entries` (already sorted). The relative order inside a group is kept; groups are ordered
/// by the rule of their type (kind: alphabetical, dates: newest first, sizes: ascending).
pub fn group_entries(entries: &[Entry], by: GroupBy, now: SystemTime) -> Vec<Group> {
    let mut keyed: Vec<(i64, String, usize)> = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let (rank, title) = match by {
            GroupBy::None => (0, String::new()),
            GroupBy::Kind => {
                let g = mime::kind_group(&mime::entry_mime(e));
                (
                    if g == KindGroup::Other {
                        i64::MAX
                    } else {
                        g as i64
                    },
                    g.title().to_string(),
                )
            }
            GroupBy::DateModified => {
                let (t, r) = date_bucket(e.modified, now);
                (r, t)
            }
            GroupBy::DateCreated => {
                let (t, r) = date_bucket(e.created, now);
                (r, t)
            }
            GroupBy::DateAccessed => {
                let (t, r) = date_bucket(e.accessed, now);
                (r, t)
            }
            GroupBy::Size => {
                let (t, r) = size_bucket(e.size);
                (r as i64, t.to_string())
            }
            GroupBy::Tags => match e.tags.first() {
                Some(t) => (0, t.clone()),
                None => (i64::MAX, "No Tags".to_string()),
            },
        };
        keyed.push((rank, title, i));
    }
    let mut groups: Vec<(i64, Group)> = Vec::new();
    for (rank, title, i) in keyed {
        match groups.iter_mut().find(|(_, g)| g.title == title) {
            Some((_, g)) => g.items.push(i),
            None => groups.push((
                rank,
                Group {
                    title,
                    items: vec![i],
                },
            )),
        }
    }
    if by == GroupBy::Tags || by == GroupBy::Kind {
        groups.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.title.cmp(&b.1.title)));
    } else {
        groups.sort_by_key(|g| g.0);
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

/// `now - days` helper for tests and callers building synthetic dates.
pub fn days_ago(now: SystemTime, days: u64) -> SystemTime {
    now - Duration::from_secs(days * 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        // 2026-10-22 12:00 UTC
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_670_400)
    }

    #[test]
    fn date_buckets() {
        let n = now();
        assert_eq!(date_bucket(Some(n), n).0, "Today");
        assert_eq!(date_bucket(Some(days_ago(n, 1)), n).0, "Yesterday");
        assert_eq!(date_bucket(Some(days_ago(n, 5)), n).0, "Previous 7 Days");
        assert_eq!(date_bucket(Some(days_ago(n, 20)), n).0, "Previous 30 Days");
        assert_eq!(date_bucket(Some(days_ago(n, 60)), n).0, "August");
        assert_eq!(date_bucket(Some(days_ago(n, 400)), n).0, "2025");
        assert_eq!(date_bucket(None, n).0, "Unknown");
    }

    #[test]
    fn kind_grouping_orders_alphabetically_with_other_last() {
        let v = vec![
            Entry::synthetic("a.png", false, 1),
            Entry::synthetic("b.pdf", false, 1),
            Entry::synthetic("dir", true, 0),
            Entry::synthetic("c.png", false, 1),
        ];
        let g = group_entries(&v, GroupBy::Kind, now());
        let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Folders", "Images", "PDF Documents"]);
        assert_eq!(g[1].items, vec![0, 3]);
    }

    #[test]
    fn size_buckets_are_monotonic() {
        let ranks: Vec<u8> = [0, 5, 50_000, 5_000_000, 5_000_000_000, 50_000_000_000]
            .iter()
            .map(|s| size_bucket(*s).1)
            .collect();
        assert_eq!(ranks, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn tag_groups_put_untagged_last() {
        let mut a = Entry::synthetic("a", false, 0);
        a.tags = vec!["Work".into()];
        let b = Entry::synthetic("b", false, 0);
        let g = group_entries(&[b, a], GroupBy::Tags, now());
        assert_eq!(g[0].title, "Work");
        assert_eq!(g[1].title, "No Tags");
    }
}
