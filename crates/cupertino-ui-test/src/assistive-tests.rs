use super::*;
use cupertino_geometry::Rect;
use cupertino_ui_core::{AccessibleStates, Role};

fn id(n: u32) -> NodeId {
    NodeId::new(n, std::num::NonZeroU32::MIN)
}

fn node(role: Role, name: &str, children: &[u32]) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: Some(name.to_owned()),
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

fn update(
    full: bool,
    focus: Option<u32>,
    nodes: Vec<(u32, AccessibilityNode)>,
) -> AccessibilityUpdate {
    AccessibilityUpdate {
        window: WindowId(1),
        root: id(1),
        focus: focus.map(id),
        nodes: nodes.into_iter().map(|(n, node)| (id(n), node)).collect(),
        removed: Vec::new(),
        full,
    }
}

#[test]
fn partial_updates_are_put_together_into_the_whole_tree() {
    let at = AssistiveTechnology::new();
    let mut adapter = at.adapter();
    adapter.update(&update(
        true,
        Some(2),
        vec![
            (1, node(Role::Window, "Panel", &[2, 3])),
            (2, node(Role::Button, "OK", &[])),
            (3, node(Role::Button, "Cancel", &[])),
        ],
    ));
    let mut gone = update(false, Some(2), vec![(1, node(Role::Window, "Panel", &[2]))]);
    gone.removed = vec![id(3)];
    adapter.update(&gone);
    let tree = at.tree(WindowId(1)).unwrap();
    let names: Vec<_> = tree
        .nodes
        .iter()
        .map(|(_, n)| n.name.clone().unwrap())
        .collect();
    assert_eq!(
        (names, tree.focus, at.focus_moves(WindowId(1))),
        (vec!["Panel".to_owned(), "OK".to_owned()], Some(id(2)), 0)
    );
    adapter.update(&update(false, Some(1), Vec::new()));
    assert_eq!(at.focus_moves(WindowId(1)), 1);
    adapter.window_closed(WindowId(1));
    assert_eq!(at.tree(WindowId(1)), None);
}

#[test]
fn a_live_node_is_said_when_it_arrives_and_when_its_words_change() {
    let at = AssistiveTechnology::new();
    let mut adapter = at.adapter();
    let mut volume = node(Role::Slider, "Volume", &[]);
    volume.live = Some(Live::Polite);
    volume.value = Some(AccessibleValue::Number {
        value: 0.5,
        min: 0.0,
        max: 1.0,
        step: None,
    });
    adapter.update(&update(
        true,
        None,
        vec![(1, node(Role::Window, "OSD", &[2])), (2, volume.clone())],
    ));
    // the same words again are not said again
    adapter.update(&update(false, None, vec![(2, volume.clone())]));
    volume.value = Some(AccessibleValue::Number {
        value: 0.75,
        min: 0.0,
        max: 1.0,
        step: None,
    });
    adapter.update(&update(false, None, vec![(2, volume)]));
    let said: Vec<_> = at
        .take_announcements()
        .into_iter()
        .map(|a| (a.live, a.text))
        .collect();
    assert_eq!(
        said,
        [
            (Live::Polite, "Volume, 0.5".to_owned()),
            (Live::Polite, "Volume, 0.75".to_owned()),
        ]
    );
    assert!(at.announcements().is_empty());
}

#[test]
fn asked_actions_are_handed_to_the_runtime_once() {
    let at = AssistiveTechnology::new();
    let mut adapter = at.adapter();
    at.ask(id(2), Action::Click, None);
    let mut out = Vec::new();
    adapter.drain_actions(&mut out);
    adapter.drain_actions(&mut out);
    assert_eq!(
        out,
        [ActionRequest {
            target: id(2),
            action: Action::Click,
            data: None
        }]
    );
    adapter.window_focus(WindowId(1), true);
    adapter.session_locked(true);
    assert_eq!(
        (at.focused_window(), at.locked()),
        (Some(WindowId(1)), true)
    );
}
