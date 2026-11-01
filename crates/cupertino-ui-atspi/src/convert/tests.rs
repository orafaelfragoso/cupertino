use super::*;
use cupertino_geometry::Rect;
use std::ops::Range;

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
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
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
fn a_table_counts_its_rows_and_a_row_its_place_and_a_list_only_its_items() {
    let mut table = base(Role::Table);
    table.item_count = Some(50_000);
    let table = node(&table);
    assert_eq!(
        (table.size_of_set(), table.row_count()),
        (Some(50_000), Some(50_000))
    );
    let mut row = base(Role::Row);
    row.item_index = Some(41);
    let row = node(&row);
    assert_eq!(
        (row.position_in_set(), row.row_index()),
        (Some(41), Some(41))
    );
    let mut list = base(Role::List);
    list.item_count = Some(3);
    let mut item = base(Role::ListItem);
    item.item_index = Some(2);
    let (list, item) = (node(&list), node(&item));
    assert_eq!((list.size_of_set(), list.row_count()), (Some(3), None));
    assert_eq!((item.position_in_set(), item.row_index()), (Some(2), None));
    assert_eq!(node(&base(Role::Button)).position_in_set(), None);
}

#[test]
fn a_cell_and_a_column_header_carry_their_column() {
    for role in [Role::Cell, Role::ColumnHeader] {
        let mut cell = base(role);
        cell.column_index = Some(1);
        assert_eq!(node(&cell).column_index(), Some(1), "{role:?}");
    }
    assert_eq!(node(&base(Role::Row)).column_index(), None);
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

/// A request of a node sent without text runs.
fn request(request: &accesskit::ActionRequest) -> Option<ActionRequest> {
    super::request(request, |_| None)
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

fn editable(value: &str, caret: usize, selection: Range<usize>) -> AccessibilityNode {
    let mut field = base(Role::MultilineTextInput);
    field.value = Some(AccessibleValue::Text {
        value: value.into(),
        editing: Some(TextSemantics { caret, selection }),
    });
    field.actions = vec![Action::Focus, Action::SetValue, Action::SetSelection];
    field
}

/// `node`'s text runs, numbered from 1000, and the node they are children of.
fn runs_of(node: &AccessibilityNode) -> (accesskit::Node, Vec<Run>) {
    let mut out = super::node(node);
    let runs = text_runs(node, &mut out, |i| accesskit::NodeId(1000 + i as u64));
    (out, runs)
}

fn at(run: u64, character_index: usize) -> accesskit::TextPosition {
    accesskit::TextPosition {
        node: accesskit::NodeId(1000 + run),
        character_index,
    }
}

#[test]
fn an_editable_value_is_a_text_run_per_line_with_its_caret_and_selection() {
    // "e\u{301}" is one character of two scalar values; the selection runs from after "a" back to the caret before "b"
    let value = "a e\u{301}!\nb";
    let (out, runs) = runs_of(&editable(value, 7, 1..7));
    let texts: Vec<_> = runs.iter().map(|r| r.node.value().unwrap()).collect();
    assert_eq!(texts, ["a e\u{301}!\n", "b"]);
    assert_eq!(runs[0].node.character_lengths(), [1, 1, 3, 1, 1]);
    assert_eq!(runs[0].node.word_starts(), [0, 2, 3]);
    assert_eq!(runs[1].node.role(), accesskit::Role::TextRun);
    assert_eq!(
        out.children(),
        [accesskit::NodeId(1000), accesskit::NodeId(1001)]
    );
    assert_eq!(
        out.text_selection().copied(),
        Some(accesskit::TextSelection {
            anchor: at(0, 1),
            focus: at(1, 0),
        })
    );
    assert!(out.supports_action(accesskit::Action::SetTextSelection));
}

#[test]
fn a_caret_at_the_end_of_a_line_is_on_its_line_break_and_after_a_last_one_on_an_empty_line() {
    let (out, runs) = runs_of(&editable("ab\n", 2, 2..2));
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].node.value(), Some(""));
    assert_eq!(out.text_selection().unwrap().focus, at(0, 2));
    let (out, _) = runs_of(&editable("ab\n", 3, 3..3));
    assert_eq!(out.text_selection().unwrap().focus, at(1, 0));
    let (out, runs) = runs_of(&editable("", 0, 0..0));
    assert_eq!(runs.len(), 1);
    assert_eq!(out.text_selection().unwrap().focus, at(0, 0));
}

#[test]
fn a_line_longer_than_a_run_holds_is_runs_on_one_line() {
    let line = "word ".repeat(60);
    let (out, runs) = runs_of(&editable(&line, 300, 300..300));
    let lengths: Vec<_> = runs
        .iter()
        .map(|r| r.node.character_lengths().len())
        .collect();
    assert_eq!(lengths, [255, 45]);
    assert_eq!(runs[0].node.next_on_line(), Some(accesskit::NodeId(1001)));
    assert_eq!(
        runs[1].node.previous_on_line(),
        Some(accesskit::NodeId(1000))
    );
    assert_eq!(runs[1].node.word_starts()[0], 0);
    assert_eq!(out.text_selection().unwrap().focus, at(1, 45));
}

#[test]
fn a_selection_from_assistive_technology_comes_back_in_bytes_of_the_value() {
    let (_, runs) = runs_of(&editable("a e\u{301}!\nb", 0, 0..0));
    let select = |anchor, focus| {
        let mut request = request_for(
            accesskit::Action::SetTextSelection,
            Some(accesskit::ActionData::SetTextSelection(
                accesskit::TextSelection { anchor, focus },
            )),
        );
        request.target_node = node_id(id(9));
        super::request(&request, |target| {
            (target == id(9)).then_some(runs.as_slice())
        })
    };
    let back = select(at(1, 1), at(0, 2)).unwrap();
    assert_eq!(
        (back.target, back.action, back.data),
        (
            id(9),
            Action::SetSelection,
            Some(ActionData::Selection(2..8))
        )
    );
    assert_eq!(select(at(0, 6), at(0, 0)), None, "past the end of its run");
    assert_eq!(
        select(
            accesskit::TextPosition {
                node: accesskit::NodeId(7),
                character_index: 0
            },
            at(0, 0)
        ),
        None,
        "in a run of no node"
    );
}

#[test]
fn a_secret_has_no_text_runs_nor_a_selection() {
    let mut field = editable("hunter2", 7, 0..7);
    field.role = Role::PasswordInput;
    let (out, runs) = runs_of(&field);
    assert!(runs.is_empty());
    assert_eq!(out.text_selection(), None);
    assert!(!out.supports_action(accesskit::Action::SetTextSelection));
    let mut field = editable("hunter2", 7, 0..7);
    field.value = Some(AccessibleValue::Protected);
    assert!(runs_of(&field).1.is_empty());
}
