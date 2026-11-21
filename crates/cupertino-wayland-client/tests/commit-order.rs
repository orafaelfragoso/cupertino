//! A present committed from another thread than the connection's: it acknowledges the configure its buffer was drawn
//! for, then attaches and commits, and a configure that arrives meanwhile waits for the next frame.

use crate::support;

use cupertino_wayland_client::{BufferFormat, Event};
use std::time::Duration;
use support::*;

#[test]
fn a_commit_from_another_thread_acknowledges_its_configure_before_it_attaches_and_commits() {
    let (server, mut conn) = Harness::start(Offer::NONE);
    let window = window(&mut conn);
    let surface = window.surface();
    let lease = conn
        .lease(surface, (16, 16), BufferFormat::Argb8888)
        .unwrap();
    let drawn_for = lease.serial();
    let (mut buffer, commit) = lease.detach().unwrap();
    assert_eq!(commit.serial(), Some(drawn_for));
    buffer.pixels_mut().fill(0x80);

    // a configure arrives while the frame is drawn elsewhere
    server.run(|s| {
        s.reconfigure(0);
    });
    let mut later = None;
    for _ in 0..100 {
        later = conn
            .wait(Some(Duration::from_millis(50)))
            .unwrap()
            .iter()
            .find_map(|e| match e {
                Event::Configure { configure, .. } => Some(configure.serial),
                _ => None,
            });
        if later.is_some() {
            break;
        }
    }
    let later = later.expect("the server configured the window again");
    assert_ne!(later, drawn_for);

    let before = server.presents().len();
    std::thread::spawn(move || commit.commit(None))
        .join()
        .unwrap()
        .unwrap();
    conn.buffer_shown(buffer, None);
    conn.roundtrip().unwrap();
    let sent = server.presents()[before..].to_vec();
    assert_eq!(
        sent,
        [
            format!("AckConfigure {drawn_for}"),
            "Attach".into(),
            "Commit".into()
        ],
        "the configure the frame was drawn for, acknowledged before the attach and the commit"
    );

    // the next frame answers the configure that waited
    let before = server.presents().len();
    conn.lease(surface, (16, 16), BufferFormat::Argb8888)
        .unwrap()
        .present()
        .unwrap();
    conn.roundtrip().unwrap();
    assert_eq!(
        server.presents()[before..],
        [
            format!("AckConfigure {later}"),
            "Attach".into(),
            "Commit".into()
        ]
    );
}

#[test]
fn a_commit_dropped_unsent_leaves_the_crop_of_the_last_magnified_frame_for_the_next_to_unset() {
    let (server, mut conn) = Harness::start(Offer {
        scaling: true,
        ..Offer::NONE
    });
    let window = window(&mut conn);
    let surface = window.surface();
    let mut zoomed = conn
        .lease(surface, (16, 16), BufferFormat::Argb8888)
        .unwrap();
    assert!(zoomed.set_zoom(2.0), "the surface has a viewport");
    zoomed.present().unwrap();
    conn.roundtrip().unwrap();
    assert!(
        server
            .presents()
            .iter()
            .any(|p| p.starts_with("SetSource 4 4")),
        "{:?}",
        server.presents()
    );

    // a frame at full size, its commit dropped unsent
    let (buffer, commit) = conn
        .lease(surface, (16, 16), BufferFormat::Argb8888)
        .unwrap()
        .detach()
        .unwrap();
    drop(commit);
    conn.buffer_unused(buffer);

    let before = server.presents().len();
    conn.lease(surface, (16, 16), BufferFormat::Argb8888)
        .unwrap()
        .present()
        .unwrap();
    conn.roundtrip().unwrap();
    assert!(
        server.presents()[before..]
            .iter()
            .any(|p| p == "SetSource -1 -1 -1 -1"),
        "the next frame shows the whole buffer again: {:?}",
        &server.presents()[before..]
    );
}
