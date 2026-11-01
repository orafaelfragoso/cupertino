use super::*;
use cupertino_geometry::Rect;
use cupertino_ui_core::{AccessibilityNode, AccessibleStates, AccessibleValue, Role};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::task::Wake;

#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::new(1).unwrap())
}

/// Two windows' handlers on one adapter's shared state, with the wakes counted.
fn handlers() -> (Arc<Count>, Handlers, Handlers) {
    let count = Arc::new(Count::default());
    let shared = Arc::new(Shared {
        active: AtomicUsize::new(0),
        epoch: AtomicU64::new(0),
        actions: Mutex::new(Vec::new()),
        waker: Waker::from(Arc::clone(&count)),
    });
    let window = || Handlers {
        shared: Arc::clone(&shared),
        mirror: Arc::new(Mutex::new(Mirror::default())),
    };
    (count, window(), window())
}

fn publish(h: &Handlers, role: Role, value: Option<AccessibleValue>, actions: Vec<Action>) {
    let node = AccessibilityNode {
        role,
        name: Some("field".into()),
        description: None,
        value,
        states: AccessibleStates::default(),
        bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
        actions,
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    };
    let mut m = lock(&h.mirror);
    m.root = Some(id(1));
    m.nodes.insert(
        id(1),
        Published {
            node: convert::node(&node),
            actions: node.actions.clone(),
            protected: convert::protected(&node),
            runs: Vec::new(),
        },
    );
}

fn ask(h: &mut Handlers, action: accesskit::Action, data: Option<accesskit::ActionData>) {
    h.do_action(accesskit::ActionRequest {
        action,
        target_tree: accesskit::TreeId::ROOT,
        target_node: convert::node_id(id(1)),
        data,
    });
}

fn queued(h: &Handlers) -> Vec<Action> {
    lock(&h.shared.actions).iter().map(|r| r.action).collect()
}

#[test]
fn an_advertised_action_reaches_the_loop_and_wakes_it() {
    let (count, mut h, _) = handlers();
    publish(&h, Role::Button, None, vec![Action::Click]);
    ask(&mut h, accesskit::Action::Click, None);
    assert_eq!(queued(&h), [Action::Click]);
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
}

#[test]
fn requests_beyond_what_the_loop_can_hold_are_dropped_until_it_drains() {
    let (_, mut h, _) = handlers();
    publish(&h, Role::Button, None, vec![Action::Click]);
    for _ in 0..MAX_QUEUED + 6 {
        ask(&mut h, accesskit::Action::Click, None);
    }
    assert_eq!(queued(&h).len(), MAX_QUEUED);
    lock(&h.shared.actions).clear();
    ask(&mut h, accesskit::Action::Click, None);
    assert_eq!(queued(&h).len(), 1);
}

#[test]
fn an_action_the_target_did_not_advertise_is_dropped() {
    let (count, mut h, _) = handlers();
    publish(&h, Role::Button, None, vec![Action::Focus]);
    ask(&mut h, accesskit::Action::Click, None);
    let value = Some(accesskit::ActionData::Value("x".into()));
    ask(&mut h, accesskit::Action::SetValue, value);
    assert!(queued(&h).is_empty());
    assert_eq!(count.0.load(Ordering::SeqCst), 0);
}

#[test]
fn a_secret_never_takes_a_value_even_when_it_advertised_one() {
    let value = || Some(accesskit::ActionData::Value("hunter2".into()));
    for (role, secret) in [
        (Role::PasswordInput, None),
        (Role::TextInput, Some(AccessibleValue::Protected)),
    ] {
        let (_, mut h, _) = handlers();
        publish(&h, role, secret, vec![Action::SetValue, Action::Focus]);
        ask(&mut h, accesskit::Action::SetValue, value());
        ask(&mut h, accesskit::Action::Focus, None);
        assert_eq!(queued(&h), [Action::Focus], "{role:?}");
    }
}

#[test]
fn activation_before_any_tree_waits_for_core_and_after_it_hands_the_tree_over() {
    let (count, mut first, mut second) = handlers();
    assert_eq!(first.request_initial_tree(), None);
    assert!(first.shared.active.load(Ordering::SeqCst) == 1);
    assert_eq!(
        count.0.load(Ordering::SeqCst),
        1,
        "core is woken to send the tree"
    );
    publish(&second, Role::Button, None, vec![Action::Click]);
    let tree = second
        .request_initial_tree()
        .expect("the tree core already sent");
    assert_eq!(tree.tree.map(|t| t.root), Some(convert::node_id(id(1))));
    assert_eq!(tree.nodes.len(), 1);
    assert!(lock(&second.mirror).initialized);
    // a second activation of the same window counts once
    second.request_initial_tree();
    assert_eq!(second.shared.active.load(Ordering::SeqCst), 2);
}

#[test]
fn a_tree_from_before_the_last_window_deactivated_is_never_handed_over() {
    let (_, mut first, mut second) = handlers();
    publish(&first, Role::Button, None, vec![]);
    publish(&second, Role::Button, None, vec![]);
    first.request_initial_tree();
    second.request_initial_tree();
    first.deactivate_accessibility();
    assert_eq!(first.shared.active.load(Ordering::SeqCst), 1);
    assert!(!lock(&first.mirror).initialized);
    // core still sends while another window is read, so this window's tree is current
    assert!(first.request_initial_tree().is_some());
    first.deactivate_accessibility();
    second.deactivate_accessibility();
    assert_eq!(second.shared.active.load(Ordering::SeqCst), 0);
    second.deactivate_accessibility();
    assert_eq!(
        second.shared.active.load(Ordering::SeqCst),
        0,
        "never below zero"
    );
    // core stopped sending: both trees may be stale, so both wait for core's whole one
    assert_eq!(first.request_initial_tree(), None);
    assert_eq!(second.request_initial_tree(), None);
}
