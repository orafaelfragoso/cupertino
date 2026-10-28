//! The interface between CupertinoKit and a window system. The runtime drives one [`WindowSystem`]: the Wayland one on
//! the desktop, a headless one in tests. Every capability beyond a plain window is a flag in [`Capabilities`] that
//! the runtime checks, so a missing protocol degrades a feature instead of failing.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_render::CpuFrame;
use cupertino_ui_text::{ImeEvent, TextInputState};
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
    pub cursor_shape: bool,
    pub primary_selection: bool,
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
}

/// Where a popup goes relative to its anchor rectangle, as `xdg_positioner` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gravity {
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
    pub anchor: Rect,
    pub gravity: Gravity,
    pub size: Size,
    pub constraint: ConstraintAdjust,
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
pub struct WindowStates {
    pub activated: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub tiled: bool,
    pub resizing: bool,
    pub suspended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configure {
    /// `None` lets the window pick its size.
    pub size: Option<Size>,
    pub states: WindowStates,
    pub bounds: Option<Size>,
    /// Whether the compositor draws the title bar.
    pub server_decorations: bool,
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
    },
    Scroll {
        position: Point,
        delta: Point,
        /// Wheel detents in 1/120 steps, when the device reports them.
        value120: Option<(i32, i32)>,
        source: ScrollSource,
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
    /// The waker was woken.
    Wake,
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
    fn set_cursor(&mut self, window: WindowId, shape: CursorShape);
    fn set_clipboard(&mut self, data: ClipboardData);
    fn read_clipboard(&mut self, mime: &str) -> ClipboardToken;
    /// Asks for a [`WindowEvent::Frame`] when the compositor wants the next frame.
    fn request_frame(&mut self, window: WindowId);
    /// Copies the frame's damage, plus whatever the chosen buffer missed since it was last used, into a buffer and
    /// commits it. After a failure the next successful present also covers this frame's damage.
    fn present(&mut self, window: WindowId, frame: &CpuFrame) -> Result<(), PlatformError>;
    /// Waits up to `timeout` (`None`: until something happens) and appends what happened to `events`.
    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError>;
    /// Wakes a waiting [`dispatch`](WindowSystem::dispatch) from any thread.
    fn waker(&self) -> Waker;
}
