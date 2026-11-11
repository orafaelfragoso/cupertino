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
        capture_regions: false,
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
        "each in a run of its own",
    ] {
        assert!(INSTRUCTIONS.contains(text), "instructions lack {text:?}");
    }
    let bound = cupertino_control_model::MAX_RUNS_PER_CONTROLLER;
    assert!(
        INSTRUCTIONS.contains(&format!("serves at most {bound} agents at once")),
        "the instructions name the controller's bound"
    );
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
    assert!(description("request_control").contains("waits its turn in phase 1"));
    assert!(description("stop").contains("other agents' runs go on"));
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

/// A bridge connected to a controller that answers each request with the next of `answers` (a status for epoch 7 comes
/// first on its own), and the requests it got once the bridge is gone.
fn answering(
    capture_regions: bool,
    answers: Vec<Value>,
) -> (Native, std::thread::JoinHandle<Vec<Value>>) {
    let status = json!({"epoch":7,"phase":2});
    replying(
        capture_regions,
        std::iter::once(status).chain(answers).collect(),
    )
}
/// A bridge connected to a controller that answers each request with the next of `answers`, and the requests it got
/// once the bridge is gone.
fn replying(
    capture_regions: bool,
    answers: Vec<Value>,
) -> (Native, std::thread::JoinHandle<Vec<Value>>) {
    use std::io::BufRead;
    let (socket, peer) = UnixStream::pair().unwrap();
    let mut native = native();
    native.capture_regions = capture_regions;
    native.reader = Some(BufReader::new(socket.try_clone().unwrap()));
    native.lease.lock().unwrap().socket = Some(socket);
    let server = std::thread::spawn(move || {
        let mut peer = BufReader::new(peer);
        let mut asked = Vec::new();
        for answer in answers {
            let mut line = String::new();
            if peer.read_line(&mut line).unwrap() == 0 {
                break;
            }
            asked.push(serde_json::from_str::<Value>(&line).unwrap());
            writeln!(peer.get_mut(), "{}", json!({"ok":true,"data":answer})).unwrap();
        }
        // whatever else the bridge sends before it is gone
        let mut line = String::new();
        while peer.read_line(&mut line).unwrap_or(0) > 0 {
            asked.push(serde_json::from_str::<Value>(&line).unwrap());
            line.clear();
        }
        asked
    });
    (native, server)
}
fn close(native: Native) {
    native.lease.lock().unwrap().close();
}
const GEOMETRY: &str =
    r#"{"revision":3,"x":0,"y":0,"width":2560,"height":1440,"scale":1.5,"name":"DP-1"}"#;

#[test]
fn a_region_capture_passes_the_region_and_reports_the_rectangle_the_image_covers() {
    let region = json!({"x":10.2,"y":0.1,"width":10.0,"height":10.0});
    let covered = json!({"x":10.0,"y":0.0,"width":10.666666666666666,"height":10.666666666666666});
    let geometry: Value = serde_json::from_str(GEOMETRY).unwrap();
    let (mut native, server) = answering(
        true,
        vec![
            json!({"mime_type":"image/png","width":16,"height":16,"region":covered,"geometry":geometry,"data":"iVBORw0KGgo="}),
        ],
    );
    let region_arg: Region = serde_json::from_value(region.clone()).unwrap();
    let image = native
        .call(Call::Capture {
            epoch: "7".into(),
            region: Some(region_arg),
        })
        .unwrap();
    close(native);
    let asked = server.join().unwrap();
    assert_eq!(asked[1], json!({"op":"capture","region":region}));
    assert_eq!(image["structuredContent"]["region"], covered);
    assert_eq!(image["structuredContent"]["width"], 16);
    assert!(image["structuredContent"].get("data").is_none());
    assert_eq!(image["content"][1]["type"], "image");
    let text: Value = serde_json::from_str(image["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["region"], covered, "the text content says it too");
}

#[test]
fn an_old_controller_is_never_sent_a_region_it_would_ignore() {
    let (mut native, server) = answering(false, vec![]);
    let error = native
        .call(Call::Capture {
            epoch: "7".into(),
            region: Some(Region {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            }),
        })
        .unwrap_err();
    assert_eq!(error, OLD_CONTROLLER);
    assert!(error.contains("next login") && error.contains("without a region"));
    close(native);
    let asked = server.join().unwrap();
    assert_eq!(asked, [json!({"op":"status"})], "no capture was asked for");
}

#[test]
fn a_whole_capture_from_an_old_controller_covers_its_geometry() {
    let geometry: Value = serde_json::from_str(GEOMETRY).unwrap();
    let (mut native, server) = answering(
        false,
        vec![
            json!({"mime_type":"image/png","width":3840,"height":2160,"geometry":geometry,"data":"iVBORw0KGgo="}),
        ],
    );
    let image = native
        .call(Call::Capture {
            epoch: "7".into(),
            region: None,
        })
        .unwrap();
    close(native);
    assert_eq!(server.join().unwrap()[1], json!({"op":"capture"}));
    let result = &image["structuredContent"];
    // image pixel (px, py) still maps through region: 3840 pixels over 2560 points
    assert_eq!(
        result["region"],
        json!({"x":0.0,"y":0.0,"width":2560.0,"height":1440.0})
    );
    assert_eq!(result["width"], 3840);
}

#[test]
fn a_result_without_a_region_for_a_region_request_is_refused() {
    let mut result = json!({"width":3840,"height":2160,"geometry":{"width":2560,"height":1440}});
    assert_eq!(covered(&mut result, true).unwrap_err(), OLD_CONTROLLER);
    let mut ours =
        json!({"width":60,"height":60,"region":{"x":1.0,"y":2.0,"width":40.0,"height":40.0}});
    let before = ours.clone();
    covered(&mut ours, true).unwrap();
    assert_eq!(ours, before);
    assert!(covered(&mut json!({"width":1}), false).is_err());
}

#[test]
fn an_image_over_the_limit_asks_for_a_smaller_region() {
    let (mut native, server) = answering(
        true,
        vec![
            json!({"mime_type":"image/png","width":1,"height":1,"region":{"x":0.0,"y":0.0,"width":1.0,"height":1.0},"geometry":{},"data":"A".repeat(MAX_IMAGE + 1)}),
        ],
    );
    let error = native
        .call(Call::Capture {
            epoch: "7".into(),
            region: None,
        })
        .unwrap_err();
    close(native);
    server.join().unwrap();
    assert!(error.contains("smaller region"), "{error}");
}

#[test]
fn the_capture_tool_takes_a_strict_region_and_explains_the_mapping() {
    let tools = tools();
    let capture = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "capture")
        .unwrap()
        .clone();
    let mapping = "(region.x + px * region.width / width, region.y + py * region.height / height)";
    assert!(capture["description"].as_str().unwrap().contains(mapping));
    assert!(INSTRUCTIONS.contains(mapping));
    assert!(INSTRUCTIONS.contains("same logical points as move"));
    let region = &capture["inputSchema"]["properties"]["region"];
    assert_eq!(region["additionalProperties"], false);
    assert_eq!(region["required"], json!(["x", "y", "width", "height"]));
    assert_eq!(capture["inputSchema"]["required"], json!(["epoch"]));
    // the bridge parses exactly that shape
    let mut protocol = Protocol::default();
    let mut native = native();
    for region in [
        json!({"x":0,"y":0,"width":10}),
        json!({"x":0,"y":0,"width":10,"height":10,"scale":2}),
        json!("0,0,10,10"),
    ] {
        let call = modern(
            "tools/call",
            json!({"name":"capture","arguments":{"epoch":"7","region":region}}),
        );
        assert_eq!(
            protocol.handle(call, &mut native).unwrap()["error"]["code"],
            -32602,
            "{region}"
        );
    }
    assert!(native.reader.is_none());
}

#[test]
fn a_controller_from_before_regions_would_ignore_the_field() {
    // the native request parser of a controller from before regions, as it was: capture took no fields
    #[derive(Deserialize, Debug)]
    #[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
    enum Before {
        Capture,
        Stop,
    }
    let asked = r#"{"op":"capture","region":{"x":0,"y":0,"width":10,"height":10}}"#;
    assert!(
        matches!(serde_json::from_str::<Before>(asked), Ok(Before::Capture)),
        "an older controller accepts the request and returns the whole output, so the bridge must not send it"
    );
    assert!(serde_json::from_str::<Before>(r#"{"op":"stop"}"#).is_ok());
}

#[test]
fn a_call_naming_a_replaced_epoch_never_reaches_the_new_run() {
    // a later request_control replaced run 7 by run 8
    let (mut native, server) = replying(false, vec![json!({"epoch":8,"phase":2})]);
    let error = native
        .call(Call::Key {
            epoch: "7".into(),
            code: 30,
            pressed: true,
        })
        .unwrap_err();
    assert_eq!(error, STALE_EPOCH);
    assert!(error.contains("stale epoch") && error.contains("nothing was done"));
    close(native);
    assert_eq!(
        server.join().unwrap(),
        [json!({"op":"status"})],
        "nothing but status was asked"
    );
}

#[test]
fn a_call_naming_an_ended_run_is_refused_except_waiting_and_stopping() {
    let ended = json!({"epoch":7,"phase":4});
    let (mut native, server) = replying(
        false,
        vec![
            ended.clone(),
            ended.clone(),
            ended.clone(),
            ended.clone(),
            ended.clone(),
            ended,
        ],
    );
    for call in [
        Call::Observe {
            epoch: "7".into(),
            window: None,
            role: None,
            name: None,
            max_nodes: None,
        },
        Call::Resume { epoch: "7".into() },
    ] {
        assert_eq!(native.call(call).unwrap_err(), REVOKED);
    }
    assert!(REVOKED.contains("request_control"));
    let waited = native
        .call(Call::Wait {
            epoch: "7".into(),
            phase: 4,
            timeout_ms: 10,
        })
        .unwrap();
    assert_eq!(waited["structuredContent"]["phase"], 4);
    let stopped = native.call(Call::Stop { epoch: "7".into() }).unwrap();
    assert_eq!(stopped["structuredContent"]["phase"], 4);
    let asked = server.join().unwrap();
    let ops: Vec<_> = asked.iter().map(|r| r["op"].clone()).collect();
    assert_eq!(
        ops,
        ["status", "status", "status", "wait", "status", "stop"],
        "only the wait and the stop reached the controller"
    );
}

#[test]
fn request_control_after_the_run_ended_asks_the_controller_for_a_fresh_run_without_a_stop() {
    let (mut native, server) = replying(false, vec![json!({"epoch":8,"phase":2})]);
    let mut protocol = Protocol::default();
    let started = protocol
        .handle(
            modern(
                "tools/call",
                json!({"name":"request_control","arguments":{"output":"headless-1","capabilities":["semantic"]}}),
            ),
            &mut native,
        )
        .unwrap();
    assert_eq!(started["result"]["isError"], false, "{started}");
    assert_eq!(started["result"]["structuredContent"]["epoch"], "8");
    // the connection stays: the controller replaced the ended run on it
    assert!(native.reader.is_some());
    close(native);
    assert_eq!(
        server.join().unwrap(),
        [json!({"op":"request","output":"headless-1","capabilities":17})]
    );
}

#[test]
fn the_agent_is_told_requests_may_be_repeated_and_start_fresh_after_the_run_ended() {
    for text in [
        "request_control is safe to repeat",
        "returns that run and its epoch",
        "another output or more capabilities ends that run",
        "starts a fresh run with a new epoch, without stopping first",
        "refused and does nothing",
    ] {
        assert!(INSTRUCTIONS.contains(text), "instructions lack {text:?}");
    }
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
    assert!(description("request_control").contains("no stop needed"));
    assert!(description("stop").contains("A later request_control starts a fresh run"));
}
