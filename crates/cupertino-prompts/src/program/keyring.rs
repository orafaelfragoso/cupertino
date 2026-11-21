//! The keyring's side of the program: the prompter on its own session bus connection, the offer about the default
//! keyring, and what the person does in the prompt. A caller's call comes as news; the model
//! ([`crate::keyring::model`]) decides, and the reply and the callbacks go out in the order it gave them: to a caller on
//! the bus through the prompter's outbox ([`prompter::send`]), to the offer inside the process on its channels.

use std::time::Duration;

use cupertino_kit::core::{Command, Key};
use cupertino_kit::task;

use crate::keyring::asking::{self, Host, Io, Setup};
use crate::keyring::model::{Effect, Field, Kind, Phase, PromptError, Prompter, MISMATCH};
use crate::keyring::prompter::{self, Call, Outgoing, Owned, Request};
use crate::program::{KeyringInput, Message, News};
use crate::view;

/// How long a file of the offer should take to read or write.
const IO_DEADLINE: Duration = Duration::from_secs(2);

pub struct Keyring {
    prompter: Prompter,
    outbox: (
        async_channel::Sender<Outgoing>,
        async_channel::Receiver<Outgoing>,
    ),
    /// The callbacks to the offer.
    inside: (
        async_channel::Sender<Effect>,
        async_channel::Receiver<Effect>,
    ),
    owned: Option<Owned>,
    /// Whether to make the offer, until it is made.
    offer: bool,
    /// The question whose field last took the focus.
    focused: Option<u32>,
}

/// The offer's work for the I/O pool.
pub fn on_pool(work: Io) -> Command<Message> {
    task::io(IO_DEADLINE, move || work.run(), |()| Message::Idle)
}

/// A call's reply and what follows it.
fn split(result: Result<Vec<Effect>, PromptError>) -> (Result<(), PromptError>, Vec<Effect>) {
    match result {
        Ok(effects) => (Ok(()), effects),
        Err(error) => (Err(error), Vec::new()),
    }
}

impl Keyring {
    pub fn new(offer: bool) -> Keyring {
        Keyring {
            prompter: Prompter::new(),
            outbox: async_channel::unbounded(),
            inside: async_channel::unbounded(),
            owned: None,
            offer,
            focused: None,
        }
    }

    /// Serve the prompter on `bus`, its own connection: the names, the calls, the callers that leave and the outbox.
    pub fn serve(
        &mut self,
        bus: zbus::Connection,
        heard: &async_channel::Sender<News>,
    ) -> Vec<Command<Message>> {
        let (calls, owned) = (heard.clone(), heard.clone());
        let served = bus.clone();
        let serve = Command::Perform(Box::pin(async move {
            Message::Stopped(
                prompter::serve(
                    &served,
                    |taken| {
                        let _ = owned.try_send(News::Owned(taken));
                    },
                    |request| {
                        let _ = calls.try_send(News::Keyring(request));
                    },
                )
                .await,
            )
        }));
        let (left, followed) = (heard.clone(), bus.clone());
        let vanished = Command::Perform(Box::pin(async move {
            Message::Stopped(
                prompter::follow_vanished(&followed, |name| {
                    let _ = left.try_send(News::Keyring(Request::Vanished { name }));
                })
                .await,
            )
        }));
        let outbox = self.outbox.1.clone();
        let send = Command::Perform(Box::pin(async move {
            prompter::send(&bus, outbox).await;
            Message::Idle
        }));
        vec![serve, vanished, send]
    }

    pub fn owned(&mut self, owned: Result<Owned, String>) -> Vec<Command<Message>> {
        match owned {
            Ok(Owned::Now) => eprintln!("cupertino-prompts: serving the keyring's prompts"),
            Ok(Owned::Queued) => {
                eprintln!("cupertino-prompts: another prompter holds the keyring's names; serving once it leaves")
            }
            Err(ref error) => {
                eprintln!("cupertino-prompts: taking the keyring prompter's names: {error}")
            }
        }
        self.owned = owned.ok();
        Vec::new()
    }

    /// Start the offer once the prompter owns its names (only the prompter that shows the prompts asks, so the
    /// question is never drawn by another program), on `bus`, for `login`.
    pub fn start_offer(
        &mut self,
        bus: Option<&zbus::Connection>,
        login: Option<String>,
        heard: &async_channel::Sender<News>,
    ) -> Vec<Command<Message>> {
        let (true, Some(Owned::Now), Some(bus), Some(login)) = (self.offer, self.owned, bus, login)
        else {
            return Vec::new();
        };
        self.offer = false;
        let Some(setup) = Setup::of_session(login) else {
            return Vec::new();
        };
        let (work, requests) = (heard.clone(), heard.clone());
        let host = Host {
            io: Box::new(move |io| {
                let _ = work.try_send(News::Io(io));
            }),
            requests: Box::new(move |request| {
                let _ = requests.try_send(News::Keyring(request));
            }),
            callbacks: self.inside.1.clone(),
        };
        let bus = bus.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::Offered(asking::run(bus, setup, host).await)
        }))]
    }

    /// Send `outgoing` where it goes, in order.
    fn out(&self, outgoing: Outgoing) {
        match outgoing {
            Outgoing::Reply(Call::Inside(answer), result) => {
                let _ = answer.try_send(result);
            }
            Outgoing::Effect(effect) if effect_caller(&effect) == asking::INSIDE => {
                let _ = self.inside.0.try_send(effect);
            }
            outgoing => {
                let _ = self.outbox.0.try_send(outgoing);
            }
        }
    }

    fn effects(&self, effects: Vec<Effect>) {
        for effect in effects {
            self.out(Outgoing::Effect(effect));
        }
    }

    fn reply(&self, call: Call, (result, effects): (Result<(), PromptError>, Vec<Effect>)) {
        self.out(Outgoing::Reply(call, result));
        self.effects(effects);
    }

    /// Decide a caller's call and answer it with what follows.
    pub fn request(&mut self, request: Request) {
        match request {
            Request::Begin { caller, call } => {
                let result = split(self.prompter.begin(caller));
                self.reply(call, result);
            }
            Request::Perform {
                caller,
                kind,
                properties,
                exchange,
                call,
            } => {
                let result = split(
                    self.prompter
                        .perform(&caller, kind, properties, &exchange.0),
                );
                self.reply(call, result);
            }
            Request::Stop { caller, call } => {
                let effects = self.prompter.stop(&caller);
                self.reply(call, (Ok(()), effects));
            }
            Request::Vanished { name } => {
                let effects = self.prompter.vanished(&name);
                self.effects(effects);
            }
        }
    }

    pub fn set_away(&mut self, away: bool) {
        let effects = self.prompter.set_away(away);
        self.effects(effects);
    }

    /// Escape, or the server closed the alert.
    pub fn escape(&mut self) {
        let effects = self.prompter.escape();
        self.effects(effects);
    }

    pub fn input(&mut self, input: KeyringInput) {
        let effects = match input {
            KeyringInput::Field(field, text) => {
                self.prompter.set_field(field, text.expose());
                Vec::new()
            }
            KeyringInput::Choice(_) => {
                self.prompter.toggle_choice();
                Vec::new()
            }
            KeyringInput::Continue => self.prompter.answer(),
            KeyringInput::Cancel => self.prompter.cancel(),
        };
        self.effects(effects);
    }

    /// The caller of the prompt to show.
    pub fn shown_about(&self) -> Option<String> {
        let prompt = self.prompter.shown()?;
        Some(format!("{} {}", prompt.caller.name, prompt.caller.path))
    }

    /// The key of the first field of a question just asked, once.
    pub fn new_question(&mut self) -> Option<Key> {
        let prompt = self
            .prompter
            .shown()
            .filter(|p| p.phase == Phase::Asking && p.kind == Kind::Password)?;
        if self.focused == Some(prompt.asked) {
            return None;
        }
        self.focused = Some(prompt.asked);
        Some(view::keyring_field_key(prompt.asked, Field::Password))
    }

    /// What the prompt on screen shows.
    pub fn view(&self, caps_lock: bool) -> Option<view::Keyring> {
        let prompt = self.prompter.shown()?;
        let properties = &prompt.properties;
        let password = prompt.kind == Kind::Password;
        let mut status = Vec::new();
        if !properties.warning.trim().is_empty() {
            status.push(crate::text::shown(&properties.warning));
        }
        if prompt.mismatch {
            status.push(MISMATCH.to_owned());
        }
        if caps_lock && password {
            status.push(view::CAPS_LOCK.to_owned());
        }
        Some(view::Keyring {
            heading: crate::text::shown(properties.heading()),
            description: crate::text::shown(&properties.description),
            password,
            confirm: prompt.confirms(),
            choice: prompt
                .has_choice()
                .then(|| (properties.choice_title(), properties.choice_chosen)),
            status,
            continue_label: properties.continue_title(),
            cancel_label: properties.cancel_title(),
            asked: prompt.asked,
            checking: prompt.phase == Phase::Checking,
            mismatch: prompt.mismatch,
        })
    }
}

fn effect_caller(effect: &Effect) -> &str {
    match effect {
        Effect::Ready { caller, .. } | Effect::Done { caller } => &caller.name,
    }
}
