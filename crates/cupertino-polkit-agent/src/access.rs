//! What the prompt tells assistive technology: a modal alert dialog labelled by its title and described by polkit's
//! message. A secret's value is protected, so neither what is typed nor its length leaves the process, and assistive
//! technology can't type into the field; the error line is announced as soon as it shows.

use std::num::NonZeroU32;

use cupertino_geometry::Rect;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleStates, AccessibleValue, Action, Live,
    NodeId, Relation, Role,
};
use cupertino_ui_platform::WindowId;

use crate::model::Phase;
use crate::panel::{
    field_label, Control, Status, View, AUTHENTICATE, CANCEL, DETAILS, TITLE, USER,
};

/// The nodes of the panel; their ids stay the same across updates, so assistive technology keeps its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptNode {
    Dialog = 1,
    Title,
    Message,
    User,
    Field,
    Details,
    Cancel,
    Authenticate,
    /// The first status line; line `i` is this plus `i`.
    Status = 16,
    /// The first detail row; row `i` is this plus `i`.
    Detail = 32,
}

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::MIN)
}

impl PromptNode {
    pub fn id(self) -> NodeId {
        id(self as u32)
    }
}

/// The whole tree of the panel `view` on `window`.
pub fn prompt_tree(window: WindowId, view: &View<'_>) -> AccessibilityUpdate {
    let frames = view.frames;
    let prompt = view.prompt;
    let checking = prompt.phase == Phase::Checking;
    let focused = |control: Control| view.panel.focus == control;
    let mut nodes = vec![
        (PromptNode::Title.id(), text(TITLE, frames.title)),
        (
            PromptNode::Message.id(),
            text(
                &prompt.request.message,
                frames
                    .message
                    .iter()
                    .copied()
                    .reduce(|a, b| a.union(&b))
                    .unwrap_or(frames.title),
            ),
        ),
    ];
    if let (Some(name), Some(frame)) = (view.user, frames.user) {
        let mut user = control(
            Role::PopUpButton,
            USER,
            frame,
            focused(Control::User),
            false,
        );
        user.value = Some(AccessibleValue::Text {
            value: name.to_owned(),
            editing: None,
        });
        // a click and the arrows step to the next user in place: a list that never opens, so it reads as collapsed
        user.states.expanded = Some(false);
        nodes.push((PromptNode::User.id(), user));
    }
    nodes.push((PromptNode::Field.id(), field(view, focused(Control::Field))));
    for (i, (status, frame)) in view.status.iter().zip(&frames.status).enumerate() {
        let mut line = text(status.text(), *frame);
        line.live = match status {
            Status::Error(_) => Some(Live::Assertive),
            Status::Info(_) => Some(Live::Polite),
            Status::CapsLock => None,
        };
        nodes.push((id(PromptNode::Status as u32 + i as u32), line));
    }
    let mut details = control(
        Role::Button,
        DETAILS,
        frames.details,
        focused(Control::Details),
        false,
    );
    details.states.expanded = Some(view.panel.details);
    nodes.push((PromptNode::Details.id(), details));
    if view.panel.details {
        for (i, ((label, value), frame)) in view.details.iter().zip(&frames.detail_rows).enumerate()
        {
            nodes.push((
                id(PromptNode::Detail as u32 + i as u32),
                text(&format!("{label}: {value}"), *frame),
            ));
        }
    }
    nodes.push((
        PromptNode::Cancel.id(),
        control(
            Role::Button,
            CANCEL,
            frames.cancel,
            focused(Control::Cancel),
            false,
        ),
    ));
    nodes.push((
        PromptNode::Authenticate.id(),
        control(
            Role::Button,
            AUTHENTICATE,
            frames.authenticate,
            focused(Control::Authenticate),
            !prompt.can_answer(),
        ),
    ));
    let mut root = node(
        Role::AlertDialog,
        Some(TITLE),
        Rect::new(0.0, 0.0, frames.width, frames.height),
    );
    root.states.modal = true;
    root.states.busy = checking;
    root.relations = vec![
        Relation::LabelledBy(PromptNode::Title.id()),
        Relation::DescribedBy(PromptNode::Message.id()),
    ];
    root.children = nodes.iter().map(|(id, _)| *id).collect();
    let focus = match view.panel.focus {
        Control::User => PromptNode::User,
        Control::Field => PromptNode::Field,
        Control::Details => PromptNode::Details,
        Control::Cancel => PromptNode::Cancel,
        Control::Authenticate => PromptNode::Authenticate,
    };
    nodes.insert(0, (PromptNode::Dialog.id(), root));
    AccessibilityUpdate {
        window,
        root: PromptNode::Dialog.id(),
        focus: Some(focus.id()),
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// What a click from assistive technology on node `id` does: the same as a click on that control.
pub fn clicked(id: NodeId) -> Option<Control> {
    [
        (PromptNode::User, Control::User),
        (PromptNode::Details, Control::Details),
        (PromptNode::Cancel, Control::Cancel),
        (PromptNode::Authenticate, Control::Authenticate),
    ]
    .into_iter()
    .find(|(node, _)| node.id() == id)
    .map(|(_, control)| control)
}

/// The field: a secret's value is protected in every phase; a question PAM asks in clear shows what is typed.
fn field(view: &View<'_>, focused: bool) -> AccessibilityNode {
    let prompt = view.prompt;
    let label = field_label(&prompt.question);
    let (role, value) = match prompt.shown_field() {
        Some(shown) => (
            Role::TextInput,
            AccessibleValue::Text {
                value: shown.to_owned(),
                editing: None,
            },
        ),
        None => (Role::PasswordInput, AccessibleValue::Protected),
    };
    AccessibilityNode {
        value: Some(value),
        states: AccessibleStates {
            focusable: true,
            focused,
            disabled: prompt.phase == Phase::Checking,
            invalid: prompt.error.is_some(),
            ..AccessibleStates::default()
        },
        actions: vec![Action::Focus],
        ..node(role, Some(&label), view.frames.field)
    }
}

pub(crate) fn control(
    role: Role,
    name: &str,
    frame: Rect,
    focused: bool,
    disabled: bool,
) -> AccessibilityNode {
    AccessibilityNode {
        actions: vec![Action::Click],
        states: AccessibleStates {
            focusable: true,
            focused,
            disabled,
            ..AccessibleStates::default()
        },
        ..node(role, Some(name), frame)
    }
}

pub(crate) fn text(content: &str, bounds: Rect) -> AccessibilityNode {
    node(Role::StaticText, Some(content), bounds)
}

pub(crate) fn node(role: Role, name: Option<&str>, bounds: Rect) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: name.map(str::to_owned),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds,
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    }
}

#[cfg(test)]
mod tests;
