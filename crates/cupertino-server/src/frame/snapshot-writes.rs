//! Scripted output is sampled by the compositor. One writer owns the samples in arrival order, including writes
//! to the same path; the compositor never waits for it while serving clients.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use image::{ImageFormat, RgbaImage};

/// Number of snapshots the worker can hold besides the one it is writing.
const QUEUE: usize = 4;
/// Largest retained sample (room for an 8K RGBA output); all paths of an output share the one image.
pub(crate) const SAMPLE_BYTES: usize = 128 * 1024 * 1024;
/// Time allowed for a FIFO reader to open its end or to make progress writing the file.
const OUTPUT_WAIT: Duration = Duration::from_secs(3);
/// Interval between attempts to open a FIFO with no reader.
const OPEN_RETRY: Duration = Duration::from_millis(20);
/// Longest shutdown waits for all accepted writes, including filesystem calls that ignore O_NONBLOCK.
const DRAIN_WAIT: Duration = Duration::from_secs(10);

enum Sample {
    Image(Vec<PathBuf>, RgbaImage),
    List(PathBuf, String),
}

struct Worker {
    send: SyncSender<Sample>,
    done: Receiver<Result<()>>,
}

#[derive(Default)]
pub(crate) struct Writes {
    worker: Option<Worker>,
    error: Option<anyhow::Error>,
}

impl Writes {
    pub(crate) fn failed(&mut self, error: anyhow::Error) {
        tracing::error!(%error, "scripted output failed");
        self.error.get_or_insert(error);
    }

    pub(crate) fn image(&mut self, paths: Vec<PathBuf>, image: RgbaImage) {
        if image.len() > SAMPLE_BYTES {
            return self.failed(anyhow!("screenshot exceeds {SAMPLE_BYTES} bytes"));
        }
        self.queue(Sample::Image(paths, image));
    }

    pub(crate) fn window_list(&mut self, path: PathBuf, list: String) {
        self.queue(Sample::List(path, list));
    }

    fn queue(&mut self, sample: Sample) {
        if self.worker.is_none() {
            match Worker::start() {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => return self.failed(e.into()),
            }
        }
        let worker = self.worker.as_ref().expect("started above");
        if let Err(e) = worker.send.try_send(sample) {
            self.failed(anyhow!("snapshot not queued: {e}"));
        }
    }

    /// Called only after the backend loop exits. Dropping the sender is the end marker, even for a full queue.
    pub(crate) fn finish(&mut self) -> Result<()> {
        if let Some(Worker { send, done }) = self.worker.take() {
            drop(send);
            done.recv_timeout(DRAIN_WAIT)
                .context("snapshot writer did not finish")??;
        }
        self.error.take().map_or(Ok(()), Err)
    }
}

impl Worker {
    fn start() -> io::Result<Self> {
        let (send, receive) = mpsc::sync_channel(QUEUE);
        let (finished, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("snapshot-writes".into())
            .spawn(move || {
                let mut first_error = None;
                let mut record = |path: &Path, result: Result<()>| match result {
                    Ok(()) => tracing::info!(path = %path.display(), "scripted output written"),
                    Err(error) => {
                        let error = error.context(format!("writing {}", path.display()));
                        tracing::error!(error = %format!("{error:#}"), "scripted output failed");
                        first_error.get_or_insert(error);
                    }
                };
                for sample in receive {
                    match sample {
                        Sample::Image(paths, image) => {
                            for path in paths {
                                record(&path, write_image(&path, &image));
                            }
                        }
                        Sample::List(path, list) => record(
                            &path,
                            (|| {
                                Output::open(&path)?.write_all(list.as_bytes())?;
                                Ok(())
                            })(),
                        ),
                    }
                }
                let _ = finished.send(first_error.map_or(Ok(()), Err));
            })?;
        Ok(Self { send, done })
    }
}

fn write_image(path: &Path, image: &RgbaImage) -> Result<()> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let format = ImageFormat::from_path(path)?;
    let mut out = BufWriter::new(Output::open(path)?);
    image.write_to(&mut out, format)?;
    out.flush()?;
    Ok(())
}

/// Nonblocking pipe writes. Regular files can still block in the kernel: the in-flight worker and sample count
/// stay bounded in that case, and the process reports an error at its drain deadline.
struct Output {
    file: File,
    end: Instant,
}

impl Output {
    fn open(path: &Path) -> io::Result<Self> {
        let end = Instant::now() + OUTPUT_WAIT;
        loop {
            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
            {
                Ok(file) => return Ok(Self { file, end }),
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                    let left = remaining(end)?;
                    std::thread::sleep(OPEN_RETRY.min(left));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn remaining(end: Instant) -> io::Result<Duration> {
    end.checked_duration_since(Instant::now())
        .ok_or(io::ErrorKind::TimedOut.into())
}

impl Write for Output {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        loop {
            match self.file.write(data) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let timeout =
                        Timespec::try_from(remaining(self.end)?).expect("short write deadline");
                    let mut fds = [PollFd::new(&self.file, PollFlags::OUT)];
                    match poll(&mut fds, Some(&timeout)) {
                        Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
                        Ok(_) | Err(rustix::io::Errno::INTR) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Output {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}
