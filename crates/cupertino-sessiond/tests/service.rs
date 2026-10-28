//! The bus interface on a private bus: a private `dbus-daemon` is started for the test (skipped when the binary is not
//! installed), the service runs on one connection and the test is the client on another. The system is scripted, so the tests
//! decide what logind "says" about the test process.
#![cfg(feature = "app")]

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use cupertino_login_ipc::{PowerAction, PowerRefusal};
use cupertino_sessiond::caller::{CallerSession, SEAT};
use cupertino_sessiond::service::{self, Authorizer, Call, ReadyRefusal, Sink, SwitchRefusal};
use cupertino_test_bus::PrivateBus;
use zbus::blocking::Connection;

/// Answers with a fixed session for every process.
struct Scripted(Option<CallerSession>);

impl Authorizer for Scripted {
    fn session_of(&self, _pid: u32) -> Result<Option<CallerSession>, String> {
        Ok(self.0.clone())
    }
}

/// The test's own uid, which is the uid the bus reports for the client.
fn my_uid() -> u32 {
    // Safety: `getuid` has no preconditions.
    unsafe { libc::getuid() }
}

fn session(active: bool) -> CallerSession {
    CallerSession {
        id: "7".to_owned(),
        uid: my_uid(),
        class: "user".to_owned(),
        seat: SEAT.to_owned(),
        active,
    }
}

struct Fixture {
    _bus: PrivateBus,
    _service: Connection,
    client: Connection,
    calls: Receiver<Call>,
}

fn fixture(session: Option<CallerSession>) -> Option<Fixture> {
    let bus = PrivateBus::start()?;
    let (sender, calls) = channel();
    let sender = Mutex::new(sender);
    let sink: Sink = Arc::new(move |call| sender.lock().unwrap().send(call).is_ok());
    let service = service::serve(
        zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap(),
        Arc::new(Scripted(session)),
        sink,
    )
    .unwrap();
    let client = bus.connect();
    Some(Fixture {
        _bus: bus,
        _service: service,
        client,
        calls,
    })
}

fn call(
    client: &Connection,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> zbus::Result<zbus::Message> {
    client.call_method(
        Some(service::BUS_NAME),
        service::OBJECT_PATH,
        Some(service::BUS_NAME),
        method,
        body,
    )
}

/// The D-Bus name of the error a call failed with.
fn error_name(result: zbus::Result<zbus::Message>) -> String {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected a method error, got {other:?}"),
    }
}

const ACCESS_DENIED: &str = "org.cupertino.LoginManager1.Error.AccessDenied";

#[test]
fn a_caller_outside_every_session_is_denied_on_every_method_and_nothing_reaches_the_manager() {
    let Some(fixture) = fixture(None) else { return };
    assert_eq!(
        error_name(call(&fixture.client, "RequestPower", &("restart",))),
        ACCESS_DENIED
    );
    assert_eq!(
        error_name(call(&fixture.client, "SessionReady", &())),
        ACCESS_DENIED
    );
    assert_eq!(
        error_name(call(&fixture.client, "SwitchToLoginWindow", &())),
        ACCESS_DENIED
    );
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn a_background_session_may_announce_itself_but_not_power_off() {
    let Some(fixture) = fixture(Some(session(false))) else {
        return;
    };
    assert_eq!(
        error_name(call(&fixture.client, "RequestPower", &("shut-down",))),
        ACCESS_DENIED
    );
    assert_eq!(
        error_name(call(&fixture.client, "SwitchToLoginWindow", &())),
        ACCESS_DENIED
    );
    assert!(fixture.calls.try_recv().is_err());
    // SessionReady reaches the manager, which answers it
    std::thread::scope(|scope| {
        let ready = scope.spawn(|| call(&fixture.client, "SessionReady", &()));
        let Call::Ready {
            session,
            agent,
            reply,
        } = fixture
            .calls
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected a ready call")
        };
        assert_eq!(session.id, "7");
        assert!(
            agent.starts_with(':'),
            "the agent is the caller's unique name: {agent}"
        );
        reply.send(Ok(())).unwrap();
        ready.join().unwrap().unwrap();
    });
}

#[test]
fn a_session_of_another_user_than_the_caller_is_denied() {
    let Some(fixture) = fixture(Some(CallerSession {
        uid: my_uid() + 1,
        ..session(true)
    })) else {
        return;
    };
    assert_eq!(
        error_name(call(&fixture.client, "RequestPower", &("restart",))),
        ACCESS_DENIED
    );
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn the_active_session_reaches_the_manager_and_the_refusal_comes_back_by_name() {
    let Some(fixture) = fixture(Some(session(true))) else {
        return;
    };
    let client = &fixture.client;
    std::thread::scope(|scope| {
        for (action, word, refusal, name) in [
            (
                PowerAction::Restart,
                "restart",
                PowerRefusal::OtherUsersLoggedIn,
                "OtherUsersLoggedIn",
            ),
            (
                PowerAction::ShutDown,
                "shut-down",
                PowerRefusal::Inhibited,
                "Inhibited",
            ),
            (PowerAction::Sleep, "sleep", PowerRefusal::Failed, "Failed"),
        ] {
            let request = scope.spawn(move || call(client, "RequestPower", &(word,)));
            let Call::Power {
                session,
                action: asked,
                reply,
            } = fixture
                .calls
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            else {
                panic!("expected a power call")
            };
            assert_eq!((session.id.as_str(), asked), ("7", action));
            reply.send(Err(refusal)).unwrap();
            assert_eq!(
                error_name(request.join().unwrap()),
                format!("org.cupertino.LoginManager1.Error.{name}")
            );
        }
        // and a success is an empty answer
        let request = scope.spawn(|| call(client, "RequestPower", &("restart",)));
        let Call::Power { reply, .. } = fixture
            .calls
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected a power call")
        };
        reply.send(Ok(())).unwrap();
        request.join().unwrap().unwrap();
    });
}

#[test]
fn an_unknown_action_is_refused_before_anything_is_asked() {
    let Some(fixture) = fixture(Some(session(true))) else {
        return;
    };
    assert_eq!(
        error_name(call(&fixture.client, "RequestPower", &("hibernate",))),
        "org.cupertino.LoginManager1.Error.InvalidArgs"
    );
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn the_manager_refusing_an_unknown_session_is_an_error_of_that_name() {
    let Some(fixture) = fixture(Some(session(true))) else {
        return;
    };
    std::thread::scope(|scope| {
        let ready = scope.spawn(|| call(&fixture.client, "SessionReady", &()));
        let Call::Ready { reply, .. } = fixture
            .calls
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected a ready call")
        };
        reply.send(Err(ReadyRefusal::UnknownSession)).unwrap();
        assert_eq!(
            error_name(ready.join().unwrap()),
            "org.cupertino.LoginManager1.Error.UnknownSession"
        );
    });
}

#[test]
fn the_active_session_asks_for_the_login_window_and_the_answer_comes_back() {
    let Some(fixture) = fixture(Some(session(true))) else {
        return;
    };
    std::thread::scope(|scope| {
        let request = scope.spawn(|| call(&fixture.client, "SwitchToLoginWindow", &()));
        let Call::SwitchToLoginWindow { session, reply } = fixture
            .calls
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        else {
            panic!("expected a switch call")
        };
        assert_eq!(session.id, "7");
        reply.send(Err(SwitchRefusal::NoLoginScreen)).unwrap();
        assert_eq!(
            error_name(request.join().unwrap()),
            "org.cupertino.LoginManager1.Error.Failed"
        );
    });
}

#[test]
fn unlock_reaches_the_agent_it_is_sent_to_and_no_other_listener() {
    let Some(fixture) = fixture(Some(session(true))) else {
        return;
    };
    let agent = fixture._bus.connect();
    let bystander = fixture._bus.connect();
    let rule = || {
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(service::BUS_NAME)
            .unwrap()
            .member("Unlock")
            .unwrap()
            .build()
    };
    let mut agent_signals =
        zbus::blocking::MessageIterator::for_match_rule(rule(), &agent, None).unwrap();
    let mut bystander_signals =
        zbus::blocking::MessageIterator::for_match_rule(rule(), &bystander, None).unwrap();
    let to = agent.unique_name().unwrap().to_string();
    service::unlock(&fixture._service, &to).unwrap();
    let message = agent_signals.next().unwrap().unwrap();
    assert_eq!(message.header().member().unwrap().as_str(), "Unlock");
    // a second signal to the bystander proves the first did not reach it: the iterators deliver in order
    service::unlock(&fixture._service, bystander.unique_name().unwrap().as_str()).unwrap();
    let message = bystander_signals.next().unwrap().unwrap();
    assert_eq!(
        message.header().destination().unwrap().to_string(),
        bystander.unique_name().unwrap().to_string()
    );
}
