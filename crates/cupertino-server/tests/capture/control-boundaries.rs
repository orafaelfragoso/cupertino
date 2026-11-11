use super::*;
use smithay::input::keyboard::Keycode;

#[test]
fn controller_disconnect_preserves_a_physical_modifier_and_releases_only_its_input() {
    let Some(mut h) = Harness::new() else { return };
    let (mut target, _) = mapped_app_named(&mut h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut target, 3);
    let target_id = h.state.wm.key_window().unwrap();
    let (mut app, authority) = authority(&mut h);
    let mut c = Controller::new(&mut h);
    h.state.process_input_event(common::absolute::key(42, true));
    c.start(&mut h, 15).unwrap();
    approve(&mut h, &mut app, &authority, &mut c);
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: target_id });
    c.key(&mut h, 42, true).unwrap();
    c.key(&mut h, 30, true).unwrap();
    c.session.destroy();
    c.client.pump();
    h.pump_until(&mut target, "owned key cleanup", |s| {
        s.control_keys.contains(&(30, false))
    });
    assert!(h
        .state
        .seat
        .get_keyboard()
        .unwrap()
        .pressed_keys()
        .contains(&Keycode::new(50)));
    assert!(!target.state.control_keys.contains(&(42, false)));
    h.state
        .process_input_event(common::absolute::key(42, false));
    h.pump_until(&mut target, "the person's own release", |s| {
        s.control_keys.contains(&(42, false))
    });
}

#[test]
fn geometry_changes_invalidate_input_and_old_capture_sources_never_acquire_a_new_run() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let mut c = Controller::new(&mut h);
    c.start(&mut h, 15).unwrap();
    approve(&mut h, &mut app, &authority, &mut c);
    let source = c.source();
    let _capture = c.client.session(&mut h, &source, false);
    let id = h.state.outputs[0].id;
    let mode = h.state.outputs[0].output.current_mode().unwrap();
    h.state
        .configure_output(id, mode, 2.0, smithay::utils::Transform::_90, (300, -100));
    h.pump_until(&mut c.client, "a revised transformed geometry", |s| {
        s.control_seen.revision == 2
    });
    let n = c.serial();
    c.session.motion(n, c.hi, c.lo, 1, 1.0, 1.0);
    assert_eq!(c.ack(&mut h, n), Err(session::Refusal::StaleGeometry));
    c.session.stop();
    c.phase(&mut h, 4);
    c.stopped_source(&mut h, &source);
    let old = c.client.state.control_seen.epoch;
    c.session.destroy();
    c.client.state.control_seen = Seen::default();
    let qh = c.client.qh();
    let manager: Manager = c.client.globals.bind(&qh, 2..=2, ()).unwrap();
    c.session = manager.get_session(&qh, ());
    h.pump_until(&mut c.client, "a new epoch", |s| s.control_seen.epoch != 0);
    let epoch = c.client.state.control_seen.epoch;
    assert_ne!(epoch, old);
    c.hi = (epoch >> 32) as u32;
    c.lo = epoch as u32;
    c.next = 1;
    c.start(&mut h, 15).unwrap();
    approve(&mut h, &mut app, &authority, &mut c);
    c.stopped_source(&mut h, &source);
    let fresh = c.source();
    let capture = c.client.session(&mut h, &fresh, false);
    assert_eq!(capture.size, (700, 1000));
    h.state.remove_output(id);
    c.phase(&mut h, 4);
    c.stopped_source(&mut h, &fresh);
}

/// Send a motion carrying `revision`, as an agent does with the geometry revision of an earlier observation.
fn motion_at(
    h: &mut Harness,
    c: &mut Controller,
    revision: u32,
    (x, y): (f64, f64),
) -> Result<(), session::Refusal> {
    let n = c.serial();
    c.session.motion(n, c.hi, c.lo, revision, x, y);
    c.ack(h, n)
}

/// An observing run over two windows, the second above the first.
struct Observing {
    /// The consent panel's client and authority.
    consent: (TestClient<App>, Authority),
    /// Both windows' clients and ids, the lower first.
    windows: [(TestClient<App>, cupertino_wm::WindowId); 2],
    /// The controller, its run active.
    c: Controller,
}

fn observing_run(h: &mut Harness) -> Observing {
    let (mut app, authority) = authority(h);
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let (mut first, _) =
        mapped_app_beside(h, stream, "org.example.first", Some("One"), &mut [&mut app]);
    let first_id = h.state.wm.key_window().unwrap();
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let (second, _) = mapped_app_beside(
        h,
        stream,
        "org.example.second",
        Some("One"),
        &mut [&mut app, &mut first],
    );
    let second_id = h.state.wm.key_window().unwrap();
    // version 3 with observe: the run sees the windows' layout
    let mut c = Controller::bound(h, 3);
    c.start(h, 15).unwrap();
    approve(h, &mut app, &authority, &mut c);
    h.idle(&mut c.client, 5);
    Observing {
        consent: (app, authority),
        windows: [(first, first_id), (second, second_id)],
        c,
    }
}

#[test]
fn title_and_focus_changes_keep_a_pending_move_valid() {
    let Some(mut h) = Harness::new() else { return };
    let Observing {
        consent: _consent,
        windows: [(_first, first_id), (mut second, _)],
        mut c,
    } = observing_run(&mut h);
    let revision = c.client.state.control_seen.revision;
    let point = (20.0, 20.0);
    let semantic = c.client.state.control_seen.semantic;
    second
        .state
        ._window
        .as_ref()
        .unwrap()
        .2
        .set_title("Two".into());
    second.pump();
    h.pump_until(&mut c.client, "the new title announced", |s| {
        s.control_seen.semantic > semantic
    });
    h.idle(&mut c.client, 5);
    assert_eq!(c.client.state.control_seen.revision, revision);
    assert_eq!(motion_at(&mut h, &mut c, revision, point), Ok(()));
    // the keyboard moves to the lower window without restacking it
    let semantic = c.client.state.control_seen.semantic;
    let surface = h.state.entry(first_id).unwrap().surface().clone();
    h.state.set_keyboard_focus(Some(surface));
    h.pump_until(&mut c.client, "the new focus announced", |s| {
        s.control_seen.semantic > semantic
    });
    h.idle(&mut c.client, 5);
    assert_eq!(c.client.state.control_seen.revision, revision);
    assert_eq!(motion_at(&mut h, &mut c, revision, point), Ok(()));
}

#[test]
fn moved_resized_restacked_mapped_and_unmapped_windows_and_a_new_scale_advance_the_revision() {
    let Some(mut h) = Harness::new() else { return };
    let Observing {
        consent: (mut consent, _authority),
        windows: [(mut first, first_id), (mut second, second_id)],
        mut c,
    } = observing_run(&mut h);
    let point = (20.0, 20.0);
    let mut revision = c.client.state.control_seen.revision;
    let mut advanced = |h: &mut Harness, c: &mut Controller, what: &str| {
        h.pump_until(&mut c.client, what, |s| s.control_seen.revision > revision);
        h.idle(&mut c.client, 5);
        assert_eq!(
            motion_at(h, c, revision, point),
            Err(session::Refusal::StaleGeometry),
            "{what}"
        );
        revision = c.client.state.control_seen.revision;
        assert_eq!(motion_at(h, c, revision, point), Ok(()), "{what}");
    };
    // moved
    let frame = h.state.wm.window(second_id).unwrap().geometry();
    h.state.command(cupertino_wm::Command::BeginMove {
        window: second_id,
        pointer: cupertino_wm::Point::new(0.0, 0.0),
    });
    h.state.command(cupertino_wm::Command::PointerMotion {
        pointer: cupertino_wm::Point::new(30.0, 20.0),
        modifiers: cupertino_wm::Modifiers::default(),
    });
    h.state.command(cupertino_wm::Command::PointerRelease);
    let moved = h.state.wm.window(second_id).unwrap().geometry();
    assert_eq!((moved.x, moved.y), (frame.x + 30.0, frame.y + 20.0));
    advanced(&mut h, &mut c, "a moved window");
    // resized by its client, where it is
    let qh = second.qh();
    let surface = second
        .state
        ._window
        .as_ref()
        .unwrap()
        .1
        .data::<wl_surface::WlSurface>()
        .unwrap()
        .clone();
    let size = (TOPLEVEL.0 + 40, TOPLEVEL.1 + 30);
    let (buffer, file) = shm_buffer(second.state.shm.as_ref().unwrap(), size, |_, _| BLUE, &qh);
    second.state._file = Some(file);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, size.0, size.1);
    surface.commit();
    second.pump();
    advanced(&mut h, &mut c, "a resized window");
    let resized = h.state.wm.window(second_id).unwrap().geometry();
    assert_eq!((resized.x, resized.y), (moved.x, moved.y));
    // restacked
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: first_id });
    assert_eq!(h.state.wm.stacking_order().last(), Some(&first_id));
    advanced(&mut h, &mut c, "a restacked window");
    // mapped, then unmapped
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let (third, _) = mapped_app_beside(
        &mut h,
        stream,
        "org.example.third",
        None,
        &mut [&mut consent, &mut first, &mut second],
    );
    advanced(&mut h, &mut c, "a mapped window");
    drop(third);
    advanced(&mut h, &mut c, "an unmapped window");
    // the output's scale
    let id = h.state.outputs[0].id;
    let mode = h.state.outputs[0].output.current_mode().unwrap();
    h.state
        .configure_output(id, mode, 2.0, smithay::utils::Transform::Normal, (0, 0));
    advanced(&mut h, &mut c, "a new output scale");
}

#[test]
fn a_pointer_grant_and_a_physical_drag_share_the_button_without_ending_each_other() {
    let Some(mut h) = Harness::new() else { return };
    // the panel first, so the window the person and the run click is above it
    let (mut app, authority) = authority(&mut h);
    let (mut target, frame) = mapped_app_named(&mut h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    h.idle(&mut target, 3);
    let mut c = Controller::new(&mut h);
    let (x, y) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
    h.state.process_input_event(common::absolute::motion(
        x / f64::from(OUTPUT.0),
        y / f64::from(OUTPUT.1),
    ));
    // a run may start while the person drags; only its approval waits for the drag to end
    h.state.process_input_event(common::absolute::button(true));
    assert_eq!(c.start(&mut h, 15), Ok(()));
    h.state.process_input_event(common::absolute::button(false));
    approve(&mut h, &mut app, &authority, &mut c);
    h.state.process_input_event(common::absolute::motion(
        x / f64::from(OUTPUT.0),
        y / f64::from(OUTPUT.1),
    ));
    h.idle(&mut target, 3);
    target.state.control_buttons.clear();
    h.state.process_input_event(common::absolute::button(true));
    assert_eq!(c.button(&mut h, 272, true), Ok(()));
    assert_eq!(c.button(&mut h, 272, false), Ok(()));
    h.idle(&mut target, 3);
    assert_eq!(
        target.state.control_buttons,
        [(272, true)],
        "the run's press and release leave the person's drag held"
    );
    h.state.process_input_event(common::absolute::button(false));
    assert_eq!(c.button(&mut h, 272, true), Ok(()));
    assert_eq!(c.button(&mut h, 272, false), Ok(()));
    h.pump_until(&mut target, "the run's own click", |s| {
        s.control_buttons.len() == 4
    });
    assert_eq!(
        target.state.control_buttons,
        [(272, true), (272, false), (272, true), (272, false)]
    );
    assert_eq!(c.client.state.control_seen.phase, 2);
}

#[test]
fn injected_reserved_keys_cannot_stop_lock_or_power_the_session() {
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
    for (code, pressed) in [
        (29, true),
        (56, true),
        (1, true),
        (1, false),
        (56, false),
        (29, false),
        (116, true),
        (116, false),
    ] {
        c.key(&mut h, code, pressed).unwrap();
    }
    h.idle(&mut target, 3);
    assert_eq!(c.client.state.control_seen.phase, 2);
    assert!(!h.state.session_lock.is_active());
    assert!(!target
        .state
        .control_keys
        .iter()
        .any(|(code, _)| *code == 1 || *code == 116));
    // The same emergency chord delivered by physical devices revokes the run.
    for code in [29, 56, 1] {
        h.state
            .process_input_event(common::absolute::key(code, true));
    }
    c.phase(&mut h, 4);
    for code in [1, 56, 29] {
        h.state
            .process_input_event(common::absolute::key(code, false));
    }
}
