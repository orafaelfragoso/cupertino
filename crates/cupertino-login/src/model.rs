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
/// What the screen says about an attempt (a wrong password, the authenticator's message) shows this long, seconds.
pub const MESSAGE_SHOWN: f64 = 15.0;
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
    /// Why the last attempt could not be decided; shown until the next edit or for [`MESSAGE_SHOWN`].
    notice: Option<String>,
    /// When what the screen says about the last attempt started to show, once it does; `None` until the next tick
    /// for a message that came without a time.
    said_at: Option<f64>,
    /// [`DENIED`](crate::view::DENIED) shows with the outline, until it has shown for [`MESSAGE_SHOWN`].
    denied_said: bool,
    /// The field holds text: the submit button shows.
    typed: bool,
    interface: Fade,
    last_input: f64,
    /// Seconds without input until the interface hides: [`HIDE_AFTER`] or [`HIDE_AFTER_EXPLICIT_LOCK`].
    hide_after: f64,
    /// A wrong password was entered; shown until the person edits the field.
    failure_outline: bool,
    /// The session asks for reduced motion: the field does not shake.
    reduce_motion: bool,
    /// When the last shake started: a denial while it plays does not start it again.
    shake_since: Option<f64>,
}

impl LockScreen {
    /// A locked screen whose interface fades in from `now` and hides after `hide_after` seconds without input.
    pub fn new(now: f64, hide_after: f64) -> Self {
        LockScreen {
            phase: Phase::Locked,
            field: 0,
            notice: None,
            said_at: None,
            denied_said: false,
            typed: false,
            interface: Fade {
                from: 0.0,
                to: 1.0,
                start: now,
            },
            last_input: now,
            hide_after,
            failure_outline: false,
            reduce_motion: false,
            shake_since: None,
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

    /// The failure of the last attempt that was not a wrong password, until the next edit or for [`MESSAGE_SHOWN`].
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Whether the screen says the last attempt was a wrong password: with the outline, for [`MESSAGE_SHOWN`] at most.
    pub fn denied_said(&self) -> bool {
        self.failure_outline && self.denied_said
    }

    /// Whether the field holds text, as its last edit said: the submit button shows while it does.
    pub fn typed(&self) -> bool {
        self.typed
    }

    /// Start the interface's fade in again at `now` (the screen's wallpaper is ready only now): it shows from nothing.
    pub fn show_from(&mut self, now: f64) {
        if self.phase == Phase::Locked && !self.hidden() {
            self.interface = Fade {
                from: if self.interface.from == self.interface.to {
                    self.interface.to
                } else {
                    0.0
                },
                to: 1.0,
                start: now,
            };
            self.last_input = now;
        }
    }

    /// Empty the field (a new [`LockScreen::field`]): the display went to sleep or woke, and what was typed before does
    /// not wait for whoever comes next. What the screen said about the last attempt stays.
    pub fn clear_field(&mut self) {
        if matches!(self.phase, Phase::Locked | Phase::Failed { .. }) {
            self.field += 1;
            self.typed = false;
        }
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

    /// Horizontal offset of the password group in points at `now`: the shake of the last denial, which plays to its
    /// end whatever comes meanwhile; always 0 under Reduce Motion.
    pub fn shake(&self, now: f64) -> f64 {
        match self.shake_since {
            Some(since) if !self.reduce_motion => shake_offset(now - since),
            _ => 0.0,
        }
    }

    /// Whether a shake plays at `now`.
    fn shaking(&self, now: f64) -> bool {
        self.shake_since
            .is_some_and(|since| (0.0..SHAKE_DURATION).contains(&(now - since)))
    }

    /// Whether the interface is on its way to hidden or hidden: the next key only shows it again.
    pub fn hidden(&self) -> bool {
        self.interface.to == 0.0 && self.phase != Phase::Unlocking
    }

    /// Whether the field takes edits now: while waiting and during the shake (it was emptied on submit, and a retry
    /// is typed straight away); never while an attempt is checked, while the interface is hidden or as the screen
    /// unlocks.
    pub fn editable(&self) -> bool {
        !self.hidden() && matches!(self.phase, Phase::Locked | Phase::Failed { .. })
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

    /// [`LockScreen::edit`] that says whether the field holds text now (`typed`).
    pub fn edit_to(&mut self, now: f64, typed: bool) {
        let editable = self.editable();
        self.edit(now);
        if editable {
            self.typed = typed;
        }
    }

    /// Escape: empty the field (a new [`LockScreen::field`]) and forget what the screen said about the last attempt.
    pub fn escape(&mut self, now: f64) {
        let editable = self.editable();
        self.activity(now);
        if editable {
            self.field += 1;
            self.typed = false;
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
        self.typed = false;
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
        // its time starts at the next tick, or with the answer of the attempt it came during
        self.said_at = None;
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
                // a shake still playing goes on as it is
                if !self.shaking(now) {
                    self.shake_since = Some(now);
                }
                self.phase = Phase::Failed { since: now };
                self.failure_outline = true;
                self.denied_said = true;
            }
            AuthOutcome::Error(reason) => {
                self.phase = Phase::Locked;
                self.notice = Some(reason);
            }
        }
        self.said_at = Some(now);
        // the answer, not the typing, counts as the last input: the screen stays up for a retry
        self.last_input = now;
        Ok(())
    }

    /// The session manager proved the password at the login screen (a fast user switch, unlock result 4 of the design,
    /// ): unlock from any phase. The caller only calls this for a message of the manager.
    pub fn unlock_by_switch(&mut self, now: f64) {
        self.field += 1;
        self.typed = false;
        self.failure_outline = false;
        self.phase = Phase::Unlocking;
        self.interface.retarget(now, 0.0);
        self.last_input = now;
    }

    /// Advance the timers: end the shake, finish the unlock fade, hide an interface nobody touched, and take away what
    /// the screen said about the last attempt once it has shown for [`MESSAGE_SHOWN`].
    pub fn tick(&mut self, now: f64) {
        if self.saying() && !matches!(self.phase, Phase::Authenticating { .. }) {
            match self.said_at {
                None => self.said_at = Some(now),
                Some(at) if now - at >= MESSAGE_SHOWN => {
                    self.notice = None;
                    self.denied_said = false;
                    self.said_at = None;
                }
                Some(_) => {}
            }
        }
        match self.phase {
            Phase::Failed { since } if now - since >= SHAKE_DURATION => self.phase = Phase::Locked,
            Phase::Unlocking if self.interface.settled(now) => self.phase = Phase::Unlocked,
            Phase::Locked if now - self.last_input >= self.hide_after => {
                self.interface.retarget(now, 0.0)
            }
            _ => {}
        }
    }

    /// Whether the screen says something about the last attempt.
    fn saying(&self) -> bool {
        self.notice.is_some() || self.denied_said()
    }

    /// Seconds until the screen next changes without input: the end of the shake or of a fade, the idle hide, the end
    /// of a message, or the spinner appearing or turning to its next frame. The caller draws a frame per refresh in between (the opacity and
    /// the shake change with time) and ticks at this deadline.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        let fade_end =
            self.interface.start + (self.interface.to - self.interface.from).abs() * FADE_DURATION;
        let said = match (self.saying(), self.phase, self.said_at) {
            (false, _, _) | (_, Phase::Authenticating { .. }, _) => None,
            (true, _, None) => Some(now),
            (true, _, Some(at)) => Some(at + MESSAGE_SHOWN),
        };
        let deadlines = [
            said,
            (!self.interface.settled(now)).then_some(fade_end),
            self.shaking(now)
                .then(|| self.shake_since.map(|since| since + SHAKE_DURATION))
                .flatten(),
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
