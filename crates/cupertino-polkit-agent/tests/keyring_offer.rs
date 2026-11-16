//! The offer to move a default keyring that login does not unlock into the login keyring, against the keyring daemon
//! the packages install, on a private bus with a home and a runtime directory of the test's own (see
//! `keyring_daemon`). The test makes a second keyring with a password of its own the default one, stores items in it
//! and locks it, as it is after a login. The offer's questions and the daemon's password prompt reach the prompter's
//! model, which the test answers as the person would. What the keyrings hold is read over a session of the test's
//! own, apart from the one the move uses. Skipped, with the reason, when `gnome-keyring-daemon` is not installed.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_polkit_agent::keyring::asking::{run, Setup};
use cupertino_polkit_agent::keyring::model::{Kind, Phase};
use cupertino_polkit_agent::keyring::offer::{Moved, Outcome, KEEP, LOGIN, MOVE};
use cupertino_polkit_agent::keyring::prompter::{serve, Host};
use cupertino_test_bus::{PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::keyring_daemon::{call, locked, type_and, unlock, wait_until, Session, PASSWORD};

const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// The second keyring's own password.
const OTHER_PASSWORD: &str = "another one 81bd";

/// An item as the test compares them: label, attributes, secret, content type.
type Item = (String, BTreeMap<String, String>, Vec<u8>, String);

fn item(label: &str, account: &str, secret: &str) -> Item {
    (
        label.to_owned(),
        BTreeMap::from([
            ("service".to_owned(), "cupertino-offer-test".to_owned()),
            ("account".to_owned(), account.to_owned()),
        ]),
        secret.as_bytes().to_vec(),
        "text/plain".to_owned(),
    )
}

fn three() -> Vec<Item> {
    vec![
        item("Mail of ana", "ana", "value-1f0c-never-in-a-log"),
        item("Mail of ben", "ben", "value-77a2-never-in-a-log"),
        // not text, and longer than one cipher block; the daemon keeps no content type and calls every secret text
        (
            "Key".to_owned(),
            BTreeMap::from([("kind".to_owned(), "key".to_owned())]),
            (0u8..=40).collect(),
            "text/plain".to_owned(),
        ),
    ]
}

/// The prompter on `bus`, its loop on a thread, and its bus name.
fn prompter(bus: &PrivateBus) -> (Arc<Mutex<Host>>, String) {
    let conn = bus.connect();
    let name = conn.unique_name().unwrap().to_string();
    let (to_loop, requests) = channel();
    serve(&conn, move |request| {
        let _ = to_loop.send(request);
    })
    .unwrap();
    let host = Arc::new(Mutex::new(Host::new(conn)));
    let looping = host.clone();
    std::thread::spawn(move || {
        for request in requests {
            looping.lock().unwrap().request(request);
        }
    });
    (host, name)
}

fn wait_asking(host: &Arc<Mutex<Host>>, kind: Kind) {
    wait_until("a question of this kind is on screen", || {
        host.lock()
            .unwrap()
            .model
            .shown()
            .is_some_and(|p| p.phase == Phase::Asking && p.kind == kind)
    });
}

fn wait_idle(host: &Arc<Mutex<Host>>) {
    wait_until("no prompt is left", || {
        host.lock().unwrap().model.prompt().is_none()
    });
}

/// Press the question's continue button (`yes`) or its cancel button.
fn press(host: &Arc<Mutex<Host>>, yes: bool) {
    let mut host = host.lock().unwrap();
    let effects = if yes {
        host.model.answer()
    } else {
        host.model.cancel()
    };
    host.apply(effects);
}

/// The test's own view of the keyrings, over a session that does not encrypt.
struct Client {
    conn: Connection,
    session: OwnedObjectPath,
}

impl Client {
    fn new(bus: &PrivateBus) -> Client {
        let conn = bus.connect();
        let (_, session): (OwnedValue, OwnedObjectPath) = call(
            &conn,
            SERVICE_PATH,
            SERVICE,
            "OpenSession",
            &("plain", Value::from("")),
        );
        Client { conn, session }
    }

    fn default(&self) -> String {
        let path: OwnedObjectPath = call(
            &self.conn,
            SERVICE_PATH,
            SERVICE,
            "ReadAlias",
            &("default",),
        );
        path.to_string()
    }

    fn property<T: TryFrom<OwnedValue>>(&self, path: &str, interface: &str, name: &str) -> T
    where
        T::Error: std::fmt::Debug,
    {
        let value: OwnedValue = call(&self.conn, path, PROPERTIES, "Get", &(interface, name));
        T::try_from(value).unwrap()
    }

    fn store(&self, collection: &str, (label, attributes, secret, content_type): &Item) {
        let attributes: HashMap<&str, &str> = attributes
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from(label.as_str()),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(attributes),
            ),
        ]);
        let sealed = (
            ObjectPath::from(&self.session),
            Vec::<u8>::new(),
            secret.clone(),
            content_type.as_str(),
        );
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = call(
            &self.conn,
            collection,
            COLLECTION,
            "CreateItem",
            &(properties, sealed, false),
        );
        assert_eq!(prompt.as_str(), "/");
    }

    /// Every item of `collection`, sorted.
    fn items(&self, collection: &str) -> Vec<Item> {
        let paths: Vec<OwnedObjectPath> = self.property(collection, COLLECTION, "Items");
        let mut items: Vec<Item> = paths
            .iter()
            .map(|path| {
                let label: String = self.property(path.as_str(), ITEM, "Label");
                let attributes: HashMap<String, String> =
                    self.property(path.as_str(), ITEM, "Attributes");
                let (_, _, secret, content_type): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
                    call(
                        &self.conn,
                        path.as_str(),
                        ITEM,
                        "GetSecret",
                        &(ObjectPath::from(&self.session),),
                    );
                (
                    label,
                    attributes
                        .into_iter()
                        // the daemon's own note of an item's schema, which it writes on every item alike
                        .filter(|(name, _)| name != "xdg:schema")
                        .collect(),
                    secret,
                    content_type,
                )
            })
            .collect();
        items.sort();
        items
    }

    fn lock(&self, collection: &str) {
        let objects = vec![ObjectPath::try_from(collection).unwrap()];
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
            call(&self.conn, SERVICE_PATH, SERVICE, "Lock", &(objects,));
        assert_eq!(prompt.as_str(), "/");
        assert!(locked(&self.conn, collection));
    }

    /// Make a keyring with [`OTHER_PASSWORD`] and make it the default one, as an application does; store `items` in
    /// it and lock it. Its path. The keyring is made through the daemon's call that takes the new password with it,
    /// so no prompt is part of the preparation.
    fn other_default_keyring(&self, items: &[Item]) -> String {
        let properties: HashMap<&str, Value<'_>> = HashMap::from([(
            "org.freedesktop.Secret.Collection.Label",
            Value::from("Other"),
        )]);
        let password = (
            ObjectPath::from(&self.session),
            Vec::<u8>::new(),
            OTHER_PASSWORD.as_bytes().to_vec(),
            "text/plain",
        );
        let other: OwnedObjectPath = call(
            &self.conn,
            SERVICE_PATH,
            "org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface",
            "CreateWithMasterPassword",
            &(properties, password),
        );
        let other = other.to_string();
        let () = call(
            &self.conn,
            SERVICE_PATH,
            SERVICE,
            "SetAlias",
            &("default", ObjectPath::try_from(other.as_str()).unwrap()),
        );
        assert_eq!(self.default(), other);
        assert_ne!(other, LOGIN);
        for item in items {
            self.store(&other, item);
        }
        self.lock(&other);
        other
    }
}

fn setup(dir: &Path, prompter: &str, account: &str, login: &str) -> Setup {
    Setup {
        prompter: prompter.to_owned(),
        answer: dir.join(account).join("state/keyring-offer"),
        asked: dir.join(format!("{account}-asked")),
        login: login.to_owned(),
        wait: Duration::ZERO,
    }
}

/// The offer running on a thread and a connection of its own; the receiver gets how it ended.
fn start(bus: &PrivateBus, setup: &Setup) -> Receiver<Outcome> {
    let (conn, setup) = (bus.connect(), setup.clone());
    let (sent, ended) = channel();
    std::thread::spawn(move || {
        let _ = sent.send(run(&conn, &setup));
    });
    ended
}

fn ends(ended: &Receiver<Outcome>) -> Outcome {
    ended.recv_timeout(WAIT).expect("the offer ends")
}

#[test]
fn nothing_is_asked_without_a_second_default_keyring_an_unlocked_login_keyring_or_a_service() {
    let dir = tempfile::tempdir().unwrap();
    // no Secret Service on the bus
    if let Some(bus) = PrivateBus::start() {
        let (host, name) = prompter(&bus);
        let setup = setup(dir.path(), &name, "nobody", "1");
        assert_eq!(ends(&start(&bus, &setup)), Outcome::Quiet);
        assert!(host.lock().unwrap().model.prompt().is_none());
        assert!(!setup.asked.exists());
    }
    let Some(session) = Session::start() else {
        return;
    };
    let (host, name) = prompter(&session.bus);
    let client = Client::new(&session.bus);
    // the login keyring is the default one
    assert_eq!(client.default(), LOGIN);
    let first = setup(dir.path(), &name, "first", "1");
    assert_eq!(ends(&start(&session.bus, &first)), Outcome::Quiet);
    assert!(!first.asked.exists() && !first.answer.exists());

    // another keyring is the default, but the login keyring is locked
    let other = client.other_default_keyring(&three());
    client.lock(LOGIN);
    assert_eq!(ends(&start(&session.bus, &first)), Outcome::Quiet);
    assert!(host.lock().unwrap().model.prompt().is_none());
    assert!(!first.asked.exists() && !first.answer.exists());
    assert_eq!(client.default(), other);
}

#[test]
fn a_decline_is_kept_and_a_cancelled_password_or_a_question_taken_away_is_not() {
    let Some(session) = Session::start() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (host, name) = prompter(&session.bus);
    let client = Client::new(&session.bus);
    let other = client.other_default_keyring(&three());
    let unchanged = || {
        assert_eq!(client.default(), other);
        assert!(client.items(LOGIN).is_empty());
        assert!(locked(&client.conn, &other));
    };

    // the person accepts, then cancels the old keyring's password prompt
    let ana = setup(dir.path(), &name, "ana", "1");
    let ended = start(&session.bus, &ana);
    wait_asking(&host, Kind::Confirm);
    {
        let host = host.lock().unwrap();
        let prompt = host.model.shown().unwrap();
        assert!(prompt.properties.heading().contains("login keyring"));
        assert!(prompt.properties.description.contains("Other"));
        assert_eq!(prompt.properties.continue_title(), MOVE);
        assert_eq!(prompt.properties.cancel_title(), KEEP);
    }
    press(&host, true);
    wait_asking(&host, Kind::Password);
    type_and(&host, OTHER_PASSWORD, true);
    assert_eq!(ends(&ended), Outcome::Left);
    wait_idle(&host);
    unchanged();
    assert!(!ana.answer.exists(), "a cancel is not an answer");
    // not again in this login, even when the prompter starts again
    assert_eq!(ends(&start(&session.bus, &ana)), Outcome::Quiet);
    assert!(host.lock().unwrap().model.prompt().is_none());

    // the next login asks again; the session is locked under the question, which is no answer either
    let ana = setup(dir.path(), &name, "ana", "2");
    let ended = start(&session.bus, &ana);
    wait_asking(&host, Kind::Confirm);
    {
        let mut host = host.lock().unwrap();
        let effects = host.model.set_away(true);
        host.apply(effects);
    }
    assert_eq!(ends(&ended), Outcome::Left);
    assert!(!ana.answer.exists());
    {
        let mut host = host.lock().unwrap();
        let effects = host.model.set_away(false);
        host.apply(effects);
    }
    unchanged();

    // the login after that: Escape on the question is no answer, in this login no second question, and nothing kept
    let ana = setup(dir.path(), &name, "ana", "escape");
    let ended = start(&session.bus, &ana);
    wait_asking(&host, Kind::Confirm);
    {
        let mut host = host.lock().unwrap();
        let effects = host.model.escape();
        host.apply(effects);
    }
    assert_eq!(ends(&ended), Outcome::Left);
    wait_idle(&host);
    assert!(!ana.answer.exists(), "Escape is not a decline");
    assert_eq!(ends(&start(&session.bus, &ana)), Outcome::Quiet);
    unchanged();

    // the login after that: the person declines with the button, and no later login asks
    let ana = setup(dir.path(), &name, "ana", "3");
    let ended = start(&session.bus, &ana);
    wait_asking(&host, Kind::Confirm);
    press(&host, false);
    assert_eq!(ends(&ended), Outcome::Declined);
    wait_idle(&host);
    assert_eq!(
        std::fs::read_to_string(&ana.answer).unwrap().trim(),
        "declined"
    );
    let ana = setup(dir.path(), &name, "ana", "4");
    assert_eq!(ends(&start(&session.bus, &ana)), Outcome::Quiet);
    assert!(host.lock().unwrap().model.prompt().is_none());
    unchanged();
}

#[test]
fn accepting_copies_every_item_into_the_login_keyring_and_makes_it_the_default() {
    let Some(session) = Session::start() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (host, name) = prompter(&session.bus);
    let client = Client::new(&session.bus);
    // the login keyring's own items: one of another application, one that an earlier move that stopped halfway
    // already copied, and one with the attributes of an item to move and another secret
    let own = item("Wi-Fi", "home", "value-c3d9-never-in-a-log");
    let older = item("Mail of ben", "ben", "value-0000-an-older-one");
    for stored in [&own, &three()[0], &older] {
        client.store(LOGIN, stored);
    }
    let other = client.other_default_keyring(&three());

    let ana = setup(dir.path(), &name, "ana", "1");
    let ended = start(&session.bus, &ana);
    wait_asking(&host, Kind::Confirm);
    press(&host, true);
    // the old keyring's password, asked once, in the daemon's own prompt
    wait_asking(&host, Kind::Password);
    type_and(&host, OTHER_PASSWORD, false);
    assert_eq!(
        ends(&ended),
        Outcome::Moved(Moved {
            copied: 2,
            already: 1
        })
    );
    wait_idle(&host);

    assert_eq!(client.default(), LOGIN);
    let mut expected = three();
    expected.extend([own, older]);
    expected.sort();
    assert_eq!(client.items(LOGIN), expected);
    // the old keyring is kept with its items
    let mut kept = three();
    kept.sort();
    assert_eq!(client.items(&other), kept);
    assert_eq!(
        std::fs::read_to_string(&ana.answer).unwrap().trim(),
        "moved"
    );

    // a later login asks nothing, and neither would one that remembered nothing: the login keyring is the default
    for account in ["ana", "new"] {
        let next = setup(dir.path(), &name, account, "2");
        assert_eq!(ends(&start(&session.bus, &next)), Outcome::Quiet);
    }
    assert!(host.lock().unwrap().model.prompt().is_none());
    assert_eq!(client.items(LOGIN), expected);

    // the login keyring still unlocks at login as before
    client.lock(LOGIN);
    let completed = unlock(&client.conn, LOGIN);
    wait_asking(&host, Kind::Password);
    type_and(&host, PASSWORD, false);
    assert!(!completed.recv_timeout(WAIT).unwrap());
}
