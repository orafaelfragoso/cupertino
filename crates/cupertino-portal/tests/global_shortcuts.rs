//! The GlobalShortcuts backend on a private bus, called as xdg-desktop-portal calls it, with a dialog the test answers
//! and the server's side played by the test: the keys the backend asks the server for, and the presses, releases and
//! reserved chords the server sends back.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};

use cupertino_defaults::desktop_control::Decision;
use cupertino_defaults::global_shortcuts::GlobalShortcutsConf;
use cupertino_portal::global_shortcuts::{
    modifier, Answer, Event, GlobalShortcuts, Handle, Keys, Refusal, Trigger, INTERFACE,
};
use cupertino_portal::request::FRONTEND;
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const APP: &str = "org.example.Meeting";
const OTHER: &str = "org.example.Chat";
const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_7/s1";
const OTHER_SESSION: &str = "/org/freedesktop/portal/desktop/session/1_7/s2";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;
type Shortcuts = Vec<(String, HashMap<String, OwnedValue>)>;

struct Running {
    bus: PrivateBus,
    served: Connection,
    home: tempfile::TempDir,
    events: mpsc::Receiver<Event>,
    keys: mpsc::Receiver<Keys>,
    handle: Handle,
    frontend: Connection,
}

impl Running {
    fn answers(&self) -> PathBuf {
        self.home.path().join("global-shortcuts.conf")
    }

    /// What the backend asked the server for since the last call.
    fn keys(&self) -> Vec<Keys> {
        self.keys.try_iter().collect()
    }
}

fn start() -> Option<Running> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let home = tempfile::tempdir().unwrap();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let applications = home.path().join("applications");
    std::fs::create_dir(&applications).unwrap();
    for (id, name) in [(APP, "Meeting"), (OTHER, "Chat")] {
        std::fs::write(
            applications.join(format!("{id}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName={name}\nExec=x\n"),
        )
        .unwrap();
    }
    let (events_sender, events) = mpsc::channel();
    let (keys_sender, keys) = mpsc::channel();
    let (events_sender, keys_sender) = (Mutex::new(events_sender), Mutex::new(keys_sender));
    let shortcuts = GlobalShortcuts::new(
        move |event| {
            let _ = events_sender.lock().unwrap().send(event);
        },
        move |k| {
            let _ = keys_sender.lock().unwrap().send(k);
        },
    )
    .with_applications(vec![applications])
    .with_answers(Some(home.path().join("global-shortcuts.conf")));
    let handle = shortcuts.handle();
    shortcuts.serve(&served).unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    Some(Running {
        bus,
        served,
        home,
        events,
        keys,
        handle,
        frontend,
    })
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

fn create(caller: &Connection, session: &str, app_id: &str) -> Call {
    caller
        .call_method(
            Some(BUS_NAME),
            PORTAL_PATH,
            Some(INTERFACE),
            "CreateSession",
            &(
                ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_7/c").unwrap(),
                ObjectPath::try_from(session).unwrap(),
                app_id,
                Results::new(),
            ),
        )
        .and_then(|m| m.body().deserialize::<(u32, Results)>())
}

fn shortcut(id: &str, description: &str, trigger: Option<&str>) -> (String, Results) {
    let mut properties = Results::new();
    properties.insert("description".into(), owned(Value::from(description)));
    if let Some(trigger) = trigger {
        properties.insert("preferred_trigger".into(), owned(Value::from(trigger)));
    }
    (id.to_owned(), properties)
}

/// `BindShortcuts` on another thread, so the test can answer the dialog meanwhile.
fn bind(
    caller: &Connection,
    n: u32,
    session: &str,
    shortcuts: Vec<(String, Results)>,
) -> mpsc::Receiver<Call> {
    let caller = caller.clone();
    let session = session.to_owned();
    let (sender, done) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = format!("/org/freedesktop/portal/desktop/request/1_7/b{n}");
        let reply = caller
            .call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                "BindShortcuts",
                &(
                    ObjectPath::try_from(handle.as_str()).unwrap(),
                    ObjectPath::try_from(session.as_str()).unwrap(),
                    shortcuts,
                    "wayland:ab12",
                    Results::new(),
                ),
            )
            .and_then(|m| m.body().deserialize::<(u32, Results)>());
        let _ = sender.send(reply);
    });
    done
}

fn list(caller: &Connection, session: &str) -> Call {
    caller
        .call_method(
            Some(BUS_NAME),
            PORTAL_PATH,
            Some(INTERFACE),
            "ListShortcuts",
            &(
                ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_7/l").unwrap(),
                ObjectPath::try_from(session).unwrap(),
            ),
        )
        .and_then(|m| m.body().deserialize::<(u32, Results)>())
}

/// The ids and trigger descriptions of the `shortcuts` result.
fn named(results: &Results) -> Vec<(String, String)> {
    let shortcuts: Shortcuts = results["shortcuts"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    shortcuts
        .into_iter()
        .map(|(id, p)| {
            let trigger: String = p["trigger_description"]
                .try_clone()
                .unwrap()
                .try_into()
                .unwrap();
            (id, trigger)
        })
        .collect()
}

/// The signals of the interface as the frontend receives them, by member name and body.
fn signals(r: &Running) -> mpsc::Receiver<zbus::Message> {
    let connection = r.frontend.clone();
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)
        .unwrap()
        .build();
    let iterator =
        zbus::blocking::MessageIterator::for_match_rule(rule, &connection, Some(64)).unwrap();
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        for message in iterator.flatten() {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    received
}

/// The next signal's member and its session and shortcut id (for Activated and Deactivated) or its session (else).
fn next_signal(signals: &mpsc::Receiver<zbus::Message>) -> (String, OwnedObjectPath, String) {
    let message = signals.recv_timeout(WAIT).unwrap();
    let member = message.header().member().unwrap().to_string();
    match member.as_str() {
        "Activated" | "Deactivated" => {
            let (session, id, _, _): (OwnedObjectPath, String, u64, Results) =
                message.body().deserialize().unwrap();
            (member, session, id)
        }
        _ => {
            let (session, shortcuts): (OwnedObjectPath, Shortcuts) =
                message.body().deserialize().unwrap();
            let ids: Vec<String> = shortcuts.into_iter().map(|(id, _)| id).collect();
            (member, session, ids.join(","))
        }
    }
}

fn ask(
    r: &Running,
) -> (
    OwnedObjectPath,
    Box<cupertino_portal::global_shortcuts::Ask>,
    cupertino_portal::global_shortcuts::Reply,
) {
    match r.events.recv_timeout(WAIT).unwrap() {
        Event::Ask { handle, ask, reply } => (handle, ask, reply),
        Event::Close { .. } => panic!("a close before an ask"),
    }
}

const CTRL_ALT_T: Trigger = Trigger {
    modifiers: modifier::CTRL | modifier::ALT,
    keysym: 0x74,
};

#[test]
fn a_client_is_asked_binds_lists_and_hears_its_key_go_down_and_up() {
    let Some(r) = start() else { return };
    assert_eq!(create(&r.frontend, SESSION, APP).unwrap().0, 0);
    let done = bind(
        &r.frontend,
        1,
        SESSION,
        vec![
            shortcut("talk", "Push to talk", Some("CTRL+ALT+t")),
            shortcut("letter", "Type a", Some("a")),
        ],
    );
    let (_, asked, reply) = ask(&r);
    assert_eq!(asked.app_name.as_deref(), Some("Meeting"));
    assert_eq!(asked.parent_window, "wayland:ab12");
    assert_eq!(asked.shortcuts.len(), 2);
    assert_eq!(asked.shortcuts[0].trigger, Some(CTRL_ALT_T));
    assert_eq!(asked.shortcuts[0].refusal, None);
    assert_eq!(asked.shortcuts[1].refusal, Some(Refusal::Typing));
    // nothing goes to the server before the person answers
    assert!(r.keys().is_empty());
    reply.send(Answer::Allowed);
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(named(&results), [("talk".to_owned(), "⌃⌥T".to_owned())]);
    let keys = r.keys();
    let [Keys::Bind { key, trigger }] = keys[..] else {
        panic!("{keys:?}")
    };
    assert_eq!(trigger, CTRL_ALT_T);
    // remembered for the app
    let (conf, _) = GlobalShortcutsConf::load(&r.answers()).unwrap();
    assert_eq!(conf.decision(APP), Some(Decision::Allow));

    let (response, results) = list(&r.frontend, SESSION).unwrap();
    assert_eq!(response, 0);
    assert_eq!(named(&results), [("talk".to_owned(), "⌃⌥T".to_owned())]);

    // the server's press and release of the key, while another app has the keyboard
    let signals = signals(&r);
    r.handle.fired(&r.served, key, true, 1000);
    r.handle.fired(&r.served, key, false, 1040);
    let session = OwnedObjectPath::try_from(SESSION).unwrap();
    assert_eq!(
        next_signal(&signals),
        ("Activated".into(), session.clone(), "talk".into())
    );
    assert_eq!(
        next_signal(&signals),
        ("Deactivated".into(), session.clone(), "talk".into())
    );

    // the system takes the key later: it is given back and the session told
    r.handle.reserved(&r.served, vec![CTRL_ALT_T]);
    assert_eq!(r.keys(), [Keys::Unbind { key }]);
    assert_eq!(
        next_signal(&signals),
        ("ShortcutsChanged".into(), session, String::new())
    );
    let (_, results) = list(&r.frontend, SESSION).unwrap();
    assert!(named(&results).is_empty());
}

#[test]
fn a_remembered_answer_is_followed_without_asking() {
    let Some(r) = start() else { return };
    GlobalShortcutsConf::update(&r.answers(), |c| {
        c.set(APP, Decision::Allow);
        c.set(OTHER, Decision::Deny);
    })
    .unwrap();
    create(&r.frontend, SESSION, APP).unwrap();
    create(&r.frontend, OTHER_SESSION, OTHER).unwrap();
    let talk = vec![shortcut("talk", "Push to talk", Some("<Control><Alt>t"))];
    let (response, results) = bind(&r.frontend, 1, SESSION, talk.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .unwrap();
    assert_eq!(response, 0);
    assert_eq!(named(&results).len(), 1);
    let (response, _) = bind(
        &r.frontend,
        2,
        OTHER_SESSION,
        vec![shortcut("open", "Open", Some("CTRL+ALT+o"))],
    )
    .recv_timeout(WAIT)
    .unwrap()
    .unwrap();
    assert_eq!(response, 1, "a denied app binds nothing");
    assert!(r.events.try_recv().is_err(), "nobody was asked");
    assert_eq!(r.keys().len(), 1);
}

#[test]
fn a_key_another_app_holds_is_refused_and_left_out() {
    let Some(r) = start() else { return };
    GlobalShortcutsConf::update(&r.answers(), |c| c.set(APP, Decision::Allow)).unwrap();
    create(&r.frontend, SESSION, APP).unwrap();
    bind(
        &r.frontend,
        1,
        SESSION,
        vec![shortcut("talk", "Push to talk", Some("CTRL+ALT+t"))],
    )
    .recv_timeout(WAIT)
    .unwrap()
    .unwrap();
    // the system's own chords come from the server
    r.handle.reserved(
        &r.served,
        vec![Trigger {
            modifiers: modifier::LOGO,
            keysym: 0xff09,
        }],
    );
    r.keys();

    create(&r.frontend, OTHER_SESSION, OTHER).unwrap();
    let done = bind(
        &r.frontend,
        2,
        OTHER_SESSION,
        vec![
            shortcut("talk", "Talk", Some("CTRL+ALT+t")),
            shortcut("switch", "Switch", Some("LOGO+Tab")),
            shortcut("open", "Open", Some("CTRL+SHIFT+o")),
        ],
    );
    let (_, asked, reply) = ask(&r);
    let refusals: Vec<_> = asked.shortcuts.iter().map(|o| o.refusal.clone()).collect();
    assert_eq!(
        refusals,
        [
            Some(Refusal::Taken(APP.into())),
            Some(Refusal::System),
            None
        ]
    );
    assert_eq!(asked.holders[APP], "Meeting");
    reply.send(Answer::Allowed);
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(named(&results), [("open".to_owned(), "⌃⇧O".to_owned())]);

    // when nothing can be bound nobody is asked
    let (response, results) = bind(
        &r.frontend,
        3,
        OTHER_SESSION,
        vec![shortcut("talk", "Talk", Some("CTRL+ALT+t"))],
    )
    .recv_timeout(WAIT)
    .unwrap()
    .unwrap();
    assert_eq!(response, 0);
    assert!(named(&results).is_empty());
    assert!(r.events.try_recv().is_err());
}

#[test]
fn dont_allow_is_remembered_and_escape_is_not() {
    let Some(r) = start() else { return };
    create(&r.frontend, SESSION, APP).unwrap();
    let talk = vec![shortcut("talk", "Push to talk", Some("CTRL+ALT+t"))];
    let done = bind(&r.frontend, 1, SESSION, talk.clone());
    ask(&r).2.send(Answer::Dismissed);
    assert_eq!(done.recv_timeout(WAIT).unwrap().unwrap().0, 1);
    assert_eq!(
        GlobalShortcutsConf::load(&r.answers())
            .unwrap()
            .0
            .decision(APP),
        None
    );
    let done = bind(&r.frontend, 2, SESSION, talk.clone());
    ask(&r).2.send(Answer::Denied);
    assert_eq!(done.recv_timeout(WAIT).unwrap().unwrap().0, 1);
    assert_eq!(
        GlobalShortcutsConf::load(&r.answers())
            .unwrap()
            .0
            .decision(APP),
        Some(Decision::Deny)
    );
    assert!(r.keys().is_empty());
}

#[test]
fn a_closed_session_gives_its_keys_back_and_ends_its_question() {
    let Some(r) = start() else { return };
    GlobalShortcutsConf::update(&r.answers(), |c| c.set(APP, Decision::Allow)).unwrap();
    create(&r.frontend, SESSION, APP).unwrap();
    bind(
        &r.frontend,
        1,
        SESSION,
        vec![shortcut("talk", "Push to talk", Some("CTRL+ALT+t"))],
    )
    .recv_timeout(WAIT)
    .unwrap()
    .unwrap();
    let [Keys::Bind { key, .. }] = r.keys()[..] else {
        panic!()
    };
    r.frontend
        .call_method(
            Some(BUS_NAME),
            SESSION,
            Some("org.freedesktop.impl.portal.Session"),
            "Close",
            &(),
        )
        .unwrap();
    assert_eq!(r.keys(), [Keys::Unbind { key }]);
    assert!(list(&r.frontend, SESSION).is_err(), "no session any more");
    let signals = signals(&r);
    r.handle.fired(&r.served, key, true, 5);
    // a signal sent now would arrive before this one
    r.served
        .emit_signal(None::<&str>, PORTAL_PATH, INTERFACE, "Marker", &())
        .unwrap();
    assert_eq!(
        signals
            .recv_timeout(WAIT)
            .unwrap()
            .header()
            .member()
            .unwrap()
            .as_str(),
        "Marker"
    );

    // a question still up when the session closes goes away and the call answers 2
    create(&r.frontend, OTHER_SESSION, OTHER).unwrap();
    let done = bind(
        &r.frontend,
        2,
        OTHER_SESSION,
        vec![shortcut("open", "Open", Some("CTRL+ALT+o"))],
    );
    let (handle, _, _reply) = ask(&r);
    r.frontend
        .call_method(
            Some(BUS_NAME),
            OTHER_SESSION,
            Some("org.freedesktop.impl.portal.Session"),
            "Close",
            &(),
        )
        .unwrap();
    assert_eq!(done.recv_timeout(WAIT).unwrap().unwrap().0, 2);
    match r.events.recv_timeout(WAIT).unwrap() {
        Event::Close { handle: closed } => assert_eq!(closed, handle),
        Event::Ask { .. } => panic!("another ask"),
    }
}

#[test]
fn the_frontend_leaving_ends_every_session_and_only_it_may_call() {
    let Some(r) = start() else { return };
    let app = r.bus.connect();
    let refused = create(&app, SESSION, APP).unwrap_err();
    assert!(refused.to_string().contains("AccessDenied"), "{refused}");
    GlobalShortcutsConf::update(&r.answers(), |c| c.set(APP, Decision::Allow)).unwrap();
    create(&r.frontend, SESSION, APP).unwrap();
    bind(
        &r.frontend,
        1,
        SESSION,
        vec![shortcut("talk", "Push to talk", Some("CTRL+ALT+t"))],
    )
    .recv_timeout(WAIT)
    .unwrap()
    .unwrap();
    let [Keys::Bind { key, .. }] = r.keys()[..] else {
        panic!()
    };
    r.frontend.release_name(FRONTEND).unwrap();
    assert_eq!(r.keys.recv_timeout(WAIT).unwrap(), Keys::Unbind { key });
}
