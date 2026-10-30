//! `--record`: the first output of the headless backend as raw RGBA frames, 60 per second of wall time, for a video
//! encoder that reads the file (a FIFO, say). The clients animate on the wall clock, so the frames follow it too: a frame
//! that comes due while the encoder still takes the previous ones repeats the last image instead of being drawn, and the
//! number of frames always matches the time that passed.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::EventLoop;

use super::Backend;
use crate::state::Cupertino;

const FPS: u64 = 60;

/// One drawn frame and how many video frames it stands for.
type Frame = (Vec<u8>, u64);

/// What a recording wrote: video frames, and how many of them were drawn rather than repeated.
#[derive(Debug, Clone, Copy)]
pub struct Written {
    pub frames: u64,
    pub drawn: u64,
}

/// The running recording; [`Recorder::finish`] writes the frames still queued.
pub struct Recorder {
    frames: Rc<RefCell<Option<SyncSender<Frame>>>>,
    writer: JoinHandle<Result<Written>>,
}

/// Record output 0 into `path` from `after` on. Opening a FIFO waits for its reader.
pub fn start(
    event_loop: &EventLoop<'static, Cupertino>,
    path: &Path,
    after: Duration,
) -> Result<Recorder> {
    let file = File::create(path).with_context(|| format!("opening {}", path.display()))?;
    let (send, receive) = sync_channel::<Frame>(2);
    let writer = std::thread::Builder::new()
        .name("record".into())
        .spawn(move || {
            let mut out = BufWriter::with_capacity(1 << 22, file);
            let mut written = Written {
                frames: 0,
                drawn: 0,
            };
            for (image, repeats) in receive {
                for _ in 0..repeats {
                    out.write_all(&image).context("writing a frame")?;
                }
                written.frames += repeats;
                written.drawn += 1;
            }
            out.flush().context("writing a frame")?;
            Ok(written)
        })
        .context("starting the recording thread")?;
    let frames = Rc::new(RefCell::new(Some(send)));
    let sender = frames.clone();
    let started = Instant::now() + after;
    let mut due = 0;
    let mut unsent = 0;
    let mut size = None;
    event_loop
        .handle()
        .insert_source(Timer::from_deadline(started), move |_, _, state| {
            let Some(send) = sender.borrow().clone() else {
                return TimeoutAction::Drop;
            };
            let now = (started.elapsed().as_secs_f64() * FPS as f64) as u64 + 1;
            unsent += now - due;
            due = now;
            match draw(state) {
                Ok(image) if *size.get_or_insert(image.dimensions()) == image.dimensions() => {
                    match send.try_send((image.into_raw(), unsent)) {
                        Ok(()) => unsent = 0,
                        Err(TrySendError::Full(_)) => {}
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
            TimeoutAction::ToInstant(started + Duration::from_secs_f64(due as f64 / FPS as f64))
        })
        .map_err(|e| anyhow!("inserting the recording timer: {e}"))?;
    Ok(Recorder { frames, writer })
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
    /// Stop taking frames and wait until the queued ones are written.
    pub fn finish(self) -> Result<Written> {
        self.frames.borrow_mut().take();
        self.writer
            .join()
            .map_err(|_| anyhow!("the recording thread panicked"))?
    }
}
