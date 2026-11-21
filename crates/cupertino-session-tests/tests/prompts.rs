//! The session's prompts (`cupertino-prompts`) as the server starts them: run in this process on a connection with the
//! `session`, `prompt` and `consent` roles, against the real server, headless, on a bus of the test's own that stands
//! for both buses, with stand-ins for logind, polkitd and polkit's helper.
//! - The authentication agent registers with polkitd for the session logind names; a `BeginAuthentication` from
//!   polkitd raises the alert, the password typed reaches the helper with the user and the cookie, and the helper's
//!   success answers polkitd. polkitd's `CancelAuthentication` takes an alert away. A caller other than polkitd is
//!   refused.
//! - The keyring's prompter answers a gcr-style caller (`BeginPrompting`, `PerformPrompt`, its callback object's
//!   `PromptReady`): the password typed comes back through the secret exchange, and another caller is refused while
//!   one is active.
//! - `org.cupertino.Prompts1` takes the portal's consent questions from the program the server started as `portal`
//!   (a stand-in of `Session1` names one test connection so) and refuses any other caller: the screenshot question is
//!   granted with Take Screenshot once the arming delay has passed, and cancelled with Escape; the shortcuts question
//!   is allowed with Allow and refused with Don't Allow; the portal's `Close` takes a question down.
//! - While an alert is up the server counts it as a prompt: desktop control is suspended and observes nothing
//!   (`prompt_shown`), every surface of the program is a prompt surface, the keyboard is the alert's, and the output's
//!   capture does not show it.

// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

/// The rig names a host program; no test here starts a shell host.
mod host {
    pub const MARK: &str = "cupertino-prompts-tests-host";
}

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_prompts::consent::service::{
    BUS_NAME as PROMPTS_NAME, INTERFACE as PROMPTS_INTERFACE, OBJECT_PATH as PROMPTS_PATH,
};
use cupertino_prompts::consent::{
    ARMING, SCREENSHOT_CANCELLED, SCREENSHOT_GRANTED, SHORTCUTS_ALLOWED, SHORTCUTS_DENIED,
};
use cupertino_prompts::keyring::exchange::Exchange;
use cupertino_prompts::keyring::prompter::{ERROR_IN_PROGRESS, INTERFACE, PATH, SYSTEM_NAME};
use cupertino_prompts::polkit::agent::AGENT_PATH;
use cupertino_prompts::polkit::helper::Route;
use cupertino_prompts::Options;
use cupertino_server::authorization::{is_prompt, ShellRole};
use cupertino_server::shell_connection::shell_process_connection;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use rig::Rig;

/// The id of the session the program is in, as `pam_systemd` would set it, and its object at logind's.
const SESSION_ID: &str = "7";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/_37";

const PASSWORD: &str = "correct horse";
const COOKIE: &str = "cookie-41";
const AGENT_INTERFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
/// The keyring caller's callback object.
const CALLBACK: &str = "/org/gnome/keyring/Prompt/p0";

/// How long a stand-in waits for the program to say something.
const ASKED: Duration = Duration::from_secs(60);
/// How many pixels of the capture may differ while an alert is up: none of the alert or its scrim, but for a step of
/// the clock's rounding.
const CAPTURE_CHANGES: usize = 16;

/// A bus of the test's own that starts no service, standing for both the session and the system bus.
struct Bus(Child, String);

const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

impl Bus {
    fn start(name: &str) -> Bus {
        let config =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("prompts-{name}-bus.conf"));
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon gives its address");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }

    fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.1.as_str())
            .and_then(|b| b.build())
            .expect("a connection to the test's bus")
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("prompts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// A home, settings, data, state and runtime directories of the test's own: nothing of the person's is read.
fn private_home(name: &str) {
    let home = private_dir(&format!("{name}-home"));
    for (variable, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        let dir = home.join(dir);
        std::fs::create_dir_all(&dir).expect("a folder of the home");
        std::env::set_var(variable, dir);
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_RUNTIME_DIR", private_dir(&format!("{name}-runtime")));
    std::env::set_var("XDG_SESSION_ID", SESSION_ID);
}

/// Run the program in this process on a connection holding the roles the server gives it, with polkit's helper
/// reached at `helper`.
fn start(rig: &mut Rig, helper: &Path) -> std::thread::JoinHandle<Result<(), String>> {
    let roles = [ShellRole::Session, ShellRole::Prompt, ShellRole::Consent];
    let stream =
        shell_process_connection(&rig.state.display_handle, &roles).expect("a shell connection");
    std::env::set_var("WAYLAND_SOCKET", stream.into_raw_fd().to_string());
    let options = Options {
        helper: Some(vec![Route::Socket(helper.to_path_buf())]),
        keyring_offer: false,
    };
    std::thread::spawn(move || cupertino_prompts::run(options))
}

/// How many of the output's pixels differ from `before` by more than a step of rounding.
fn changed(rig: &mut Rig, before: &image::RgbaImage) -> usize {
    let now = rig.image();
    now.pixels()
        .zip(before.pixels())
        .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > 2))
        .count()
}

/// Wait until an alert is up and has the keyboard, then check that desktop control and captures can't see it.
fn alert_up(rig: &mut Rig, bare: &image::RgbaImage) {
    rig.until("the alert is up", |rig| rig.state.prompt_shown());
    rig.until("the alert has the keyboard", |rig| {
        let focus = rig
            .state
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus());
        focus.is_some_and(|surface| is_prompt(&surface))
    });
    // every surface of the program counts as a prompt: its client holds the role
    assert!(
        rig.state.prompt_shown(),
        "desktop control is suspended and observes nothing"
    );
    let shown = changed(rig, bare);
    assert!(
        shown <= CAPTURE_CHANGES,
        "the capture leaves the alert and its scrims out, {shown} pixels changed"
    );
}

/// logind's manager and the program's session, as far as the program asks.
struct Logind;

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Logind {
    fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if id == SESSION_ID {
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        } else {
            Err(zbus::fdo::Error::Failed(format!("no session {id}")))
        }
    }

    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, _pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
    }
}

struct LoginSession;

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    #[zbus(property)]
    fn id(&self) -> String {
        SESSION_ID.to_owned()
    }

    #[zbus(property)]
    fn active(&self) -> bool {
        true
    }
}

/// An agent's registration as polkitd heard it: who registered, for which session, at which path.
#[derive(Debug, PartialEq)]
struct Registration {
    sender: String,
    session: String,
    path: String,
}

/// polkitd's authority, as far as an agent registers with it.
struct Authority {
    registered: mpsc::Sender<Registration>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Authority {
    fn register_authentication_agent_with_options(
        &self,
        subject: (String, HashMap<String, OwnedValue>),
        _locale: String,
        object_path: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
    ) {
        assert_eq!(
            subject.0, "unix-session",
            "the agent registers for its session"
        );
        let session = subject
            .1
            .get("session-id")
            .and_then(|v| <&str>::try_from(&**v).ok())
            .unwrap_or_default()
            .to_owned();
        let _ = self.registered.send(Registration {
            sender: header.sender().map(|s| s.to_string()).unwrap_or_default(),
            session,
            path: object_path,
        });
    }
}

/// The stand-ins for logind and polkitd, each on a connection of its own, as on the system bus.
fn system_stand_ins(
    bus: &Bus,
) -> (
    zbus::blocking::Connection,
    zbus::blocking::Connection,
    mpsc::Receiver<Registration>,
) {
    let logind = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.freedesktop.login1"))
        .and_then(|b| b.serve_at("/org/freedesktop/login1", Logind))
        .and_then(|b| b.serve_at(SESSION_PATH, LoginSession))
        .and_then(|b| b.build())
        .expect("logind's stand-in");
    let (registered, registrations) = mpsc::channel();
    let polkitd = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.freedesktop.PolicyKit1"))
        .and_then(|b| {
            b.serve_at(
                "/org/freedesktop/PolicyKit1/Authority",
                Authority { registered },
            )
        })
        .and_then(|b| b.build())
        .expect("polkitd's stand-in");
    (logind, polkitd, registrations)
}

/// What polkit's helper heard in one conversation: the user, the cookie, and the answer or `None` when the agent hung
/// up before it answered.
#[derive(Debug, PartialEq)]
struct Conversation {
    user: String,
    cookie: String,
    answer: Option<String>,
}

/// polkit's helper on the socket at `path`, for `conversations` conversations: each asks for a password and answers
/// SUCCESS for [`PASSWORD`], FAILURE for anything else.
fn helper(path: &Path, conversations: usize) -> mpsc::Receiver<Conversation> {
    let listener = UnixListener::bind(path).expect("the helper's socket");
    let (heard, conversed) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..conversations {
            let (stream, _) = listener.accept().expect("the agent connects");
            stream.set_read_timeout(Some(ASKED)).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = || {
                let mut line = String::new();
                let read = reader.read_line(&mut line).unwrap_or(0);
                (read > 0).then(|| line.trim_end_matches('\n').to_owned())
            };
            let user = line().expect("the user");
            let cookie = line().expect("the cookie");
            writer
                .write_all(b"PAM_PROMPT_ECHO_OFF Password: \n")
                .unwrap();
            let answer = line();
            if let Some(answer) = &answer {
                let verdict: &[u8] = if answer == PASSWORD {
                    b"SUCCESS\n"
                } else {
                    b"FAILURE\n"
                };
                writer.write_all(verdict).unwrap();
            }
            let _ = heard.send(Conversation {
                user,
                cookie,
                answer,
            });
        }
    });
    conversed
}

/// Ask the agent at `agent` to authenticate the test's user for `cookie`, from `connection`; the call's outcome comes
/// on the channel once the agent answers.
fn begin_authentication(
    connection: &zbus::blocking::Connection,
    agent: &str,
    cookie: &'static str,
) -> mpsc::Receiver<zbus::Result<()>> {
    let connection = connection.clone();
    let agent = agent.to_owned();
    let (done, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let details = HashMap::from([("polkit.subject-pid", "1")]);
        let identities = vec![(
            "unix-user",
            HashMap::from([("uid", Value::from(rustix::process::getuid().as_raw()))]),
        )];
        let result = connection
            .call_method(
                Some(agent.as_str()),
                AGENT_PATH,
                Some(AGENT_INTERFACE),
                "BeginAuthentication",
                &(
                    "org.example.mount",
                    "Authentication is required to mount the disk.",
                    "",
                    details,
                    cookie,
                    identities,
                ),
            )
            .map(|_| ());
        let _ = done.send(result);
    });
    outcome
}

/// The D-Bus error name of a failed call.
fn error_name(result: &zbus::Result<()>) -> Option<String> {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => Some(name.to_string()),
        _ => None,
    }
}

#[test]
fn polkitds_request_is_answered_by_the_helper_with_the_password_typed() {
    private_home("polkit");
    let socket = std::env::temp_dir().join(format!("prompts-helper-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let conversations = helper(&socket, 2);
    let bus = Bus::start("polkit");
    let (_logind, polkitd, registrations) = system_stand_ins(&bus);
    let mut rig = Rig::new("prompts-polkit", &[], &[]);
    let bare = rig.image();
    let program = start(&mut rig, &socket);

    let mut registration = None;
    rig.until("the agent registers with polkitd", |_| {
        registration = registrations.try_recv().ok();
        registration.is_some()
    });
    let registration = registration.unwrap();
    assert_eq!(
        (registration.session.as_str(), registration.path.as_str()),
        (SESSION_ID, AGENT_PATH),
        "for the session logind names, at the agent's path"
    );

    // a caller that is not polkitd may not raise the prompt
    let stranger = bus.connect();
    let refused = begin_authentication(&stranger, &registration.sender, "forged");
    let mut outcome = None;
    rig.until("the stranger is answered", |_| {
        outcome = refused.try_recv().ok();
        outcome.is_some()
    });
    assert!(
        error_name(&outcome.unwrap()).is_some(),
        "the stranger is refused"
    );
    assert!(!rig.state.prompt_shown(), "and nothing is shown for it");

    let answered = begin_authentication(&polkitd, &registration.sender, COOKIE);
    alert_up(&mut rig, &bare);
    rig.state.scripted_type(PASSWORD);
    rig.state.scripted_key("Return").unwrap();
    let mut outcome = None;
    rig.until("polkitd is answered", |_| {
        outcome = answered.try_recv().ok();
        outcome.is_some()
    });
    outcome.unwrap().expect("the authentication succeeded");
    let heard = conversations
        .recv_timeout(ASKED)
        .expect("the helper was asked");
    assert_eq!(
        (heard.cookie.as_str(), heard.answer.as_deref()),
        (COOKIE, Some(PASSWORD))
    );
    assert!(!heard.user.is_empty(), "the helper is named the user");
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // polkitd withdraws a request
    let withdrawn = begin_authentication(&polkitd, &registration.sender, "cookie-42");
    alert_up(&mut rig, &bare);
    polkitd
        .call_method(
            Some(registration.sender.as_str()),
            AGENT_PATH,
            Some(AGENT_INTERFACE),
            "CancelAuthentication",
            &("cookie-42",),
        )
        .expect("the agent takes polkitd's cancel");
    let mut outcome = None;
    rig.until("the withdrawn request is answered", |_| {
        outcome = withdrawn.try_recv().ok();
        outcome.is_some()
    });
    assert_eq!(
        error_name(&outcome.unwrap()).as_deref(),
        Some("org.freedesktop.PolicyKit1.Error.Cancelled")
    );
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());
    let heard = conversations
        .recv_timeout(ASKED)
        .expect("the helper was started again");
    assert_eq!(
        (heard.cookie.as_str(), heard.answer),
        ("cookie-42", None),
        "and hung up on"
    );
    assert!(!program.is_finished(), "the program runs on");
    let _ = std::fs::remove_file(&socket);
}

/// What a keyring caller's callback object heard.
#[derive(Debug)]
enum Heard {
    Ready { reply: String, exchange: String },
    Done,
}

struct Callback {
    heard: mpsc::Sender<Heard>,
}

#[zbus::interface(name = "org.gnome.keyring.internal.Prompter.Callback")]
impl Callback {
    fn prompt_ready(
        &self,
        reply: String,
        _properties: HashMap<String, OwnedValue>,
        exchange: String,
    ) {
        let _ = self.heard.send(Heard::Ready { reply, exchange });
    }

    fn prompt_done(&self) {
        let _ = self.heard.send(Heard::Done);
    }
}

/// Call the prompter's `method` from `caller`.
fn prompter(
    caller: &zbus::blocking::Connection,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> zbus::Result<()> {
    caller
        .call_method(Some(SYSTEM_NAME), PATH, Some(INTERFACE), method, body)
        .map(|_| ())
}

/// The next thing `heard` hears, while the server turns.
fn next(rig: &mut Rig, heard: &mpsc::Receiver<Heard>, what: &str) -> Heard {
    let mut got = None;
    rig.until(what, |_| {
        got = heard.try_recv().ok();
        got.is_some()
    });
    got.unwrap()
}

#[test]
fn the_keyring_prompter_answers_a_callers_password_question_through_the_exchange() {
    private_home("keyring");
    let socket = std::env::temp_dir().join(format!("prompts-keyring-{}.sock", std::process::id()));
    let bus = Bus::start("keyring");
    let (_logind, _polkitd, _registrations) = system_stand_ins(&bus);
    let mut rig = Rig::new("prompts-keyring", &[], &[]);
    let bare = rig.image();
    let program = start(&mut rig, &socket);

    let (heard, hears) = mpsc::channel();
    let caller = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.serve_at(CALLBACK, Callback { heard }))
        .and_then(|b| b.build())
        .expect("the keyring caller");
    let callback = ObjectPath::try_from(CALLBACK).unwrap();
    let daemon = zbus::blocking::fdo::DBusProxy::new(&caller).expect("the bus daemon");
    rig.until("the prompter takes its name", |_| {
        daemon
            .name_has_owner(SYSTEM_NAME.try_into().unwrap())
            .unwrap_or(false)
    });
    prompter(&caller, "BeginPrompting", &(&callback,)).expect("the caller begins");
    let Heard::Ready { reply, exchange } = next(&mut rig, &hears, "the prompter is ready") else {
        panic!("the prompter is ready first");
    };
    assert_eq!(reply, "");
    let mut side = Exchange::new().unwrap();
    assert_eq!(side.receive(&exchange).unwrap(), None);

    // another caller waits its turn and may not ask
    let other = bus.connect();
    let refused = prompter(
        &other,
        "PerformPrompt",
        &(
            &callback,
            "password",
            HashMap::<&str, Value<'_>>::new(),
            side.begin(),
        ),
    );
    assert_eq!(error_name(&refused).as_deref(), Some(ERROR_IN_PROGRESS));

    let properties = HashMap::from([
        ("message", Value::from("Unlock the keyring")),
        (
            "description",
            Value::from("An application wants access to the keyring \u{201c}Default\u{201d}."),
        ),
        ("continue-label", Value::from("_Unlock")),
    ]);
    prompter(
        &caller,
        "PerformPrompt",
        &(&callback, "password", properties, side.begin()),
    )
    .expect("the caller asks for the password");
    alert_up(&mut rig, &bare);
    rig.state.scripted_type(PASSWORD);
    rig.state.scripted_key("Return").unwrap();
    let Heard::Ready { reply, exchange } = next(&mut rig, &hears, "the caller is answered") else {
        panic!("an answer, not the end");
    };
    assert_eq!(reply, "yes");
    assert!(
        !exchange.contains(PASSWORD),
        "the password is not in the clear"
    );
    let secret = side.receive(&exchange).unwrap().expect("the password");
    assert_eq!(secret.as_slice(), PASSWORD.as_bytes());

    prompter(&caller, "StopPrompting", &(&callback,)).expect("the caller stops");
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());
    assert!(!program.is_finished(), "the program runs on");
}

/// A stand-in of `org.cupertino.Session1` with no shell hosts, which names one connection the portal.
struct Session {
    portal: String,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session {
    fn shell_hosts(&self) -> Vec<(u32, u64)> {
        Vec::new()
    }

    fn component_of(&self, _unique_name: String) -> String {
        String::new()
    }

    fn program_of(&self, unique_name: String) -> String {
        if unique_name == self.portal {
            "portal".to_owned()
        } else {
            String::new()
        }
    }
}

/// The shortcuts the portal words for a call app: two that get their keys and one that does not.
fn shortcut_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Push to talk", "\u{2303}\u{2325}T", ""),
        ("Mute the microphone", "\u{2303}\u{2318}M", ""),
        (
            "Raise a hand",
            "H",
            "not available: it needs \u{2303}, \u{2325} or \u{2318}",
        ),
    ]
}

/// Ask the prompts `method` with `body` from `caller`; the response comes on the channel once the person answered.
fn ask<B>(
    caller: &zbus::blocking::Connection,
    method: &'static str,
    body: B,
) -> mpsc::Receiver<zbus::Result<u32>>
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let caller = caller.clone();
    let (done, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let result = caller
            .call_method(
                Some(PROMPTS_NAME),
                PROMPTS_PATH,
                Some(PROMPTS_INTERFACE),
                method,
                &body,
            )
            .and_then(|reply| reply.body().deserialize::<u32>());
        let _ = done.send(result);
    });
    outcome
}

fn ask_screenshot(
    caller: &zbus::blocking::Connection,
    handle: &str,
) -> mpsc::Receiver<zbus::Result<u32>> {
    let handle = OwnedObjectPath::try_from(handle).unwrap();
    ask(
        caller,
        "AskScreenshot",
        (handle, "org.example.Notes", "Notes", "", ""),
    )
}

fn ask_shortcuts(
    caller: &zbus::blocking::Connection,
    handle: &str,
) -> mpsc::Receiver<zbus::Result<u32>> {
    let handle = OwnedObjectPath::try_from(handle).unwrap();
    ask(
        caller,
        "AskShortcuts",
        (
            handle,
            "org.example.Talk",
            "Talk",
            "",
            "",
            shortcut_rows(),
            0u32,
        ),
    )
}

/// The response `asked` gets, while the server turns.
fn response(
    rig: &mut Rig,
    asked: &mpsc::Receiver<zbus::Result<u32>>,
    what: &str,
) -> zbus::Result<u32> {
    let mut got = None;
    rig.until(what, |_| {
        got = asked.try_recv().ok();
        got.is_some()
    });
    got.unwrap()
}

/// Turn the server until the alert that came up at `up` is armed, with a margin for its first frame.
fn armed(rig: &mut Rig, up: Instant) {
    let ready = up + ARMING + Duration::from_millis(400);
    rig.until("the arming delay passes", |_| Instant::now() >= ready);
}

#[test]
fn the_portals_consent_questions_are_answered_by_the_person() {
    private_home("consent");
    let socket = std::env::temp_dir().join(format!("prompts-consent-{}.sock", std::process::id()));
    let bus = Bus::start("consent");
    let (_logind, _polkitd, _registrations) = system_stand_ins(&bus);
    let portal = bus.connect();
    let portal_name = portal.unique_name().expect("a unique name").to_string();
    let _session = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.cupertino.Session1"))
        .and_then(|b| {
            b.serve_at(
                "/org/cupertino/Session1",
                Session {
                    portal: portal_name,
                },
            )
        })
        .and_then(|b| b.build())
        .expect("Session1's stand-in");
    let mut rig = Rig::new("prompts-consent", &[], &[]);
    let bare = rig.image();
    let program = start(&mut rig, &socket);
    let daemon = zbus::blocking::fdo::DBusProxy::new(&portal).expect("the bus daemon");
    rig.until("the prompts take their name", |_| {
        daemon
            .name_has_owner(PROMPTS_NAME.try_into().unwrap())
            .unwrap_or(false)
    });

    // only the portal may ask
    let stranger = bus.connect();
    let refused = ask_screenshot(&stranger, "/request/stranger");
    let refused = response(&mut rig, &refused, "the stranger is answered");
    assert_eq!(
        error_name(&refused.map(|_| ())).as_deref(),
        Some("org.freedesktop.DBus.Error.AccessDenied")
    );
    assert!(!rig.state.prompt_shown(), "and nothing is shown for it");

    // a screenshot: a key before the arming delay grants nothing, Take Screenshot after it does
    let asked = ask_screenshot(&portal, "/request/1");
    alert_up(&mut rig, &bare);
    let up = Instant::now();
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    armed(&mut rig, up);
    assert!(
        asked.try_recv().is_err(),
        "no key answered before the delay"
    );
    assert!(rig.state.prompt_shown(), "the alert stays");
    // the focus starts on Cancel; Take Screenshot is the other button
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    let granted = response(&mut rig, &asked, "the screenshot is granted");
    assert_eq!(granted.expect("an answer"), SCREENSHOT_GRANTED);
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // Escape cancels
    let asked = ask_screenshot(&portal, "/request/2");
    alert_up(&mut rig, &bare);
    rig.state.scripted_key("Escape").unwrap();
    let cancelled = response(&mut rig, &asked, "the screenshot is cancelled");
    assert_eq!(cancelled.expect("an answer"), SCREENSHOT_CANCELLED);
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // shortcuts: Allow, past Don't Allow where the focus starts
    let asked = ask_shortcuts(&portal, "/request/3");
    alert_up(&mut rig, &bare);
    armed(&mut rig, Instant::now());
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    let allowed = response(&mut rig, &asked, "the shortcuts are allowed");
    assert_eq!(allowed.expect("an answer"), SHORTCUTS_ALLOWED);
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // shortcuts: Don't Allow, where the focus starts
    let asked = ask_shortcuts(&portal, "/request/4");
    alert_up(&mut rig, &bare);
    armed(&mut rig, Instant::now());
    rig.state.scripted_key("Return").unwrap();
    let denied = response(&mut rig, &asked, "the shortcuts are refused");
    assert_eq!(denied.expect("an answer"), SHORTCUTS_DENIED);
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // the portal's Close takes a question down without an answer of the person's
    let asked = ask_screenshot(&portal, "/request/5");
    alert_up(&mut rig, &bare);
    let handle = OwnedObjectPath::try_from("/request/5").unwrap();
    portal
        .call_method(
            Some(PROMPTS_NAME),
            PROMPTS_PATH,
            Some(PROMPTS_INTERFACE),
            "Close",
            &(handle,),
        )
        .expect("the prompts take the portal's Close");
    let closed = response(&mut rig, &asked, "the closed question is answered");
    assert_eq!(closed.expect("an answer"), SCREENSHOT_CANCELLED);
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());
    assert!(!program.is_finished(), "the program runs on");
}
