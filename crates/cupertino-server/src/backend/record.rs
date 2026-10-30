//! `--record`: the first output of the headless backend as raw RGBA frames, 60 per second of wall time, for a video
//! encoder that reads the file (a FIFO, say). The clients animate on the wall clock, so the frames follow it too: frames
//! that come due while the encoder still takes the previous ones are not drawn but written as repeats of the next image
//! sent, those still due when the recording finishes as repeats of the last one, and the number of frames always matches
//! the time that passed.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::EventLoop;

use super::Backend;
use crate::state::Cupertino;

const FPS: u64 = 60;

/// The drawn frames that may wait for the encoder; while that many wait, due frames are not drawn.
const QUEUE: usize = 2;

/// How long opening a FIFO waits for its reader (an encoder that failed to start never opens it).
const READER_WAIT: Duration = Duration::from_secs(10);

/// A drawn frame (`None`: the last one again) and how many video frames it stands for.
type Frame = (Option<Vec<u8>>, u64);

/// The timer's side of a recording, which [`Recorder::finish`] ends.
struct Sending {
    /// `None` once the recording finished.
    send: Option<SyncSender<Frame>>,
    /// The video frames that came due since `started`.
    due: u64,
    /// The video frames that came due since the last image sent.
    unsent: u64,
}

impl Sending {
    /// Count the video frames due `elapsed` after the start.
    fn advance(&mut self, elapsed: Duration) {
        let now = (elapsed.as_secs_f64() * FPS as f64) as u64 + 1;
        self.unsent += now.saturating_sub(self.due);
        self.due = self.due.max(now);
    }
}

/// What a recording wrote: video frames, how many of them were drawn rather than repeated, and how many the time it
/// ran comes to.
#[derive(Debug, Clone, Copy)]
pub struct Written {
    pub frames: u64,
    pub drawn: u64,
    pub due: u64,
}

/// The running recording; [`Recorder::finish`] writes the frames still queued and those still due.
pub struct Recorder {
    sending: Rc<RefCell<Sending>>,
    started: Instant,
    writer: JoinHandle<Result<Written>>,
}

/// `path` created or truncated for writing; a FIFO once its reader opened it, within [`READER_WAIT`].
fn open(path: &Path) -> Result<File> {
    let end = Instant::now() + READER_WAIT;
    loop {
        let opened = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path);
        match opened {
            Ok(file) => {
                let flags = rustix::fs::fcntl_getfl(&file)?;
                rustix::fs::fcntl_setfl(&file, flags - rustix::fs::OFlags::NONBLOCK)?;
                return Ok(file);
            }
            // a FIFO nobody reads yet
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) && Instant::now() < end => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                return Err(anyhow!(
                    "{}: no reader opened it within {READER_WAIT:?}",
                    path.display()
                ));
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
        }
    }
}

/// Record output 0 into `path` from `started` on.
pub fn start(
    event_loop: &EventLoop<'static, Cupertino>,
    path: &Path,
    started: Instant,
) -> Result<Recorder> {
    let file = open(path)?;
    let queued = Arc::new(AtomicUsize::new(0));
    let written_out = queued.clone();
    let (send, receive) = sync_channel::<Frame>(QUEUE);
    let writer = std::thread::Builder::new()
        .name("record".into())
        .spawn(move || {
            let mut out = BufWriter::with_capacity(1 << 22, file);
            let mut written = Written {
                frames: 0,
                drawn: 0,
                due: 0,
            };
            let mut last = None;
            for (image, repeats) in receive {
                if image.is_some() {
                    last = image;
                    written.drawn += 1;
                    written_out.fetch_sub(1, Ordering::AcqRel);
                }
                let Some(image) = &last else { continue };
                for _ in 0..repeats {
                    out.write_all(image).context("writing a frame")?;
                }
                written.frames += repeats;
            }
            out.flush().context("writing a frame")?;
            Ok(written)
        })
        .context("starting the recording thread")?;
    let sending = Rc::new(RefCell::new(Sending {
        send: Some(send),
        due: 0,
        unsent: 0,
    }));
    let timer = sending.clone();
    let mut size = None;
    event_loop
        .handle()
        .insert_source(Timer::from_deadline(started), move |_, _, state| {
            let mut sending = timer.borrow_mut();
            let Some(send) = sending.send.clone() else {
                return TimeoutAction::Drop;
            };
            sending.advance(started.elapsed());
            let next = started + Duration::from_secs_f64(sending.due as f64 / FPS as f64);
            if queued.load(Ordering::Acquire) >= QUEUE {
                return TimeoutAction::ToInstant(next);
            }
            match draw(state) {
                Ok(image) if *size.get_or_insert(image.dimensions()) == image.dimensions() => {
                    queued.fetch_add(1, Ordering::AcqRel);
                    match send.try_send((Some(image.into_raw()), sending.unsent)) {
                        Ok(()) => sending.unsent = 0,
                        Err(TrySendError::Full(_)) => {
                            queued.fetch_sub(1, Ordering::AcqRel);
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            state.running = false;
                            state.loop_signal.stop();
                            return TimeoutAction::Drop;
                        }
                    }
                }
                Ok(image) => tracing::error!(
                    size = ?image.dimensions(),
                    recording = ?size,
                    "the output changed size; frame not recorded"
                ),
                Err(e) => tracing::error!(error = %e, "drawing a recorded frame failed"),
            }
            TimeoutAction::ToInstant(next)
        })
        .map_err(|e| anyhow!("inserting the recording timer: {e}"))?;
    Ok(Recorder {
        sending,
        started,
        writer,
    })
}

fn draw(state: &mut Cupertino) -> Result<image::RgbaImage> {
    let mut backend = state.backend.take().context("no backend")?;
    let image = match &mut backend {
        Backend::Headless(h) => state.draw_output_image(&mut h.renderer, 0, true, false),
        #[allow(unreachable_patterns)]
        _ => Err(anyhow!("recording needs the headless backend")),
    };
    state.backend = Some(backend);
    image
}

impl Recorder {
    /// Stop taking frames, send the ones due since the last image as repeats of it and wait until all are written.
    pub fn finish(self) -> Result<Written> {
        let mut sending = self.sending.borrow_mut();
        if let Some(send) = sending.send.take() {
            sending.advance(self.started.elapsed());
            // a writer that ended has its error in the join below
            let _ = send.send((None, sending.unsent));
        }
        let due = sending.due;
        drop(sending);
        let written = self
            .writer
            .join()
            .map_err(|_| anyhow!("the recording thread panicked"))??;
        Ok(Written { due, ..written })
    }
}
