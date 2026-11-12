//! The keyring prompt as a pure model: who began prompting, what the active caller asks, what is typed, and what goes
//! back. One caller is active at a time; the others wait in the order they began. A caller's prompt stays on screen
//! from its first question until it stops prompting, busy while the caller checks an answer. While the session is
//! locked or not the active one, nothing is on screen: a question asked then is answered as cancelled, the active
//! caller is told its prompt is done, and the waiting callers stay until the session is back.

use std::collections::VecDeque;

use zeroize::Zeroizing;

use crate::keyring::exchange::{Exchange, ExchangeError};

/// The longest password the fields take, in bytes; allocated at this size so typing never moves them.
pub const MAX_PASSWORD: usize = 512;
/// Callers that may wait behind the active one; more are refused.
pub const MAX_WAITING: usize = 8;
/// What the panel says when a new password and its confirmation differ.
pub const MISMATCH: &str = "The passwords do not match.";
pub const CONTINUE: &str = "Continue";
pub const CANCEL: &str = "Cancel";

/// Who asks: the caller's unique bus name and the path of its callback object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Caller {
    pub name: String,
    pub path: String,
}

/// What the caller asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A password, with a confirmation when it is a new one.
    Password,
    /// Continue or cancel.
    Confirm,
}

impl Kind {
    /// The protocol's name of a kind; `None` for any other.
    pub fn parse(name: &str) -> Option<Kind> {
        match name {
            "password" => Some(Kind::Password),
            "confirm" => Some(Kind::Confirm),
            _ => None,
        }
    }
}

/// A property's value as the caller sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Int(i32),
}

/// The prompt's properties, as the caller last set them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Properties {
    pub title: String,
    pub message: String,
    pub description: String,
    pub warning: String,
    pub choice_label: String,
    pub choice_chosen: bool,
    pub password_new: bool,
    pub password_strength: i32,
    /// The caller's window as an X11 id. A Wayland surface can't be parented to it; the prompt is modal over the
    /// active output whatever it names.
    pub caller_window: String,
    pub continue_label: String,
    pub cancel_label: String,
}

impl Properties {
    /// Apply what the caller changed; unknown names and values of the wrong type are ignored.
    pub fn update(&mut self, changed: impl IntoIterator<Item = (String, Value)>) {
        for (name, value) in changed {
            match (name.as_str(), value) {
                ("title", Value::Str(v)) => self.title = v,
                ("message", Value::Str(v)) => self.message = v,
                ("description", Value::Str(v)) => self.description = v,
                ("warning", Value::Str(v)) => self.warning = v,
                ("choice-label", Value::Str(v)) => self.choice_label = v,
                ("choice-chosen", Value::Bool(v)) => self.choice_chosen = v,
                ("password-new", Value::Bool(v)) => self.password_new = v,
                ("password-strength", Value::Int(v)) => self.password_strength = v,
                ("caller-window", Value::Str(v)) => self.caller_window = v,
                ("continue-label", Value::Str(v)) => self.continue_label = v,
                ("cancel-label", Value::Str(v)) => self.cancel_label = v,
                _ => {}
            }
        }
    }

    /// The panel's bold line: the message, else the title.
    pub fn heading(&self) -> &str {
        if self.message.trim().is_empty() {
            &self.title
        } else {
            &self.message
        }
    }

    /// The continue button's title, its mnemonic marks removed.
    pub fn continue_title(&self) -> String {
        label_or(&self.continue_label, CONTINUE)
    }

    pub fn cancel_title(&self) -> String {
        label_or(&self.cancel_label, CANCEL)
    }

    /// The choice's label, its mnemonic marks removed; empty when there is no choice.
    pub fn choice_title(&self) -> String {
        label_or(&self.choice_label, "")
    }
}

/// `label` without its mnemonic underscores (`__` is one underscore), or `default` when it is empty.
pub fn label_or(label: &str, default: &str) -> String {
    let mut out = String::new();
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    if out.trim().is_empty() {
        default.to_owned()
    } else {
        out
    }
}

/// The caller's answer to a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The caller may ask now.
    Ready,
    Yes,
    No,
}

impl Reply {
    pub fn as_str(self) -> &'static str {
        match self {
            Reply::Ready => "",
            Reply::Yes => "yes",
            Reply::No => "no",
        }
    }
}

/// What the host sends a caller. `Debug` never prints an exchange message.
#[derive(Clone, PartialEq, Eq)]
pub enum Effect {
    /// Call the caller's `PromptReady` with this reply, the properties the prompt changed, and the exchange message.
    Ready {
        caller: Caller,
        reply: Reply,
        properties: Vec<(&'static str, Value)>,
        exchange: String,
    },
    /// Call the caller's `PromptDone`: the prompter ended its prompt.
    Done { caller: Caller },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Ready {
                caller,
                reply,
                properties,
                ..
            } => f
                .debug_struct("Ready")
                .field("caller", caller)
                .field("reply", reply)
                .field("properties", properties)
                .finish_non_exhaustive(),
            Effect::Done { caller } => f.debug_struct("Done").field("caller", caller).finish(),
        }
    }
}

/// Why a caller's call is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptError {
    /// The caller is not the active one, or asks while its last question is unanswered.
    InProgress,
    /// Too many callers wait.
    Busy,
    /// The exchange message was refused.
    Exchange(ExchangeError),
}

/// Where the active caller's prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Began, and nothing asked yet: no panel.
    Idle,
    /// The panel takes keys.
    Asking,
    /// Answered; the panel waits busy for the caller's next question or its stop.
    Checking,
}

/// One of the prompt's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Password,
    Confirm,
}

/// A key typed into a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
}

fn empty_field() -> Zeroizing<String> {
    Zeroizing::new(String::with_capacity(MAX_PASSWORD))
}

/// The active caller's prompt.
pub struct Prompt {
    pub caller: Caller,
    exchange: Exchange,
    pub properties: Properties,
    pub kind: Kind,
    pub phase: Phase,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    /// The new password and its confirmation differ.
    pub mismatch: bool,
    /// How many questions the caller asked: a new one gives the panel a fresh focus.
    pub asked: u32,
}

impl Prompt {
    /// Characters a field shows (as bullets).
    pub fn field_len(&self, field: Field) -> usize {
        match field {
            Field::Password => self.password.chars().count(),
            Field::Confirm => self.confirm.chars().count(),
        }
    }

    /// Whether the panel asks for a confirmation of the password.
    pub fn confirms(&self) -> bool {
        self.kind == Kind::Password && self.properties.password_new
    }

    /// Whether the panel shows the choice.
    pub fn has_choice(&self) -> bool {
        !self.properties.choice_label.trim().is_empty()
    }

    /// Whether the panel is on screen: from the caller's first question.
    pub fn shown(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// How strong the typed password is, as the prompter reports it: 0 when empty, else 1.
    pub fn strength(&self) -> i32 {
        i32::from(!self.password.is_empty())
    }

    fn changed(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("choice-chosen", Value::Bool(self.properties.choice_chosen)),
            ("password-strength", Value::Int(self.strength())),
        ]
    }

    fn clear(&mut self) {
        self.password = empty_field();
        self.confirm = empty_field();
        self.mismatch = false;
    }
}

/// Every caller that began prompting and has not stopped.
#[derive(Default)]
pub struct Prompter {
    active: Option<Prompt>,
    waiting: VecDeque<Caller>,
    /// The session is locked, or isn't the active one.
    away: bool,
}

impl Prompter {
    pub fn new() -> Prompter {
        Prompter::default()
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.active.as_ref()
    }

    /// The prompt on screen, if any.
    pub fn shown(&self) -> Option<&Prompt> {
        self.active.as_ref().filter(|p| p.shown())
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// `caller` begins prompting; it becomes active when no other is.
    pub fn begin(&mut self, caller: Caller) -> Result<Vec<Effect>, PromptError> {
        let known = self.active.as_ref().is_some_and(|p| p.caller == caller)
            || self.waiting.contains(&caller);
        if known {
            return Err(PromptError::InProgress);
        }
        if self.waiting.len() >= MAX_WAITING {
            return Err(PromptError::Busy);
        }
        self.waiting.push_back(caller);
        Ok(self.activate_next())
    }

    /// The active `caller` asks `kind` with the properties it changed and its exchange message.
    pub fn perform(
        &mut self,
        caller: &Caller,
        kind: Kind,
        changed: impl IntoIterator<Item = (String, Value)>,
        exchange: &str,
    ) -> Result<Vec<Effect>, PromptError> {
        let away = self.away;
        let prompt = self
            .active
            .as_mut()
            .filter(|p| p.caller == *caller && p.phase != Phase::Asking)
            .ok_or(PromptError::InProgress)?;
        prompt
            .exchange
            .receive(exchange)
            .map_err(PromptError::Exchange)?;
        prompt.properties.update(changed);
        prompt.kind = kind;
        prompt.asked += 1;
        prompt.clear();
        prompt.phase = Phase::Asking;
        if away {
            // nothing is shown while the session is away: the question is answered as cancelled
            return Ok(self.reply(Reply::No));
        }
        Ok(Vec::new())
    }

    /// `caller` stops prompting: its prompt goes away and the next caller becomes active.
    pub fn stop(&mut self, caller: &Caller) -> Vec<Effect> {
        if self.active.as_ref().is_some_and(|p| p.caller == *caller) {
            self.active = None;
            return self.activate_next();
        }
        self.waiting.retain(|c| c != caller);
        Vec::new()
    }

    /// The bus name `name` left the bus: every prompt of it ends without a word to it.
    pub fn vanished(&mut self, name: &str) -> Vec<Effect> {
        self.waiting.retain(|c| c.name != name);
        if self.active.as_ref().is_some_and(|p| p.caller.name == name) {
            self.active = None;
            return self.activate_next();
        }
        Vec::new()
    }

    /// The session was locked or left (`away`), or is back. Going away cancels a question on screen and ends the
    /// active prompt; coming back makes the next waiting caller active.
    pub fn set_away(&mut self, away: bool) -> Vec<Effect> {
        self.away = away;
        if !away {
            return self.activate_next();
        }
        let mut effects = Vec::new();
        if self
            .active
            .as_ref()
            .is_some_and(|p| p.phase == Phase::Asking)
        {
            effects.extend(self.reply(Reply::No));
        }
        if let Some(prompt) = self.active.take() {
            effects.push(Effect::Done {
                caller: prompt.caller,
            });
        }
        effects
    }

    /// A key typed into `field` of the question on screen.
    pub fn key(&mut self, field: Field, key: Key) {
        let Some(prompt) = self.asking_mut() else {
            return;
        };
        if prompt.kind != Kind::Password || (field == Field::Confirm && !prompt.confirms()) {
            return;
        }
        let text = match field {
            Field::Password => &mut prompt.password,
            Field::Confirm => &mut prompt.confirm,
        };
        match key {
            Key::Char(c) => {
                if c.is_control() || text.len() + c.len_utf8() > MAX_PASSWORD {
                    return;
                }
                text.push(c);
            }
            Key::Backspace => {
                text.pop();
            }
        }
        prompt.mismatch = false;
    }

    /// The choice is toggled.
    pub fn toggle_choice(&mut self) {
        if let Some(prompt) = self.asking_mut().filter(|p| p.has_choice()) {
            prompt.properties.choice_chosen = !prompt.properties.choice_chosen;
        }
    }

    /// The person continues: a password goes back through the exchange, a confirmation as yes. A new password that
    /// differs from its confirmation is not sent; the panel says so.
    pub fn answer(&mut self) -> Vec<Effect> {
        let Some(prompt) = self.asking_mut() else {
            return Vec::new();
        };
        if prompt.confirms() && *prompt.password != *prompt.confirm {
            prompt.mismatch = true;
            prompt.confirm = empty_field();
            return Vec::new();
        }
        self.reply(Reply::Yes)
    }

    /// The person cancels the question on screen.
    pub fn cancel(&mut self) -> Vec<Effect> {
        if self.asking_mut().is_none() {
            return Vec::new();
        }
        self.reply(Reply::No)
    }

    fn asking_mut(&mut self) -> Option<&mut Prompt> {
        self.active.as_mut().filter(|p| p.phase == Phase::Asking)
    }

    /// Answer the active caller's question with `reply`; only yes to a password carries the secret.
    fn reply(&mut self, reply: Reply) -> Vec<Effect> {
        let Some(prompt) = self.active.as_mut() else {
            return Vec::new();
        };
        let secret = (reply == Reply::Yes && prompt.kind == Kind::Password)
            .then(|| Zeroizing::new(prompt.password.as_bytes().to_vec()));
        // the exchange can't fail once the caller's key arrived, except for want of random bytes: then it is a no
        let (reply, exchange) = match prompt.exchange.send(secret.as_deref().map(Vec::as_slice)) {
            Ok(message) => (reply, message),
            Err(_) => (Reply::No, prompt.exchange.begin()),
        };
        let properties = prompt.changed();
        prompt.clear();
        prompt.phase = Phase::Checking;
        vec![Effect::Ready {
            caller: prompt.caller.clone(),
            reply,
            properties,
            exchange,
        }]
    }

    fn activate_next(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        while !self.away && self.active.is_none() {
            let Some(caller) = self.waiting.pop_front() else {
                break;
            };
            match Exchange::new() {
                Ok(exchange) => {
                    effects.push(Effect::Ready {
                        caller: caller.clone(),
                        reply: Reply::Ready,
                        properties: Vec::new(),
                        exchange: exchange.begin(),
                    });
                    self.active = Some(Prompt {
                        caller,
                        exchange,
                        properties: Properties::default(),
                        kind: Kind::Password,
                        phase: Phase::Idle,
                        password: empty_field(),
                        confirm: empty_field(),
                        mismatch: false,
                        asked: 0,
                    });
                }
                // without random bytes there is no prompt: the caller hears it is done
                Err(_) => effects.push(Effect::Done { caller }),
            }
        }
        effects
    }
}

#[cfg(test)]
pub(crate) mod tests;
