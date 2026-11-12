//! The real Kit examples, compositor, native consent authority and shipped MCP bridge, without touching the login session.
use super::*;
use cupertino_defaults::desktop_control as remembered;
#[path = "terra.rs"]
mod terra;

struct App(Child);
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn application(session: &Session, example: &str) -> App {
    let program = common::fixtures::artifact("cupertino-kit", "example", example);
    let mut command = common::spawn::cleared(&program, &session.fixture.root.join("home"));
    App(command
        .env(
            "XDG_RUNTIME_DIR",
            std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
        )
        .env(
            "WAYLAND_DISPLAY",
            session.server.state.socket_name.as_deref().unwrap(),
        )
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap())
}
struct Mcp {
    local: Local,
    id: u64,
}
impl Mcp {
    fn connect(session: &mut Session) -> Self {
        let program = common::fixtures::artifact("cupertino-control", "bin", "cupertino-control");
        let mut command = common::spawn::cleared(&program, &session.fixture.root.join("home"));
        let mut child = command
            .args(["--mcp"])
            .env(
                "WAYLAND_DISPLAY",
                session.server.state.socket_name.as_deref().unwrap(),
            )
            .env(
                "XDG_RUNTIME_DIR",
                std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let local = Local {
            input: child.stdin.take().unwrap(),
            output: Some(BufReader::new(child.stdout.take().unwrap())),
            child,
        };
        Self { local, id: 0 }
    }
    fn rpc(&mut self, server: &mut Server, method: &str, params: Value) -> Value {
        self.id += 1;
        let response = self.local.call(
            server,
            json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}),
        );
        assert_eq!(response["id"], self.id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
    fn tool(&mut self, server: &mut Server, name: &str, args: Value) -> Value {
        self.rpc(server, "tools/call", json!({"name":name,"arguments":args}))
    }
    #[track_caller]
    fn ok(&mut self, server: &mut Server, name: &str, args: Value) -> Value {
        let response = self.tool(server, name, args);
        assert_eq!(response["isError"], false, "{response}");
        response["structuredContent"].clone()
    }
}
fn window<'a>(observation: &'a Value, app_id: &str) -> &'a Value {
    observation["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["app_id"] == app_id)
        .unwrap_or_else(|| panic!("missing {app_id}: {observation}"))
}
fn named<'a>(window: &'a Value, name: &str) -> &'a Value {
    window["snapshot"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == name)
        .unwrap_or_else(|| panic!("missing {name}: {window}"))
}

#[test]
fn rust_mcp_remembers_the_answer_and_a_new_run_after_stopping_starts_at_once() {
    let Some(mut session) = Session::start() else {
        return;
    };
    start_controller(&mut session);
    let store = Remembered::watched(&mut session);
    let mut mcp = Mcp::connect(&mut session);
    mcp.rpc(&mut session.server, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"reuse-test","version":"1"}}));
    let first = mcp.ok(&mut session.server, "status", json!({}));
    let epoch = first["epoch"].clone();
    mcp.ok(
        &mut session.server,
        "request_control",
        json!({"output":"headless-1","capabilities":["observe","capture"]}),
    );
    session
        .server
        .wait_until("initial consent", |s| panel_rect(s).is_some());
    allow(&mut session.server);
    mcp.ok(
        &mut session.server,
        "wait",
        json!({"epoch":epoch,"phase":2,"timeout_ms":5000}),
    );
    for service in remembered::Service::ALL {
        assert_eq!(store.decision(service), Some(remembered::Decision::Allow));
    }
    mcp.ok(&mut session.server, "stop", json!({"epoch":epoch}));
    let next = mcp.ok(&mut session.server, "status", json!({}));
    assert_eq!(next["phase"], 0);
    assert_ne!(next["epoch"], epoch);
    // The old epoch's grant is gone with its run.
    assert!(
        mcp.tool(&mut session.server, "observe", json!({"epoch":epoch}))["isError"]
            .as_bool()
            .unwrap()
    );
    // The app's remembered answer starts the next run without asking.
    mcp.ok(
        &mut session.server,
        "request_control",
        json!({"output":"headless-1","capabilities":["observe"]}),
    );
    mcp.ok(
        &mut session.server,
        "wait",
        json!({"epoch":next["epoch"],"phase":2,"timeout_ms":5000}),
    );
    session.server.run_for(Duration::from_millis(300));
    assert!(panel_rect(&session.server.state).is_none());
    mcp.ok(
        &mut session.server,
        "observe",
        json!({"epoch":next["epoch"]}),
    );
    mcp.ok(&mut session.server, "stop", json!({"epoch":next["epoch"]}));
}

#[test]
fn rust_mcp_controls_real_kit_nodes_with_consent_stale_checks_and_protected_values() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (_third_party, _) = session.server.application();
    let counter_app = application(&session, "counter");
    let _forms = application(&session, "forms");
    session.server.wait_until("the real Kit examples", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 3
    });
    start_controller(&mut session);
    let mut mcp = Mcp::connect(&mut session);
    assert_eq!(mcp.rpc(&mut session.server,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}))["protocolVersion"],"2025-11-25");
    assert_eq!(
        mcp.rpc(&mut session.server, "tools/list", json!({}))["tools"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
    let initial = mcp.ok(&mut session.server, "status", json!({}));
    let epoch = initial["epoch"].clone();
    assert_eq!(initial["outputs"], json!(["headless-1"]));
    assert_eq!(
        mcp.tool(&mut session.server, "observe", json!({"epoch":epoch}))["isError"],
        true
    );
    mcp.ok(
        &mut session.server,
        "request_control",
        json!({"output":"headless-1","capabilities":["semantic","capture","pointer","keyboard"]}),
    );
    session.server.wait_until("native semantic consent", |s| {
        panel_rect(s).is_some_and(|(_, _, _, h)| h == CONSENT_HEIGHT)
    });
    assert_eq!(
        mcp.tool(&mut session.server, "observe", json!({"epoch":epoch}))["isError"],
        true
    );
    allow(&mut session.server);
    mcp.ok(
        &mut session.server,
        "wait",
        json!({"epoch":epoch,"phase":2,"timeout_ms":5000}),
    );
    session.server.run_for(Duration::from_millis(500));
    let observed = mcp.ok(&mut session.server, "observe", json!({"epoch":epoch}));
    assert!(window(&observed, APP_ID)["snapshot"].is_null());
    assert!(!observed.to_string().contains("desktop-control-consent"));
    let counter = window(&observed, "org.cupertino.examples.Counter");
    named(counter, "0");
    let button = named(counter, "+");
    let action = json!({"epoch":epoch,"window":counter["handle"],"revision":counter["snapshot"]["revision"],"node":button["id"],"action":"click"});
    let changed = mcp.ok(&mut session.server, "action", action.clone());
    assert_eq!(changed["action"]["outcome"], "applied");
    named(
        window(&changed["observation"], "org.cupertino.examples.Counter"),
        "1",
    );
    assert_eq!(
        mcp.tool(&mut session.server, "action", action)["isError"],
        true
    );
    let forms = window(&observed, "org.cupertino.examples.Forms");
    let password = named(forms, "Password");
    assert_eq!(password["value"], json!({"kind":"protected"}));
    assert_eq!(password["actions"], json!([]));
    assert_eq!(mcp.tool(&mut session.server,"action",json!({"epoch":epoch,"window":forms["handle"],"revision":forms["snapshot"]["revision"],"node":password["id"],"action":"set-value","data":{"kind":"text","value":"secret"}}))["isError"],true);
    let name = named(forms, "Name");
    let changed=mcp.ok(&mut session.server,"action",json!({"epoch":epoch,"window":forms["handle"],"revision":forms["snapshot"]["revision"],"node":name["id"],"action":"set-value","data":{"kind":"text","value":"Terra ✓"}}));
    assert_eq!(
        named(
            window(&changed["observation"], "org.cupertino.examples.Forms"),
            "Name"
        )["value"]["text"],
        "Terra ✓"
    );
    let image = mcp.tool(&mut session.server, "capture", json!({"epoch":epoch}));
    assert_eq!(image["isError"], false, "{image}");
    assert_eq!(image["content"][1]["mimeType"], "image/png");
    assert!(image["content"][1]["data"]
        .as_str()
        .unwrap()
        .starts_with("iVBORw0KGgo"));
    assert!(image["structuredContent"].get("data").is_none());
    // the shipped bridge learns at its handshake that the controller takes regions
    let part = mcp.ok(
        &mut session.server,
        "capture",
        json!({"epoch":epoch,"region":{"x":8,"y":4,"width":64,"height":32}}),
    );
    assert_eq!(
        (part["width"].as_u64(), part["height"].as_u64()),
        (Some(64), Some(32))
    );
    assert_eq!(
        part["region"],
        json!({"x":8.0,"y":4.0,"width":64.0,"height":32.0})
    );
    // The counter and form mutations changed published content, not where any window lies: the first observation's
    // coordinate token still holds.
    let moved = mcp.tool(
        &mut session.server,
        "move",
        json!({"epoch":epoch,"x":20,"y":20,"geometry_revision":observed["geometry_revision"]}),
    );
    assert_eq!(moved["isError"], false, "{moved}");
    // A stopped fixture app cannot complete its accepted action. The compositor must expire it even with no input.
    let fresh = mcp.ok(&mut session.server, "observe", json!({"epoch":epoch}));
    let counter = window(&fresh, "org.cupertino.examples.Counter");
    let plus = named(counter, "+");
    // Safety: this unreaped Child owns the exact fixture PID; it cannot have been reused.
    assert_eq!(
        unsafe { libc::kill(counter_app.0.id() as libc::pid_t, libc::SIGSTOP) },
        0
    );
    let began = Instant::now();
    let timed=mcp.tool(&mut session.server,"action",json!({"epoch":epoch,"window":counter["handle"],"revision":counter["snapshot"]["revision"],"node":plus["id"],"action":"click"}));
    // Safety: the same owned fixture child is resumed before the remaining interactions.
    assert_eq!(
        unsafe { libc::kill(counter_app.0.id() as libc::pid_t, libc::SIGCONT) },
        0
    );
    assert_eq!(timed["isError"], true, "{timed}");
    assert!(
        timed["structuredContent"]["error"]
            .as_str()
            .unwrap()
            .contains("unknown"),
        "{timed}"
    );
    assert!(
        began.elapsed() < Duration::from_secs(7),
        "native action deadline did not wake the loop"
    );
    // The person's own input does not pause the run.
    session
        .server
        .state
        .process_input_event(common::absolute::motion(0.1, 0.9));
    session.server.run_for(Duration::from_millis(200));
    assert_eq!(mcp.ok(&mut session.server, "status", json!({}))["phase"], 2);
    mcp.ok(&mut session.server, "observe", json!({"epoch":epoch}));
    // A matching MCP cancellation interrupts the blocking native wait and revokes its lease.
    mcp.id += 1;
    for request in [
        json!({"jsonrpc":"2.0","id":mcp.id,"method":"tools/call","params":{"name":"wait","arguments":{"epoch":epoch,"phase":4,"timeout_ms":30000}}}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":mcp.id}}),
    ] {
        serde_json::to_writer(&mut mcp.local.input, &request).unwrap();
        mcp.local.input.write_all(b"\n").unwrap();
        mcp.local.input.flush().unwrap();
        session.server.run_for(Duration::from_millis(100));
    }
    session
        .server
        .wait_until("the cancelled MCP grant to end", |s| !s.control_capturing());
    let next = mcp.ok(&mut session.server, "status", json!({}));
    assert_ne!(next["epoch"], epoch);
    assert_eq!(next["phase"], 0);
}
