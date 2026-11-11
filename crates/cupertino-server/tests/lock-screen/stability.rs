//! Capture remains useful during normal desktop activity, with exact output-layout checks.
use super::*;

#[test]
fn readonly_control_survives_physical_input_and_capture_survives_semantic_churn() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (_first, _) = session.server.application();
    let second = Client::listening(session.server.socket_path(), "org.example.other-window");
    session.server.wait_until("two mapped windows", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let windows: Vec<_> = session
        .server
        .state
        .windows
        .iter()
        .filter(|e| e.mapped)
        .map(|e| e.id)
        .collect();
    start_controller(&mut session);
    let mut local = Local::connect(&mut session);
    local.ok(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":3}),
    );
    session
        .server
        .wait_until("read-only consent", |s| panel_rect(s).is_some());
    let (x, y, _, _) = panel_rect(&session.server.state).unwrap();
    click(&mut session.server, x + 345.0, y + 196.0);
    local.ok(
        &mut session.server,
        json!({"op":"wait","phase":2,"timeout_ms":5000}),
    );
    session
        .server
        .wait_until("consent window to disappear", |s| panel_rect(s).is_none());
    session.server.run_for(Duration::from_millis(800));
    // A screen observer can follow the person using their desktop, including touch-frame completion.
    session
        .server
        .state
        .process_input_event(common::absolute::motion(0.2, 0.8));
    for pressed in [true, false] {
        session
            .server
            .state
            .process_input_event(common::absolute::key(30, pressed));
    }
    session
        .server
        .state
        .process_input_event(common::absolute::frame());
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        2
    );
    let before =
        local.ok(&mut session.server, json!({"op":"status"}))["geometry"]["revision"].clone();
    let mut n = 0;
    let image = local.call_while(&mut session.server, json!({"op":"capture"}), |s| {
        // Raising windows restacks them, which changes the input token while display geometry and pixel mapping stay
        // fixed.
        s.command(cupertino_wm::Command::FocusWindow {
            window: windows[n % windows.len()],
        });
        n += 1;
    });
    assert_eq!(image["ok"], true, "{image}");
    assert_eq!(image["data"]["width"], OUTPUT.0);
    assert!(n > 1);
    let after =
        local.ok(&mut session.server, json!({"op":"status"}))["geometry"]["revision"].clone();
    assert_ne!(
        before, after,
        "the capture must overlap real input-token invalidations"
    );
    // A 180-degree output transform changes pixel mapping even with unchanged logical dimensions.
    let mut rotated = false;
    let image = local.call_while(&mut session.server, json!({"op":"capture"}), |s| {
        if !rotated && s.capture_running() {
            let id = s.outputs[0].id;
            let mode = s.outputs[0].output.current_mode().unwrap();
            s.configure_output(id, mode, 1.0, smithay::utils::Transform::_180, (0, 0));
            rotated = true;
        }
    });
    assert!(rotated);
    assert_eq!(
        image["ok"], false,
        "a transformed output cannot reuse capture metadata"
    );
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        2
    );
    local.ok(&mut session.server, json!({"op":"stop"}));
    drop(second);
}
