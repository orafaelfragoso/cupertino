//! Applications and windows.

use crate::geometry::{Rect, Size};
use crate::ids::{AppId, OutputId, WindowId, WorkspaceId};
use crate::lifecycle::{Lifecycle, LifecycleState};
use crate::zoom::SizeHints;

/// Per-application workspace assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Assignment {
    #[default]
    None,
    /// Windows of the app appear on every workspace of their output.
    AllWorkspaces,
    /// New windows of the app open on this workspace.
    Workspace(WorkspaceId),
}

/// Stacking level of a window: a window never stacks below one of a lower level (level
/// values,: normal 0, floating 3, modal panel 8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowLevel {
    #[default]
    Normal,
    /// Floating panels; utility panels are floating (only the floating-panel
    /// setter raises a panel, and utility panels set it).
    Floating,
    /// Application-modal panels (the modal panel level).
    ModalPanel,
}

impl WindowLevel {
    /// The design's level number.
    pub fn value(self) -> i32 {
        match self {
            WindowLevel::Normal => 0,
            WindowLevel::Floating => 3,
            WindowLevel::ModalPanel => 8,
        }
    }

    /// Level of a new window of `kind`: utility panels float, application-modal windows without a
    /// parent are modal panels; a child window takes its parent's level (a sheet and a
    /// dialog are ordered with their parent).
    pub fn for_kind(
        kind: cupertino_effects::WindowKind,
        parent: Option<WindowLevel>,
    ) -> WindowLevel {
        use cupertino_effects::WindowKind;
        match (parent, kind) {
            (Some(p), _) => p,
            (None, WindowKind::Utility) => WindowLevel::Floating,
            (None, WindowKind::AppModal) => WindowLevel::ModalPanel,
            (None, _) => WindowLevel::Normal,
        }
    }
}

#[derive(Clone, Debug)]
pub struct App {
    pub id: AppId,
    /// Application identity string (the toolkit-provided application id).
    pub identity: String,
    /// App-level hidden state (no Wayland equivalent: the compositor withholds the windows).
    pub hidden: bool,
    /// Windows of the app, most recently used first.
    pub windows_mru: Vec<WindowId>,
    pub assignment: Assignment,
    /// When the application last became the active one (model seconds); `None` until it was.
    pub activated_at: Option<f64>,
}

/// The controls an application disabled on its own window (`cupertino_app_effects_v1.set_disabled_controls`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisabledControls {
    pub minimize: bool,
    pub zoom: bool,
}

#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub app: AppId,
    pub title: String,
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,
    pub content_hint: Option<Size>,
    pub parent: Option<WindowId>,
    pub modal: bool,
    pub output: OutputId,
    pub workspace: WorkspaceId,
    /// Shown on every workspace of `output`.
    pub sticky: bool,
    pub(crate) sticky_from_app: bool,
    pub level: WindowLevel,
    pub lifecycle: Lifecycle,
    pub disabled: DisabledControls,
}

impl Window {
    /// Whether the window may be minimized: its application did not disable it.
    pub fn can_minimize(&self) -> bool {
        !self.disabled.minimize
    }

    /// Whether the window may zoom: its client does not fix its size (minimum equal to maximum) and did not disable it.
    pub fn can_zoom(&self) -> bool {
        !self.disabled.zoom && self.min_size.zip(self.max_size).is_none_or(|(a, b)| a != b)
    }

    /// Logical frame the client is configured to (the model's notion of "geometry").
    pub fn geometry(&self) -> Rect {
        self.lifecycle.target_frame()
    }
    pub fn state(&self) -> LifecycleState {
        self.lifecycle.state()
    }
    /// The smallest and the largest frame size a resize may reach: the client's minimum (at least 1) and maximum
    /// (unbounded where unset or 0).
    pub fn size_limits(&self) -> (Size, Size) {
        let (min_w, min_h) = self
            .min_size
            .map_or((1.0, 1.0), |m| (m.w.max(1.0), m.h.max(1.0)));
        let max_w = self
            .max_size
            .map(|m| m.w)
            .filter(|v| *v > 0.0)
            .unwrap_or(f64::INFINITY);
        let max_h = self
            .max_size
            .map(|m| m.h)
            .filter(|v| *v > 0.0)
            .unwrap_or(f64::INFINITY);
        (
            Size::new(min_w, min_h),
            Size::new(max_w.max(min_w), max_h.max(min_h)),
        )
    }

    pub fn hints(&self) -> SizeHints {
        SizeHints {
            min: self.min_size,
            max: self.max_size,
            content: self.content_hint,
        }
    }
}
