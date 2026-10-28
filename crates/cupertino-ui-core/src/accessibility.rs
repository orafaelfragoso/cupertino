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

#[derive(Clone, Debug, PartialEq)]
pub enum AccessibleValue {
    Text(String),
    Number {
        value: f64,
        min: f64,
        max: f64,
        step: Option<f64>,
    },
    Toggle(Checked),
    /// A secret: assistive technology learns that there is a value, never what it is.
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

/// The caret and selection of an editable node, in bytes of its value. Protected nodes have none.
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
    pub children: Vec<NodeId>,
    pub text: Option<TextSemantics>,
}

/// What changed in one window's tree since the previous update.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityUpdate {
    pub window: WindowId,
    pub root: NodeId,
    pub focus: Option<NodeId>,
    pub nodes: Vec<(NodeId, AccessibilityNode)>,
    pub removed: Vec<NodeId>,
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

/// Publishes the tree to assistive technology and hands back its requests.
pub trait AccessibilityAdapter {
    fn update(&mut self, update: &AccessibilityUpdate);
    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>);
}
