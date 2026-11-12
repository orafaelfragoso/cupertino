//! What the keyring prompt tells assistive technology: a modal alert dialog labelled by its heading and described by
//! the caller's description. The password fields' values are protected, so neither what is typed nor its length leaves
//! the process; the warning and the mismatch are announced as soon as they show.

use std::num::NonZeroU32;

use cupertino_geometry::Rect;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleStates, AccessibleValue, Action, Checked,
    Live, NodeId, Relation, Role,
};
use cupertino_ui_platform::WindowId;

use crate::access::{control, node, text};
use crate::keyring::model::Phase;
use crate::keyring::panel::{Control, Status, View, CONFIRM, PASSWORD};

/// The nodes of the panel; their ids stay the same across updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyringNode {
    Dialog = 1,
    Heading,
    Description,
    Password,
    Confirm,
    Choice,
    Cancel,
    Continue,
    /// The first status line; line `i` is this plus `i`.
    Status = 16,
}

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::MIN)
}

impl KeyringNode {
    pub fn id(self) -> NodeId {
        id(self as u32)
    }
}

fn union(frames: &[Rect]) -> Option<Rect> {
    frames.iter().copied().reduce(|a, b| a.union(&b))
}

/// The whole tree of the panel `view` on `window`.
pub fn keyring_tree(window: WindowId, view: &View<'_>) -> AccessibilityUpdate {
    let frames = view.frames;
    let prompt = view.prompt;
    let checking = prompt.phase != Phase::Asking;
    let focused = |c: Control| view.panel.focus == c;
    let heading = view.heading.join(" ");
    let heading_frame = union(&frames.heading).unwrap_or(frames.symbol);
    let mut nodes = vec![(KeyringNode::Heading.id(), text(&heading, heading_frame))];
    let description = prompt.properties.description.trim();
    if let Some(frame) = union(&frames.description) {
        nodes.push((KeyringNode::Description.id(), text(description, frame)));
    }
    let warned = !prompt.properties.warning.trim().is_empty();
    for (node_id, frame, name, control_id, invalid) in [
        (
            KeyringNode::Password,
            frames.password,
            PASSWORD,
            Control::Password,
            warned,
        ),
        (
            KeyringNode::Confirm,
            frames.confirm,
            CONFIRM,
            Control::Confirm,
            prompt.mismatch,
        ),
    ] {
        if let Some(frame) = frame {
            nodes.push((
                node_id.id(),
                AccessibilityNode {
                    value: Some(AccessibleValue::Protected),
                    states: AccessibleStates {
                        focusable: true,
                        focused: focused(control_id),
                        disabled: checking,
                        invalid,
                        ..AccessibleStates::default()
                    },
                    actions: vec![Action::Focus],
                    ..node(Role::PasswordInput, Some(name), frame)
                },
            ));
        }
    }
    if let Some(frame) = frames.choice {
        let mut choice = control(
            Role::CheckBox,
            &prompt.properties.choice_title(),
            frame,
            focused(Control::Choice),
            checking,
        );
        choice.value = Some(AccessibleValue::Toggle(
            if prompt.properties.choice_chosen {
                Checked::True
            } else {
                Checked::False
            },
        ));
        nodes.push((KeyringNode::Choice.id(), choice));
    }
    for (i, (status, frame)) in view.status.iter().zip(&frames.status).enumerate() {
        let mut line = text(status.text(), *frame);
        line.live = match status {
            Status::Warning(_) | Status::Mismatch => Some(Live::Assertive),
            Status::CapsLock => None,
        };
        nodes.push((id(KeyringNode::Status as u32 + i as u32), line));
    }
    nodes.push((
        KeyringNode::Cancel.id(),
        control(
            Role::Button,
            view.cancel_title,
            frames.cancel,
            focused(Control::Cancel),
            checking,
        ),
    ));
    nodes.push((
        KeyringNode::Continue.id(),
        control(
            Role::Button,
            view.continue_title,
            frames.continue_,
            focused(Control::Continue),
            checking,
        ),
    ));
    let mut root = node(
        Role::AlertDialog,
        Some(&heading),
        Rect::new(0.0, 0.0, frames.width, frames.height),
    );
    root.states.modal = true;
    root.states.busy = checking;
    root.relations = vec![Relation::LabelledBy(KeyringNode::Heading.id())];
    if !frames.description.is_empty() {
        root.relations
            .push(Relation::DescribedBy(KeyringNode::Description.id()));
    }
    root.children = nodes.iter().map(|(id, _)| *id).collect();
    let focus = match view.panel.focus {
        Control::Password => KeyringNode::Password,
        Control::Confirm => KeyringNode::Confirm,
        Control::Choice => KeyringNode::Choice,
        Control::Cancel => KeyringNode::Cancel,
        Control::Continue => KeyringNode::Continue,
    };
    nodes.insert(0, (KeyringNode::Dialog.id(), root));
    AccessibilityUpdate {
        window,
        root: KeyringNode::Dialog.id(),
        focus: Some(focus.id()),
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// What a request from assistive technology on node `id` does: a click on that control, or the keyboard for a field.
pub fn requested(id: NodeId) -> Option<Control> {
    [
        (KeyringNode::Password, Control::Password),
        (KeyringNode::Confirm, Control::Confirm),
        (KeyringNode::Choice, Control::Choice),
        (KeyringNode::Cancel, Control::Cancel),
        (KeyringNode::Continue, Control::Continue),
    ]
    .into_iter()
    .find(|(node, _)| node.id() == id)
    .map(|(_, control)| control)
}
