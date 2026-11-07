//! An administrator's action from polkitd's request to its answer, without a display: a fake polkitd on a private bus
//! asks the agent, the prompt's model and its [`Conversation`] run the fixture helper (`tests/fixtures`, which accepts
//! the password "right"), and the test types as the person would. The prompt names the application and the reason;
//! a valid password answers polkitd; wrong ones are retried and then end the authentication; Escape and polkitd's
//! own cancel stop it with polkitd told it was cancelled.

use std::collections::HashMap;
use std::process::{Child, Command};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use cupertino_polkit_agent::agent::{serve, Event, User, AGENT_PATH, POLKIT};
use cupertino_polkit_agent::conversation::Conversation;
use cupertino_polkit_agent::helper::{HelperError, Line, Route};
use cupertino_polkit_agent::model::{Agent, Key, Phase, TRY_AGAIN};
use cupertino_polkit_agent::users;
use cupertino_test_bus::PrivateBus;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

const AGENT_INTERFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
const CANCELLED: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
const WAIT: Duration = Duration::from_secs(5);
const ACTION: &str = "org.example.disk.mount";
const REASON: &str = "Authentication is required to mount the disk";

/// A polkitd that takes the agent's registration.
struct Authority(Mutex<Sender<()>>);

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Authority {
    fn register_authentication_agent_with_options(
        &self,
        _subject: (String, HashMap<String, OwnedValue>),
        _locale: String,
        _object_path: String,
        _options: HashMap<String, OwnedValue>,
    ) {
        let _ = self.0.lock().unwrap().send(());
    }
}

/// What reaches the prompt's host: polkitd's requests and the helpers' lines.
enum Message {
    Polkit(Event),
    Line(u64, Result<Line, HelperError>),
}

struct Flow {
    _bus: PrivateBus,
    polkitd: Connection,
    agent: String,
    _agent: Connection,
    messages: Receiver<Message>,
    conversation: Conversation,
    /// The application the person "used": polkitd's subject.
    application: Child,
}

impl Drop for Flow {
    fn drop(&mut self) {
        let _ = self.application.kill();
        let _ = self.application.wait();
    }
}

fn me() -> User {
    // Safety: `getuid` has no preconditions.
    User(unsafe { libc::getuid() })
}

fn start() -> Option<Flow> {
    let bus = PrivateBus::start()?;
    let (registered, registration) = channel();
    let polkitd = bus
        .blocking_builder()
        .name(POLKIT)
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            Authority(Mutex::new(registered)),
        )
        .unwrap()
        .build()
        .unwrap();
    let agent = bus.connect();
    let (sender, messages) = channel();
    let to_host = Mutex::new(sender.clone());
    serve(&agent, "c2", "C", move |event| {
        let _ = to_host.lock().unwrap().send(Message::Polkit(event));
    })
    .unwrap();
    registration
        .recv_timeout(WAIT)
        .expect("the agent registered");
    let helper = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/polkit-agent-helper-1"
    );
    let lines = Mutex::new(sender);
    let conversation = Conversation::new(
        Agent::new(me()),
        vec![Route::Binary(helper.into())],
        move |run, line| {
            let _ = lines.lock().unwrap().send(Message::Line(run, line));
        },
    );
    let application = Command::new("sleep").arg("60").spawn().unwrap();
    Some(Flow {
        agent: agent.unique_name().unwrap().to_string(),
        _bus: bus,
        polkitd,
        _agent: agent,
        messages,
        conversation,
        application,
    })
}

impl Flow {
    /// polkitd's `BeginAuthentication` on its own thread, as polkitd waits for the answer; the subject is the
    /// application and the caller this test, as for a daemon that asks on an application's behalf.
    fn begin(&self, cookie: &str) -> JoinHandle<Result<(), String>> {
        let (polkitd, agent, cookie) =
            (self.polkitd.clone(), self.agent.clone(), cookie.to_owned());
        let details = HashMap::from([
            (
                "polkit.subject-pid".to_owned(),
                self.application.id().to_string(),
            ),
            (
                "polkit.caller-pid".to_owned(),
                std::process::id().to_string(),
            ),
        ]);
        let me = me().0;
        std::thread::spawn(move || {
            let identities = vec![("unix-user", HashMap::from([("uid", Value::from(me))]))];
            polkitd
                .call_method(
                    Some(agent.as_str()),
                    AGENT_PATH,
                    Some(AGENT_INTERFACE),
                    "BeginAuthentication",
                    &(
                        ACTION,
                        REASON,
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

    /// Hands what arrives to the conversation, as the prompt's loop does, until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&Conversation) -> bool) {
        while !done(&self.conversation) {
            match self.messages.recv_timeout(WAIT) {
                Ok(Message::Polkit(Event::Begin(request, reply))) => {
                    self.conversation.begin(request, reply)
                }
                Ok(Message::Polkit(Event::Cancel(cookie))) => self.conversation.cancel(&cookie),
                Ok(Message::Line(run, line)) => self.conversation.helper_line(run, line),
                Err(_) => panic!("{what} within {WAIT:?}"),
            }
        }
    }

    fn until_asked(&mut self) {
        self.until("the helper asks for the password", |c| {
            c.agent().prompt().is_some_and(|p| p.phase == Phase::Asking)
        });
    }

    fn type_password(&mut self, text: &str) {
        for c in text.chars() {
            let effects = self.conversation.agent_mut().key(Key::Char(c));
            self.conversation.apply(effects);
        }
        let effects = self.conversation.agent_mut().key(Key::Return);
        self.conversation.apply(effects);
    }
}

#[test]
fn a_valid_password_answers_polkitd_and_the_prompt_names_the_application_and_the_reason() {
    let Some(mut flow) = start() else { return };
    let call = flow.begin("ok");
    flow.until_asked();
    let prompt = flow.conversation.agent().prompt().unwrap();
    assert_eq!(prompt.request.message, REASON);
    assert_eq!(prompt.user, me());
    let login = users::account(me()).unwrap().login;
    assert_eq!(
        prompt.info.as_deref(),
        Some(format!("user={login} args=1 cookie=ok home=none").as_str()),
        "the helper runs for the session's user with polkitd's cookie"
    );
    let exe = |pid: u32| std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(
        prompt.request.detail_rows(),
        [
            (
                "Application",
                exe(flow.application.id()).to_string_lossy().into_owned()
            ),
            (
                "Program",
                exe(std::process::id()).to_string_lossy().into_owned()
            ),
            ("Action", ACTION.to_owned()),
        ]
    );

    flow.type_password("right");
    assert_eq!(
        flow.conversation.agent().prompt().unwrap().phase,
        Phase::Checking
    );
    flow.until("the prompt closes", |c| c.agent().prompt().is_none());
    assert_eq!(call.join().unwrap(), Ok(()));
}

#[test]
fn a_wrong_password_is_retried_and_the_third_ends_the_authentication() {
    let Some(mut flow) = start() else { return };
    let call = flow.begin("denied");
    for attempt in 1..=3 {
        flow.until_asked();
        flow.type_password("wrong");
        if attempt < 3 {
            flow.until("a second helper asks again", |c| {
                c.agent().prompt().is_some_and(|p| {
                    p.phase == Phase::Asking && p.error.as_deref() == Some(TRY_AGAIN)
                })
            });
            assert!(
                !call.is_finished(),
                "polkitd was answered before the last attempt"
            );
        }
    }
    flow.until("the prompt closes", |c| c.agent().prompt().is_none());
    // the helper told polkitd each failure itself; the agent only says it is done, and polkitd refuses the action
    assert_eq!(call.join().unwrap(), Ok(()));
}

#[test]
fn escape_or_polkitds_cancel_ends_the_authentication_as_cancelled() {
    let Some(mut flow) = start() else { return };
    let call = flow.begin("escape");
    flow.until_asked();
    let effects = flow.conversation.agent_mut().key(Key::Escape);
    flow.conversation.apply(effects);
    assert!(flow.conversation.agent().prompt().is_none());
    assert_eq!(call.join().unwrap(), Err(CANCELLED.to_owned()));

    let call = flow.begin("withdrawn");
    flow.until_asked();
    flow.polkitd
        .call_method(
            Some(flow.agent.as_str()),
            AGENT_PATH,
            Some(AGENT_INTERFACE),
            "CancelAuthentication",
            &("withdrawn",),
        )
        .unwrap();
    flow.until("the prompt closes", |c| c.agent().prompt().is_none());
    assert_eq!(call.join().unwrap(), Err(CANCELLED.to_owned()));
}

#[test]
fn a_process_that_is_gone_or_a_forged_id_names_no_application() {
    let Some(mut flow) = start() else { return };
    let mut gone = Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    let call = {
        let (polkitd, agent) = (flow.polkitd.clone(), flow.agent.clone());
        let me = me().0;
        std::thread::spawn(move || {
            let details = HashMap::from([
                ("polkit.subject-pid", pid.to_string()),
                ("polkit.caller-pid", "self".to_owned()),
            ]);
            let identities = vec![("unix-user", HashMap::from([("uid", Value::from(me))]))];
            polkitd
                .call_method(
                    Some(agent.as_str()),
                    AGENT_PATH,
                    Some(AGENT_INTERFACE),
                    "BeginAuthentication",
                    &(ACTION, REASON, "", details, "gone", identities),
                )
                .map(|_| ())
        })
    };
    flow.until_asked();
    let prompt = flow.conversation.agent().prompt().unwrap();
    assert_eq!(
        prompt.request.detail_rows(),
        [("Action", ACTION.to_owned())]
    );
    flow.type_password("right");
    flow.until("the prompt closes", |c| c.agent().prompt().is_none());
    call.join().unwrap().unwrap();
}
