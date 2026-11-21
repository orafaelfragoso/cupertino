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
//! - The server's consent authority asks the program whether a desktop controller's run may capture: the alert is
//!   the prompts', on the surface the person answers the authority on; an active run is not paused and its input
//!   reaches neither the alert nor its scrim; a scripted Allow starts nothing, Don't Allow and Escape refuse the run.
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

use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
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
use cupertino_protocols::client::control::cupertino_control_session_v1::{
    self, Capability, CupertinoControlSessionV1, Phase, Refusal,
};
use cupertino_protocols::client::control::cupertino_control_v1::CupertinoControlV1;
use cupertino_server::authorization::{is_prompt, ShellRole};
use cupertino_server::control::protected_surface;
use cupertino_server::shell_connection::shell_process_connection;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::WEnum;
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

/// What a desktop controller's connection heard.
#[derive(Default)]
struct Controlled {
    globals: Vec<(u32, String, u32)>,
    epoch: Option<(u32, u32)>,
    phase: Option<WEnum<Phase>>,
    /// Every phase the server reported, in order.
    phases: Vec<WEnum<Phase>>,
    revision: Option<u32>,
    completed: Vec<u32>,
    refused: Vec<(u32, WEnum<Refusal>)>,
}

impl wayland_client::Dispatch<wl_registry::WlRegistry, ()> for Controlled {
    fn event(
        heard: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            heard.globals.push((name, interface, version));
        }
    }
}

wayland_client::delegate_noop!(Controlled: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Controlled: ignore CupertinoControlV1);

impl wayland_client::Dispatch<CupertinoControlSessionV1, ()> for Controlled {
    fn event(
        heard: &mut Self,
        _: &CupertinoControlSessionV1,
        event: cupertino_control_session_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        use cupertino_control_session_v1::Event;
        match event {
            Event::Created {
                epoch_hi, epoch_lo, ..
            } => heard.epoch = Some((epoch_hi, epoch_lo)),
            Event::Status { phase, .. } => {
                heard.phase = Some(phase);
                heard.phases.push(phase);
            }
            Event::Geometry { revision, .. } => heard.revision = Some(revision),
            Event::Completed { serial } => heard.completed.push(serial),
            Event::Refused { serial, reason } => heard.refused.push((serial, reason)),
            _ => {}
        }
    }
}

/// The app desktop control names a run of this process by: the executable of its parent, as a run under no window.
fn this_app() -> App {
    let parent = std::os::unix::process::parent_id();
    let exe = std::fs::read_link(format!("/proc/{parent}/exe")).unwrap();
    let exe = exe.to_str().unwrap();
    App::Executable(exe.strip_suffix(" (deleted)").unwrap_or(exe).into())
}

/// This process's start, field 22 of `/proc/self/stat` counted from the last `)`, which names it to desktop control.
fn own_start() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let after_name = &stat[stat.rfind(')').unwrap() + 1..];
    after_name
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}

/// Read what the server sent the controller, without waiting, and dispatch it.
fn pump(
    conn: &wayland_client::Connection,
    queue: &mut wayland_client::EventQueue<Controlled>,
    heard: &mut Controlled,
) {
    let _ = conn.flush();
    if let Some(guard) = conn.prepare_read() {
        let _ = guard.read();
    }
    queue.dispatch_pending(heard).unwrap();
    let _ = conn.flush();
}

/// A desktop controller's run, on a connection of its own with the `desktop_control` role, named as this process.
struct Controller {
    conn: wayland_client::Connection,
    queue: wayland_client::EventQueue<Controlled>,
    heard: Controlled,
    session: CupertinoControlSessionV1,
    epoch: (u32, u32),
    serial: u32,
}

impl Controller {
    /// A run asking for `capabilities` on the output, its start sent as request 1.
    fn request(rig: &mut Rig, capabilities: Capability) -> Controller {
        let stream =
            shell_process_connection(&rig.state.display_handle, &[ShellRole::DesktopControl])
                .expect("a controller's connection");
        let conn = wayland_client::Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut heard = Controlled::default();
        rig.until("the controller sees the globals", |_| {
            pump(&conn, &mut queue, &mut heard);
            heard
                .globals
                .iter()
                .any(|(_, i, _)| i == "cupertino_control_v1")
        });
        let bind = |interface: &str| {
            heard
                .globals
                .iter()
                .find(|(_, i, _)| i == interface)
                .map(|&(name, _, version)| (name, version))
                .unwrap()
        };
        let (name, _) = bind("wl_output");
        let output: wl_output::WlOutput = registry.bind(name, 4, &qh, ());
        let (name, version) = bind("cupertino_control_v1");
        let control: CupertinoControlV1 = registry.bind(name, version.min(5), &qh, ());
        let session = control.get_session(&qh, ());
        rig.until("the run is created", |_| {
            pump(&conn, &mut queue, &mut heard);
            heard.epoch.is_some()
        });
        let epoch = heard.epoch.unwrap();
        let start = own_start();
        session.identify(std::process::id(), (start >> 32) as u32, start as u32);
        session.start(1, epoch.0, epoch.1, &output, capabilities);
        Controller {
            conn,
            queue,
            heard,
            session,
            epoch,
            serial: 1,
        }
    }

    fn step(&mut self) {
        pump(&self.conn, &mut self.queue, &mut self.heard);
    }

    /// The answer to request `serial`: `Ok` once it was carried out, the refusal's reason else.
    fn answer(&mut self, rig: &mut Rig, serial: u32) -> Result<(), WEnum<Refusal>> {
        rig.until(&format!("request {serial} is answered"), |_| {
            self.step();
            self.heard.completed.contains(&serial)
                || self.heard.refused.iter().any(|(s, _)| *s == serial)
        });
        match self.heard.refused.iter().find(|(s, _)| *s == serial) {
            Some((_, reason)) => Err(*reason),
            None => Ok(()),
        }
    }

    /// The run's phase once the server has said it.
    fn phase_until(&mut self, rig: &mut Rig, phase: Phase, what: &str) {
        rig.until(what, |_| {
            self.step();
            self.heard.phase == Some(WEnum::Value(phase))
        });
    }

    /// Send the input `send` makes of a serial and the run's geometry revision, again while the geometry moves on; its
    /// answer.
    fn input(
        &mut self,
        rig: &mut Rig,
        send: impl Fn(&CupertinoControlSessionV1, u32, (u32, u32), u32),
    ) -> Result<(), WEnum<Refusal>> {
        loop {
            self.serial += 1;
            let serial = self.serial;
            let revision = self.heard.revision.expect("the run's geometry");
            send(&self.session, serial, self.epoch, revision);
            match self.answer(rig, serial) {
                Err(WEnum::Value(Refusal::StaleGeometry)) => {}
                answer => return answer,
            }
        }
    }
}

/// Turn the server for `time`: what the program sends in it is handled.
fn turn_for(rig: &mut Rig, time: Duration) {
    let until = Instant::now() + time;
    rig.until("the time passes", |_| Instant::now() >= until);
}

#[test]
fn desktop_controls_consent_alert_is_the_prompts_and_answers_the_person() {
    private_home("control");
    let socket = std::env::temp_dir().join(format!("prompts-control-{}.sock", std::process::id()));
    let bus = Bus::start("control");
    let (_logind, _polkitd, _registrations) = system_stand_ins(&bus);
    let mut rig = Rig::new("prompts-control", &[], &[]);
    let bare = rig.image();
    let program = start(&mut rig, &socket);
    let daemon = zbus::blocking::fdo::DBusProxy::new(&bus.connect()).expect("the bus daemon");
    rig.until("the prompts serve", |_| {
        daemon
            .name_has_owner(PROMPTS_NAME.try_into().unwrap())
            .unwrap_or(false)
    });

    // a run the person allowed before, active
    let mut store = DesktopControlConf::default();
    store
        .set(Service::Control, &this_app(), Decision::Allow)
        .unwrap();
    rig.state.apply_desktop_control_store(store);
    let mut active = Controller::request(
        &mut rig,
        Capability::Observe | Capability::Pointer | Capability::Keyboard,
    );
    assert_eq!(active.answer(&mut rig, 1), Ok(()), "the allowed run starts");
    active.phase_until(&mut rig, Phase::Active, "the allowed run is active");

    // a run asking to capture, which the person has not decided: the prompts ask on the consent authority's surface
    let mut asking = Controller::request(&mut rig, Capability::Observe | Capability::Capture);
    assert_eq!(asking.answer(&mut rig, 1), Ok(()), "the run asks");
    alert_up(&mut rig, &bare);
    let focus = rig
        .state
        .seat
        .get_keyboard()
        .and_then(|k| k.current_focus());
    assert!(
        focus.is_some_and(|surface| protected_surface(&surface)),
        "the alert is the surface the person answers the consent authority on"
    );
    let up = Instant::now();
    assert_eq!(
        asking.heard.phase,
        Some(WEnum::Value(Phase::AwaitingConsent))
    );

    // the alert pauses no run, and the active run's input reaches neither it nor its scrim
    active.step();
    assert_eq!(active.heard.phase, Some(WEnum::Value(Phase::Active)));
    assert!(
        !active
            .heard
            .phases
            .contains(&WEnum::Value(Phase::Suspended)),
        "the alert never paused the active run, not even before its first frame: {:?}",
        active.heard.phases
    );
    let pressed = active.input(&mut rig, |session, serial, (hi, lo), revision| {
        session.key(
            serial,
            hi,
            lo,
            revision,
            28,
            cupertino_control_session_v1::KeyState::Pressed,
        )
    });
    assert_eq!(pressed, Err(WEnum::Value(Refusal::Blocked)), "Return");
    let centre = (
        f64::from(rig::OUTPUT_SIZE.0) / 2.0,
        f64::from(rig::OUTPUT_SIZE.1) / 2.0,
    );
    let moved = active.input(&mut rig, |session, serial, (hi, lo), revision| {
        session.motion(serial, hi, lo, revision, centre.0, centre.1)
    });
    assert_eq!(moved, Err(WEnum::Value(Refusal::Blocked)), "the pointer");

    // Allow by a scripted key, once armed, allows nothing: only the person's own input does
    armed(&mut rig, up);
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    turn_for(&mut rig, ARMING);
    asking.step();
    assert_eq!(
        asking.heard.phase,
        Some(WEnum::Value(Phase::AwaitingConsent)),
        "a scripted Allow does not start the run"
    );
    assert!(rig.state.prompt_shown(), "the alert stays");

    // Don't Allow refuses the run, and the alert goes
    rig.state.scripted_key("Tab").unwrap();
    rig.state.scripted_key("Return").unwrap();
    asking.phase_until(&mut rig, Phase::Inactive, "Don't Allow refuses the run");
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    // Escape refuses this request
    let mut dismissed = Controller::request(&mut rig, Capability::Observe | Capability::Capture);
    assert_eq!(dismissed.answer(&mut rig, 1), Ok(()), "the next run asks");
    alert_up(&mut rig, &bare);
    rig.state.scripted_key("Escape").unwrap();
    dismissed.phase_until(&mut rig, Phase::Inactive, "Escape refuses the run");
    rig.until("the alert goes", |rig| !rig.state.prompt_shown());

    active.step();
    assert_eq!(active.heard.phase, Some(WEnum::Value(Phase::Active)));
    assert!(!program.is_finished(), "the program runs on");
}
