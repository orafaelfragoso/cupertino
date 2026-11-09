//! What the prompt's host does for the [`model`](crate::model): it starts polkit's helper for the prompt on screen,
//! passes the answer on, and answers polkitd once a prompt is over. Each helper reads its lines on a thread of its own
//! and hands them to `on_line` tagged with its run; the host passes them back with [`Conversation::helper_line`], and a
//! line of a run that was replaced is ignored. Nothing here draws, so the whole path from polkitd's request to its
//! answer runs in tests without a display.

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use cupertino_diagnostic_events::{self as diag, Severity, Value};

use crate::agent::{Outcome, Reply, Request, User};
use crate::helper::{Answers, Helper, HelperError, Line, Route};
use crate::model::{Agent, Effect, ATTEMPTS};
use crate::users;

/// Where a helper's lines go: the run they belong to and the line, or how the helper ended.
pub type OnLine = Arc<dyn Fn(u64, Result<Line, HelperError>) + Send + Sync>;

pub struct Conversation {
    agent: Agent,
    replies: HashMap<String, Reply>,
    routes: Vec<Route>,
    helper: Option<(u64, Answers)>,
    runs: u64,
    on_line: OnLine,
}

impl Conversation {
    pub fn new(
        agent: Agent,
        routes: Vec<Route>,
        on_line: impl Fn(u64, Result<Line, HelperError>) + Send + Sync + 'static,
    ) -> Conversation {
        Conversation {
            agent,
            replies: HashMap::new(),
            routes,
            helper: None,
            runs: 0,
            on_line: Arc::new(on_line),
        }
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// The model, for the person's keys and choices; pass what it returns to [`Conversation::apply`].
    pub fn agent_mut(&mut self) -> &mut Agent {
        &mut self.agent
    }

    /// polkitd asks for `request`; `reply` is answered when its prompt is over.
    pub fn begin(&mut self, request: Request, reply: Reply) {
        self.replies.insert(request.cookie.clone(), reply);
        let effects = self.agent.begin(request);
        self.apply(effects);
    }

    /// polkitd withdraws the request of `cookie`.
    pub fn cancel(&mut self, cookie: &str) {
        let effects = self.agent.cancel(cookie);
        self.apply(effects);
    }

    /// Whether `run` is the helper of the prompt on screen.
    pub fn is_current(&self, run: u64) -> bool {
        self.helper
            .as_ref()
            .is_some_and(|(current, _)| *current == run)
    }

    /// A line of the helper started as `run`, or how it ended.
    pub fn helper_line(&mut self, run: u64, line: Result<Line, HelperError>) {
        if !self.is_current(run) {
            return;
        }
        let effects = match line {
            Ok(line) => {
                if matches!(line, Line::Failure) {
                    self.auth_failed("refused", None);
                }
                self.agent.helper(line)
            }
            Err(error) => {
                eprintln!("cupertino-polkit-agent: the helper: {error}");
                self.auth_failed("helper_failed", Some(&error.to_string()));
                self.helper = None;
                self.agent.helper_lost()
            }
        };
        self.apply(effects);
    }

    /// Carry out what the model asks, and what that leads to.
    pub fn apply(&mut self, effects: Vec<Effect>) {
        let mut effects = std::collections::VecDeque::from(effects);
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::StartHelper { user, cookie } => {
                    self.helper = None;
                    if let Err(error) = self.start_helper(user, &cookie) {
                        eprintln!("cupertino-polkit-agent: starting the helper: {error}");
                        self.auth_failed("helper_start", Some(&error.to_string()));
                        effects.extend(self.agent.helper_lost());
                    }
                }
                Effect::Answer(answer) => {
                    let sent = match &mut self.helper {
                        Some((_, answers)) => answers.answer(&answer),
                        None => Err(std::io::Error::other("no helper runs")),
                    };
                    if let Err(error) = sent {
                        eprintln!("cupertino-polkit-agent: answering the helper: {error}");
                        self.auth_failed("helper_answer", Some(&error.to_string()));
                        self.helper = None;
                        effects.extend(self.agent.helper_lost());
                    }
                }
                Effect::Finish { cookie, outcome } => {
                    self.helper = None;
                    if let Some(reply) = self.replies.remove(&cookie) {
                        reply.send(outcome);
                    }
                    if outcome == Outcome::Cancelled {
                        eprintln!("cupertino-polkit-agent: an authentication was cancelled");
                    }
                }
            }
        }
    }

    /// Records `polkit.auth.failed` for the attempt of the prompt on screen: `reason` is `refused` (PAM refused what was
    /// typed) or how polkit's helper failed (`helper_failed`, `helper_start`, `helper_answer`, with its `error`). It
    /// names the action and the attempt, never the user or what was typed.
    fn auth_failed(&self, reason: &'static str, error: Option<&str>) {
        let Some(prompt) = self.agent.prompt() else {
            return;
        };
        let attempt = prompt.failures() + 1;
        let mut event = diag::event(
            Severity::Warn,
            "polkit.auth.failed",
            "an authentication attempt failed",
        )
        .field("action", Value::Public(&prompt.request.action))
        .field("reason", Value::Public(reason))
        .field("attempt", Value::Count(u64::from(attempt)))
        .field("last", Value::Bool(attempt >= ATTEMPTS));
        if let Some(error) = error {
            event = event.field("error", Value::Public(error));
        }
        event.emit();
    }

    /// Start the helper for `user` and read its lines on a thread of its own.
    fn start_helper(&mut self, user: User, cookie: &str) -> Result<(), Box<dyn Error>> {
        let account = users::account(user).ok_or("the user is not in the password database")?;
        let (mut lines, answers) = Helper::start(&self.routes, &account.login, cookie)?.split();
        self.runs += 1;
        let run = self.runs;
        self.helper = Some((run, answers));
        let on_line = Arc::clone(&self.on_line);
        std::thread::Builder::new()
            .name("polkit-helper".to_owned())
            .spawn(move || loop {
                let line = lines.read_line();
                let last = !matches!(
                    line,
                    Ok(Line::Prompt { .. } | Line::Error(_) | Line::Info(_))
                );
                on_line(run, line);
                if last {
                    return;
                }
            })?;
        Ok(())
    }
}
