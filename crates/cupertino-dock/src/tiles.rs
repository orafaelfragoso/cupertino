//! Tile model ("Tile lifecycle": launching → running → attention → quit;
//! minimized windows live in the right section; separators and a recents separator exist).

use cupertino_animation::AnimatedValue;
use cupertino_wm::ids::WindowId;

use crate::bounce::Bounce;
use crate::config::BounceConfig;

/// How long a running dot takes to fade in once its app has started.
pub const RUNNING_FADE_S: f64 = 0.28;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    App,
    Recent,
    MinimizedWindow,
    Separator,
    /// A folder whose files open as a stack over the bar; its key is the folder's path.
    Folder,
    Trash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileLifecycle {
    /// Pinned app that is not running.
    Idle,
    Launching,
    Running,
    Attention,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub id: TileId,
    pub kind: TileKind,
    /// App identity string, recent-item key, or window title key for minimized tiles.
    pub key: String,
    pub window: Option<WindowId>,
    pub title: String,
    /// Kept in the dock when not running.
    pub pinned: bool,
    /// The session file manager's tile: built into the dock, always first, never stored with the
    /// pins, removed or moved ([`crate::dock::Dock::set_built_in`]).
    pub built_in: bool,
    pub running: bool,
    /// When the app's running dot began to appear: it fades in over [`RUNNING_FADE_S`] from then. `None` shows it in
    /// full (an app that was running before the dock was).
    pub(crate) running_since: Option<f64>,
    pub launch: Option<Bounce>,
    /// When a launch that has not produced a running app gives up: an unpinned tile then leaves.
    pub(crate) launch_deadline: Option<f64>,
    pub attention: Option<Bounce>,
    /// Trash only.
    pub full: bool,
    pub(crate) presence: AnimatedValue<f64>,
    pub(crate) leaving: bool,
}

impl Tile {
    pub(crate) fn new(id: TileId, kind: TileKind, key: &str) -> Self {
        Self {
            id,
            kind,
            key: key.to_string(),
            window: None,
            title: String::new(),
            pinned: false,
            built_in: false,
            running: false,
            running_since: None,
            launch: None,
            launch_deadline: None,
            attention: None,
            full: false,
            presence: AnimatedValue::new(1.0),
            leaving: false,
        }
    }

    /// Whether the tile is on its way out (disappear animation running).
    pub fn is_leaving(&self) -> bool {
        self.leaving
    }

    /// Size scale of the appear/disappear animation (1 = fully present).
    pub fn presence(&self, now: f64) -> f64 {
        self.presence.value(now).clamp(0.0, 1.0)
    }

    /// How much of its running dot shows at `now`, from 0 to 1: it fades in once the app has started.
    pub fn indicator_alpha(&self, now: f64) -> f64 {
        self.running_since.map_or(1.0, |since| {
            ((now - since) / RUNNING_FADE_S).clamp(0.0, 1.0)
        })
    }

    pub fn lifecycle(&self, cfg: &BounceConfig, now: f64) -> TileLifecycle {
        if self.attention.is_some_and(|b| b.is_active(cfg, now)) {
            TileLifecycle::Attention
        } else if self.launch.is_some_and(|b| b.is_active(cfg, now)) && !self.running {
            TileLifecycle::Launching
        } else if self.running {
            TileLifecycle::Running
        } else {
            TileLifecycle::Idle
        }
    }

    /// Lift along the bar normal as a fraction of the tile size (evaluated for `cfg.tile_len`).
    pub fn bounce_lift(&self, cfg: &BounceConfig, now: f64) -> f64 {
        let a = self.launch.map_or(0.0, |b| b.offset(cfg, now));
        let b = self.attention.map_or(0.0, |b| b.offset(cfg, now));
        a.max(b)
    }

    /// Lift along the bar normal in points for a tile of length `len`.
    pub fn bounce_lift_pt(&self, cfg: &BounceConfig, len: f64, now: f64) -> f64 {
        let a = self.launch.map_or(0.0, |b| b.lift_pt(cfg, len, now));
        let b = self.attention.map_or(0.0, |b| b.lift_pt(cfg, len, now));
        a.max(b)
    }
}
