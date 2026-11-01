//! The interface between CupertinoKit and a window system. The runtime drives one [`WindowSystem`]: the Wayland one on
//! the desktop, a headless one in tests. Every capability beyond a plain window is a flag in [`Capabilities`] that
//! the runtime checks, so a missing protocol degrades a feature instead of failing.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_menu_model::{ActionName, MenuBar};
use cupertino_ui_render::{CpuFrame, RenderFrameId};
use cupertino_ui_text::{ImeEvent, TextInputState};
use std::collections::BTreeSet;
use std::task::Waker;
use std::time::Duration;

pub use cupertino_modifiers::Mods;

/// A toplevel window or a popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u32);

/// Monotonic time in nanoseconds, from the window system's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    pub fractional_scale: bool,
    pub presentation_time: bool,
    pub text_input: bool,
    pub touch: bool,
    pub cursor_shape: bool,
    pub primary_selection: bool,
    pub drag_and_drop: bool,
    pub server_decorations: bool,
    pub activation: bool,
    /// The desktop's optional public effects protocol (materials, blur); without it materials use their fallback.
    pub app_effects: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DecorationMode {
    /// The compositor draws the title bar when it offers to, else the toolkit does.
    #[default]
    Preferred,
    Client,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub title: String,
    pub app_id: String,
    pub size: Size,
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,
    pub decorations: DecorationMode,
    /// The handle another application exported one of its windows under, which this window opens as a child of (a
    /// dialog the system shows for that window). Read when the window opens; an unknown handle, or a platform without
    /// the protocol, leaves the window without a parent.
    pub foreign_parent: Option<String>,
}

/// An edge or corner of a popup's anchor rectangle, or the direction the popup extends from it, as
/// `xdg_positioner` takes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PopupEdge {
    #[default]
    None,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// How the compositor may move a popup that would not fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ConstraintAdjust {
    pub slide: bool,
    pub flip: bool,
    pub resize: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupSpec {
    /// In the parent's coordinates.
    pub anchor_rect: Rect,
    pub anchor: PopupEdge,
    pub gravity: PopupEdge,
    pub offset: Point,
    pub size: Size,
    pub constraint: ConstraintAdjust,
    /// Lets the compositor move the popup again when its parent moves or resizes.
    pub reactive: bool,
    /// Takes the keyboard and dismisses on a click outside (menus); a tooltip does not.
    pub grab: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowRequest {
    SetTitle(String),
    SetMinSize(Option<Size>),
    SetMaxSize(Option<Size>),
    /// Interactive move, started by the pointer press being handled.
    Move,
    Resize(ResizeEdge),
    ShowWindowMenu(Point),
    Maximize(bool),
    Fullscreen(bool),
    Minimize,
    /// Raise and focus, with an activation token when the request came from another client.
    Activate(Option<String>),
    /// Moves an open popup.
    Reposition(PopupSpec),
    /// Puts a compositor material under the whole window, or takes it away with `None`. Needs
    /// [`Capabilities::app_effects`].
    Backdrop(Option<Backdrop>),
    /// Publishes a toplevel's menus to the desktop's menu bar, or withdraws them with `None`. A platform without a
    /// desktop menu bar takes the request and shows nothing.
    Menus(Option<MenuDescription>),
}

/// A window's menus as the desktop's menu bar shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuDescription {
    pub bar: MenuBar,
    /// Items shown but not available now.
    pub disabled: BTreeSet<ActionName>,
}

/// A material of the theme data (`materials.json`) the compositor draws under a window, the window's size with its
/// corners rounded by `radius` points, at `alpha` from 0 (hidden) to 1 (opaque).
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub material: String,
    pub radius: f64,
    pub alpha: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    #[default]
    Default,
    Pointer,
    Text,
    Grab,
    Grabbing,
    Move,
    NotAllowed,
    Wait,
    Progress,
    Crosshair,
    ResizeEw,
    ResizeNs,
    ResizeNwse,
    ResizeNesw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardData {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Names one clipboard read; its data arrives as [`PlatformEvent::Clipboard`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClipboardToken(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ClipboardKind {
    #[default]
    Clipboard,
    /// The selection pasted with the middle button, when the compositor has one.
    Primary,
}

/// Names one activation-token request; the token arrives as [`PlatformEvent::ActivationToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivationRequest(pub u64);

/// Whether [`WindowSystem::present`] submitted the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PresentOutcome {
    Presented,
    /// No buffer is free or the compositor has not asked for a frame; the damage carries over to the next present.
    NotReady,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TiledEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WindowStates {
    pub activated: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub tiled: TiledEdges,
    pub resizing: bool,
    pub suspended: bool,
}

/// Names one configure; the window acknowledges it with the first frame drawn for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigureId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configure {
    pub id: ConfigureId,
    /// `None` lets the window pick its size.
    pub size: Option<Size>,
    pub states: WindowStates,
    pub bounds: Option<Size>,
    /// Whether the compositor draws the title bar.
    pub server_decorations: bool,
    pub wm_capabilities: WmCapabilities,
}

/// The window-management requests the compositor supports for this window; a title bar shows only those.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WmCapabilities {
    pub window_menu: bool,
    pub maximize: bool,
    pub fullscreen: bool,
    pub minimize: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollSource {
    Wheel,
    Finger,
    Continuous,
}

/// Pointer input in the window's logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Enter {
        position: Point,
    },
    Leave,
    Motion {
        position: Point,
    },
    Button {
        position: Point,
        button: MouseButton,
        pressed: bool,
        /// Passed back with move, resize and window-menu requests.
        serial: u32,
        /// The keyboard modifiers held at the press or release.
        mods: Mods,
    },
    Scroll {
        position: Point,
        delta: Point,
        /// Wheel detents in 1/120 steps, when the device reports them.
        value120: Option<(i32, i32)>,
        source: ScrollSource,
        /// The fingers left the surface: kinetic scrolling may start.
        stop: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TouchEvent {
    Down { id: i32, position: Point },
    Motion { id: i32, position: Point },
    Up { id: i32 },
    Cancel,
}

/// An xkb keysym.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keysym(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The evdev key code.
    pub code: u32,
    pub keysym: Keysym,
    /// What the key types under the layout and modifiers, if it types anything.
    pub text: Option<char>,
    pub pressed: bool,
    pub repeat: bool,
    pub mods: Mods,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowEvent {
    Configure(Configure),
    CloseRequested,
    /// Physical pixels per logical point.
    Scale(f64),
    /// The compositor is ready for the next frame, which it will show around this time.
    Frame(Timestamp),
    Pointer(PointerEvent, Timestamp),
    Touch(TouchEvent, Timestamp),
    Key(KeyEvent, Timestamp),
    /// The window gained or lost the keyboard focus.
    Focus(bool),
    Ime(ImeEvent),
    /// The compositor closed a popup.
    PopupDone,
    /// The compositor showed a presented frame at `time`.
    Presented {
        frame: RenderFrameId,
        time: Timestamp,
    },
    /// The compositor never showed a presented frame, such as one a later frame replaced first.
    Discarded {
        frame: RenderFrameId,
    },
    /// The user chose an item of the window's menus in the desktop's menu bar.
    MenuActivated(ActionName),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlatformEvent {
    Window {
        window: WindowId,
        event: WindowEvent,
    },
    Clipboard {
        token: ClipboardToken,
        data: Option<ClipboardData>,
    },
    /// What the clipboard now offers, by MIME type; `None` when it was cleared.
    ClipboardOffered {
        kind: ClipboardKind,
        mimes: Option<Vec<String>>,
    },
    /// A token to hand to another client so it may take the focus; `None` when the window is gone or the compositor
    /// gives none.
    ActivationToken {
        request: ActivationRequest,
        token: Option<String>,
    },
    /// The waker was woken.
    Wake,
    /// The session's appearance or display options, once when the window system learns them and on every change.
    Appearance(SystemAppearance),
    /// Whether the session is locked, once when the window system learns it and on every change; a window system that
    /// can't tell sends false once.
    SessionLock(bool),
}

/// The session's appearance and the display options that change how windows draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemAppearance {
    pub dark: bool,
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
    pub reduce_motion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// The connection to the window system is gone.
    Disconnected(String),
    UnknownWindow(WindowId),
    /// The window system cannot do this; see [`Capabilities`].
    Unsupported,
    Protocol(String),
}

pub trait WindowSystem {
    fn capabilities(&self) -> Capabilities;
    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError>;
    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError>;
    fn destroy(&mut self, window: WindowId);
    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError>;
    /// Enables the input method for `window` with this state, or disables it with `None`.
    fn set_text_input(&mut self, window: WindowId, state: Option<&TextInputState>);
    /// `None` hides the pointer over the window.
    fn set_cursor(&mut self, window: WindowId, shape: Option<CursorShape>);
    fn set_clipboard(&mut self, kind: ClipboardKind, data: ClipboardData);
    fn read_clipboard(&mut self, kind: ClipboardKind, mime: &str) -> ClipboardToken;
    /// Asks for a token for the application whose desktop entry is `app_id`, when known, so the desktop can tell
    /// what is starting. The answer always arrives, as [`PlatformEvent::ActivationToken`].
    fn request_activation_token(
        &mut self,
        window: WindowId,
        app_id: Option<&str>,
    ) -> ActivationRequest;
    /// Asks for a [`WindowEvent::Frame`] when the compositor wants the next frame.
    fn request_frame(&mut self, window: WindowId);
    /// Copies the frame's damage, plus whatever the chosen buffer missed since it was last used, into a buffer and
    /// commits it. After a failure the next successful present also covers this frame's damage.
    /// `configure`, when set, is acknowledged with this frame.
    fn present(
        &mut self,
        window: WindowId,
        frame: &CpuFrame,
        configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError>;
    /// Waits up to `timeout` (`None`: until something happens) and appends what happened to `events`.
    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError>;
    /// Wakes a waiting [`dispatch`](WindowSystem::dispatch) from any thread.
    fn waker(&self) -> Waker;
}
