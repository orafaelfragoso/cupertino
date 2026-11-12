//! Settings files written off the compositor's thread. A write is atomic and synced to disk, which can take long on a
//! busy disk; the change it records is applied at once, and the writes run on one thread, one after another in the
//! order they were asked, so the last change made is the one the file keeps.

use std::sync::mpsc;
use std::sync::OnceLock;

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

/// Run `job` on the settings writer's thread after the writes asked before it; on this thread when there is none.
pub fn store_later(job: impl FnOnce() + Send + 'static) {
    let job: Job = Box::new(job);
    match writer() {
        Some(writer) => {
            if let Err(mpsc::SendError(job)) = writer.send(job) {
                job();
            }
        }
        None => job(),
    }
}

/// Wait until every write asked before this call has finished.
pub fn flush() {
    let (done, finished) = mpsc::sync_channel(1);
    store_later(move || {
        let _ = done.send(());
    });
    let _ = finished.recv();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn writes_run_in_the_order_they_were_asked_and_flush_waits_for_them() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        for n in 0..100 {
            let seen = seen.clone();
            store_later(move || seen.lock().unwrap().push(n));
        }
        flush();
        assert_eq!(*seen.lock().unwrap(), (0..100).collect::<Vec<_>>());
    }
}
