//! The polkit agent's side of the program: the agent on the system bus, the helper of the prompt on screen, and what
//! the person does in the prompt. polkitd's requests come through the agent ([`crate::polkit::agent`]) as news; the
//! model ([`crate::polkit::model`]) decides, and what it asks for is carried out here: the helper started on the I/O
//! pool and read by a future of the loop, an answer written to it, polkitd answered once a prompt is over.

use std::collections::HashMap;
use std::time::Duration;

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use cupertino_kit::core::{Command, Key};
use cupertino_kit::task;

use crate::polkit::agent;
use crate::polkit::helper::{AnswerSender, Answers, Helper, Line, Route};
use crate::polkit::model::{Agent, Effect, Phase, ATTEMPTS};
use crate::polkit::{detail_rows, Event, Outcome, Reply, User};
use crate::program::{AuthInput, Handoff, Message, News, Secret};
use crate::users::{self, display_names};
use crate::{logind, view};

/// How long finding the helper, reading the password database or starting the helper should take.
const IO_DEADLINE: Duration = Duration::from_secs(2);

/// The helper of the prompt on screen.
struct Running {
    run: u64,
    /// Once it started.
    answers: Option<(Answers, AnswerSender)>,
}

/// What the prompt on screen shows that the model does not hold, read once per request.
struct Looked {
    cookie: String,
    users: Vec<(User, String)>,
    details: Vec<(&'static str, String)>,
}

pub struct Authentication {
    agent: Agent,
    replies: HashMap<String, Reply>,
    /// polkit's helper; `None` until it is found.
    routes: Option<Vec<Route>>,
    helper: Option<Running>,
    runs: u64,
    /// The agent is registered, or will never be.
    started: bool,
    looked: Option<Looked>,
    looking: Option<String>,
    /// The question whose field last took the focus.
    focused: Option<u32>,
    details: bool,
    menu: bool,
    secret: Secret,
    text: (String, u64),
}

/// The locale polkitd translates the action's message into.
fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "C".to_owned())
}

/// Starts the helper for `user` and `cookie` through `routes`. Reads the password database and connects or starts a
/// process: run it on the I/O pool.
fn start_helper(routes: &[Route], user: User, cookie: &str) -> Result<Helper, String> {
    let account = users::account(user).ok_or("the user is not in the password database")?;
    Helper::start(routes, &account.login, cookie).map_err(|e| e.to_string())
}

impl Authentication {
    /// The agent's side, and the commands that find polkit's helper when `routes` does not name it.
    pub fn new(routes: Option<Vec<Route>>) -> (Authentication, Vec<Command<Message>>) {
        let find = match &routes {
            Some(_) => Vec::new(),
            None => vec![task::io(IO_DEADLINE, Route::find, Message::Routes)],
        };
        let auth = Authentication {
            agent: Agent::new(User(cupertino_kit::account::uid())),
            replies: HashMap::new(),
            routes,
            helper: None,
            runs: 0,
            started: false,
            looked: None,
            looking: None,
            focused: None,
            details: false,
            menu: false,
            secret: Secret::default(),
            text: (String::new(), 0),
        };
        (auth, find)
    }

    pub fn found(&mut self, routes: Vec<Route>) {
        self.routes = Some(routes);
    }

    /// Export the agent and register it with polkitd once the helper is found, the system bus is there and logind
    /// named the session. Without a helper or a session the agent never registers, so polkitd asks another one.
    pub fn start(
        &mut self,
        system: Option<&zbus::Connection>,
        login: Option<&Option<logind::Session>>,
        heard: &async_channel::Sender<News>,
    ) -> Vec<Command<Message>> {
        let (Some(routes), Some(system), Some(login)) = (&self.routes, system, login) else {
            return Vec::new();
        };
        if self.started {
            return Vec::new();
        }
        self.started = true;
        if routes.is_empty() {
            eprintln!("cupertino-prompts: polkit's agent helper is not installed; no authentication agent");
            return Vec::new();
        }
        let Some(session) = login else {
            eprintln!(
                "cupertino-prompts: no logind session to register the authentication agent for"
            );
            return Vec::new();
        };
        let (system, id, heard) = (system.clone(), session.id.clone(), heard.clone());
        vec![Command::Perform(Box::pin(async move {
            let events = heard.clone();
            let exported = agent::export(&system, move |event| {
                events.try_send(News::Polkit(event)).is_ok()
            })
            .await;
            if let Err(error) = exported {
                return Message::Stopped(format!("exporting the authentication agent: {error}"));
            }
            Message::Stopped(
                agent::follow(&system, &id, &locale(), |registered| {
                    let _ = heard.try_send(News::Registered(registered));
                })
                .await,
            )
        }))]
    }

    pub fn polkit(&mut self, event: Event) -> Vec<Command<Message>> {
        let effects = match event {
            Event::Begin(request, reply) => {
                self.replies.insert(request.cookie.clone(), reply);
                self.agent.begin(request)
            }
            Event::Cancel(cookie) => self.agent.cancel(&cookie),
        };
        self.apply(effects)
    }

    pub fn set_away(&mut self, away: bool) -> Vec<Command<Message>> {
        if self.agent.away() == away {
            return Vec::new();
        }
        let effects = self.agent.set_away(away);
        self.apply(effects)
    }

    pub fn dismiss(&mut self) -> Vec<Command<Message>> {
        let effects = self.agent.dismiss();
        self.apply(effects)
    }

    fn is_current(&self, run: u64) -> bool {
        self.helper.as_ref().is_some_and(|h| h.run == run)
    }

    /// The helper started as `run` is there, or did not start.
    pub fn helper_started(
        &mut self,
        run: u64,
        helper: Handoff<Result<Helper, String>>,
        heard: &async_channel::Sender<News>,
    ) -> Vec<Command<Message>> {
        let Some(started) = helper.take() else {
            return Vec::new();
        };
        if !self.is_current(run) {
            // replaced while it started: it ends
            return match started {
                Ok(helper) => vec![end(helper.split().1)],
                Err(_) => Vec::new(),
            };
        }
        let helper = match started {
            Ok(helper) => helper,
            Err(error) => {
                eprintln!("cupertino-prompts: starting polkit's helper: {error}");
                self.auth_failed("helper_start", Some(&error));
                self.helper = None;
                let effects = self.agent.helper_lost();
                return self.apply(effects);
            }
        };
        let (mut lines, answers) = helper.split();
        let sender = answers.sender();
        if let Some(running) = &mut self.helper {
            running.answers = Some((answers, sender));
        }
        let heard = heard.clone();
        vec![Command::Perform(Box::pin(async move {
            loop {
                let line = lines.read_line().await.map_err(|e| e.to_string());
                let last = !matches!(
                    line,
                    Ok(Line::Prompt { .. } | Line::Error(_) | Line::Info(_))
                );
                if heard.send(News::Helper { run, line }).await.is_err() || last {
                    return Message::Idle;
                }
            }
        }))]
    }

    /// A line of the helper started as `run`, or why it can't be read.
    pub fn helper_line(&mut self, run: u64, line: Result<Line, String>) -> Vec<Command<Message>> {
        if !self.is_current(run) {
            return Vec::new();
        }
        let effects = match line {
            Ok(line) => {
                if matches!(line, Line::Failure) {
                    self.auth_failed("refused", None);
                }
                self.agent.helper(line)
            }
            Err(error) => {
                eprintln!("cupertino-prompts: polkit's helper: {error}");
                self.auth_failed("helper_failed", Some(&error));
                let mut commands = self.end_helper();
                commands.extend({
                    let effects = self.agent.helper_lost();
                    self.apply(effects)
                });
                return commands;
            }
        };
        self.apply(effects)
    }

    pub fn answer_failed(&mut self, run: u64, error: &str) -> Vec<Command<Message>> {
        if !self.is_current(run) {
            return Vec::new();
        }
        eprintln!("cupertino-prompts: answering polkit's helper: {error}");
        self.auth_failed("helper_answer", Some(error));
        let mut commands = self.end_helper();
        let effects = self.agent.helper_lost();
        commands.extend(self.apply(effects));
        commands
    }

    /// The helper of the prompt on screen ends: a started process is killed and waited for on the I/O pool.
    fn end_helper(&mut self) -> Vec<Command<Message>> {
        match self.helper.take().and_then(|h| h.answers) {
            Some((answers, _)) => vec![end(answers)],
            None => Vec::new(),
        }
    }

    /// Carry out what the model asks, and what that leads to.
    fn apply(&mut self, effects: Vec<Effect>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for effect in effects {
            match effect {
                Effect::StartHelper { user, cookie } => {
                    commands.extend(self.end_helper());
                    self.runs += 1;
                    let run = self.runs;
                    self.helper = Some(Running { run, answers: None });
                    let routes = self.routes.clone().unwrap_or_default();
                    commands.push(task::io(
                        IO_DEADLINE,
                        move || start_helper(&routes, user, &cookie),
                        move |started| Message::HelperStarted {
                            run,
                            helper: Handoff::new(started),
                        },
                    ));
                }
                Effect::Answer(answer) => {
                    self.secret = Secret::default();
                    let running = self.helper.as_ref().and_then(|h| {
                        h.answers
                            .as_ref()
                            .map(|(_, sender)| (h.run, sender.clone()))
                    });
                    match running {
                        Some((run, sender)) => {
                            commands.push(Command::Perform(Box::pin(async move {
                                match sender.answer(&answer).await {
                                    Ok(()) => Message::Idle,
                                    Err(e) => Message::AnswerFailed {
                                        run,
                                        error: e.to_string(),
                                    },
                                }
                            })))
                        }
                        None => {
                            self.auth_failed("helper_answer", Some("no helper runs"));
                            commands.extend(self.end_helper());
                            let lost = self.agent.helper_lost();
                            commands.extend(self.apply(lost));
                        }
                    }
                }
                Effect::Finish { cookie, outcome } => {
                    commands.extend(self.end_helper());
                    if let Some(reply) = self.replies.remove(&cookie) {
                        reply.send(outcome);
                    }
                    if outcome == Outcome::Cancelled {
                        eprintln!("cupertino-prompts: an authentication was cancelled");
                    }
                }
            }
        }
        commands
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

    /// The cookie of the prompt to show, once what it shows was read and polkit's helper said something: a field
    /// shown before the helper asks would take keys its first question then clears.
    pub fn shown_about(&self) -> Option<String> {
        let prompt = self.agent.prompt()?;
        let looked = self.looked.as_ref()?;
        let spoken = prompt.asked > 0
            || prompt.error.is_some()
            || prompt.info.is_some()
            || prompt.failures() > 0;
        (spoken && looked.cookie == prompt.request.cookie).then(|| looked.cookie.clone())
    }

    /// A new prompt is on screen: the details are closed, the menu too, and nothing is typed.
    pub fn reset_view(&mut self) {
        self.details = false;
        self.menu = false;
        self.secret = Secret::default();
        self.text = (String::new(), self.text.1 + 1);
    }

    pub fn details_shown(&self) -> bool {
        self.details
    }

    /// Read the users' names and the details of the request now on screen, on the I/O pool.
    pub fn look_up(&mut self) -> Vec<Command<Message>> {
        let Some(prompt) = self.agent.prompt() else {
            return Vec::new();
        };
        let cookie = &prompt.request.cookie;
        if self.looked.as_ref().is_some_and(|l| l.cookie == *cookie)
            || self.looking.as_ref() == Some(cookie)
        {
            return Vec::new();
        }
        self.looking = Some(cookie.clone());
        let request = prompt.request.clone();
        vec![task::io(
            IO_DEADLINE,
            move || {
                let accounts: Vec<_> = request
                    .users
                    .iter()
                    .map(|u| (*u, users::account(*u)))
                    .collect();
                let users = request
                    .users
                    .iter()
                    .copied()
                    .zip(display_names(&accounts))
                    .collect();
                (request.cookie.clone(), users, detail_rows(&request))
            },
            |(cookie, users, details)| Message::Looked {
                cookie,
                users,
                details,
            },
        )]
    }

    pub fn looked(
        &mut self,
        cookie: String,
        users: Vec<(User, String)>,
        details: Vec<(&'static str, String)>,
    ) {
        if self.looking.as_ref() == Some(&cookie) {
            self.looking = None;
        }
        self.looked = Some(Looked {
            cookie,
            users,
            details,
        });
    }

    /// The key of the field of a question the helper just asked, once.
    pub fn new_question(&mut self) -> Option<Key> {
        let prompt = self.agent.prompt().filter(|p| p.phase == Phase::Asking)?;
        if self.focused == Some(prompt.asked) {
            return None;
        }
        self.focused = Some(prompt.asked);
        Some(view::field_key(prompt.asked))
    }

    pub fn input(&mut self, input: AuthInput) -> Vec<Command<Message>> {
        let effects = match input {
            AuthInput::Secret(secret) => {
                self.secret = secret;
                self.agent.edited();
                Vec::new()
            }
            AuthInput::Text(text) => {
                self.text.0 = text;
                self.agent.edited();
                Vec::new()
            }
            AuthInput::SubmitSecret(secret) => self.agent.submit(secret.expose().as_bytes()),
            AuthInput::Submit => {
                let echo = self.agent.prompt().is_some_and(|p| p.echo);
                if echo {
                    let text = std::mem::take(&mut self.text.0);
                    self.agent.submit(text.as_bytes())
                } else {
                    let secret = self.secret.clone();
                    self.agent.submit(secret.expose().as_bytes())
                }
            }
            AuthInput::Cancel => self.agent.dismiss(),
            AuthInput::Details(open) => {
                self.details = open;
                Vec::new()
            }
            AuthInput::Users(open) => {
                self.menu = open;
                Vec::new()
            }
            AuthInput::User(index) => {
                self.menu = false;
                let user = self
                    .agent
                    .prompt()
                    .and_then(|p| p.request.users.get(index).copied());
                match user {
                    Some(user) => self.agent.choose_user(user),
                    None => Vec::new(),
                }
            }
        };
        self.apply(effects)
    }

    /// What the prompt on screen shows.
    pub fn view(&self, caps_lock: bool) -> Option<view::Authentication> {
        let prompt = self.agent.prompt()?;
        let looked = self
            .looked
            .as_ref()
            .filter(|l| l.cookie == prompt.request.cookie)?;
        let mut status = Vec::new();
        status.extend(prompt.error.clone());
        status.extend(prompt.info.clone());
        if caps_lock && !prompt.echo {
            status.push(view::CAPS_LOCK.to_owned());
        }
        Some(view::Authentication {
            message: prompt.request.message.clone(),
            users: looked.users.iter().map(|(_, name)| name.clone()).collect(),
            user: looked
                .users
                .iter()
                .position(|(u, _)| *u == prompt.user)
                .unwrap_or(0),
            menu: self.menu,
            question: view::field_label(&prompt.question),
            echo: prompt.echo.then(|| self.text.clone()),
            asked: prompt.asked,
            asking: prompt.phase == Phase::Asking,
            can_submit: if prompt.echo {
                !self.text.0.is_empty()
            } else {
                !self.secret.expose().is_empty()
            },
            checking: prompt.phase == Phase::Checking,
            failed: prompt.error.is_some(),
            status,
            details: looked.details.clone(),
            details_shown: self.details,
        })
    }
}

/// The conversation with a helper ends, on the I/O pool.
fn end(answers: Answers) -> Command<Message> {
    task::io(IO_DEADLINE, move || answers.end(), |()| Message::Idle)
}
