//! The CupertinoKit runtime. An [`Application`] keeps its state in a model, describes its windows as [`Element`]
//! trees after every update, and changes the model only through messages. Core reconciles each description with the
//! retained node tree, routes events to [`Widget`]s, lays out, paints and keeps the accessibility tree.

mod accessibility;
mod clock;
mod command;
mod cx;
mod damage;
mod focus;
mod icons;
mod layout_view;
mod paint;
mod pointer;
mod route;
mod runtime;
mod scroll;
mod semantics;
mod task;
#[cfg(test)]
mod test_support;
mod tree;
mod widget;

pub use accessibility::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleStates,
    AccessibleValue, Action, ActionData, ActionRequest, Checked, Live, Relation, Role,
    TextSemantics,
};
pub use clock::MonotonicClock;
pub use command::CommandRegistry;
pub use cx::{EventCx, FocusTarget, MeasureCx, PaintCx, SemanticsCx};
pub use icons::{next_resource_id, IconImage, Icons, PlaceholderIcons};
pub use runtime::{
    RendererFactory, Runtime, RuntimeOptions, ThemeForAppearance, WindowErrorHandler,
};
pub use widget::{Change, Element, Propagation, Widget};

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{LayoutDirection, LayoutError, LayoutStats};
use cupertino_ui_platform::{
    KeyEvent, Keysym, Mods, PlatformError, PointerEvent, Timestamp, TouchEvent, WindowId,
    WindowRequest, WindowSpec,
};
use cupertino_ui_render::{DisplayList, RenderError};
use cupertino_ui_text::{ImeEvent, TextSystem};
use cupertino_ui_theme::ThemeTokens;
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::num::{NonZeroU32, NonZeroU64};
use std::pin::Pin;
use std::sync::Arc;

pub trait Application: 'static {
    type Model: 'static;
    type Message: Clone + Send + 'static;

    fn init() -> (Self::Model, Vec<Command<Self::Message>>);
    fn update(model: &mut Self::Model, message: Self::Message) -> Vec<Command<Self::Message>>;
    /// The application's windows; a window whose key disappears closes.
    fn view(model: &Self::Model) -> Vec<Window<Self::Message>>;
}

/// Identifies an element among its siblings, or a window among the application's windows, across updates.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Int(u64),
    Str(Cow<'static, str>),
}

impl From<u64> for Key {
    fn from(k: u64) -> Self {
        Key::Int(k)
    }
}

impl From<&'static str> for Key {
    fn from(k: &'static str) -> Self {
        Key::Str(Cow::Borrowed(k))
    }
}

impl From<String> for Key {
    fn from(k: String) -> Self {
        Key::Str(Cow::Owned(k))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey(pub Key);

pub struct Window<M> {
    pub key: WindowKey,
    pub spec: WindowSpec,
    pub shortcuts: CommandRegistry<M>,
    /// Sent when the user asks to close the window; without it the request ends the application.
    pub on_close: Option<M>,
    pub background: WindowBackground,
    pub content: Element<M>,
    /// Opens the window as a popup of another window instead of as a toplevel.
    pub popup: Option<Popup<M>>,
}

/// Where a popup opens and what tells the application it closed. The window's `spec.size` is the most the popup
/// may take; it opens at the size its content measures within that.
pub struct Popup<M> {
    /// A window declared before the popup.
    pub parent: WindowKey,
    pub anchor: PopupAnchor,
    /// Takes the keyboard and closes on a press outside, as a menu or a transient popover does; a tooltip does not.
    pub grab: bool,
    /// Sent when the compositor dismisses the popup; the application then stops declaring it.
    pub on_dismiss: M,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PopupAnchor {
    /// Centred below the parent's node with this key, `gap` apart, flipping above it when there is no room. The popup
    /// opens once that node has been laid out.
    Below { key: Key, gap: f64 },
    /// At a point of the parent, opening down and to the trailing side, flipping when there is no room: a context
    /// menu.
    At(Point),
}

/// What a window paints under its content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowBackground {
    /// The theme's window background, full size and opaque: an ordinary window.
    #[default]
    Theme,
    /// Nothing: the content paints its own surface, shaped or translucent, as a menu or popover panel does.
    None,
}

/// A node of the retained tree: a 32-bit slot index and the slot's 32-bit generation, so a stale id never matches
/// the node that reused its slot.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(NonZeroU64);

impl NodeId {
    pub fn new(index: u32, generation: NonZeroU32) -> Self {
        let raw = u64::from(generation.get()) << 32 | u64::from(index);
        Self(NonZeroU64::new(raw).expect("a non-zero generation makes a non-zero id"))
    }

    pub fn index(self) -> u32 {
        self.0.get() as u32
    }

    pub fn generation(self) -> NonZeroU32 {
        NonZeroU32::new((self.0.get() >> 32) as u32).expect("a node id's generation is never zero")
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({}v{})", self.index(), self.generation())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Capture,
    Target,
    Bubble,
}

/// What a widget receives.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Pointer(PointerEvent, Timestamp),
    Touch(TouchEvent, Timestamp),
    Key(KeyEvent, Timestamp),
    Ime(ImeEvent),
    /// `visible` when the focus came from the keyboard or assistive technology, so the ring shows.
    Focus {
        gained: bool,
        visible: bool,
    },
    Action(ActionRequest),
    /// The frame a node asked for with [`EventCx::request_animation_frame`] is being prepared.
    AnimationFrame(Timestamp),
    /// The time a node asked for with [`EventCx::wake_at`] has come; it carries the clock's time now.
    Wake(Timestamp),
    /// A scroll container's size or its content's size changed since it last heard, or it was laid out for the
    /// first time. `content` counts from the start edge, as the offset does.
    ScrollGeometry {
        viewport: Size,
        content: Size,
    },
    /// Keyboard or programmatic focus moved to a node this scroll container hides; scrolling to this offset shows it.
    Reveal(Point),
    /// The window of the focused node became active (`true`) or inactive; only that node hears it.
    WindowActive(bool),
    /// A press, drag or composition in progress ends without effect: focus, pointer capture or the surface was lost.
    Cancel,
}

/// The state every interactive control resolves its look from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ControlState {
    pub enabled: bool,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub focus_visible: bool,
    pub selected: bool,
    pub window_active: bool,
    pub busy: bool,
    pub read_only: bool,
    /// Holds a secret: never exposed as text to accessibility, the inspector or the input method.
    pub protected: bool,
}

/// What activated a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActivationSource {
    Pointer,
    Touch,
    Keyboard,
    Accessibility,
}

pub type Task<M> = Pin<Box<dyn Future<Output = M> + Send>>;

/// Work an update asks the runtime to do.
pub enum Command<M> {
    /// Runs a future on the runtime's executor and sends its output as a message.
    Perform(Task<M>),
    Focus(Key),
    CloseWindow(WindowKey),
    Window(WindowKey, WindowRequest),
    SetClipboard(String),
    Exit,
}

/// What a subtree is laid out and painted with.
#[derive(Clone, Debug)]
pub struct Environment {
    pub theme: Arc<ThemeTokens>,
    /// Shapes and measures every label and field; shared by all the app's windows.
    pub text: Arc<TextSystem>,
    pub direction: LayoutDirection,
    /// Physical pixels per logical point; layout never reads it.
    pub scale: f64,
    /// The system's symbols; shared by all the app's windows.
    pub icons: Arc<dyn Icons>,
}

/// The text system compares by identity: two environments with the same fonts shape alike only if they share one.
impl PartialEq for Environment {
    fn eq(&self, other: &Self) -> bool {
        self.theme == other.theme
            && Arc::ptr_eq(&self.text, &other.text)
            && Arc::ptr_eq(&self.icons, &other.icons)
            && self.direction == other.direction
            && self.scale == other.scale
    }
}

/// The runtime's time source; tests inject one.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub mods: Mods,
    pub keysym: Keysym,
}

/// Work a runtime step left for later, so a test can tell a settled runtime from one that is waiting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pending {
    pub messages: usize,
    pub tasks: usize,
    pub frames: usize,
    pub animating: bool,
    /// The earliest time a node asked to be woken at.
    pub next_wake: Option<Timestamp>,
}

/// What a bounded runtime step ended with. Steps never advance time on their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepStatus {
    /// Nothing is left to do at the current time.
    Idle,
    /// Work waits for time, a task, a frame or a buffer release.
    Waiting(Pending),
    /// The turn budget ran out with work still ready.
    BudgetExhausted(Pending),
}

/// A read-only view of one node after a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSnapshot {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub widget: &'static str,
    pub key: Option<Key>,
    /// In window coordinates.
    pub rect: Rect,
    pub clip: Option<Rect>,
}

/// A read-only view of one window after a frame: the input of every inspector and test snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameSnapshot {
    pub window: WindowId,
    pub nodes: Vec<NodeSnapshot>,
    pub focus: Option<NodeId>,
    pub pointer_capture: Option<NodeId>,
    pub text_input: Option<NodeId>,
    pub accessibility: Vec<(NodeId, AccessibilityNode)>,
    pub display_list: DisplayList,
    pub pending: Pending,
    /// What the window's last layout did.
    pub layout: LayoutStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiError {
    Platform(PlatformError),
    Layout(LayoutError),
    Render(RenderError),
    /// The tree reached its node ceiling.
    NodeLimit(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_id_keeps_its_index_and_generation() {
        for (index, generation) in [(0, 1), (7, 3), (u32::MAX, u32::MAX)] {
            let id = NodeId::new(index, NonZeroU32::new(generation).unwrap());
            assert_eq!(id.index(), index);
            assert_eq!(id.generation().get(), generation);
        }
        let one = NonZeroU32::new(1).unwrap();
        let two = NonZeroU32::new(2).unwrap();
        assert_ne!(NodeId::new(5, one), NodeId::new(5, two));
    }
}
