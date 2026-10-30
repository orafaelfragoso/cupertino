use super::*;
use crate::model::{AuthOutcome, Key};
use crate::panel::{Action as PanelAction, Decision, Kind};

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
    assert!(update.nodes.iter().all(|(_, n)| n.live.is_none()));
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
fn every_rejected_attempt_is_announced_the_same_and_an_error_says_what_the_screen_shows() {
    let mut denied = screen();
    type_str(&mut denied, "wrong");
    denied.press(1.0, Key::Return);
    assert!(
        found(&tree(&denied), LockNode::Denied).is_none(),
        "not while the attempt runs"
    );
    denied.finish_auth(2.0, AuthOutcome::Denied).unwrap();
    let update = tree(&denied);
    let said = found(&update, LockNode::Denied).unwrap();
    assert_eq!(
        (said.name.as_deref(), said.live),
        (Some(DENIED), Some(Live::Assertive))
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
    let said = found(&update, LockNode::Notice).unwrap();
    assert_eq!(
        (said.name.as_deref(), said.live),
        (error.notice(), Some(Live::Assertive))
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

#[test]
fn a_panel_reads_its_title_countdown_and_buttons_with_the_default_one_focused() {
    let frames = Kind::LogOut.frames(2, 1);
    let update = panel_tree(WindowId(7), Kind::LogOut, &frames, Some(42));
    let root = &update.nodes[0].1;
    assert_eq!((root.role, root.states.modal), (Role::AlertDialog, true));
    let texts: Vec<_> = update.nodes[1..]
        .iter()
        .map(|(_, n)| (n.role, n.name.clone().unwrap()))
        .collect();
    assert_eq!(
        texts[0],
        (Role::StaticText, Kind::LogOut.text().title.to_string())
    );
    assert_eq!(
        texts[1],
        (
            Role::StaticText,
            Kind::LogOut.countdown_message(42).unwrap()
        )
    );
    let buttons: Vec<_> = frames.buttons.iter().map(|b| b.label.to_string()).collect();
    assert_eq!(
        texts[2..]
            .iter()
            .map(|(_, name)| name.clone())
            .collect::<Vec<_>>(),
        buttons
    );
    let focused = update.focus.unwrap();
    assert_eq!(
        panel_click(&frames, focused),
        Some(Decision::Do(PanelAction::LogOut)),
        "Return's button has the focus"
    );
}

#[test]
fn a_click_from_assistive_technology_decides_what_a_click_on_the_button_does() {
    let frames = Kind::PowerButton.frames(1, 0);
    let update = panel_tree(WindowId(7), Kind::PowerButton, &frames, None);
    assert!(update
        .nodes
        .iter()
        .all(|(_, n)| n.name.as_deref() != Some("")));
    for (i, button) in frames.buttons.iter().enumerate() {
        let id = NodeId::new(10 + i as u32, NonZeroU32::MIN);
        assert_eq!(panel_click(&frames, id), Some(button.decision));
        let node = &update.nodes.iter().find(|(n, _)| *n == id).unwrap().1;
        assert_eq!(node.actions, [Action::Click]);
    }
    assert_eq!(
        panel_click(&frames, NodeId::new(2, NonZeroU32::MIN)),
        None,
        "the title"
    );
    assert_eq!(
        panel_click(&frames, NodeId::new(10 + 9, NonZeroU32::MIN)),
        None
    );
}
