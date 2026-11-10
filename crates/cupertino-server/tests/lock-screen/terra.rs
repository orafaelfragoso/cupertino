//! Opt-in model acceptance: the installed Codex host is the client under test, using its existing authentication.
use super::*;
use std::fs::File;

#[test]
#[ignore = "explicit model acceptance uses the installed Codex host and existing account"]
fn terra_controls_kit_through_the_native_rust_mcp_bridge() {
    let codex = std::env::var_os("CUPERTINO_TERRA_CODEX")
        .expect("set CUPERTINO_TERRA_CODEX to the installed Codex CLI");
    let evidence = PathBuf::from(
        std::env::var_os("CUPERTINO_TERRA_EVIDENCE")
            .expect("set CUPERTINO_TERRA_EVIDENCE outside the disposable test fixture"),
    );
    let Some(mut session) = Session::start() else {
        panic!("Terra acceptance requires an EGL device");
    };
    let _counter = application(&session, "counter");
    session
        .server
        .wait_until("Counter to map", |s| s.windows.iter().any(|e| e.mapped));
    let program = common::fixtures::artifact("cupertino-control", "bin", "cupertino-control");
    session.server.state.supervisor.add_built_ins(
        [(
            cupertino_server::config::BuiltIn::DesktopControl,
            Some(program.clone()),
        )],
        Instant::now(),
    );
    start_shell_supervision(&session.server.state, &session.server.event_loop).unwrap();
    let display = session
        .server
        .state
        .socket_name
        .as_deref()
        .unwrap()
        .to_owned();
    let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
        .join("cupertino")
        .join(format!("control-{display}.sock"));
    session
        .server
        .wait_until("the native controller", |_| socket.exists());
    let output = File::create(&evidence).unwrap();
    let errors = File::create(evidence.with_extension("stderr")).unwrap();
    let args = serde_json::to_string(&vec!["--mcp"]).unwrap();
    let prompt="This is an isolated Cupertino integration fixture. Use only the cupertino MCP tools. Do not use shell, files, browser or other tools. Read status and request semantic and capture capabilities on output headless-1. The test harness will click the real native consent panel using genuine fixture backend input. Wait for active phase, observe Counter, find its + button, click it exactly once with its observed publication revision, and verify the new count is 1 in the tool observation. Capture the output and inspect its image to confirm Counter shows 1. Finally stop this epoch and report the observed count. Do not retry a mutation without inspecting state. Do not request user input; the fixture supplies consent.";
    let mut model = App(std::process::Command::new(codex)
        .args([
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--model",
            "gpt-6.1-terra",
            "--json",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            &format!(
                "mcp_servers.cupertino.command={}",
                serde_json::to_string(&program.to_string_lossy()).unwrap()
            ),
            "-c",
            &format!("mcp_servers.cupertino.args={args}"),
            "-c",
            "mcp_servers.cupertino.env_vars=[\"XDG_RUNTIME_DIR\",\"WAYLAND_DISPLAY\"]",
            "-c",
            "mcp_servers.cupertino.tool_timeout_sec=45",
            "-c",
            "mcp_servers.cupertino.default_tools_approval_mode=\"approve\"",
            prompt,
        ])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("WAYLAND_DISPLAY", &display)
        .envs(common::events::isolated_vars())
        .current_dir(&session.fixture.root)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors)
        .spawn()
        .unwrap());
    let end = Instant::now() + Duration::from_secs(110);
    let mut approved = false;
    loop {
        if !approved {
            if let Some((x, y, _, h)) = panel_rect(&session.server.state) {
                if h == 228.0 {
                    click(&mut session.server, x + 345.0, y + 196.0);
                    approved = true;
                }
            }
        }
        if let Some(status) = model.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "Codex failed; inspect {}",
                evidence.display()
            );
            break;
        }
        assert!(
            Instant::now() < end,
            "model acceptance exceeded deadline; inspect {}",
            evidence.display()
        );
        session.server.step();
    }
    assert!(approved, "the model never requested native consent");
    let events = std::fs::read_to_string(&evidence).unwrap();
    let items: Vec<Value> = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| event.get("item").cloned())
        .collect();
    let tool = |name: &str| {
        items
            .iter()
            .filter(|item| {
                item["type"] == "mcp_tool_call"
                    && item["tool"] == name
                    && item["status"] == "completed"
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        tool("action").len(),
        1,
        "expected one mutation; inspect {}",
        evidence.display()
    );
    let actions = tool("action");
    let action = actions[0];
    let result = &action["result"]["structured_content"];
    // Codex's JSON event field naming is checked against actual host output, not the model's final prose.
    let result = if result.is_null() {
        &action["result"]["structuredContent"]
    } else {
        result
    };
    assert_eq!(result["action"]["outcome"], "applied", "{result}");
    named(
        window(&result["observation"], "org.cupertino.examples.Counter"),
        "1",
    );
    assert_eq!(tool("capture").len(), 1, "model did not inspect an image");
    assert_eq!(tool("stop").len(), 1, "model did not end its run");
}
