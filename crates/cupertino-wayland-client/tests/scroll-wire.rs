//! The scroll count protocol against an in-process server.

mod support;

use cupertino_app_protocols::server::scroll::cupertino_pointer_scroll_v1::CupertinoPointerScrollV1;
use cupertino_wayland_client::app::{AppEvent, ScrollCounts};
use cupertino_wayland_client::Connection;
use support::*;
use wayland_server::protocol::wl_pointer::Axis;

/// A connection whose seat has a pointer, the server's scroll resources made for it.
fn watched() -> (Harness, Connection, ScrollCounts) {
    let (server, mut conn) = Harness::start(Offer {
        scroll: Some(1),
        ..Offer::NONE
    });
    let scroll = ScrollCounts::bind(&mut conn).expect("the server offers scroll counts");
    (server, conn, scroll)
}

#[test]
fn a_server_without_the_global_gives_none() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(ScrollCounts::bind(&mut conn).is_none());
}

#[test]
fn the_seat_has_a_pointer_to_watch() {
    let (_server, conn, _scroll) = watched();
    assert!(conn.pointer().is_some());
}

#[test]
fn counts_arrive_per_axis_in_order() {
    let (server, mut conn, scroll) = watched();
    let pointer = conn.pointer().unwrap();
    let _watch = scroll.watch(&conn, &pointer);
    conn.roundtrip().unwrap();
    server.wait_for("get_scroll", 1);
    server.run(|s| {
        let scroll: &CupertinoPointerScrollV1 = &s.scrolls[0];
        scroll.scroll_count(Axis::VerticalScroll, 3);
        scroll.scroll_count(Axis::HorizontalScroll, 0);
        scroll.scroll_count(Axis::VerticalScroll, 4);
    });
    let (events, _) = wait_app_events(&mut conn, |e| e.len() >= 3);
    assert_eq!(
        events,
        [
            AppEvent::ScrollCount {
                vertical: true,
                count: 3
            },
            AppEvent::ScrollCount {
                vertical: false,
                count: 0
            },
            AppEvent::ScrollCount {
                vertical: true,
                count: 4
            },
        ]
    );
}

#[test]
fn a_pointer_may_be_watched_twice() {
    let (server, mut conn, scroll) = watched();
    let pointer = conn.pointer().unwrap();
    let _a = scroll.watch(&conn, &pointer);
    let _b = scroll.watch(&conn, &pointer);
    conn.roundtrip().unwrap();
    server.wait_for("get_scroll", 2);
    server.run(|s| {
        for watch in &s.scrolls {
            watch.scroll_count(Axis::VerticalScroll, 1);
        }
    });
    let (events, _) = wait_app_events(&mut conn, |e| e.len() >= 2);
    assert_eq!(events.len(), 2);
}

#[test]
fn dropping_the_watch_destroys_it() {
    let (server, mut conn, scroll) = watched();
    let pointer = conn.pointer().unwrap();
    let watch = scroll.watch(&conn, &pointer);
    drop(watch);
    conn.roundtrip().unwrap();
    server.wait_for("Destroy", 1);
}
