//! Native control and consent on the existing transport queue. The controller never binds its own authority.
use cupertino_protocols::client::control::cupertino_control_authority_v1::{
    self as authority, CupertinoControlAuthorityV1,
};
use cupertino_protocols::client::control::cupertino_control_session_v1::{
    self as session, CupertinoControlSessionV1,
};
use cupertino_protocols::client::control::cupertino_control_v1::CupertinoControlV1;
use cupertino_wayland_client::{Connection, OutputId, State, SurfaceId};
use std::os::fd::AsFd;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle, WEnum};

#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    Created {
        epoch: u64,
        available: u32,
    },
    Status {
        epoch: u64,
        serial: u32,
        phase: u32,
    },
    Geometry {
        epoch: u64,
        revision: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale: f64,
        name: String,
    },
    Completed {
        epoch: u64,
        serial: u32,
    },
    Refused {
        epoch: u64,
        serial: u32,
        reason: u32,
    },
    Observation {
        epoch: u64,
        serial: u32,
        data: cupertino_semantics::Observation,
    },
    ActionFinished {
        epoch: u64,
        serial: u32,
        reply: cupertino_semantics::ActionReply,
    },
    SemanticChanged {
        epoch: u64,
        revision: u64,
    },
    ConsentRequested {
        epoch: u64,
        capabilities: u32,
        output: String,
        resuming: bool,
    },
    /// The app the next [`ControlEvent::ConsentRequested`] of `epoch` asks for (authority version 2).
    ConsentApp {
        epoch: u64,
        app: ConsentApp,
    },
    ConsentChanged {
        epoch: u64,
        phase: u32,
    },
}
/// The app a consent request names, as the compositor found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentApp {
    /// The desktop entry id (`desktop`), else the absolute path of the executable.
    pub identity: String,
    /// Whether `identity` is a desktop entry id.
    pub desktop: bool,
    /// The entry's name, or the executable's file name.
    pub name: String,
    /// The services asked: desktop control 1, screen capture 2.
    pub services: u32,
    /// Whether the answer is remembered for the app.
    pub remembered: bool,
}

/// The person's answer on the consent panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentAnswer {
    /// Allow the run, and remember allow for the app.
    Allow,
    /// Refuse the run, and remember deny for the app.
    Deny,
    /// Refuse this run only: the panel was dismissed or could not be shown.
    Dismiss,
}

struct Quiet;
struct RunData(Arc<AtomicU64>);
pub struct DesktopControl(CupertinoControlV1);
pub struct ControlSession {
    proxy: CupertinoControlSessionV1,
    epoch: Arc<AtomicU64>,
}
pub struct ControlAuthority(CupertinoControlAuthorityV1);
impl DesktopControl {
    /// Version 3 runs are asked every time; version 4 runs name their app ([`ControlSession::identify`]).
    pub fn bind(conn: &mut Connection) -> Option<Self> {
        conn.bind_extension(3..=4, Quiet).map(Self)
    }
    pub fn session(&self, conn: &Connection) -> ControlSession {
        let epoch = Arc::new(AtomicU64::new(0));
        let proxy = self
            .0
            .get_session(&conn.queue_handle(), RunData(epoch.clone()));
        ControlSession { proxy, epoch }
    }
}
impl Drop for DesktopControl {
    fn drop(&mut self) {
        self.0.destroy();
    }
}
impl ControlSession {
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
    }
    /// Whether the compositor takes the app a run comes from: version 4 needs it before `start`.
    pub fn names_apps(&self) -> bool {
        use wayland_client::Proxy;
        self.proxy.version() >= 4
    }
    /// Name the local process this run serves: its id and start time in clock ticks since boot. Only before `start`,
    /// once, and only when [`Self::names_apps`].
    pub fn identify(&self, pid: u32, start: u64) {
        self.proxy.identify(pid, (start >> 32) as u32, start as u32);
    }
    fn parts(&self) -> (u32, u32) {
        let e = self.epoch();
        ((e >> 32) as u32, e as u32)
    }
    pub fn start(
        &self,
        conn: &Connection,
        serial: u32,
        output: OutputId,
        capabilities: u32,
    ) -> Result<(), crate::Error> {
        let output = conn.wl_output(output).ok_or(crate::Error::UnknownOutput)?;
        let (hi, lo) = self.parts();
        self.proxy.start(
            serial,
            hi,
            lo,
            &output,
            session::Capability::from_bits_retain(capabilities),
        );
        Ok(())
    }
    pub fn motion(&self, serial: u32, revision: u32, x: f64, y: f64) {
        let (h, l) = self.parts();
        self.proxy.motion(serial, h, l, revision, x, y);
    }
    pub fn button(&self, serial: u32, revision: u32, code: u32, pressed: bool) {
        let (h, l) = self.parts();
        self.proxy.button(
            serial,
            h,
            l,
            revision,
            code,
            if pressed {
                session::KeyState::Pressed
            } else {
                session::KeyState::Released
            },
        );
    }
    pub fn key(&self, serial: u32, revision: u32, code: u32, pressed: bool) {
        let (h, l) = self.parts();
        self.proxy.key(
            serial,
            h,
            l,
            revision,
            code,
            if pressed {
                session::KeyState::Pressed
            } else {
                session::KeyState::Released
            },
        );
    }
    pub fn axis(&self, serial: u32, revision: u32, horizontal: f64, vertical: f64) {
        let (h, l) = self.parts();
        self.proxy
            .axis(serial, h, l, revision, horizontal, vertical);
    }
    pub fn resume(&self, serial: u32) {
        let (h, l) = self.parts();
        self.proxy.resume(serial, h, l);
    }
    pub fn observe(&self, serial: u32) {
        let (h, l) = self.parts();
        self.proxy.observe(serial, h, l);
    }
    pub fn perform(
        &self,
        serial: u32,
        call: &cupertino_semantics::Invocation,
    ) -> Result<(), std::io::Error> {
        let (file, len) = cupertino_semantics::seal(call, cupertino_semantics::MAX_FRAME)?;
        let (h, l) = self.parts();
        self.proxy.perform(serial, h, l, file.as_fd(), len);
        Ok(())
    }
    pub fn stop(&self) {
        self.proxy.stop();
    }
}
impl Drop for ControlSession {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}
impl ControlAuthority {
    /// Version 2 names the app each request is for and remembers the answer.
    pub fn bind(conn: &mut Connection) -> Option<Self> {
        conn.bind_extension(1..=2, Quiet).map(Self)
    }
    pub fn set_surface(
        &self,
        conn: &Connection,
        surface: SurfaceId,
    ) -> Result<(), cupertino_wayland_client::Error> {
        self.0.set_surface(&conn.wl_surface(surface)?);
        Ok(())
    }
    /// Answer the request of `epoch`, `serial` being the input the person gave the panel.
    pub fn answer(&self, epoch: u64, answer: ConsentAnswer, serial: u32) {
        use wayland_client::Proxy;
        let allow = match answer {
            ConsentAnswer::Allow => 1,
            // version 1 knows no remembered refusal
            ConsentAnswer::Deny if self.0.version() >= 2 => 2,
            ConsentAnswer::Deny | ConsentAnswer::Dismiss => 0,
        };
        self.0
            .answer((epoch >> 32) as u32, epoch as u32, allow, serial);
    }
    pub fn stop(&self, epoch: u64) {
        self.0.stop((epoch >> 32) as u32, epoch as u32);
    }
}
impl Drop for ControlAuthority {
    fn drop(&mut self) {
        self.0.destroy();
    }
}
impl Dispatch<CupertinoControlV1, Quiet> for State {
    fn event(
        _: &mut Self,
        _: &CupertinoControlV1,
        _: <CupertinoControlV1 as wayland_client::Proxy>::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<CupertinoControlSessionV1, RunData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoControlSessionV1,
        event: session::Event,
        data: &RunData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let epoch = data.0.load(Ordering::Relaxed);
        let event = match event {
            session::Event::Created {
                epoch_hi,
                epoch_lo,
                available,
            } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                data.0.store(epoch, Ordering::Relaxed);
                ControlEvent::Created { epoch, available }
            }
            session::Event::Status { serial, phase } => ControlEvent::Status {
                epoch,
                serial,
                phase: match phase {
                    WEnum::Value(v) => v as u32,
                    WEnum::Unknown(v) => v,
                },
            },
            session::Event::Geometry {
                revision,
                x,
                y,
                width,
                height,
                scale,
                name,
            } => ControlEvent::Geometry {
                epoch,
                revision,
                x,
                y,
                width,
                height,
                scale,
                name,
            },
            session::Event::Observation {
                serial,
                payload,
                length,
            } => {
                let Ok(data) = cupertino_semantics::read(
                    payload,
                    length,
                    cupertino_semantics::MAX_OBSERVATION,
                ) else {
                    return;
                };
                ControlEvent::Observation {
                    epoch,
                    serial,
                    data,
                }
            }
            session::Event::ActionFinished {
                serial,
                payload,
                length,
            } => {
                let Ok(reply) =
                    cupertino_semantics::read(payload, length, cupertino_semantics::MAX_FRAME)
                else {
                    return;
                };
                ControlEvent::ActionFinished {
                    epoch,
                    serial,
                    reply,
                }
            }
            session::Event::SemanticChanged {
                revision_hi,
                revision_lo,
            } => ControlEvent::SemanticChanged {
                epoch,
                revision: (u64::from(revision_hi) << 32) | u64::from(revision_lo),
            },
            session::Event::Completed { serial } => ControlEvent::Completed { epoch, serial },
            session::Event::Refused { serial, reason } => ControlEvent::Refused {
                epoch,
                serial,
                reason: match reason {
                    WEnum::Value(v) => v as u32,
                    WEnum::Unknown(v) => v,
                },
            },
            _ => return,
        };
        state.emit(event);
    }
}
impl Dispatch<CupertinoControlAuthorityV1, Quiet> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoControlAuthorityV1,
        event: authority::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            authority::Event::Requested {
                epoch_hi,
                epoch_lo,
                capabilities,
                output_name,
                resuming,
            } => ControlEvent::ConsentRequested {
                epoch: (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
                capabilities,
                output: output_name,
                resuming: resuming != 0,
            },
            authority::Event::Changed {
                epoch_hi,
                epoch_lo,
                phase,
            } => ControlEvent::ConsentChanged {
                epoch: (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
                phase,
            },
            authority::Event::App {
                epoch_hi,
                epoch_lo,
                kind,
                identity,
                name,
                services,
                remembered,
            } => ControlEvent::ConsentApp {
                epoch: (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
                app: ConsentApp {
                    identity,
                    desktop: kind == WEnum::Value(authority::AppKind::Desktop),
                    name,
                    services,
                    remembered: remembered != 0,
                },
            },
            _ => return,
        };
        state.emit(event);
    }
}
