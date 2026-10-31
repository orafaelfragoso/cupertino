//! The agent on a private bus with a fake polkitd: it registers for its session and again when polkitd restarts,
//! passes polkitd's requests on with their text made safe, answers polkitd as the prompt decides, refuses every
//! caller but polkitd and refuses requests beyond its limit.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use cupertino_polkit_agent::agent::{serve, Event, Outcome, User, AGENT_PATH, MAX_PENDING, POLKIT};
use cupertino_test_bus::PrivateBus;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::{OwnedValue, Value};

const AGENT_INTERFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
const WAIT: Duration = Duration::from_secs(5);

/// What the fake polkitd was asked: the subject's kind and session id, the locale, the object path and the agent's bus
/// name.
type Registration = (String, String, String, String, String);

struct Authority(Sender<Registration>);

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Authority {
    fn register_authentication_agent_with_options(
        &self,
        subject: (String, HashMap<String, OwnedValue>),
        locale: String,
        object_path: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
    ) {
        let session = subject
            .1
            .get("session-id")
            .and_then(|id| String::try_from(id.clone()).ok())
            .unwrap_or_default();
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let _ = self
            .0
            .send((subject.0, session, locale, object_path, sender));
    }
}

fn polkitd(bus: &PrivateBus, registrations: Sender<Registration>) -> Connection {
    bus.blocking_builder()
        .name(POLKIT)
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            Authority(registrations),
        )
        .unwrap()
        .build()
        .unwrap()
}

struct Running {
    bus: PrivateBus,
    polkitd: Connection,
    registrations: Receiver<Registration>,
    /// The agent's unique name.
    agent: String,
    events: Receiver<Event>,
    _agent: Connection,
}

fn start() -> Option<Running> {
    let bus = PrivateBus::start()?;
    let (registered, registrations) = channel();
    let polkitd = polkitd(&bus, registered);
    let agent = bus.connect();
    let (sink, events) = channel();
    let sink = std::sync::Mutex::new(sink);
    serve(&agent, "c2", "en_US.UTF-8", move |event| {
        let _ = sink.lock().unwrap().send(event);
    })
    .unwrap();
    let first = registrations.recv_timeout(WAIT).unwrap();
    assert_eq!(
        first,
        (
            "unix-session".into(),
            "c2".into(),
            "en_US.UTF-8".into(),
            AGENT_PATH.into(),
            agent.unique_name().unwrap().to_string()
        )
    );
    Some(Running {
        agent: first.4,
        bus,
        polkitd,
        registrations,
        events,
        _agent: agent,
    })
}

/// `BeginAuthentication` from `caller` on its own thread, as polkitd waits for the answer.
fn begin(
    caller: &Connection,
    agent: &str,
    cookie: &str,
    identities: &[(&'static str, u32)],
) -> std::thread::JoinHandle<Result<(), String>> {
    let (caller, agent, cookie) = (caller.clone(), agent.to_owned(), cookie.to_owned());
    let identities: Vec<(&str, HashMap<&str, Value<'static>>)> = identities
        .iter()
        .map(|(kind, id)| {
            let key = if *kind == "unix-user" { "uid" } else { "gid" };
            (*kind, HashMap::from([(key, Value::from(*id))]))
        })
        .collect();
    std::thread::spawn(move || {
        let details = HashMap::from([
            ("polkit.caller-pid", "4242"),
            ("program", "/usr/bin/\u{202E}fdisk\u{2069}\nrm"),
        ]);
        caller
            .call_method(
                Some(agent.as_str()),
                AGENT_PATH,
                Some(AGENT_INTERFACE),
                "BeginAuthentication",
                &(
                    "org.example.mount",
                    "Authentication is required to mount\tthe disk",
                    "drive-harddisk",
                    details,
                    cookie.as_str(),
                    identities,
                ),
            )
            .map(|_| ())
            .map_err(|error| match error {
                zbus::Error::MethodError(name, _, _) => name.to_string(),
                other => other.to_string(),
            })
    })
}

fn next(events: &Receiver<Event>) -> Event {
    events.recv_timeout(WAIT).expect("an event from the agent")
}

#[test]
fn a_request_from_polkitd_reaches_the_prompt_with_safe_text_and_its_answer_goes_back() {
    let Some(running) = start() else { return };
    let call = begin(
        &running.polkitd,
        &running.agent,
        "cookie-1",
        &[("unix-group", 10), ("unix-user", 1000)],
    );
    let Event::Begin(request, reply) = next(&running.events) else {
        panic!("a begin")
    };
    assert_eq!(request.cookie, "cookie-1");
    assert_eq!(request.action, "org.example.mount");
    assert_eq!(
        request.message,
        "Authentication is required to mount the disk"
    );
    assert_eq!(
        request.details,
        [
            ("polkit.caller-pid".to_owned(), "4242".to_owned()),
            ("program".to_owned(), "/usr/bin/fdisk rm".to_owned()),
        ]
    );
    assert_eq!(request.users, [User(1000)]);
    reply.send(Outcome::Done);
    assert_eq!(call.join().unwrap(), Ok(()));
}

#[test]
fn a_dismissed_or_abandoned_prompt_is_cancelled_for_polkitd() {
    let Some(running) = start() else { return };
    const CANCELLED: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
    let call = begin(
        &running.polkitd,
        &running.agent,
        "a",
        &[("unix-user", 1000)],
    );
    let Event::Begin(_, reply) = next(&running.events) else {
        panic!()
    };
    reply.send(Outcome::Cancelled);
    assert_eq!(call.join().unwrap(), Err(CANCELLED.to_owned()));

    let call = begin(
        &running.polkitd,
        &running.agent,
        "b",
        &[("unix-user", 1000)],
    );
    let Event::Begin(_, reply) = next(&running.events) else {
        panic!()
    };
    drop(reply);
    assert_eq!(call.join().unwrap(), Err(CANCELLED.to_owned()));
}

#[test]
fn only_polkitd_may_begin_or_cancel() {
    let Some(running) = start() else { return };
    const DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
    let stranger = running.bus.connect();
    let call = begin(&stranger, &running.agent, "forged", &[("unix-user", 1000)]);
    assert_eq!(call.join().unwrap(), Err(DENIED.to_owned()));
    let cancel = |caller: &Connection| {
        caller
            .call_method(
                Some(running.agent.as_str()),
                AGENT_PATH,
                Some(AGENT_INTERFACE),
                "CancelAuthentication",
                &("cookie-1",),
            )
            .map(|_| ())
    };
    assert!(
        matches!(cancel(&stranger), Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == DENIED)
    );
    assert!(
        running.events.try_recv().is_err(),
        "a stranger reached the prompt"
    );
    cancel(&running.polkitd).unwrap();
    assert!(matches!(next(&running.events), Event::Cancel(cookie) if cookie == "cookie-1"));
}

#[test]
fn requests_beyond_the_limit_are_refused_at_once() {
    let Some(running) = start() else { return };
    let calls: Vec<_> = (0..MAX_PENDING)
        .map(|i| {
            begin(
                &running.polkitd,
                &running.agent,
                &format!("c{i}"),
                &[("unix-user", 1000)],
            )
        })
        .collect();
    let replies: Vec<_> = (0..MAX_PENDING)
        .map(|_| match next(&running.events) {
            Event::Begin(_, reply) => reply,
            Event::Cancel(_) => panic!("a begin"),
        })
        .collect();
    let refused = begin(
        &running.polkitd,
        &running.agent,
        "over",
        &[("unix-user", 1000)],
    );
    assert_eq!(
        refused.join().unwrap(),
        Err("org.freedesktop.PolicyKit1.Error.Failed".to_owned())
    );
    for reply in replies {
        reply.send(Outcome::Done);
    }
    for call in calls {
        assert_eq!(call.join().unwrap(), Ok(()));
    }
    let again = begin(
        &running.polkitd,
        &running.agent,
        "again",
        &[("unix-user", 1000)],
    );
    let Event::Begin(_, reply) = next(&running.events) else {
        panic!()
    };
    reply.send(Outcome::Done);
    assert_eq!(again.join().unwrap(), Ok(()));
}

#[test]
fn a_request_with_no_user_is_refused() {
    let Some(running) = start() else { return };
    let call = begin(
        &running.polkitd,
        &running.agent,
        "group",
        &[("unix-group", 10)],
    );
    assert_eq!(
        call.join().unwrap(),
        Err("org.freedesktop.PolicyKit1.Error.Failed".to_owned())
    );
    assert!(running.events.try_recv().is_err());
}

#[test]
fn the_agent_registers_again_when_polkitd_restarts() {
    let Some(running) = start() else { return };
    drop(running.polkitd);
    let (registered, registrations) = channel();
    let _restarted = polkitd(&running.bus, registered);
    let again = registrations
        .recv_timeout(WAIT)
        .expect("a second registration");
    assert_eq!((again.1.as_str(), again.3.as_str()), ("c2", AGENT_PATH));
    assert!(running.registrations.try_recv().is_err());
}
