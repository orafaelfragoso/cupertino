//! When the screen offers password help after failed attempts. The
//! count is per user and follows only the last user that failed: a failure by another user starts it again at 1.

/// Failures at which help shows when no hint threshold is configured (`RetriesUntilHint` absent or 0): the help appears
/// without hint text.
const HELP_AFTER_WITHOUT_HINTS: u32 = 3;

/// The failed attempts of the user that failed last.
#[derive(Debug, Default)]
pub struct FailedAttempts {
    last: Option<(String, u32)>,
}

impl FailedAttempts {
    /// Note a failed attempt of `login`: the count goes up by one, or restarts at 1 when another user failed last.
    pub fn record(&mut self, login: &str) {
        self.last = match self.last.take() {
            Some((last, count)) if last == login => Some((last, count + 1)),
            _ => Some((login.to_owned(), 1)),
        };
    }

    /// Whether the password help shows for `login`. `retries_until_hint` is the setting `RetriesUntilHint` (absent = 0):
    /// 0 shows the help after three failures, a larger value after that many.
    pub fn help_shown(&self, login: &str, retries_until_hint: u32) -> bool {
        let count = self
            .last
            .as_ref()
            .filter(|(last, _)| last == login)
            .map_or(0, |&(_, count)| count);
        let needed = if retries_until_hint == 0 {
            HELP_AFTER_WITHOUT_HINTS
        } else {
            retries_until_hint
        };
        count >= needed
    }
}
