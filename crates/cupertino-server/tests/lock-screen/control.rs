//! The shipped Rust controller and native Kit consent UI run together under an isolated session.
#[path = "semantics.rs"]
mod semantics;
#[path = "stability.rs"]
mod stability;
use super::*;
use cupertino_defaults::desktop_control as remembered;
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
/// The session's remembered answers: a file in the fixture's settings folder, which the server watches.
struct Remembered {
    path: PathBuf,
}
impl Remembered {
    fn watched(session: &mut Session) -> Self {
        let dir = session.fixture.root.join("config").join("cupertino");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(remembered::DESKTOP_CONTROL_FILE);
        session.server.state.start_settings_watch(
            cupertino_server::config::SettingsFiles::default(),
            cupertino_server::live::SettingsPaths {
                desktop_control: Some(path.clone()),
                ..Default::default()
            },
        );
        Self { path }
    }
    fn decision(&self, service: remembered::Service) -> Option<remembered::Decision> {
        let (conf, _) = remembered::DesktopControlConf::load(&self.path).unwrap();
        conf.decision(service, &this_program())
    }
    /// Remove the app's answers, as the settings application does, and wait until the server read the change.
    fn forget(&self, server: &mut Server) {
        let before = server.state.live.reloads;
        remembered::DesktopControlConf::update(&self.path, |conf| {
            for service in remembered::Service::ALL {
                conf.forget(service, &this_program());
            }
            Ok(())
        })
        .unwrap();
        server.wait_until("the removed answers to be read", |s| {
            s.live.reloads > before
        });
    }
}
/// The app of the runs: the program that started the bridge, this test.
fn this_program() -> remembered::App {
    remembered::App::Executable(std::env::current_exe().unwrap())
}
#[test]
fn native_controller_and_session_consent_remember_the_answer_and_share_the_desktop() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (application, point) = session.server.application();
    start_controller(&mut session);
    let store = Remembered::watched(&mut session);
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
    // Return activates the initially focused Don't Allow: the person's refusal is remembered.
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
    for service in remembered::Service::ALL {
        assert_eq!(store.decision(service), Some(remembered::Decision::Deny));
    }
    // A remembered refusal is not asked again, and tells where the person can change it.
    let refused = local.call(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":15}),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Settings › Privacy › Desktop Control"),
        "{refused}"
    );
    session.server.run_for(Duration::from_millis(200));
    assert!(panel_rect(&session.server.state).is_none());
    // Removed in Settings, the app is asked again.
    store.forget(&mut session.server);
    local.ok(
        &mut session.server,
        json!({"op":"request","output":"headless-1","capabilities":15}),
    );
    session.server.wait_until("consent asked again", |s| {
        panel_rect(s).is_some_and(|(x, _, _, h)| x > 100.0 && x < 300.0 && h == 228.0)
    });
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
    for service in remembered::Service::ALL {
        assert_eq!(store.decision(service), Some(remembered::Decision::Allow));
    }
    session.server.run_for(Duration::from_millis(800));
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
    // The person uses the desktop at the same time: the run goes on, holding what it pressed.
    local.ok(
        &mut session.server,
        json!({"op":"key","code":42,"pressed":true}),
    );
    session
        .server
        .state
        .process_input_event(common::absolute::motion(0.1, 0.9));
    for pressed in [true, false] {
        session
            .server
            .state
            .process_input_event(common::absolute::key(48, pressed));
    }
    session
        .server
        .state
        .process_input_event(common::absolute::frame());
    session.server.run_for(Duration::from_millis(200));
    assert_eq!(
        local.ok(&mut session.server, json!({"op":"status"}))["phase"],
        2
    );
    session.server.sync(&application);
    assert!(!application.keys().contains(&(42, false)));
    let image = local.ok(&mut session.server, json!({"op":"capture"}));
    assert_eq!(image["width"], OUTPUT.0);
    // Resuming a run that is not suspended asks nobody.
    local.ok(&mut session.server, json!({"op":"resume"}));
    session.server.run_for(Duration::from_millis(200));
    assert!(panel_rect(&session.server.state).is_none());
    local.ok(
        &mut session.server,
        json!({"op":"key","code":42,"pressed":false}),
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
        .wait_until("the run to end after peer exit", |s| !s.control_capturing());
    assert!(disconnected.elapsed() < Duration::from_secs(10));
    // The next bridge of the same app starts at once, without a panel.
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
    next.ok(
        &mut session.server,
        json!({"op":"wait","phase":2,"timeout_ms":5000}),
    );
    session.server.run_for(Duration::from_millis(300));
    assert!(panel_rect(&session.server.state).is_none());
    // The reserved physical shortcut ends the run and leaves the person's answer as it is.
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
    for service in remembered::Service::ALL {
        assert_eq!(store.decision(service), Some(remembered::Decision::Allow));
    }
}
