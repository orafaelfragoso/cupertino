use super::*;
fn accessible() -> AccessibilityNode {
    AccessibilityNode {
        role: crate::Role::TextInput,
        name: Some("Name".into()),
        description: None,
        value: Some(crate::AccessibleValue::Text {
            value: "hello".into(),
            editing: None,
        }),
        states: crate::AccessibleStates::default(),
        bounds: cupertino_geometry::Rect::new(1.0, 2.0, 30.0, 20.0),
        actions: vec![crate::Action::SetValue],
        relations: vec![],
        children: vec![],
        live: None,
        item_count: Some(100),
        item_index: Some(42),
        column_index: Some(2),
        level: Some(1),
        sort: None,
    }
}
#[test]
fn native_export_bounds_unicode_and_preserves_positions_and_generational_ids() {
    let key = NodeId::new(7, std::num::NonZeroU32::new(3).unwrap());
    let mut n = accessible();
    n.name = Some("é".repeat(wire::MAX_TEXT));
    let exported = node(key, &n);
    assert!(exported.truncated);
    assert_eq!(exported.name.as_ref().unwrap().len(), wire::MAX_TEXT);
    assert_eq!(exported.position.item_index, Some(42));
    assert_eq!(exported.id, (3u64 << 32) | 7);
    let snapshot = wire::Snapshot {
        revision: 1,
        root: exported.id,
        focus: None,
        nodes: vec![exported],
    };
    assert!(snapshot.validate().is_ok());
    assert_ne!(
        id(key),
        id(NodeId::new(7, std::num::NonZeroU32::new(4).unwrap()))
    );
}
#[test]
fn password_role_redacts_even_an_incorrect_upstream_value_and_all_actions() {
    let key = NodeId::new(0, std::num::NonZeroU32::new(1).unwrap());
    let mut n = accessible();
    n.role = crate::Role::PasswordInput;
    let exported = node(key, &n);
    assert_eq!(exported.value, Some(wire::Value::Protected));
    assert!(exported.actions.is_empty());
    assert!(!format!("{exported:?}").contains("hello"));
}
