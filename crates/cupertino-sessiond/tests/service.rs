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

/// Answers with a fixed session for every process, and whether its user is an administrator.
struct Scripted(Option<CallerSession>, bool);

impl Authorizer for Scripted {
    fn session_of(&self, _pid: u32) -> Result<Option<CallerSession>, String> {
        Ok(self.0.clone())
    }

    fn is_administrator(&self, _uid: u32) -> Result<bool, String> {
        Ok(self.1)
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
        remote: false,
        state: if active { "active" } else { "online" }.to_owned(),
    }
}

struct Fixture {
    /// The machine's files (`/etc/cupertino` of the test).
    machine: tempfile::TempDir,
    _bus: PrivateBus,
    _service: Connection,
    client: Connection,
    calls: Receiver<Call>,
}

fn fixture(session: Option<CallerSession>) -> Option<Fixture> {
    fixture_as(session, false, false)
}

/// A fixture whose caller's user is an administrator when `administrator`, and is the settings application when `settings`
/// (the test binary stands for it).
fn fixture_as(
    session: Option<CallerSession>,
    administrator: bool,
    settings: bool,
) -> Option<Fixture> {
    fixture_on(PrivateBus::start()?, session, administrator, settings)
}

fn fixture_on(
    bus: PrivateBus,
    session: Option<CallerSession>,
    administrator: bool,
    settings: bool,
) -> Option<Fixture> {
    let machine = tempfile::tempdir().unwrap();
    let test_program = std::env::current_exe().unwrap();
    let files = service::MachineFiles {
        dir: machine.path().join("cupertino"),
        settings_program: Arc::new(move |exe: &std::path::Path| settings && exe == test_program),
    };
    let (sender, calls) = channel();
    let sender = Mutex::new(sender);
    let sink: Sink = Arc::new(move |call| sender.lock().unwrap().send(call).is_ok());
    let service = service::serve(
        zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap(),
        Arc::new(Scripted(session, administrator)),
        sink,
        files,
    )
    .unwrap();
    let client = bus.connect();
    Some(Fixture {
        machine,
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

const NOT_ADMINISTRATOR: &str = "org.cupertino.LoginManager1.Error.NotAdministrator";
const NOT_AT_CONSOLE: &str = "org.cupertino.LoginManager1.Error.NotAtConsole";
const TOO_MANY_WRITES: &str = "org.cupertino.LoginManager1.Error.TooManyWrites";
const INVALID_ARGS: &str = "org.cupertino.LoginManager1.Error.InvalidArgs";

fn reduce_motion(value: &str) -> (std::collections::HashMap<String, String>,) {
    ([("reduce_motion".to_owned(), value.to_owned())].into(),)
}

fn can_set(fixture: &Fixture) -> bool {
    call(&fixture.client, "CanSetSystemAccessibility", &())
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
}

fn machine_file(fixture: &Fixture) -> Option<String> {
    std::fs::read_to_string(fixture.machine.path().join("cupertino/accessibility.conf")).ok()
}

#[test]
fn an_administrator_at_the_console_sets_the_machines_accessibility_through_the_settings_application(
) {
    let Some(fixture) = fixture_as(Some(session(true)), true, true) else {
        return;
    };
    assert!(can_set(&fixture));
    call(
        &fixture.client,
        "SetSystemAccessibility",
        &reduce_motion("on"),
    )
    .unwrap();
    assert_eq!(
        machine_file(&fixture).as_deref(),
        Some("reduce_motion = on\n")
    );
    assert_eq!(
        error_name(call(
            &fixture.client,
            "SetSystemAccessibility",
            &reduce_motion("off")
        )),
        TOO_MANY_WRITES,
        "a second write within a second"
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    call(
        &fixture.client,
        "SetSystemAccessibility",
        &reduce_motion("off"),
    )
    .unwrap();
    assert_eq!(
        machine_file(&fixture).as_deref(),
        Some("reduce_motion = off\n")
    );
    assert!(
        fixture.calls.try_recv().is_err(),
        "the main loop is not involved"
    );
}

#[test]
fn the_machines_accessibility_is_refused_to_anyone_else_and_nothing_is_written() {
    let remote = CallerSession {
        remote: true,
        ..session(true)
    };
    for (who, session, administrator, settings, error) in [
        (
            "not an administrator",
            Some(session(true)),
            false,
            true,
            NOT_ADMINISTRATOR,
        ),
        (
            "a session in the background",
            Some(session(false)),
            true,
            true,
            NOT_AT_CONSOLE,
        ),
        (
            "a session over the network",
            Some(remote),
            true,
            true,
            NOT_AT_CONSOLE,
        ),
        ("no session", None, true, true, NOT_AT_CONSOLE),
        (
            "another program",
            Some(session(true)),
            true,
            false,
            ACCESS_DENIED,
        ),
    ] {
        let Some(fixture) = fixture_as(session, administrator, settings) else {
            return;
        };
        assert!(!can_set(&fixture), "{who}");
        assert_eq!(
            error_name(call(
                &fixture.client,
                "SetSystemAccessibility",
                &reduce_motion("on")
            )),
            error,
            "{who}"
        );
        assert_eq!(machine_file(&fixture), None, "{who}");
    }
}

#[test]
fn a_call_over_the_bounds_or_with_a_key_the_file_does_not_take_is_refused_whole() {
    let Some(fixture) = fixture_as(Some(session(true)), true, true) else {
        return;
    };
    let long = "o".repeat(65);
    for options in [
        vec![("invert_colors", "on")],
        vec![("reduce_motion", "maybe")],
        vec![("reduce_motion", "on"), ("invert_colors", "on")],
        vec![("reduce_motion", long.as_str())],
    ] {
        let options: std::collections::HashMap<String, String> = options
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(
            error_name(call(
                &fixture.client,
                "SetSystemAccessibility",
                &(options.clone(),)
            )),
            INVALID_ARGS,
            "{options:?}"
        );
    }
    assert_eq!(machine_file(&fixture), None);
}

/// A bus with the system bus's default policy (no one owns a name or calls a method unless a rule allows it), the packaged
/// policy of the Cupertino services, and leave for the test's user to own the manager's name.
fn system_like_bus(dir: &std::path::Path) -> Option<PrivateBus> {
    let packaged = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/files/usr/share/dbus-1/system.d/org.cupertino.conf");
    let config = dir.join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>system</type>
  <listen>unix:dir={dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <deny own="*"/>
    <deny send_type="method_call"/>
    <allow send_type="signal"/>
    <allow send_requested_reply="true" send_type="method_return"/>
    <allow send_requested_reply="true" send_type="error"/>
    <allow receive_type="method_call"/>
    <allow receive_type="method_return"/>
    <allow receive_type="error"/>
    <allow receive_type="signal"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus.Introspectable"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus.Peer"/>
  </policy>
  <policy user="{uid}">
    <allow own="org.cupertino.LoginManager1"/>
  </policy>
  <include>{packaged}</include>
</busconfig>"#,
            dir = dir.display(),
            uid = my_uid(),
            packaged = packaged.display(),
        ),
    )
    .unwrap();
    PrivateBus::start_with_config(&config)
}

#[test]
fn the_packaged_bus_policy_lets_users_call_the_managers_methods_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let Some(bus) = system_like_bus(dir.path()) else {
        return;
    };
    let Some(fixture) = fixture_on(bus, None, false, false) else {
        return;
    };
    // the service answers each of them (its own refusal, since the caller is in no session): the policy let them through
    for (method, answer) in [
        ("RequestPower", Some(ACCESS_DENIED)),
        ("SwitchToLoginWindow", Some(ACCESS_DENIED)),
        ("SessionReady", Some(ACCESS_DENIED)),
        ("SetSystemAccessibility", Some(ACCESS_DENIED)),
        ("CanSetSystemAccessibility", None),
    ] {
        let result = match method {
            "RequestPower" => call(&fixture.client, method, &("restart",)),
            "SetSystemAccessibility" => call(&fixture.client, method, &reduce_motion("on")),
            _ => call(&fixture.client, method, &()),
        };
        match answer {
            Some(error) => assert_eq!(error_name(result), error, "{method}"),
            None => assert!(result.is_ok(), "{method}: {result:?}"),
        }
    }
    // any other member is the bus's refusal, before the service sees it
    assert_eq!(
        error_name(call(&fixture.client, "Shutdown", &())),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
}
