//! Command / effect API between the compositor and the model.

use crate::focus::SwitcherStep;
use crate::geometry::{Edges, Point, Rect, Size};
use crate::ids::{AppId, OutputId, WindowId, WorkspaceId};
use crate::lifecycle::{LifecycleState, ToplevelStates};
use crate::tiling::TileKind;
use crate::window::Assignment;
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
    /// Focus and raise the new window (activation policy is the caller's).
    pub activate: bool,
    /// Selects the open / close transform (table).
    pub kind: WindowKind,
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
            activate: true,
            kind: WindowKind::Plain,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    AddOutput { output: OutputId, rect: Rect, usable: Rect },
    RemoveOutput { output: OutputId },
    SetUsableArea { output: OutputId, usable: Rect },
    /// The output changed size or position: windows resting in full screen, zoomed, tiled or
    /// normal layouts are refitted to the new `rect` / `usable` area (no animation).
    SetOutputRect { output: OutputId, rect: Rect, usable: Rect },
    /// Region owned by the dock of `output`; tiling hot zones inside it are ignored when configured.
    SetDockZone { output: OutputId, zone: Option<Rect> },

    RegisterApp { identity: String },
    RemoveApp { app: AppId },

    Map(MapParams),
    /// The client unmapped its surface: plays the closing animation, then destroys.
    Unmap { window: WindowId },
    /// The surface is gone: immediate destruction.
    Destroy { window: WindowId },
    SetTitle { window: WindowId, title: String },
    SetSizeHints { window: WindowId, min: Option<Size>, max: Option<Size>, content: Option<Size> },
    SetParent { window: WindowId, parent: Option<WindowId> },
    SetModal { window: WindowId, modal: bool },
    /// Ask the client to close (compositor-initiated, e.g. window control button).
    RequestClose { window: WindowId },

    Minimize { window: WindowId, target: Option<Rect> },
    Restore { window: WindowId },
    /// Move the rectangle a minimized window's restore animation flows out of (its tile moved).
    /// Ignored unless the window is minimizing or minimized.
    SetMinimizeTarget { window: WindowId, target: Rect },
    ToggleZoom { window: WindowId },
    Fullscreen { window: WindowId, output: Option<OutputId> },
    ExitFullscreen { window: WindowId },
    Tile { window: WindowId, kind: TileKind },
    Untile { window: WindowId },

    FocusWindow { window: WindowId },
    /// The client of `window` wants attention: an activation the compositor refused
    /// ([`crate::Wm::activation_allowed`]). Its application's tile bounces unless it is active.
    DemandAttention { window: WindowId },
    ActivateApp { app: AppId },
    HideApp { app: AppId },
    UnhideApp { app: AppId },
    HideOthers { app: AppId },
    ShowAll,

    /// The switch chord: opens the switcher, or steps it like a fresh Tab press when it is open.
    SwitcherOpen { backward: bool },
    /// A switcher key moved the selection; `repeat` is a key repeat, which does not wrap.
    SwitcherStep { step: SwitcherStep, repeat: bool },
    /// Select entry `index` of the open switcher (a pointer on its panel); out of range or without
    /// a switcher it does nothing.
    SwitcherSelect { index: usize },
    SwitcherCommit,
    SwitcherCancel,
    /// Q in the switcher: ask the selected application's windows to close.
    SwitcherQuit,
    /// H in the switcher: hide the selected application, or show it when hidden.
    SwitcherToggleHide,
    CycleWindows { backward: bool },
    EndCycleWindows,

    BeginMove { window: WindowId, pointer: Point },
    BeginResize { window: WindowId, edges: Edges, pointer: Point },
    /// Double click on a resize edge or corner: each of `edges` (with `opposite`, the Alt
    /// modifier, also the opposite ones) moves to the usable area's edge.
    SnapEdgesToScreen { window: WindowId, edges: Edges, opposite: bool },
    PointerMotion { pointer: Point, modifiers: Modifiers },
    PointerRelease,
    CancelInteraction,

    AddWorkspace { output: OutputId },
    RemoveWorkspace { workspace: WorkspaceId },
    /// Move a regular workspace to `index` (0-based, clamped to the end) among its output's workspaces.
    MoveWorkspace { workspace: WorkspaceId, index: usize },
    SwitchWorkspace { output: OutputId, workspace: WorkspaceId },
    SwitchWorkspaceRelative { output: OutputId, delta: i32 },
    MoveWindowToWorkspace { window: WindowId, workspace: WorkspaceId },
    SetSticky { window: WindowId, sticky: bool },
    AssignApp { app: AppId, assignment: Assignment },
    /// `fraction` is the finger displacement divided by the output width (positive = right).
    WorkspaceGestureBegin { output: OutputId },
    WorkspaceGestureUpdate { output: OutputId, fraction: f64 },
    WorkspaceGestureEnd { output: OutputId },
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
    AppRegistered { app: AppId, identity: String },
    /// `xdg_toplevel` configure: size and states.
    Configure { window: WindowId, size: Size, states: ToplevelStates },
    /// Logical frame (position and size) of the window.
    Place { window: WindowId, rect: Rect },
    RequestClose { window: WindowId },
    /// Keyboard focus; `None` clears it.
    Focus { window: Option<WindowId> },
    /// Application owning the menu bar; `None` when no app is active.
    Activate { app: Option<AppId> },
    Raise { window: WindowId },
    /// App-level hide/unhide (`hidden` apps are withheld from rendering).
    SetVisible { window: WindowId, visible: bool },
    StartAnimation { window: WindowId, kind: AnimationKind },
    WindowStateChanged { window: WindowId, from: LifecycleState, to: LifecycleState },
    WindowDestroyed { window: WindowId },
    /// A click on a modal-blocked window: pulse the dialog.
    Attention { window: WindowId },
    /// The application asked for attention while inactive: its dock tile bounces once
    /// (informational request).
    AppAttention { app: AppId },
    WorkspaceCreated { output: OutputId, workspace: WorkspaceId, index: usize },
    WorkspaceRemoved { output: OutputId, workspace: WorkspaceId },
    WorkspaceSwitched { output: OutputId, from: WorkspaceId, to: WorkspaceId },
    WorkspacesReordered { output: OutputId },
    SnapPreview { window: WindowId, rect: Option<Rect>, immediate: bool },
    SwitcherChanged { apps: Vec<AppId>, selected: usize },
    /// The switcher is gone. A commit fades the panel out, a cancel removes it at once.
    SwitcherClosed { committed: bool },
}
