//! The Secret Service a session gets (README, "Secret Service"): gnome-keyring's `secrets` component, the provider the
//! packages install, on a private bus that has no activatable services, with a home and a runtime directory of the
//! test's own. The daemon is started as `pam_gnome_keyring` starts it at login: with the login password on its standard
//! input (`--unlock`), or with none for a login that has no password (auto login). A client stores, reads and deletes a
//! secret over the D-Bus API, and libsecret's `secret-tool` does the same when it is installed. Once the keyring is
//! locked (the end of a session locks it) the client is refused, and an unlock asks the system prompter, here a
//! stand-in that records the request and refuses it as a person dismissing the prompt would. Neither the password nor
//! a stored value appears in what the daemon prints. Skipped, with the reason, when `gnome-keyring-daemon` is not
//! installed (its `secret-tool` part when that is not); `CUPERTINO_KEYRING_REQUIRED=1` fails instead.
#![cfg(feature = "app")]

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use cupertino_test_bus::{PrivateBus, WAIT};
use serde::de::DeserializeOwned;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Type, Value};

const SECRETS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const IS_LOCKED: &str = "org.freedesktop.Secret.Error.IsLocked";
/// The name the daemon asks for a prompt on: gcr's system prompter, a window in a real session.
const PROMPTER: &str = "org.gnome.keyring.SystemPrompter";
const PROMPTER_PATH: &str = "/org/gnome/keyring/Prompter";
/// What "no object" is on the Secret Service API.
const NO_PROMPT: &str = "/";

/// The login password of the test's account.
const PASSWORD: &str = "correct horse 7f3a";
/// The secret the client stores.
const VALUE: &str = "token-5d1e-never-in-a-log";
/// The secret `secret-tool` stores.
const TOOL_VALUE: &str = "token-90b2-never-in-a-log";
/// The attribute that finds the test's items.
const ATTRIBUTE: (&str, &str) = ("service", "cupertino-secret-service-test");

/// A secret on the bus: the session, the algorithm's parameters, the value and its content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// The daemon's path when it is installed; else the test is skipped (or fails when it must run).
fn keyring_daemon() -> Option<PathBuf> {
    let found = installed("gnome-keyring-daemon");
    if found.is_none() {
        assert!(
            std::env::var("CUPERTINO_KEYRING_REQUIRED").as_deref() != Ok("1"),
            "gnome-keyring-daemon missing and CUPERTINO_KEYRING_REQUIRED=1"
        );
        eprintln!(
            "SKIPPED: gnome-keyring-daemon is not installed; set CUPERTINO_KEYRING_REQUIRED=1 to fail instead"
        );
    }
    found
}

fn installed(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

/// Until `done` holds, polled every 20 ms for at most [`WAIT`].
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what} within {WAIT:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One account's session: the private bus, the account's home and runtime directory, and the keyring daemon of the
/// current login.
struct Session {
    program: PathBuf,
    dir: tempfile::TempDir,
    bus: PrivateBus,
    watcher: Connection,
    daemon: Option<Child>,
    logins: u32,
}

impl Session {
    fn start() -> Option<Session> {
        let program = keyring_daemon()?;
        // Under /tmp so that the daemon's control socket stays within the 108 bytes of a socket path.
        let dir = tempfile::Builder::new()
            .prefix("kr")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["home", "run"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let config = dir.path().join("bus.conf");
        // No <servicedir>: nothing on this bus is started on demand, not the machine's prompter nor its keyring.
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
        let watcher = bus.connect();
        Some(Session {
            program,
            dir,
            bus,
            watcher,
            daemon: None,
            logins: 0,
        })
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn has_provider(&self) -> bool {
        zbus::blocking::fdo::DBusProxy::new(&self.watcher)
            .unwrap()
            .name_has_owner(SECRETS.try_into().unwrap())
            .unwrap()
    }

    /// A login: the daemon starts with the login password, as `pam_gnome_keyring` starts or unlocks it, or with none
    /// (a login without a password). The daemon of an earlier login is stopped first.
    fn login(&mut self, password: Option<&str>) {
        self.end();
        self.logins += 1;
        let log = File::create(self.log_path(self.logins)).unwrap();
        let run = self.dir.path().join("run");
        let mut command = Command::new(&self.program);
        command
            .args(["--foreground", "--components=secrets"])
            .arg(format!(
                "--control-directory={}",
                run.join("keyring").display()
            ))
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", &run)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus.address)
            .current_dir(self.home())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        if password.is_some() {
            command.arg("--unlock").stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
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
        if let Some(password) = password {
            // the daemon reads the password to the end of its input, as the PAM module writes it
            let mut stdin = daemon.stdin.take().unwrap();
            stdin.write_all(password.as_bytes()).unwrap();
        }
        self.daemon = Some(daemon);
        wait_until("the daemon owns org.freedesktop.secrets", || {
            assert!(
                self.daemon.as_mut().unwrap().try_wait().unwrap().is_none(),
                "gnome-keyring-daemon exited: {}",
                self.logs()
            );
            self.has_provider()
        });
    }

    /// The daemon goes away, as at the end of the user's last session.
    fn end(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
            wait_until("the bus drops the daemon's name", || !self.has_provider());
        }
    }

    fn log_path(&self, login: u32) -> PathBuf {
        self.dir.path().join(format!("daemon-{login}.log"))
    }

    /// Everything the daemons of this session printed.
    fn logs(&self) -> String {
        (1..=self.logins)
            .map(|login| std::fs::read_to_string(self.log_path(login)).unwrap_or_default())
            .collect()
    }

    fn assert_no_secret_in_logs(&self) {
        let logs = self.logs();
        for secret in [PASSWORD, VALUE, TOOL_VALUE] {
            assert!(!logs.contains(secret), "a secret is in the daemon's output");
        }
    }

    fn client(&self) -> Client {
        Client::open(self.bus.connect())
    }

    /// `secret-tool` (libsecret, the library browsers and mail clients use) with the session's bus and home.
    fn secret_tool(&self, args: &[&str], input: Option<&str>) -> Option<Output> {
        let Some(tool) = installed("secret-tool") else {
            assert!(
                std::env::var("CUPERTINO_KEYRING_REQUIRED").as_deref() != Ok("1"),
                "secret-tool missing and CUPERTINO_KEYRING_REQUIRED=1"
            );
            return None;
        };
        let mut child = Command::new(tool)
            .args(args)
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home())
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus.address)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        drop(stdin);
        let (sent, output) = channel();
        std::thread::spawn(move || sent.send(child.wait_with_output()));
        Some(
            output
                .recv_timeout(WAIT * 2)
                .expect("secret-tool finished in time")
                .unwrap(),
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
    }
}

/// A desktop client of the Secret Service with a session of the `plain` algorithm (the bus is private to the test).
struct Client {
    conn: Connection,
    session: OwnedObjectPath,
}

impl Client {
    fn open(conn: Connection) -> Client {
        let (_, session): (OwnedValue, OwnedObjectPath) = call(
            &conn,
            SERVICE_PATH,
            SERVICE,
            "OpenSession",
            &("plain", Value::from("")),
        )
        .unwrap();
        Client { conn, session }
    }

    fn call<R: DeserializeOwned + Type>(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        body: &(impl serde::Serialize + DynamicType),
    ) -> Result<R, String> {
        call(&self.conn, path, interface, method, body)
    }

    /// The collection secrets are stored in by default: the login keyring.
    fn default_collection(&self) -> OwnedObjectPath {
        self.call(SERVICE_PATH, SERVICE, "ReadAlias", &("default",))
            .unwrap()
    }

    fn locked(&self, collection: &str) -> bool {
        zbus::blocking::fdo::PropertiesProxy::builder(&self.conn)
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

    fn store(&self, collection: &str, value: &str) -> Result<OwnedObjectPath, String> {
        let attributes = HashMap::from([ATTRIBUTE]);
        let properties = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from("Cupertino test"),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(attributes),
            ),
        ]);
        let secret = (
            ObjectPath::from(&self.session),
            Vec::<u8>::new(),
            value.as_bytes().to_vec(),
            "text/plain",
        );
        let (item, prompt): (OwnedObjectPath, OwnedObjectPath) = self.call(
            collection,
            COLLECTION,
            "CreateItem",
            &(properties, secret, true),
        )?;
        assert_eq!(
            prompt.as_str(),
            NO_PROMPT,
            "an unlocked keyring stores without a prompt"
        );
        Ok(item)
    }

    /// The test's items, unlocked and locked.
    fn search(&self) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        self.call(
            SERVICE_PATH,
            SERVICE,
            "SearchItems",
            &(HashMap::from([ATTRIBUTE]),),
        )
        .unwrap()
    }

    fn read(&self, item: &str) -> Result<String, String> {
        let (_, _, value, _): Secret =
            self.call(item, ITEM, "GetSecret", &(ObjectPath::from(&self.session),))?;
        Ok(String::from_utf8(value).unwrap())
    }

    fn delete(&self, item: &str) -> Result<(), String> {
        let prompt: OwnedObjectPath = self.call(item, ITEM, "Delete", &())?;
        assert_eq!(prompt.as_str(), NO_PROMPT);
        Ok(())
    }

    fn lock(&self, objects: &[&str]) {
        let objects: Vec<ObjectPath> = objects
            .iter()
            .map(|o| ObjectPath::try_from(*o).unwrap())
            .collect();
        let (locked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self
            .call(SERVICE_PATH, SERVICE, "Lock", &(objects,))
            .unwrap();
        assert_eq!(prompt.as_str(), NO_PROMPT);
        assert!(!locked.is_empty());
    }

    /// Asks to unlock `collection`: what is unlocked at once, and the prompt that would unlock the rest.
    fn unlock(&self, collection: &str) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        let objects = vec![ObjectPath::try_from(collection).unwrap()];
        self.call(SERVICE_PATH, SERVICE, "Unlock", &(objects,))
            .unwrap()
    }

    /// Runs `start` on `prompt` and waits for its `Completed`: whether it was dismissed.
    fn complete(
        &self,
        prompt: &str,
        start: &str,
        body: &(impl serde::Serialize + DynamicType),
    ) -> bool {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(PROMPT)
            .unwrap()
            .member("Completed")
            .unwrap()
            .path(prompt.to_owned())
            .unwrap()
            .build();
        let mut completions = MessageIterator::for_match_rule(rule, &self.conn, None).unwrap();
        let (sent, completed) = channel();
        std::thread::spawn(move || {
            if let Some(Ok(message)) = completions.next() {
                let (dismissed, _): (bool, OwnedValue) = message.body().deserialize().unwrap();
                let _ = sent.send(dismissed);
            }
        });
        let () = self.call(prompt, PROMPT, start, body).unwrap();
        completed.recv_timeout(WAIT).expect("the prompt completed")
    }
}

fn call<R: DeserializeOwned + Type>(
    conn: &Connection,
    path: &str,
    interface: &str,
    method: &str,
    body: &(impl serde::Serialize + DynamicType),
) -> Result<R, String> {
    let reply = conn
        .call_method(Some(SECRETS), path, Some(interface), method, body)
        .map_err(|error| match error {
            zbus::Error::MethodError(name, _, _) => name.to_string(),
            other => panic!("{interface}.{method}: {other}"),
        })?;
    Ok(reply.body().deserialize().unwrap())
}

/// The system prompter's stand-in: it records each request and refuses it, as a person dismissing the window does.
struct Prompter(Sender<OwnedObjectPath>);

#[zbus::interface(name = "org.gnome.keyring.internal.Prompter")]
impl Prompter {
    fn begin_prompting(&self, callback: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        let _ = self.0.send(callback.into());
        Err(zbus::fdo::Error::AccessDenied(
            "the prompt was dismissed".into(),
        ))
    }
}

fn prompter(bus: &PrivateBus) -> (Connection, Receiver<OwnedObjectPath>) {
    let (asked, requests) = channel();
    let conn = bus
        .blocking_builder()
        .name(PROMPTER)
        .unwrap()
        .serve_at(PROMPTER_PATH, Prompter(asked))
        .unwrap()
        .build()
        .unwrap();
    (conn, requests)
}

#[test]
fn after_login_a_client_stores_reads_and_deletes_a_secret() {
    let Some(mut session) = Session::start() else {
        return;
    };
    session.login(Some(PASSWORD));
    let client = session.client();
    let login = client.default_collection();
    assert_eq!(login.as_str(), "/org/freedesktop/secrets/collection/login");
    assert!(
        !client.locked(&login),
        "the login password unlocks the login keyring"
    );

    let item = client.store(&login, VALUE).unwrap();
    assert_eq!(client.search(), (vec![item.clone()], vec![]));
    assert_eq!(client.read(&item).unwrap(), VALUE);
    // another client of the same session reads it too
    assert_eq!(session.client().read(&item).unwrap(), VALUE);
    client.delete(&item).unwrap();
    assert_eq!(client.search(), (vec![], vec![]));

    match session.secret_tool(
        &["store", "--label=Cupertino test", ATTRIBUTE.0, ATTRIBUTE.1],
        Some(TOOL_VALUE),
    ) {
        None => eprintln!(
            "SKIPPED (part): secret-tool is not installed; the libsecret client was not run"
        ),
        Some(stored) => {
            assert!(stored.status.success(), "secret-tool store failed");
            let found = session
                .secret_tool(&["lookup", ATTRIBUTE.0, ATTRIBUTE.1], None)
                .unwrap();
            assert!(found.status.success());
            assert_eq!(String::from_utf8_lossy(&found.stdout), TOOL_VALUE);
            let cleared = session
                .secret_tool(&["clear", ATTRIBUTE.0, ATTRIBUTE.1], None)
                .unwrap();
            assert!(cleared.status.success());
            let gone = session
                .secret_tool(&["lookup", ATTRIBUTE.0, ATTRIBUTE.1], None)
                .unwrap();
            assert!(!gone.status.success() && gone.stdout.is_empty());
            for output in [stored, found, cleared, gone] {
                assert!(!String::from_utf8_lossy(&output.stderr).contains(TOOL_VALUE));
            }
        }
    }
    session.assert_no_secret_in_logs();
}

#[test]
fn a_locked_keyring_refuses_the_client_and_an_unlock_asks_the_prompter() {
    let Some(mut session) = Session::start() else {
        return;
    };
    session.login(Some(PASSWORD));
    let (_prompter, requests) = prompter(&session.bus);
    let client = session.client();
    let login = client.default_collection();
    let item = client.store(&login, VALUE).unwrap();

    // what the end of a session does: every collection is locked
    client.lock(&[login.as_str()]);
    assert!(client.locked(&login));
    assert_eq!(client.search(), (vec![], vec![item.clone()]));
    assert_eq!(client.read(&item), Err(IS_LOCKED.to_owned()));
    assert!(
        client.store(&login, "another").is_err(),
        "a locked keyring takes no new item"
    );
    assert!(requests.try_recv().is_err(), "a refusal asks nobody");

    // unlocking needs the person: the daemon hands back a prompt, and running it asks the system prompter
    let (unlocked, prompt) = client.unlock(&login);
    assert!(unlocked.is_empty());
    assert_ne!(prompt.as_str(), NO_PROMPT);
    assert!(
        client.complete(&prompt, "Prompt", &("",)),
        "a refused prompt completes dismissed"
    );
    requests
        .recv_timeout(WAIT)
        .expect("the daemon asked the system prompter");
    assert!(client.locked(&login));
    assert_eq!(client.read(&item), Err(IS_LOCKED.to_owned()));

    // a client that gives up dismisses its prompt itself; nobody is asked
    let (_, prompt) = client.unlock(&login);
    assert!(client.complete(&prompt, "Dismiss", &()));
    assert!(requests.try_recv().is_err());
    assert!(client.locked(&login));

    // libsecret asks for the unlock itself and is refused the same way
    if let Some(found) = session.secret_tool(&["lookup", ATTRIBUTE.0, ATTRIBUTE.1], None) {
        assert!(!found.status.success() && found.stdout.is_empty());
        requests
            .recv_timeout(WAIT)
            .expect("libsecret's unlock asked the system prompter");
        let stored = session
            .secret_tool(
                &["store", "--label=x", ATTRIBUTE.0, "other"],
                Some(TOOL_VALUE),
            )
            .unwrap();
        assert!(
            !stored.status.success(),
            "secret-tool stored into a locked keyring"
        );
    }
    session.assert_no_secret_in_logs();
}

#[test]
fn a_wrong_or_missing_login_password_leaves_the_keyring_locked_and_the_right_one_opens_it() {
    let Some(mut session) = Session::start() else {
        return;
    };
    // the first login makes the login keyring with the login password
    session.login(Some(PASSWORD));
    let client = session.client();
    let login = client.default_collection();
    let item = client.store(&login, VALUE).unwrap();
    assert!(session
        .home()
        .join(".local/share/keyrings/login.keyring")
        .is_file());

    // a password that is not the keyring's (changed elsewhere, or mistyped by a module that let it through)
    session.login(Some("not the password"));
    let client = session.client();
    assert!(client.locked(&login));
    assert_eq!(client.read(&item), Err(IS_LOCKED.to_owned()));

    // a login without a password: the keyring stays locked until the person unlocks it
    session.login(None);
    let client = session.client();
    assert!(client.locked(&login));
    assert_eq!(client.read(&item), Err(IS_LOCKED.to_owned()));

    session.login(Some(PASSWORD));
    let client = session.client();
    assert!(!client.locked(&login));
    assert_eq!(client.read(&item).unwrap(), VALUE);
    session.assert_no_secret_in_logs();
}

/// The login keyring on disk is readable by the account alone and holds no secret in clear: what logout leaves behind.
#[test]
fn the_keyring_files_belong_to_the_account_alone() {
    let Some(mut session) = Session::start() else {
        return;
    };
    session.login(Some(PASSWORD));
    session
        .client()
        .store(&session.client().default_collection(), VALUE)
        .unwrap();
    let dir = session.home().join(".local/share/keyrings");
    let mode = |path: &Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    };
    assert_eq!(mode(&dir), 0o700);
    let file = dir.join("login.keyring");
    wait_until("the login keyring is written", || file.is_file());
    assert_eq!(mode(&file), 0o600);
    let bytes = std::fs::read(&file).unwrap();
    let stored = VALUE.as_bytes();
    assert!(
        !bytes.windows(stored.len()).any(|w| w == stored),
        "the login keyring holds the secret in clear"
    );
}
