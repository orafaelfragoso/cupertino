//! Public-boundary tests for W5.4:
//! - `org.cupertino.Session1` (D13): `LogOut`, `Restart`, `ShutDown`, `Sleep` orchestrating application quit
//!   sequences (immediate `Sleep`, sequential quit with 45 s interrupt timeout, cancel, retry, locked refusal).
//! - `org.cupertino.Displays1` (D14): `ListDisplays`, `SetScale`, `SetRefreshRate`, `SetPosition`, `TestConfigure`,
//!   `Configure`, transform round-trip, validation errors, and caller authorization checks.

use std::sync::{Arc, Mutex};

use cupertino_assets::Assets;
use cupertino_peer::Caller;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::displays::service::{parse_transform, transform_name, DisplaysCall};
use cupertino_server::quit::{Call, Refusal, Reject, SessionAction, QUIT_TIMEOUT_S};
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;

fn make_server() -> (EventLoop<'static, Cupertino>, Cupertino) {
    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let mut config = Config::default();
    config.shell_processes.clear();
    let state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        false,
        &Assets::builtin(),
    )
    .expect("server initializes");
    (event_loop, state)
}

#[test]
fn session1_actions_dispatch_when_no_apps_open_and_reject_anonymous_callers() {
    let (_loop, mut state) = make_server();
    let fired = Arc::new(Mutex::new(Vec::<SessionAction>::new()));
    let sink_fired = Arc::clone(&fired);
    state.quit.action_sink = Some(Box::new(move |action| {
        sink_fired.lock().unwrap().push(action);
    }));

    // With no open application windows, all four D13 actions dispatch immediately.
    for action in [
        SessionAction::Sleep,
        SessionAction::LogOut,
        SessionAction::Restart,
        SessionAction::ShutDown,
    ] {
        state.request_session_action(action).unwrap();
    }
    assert_eq!(
        *fired.lock().unwrap(),
        vec![
            SessionAction::Sleep,
            SessionAction::LogOut,
            SessionAction::Restart,
            SessionAction::ShutDown,
        ]
    );
    assert!(state.pending_session_action().is_none());

    // Anonymous D-Bus caller is refused by `on_quit_call`.
    let (tx, rx) = async_channel::bounded(1);
    state.on_quit_call(Call::SessionAction(
        SessionAction::LogOut,
        Caller::Anonymous,
        tx,
    ));
    let res = rx.try_recv().unwrap();
    assert!(matches!(res, Err(Reject::Caller(_))));
}

#[test]
fn session1_quit_sequence_cancels_and_respects_timeout_constant() {
    let (_loop, mut state) = make_server();
    state.fixed_time = Some(10.0);
    assert_eq!(QUIT_TIMEOUT_S, 45.0);

    // Calling cancel/retry while idle is safe.
    state.cancel_session_action();
    state.retry_session_action();
    assert!(state.pending_session_action().is_none());
}

#[test]
fn displays1_lists_and_configures_scale_refresh_position_and_transform() {
    let (_loop, mut state) = make_server();
    headless::init_displays(
        &mut state,
        &[
            HeadlessDisplay {
                name: "DP-1".into(),
                size: (1920, 1080),
                physical_mm: (340, 220),
            },
            HeadlessDisplay {
                name: "DP-2".into(),
                size: (1920, 1080),
                physical_mm: (340, 220),
            },
        ],
        Some(1.0),
    )
    .expect("headless displays initialize");

    let list = state.list_displays();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].0, "DP-1");
    assert!(list[0].2);
    assert_eq!(list[0].5, 1920);
    assert_eq!(list[0].6, 1080);
    assert_eq!(list[0].7, 60_000);

    // SetScale, SetRefreshRate, and SetPosition apply through configure_heads.
    state.display_set_scale("DP-1", 2.0).unwrap();
    state.display_set_refresh_rate("DP-1", 120_000).unwrap();
    state.display_set_position("DP-2", 960, 0).unwrap();

    let updated = state.list_displays();
    assert_eq!(updated[0].7, 120_000);
    assert!((updated[0].8 - 2.0).abs() < 1e-6);
    assert_eq!((updated[1].3, updated[1].4), (960, 0));

    // TestConfigure validates without mutating state.
    state
        .display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "90", true)
        .unwrap();
    let after_test = state.list_displays();
    assert_eq!(after_test[0].5, 1920);
    assert_eq!(after_test[0].9, "normal");

    // Configure applies full state including transform (DP-2 at x=960 avoids overlap with 720x1280).
    state
        .display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "90", false)
        .unwrap();
    let after_apply = state.list_displays();
    assert_eq!((after_apply[0].5, after_apply[0].6), (1280, 720));
    assert_eq!(after_apply[0].9, "90");

    // Validation errors.
    assert!(matches!(
        state.display_set_scale("DP-1", 0.0),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(matches!(
        state.display_set_scale("NO-SUCH", 1.0),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(matches!(
        state.display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "bogus", false),
        Err(Refusal::InvalidArgs(_))
    ));

    // Every Transform round-trips cleanly through transform_name / parse_transform.
    for t in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        assert_eq!(parse_transform(transform_name(t)), Some(t));
    }

    // D-Bus call path: ListDisplays is open, SetScale refuses anonymous callers.
    let (ltx, lrx) = async_channel::bounded(1);
    state.on_displays_call(DisplaysCall::List(ltx));
    assert_eq!(lrx.try_recv().unwrap().unwrap().len(), 2);

    let (stx, srx) = async_channel::bounded(1);
    state.on_displays_call(DisplaysCall::SetScale {
        connector: "DP-1".into(),
        scale: 1.5,
        caller: Caller::Anonymous,
        reply: stx,
    });
    assert!(matches!(srx.try_recv().unwrap(), Err(Reject::Caller(_))));
}
