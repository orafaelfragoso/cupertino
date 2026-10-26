//! Talks to a real bus: a private `dbus-daemon` is started for the test (skipped when the binary
//! is not installed), the server connects to it and a second connection plays the client.
#![cfg(feature = "dbus")]

use cupertino_notifications::dbus::{Server, ServerOptions, BUS_NAME, CENTRE_BUS_NAME, CENTRE_OBJECT_PATH, OBJECT_PATH};
use cupertino_notifications::{CloseReason, DndMode};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

struct PrivateBus {
    child: Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_bus() -> Option<PrivateBus> {
    let dir = tempfile::tempdir().ok()?;
    let config = dir.path().join("bus.conf");
    let socket_dir = dir.path().display();
    let mut f = std::fs::File::create(&config).ok()?;
    write!(
        f,
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={socket_dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>"#
    )
    .ok()?;
    let mut child = Command::new("dbus-daemon").arg(format!("--config-file={}", config.display())).args(["--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let address = line.trim().to_string();
    if address.is_empty() {
        let _ = child.kill();
        return None;
    }
    Some(PrivateBus { child, address, _dir: dir })
}

fn client(bus: &PrivateBus) -> Connection {
    zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap().build().unwrap()
}

fn proxy<'a>(c: &'a Connection) -> Proxy<'a> {
    Proxy::new(c, BUS_NAME, OBJECT_PATH, "org.freedesktop.Notifications").unwrap()
}

type NotifyArgs<'a> = (&'a str, u32, &'a str, &'a str, &'a str, Vec<&'a str>, HashMap<&'a str, Value<'a>>, i32);

fn notify(p: &Proxy<'_>, args: NotifyArgs<'_>) -> u32 {
    p.call("Notify", &args).unwrap()
}

fn spawn_signal_listener(c: &Connection, member: &'static str) -> mpsc::Receiver<zbus::Message> {
    let (tx, rx) = mpsc::channel();
    let conn = c.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let p = proxy(&conn);
        let it = p.receive_signal(member).unwrap();
        ready_tx.send(()).unwrap();
        for m in it {
            if tx.send(m).is_err() {
                break;
            }
        }
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    rx
}

#[test]
fn full_round_trip_over_a_private_bus() {
    let Some(bus) = start_bus() else {
        eprintln!("dbus-daemon not available, skipping");
        return;
    };
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let server = Server::start(ServerOptions {
        address: Some(bus.address.clone()),
        wake: Some(Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })),
        ..Default::default()
    })
    .expect("server starts on the private bus");

    let c = client(&bus);
    let p = proxy(&c);
    let closed = spawn_signal_listener(&c, "NotificationClosed");
    let actions = spawn_signal_listener(&c, "ActionInvoked");

    let (name, vendor, version, spec): (String, String, String, String) = p.call("GetServerInformation", &()).unwrap();
    assert_eq!(name, "cupertino-notifications");
    assert!(!vendor.is_empty() && !version.is_empty());
    assert_eq!(spec, "1.2");

    let caps: Vec<String> = p.call("GetCapabilities", &()).unwrap();
    for c in ["actions", "body", "body-markup", "persistence"] {
        assert!(caps.iter().any(|x| x == c), "capability {c}: {caps:?}");
    }

    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(1));
    hints.insert("category", Value::from("email.arrived"));
    hints.insert("desktop-entry", Value::from("org.example.Mail"));
    let id = notify(&p, ("Mail", 0, "mail-unread", "New message", "<b>Alice</b> wrote: hi &amp; bye", vec!["default", "Open", "reply", "Reply"], hints, -1));
    assert!(id > 0);
    assert!(wakes.load(Ordering::SeqCst) >= 1);
    {
        let center = server.center();
        let n = center.get(id).expect("stored");
        assert_eq!(n.app_name, "Mail");
        assert_eq!(n.body_text(), "Alice wrote: hi & bye");
        assert_eq!(n.actions.len(), 2);
        assert_eq!(n.group_key(), "org.example.Mail");
        assert_eq!(center.visible_count(), 1);
    }

    // expire_timeout 0: the banner stays until dismissed, so later notifications wait behind it
    let id2 = notify(&p, ("Mail", id, "", "New message (edited)", "", vec![], HashMap::new(), 0));
    assert_eq!(id2, id, "replaces_id keeps the id");
    assert_eq!(server.center().get(id).unwrap().summary, "New message (edited)");
    assert_eq!(server.center().history_len(), 1);

    let other = notify(&p, ("Chat", 0, "", "Ping", "", vec![], HashMap::new(), 0));
    assert_ne!(other, id);

    let _: () = p.call("CloseNotification", &(other,)).unwrap();
    let m = closed.recv_timeout(Duration::from_secs(5)).expect("NotificationClosed signal");
    let (cid, reason): (u32, u32) = m.body().deserialize().unwrap();
    assert_eq!((cid, reason), (other, CloseReason::Closed as u32));
    assert!(server.center().get(other).is_none() || server.center().history_len() == 1);

    // host side: invoke an action and expire another notification through the model
    let with_action = notify(&p, ("Mail", 0, "", "Action", "", vec!["reply", "Reply"], HashMap::new(), -1));
    let now = server.now();
    assert!(server.center().invoke_action(with_action, "reply", None, now));
    assert_eq!(server.flush_events().unwrap(), 2);
    let m = actions.recv_timeout(Duration::from_secs(5)).expect("ActionInvoked signal");
    let (aid, key): (u32, String) = m.body().deserialize().unwrap();
    assert_eq!((aid, key.as_str()), (with_action, "reply"));
    let m = closed.recv_timeout(Duration::from_secs(5)).expect("NotificationClosed after the action");
    let (cid, reason): (u32, u32) = m.body().deserialize().unwrap();
    assert_eq!((cid, reason), (with_action, CloseReason::Dismissed as u32));

    // expiry produced by tick(): the persistent banner is dismissed, the waiting one takes its
    // place and expires after its 500 ms client timeout
    let short = notify(&p, ("Mail", 0, "", "Short", "", vec![], HashMap::new(), 500));
    let now = server.now();
    server.center().dismiss_banner(id, now);
    server.center().tick(now + 0.5);
    assert_eq!(server.flush_events().unwrap(), 2);
    for expected in [(id, CloseReason::Dismissed), (short, CloseReason::Expired)] {
        let m = closed.recv_timeout(Duration::from_secs(5)).unwrap();
        let (cid, reason): (u32, u32) = m.body().deserialize().unwrap();
        assert_eq!((cid, reason), (expected.0, expected.1 as u32));
    }
}

#[test]
fn do_not_disturb_and_image_data_over_the_bus() {
    let Some(bus) = start_bus() else { return };
    let server = Server::start(ServerOptions { address: Some(bus.address.clone()), ..Default::default() }).unwrap();
    let c = client(&bus);
    let p = proxy(&c);
    let now = server.now();
    server.center().set_dnd(DndMode::On, now);
    let id = notify(&p, ("App", 0, "", "quiet", "", vec![], HashMap::new(), -1));
    assert_eq!(server.center().visible_count(), 0);
    assert!(server.center().get(id).is_some(), "kept in the centre");

    let pixels: Vec<u8> = vec![10, 20, 30, 255, 40, 50, 60, 255];
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("image-data", Value::from((2i32, 1i32, 8i32, true, 8i32, 4i32, pixels.clone())));
    hints.insert("urgency", Value::U8(2));
    let critical = notify(&p, ("App", 0, "", "important", "", vec![], hints, -1));
    let center = server.center();
    assert_eq!(center.visible_count(), 1, "critical bypasses do-not-disturb");
    assert_eq!(center.get(critical).unwrap().image.as_ref().unwrap().rgba, pixels);
}

#[test]
fn a_second_server_cannot_take_the_name() {
    let Some(bus) = start_bus() else { return };
    let _first = Server::start(ServerOptions { address: Some(bus.address.clone()), ..Default::default() }).unwrap();
    let second = Server::start(ServerOptions { address: Some(bus.address.clone()), ..Default::default() });
    assert!(second.is_err());
}

#[test]
fn toggle_centre_calls_the_host_callback() {
    let Some(bus) = start_bus() else { return };
    let server = Server::start(ServerOptions { address: Some(bus.address.clone()), ..Default::default() }).unwrap();
    let toggles = Arc::new(AtomicUsize::new(0));
    let t = toggles.clone();
    server
        .serve_centre_control(move || {
            t.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let c = client(&bus);
    let p = Proxy::new(&c, CENTRE_BUS_NAME, CENTRE_OBJECT_PATH, "org.cupertino.NotificationCenter1").unwrap();
    let _: () = p.call("ToggleCentre", &()).unwrap();
    let _: () = p.call("ToggleCentre", &()).unwrap();
    assert_eq!(toggles.load(Ordering::SeqCst), 2);
}

#[test]
fn do_not_disturb_is_set_and_read_over_the_bus() {
    let Some(bus) = start_bus() else { return };
    let server = Server::start(ServerOptions { address: Some(bus.address.clone()), ..Default::default() }).unwrap();
    server.serve_centre_control(|| {}).unwrap();
    let c = client(&bus);
    // uncached, so every read asks the server
    let p: Proxy = zbus::blocking::proxy::Builder::new(&c)
        .destination(CENTRE_BUS_NAME)
        .unwrap()
        .path(CENTRE_OBJECT_PATH)
        .unwrap()
        .interface("org.cupertino.NotificationCenter1")
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap();
    assert!(!p.get_property::<bool>("DoNotDisturb").unwrap());
    let _: () = p.call("SetDoNotDisturb", &(true,)).unwrap();
    assert_eq!(server.center().dnd(), DndMode::On);
    assert!(p.get_property::<bool>("DoNotDisturb").unwrap());
    let _: () = p.call("SetDoNotDisturb", &(false,)).unwrap();
    assert_eq!(server.center().dnd(), DndMode::Off);
    assert!(!p.get_property::<bool>("DoNotDisturb").unwrap());
}

#[test]
fn focus_durations_are_offered_set_and_their_end_announced() {
    let Some(bus) = start_bus() else { return };
    let now = Arc::new(std::sync::Mutex::new(100.0_f64));
    let clock = now.clone();
    let server = Server::start(ServerOptions { address: Some(bus.address.clone()), clock: Some(Box::new(move || *clock.lock().unwrap())), ..Default::default() }).unwrap();
    server.serve_centre_control(|| {}).unwrap();
    let c = client(&bus);
    let p: Proxy = zbus::blocking::proxy::Builder::new(&c)
        .destination(CENTRE_BUS_NAME)
        .unwrap()
        .path(CENTRE_OBJECT_PATH)
        .unwrap()
        .interface("org.cupertino.NotificationCenter1")
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap();

    // "For 1 hour" first, then the evening or the morning row, whichever the hour offers
    let rows: Vec<(String, String)> = p.get_property("FocusLifetimes").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("one-hour".to_owned(), "For 1 hour".to_owned()));
    assert!(matches!(rows[1].0.as_str(), "evening" | "morning"), "{rows:?}");

    assert!(p.call::<_, _, ()>("SetDoNotDisturbFor", &("location",)).is_err(), "rows the session cannot offer are refused");
    let _: () = p.call("SetDoNotDisturbFor", &("one-hour",)).unwrap();
    assert_eq!(server.center().dnd(), DndMode::Until(100.0 + 3600.0));
    assert!(p.get_property::<bool>("DoNotDisturb").unwrap());

    // the end is announced like a switch, so the menu bar's indicator goes away
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).path(CENTRE_OBJECT_PATH).unwrap().member("PropertiesChanged").unwrap().build();
    let mut changes = zbus::blocking::MessageIterator::for_match_rule(rule, &c, None).unwrap();
    *now.lock().unwrap() = 100.0 + 3600.0;
    server.center().tick(server.now());
    server.flush_events().unwrap();
    let message = changes.next().expect("a signal").unwrap();
    let (interface, changed, _): (String, HashMap<String, zbus::zvariant::OwnedValue>, Vec<String>) = message.body().deserialize().unwrap();
    assert_eq!(interface, "org.cupertino.NotificationCenter1");
    assert_eq!(changed.get("DoNotDisturb").map(|v| bool::try_from(v).unwrap()), Some(false));
    assert!(!p.get_property::<bool>("DoNotDisturb").unwrap());
}
