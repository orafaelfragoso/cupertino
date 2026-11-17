//! The idle notification over a real wire: the timeout the watch asks for, the events it reports and its end.

mod support;

use cupertino_shell_client::{IdleNotifier, ShellEvent};
use support::{Offer, Seen, Wire};
use wayland_protocols::ext::idle_notify::v1::server::ext_idle_notification_v1::Request;

#[test]
fn the_notifier_binds_at_version_one_or_two_and_not_when_absent() {
    for version in [1, 2] {
        let mut wire = Wire::new(Offer {
            idle: Some(version),
            ..Offer::all()
        });
        assert!(IdleNotifier::bind(&mut wire.conn).is_some());
    }
    let mut wire = Wire::new(Offer {
        idle: None,
        ..Offer::all()
    });
    assert!(IdleNotifier::bind(&mut wire.conn).is_none());
}

#[test]
fn a_watch_asks_for_its_timeout_and_reports_idle_and_resume() {
    let mut wire = Wire::all();
    let notifier = IdleNotifier::bind(&mut wire.conn).expect("notifier");
    let seat = wire.conn.seat().expect("seat");
    let watch = notifier.watch(&wire.conn, &seat, 300_000);
    wire.events();
    assert!(matches!(
        wire.take()[..],
        [Seen::IdleCreated { millis: 300_000 }]
    ));

    wire.server(|s| {
        s.idle[0].idled();
        s.idle[0].resumed();
        s.idle[0].idled();
    });
    assert_eq!(
        wire.shell_events(),
        [
            ShellEvent::Idle(true),
            ShellEvent::Idle(false),
            ShellEvent::Idle(true)
        ]
    );
    drop(watch);
}

#[test]
fn dropping_a_watch_destroys_its_notification() {
    let mut wire = Wire::all();
    let notifier = IdleNotifier::bind(&mut wire.conn).expect("notifier");
    let seat = wire.conn.seat().expect("seat");
    drop(notifier.watch(&wire.conn, &seat, 1000));
    wire.events();
    let seen = wire.take();
    assert!(matches!(seen[0], Seen::IdleCreated { millis: 1000 }));
    assert!(matches!(seen[1], Seen::Idle(Request::Destroy)));
}
