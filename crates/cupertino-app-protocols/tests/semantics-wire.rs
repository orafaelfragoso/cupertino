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

/// A root (id 1) with one child (id 2) changed by `edit`.
fn with_child(edit: impl Fn(&mut Node)) -> Snapshot {
    let mut child = node(2, &[]);
    edit(&mut child);
    snap(vec![node(1, &[2]), child])
}

/// A chain of `levels` nodes: node i has child i + 1.
fn chain(levels: u64) -> Snapshot {
    snap(
        (1..=levels)
            .map(|i| {
                let child = [i + 1];
                node(i, if i < levels { &child } else { &[] })
            })
            .collect(),
    )
}

/// A root holding `count - 1` children, `count` nodes in all.
fn wide(count: usize) -> Snapshot {
    let ids: Vec<u64> = (2..=count as u64).collect();
    let mut nodes = vec![node(1, &ids)];
    nodes.extend(ids.iter().map(|i| node(*i, &[])));
    snap(nodes)
}

fn text(s: &str) -> Value {
    Value::Text { text: s.into() }
}

fn number(value: f64, min: f64, max: f64, step: Option<f64>) -> Value {
    Value::Number {
        value,
        min,
        max,
        step,
    }
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
fn bounds_must_be_finite_and_non_negative() {
    let b = |x, y, width, height| Bounds {
        x,
        y,
        width,
        height,
    };
    for (row, bounds, valid) in [
        ("negative origin, empty", b(-5.0, -5.0, 0.0, 0.0), true),
        ("negative width", b(0.0, 0.0, -1.0, 1.0), false),
        ("negative height", b(0.0, 0.0, 1.0, -0.5), false),
        ("NaN x", b(f64::NAN, 0.0, 1.0, 1.0), false),
        ("infinite y", b(0.0, f64::INFINITY, 1.0, 1.0), false),
        ("NaN width", b(0.0, 0.0, f64::NAN, 1.0), false),
        (
            "infinite height",
            b(0.0, 0.0, 1.0, f64::NEG_INFINITY),
            false,
        ),
    ] {
        assert_eq!(bounds.valid(), valid, "{row}");
    }
}

#[test]
fn validate_accepts_one_bounded_tree_and_names_what_is_wrong_with_any_other() {
    const SIZE: &str = "invalid semantic snapshot size or revision";
    const NODE: &str = "invalid semantic node";
    const TEXT: &str = "semantic text exceeds limit";
    const VALUE: &str = "semantic value exceeds limit";
    const NUMBER: &str = "invalid semantic number";
    const SHAPE: &str = "semantic cycle or shared child";
    const MISSING: &str = "missing semantic node";
    let long = "a".repeat(MAX_TEXT + 1);
    let full = "a".repeat(MAX_TEXT);
    let rows: Vec<(&str, Snapshot, Result<(), &str>)> = vec![
        (
            "a tree with focus",
            Snapshot {
                focus: Some(4),
                ..snap(vec![
                    node(1, &[2, 3]),
                    node(2, &[4]),
                    node(3, &[]),
                    node(4, &[]),
                ])
            },
            Ok(()),
        ),
        ("a lone root", snap(vec![node(1, &[])]), Ok(())),
        ("the most nodes", wide(MAX_NODES), Ok(())),
        (
            "the longest role and most actions",
            with_child(|n| {
                n.role = "r".repeat(64);
                n.actions = vec![Action::Click; 10];
            }),
            Ok(()),
        ),
        (
            "revision 0",
            Snapshot {
                revision: 0,
                ..snap(vec![node(1, &[])])
            },
            Err(SIZE),
        ),
        ("no nodes", snap(vec![]), Err(SIZE)),
        ("one node too many", wide(MAX_NODES + 1), Err(SIZE)),
        ("id 0", with_child(|n| n.id = 0), Err(NODE)),
        (
            "empty role",
            with_child(|n| n.role = String::new()),
            Err(NODE),
        ),
        (
            "long role",
            with_child(|n| n.role = "r".repeat(65)),
            Err(NODE),
        ),
        (
            "negative width",
            with_child(|n| n.bounds.width = -1.0),
            Err(NODE),
        ),
        ("NaN x", with_child(|n| n.bounds.x = f64::NAN), Err(NODE)),
        (
            "too many actions",
            with_child(|n| n.actions = vec![Action::Click; 11]),
            Err(NODE),
        ),
        (
            "too many children",
            with_child(|n| n.children = vec![1; MAX_NODES + 1]),
            Err(NODE),
        ),
        (
            "duplicate id",
            snap(vec![node(1, &[2]), node(2, &[]), node(2, &[])]),
            Err(NODE),
        ),
        (
            "long name",
            with_child(|n| n.name = Some(long.clone())),
            Err(TEXT),
        ),
        (
            "long description",
            with_child(|n| n.description = Some(long.clone())),
            Err(TEXT),
        ),
        (
            "the longest description",
            with_child(|n| n.description = Some(full.clone())),
            Ok(()),
        ),
        (
            "text counted in bytes",
            with_child(|n| n.description = Some("é".repeat(MAX_TEXT / 2 + 1))),
            Err(TEXT),
        ),
        ("long value", with_field(text(&long), vec![]), Err(VALUE)),
        ("the longest value", with_field(text(&full), vec![]), Ok(())),
        (
            "a stepped number",
            with_field(number(5.0, 0.0, 10.0, Some(1.0)), vec![]),
            Ok(()),
        ),
        (
            "an empty range",
            with_field(number(5.0, 5.0, 5.0, None), vec![]),
            Ok(()),
        ),
        (
            "NaN number",
            with_field(number(f64::NAN, 0.0, 10.0, None), vec![]),
            Err(NUMBER),
        ),
        (
            "infinite number",
            with_field(number(f64::INFINITY, 0.0, 10.0, None), vec![]),
            Err(NUMBER),
        ),
        (
            "infinite min",
            with_field(number(5.0, f64::NEG_INFINITY, 10.0, None), vec![]),
            Err(NUMBER),
        ),
        (
            "NaN max",
            with_field(number(5.0, 0.0, f64::NAN, None), vec![]),
            Err(NUMBER),
        ),
        (
            "min over max",
            with_field(number(5.0, 10.0, 0.0, None), vec![]),
            Err(NUMBER),
        ),
        (
            "zero step",
            with_field(number(5.0, 0.0, 10.0, Some(0.0)), vec![]),
            Err(NUMBER),
        ),
        (
            "negative step",
            with_field(number(5.0, 0.0, 10.0, Some(-1.0)), vec![]),
            Err(NUMBER),
        ),
        (
            "NaN step",
            with_field(number(5.0, 0.0, 10.0, Some(f64::NAN)), vec![]),
            Err(NUMBER),
        ),
        (
            "focus on a node",
            Snapshot {
                focus: Some(1),
                ..snap(vec![node(1, &[])])
            },
            Ok(()),
        ),
        (
            "focus on no node",
            Snapshot {
                focus: Some(9),
                ..snap(vec![node(1, &[])])
            },
            Err("invalid semantic focus"),
        ),
        (
            "a cycle",
            snap(vec![node(1, &[2]), node(2, &[1])]),
            Err(SHAPE),
        ),
        ("its own child", snap(vec![node(1, &[1])]), Err(SHAPE)),
        (
            "a shared child",
            snap(vec![node(1, &[2, 3]), node(2, &[3]), node(3, &[])]),
            Err(SHAPE),
        ),
        ("a dangling child", snap(vec![node(1, &[9])]), Err(MISSING)),
        (
            "an orphan",
            snap(vec![node(1, &[]), node(2, &[])]),
            Err("unreachable semantic node"),
        ),
        ("no root", snap(vec![node(2, &[])]), Err(MISSING)),
        ("the deepest chain", chain(MAX_DEPTH as u64 + 1), Ok(())),
        (
            "one level too deep",
            chain(MAX_DEPTH as u64 + 2),
            Err(SHAPE),
        ),
    ];
    for (row, snapshot, expected) in rows {
        assert_eq!(snapshot.validate(), expected, "{row}");
    }
}

#[test]
fn a_protected_node_discloses_no_value_and_offers_no_action() {
    for (row, role, value, protected) in [
        ("a plain node", "group", None, false),
        ("a password input", "password-input", None, true),
        (
            "a protected value",
            "text-input",
            Some(Value::Protected {}),
            true,
        ),
        ("a text value", "text-input", Some(text("x")), false),
    ] {
        let mut n = node(1, &[]);
        n.role = role.into();
        n.value = value;
        assert_eq!(n.protected(), protected, "{row}");
    }

    const DISCLOSED: &str = "protected node disclosed a value or actions";
    let password = |value: Option<Value>| {
        with_child(|n| {
            n.role = "password-input".into();
            n.value = value.clone();
        })
    };
    for (row, snapshot, expected) in [
        (
            "a protected value",
            with_field(Value::Protected {}, vec![]),
            Ok(()),
        ),
        (
            "a protected value with an action",
            with_field(Value::Protected {}, vec![Action::Focus]),
            Err(DISCLOSED),
        ),
        (
            "a password with text",
            password(Some(text("hunter2"))),
            Err(DISCLOSED),
        ),
        ("a password without a value", password(None), Err(DISCLOSED)),
        (
            "a password with a protected value",
            password(Some(Value::Protected {})),
            Ok(()),
        ),
    ] {
        assert_eq!(snapshot.validate(), expected, "{row}");
    }

    let s = with_field(Value::Protected {}, vec![Action::Click]);
    assert!(
        !s.permits(2, Action::Click, None),
        "a protected node permits nothing"
    );
}

#[test]
fn permits_allows_only_a_listed_action_on_a_usable_node_with_the_data_it_takes() {
    let plain = with_field(text("a"), vec![Action::Click, Action::Focus]);
    let disabled = with_child(|n| {
        n.actions = vec![Action::Click];
        n.states.disabled = true;
    });
    let busy = with_child(|n| {
        n.actions = vec![Action::Click];
        n.states.busy = true;
    });
    let set_text = with_field(text("a"), vec![Action::SetValue]);
    let mut read_only_text = set_text.clone();
    read_only_text.nodes[1].states.read_only = true;
    let set_number = with_field(number(5.0, 0.0, 10.0, None), vec![Action::SetValue]);
    let mut read_only_number = set_number.clone();
    read_only_number.nodes[1].states.read_only = true;
    // Bytes: a (0), é (1..3), b (3), end 4.
    let select = with_field(text("aéb"), vec![Action::SetSelection]);
    let select_number = with_field(number(1.0, 0.0, 2.0, None), vec![Action::SetSelection]);

    let t = |s: &str| Some(Data::Text(s.into()));
    let n = |v: f64| Some(Data::Number(v));
    let sel = |start, end| Some(Data::Selection { start, end });
    let rows: Vec<(&str, &Snapshot, u64, Action, Option<Data>, bool)> = vec![
        ("a listed click", &plain, 2, Action::Click, None, true),
        ("an unknown node", &plain, 99, Action::Click, None, false),
        ("an unlisted action", &plain, 2, Action::Expand, None, false),
        ("a disabled node", &disabled, 2, Action::Click, None, false),
        ("a busy node", &busy, 2, Action::Click, None, false),
        ("a click with data", &plain, 2, Action::Click, n(1.0), false),
        ("text", &set_text, 2, Action::SetValue, t("b"), true),
        (
            "the longest text",
            &set_text,
            2,
            Action::SetValue,
            t(&"b".repeat(MAX_TEXT)),
            true,
        ),
        (
            "text too long",
            &set_text,
            2,
            Action::SetValue,
            t(&"b".repeat(MAX_TEXT + 1)),
            false,
        ),
        (
            "a value without data",
            &set_text,
            2,
            Action::SetValue,
            None,
            false,
        ),
        (
            "a number for text",
            &set_text,
            2,
            Action::SetValue,
            n(1.0),
            false,
        ),
        (
            "read-only text",
            &read_only_text,
            2,
            Action::SetValue,
            t("b"),
            false,
        ),
        (
            "the minimum",
            &set_number,
            2,
            Action::SetValue,
            n(0.0),
            true,
        ),
        (
            "the maximum",
            &set_number,
            2,
            Action::SetValue,
            n(10.0),
            true,
        ),
        (
            "over the maximum",
            &set_number,
            2,
            Action::SetValue,
            n(10.5),
            false,
        ),
        (
            "under the minimum",
            &set_number,
            2,
            Action::SetValue,
            n(-0.5),
            false,
        ),
        ("NaN", &set_number, 2, Action::SetValue, n(f64::NAN), false),
        (
            "text for a number",
            &set_number,
            2,
            Action::SetValue,
            t("1"),
            false,
        ),
        (
            "a read-only number",
            &read_only_number,
            2,
            Action::SetValue,
            n(1.0),
            false,
        ),
        (
            "the whole text",
            &select,
            2,
            Action::SetSelection,
            sel(0, 4),
            true,
        ),
        (
            "around the é",
            &select,
            2,
            Action::SetSelection,
            sel(1, 3),
            true,
        ),
        (
            "an empty selection",
            &select,
            2,
            Action::SetSelection,
            sel(3, 3),
            true,
        ),
        (
            "inside the é",
            &select,
            2,
            Action::SetSelection,
            sel(2, 3),
            false,
        ),
        (
            "reversed",
            &select,
            2,
            Action::SetSelection,
            sel(3, 1),
            false,
        ),
        (
            "past the end",
            &select,
            2,
            Action::SetSelection,
            sel(0, 5),
            false,
        ),
        (
            "a selection without data",
            &select,
            2,
            Action::SetSelection,
            None,
            false,
        ),
        (
            "a selection in a number",
            &select_number,
            2,
            Action::SetSelection,
            sel(0, 0),
            false,
        ),
    ];
    for (row, snapshot, id, action, data, permitted) in rows {
        assert_eq!(
            snapshot.permits(id, action, data.as_ref()),
            permitted,
            "{row}"
        );
    }
}

#[test]
fn json_shapes_are_stable() {
    for (row, got, expected) in [
        (
            "protected",
            json(&Value::Protected {}),
            r#"{"kind":"protected"}"#,
        ),
        ("text", json(&text("a")), r#"{"kind":"text","text":"a"}"#),
        (
            "an action",
            json(&Action::ScrollIntoView),
            r#""scroll-into-view""#,
        ),
        ("show menu", json(&Action::ShowMenu), r#""show-menu""#),
        (
            "number data",
            json(&Data::Number(2.5)),
            r#"{"kind":"number","value":2.5}"#,
        ),
        (
            "text data",
            json(&Data::Text("t".into())),
            r#"{"kind":"text","value":"t"}"#,
        ),
        (
            "selection data",
            json(&Data::Selection { start: 1, end: 2 }),
            r#"{"kind":"selection","value":{"start":1,"end":2}}"#,
        ),
        (
            "an outcome",
            json(&Outcome::Unsupported),
            r#""unsupported""#,
        ),
    ] {
        assert_eq!(got, expected, "{row}");
    }
}

#[test]
fn every_type_round_trips_through_json() {
    let mut s = with_field(
        number(1.0, 0.0, 2.0, Some(0.5)),
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
fn malformed_json_is_refused() {
    let call = r#"{"id":1,"epoch":2,"revision":3,"node":4,"action":"click","data":null}"#;
    assert!(serde_json::from_str::<ActionCall>(call).is_ok());
    assert_eq!(
        serde_json::from_str::<Value>(r#"{"kind":"protected"}"#).unwrap(),
        Value::Protected {}
    );
    let refused = |row: &str, result: Result<(), serde_json::Error>| {
        assert!(result.is_err(), "{row} is refused");
    };
    let as_call = |s: &str| serde_json::from_str::<ActionCall>(s).map(drop);
    let as_value = |s: &str| serde_json::from_str::<Value>(s).map(drop);
    refused(
        "an unknown field",
        as_call(&call.replace("\"data\":null", "\"data\":null,\"extra\":1")),
    );
    refused(
        "an unknown action",
        as_call(&call.replace("click", "explode")),
    );
    refused(
        "an unknown outcome",
        serde_json::from_str::<Outcome>(r#""maybe""#).map(drop),
    );
    refused("an unknown value kind", as_value(r#"{"kind":"image"}"#));
    refused(
        "an unknown field on a text value",
        as_value(r#"{"kind":"text","text":"x","more":1}"#),
    );
    // A protected value has no fields, so leaked text is refused instead of dropped.
    refused(
        "text on a protected value",
        as_value(r#"{"kind":"protected","text":"secret"}"#),
    );
    refused("a missing field", as_call(r#"{"id":1}"#));
    refused(
        "a wrong type",
        as_call(&call.replace("\"id\":1", "\"id\":\"1\"")),
    );
    refused(
        "a negative id",
        as_call(&call.replace("\"id\":1", "\"id\":-1")),
    );
    refused("truncated input", as_call(&call[..call.len() - 1]));
    refused(
        "wrong casing",
        serde_json::from_str::<Action>(r#""Click""#).map(drop),
    );
    refused(
        "a negative selection",
        serde_json::from_str::<Data>(r#"{"kind":"selection","value":{"start":-1,"end":0}}"#)
            .map(drop),
    );
    refused(
        "an array for a snapshot",
        serde_json::from_str::<Snapshot>("[]").map(drop),
    );
}

#[test]
fn a_sealed_payload_reads_back_and_cannot_be_changed() {
    let s = with_field(text("hello"), vec![Action::Focus]);
    let (file, len) = seal(&s, MAX_FRAME).unwrap();
    assert_eq!(file.metadata().unwrap().len(), u64::from(len));
    let back: Snapshot = read(file.try_clone().unwrap().into(), len, MAX_FRAME).unwrap();
    assert_eq!(back, s);

    let mut file = file;
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
    for (limit, sealed) in [(50, false), (101, false), (102, true)] {
        assert_eq!(seal(&big, limit).is_ok(), sealed, "limit {limit}");
    }
    assert_eq!(seal(&big, 102).unwrap().1, 102);
}

#[test]
fn read_refuses_anything_but_a_fully_sealed_memfd_of_the_named_length_and_type() {
    let (file, len) = seal(&"abc", MAX_FRAME).unwrap();
    let dup = || OwnedFd::from(file.try_clone().unwrap());
    assert_eq!(read::<String>(dup(), len, MAX_FRAME).unwrap(), "abc");

    let unsealed = || {
        let mut f = memfd(true);
        f.write_all(b"\"abc\"").unwrap();
        f
    };
    let writable = unsealed();
    let partial = rustix::fs::SealFlags::SHRINK | rustix::fs::SealFlags::GROW;
    rustix::fs::fcntl_add_seals(writable.as_fd(), partial).unwrap();
    let mut unsealable = memfd(false);
    unsealable.write_all(b"\"abc\"").unwrap();
    let (pipe, writer) = std::io::pipe().unwrap();
    drop(writer);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("app-protocols-seal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("payload");
    std::fs::write(&path, b"\"abc\"").unwrap();
    let regular = File::open(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let rows: Vec<(&str, OwnedFd, u32, usize)> = vec![
        ("a limit under the length", dup(), len, len as usize - 1),
        ("a length too short", dup(), len - 1, MAX_FRAME),
        ("a length too long", dup(), len + 1, MAX_FRAME),
        ("an unsealed memfd", unsealed().into(), 5, MAX_FRAME),
        ("a memfd that allows writes", writable.into(), 5, MAX_FRAME),
        (
            "a memfd that cannot be sealed",
            unsealable.into(),
            5,
            MAX_FRAME,
        ),
        ("a pipe", pipe.into(), 0, MAX_FRAME),
        ("a regular file", regular.into(), 5, MAX_FRAME),
    ];
    for (row, fd, len, limit) in rows {
        assert!(read::<String>(fd, len, limit).is_err(), "{row}");
    }

    let (file, len) = seal(&"abc", MAX_FRAME).unwrap();
    assert!(
        read::<Snapshot>(file.into(), len, MAX_FRAME).is_err(),
        "JSON of another type"
    );
    let unknown =
        serde_json::json!({"revision": 1, "root": 1, "focus": null, "nodes": [], "extra": 1});
    let (file, len) = seal(&unknown, MAX_FRAME).unwrap();
    assert!(
        read::<Snapshot>(file.into(), len, MAX_FRAME).is_err(),
        "a snapshot with an unknown field"
    );
}
