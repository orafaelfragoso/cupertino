//! How sizes and dates read in the list, the status bar and notices.

/// A size in decimal units: "1 byte", "12 bytes", "1.5 KB", "250 MB".
pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes == 1 {
        return "1 byte".to_string();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < 4 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, U[i])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

/// Space left on a file system for the status bar: decimal units like [`human_size`], with two decimals from a
/// gigabyte.
pub fn available_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000_000 => format!("{:.2} TB", b as f64 / 1e12),
        b if b >= 1_000_000_000 => format!("{:.2} GB", b as f64 / 1e9),
        b => human_size(b),
    }
}

/// Month abbreviations of the list's dates (English).
const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Today at 09:41", "Yesterday at 18:02", else "22 Nov 2024 at 08:30", in local time.
pub fn relative_date(t: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let (bucket, _) = crate::group::date_bucket(Some(t), now);
    let secs = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    };
    let tt = secs as libc::time_t;
    // SAFETY: a zeroed tm is a valid out value for localtime_r
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&tt, &mut tm);
    }
    let hm = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    match bucket.as_str() {
        "Today" | "Yesterday" => format!("{bucket} at {hm}"),
        _ => format!(
            "{} {} {} at {hm}",
            tm.tm_mday,
            MONTH_ABBREVIATIONS[tm.tm_mon as usize],
            tm.tm_year + 1900
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_decimal() {
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(12), "12 bytes");
        assert_eq!(human_size(1500), "1.5 KB");
        assert_eq!(human_size(250_000_000), "250 MB");
    }

    #[test]
    fn available_space_has_two_decimals_from_a_gigabyte() {
        assert_eq!(available_size(227_270_000_000), "227.27 GB");
        assert_eq!(available_size(1_500_000_000_000), "1.50 TB");
        assert_eq!(available_size(250_000_000), "250 MB");
    }

    /// The instant of a local date and time.
    fn local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> std::time::SystemTime {
        // SAFETY: a zeroed tm is valid input once its fields are set; mktime only reads and normalises it
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (
            tm.tm_year,
            tm.tm_mon,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_isdst,
        ) = (year - 1900, month - 1, day, hour, minute, -1);
        let t = unsafe { libc::mktime(&mut tm) };
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64)
    }

    #[test]
    fn dates_are_relative_words_for_the_last_two_days_and_a_medium_date_before() {
        let now = local(2026, 10, 25, 18, 0);
        assert_eq!(
            relative_date(local(2026, 10, 25, 9, 41), now),
            "Today at 09:41"
        );
        assert_eq!(
            relative_date(local(2026, 10, 24, 18, 2), now),
            "Yesterday at 18:02"
        );
        assert_eq!(
            relative_date(local(2024, 11, 22, 8, 30), now),
            "22 Nov 2024 at 08:30"
        );
        assert_eq!(
            relative_date(local(2026, 1, 5, 23, 59), now),
            "5 Jan 2026 at 23:59"
        );
    }
}
