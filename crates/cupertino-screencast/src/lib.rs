//! PipeWire video source streams for screen sharing. See the README.

#[cfg(feature = "fake")]
pub mod fake;
pub mod format;
mod node;

use std::fmt;
use std::os::fd::OwnedFd;
use std::thread::JoinHandle;
use std::time::Duration;

pub use format::{Format, Mismatch, Offer, Pixel, Rate, Size};

/// A stream, named by its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub u64);

/// A buffer of a stream; never reused while the process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(u64);

/// Where a stream is. A stream its owner closed reports nothing more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for a consumer to fix a format, again after the source was resized.
    Connecting,
    /// A consumer fixed `Format`, and buffers of it follow as [`Notice::Added`]; it takes no frames yet, or stopped.
    Negotiated(Format),
    /// The consumer takes frames of `Format`.
    Streaming(Format),
    /// PipeWire failed the stream or went away, for the reason given.
    Closed(String),
}

/// A changed rectangle of a frame, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A frame written into a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// What changed since the frame before.
    pub damage: Vec<Region>,
    /// When it was shown, on `CLOCK_MONOTONIC`.
    pub pts: Duration,
}

/// What the PipeWire thread tells the owner of the streams.
#[derive(Debug)]
pub enum Notice {
    /// The stream's node is in the graph; a consumer connects to `node`.
    Node {
        stream: StreamId,
        node: u32,
    },
    State {
        stream: StreamId,
        state: State,
    },
    /// A buffer of the negotiated format: `fd` is a memfd that holds one frame at offset 0, at the format's stride.
    Added {
        stream: StreamId,
        buffer: BufferId,
        fd: OwnedFd,
    },
    /// The buffer is gone and is not filled any more.
    Removed {
        stream: StreamId,
        buffer: BufferId,
    },
    /// The answer to [`Screencast::dequeue`]: a buffer to fill, or `None` while the consumer holds them all.
    Dequeued {
        stream: StreamId,
        buffer: Option<BufferId>,
    },
}

enum Command {
    Open {
        stream: StreamId,
        name: String,
        offer: Offer,
    },
    Resize {
        stream: StreamId,
        size: Size,
    },
    Dequeue(StreamId),
    Queue {
        stream: StreamId,
        buffer: BufferId,
        frame: Frame,
    },
    Close(StreamId),
    Quit,
}

/// PipeWire could not be reached.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The connection to the user's PipeWire, served by a thread of its own that owns every stream. Dropping it closes
/// them and ends the thread.
pub struct Screencast {
    commands: pipewire::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Screencast {
    /// Connect to the PipeWire that `PIPEWIRE_REMOTE` or the runtime directory names. `notices` runs on the PipeWire
    /// thread, so it hands them on without blocking.
    pub fn connect(notices: impl Fn(Notice) + Send + 'static) -> Result<Screencast, Error> {
        let (commands, receiver) = pipewire::channel::channel();
        let (ready, connected) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pipewire".to_owned())
            .spawn(move || node::run(receiver, Box::new(notices), ready))
            .map_err(|e| Error(format!("starting the PipeWire thread: {e}")))?;
        match connected.recv() {
            Ok(Ok(())) => Ok(Screencast {
                commands,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(Error(e))
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error("the PipeWire thread ended".to_owned()))
            }
        }
    }

    /// Whether the connection still runs; when PipeWire goes away it reports every stream closed and ends.
    pub fn is_connected(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn send(&self, command: Command) {
        // a thread that ended already reported every stream closed
        let _ = self.commands.send(command);
    }

    /// Add a video source node named `name` that offers `offer`; [`Notice::Node`] follows.
    pub fn open(&self, stream: StreamId, name: &str, offer: Offer) {
        self.send(Command::Open {
            stream,
            name: name.to_owned(),
            offer,
        });
    }

    /// The source is now `size`: the stream's buffers go and the consumer fixes a format again.
    pub fn resize(&self, stream: StreamId, size: Size) {
        self.send(Command::Resize { stream, size });
    }

    /// Ask for a buffer to fill; [`Notice::Dequeued`] answers.
    pub fn dequeue(&self, stream: StreamId) {
        self.send(Command::Dequeue(stream));
    }

    /// Hand the filled `buffer` to the consumer.
    pub fn queue(&self, stream: StreamId, buffer: BufferId, frame: Frame) {
        self.send(Command::Queue {
            stream,
            buffer,
            frame,
        });
    }

    /// Remove the stream's node.
    pub fn close(&self, stream: StreamId) {
        self.send(Command::Close(stream));
    }
}

impl Drop for Screencast {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
