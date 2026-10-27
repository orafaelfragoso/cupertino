//! The control interface against a private session bus (`dbus-daemon --session`); skipped when the daemon is not
//! available.
#![cfg(feature = "dbus")]

use cupertino_login::capabilities::{Answer, Capabilities, Inputs};
use cupertino_login::control::{serve, Request, BUS_NAME, OBJECT_PATH};
use cupertino_login::panel::Kind;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::MessageIterator;
use zbus::message::Type;
use zbus::zvariant::OwnedValue;
use zbus::{block_on, connection, Connection, MatchRule};

const INTERFACE: &str = "org.cupertino.Login1";

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout)
            .ok()
            .filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }

    fn builder(&self) -> connection::Builder<'static> {
        connection::Builder::address(self.address.as_str()).expect("address")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill")
            .arg(&self.pid)
            .status()
            .expect("kill the private bus");
    }
}

#[test]
fn lock_reaches_the_callback_and_a_second_lock_screen_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (locked_tx, locked_rx) = mpsc::channel();
    let _login = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        move |request| locked_tx.send(request).unwrap(),
    )
    .expect("claims the name");

    // what the server does for the lock shortcut: call `Lock` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    block_on(caller.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.cupertino.Login1"),
        "Lock",
        &(),
    ))
    .expect("Lock");
    assert_eq!(
        locked_rx.recv_timeout(Duration::from_secs(5)),
        Ok(Request::Lock { explicit: false })
    );
    assert_eq!(locked_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    block_on(caller.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.cupertino.Login1"),
        "LockNow",
        &(),
    ))
    .expect("LockNow");
    assert_eq!(
        locked_rx.recv_timeout(Duration::from_secs(5)),
        Ok(Request::Lock { explicit: true })
    );

    let second = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        |_| {},
    );
    assert!(
        matches!(second, Err(zbus::Error::NameTaken)),
        "{:?}",
        second.err()
    );
}

#[test]
fn the_panel_methods_reach_the_callback_until_the_session_is_locked() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (requests_tx, requests_rx) = mpsc::channel();
    let login = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        move |request| requests_tx.send(request).unwrap(),
    )
    .expect("claims the name");
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    let call = |method: &'static str| {
        block_on(caller.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, &()))
    };

    for (method, kind) in [
        ("RequestLogOut", Kind::LogOut),
        ("RequestRestart", Kind::Restart),
        ("RequestShutDown", Kind::ShutDown),
    ] {
        call(method).expect(method);
        assert_eq!(
            requests_rx.recv_timeout(Duration::from_secs(5)),
            Ok(Request::Panel(kind)),
            "{method}"
        );
    }

    login.set_locked(true).unwrap();
    let refused = call("RequestShutDown").expect_err("a locked session shows no panel");
    assert!(
        refused.to_string().contains("the session is locked"),
        "{refused}"
    );
    assert_eq!(requests_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
}

#[test]
fn the_power_key_press_and_release_reach_the_callback_in_order_even_when_locked() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (requests_tx, requests_rx) = mpsc::channel();
    let login = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        move |request| requests_tx.send(request).unwrap(),
    )
    .expect("claims the name");
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    let call = |pressed: bool| {
        block_on(caller.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "PowerKey",
            &pressed,
        ))
    };

    call(true).expect("press");
    call(false).expect("release");
    login.set_locked(true).unwrap();
    call(true).expect("a locked session takes the call and the process ignores it");
    let received: Vec<Request> = (0..3)
        .map(|_| {
            requests_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("a request")
        })
        .collect();
    assert_eq!(
        received,
        [
            Request::PowerKey { pressed: true },
            Request::PowerKey { pressed: false },
            Request::PowerKey { pressed: true }
        ]
    );
}

#[test]
fn locked_is_readable_and_announces_each_change_once() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let login = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        |_| {},
    )
    .expect("claims the name");
    let caller = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .expect("address")
        .build()
        .expect("connect");
    let read_locked = || -> bool {
        let reply = caller
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(INTERFACE, "Locked"),
            )
            .expect("Get");
        let value: OwnedValue = reply.body().deserialize().expect("a variant");
        bool::try_from(value).expect("a boolean")
    };
    assert!(!read_locked());
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.Properties")
        .unwrap()
        .member("PropertiesChanged")
        .unwrap()
        .path(OBJECT_PATH)
        .unwrap()
        .build();
    let signals = MessageIterator::for_match_rule(rule, &caller, None).expect("subscribe");
    let (announced_tx, announced_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for signal in signals {
            let body = signal.expect("a signal").body();
            let (interface, changed, _invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = body.deserialize().expect("PropertiesChanged arguments");
            assert_eq!(interface, INTERFACE);
            let locked = bool::try_from(
                changed
                    .get("Locked")
                    .expect("the changed property")
                    .try_clone()
                    .unwrap(),
            )
            .expect("a boolean");
            if announced_tx.send(locked).is_err() {
                return;
            }
        }
    });

    login.set_locked(true).unwrap();
    assert_eq!(announced_rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    // the same value again changes nothing and announces nothing
    login.set_locked(true).unwrap();
    login.set_locked(false).unwrap();
    assert_eq!(announced_rx.recv_timeout(Duration::from_secs(5)), Ok(false));
    assert_eq!(
        announced_rx.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    assert!(!read_locked());
}

#[test]
fn sleep_and_the_power_dialog_reach_the_callback_and_a_locked_session_refuses_them() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (requests_tx, requests_rx) = mpsc::channel();
    let login = serve(
        bus.builder(),
        "Test User".into(),
        Capabilities::default(),
        move |request| requests_tx.send(request).unwrap(),
    )
    .expect("claims the name");
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    let call = |method: &str| {
        block_on(caller.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, &()))
    };
    call("Sleep").expect("Sleep");
    call("RequestPowerDialog").expect("RequestPowerDialog");
    assert_eq!(
        requests_rx.recv_timeout(Duration::from_secs(5)),
        Ok(Request::Sleep)
    );
    assert_eq!(
        requests_rx.recv_timeout(Duration::from_secs(5)),
        Ok(Request::Panel(Kind::PowerButton))
    );
    login.set_locked(true).unwrap();
    for method in ["Sleep", "RequestPowerDialog"] {
        let refused = call(method).expect_err(method).to_string();
        assert!(
            refused.contains("the session is locked"),
            "{method}: {refused}"
        );
    }
    assert!(
        requests_rx.try_recv().is_err(),
        "a locked session starts nothing"
    );
}

#[test]
fn the_menu_properties_are_read_and_capabilities_announce_a_change() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let before = Capabilities::new(&Inputs {
        can_suspend: Answer::Yes,
        can_reboot: Answer::Challenge,
        can_power_off: Answer::No,
        session_manager: false,
    });
    let login = serve(bus.builder(), "Rafael".into(), before, |_| {}).expect("claims the name");
    let caller = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .expect("address")
        .build()
        .expect("connect");
    let get = |name: &str| -> OwnedValue {
        let reply = caller
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(INTERFACE, name),
            )
            .expect("Get");
        reply.body().deserialize().expect("a variant")
    };
    assert_eq!(String::try_from(get("UserName")).unwrap(), "Rafael");
    let read = || -> HashMap<String, bool> { HashMap::try_from(get("Capabilities")).unwrap() };
    assert_eq!(read(), before.to_map());
    assert!(read()["restart"]);
    assert!(!read()["shut-down"]);
    assert!(!read()["switch-user"]);

    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.Properties")
        .unwrap()
        .member("PropertiesChanged")
        .unwrap()
        .path(OBJECT_PATH)
        .unwrap()
        .build();
    let signals = MessageIterator::for_match_rule(rule, &caller, None).expect("subscribe");
    let (announced_tx, announced_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for signal in signals {
            let body = signal.expect("a signal").body();
            let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                body.deserialize().expect("PropertiesChanged arguments");
            if let Some(value) = changed.get("Capabilities") {
                let map = HashMap::<String, bool>::try_from(value.try_clone().unwrap()).unwrap();
                announced_tx.send(map).unwrap();
            }
        }
    });
    // the session manager comes up: nothing else changes
    let after = Capabilities {
        switch_user: true,
        ..before
    };
    login.set_capabilities(after).unwrap();
    login.set_capabilities(after).unwrap();
    assert_eq!(
        announced_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        after.to_map()
    );
    assert!(
        announced_rx
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "an unchanged value is not announced again"
    );
    assert!(read()["switch-user"]);
}
