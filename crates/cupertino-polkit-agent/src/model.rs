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

/// What the host does for the model.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Start the helper for this user and this cookie, dropping any running one.
    StartHelper { user: User, cookie: String },
    /// Send this answer to the running helper.
    Answer(Zeroizing<Vec<u8>>),
    /// Drop the running helper, then answer the request of this cookie with this outcome.
    Finish { cookie: String, outcome: Outcome },
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
    field: Zeroizing<String>,
    pub error: Option<String>,
    pub info: Option<String>,
    failures: u8,
}

impl Prompt {
    /// Characters in the field.
    pub fn field_len(&self) -> usize {
        self.field.chars().count()
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
        prompt.field = Zeroizing::new(String::new());
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
                }
            }
            Key::Backspace => {
                prompt.field.pop();
            }
            Key::Escape => unreachable!("handled above"),
            Key::Return => {
                prompt.phase = Phase::Checking;
                prompt.error = None;
                let field = std::mem::replace(&mut prompt.field, Zeroizing::new(String::new()));
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
                prompt.field = Zeroizing::new(String::new());
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
            field: Zeroizing::new(String::new()),
            error: None,
            info: None,
            failures: 0,
        });
        vec![effect]
    }
}

#[cfg(test)]
mod tests;
