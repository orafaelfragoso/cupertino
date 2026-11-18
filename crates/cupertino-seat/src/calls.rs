//! One ordered queue on the compositor's private controller connection. Only the worker enters
//! blocking zbus calls: its method timeout bounds a reply, not the preceding socket write or
//! authentication. The loop waits up to its own deadline, including time spent in the queue.

use std::collections::HashSet;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::time::{Duration, Instant};

use cupertino_devices::DevT;
use smithay::reexports::calloop::channel::Sender as LoopSender;
use zbus::blocking::{connection::Builder, Connection};

use crate::logind::SessionLookup;
use crate::session::{listen, Delivery};
use crate::Error;

/// The compositor's bus patience for a single operation, including giving up control on exit.
const CALL_WAIT: Duration = Duration::from_secs(2);
/// Patience for connecting, finding the session, subscribing and acquiring control together.
const START_WAIT: Duration = Duration::from_secs(10);

type Answer<T> = SyncSender<Result<T, Error>>;

enum Request {
    Take(DevT, Instant, Answer<(OwnedFd, bool)>),
    Release(DevT, bool, Answer<()>),
    Complete(DevT, Answer<()>),
    Switch(u32, Instant, Answer<()>),
}

pub(crate) struct Calls {
    requests: Option<Sender<Request>>,
    stopped: Receiver<()>,
}

fn timeout(what: &'static str) -> Error {
    Error::bus(what)(
        io::Error::new(io::ErrorKind::TimedOut, "logind did not answer in time").into(),
    )
}

fn wait<T>(rx: Receiver<Result<T, Error>>, until: Instant, what: &'static str) -> Result<T, Error> {
    rx.recv_timeout(until.saturating_duration_since(Instant::now()))
        .map_err(|error| match error {
            RecvTimeoutError::Timeout => timeout(what),
            RecvTimeoutError::Disconnected => Error::SessionLost,
        })?
}

impl Calls {
    pub(crate) fn start(
        lookup: SessionLookup,
        events: LoopSender<Delivery>,
    ) -> Result<(Self, bool, String), Error> {
        let until = Instant::now() + START_WAIT;
        let (requests, queue) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(0);
        let (done, stopped) = mpsc::channel();
        std::thread::Builder::new()
            .name("cupertino-seat-calls".into())
            .spawn(move || {
                match Builder::system().and_then(|b| b.method_timeout(CALL_WAIT).build()) {
                    Ok(conn) => {
                        serve(&conn, lookup, events, ready, queue);
                        // The signal readers also own this connection. Even a lost TakeControl or
                        // ReleaseControl reply must not keep the session or its devices claimed.
                        if let Err(error) = conn.close() {
                            tracing::warn!(%error, "closing the logind controller connection");
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(Error::bus("connecting to the system bus")(error)));
                    }
                }
                let _ = done.send(());
            })
            .map_err(Error::Thread)?;
        let (active, seat) = wait(started, until, "starting the logind session")?;
        Ok((
            Self {
                requests: Some(requests),
                stopped,
            },
            active,
            seat,
        ))
    }

    fn ask<T>(
        &self,
        what: &'static str,
        request: impl FnOnce(Instant, Answer<T>) -> Request,
    ) -> Result<T, Error> {
        let until = Instant::now() + CALL_WAIT;
        // Rendezvous, not a buffered reply: a successful send means the loop took ownership.
        let (answer, rx) = mpsc::sync_channel(0);
        self.requests
            .as_ref()
            .ok_or(Error::SessionLost)?
            .send(request(until, answer))
            .map_err(|_| Error::SessionLost)?;
        wait(rx, until, what)
    }

    pub(crate) fn take_device(&self, dev: DevT) -> Result<(OwnedFd, bool), Error> {
        self.ask("TakeDevice", |until, answer| {
            Request::Take(dev, until, answer)
        })
    }

    pub(crate) fn release_device(&self, dev: DevT, retry: bool) -> Result<(), Error> {
        self.ask("ReleaseDevice", |_, answer| {
            Request::Release(dev, retry, answer)
        })
    }

    pub(crate) fn pause_device_complete(&self, dev: DevT) -> Result<(), Error> {
        self.ask("PauseDeviceComplete", |_, answer| {
            Request::Complete(dev, answer)
        })
    }

    pub(crate) fn switch_to(&self, vt: u32) -> Result<(), Error> {
        self.ask("SwitchTo", |until, answer| {
            Request::Switch(vt, until, answer)
        })
    }
}

impl Drop for Calls {
    fn drop(&mut self) {
        drop(self.requests.take());
        if self.stopped.recv_timeout(CALL_WAIT) == Err(RecvTimeoutError::Timeout) {
            tracing::warn!("logind teardown exceeded its deadline; finishing on the seat worker");
        }
    }
}

fn serve(
    conn: &Connection,
    lookup: SessionLookup,
    events: LoopSender<Delivery>,
    ready: Answer<(bool, String)>,
    queue: Receiver<Request>,
) {
    let (logind, active, seat) = match listen(conn, &lookup, events) {
        Ok(started) => started,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok((active, seat))).is_err() {
        return;
    }

    // Only abandoned takes whose compensating release failed. Retry before another TakeDevice;
    // dropping control also releases these if the loop never opens the device again.
    let mut lost = HashSet::new();
    for request in queue {
        match request {
            Request::Take(dev, until, answer) => {
                if lost.contains(&dev) {
                    if let Err(error) = logind.reconcile_release(dev) {
                        let _ = answer.send(Err(error));
                        continue;
                    }
                    lost.remove(&dev);
                }
                if Instant::now() >= until {
                    let _ = answer.send(Err(timeout("TakeDevice")));
                    continue;
                }
                let result = logind.take_device(dev);
                let uncertain = result.as_ref().is_err_and(Error::unanswered);
                let abandoned = answer.send(result).is_err_and(|unsent| unsent.0.is_ok());
                if uncertain || abandoned {
                    if let Err(error) = logind.reconcile_release(dev) {
                        tracing::warn!(%dev, %error, "releasing an unanswered TakeDevice; will retry before opening");
                        lost.insert(dev);
                    }
                }
            }
            Request::Release(dev, retry, answer) => {
                let _ = answer.send(if retry {
                    logind.reconcile_release(dev)
                } else {
                    logind.release_device(dev)
                });
            }
            Request::Complete(dev, answer) => {
                let _ = answer.send(logind.pause_device_complete(dev));
            }
            Request::Switch(vt, until, answer) => {
                let _ = answer.send(if Instant::now() < until {
                    logind.switch_to(vt)
                } else {
                    Err(timeout("SwitchTo"))
                });
            }
        }
    }
}
