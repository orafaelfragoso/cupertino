use super::*;
use crate::agent::{Request, User};
use crate::helper::Line;
use crate::model::{Agent, Key};
use crate::panel::{Content, Frames, Panel};

const WINDOW: WindowId = WindowId(7);

fn asking(echo: bool, users: Vec<User>) -> Agent {
    let mut agent = Agent::new(User(1000));
    agent.begin(Request {
        cookie: "c".into(),
        action: "org.example.mount".into(),
        message: "Authentication is required to mount the disk".into(),
        details: Vec::new(),
        users,
    });
    agent.helper(Line::Prompt {
        text: "Password: ".into(),
        echo,
    });
    agent
}

fn typed(agent: &mut Agent, text: &str) {
    for c in text.chars() {
        agent.key(Key::Char(c));
    }
}

fn tree(agent: &Agent, panel: &Panel, user: Option<&str>) -> AccessibilityUpdate {
    let prompt = agent.prompt().unwrap();
    let status = Status::of(prompt, false);
    let details = [("Action", prompt.request.action.clone())];
    let frames = Frames::new(&Content {
        message_lines: 1,
        users: user.is_some(),
        status_lines: status.len(),
        details: panel.details.then_some(details.len()),
        cancel_title: 40.0,
        authenticate_title: 80.0,
    });
    prompt_tree(
        WINDOW,
        &View {
            frames: &frames,
            panel,
            prompt,
            message: std::slice::from_ref(&prompt.request.message),
            user,
            status: &status,
            details: &details,
        },
    )
}

fn node(update: &AccessibilityUpdate, which: PromptNode) -> &AccessibilityNode {
    &update
        .nodes
        .iter()
        .find(|(id, _)| *id == which.id())
        .unwrap_or_else(|| panic!("no {which:?}"))
        .1
}

#[test]
fn the_dialog_is_a_modal_alert_labelled_by_its_title_and_described_by_the_message() {
    let agent = asking(false, vec![User(1000)]);
    let update = tree(&agent, &Panel::new(false), None);
    assert_eq!(update.root, PromptNode::Dialog.id());
    let dialog = node(&update, PromptNode::Dialog);
    assert_eq!(dialog.role, Role::AlertDialog);
    assert!(dialog.states.modal);
    assert_eq!(
        dialog.relations,
        [
            Relation::LabelledBy(PromptNode::Title.id()),
            Relation::DescribedBy(PromptNode::Message.id())
        ]
    );
    assert_eq!(
        node(&update, PromptNode::Message).name.as_deref(),
        Some("Authentication is required to mount the disk")
    );
    assert_eq!(update.focus, Some(PromptNode::Field.id()));
    assert!(update
        .nodes
        .iter()
        .all(|(id, _)| *id != PromptNode::User.id()));
}

#[test]
fn a_secret_is_never_in_the_tree_nor_its_length_in_any_phase() {
    let mut agent = asking(false, vec![User(1000)]);
    let panel = Panel::new(false);
    typed(&mut agent, "h");
    let one = format!("{:?}", tree(&agent, &panel, None));
    typed(&mut agent, "unter2");
    let typing = tree(&agent, &panel, None);
    let field = node(&typing, PromptNode::Field);
    assert_eq!(field.role, Role::PasswordInput);
    assert_eq!(field.value, Some(AccessibleValue::Protected));
    assert_eq!(format!("{typing:?}"), one, "more typing tells nothing more");
    agent.key(Key::Return);
    let checking = tree(&agent, &panel, None);
    let dump = format!("{checking:?}");
    assert!(!dump.contains("hunter2") && !dump.contains('•'));
    assert!(node(&checking, PromptNode::Dialog).states.busy);
    assert!(node(&checking, PromptNode::Field).states.disabled);
}

#[test]
fn a_question_asked_in_clear_shows_what_is_typed() {
    let mut agent = asking(true, vec![User(1000)]);
    typed(&mut agent, "alice");
    let update = tree(&agent, &Panel::new(false), None);
    let field = node(&update, PromptNode::Field);
    assert_eq!(field.role, Role::TextInput);
    assert_eq!(
        field.value,
        Some(AccessibleValue::Text {
            value: "alice".into(),
            editing: None
        })
    );
}

#[test]
fn a_failed_attempt_is_announced_and_marks_the_field() {
    let mut agent = asking(false, vec![User(1000)]);
    typed(&mut agent, "wrong");
    agent.key(Key::Return);
    agent.helper(Line::Failure);
    let update = tree(&agent, &Panel::new(false), None);
    let line = node(&update, PromptNode::Status);
    assert_eq!(line.name.as_deref(), Some(crate::model::TRY_AGAIN));
    assert_eq!(line.live, Some(Live::Assertive));
    assert!(node(&update, PromptNode::Field).states.invalid);
    assert!(node(&update, PromptNode::Authenticate).states.disabled);
}

#[test]
fn the_user_pop_up_and_the_details_follow_the_panel() {
    let mut agent = asking(false, vec![User(1000), User(0)]);
    typed(&mut agent, "x");
    let mut panel = Panel::new(true);
    panel.focus = Control::Details;
    let collapsed = tree(&agent, &panel, Some("Ada"));
    assert_eq!(
        node(&collapsed, PromptNode::User).value,
        Some(AccessibleValue::Text {
            value: "Ada".into(),
            editing: None
        })
    );
    assert_eq!(collapsed.focus, Some(PromptNode::Details.id()));
    assert_eq!(
        node(&collapsed, PromptNode::Details).states.expanded,
        Some(false)
    );
    assert!(!node(&collapsed, PromptNode::Authenticate).states.disabled);
    panel.details = true;
    let expanded = tree(&agent, &panel, Some("Ada"));
    assert_eq!(
        node(&expanded, PromptNode::Detail).name.as_deref(),
        Some("Action: org.example.mount")
    );
}

#[test]
fn a_click_from_assistive_technology_names_the_control_a_click_would() {
    assert_eq!(clicked(PromptNode::Cancel.id()), Some(Control::Cancel));
    assert_eq!(
        clicked(PromptNode::Authenticate.id()),
        Some(Control::Authenticate)
    );
    assert_eq!(clicked(PromptNode::Details.id()), Some(Control::Details));
    assert_eq!(clicked(PromptNode::User.id()), Some(Control::User));
    assert_eq!(clicked(PromptNode::Field.id()), None);
}

/// The audit's findings and the secret's leaks in `update`, with its outline when there are any.
fn audit(what: &str, update: &AccessibilityUpdate) {
    let mut problems = cupertino_ui_test::accessibility_audit(update);
    problems.extend(cupertino_ui_test::secret_leaks(
        update,
        &["hunter2", "wrong"],
    ));
    assert!(
        problems.is_empty(),
        "{what}: {problems:#?}\n{}",
        cupertino_ui_test::semantic_outline(update).join("\n")
    );
}

#[test]
fn every_phase_passes_the_accessibility_audit_and_leaks_no_secret() {
    let panel = Panel::new(false);
    let mut agent = asking(false, vec![User(1000)]);
    audit("asking", &tree(&agent, &panel, None));
    typed(&mut agent, "hunter2");
    audit("typing", &tree(&agent, &panel, None));
    agent.key(Key::Return);
    audit("checking", &tree(&agent, &panel, None));
    agent.helper(Line::Failure);
    audit("failed", &tree(&agent, &panel, None));
    typed(&mut agent, "wrong");
    audit("typing again", &tree(&agent, &panel, None));

    let mut agent = asking(false, vec![User(1000), User(0)]);
    let mut panel = Panel::new(true);
    audit("with users", &tree(&agent, &panel, Some("Ada")));
    panel.focus = Control::Details;
    panel.details = true;
    typed(&mut agent, "hunter2");
    audit("details shown", &tree(&agent, &panel, Some("Ada")));

    let mut clear = asking(true, vec![User(1000)]);
    typed(&mut clear, "alice");
    audit("asked in clear", &tree(&clear, &Panel::new(false), None));
}
