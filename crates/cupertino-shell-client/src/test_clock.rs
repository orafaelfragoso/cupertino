//! The clock the shell processes show, which a debug build lets a test fix: with `CUPERTINO_TEST_CLOCK` set to a decimal
//! number of Unix seconds, [`display_unix_seconds`] returns it, [`display_now`] stays at the process's first reading and
//! [`motion_settled`] is true, so a scene settles (the goldens). Only for what is drawn: never for authentication, back-off, locks or IPC deadlines.
//! The override exists only with `debug_assertions`, which every release profile turns off: a packaged binary neither
//! reads the variable nor contains its name (the release build checks that with `strings`).

use std::time::{Instant, SystemTime, SystemTimeError, UNIX_EPOCH};

/// The wall clock as drawn, in Unix seconds.
pub fn display_unix_seconds() -> Result<i64, SystemTimeError> {
    #[cfg(debug_assertions)]
    if let Some(fixed) = fixed::seconds() {
        return Ok(fixed);
    }
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

/// The monotonic clock for what is drawn (a caret's blink, a cached icon's age).
pub fn display_now() -> Instant {
    #[cfg(debug_assertions)]
    if let Some(frozen) = fixed::instant() {
        return frozen;
    }
    Instant::now()
}

/// Whether an opening animation starts at its end (the clock is fixed for a test). Only for where an animation starts,
/// never for its formula or timing, and never for timers, hides or countdowns.
pub fn motion_settled() -> bool {
    #[cfg(debug_assertions)]
    {
        fixed::seconds().is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[cfg(debug_assertions)]
mod fixed {
    use std::sync::OnceLock;
    use std::time::Instant;

    const VARIABLE: &str = "CUPERTINO_TEST_CLOCK";

    /// A decimal `i64`; anything else is no override.
    pub(super) fn parse(value: Option<&str>) -> Option<i64> {
        let value = value?;
        if value.is_empty()
            || !value
                .trim_start_matches('-')
                .bytes()
                .all(|b| b.is_ascii_digit())
        {
            return None;
        }
        value.parse().ok()
    }

    pub(super) fn seconds() -> Option<i64> {
        static SECONDS: OnceLock<Option<i64>> = OnceLock::new();
        *SECONDS.get_or_init(|| parse(std::env::var(VARIABLE).ok().as_deref()))
    }

    pub(super) fn instant() -> Option<Instant> {
        static FROZEN: OnceLock<Instant> = OnceLock::new();
        seconds().map(|_| *FROZEN.get_or_init(Instant::now))
    }
}

#[cfg(test)]
mod tests {
    use super::fixed::parse;

    #[test]
    fn only_a_decimal_number_fixes_the_clock() {
        assert_eq!(parse(Some("1700000000")), Some(1_700_000_000));
        assert_eq!(parse(Some("-5")), Some(-5));
        for not in [
            "",
            "+5",
            "1e9",
            " 5",
            "5 ",
            "0x10",
            "abc",
            "99999999999999999999",
        ] {
            assert_eq!(parse(Some(not)), None, "{not:?}");
        }
        assert_eq!(parse(None), None);
    }
}
