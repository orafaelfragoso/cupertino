#![cfg(feature = "fake")]
//! A stream against a private PipeWire daemon, linked to a video consumer the test runs: the consumer fixes a format,
//! takes a frame with its damage and timestamp, and fixes the new size after a resize. Skipped when `pipewire` is not
//! installed.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_screencast::fake::{Consumer, Daemon, Seen};
use cupertino_screencast::{
    BufferId, Format, Frame, Notice, Offer, Pixel, Rate, Region, Screencast, Size, State, StreamId,
};

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
fn a_consumer_fixes_a_format_takes_a_frame_and_follows_a_resize() {
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
    let size = Size {
        width: 64,
        height: 48,
    };
    screencast.open(
        STREAM,
        "cupertino-screencast-test",
        Offer {
            size,
            pixels: vec![Pixel::Bgrx, Pixel::Bgra],
        },
    );
    let Notice::Node { node, .. } = next("node", &notices, |n| {
        matches!(n, Notice::Node { .. }).then_some(n)
    }) else {
        unreachable!()
    };

    let consumer = Consumer::link(&daemon.remote(), node);

    let negotiated = Format {
        width: 64,
        height: 48,
        pixel: Pixel::Bgrx,
        framerate: Rate { num: 30, denom: 1 },
    };
    let mut buffers: Vec<(BufferId, OwnedFd)> = Vec::new();
    let mut streaming = false;
    while !(streaming && !buffers.is_empty()) {
        match next("streaming", &notices, Some) {
            Notice::Added { buffer, fd, .. } => buffers.push((buffer, fd)),
            Notice::State { state, .. } => {
                if let State::Closed(reason) = &state {
                    panic!("the stream closed: {reason}");
                }
                streaming = state == State::Streaming(negotiated);
            }
            _ => {}
        }
    }
    assert!(buffers.len() >= 2, "{} buffers", buffers.len());

    screencast.dequeue(STREAM);
    let Notice::Dequeued {
        buffer: Some(buffer),
        ..
    } = next("a buffer", &notices, |n| {
        matches!(n, Notice::Dequeued { .. }).then_some(n)
    })
    else {
        panic!("no buffer was free");
    };
    let fd = &buffers.iter().find(|(id, _)| *id == buffer).unwrap().1;
    let pattern: Vec<u8> = (0..64 * 48 * 4).map(|i: u32| (i % 251) as u8).collect();
    File::from(fd.try_clone().unwrap())
        .write_all_at(&pattern, 0)
        .unwrap();
    screencast.queue(
        STREAM,
        buffer,
        Frame {
            damage: vec![Region {
                x: 4,
                y: 8,
                width: 16,
                height: 10,
            }],
            pts: Duration::from_millis(1234),
        },
    );
    let Some(Seen::Frame {
        size: len,
        stride,
        pixels,
        pts,
        damage,
    }) = consumer.next(WAIT, |s| matches!(s, Seen::Frame { .. }))
    else {
        panic!("no frame within {WAIT:?}");
    };
    assert_eq!((len, stride), (64 * 48 * 4, 256));
    assert!(
        pixels == pattern,
        "the consumer reads the frame written into the memfd"
    );
    assert_eq!(pts, 1_234_000_000);
    assert_eq!(
        damage,
        [Region {
            x: 4,
            y: 8,
            width: 16,
            height: 10
        }]
    );

    let smaller = Size {
        width: 32,
        height: 24,
    };
    screencast.resize(STREAM, smaller);
    let resized = Format {
        width: 32,
        height: 24,
        ..negotiated
    };
    let mut removed = 0;
    let mut added = 0;
    loop {
        match next("the new size", &notices, Some) {
            Notice::Removed { .. } => removed += 1,
            Notice::Added { .. } => added += 1,
            Notice::State { state, .. } if state == State::Streaming(resized) && added > 0 => break,
            Notice::State {
                state: State::Closed(reason),
                ..
            } => panic!("the stream closed: {reason}"),
            _ => {}
        }
    }
    assert_eq!(removed, buffers.len(), "every buffer of the old size goes");
    let format = Seen::Format {
        width: 32,
        height: 24,
    };
    assert!(
        consumer.next(WAIT, |s| *s == format).is_some(),
        "the consumer fixes the new size"
    );
}
