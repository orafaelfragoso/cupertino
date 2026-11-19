//! Screen and window capture (`capture` role, `cupertino-screenshot`, `docs/architecture/shell-host.md#consent-and-capture`).

use cupertino_kit::core::Command;
use cupertino_kit::geometry::Rect;
use cupertino_kit::render::ImageResource;

use crate::context::{self, ShellRequest};
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

/// Capture a single frame of `source` with `options` and deliver the result via `done`.
pub fn once<M: Send + 'static>(
    source: Source,
    options: Options,
    done: impl FnOnce(Result<ImageResource, Error>) -> M + Send + 'static,
) -> Command<M> {
    let _ = (source, options);
    Command::Perform(Box::pin(async move {
        done(Err(Error::Protocol(
            "capture::once requires an active capture session".into(),
        )))
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
