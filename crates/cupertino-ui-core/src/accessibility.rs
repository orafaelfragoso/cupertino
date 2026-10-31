//! The accessibility tree core keeps beside the node tree. Nodes share the tree's [`NodeId`]s, so assistive
//! technology sees the same object across updates. A protected node's value never enters this tree: its value is
//! [`AccessibleValue::Protected`], which carries nothing.

use crate::NodeId;
use cupertino_geometry::Rect;
use cupertino_ui_platform::WindowId;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Window,
    Dialog,
    AlertDialog,
    Group,
    StaticText,
    Link,
    Image,
    Button,
    PopUpButton,
    Switch,
    CheckBox,
    RadioButton,
    RadioGroup,
    Slider,
    /// A number stepped up and down, with a value range.
    SpinButton,
    ProgressIndicator,
    TextInput,
    MultilineTextInput,
    PasswordInput,
    Menu,
    MenuBar,
    MenuItem,
    MenuItemCheckBox,
    MenuItemRadio,
    TabList,
    Tab,
    TabPanel,
    List,
    ListItem,
    Table,
    Row,
    Cell,
    ColumnHeader,
    Tree,
    TreeItem,
    ScrollView,
    ScrollBar,
    Separator,
    Splitter,
    Toolbar,
    Tooltip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Checked {
    False,
    True,
    Mixed,
}

/// How assistive technology announces a node's changes as they happen: politely when it is idle, or at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Live {
    Polite,
    Assertive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AccessibleValue {
    Text {
        value: String,
        /// Set for editable text.
        editing: Option<TextSemantics>,
    },
    Number {
        value: f64,
        min: f64,
        max: f64,
        step: Option<f64>,
    },
    Toggle(Checked),
    /// A secret: assistive technology learns that there is a value, never what it is or where the caret sits in it.
    Protected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AccessibleStates {
    pub disabled: bool,
    pub focusable: bool,
    pub focused: bool,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub busy: bool,
    pub read_only: bool,
    pub required: bool,
    pub invalid: bool,
    pub modal: bool,
    pub multiselectable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Click,
    Focus,
    Increment,
    Decrement,
    Expand,
    Collapse,
    ShowMenu,
    ScrollIntoView,
    SetValue,
    SetSelection,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Relation {
    LabelledBy(NodeId),
    DescribedBy(NodeId),
    Controls(NodeId),
    FlowsTo(NodeId),
}

/// The caret and selection of an editable node, in bytes of its value. A protected value has none, by type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextSemantics {
    pub caret: usize,
    pub selection: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityNode {
    pub role: Role,
    pub name: Option<String>,
    pub description: Option<String>,
    pub value: Option<AccessibleValue>,
    pub states: AccessibleStates,
    /// In window coordinates, logical points.
    pub bounds: Rect,
    pub actions: Vec<Action>,
    pub relations: Vec<Relation>,
    /// In reading order.
    pub children: Vec<NodeId>,
    /// A live region: its changes are announced as they happen.
    pub live: Option<Live>,
    /// On a list, table or tree: how many items it holds, though only those in view may be in the tree.
    pub item_count: Option<usize>,
    /// A tree item's depth, from 1 at the top level.
    pub level: Option<usize>,
    /// On an item of a list, table or tree: its place among all of the container's items, from 0.
    pub item_index: Option<usize>,
    /// On a table's cell or column header: its column, from 0, which pairs a cell with its header.
    pub column_index: Option<usize>,
}

/// What changed in one window's tree since the previous update.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityUpdate {
    pub window: WindowId,
    pub root: NodeId,
    pub focus: Option<NodeId>,
    pub nodes: Vec<(NodeId, AccessibilityNode)>,
    pub removed: Vec<NodeId>,
    /// `nodes` is the window's whole tree: the adapter replaces what it holds for the window, dropping any node not
    /// listed. Sent when a window opens and when the adapter becomes active again.
    pub full: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionData {
    Number(f64),
    Text(String),
    Selection(Range<usize>),
}

/// An action assistive technology asked a node to perform.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionRequest {
    pub target: NodeId,
    pub action: Action,
    pub data: Option<ActionData>,
}

/// Publishes the tree to assistive technology and hands back its requests. An adapter wakes the window system's
/// waker when a request arrives and when it becomes active, because a running application may be blocked waiting for
/// the window system and would otherwise not send the tree until its next event.
pub trait AccessibilityAdapter {
    /// Whether an assistive technology is listening; core builds updates only while it is.
    fn is_active(&self) -> bool;
    fn window_focus(&mut self, window: WindowId, focused: bool);
    fn update(&mut self, update: &AccessibilityUpdate);
    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>);
    /// The window is gone; the adapter drops what it holds for it.
    fn window_closed(&mut self, _window: WindowId) {}
    /// The session was locked or unlocked. While it is locked the application's windows aren't shown, so the adapter
    /// announces nothing.
    fn session_locked(&mut self, _locked: bool) {}
}
