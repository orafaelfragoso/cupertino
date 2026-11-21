//! The consent questions as a pure model: they wait in the order they came, the first is the one shown, and each is
//! answered once. A question takes an answer that grants only once its alert has been on screen for the arming delay
//! ([`crate::consent::Arming`]) and the program has heard the delay pass. While the session is away (locked, or not
//! the active one of its seat) nothing is shown: every question is dismissed, and a new one is dismissed as it comes,
//! so nothing is granted or remembered.

use std::time::{Duration, Instant};

use crate::consent::{Arming, Choice, Question};

/// A question waiting, and how far its alert came.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    call: u64,
    handle: String,
    question: Question,
    /// From the alert's first presented frame; `None` before it.
    arming: Option<Arming>,
    /// The program heard the arming delay pass.
    armed: bool,
}

/// A call answered: its response goes to its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answered {
    pub call: u64,
    pub response: u32,
}

/// The question on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown<'a> {
    pub call: u64,
    pub handle: &'a str,
    pub question: &'a Question,
    /// Whether an answer that grants is taken now.
    pub armed: bool,
}

/// The waiting questions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consents {
    waiting: Vec<Waiting>,
    away: bool,
}

impl Waiting {
    fn answered(&self, choice: Choice) -> Answered {
        Answered {
            call: self.call,
            response: self.question.response(choice),
        }
    }

    fn ready(&self, now: Instant) -> bool {
        self.armed && self.arming.is_some_and(|a| a.armed(now))
    }
}

impl Consents {
    pub fn new() -> Consents {
        Consents::default()
    }

    /// The number of questions waiting, the one shown among them.
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub fn away(&self) -> bool {
        self.away
    }

    /// Ask `question` for the portal's request at `handle`, as the call `call`. While the session is away it is
    /// dismissed at once.
    pub fn ask(&mut self, call: u64, handle: String, question: Question) -> Vec<Answered> {
        let waiting = Waiting {
            call,
            handle,
            question,
            arming: None,
            armed: false,
        };
        if self.away {
            return vec![waiting.answered(Choice::Dismiss)];
        }
        self.waiting.push(waiting);
        Vec::new()
    }

    /// The portal's request at `handle` was closed: its questions go, dismissed.
    pub fn close(&mut self, handle: &str) -> Vec<Answered> {
        let (closed, kept) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|w| w.handle == handle);
        self.waiting = kept;
        closed
            .iter()
            .map(|w: &Waiting| w.answered(Choice::Dismiss))
            .collect()
    }

    /// The question to show: the first that came, none while the session is away.
    pub fn shown(&self, now: Instant) -> Option<Shown<'_>> {
        if self.away {
            return None;
        }
        self.waiting.first().map(|w| Shown {
            call: w.call,
            handle: &w.handle,
            question: &w.question,
            armed: w.ready(now),
        })
    }

    /// The alert of call `call` presented its first frame at `now`: its arming delay starts, once. How long until it
    /// is armed, the first time.
    pub fn presented(&mut self, call: u64, now: Instant) -> Option<Duration> {
        let waiting = self
            .waiting
            .iter_mut()
            .find(|w| w.call == call && w.arming.is_none())?;
        let arming = Arming::new(now);
        waiting.arming = Some(arming);
        Some(arming.remaining(now))
    }

    /// The arming delay of call `call` has passed.
    pub fn arm(&mut self, call: u64) {
        if let Some(waiting) = self.waiting.iter_mut().find(|w| w.call == call) {
            waiting.armed = true;
        }
    }

    /// The person's `choice` for the question shown, at `now`. An answer that grants is taken only once the question
    /// is armed; any other is taken at once.
    pub fn answer(&mut self, choice: Choice, now: Instant) -> Vec<Answered> {
        if self.away {
            return Vec::new();
        }
        let Some(first) = self.waiting.first() else {
            return Vec::new();
        };
        if choice == Choice::Allow && !first.ready(now) {
            return Vec::new();
        }
        vec![self.waiting.remove(0).answered(choice)]
    }

    /// The session went away or came back. Going away dismisses every question.
    pub fn set_away(&mut self, away: bool) -> Vec<Answered> {
        self.away = away;
        if !away {
            return Vec::new();
        }
        std::mem::take(&mut self.waiting)
            .iter()
            .map(|w| w.answered(Choice::Dismiss))
            .collect()
    }
}
