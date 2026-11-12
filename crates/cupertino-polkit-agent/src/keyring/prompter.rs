//! The prompter on the session bus: `org.gnome.keyring.internal.Prompter` at `/org/gnome/keyring/Prompter`, under the
//! system and the private prompter's names. A caller (the keyring daemon) begins prompting with the path of its
//! callback object, asks one question at a time, and stops; the prompter answers each through the callback's
//! `PromptReady` and tells it `PromptDone` when it ends a prompt itself. Calls only reach the loop as [`Request`]s;
//! the loop's model decides and [`send`] carries its effects back.

use std::collections::HashMap;

use std::sync::mpsc::{channel, Sender};

use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::zvariant::{ObjectPath, OwnedValue, Value as Variant};

use crate::keyring::model::{Caller, Effect, Kind, PromptError, Value};

/// The names a keyring daemon asks its prompts on.
pub const SYSTEM_NAME: &str = "org.gnome.keyring.SystemPrompter";
pub const PRIVATE_NAME: &str = "org.gnome.keyring.PrivatePrompter";
pub const PATH: &str = "/org/gnome/keyring/Prompter";
pub const INTERFACE: &str = "org.gnome.keyring.internal.Prompter";
pub const CALLBACK_INTERFACE: &str = "org.gnome.keyring.internal.Prompter.Callback";
/// The errors a caller understands.
pub const ERROR_IN_PROGRESS: &str = "org.gnome.keyring.Prompter.InProgress";
pub const ERROR_FAILED: &str = "org.gnome.keyring.Prompter.Failed";

/// What the loop answers a call: the call's result, then the effects to send once the call has its reply.
pub type Answer = (Result<(), PromptError>, Vec<Effect>);

/// A call that reached the prompter, for the loop.
pub enum Request {
    Begin {
        caller: Caller,
        answer: Sender<Answer>,
    },
    Perform {
        caller: Caller,
        kind: Kind,
        properties: Vec<(String, Value)>,
        exchange: String,
        answer: Sender<Answer>,
    },
    Stop {
        caller: Caller,
        answer: Sender<Answer>,
    },
    /// A bus name left the bus.
    Vanished { name: String },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Begin { caller, .. } => write!(f, "Begin({caller:?})"),
            Request::Perform { caller, kind, .. } => write!(f, "Perform({caller:?}, {kind:?})"),
            Request::Stop { caller, .. } => write!(f, "Stop({caller:?})"),
            Request::Vanished { name } => write!(f, "Vanished({name})"),
        }
    }
}

/// The error name and text a refused call answers.
fn error_reply(error: &PromptError) -> (&'static str, String) {
    match error {
        PromptError::InProgress => (ERROR_IN_PROGRESS, "another prompt is in progress".into()),
        PromptError::Busy => (ERROR_FAILED, "too many prompts wait".into()),
        PromptError::Exchange(e) => (ERROR_FAILED, e.to_string()),
    }
}

/// A property's value as the model takes it; other types are dropped.
fn value(variant: &Variant<'_>) -> Option<Value> {
    match variant {
        Variant::Str(s) => Some(Value::Str(s.to_string())),
        Variant::Bool(b) => Some(Value::Bool(*b)),
        Variant::I32(i) => Some(Value::Int(*i)),
        Variant::Value(inner) => value(inner),
        _ => None,
    }
}

/// How the prompter came to its names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owned {
    /// It owns them.
    Now,
    /// Another prompter holds one; the bus gives it over when that one leaves.
    Queued,
}

/// Serve the prompter on `bus`: a thread that takes the calls to its object, a watch for callers leaving, then both
/// names, which replace a prompter that allows it and are never given up to another. Every call reaches `to_loop`;
/// the thread waits for the loop's [`Answer`], replies, and only then sends what the answer leads to, so a caller
/// always has its reply before its callback is called.
pub fn serve(
    bus: &Connection,
    to_loop: impl Fn(Request) + Send + Sync + Clone + 'static,
) -> zbus::Result<Owned> {
    let calls = zbus::blocking::MessageIterator::from(bus);
    let conn = bus.clone();
    let calls_to_loop = to_loop.clone();
    std::thread::Builder::new()
        .name("keyring-calls".into())
        .spawn(move || {
            for message in calls.flatten() {
                if message.message_type() == zbus::message::Type::MethodCall {
                    if let Err(error) = call(&conn, &message, &calls_to_loop) {
                        eprintln!("cupertino-keyring-prompter: answering a call: {error}");
                    }
                }
            }
        })
        .map_err(|e| zbus::Error::InputOutput(std::sync::Arc::new(e)))?;
    watch_vanished(bus, to_loop)?;
    let mut owned = Owned::Now;
    let proxy = zbus::blocking::fdo::DBusProxy::new(bus)?;
    for name in [SYSTEM_NAME, PRIVATE_NAME] {
        let reply =
            proxy.request_name(name.try_into()?, RequestNameFlags::ReplaceExisting.into())?;
        if reply == RequestNameReply::InQueue {
            owned = Owned::Queued;
        }
    }
    Ok(owned)
}

/// Answer one method call to the prompter's connection.
fn call(bus: &Connection, message: &zbus::Message, to_loop: &impl Fn(Request)) -> zbus::Result<()> {
    let header = message.header();
    let member = header.member().map(|m| m.as_str()).unwrap_or_default();
    let interface = header.interface().map(|i| i.as_str());
    let path = header.path().map(|p| p.as_str());
    if interface == Some("org.freedesktop.DBus.Peer") && member == "Ping" {
        return bus.reply(&header, &());
    }
    if path != Some(PATH) || !matches!(interface, None | Some(INTERFACE)) {
        return bus.reply_error(
            &header,
            "org.freedesktop.DBus.Error.UnknownMethod",
            &"no such method",
        );
    }
    let Some(sender) = header.sender().map(|s| s.to_string()) else {
        return Ok(());
    };
    let caller = |path: ObjectPath<'_>| Caller {
        name: sender.clone(),
        path: path.to_string(),
    };
    let (answer, answered) = channel();
    let body = message.body();
    let request = match member {
        "BeginPrompting" => body
            .deserialize::<ObjectPath<'_>>()
            .map(|path| Request::Begin {
                caller: caller(path),
                answer,
            }),
        "StopPrompting" => body
            .deserialize::<ObjectPath<'_>>()
            .map(|path| Request::Stop {
                caller: caller(path),
                answer,
            }),
        "PerformPrompt" => body
            .deserialize::<(ObjectPath<'_>, String, HashMap<String, OwnedValue>, String)>()
            .and_then(|(path, kind, properties, exchange)| {
                let kind = Kind::parse(&kind)
                    .ok_or_else(|| zbus::Error::Failure(format!("no prompt of type {kind:?}")))?;
                let properties = properties
                    .iter()
                    .filter_map(|(name, v)| value(v).map(|v| (name.clone(), v)))
                    .collect();
                Ok(Request::Perform {
                    caller: caller(path),
                    kind,
                    properties,
                    exchange,
                    answer,
                })
            }),
        _ => {
            return bus.reply_error(
                &header,
                "org.freedesktop.DBus.Error.UnknownMethod",
                &"no such method",
            )
        }
    };
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return bus.reply_error(
                &header,
                "org.freedesktop.DBus.Error.InvalidArgs",
                &error.to_string(),
            )
        }
    };
    to_loop(request);
    let Ok((result, effects)) = answered.recv() else {
        return bus.reply_error(&header, ERROR_FAILED, &"the prompter is ending");
    };
    match result {
        Ok(()) => bus.reply(&header, &())?,
        Err(error) => {
            let (name, text) = error_reply(&error);
            bus.reply_error(&header, name, &text)?
        }
    }
    for effect in &effects {
        send(bus, effect)?;
    }
    Ok(())
}

/// Tell the loop each bus name that leaves, believed only from the bus daemon itself.
fn watch_vanished(
    bus: &Connection,
    to_loop: impl Fn(Request) + Send + 'static,
) -> zbus::Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .build();
    let messages = zbus::blocking::MessageIterator::for_match_rule(rule, bus, Some(64))?;
    std::thread::Builder::new()
        .name("keyring-callers".into())
        .spawn(move || {
            for message in messages.flatten() {
                let header = message.header();
                if header.sender().map(|s| s.as_str()) != Some("org.freedesktop.DBus") {
                    continue;
                }
                let Ok((name, _old, new)) =
                    message.body().deserialize::<(String, String, String)>()
                else {
                    continue;
                };
                if new.is_empty() && name.starts_with(':') {
                    to_loop(Request::Vanished { name });
                }
            }
        })
        .map_err(|e| zbus::Error::InputOutput(std::sync::Arc::new(e)))?;
    Ok(())
}

/// Carry `effect` to its caller without waiting for the reply, so the loop never blocks on a caller.
pub fn send(bus: &Connection, effect: &Effect) -> zbus::Result<()> {
    let (caller, member) = match effect {
        Effect::Ready { caller, .. } => (caller, "PromptReady"),
        Effect::Done { caller } => (caller, "PromptDone"),
    };
    let builder = zbus::Message::method_call(caller.path.as_str(), member)?
        .destination(caller.name.as_str())?
        .interface(CALLBACK_INTERFACE)?;
    let message = match effect {
        Effect::Ready {
            reply,
            properties,
            exchange,
            ..
        } => {
            let properties: HashMap<&str, Variant<'_>> = properties
                .iter()
                .map(|(name, v)| {
                    (
                        *name,
                        match v {
                            Value::Str(s) => Variant::from(s.as_str()),
                            Value::Bool(b) => Variant::from(*b),
                            Value::Int(i) => Variant::from(*i),
                        },
                    )
                })
                .collect();
            builder.build(&(reply.as_str(), properties, exchange.as_str()))?
        }
        Effect::Done { .. } => builder.build(&())?,
    };
    bus.send(&message)
}

fn split(result: Result<Vec<Effect>, PromptError>) -> Answer {
    match result {
        Ok(effects) => (Ok(()), effects),
        Err(error) => (Err(error), Vec::new()),
    }
}

/// The model and the bus it answers on: what the loop holds.
pub struct Host {
    pub bus: Connection,
    pub model: crate::keyring::model::Prompter,
}

impl Host {
    pub fn new(bus: Connection) -> Host {
        Host {
            bus,
            model: crate::keyring::model::Prompter::new(),
        }
    }

    /// Decide `request` and answer its call with what follows, which the call's thread sends after the reply.
    pub fn request(&mut self, request: Request) {
        match request {
            Request::Begin { caller, answer } => {
                let _ = answer.send(split(self.model.begin(caller)));
            }
            Request::Perform {
                caller,
                kind,
                properties,
                exchange,
                answer,
            } => {
                let result = self.model.perform(&caller, kind, properties, &exchange);
                let _ = answer.send(split(result));
            }
            Request::Stop { caller, answer } => {
                let _ = answer.send((Ok(()), self.model.stop(&caller)));
            }
            Request::Vanished { name } => {
                let effects = self.model.vanished(&name);
                self.apply(effects);
            }
        }
    }

    /// Send each effect; a caller that is gone is no error of the prompter's.
    pub fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if let Err(error) = send(&self.bus, &effect) {
                eprintln!("cupertino-keyring-prompter: answering a caller: {error}");
            }
        }
    }
}
