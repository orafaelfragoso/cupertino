//! Settings files written off the compositor's thread. A write is atomic and synced to disk, which can take long on a
//! busy disk; the change it records is applied at once, and the writes run on one thread, one after another in the
//! order they were asked, so the last change made is the one the file keeps.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

fn writer() -> Option<&'static mpsc::Sender<Job>> {
    static WRITER: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (send, receive) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("settings-store".into())
                .spawn(move || {
                    for job in receive {
                        job();
                    }
                })
                .ok()
                .map(|_| send)
        })
        .as_ref()
}

/// Run `job` after earlier writes. Without the settings writer the change lasts for this session only.
pub fn store_later(job: impl FnOnce() + Send + 'static) {
    if !writer().is_some_and(|writer| writer.send(Box::new(job)).is_ok()) {
        tracing::error!(
            "the settings writer is unavailable: the change lasts until the server stops"
        );
    }
}

/// How long a flush waits before leaving remaining writes to the settings writer.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

/// Wait for earlier writes for up to two seconds. A slow write continues in the background.
pub fn flush() {
    let (done, finished) = mpsc::sync_channel(1);
    store_later(move || {
        let _ = done.send(());
    });
    if finished.recv_timeout(FLUSH_WAIT) == Err(mpsc::RecvTimeoutError::Timeout) {
        tracing::warn!("settings writes have not finished by the flush deadline");
    }
}
