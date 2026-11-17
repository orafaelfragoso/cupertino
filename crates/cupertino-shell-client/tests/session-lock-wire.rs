//! The session lock over a real wire: generations of locks, confirmation, lock surfaces and ending a lock.

mod support;

use cupertino_shell_client::{Error, SessionLockManager, ShellEvent};
use cupertino_wayland_client::OutputId;
use std::time::Duration;
use support::{Offer, Seen, Wire};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::Request;

#[test]
fn the_manager_binds_only_when_offered() {
    let mut wire = Wire::new(Offer {
        lock: false,
        ..Offer::all()
    });
    assert!(SessionLockManager::bind(&mut wire.conn).is_none());
    let mut wire = Wire::all();
    assert!(SessionLockManager::bind(&mut wire.conn).is_some());
}

#[test]
fn each_lock_has_the_next_generation() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    let first = manager.lock(&wire.conn);
    let second = manager.lock(&wire.conn);
    assert_eq!(first.generation(), 1);
    assert_eq!(second.generation(), 2);
    wire.events();
    let created = wire
        .take()
        .iter()
        .filter(|s| matches!(s, Seen::LockCreated))
        .count();
    assert_eq!(created, 2);
}

#[test]
fn a_confirmation_and_a_finish_name_the_lock_they_belong_to() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    let _first = manager.lock(&wire.conn);
    let _second = manager.lock(&wire.conn);
    wire.events();

    wire.server(|s| {
        s.locks[1].locked();
        s.locks[0].finished();
    });
    assert_eq!(
        wire.shell_events(),
        [
            ShellEvent::Lock {
                generation: 2,
                locked: true
            },
            ShellEvent::Lock {
                generation: 1,
                locked: false
            }
        ]
    );
}

#[test]
fn a_lock_surface_is_configured_and_reported() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    let lock = manager.lock(&wire.conn);
    let output = wire.conn.outputs()[0].id;
    let surface = lock
        .lock_surface(&mut wire.conn, output)
        .expect("lock surface");
    wire.events();
    assert!(wire
        .take()
        .iter()
        .any(|s| matches!(s, Seen::LockSurfaceCreated)));

    wire.server(|s| s.lock_surfaces[0].configure(3, 1920, 1080));
    assert_eq!(
        wire.shell_events(),
        [ShellEvent::Configure {
            surface,
            size: (1920, 1080)
        }]
    );
}

#[test]
fn a_lock_surface_for_an_unplugged_output_is_refused() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    let lock = manager.lock(&wire.conn);
    let result = lock.lock_surface(&mut wire.conn, OutputId::from_registry_name(9999));
    assert!(matches!(result, Err(Error::UnknownOutput)));
}

#[test]
fn a_lock_surface_configured_without_a_size_fails_the_connection() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    let lock = manager.lock(&wire.conn);
    let output = wire.conn.outputs()[0].id;
    lock.lock_surface(&mut wire.conn, output)
        .expect("lock surface");
    wire.events();

    wire.server(|s| s.lock_surfaces[0].configure(3, 0, 1080));
    let result = wire
        .conn
        .roundtrip()
        .and_then(|()| wire.conn.wait(Some(Duration::ZERO)).map(|_| ()));
    let error = result.expect_err("a configure without a size is refused");
    assert!(matches!(
        error,
        cupertino_wayland_client::Error::Protocol(_)
    ));
}

#[test]
fn unlock_and_destroy_send_their_request() {
    let mut wire = Wire::all();
    let mut manager = SessionLockManager::bind(&mut wire.conn).expect("manager");
    manager.lock(&wire.conn).unlock();
    manager.lock(&wire.conn).destroy();
    wire.events();
    let requests: Vec<_> = wire
        .take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Lock(r) => Some(r),
            _ => None,
        })
        .collect();
    assert!(matches!(requests[0], Request::UnlockAndDestroy));
    assert!(matches!(requests[1], Request::Destroy));
}
