//! The prompt as a pure model: what polkitd asked, what the helper says, what is typed, and when to start the helper,
//! answer it or tell polkitd the authentication is over. One prompt is on screen at a time; the others wait in
//! polkitd's order. While the session is locked or not the active one, nothing is on screen: a prompt showing then
//! is dismissed, and the waiting ones stay until the session is back.

use std::collections::VecDeque;

use zeroize::Zeroizing;

use crate::agent::{Outcome, Request, User};
use crate::helper::Line;
use crate::text::shown;

/// Helper runs per prompt: after this many failures polkitd is told the authentication is over, unauthorised.
pub const ATTEMPTS: u8 = 3;
/// What the prompt says after a failed attempt.
pub const TRY_AGAIN: &str = "Incorrect password. Try again.";
/// The longest answer the field takes, in bytes.
pub const MAX_ANSWER: usize = 512;

/// A key that reaches the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Return,
    Escape,
}

/// What the host does for the model. `Debug` never prints an answer.
#[derive(PartialEq, Eq)]
pub enum Effect {
    /// Start the helper for this user and this cookie, dropping any running one.
    StartHelper { user: User, cookie: String },
    /// Send this answer to the running helper.
    Answer(Zeroizing<Vec<u8>>),
    /// Drop the running helper, then answer the request of this cookie with this outcome.
    Finish { cookie: String, outcome: Outcome },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::StartHelper { user, cookie } => f
                .debug_struct("StartHelper")
                .field("user", user)
                .field("cookie", cookie)
                .finish(),
            Effect::Answer(_) => f.write_str("Answer(<secret>)"),
            Effect::Finish { cookie, outcome } => f
                .debug_struct("Finish")
                .field("cookie", cookie)
                .field("outcome", outcome)
                .finish(),
        }
    }
}

/// An empty field, allocated at [`MAX_ANSWER`] so that typing never moves its text and leaves a copy behind in the
/// block it left; it is zeroed when it drops.
fn empty_field() -> Zeroizing<String> {
    Zeroizing::new(String::with_capacity(MAX_ANSWER))
}

/// Where the prompt on screen is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The helper runs and hasn't asked anything yet.
    Waiting,
    /// The helper asked; the field takes keys.
    Asking,
    /// The answer went to the helper.
    Checking,
}

/// The prompt on screen.
pub struct Prompt {
    pub request: Request,
    pub user: User,
    pub phase: Phase,
    /// What PAM asks for, such as "Password: ".
    pub question: String,
    /// Whether the field shows what is typed; false for a secret.
    pub echo: bool,
    /// What is typed, in a block of its own that never grows ([`empty_field`]).
    field: Zeroizing<String>,
    /// How many characters the field showed when the answer went to the helper; it keeps showing them while checking.
    sent_len: usize,
    pub error: Option<String>,
    pub info: Option<String>,
    failures: u8,
}

impl Prompt {
    /// How many attempts of this prompt have failed so far.
    pub fn failures(&self) -> u8 {
        self.failures
    }

    /// Characters the field shows.
    pub fn field_len(&self) -> usize {
        match self.phase {
            Phase::Checking => self.sent_len,
            _ => self.field.chars().count(),
        }
    }

    /// Whether Return answers: the helper asked and the field isn't empty.
    pub fn can_answer(&self) -> bool {
        self.phase == Phase::Asking && !self.field.is_empty()
    }

    /// The field's text when it may be shown: only for a question PAM asks in clear.
    pub fn shown_field(&self) -> Option<&str> {
        self.echo.then_some(self.field.as_str())
    }
}

/// Every authentication polkitd asked for and hasn't been answered yet.
pub struct Agent {
    /// The user of the session, whom a prompt offers first when polkitd accepts them.
    me: User,
    prompt: Option<Prompt>,
    waiting: VecDeque<Request>,
    /// The session is locked, or isn't the active one.
    away: bool,
}

impl Agent {
    pub fn new(me: User) -> Agent {
        Agent {
            me,
            prompt: None,
            waiting: VecDeque::new(),
            away: false,
        }
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// polkitd asks for `request`.
    pub fn begin(&mut self, request: Request) -> Vec<Effect> {
        self.waiting.push_back(request);
        self.show_next()
    }

    /// polkitd withdraws the request of `cookie`.
    pub fn cancel(&mut self, cookie: &str) -> Vec<Effect> {
        let finish = |cookie: String| Effect::Finish {
            cookie,
            outcome: Outcome::Cancelled,
        };
        if self
            .prompt
            .as_ref()
            .is_some_and(|p| p.request.cookie == cookie)
        {
            let prompt = self.prompt.take().expect("checked");
            let mut effects = vec![finish(prompt.request.cookie)];
            effects.extend(self.show_next());
            return effects;
        }
        match self.waiting.iter().position(|r| r.cookie == cookie) {
            Some(i) => vec![finish(self.waiting.remove(i).expect("found").cookie)],
            None => Vec::new(),
        }
    }

    /// The session was locked or left (`away`), or is back. Going away dismisses the prompt on screen; coming back
    /// shows the next waiting one.
    pub fn set_away(&mut self, away: bool) -> Vec<Effect> {
        self.away = away;
        if !away {
            return self.show_next();
        }
        self.prompt
            .take()
            .map(|prompt| Effect::Finish {
                cookie: prompt.request.cookie,
                outcome: Outcome::Cancelled,
            })
            .into_iter()
            .collect()
    }

    /// Picks another of the request's users for the prompt on screen, which starts over with the helper for them.
    pub fn choose_user(&mut self, user: User) -> Vec<Effect> {
        let Some(prompt) = &mut self.prompt else {
            return Vec::new();
        };
        if prompt.user == user || !prompt.request.users.contains(&user) {
            return Vec::new();
        }
        prompt.user = user;
        prompt.phase = Phase::Waiting;
        prompt.field = empty_field();
        prompt.error = None;
        prompt.info = None;
        vec![Effect::StartHelper {
            user,
            cookie: prompt.request.cookie.clone(),
        }]
    }

    /// A key typed into the prompt on screen.
    pub fn key(&mut self, key: Key) -> Vec<Effect> {
        if key == Key::Escape {
            return self.dismiss();
        }
        let Some(prompt) = &mut self.prompt else {
            return Vec::new();
        };
        match key {
            _ if prompt.phase != Phase::Asking => {}
            Key::Char(c) => {
                if !c.is_control() && prompt.field.len() + c.len_utf8() <= MAX_ANSWER {
                    prompt.field.push(c);
                    prompt.error = None;
                }
            }
            Key::Backspace => {
                prompt.field.pop();
                prompt.error = None;
            }
            Key::Escape => unreachable!("handled above"),
            Key::Return if prompt.field.is_empty() => {}
            Key::Return => {
                prompt.sent_len = prompt.field.chars().count();
                prompt.phase = Phase::Checking;
                prompt.error = None;
                // the answer is copied once, into a block of its exact size; the field it came from is zeroed here
                let field = std::mem::replace(&mut prompt.field, empty_field());
                return vec![Effect::Answer(Zeroizing::new(field.as_bytes().to_vec()))];
            }
        }
        Vec::new()
    }

    /// The person dismissed the prompt on screen.
    pub fn dismiss(&mut self) -> Vec<Effect> {
        match &self.prompt {
            Some(prompt) => {
                let cookie = prompt.request.cookie.clone();
                self.cancel(&cookie)
            }
            None => Vec::new(),
        }
    }

    /// A line from the helper of the prompt on screen.
    pub fn helper(&mut self, line: Line) -> Vec<Effect> {
        let Some(prompt) = &mut self.prompt else {
            return Vec::new();
        };
        match line {
            Line::Prompt { text, echo } => {
                prompt.phase = Phase::Asking;
                prompt.question = shown(&text);
                prompt.echo = echo;
                prompt.field = empty_field();
            }
            Line::Error(text) => prompt.error = Some(shown(&text)),
            Line::Info(text) => prompt.info = Some(shown(&text)),
            Line::Success => return self.finish(Outcome::Done),
            Line::Failure => return self.failed(),
        }
        Vec::new()
    }

    /// The helper stopped without a result or broke its protocol: the attempt failed.
    pub fn helper_lost(&mut self) -> Vec<Effect> {
        if self.prompt.is_some() {
            self.failed()
        } else {
            Vec::new()
        }
    }

    fn failed(&mut self) -> Vec<Effect> {
        let Some(prompt) = &mut self.prompt else {
            return Vec::new();
        };
        prompt.failures += 1;
        if prompt.failures >= ATTEMPTS {
            return self.finish(Outcome::Done);
        }
        prompt.phase = Phase::Waiting;
        prompt.error = Some(TRY_AGAIN.to_owned());
        prompt.info = None;
        vec![Effect::StartHelper {
            user: prompt.user,
            cookie: prompt.request.cookie.clone(),
        }]
    }

    fn finish(&mut self, outcome: Outcome) -> Vec<Effect> {
        let Some(prompt) = self.prompt.take() else {
            return Vec::new();
        };
        let mut effects = vec![Effect::Finish {
            cookie: prompt.request.cookie,
            outcome,
        }];
        effects.extend(self.show_next());
        effects
    }

    fn show_next(&mut self) -> Vec<Effect> {
        if self.away || self.prompt.is_some() {
            return Vec::new();
        }
        let Some(request) = self.waiting.pop_front() else {
            return Vec::new();
        };
        let user = if request.users.contains(&self.me) {
            self.me
        } else {
            request.users[0]
        };
        let effect = Effect::StartHelper {
            user,
            cookie: request.cookie.clone(),
        };
        self.prompt = Some(Prompt {
            request,
            user,
            phase: Phase::Waiting,
            question: String::new(),
            echo: false,
            field: empty_field(),
            sent_len: 0,
            error: None,
            info: None,
            failures: 0,
        });
        vec![effect]
    }
}

#[cfg(test)]
mod tests;
