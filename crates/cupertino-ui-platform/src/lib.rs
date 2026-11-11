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
pub use cupertino_ui_render::Color;

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
    /// A window may show a raised layer above its content ([`WindowSystem::present_layer`]), with the materials of its
    /// raised regions ([`MaterialRegion::raised`]) between the two, reading what of the content lies below them.
    pub raised_layer: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DecorationMode {
    /// The compositor draws the title bar when it offers to, else the toolkit does: a titled window, with a title bar
    /// above the content and the title centred in it.
    #[default]
    Preferred,
    Client,
    None,
    /// A unified window: the content fills the window and its toolbar row is the title bar; the compositor draws the
    /// window control buttons at the row's leading end. The window keeps that end free of controls and moves itself
    /// from the row (a [`WindowRequest::Move`] from its drag region). Without the desktop's effects it is
    /// [`DecorationMode::Preferred`].
    Unified,
    /// [`DecorationMode::Unified`] with the compact toolbar row.
    UnifiedCompact,
    /// A full-height sidebar: the content fills the window, the sidebar panel runs from its top edge to its bottom
    /// edge and the compositor draws the window control buttons in the top of that panel, the toolbar floating over
    /// the content beside it. The window keeps the top of the sidebar free of controls and moves itself from there
    /// and from its toolbar. Without the desktop's effects it is [`DecorationMode::Preferred`].
    Sidebar,
}

impl DecorationMode {
    /// The content fills the window and draws the row the compositor's window control buttons lie in.
    pub fn content_fills_window(self) -> bool {
        matches!(
            self,
            DecorationMode::Unified | DecorationMode::UnifiedCompact | DecorationMode::Sidebar
        )
    }
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
    /// The part of the window that takes pointer input, in points; elsewhere input reaches what is below. `None` is the
    /// whole window, an empty list none of it.
    pub input_region: Option<Vec<Rect>>,
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
    /// Puts compositor materials under parts of the window besides its own [`Backdrop`], or takes them all away with
    /// an empty list. Needs [`Capabilities::app_effects`].
    Materials(Vec<MaterialRegion>),
    /// Publishes a toplevel's menus to the desktop's menu bar, or withdraws them with `None`. A platform without a
    /// desktop menu bar takes the request and shows nothing.
    Menus(Option<MenuDescription>),
    /// Sets [`WindowSpec::input_region`]; it applies with the next present.
    SetInputRegion(Option<Vec<Rect>>),
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

/// A material of the theme data (`materials.json`) the compositor draws under one part of a window: `rect` in the
/// window's coordinates with its corners rounded by `radius` points, at `alpha` from 0 (hidden) to 1 (opaque).
/// With `glass`, the compositor draws glass there instead, and `material` names what a compositor without glass draws.
/// A `raised` region lies above the window's content, under its raised layer ([`WindowSystem::present_layer`]), and
/// reads the content below it; the others lie under all of the window's content.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialRegion {
    pub material: String,
    pub rect: Rect,
    pub radius: f64,
    pub alpha: f64,
    pub glass: Option<GlassRegion>,
    pub raised: bool,
}

/// The raised layer of a window ([`WindowSystem::present_layer`]): a frame drawn over the window's content from its top
/// edge, as wide as the window and `height` whole points tall, at the window's scale.
#[derive(Clone, Copy, Debug)]
pub struct RaisedLayer<'a> {
    pub frame: &'a CpuFrame,
    pub height: u32,
}

/// The look of a glass region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// The frosted glass of bars, menus, panels and controls.
    #[default]
    Regular,
    /// The clearer glass of controls over rich content, such as media.
    Clear,
    /// The regular glass of a large floating panel (the launcher): its face toned further towards the middle, with a
    /// soft drop shadow of its own below it.
    LargePanel,
}

impl GlassStyle {
    /// The material name that stands for this glass on its own, as the theme names it (`Glass`, `ClearGlass`,
    /// `LargePanelGlass`).
    pub fn material(self) -> &'static str {
        match self {
            Self::Regular => "Glass",
            Self::Clear => "ClearGlass",
            Self::LargePanel => "LargePanelGlass",
        }
    }

    /// The glass a material name stands for.
    pub fn of_material(name: &str) -> Option<Self> {
        [Self::Regular, Self::Clear, Self::LargePanel]
            .into_iter()
            .find(|s| s.material() == name)
    }
}

/// How a [`MaterialRegion`] draws as glass: regions of a window with the same nonzero `container` join into one shape
/// whose outlines blend into each other over `spacing` points (0 joins them with a hard edge); container 0 keeps the
/// region on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRegion {
    pub style: GlassStyle,
    pub container: u32,
    pub spacing: f64,
}

/// What the text and symbols over a glass region are drawn with: the compositor measures the face of a small glass
/// shape and tells whether it is dark or light ([`WindowEvent::GlassTone`]); larger ones keep the appearance's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassTone {
    /// The appearance's: nothing was measured, or the shape is too large to follow what lies behind it.
    #[default]
    Appearance,
    /// A dark face: light text and symbols.
    Dark,
    /// A light face: dark text and symbols.
    Light,
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

/// What a drop may do with the data dragged to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropAction {
    Copy,
    Move,
    /// The target asks the person which.
    Ask,
}

/// The image that follows the pointer during a drag: `width` × `height` premultiplied RGBA pixels in rows of
/// `width * 4` bytes, drawn at `scale` whole pixels per point, with the pointer at `hotspot`, in points from its
/// top-left corner. Both sides are whole points at `scale`.
#[derive(Clone, PartialEq)]
pub struct DragImage {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub pixels: Vec<u8>,
    pub hotspot: Point,
}

/// Sizes and hotspot only: the pixels are an image of what is dragged.
impl std::fmt::Debug for DragImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DragImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("hotspot", &self.hotspot)
            .finish_non_exhaustive()
    }
}

/// A drag of data out of a window: the data in each of its MIME types, what a drop may do with it, and the image that
/// follows the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct DragRequest {
    pub data: Vec<ClipboardData>,
    pub actions: BTreeSet<DropAction>,
    pub image: Option<DragImage>,
}

/// Names one drag [`WindowSystem::start_drag`] started; how it goes arrives as [`PlatformEvent::Drag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DragToken(pub u64);

/// How a drag this application started goes. It ends with [`DragProgress::Finished`] or
/// [`DragProgress::Cancelled`]; the image is gone by then, and from the drop on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragProgress {
    /// The MIME type the target under the pointer accepts, `None` when it accepts none.
    Target(Option<String>),
    /// What a drop would do now, `None` when nothing.
    Action(Option<DropAction>),
    /// The data was dropped and the target is reading it.
    Dropped,
    /// The target took the data; with [`DropAction::Move`] the source removes its own copy.
    Finished,
    /// No drop: the drag was released where nothing accepts it, refused by the target, cancelled, or ignored by the
    /// compositor because the button had gone up.
    Cancelled,
}

/// Names one drag from any application, this one included, over this application's windows: from its enter until it
/// leaves, or until its drop is finished with [`WindowSystem::finish_drop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropToken(pub u64);

/// A drag from any application, this one included, over a window, in the window's logical coordinates. It enters,
/// moves, and then leaves or drops; [`WindowSystem::answer_drop`] tells the dragging application what the window would
/// take where the pointer is. `mods` are the keyboard modifiers held as the window system last knew them.
#[derive(Clone, Debug, PartialEq)]
pub enum DropEvent {
    /// The drag came over the window: its data's MIME types, as the source offers them, and what the source allows
    /// done with it (a copy alone when its protocol has no actions).
    Enter {
        token: DropToken,
        position: Point,
        mimes: Vec<String>,
        actions: BTreeSet<DropAction>,
        mods: Mods,
    },
    Motion {
        position: Point,
        mods: Mods,
    },
    /// What a drop would do now, chosen from the source's actions and the window's answer; `None` when nothing.
    Action(Option<DropAction>),
    /// The drag left the window, or ended without a drop on it.
    Leave,
    /// The data was dropped where the drag was last: read it with [`WindowSystem::read_drop`] in the type the answer
    /// took, then end it with [`WindowSystem::finish_drop`].
    Drop,
}

/// What a window takes of the drag over it, where the pointer is now: the MIME type it would read (`None` refuses the
/// drag there), the actions it can do with the data and the one it prefers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropAnswer {
    pub mime: Option<String>,
    pub actions: BTreeSet<DropAction>,
    pub preferred: Option<DropAction>,
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

/// What produced a scroll. Only [`ScrollSource::Finger`] scrolls end with a stop that kinetic scrolling may start
/// from; a wheel's steps are complete movements and get no inertia of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollSource {
    Wheel,
    Finger,
    /// Continuous movement without a finger on a surface (a pointing stick, button scrolling).
    Continuous,
    /// A wheel pushed sideways: horizontal steps.
    WheelTilt,
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
        /// Per axis `(x, y)`: the physical motion ran against `delta` (natural scrolling is on). Content follows
        /// `delta`; a value control that should follow the hand can undo the inversion.
        inverted: (bool, bool),
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
    /// The compositor placed a popup at this rectangle of its parent, in points: beside its anchor, or flipped or slid
    /// where there was no room. Sent with each placement, before the [`WindowEvent::Configure`] that comes with it.
    PopupPlaced(Rect),
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
    /// A drag from any application over the window. Needs [`Capabilities::drag_and_drop`].
    Drop(DropEvent),
    /// The face of a glass region changed tone: `region` is the index of a [`MaterialRegion`] of the last
    /// [`WindowRequest::Materials`], or `None` for the window's own [`Backdrop`]. A window system that measures nothing
    /// never sends it, and every region keeps [`GlassTone::Appearance`].
    GlassTone {
        region: Option<usize>,
        tone: GlassTone,
    },
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
    /// How a drag [`WindowSystem::start_drag`] started goes.
    Drag {
        token: DragToken,
        progress: DragProgress,
    },
    /// The waker was woken.
    Wake,
    /// The session's appearance or display options, once when the window system learns them and on every change.
    Appearance(SystemAppearance),
    /// Whether the session is locked, once when the window system learns it and on every change; a window system that
    /// can't tell sends false once.
    SessionLock(bool),
    /// How the session wants scroll views to show their bars, once when the window system learns it and on every
    /// change; a window system that can't tell never sends it, and each view keeps its own style.
    ScrollBars(ScrollBars),
}

/// How the session wants scroll views to show their bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScrollBars {
    /// Thin bars over the content, shown while it scrolls.
    WhenScrolling,
    /// Tracks beside the content that stay shown.
    Always,
}

/// The session's appearance and the display options that change how windows draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemAppearance {
    pub dark: bool,
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
    pub reduce_motion: bool,
    /// The text caret stays shown instead of blinking.
    pub steady_caret: bool,
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

pub use cupertino_semantics as semantics;

pub trait WindowSystem {
    /// Native semantics are opt-in per surface and require compositor consent.
    fn semantics_active(&self, _window: WindowId) -> bool {
        false
    }
    fn publish_semantics(&mut self, _window: WindowId, _snapshot: &semantics::Snapshot) {}
    fn semantic_requests(&mut self) -> Vec<(WindowId, semantics::ActionCall)> {
        Vec::new()
    }
    fn semantic_finished(
        &mut self,
        _window: WindowId,
        _call: &semantics::ActionCall,
        _outcome: semantics::Outcome,
        _revision: u64,
    ) {
    }

    fn capabilities(&self) -> Capabilities;
    /// Whether a popup opened now can take a grab: the window system has had a user input event (a press, a touch, a
    /// key, the keyboard coming to a window) for the grab to answer. Before that, as for a menu an assistive action
    /// opens in a shell component nobody has touched yet, a grab would be refused, so the runtime opens the popup
    /// without one and closes it itself.
    fn can_grab(&self) -> bool {
        true
    }
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
    /// Starts dragging `request` out of `window`, from the pointer button held in it, with the image following the
    /// pointer. The window then gets a pointer leave while the drag runs, and how it goes arrives as
    /// [`PlatformEvent::Drag`]. Needs [`Capabilities::drag_and_drop`]; without it, and when no button is held, the drag
    /// is refused.
    fn start_drag(
        &mut self,
        window: WindowId,
        request: DragRequest,
    ) -> Result<DragToken, PlatformError> {
        let _ = (window, request);
        Err(PlatformError::Unsupported)
    }
    /// Tells the application dragging `token` what the window under it would take where the pointer is now; answered
    /// on the drag's enter and whenever that changes, until the drop. A drag dropped where the answer took no type
    /// (or no action the source allows) is not dropped. Needs [`Capabilities::drag_and_drop`].
    fn answer_drop(&mut self, token: DropToken, answer: &DropAnswer) -> Result<(), PlatformError> {
        let _ = (token, answer);
        Err(PlatformError::Unsupported)
    }
    /// Starts reading the data dropped as `token` in `mime`, the type the last answer took; it arrives as
    /// [`PlatformEvent::Clipboard`] with the returned token, `None` when it could not be read. Before the drop, or in
    /// another type, it is refused.
    fn read_drop(&mut self, token: DropToken, mime: &str) -> Result<ClipboardToken, PlatformError> {
        let _ = (token, mime);
        Err(PlatformError::Unsupported)
    }
    /// Ends the drop of `token`: `Some(action)` took the data with that action (the source removes its own copy after a
    /// move), told to the source even when another was chosen during the drag, and refused when the source does not
    /// allow it; `None` refused it and the source keeps it. A token that is gone is ignored.
    fn finish_drop(&mut self, token: DropToken, action: Option<DropAction>) {
        let _ = (token, action);
    }
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
    /// Shows `layer` over the window's content with the window's next [`present`](WindowSystem::present), or hides it
    /// then with `None`: the window's raised materials ([`MaterialRegion::raised`]) lie between the two. Like a present
    /// it copies the frame's damage, plus whatever the buffer it is drawn into missed. `NotReady` keeps nothing: the
    /// window presents neither this frame nor its content's until the layer can be drawn again. Needs
    /// [`Capabilities::raised_layer`].
    fn present_layer(
        &mut self,
        window: WindowId,
        layer: Option<RaisedLayer<'_>>,
    ) -> Result<PresentOutcome, PlatformError> {
        let _ = window;
        match layer {
            Some(_) => Err(PlatformError::Unsupported),
            None => Ok(PresentOutcome::Presented),
        }
    }
    /// Shows `window` as `color` all over, without a frame of its size: the compositor stretches one pixel of the
    /// colour over the window, so a large window that only takes input (transparent) or only dims what is under it
    /// costs nothing to show, however often it is shown again. Acknowledges `configure` and is paced like
    /// [`present`](WindowSystem::present); the window's input region still applies, and its scale needs nothing
    /// more. A frame presented afterwards is presented whole. [`PlatformError::Unsupported`] (the default) when the
    /// window system cannot: the caller presents a frame filled with the colour instead.
    fn present_solid(
        &mut self,
        window: WindowId,
        color: Color,
        configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        let _ = (window, color, configure);
        Err(PlatformError::Unsupported)
    }
    /// Waits up to `timeout` (`None`: until something happens) and appends what happened to `events`.
    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError>;
    /// Wakes a waiting [`dispatch`](WindowSystem::dispatch) from any thread.
    fn waker(&self) -> Waker;
}
