use super::*;
fn snapshot() -> Snapshot {
    Snapshot {
        revision: 1,
        root: 1,
        focus: None,
        nodes: vec![Node {
            truncated: false,
            position: Position::default(),
            id: 1,
            role: "button".into(),
            name: Some("Go".into()),
            description: None,
            value: None,
            states: States::default(),
            bounds: Bounds::default(),
            actions: vec![Action::Click],
            children: vec![],
        }],
    }
}
#[test]
fn snapshots_require_one_bounded_reachable_tree() {
    let valid = snapshot();
    assert_eq!(valid.validate(), Ok(()));
    let mut s = valid.clone();
    s.nodes[0].children.push(1);
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.nodes[0].children.push(2);
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.nodes.push(s.nodes[0].clone());
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.nodes[0].bounds.width = f64::NAN;
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.nodes[0].name = Some("x".repeat(MAX_TEXT + 1));
    assert!(s.validate().is_err());
    let mut s = valid;
    s.nodes.push(Node {
        id: 2,
        ..s.nodes[0].clone()
    });
    assert!(s.validate().is_err());
}
#[test]
fn protected_disabled_and_typed_actions_are_checked_before_dispatch() {
    let mut s = snapshot();
    assert!(s.permits(1, Action::Click, None));
    s.nodes[0].states.disabled = true;
    assert!(!s.permits(1, Action::Click, None));
    s.nodes[0].states.disabled = false;
    s.nodes[0].role = "password-input".into();
    s.nodes[0].value = Some(Value::Text {
        text: "secret".into(),
    });
    assert!(s.validate().is_err());
    s.nodes[0].value = Some(Value::Protected);
    s.nodes[0].actions.clear();
    assert!(s.validate().is_ok());
    assert!(!s.permits(1, Action::Click, None));
    s.nodes[0].role = "text-input".into();
    s.nodes[0].value = Some(Value::Text { text: "é".into() });
    s.nodes[0].actions = vec![Action::SetValue, Action::SetSelection];
    assert!(!s.permits(1, Action::SetValue, Some(&Data::Number(1.0))));
    assert!(!s.permits(
        1,
        Action::SetSelection,
        Some(&Data::Selection { start: 1, end: 2 })
    ));
    assert!(s.permits(
        1,
        Action::SetSelection,
        Some(&Data::Selection { start: 0, end: 2 })
    ));
    s.nodes[0].states.read_only = true;
    assert!(!s.permits(1, Action::SetValue, Some(&Data::Text("new".into()))));
}
#[test]
fn only_exact_bounded_immutable_payloads_are_read() {
    let s = snapshot();
    let (file, len) = seal(&s, MAX_FRAME).unwrap();
    assert_eq!(
        read::<Snapshot>(file.try_clone().unwrap().into(), len, MAX_FRAME).unwrap(),
        s
    );
    assert!(read::<Snapshot>(file.try_clone().unwrap().into(), len - 1, MAX_FRAME).is_err());
    assert!(read::<Snapshot>(file.into(), len, 1).is_err());
    let fd = rustix::fs::memfd_create("mutable", rustix::fs::MemfdFlags::ALLOW_SEALING).unwrap();
    assert!(read::<Snapshot>(fd, 0, MAX_FRAME).is_err());
    let pipe = std::os::unix::net::UnixStream::pair().unwrap();
    assert!(read::<Snapshot>(pipe.0.into(), 0, MAX_FRAME).is_err());
}
