//! Native control exercised over real private Wayland connections, with backend-originated consent input.
use super::*;
#[path = "control-boundaries.rs"]
mod boundaries;
#[path = "control-permissions.rs"]
mod permissions;
#[path = "control-runs.rs"]
mod runs;
use cupertino_protocols::client::control::{
    cupertino_control_authority_v1::{self as authority, CupertinoControlAuthorityV1 as Authority},
    cupertino_control_session_v1::{self as session, CupertinoControlSessionV1 as Session},
    cupertino_control_v1::CupertinoControlV1 as Manager,
};
use cupertino_server::authorization::ShellRole;
use cupertino_server::shell_connection::shell_process_connection;
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat};

#[derive(Default)]
pub(super) struct Seen {
    epoch: u64,
    phase: u32,
    revision: u32,
    completed: Vec<u32>,
    refused: Vec<(u32, session::Refusal)>,
}
ignore_events!(Capturer: Manager);
ignore_events!(App: wl_seat::WlSeat);
impl Dispatch<Session, ()> for Capturer {
    fn event(
        s: &mut Self,
        _: &Session,
        e: session::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let s = &mut s.control_seen;
        match e {
            session::Event::Created {
                epoch_hi, epoch_lo, ..
            } => s.epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
            session::Event::Status { phase, .. } => s.phase = phase.into(),
            session::Event::Geometry { revision, .. } => s.revision = revision,
            session::Event::Completed { serial } => s.completed.push(serial),
            session::Event::Refused { serial, reason } => {
                s.refused.push((serial, reason.into_result().unwrap()))
            }
            _ => {}
        }
    }
}
impl Dispatch<Authority, ()> for App {
    fn event(
        s: &mut Self,
        _: &Authority,
        e: authority::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            authority::Event::Requested {
                epoch_hi,
                epoch_lo,
                capabilities,
                resuming,
                ..
            } => {
                s.control_request = Some((
                    (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
                    capabilities,
                    resuming != 0,
                ))
            }
            authority::Event::App {
                epoch_hi,
                epoch_lo,
                kind,
                identity,
                name,
                services,
                remembered,
            } => {
                s.control_app = Some((
                    (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
                    kind == wayland_client::WEnum::Value(authority::AppKind::Desktop),
                    identity,
                    name,
                    services,
                    remembered != 0,
                ))
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        s: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        e: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            serial, key, state, ..
        } = e
        {
            s.control_serial = serial;
            s.control_keys.push((
                key,
                state == wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
            ));
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        s: &mut Self,
        _: &wl_pointer::WlPointer,
        e: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button { button, state, .. } = e {
            s.control_buttons.push((
                button,
                state == wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
            ));
        }
    }
}

struct Controller {
    client: TestClient<Capturer>,
    session: Session,
    output: wl_output::WlOutput,
    hi: u32,
    lo: u32,
    next: u32,
}
impl Controller {
    /// A version 2 controller, which names no app: it is asked every time.
    fn new(h: &mut Harness) -> Self {
        Self::bound(h, 2)
    }
    /// A version 4 controller that names `bridge` as the process it serves.
    fn named(h: &mut Harness, bridge: &permissions::Bridge) -> Self {
        let c = Self::bound(h, 4);
        c.session
            .identify(bridge.pid, (bridge.start >> 32) as u32, bridge.start as u32);
        c
    }
    fn bound(h: &mut Harness, version: u32) -> Self {
        let stream =
            shell_process_connection(&h.state.display_handle, &[ShellRole::DesktopControl])
                .unwrap();
        let mut c = TestClient::connect(h, stream, Capturer::default());
        let qh = c.qh();
        let manager: Manager = c.globals.bind(&qh, version..=version, ()).unwrap();
        let session = manager.get_session(&qh, ());
        let output = c.globals.bind(&qh, 1..=4, ()).unwrap();
        h.pump_until(&mut c, "the run epoch", |s| s.control_seen.epoch != 0);
        let epoch = c.state.control_seen.epoch;
        Self {
            client: c,
            session,
            output,
            hi: (epoch >> 32) as u32,
            lo: epoch as u32,
            next: 1,
        }
    }
    fn serial(&mut self) -> u32 {
        let n = self.next;
        self.next += 1;
        n
    }
    fn ack(&mut self, h: &mut Harness, serial: u32) -> Result<(), session::Refusal> {
        h.pump_until(&mut self.client, "the control reply", |s| {
            s.control_seen.completed.contains(&serial)
                || s.control_seen.refused.iter().any(|(n, _)| *n == serial)
        });
        self.client
            .state
            .control_seen
            .refused
            .iter()
            .find(|(n, _)| *n == serial)
            .map_or(Ok(()), |(_, e)| Err(*e))
    }
    fn start(&mut self, h: &mut Harness, bits: u32) -> Result<(), session::Refusal> {
        let n = self.serial();
        self.session.start(
            n,
            self.hi,
            self.lo,
            &self.output,
            session::Capability::from_bits_retain(bits),
        );
        self.ack(h, n)
    }
    fn motion(&mut self, h: &mut Harness, x: f64, y: f64) -> Result<(), session::Refusal> {
        let n = self.serial();
        self.session.motion(
            n,
            self.hi,
            self.lo,
            self.client.state.control_seen.revision,
            x,
            y,
        );
        self.ack(h, n)
    }
    fn key(&mut self, h: &mut Harness, code: u32, pressed: bool) -> Result<(), session::Refusal> {
        let n = self.serial();
        self.session.key(
            n,
            self.hi,
            self.lo,
            self.client.state.control_seen.revision,
            code,
            if pressed {
                session::KeyState::Pressed
            } else {
                session::KeyState::Released
            },
        );
        self.ack(h, n)
    }
    fn button(
        &mut self,
        h: &mut Harness,
        code: u32,
        pressed: bool,
    ) -> Result<(), session::Refusal> {
        let n = self.serial();
        self.session.button(
            n,
            self.hi,
            self.lo,
            self.client.state.control_seen.revision,
            code,
            if pressed {
                session::KeyState::Pressed
            } else {
                session::KeyState::Released
            },
        );
        self.ack(h, n)
    }
    fn phase(&mut self, h: &mut Harness, phase: u32) {
        h.pump_until(&mut self.client, "the control phase", |s| {
            s.control_seen.phase == phase
        });
    }
    fn source(&self) -> ext_image_capture_source_v1::ExtImageCaptureSourceV1 {
        let qh = self.client.qh();
        let manager:ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1=self.client.globals.bind(&qh,1..=1,()).unwrap();
        manager.create_source(&self.output, &qh, ())
    }
    fn stopped_source(
        &mut self,
        h: &mut Harness,
        source: &ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ) {
        self.client.state.session = SessionSeen::default();
        let qh = self.client.qh();
        let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
            self.client.globals.bind(&qh, 1..=1, ()).unwrap();
        let s = manager.create_session(
            source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &qh,
            (),
        );
        h.pump_until(&mut self.client, "denied capture", |s| s.session.stopped);
        assert_eq!(self.client.state.session.size, None);
        assert!(!self.client.state.session.done);
        s.destroy();
    }
}
fn authority(h: &mut Harness) -> (TestClient<App>, Authority) {
    let stream =
        shell_process_connection(&h.state.display_handle, &[ShellRole::SessionLock]).unwrap();
    let (mut app, _) = mapped_app_stream(h, stream, "org.example.consent", None);
    app.state.control_window = h.state.wm.key_window();
    let qh = app.qh();
    let authority: Authority = app.globals.bind(&qh, 2..=2, ()).unwrap();
    let surface = app
        .state
        ._window
        .as_ref()
        .unwrap()
        .1
        .data::<wl_surface::WlSurface>()
        .unwrap();
    authority.set_surface(surface);
    let seat: wl_seat::WlSeat = app.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut app, 3);
    (app, authority)
}
fn approve(h: &mut Harness, app: &mut TestClient<App>, authority: &Authority, c: &mut Controller) {
    answer(h, app, authority, c, 1);
    c.phase(h, 2);
}
/// Answer the pending request of `c` with `allow`, given by a physical key on the consent surface.
fn answer(
    h: &mut Harness,
    app: &mut TestClient<App>,
    authority: &Authority,
    c: &mut Controller,
    allow: u32,
) {
    h.pump_until(app, "the native consent request", |s| {
        s.control_request.is_some()
    });
    app.state.control_serial = 0;
    let window = app.state.control_window.unwrap();
    let surface = h.state.entry(window).unwrap().surface().clone();
    h.state.set_keyboard_focus(Some(surface));
    for pressed in [true, false] {
        h.state
            .process_input_event(common::absolute::key(57, pressed));
    }
    h.pump_until(app, "a physical consent serial", |s| s.control_serial != 0);
    authority.answer(c.hi, c.lo, allow, app.state.control_serial);
    app.pump();
}

#[test]
fn native_consent_rejects_forged_serials_and_grants_scoped_input_and_capture() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let mut c = Controller::new(&mut h);
    let old_source = c.source();
    c.stopped_source(&mut h, &old_source);
    assert_eq!(
        c.motion(&mut h, 20.0, 20.0),
        Err(session::Refusal::NotGranted)
    );
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 1);
    authority.answer(c.hi, c.lo, 1, 0);
    app.pump();
    h.idle(&mut c.client, 5);
    assert_eq!(c.client.state.control_seen.phase, 1);
    // A true user serial created before the request cannot approve it either.
    approve(&mut h, &mut app, &authority, &mut c);
    assert_eq!(c.client.state.control_seen.revision, 1);
    c.stopped_source(&mut h, &old_source);
    // The authority's surface cannot receive injected input, even after consent.
    assert_eq!(c.key(&mut h, 30, true), Err(session::Refusal::Blocked));
    let (mut target, frame) = mapped_app_named(&mut h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut target, 3);
    assert_eq!(
        c.motion(&mut h, frame.x + frame.w / 2.0, frame.y + frame.h / 2.0),
        Ok(())
    );
    assert_eq!(
        c.motion(&mut h, -1.0, 20.0),
        Err(session::Refusal::OutsideScope)
    );
    assert_eq!(c.key(&mut h, 30, true), Ok(()));
    assert_eq!(c.key(&mut h, 30, false), Ok(()));
    h.pump_until(&mut target, "the injected key pair", |s| {
        s.control_keys.len() == 2
    });
    assert_eq!(target.state.control_keys, [(30, true), (30, false)]);
    let source = c.source();
    let capture = c.client.session(&mut h, &source, false);
    let first_frame = c.client.request_frame(&capture);
    h.pump_until(&mut c.client, "the granted capture", |s| s.frame.ready);
    assert!(c.client.state.frame.failed.is_none());
    assert!(capture.pixels().contains(&RED));
    first_frame.destroy();
    // The person's input never pauses the run: both keep using the desktop, and a key both hold is released only when
    // the last of them lets go.
    assert_eq!(c.key(&mut h, 42, true), Ok(()));
    h.state
        .process_input_event(common::absolute::motion(0.1, 0.1));
    for pressed in [true, false] {
        h.state
            .process_input_event(common::absolute::key(42, pressed));
    }
    h.state.process_input_event(common::absolute::key(30, true));
    let during = c.client.session(&mut h, &source, false);
    c.client.request_frame(&during);
    h.state.request_redraw();
    h.pump_until(&mut c.client, "a capture during physical input", |s| {
        s.frame.ready
    });
    assert_eq!(c.client.state.control_seen.phase, 2);
    assert_eq!(c.key(&mut h, 48, true), Ok(()));
    assert_eq!(c.key(&mut h, 48, false), Ok(()));
    h.state
        .process_input_event(common::absolute::key(30, false));
    h.pump_until(&mut target, "the interleaved keys", |s| {
        s.control_keys.contains(&(30, false))
    });
    assert!(
        !target.state.control_keys.contains(&(42, false)),
        "the run still holds 42: {:?}",
        target.state.control_keys
    );
    assert_eq!(c.key(&mut h, 42, false), Ok(()));
    h.pump_until(&mut target, "the run's own release", |s| {
        s.control_keys.contains(&(42, false))
    });
    assert_eq!(
        target.state.control_keys,
        [
            (30, true),
            (30, false),
            (42, true),
            (30, true),
            (48, true),
            (48, false),
            (30, false),
            (42, false)
        ]
    );
    h.state.lock_session_now();
    c.phase(&mut h, 4);
    c.stopped_source(&mut h, &source);
}

#[test]
fn denial_and_authority_loss_are_terminal_and_capabilities_are_enforced() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let mut c = Controller::new(&mut h);
    assert_eq!(c.start(&mut h, 16), Err(session::Refusal::Unsupported));
    assert_eq!(c.start(&mut h, 2), Ok(()));
    approve(&mut h, &mut app, &authority, &mut c);
    assert_eq!(
        c.motion(&mut h, 10.0, 10.0),
        Err(session::Refusal::NotGranted)
    );
    let source = c.source();
    let capture = c.client.session(&mut h, &source, false);
    c.client.request_frame(&capture);
    h.pump_until(
        &mut c.client,
        "capture with the consent surface omitted",
        |s| s.frame.ready,
    );
    assert!(!capture.pixels().contains(&RED));
    // An active run does not depend on the panel that asked for it; a run that must ask needs one.
    authority.destroy();
    app.pump();
    h.idle(&mut c.client, 5);
    assert_eq!(c.client.state.control_seen.phase, 2);
    c.session.stop();
    c.phase(&mut h, 4);
    let mut next = Controller::new(&mut h);
    assert_eq!(next.start(&mut h, 15), Err(session::Refusal::NoAuthority));
    assert_eq!(next.client.state.control_seen.phase, 0);
}

#[test]
fn repeated_mutations_do_not_replay_and_stale_geometry_is_refused() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let mut c = Controller::new(&mut h);
    c.start(&mut h, 15).unwrap();
    approve(&mut h, &mut app, &authority, &mut c);
    let (mut target, _) = mapped_app_named(&mut h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut target, 3);
    let n = c.serial();
    c.session
        .key(n, c.hi, c.lo, 1, 30, session::KeyState::Pressed);
    c.ack(&mut h, n).unwrap();
    c.session
        .key(n, c.hi, c.lo, 1, 30, session::KeyState::Pressed);
    h.idle(&mut c.client, 3);
    h.idle(&mut target, 3);
    assert_eq!(target.state.control_keys, [(30, true)]);
    let n = c.serial();
    c.session.motion(n, c.hi, c.lo, 0, 1.0, 1.0);
    assert_eq!(c.ack(&mut h, n), Err(session::Refusal::StaleGeometry));
    c.session.stop();
    c.phase(&mut h, 4);
    h.pump_until(&mut target, "cleanup release", |s| {
        s.control_keys.len() == 2
    });
    assert_eq!(target.state.control_keys, [(30, true), (30, false)]);
}
