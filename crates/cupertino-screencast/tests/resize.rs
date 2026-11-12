#![cfg(feature = "fake")]
//! A resize that reaches a stream between the consumer fixing a format and PipeWire adding its buffers: the stream
//! goes on in the new size. Skipped when `pipewire` is not
//! installed.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_screencast::fake::{Consumer, Daemon, Seen};
use cupertino_screencast::{Notice, Offer, Pixel, Screencast, Size, State, StreamId};

const WAIT: Duration = Duration::from_secs(10);
const STREAM: StreamId = StreamId(7);

fn next<T>(what: &str, from: &mpsc::Receiver<T>, mut pick: impl FnMut(T) -> Option<T>) -> T {
    let until = Instant::now() + WAIT;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let item = from
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no {what} within {WAIT:?}"));
        if let Some(item) = pick(item) {
            return item;
        }
    }
}

#[test]
fn a_resize_while_the_buffers_are_allocated_still_streams_the_new_size() {
    let Some(daemon) = Daemon::start(WAIT) else {
        eprintln!("SKIPPED: pipewire is not installed or did not start");
        return;
    };
    // set before any thread of this process reads the environment
    std::env::set_var("PIPEWIRE_REMOTE", daemon.remote());

    let (notify, notices) = mpsc::channel();
    let screencast = Screencast::connect(move |n| {
        let _ = notify.send(n);
    })
    .unwrap();
    screencast.open(
        STREAM,
        "cupertino-screencast-test",
        Offer {
            size: Size {
                width: 64,
                height: 48,
            },
            pixels: vec![Pixel::Bgrx],
        },
    );
    let Notice::Node { node, .. } = next("node", &notices, |n| {
        matches!(n, Notice::Node { .. }).then_some(n)
    }) else {
        unreachable!()
    };
    let consumer = Consumer::link(&daemon.remote(), node);

    // the format is told before its buffers are added
    next("a fixed format", &notices, |n| {
        matches!(
            n,
            Notice::State {
                state: State::Negotiated(_) | State::Streaming(_),
                ..
            }
        )
        .then_some(n)
    });
    screencast.resize(
        STREAM,
        Size {
            width: 32,
            height: 24,
        },
    );
    loop {
        match next("the new size", &notices, Some) {
            Notice::State {
                state: State::Streaming(format),
                ..
            } if (format.width, format.height) == (32, 24) => break,
            Notice::State {
                state: State::Closed(reason),
                ..
            } => panic!("the stream closed: {reason}"),
            _ => {}
        }
    }
    let format = Seen::Format {
        width: 32,
        height: 24,
    };
    assert!(
        consumer.next(WAIT, |s| *s == format).is_some(),
        "the consumer fixes the new size"
    );
}
