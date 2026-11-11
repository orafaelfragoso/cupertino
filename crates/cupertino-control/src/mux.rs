//! The controller's loop: up to [`MAX_RUNS_PER_CONTROLLER`] local peers at once, each on a run of its own, on the one
//! compositor connection. Nothing waits for a peer or the compositor inside the loop: a request whose answer comes
//! later (a reply, a phase, a captured frame) is a pending operation with a deadline, and the loop sleeps until a
//! compositor event, a peer's command or the nearest deadline. Each peer has at most one command in flight.
#[cfg(test)]
#[path = "mux-tests.rs"]
mod tests;
use crate::capture::Pixels;
use crate::ipc::{self, Reply, Wake};
use crate::{base64, refusal, Request, MAX_WAIT_MS, REPLY_TIMEOUT};
use cupertino_control_model::MAX_RUNS_PER_CONTROLLER;
use cupertino_shell_client::ControlEvent;
use cupertino_wayland_client::{OutputId, OutputInfo};
use serde_json::{json, Value};
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Extra time an action's reply may take beyond [`REPLY_TIMEOUT`]: the compositor waits that long for the app.
const ACTION_GRACE: Duration = Duration::from_secs(1);

/// A run request the compositor answers with completed or refused.
pub enum Command {
    Start {
        output: OutputId,
        capabilities: u32,
    },
    Motion {
        revision: u32,
        x: f64,
        y: f64,
    },
    Button {
        revision: u32,
        code: u32,
        pressed: bool,
    },
    Key {
        revision: u32,
        code: u32,
        pressed: bool,
    },
    Axis {
        revision: u32,
        horizontal: f64,
        vertical: f64,
    },
    Observe,
    Perform(cupertino_semantics::Invocation),
    Resume,
}

/// The compositor as the loop uses it, so the peers' multiplexing is tested without a display.
pub trait Desktop {
    type Session;
    type Frame;
    /// How many runs the compositor allows on this connection at once.
    fn capacity(&self) -> usize;
    fn open(&mut self) -> Self::Session;
    /// The run's epoch, zero until the compositor named it.
    fn epoch(&self, session: &Self::Session) -> u64;
    /// Name the peer's process to the compositor when it takes names.
    fn identify(&mut self, session: &Self::Session, peer: Option<(u32, u64)>);
    fn send(
        &mut self,
        session: &Self::Session,
        serial: u32,
        command: Command,
    ) -> Result<(), String>;
    fn stop(&mut self, session: &Self::Session);
    /// End the session for good.
    fn close(&mut self, session: Self::Session);
    fn outputs(&self) -> Vec<OutputInfo>;
    /// Start capturing `output` for the run of `session`.
    fn capture(&mut self, session: &Self::Session, output: OutputId)
        -> Result<Self::Frame, String>;
    /// Whether the frame's constraints arrived, it is ready, or it failed.
    fn progress(&self, frame: &Self::Frame) -> (bool, bool, bool);
    /// Attach a buffer once the constraints arrived and ask for the frame.
    fn prepare(&mut self, frame: &mut Self::Frame) -> Result<(), String>;
    fn pixels(&mut self, frame: Self::Frame) -> Result<Pixels, String>;
    /// Wait for compositor events until `timeout` (`None` waits for an event or a wake) and return the control events.
    fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<ControlEvent>, String>;
}

/// What the compositor told a run.
#[derive(Default)]
struct Remote {
    epoch: u64,
    available: u32,
    phase: u32,
    revision: u32,
    output: Option<OutputId>,
    geometry: Value,
    reply: Option<(u32, Result<(), u32>)>,
    observation: Option<(u32, cupertino_semantics::Observation)>,
    action_reply: Option<(u32, cupertino_semantics::ActionReply)>,
}
impl Remote {
    /// Take the events of the run whose epoch is `epoch`.
    fn observe(&mut self, events: &[ControlEvent], epoch: u64) {
        if epoch == 0 {
            return;
        }
        for event in events {
            match event {
                ControlEvent::Created {
                    epoch: e,
                    available,
                } if *e == epoch => {
                    self.epoch = *e;
                    self.available = *available;
                }
                ControlEvent::Status {
                    epoch: e, phase, ..
                } if *e == epoch => self.phase = *phase,
                ControlEvent::Geometry {
                    epoch: e,
                    revision,
                    x,
                    y,
                    width,
                    height,
                    scale,
                    name,
                } if *e == epoch => {
                    self.revision = *revision;
                    self.geometry = json!({"revision":revision,"x":x,"y":y,"width":width,"height":height,"scale":scale,"name":name});
                }
                ControlEvent::Observation {
                    epoch: e,
                    serial,
                    data,
                } if *e == epoch => self.observation = Some((*serial, data.clone())),
                ControlEvent::ActionFinished {
                    epoch: e,
                    serial,
                    reply,
                } if *e == epoch => self.action_reply = Some((*serial, reply.clone())),
                ControlEvent::Completed { epoch: e, serial } if *e == epoch => {
                    self.reply = Some((*serial, Ok(())))
                }
                ControlEvent::Refused {
                    epoch: e,
                    serial,
                    reason,
                } if *e == epoch => self.reply = Some((*serial, Err(*reason))),
                _ => {}
            }
        }
    }
    fn snapshot(&self) -> Value {
        json!({"epoch":self.epoch,"available":self.available,"phase":self.phase,"geometry":self.geometry})
    }
}

/// What a peer's command waits for.
enum Pending<F> {
    /// The compositor's reply to request `serial`, then the response `then` makes of it.
    Reply {
        serial: u32,
        deadline: Instant,
        then: Then,
    },
    /// The run's phase becoming `phase`.
    Phase { phase: u32, deadline: Instant },
    /// A frame of the granted output, taken while the output keeps `layout`.
    Capture {
        frame: F,
        prepared: bool,
        layout: (Value, OutputInfo),
        deadline: Instant,
    },
}
impl<F> Pending<F> {
    fn deadline(&self) -> Instant {
        match self {
            Self::Reply { deadline, .. }
            | Self::Phase { deadline, .. }
            | Self::Capture { deadline, .. } => *deadline,
        }
    }
}
/// The response a completed request makes.
enum Then {
    Snapshot,
    Observation,
    Action,
    Started(OutputId),
}
/// Where a pending operation stands.
enum Progress<F> {
    Waiting(Pending<F>),
    Done(Reply),
}

struct Peer<D: Desktop> {
    inbox: ipc::Inbox,
    session: D::Session,
    remote: Remote,
    next: u32,
    hello: bool,
    /// Until the compositor named the run's epoch, by this deadline.
    opening: Option<Instant>,
    pending: Option<Pending<D::Frame>>,
}
impl<D: Desktop> Peer<D> {
    fn deadline(&self) -> Option<Instant> {
        self.opening
            .or_else(|| self.pending.as_ref().map(Pending::deadline))
    }
    fn serial(&mut self) -> Result<u32, String> {
        let serial = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("request identifiers exhausted")?;
        self.remote.reply = None;
        Ok(serial)
    }
    /// The output geometry a capture keeps, and the output's own metadata, which also covers transforms and pixel modes.
    // Semantic publications and window animation advance the input token without changing the captured output.
    fn capture_layout(&self, desktop: &D) -> Result<(Value, OutputInfo), String> {
        let mut geometry = self.remote.geometry.clone();
        geometry
            .as_object_mut()
            .ok_or("output geometry is unavailable")?
            .remove("revision");
        let output = desktop
            .outputs()
            .into_iter()
            .find(|o| Some(o.id) == self.remote.output)
            .ok_or("output disappeared during capture")?;
        Ok((geometry, output))
    }
}

/// The error a peer beyond the bound reads at once.
pub fn turned_away(capacity: usize) -> String {
    format!(
        "this session's desktop controller already serves {capacity} local clients at once; try again once one of them disconnects"
    )
}

/// Serve the peers `incoming` hands over until the compositor connection or the listener ends.
pub fn run<D: Desktop>(
    desktop: &mut D,
    incoming: &Receiver<io::Result<UnixStream>>,
    wake: &Wake,
) -> Result<(), String> {
    let capacity = desktop.capacity().min(MAX_RUNS_PER_CONTROLLER);
    let mut peers: Vec<Peer<D>> = Vec::new();
    loop {
        loop {
            match incoming.try_recv() {
                Ok(stream) => {
                    let stream = stream.map_err(|e| e.to_string())?;
                    if peers.len() >= capacity {
                        ipc::refuse(stream, &turned_away(capacity));
                        continue;
                    }
                    let Ok(inbox) = ipc::read_peer(&stream, wake.clone()) else {
                        continue;
                    };
                    peers.push(Peer {
                        inbox,
                        session: desktop.open(),
                        remote: Remote::default(),
                        next: 1,
                        hello: false,
                        opening: Some(Instant::now() + REPLY_TIMEOUT),
                        pending: None,
                    });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err("local listener ended".into()),
            }
        }
        let mut i = 0;
        while i < peers.len() {
            if step(desktop, &mut peers[i]).is_ok() {
                i += 1;
                continue;
            }
            // a disconnected or failed peer ends its own run only
            let peer = peers.swap_remove(i);
            desktop.stop(&peer.session);
            drop(peer.pending);
            desktop.close(peer.session);
        }
        let now = Instant::now();
        let timeout = peers
            .iter()
            .filter_map(Peer::deadline)
            .min()
            .map(|at| at.saturating_duration_since(now));
        let events = desktop.wait(timeout)?;
        for peer in &mut peers {
            let epoch = desktop.epoch(&peer.session);
            peer.remote.observe(&events, epoch);
        }
    }
}

/// Move one peer on: name its run once the epoch is known, settle its pending operation, then take its next command.
/// An error disconnects the peer.
fn step<D: Desktop>(desktop: &mut D, peer: &mut Peer<D>) -> Result<(), String> {
    if peer.inbox.closed.load(Ordering::SeqCst) {
        return Err("local client disconnected".into());
    }
    if let Some(deadline) = peer.opening {
        if peer.remote.epoch == 0 {
            return if Instant::now() >= deadline {
                Err("the run epoch did not arrive".into())
            } else {
                Ok(())
            };
        }
        peer.opening = None;
        // without a name the compositor refuses the run: it never decides for an app it cannot tell
        desktop.identify(&peer.session, peer.inbox.peer);
    }
    loop {
        if let Some(pending) = peer.pending.take() {
            match advance(desktop, peer, pending) {
                Progress::Waiting(pending) => {
                    peer.pending = Some(pending);
                    return Ok(());
                }
                Progress::Done(reply) => peer.inbox.respond(reply)?,
            }
        }
        let Ok(line) = peer.inbox.commands.try_recv() else {
            return Ok(());
        };
        let request = match serde_json::from_slice::<Request>(&line) {
            Ok(Request::Hello { version: 1 }) => {
                peer.hello = true;
                Ok(Request::Hello { version: 1 })
            }
            Ok(_) if !peer.hello => Err("local protocol handshake required".to_owned()),
            Ok(request) => Ok(request),
            Err(_) => Err("invalid local command".to_owned()),
        };
        match request.and_then(|r| execute(desktop, peer, r)) {
            Ok(Progress::Waiting(pending)) => peer.pending = Some(pending),
            Ok(Progress::Done(reply)) => peer.inbox.respond(reply)?,
            Err(error) => peer.inbox.respond(Reply::Now(Err(error)))?,
        }
    }
}

/// Answer `request` now, or start what it waits for.
fn execute<D: Desktop>(
    desktop: &mut D,
    peer: &mut Peer<D>,
    request: Request,
) -> Result<Progress<D::Frame>, String> {
    let done = |value: Value| Ok(Progress::Done(Reply::Now(Ok(value))));
    let reply = |serial, then, timeout| {
        Ok(Progress::Waiting(Pending::Reply {
            serial,
            deadline: Instant::now() + timeout,
            then,
        }))
    };
    match request {
        Request::Hello { version: 1 } => done(
            json!({"schema":"cupertino.control.v1","protocol":3,"state":peer.remote.snapshot()}),
        ),
        Request::Hello { .. } => Err("unsupported local protocol version".into()),
        Request::Status => {
            let mut state = peer.remote.snapshot();
            state["outputs"] = json!(desktop
                .outputs()
                .iter()
                .filter_map(|o| o.name.clone())
                .collect::<Vec<_>>());
            done(state)
        }
        Request::Observe => {
            let serial = peer.serial()?;
            desktop.send(&peer.session, serial, Command::Observe)?;
            reply(serial, Then::Observation, REPLY_TIMEOUT)
        }
        Request::Action {
            window,
            revision,
            node,
            action,
            data,
        } => {
            let serial = peer.serial()?;
            let invocation = cupertino_semantics::Invocation {
                window,
                revision,
                node,
                action,
                data,
            };
            desktop.send(&peer.session, serial, Command::Perform(invocation))?;
            reply(serial, Then::Action, REPLY_TIMEOUT + ACTION_GRACE)
        }
        Request::Start {
            output,
            capabilities,
        } => {
            let output = desktop
                .outputs()
                .into_iter()
                .find(|o| o.name.as_deref() == Some(output.as_str()))
                .ok_or("output is not present")?
                .id;
            let serial = peer.serial()?;
            desktop.send(
                &peer.session,
                serial,
                Command::Start {
                    output,
                    capabilities,
                },
            )?;
            reply(serial, Then::Started(output), REPLY_TIMEOUT)
        }
        Request::Wait { phase, timeout_ms } => {
            if timeout_ms > MAX_WAIT_MS {
                return Err("state wait exceeds deadline limit".into());
            }
            Ok(Progress::Waiting(Pending::Phase {
                phase,
                deadline: Instant::now() + Duration::from_millis(timeout_ms),
            }))
        }
        Request::Stop => {
            desktop.stop(&peer.session);
            Ok(Progress::Waiting(Pending::Phase {
                phase: 4,
                deadline: Instant::now() + REPLY_TIMEOUT,
            }))
        }
        Request::Capture => {
            if peer.remote.phase != 2 {
                return Err("desktop control is not active".into());
            }
            let layout = peer.capture_layout(desktop)?;
            let output = peer.remote.output.ok_or("no granted output")?;
            let frame = desktop.capture(&peer.session, output)?;
            Ok(Progress::Waiting(Pending::Capture {
                frame,
                prepared: false,
                layout,
                deadline: Instant::now() + REPLY_TIMEOUT,
            }))
        }
        Request::Resume => {
            let serial = peer.serial()?;
            desktop.send(&peer.session, serial, Command::Resume)?;
            reply(serial, Then::Snapshot, REPLY_TIMEOUT)
        }
        Request::Move { x, y, revision } => {
            let revision = revision.unwrap_or(peer.remote.revision);
            let serial = peer.serial()?;
            desktop.send(&peer.session, serial, Command::Motion { revision, x, y })?;
            reply(serial, Then::Snapshot, REPLY_TIMEOUT)
        }
        Request::Button {
            code,
            pressed,
            revision,
        } => {
            let revision = revision.unwrap_or(peer.remote.revision);
            let serial = peer.serial()?;
            let command = Command::Button {
                revision,
                code,
                pressed,
            };
            desktop.send(&peer.session, serial, command)?;
            reply(serial, Then::Snapshot, REPLY_TIMEOUT)
        }
        Request::Key { code, pressed } => {
            let revision = peer.remote.revision;
            let serial = peer.serial()?;
            let command = Command::Key {
                revision,
                code,
                pressed,
            };
            desktop.send(&peer.session, serial, command)?;
            reply(serial, Then::Snapshot, REPLY_TIMEOUT)
        }
        Request::Scroll {
            horizontal,
            vertical,
            revision,
        } => {
            let revision = revision.unwrap_or(peer.remote.revision);
            let serial = peer.serial()?;
            let command = Command::Axis {
                revision,
                horizontal,
                vertical,
            };
            desktop.send(&peer.session, serial, command)?;
            reply(serial, Then::Snapshot, REPLY_TIMEOUT)
        }
    }
}

/// Settle `pending` when what it waits for arrived or its deadline passed.
fn advance<D: Desktop>(
    desktop: &mut D,
    peer: &mut Peer<D>,
    pending: Pending<D::Frame>,
) -> Progress<D::Frame> {
    let failed = |error: &str| Progress::Done(Reply::Now(Err(error.to_owned())));
    let late = Instant::now() >= pending.deadline();
    match pending {
        Pending::Reply {
            serial,
            deadline,
            then,
        } => {
            let Some((_, result)) = peer.remote.reply.filter(|(s, _)| *s == serial) else {
                return if late {
                    failed("operation timed out")
                } else {
                    Progress::Waiting(Pending::Reply {
                        serial,
                        deadline,
                        then,
                    })
                };
            };
            if let Err(code) = result {
                return Progress::Done(Reply::Now(Err(refusal(code))));
            }
            let remote = &mut peer.remote;
            let value = match then {
                Then::Snapshot => Ok(remote.snapshot()),
                Then::Started(output) => {
                    remote.output = Some(output);
                    Ok(remote.snapshot())
                }
                Then::Observation => remote
                    .observation
                    .as_ref()
                    .filter(|(s, _)| *s == serial)
                    .map(|(_, o)| serde_json::to_value(o).unwrap())
                    .ok_or("observation did not arrive".into()),
                Then::Action => remote
                    .action_reply
                    .as_ref()
                    .filter(|(s, _)| *s == serial)
                    .ok_or_else(|| "action outcome is unknown; observe before retrying".to_owned())
                    .and_then(|(_, r)| {
                        serde_json::to_value(r).map_err(|_| "invalid semantic result".into())
                    }),
            };
            Progress::Done(Reply::Now(value))
        }
        Pending::Phase { phase, deadline } => {
            if peer.remote.phase == phase {
                Progress::Done(Reply::Now(Ok(peer.remote.snapshot())))
            } else if late {
                failed("operation timed out")
            } else {
                Progress::Waiting(Pending::Phase { phase, deadline })
            }
        }
        Pending::Capture {
            mut frame,
            mut prepared,
            layout,
            deadline,
        } => {
            let (constraints, ready, broken) = desktop.progress(&frame);
            if broken {
                return failed(if prepared {
                    "capture grant ended"
                } else {
                    "capture grant is unavailable"
                });
            }
            if constraints && !prepared {
                if let Err(error) = desktop.prepare(&mut frame) {
                    return failed(&error);
                }
                prepared = true;
            }
            if !ready {
                return if late {
                    failed("capture timed out")
                } else {
                    Progress::Waiting(Pending::Capture {
                        frame,
                        prepared,
                        layout,
                        deadline,
                    })
                };
            }
            if peer.remote.phase != 2 {
                return failed("capture grant ended");
            }
            match peer.capture_layout(desktop) {
                Ok(now) if now == layout => {}
                Ok(_) => return failed("output geometry changed during capture; observe again"),
                Err(error) => return failed(&error),
            }
            let pixels = match desktop.pixels(frame) {
                Ok(pixels) => pixels,
                Err(error) => return failed(&error),
            };
            let geometry = peer.remote.geometry.clone();
            Progress::Done(Reply::Later(Box::new(move || {
                let (w, h, png) = pixels.png()?;
                Ok(
                    json!({"mime_type":"image/png","width":w,"height":h,"geometry":geometry,"data":base64(&png)}),
                )
            })))
        }
    }
}
