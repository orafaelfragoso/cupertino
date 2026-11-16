//! The keyring prompter answering the keyring daemon the packages install (gnome-keyring's `secrets` component) on a
//! private bus with no activatable services: a locked login keyring is unlocked with the password typed into the
//! prompt, after a wrong one the daemon asks again with its warning, and a cancel leaves the keyring locked; a keyring an
//! application asks for is made with the new password typed twice, and opens with it. Skipped, with the reason, when `gnome-keyring-daemon` is not installed; `CUPERTINO_KEYRING_REQUIRED=1` fails instead.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_polkit_agent::keyring::model::{Field, Key, Kind, Phase};
use cupertino_polkit_agent::keyring::panel::Status;
use cupertino_polkit_agent::keyring::prompter::{serve, Host};
use cupertino_test_bus::{PrivateBus, WAIT};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SECRETS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
/// The login password of the test's account.
pub(super) const PASSWORD: &str = "correct horse 2c9e";

fn keyring_daemon() -> Option<PathBuf> {
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("gnome-keyring-daemon"))
            .find(|path| path.is_file())
    });
    if found.is_none() {
        assert!(
            std::env::var("CUPERTINO_KEYRING_REQUIRED").as_deref() != Ok("1"),
            "gnome-keyring-daemon missing and CUPERTINO_KEYRING_REQUIRED=1"
        );
        eprintln!("SKIPPED: gnome-keyring-daemon is not installed; set CUPERTINO_KEYRING_REQUIRED=1 to fail instead");
    }
    found
}

pub(super) fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what} within {WAIT:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The daemon of a login with [`PASSWORD`], its home and runtime directory, on a bus of the test's own.
pub(super) struct Session {
    _dir: tempfile::TempDir,
    pub(super) bus: PrivateBus,
    daemon: Child,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

impl Session {
    pub(super) fn start() -> Option<Session> {
        let program = keyring_daemon()?;
        // under /tmp so that the daemon's control socket stays within the 108 bytes of a socket path
        let dir = tempfile::Builder::new()
            .prefix("kp")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["home", "run"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let config = dir.path().join("bus.conf");
        // no <servicedir>: nothing on this bus is started on demand, not the machine's prompter nor its keyring
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
                dir.path().display()
            ),
        )
        .unwrap();
        let bus = PrivateBus::start_with_config(&config)?;
        let home = dir.path().join("home");
        let run = dir.path().join("run");
        let mut command = Command::new(program);
        command
            .args(["--foreground", "--components=secrets", "--unlock"])
            .arg(format!(
                "--control-directory={}",
                run.join("keyring").display()
            ))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("CUPERTINO_DIAG_JOURNAL_SOCKET", dir.path().join("journal"))
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &run)
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .current_dir(&home)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Safety: the closure runs in the child between fork and exec and calls only prctl, which is
        // async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut daemon = command.spawn().expect("starting gnome-keyring-daemon");
        daemon
            .stdin
            .take()
            .unwrap()
            .write_all(PASSWORD.as_bytes())
            .unwrap();
        let session = Session {
            _dir: dir,
            bus,
            daemon,
        };
        let watcher = session.bus.connect();
        let dbus = zbus::blocking::fdo::DBusProxy::new(&watcher).unwrap();
        wait_until("the daemon owns org.freedesktop.secrets", || {
            dbus.name_has_owner(SECRETS.try_into().unwrap()).unwrap()
        });
        Some(session)
    }
}

pub(super) fn call<R: zbus::export::serde::de::DeserializeOwned + zbus::zvariant::Type>(
    conn: &Connection,
    path: &str,
    interface: &str,
    method: &str,
    body: &(impl zbus::export::serde::Serialize + zbus::zvariant::DynamicType),
) -> R {
    conn.call_method(Some(SECRETS), path, Some(interface), method, body)
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
}

pub(super) fn locked(conn: &Connection, collection: &str) -> bool {
    zbus::blocking::fdo::PropertiesProxy::builder(conn)
        .destination(SECRETS)
        .unwrap()
        .path(collection)
        .unwrap()
        .build()
        .unwrap()
        .get(COLLECTION.try_into().unwrap(), "Locked")
        .map(|value| bool::try_from(value).unwrap())
        .unwrap()
}

/// Run the daemon's `prompt` on a thread; the receiver gets whether it was dismissed, and its result.
fn run_prompt(conn: &Connection, prompt: &str) -> std::sync::mpsc::Receiver<(bool, OwnedValue)> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(PROMPT)
        .unwrap()
        .member("Completed")
        .unwrap()
        .path(prompt.to_owned())
        .unwrap()
        .build();
    let mut completions = MessageIterator::for_match_rule(rule, conn, None).unwrap();
    let (sent, completed) = channel();
    std::thread::spawn(move || {
        if let Some(Ok(message)) = completions.next() {
            let _ = sent.send(message.body().deserialize().unwrap());
        }
    });
    let () = call(conn, prompt, PROMPT, "Prompt", &("",));
    completed
}

/// Ask the daemon to unlock `collection` and run its prompt on a thread; the receiver gets whether it was dismissed.
pub(super) fn unlock(conn: &Connection, collection: &str) -> std::sync::mpsc::Receiver<bool> {
    let objects = vec![ObjectPath::try_from(collection).unwrap()];
    let (unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
        call(conn, SERVICE_PATH, SERVICE, "Unlock", &(objects,));
    assert!(unlocked.is_empty());
    let completed = run_prompt(conn, prompt.as_str());
    let (sent, dismissed) = channel();
    std::thread::spawn(move || {
        if let Ok((was_dismissed, _)) = completed.recv() {
            let _ = sent.send(was_dismissed);
        }
    });
    dismissed
}

/// Ask the daemon for a new keyring labelled `label`, as an application does, and run its prompt on a thread; the
/// receiver gets whether it was dismissed, and the keyring's path.
fn create(conn: &Connection, label: &str) -> std::sync::mpsc::Receiver<(bool, OwnedValue)> {
    let properties: HashMap<&str, Value<'_>> = HashMap::from([(
        "org.freedesktop.Secret.Collection.Label",
        Value::from(label),
    )]);
    let (made, prompt): (OwnedObjectPath, OwnedObjectPath) = call(
        conn,
        SERVICE_PATH,
        SERVICE,
        "CreateCollection",
        &(properties, ""),
    );
    assert_eq!(made.as_str(), "/", "a new keyring needs its password first");
    run_prompt(conn, prompt.as_str())
}

/// The paths of the daemon's keyrings.
fn collections(conn: &Connection) -> Vec<String> {
    let all = zbus::blocking::fdo::PropertiesProxy::builder(conn)
        .destination(SECRETS)
        .unwrap()
        .path(SERVICE_PATH)
        .unwrap()
        .build()
        .unwrap()
        .get(SERVICE.try_into().unwrap(), "Collections")
        .unwrap();
    Vec::<OwnedObjectPath>::try_from(all)
        .unwrap()
        .iter()
        .map(|path| path.to_string())
        .collect()
}

/// The prompter on the session's bus with its loop on a thread, as the program runs it.
fn prompter(session: &Session) -> Arc<Mutex<Host>> {
    let prompter = session.bus.connect();
    let (to_loop, requests) = channel();
    serve(&prompter, move |request| {
        let _ = to_loop.send(request);
    })
    .unwrap();
    let host = Arc::new(Mutex::new(Host::new(prompter)));
    let looping = host.clone();
    std::thread::spawn(move || {
        for request in requests {
            looping.lock().unwrap().request(request);
        }
    });
    host
}

fn type_into(host: &Arc<Mutex<Host>>, field: Field, text: &str) {
    let mut host = host.lock().unwrap();
    for c in text.chars() {
        host.model.key(field, Key::Char(c));
    }
}

/// Wait for the daemon's question for a new keyring's password: a password with its confirmation.
fn wait_for_new_password(host: &Arc<Mutex<Host>>) {
    wait_for_question(host);
    let host = host.lock().unwrap();
    let prompt = host.model.shown().unwrap();
    assert_eq!(prompt.kind, Kind::Password);
    assert!(prompt.confirms(), "a new password is typed twice");
    assert!(
        !prompt.properties.heading().is_empty(),
        "the daemon says what it asks"
    );
}

pub(super) fn wait_for_question(host: &Arc<Mutex<Host>>) {
    wait_until("the daemon asks the prompter", || {
        host.lock()
            .unwrap()
            .model
            .shown()
            .is_some_and(|p| p.phase == Phase::Asking)
    });
}

pub(super) fn type_and(host: &Arc<Mutex<Host>>, password: &str, cancel: bool) {
    let mut host = host.lock().unwrap();
    for c in password.chars() {
        host.model.key(Field::Password, Key::Char(c));
    }
    let effects = if cancel {
        host.model.cancel()
    } else {
        host.model.answer()
    };
    host.apply(effects);
}

#[test]
fn the_daemon_unlocks_its_keyring_with_the_password_typed_into_the_prompt() {
    let Some(session) = Session::start() else {
        return;
    };
    let host = prompter(&session);

    let client = session.bus.connect();
    let _: (OwnedValue, OwnedObjectPath) = call(
        &client,
        SERVICE_PATH,
        SERVICE,
        "OpenSession",
        &("plain", Value::from("")),
    );
    let login: OwnedObjectPath = call(&client, SERVICE_PATH, SERVICE, "ReadAlias", &("default",));
    let objects = vec![ObjectPath::try_from(login.as_str()).unwrap()];
    let _: (Vec<OwnedObjectPath>, OwnedObjectPath) =
        call(&client, SERVICE_PATH, SERVICE, "Lock", &(objects,));
    assert!(locked(&client, &login));

    // cancelled: the keyring stays locked
    let completed = unlock(&client, &login);
    wait_for_question(&host);
    type_and(&host, PASSWORD, true);
    assert!(
        completed.recv_timeout(WAIT).unwrap(),
        "a cancelled prompt completes dismissed"
    );
    assert!(locked(&client, &login));
    wait_until("the daemon stops prompting", || {
        host.lock().unwrap().model.prompt().is_none()
    });

    // a wrong password, then the right one
    let completed = unlock(&client, &login);
    wait_for_question(&host);
    {
        let host = host.lock().unwrap();
        let prompt = host.model.shown().unwrap();
        assert_eq!(prompt.kind, Kind::Password);
        assert!(
            !prompt.properties.heading().is_empty(),
            "the daemon says what it asks"
        );
        assert!(prompt.properties.warning.is_empty());
        assert!(!prompt.confirms());
    }
    type_and(&host, "not the password", false);
    wait_until("the daemon asks again with its warning", || {
        let host = host.lock().unwrap();
        host.model.shown().is_some_and(|p| {
            p.phase == Phase::Asking
                && matches!(Status::of(p, false).first(), Some(Status::Warning(_)))
        })
    });
    assert!(locked(&client, &login));
    type_and(&host, PASSWORD, false);
    assert!(
        !completed.recv_timeout(WAIT).unwrap(),
        "the prompt completes unlocked"
    );
    assert!(!locked(&client, &login));
    wait_until("the daemon stops prompting", || {
        host.lock().unwrap().model.prompt().is_none()
    });
}

/// A password for the keyring the test makes: not the login's.
const NEW_PASSWORD: &str = "battery staple 7f1a";

#[test]
fn the_daemon_makes_a_keyring_with_the_new_password_typed_twice_into_the_prompt() {
    let Some(session) = Session::start() else {
        return;
    };
    let host = prompter(&session);
    let client = session.bus.connect();
    let _: (OwnedValue, OwnedObjectPath) = call(
        &client,
        SERVICE_PATH,
        SERVICE,
        "OpenSession",
        &("plain", Value::from("")),
    );
    let before = collections(&client);

    // cancelled: no keyring is made
    let completed = create(&client, "Cancelled");
    wait_for_new_password(&host);
    type_into(&host, Field::Password, NEW_PASSWORD);
    type_into(&host, Field::Confirm, NEW_PASSWORD);
    type_and(&host, "", true);
    let (dismissed, _) = completed.recv_timeout(WAIT).unwrap();
    assert!(dismissed, "a cancelled prompt completes dismissed");
    assert_eq!(collections(&client), before);
    wait_until("the daemon stops prompting", || {
        host.lock().unwrap().model.prompt().is_none()
    });

    // a confirmation that differs is not sent: the question stays, says so, and the daemon hears nothing
    let completed = create(&client, "Made");
    wait_for_new_password(&host);
    type_into(&host, Field::Confirm, "not the same");
    type_and(&host, NEW_PASSWORD, false);
    {
        let host = host.lock().unwrap();
        let prompt = host.model.shown().unwrap();
        assert_eq!(prompt.phase, Phase::Asking);
        assert_eq!(Status::of(prompt, false), [Status::Mismatch]);
        assert_eq!(prompt.field_len(Field::Confirm), 0);
    }
    // only the password typed, as for an unlock, is a confirmation that differs too
    type_and(&host, "", false);
    assert!(host.lock().unwrap().model.shown().unwrap().mismatch);
    assert!(completed.try_recv().is_err(), "nothing was answered");
    assert_eq!(collections(&client), before);

    // the same password twice makes the keyring, unlocked
    type_into(&host, Field::Confirm, NEW_PASSWORD);
    type_and(&host, "", false);
    let (dismissed, made) = completed.recv_timeout(WAIT).unwrap();
    assert!(!dismissed, "the prompt completes with the keyring");
    let made = OwnedObjectPath::try_from(made).unwrap().to_string();
    assert!(!before.contains(&made));
    assert!(collections(&client).contains(&made));
    assert!(!locked(&client, &made));
    wait_until("the daemon stops prompting", || {
        host.lock().unwrap().model.prompt().is_none()
    });

    // locked again, it opens with the password that was typed and with no other
    let objects = vec![ObjectPath::try_from(made.as_str()).unwrap()];
    let _: (Vec<OwnedObjectPath>, OwnedObjectPath) =
        call(&client, SERVICE_PATH, SERVICE, "Lock", &(objects,));
    assert!(locked(&client, &made));
    let completed = unlock(&client, &made);
    wait_for_question(&host);
    assert!(!host.lock().unwrap().model.shown().unwrap().confirms());
    type_and(&host, PASSWORD, false);
    wait_until("the daemon asks again with its warning", || {
        let host = host.lock().unwrap();
        host.model.shown().is_some_and(|p| {
            p.phase == Phase::Asking
                && matches!(Status::of(p, false).first(), Some(Status::Warning(_)))
        })
    });
    assert!(locked(&client, &made));
    // the daemon offers to open this keyring with the login from now on: the choice, ticked, goes back to it
    {
        let mut host = host.lock().unwrap();
        assert!(host.model.shown().unwrap().has_choice());
        host.model.toggle_choice();
    }
    type_and(&host, NEW_PASSWORD, false);
    assert!(
        !completed.recv_timeout(WAIT).unwrap(),
        "the prompt completes unlocked"
    );
    assert!(!locked(&client, &made));
    wait_until("the daemon stops prompting", || {
        host.lock().unwrap().model.prompt().is_none()
    });

    // so the next time it is locked, the daemon opens it without a question
    let objects = vec![ObjectPath::try_from(made.as_str()).unwrap()];
    let _: (Vec<OwnedObjectPath>, OwnedObjectPath) =
        call(&client, SERVICE_PATH, SERVICE, "Lock", &(objects,));
    assert!(locked(&client, &made));
    let completed = unlock(&client, &made);
    assert!(
        !completed.recv_timeout(WAIT).unwrap(),
        "the prompt completes unlocked with nothing typed"
    );
    assert!(!locked(&client, &made));
    assert!(host.lock().unwrap().model.shown().is_none());
}
