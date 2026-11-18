use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

#[test]
fn discovery_works_before_a_controller_is_available() {
    // The bridge sets its stdout nonblocking. Stdin must have its own open file description,
    // just as it does when an MCP host gives the child a pipe in each direction.
    let (mut requests, input) = UnixStream::pair().unwrap();
    let (replies, output) = UnixStream::pair().unwrap();
    replies
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cupertino-control"))
        .args(["--mcp", "offline"])
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(OwnedFd::from(input))
        .stdout(OwnedFd::from(output))
        .spawn()
        .unwrap();
    let mut replies = BufReader::new(replies);
    let mut call = |id: u32, method: &str, params: Value| {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(requests, "{request}").unwrap();
        let mut line = String::new();
        replies.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        reply
    };

    let init = call(1, "initialize", json!({"protocolVersion": "2025-11-25"}));
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let list = call(2, "tools/list", json!({}));
    assert!(list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "request_control"));
    requests.shutdown(Shutdown::Write).unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn implicit_session_requires_the_hosts_display() {
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-control"))
        .arg("--mcp")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("WAYLAND_DISPLAY"));
    assert!(output.stdout.is_empty());
}
