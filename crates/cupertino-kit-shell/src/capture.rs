//! Screen and window capture (`capture` role, `cupertino-screenshot`, `docs/architecture/shell-host.md#consent-and-capture`).

use std::collections::HashMap;
use std::sync::Arc;

use cupertino_kit::core::Command;
use cupertino_kit::geometry::Rect;
use cupertino_kit::render::ImageResource;

use crate::context::{self, ShellRequest};
use crate::oneshot::Oneshot;
use crate::outputs::OutputId;
use crate::Error;

/// Capture source: an output or a window identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Output(OutputId),
    Window(String),
}

/// Capture options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    pub cursor: bool,
    pub region: Option<Rect>,
}

/// One window on screen reported by [`windows`].
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub identifier: String,
    pub frame: Rect,
}

/// Active continuous capture session; dropping it stops capturing.
#[derive(Debug)]
pub struct Follow {
    id: u32,
}

impl Follow {
    /// Identifier matching [`Event::Frame::follow`].
    pub fn id(&self) -> u32 {
        self.id
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        context::push_request(ShellRequest::CaptureFollowStop { id: self.id });
    }
}

/// Capture events delivered as [`crate::Event::Capture`].
#[derive(Clone, Debug)]
pub enum Event {
    Windows(Vec<Window>),
    Frame { follow: u32, image: ImageResource },
}

/// Follow the list of windows on screen, front to back (`Event::Windows`).
pub fn windows() {
    context::push_request(ShellRequest::CaptureWindows);
}

/// The result of one single-frame capture.
type Frame = Oneshot<Result<ImageResource, Error>>;

/// The single-frame captures of a program that wait for their frame, by the id of their request. A capture still
/// waiting when the program's context goes ends with an error, so its task never waits for good.
#[derive(Default)]
pub(crate) struct Captures {
    waiting: HashMap<u32, Arc<Frame>>,
}

impl Captures {
    /// Gives capture `id` its frame, or why it has none; false when no capture of that id waits.
    #[allow(dead_code)] // the protocol side answers through it (`cupertino_screenshot_v1`)
    pub(crate) fn finish(&mut self, id: u32, frame: Result<ImageResource, Error>) -> bool {
        self.waiting
            .remove(&id)
            .is_some_and(|waiting| waiting.fill(frame))
    }
}

impl Drop for Captures {
    fn drop(&mut self) {
        for (_, waiting) in self.waiting.drain() {
            waiting.fill(Err(Error::Protocol(
                "the capture ended with its program".into(),
            )));
        }
    }
}

/// Capture a single frame of `source` with `options` and deliver the result via `done`.
///
/// The request is queued with the program's other capture requests (`cupertino_screenshot_v1`, the `capture` role),
/// and the task waits, without polling, until the side that carries those requests gives the frame. On a thread that
/// runs no shell program, and when the program ends first, `done` gets an error.
pub fn once<M: Send + 'static>(
    source: Source,
    options: Options,
    done: impl FnOnce(Result<ImageResource, Error>) -> M + Send + 'static,
) -> Command<M> {
    let waiting = context::with_ctx_mut(|ctx| {
        let id = ctx.next_capture_id;
        ctx.next_capture_id = ctx.next_capture_id.wrapping_add(1).max(1);
        let frame = Arc::new(Frame::default());
        ctx.captures.waiting.insert(id, Arc::clone(&frame));
        ctx.outgoing.push_back(ShellRequest::CaptureOnce {
            id,
            source,
            options,
        });
        frame
    });
    Command::Perform(Box::pin(async move {
        match waiting {
            Some(frame) => done(frame.wait().await),
            None => done(Err(Error::Protocol(
                "capture::once called on a thread that runs no shell program".into(),
            ))),
        }
    }))
}

/// Follow every changing frame of `source` with `options` as [`Event::Frame`]; dropping [`Follow`] stops it.
pub fn follow(source: Source, options: Options) -> Follow {
    let id = context::with_ctx_mut(|ctx| {
        let id = ctx.next_follow_id;
        ctx.next_follow_id = ctx.next_follow_id.wrapping_add(1).max(1);
        ctx.outgoing.push_back(ShellRequest::CaptureFollowStart {
            id,
            source,
            options,
        });
        id
    })
    .unwrap_or(0);
    Follow { id }
}
