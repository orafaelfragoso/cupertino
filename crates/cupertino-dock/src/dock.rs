//! The dock model: tile sections, layout, hit-testing, minimize targets and drag-reorder.
//!
//! Section order along the axis: apps (pinned first, then running unpinned),
//! a separator, recents (optional, followed by a separator of their own),
//! folders, minimized-window tiles, trash.

use std::collections::HashMap;

use crate::ease::{cosine_ease, EasedValue};
use cupertino_animation::{AnimatedValue, CubicBezier, Curve};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::WindowId;

use crate::autohide::{reveal_zone, AutoHide, HideState};
use crate::bounce::{Bounce, BounceKind};
use crate::config::{BounceConfig, DockConfig, Orientation};
use crate::magnify::{
    base_length, fish_ramp_ms, fish_ramp_out_ms, layout_items, FisheyeProfile, LayoutItem,
    MagnificationProfile,
};
use crate::tile_drag::{
    LOOK_S, REMOVE_DELAY_S, REMOVE_FADE_CURVE, REMOVE_FADE_S, REMOVING_OPACITY, SETTLE_S, SLIDE_S,
};
use crate::tiles::{Tile, TileId, TileKind, TileLifecycle};

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedTile {
    pub id: TileId,
    pub kind: TileKind,
    /// Rectangle in screen coordinates, without the bounce lift.
    pub rect: Rect,
    /// Bounce lift in points along the bar normal (away from the screen edge); 0 under Reduce Motion.
    pub lift: f64,
    /// Under Reduce Motion, where a bounce would be from 0 (at rest) to 1 (the top of its highest hop): the tile dims
    /// in place by it instead of lifting. 0 otherwise.
    pub pulse: f64,
    pub lifecycle: TileLifecycle,
    /// Running indicator should be drawn (running tile and indicators enabled).
    pub indicator: bool,
    /// How much of the indicator shows, from 0 to 1: it fades in once its app has started (always 1 under Reduce
    /// Motion).
    pub indicator_alpha: f64,
    pub presence: f64,
}

impl PlacedTile {
    /// The rectangle the tile is drawn at: [`PlacedTile::rect`] raised by its bounce lift.
    pub fn visible_rect(&self, orientation: Orientation) -> Rect {
        let (dx, dy) = orientation.outward();
        self.rect.translate(dx * self.lift, dy * self.lift)
    }
}

/// A tile drawn outside the flow of the bar: the one held by the pointer, the one settling into its
/// slot after a drop, or the one fading out after a removal.
#[derive(Clone, Debug, PartialEq)]
pub struct DraggedTile {
    pub tile: PlacedTile,
    /// Key of the application (or recent item) the icon is drawn for.
    pub key: String,
    /// Opacity of the icon.
    pub opacity: f64,
    /// Opacity of the "Remove" label above the icon.
    pub label: f64,
    /// Opacity of the running indicator under the icon (it comes back as the tile settles).
    pub indicator: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DockLayout {
    pub bar: Rect,
    pub tiles: Vec<PlacedTile>,
    /// The tile being dragged, settling after a drop or fading after a removal: drawn on its own,
    /// free of the bar (it may leave the dock surface).
    pub dragging: Option<DraggedTile>,
    /// Gap kept open for the dragged tile (or reserved for a tile about to appear).
    pub placeholder: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileHit {
    pub id: TileId,
    pub kind: TileKind,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Apps,
    Recents,
}

#[derive(Clone, Debug)]
struct DragState {
    id: TileId,
    section: Section,
    pointer: Point,
    /// Insertion index among the other live tiles of the section.
    insertion: usize,
    /// Index among the live tiles of the section when the drag started.
    from: usize,
    /// Offset from the pointer to the top-left of the dragged icon: it stays where it was grabbed
    /// (the drag window's offset is its origin minus the press point).
    grab: Point,
    /// Size of the dragged icon: the tile's size when the drag began, magnification included.
    size: Point,
    start: f64,
    /// The gap is open: the pointer is over the flow of the dragged tile's own section, within a
    /// tile size of the screen edge. Without it the tile returns where it was.
    inserting: bool,
    /// The pointer is past the removal edge; a drop removes the tile once the delay passed.
    zone: bool,
    over_trash: bool,
    /// 0 to 1: how far the icon has dimmed and the "Remove" label shown.
    look: AnimatedValue<f64>,
}

impl DragState {
    /// A drop at `now` removes the tile: it is in the removal zone for good (the delay is over), or
    /// on the trash.
    fn removes(&self, now: f64) -> bool {
        (self.zone && now - self.start >= REMOVE_DELAY_S) || self.over_trash
    }
}

/// A neighbour's way back to the place it was moved from.
#[derive(Clone, Copy, Debug)]
struct Slide {
    offset: f64,
    start: f64,
}

impl Slide {
    /// How far the tile still lags its place at `now`, in points along the dock axis.
    fn remaining(&self, now: f64) -> f64 {
        self.offset * (1.0 - cosine_ease((now - self.start) / SLIDE_S))
    }

    fn is_active(&self, now: f64) -> bool {
        now < self.start + SLIDE_S
    }
}

/// A dropped tile on its way from where it was released to its slot.
#[derive(Clone, Copy, Debug)]
struct Settle {
    id: TileId,
    from: Rect,
    opacity: f64,
    label: f64,
    start: f64,
}

/// A removed tile fading out where it was released.
#[derive(Clone, Debug)]
struct Departing {
    tile: PlacedTile,
    key: String,
    start: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragOutcome {
    Cancelled,
    Reordered {
        key: String,
        from: usize,
        to: usize,
    },
    /// Dragged out of the dock or dropped on the trash (the "poof" removal).
    Removed {
        key: String,
        kind: TileKind,
    },
}

#[derive(Clone, Copy)]
enum Slot<'a> {
    Tile(&'a Tile),
    Placeholder(f64),
}

const SEP_MAIN: TileId = TileId(u64::MAX);
const SEP_RECENTS: TileId = TileId(u64::MAX - 1);
const TRASH: TileId = TileId(u64::MAX - 2);

pub struct Dock {
    pub cfg: DockConfig,
    pub bounce: BounceConfig,
    pub autohide: AutoHide,
    /// The auto-hide is driven from outside through [`Dock::set_revealed`] (an edge reveal
    /// component owns the dwell timers); `pointer_moved` then leaves it alone.
    pub autohide_external: bool,
    /// An auto-hidden dock shows, as if the pointer were over it, while this holds (the dock has the keyboard).
    pub keep_shown: bool,
    /// Reduce Motion: the magnification comes and goes at once, and bounces (launch, the launch hop, attention) dim
    /// the tile in place instead of lifting it ([`PlacedTile::pulse`]).
    reduce_motion: bool,
    profile: Box<dyn MagnificationProfile>,
    apps: Vec<Tile>,
    recents: Vec<Tile>,
    minimized: Vec<Tile>,
    /// The folders shown as stacks, before the minimized windows.
    folders: Vec<Tile>,
    trash: Tile,
    sep_main: Tile,
    sep_recents: Tile,
    minimized_windows: HashMap<WindowId, String>,
    next_id: u64,
    fish: EasedValue,
    hot: bool,
    hover_axis: Option<f64>,
    hovered: Option<TileId>,
    drag: Option<DragState>,
    /// Where the flow put each tile (at rest, along the dock axis) at the last change of the gap,
    /// and the neighbours still sliding to a new place. Kept only around a drag.
    rest: HashMap<TileId, f64>,
    slides: HashMap<TileId, Slide>,
    settle: Option<Settle>,
    departing: Option<Departing>,
}

fn push_section<'a>(
    out: &mut Vec<Slot<'a>>,
    list: &'a [Tile],
    section: Section,
    drag: Option<&DragState>,
    tile: f64,
) {
    let dragged = drag.filter(|d| d.section == section);
    let mut live = 0usize;
    let mut placed = false;
    let ph = |d: &DragState| Slot::Placeholder(if d.inserting { tile } else { 0.0 });
    for t in list {
        if let Some(d) = dragged {
            if t.id == d.id {
                continue;
            }
            if !t.leaving && live == d.insertion && !placed {
                out.push(ph(d));
                placed = true;
            }
        }
        if !t.leaving {
            live += 1;
        }
        out.push(Slot::Tile(t));
    }
    if let (Some(d), false) = (dragged, placed) {
        out.push(ph(d));
    }
}

fn presence_curve(cfg: &DockConfig) -> Curve {
    Curve::Timed {
        duration: cfg.tile_presence_s,
        bezier: CubicBezier::EASE_IN_EASE_OUT,
    }
}

impl Dock {
    pub fn new(cfg: DockConfig) -> Self {
        let autohide = if cfg.autohide {
            AutoHide::new(cfg.autohide_delay, cfg.autohide_duration)
        } else {
            AutoHide::shown(cfg.autohide_delay, cfg.autohide_duration)
        };
        Self {
            autohide,
            autohide_external: false,
            keep_shown: false,
            reduce_motion: false,
            cfg,
            bounce: BounceConfig::default(),
            profile: Box::new(FisheyeProfile),
            apps: Vec::new(),
            recents: Vec::new(),
            minimized: Vec::new(),
            folders: Vec::new(),
            trash: Tile::new(TRASH, TileKind::Trash, "trash"),
            sep_main: Tile::new(SEP_MAIN, TileKind::Separator, "section"),
            sep_recents: Tile::new(SEP_RECENTS, TileKind::Separator, "recents"),
            minimized_windows: HashMap::new(),
            next_id: 1,
            fish: EasedValue::new(0.0),
            hot: false,
            hover_axis: None,
            hovered: None,
            drag: None,
            rest: HashMap::new(),
            slides: HashMap::new(),
            settle: None,
            departing: None,
        }
    }

    /// Reveal (`shown`) or hide the dock on behalf of an external dwell timer. The dock's own delays
    /// are bypassed: the caller already waited.
    pub fn set_revealed(&mut self, shown: bool, now: f64) {
        self.autohide_external = true;
        self.autohide.delay = 0.0;
        // every call advances the state machine by one step; three reach Showing / Hiding
        for _ in 0..3 {
            self.autohide.update(shown, now);
        }
    }

    /// Turn auto-hide on or off, as the auto-hide shortcut does. Turning it on starts the hide
    /// delay, so the dock slides out unless the pointer is over it; turning it off shows the dock at
    /// once. Callers update the exclusive zone from [`Dock::exclusive_zone`].
    pub fn toggle_autohide(&mut self, now: f64) {
        self.cfg.autohide = !self.cfg.autohide;
        self.autohide = AutoHide::shown(self.autohide.delay, self.autohide.duration);
        if self.cfg.autohide {
            self.autohide.update(false, now);
        }
    }

    /// Follow the session's Reduce Motion option. A magnification under way jumps to where it was going.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
        if on {
            let target = self.fish.target();
            self.fish.set_immediately(target);
        }
    }

    pub fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    /// Replace the magnification profile (the verified fisheye is the default; the raised-cosine
    /// alternative is [`crate::CosineProfile`]).
    pub fn set_profile(&mut self, profile: Box<dyn MagnificationProfile>) {
        self.profile = profile;
    }

    fn alloc(&mut self, kind: TileKind, key: &str) -> Tile {
        let t = Tile::new(TileId(self.next_id), kind, key);
        self.next_id += 1;
        t
    }

    fn appear(&self, mut t: Tile, now: f64) -> Tile {
        t.presence = AnimatedValue::new(0.0);
        t.presence.animate_to(1.0, presence_curve(&self.cfg), now);
        t
    }

    fn fade_out(cfg: &DockConfig, t: &mut Tile, now: f64) {
        if !t.leaving {
            t.leaving = true;
            t.presence.animate_to(0.0, presence_curve(cfg), now);
        }
    }

    fn revive(cfg: &DockConfig, t: &mut Tile, now: f64) {
        if t.leaving {
            t.leaving = false;
            t.presence.animate_to(1.0, presence_curve(cfg), now);
        }
    }

    // ---- model mutation -------------------------------------------------------------------

    fn app_index(&self, identity: &str) -> Option<usize> {
        self.apps
            .iter()
            .position(|t| t.key == identity && !t.leaving)
    }

    fn ensure_app(&mut self, identity: &str, now: f64) -> usize {
        if let Some(i) = self.app_index(identity) {
            return i;
        }
        if let Some(i) = self.apps.iter().position(|t| t.key == identity) {
            Self::revive(&self.cfg, &mut self.apps[i], now);
            return i;
        }
        let t = self.alloc(TileKind::App, identity);
        let t = self.appear(t, now);
        self.apps.push(t);
        self.apps.len() - 1
    }

    /// Where the movable app tiles start: after the built-in tile.
    fn first_movable(&self) -> usize {
        usize::from(self.apps.first().is_some_and(|t| t.built_in))
    }

    /// The built-in first tile, the session file manager with this key: always present, first, kept
    /// when it does not run, never stored with the pins, removed or moved. The design's file
    /// manager tile is not one of the default pins. Its menus,
    /// running and not running, have an empty Options submenu, so no Keep in Dock and no Remove from
    /// Dock. That it is first and cannot be dragged out of first place is.
    /// Call it before [`Dock::set_pinned`].
    pub fn set_built_in(&mut self, key: &str) {
        let mut t = self.alloc(TileKind::App, key);
        t.pinned = true;
        t.built_in = true;
        self.apps.insert(0, t);
    }

    /// Pin (keep in the dock) an app at `index` among the app tiles (end when `None`), never ahead
    /// of the built-in tile, which itself stays where it is.
    pub fn pin(&mut self, identity: &str, index: Option<usize>, now: f64) {
        let i = self.ensure_app(identity, now);
        self.apps[i].pinned = true;
        if let Some(to) = index.filter(|_| !self.apps[i].built_in) {
            let t = self.apps.remove(i);
            let to = to.clamp(self.first_movable(), self.apps.len());
            self.apps.insert(to, t);
        }
    }

    /// Initial pinned set, in order, without animation. A key of the built-in tile is skipped: an
    /// older pins file that holds the file manager shows it once, first.
    pub fn set_pinned(&mut self, identities: &[&str]) {
        for id in identities {
            if self.apps.iter().any(|t| t.built_in && t.key == *id) {
                continue;
            }
            let mut t = self.alloc(TileKind::App, id);
            t.pinned = true;
            self.apps.push(t);
        }
    }

    /// Keys of the pinned applications in display order, for storing the pins ([`crate::conf::store_pins`]).
    pub fn pinned_keys(&self) -> Vec<String> {
        self.apps
            .iter()
            .filter(|t| t.pinned && !t.built_in && !t.leaving)
            .map(|t| t.key.clone())
            .collect()
    }

    pub fn unpin(&mut self, identity: &str, now: f64) {
        if let Some(i) = self.app_index(identity).filter(|i| !self.apps[*i].built_in) {
            self.apps[i].pinned = false;
            if !self.apps[i].running {
                Self::fade_out(&self.cfg, &mut self.apps[i], now);
            }
        }
    }

    /// The user (or the shell) started launching an app: adds its tile if needed and bounces it.
    /// The launch is announced more than once (the dock's own start of the program, the server's
    /// activation token): a second signal while the bounce still runs does not restart it.
    pub fn app_launching(&mut self, identity: &str, now: f64) {
        let i = self.ensure_app(identity, now);
        self.apps[i].launch_deadline = Some(now + self.bounce.launch_timeout);
        let running_bounce = self.apps[i]
            .launch
            .is_some_and(|b| !b.is_stopping() && b.is_active(&self.bounce, now));
        if self.cfg.launch_animation && !running_bounce {
            self.apps[i].launch = Some(Bounce::new(BounceKind::Launch, now));
        }
    }

    /// The first window of an app appeared and nothing announced its launch (a program started
    /// from a terminal, a script or a client that names no application in its activation token):
    /// its tile bounces for the one cycle the stop rule of [`Dock::app_started`] allows. The
    /// reference registers every process that starts, whoever started it.
    pub fn app_appeared(&mut self, identity: &str, now: f64) {
        let announced = self.app_index(identity).is_some_and(|i| {
            self.apps[i]
                .launch
                .is_some_and(|b| b.is_active(&self.bounce, now))
        });
        if !announced {
            self.app_launching(identity, now);
        }
        self.app_started(identity, now);
    }

    /// The app has a running process / first window (launched here or elsewhere). A launch bounce
    /// stops here: the arc in progress completes (the design stops it on the launch-complete
    /// notification, which the first window stands for).
    pub fn app_started(&mut self, identity: &str, now: f64) {
        let i = self.ensure_app(identity, now);
        let cfg = self.bounce.clone();
        let t = &mut self.apps[i];
        if !t.running {
            t.running_since = Some(now);
        }
        t.running = true;
        t.launch_deadline = None;
        if let Some(b) = t.launch.as_mut() {
            b.stop(&cfg, now);
        }
    }

    pub fn app_quit(&mut self, identity: &str, now: f64) {
        let windows: Vec<WindowId> = self
            .minimized_windows
            .iter()
            .filter(|(_, a)| a.as_str() == identity)
            .map(|(w, _)| *w)
            .collect();
        for w in windows {
            self.window_restored(w, now);
        }
        if let Some(i) = self.app_index(identity) {
            let t = &mut self.apps[i];
            t.running = false;
            t.running_since = None;
            t.launch = None;
            t.launch_deadline = None;
            t.attention = None;
            if !t.pinned {
                Self::fade_out(&self.cfg, t, now);
            }
        }
    }

    /// Request attention (critical bounces until cleared; informational bounces a few arcs).
    pub fn attention(&mut self, identity: &str, critical: bool, now: f64) {
        if let Some(i) = self.app_index(identity) {
            let kind = if critical {
                BounceKind::Critical
            } else {
                BounceKind::Informational
            };
            self.apps[i].attention = Some(Bounce::new(kind, now));
        }
    }

    pub fn clear_attention(&mut self, identity: &str, now: f64) {
        let cfg = self.bounce.clone();
        if let Some(i) = self.app_index(identity) {
            if let Some(b) = self.apps[i].attention.as_mut() {
                b.stop(&cfg, now);
            }
        }
    }

    /// A window was minimized: adds a tile for it, or counts it on the app tile when windows
    /// minimize into the app.
    pub fn window_minimized(&mut self, window: WindowId, app: &str, title: &str, now: f64) {
        if self
            .minimized_windows
            .insert(window, app.to_string())
            .is_some()
        {
            return;
        }
        if self.cfg.minimize_to_app {
            return;
        }
        let mut t = self.alloc(TileKind::MinimizedWindow, app);
        t.window = Some(window);
        t.title = title.to_string();
        let t = self.appear(t, now);
        self.minimized.push(t);
    }

    /// The window was restored or closed.
    pub fn window_restored(&mut self, window: WindowId, now: f64) {
        self.minimized_windows.remove(&window);
        if let Some(t) = self.minimized.iter_mut().find(|t| t.window == Some(window)) {
            Self::fade_out(&self.cfg, t, now);
        }
    }

    pub fn minimized_count(&self, app: &str) -> usize {
        self.minimized_windows
            .values()
            .filter(|a| a.as_str() == app)
            .count()
    }

    /// Replace the recents list (most recent first), truncated to `recents_max`.
    pub fn set_recents(&mut self, items: &[&str], now: f64) {
        let items: Vec<&str> = items.iter().copied().take(self.cfg.recents_max).collect();
        let old = std::mem::take(&mut self.recents);
        let mut new = Vec::new();
        for key in &items {
            match old.iter().find(|t| t.key == *key && !t.leaving) {
                Some(t) => new.push(t.clone()),
                None => {
                    let t = self.alloc(TileKind::Recent, key);
                    let t = self.appear(t, now);
                    new.push(t);
                }
            }
        }
        for mut t in old {
            if !items.contains(&t.key.as_str()) || t.leaving {
                Self::fade_out(&self.cfg, &mut t, now);
                new.push(t);
            }
        }
        // leaving tiles keep a stable position at the end of their section
        new.sort_by_key(|t| t.leaving);
        self.recents = new;
    }

    /// The folders the bar shows as stacks, by path, in order. A folder already shown keeps its tile.
    pub fn set_folders(&mut self, paths: &[&str]) {
        let old = std::mem::take(&mut self.folders);
        self.folders = paths
            .iter()
            .map(|path| match old.iter().find(|t| t.key == *path) {
                Some(t) => t.clone(),
                None => self.alloc(TileKind::Folder, path),
            })
            .collect();
    }

    pub fn set_trash_full(&mut self, full: bool) {
        self.trash.full = full;
    }

    /// Drop tiles whose disappear animation has finished.
    pub fn tick(&mut self, now: f64) {
        // a launch that never produced a running app: an unpinned tile has nothing left to show
        for t in self
            .apps
            .iter_mut()
            .filter(|t| t.launch_deadline.is_some_and(|d| now >= d))
        {
            t.launch_deadline = None;
            t.launch = None;
            if !t.pinned {
                Self::fade_out(&self.cfg, t, now);
            }
        }
        for v in [&mut self.apps, &mut self.recents, &mut self.minimized] {
            v.retain(|t| !(t.leaving && !t.presence.is_animating(now)));
        }
        self.sync_look(now);
        self.slides.retain(|_, s| s.is_active(now));
        if self.settle.is_some_and(|s| now >= s.start + SETTLE_S) {
            self.settle = None;
        }
        if self
            .departing
            .as_ref()
            .is_some_and(|d| now >= d.start + REMOVE_FADE_S)
        {
            self.departing = None;
        }
        if self.drag.is_none() && self.slides.is_empty() {
            self.rest.clear();
        }
    }

    pub fn wants_tick(&self, now: f64) -> bool {
        let tiles = self.apps.iter().chain(&self.recents).chain(&self.minimized);
        let animating = tiles
            .clone()
            .any(|t| t.presence.is_animating(now) || t.bounce_lift(&self.bounce, now) > 0.0)
            || tiles
                .clone()
                .any(|t| t.launch.is_some_and(|b| b.is_active(&self.bounce, now)))
            || tiles
                .clone()
                .any(|t| t.attention.is_some_and(|b| b.is_active(&self.bounce, now)))
            || self.apps.iter().any(|t| t.launch_deadline.is_some())
            || self
                .apps
                .iter()
                .any(|t| t.running && !self.reduce_motion && t.indicator_alpha(now) < 1.0);
        // the auto-hide state machine only advances when it is updated, so a pending step or a
        // delay being waited out needs frames too
        let autohide_pending = self.cfg.autohide
            && !matches!(self.autohide.state, HideState::Shown | HideState::Hidden);
        // a drag also waits for the delay before a drop can remove its tile
        let dragging = self.drag.as_ref().is_some_and(|d| {
            d.look.is_animating(now) || (d.zone && now - d.start < REMOVE_DELAY_S)
        });
        let moving = self.slides.values().any(|s| s.is_active(now))
            || self.settle.is_some_and(|s| now < s.start + SETTLE_S)
            || self
                .departing
                .as_ref()
                .is_some_and(|d| now < d.start + REMOVE_FADE_S);
        animating
            || dragging
            || moving
            || autohide_pending
            || self.fish.is_animating(now)
            || self.autohide.offset.is_animating(now)
    }

    // ---- queries ---------------------------------------------------------------------------

    pub fn tile(&self, id: TileId) -> Option<&Tile> {
        self.flow_tiles().into_iter().find(|t| t.id == id)
    }

    pub fn app_tile(&self, identity: &str) -> Option<&Tile> {
        self.apps.iter().find(|t| t.key == identity && !t.leaving)
    }

    /// Tiles in display order (including separators and the trash; leaving tiles included).
    pub fn tiles(&self) -> Vec<&Tile> {
        self.flow_tiles()
    }

    pub fn hovered(&self) -> Option<TileId> {
        self.hovered
    }

    pub fn magnification_amount(&self, now: f64) -> f64 {
        self.fish.value(now).clamp(0.0, 1.0)
    }

    fn flow_tiles(&self) -> Vec<&Tile> {
        self.slots(None, false)
            .into_iter()
            .filter_map(|s| if let Slot::Tile(t) = s { Some(t) } else { None })
            .collect()
    }

    fn live_recents(&self) -> bool {
        self.cfg.show_recents && self.recents.iter().any(|t| !t.leaving)
    }

    /// Flow of the dock. When dragging, the dragged tile is replaced by a placeholder at the
    /// insertion index. `ghost` appends a placeholder to the minimized section.
    fn slots(&self, drag: Option<&DragState>, ghost: bool) -> Vec<Slot<'_>> {
        let tile = self.cfg.tile_size;
        let mut out: Vec<Slot> = Vec::new();
        push_section(&mut out, &self.apps, Section::Apps, drag, tile);
        out.push(Slot::Tile(&self.sep_main));
        if self.cfg.show_recents {
            push_section(&mut out, &self.recents, Section::Recents, drag, tile);
        }
        // the trash always follows, so recents always have a separator after them
        if self.live_recents() {
            out.push(Slot::Tile(&self.sep_recents));
        }
        out.extend(self.folders.iter().map(Slot::Tile));
        out.extend(self.minimized.iter().map(Slot::Tile));
        if ghost {
            out.push(Slot::Placeholder(tile));
        }
        out.push(Slot::Tile(&self.trash));
        out
    }

    fn slot_item(&self, s: &Slot<'_>, now: Option<f64>) -> LayoutItem {
        match s {
            Slot::Placeholder(e) => LayoutItem {
                extent: *e,
                magnify: false,
            },
            Slot::Tile(t) => {
                let base = match t.kind {
                    TileKind::Separator => self.cfg.separator_extent(),
                    _ => self.cfg.tile_size,
                };
                let scale = match now {
                    Some(n) => t.presence(n),
                    None if t.leaving => 0.0,
                    None => 1.0,
                };
                LayoutItem {
                    extent: base * scale,
                    magnify: t.kind != TileKind::Separator,
                }
            }
        }
    }

    fn axis_centre(&self, screen: Rect) -> f64 {
        if self.cfg.orientation.is_horizontal() {
            screen.cx()
        } else {
            screen.cy()
        }
    }

    fn pointer_axis(&self, p: Point) -> f64 {
        if self.cfg.orientation.is_horizontal() {
            p.x
        } else {
            p.y
        }
    }

    /// Distance from the screen edge that windows keep clear for the dock: the bar and its margin,
    /// nothing while the dock hides itself.
    pub fn exclusive_zone(&self) -> f64 {
        if self.cfg.autohide {
            return 0.0;
        }
        self.bar_depth()
    }

    /// Depth of the shown bar across the dock axis: the margin, the padding and a tile at rest.
    pub fn bar_depth(&self) -> f64 {
        self.cfg.bar_thickness() + self.cfg.edge_margin
    }

    /// Extent across the dock axis that draws the dock at its fullest: the margin, the padding and
    /// a tile at the magnified size, plus the highest bounce.
    pub fn surface_extent(&self) -> f64 {
        let tile = self.cfg.tile_size;
        let size = if self.cfg.magnification {
            self.cfg.large_size.max(tile)
        } else {
            tile
        };
        let lift = [
            self.bounce.launch_lift.pt(tile),
            self.bounce.attention_lift.pt(tile),
            self.bounce.height * tile,
        ]
        .into_iter()
        .fold(0.0, f64::max);
        (self.cfg.edge_margin + self.cfg.bar_padding() + size + lift).ceil()
    }

    /// The parts of `screen` that take pointer input at `now`: the bar and its tiles, or only the
    /// reveal strip while the dock is hidden.
    pub fn input_rects(&self, screen: Rect, now: f64) -> Vec<Rect> {
        if self.is_hidden(now) {
            return vec![reveal_zone(
                self.cfg.orientation,
                screen,
                self.cfg.reveal_thickness,
            )];
        }
        let layout = self.layout(screen, now);
        let tiles = layout
            .tiles
            .iter()
            .filter(|t| t.kind != TileKind::Separator && t.presence > 0.0)
            .map(|t| t.visible_rect(self.cfg.orientation));
        std::iter::once(layout.bar).chain(tiles).collect()
    }

    /// Auto-hide is on and the bar has slid more than halfway out.
    fn is_hidden(&self, now: f64) -> bool {
        self.cfg.autohide && self.autohide.hidden_fraction(now) > 0.5
    }

    /// Unmagnified length of the dock flow at `now`.
    pub fn unmagnified_length(&self, now: f64) -> f64 {
        let items: Vec<LayoutItem> = self
            .slots(self.drag.as_ref(), false)
            .iter()
            .map(|s| self.slot_item(s, Some(now)))
            .collect();
        base_length(&self.cfg, &items)
    }

    fn effective_hover(&self, now: f64) -> Option<f64> {
        if self.drag.is_some() {
            return None;
        }
        match self.hover_axis {
            Some(c) if self.hot || self.fish.value(now) > 1e-6 => Some(c),
            _ => None,
        }
    }

    /// Current layout in screen coordinates.
    pub fn layout(&self, screen: Rect, now: f64) -> DockLayout {
        let hide = if self.cfg.autohide {
            self.autohide.hidden_fraction(now)
        } else {
            0.0
        };
        let drag = self.drag.clone();
        self.build(
            screen,
            self.effective_hover(now),
            self.fish.value(now),
            hide,
            Some(now),
            drag.as_ref(),
            false,
        )
    }

    /// Layout at rest (no magnification, shown, appear/disappear animations completed) with an
    /// optional extra minimized slot; used for minimize targets.
    fn rest_layout(&self, screen: Rect, ghost: bool) -> DockLayout {
        self.build(screen, None, 0.0, 0.0, None, None, ghost)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        screen: Rect,
        cursor: Option<f64>,
        fish: f64,
        hide: f64,
        now: Option<f64>,
        drag: Option<&DragState>,
        ghost: bool,
    ) -> DockLayout {
        let cfg = &self.cfg;
        let slots = self.slots(drag, ghost);
        let items: Vec<LayoutItem> = slots.iter().map(|s| self.slot_item(s, now)).collect();
        let mut frames = layout_items(cfg, &items, cursor, fish, self.profile.as_ref());
        // a neighbour of the gap lags the place the flow gave it, and catches up
        if let Some(n) = now {
            for (slot, f) in slots.iter().zip(frames.iter_mut()) {
                if let Slot::Tile(t) = slot {
                    f.pos += self.slides.get(&t.id).map_or(0.0, |s| s.remaining(n));
                }
            }
        }
        let centre = self.axis_centre(screen);
        let thickness = cfg.bar_thickness();
        let padding = cfg.bar_padding();
        let edge = cfg.edge_padding();
        let first = frames.first().map_or(0.0, |f| f.pos);
        let last = frames.last().map_or(0.0, |f| f.pos + f.size);
        let (a0, a1) = (centre + first - padding, centre + last + padding);
        let slide = hide * (thickness + cfg.edge_margin);
        let bar = match cfg.orientation {
            Orientation::Bottom => Rect::new(
                a0,
                screen.bottom() - cfg.edge_margin - thickness + slide,
                a1 - a0,
                thickness,
            ),
            Orientation::Left => {
                Rect::new(screen.x + cfg.edge_margin - slide, a0, thickness, a1 - a0)
            }
            Orientation::Right => Rect::new(
                screen.right() - cfg.edge_margin - thickness + slide,
                a0,
                thickness,
                a1 - a0,
            ),
        };
        let place = |axis: f64, size: f64, cross: f64| -> Rect {
            match cfg.orientation {
                Orientation::Bottom => Rect::new(axis, bar.bottom() - edge - cross, size, cross),
                Orientation::Left => Rect::new(bar.x + edge, axis, cross, size),
                Orientation::Right => Rect::new(bar.right() - edge - cross, axis, cross, size),
            }
        };
        let at = now.unwrap_or(0.0);
        let mut tiles = Vec::new();
        let mut placeholder = None;
        for (slot, f) in slots.iter().zip(&frames) {
            let axis = centre + f.pos;
            match slot {
                Slot::Placeholder(_) => placeholder = Some(place(axis, f.size, cfg.tile_size)),
                Slot::Tile(t) => {
                    let cross = if t.kind == TileKind::Separator {
                        cfg.tile_size
                    } else {
                        f.size
                    };
                    tiles.push(self.placed(t, place(axis, f.size, cross), now.map(|_| at)));
                }
            }
        }
        let mut dragging = drag.and_then(|d| {
            let t = self
                .apps
                .iter()
                .chain(&self.recents)
                .find(|t| t.id == d.id)?;
            let rect = self.dragged_rect(d);
            let look = d.look.value(at);
            Some(DraggedTile {
                tile: self.placed(t, rect, now),
                key: t.key.clone(),
                opacity: 1.0 - look * (1.0 - REMOVING_OPACITY),
                label: look,
                indicator: 0.0,
            })
        });
        if dragging.is_none() {
            if let Some(n) = now {
                dragging = self
                    .settling_tile(&mut tiles, n)
                    .or_else(|| self.departing_tile(n));
            }
        }
        DockLayout {
            bar,
            tiles,
            dragging,
            placeholder,
        }
    }

    fn placed(&self, t: &Tile, rect: Rect, now: Option<f64>) -> PlacedTile {
        let n = now.unwrap_or(0.0);
        let lift = if now.is_some() && !self.reduce_motion {
            t.bounce_lift_pt(&self.bounce, self.cfg.tile_size, n)
        } else {
            0.0
        };
        let pulse = if now.is_some() && self.reduce_motion {
            let shape =
                |b: Option<crate::bounce::Bounce>| b.map_or(0.0, |b| b.shape(&self.bounce, n));
            shape(t.launch).max(shape(t.attention)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        PlacedTile {
            id: t.id,
            kind: t.kind,
            rect,
            lift,
            pulse,
            lifecycle: t.lifecycle(&self.bounce, n),
            indicator: self.cfg.show_running_indicators && t.running && t.kind == TileKind::App,
            indicator_alpha: match now {
                Some(n) if !self.reduce_motion => t.indicator_alpha(n),
                _ => 1.0,
            },
            presence: now.map_or(1.0, |n| t.presence(n)),
        }
    }

    fn cross_range(&self, bar: &Rect, r: &Rect) -> (f64, f64) {
        match self.cfg.orientation {
            Orientation::Bottom => (bar.y.min(r.y), bar.bottom()),
            Orientation::Left => (bar.x, bar.right().max(r.right())),
            Orientation::Right => (bar.x.min(r.x), bar.right()),
        }
    }

    fn hit_in(&self, layout: &DockLayout, p: Point) -> Option<TileHit> {
        let horizontal = self.cfg.orientation.is_horizontal();
        for (index, t) in layout.tiles.iter().enumerate() {
            if t.presence <= 0.0 {
                continue;
            }
            if t.rect.contains(p) {
                return Some(TileHit {
                    id: t.id,
                    kind: t.kind,
                    index,
                });
            }
            let (lo, hi) = self.cross_range(&layout.bar, &t.rect);
            // a tile's slot takes half the gap on either side, so the pointer between two tiles is on one of them
            let half_gap = self.cfg.gap() / 2.0;
            let (axis, cross, a0, a1) = if horizontal {
                (p.x, p.y, t.rect.x - half_gap, t.rect.right() + half_gap)
            } else {
                (p.y, p.x, t.rect.y - half_gap, t.rect.bottom() + half_gap)
            };
            if axis >= a0 && axis < a1 && cross >= lo && cross < hi {
                return Some(TileHit {
                    id: t.id,
                    kind: t.kind,
                    index,
                });
            }
        }
        None
    }

    /// Tile under `pointer` in the current (magnified) layout.
    pub fn hit_test(&self, pointer: Point, screen: Rect, now: f64) -> Option<TileHit> {
        self.hit_in(&self.layout(screen, now), pointer)
    }

    fn over_dock(&self, layout: &DockLayout, p: Point) -> bool {
        layout.bar.contains(p)
            || layout
                .tiles
                .iter()
                .any(|t| t.kind != TileKind::Separator && t.rect.contains(p))
    }

    /// Feed the pointer position (`None` when it left the output). Drives the magnification ramp,
    /// the hovered tile and auto-hide. Ignored while a tile is being dragged.
    pub fn pointer_moved(&mut self, pointer: Option<Point>, screen: Rect, now: f64) {
        if self.drag.is_some() {
            return;
        }
        let layout = self.layout(screen, now);
        let over = pointer.is_some_and(|p| !self.is_hidden(now) && self.over_dock(&layout, p));
        if self.cfg.autohide && !self.autohide_external {
            let zone = reveal_zone(self.cfg.orientation, screen, self.cfg.reveal_thickness);
            let hot = self.keep_shown || pointer.is_some_and(|p| over || zone.contains(p));
            self.autohide.update(hot, now);
        }
        if let (true, Some(p)) = (over, pointer) {
            let base = self.unmagnified_length(now);
            self.hover_axis = Some(self.pointer_axis(p) - self.axis_centre(screen) + base / 2.0);
        }
        if over != self.hot {
            self.hot = over;
            let amount = self.fish.value(now) * (self.cfg.large_size - self.cfg.tile_size).max(0.0);
            let ms = if self.reduce_motion {
                0.0
            } else if over {
                fish_ramp_ms(&self.cfg, amount, false)
            } else {
                fish_ramp_out_ms(&self.cfg, amount, false)
            };
            self.fish
                .animate_to(if over { 1.0 } else { 0.0 }, ms / 1000.0, now);
        }
        self.hovered = pointer
            .filter(|_| over)
            .and_then(|p| self.hit_in(&self.layout(screen, now), p))
            .map(|h| h.id);
    }

    // ---- minimize target ---------------------------------------------------------------------

    /// Rectangle a minimizing window should fly to (the dock at rest, shown): the window's own tile
    /// when it exists, the app tile when windows minimize into the app, otherwise the slot where
    /// the new minimized tile will appear. `None` when the app has no tile and per-app minimizing
    /// is on.
    pub fn minimize_target(&self, window: WindowId, app: &str, screen: Rect) -> Option<Rect> {
        if self.cfg.minimize_to_app {
            let t = self.app_tile(app)?;
            return self
                .rest_layout(screen, false)
                .tiles
                .iter()
                .find(|p| p.id == t.id)
                .map(|p| p.rect);
        }
        if let Some(t) = self
            .minimized
            .iter()
            .find(|t| t.window == Some(window) && !t.leaving)
        {
            return self
                .rest_layout(screen, false)
                .tiles
                .iter()
                .find(|p| p.id == t.id)
                .map(|p| p.rect);
        }
        self.rest_layout(screen, true).placeholder
    }

    /// Rectangle (dock at rest, shown) of the slot the tile of the next minimized window takes.
    pub fn next_minimized_slot(&self, screen: Rect) -> Rect {
        self.rest_layout(screen, true)
            .placeholder
            .expect("a layout with a ghost slot has a placeholder")
    }

    // ---- drag and reorder --------------------------------------------------------------------

    fn section_of(&self, id: TileId) -> Option<(Section, usize)> {
        let live = |list: &[Tile]| list.iter().filter(|t| !t.leaving).position(|t| t.id == id);
        live(&self.apps)
            .map(|i| (Section::Apps, i))
            .or_else(|| live(&self.recents).map(|i| (Section::Recents, i)))
    }

    /// Where the dragged icon is drawn: where it was grabbed, relative to the pointer.
    fn dragged_rect(&self, d: &DragState) -> Rect {
        Rect::new(
            d.pointer.x + d.grab.x,
            d.pointer.y + d.grab.y,
            d.size.x,
            d.size.y,
        )
    }

    /// Start dragging an app or recent tile pressed at `press`, the pointer now at `pointer`
    /// (more than the drag threshold away). The icon keeps the place it was grabbed at, at the
    /// size it had; its slot becomes the gap. Magnification is released.
    pub fn begin_drag(
        &mut self,
        id: TileId,
        press: Point,
        pointer: Point,
        screen: Rect,
        now: f64,
    ) -> bool {
        let Some((section, from)) = self.section_of(id) else {
            return false;
        };
        if self.tile(id).is_some_and(|t| t.built_in) {
            return false;
        }
        let Some(rect) = self
            .layout(screen, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.rect)
        else {
            return false;
        };
        let look = AnimatedValue::new(0.0);
        self.drag = Some(DragState {
            id,
            section,
            pointer,
            insertion: from,
            from,
            grab: Point::new(rect.x - press.x, rect.y - press.y),
            size: Point::new(rect.w, rect.h),
            start: now,
            inserting: true,
            zone: false,
            over_trash: false,
            look,
        });
        self.settle = None;
        self.rest = self.rest_positions(screen);
        self.slides.clear();
        self.hot = false;
        let ms = if self.reduce_motion {
            0.0
        } else {
            fish_ramp_ms(&self.cfg, 0.0, true)
        };
        self.fish.animate_to(0.0, ms / 1000.0, now);
        self.drag_move(pointer, screen, now);
        true
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether releasing at `now` would remove the dragged tile: the pointer is past the removal
    /// edge and the delay is over, or it is on the trash.
    pub fn drag_will_remove(&self, now: f64) -> bool {
        self.drag.as_ref().is_some_and(|d| d.removes(now))
    }

    /// How far the dragged icon has dimmed and its "Remove" label shown, 0 to 1.
    pub fn drag_look(&self, now: f64) -> f64 {
        self.drag.as_ref().map_or(0.0, |d| d.look.value(now))
    }

    /// Where along the dock axis the flow puts each tile at rest, with the drag as it is.
    fn rest_positions(&self, screen: Rect) -> HashMap<TileId, f64> {
        let horizontal = self.cfg.orientation.is_horizontal();
        let drag = self.drag.clone();
        let rest = self.build(screen, None, 0.0, 0.0, None, drag.as_ref(), false);
        rest.tiles
            .iter()
            .map(|t| (t.id, if horizontal { t.rect.x } else { t.rect.y }))
            .collect()
    }

    /// The gap moved: every tile whose place changed slides from where it is drawn to the new one.
    fn retarget_slides(&mut self, screen: Rect, now: f64) {
        let next = self.rest_positions(screen);
        for (id, to) in &next {
            let Some(old) = self.rest.get(id) else {
                continue;
            };
            if (old - to).abs() > 1e-6 {
                let lag = self.slides.get(id).map_or(0.0, |s| s.remaining(now));
                self.slides.insert(
                    *id,
                    Slide {
                        offset: lag + old - to,
                        start: now,
                    },
                );
            }
        }
        self.rest = next;
    }

    /// The pointer moved with a tile held: where the gap goes, whether it is open, whether a drop
    /// would remove the tile.
    pub fn drag_move(&mut self, pointer: Point, screen: Rect, now: f64) {
        let Some(mut d) = self.drag.clone() else {
            return;
        };
        d.pointer = pointer;
        let axis = self.pointer_axis(pointer);
        let horizontal = self.cfg.orientation.is_horizontal();
        let span = |r: &Rect| {
            if horizontal {
                (r.x, r.right())
            } else {
                (r.y, r.bottom())
            }
        };
        let current = self.layout(screen, now);
        let rest = self.build(screen, None, 0.0, 0.0, None, None, false);

        // the tile the pointer is over decides the index: before its middle the gap goes before
        // it, after its middle after it, on the tiles as they are drawn now
        let list = if d.section == Section::Apps {
            &self.apps
        } else {
            &self.recents
        };
        let ids: Vec<TileId> = list
            .iter()
            .filter(|t| !t.leaving && t.id != d.id)
            .map(|t| t.id)
            .collect();
        let mut insertion = ids.len();
        for (k, id) in ids.iter().enumerate() {
            let Some(r) = current
                .tiles
                .iter()
                .find(|p| p.id == *id)
                .map(|p| span(&p.rect))
            else {
                continue;
            };
            if axis < r.0 {
                insertion = k;
                break;
            }
            if axis <= r.1 {
                insertion = if axis >= (r.0 + r.1) / 2.0 { k + 1 } else { k };
                break;
            }
        }
        // nothing goes ahead of the built-in tile
        d.insertion = if d.section == Section::Apps {
            insertion.max(self.first_movable())
        } else {
            insertion
        };

        // the gap is open while the pointer is within a tile of the screen edge and over the flow
        // of the section the tile came from
        let tile = self.cfg.tile_size;
        let near_edge = match self.cfg.orientation {
            Orientation::Bottom => pointer.y >= screen.bottom() - tile,
            Orientation::Left => pointer.x <= screen.x + tile,
            Orientation::Right => pointer.x >= screen.right() - tile,
        };
        let start_of = |kind: TileKind| {
            rest.tiles
                .iter()
                .find(|p| p.kind == kind)
                .map(|p| span(&p.rect))
        };
        let separators: Vec<(f64, f64)> = rest
            .tiles
            .iter()
            .filter(|p| p.kind == TileKind::Separator)
            .map(|p| span(&p.rect))
            .collect();
        let trash_start = start_of(TileKind::Trash).map_or(f64::INFINITY, |r| r.0);
        let (lower, upper) = match d.section {
            Section::Apps => (
                f64::NEG_INFINITY,
                separators.first().map_or(trash_start, |s| s.0),
            ),
            Section::Recents => (
                separators.first().map_or(f64::NEG_INFINITY, |s| s.1),
                separators.get(1).map_or(trash_start, |s| s.0),
            ),
        };
        d.inserting = near_edge && axis >= lower && axis <= upper;

        // the removal edge is a bar thickness beyond the bar, away from the screen edge
        let thickness = self.cfg.bar_thickness();
        d.zone = match self.cfg.orientation {
            Orientation::Bottom => pointer.y < rest.bar.y - thickness,
            Orientation::Left => pointer.x > rest.bar.right() + thickness,
            Orientation::Right => pointer.x < rest.bar.x - thickness,
        };
        d.over_trash = rest
            .tiles
            .iter()
            .any(|p| p.kind == TileKind::Trash && p.rect.contains(pointer));
        self.drag = Some(d);
        self.sync_look(now);
        self.retarget_slides(screen, now);
    }

    /// The icon dims and the label shows once a drop would remove the tile by the pointer's place
    /// alone, and go back when it no longer would.
    fn sync_look(&mut self, now: f64) {
        let Some(d) = self.drag.as_mut() else { return };
        let target = if d.zone && now - d.start >= REMOVE_DELAY_S {
            1.0
        } else {
            0.0
        };
        if d.look.target() != target {
            d.look.animate_to(
                target,
                Curve::Timed {
                    duration: LOOK_S,
                    bezier: CubicBezier::EASE_IN_EASE_OUT,
                },
                now,
            );
        }
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.rest.clear();
        self.slides.clear();
    }

    /// Finish the drag and apply the reorder or removal. A removal fades the icon out where it was
    /// released; anything else settles the icon into its slot.
    pub fn end_drag(&mut self, screen: Rect, now: f64) -> DragOutcome {
        let Some(d) = self.drag.take() else {
            return DragOutcome::Cancelled;
        };
        let from_rect = self.dragged_rect(&d);
        let list = if d.section == Section::Apps {
            &mut self.apps
        } else {
            &mut self.recents
        };
        let Some(pos) = list.iter().position(|t| t.id == d.id) else {
            return DragOutcome::Cancelled;
        };
        let key = list[pos].key.clone();
        let kind = list[pos].kind;
        if d.removes(now) {
            let t = &mut list[pos];
            if d.section == Section::Apps {
                t.pinned = false;
            }
            if d.section == Section::Recents || !t.running {
                // its slot was closed since the pointer left the dock: it goes at once
                t.presence = AnimatedValue::new(0.0);
                t.leaving = true;
            }
            let tile = PlacedTile {
                id: d.id,
                kind,
                rect: from_rect,
                lift: 0.0,
                pulse: 0.0,
                lifecycle: TileLifecycle::Idle,
                indicator: false,
                indicator_alpha: 1.0,
                presence: 1.0,
            };
            self.departing = Some(Departing {
                tile,
                key: key.clone(),
                start: now,
            });
            self.retarget_slides(screen, now);
            return DragOutcome::Removed { key, kind };
        }
        let look = d.look.value(now);
        self.settle = Some(Settle {
            id: d.id,
            from: from_rect,
            opacity: 1.0 - look * (1.0 - REMOVING_OPACITY),
            label: look,
            start: now,
        });
        let outcome = if !d.inserting || d.insertion == d.from {
            DragOutcome::Cancelled
        } else {
            let t = list.remove(pos);
            let live_before: Vec<usize> = list
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.leaving)
                .map(|(i, _)| i)
                .collect();
            let at = live_before.get(d.insertion).copied().unwrap_or(list.len());
            list.insert(at, t);
            DragOutcome::Reordered {
                key,
                from: d.from,
                to: d.insertion,
            }
        };
        self.retarget_slides(screen, now);
        outcome
    }

    pub fn drag_insertion(&self) -> Option<usize> {
        self.drag.as_ref().map(|d| d.insertion)
    }

    /// The tile that settles into its slot after a drop, taken out of `tiles`, while it does.
    fn settling_tile(&self, tiles: &mut Vec<PlacedTile>, now: f64) -> Option<DraggedTile> {
        let s = self
            .settle
            .filter(|s| (s.start..s.start + SETTLE_S).contains(&now))?;
        let slot = tiles.remove(tiles.iter().position(|t| t.id == s.id)?);
        let e = cosine_ease((now - s.start) / SETTLE_S);
        let lerp = |a: f64, b: f64| a + (b - a) * e;
        let rect = Rect::new(
            lerp(s.from.x, slot.rect.x),
            lerp(s.from.y, slot.rect.y),
            lerp(s.from.w, slot.rect.w),
            lerp(s.from.h, slot.rect.h),
        );
        let key = self
            .apps
            .iter()
            .chain(&self.recents)
            .find(|t| t.id == s.id)?
            .key
            .clone();
        Some(DraggedTile {
            indicator: if slot.indicator { e } else { 0.0 },
            tile: PlacedTile { rect, ..slot },
            key,
            opacity: lerp(s.opacity, 1.0),
            label: s.label * (1.0 - e),
        })
    }

    /// The tile that fades out after a removal, while it does.
    fn departing_tile(&self, now: f64) -> Option<DraggedTile> {
        let d = self
            .departing
            .as_ref()
            .filter(|d| (d.start..d.start + REMOVE_FADE_S).contains(&now))?;
        let fade = 1.0 - REMOVE_FADE_CURVE.eval((now - d.start) / REMOVE_FADE_S);
        Some(DraggedTile {
            tile: d.tile.clone(),
            key: d.key.clone(),
            opacity: fade,
            label: fade,
            indicator: 0.0,
        })
    }
}
