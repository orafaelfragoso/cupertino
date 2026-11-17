//! The semantic wire values: JSON shape, snapshot validation, action permission and sealed memfd payloads.

use cupertino_app_protocols::semantics::*;
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};

fn node(id: u64, children: &[u64]) -> Node {
    Node {
        truncated: false,
        position: Position::default(),
        id,
        role: "group".into(),
        name: None,
        description: None,
        value: None,
        states: States::default(),
        bounds: Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
        actions: vec![],
        children: children.to_vec(),
    }
}

fn snap(nodes: Vec<Node>) -> Snapshot {
    Snapshot {
        revision: 1,
        root: 1,
        focus: None,
        nodes,
    }
}

/// A root (id 1) with one child (id 2) holding `value` and `actions`.
fn with_field(value: Value, actions: Vec<Action>) -> Snapshot {
    let mut field = node(2, &[]);
    field.role = "text-input".into();
    field.value = Some(value);
    field.actions = actions;
    snap(vec![node(1, &[2]), field])
}

fn text(s: &str) -> Value {
    Value::Text { text: s.into() }
}

fn err(s: &Snapshot) -> &'static str {
    s.validate().unwrap_err()
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(v: &T) {
    assert_eq!(&serde_json::from_str::<T>(&json(v)).unwrap(), v);
}

fn memfd(sealing: bool) -> File {
    let flags = if sealing {
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING
    } else {
        rustix::fs::MemfdFlags::CLOEXEC
    };
    File::from(rustix::fs::memfd_create("test", flags).unwrap())
}

#[test]
fn limits_are_the_documented_values() {
    assert_eq!(MAX_FRAME, 512 * 1024);
    assert_eq!(MAX_OBSERVATION, 4 * 1024 * 1024);
    assert_eq!(MAX_NODES, 4096);
    assert_eq!(MAX_TEXT, 4096);
    assert_eq!(MAX_DEPTH, 64);
    assert_eq!(MAX_WINDOWS, 64);
}

#[test]
fn bounds_must_be_finite_and_non_negative() {
    let b = |x, y, width, height| Bounds {
        x,
        y,
        width,
        height,
    };
    assert!(b(-5.0, -5.0, 0.0, 0.0).valid());
    assert!(!b(0.0, 0.0, -1.0, 1.0).valid());
    assert!(!b(0.0, 0.0, 1.0, -0.5).valid());
    assert!(!b(f64::NAN, 0.0, 1.0, 1.0).valid());
    assert!(!b(0.0, f64::INFINITY, 1.0, 1.0).valid());
    assert!(!b(0.0, 0.0, f64::NAN, 1.0).valid());
    assert!(!b(0.0, 0.0, 1.0, f64::NEG_INFINITY).valid());
}

#[test]
fn protected_is_by_role_or_value() {
    let mut n = node(1, &[]);
    assert!(!n.protected());
    n.role = "password-input".into();
    assert!(n.protected());
    n.role = "text-input".into();
    n.value = Some(Value::Protected {});
    assert!(n.protected());
    n.value = Some(text("x"));
    assert!(!n.protected());
}

#[test]
fn a_well_formed_tree_validates() {
    let mut s = snap(vec![
        node(1, &[2, 3]),
        node(2, &[4]),
        node(3, &[]),
        node(4, &[]),
    ]);
    s.focus = Some(4);
    assert_eq!(s.validate(), Ok(()));
    assert_eq!(snap(vec![node(1, &[])]).validate(), Ok(()));
}

#[test]
fn size_and_revision_are_checked() {
    let mut s = snap(vec![node(1, &[])]);
    s.revision = 0;
    assert_eq!(err(&s), "invalid semantic snapshot size or revision");
    assert_eq!(
        err(&snap(vec![])),
        "invalid semantic snapshot size or revision"
    );

    let ids: Vec<u64> = (2..=MAX_NODES as u64).collect();
    let mut nodes = vec![node(1, &ids)];
    nodes.extend(ids.iter().map(|i| node(*i, &[])));
    assert_eq!(nodes.len(), MAX_NODES);
    assert_eq!(snap(nodes.clone()).validate(), Ok(()));
    nodes[0].children.push(MAX_NODES as u64 + 1);
    nodes.push(node(MAX_NODES as u64 + 1, &[]));
    assert_eq!(
        err(&snap(nodes)),
        "invalid semantic snapshot size or revision"
    );
}

#[test]
fn node_fields_are_checked() {
    let bad = |edit: &dyn Fn(&mut Node)| {
        let mut n = node(2, &[]);
        edit(&mut n);
        assert_eq!(err(&snap(vec![node(1, &[2]), n])), "invalid semantic node");
    };
    bad(&|n| n.id = 0);
    bad(&|n| n.role = String::new());
    bad(&|n| n.role = "r".repeat(65));
    bad(&|n| n.bounds.width = -1.0);
    bad(&|n| n.bounds.x = f64::NAN);
    bad(&|n| n.actions = vec![Action::Click; 11]);
    bad(&|n| n.children = vec![1; MAX_NODES + 1]);

    let mut n = node(2, &[]);
    n.role = "r".repeat(64);
    n.actions = vec![Action::Click; 10];
    assert_eq!(snap(vec![node(1, &[2]), n]).validate(), Ok(()));
}

#[test]
fn duplicate_ids_are_refused() {
    assert_eq!(
        err(&snap(vec![node(1, &[2]), node(2, &[]), node(2, &[])])),
        "invalid semantic node"
    );
}

#[test]
fn text_is_bounded_in_bytes() {
    let long = "a".repeat(MAX_TEXT + 1);
    let mut s = snap(vec![node(1, &[])]);
    s.nodes[0].name = Some(long.clone());
    assert_eq!(err(&s), "semantic text exceeds limit");
    s.nodes[0].name = None;
    s.nodes[0].description = Some(long.clone());
    assert_eq!(err(&s), "semantic text exceeds limit");
    s.nodes[0].description = Some("a".repeat(MAX_TEXT));
    assert_eq!(s.validate(), Ok(()));
    // The bound counts bytes: 2049 two-byte characters exceed it.
    s.nodes[0].description = Some("é".repeat(MAX_TEXT / 2 + 1));
    assert_eq!(err(&s), "semantic text exceeds limit");

    assert_eq!(
        err(&with_field(text(&long), vec![])),
        "semantic value exceeds limit"
    );
    assert_eq!(
        with_field(text(&"a".repeat(MAX_TEXT)), vec![]).validate(),
        Ok(())
    );
}

#[test]
fn numbers_must_be_finite_ordered_with_a_positive_step() {
    let num = |value, min, max, step| Value::Number {
        value,
        min,
        max,
        step,
    };
    assert_eq!(
        with_field(num(5.0, 0.0, 10.0, Some(1.0)), vec![]).validate(),
        Ok(())
    );
    assert_eq!(
        with_field(num(5.0, 5.0, 5.0, None), vec![]).validate(),
        Ok(())
    );
    for v in [
        num(f64::NAN, 0.0, 10.0, None),
        num(f64::INFINITY, 0.0, 10.0, None),
        num(5.0, f64::NEG_INFINITY, 10.0, None),
        num(5.0, 0.0, f64::NAN, None),
        num(5.0, 10.0, 0.0, None),
        num(5.0, 0.0, 10.0, Some(0.0)),
        num(5.0, 0.0, 10.0, Some(-1.0)),
        num(5.0, 0.0, 10.0, Some(f64::NAN)),
    ] {
        assert_eq!(err(&with_field(v, vec![])), "invalid semantic number");
    }
}

#[test]
fn protected_nodes_disclose_nothing_and_offer_nothing() {
    let mut s = with_field(Value::Protected {}, vec![]);
    assert_eq!(s.validate(), Ok(()));
    s.nodes[1].actions = vec![Action::Focus];
    assert_eq!(err(&s), "protected node disclosed a value or actions");

    let mut s = with_field(text("hunter2"), vec![]);
    s.nodes[1].role = "password-input".into();
    assert_eq!(err(&s), "protected node disclosed a value or actions");
    s.nodes[1].value = None;
    assert_eq!(err(&s), "protected node disclosed a value or actions");
    s.nodes[1].value = Some(Value::Protected {});
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn focus_must_name_a_node() {
    let mut s = snap(vec![node(1, &[])]);
    s.focus = Some(1);
    assert_eq!(s.validate(), Ok(()));
    s.focus = Some(9);
    assert_eq!(err(&s), "invalid semantic focus");
}

#[test]
fn the_tree_must_be_one_acyclic_connected_tree() {
    let cycle = snap(vec![node(1, &[2]), node(2, &[1])]);
    assert_eq!(err(&cycle), "semantic cycle or shared child");
    let own_child = snap(vec![node(1, &[1])]);
    assert_eq!(err(&own_child), "semantic cycle or shared child");
    let shared = snap(vec![node(1, &[2, 3]), node(2, &[3]), node(3, &[])]);
    assert_eq!(err(&shared), "semantic cycle or shared child");
    let dangling = snap(vec![node(1, &[9])]);
    assert_eq!(err(&dangling), "missing semantic node");
    let orphan = snap(vec![node(1, &[]), node(2, &[])]);
    assert_eq!(err(&orphan), "unreachable semantic node");
    let no_root = snap(vec![node(2, &[])]);
    assert_eq!(err(&no_root), "missing semantic node");
}

#[test]
fn depth_is_bounded() {
    // A chain of `levels` nodes: node i has child i + 1.
    let chain = |levels: u64| {
        snap(
            (1..=levels)
                .map(|i| {
                    let child = [i + 1];
                    node(i, if i < levels { &child } else { &[] })
                })
                .collect(),
        )
    };
    assert_eq!(chain(MAX_DEPTH as u64 + 1).validate(), Ok(()));
    assert_eq!(
        err(&chain(MAX_DEPTH as u64 + 2)),
        "semantic cycle or shared child"
    );
}

#[test]
fn permits_refuses_unknown_protected_disabled_busy_and_unlisted() {
    let mut s = with_field(text("a"), vec![Action::Click, Action::Focus]);
    assert!(s.permits(2, Action::Click, None));
    assert!(!s.permits(99, Action::Click, None));
    assert!(!s.permits(2, Action::Expand, None));
    s.nodes[1].states.disabled = true;
    assert!(!s.permits(2, Action::Click, None));
    s.nodes[1].states.disabled = false;
    s.nodes[1].states.busy = true;
    assert!(!s.permits(2, Action::Click, None));
    s.nodes[1].states.busy = false;
    s.nodes[1].value = Some(Value::Protected {});
    assert!(!s.permits(2, Action::Click, None));
}

#[test]
fn plain_actions_take_no_data() {
    let s = with_field(text("a"), vec![Action::Click]);
    assert!(s.permits(2, Action::Click, None));
    assert!(!s.permits(2, Action::Click, Some(&Data::Number(1.0))));
}

#[test]
fn set_value_text() {
    let mut s = with_field(text("a"), vec![Action::SetValue]);
    assert!(s.permits(2, Action::SetValue, Some(&Data::Text("b".into()))));
    let max = Data::Text("b".repeat(MAX_TEXT));
    assert!(s.permits(2, Action::SetValue, Some(&max)));
    let over = Data::Text("b".repeat(MAX_TEXT + 1));
    assert!(!s.permits(2, Action::SetValue, Some(&over)));
    assert!(!s.permits(2, Action::SetValue, None));
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Number(1.0))));
    s.nodes[1].states.read_only = true;
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Text("b".into()))));
}

#[test]
fn set_value_number_stays_inside_the_range() {
    let num = Value::Number {
        value: 5.0,
        min: 0.0,
        max: 10.0,
        step: None,
    };
    let mut s = with_field(num, vec![Action::SetValue]);
    assert!(s.permits(2, Action::SetValue, Some(&Data::Number(0.0))));
    assert!(s.permits(2, Action::SetValue, Some(&Data::Number(10.0))));
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Number(10.5))));
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Number(-0.5))));
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Number(f64::NAN))));
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Text("1".into()))));
    s.nodes[1].states.read_only = true;
    assert!(!s.permits(2, Action::SetValue, Some(&Data::Number(1.0))));
}

#[test]
fn set_selection_needs_ordered_char_boundaries() {
    // Bytes: a (0), é (1..3), b (3), end 4.
    let s = with_field(text("aéb"), vec![Action::SetSelection]);
    let sel = |start, end| Data::Selection { start, end };
    assert!(s.permits(2, Action::SetSelection, Some(&sel(0, 4))));
    assert!(s.permits(2, Action::SetSelection, Some(&sel(1, 3))));
    assert!(s.permits(2, Action::SetSelection, Some(&sel(2 + 1, 3))));
    assert!(
        !s.permits(2, Action::SetSelection, Some(&sel(2, 3))),
        "inside the two-byte é"
    );
    assert!(!s.permits(2, Action::SetSelection, Some(&sel(3, 1))));
    assert!(
        !s.permits(2, Action::SetSelection, Some(&sel(0, 5))),
        "past the end"
    );
    assert!(!s.permits(2, Action::SetSelection, None));
    let num = with_field(
        Value::Number {
            value: 1.0,
            min: 0.0,
            max: 2.0,
            step: None,
        },
        vec![Action::SetSelection],
    );
    assert!(!num.permits(2, Action::SetSelection, Some(&sel(0, 0))));
}

#[test]
fn json_shapes_are_stable() {
    assert_eq!(json(&Value::Protected {}), r#"{"kind":"protected"}"#);
    assert_eq!(json(&text("a")), r#"{"kind":"text","text":"a"}"#);
    assert_eq!(json(&Action::ScrollIntoView), r#""scroll-into-view""#);
    assert_eq!(json(&Action::ShowMenu), r#""show-menu""#);
    assert_eq!(json(&Data::Number(2.5)), r#"{"kind":"number","value":2.5}"#);
    assert_eq!(
        json(&Data::Text("t".into())),
        r#"{"kind":"text","value":"t"}"#
    );
    assert_eq!(
        json(&Data::Selection { start: 1, end: 2 }),
        r#"{"kind":"selection","value":{"start":1,"end":2}}"#
    );
    assert_eq!(json(&Outcome::Unsupported), r#""unsupported""#);
}

#[test]
fn every_type_round_trips_through_json() {
    let mut s = with_field(
        Value::Number {
            value: 1.0,
            min: 0.0,
            max: 2.0,
            step: Some(0.5),
        },
        vec![Action::Increment, Action::Decrement, Action::SetValue],
    );
    s.focus = Some(2);
    s.nodes[1].name = Some("Volume".into());
    s.nodes[1].states.expanded = Some(true);
    s.nodes[1].position = Position {
        item_count: Some(3),
        item_index: Some(1),
        column_index: None,
        level: Some(2),
    };
    s.nodes[1].truncated = true;
    round_trip(&s);

    let window = |handle, snapshot| Window {
        handle,
        app_id: "org.example".into(),
        title: "T".into(),
        origin: [1.0, 2.0],
        scale: [1.5, 1.5],
        focused: handle == 1,
        snapshot,
    };
    round_trip(&Observation {
        geometry: OutputGeometry {
            revision: 3,
            name: "DP-1".into(),
            x: 0.0,
            y: 0.0,
            width: 3840.0,
            height: 2160.0,
            scale: 1.5,
            transform: "normal".into(),
        },
        epoch: 7,
        revision: 9,
        geometry_revision: 3,
        windows: vec![window(1, Some(s)), window(2, None)],
    });

    round_trip(&Invocation {
        window: 1,
        revision: 2,
        node: 3,
        action: Action::SetSelection,
        data: Some(Data::Selection { start: 0, end: 1 }),
    });
    round_trip(&ActionCall {
        id: 1,
        epoch: 2,
        revision: 3,
        node: 4,
        action: Action::Click,
        data: None,
    });
    for outcome in [
        Outcome::Applied,
        Outcome::Stale,
        Outcome::Unavailable,
        Outcome::Unsupported,
        Outcome::Denied,
    ] {
        round_trip(&ActionReply {
            id: 1,
            epoch: 2,
            revision: 3,
            outcome,
        });
    }
}

#[test]
fn malformed_json_is_rejected() {
    let call = r#"{"id":1,"epoch":2,"revision":3,"node":4,"action":"click","data":null}"#;
    assert!(serde_json::from_str::<ActionCall>(call).is_ok());
    // An unknown field on a deny-unknown-fields type.
    let extra = call.replace("\"data\":null", "\"data\":null,\"extra\":1");
    assert!(serde_json::from_str::<ActionCall>(&extra).is_err());
    // Unknown action, outcome and value kind.
    assert!(serde_json::from_str::<ActionCall>(&call.replace("click", "explode")).is_err());
    assert!(serde_json::from_str::<Outcome>(r#""maybe""#).is_err());
    assert!(serde_json::from_str::<Value>(r#"{"kind":"image"}"#).is_err());
    assert!(serde_json::from_str::<Value>(r#"{"kind":"text","text":"x","more":1}"#).is_err());
    // Missing field, wrong type, negative unsigned, truncated input, wrong casing.
    assert!(serde_json::from_str::<ActionCall>(r#"{"id":1}"#).is_err());
    assert!(serde_json::from_str::<ActionCall>(&call.replace("\"id\":1", "\"id\":\"1\"")).is_err());
    assert!(serde_json::from_str::<ActionCall>(&call.replace("\"id\":1", "\"id\":-1")).is_err());
    assert!(serde_json::from_str::<ActionCall>(&call[..call.len() - 1]).is_err());
    assert!(serde_json::from_str::<Action>(r#""Click""#).is_err());
    assert!(
        serde_json::from_str::<Data>(r#"{"kind":"selection","value":{"start":-1,"end":0}}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<Snapshot>("[]").is_err());
}

#[test]
fn sealed_payloads_round_trip() {
    let s = with_field(text("hello"), vec![Action::Focus]);
    let (file, len) = seal(&s, MAX_FRAME).unwrap();
    assert_eq!(file.metadata().unwrap().len(), u64::from(len));
    let back: Snapshot = read(file.into(), len, MAX_FRAME).unwrap();
    assert_eq!(back, s);
}

#[test]
fn sealed_files_cannot_be_changed() {
    let (mut file, len) = seal(&"x", MAX_FRAME).unwrap();
    assert!(file.write_all(b"more").is_err(), "writes are sealed");
    assert!(file.set_len(0).is_err(), "shrinking is sealed");
    assert!(
        file.set_len(u64::from(len) + 10).is_err(),
        "growing is sealed"
    );
}

#[test]
fn seal_refuses_a_payload_over_the_limit() {
    // The JSON of a 100-character string is 102 bytes with its quotes.
    let big = "a".repeat(100);
    assert!(seal(&big, 50).is_err());
    assert!(seal(&big, 101).is_err());
    let (_, len) = seal(&big, 102).unwrap();
    assert_eq!(len, 102);
}

#[test]
fn read_refuses_a_length_over_the_limit_or_not_matching() {
    let (file, len) = seal(&"abc", MAX_FRAME).unwrap();
    let dup = || OwnedFd::from(file.try_clone().unwrap());
    assert_eq!(read::<String>(dup(), len, MAX_FRAME).unwrap(), "abc");
    assert!(read::<String>(dup(), len, len as usize - 1).is_err());
    assert!(read::<String>(dup(), len - 1, MAX_FRAME).is_err());
    assert!(read::<String>(dup(), len + 1, MAX_FRAME).is_err());
}

#[test]
fn read_refuses_files_that_are_not_fully_sealed_memfds() {
    // An unsealed memfd.
    let mut f = memfd(true);
    f.write_all(b"\"abc\"").unwrap();
    assert!(read::<String>(f.into(), 5, MAX_FRAME).is_err());

    // A memfd that still allows writes.
    let mut f = memfd(true);
    f.write_all(b"\"abc\"").unwrap();
    let partial = rustix::fs::SealFlags::SHRINK | rustix::fs::SealFlags::GROW;
    rustix::fs::fcntl_add_seals(f.as_fd(), partial).unwrap();
    assert!(read::<String>(f.into(), 5, MAX_FRAME).is_err());

    // A memfd that cannot be sealed at all.
    let mut f = memfd(false);
    f.write_all(b"\"abc\"").unwrap();
    assert!(read::<String>(f.into(), 5, MAX_FRAME).is_err());

    // A pipe.
    let (rd, wr) = std::io::pipe().unwrap();
    drop(wr);
    assert!(read::<String>(rd.into(), 0, MAX_FRAME).is_err());
}

#[test]
fn read_refuses_a_regular_file() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("app-protocols-seal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("payload");
    std::fs::write(&path, b"\"abc\"").unwrap();
    let r = read::<String>(File::open(&path).unwrap().into(), 5, MAX_FRAME);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(r.is_err());
}

#[test]
fn read_refuses_invalid_json_in_a_valid_seal() {
    let (file, len) = seal(&"abc", MAX_FRAME).unwrap();
    assert!(read::<Snapshot>(file.into(), len, MAX_FRAME).is_err());
    let unknown =
        serde_json::json!({"revision": 1, "root": 1, "focus": null, "nodes": [], "extra": 1});
    let (file, len) = seal(&unknown, MAX_FRAME).unwrap();
    assert!(read::<Snapshot>(file.into(), len, MAX_FRAME).is_err());
}

#[test]
fn a_protected_value_refuses_an_unknown_field() {
    assert_eq!(
        serde_json::from_str::<Value>(r#"{"kind":"protected"}"#).unwrap(),
        Value::Protected {}
    );
    assert!(serde_json::from_str::<Value>(r#"{"kind":"protected","text":"secret"}"#).is_err());
}
