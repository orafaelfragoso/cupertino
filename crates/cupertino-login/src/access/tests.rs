use super::*;
use crate::model::{AuthOutcome, Key};

fn screen() -> LockScreen {
    LockScreen::new(0.0, 30.0)
}

fn tree(screen: &LockScreen) -> AccessibilityUpdate {
    let layout = Layout::new(1280.0, 800.0, 0.25);
    lock_tree(
        WindowId(3),
        &LockView {
            screen,
            layout: &layout,
            size: (1280.0, 800.0),
            user_name: "Ada Lovelace",
            time: "9:41",
            date: Some("Friday 30 October"),
            caps_lock: false,
            help: None,
            power: true,
        },
    )
}

fn found(update: &AccessibilityUpdate, node: LockNode) -> Option<&AccessibilityNode> {
    update
        .nodes
        .iter()
        .find(|(id, _)| *id == node.id())
        .map(|(_, n)| n)
}

fn named(update: &AccessibilityUpdate) -> Vec<(Role, String)> {
    update
        .nodes
        .iter()
        .map(|(_, n)| (n.role, n.name.clone().unwrap_or_default()))
        .collect()
}

fn type_str(screen: &mut LockScreen, text: &str) {
    for c in text.chars() {
        screen.press(1.0, Key::Char(c));
    }
}

#[test]
fn a_locked_screen_names_its_clock_user_field_and_power_item_with_the_field_focused() {
    let update = tree(&screen());
    assert!(update.full);
    assert_eq!(
        (update.window, update.root),
        (WindowId(3), LockNode::Window.id())
    );
    assert_eq!(update.focus, Some(LockNode::Password.id()));
    assert_eq!(
        named(&update),
        [
            (Role::Window, "Locked".into()),
            (Role::StaticText, "9:41".into()),
            (Role::StaticText, "Friday 30 October".into()),
            (Role::StaticText, "Ada Lovelace".into()),
            (Role::PasswordInput, "Password".into()),
            (Role::Button, "Power".into()),
        ]
    );
    let root = found(&update, LockNode::Window).unwrap();
    assert_eq!(root.children.len(), update.nodes.len() - 1);
    assert_eq!(
        found(&update, LockNode::Power).unwrap().actions,
        [Action::Click]
    );
}

#[test]
fn the_password_never_shows_its_text_or_length_nor_takes_one() {
    let empty = tree(&screen());
    let field = found(&empty, LockNode::Password).unwrap();
    assert_eq!(field.value, Some(AccessibleValue::Protected));
    assert_eq!(field.actions, [Action::Focus]);
    // whatever is typed, the tree is the same one: nothing in it depends on the password
    for typed in ["s", "secret", "a much longer passphrase"] {
        let mut screen = screen();
        type_str(&mut screen, typed);
        assert_eq!(tree(&screen), empty, "after {} characters", typed.len());
    }
}

#[test]
fn every_rejected_attempt_says_the_same_and_an_error_says_what_the_screen_shows() {
    let mut denied = screen();
    type_str(&mut denied, "wrong");
    denied.press(1.0, Key::Return);
    assert!(
        found(&tree(&denied), LockNode::Denied).is_none(),
        "not while the attempt runs"
    );
    denied.finish_auth(2.0, AuthOutcome::Denied).unwrap();
    let update = tree(&denied);
    assert_eq!(
        found(&update, LockNode::Denied).unwrap().name.as_deref(),
        Some(DENIED)
    );
    assert!(found(&update, LockNode::Password).unwrap().states.invalid);
    denied.press(3.0, Key::Char('x'));
    assert!(
        found(&tree(&denied), LockNode::Denied).is_none(),
        "gone with the next edit"
    );

    let mut error = screen();
    type_str(&mut error, "pw");
    error.press(1.0, Key::Return);
    error
        .finish_auth(2.0, AuthOutcome::Error("The account is locked.".into()))
        .unwrap();
    let update = tree(&error);
    assert!(found(&update, LockNode::Denied).is_none());
    assert_eq!(
        found(&update, LockNode::Notice).unwrap().name.as_deref(),
        error.notice()
    );
}

#[test]
fn an_attempt_marks_the_field_busy() {
    let mut screen = screen();
    type_str(&mut screen, "pw");
    screen.press(1.0, Key::Return);
    assert!(
        found(&tree(&screen), LockNode::Password)
            .unwrap()
            .states
            .busy
    );
}

#[test]
fn each_node_id_names_its_node() {
    for node in [LockNode::Window, LockNode::Password, LockNode::Power] {
        assert_eq!(LockNode::of(node.id()), Some(node));
    }
    assert_eq!(LockNode::of(NodeId::new(99, NonZeroU32::MIN)), None);
}
