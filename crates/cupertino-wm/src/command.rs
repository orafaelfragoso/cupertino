//! Command / effect API between the compositor and the model.

use crate::focus::SwitcherStep;
use crate::geometry::{Edges, Point, Rect, Size};
use crate::ids::{AppId, OutputId, WindowId, WorkspaceId};
use crate::lifecycle::{LifecycleState, ToplevelStates};
use crate::tiling::TileKind;
use crate::window::{Assignment, DisabledControls};
use cupertino_effects::WindowKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// The primary modifier (app switcher / window cycling).
    pub primary: bool,
    /// The option-accelerator modifier (immediate tiling preview).
    pub option: bool,
    pub shift: bool,
    pub ctrl: bool,
}

/// What the compositor made of an activation token for a window that is about to map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grant {
    /// When the token was made, on the model's clock.
    pub created: f64,
    /// It was made by the compositor, or by a client for an input event it got while it had the keyboard focus.
    pub valid: bool,
}

/// The facts the focus-on-map rule ([`crate::Wm`]) needs from the compositor, which owns the connections and the input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapFocus {
    /// The window is the first toplevel its client maps.
    pub first_of_client: bool,
    /// The person has given input to another client since this one connected.
    pub input_elsewhere: bool,
    /// The activation token the client presented for this window, if any.
    pub grant: Option<Grant>,
    /// The last key or button press, on the model's clock.
    pub last_user_event: Option<f64>,
}

/// Whether a window that maps takes the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Activation {
    /// Always: the shell's own windows and the tests.
    #[default]
    Always,
    /// When the focus policy allows it; else the window opens behind the key window and its application's tile asks for
    /// attention.
    Policy(MapFocus),
}

/// Who asked for a change of a window: the person (a shortcut, a menu, a gesture) or the window's client itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requester {
    User,
    Client,
}

#[derive(Clone, Debug)]
pub struct MapParams {
    pub window: WindowId,
    /// Application identity string; apps are created on first use.
    pub app: String,
    pub title: String,
    pub geometry: Rect,
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,
    pub parent: Option<WindowId>,
    pub modal: bool,
    /// Output to open on; defaults to the first registered output.
    pub output: Option<OutputId>,
    /// Whether the new window takes the focus.
    pub activation: Activation,
    /// Selects the open / close transform (table).
    pub kind: WindowKind,
    /// The controls its application disabled before the first commit.
    pub disabled: DisabledControls,
    /// How far below the top of this window's frame its sheets hang: the bottom of the row its title or toolbar is in.
    /// `None` is a title bar of [`crate::SnapConfig::titlebar_height`].
    pub sheet_top: Option<f64>,
}

impl MapParams {
    pub fn new(window: WindowId, app: &str, geometry: Rect) -> Self {
        Self {
            window,
            app: app.to_string(),
            title: String::new(),
            geometry,
            min_size: None,
            max_size: None,
            parent: None,
            modal: false,
            output: None,
            activation: Activation::Always,
            kind: WindowKind::Plain,
            disabled: DisabledControls::default(),
            sheet_top: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    AddOutput {
        output: OutputId,
        rect: Rect,
        usable: Rect,
    },
    RemoveOutput {
        output: OutputId,
    },
    SetUsableArea {
        output: OutputId,
        usable: Rect,
    },
    /// The output changed size or position: windows resting in full screen, zoomed, tiled or
    /// normal layouts are refitted to the new `rect` / `usable` area (no animation).
    SetOutputRect {
        output: OutputId,
        rect: Rect,
        usable: Rect,
    },
    /// Region owned by the dock of `output`; tiling hot zones inside it are ignored when configured.
    SetDockZone {
        output: OutputId,
        zone: Option<Rect>,
    },

    RegisterApp {
        identity: String,
    },
    RemoveApp {
        app: AppId,
    },

    Map(MapParams),
    /// The client unmapped its surface: plays the closing animation, then destroys.
    Unmap {
        window: WindowId,
    },
    /// The surface is gone: immediate destruction.
    Destroy {
        window: WindowId,
    },
    SetTitle {
        window: WindowId,
        title: String,
    },
    /// The controls the window's application disabled; minimize and zoom are refused while disabled, whoever asks (an
    /// unzoom and a restore are not).
    SetDisabledControls {
        window: WindowId,
        disabled: DisabledControls,
    },
    SetSizeHints {
        window: WindowId,
        min: Option<Size>,
        max: Option<Size>,
        content: Option<Size>,
    },
    SetParent {
        window: WindowId,
        parent: Option<WindowId>,
    },
    SetModal {
        window: WindowId,
        modal: bool,
    },
    /// Ask the client to close (compositor-initiated, e.g. window control button).
    RequestClose {
        window: WindowId,
    },

    Minimize {
        window: WindowId,
        target: Option<Rect>,
    },
    Restore {
        window: WindowId,
    },
    /// Move the rectangle a minimized window's restore animation flows out of (its tile moved).
    /// Ignored unless the window is minimizing or minimized.
    SetMinimizeTarget {
        window: WindowId,
        target: Rect,
    },
    ToggleZoom {
        window: WindowId,
    },
    Fullscreen {
        window: WindowId,
        output: Option<OutputId>,
        requested_by: Requester,
    },
    ExitFullscreen {
        window: WindowId,
    },
    Tile {
        window: WindowId,
        kind: TileKind,
    },
    Untile {
        window: WindowId,
    },

    FocusWindow {
        window: WindowId,
    },
    /// The client of `window` wants attention: an activation the compositor refused
    /// ([`crate::Wm::activation_allowed`]). Its application's tile bounces unless it is active.
    DemandAttention {
        window: WindowId,
    },
    /// The Windows habit of Super+Down: a tiled, filled or zoomed window goes back to the frame it had; any other
    /// window is minimized.
    MinimizeOrRestore {
        window: WindowId,
    },
    ActivateApp {
        app: AppId,
    },
    HideApp {
        app: AppId,
    },
    UnhideApp {
        app: AppId,
    },
    HideOthers {
        app: AppId,
    },
    ShowAll,
    /// Raise every shown window of `app` above the other applications' windows, keeping their order and the key
    /// window. With `cascade` the app's other top-level windows on the key window's output then step
    /// [`crate::CASCADE_STEP`] further up and left of it each, from the top of the stack, kept in the usable area;
    /// their children move with them and only windows in the normal layout move.
    BringAllToFront {
        app: AppId,
        cascade: bool,
    },

    /// The switch chord: opens the switcher, or steps it like a fresh Tab press when it is open.
    SwitcherOpen {
        backward: bool,
    },
    /// A switcher key moved the selection; `repeat` is a key repeat, which does not wrap.
    SwitcherStep {
        step: SwitcherStep,
        repeat: bool,
    },
    /// Select entry `index` of the open switcher (a pointer on its panel); out of range or without
    /// a switcher it does nothing.
    SwitcherSelect {
        index: usize,
    },
    SwitcherCommit,
    SwitcherCancel,
    /// Q in the switcher: ask the selected application's windows to close.
    SwitcherQuit,
    /// H in the switcher: hide the selected application, or show it when hidden.
    SwitcherToggleHide,
    CycleWindows {
        backward: bool,
    },
    EndCycleWindows,

    BeginMove {
        window: WindowId,
        pointer: Point,
    },
    BeginResize {
        window: WindowId,
        edges: Edges,
        pointer: Point,
    },
    /// Double click on a resize edge or corner: each of `edges` (with `opposite`, the Alt
    /// modifier, also the opposite ones) moves to the usable area's edge.
    SnapEdgesToScreen {
        window: WindowId,
        edges: Edges,
        opposite: bool,
    },
    PointerMotion {
        pointer: Point,
        modifiers: Modifiers,
    },
    PointerRelease,
    CancelInteraction,

    AddWorkspace {
        output: OutputId,
    },
    RemoveWorkspace {
        workspace: WorkspaceId,
    },
    /// Move a regular workspace to `index` (0-based, clamped to the end) among its output's workspaces.
    MoveWorkspace {
        workspace: WorkspaceId,
        index: usize,
    },
    SwitchWorkspace {
        output: OutputId,
        workspace: WorkspaceId,
    },
    SwitchWorkspaceRelative {
        output: OutputId,
        delta: i32,
    },
    MoveWindowToWorkspace {
        window: WindowId,
        workspace: WorkspaceId,
    },
    SetSticky {
        window: WindowId,
        sticky: bool,
    },
    AssignApp {
        app: AppId,
        assignment: Assignment,
    },
    /// `fraction` is the finger displacement divided by the output width (positive = right).
    WorkspaceGestureBegin {
        output: OutputId,
    },
    WorkspaceGestureUpdate {
        output: OutputId,
        fraction: f64,
    },
    WorkspaceGestureEnd {
        output: OutputId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationKind {
    Opening,
    Minimize,
    Restore,
    Zoom,
    Unzoom,
    EnterFullScreen,
    ExitFullScreen,
    Tile(TileKind),
    DragOut,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    AppRegistered {
        app: AppId,
        identity: String,
    },
    /// `xdg_toplevel` configure: size and states.
    Configure {
        window: WindowId,
        size: Size,
        states: ToplevelStates,
    },
    /// Logical frame (position and size) of the window.
    Place {
        window: WindowId,
        rect: Rect,
    },
    RequestClose {
        window: WindowId,
    },
    /// Keyboard focus; `None` clears it.
    Focus {
        window: Option<WindowId>,
    },
    /// Application owning the menu bar; `None` when no app is active.
    Activate {
        app: Option<AppId>,
    },
    Raise {
        window: WindowId,
    },
    /// App-level hide/unhide (`hidden` apps are withheld from rendering).
    SetVisible {
        window: WindowId,
        visible: bool,
    },
    StartAnimation {
        window: WindowId,
        kind: AnimationKind,
    },
    WindowStateChanged {
        window: WindowId,
        from: LifecycleState,
        to: LifecycleState,
    },
    WindowDestroyed {
        window: WindowId,
    },
    /// A click on a modal-blocked window: pulse the dialog.
    Attention {
        window: WindowId,
    },
    /// The application asked for attention while inactive: its dock tile bounces once
    /// (informational request).
    AppAttention {
        app: AppId,
    },
    WorkspaceCreated {
        output: OutputId,
        workspace: WorkspaceId,
        index: usize,
    },
    WorkspaceRemoved {
        output: OutputId,
        workspace: WorkspaceId,
    },
    WorkspaceSwitched {
        output: OutputId,
        from: WorkspaceId,
        to: WorkspaceId,
    },
    WorkspacesReordered {
        output: OutputId,
    },
    SnapPreview {
        window: WindowId,
        rect: Option<Rect>,
        immediate: bool,
    },
    SwitcherChanged {
        apps: Vec<AppId>,
        selected: usize,
    },
    /// The switcher is gone. A commit fades the panel out, a cancel removes it at once.
    SwitcherClosed {
        committed: bool,
    },
}
