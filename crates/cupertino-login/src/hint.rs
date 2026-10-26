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

#[cfg(test)]
mod tests {
    use super::*;

    fn failures(login: &str, n: usize) -> FailedAttempts {
        let mut failed = FailedAttempts::default();
        for _ in 0..n {
            failed.record(login);
        }
        failed
    }

    #[test]
    fn without_a_hint_threshold_the_help_shows_at_the_third_failure() {
        let shown: Vec<bool> = (0..5)
            .map(|n| failures("alice", n).help_shown("alice", 0))
            .collect();
        assert_eq!(shown, [false, false, false, true, true]);
    }

    #[test]
    fn with_a_hint_threshold_the_help_shows_at_that_many_failures() {
        let at_one: Vec<bool> = (0..3)
            .map(|n| failures("alice", n).help_shown("alice", 1))
            .collect();
        let at_five: Vec<bool> = (3..7)
            .map(|n| failures("alice", n).help_shown("alice", 5))
            .collect();
        assert_eq!(
            (at_one, at_five),
            (vec![false, true, true], vec![false, false, true, true])
        );
    }

    #[test]
    fn a_failure_of_another_user_restarts_the_count() {
        let mut failed = failures("alice", 2);
        failed.record("bob");
        // alice had two; bob's failure made the count bob's and restarted it at 1
        assert!(!failed.help_shown("alice", 0));
        failed.record("alice");
        failed.record("alice");
        assert!(!failed.help_shown("alice", 0));
        failed.record("alice");
        assert!(failed.help_shown("alice", 0));
        assert!(!failed.help_shown("bob", 0));
    }
}
