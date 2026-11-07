//! The lock screen's state machine: the phases of an attempt, the shake after a failure, the fades of the interface and
//! what the screen says about the last attempt. Pure: time is an absolute `f64` in seconds on the caller's clock, the
//! password field's edits arrive as calls and the outcome of the authentication as [`AuthOutcome`]. The typed text is not
//! here: it lives in the toolkit's secure field until it is submitted, and [`LockScreen::submit`] takes it then. The only
//! way to [`Phase::Unlocked`] is [`LockScreen::finish_auth`] with [`AuthOutcome::Success`] for an attempt that
//! [`LockScreen::submit`] handed out.

use crate::spinner;
use std::f64::consts::PI;
use zeroize::Zeroizing;

/// Duration of the shake after a failed attempt.
pub const SHAKE_DURATION: f64 = 0.4;
/// Peak offset of the shake in points.
pub const SHAKE_AMPLITUDE: f64 = 14.0;
/// A full fade of the interface takes this long, linear; a partial one takes the fraction of it.
pub const FADE_DURATION: f64 = 0.25;
/// The interface hides after this long without input.
pub const HIDE_AFTER: f64 = 30.0;
/// The same after a lock the person asked for: lock request type 8,.
pub const HIDE_AFTER_EXPLICIT_LOCK: f64 = 5.0;
/// Longest password in bytes: `PAM_MAX_RESP_SIZE` (`security/_pam_types.h`); further characters are dropped.
pub const MAX_PASSWORD_BYTES: usize = 512;

/// Horizontal offset in points of the password field `elapsed` seconds into the shake (curve 4):
/// `p` runs 0 to 1 over [`SHAKE_DURATION`], `v = 0.5 + 0.5 sin(6 pi p) d` with `d = 1` below `p = 0.5` and
/// `exp(-0.8 p)` after, `x = (v - 0.5) * 2 * 14`. Zero outside the shake.
pub fn shake_offset(elapsed: f64) -> f64 {
    let p = elapsed / SHAKE_DURATION;
    if !(0.0..1.0).contains(&p) {
        return 0.0;
    }
    let damping = if p < 0.5 { 1.0 } else { (-0.8 * p).exp() };
    let v = 0.5 + 0.5 * (6.0 * PI * p).sin() * damping;
    (v - 0.5) * 2.0 * SHAKE_AMPLITUDE
}

/// A linear opacity animation toward `to` (duration `|to - from| * 0.25 s`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    from: f64,
    to: f64,
    start: f64,
}

impl Fade {
    fn value(&self, now: f64) -> f64 {
        let duration = (self.to - self.from).abs() * FADE_DURATION;
        if duration == 0.0 {
            return self.to;
        }
        let t = ((now - self.start) / duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * t
    }

    fn settled(&self, now: f64) -> bool {
        self.value(now) == self.to
    }

    /// Head for `to` from wherever the fade is at `now`.
    fn retarget(&mut self, now: f64, to: f64) {
        if self.to != to {
            *self = Fade {
                from: self.value(now),
                to,
                start: now,
            };
        }
    }
}

/// What the user can do with the lock screen right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Waiting for a password.
    Locked,
    /// A password went to the authenticator at `since` and the answer is pending.
    Authenticating { since: f64 },
    /// The attempt failed at `since`: the field shakes for [`SHAKE_DURATION`], then the screen is [`Phase::Locked`] again.
    Failed { since: f64 },
    /// The attempt succeeded: the interface fades out, then the screen is [`Phase::Unlocked`].
    Unlocking,
    /// Authenticated and faded out: the session may be unlocked.
    Unlocked,
}

/// A key of a field that is not a password: the name typed at the greeter, and the panels' shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A typed character; control characters are ignored.
    Char(char),
    Backspace,
    /// Submit the field.
    Return,
    /// Clear the field, or cancel.
    Escape,
}

/// The authenticator's answer to an attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    Success,
    /// The password is wrong.
    Denied,
    /// The authenticator could not decide (or refused the account); the text says why.
    Error(String),
}

/// [`LockScreen::finish_auth`] was called without an attempt in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoAttempt;

impl std::fmt::Display for NoAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "an authentication result arrived with no attempt in flight"
        )
    }
}

impl std::error::Error for NoAttempt {}

/// A password in a buffer that is zeroed when it is dropped.
pub type Password = Zeroizing<String>;

/// A copy of `text` in a zeroed-on-drop buffer of [`MAX_PASSWORD_BYTES`], cut at the last character that fits: the
/// buffer is allocated at its limit, so filling it never moves it and leaves a copy behind.
pub fn password(text: &str) -> Password {
    let mut end = text.len().min(MAX_PASSWORD_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut password = Zeroizing::new(String::with_capacity(MAX_PASSWORD_BYTES));
    password.push_str(&text[..end]);
    password
}

/// The lock screen while the session is locked.
#[derive(Debug)]
pub struct LockScreen {
    phase: Phase,
    /// Counts the times the password field was emptied (a submission, Escape, an unlock): the view makes a new field
    /// for each, so the typed text of the one before is dropped and zeroed.
    field: u64,
    /// Why the last attempt could not be decided; shown until the next edit.
    notice: Option<String>,
    interface: Fade,
    last_input: f64,
    /// Seconds without input until the interface hides: [`HIDE_AFTER`] or [`HIDE_AFTER_EXPLICIT_LOCK`].
    hide_after: f64,
    /// A wrong password was entered; shown until the person edits the field.
    failure_outline: bool,
    /// The session asks for reduced motion: the field does not shake.
    reduce_motion: bool,
}

impl LockScreen {
    /// A locked screen whose interface fades in from `now` and hides after `hide_after` seconds without input.
    pub fn new(now: f64, hide_after: f64) -> Self {
        LockScreen {
            phase: Phase::Locked,
            field: 0,
            notice: None,
            interface: Fade {
                from: 0.0,
                to: 1.0,
                start: now,
            },
            last_input: now,
            hide_after,
            failure_outline: false,
            reduce_motion: false,
        }
    }

    /// Follow the session's Reduce Motion: no shake while it is on.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
    }

    /// Whether the password field shows that the last attempt was a wrong password: from the answer until the person
    /// edits the field. With or without the shake, so a failure is never shown by motion alone.
    pub fn failure_outline(&self) -> bool {
        self.failure_outline
    }

    /// Show the interface at full opacity at once instead of fading it in (the shell client's fixed test clock).
    pub fn settle_interface(&mut self) {
        self.interface.from = self.interface.to;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether an attempt is being checked: from its submission until the authenticator answers. The password field is
    /// busy for assistive technology meanwhile.
    pub fn checking(&self) -> bool {
        matches!(self.phase, Phase::Authenticating { .. })
    }

    /// Which password field the screen shows: a new number is a new, empty field.
    pub fn field(&self) -> u64 {
        self.field
    }

    /// The failure of the last attempt that was not a wrong password, until the next edit.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Opacity of the interface (everything but the wallpaper) at `now`.
    pub fn opacity(&self, now: f64) -> f64 {
        self.interface.value(now)
    }

    /// The spinner's frame at `now`: shown once the attempt has been pending for [`spinner::SHOW_AFTER`], starting at
    /// frame 0; `None` before that and when no attempt is pending.
    pub fn spinner(&self, now: f64) -> Option<u32> {
        match self.phase {
            Phase::Authenticating { since } if now - since >= spinner::SHOW_AFTER => {
                Some(spinner::frame_at(now - since - spinner::SHOW_AFTER))
            }
            _ => None,
        }
    }

    /// Horizontal offset of the password field in points at `now`; always 0 under Reduce Motion.
    pub fn shake(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Failed { since } if !self.reduce_motion => shake_offset(now - since),
            _ => 0.0,
        }
    }

    /// Whether the interface is on its way to hidden or hidden: the next key only shows it again.
    pub fn hidden(&self) -> bool {
        self.interface.to == 0.0 && self.phase != Phase::Unlocking
    }

    /// Whether the field takes edits now: while waiting, during an attempt and during the shake (it was emptied on
    /// submit, and a retry is typed straight away), never while the interface is hidden or the screen unlocks.
    pub fn editable(&self) -> bool {
        !self.hidden()
            && matches!(
                self.phase,
                Phase::Locked | Phase::Authenticating { .. } | Phase::Failed { .. }
            )
    }

    /// Note input: it shows a hidden interface and restarts the idle time. The key or click that shows a hidden
    /// interface does nothing else; the view swallows it.
    pub fn activity(&mut self, now: f64) {
        self.last_input = now;
        if self.phase == Phase::Locked {
            self.interface.retarget(now, 1.0);
        }
    }

    /// The person changed the text in the field: what the screen said about the last attempt goes.
    pub fn edit(&mut self, now: f64) {
        let editable = self.editable();
        self.activity(now);
        if editable {
            self.notice = None;
            self.failure_outline = false;
        }
    }

    /// Escape: empty the field (a new [`LockScreen::field`]) and forget what the screen said about the last attempt.
    pub fn escape(&mut self, now: f64) {
        let editable = self.editable();
        self.activity(now);
        if editable {
            self.field += 1;
            self.notice = None;
            self.failure_outline = false;
        }
    }

    /// Return in the field with `typed` in it, borrowed from the field's zeroed buffer. Returns the password to authenticate
    /// when it is submitted, its only copy: the screen is then [`Phase::Authenticating`], waiting for
    /// [`LockScreen::finish_auth`], and the field is a new, empty one, so the view drops the old field and its buffer. An
    /// empty field, a second submission while an attempt runs, and Return on a hidden interface submit nothing.
    pub fn submit(&mut self, now: f64, typed: &str) -> Option<Password> {
        let editable = self.editable();
        self.activity(now);
        if !editable || typed.is_empty() || matches!(self.phase, Phase::Authenticating { .. }) {
            return None;
        }
        self.phase = Phase::Authenticating { since: now };
        self.field += 1;
        self.notice = None;
        self.failure_outline = false;
        Some(password(typed))
    }

    /// Show a message the authenticator sent while the attempt runs, until the next edit. The messages of one attempt follow
    /// one another (a lock-out says that the account is locked and then how long it lasts); any other message replaces the last.
    pub fn notify(&mut self, text: String) {
        self.notice = Some(match (self.notice.take(), self.phase) {
            (Some(earlier), Phase::Authenticating { .. }) => format!("{earlier} {text}"),
            _ => text,
        });
    }

    /// Take the authenticator's answer to the attempt [`LockScreen::submit`] handed out.
    pub fn finish_auth(&mut self, now: f64, outcome: AuthOutcome) -> Result<(), NoAttempt> {
        if !matches!(self.phase, Phase::Authenticating { .. }) {
            return Err(NoAttempt);
        }
        match outcome {
            AuthOutcome::Success => {
                self.phase = Phase::Unlocking;
                self.interface.retarget(now, 0.0);
            }
            AuthOutcome::Denied => {
                self.phase = Phase::Failed { since: now };
                self.failure_outline = true;
            }
            AuthOutcome::Error(reason) => {
                self.phase = Phase::Locked;
                self.notice = Some(reason);
            }
        }
        // the answer, not the typing, counts as the last input: the screen stays up for a retry
        self.last_input = now;
        Ok(())
    }

    /// The session manager proved the password at the login screen (a fast user switch, unlock result 4 of the design,
    /// ): unlock from any phase. The caller only calls this for a message of the manager.
    pub fn unlock_by_switch(&mut self, now: f64) {
        self.field += 1;
        self.failure_outline = false;
        self.phase = Phase::Unlocking;
        self.interface.retarget(now, 0.0);
        self.last_input = now;
    }

    /// Advance the timers: end the shake, finish the unlock fade, hide an interface nobody touched.
    pub fn tick(&mut self, now: f64) {
        match self.phase {
            Phase::Failed { since } if now - since >= SHAKE_DURATION => self.phase = Phase::Locked,
            Phase::Unlocking if self.interface.settled(now) => self.phase = Phase::Unlocked,
            Phase::Locked if now - self.last_input >= self.hide_after => {
                self.interface.retarget(now, 0.0)
            }
            _ => {}
        }
    }

    /// Seconds until the screen next changes without input: the end of the shake or of a fade, the idle hide, or the
    /// spinner appearing or turning to its next frame. The caller draws a frame per refresh in between (the opacity and
    /// the shake change with time) and ticks at this deadline.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        let fade_end =
            self.interface.start + (self.interface.to - self.interface.from).abs() * FADE_DURATION;
        let deadlines = [
            (!self.interface.settled(now)).then_some(fade_end),
            match self.phase {
                Phase::Failed { since } => Some(since + SHAKE_DURATION),
                Phase::Locked if !self.hidden() => Some(self.last_input + self.hide_after),
                Phase::Authenticating { since } if now - since < spinner::SHOW_AFTER => {
                    Some(since + spinner::SHOW_AFTER)
                }
                Phase::Authenticating { since } => {
                    Some(now + spinner::next_frame_in(now - since - spinner::SHOW_AFTER))
                }
                _ => None,
            },
        ];
        deadlines
            .into_iter()
            .flatten()
            .reduce(f64::min)
            .map(|at| (at - now).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Submit `typed` at `now`; the password handed to the authenticator, if any.
    fn submit(screen: &mut LockScreen, now: f64, typed: &str) -> Option<String> {
        screen.submit(now, typed).map(|p| p.as_str().to_owned())
    }

    fn approx(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn the_shake_follows_the_curve_of_the_register() {
        approx(shake_offset(0.0), 0.0);
        // p = 1/12: the first peak, d = 1
        approx(shake_offset(SHAKE_DURATION / 12.0), 14.0);
        approx(shake_offset(SHAKE_DURATION * 0.25), -14.0);
        // p = 0.75: sin(4.5 pi) = 1, damped by exp(-0.6)
        approx(shake_offset(SHAKE_DURATION * 0.75), 14.0 * (-0.6f64).exp());
        approx(shake_offset(SHAKE_DURATION), 0.0);
        approx(shake_offset(-0.1), 0.0);
        approx(shake_offset(2.0), 0.0);
    }

    #[test]
    fn a_settled_interface_is_fully_shown_at_once_and_keeps_its_timers() {
        let mut screen = LockScreen::new(10.0, HIDE_AFTER);
        screen.settle_interface();
        approx(screen.opacity(10.0), 1.0);
        screen.tick(10.0 + HIDE_AFTER - 0.01);
        approx(screen.opacity(10.0 + HIDE_AFTER - 0.01), 1.0);
        screen.tick(10.0 + HIDE_AFTER);
        approx(screen.opacity(10.0 + HIDE_AFTER + FADE_DURATION), 0.0);
    }

    #[test]
    fn the_interface_fades_in_linearly_over_a_quarter_second() {
        let screen = LockScreen::new(10.0, HIDE_AFTER);
        approx(screen.opacity(10.0), 0.0);
        approx(screen.opacity(10.125), 0.5);
        approx(screen.opacity(10.25), 1.0);
        approx(screen.opacity(11.0), 1.0);
    }

    #[test]
    fn a_password_is_cut_at_the_authenticator_limit_on_a_character_boundary() {
        let long = "a".repeat(MAX_PASSWORD_BYTES + 10);
        assert_eq!(password(&long).len(), MAX_PASSWORD_BYTES);
        // a two-byte character does not fit in the one byte left over
        let straddling = format!("{}é", "a".repeat(MAX_PASSWORD_BYTES - 1));
        assert_eq!(password(&straddling).len(), MAX_PASSWORD_BYTES - 1);
        let fits = format!("{}é", "a".repeat(MAX_PASSWORD_BYTES - 2));
        assert_eq!(password(&fits).len(), MAX_PASSWORD_BYTES);
        assert_eq!(password(&long).capacity(), MAX_PASSWORD_BYTES);
    }

    #[test]
    fn return_with_an_empty_field_does_nothing() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        assert_eq!(submit(&mut screen, 1.0, ""), None);
        assert_eq!(screen.phase(), Phase::Locked);
        assert_eq!(screen.field(), 0, "the field stays as it is");
    }

    #[test]
    fn return_hands_over_the_password_and_empties_the_field() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        assert_eq!(
            submit(&mut screen, 1.0, "hunter2").as_deref(),
            Some("hunter2")
        );
        assert_eq!(screen.phase(), Phase::Authenticating { since: 1.0 });
        assert_eq!(screen.field(), 1, "a new, empty field");
        // a retry can be typed while the answer is pending, but not submitted: one attempt at a time
        screen.edit(1.1);
        assert!(screen.editable());
        assert_eq!(submit(&mut screen, 1.1, "x"), None);
        assert_eq!(screen.phase(), Phase::Authenticating { since: 1.0 });
        assert_eq!(screen.field(), 1, "the retry is kept");
    }

    #[test]
    fn a_wrong_password_shakes_then_waits_for_a_new_one() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "wrong");
        screen.finish_auth(2.0, AuthOutcome::Denied).unwrap();
        assert_eq!(screen.phase(), Phase::Failed { since: 2.0 });
        approx(screen.shake(2.0 + SHAKE_DURATION / 12.0), 14.0);
        // a retry typed and submitted during the shake goes out at once
        screen.edit(2.1);
        screen.tick(2.375);
        assert_eq!(screen.phase(), Phase::Failed { since: 2.0 });
        screen.tick(2.5);
        assert_eq!(screen.phase(), Phase::Locked);
        approx(screen.shake(2.5), 0.0);
        assert_eq!(submit(&mut screen, 2.6, "right").as_deref(), Some("right"));
        assert_eq!(screen.field(), 2);
    }

    #[test]
    fn a_wrong_password_is_outlined_until_the_person_edits_the_field_and_nothing_else_is() {
        for reduce_motion in [false, true] {
            let mut screen = LockScreen::new(0.0, HIDE_AFTER);
            screen.set_reduce_motion(reduce_motion);
            assert!(!screen.failure_outline());
            submit(&mut screen, 1.0, "wrong");
            screen.tick(2.5);
            assert!(screen.spinner(2.5).is_some());
            assert!(!screen.failure_outline(), "not while the attempt runs");
            screen.finish_auth(3.0, AuthOutcome::Denied).unwrap();
            assert!(screen.failure_outline());
            // the shake ends; the field, emptied by the submission, is still the person's to change
            screen.tick(3.0 + SHAKE_DURATION + 1.0);
            assert_eq!(screen.phase(), Phase::Locked);
            assert!(screen.failure_outline(), "it stays after the shake");
            screen.activity(4.5);
            assert!(screen.failure_outline(), "a pointer move is not an edit");
            screen.edit(5.0);
            assert!(!screen.failure_outline(), "the next keystroke ends it");

            submit(&mut screen, 6.0, "again");
            screen.finish_auth(6.5, AuthOutcome::Denied).unwrap();
            assert!(screen.failure_outline());
            screen.escape(7.0);
            assert!(!screen.failure_outline(), "Escape ends it");

            submit(&mut screen, 8.0, "r");
            screen
                .finish_auth(8.5, AuthOutcome::Error("account locked".into()))
                .unwrap();
            assert!(
                !screen.failure_outline(),
                "an undecided attempt shows its notice instead"
            );
            submit(&mut screen, 9.0, "right");
            screen.finish_auth(9.5, AuthOutcome::Success).unwrap();
            assert!(!screen.failure_outline());
        }
    }

    #[test]
    fn escape_empties_the_field_and_clears_the_notice() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen
            .finish_auth(1.5, AuthOutcome::Error("no PAM".into()))
            .unwrap();
        assert_eq!(screen.field(), 1);
        screen.escape(2.0);
        assert_eq!((screen.field(), screen.notice()), (2, None));
    }

    #[test]
    fn under_reduce_motion_a_wrong_password_does_not_shake() {
        let mut moving = LockScreen::new(0.0, HIDE_AFTER);
        let mut still = LockScreen::new(0.0, HIDE_AFTER);
        still.set_reduce_motion(true);
        for screen in [&mut moving, &mut still] {
            submit(screen, 1.0, "wrong");
            screen.finish_auth(2.0, AuthOutcome::Denied).unwrap();
        }
        let mut t = 2.0;
        while t <= 2.0 + SHAKE_DURATION {
            approx(still.shake(t), 0.0);
            approx(moving.shake(t), shake_offset(t - 2.0));
            approx(still.opacity(t), moving.opacity(t));
            t += SHAKE_DURATION / 40.0;
        }
        assert_eq!(
            still.phase(),
            moving.phase(),
            "the failure keeps its timing"
        );
    }

    #[test]
    fn an_authenticator_error_keeps_the_screen_locked_and_shows_the_reason_until_the_next_edit() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen
            .finish_auth(
                1.5,
                AuthOutcome::Error("authentication service unavailable".into()),
            )
            .unwrap();
        assert_eq!(screen.phase(), Phase::Locked);
        assert_eq!(screen.notice(), Some("authentication service unavailable"));
        screen.edit(2.0);
        assert_eq!(screen.notice(), None);
    }

    #[test]
    fn a_message_of_the_authenticator_shows_until_the_next_edit() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen.notify("Password expires soon".into());
        assert_eq!(screen.notice(), Some("Password expires soon"));
        screen.finish_auth(1.5, AuthOutcome::Denied).unwrap();
        assert_eq!(screen.notice(), Some("Password expires soon"));
        screen.edit(2.0);
        assert_eq!(screen.notice(), None);
    }

    #[test]
    fn success_fades_the_interface_out_and_then_unlocks() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen.finish_auth(2.0, AuthOutcome::Success).unwrap();
        assert_eq!(screen.phase(), Phase::Unlocking);
        assert!(!screen.editable(), "nothing more is typed");
        assert_eq!(submit(&mut screen, 2.05, "more"), None);
        approx(screen.opacity(2.125), 0.5);
        screen.tick(2.2);
        assert_eq!(screen.phase(), Phase::Unlocking);
        screen.tick(2.25);
        assert_eq!(screen.phase(), Phase::Unlocked);
    }

    #[test]
    fn a_fast_user_switch_unlocks_from_any_phase_and_forgets_what_was_typed() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        screen.edit(1.0);
        screen.unlock_by_switch(2.0);
        assert_eq!((screen.phase(), screen.field()), (Phase::Unlocking, 1));
        screen.tick(2.25);
        assert_eq!(screen.phase(), Phase::Unlocked);
        // also with an attempt on its way to the authenticator
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen.unlock_by_switch(1.5);
        assert_eq!(screen.phase(), Phase::Unlocking);
    }

    #[test]
    fn a_result_without_an_attempt_is_refused() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        assert_eq!(
            screen.finish_auth(1.0, AuthOutcome::Success),
            Err(NoAttempt)
        );
        assert_eq!(screen.phase(), Phase::Locked);
        // and a second answer to one attempt too
        submit(&mut screen, 2.0, "pw");
        screen.finish_auth(2.5, AuthOutcome::Denied).unwrap();
        assert_eq!(
            screen.finish_auth(2.6, AuthOutcome::Success),
            Err(NoAttempt)
        );
        assert_eq!(screen.phase(), Phase::Failed { since: 2.5 });
    }

    #[test]
    fn nothing_but_a_successful_attempt_reaches_unlocked() {
        // every sequence of up to five steps from the inputs that do not carry a success
        #[derive(Clone, Copy, Debug)]
        enum Step {
            Edit,
            Escape,
            Submit,
            SubmitEmpty,
            Activity,
            Tick(f64),
            Denied,
            Error,
        }
        let steps = [
            Step::Edit,
            Step::Escape,
            Step::Submit,
            Step::SubmitEmpty,
            Step::Activity,
            Step::Tick(0.5),
            Step::Tick(40.0),
            Step::Denied,
            Step::Error,
        ];
        let mut sequences: Vec<Vec<Step>> = vec![Vec::new()];
        for _ in 0..5 {
            sequences = sequences
                .into_iter()
                .flat_map(|s| {
                    steps
                        .iter()
                        .map(move |&step| s.iter().copied().chain([step]).collect())
                })
                .collect();
            for sequence in &sequences {
                let mut screen = LockScreen::new(0.0, HIDE_AFTER);
                let mut now = 0.0;
                for step in sequence {
                    now += 1.0;
                    match *step {
                        Step::Edit => screen.edit(now),
                        Step::Escape => screen.escape(now),
                        Step::Submit => drop(submit(&mut screen, now, "a")),
                        Step::SubmitEmpty => drop(submit(&mut screen, now, "")),
                        Step::Activity => screen.activity(now),
                        Step::Tick(seconds) => {
                            now += seconds;
                            screen.tick(now);
                        }
                        Step::Denied => drop(screen.finish_auth(now, AuthOutcome::Denied)),
                        Step::Error => {
                            let _ = screen.finish_auth(now, AuthOutcome::Error("x".into()));
                        }
                    }
                    assert!(
                        !matches!(screen.phase(), Phase::Unlocking | Phase::Unlocked),
                        "{sequence:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_messages_of_one_attempt_follow_each_other_and_a_later_one_replaces_them() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        assert!(submit(&mut screen, 1.0, "x").is_some());
        screen.notify("The account is locked due to 3 failed logins.".to_owned());
        screen.notify("(10 minutes left to unlock)".to_owned());
        assert_eq!(
            screen.notice(),
            Some("The account is locked due to 3 failed logins. (10 minutes left to unlock)")
        );
        screen
            .finish_auth(2.0, AuthOutcome::Error("locked".to_owned()))
            .unwrap();
        // a message outside an attempt replaces what is shown
        screen.notify("Enter an administrator's password.".to_owned());
        assert_eq!(screen.notice(), Some("Enter an administrator's password."));
    }

    #[test]
    fn a_screen_locked_on_request_hides_after_five_seconds_without_input() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER_EXPLICIT_LOCK);
        screen.edit(1.0);
        approx(screen.next_wakeup(1.0).unwrap(), 5.0);
        screen.tick(5.9);
        approx(screen.opacity(5.9), 1.0);
        screen.tick(6.0);
        approx(screen.opacity(6.25), 0.0);
        assert_eq!(screen.next_wakeup(7.0), None);
    }

    #[test]
    fn an_untouched_interface_hides_after_thirty_seconds_and_the_waking_input_is_not_typed() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        screen.edit(5.0);
        approx(screen.next_wakeup(5.0).unwrap(), 30.0);
        // the fade-in ended at 0.25, before the first input at 5
        approx(screen.next_wakeup(0.0).unwrap(), 0.25);
        screen.tick(34.9);
        approx(screen.opacity(34.9), 1.0);
        screen.tick(35.0);
        assert!(screen.hidden() && !screen.editable());
        approx(screen.opacity(35.125), 0.5);
        approx(screen.opacity(35.25), 0.0);
        assert_eq!(screen.next_wakeup(36.0), None);
        // Return on the hidden interface shows it and submits nothing
        assert_eq!(submit(&mut screen, 40.0, "typed before"), None);
        assert_eq!(screen.phase(), Phase::Locked);
        approx(screen.opacity(40.25), 1.0);
        assert!(screen.editable());
        assert_eq!(submit(&mut screen, 41.0, "pw").as_deref(), Some("pw"));
    }

    #[test]
    fn the_interface_does_not_hide_while_an_attempt_is_pending() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        submit(&mut screen, 1.0, "pw");
        screen.tick(100.0);
        approx(screen.opacity(100.0), 1.0);
        // only the spinner moves: its frame turns 1/48 s into the next frame length
        approx(screen.next_wakeup(100.0).unwrap(), 1.0 / 48.0);
    }

    #[test]
    fn the_spinner_appears_after_a_second_of_authentication_and_turns_at_24_frames_a_second() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        assert_eq!(screen.spinner(1.0), None);
        submit(&mut screen, 2.0, "pw");
        assert_eq!((screen.spinner(2.0), screen.spinner(2.999)), (None, None));
        approx(screen.next_wakeup(2.5).unwrap(), 0.5);
        assert_eq!(
            (
                screen.spinner(3.0),
                screen.spinner(3.0 + 1.0 / 24.0),
                screen.spinner(3.5),
                screen.spinner(4.0)
            ),
            (Some(0), Some(1), Some(12), Some(0))
        );
        screen.finish_auth(4.2, AuthOutcome::Denied).unwrap();
        assert_eq!(screen.spinner(4.2), None);
    }

    #[test]
    fn the_screen_holds_no_password_to_print() {
        let mut screen = LockScreen::new(0.0, HIDE_AFTER);
        let handed = submit(&mut screen, 1.0, "hunter2");
        assert_eq!(handed.as_deref(), Some("hunter2"));
        let printed = format!("{screen:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}
