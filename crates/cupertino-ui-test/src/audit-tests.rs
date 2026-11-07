use super::*;
use cupertino_ui_core::{AccessibleStates, Checked};
use cupertino_ui_platform::WindowId;

fn id(n: u32) -> NodeId {
    NodeId::new(n, std::num::NonZeroU32::MIN)
}

fn node(role: Role, name: Option<&str>, children: &[u32]) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: name.map(str::to_owned),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
        actions: Vec::new(),
        relations: Vec::new(),
        children: children.iter().copied().map(id).collect(),
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    }
}

fn control(role: Role, name: Option<&str>) -> AccessibilityNode {
    let mut n = node(role, name, &[]);
    n.states.focusable = true;
    n.actions = vec![Action::Focus, Action::Click];
    n
}

fn tree(nodes: Vec<(u32, AccessibilityNode)>, focus: Option<u32>) -> AccessibilityUpdate {
    AccessibilityUpdate {
        window: WindowId(1),
        root: id(1),
        focus: focus.map(id),
        nodes: nodes.into_iter().map(|(n, node)| (id(n), node)).collect(),
        removed: Vec::new(),
        full: true,
    }
}

#[test]
fn a_tree_of_named_controls_with_their_states_has_nothing_to_report() {
    let mut wifi = control(Role::Switch, Some("Wi-Fi"));
    wifi.value = Some(AccessibleValue::Toggle(Checked::True));
    wifi.states.focused = true;
    let mut volume = control(Role::Slider, Some("Volume"));
    volume.value = Some(AccessibleValue::Number {
        value: 0.5,
        min: 0.0,
        max: 1.0,
        step: None,
    });
    let mut menu = control(Role::PopUpButton, Some("Colour filter"));
    menu.states.expanded = Some(false);
    let mut list = node(Role::List, Some("Sidebar"), &[6, 7]);
    list.item_count = Some(2);
    let mut home = control(Role::ListItem, Some("Home"));
    home.item_index = Some(0);
    let mut docs = control(Role::ListItem, Some("Documents"));
    docs.item_index = Some(1);
    let mut password = control(Role::PasswordInput, Some("Password"));
    password.value = Some(AccessibleValue::Protected);
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Settings"), &[2, 3, 4, 5, 8])),
            (2, wifi),
            (3, volume),
            (4, menu),
            (5, list),
            (6, home),
            (7, docs),
            (8, password),
        ],
        Some(2),
    );
    assert_eq!(accessibility_audit(&t), Vec::<String>::new());
}

#[test]
fn unnamed_controls_and_clickable_text_are_reported() {
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Panel"), &[2, 3, 4])),
            (2, control(Role::Button, None)),
            (3, control(Role::Group, None)),
            (4, control(Role::StaticText, Some("Open"))),
        ],
        None,
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            "Window \"Panel\" > Button #0: no name",
            "Window \"Panel\" > Group #1: a control without a name",
            "Window \"Panel\" > StaticText \"Open\": takes [Click] but its role is no control",
        ]
    );
}

#[test]
fn values_a_reader_needs_are_reported_missing_or_out_of_range() {
    let mut progress = node(Role::ProgressIndicator, Some("Copying"), &[]);
    progress.value = Some(AccessibleValue::Number {
        value: 2.0,
        min: 0.0,
        max: 1.0,
        step: None,
    });
    let mut password = control(Role::PasswordInput, Some("Password"));
    password.value = Some(AccessibleValue::Text {
        value: "hunter2".into(),
        editing: None,
    });
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Panel"), &[2, 3, 4, 5, 6])),
            (2, control(Role::CheckBox, Some("Reopen windows"))),
            (3, control(Role::Slider, Some("Brightness"))),
            (4, progress),
            (5, password),
            (6, control(Role::PopUpButton, Some("Show date"))),
        ],
        None,
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            "Window \"Panel\" > CheckBox \"Reopen windows\": does not say whether it is on or off",
            "Window \"Panel\" > Slider \"Brightness\": does not read a number within its range",
            "Window \"Panel\" > ProgressIndicator \"Copying\": reads progress outside its range",
            "Window \"Panel\" > PasswordInput \"Password\": reads its secret as text",
            "Window \"Panel\" > PopUpButton \"Show date\": does not say whether its menu is open",
        ]
    );
}

#[test]
fn a_mask_that_tells_a_secrets_length_and_the_secret_itself_are_reported() {
    let mut field = control(Role::PasswordInput, Some("Password"));
    field.description = Some("•••••".into());
    field.value = Some(AccessibleValue::Protected);
    let mut hint = node(Role::StaticText, Some("You typed hunter2"), &[]);
    hint.description = Some("Version 1 • 2".into());
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Lock"), &[2, 3])),
            (2, field),
            (3, hint),
        ],
        None,
    );
    assert_eq!(
        accessibility_audit(&t),
        ["Window \"Lock\" > PasswordInput \"Password\": reads a mask, which tells a secret's length"]
    );
    assert_eq!(
        secret_leaks(&t, &["hunter2", ""]),
        ["StaticText \"You typed hunter2\": reads a secret"]
    );
}

#[test]
fn focus_the_tree_does_not_hold_or_that_disagrees_with_the_states_is_reported() {
    let mut a = control(Role::Button, Some("OK"));
    a.states.focused = true;
    let text = node(Role::StaticText, Some("Title"), &[]);
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Dialog"), &[2, 3])),
            (2, a.clone()),
            (3, text.clone()),
        ],
        Some(3),
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            "Window \"Dialog\" > Button \"OK\": says it is focused, but focus is elsewhere",
            "Window \"Dialog\" > StaticText \"Title\": has the focus but does not say so",
            "focus StaticText \"Title\": cannot take focus",
        ]
    );
    let t = tree(
        vec![(1, node(Role::Window, Some("Dialog"), &[2])), (2, a)],
        Some(9),
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            "Window \"Dialog\" > Button \"OK\": says it is focused, but focus is elsewhere",
            &format!("focus {:?}: not in the tree", id(9)),
        ]
    );
}

#[test]
fn items_need_a_container_a_place_and_a_count() {
    let mut open = control(Role::MenuItem, Some("Open"));
    open.item_index = Some(0);
    let mut recent = control(Role::ListItem, Some("notes.txt"));
    recent.item_index = Some(0);
    let mut list = node(Role::List, Some("Projects"), &[7]);
    list.item_count = Some(1);
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Menus"), &[2, 4, 6])),
            (2, node(Role::Group, None, &[3])),
            (3, open),
            (4, node(Role::List, Some("Recent"), &[5])),
            (5, recent),
            (6, list),
            (7, control(Role::ListItem, Some("Cupertino"))),
        ],
        None,
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            "Window \"Menus\" > Group #0 > MenuItem \"Open\": an item outside a [Menu, MenuBar]",
            "Window \"Menus\" > List \"Recent\" > ListItem \"notes.txt\": its list does not say how many items it holds",
            "Window \"Menus\" > List \"Projects\" > ListItem \"Cupertino\": does not say its place in its list",
        ]
    );
}

#[test]
fn places_beyond_the_count_or_shared_by_two_items_are_reported() {
    let mut menu = node(Role::Menu, Some("File"), &[2, 3, 4]);
    menu.item_count = Some(2);
    let mut open = control(Role::MenuItem, Some("Open"));
    open.item_index = Some(0);
    let mut close = control(Role::MenuItem, Some("Close"));
    close.item_index = Some(0);
    let mut quit = control(Role::MenuItem, Some("Quit"));
    quit.item_index = Some(2);
    let t = tree(vec![(1, menu), (2, open), (3, close), (4, quit)], None);
    assert_eq!(
        accessibility_audit(&t),
        [
            "Menu \"File\" > MenuItem \"Quit\": place 2 of 2",
            "Menu \"File\" > MenuItem \"Close\": has place 0, as Menu \"File\" > MenuItem \"Open\" does",
        ]
    );
}

#[test]
fn relations_to_missing_nodes_and_controls_without_area_are_reported() {
    let mut field = control(Role::TextInput, None);
    field.relations = vec![Relation::LabelledBy(id(9))];
    field.value = Some(AccessibleValue::Text {
        value: String::new(),
        editing: None,
    });
    let mut hidden = control(Role::Button, Some("Close"));
    hidden.bounds = Rect::new(4.0, 4.0, 0.0, 0.0);
    let t = tree(
        vec![
            (1, node(Role::Window, Some("Form"), &[2, 3])),
            (2, field),
            (3, hidden),
        ],
        None,
    );
    assert_eq!(
        accessibility_audit(&t),
        [
            &format!(
                "Window \"Form\" > TextInput #0: LabelledBy({:?}) names a node not in the tree",
                id(9)
            ),
            "Window \"Form\" > Button \"Close\": a control with no area on screen",
        ]
    );
}

#[test]
fn reading_order_allows_rows_and_the_next_column_but_not_going_back() {
    let at = |n: u32, x: f64, y: f64| {
        let mut b = control(Role::Button, Some(&format!("b{n}")));
        b.bounds = Rect::new(x, y, 40.0, 20.0);
        (n, b)
    };
    let t = tree(
        vec![
            (1, node(Role::Window, Some("W"), &[2, 3, 4, 5, 6])),
            at(2, 0.0, 0.0),
            at(3, 50.0, 0.0),
            at(4, 0.0, 30.0),
            at(5, 100.0, 0.0),
            at(6, 100.0, 60.0),
        ],
        None,
    );
    // a row left to right, the next row, the top of the next column, down it
    assert_eq!(
        reading_order_breaks(&t, &[id(2), id(3), id(4), id(5), id(6)]),
        Vec::<String>::new()
    );
    // right to left on a row, and up the same column
    assert_eq!(
        reading_order_breaks(&t, &[id(3), id(2), id(6), id(5)]),
        [
            "Button \"b3\" focusable -> Button \"b2\" focusable",
            "Button \"b6\" focusable -> Button \"b5\" focusable",
        ]
    );
}
