//! The keyring prompter on a private bus, with a caller that does what the keyring daemon does: it begins prompting
//! with a callback object, asks, takes the answer through the secret exchange (decrypted here with the test's own end of
//! it) and stops. The prompter's model answers as the panel would after the keys the test types.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};

use cupertino_polkit_agent::keyring::exchange::Exchange;
use cupertino_polkit_agent::keyring::model::{Field, Key, Kind, Phase, MAX_WAITING};
use cupertino_polkit_agent::keyring::panel::{Command, Panel, PanelKey, Status};
use cupertino_polkit_agent::keyring::prompter::{
    serve, Host, Owned, ERROR_FAILED, ERROR_IN_PROGRESS, INTERFACE, PATH, PRIVATE_NAME, SYSTEM_NAME,
};
use cupertino_test_bus::{PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const CALLBACK: &str = "/org/test/keyring/prompt/1";

/// What a caller's callback heard.
#[derive(Debug)]
enum Heard {
    Ready {
        reply: String,
        properties: HashMap<String, OwnedValue>,
        exchange: String,
    },
    Done,
}

struct Callback(Sender<Heard>);

#[zbus::interface(name = "org.gnome.keyring.internal.Prompter.Callback")]
impl Callback {
    fn prompt_ready(
        &self,
        reply: String,
        properties: HashMap<String, OwnedValue>,
        exchange: String,
    ) {
        let _ = self.0.send(Heard::Ready {
            reply,
            properties,
            exchange,
        });
    }

    fn prompt_done(&self) {
        let _ = self.0.send(Heard::Done);
    }
}

/// The prompter on its own connection, its loop on a thread of its own.
struct Running {
    bus: PrivateBus,
    host: Arc<Mutex<Host>>,
}

impl Running {
    fn start() -> Option<Running> {
        let bus = PrivateBus::start()?;
        let conn = bus.connect();
        let (to_loop, requests) = channel();
        let owned = serve(&conn, move |request| {
            let _ = to_loop.send(request);
        })
        .unwrap();
        assert_eq!(owned, Owned::Now);
        let host = Arc::new(Mutex::new(Host::new(conn)));
        let looping = host.clone();
        std::thread::spawn(move || {
            for request in requests {
                looping.lock().unwrap().request(request);
            }
        });
        Some(Running { bus, host })
    }

    /// The prompter with its loop run by the thread that takes the calls: each call is answered, then `person` acts
    /// on the model, before that thread does anything else. A person can't be quicker.
    fn start_inline(person: impl Fn(&mut Host) + Send + Sync + Clone + 'static) -> Option<Running> {
        let bus = PrivateBus::start()?;
        let conn = bus.connect();
        let host = Arc::new(Mutex::new(Host::new(conn.clone())));
        let looping = host.clone();
        let owned = serve(&conn, move |request| {
            let mut host = looping.lock().unwrap();
            host.request(request);
            person(&mut host);
        })
        .unwrap();
        assert_eq!(owned, Owned::Now);
        Some(Running { bus, host })
    }

    fn caller(&self) -> Caller {
        let (heard, hearing) = channel();
        let conn = self
            .bus
            .blocking_builder()
            .serve_at(CALLBACK, Callback(heard))
            .unwrap()
            .build()
            .unwrap();
        Caller {
            conn,
            hearing,
            exchange: Exchange::new().unwrap(),
        }
    }

    /// Run `f` on the prompter's model and send what it leads to, as the panel does.
    fn person<R>(&self, f: impl FnOnce(&mut Host) -> R) -> R {
        f(&mut self.host.lock().unwrap())
    }

    fn type_text(&self, field: Field, text: &str) {
        self.person(|host| {
            for c in text.chars() {
                host.model.key(field, Key::Char(c));
            }
        });
    }

    fn answer(&self) {
        self.person(|host| {
            let effects = host.model.answer();
            host.apply(effects);
        });
    }

    fn cancel(&self) {
        self.person(|host| {
            let effects = host.model.cancel();
            host.apply(effects);
        });
    }
}

/// The keyring daemon's side: its callback object and its end of the exchange.
struct Caller {
    conn: Connection,
    hearing: Receiver<Heard>,
    exchange: Exchange,
}

impl Caller {
    fn call(
        &self,
        method: &str,
        body: &(impl zbus::export::serde::Serialize + zbus::zvariant::DynamicType),
    ) -> Result<(), String> {
        self.conn
            .call_method(Some(SYSTEM_NAME), PATH, Some(INTERFACE), method, body)
            .map(|_| ())
            .map_err(|error| match error {
                zbus::Error::MethodError(name, _, _) => name.to_string(),
                other => panic!("{method}: {other}"),
            })
    }

    fn heard(&self) -> Heard {
        self.hearing
            .recv_timeout(WAIT)
            .expect("the callback was called")
    }

    fn nothing_heard(&self) {
        assert!(matches!(
            self.hearing
                .recv_timeout(std::time::Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout)
        ));
    }

    /// Begin prompting and take the prompter's public key from its first `PromptReady`.
    fn begin(&mut self) {
        self.call(
            "BeginPrompting",
            &(ObjectPath::try_from(CALLBACK).unwrap(),),
        )
        .unwrap();
        match self.heard() {
            Heard::Ready {
                reply,
                properties,
                exchange,
            } => {
                assert_eq!(reply, "", "ready to be asked");
                assert!(properties.is_empty());
                assert_eq!(self.exchange.receive(&exchange).unwrap(), None);
            }
            Heard::Done => panic!("done before asked"),
        }
    }

    fn perform(&self, kind: &str, properties: &[(&str, Value<'_>)]) -> Result<(), String> {
        let properties: HashMap<&str, Value<'_>> = properties.iter().cloned().collect();
        self.call(
            "PerformPrompt",
            &(
                ObjectPath::try_from(CALLBACK).unwrap(),
                kind,
                properties,
                self.exchange.begin(),
            ),
        )
    }

    /// The reply to the question asked, its properties and the secret it carries.
    fn reply(&mut self) -> (String, HashMap<String, OwnedValue>, Option<String>) {
        match self.heard() {
            Heard::Ready {
                reply,
                properties,
                exchange,
            } => {
                let secret = self
                    .exchange
                    .receive(&exchange)
                    .unwrap()
                    .map(|s| String::from_utf8(s.to_vec()).unwrap());
                (reply, properties, secret)
            }
            Heard::Done => panic!("done instead of a reply"),
        }
    }

    fn stop(&self) {
        self.call("StopPrompting", &(ObjectPath::try_from(CALLBACK).unwrap(),))
            .unwrap();
    }
}

fn bool_of(properties: &HashMap<String, OwnedValue>, name: &str) -> bool {
    bool::try_from(properties[name].clone()).unwrap()
}

fn int_of(properties: &HashMap<String, OwnedValue>, name: &str) -> i32 {
    i32::try_from(properties[name].clone()).unwrap()
}

/// Until the prompter's model holds `done`, polled for at most [`WAIT`].
fn wait_for(running: &Running, what: &str, done: impl Fn(&Host) -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !done(&running.host.lock().unwrap()) {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_typed_password_returns_through_the_exchange_and_never_in_clear() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    assert!(
        running.person(|h| h.model.shown().is_none()),
        "no panel before a question"
    );
    caller
        .perform(
            "password",
            &[
                ("title", Value::from("Unlock Keyring")),
                ("message", Value::from("Authentication required")),
                (
                    "description",
                    Value::from("An application wants access to the keyring “login”."),
                ),
                ("continue-label", Value::from("_Unlock")),
                ("cancel-label", Value::from("_Cancel")),
                ("caller-window", Value::from("")),
                ("password-new", Value::from(false)),
            ],
        )
        .unwrap();
    running.person(|host| {
        let prompt = host.model.shown().expect("the panel shows");
        assert_eq!(prompt.kind, Kind::Password);
        assert_eq!(prompt.properties.heading(), "Authentication required");
        assert_eq!(prompt.properties.continue_title(), "Unlock");
        assert_eq!(prompt.properties.cancel_title(), "Cancel");
        assert!(!prompt.confirms() && !prompt.has_choice());
    });
    running.type_text(Field::Password, "correct horse 7f3a");
    running.answer();
    let (reply, properties, secret) = caller.reply();
    assert_eq!(reply, "yes");
    assert_eq!(secret.as_deref(), Some("correct horse 7f3a"));
    assert_eq!(int_of(&properties, "password-strength"), 1);
    assert!(!bool_of(&properties, "choice-chosen"));
    running.person(|host| {
        let prompt = host.model.shown().expect("busy until the caller stops");
        assert_eq!(prompt.phase, Phase::Checking);
        assert_eq!(prompt.field_len(Field::Password), 0, "the field is emptied");
    });
    caller.stop();
    wait_for(&running, "the prompt goes away", |h| {
        h.model.prompt().is_none()
    });
    caller.nothing_heard();
}

/// A person who answers the moment the question arrives still answers after the caller has the reply to its asking:
/// the caller's connection gets the reply to `PerformPrompt` first, then `PromptReady`. A caller that heard the answer
/// before its reply would take the reply for the end of the question and never stop prompting.
#[test]
fn an_answer_given_at_once_reaches_the_caller_after_the_reply_to_its_question() {
    let Some(running) = Running::start_inline(|host| {
        if host.model.shown().is_some_and(|p| p.phase == Phase::Asking) {
            let effects = host.model.cancel();
            host.apply(effects);
        }
    }) else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    let incoming = zbus::blocking::MessageIterator::from(&caller.conn);
    let (seen, seeing) = channel();
    std::thread::spawn(move || {
        for message in incoming.flatten() {
            let header = message.header();
            let what = match message.message_type() {
                zbus::message::Type::MethodReturn | zbus::message::Type::Error => {
                    header.reply_serial().map(|serial| serial.get().to_string())
                }
                zbus::message::Type::MethodCall => header.member().map(|m| m.to_string()),
                zbus::message::Type::Signal => None,
            };
            if let Some(what) = what {
                if seen.send(what).is_err() {
                    break;
                }
            }
        }
    });
    let properties: HashMap<&str, Value<'_>> = HashMap::new();
    let ask = zbus::Message::method_call(PATH, "PerformPrompt")
        .unwrap()
        .destination(SYSTEM_NAME)
        .unwrap()
        .interface(INTERFACE)
        .unwrap()
        .build(&(
            ObjectPath::try_from(CALLBACK).unwrap(),
            "password",
            properties,
            caller.exchange.begin(),
        ))
        .unwrap();
    let serial = ask.primary_header().serial_num().get().to_string();
    caller.conn.send(&ask).unwrap();
    let order = [
        seeing.recv_timeout(WAIT).unwrap(),
        seeing.recv_timeout(WAIT).unwrap(),
    ];
    assert_eq!(order, [serial, "PromptReady".to_string()]);
    assert_eq!(caller.reply().0, "no");
}

#[test]
fn a_wrong_password_is_asked_again_with_the_callers_warning_in_red() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    caller
        .perform("password", &[("message", Value::from("Unlock"))])
        .unwrap();
    running.type_text(Field::Password, "wrong");
    running.answer();
    assert_eq!(caller.reply().2.as_deref(), Some("wrong"));
    caller
        .perform(
            "password",
            &[("warning", Value::from("The unlock password was incorrect"))],
        )
        .unwrap();
    running.person(|host| {
        let prompt = host.model.shown().unwrap();
        assert_eq!(prompt.phase, Phase::Asking);
        assert_eq!(
            prompt.properties.message, "Unlock",
            "the message the caller set before stays"
        );
        assert_eq!(
            Status::of(prompt, false),
            vec![Status::Warning("The unlock password was incorrect".into())]
        );
        assert_eq!(
            Status::of(prompt, true),
            vec![
                Status::Warning("The unlock password was incorrect".into()),
                Status::CapsLock
            ]
        );
    });
    running.type_text(Field::Password, "right");
    running.answer();
    assert_eq!(caller.reply().2.as_deref(), Some("right"));
    caller.stop();
}

#[test]
fn a_new_password_needs_its_confirmation_and_the_choice_goes_back() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    caller
        .perform(
            "password",
            &[
                ("message", Value::from("Choose password for new keyring")),
                ("password-new", Value::from(true)),
                (
                    "choice-label",
                    Value::from("_Automatically unlock this keyring whenever I'm logged in"),
                ),
                ("choice-chosen", Value::from(false)),
            ],
        )
        .unwrap();
    running.person(|host| {
        let prompt = host.model.shown().unwrap();
        assert!(prompt.confirms() && prompt.has_choice());
        let mut panel = Panel::new(prompt);
        use cupertino_polkit_agent::keyring::panel::Control::*;
        assert_eq!(
            panel.order(),
            &[Password, Confirm, Choice, Cancel, Continue]
        );
        // Return in the first field goes on to the confirmation
        assert_eq!(panel.key(PanelKey::Return), Some(Command::Focused));
        assert_eq!(panel.focus, Confirm);
    });
    running.type_text(Field::Password, "new secret");
    running.type_text(Field::Confirm, "new secrex");
    running.answer();
    caller.nothing_heard();
    running.person(|host| {
        let prompt = host.model.shown().unwrap();
        assert!(prompt.mismatch);
        assert_eq!(Status::of(prompt, false), vec![Status::Mismatch]);
        assert_eq!(
            prompt.field_len(Field::Confirm),
            0,
            "the confirmation is emptied"
        );
        assert_eq!(prompt.field_len(Field::Password), 10);
        host.model.toggle_choice();
    });
    running.type_text(Field::Confirm, "new secret");
    running.answer();
    let (reply, properties, secret) = caller.reply();
    assert_eq!(reply, "yes");
    assert_eq!(secret.as_deref(), Some("new secret"));
    assert!(bool_of(&properties, "choice-chosen"));
    caller.stop();
}

#[test]
fn a_confirmation_answers_yes_without_a_secret_and_cancel_answers_no() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    caller
        .perform(
            "confirm",
            &[
                ("message", Value::from("Delete the keyring?")),
                ("continue-label", Value::from("Delete")),
            ],
        )
        .unwrap();
    running.person(|host| {
        let prompt = host.model.shown().unwrap();
        assert_eq!(prompt.kind, Kind::Confirm);
        assert_eq!(
            Panel::new(prompt).order().len(),
            2,
            "Cancel and the continue button alone"
        );
        // typing goes nowhere
        host.model.key(Field::Password, Key::Char('x'));
        assert_eq!(prompt_len(host), 0);
    });
    running.answer();
    let (reply, _, secret) = caller.reply();
    assert_eq!((reply.as_str(), secret), ("yes", None));

    caller.perform("password", &[]).unwrap();
    running.type_text(Field::Password, "never sent");
    running.cancel();
    let (reply, _, secret) = caller.reply();
    assert_eq!(
        (reply.as_str(), secret),
        ("no", None),
        "a cancel carries no secret"
    );
    caller.stop();
}

fn prompt_len(host: &Host) -> usize {
    host.model
        .prompt()
        .map_or(0, |p| p.field_len(Field::Password))
}

#[test]
fn one_caller_prompts_at_a_time_and_only_the_active_one_may_ask() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut first = running.caller();
    let second = running.caller();
    first.begin();
    second
        .call(
            "BeginPrompting",
            &(ObjectPath::try_from(CALLBACK).unwrap(),),
        )
        .unwrap();
    second.nothing_heard();
    assert_eq!(running.person(|h| h.model.waiting()), 1);
    // the waiting caller may not ask, nor may the active one ask twice
    assert_eq!(
        second.perform("password", &[]),
        Err(ERROR_IN_PROGRESS.into())
    );
    first.perform("password", &[]).unwrap();
    assert_eq!(
        first.perform("password", &[]),
        Err(ERROR_IN_PROGRESS.into())
    );
    assert_eq!(
        first.perform("pin", &[]).unwrap_err(),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    // the same caller beginning again is refused
    assert_eq!(
        first.call(
            "BeginPrompting",
            &(ObjectPath::try_from(CALLBACK).unwrap(),)
        ),
        Err(ERROR_IN_PROGRESS.into())
    );
    first.stop();
    match second.heard() {
        Heard::Ready { reply, .. } => assert_eq!(reply, ""),
        Heard::Done => panic!(),
    }
    drop(second);
    wait_for(
        &running,
        "a caller that leaves the bus ends its prompt",
        |h| h.model.prompt().is_none(),
    );
}

#[test]
fn a_broken_exchange_is_refused_and_too_many_callers_are_turned_away() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    let refused = caller.call(
        "PerformPrompt",
        &(
            ObjectPath::try_from(CALLBACK).unwrap(),
            "password",
            HashMap::<&str, Value<'_>>::new(),
            "[sx-aes-1]\npublic=AQ==\n",
        ),
    );
    assert_eq!(refused, Err(ERROR_FAILED.into()));
    let others: Vec<Caller> = (0..MAX_WAITING).map(|_| running.caller()).collect();
    for other in &others {
        other
            .call(
                "BeginPrompting",
                &(ObjectPath::try_from(CALLBACK).unwrap(),),
            )
            .unwrap();
    }
    let one_more = running.caller();
    assert_eq!(
        one_more.call(
            "BeginPrompting",
            &(ObjectPath::try_from(CALLBACK).unwrap(),)
        ),
        Err(ERROR_FAILED.into())
    );
}

#[test]
fn a_locked_session_cancels_the_question_and_ends_the_prompt() {
    let Some(running) = Running::start() else {
        return;
    };
    let mut caller = running.caller();
    caller.begin();
    caller.perform("password", &[]).unwrap();
    running.type_text(Field::Password, "half typed");
    running.person(|host| {
        let effects = host.model.set_away(true);
        host.apply(effects);
    });
    let (reply, _, secret) = caller.reply();
    assert_eq!((reply.as_str(), secret), ("no", None));
    assert!(matches!(caller.heard(), Heard::Done));
    assert!(running.person(|h| h.model.prompt().is_none()));
    // a caller beginning while the session is away waits for it
    let mut later = running.caller();
    later
        .call(
            "BeginPrompting",
            &(ObjectPath::try_from(CALLBACK).unwrap(),),
        )
        .unwrap();
    later.nothing_heard();
    running.person(|host| {
        let effects = host.model.set_away(false);
        host.apply(effects);
    });
    match later.heard() {
        Heard::Ready {
            reply, exchange, ..
        } => {
            assert_eq!(reply, "");
            later.exchange.receive(&exchange).unwrap();
        }
        Heard::Done => panic!(),
    }
}

#[test]
fn the_prompter_owns_both_names_on_one_object() {
    let Some(running) = Running::start() else {
        return;
    };
    let conn = running.bus.connect();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).unwrap();
    let system = dbus
        .get_name_owner(SYSTEM_NAME.try_into().unwrap())
        .unwrap();
    let private = dbus
        .get_name_owner(PRIVATE_NAME.try_into().unwrap())
        .unwrap();
    assert_eq!(system, private);
    // a second prompter queues behind the first and is not given the names
    let other = running.bus.connect();
    assert_eq!(serve(&other, |_| {}).unwrap(), Owned::Queued);
    assert_eq!(
        dbus.get_name_owner(SYSTEM_NAME.try_into().unwrap())
            .unwrap(),
        system
    );
}
