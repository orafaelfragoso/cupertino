//! The authentication prompt as a pure model: what polkitd asked, what the helper says, and when to start the helper,
//! answer it or tell polkitd the authentication is over. One prompt is on screen at a time; the others wait in
//! polkitd's order. While the session is locked or not the active one, nothing is on screen: a prompt showing then is
//! dismissed, and the waiting ones stay until the session is back.
//!
//! The password itself is the field's (`cupertino_kit::controls::SecureField`): the model is handed a copy when the
//! person submits it ([`Agent::submit`]) and passes it on to the helper at once.

use std::collections::VecDeque;

use zeroize::Zeroizing;

use crate::polkit::helper::Line;
use crate::polkit::{Outcome, Request, User};
use crate::text::shown;

/// Helper runs per prompt: after this many failures polkitd is told the authentication is over, unauthorised.
pub const ATTEMPTS: u8 = 3;
/// What the prompt says after a failed attempt.
pub const TRY_AGAIN: &str = "Incorrect password. Try again.";
/// The longest answer the field takes, in bytes.
pub const MAX_ANSWER: usize = 512;

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

/// Where the prompt on screen is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The helper runs and hasn't asked anything yet.
    Waiting,
    /// The helper asked; the field takes the answer.
    Asking,
    /// The answer went to the helper.
    Checking,
}

/// The prompt on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub request: Request,
    pub user: User,
    pub phase: Phase,
    /// What PAM asks for, such as "Password: ".
    pub question: String,
    /// Whether the field shows what is typed; false for a secret.
    pub echo: bool,
    pub error: Option<String>,
    pub info: Option<String>,
    failures: u8,
    /// How many questions the helper asked: each one is a fresh, empty field.
    pub asked: u32,
}

impl Prompt {
    /// How many attempts of this prompt have failed so far.
    pub fn failures(&self) -> u8 {
        self.failures
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

    /// Whether the session is locked or not the active one.
    pub fn away(&self) -> bool {
        self.away
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
        prompt.error = None;
        prompt.info = None;
        prompt.asked += 1;
        vec![Effect::StartHelper {
            user,
            cookie: prompt.request.cookie.clone(),
        }]
    }

    /// The person edited the field: an error about the last answer no longer stands.
    pub fn edited(&mut self) {
        if let Some(prompt) = self.prompt.as_mut().filter(|p| p.phase == Phase::Asking) {
            prompt.error = None;
        }
    }

    /// The person submits `answer` to the question on screen. Nothing happens unless the helper asked and the answer
    /// is neither empty, longer than [`MAX_ANSWER`] nor broken by a control character.
    pub fn submit(&mut self, answer: &[u8]) -> Vec<Effect> {
        let Some(prompt) = self.prompt.as_mut().filter(|p| p.phase == Phase::Asking) else {
            return Vec::new();
        };
        if answer.is_empty() || answer.len() > MAX_ANSWER || answer.contains(&b'\n') {
            return Vec::new();
        }
        prompt.phase = Phase::Checking;
        prompt.error = None;
        vec![Effect::Answer(Zeroizing::new(answer.to_vec()))]
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
                prompt.asked += 1;
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
            error: None,
            info: None,
            failures: 0,
            asked: 0,
        });
        vec![effect]
    }
}
