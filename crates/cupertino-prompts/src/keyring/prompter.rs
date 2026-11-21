//! The prompter on the session bus: `org.gnome.keyring.internal.Prompter` at `/org/gnome/keyring/Prompter`, under the
//! system and the private prompter's names. A caller (the keyring daemon) begins prompting with the path of its
//! callback object, asks one question at a time, and stops; the prompter answers each through the callback's
//! `PromptReady` and tells it `PromptDone` when it ends a prompt itself.
//!
//! The prompter has a session bus connection of its own, which serves no object: [`serve`] reads every call to it and
//! hands those to the prompter to the program as [`Request`]s; the program's model decides and [`send`] carries the
//! reply and the effects back in the order the model gave them, so a caller always has its reply before its callback
//! is called. Both are futures on Kit's loop.

use std::collections::HashMap;

use futures_lite::StreamExt;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, OwnedValue, Value as Variant};
use zbus::{Connection, MatchRule, MessageStream};

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
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const BUS_DAEMON: &str = "org.freedesktop.DBus";

/// A call to the prompter, which the program answers ([`Outgoing::Reply`]): one on the bus, or one of the program's own
/// ([`crate::keyring::asking`]), answered on a channel.
#[derive(Clone, Debug)]
pub enum Call {
    Bus(zbus::Message),
    Inside(async_channel::Sender<Result<(), PromptError>>),
}

/// A call that reached the prompter, for the program.
#[derive(Clone, Debug)]
pub enum Request {
    Begin {
        caller: Caller,
        call: Call,
    },
    Perform {
        caller: Caller,
        kind: Kind,
        properties: Vec<(String, Value)>,
        exchange: Exchange,
        call: Call,
    },
    Stop {
        caller: Caller,
        call: Call,
    },
    /// A bus name left the bus.
    Vanished {
        name: String,
    },
}

/// A caller's exchange message, which `Debug` never prints.
#[derive(Clone)]
pub struct Exchange(pub String);

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Exchange(..)")
    }
}

/// What the prompter sends, in the order the program gives it.
#[derive(Clone, Debug)]
pub enum Outgoing {
    /// The reply to `Call`.
    Reply(Call, Result<(), PromptError>),
    /// A callback of a caller.
    Effect(Effect),
}

/// How the prompter came to its names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owned {
    /// It owns them.
    Now,
    /// Another prompter holds one; the bus gives it over when that one leaves.
    Queued,
}

/// Take both names on `bus`, replacing a prompter that allows it and never giving them up to another.
pub async fn own(bus: &Connection) -> zbus::Result<Owned> {
    let proxy = DBusProxy::new(bus).await?;
    let mut owned = Owned::Now;
    for name in [SYSTEM_NAME, PRIVATE_NAME] {
        let reply = proxy
            .request_name(name.try_into()?, RequestNameFlags::ReplaceExisting.into())
            .await?;
        if reply == RequestNameReply::InQueue {
            owned = Owned::Queued;
        }
    }
    Ok(owned)
}

/// Take the prompter's names on `bus` ([`own`]), telling `owned` how, and hand every call to the prompter to `requests`
/// until the connection closes; returns why it stopped. The calls are read from before the names are asked for, so
/// none is missed. A call the prompter does not serve is refused here.
pub async fn serve(
    bus: &Connection,
    owned: impl FnOnce(Result<Owned, String>),
    requests: impl Fn(Request),
) -> String {
    let mut messages = MessageStream::from(bus);
    let taken = own(bus).await;
    let failed = taken.is_err();
    owned(taken.map_err(|e| e.to_string()));
    if failed {
        return "the keyring prompter's names were not taken".to_owned();
    }
    while let Some(message) = messages.next().await {
        let Ok(message) = message else { continue };
        if message.message_type() != Type::MethodCall {
            continue;
        }
        if let Err(error) = call(bus, &message, &requests).await {
            eprintln!("cupertino-prompts: answering a keyring call: {error}");
        }
    }
    "the keyring prompter's connection closed".to_owned()
}

/// Answer one method call to the prompter's connection, or hand it to the program.
async fn call(
    bus: &Connection,
    message: &zbus::Message,
    requests: &impl Fn(Request),
) -> zbus::Result<()> {
    let header = message.header();
    let member = header.member().map(|m| m.as_str()).unwrap_or_default();
    let interface = header.interface().map(|i| i.as_str());
    let path = header.path().map(|p| p.as_str());
    if interface == Some("org.freedesktop.DBus.Peer") && member == "Ping" {
        return bus.reply(&header, &()).await;
    }
    if path != Some(PATH) || !matches!(interface, None | Some(INTERFACE)) {
        return bus
            .reply_error(&header, UNKNOWN_METHOD, &"no such method")
            .await;
    }
    let Some(sender) = header.sender().map(|s| s.to_string()) else {
        return Ok(());
    };
    let caller = |path: ObjectPath<'_>| Caller {
        name: sender.clone(),
        path: path.to_string(),
    };
    let call = || Call::Bus(message.clone());
    let body = message.body();
    let request = match member {
        "BeginPrompting" => body
            .deserialize::<ObjectPath<'_>>()
            .map(|path| Request::Begin {
                caller: caller(path),
                call: call(),
            }),
        "StopPrompting" => body
            .deserialize::<ObjectPath<'_>>()
            .map(|path| Request::Stop {
                caller: caller(path),
                call: call(),
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
                    exchange: Exchange(exchange),
                    call: call(),
                })
            }),
        _ => {
            return bus
                .reply_error(&header, UNKNOWN_METHOD, &"no such method")
                .await
        }
    };
    match request {
        Ok(request) => {
            requests(request);
            Ok(())
        }
        Err(error) => {
            bus.reply_error(&header, INVALID_ARGS, &error.to_string())
                .await
        }
    }
}

/// Hand each bus name that leaves to `vanished`, believed only from the bus daemon itself, until the connection closes;
/// returns why it stopped.
pub async fn follow_vanished(bus: &Connection, vanished: impl Fn(String)) -> String {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BUS_DAEMON)
        .and_then(|r| r.interface(BUS_DAEMON))
        .and_then(|r| r.member("NameOwnerChanged"))
        .map(|r| r.build());
    let messages = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, bus, Some(64)).await,
        Err(e) => Err(e),
    };
    let mut messages = match messages {
        Ok(messages) => messages,
        Err(e) => return format!("following the keyring's callers: {e}"),
    };
    while let Some(message) = messages.next().await {
        let Ok(message) = message else { continue };
        if !cupertino_peer::signal::sent_by_bus(&message) {
            continue;
        }
        let Ok((name, _old, new)) = message.body().deserialize::<(String, String, String)>() else {
            continue;
        };
        if new.is_empty() && name.starts_with(':') {
            vanished(name);
        }
    }
    "the keyring prompter's connection closed".to_owned()
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

/// Send what the program gives `outbox`, one after the other, until the program drops it; a caller that is gone is no
/// error of the prompter's.
pub async fn send(bus: &Connection, outbox: async_channel::Receiver<Outgoing>) {
    while let Ok(outgoing) = outbox.recv().await {
        let sent = match &outgoing {
            Outgoing::Reply(Call::Inside(answer), result) => {
                let _ = answer.try_send(result.clone());
                Ok(())
            }
            Outgoing::Reply(Call::Bus(message), result) => {
                let header = message.header();
                match result {
                    Ok(()) => bus.reply(&header, &()).await,
                    Err(error) => {
                        let (name, text) = error_reply(error);
                        bus.reply_error(&header, name, &text).await
                    }
                }
            }
            Outgoing::Effect(effect) => send_effect(bus, effect).await,
        };
        if let Err(error) = sent {
            eprintln!("cupertino-prompts: answering a keyring caller: {error}");
        }
    }
}

/// Carry `effect` to its caller without waiting for the reply, so the program never waits on a caller.
async fn send_effect(bus: &Connection, effect: &Effect) -> zbus::Result<()> {
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
                    let value = match v {
                        Value::Str(s) => Variant::from(s.as_str()),
                        Value::Bool(b) => Variant::from(*b),
                        Value::Int(i) => Variant::from(*i),
                    };
                    // a caller reads a property's value out of a variant of its own, inside the dictionary's
                    (*name, Variant::Value(Box::new(value)))
                })
                .collect();
            builder.build(&(reply.as_str(), properties, exchange.as_str()))?
        }
        Effect::Done { .. } => builder.build(&())?,
    };
    bus.send(&message).await
}
