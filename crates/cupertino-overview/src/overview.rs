//! Overview model: modes, windows, progress-driven transition, selection and keyboard navigation.

use std::collections::HashMap;

use cupertino_animation::{AnimatedValue, CubicBezier, Curve, SpringParams};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::{AppId, WindowId, WorkspaceId};

use crate::bar::{BarHit, WorkspaceBar};
use crate::config::OverviewConfig;
use crate::layout::{layout, layout_row, reading_order};
use crate::transition::Transition;

/// Which windows are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewMode {
    /// Every window of the current workspace, plus the workspaces bar.
    Overview,
    /// One app's windows, with its minimized windows in a lower row.
    AppOverview(AppId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverviewWindow {
    pub id: WindowId,
    pub app: AppId,
    pub title: String,
    /// Where the window is on the desktop (for minimized windows: its dock tile, the point the
    /// thumbnail flies in from).
    pub from: Rect,
    /// Size used for the layout (the window's real size; differs from `from` when minimized).
    pub size: Rect,
    pub minimized: bool,
}

impl OverviewWindow {
    pub fn new(id: WindowId, app: AppId, title: &str, frame: Rect) -> Self {
        Self {
            id,
            app,
            title: title.to_string(),
            from: frame,
            size: frame,
            minimized: false,
        }
    }

    /// A minimized window: `size` is its real frame, `dock_rect` its dock tile.
    pub fn minimized(id: WindowId, app: AppId, title: &str, size: Rect, dock_rect: Rect) -> Self {
        Self {
            id,
            app,
            title: title.to_string(),
            from: dock_rect,
            size,
            minimized: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Next,
    Previous,
    Activate,
    Cancel,
}

/// Result of an input event that the compositor must act upon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewAction {
    None,
    /// Leave the overview, activating this window (minimized windows are restored).
    Select(WindowId),
    /// Leave the overview without selecting anything.
    Close,
    SwitchWorkspace(WorkspaceId),
    AddWorkspace,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWindow {
    pub id: WindowId,
    pub frame: Rect,
    pub title_opacity: f64,
    pub minimized: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Highlight {
    pub window: WindowId,
    pub rect: Rect,
    pub opacity: f64,
}

pub struct Overview {
    pub cfg: OverviewConfig,
    pub bar: WorkspaceBar,
    area: Rect,
    mode: OverviewMode,
    all: Vec<OverviewWindow>,
    /// Indices into `all` of the windows shown in the current mode.
    shown: Vec<usize>,
    to: Vec<Rect>,
    prev_to: Vec<Rect>,
    reflow: AnimatedValue<f64>,
    /// The open/close transition; the display server drives its twin, this one serves the layout.
    pub transition: Transition,
    selection: Option<WindowId>,
    keyboard_active: bool,
    last_pointer: Option<Point>,
    highlight_rect: AnimatedValue<[f64; 4]>,
    highlight_opacity: AnimatedValue<f64>,
    /// Counts the layouts, so a caller can tell the final frames changed.
    revision: u64,
}

impl Overview {
    /// `area` is the output rectangle available to the overview.
    pub fn new(
        cfg: OverviewConfig,
        area: Rect,
        mode: OverviewMode,
        windows: Vec<OverviewWindow>,
    ) -> Self {
        let bar = WorkspaceBar::new(cfg.bar.clone(), area, area);
        let transition = Transition::new(&cfg);
        let mut o = Self {
            cfg,
            bar,
            area,
            mode,
            all: windows,
            shown: Vec::new(),
            to: Vec::new(),
            prev_to: Vec::new(),
            reflow: AnimatedValue::new(1.0),
            transition,
            selection: None,
            keyboard_active: false,
            last_pointer: None,
            highlight_rect: AnimatedValue::new([0.0; 4]),
            highlight_opacity: AnimatedValue::new(0.0),
            revision: 0,
        };
        o.relayout(None, HashMap::new());
        o.prev_to = o.to.clone();
        o
    }

    pub fn mode(&self) -> OverviewMode {
        self.mode
    }

    /// Area the windows are packed into in the current mode. In `Overview` it starts below the
    /// desktops band at the height it is heading for: the packing's inset then puts the thumbnails at least the
    /// band's [`layout_gap`](crate::BarConfig::layout_gap) below its bottom.
    pub fn layout_area(&self) -> Rect {
        match self.mode {
            OverviewMode::Overview => {
                let open = self.bar.is_expanded_target();
                let top = (self.cfg.bar.windows_top(open) - self.cfg.layout.inset)
                    .max(self.cfg.bar.band_height(open));
                Rect::new(
                    self.area.x,
                    self.area.y + top,
                    self.area.w,
                    (self.area.h - top).max(1.0),
                )
            }
            OverviewMode::AppOverview(_) => self.area,
        }
    }

    fn is_shown(&self, w: &OverviewWindow) -> bool {
        match self.mode {
            OverviewMode::Overview => !w.minimized,
            OverviewMode::AppOverview(app) => w.app == app,
        }
    }

    fn compute_layout(&self, shown: &[&OverviewWindow]) -> Vec<Rect> {
        let area = self.layout_area();
        match self.mode {
            OverviewMode::Overview => {
                let frames: Vec<Rect> = shown.iter().map(|w| w.size).collect();
                layout(&frames, area, &self.cfg.layout)
            }
            OverviewMode::AppOverview(_) => {
                let ao = &self.cfg.app_overview;
                let open: Vec<usize> = (0..shown.len()).filter(|i| !shown[*i].minimized).collect();
                let mini: Vec<usize> = (0..shown.len()).filter(|i| shown[*i].minimized).collect();
                let mut out = vec![Rect::default(); shown.len()];
                let (upper, row) = if mini.is_empty() {
                    (area, area)
                } else {
                    let row_h = area.h * ao.row_height_fraction;
                    let upper_h = (area.h - row_h - ao.row_margin).max(1.0);
                    (
                        Rect::new(area.x, area.y, area.w, upper_h),
                        Rect::new(area.x, area.bottom() - row_h, area.w, row_h),
                    )
                };
                let frames: Vec<Rect> = open.iter().map(|i| shown[*i].size).collect();
                for (k, r) in layout(&frames, upper, &self.cfg.layout)
                    .into_iter()
                    .enumerate()
                {
                    out[open[k]] = r;
                }
                let frames: Vec<Rect> = mini.iter().map(|i| shown[*i].size).collect();
                for (k, r) in layout_row(&frames, row, ao.row_gap, ao.row_max_scale)
                    .into_iter()
                    .enumerate()
                {
                    out[mini[k]] = r;
                }
                out
            }
        }
    }

    /// Recompute `shown`/`to`. With `now`, windows glide from their current positions.
    fn current_targets(&self, now: f64) -> HashMap<WindowId, Rect> {
        self.shown
            .iter()
            .zip(self.targets(now))
            .map(|(i, r)| (self.all[*i].id, r))
            .collect()
    }

    fn relayout(&mut self, now: Option<f64>, current: HashMap<WindowId, Rect>) {
        self.shown = (0..self.all.len())
            .filter(|i| self.is_shown(&self.all[*i]))
            .collect();
        let shown: Vec<&OverviewWindow> = self.shown.iter().map(|i| &self.all[*i]).collect();
        self.to = self.compute_layout(&shown);
        self.revision += 1;
        self.prev_to = self
            .shown
            .iter()
            .zip(&self.to)
            .map(|(i, r)| current.get(&self.all[*i].id).copied().unwrap_or(*r))
            .collect();
        if let Some(t) = now {
            self.reflow.set_immediately(0.0);
            self.reflow.animate_to(1.0, self.cfg.reflow, t);
        }
        if self
            .selection
            .is_some_and(|s| !self.shown.iter().any(|i| self.all[*i].id == s))
        {
            self.selection = None;
        }
    }

    fn targets(&self, now: f64) -> Vec<Rect> {
        let r = self.reflow.value(now);
        self.prev_to
            .iter()
            .zip(&self.to)
            .map(|(a, b)| Rect::lerp(*a, *b, r))
            .collect()
    }

    /// Replace the window set (a window opened, closed or changed); windows reflow with a spring.
    pub fn set_windows(&mut self, windows: Vec<OverviewWindow>, now: f64) {
        let current = self.current_targets(now);
        self.all = windows;
        self.relayout(Some(now), current);
        self.update_highlight(now);
    }

    pub fn set_mode(&mut self, mode: OverviewMode, now: f64) {
        if self.mode != mode {
            let current = self.current_targets(now);
            self.mode = mode;
            self.relayout(Some(now), current);
            self.update_highlight(now);
        }
    }

    pub fn set_workspaces(&mut self, ids: &[WorkspaceId], current: Option<WorkspaceId>) {
        self.bar.set_workspaces(ids, current);
    }

    // ---- transition ----------------------------------------------------------------------

    /// Gesture-driven progress (0 = desktop, 1 = overview), rubber-banded beyond the ends.
    pub fn gesture_update(&mut self, cumulative_delta: f64, start_progress: f64, now: f64) {
        self.transition
            .gesture_update(cumulative_delta, start_progress, now);
    }

    /// Decide the target from position and velocity (projected end point) and spring to it.
    pub fn gesture_end(&mut self, now: f64) -> bool {
        self.transition.gesture_end(now)
    }

    pub fn toggle(&mut self, open: bool, now: f64) {
        self.transition.toggle(open, now);
    }

    /// The slow-motion modifier is held: transitions that start from now on use the slow duration.
    pub fn set_slow_motion(&mut self, on: bool) {
        self.transition.set_slow_motion(on);
    }

    pub fn slow_motion(&self) -> bool {
        self.transition.slow_motion()
    }

    /// Open/close spring: the configured one, or the slow-motion one while the modifier is held.
    pub fn transition_spring(&self) -> SpringParams {
        self.transition.spring()
    }

    pub fn is_open(&self) -> bool {
        self.transition.is_open()
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.transition.is_animating(now)
            || self.reflow.is_animating(now)
            || self.bar.is_animating(now)
            || self.highlight_rect.is_animating(now)
            || self.highlight_opacity.is_animating(now)
    }

    /// Progress of the transition at `now` (can leave [0, 1] while rubber-banding).
    pub fn progress_at(&self, now: f64) -> f64 {
        self.transition.progress_at(now)
    }

    /// Window title opacity (approximate schedule: fade in after progress 0.8).
    pub fn title_opacity(&self, now: f64) -> f64 {
        self.cfg
            .title_fade
            .opacity(self.transition.progress_at(now))
    }

    /// Windows shown in the current mode, in layout order of `shown`.
    pub fn shown_windows(&self) -> Vec<&OverviewWindow> {
        self.shown.iter().map(|i| &self.all[*i]).collect()
    }

    /// Frames of the shown windows at `now`: desktop frames at progress 0, layout at progress 1.
    pub fn frames(&self, now: f64) -> Vec<Rect> {
        let p = self.transition.progress_at(now);
        self.shown
            .iter()
            .zip(self.targets(now))
            .map(|(i, t)| Rect::lerp(self.all[*i].from, t, p))
            .collect()
    }

    pub fn placed(&self, now: f64) -> Vec<PlacedWindow> {
        let op = self.title_opacity(now);
        self.shown
            .iter()
            .zip(self.frames(now))
            .map(|(i, frame)| {
                let w = &self.all[*i];
                PlacedWindow {
                    id: w.id,
                    frame,
                    title_opacity: op,
                    minimized: w.minimized,
                    selected: self.selection == Some(w.id),
                }
            })
            .collect()
    }

    /// Final (progress 1) layout rectangles.
    pub fn layout_rects(&self) -> &[Rect] {
        &self.to
    }

    /// How many layouts were made: it changes whenever [`Self::layout_rects`] may have.
    pub fn layout_revision(&self) -> u64 {
        self.revision
    }

    // ---- selection and input ------------------------------------------------------------

    pub fn selection(&self) -> Option<WindowId> {
        self.selection
    }

    fn rect_of(&self, id: WindowId, now: f64) -> Option<Rect> {
        let k = self.shown.iter().position(|i| self.all[*i].id == id)?;
        Some(self.targets(now)[k])
    }

    /// Window under `p` in the final layout.
    pub fn hit_test(&self, p: Point, now: f64) -> Option<WindowId> {
        let targets = self.targets(now);
        self.shown
            .iter()
            .zip(targets)
            .find(|(_, r)| r.contains(p))
            .map(|(i, _)| self.all[*i].id)
    }

    fn set_selection(&mut self, sel: Option<WindowId>, now: f64) {
        if sel != self.selection {
            self.selection = sel;
            self.update_highlight(now);
        }
    }

    fn update_highlight(&mut self, now: f64) {
        let h = &self.cfg.highlight;
        let fade = Curve::Timed {
            duration: h.fade_s,
            bezier: CubicBezier::EASE_IN_EASE_OUT,
        };
        match self.selection.and_then(|s| self.rect_of(s, now)) {
            Some(r) => {
                let target = r.inset(-h.inset).to_array();
                if self.highlight_opacity.target() < 0.5 {
                    self.highlight_rect.set_immediately(target);
                } else {
                    self.highlight_rect.animate_to(target, h.move_curve, now);
                }
                self.highlight_opacity.animate_to(1.0, fade, now);
            }
            None => self.highlight_opacity.animate_to(0.0, fade, now),
        }
    }

    pub fn highlight(&self, now: f64) -> Option<Highlight> {
        let window = self.selection?;
        let opacity = self.highlight_opacity.value(now);
        (opacity > 0.0).then(|| Highlight {
            window,
            rect: Rect::from_array(self.highlight_rect.value(now)),
            opacity,
        })
    }

    /// Open or close the desktops band (the pointer reached the top, or left the band): the windows are laid out
    /// again below the band's new height.
    pub fn set_bar_open(&mut self, open: bool, now: f64) {
        if self.bar.is_expanded_target() != open {
            self.bar.set_expanded(open, now);
            let current = self.current_targets(now);
            self.relayout(Some(now), current);
            self.update_highlight(now);
        }
    }

    /// Pointer motion: hover selection (suppressed while the keyboard drives the selection until
    /// the pointer really moves) and the workspaces bar.
    pub fn pointer_moved(&mut self, p: Option<Point>, now: f64) {
        if self.mode == OverviewMode::Overview {
            let open = self.bar.is_expanded_target();
            self.bar.pointer_moved(p, now);
            if self.bar.is_expanded_target() != open {
                // the band opens or closes: the windows move down below it or back up
                let current = self.current_targets(now);
                self.relayout(Some(now), current);
                self.update_highlight(now);
            }
        }
        let moved = match (p, self.last_pointer) {
            (Some(a), Some(b)) => (a.x - b.x).abs() + (a.y - b.y).abs() > 0.5,
            (a, b) => a.is_some() != b.is_some(),
        };
        self.last_pointer = p;
        if moved {
            self.keyboard_active = false;
        }
        if self.keyboard_active {
            return;
        }
        let hit = p.and_then(|p| self.hit_test(p, now));
        self.set_selection(hit, now);
    }

    /// Click at `p`: window → select, bar → workspace actions, background → close.
    pub fn click(&mut self, p: Point, now: f64) -> OverviewAction {
        if self.mode == OverviewMode::Overview {
            match self.bar.hit_test(p, now) {
                Some(BarHit::Workspace(id)) => return OverviewAction::SwitchWorkspace(id),
                Some(BarHit::Add) => return OverviewAction::AddWorkspace,
                None => {}
            }
        }
        match self.hit_test(p, now) {
            Some(w) => OverviewAction::Select(w),
            None => OverviewAction::Close,
        }
    }

    fn navigation_order(&self, now: f64) -> Vec<usize> {
        let targets = self.targets(now);
        let open: Vec<usize> = (0..targets.len())
            .filter(|k| !self.all[self.shown[*k]].minimized)
            .collect();
        let mini: Vec<usize> = (0..targets.len())
            .filter(|k| self.all[self.shown[*k]].minimized)
            .collect();
        let order = |ids: &[usize]| -> Vec<usize> {
            let rects: Vec<Rect> = ids.iter().map(|k| targets[*k]).collect();
            reading_order(&rects).into_iter().map(|j| ids[j]).collect()
        };
        let mut v = order(&open);
        v.extend(order(&mini));
        v
    }

    pub fn key(&mut self, key: Key, now: f64) -> OverviewAction {
        match key {
            Key::Cancel => return OverviewAction::Close,
            Key::Activate => {
                return self
                    .selection
                    .map_or(OverviewAction::None, OverviewAction::Select)
            }
            _ => {}
        }
        if self.shown.is_empty() {
            return OverviewAction::None;
        }
        self.keyboard_active = true;
        let targets = self.targets(now);
        let order = self.navigation_order(now);
        let cur = self
            .selection
            .and_then(|s| self.shown.iter().position(|i| self.all[*i].id == s));
        let next = match (key, cur) {
            (_, None) => Some(if key == Key::Previous {
                *order.last().unwrap()
            } else {
                order[0]
            }),
            (Key::Next, Some(c)) | (Key::Previous, Some(c)) => {
                let pos = order.iter().position(|k| *k == c).unwrap_or(0);
                let n = order.len();
                Some(
                    order[if key == Key::Next {
                        (pos + 1) % n
                    } else {
                        (pos + n - 1) % n
                    }],
                )
            }
            (dir, Some(c)) => {
                let from = targets[c];
                (0..targets.len())
                    .filter(|k| *k != c)
                    .filter_map(|k| {
                        let (dx, dy) = (targets[k].cx() - from.cx(), targets[k].cy() - from.cy());
                        let (primary, cross) = match dir {
                            Key::Right => (dx, dy),
                            Key::Left => (-dx, dy),
                            Key::Down => (dy, dx),
                            _ => (-dy, dx),
                        };
                        (primary > 1e-6).then_some((k, primary + 2.0 * cross.abs()))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(k, _)| k)
            }
        };
        if let Some(k) = next {
            let id = self.all[self.shown[k]].id;
            self.set_selection(Some(id), now);
        }
        OverviewAction::None
    }
}
