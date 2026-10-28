use crate::accessibility::{AccessibleStates, AccessibleValue, Action, Relation, Role};
use crate::route::Placement;
use crate::widget::Change;
use crate::{Environment, NodeId, Phase};
use cupertino_geometry::{Point, Size};
use cupertino_ui_platform::{CursorShape, Timestamp};
use cupertino_ui_render::{Effect, Group, PaintOp};
use cupertino_ui_text::TextInputState;
use std::collections::BTreeSet;

pub struct MeasureCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) env: &'a Environment,
}

impl MeasureCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    pub fn env(&self) -> &Environment {
        self.env
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureRequest {
    Capture(NodeId),
    Release,
}

/// What widgets asked for while one event was routed.
pub(crate) struct Effects<M> {
    pub(crate) messages: Vec<M>,
    pub(crate) focus: Option<NodeId>,
    pub(crate) capture: Option<CaptureRequest>,
    pub(crate) changes: Vec<(NodeId, Change)>,
    pub(crate) cursor: Option<Option<CursorShape>>,
    pub(crate) animate: BTreeSet<NodeId>,
    pub(crate) text_input: Option<(NodeId, Option<TextInputState>)>,
}

impl<M> Default for Effects<M> {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            focus: None,
            capture: None,
            changes: Vec::new(),
            cursor: None,
            animate: BTreeSet::new(),
            text_input: None,
        }
    }
}

impl<M> Effects<M> {
    /// Takes on what a later route asked for; the later request wins where only one can.
    pub(crate) fn absorb(&mut self, other: Self) {
        self.messages.extend(other.messages);
        self.focus = other.focus.or(self.focus);
        self.capture = other.capture.or(self.capture.take());
        self.changes.extend(other.changes);
        self.cursor = other.cursor.or(self.cursor);
        self.animate.extend(other.animate);
        if other.text_input.is_some() {
            self.text_input = other.text_input;
        }
    }
}

/// The parts of the runtime's state an event handler reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Status {
    pub(crate) focus: Option<NodeId>,
    pub(crate) focus_visible: bool,
    pub(crate) capture: Option<NodeId>,
    pub(crate) window_active: bool,
    pub(crate) now: Timestamp,
}

pub struct EventCx<'a, M> {
    pub(crate) node: NodeId,
    pub(crate) target: NodeId,
    pub(crate) phase: Phase,
    pub(crate) placement: Option<Placement>,
    pub(crate) env: &'a Environment,
    pub(crate) status: Status,
    pub(crate) effects: &'a mut Effects<M>,
}

impl<M> EventCx<'_, M> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The node the event was aimed at; it differs from [`node`](Self::node) during capture and bubble.
    pub fn target(&self) -> NodeId {
        self.target
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn env(&self) -> &Environment {
        self.env
    }

    /// Whether a pointer or touch position, in window coordinates as events carry them, lands on this node and inside
    /// every clip above it; false before the node's first layout.
    pub fn hit(&self, position: Point) -> bool {
        self.placement.is_some_and(|p| p.hit(position))
    }

    pub fn now(&self) -> Timestamp {
        self.status.now
    }

    pub fn is_focused(&self) -> bool {
        self.status.focus == Some(self.node)
    }

    pub fn focus_visible(&self) -> bool {
        self.is_focused() && self.status.focus_visible
    }

    pub fn has_pointer_capture(&self) -> bool {
        self.status.capture == Some(self.node)
    }

    pub fn window_active(&self) -> bool {
        self.status.window_active
    }

    /// Sends `message` to the application's `update`.
    pub fn emit(&mut self, message: M) {
        self.effects.messages.push(message);
    }

    pub fn request_focus(&mut self) {
        self.effects.focus = Some(self.node);
    }

    /// Sends every pointer event to this node until it releases the pointer or the capture is revoked.
    pub fn capture_pointer(&mut self) {
        self.effects.capture = Some(CaptureRequest::Capture(self.node));
    }

    pub fn release_pointer(&mut self) {
        if self.has_pointer_capture() {
            self.effects.capture = Some(CaptureRequest::Release);
        }
    }

    pub fn request_paint(&mut self) {
        self.effects.changes.push((self.node, Change::Paint));
    }

    pub fn request_layout(&mut self) {
        self.effects.changes.push((self.node, Change::Layout));
    }

    /// `None` hides the pointer.
    pub fn set_cursor(&mut self, cursor: Option<CursorShape>) {
        self.effects.cursor = Some(cursor);
    }

    /// Turns the input method on for this node with `state`, or off with `None`. The runtime turns it off when the
    /// node loses the focus.
    pub fn set_text_input(&mut self, state: Option<TextInputState>) {
        self.effects.text_input = Some((self.node, state));
    }

    /// Delivers [`Event::AnimationFrame`](crate::Event::AnimationFrame) to this node on the next frame.
    pub fn request_animation_frame(&mut self) {
        self.effects.animate.insert(self.node);
    }
}

pub struct PaintCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) size: Size,
    pub(crate) env: &'a Environment,
    pub(crate) focused: bool,
    pub(crate) focus_visible: bool,
    pub(crate) window_active: bool,
    pub(crate) now: Timestamp,
    pub(crate) ops: &'a mut Vec<PaintOp>,
    pub(crate) group: &'a mut Option<Group>,
}

impl PaintCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The node's size; painting is in its local coordinates, with the origin at its top-left corner.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn env(&self) -> &Environment {
        self.env
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    pub fn window_active(&self) -> bool {
        self.window_active
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    pub fn push(&mut self, op: PaintOp) {
        self.ops.push(op);
    }

    /// Paints this node and its whole subtree as one layer, with `opacity` applied once and `effects` (a fade, or a
    /// shadow or blur of the subtree) on the result.
    pub fn set_group(&mut self, opacity: f32, effects: Vec<Effect>) {
        *self.group = Some(Group { opacity, effects });
    }
}

/// What a node declared about itself for accessibility and keyboard focus.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Semantics {
    pub(crate) role: Option<Role>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) value: Option<AccessibleValue>,
    pub(crate) states: AccessibleStates,
    pub(crate) actions: Vec<Action>,
    pub(crate) relations: Vec<Relation>,
    pub(crate) focusable: bool,
    pub(crate) focus_scope: bool,
    pub(crate) protected: bool,
}

/// A node without a role is left out of the accessibility tree and its children take its place.
pub struct SemanticsCx<'a> {
    pub(crate) node: NodeId,
    pub(crate) semantics: &'a mut Semantics,
}

impl SemanticsCx<'_> {
    pub fn node(&self) -> NodeId {
        self.node
    }

    pub fn role(&mut self, role: Role) {
        self.semantics.role = Some(role);
    }

    pub fn name(&mut self, name: impl Into<String>) {
        self.semantics.name = Some(name.into());
    }

    pub fn description(&mut self, description: impl Into<String>) {
        self.semantics.description = Some(description.into());
    }

    pub fn value(&mut self, value: AccessibleValue) {
        self.semantics.value = Some(value);
    }

    pub fn states(&mut self, states: AccessibleStates) {
        self.semantics.states = states;
    }

    pub fn action(&mut self, action: Action) {
        self.semantics.actions.push(action);
    }

    pub fn relation(&mut self, relation: Relation) {
        self.semantics.relations.push(relation);
    }

    /// Makes the node a Tab stop.
    pub fn focusable(&mut self) {
        self.semantics.focusable = true;
    }

    /// Keeps Tab inside this subtree while it holds the focus, and gives the focus back when it unmounts (dialogs,
    /// popovers, menus).
    pub fn focus_scope(&mut self) {
        self.semantics.focus_scope = true;
    }

    /// Marks the node's value a secret: whatever value it sets reaches accessibility as
    /// [`AccessibleValue::Protected`].
    pub fn protected(&mut self) {
        self.semantics.protected = true;
    }
}
