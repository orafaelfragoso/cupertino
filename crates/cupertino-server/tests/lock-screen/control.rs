//! The shipped Rust controller and native Kit consent UI run together under an isolated session.
#[path = "semantics.rs"]
mod semantics;
#[path = "stability.rs"]
mod stability;
use super::*;
use serde_json::{json, Value};
use smithay::desktop::layer_map_for_output;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

struct Local {
    child: Child,
    input: ChildStdin,
    output: Option<BufReader<ChildStdout>>,
}
impl Local {
    fn connect(session: &mut Session) -> Self {
        let program = common::fixtures::artifact("cupertino-control", "bin", "cupertino-control");
        let mut command = common::spawn::cleared(&program, &session.fixture.root.join("home"));
        let mut child = command
            .args([
                "--session",
                session.server.state.socket_name.as_deref().unwrap(),
            ])
            .env(
                "XDG_RUNTIME_DIR",
                std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: Some(BufReader::new(child.stdout.take().unwrap())),
            child,
        }
    }
    fn call(&mut self, server: &mut Server, command: Value) -> Value {
        self.call_while(server, command, |_| {})
    }
    fn call_while(
        &mut self,
        server: &mut Server,
        command: Value,
        mut step: impl FnMut(&mut Cupertino),
    ) -> Value {
        serde_json::to_writer(&mut self.input, &command).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
        let mut output = self.output.take().unwrap();
        let read = std::thread::spawn(move || {
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            (output, line)
        });
        let deadline = Instant::now() + common::HANG_GUARD;
        while !read.is_finished() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for native response: {command}"
            );
            step(&mut server.state);
            server.step();
        }
        let (output, line) = read.join().unwrap();
        self.output = Some(output);
        serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("bad native response to {command}: {line:?}"))
    }
    #[track_caller]
    fn ok(&mut self, server: &mut Server, command: Value) -> Value {
        let response = self.call(server, command.clone());
        assert_eq!(response["ok"], true, "{command}: {response}");
        response["data"].clone()
    }
}
impl Drop for Local {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn panel_rect(state: &Cupertino) -> Option<(f64, f64, f64, f64)> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map
        .layers()
        .find(|l| l.namespace() == "desktop-control-consent")?;
    let geometry = map.layer_geometry(layer)?;
    (layer.bbox().size.h > 0).then_some((
        f64::from(geometry.loc.x),
        f64::from(geometry.loc.y),
        f64::from(geometry.size.w),
        f64::from(geometry.size.h),
    ))
}
fn start_controller(session: &mut Session) {
    use std::os::unix::fs::MetadataExt;
    let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
        .join("cupertino")
        .join(format!(
            "control-{}.sock",
            session.server.state.socket_name.as_deref().unwrap()
        ));
    let identity = || {
        std::fs::metadata(&socket)
            .ok()
            .map(|m| (m.ino(), m.ctime(), m.ctime_nsec()))
    };
    // Earlier fixture processes can leave a socket behind. Wait for this controller's bind without consuming a peer.
    let previous = identity();
    let program = common::fixtures::artifact("cupertino-control", "bin", "cupertino-control");
    session.server.state.supervisor.add_built_ins(
        [(
            cupertino_server::config::BuiltIn::DesktopControl,
            Some(program),
        )],
        Instant::now(),
    );
    start_shell_supervision(&session.server.state, &session.server.event_loop).unwrap();
    session
        .server
        .wait_until("the new native controller socket", |_| {
            identity().is_some_and(|current| Some(current) != previous)
        });
}
fn click(server: &mut Server, x: f64, y: f64) {
    server.state.process_input_event(common::absolute::motion(
        x / f64::from(OUTPUT.0),
        y / f64::from(OUTPUT.1),
    ));
    for pressed in [true, false] {
        server
            .state
            .process_input_event(common::absolute::button(pressed));
        server.step();
    }
}
#[test]
fn native_controller_and_session_consent_support_granted_input_capture_resume_and_disconnect() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (application, point) = session.server.application();
    start_controller(&mut session);
    let mut local = Local::connect(&mut session);
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"hello","version":1}))["schema"],
        "cupertino.control.v1"
    );
    let first = local.ok(&mut session.server, json!({"op":"status"}));
    assert_eq!(first["phase"], 0);
    assert_eq!(
        local.call(&mut session.server, json!({"op":"capture"}))["ok"],
        false
    );
    local.ok(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":15}),
    );
    session
        .server
        .wait_until("the native consent panel", |s| panel_rect(s).is_some());
    // Return cannot approve the initially focused Don't Allow button.
    for pressed in [true, false] {
        session
            .server
            .state
            .process_input_event(common::absolute::key(28, pressed));
        session.server.step();
    }
    session
        .server
        .wait_until("the denied consent panel to close", |s| {
            panel_rect(s).is_none()
        });
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        0
    );
    local.ok(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":15}),
    );
    session
        .server
        .wait_until("fresh native consent", |s| panel_rect(s).is_some());
    let (x, y, _, _) = panel_rect(&session.server.state).unwrap();
    click(&mut session.server, x + 345.0, y + 196.0);
    local.ok(
        &mut session.server,
        json!({"op":"wait","phase":2,"timeout_ms":5000}),
    );
    session
        .server
        .wait_until("consent to close after approval", |s| {
            panel_rect(s).is_none()
        });
    session.server.run_for(Duration::from_millis(800));
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        2
    );
    // A trailing touch frame completes a gesture; it is not a new takeover.
    session
        .server
        .state
        .process_input_event(common::absolute::frame());
    session
        .server
        .state
        .process_input_event(common::absolute::relative(0.0, 0.0));
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        2
    );
    local.ok(
        &mut session.server,
        json!({"op":"move","x":point.0,"y":point.1}),
    );
    for pressed in [true, false] {
        local.ok(
            &mut session.server,
            json!({"op":"button","code":272,"pressed":pressed}),
        );
    }
    for pressed in [true, false] {
        local.ok(
            &mut session.server,
            json!({"op":"key","code":30,"pressed":pressed}),
        );
    }
    session.server.sync(&application);
    assert!(application.keys().contains(&(30, true)));
    assert!(application.presses() > 0);
    let image = local.ok(&mut session.server, json!({"op":"capture"}));
    assert_eq!(image["mime_type"], "image/png");
    assert_eq!(image["width"], OUTPUT.0);
    assert!(image["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    // A physical takeover releases only owned input and stops capture.
    local.ok(
        &mut session.server,
        json!({"op":"key","code":42,"pressed":true}),
    );
    session
        .server
        .state
        .process_input_event(common::absolute::motion(0.1, 0.9));
    local.ok(
        &mut session.server,
        json!({"op":"wait","phase":3,"timeout_ms":5000}),
    );
    session.server.sync(&application);
    assert!(application.keys().contains(&(42, false)));
    assert_eq!(
        local.call(&mut session.server, json!({"op":"capture"}))["ok"],
        false
    );
    local.ok(&mut session.server, json!({"op":"resume"}));
    session.server.wait_until("the resumed consent panel", |s| {
        panel_rect(s).is_some_and(|(x, _, _, h)| x > 100.0 && x < 300.0 && h == 228.0)
    });
    let (x, y, _, _) = panel_rect(&session.server.state).unwrap();
    click(&mut session.server, x + 345.0, y + 196.0);
    local.ok(
        &mut session.server,
        json!({"op":"wait","phase":2,"timeout_ms":5000}),
    );
    let epoch = local.ok(&mut session.server, json!({"op":"status"}))["epoch"].clone();
    // Disconnect while a state wait is pending; the peer, not its deadline, must end the run.
    serde_json::to_writer(
        &mut local.input,
        &json!({"op":"wait","phase":4,"timeout_ms":30000}),
    )
    .unwrap();
    local.input.write_all(b"\n").unwrap();
    local.input.flush().unwrap();
    let disconnected = Instant::now();
    drop(local);
    session
        .server
        .wait_until("the control panel to close after peer exit", |s| {
            panel_rect(s).is_none()
        });
    assert!(disconnected.elapsed() < Duration::from_secs(10));
    let mut next = Local::connect(&mut session);
    let state = next.ok(&mut session.server, json!({"op":"status"}));
    assert_eq!(state["phase"], 0);
    assert_ne!(state["epoch"], epoch);
    assert_eq!(
        next.call(&mut session.server, json!({"op":"capture"}))["ok"],
        false
    );
    next.ok(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":15}),
    );
    session.server.wait_until("another consent panel", |s| {
        panel_rect(s).is_some_and(|(_, _, _, h)| h == 228.0)
    });
    let (x, y, _, _) = panel_rect(&session.server.state).unwrap();
    click(&mut session.server, x + 345.0, y + 196.0);
    next.ok(
        &mut session.server,
        json!({"op":"wait","phase":2,"timeout_ms":5000}),
    );
    session
        .server
        .wait_until("no permanent control panel", |s| panel_rect(s).is_none());
    // The reserved physical shortcut remains available without a status panel.
    for code in [29, 56, 1] {
        session
            .server
            .state
            .process_input_event(common::absolute::key(code, true));
        session.server.step();
    }
    for code in [1, 56, 29] {
        session
            .server
            .state
            .process_input_event(common::absolute::key(code, false));
        session.server.step();
    }
    next.ok(
        &mut session.server,
        json!({"op":"wait","phase":4,"timeout_ms":5000}),
    );
}
