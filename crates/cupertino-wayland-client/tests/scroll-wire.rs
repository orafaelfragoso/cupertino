//! The scroll count protocol against an in-process server.

mod support;

use cupertino_wayland_client::app::{AppEvent, ScrollCounts};
use support::*;
use wayland_server::protocol::wl_pointer::Axis;

#[test]
fn a_server_without_the_global_gives_none() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(ScrollCounts::bind(&mut conn).is_none());
}

#[test]
fn counts_arrive_per_axis_in_order_from_every_watch() {
    let (server, mut conn) = Harness::start(Offer {
        scroll: Some(1),
        ..Offer::NONE
    });
    let scroll = ScrollCounts::bind(&mut conn).expect("the server offers scroll counts");
    let pointer = conn.pointer().expect("the seat has a pointer to watch");
    let _a = scroll.watch(&conn, &pointer);
    let _b = scroll.watch(&conn, &pointer);
    conn.roundtrip().unwrap();
    server.wait_for("get_scroll", 2);
    server.run(|s| {
        s.scrolls[0].scroll_count(Axis::VerticalScroll, 3);
        s.scrolls[0].scroll_count(Axis::HorizontalScroll, 0);
        s.scrolls[1].scroll_count(Axis::VerticalScroll, 1);
        s.scrolls[0].scroll_count(Axis::VerticalScroll, 4);
    });
    let (events, _) = wait_app_events(&mut conn, |e| e.len() >= 4);
    let count = |vertical, count| AppEvent::ScrollCount { vertical, count };
    assert_eq!(
        events,
        [
            count(true, 3),
            count(false, 0),
            count(true, 1),
            count(true, 4)
        ]
    );
}

#[test]
fn dropping_the_watch_destroys_it() {
    let (server, mut conn) = Harness::start(Offer {
        scroll: Some(1),
        ..Offer::NONE
    });
    let scroll = ScrollCounts::bind(&mut conn).unwrap();
    let pointer = conn.pointer().unwrap();
    drop(scroll.watch(&conn, &pointer));
    conn.roundtrip().unwrap();
    server.wait_for("Destroy", 1);
}
