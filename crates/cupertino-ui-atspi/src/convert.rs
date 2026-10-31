//! Between core's accessibility tree and AccessKit's: nodes out, action requests back.

use std::num::NonZeroU32;

use accesskit::{Invalid, Toggled};
use cupertino_ui_core::{
    AccessibilityNode, AccessibleValue, Action, ActionData, ActionRequest, Checked, Live, NodeId,
    Relation, Role,
};

pub(crate) fn node_id(id: NodeId) -> accesskit::NodeId {
    accesskit::NodeId(u64::from(id.generation().get()) << 32 | u64::from(id.index()))
}

/// The core id of an AccessKit id this crate gave out; `None` for any other.
fn core_id(id: accesskit::NodeId) -> Option<NodeId> {
    let generation = NonZeroU32::new((id.0 >> 32) as u32)?;
    Some(NodeId::new(id.0 as u32, generation))
}

/// Whether a node's value is a secret: a password field's, whatever core says, or one core marks protected.
pub(crate) fn protected(node: &AccessibilityNode) -> bool {
    node.role == Role::PasswordInput || node.value == Some(AccessibleValue::Protected)
}

/// The AccessKit node for `node`. A protected node gets no value, offers no way to set one or its selection, and is
/// never a live region.
pub(crate) fn node(node: &AccessibilityNode) -> accesskit::Node {
    let secret = protected(node);
    let mut out = accesskit::Node::new(role(node.role));
    match &node.name {
        // AccessKit names a label from its value
        Some(name) if node.role == Role::StaticText => out.set_value(name.as_str()),
        Some(name) => out.set_label(name.as_str()),
        None => {}
    }
    if let Some(description) = &node.description {
        out.set_description(description.as_str());
    }
    match &node.value {
        _ if secret => {}
        Some(AccessibleValue::Text { value, .. }) => out.set_value(value.as_str()),
        Some(AccessibleValue::Number {
            value,
            min,
            max,
            step,
        }) => {
            out.set_numeric_value(*value);
            out.set_min_numeric_value(*min);
            out.set_max_numeric_value(*max);
            if let Some(step) = step {
                out.set_numeric_value_step(*step);
            }
        }
        Some(AccessibleValue::Toggle(checked)) => out.set_toggled(match checked {
            Checked::False => Toggled::False,
            Checked::True => Toggled::True,
            Checked::Mixed => Toggled::Mixed,
        }),
        Some(AccessibleValue::Protected) | None => {}
    }
    let s = node.states;
    if s.disabled {
        out.set_disabled();
    }
    if s.focusable {
        out.add_action(accesskit::Action::Focus);
    }
    if s.selected {
        out.set_selected(true);
    }
    if let Some(expanded) = s.expanded {
        out.set_expanded(expanded);
    }
    if s.busy {
        out.set_busy();
    }
    if s.read_only {
        out.set_read_only();
    }
    if s.required {
        out.set_required();
    }
    if s.invalid {
        out.set_invalid(Invalid::True);
    }
    if s.modal {
        out.set_modal();
    }
    if s.multiselectable {
        out.set_multiselectable();
    }
    if let Some(count) = node.item_count {
        out.set_size_of_set(count);
        if node.role == Role::Table {
            out.set_row_count(count);
        }
    }
    if let Some(level) = node.level {
        out.set_level(level);
    }
    if let Some(index) = node.item_index {
        out.set_position_in_set(index);
        if node.role == Role::Row {
            out.set_row_index(index);
        }
    }
    if let Some(column) = node.column_index {
        out.set_column_index(column);
    }
    let b = node.bounds;
    out.set_bounds(accesskit::Rect::new(
        b.min_x(),
        b.min_y(),
        b.max_x(),
        b.max_y(),
    ));
    for action in &node.actions {
        if let Some(action) = action_out(*action).filter(|_| !(secret && edits_value(*action))) {
            out.add_action(action);
        }
    }
    for relation in &node.relations {
        match relation {
            Relation::LabelledBy(id) => out.push_labelled_by(node_id(*id)),
            Relation::DescribedBy(id) => out.push_described_by(node_id(*id)),
            Relation::Controls(id) => out.push_controlled(node_id(*id)),
            Relation::FlowsTo(id) => out.push_flow_to(node_id(*id)),
        }
    }
    out.set_children(
        node.children
            .iter()
            .copied()
            .map(node_id)
            .collect::<Vec<_>>(),
    );
    match node.live {
        _ if secret => {}
        Some(Live::Polite) => out.set_live(accesskit::Live::Polite),
        Some(Live::Assertive) => out.set_live(accesskit::Live::Assertive),
        None => {}
    }
    out
}

/// The actions that change a value or the selection in it, which a secret never takes from assistive technology.
fn edits_value(action: Action) -> bool {
    matches!(action, Action::SetValue | Action::SetSelection)
}

fn role(role: Role) -> accesskit::Role {
    use accesskit::Role as A;
    match role {
        Role::Window => A::Window,
        Role::Dialog => A::Dialog,
        Role::AlertDialog => A::AlertDialog,
        Role::Group => A::Group,
        Role::StaticText => A::Label,
        Role::Link => A::Link,
        Role::Image => A::Image,
        Role::Button => A::Button,
        Role::PopUpButton => A::ComboBox,
        Role::Switch => A::Switch,
        Role::CheckBox => A::CheckBox,
        Role::RadioButton => A::RadioButton,
        Role::RadioGroup => A::RadioGroup,
        Role::Slider => A::Slider,
        Role::SpinButton => A::SpinButton,
        Role::ProgressIndicator => A::ProgressIndicator,
        Role::TextInput => A::TextInput,
        Role::MultilineTextInput => A::MultilineTextInput,
        Role::PasswordInput => A::PasswordInput,
        Role::Menu => A::Menu,
        Role::MenuBar => A::MenuBar,
        Role::MenuItem => A::MenuItem,
        Role::MenuItemCheckBox => A::MenuItemCheckBox,
        Role::MenuItemRadio => A::MenuItemRadio,
        Role::TabList => A::TabList,
        Role::Tab => A::Tab,
        Role::TabPanel => A::TabPanel,
        Role::List => A::List,
        Role::ListItem => A::ListItem,
        Role::Table => A::Table,
        Role::Row => A::Row,
        Role::Cell => A::Cell,
        Role::ColumnHeader => A::ColumnHeader,
        Role::Tree => A::Tree,
        Role::TreeItem => A::TreeItem,
        Role::ScrollView => A::ScrollView,
        Role::ScrollBar => A::ScrollBar,
        // AccessKit's splitter is ARIA's separator
        Role::Separator | Role::Splitter => A::Splitter,
        Role::Toolbar => A::Toolbar,
        Role::Tooltip => A::Tooltip,
    }
}

/// `None` for a selection: AccessKit expresses one in text runs, which this crate doesn't build.
fn action_out(action: Action) -> Option<accesskit::Action> {
    use accesskit::Action as A;
    Some(match action {
        Action::Click => A::Click,
        Action::Focus => A::Focus,
        Action::Increment => A::Increment,
        Action::Decrement => A::Decrement,
        Action::Expand => A::Expand,
        Action::Collapse => A::Collapse,
        Action::ShowMenu => A::ShowContextMenu,
        Action::ScrollIntoView => A::ScrollIntoView,
        Action::SetValue => A::SetValue,
        Action::SetSelection => return None,
    })
}

/// Core's request for an AccessKit one, when it maps to an action core knows with the data that action takes.
pub(crate) fn request(request: &accesskit::ActionRequest) -> Option<ActionRequest> {
    use accesskit::Action as A;
    let target = core_id(request.target_node)?;
    let (action, data) = match (request.action, &request.data) {
        (A::Click, _) => (Action::Click, None),
        (A::Focus, _) => (Action::Focus, None),
        (A::Increment, _) => (Action::Increment, None),
        (A::Decrement, _) => (Action::Decrement, None),
        (A::Expand, _) => (Action::Expand, None),
        (A::Collapse, _) => (Action::Collapse, None),
        (A::ShowContextMenu, _) => (Action::ShowMenu, None),
        (A::ScrollIntoView, _) => (Action::ScrollIntoView, None),
        (A::SetValue, Some(accesskit::ActionData::Value(text))) => {
            (Action::SetValue, Some(ActionData::Text(text.to_string())))
        }
        (A::SetValue, Some(accesskit::ActionData::NumericValue(n))) => {
            (Action::SetValue, Some(ActionData::Number(*n)))
        }
        _ => return None,
    };
    Some(ActionRequest {
        target,
        action,
        data,
    })
}

#[cfg(test)]
mod tests;
