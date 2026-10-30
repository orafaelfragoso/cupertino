use super::*;
use cupertino_geometry::Rect;
use cupertino_ui_core::{AccessibleStates, TextSemantics};

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::new(3).unwrap())
}

fn base(role: Role) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: None,
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds: Rect::new(10.0, 20.0, 30.0, 40.0),
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
    }
}

fn text(value: &str) -> AccessibleValue {
    AccessibleValue::Text {
        value: value.into(),
        editing: Some(TextSemantics {
            caret: 1,
            selection: 0..2,
        }),
    }
}

#[test]
fn ids_round_trip_with_their_generation() {
    let core = NodeId::new(7, NonZeroU32::new(u32::MAX).unwrap());
    assert_eq!(core_id(node_id(core)), Some(core));
    assert_eq!(
        core_id(accesskit::NodeId(5)),
        None,
        "generation 0 is no id of ours"
    );
}

#[test]
fn a_button_carries_its_name_bounds_actions_relations_and_children() {
    let mut button = base(Role::Button);
    button.name = Some("OK".into());
    button.description = Some("Closes the window".into());
    button.states.focusable = true;
    button.states.disabled = true;
    button.actions = vec![Action::Click, Action::ShowMenu];
    button.relations = vec![Relation::LabelledBy(id(2)), Relation::Controls(id(3))];
    button.children = vec![id(4), id(5)];
    let out = node(&button);
    assert_eq!(out.role(), accesskit::Role::Button);
    assert_eq!(out.label(), Some("OK"));
    assert_eq!(out.description(), Some("Closes the window"));
    assert!(out.is_disabled());
    for action in [
        accesskit::Action::Click,
        accesskit::Action::ShowContextMenu,
        accesskit::Action::Focus,
    ] {
        assert!(out.supports_action(action), "{action:?}");
    }
    assert_eq!(
        out.bounds(),
        Some(accesskit::Rect::new(10.0, 20.0, 40.0, 60.0))
    );
    assert_eq!(out.labelled_by(), [node_id(id(2))]);
    assert_eq!(out.controls(), [node_id(id(3))]);
    assert_eq!(out.children(), [node_id(id(4)), node_id(id(5))]);
}

#[test]
fn static_text_is_named_through_its_value_as_accesskit_reads_a_label() {
    let mut text = base(Role::StaticText);
    text.name = Some("Version 1.0".into());
    let out = node(&text);
    assert_eq!(out.role(), accesskit::Role::Label);
    assert_eq!(out.value(), Some("Version 1.0"));
}

#[test]
fn a_live_region_keeps_its_politeness_and_other_nodes_have_none() {
    let mut toast = base(Role::StaticText);
    assert_eq!(node(&toast).live(), None);
    for (live, expected) in [
        (Live::Polite, accesskit::Live::Polite),
        (Live::Assertive, accesskit::Live::Assertive),
    ] {
        toast.live = Some(live);
        assert_eq!(node(&toast).live(), Some(expected));
    }
}

#[test]
fn values_keep_their_kind() {
    let mut field = base(Role::TextInput);
    field.value = Some(text("Ada"));
    assert_eq!(node(&field).value(), Some("Ada"));

    let mut slider = base(Role::Slider);
    slider.value = Some(AccessibleValue::Number {
        value: 0.5,
        min: 0.0,
        max: 1.0,
        step: Some(0.1),
    });
    let out = node(&slider);
    assert_eq!(
        (
            out.numeric_value(),
            out.min_numeric_value(),
            out.max_numeric_value(),
            out.numeric_value_step()
        ),
        (Some(0.5), Some(0.0), Some(1.0), Some(0.1))
    );

    let mut switch = base(Role::Switch);
    switch.value = Some(AccessibleValue::Toggle(Checked::Mixed));
    assert_eq!(node(&switch).toggled(), Some(Toggled::Mixed));
}

#[test]
fn a_password_field_never_carries_its_value_nor_takes_one() {
    // a second defence: a password field's value is dropped even when core sent it
    let mut field = base(Role::PasswordInput);
    field.value = Some(text("hunter2"));
    field.actions = vec![Action::Focus, Action::SetValue, Action::SetSelection];
    let out = node(&field);
    assert_eq!(out.value(), None);
    assert!(!out.supports_action(accesskit::Action::SetValue));
    assert!(out.supports_action(accesskit::Action::Focus));
    assert!(protected(&field));
}

#[test]
fn a_secret_is_never_a_live_region() {
    let mut field = base(Role::PasswordInput);
    field.live = Some(Live::Assertive);
    assert_eq!(node(&field).live(), None);
    let mut field = base(Role::TextInput);
    field.value = Some(AccessibleValue::Protected);
    field.live = Some(Live::Polite);
    assert_eq!(node(&field).live(), None);
}

#[test]
fn a_protected_value_of_any_role_is_a_secret() {
    let mut field = base(Role::TextInput);
    field.value = Some(AccessibleValue::Protected);
    field.actions = vec![Action::SetValue];
    assert!(protected(&field));
    assert!(!node(&field).supports_action(accesskit::Action::SetValue));
    field.value = Some(text("public"));
    assert!(!protected(&field));
    assert!(node(&field).supports_action(accesskit::Action::SetValue));
}

#[test]
fn a_selection_is_not_offered_without_text_runs() {
    let mut field = base(Role::TextInput);
    field.actions = vec![Action::SetSelection];
    assert!(!node(&field).supports_action(accesskit::Action::SetTextSelection));
}

fn request_for(
    action: accesskit::Action,
    data: Option<accesskit::ActionData>,
) -> accesskit::ActionRequest {
    accesskit::ActionRequest {
        action,
        target_tree: accesskit::TreeId::ROOT,
        target_node: node_id(id(9)),
        data,
    }
}

#[test]
fn requests_come_back_as_core_actions_with_their_data() {
    let click = request(&request_for(accesskit::Action::Click, None)).unwrap();
    assert_eq!(
        (click.target, click.action, click.data),
        (id(9), Action::Click, None)
    );
    let menu = request(&request_for(accesskit::Action::ShowContextMenu, None)).unwrap();
    assert_eq!(menu.action, Action::ShowMenu);
    let set = request(&request_for(
        accesskit::Action::SetValue,
        Some(accesskit::ActionData::Value("Grace".into())),
    ))
    .unwrap();
    assert_eq!(set.data, Some(ActionData::Text("Grace".into())));
    let number = request(&request_for(
        accesskit::Action::SetValue,
        Some(accesskit::ActionData::NumericValue(2.5)),
    ))
    .unwrap();
    assert_eq!(number.data, Some(ActionData::Number(2.5)));
}

#[test]
fn requests_core_has_no_action_for_are_dropped() {
    assert_eq!(
        request(&request_for(accesskit::Action::SetValue, None)),
        None
    );
    assert_eq!(
        request(&request_for(accesskit::Action::ScrollDown, None)),
        None
    );
    let mut foreign = request_for(accesskit::Action::Click, None);
    foreign.target_node = accesskit::NodeId(1);
    assert_eq!(request(&foreign), None);
}
