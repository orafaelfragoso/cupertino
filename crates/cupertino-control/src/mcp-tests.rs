use super::*;

#[test]
fn stopping_releases_the_native_connection_so_the_next_task_gets_a_fresh_epoch() {
    use std::io::BufRead;
    let (socket, peer) = UnixStream::pair().unwrap();
    let mut native = native();
    native.reader = Some(BufReader::new(socket.try_clone().unwrap()));
    native.lease.lock().unwrap().socket = Some(socket);
    let server = std::thread::spawn(move || {
        let mut peer = BufReader::new(peer);
        for (op, phase) in [("status", 2), ("stop", 4)] {
            let mut line = String::new();
            peer.read_line(&mut line).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["op"], op);
            writeln!(
                peer.get_mut(),
                "{}",
                json!({"ok":true,"data":{"epoch":7,"phase":phase}})
            )
            .unwrap();
        }
    });
    let stopped = native.call(Call::Stop { epoch: "7".into() }).unwrap();
    assert_eq!(stopped["structuredContent"]["phase"], 4);
    assert!(native.reader.is_none());
    assert!(native.lease.lock().unwrap().socket.is_none());
    server.join().unwrap();
}

fn native() -> Native {
    Native {
        session: "test".into(),
        reader: None,
        lease: Arc::new(Mutex::new(Lease::default())),
    }
}
fn modern(method: &str, params: Value) -> Value {
    let mut request = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    request["params"]["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":MODERN,"io.modelcontextprotocol/clientCapabilities":{}});
    request
}
#[test]
fn legacy_and_modern_discovery_work_without_a_desktop_or_grant() {
    let mut protocol = Protocol::default();
    let mut native = native();
    assert_eq!(
        protocol
            .handle(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                &mut native
            )
            .unwrap()["error"]["code"],
        -32602
    );
    let init=protocol.handle(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),&mut native).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(protocol
        .handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &mut native
        )
        .is_none());
    let list = protocol
        .handle(
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            &mut native,
        )
        .unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 12);
    let discover = Protocol::default()
        .handle(modern("server/discover", json!({})), &mut native)
        .unwrap();
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(discover["result"]["supportedVersions"][0], MODERN);
    let mut unsupported = modern("ping", json!({}));
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("1900-01-01");
    assert_eq!(
        protocol.handle(unsupported, &mut native).unwrap()["error"]["code"],
        -32022
    );
    assert!(native.reader.is_none());
}
#[test]
fn malformed_and_extra_arguments_never_reach_the_native_controller() {
    let mut protocol = Protocol::default();
    let mut native = native();
    for args in [
        json!({"name":"exec","arguments":{"command":"bad"}}),
        json!({"name":"status","arguments":{"path":"bad"}}),
        json!({"name":"action","arguments":{"epoch":"1","window":"2","node":"3","revision":"1","action":"exec"}}),
    ] {
        assert_eq!(
            protocol
                .handle(modern("tools/call", args), &mut native)
                .unwrap()["error"]["code"],
            -32602
        );
    }
    assert_eq!(
        protocol.handle(json!([]), &mut native).unwrap()["error"]["code"],
        -32600
    );
    assert!(native.reader.is_none());
}
#[test]
fn identifiers_preserve_all_bits_and_geometry_revisions_keep_their_input_type() {
    assert_eq!(identifier("18446744073709551615").unwrap(), u64::MAX);
    for bad in ["0", "01", "-1", "1.2", "18446744073709551616"] {
        assert!(identifier(bad).is_err());
    }
    let value = string_ids(
        json!({"epoch":u64::MAX,"revision":u64::MAX,"geometry_revision":4,"geometry":{"revision":4},"nodes":[{"id":u64::MAX,"children":[u64::MAX],"focus":null}]}),
    );
    assert_eq!(value["nodes"][0]["id"], u64::MAX.to_string());
    assert_eq!(value["nodes"][0]["children"][0], u64::MAX.to_string());
    assert_eq!(value["geometry"]["revision"], 4);
    assert_eq!(value["geometry_revision"], 4);
}
#[test]
fn filters_report_truncation_and_map_window_bounds_to_the_output() {
    let node = json!({"id":42,"role":"button","name":"Run","bounds":{"x":4,"y":8,"width":20,"height":10},"children":[]});
    let value = json!({"epoch":1,"revision":2,"geometry_revision":3,"windows":[{"handle":2,"origin":[110,220],"scale":[2,2],"snapshot":{"revision":5,"nodes":[node.clone(),node]}}]});
    let output = observation(
        value,
        json!({"x":100,"y":200,"revision":3}),
        Some("2".into()),
        Some("button".into()),
        Some("Run".into()),
        Some(1),
    )
    .unwrap();
    assert_eq!(output["windows"][0]["snapshot"]["matched_nodes"], 2);
    assert_eq!(output["windows"][0]["snapshot"]["truncated"], true);
    assert_eq!(
        output["windows"][0]["snapshot"]["nodes"][0]["output_bounds"],
        json!({"x":18.0,"y":36.0,"width":40.0,"height":20.0})
    );
}
#[test]
fn cancellation_closes_only_the_current_native_lease() {
    use std::io::Read;
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut lease = Lease {
        current: Some(json!(7)),
        socket: Some(socket),
        ..Lease::default()
    };
    lease.cancel(json!(8));
    assert!(lease.socket.is_some());
    lease.cancel(json!(7));
    assert!(lease.socket.is_none());
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
#[test]
fn the_agent_is_told_the_person_decides_once_per_app_and_where_to_change_it() {
    for text in [
        "once per app",
        "Settings › Privacy › Desktop Control or Screen Capture",
        "never pauses control",
        "resumes by itself",
    ] {
        assert!(INSTRUCTIONS.contains(text), "instructions lack {text:?}");
    }
    assert!(
        !INSTRUCTIONS.contains("pauses control;"),
        "physical input no longer pauses a run"
    );
    let tools = tools();
    let description = |name: &str| {
        tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(description("request_control").contains("Settings › Privacy"));
    assert!(description("request_control").contains("remembers the answer"));
    assert!(description("resume").contains("never asks the person again"));
}
#[test]
fn a_remembered_refusal_reaches_the_agent_with_where_the_person_can_change_it() {
    use std::io::BufRead;
    let (socket, peer) = UnixStream::pair().unwrap();
    let mut native = native();
    native.reader = Some(BufReader::new(socket.try_clone().unwrap()));
    native.lease.lock().unwrap().socket = Some(socket);
    let server = std::thread::spawn(move || {
        let mut peer = BufReader::new(peer);
        let mut line = String::new();
        peer.read_line(&mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["op"], "request");
        assert_eq!(request["capabilities"], 3);
        writeln!(
            peer.get_mut(),
            "{}",
            json!({"ok":false,"error":crate::refusal(2)})
        )
        .unwrap();
    });
    let mut protocol = Protocol::default();
    let refused = protocol
        .handle(
            modern(
                "tools/call",
                json!({"name":"request_control","arguments":{"output":"headless-1","capabilities":["observe","capture"]}}),
            ),
            &mut native,
        )
        .unwrap();
    server.join().unwrap();
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let error = refused["result"]["structuredContent"]["error"]
        .as_str()
        .unwrap();
    assert!(error.contains("(denied)"), "{error}");
    assert!(
        error.contains("Settings › Privacy › Desktop Control or Screen Capture"),
        "{error}"
    );
    // a refusal leaves the connection to the controller open for the next request
    assert!(native.reader.is_some());
}
