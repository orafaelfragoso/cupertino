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

mod surface_layers;
pub use cupertino_animation::{CubicBezier, LayerCurve};
pub use cupertino_ui_render::Color;
pub use surface_layers::{LayerChange, LayerProperty, SurfaceLayer, SurfaceLayerLook};

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
    /// A window given its role by the application's surface roles may hand transitions of its presentation (offset,
    /// opacity and scale of all it draws) to the compositor, which runs them on its clock ([`WindowRequest::Present`]).
    pub present_animation: bool,
    /// A popup may hand the transitions of its presentation to the compositor too ([`WindowRequest::Present`]): a
    /// popover's growth out of its anchor and shrinking back, a closing menu's fade. Its scale is about the centre of
    /// the popup's window.
    pub present_popups: bool,
    /// A window may draw marked subtrees into surfaces of its own that the compositor places and animates
    /// ([`WindowSystem::present_surface_layers`], [`WindowRequest::Layer`]).
    pub surface_layers: bool,
}

/// A property of a window's presentation ([`WindowRequest::Present`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PresentProperty {
    /// Moves all the window draws across, in points.
    OffsetX,
    /// Moves all the window draws down, in points.
    OffsetY,
    /// Fades all the window draws, its materials included, from 0 to 1.
    Opacity,
    /// Scales all the window draws about the window's centre.
    Scale,
}

/// One change of a window's presentation: `property` goes to `to` at once (`curve` `None`), or along `curve` from what
/// is shown when the change is first drawn, keeping its velocity; `velocity` (units per second) starts a spring from
/// rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresentChange {
    pub property: PresentProperty,
    pub to: f64,
    pub curve: Option<cupertino_animation::LayerCurve>,
    pub velocity: f64,
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
    /// Changes the window's presentation on the compositor's clock, all the changes together and without drawing a
    /// frame: with the window's first frame when it has not shown one, at once otherwise. Input and the published
    /// semantics stay with the window's layout, the presentation's target. Needs
    /// [`Capabilities::present_animation`].
    Present(Vec<PresentChange>),
    /// Changes the window's surface layer `key` ([`SurfaceLayer`]) on the compositor's clock, all the changes together
    /// and without drawing a frame: with the layer's first frame when it has not shown one, at once otherwise. Needs
    /// [`Capabilities::surface_layers`].
    Layer {
        key: u64,
        changes: Vec<LayerChange>,
    },
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
    /// The glass of a floating bar of icons (the dock): a narrow blur, a thin bending edge and a thin line of light along its top and bottom.
    Bar,
    /// The glass of a dense floating surface with text on it (a menu, a popover): regular glass over a mid grey veil.
    Dense,
    /// The glass of the tiles of a panel of modules: the clear face under a wide blur and a strong bending edge.
    Module,
    /// The glass of a notification card: the clear face over a wide blur and a blur fill, with a faint ring shadow.
    Card,
    /// The glass of a level's track: no blur, a bending edge that follows its height and a ring shadow.
    Track,
    /// The glass of a small popover over a floating bar (a name, a menu): a wide soft blur fill, a strong bending edge
    /// and a thin line of light along its top and bottom edges.
    Popover,
    /// The glass of a wide floating panel of large icons (the application switcher): a blur of about a point, a strong
    /// bending edge and no shadow.
    Switcher,
    /// The glass of a window's sidebar: a wide soft blur fill over a light pane in the light appearance and a dark,
    /// nearly grey one in the dark.
    Sidebar,
    /// The module glass of a tile that steps back: twice the blur and a quieter face.
    SubduedModule,
    /// The glass of a small card of content over the desktop: a faint blur under a wide soft blur fill and a veiled face.
    Snippet,
    /// Clear glass veiled black from a fifth at its top to fully at its bottom.
    Veiled,
    /// The glass of a window's toolbar controls: regular glass with a face lifted a little further and a thin line of
    /// light along its top and bottom edges.
    Toolbar,
    /// The glass of a menu: a wide blur under a face that tones the backdrop into a narrow band, with a thin line of light
    /// along its top and bottom edges.
    Menu,
}

impl GlassStyle {
    /// The material name that stands for this glass on its own, as the theme names it (`Glass`, `ClearGlass`,
    /// `LargePanelGlass`, `BarGlass`, `DenseGlass`, `ModuleGlass`, `CardGlass`, `TrackGlass`).
    pub fn material(self) -> &'static str {
        match self {
            Self::Regular => "Glass",
            Self::Clear => "ClearGlass",
            Self::LargePanel => "LargePanelGlass",
            Self::Bar => "BarGlass",
            Self::Dense => "DenseGlass",
            Self::Module => "ModuleGlass",
            Self::Card => "CardGlass",
            Self::Track => "TrackGlass",
            Self::Popover => "PopoverGlass",
            Self::Switcher => "SwitcherGlass",
            Self::Sidebar => "SidebarGlass",
            Self::SubduedModule => "SubduedModuleGlass",
            Self::Snippet => "SnippetGlass",
            Self::Veiled => "VeiledGlass",
            Self::Toolbar => "ToolbarGlass",
            Self::Menu => "MenuGlass",
        }
    }

    /// The glass a material name stands for.
    pub fn of_material(name: &str) -> Option<Self> {
        [
            Self::Regular,
            Self::Clear,
            Self::LargePanel,
            Self::Bar,
            Self::Dense,
            Self::Module,
            Self::Card,
            Self::Track,
            Self::Popover,
            Self::Switcher,
            Self::Sidebar,
            Self::SubduedModule,
            Self::Snippet,
            Self::Veiled,
            Self::Toolbar,
            Self::Menu,
        ]
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
    /// What the window system showed is gone (the GPU was reset or its memory purged): every window draws a whole
    /// frame again, even one that changed nothing.
    BuffersLost,
    /// The process now draws with `RendererKind`: every window makes its renderer anew, so what the old one kept
    /// (its frame, layers and caches) is freed, and draws a whole frame.
    RendererChanged(RendererKind),
}

/// The renderer a process draws its windows with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RendererKind {
    Cpu,
    Gpu,
}

impl RendererKind {
    /// `cpu` or `gpu`, as logs and reports name it.
    pub fn name(self) -> &'static str {
        match self {
            RendererKind::Cpu => "cpu",
            RendererKind::Gpu => "gpu",
        }
    }
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
    /// Whether a popup of `parent` opened now can take a grab: the window system has had a user input event (a press, a
    /// touch, a key, the keyboard coming to a window) for the grab to answer, or a desktop-control action on `parent`'s
    /// window came with a grab of its own and is being handled (from [`WindowSystem::semantic_requests`] to
    /// [`WindowSystem::semantic_finished`], and a moment after an applied answer). Otherwise, as for a menu an
    /// assistive action opens in a shell component nobody has touched yet, a grab would be refused, so the runtime
    /// opens the popup without one and closes it itself.
    fn can_grab(&self, parent: WindowId) -> bool {
        let _ = parent;
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
    /// Times the copies of frames into buffers from now on (`on`), or stops; off at first, and a window system that
    /// copies nothing ignores it.
    fn time_copies(&mut self, on: bool) {
        let _ = on;
    }
    /// The time spent copying frames into buffers since the last call, while copies are timed; zero otherwise.
    fn take_copy_time(&mut self) -> Duration {
        Duration::ZERO
    }
    /// Whether the compositor's latest configure of `window` states it activated, where its protocol states that (a
    /// toplevel); `None` where only the keyboard tells. A toplevel the compositor keeps activated while a surface of
    /// the desktop holds the keyboard stays the window in use.
    fn activated(&self, window: WindowId) -> Option<bool> {
        let _ = window;
        None
    }
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
    /// Shows `layers` with the window's next [`present`](WindowSystem::present), in their order back to front over
    /// the window's content (and its raised layer), and takes away any shown before that is not among them. A layer's
    /// frame, when it has one, is copied as a present's is, its damage and whatever its buffer missed. `NotReady`
    /// keeps nothing: the window presents neither these frames nor its content's until they can be drawn again. Needs
    /// [`Capabilities::surface_layers`]; without it (the default) only an empty list is taken.
    fn present_surface_layers(
        &mut self,
        window: WindowId,
        layers: &[SurfaceLayer<'_>],
    ) -> Result<PresentOutcome, PlatformError> {
        let _ = window;
        if layers.is_empty() {
            Ok(PresentOutcome::Presented)
        } else {
            Err(PlatformError::Unsupported)
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

#[cfg(test)]
mod glass_style_tests {
    use super::GlassStyle;

    #[test]
    fn every_glass_style_has_a_material_name_that_stands_for_it() {
        for style in [
            GlassStyle::Regular,
            GlassStyle::Clear,
            GlassStyle::LargePanel,
            GlassStyle::Bar,
            GlassStyle::Dense,
            GlassStyle::Module,
            GlassStyle::Card,
            GlassStyle::Track,
            GlassStyle::Popover,
            GlassStyle::Switcher,
            GlassStyle::Sidebar,
            GlassStyle::SubduedModule,
            GlassStyle::Snippet,
            GlassStyle::Veiled,
            GlassStyle::Toolbar,
            GlassStyle::Menu,
        ] {
            assert_eq!(GlassStyle::of_material(style.material()), Some(style));
        }
        assert_eq!(GlassStyle::Card.material(), "CardGlass");
        assert_eq!(GlassStyle::Track.material(), "TrackGlass");
        assert_eq!(GlassStyle::Popover.material(), "PopoverGlass");
        assert_eq!(GlassStyle::Switcher.material(), "SwitcherGlass");
        assert_eq!(GlassStyle::Sidebar.material(), "SidebarGlass");
        assert_eq!(GlassStyle::SubduedModule.material(), "SubduedModuleGlass");
        assert_eq!(GlassStyle::Snippet.material(), "SnippetGlass");
        assert_eq!(GlassStyle::Veiled.material(), "VeiledGlass");
        assert_eq!(GlassStyle::Toolbar.material(), "ToolbarGlass");
        assert_eq!(GlassStyle::Menu.material(), "MenuGlass");
        assert_eq!(GlassStyle::of_material("Sidebar"), None);
    }

    #[test]
    fn the_sidebar_subdued_module_snippet_and_veiled_glass_go_by_their_material_names() {
        for (name, style) in [
            ("SidebarGlass", GlassStyle::Sidebar),
            ("SubduedModuleGlass", GlassStyle::SubduedModule),
            ("SnippetGlass", GlassStyle::Snippet),
            ("VeiledGlass", GlassStyle::Veiled),
        ] {
            assert_eq!(GlassStyle::of_material(name), Some(style));
        }
    }

    #[test]
    fn the_switcher_glass_goes_by_its_material_name() {
        assert_eq!(
            GlassStyle::of_material("SwitcherGlass"),
            Some(GlassStyle::Switcher)
        );
    }
}
