//! Stage model: groups, centre stage, strip of recent groups, swap animation, add by drag, strip
//! hide/peek, desktop hiding.
//!
//! Layout
//! defaults were, and the strip moves with the window manager's generic
//! 0.38 s critically damped frame spring (see [`StageConfig`]).
//!
//! Visual contract for the compositor. Every group has a `blend` (0 = shown as a strip thumbnail
//! inside its slot, 1 = shown on the stage at the window's stage frame) and an animated slot
//! rectangle. [`Stage::window_frames`] combines them; swaps retarget the springs without jumps.

use std::collections::HashMap;

use cupertino_animation::AnimatedValue;
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::{AppId, WindowId};

use crate::config::{ClickDesktop, Grouping, StageConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum StageEvent {
    GroupCreated { group: GroupId },
    GroupRemoved { group: GroupId },
    /// `incoming` becomes the centre group; `outgoing` (if any) moves to the top of the strip.
    Swap { incoming: GroupId, outgoing: Option<GroupId> },
    /// The centre group left the stage and nothing replaced it (empty stage).
    CenterEmptied { group: Option<GroupId> },
    WindowMoved { window: WindowId, from: GroupId, to: GroupId },
    Enabled,
    Disabled,
    StripHidden,
    StripShown,
    DesktopItemsVisible(bool),
}

/// Where a window rests once every animation has settled: what a window manager tells the
/// compositor, which animates each window towards it on its own frame clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// On the stage at its own frame.
    Stage,
    /// A thumbnail of the window at this rectangle of the strip.
    Strip(Rect),
    /// Not shown: its group is beyond the strip's rows.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    Stage,
    Strip,
}

struct Group {
    id: GroupId,
    /// Most recently used first.
    windows: Vec<WindowId>,
    slot: AnimatedValue<[f64; 4]>,
    slot_valid: bool,
    blend: AnimatedValue<f64>,
}

struct WindowInfo {
    app: AppId,
    group: GroupId,
    frame: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StripSlot {
    pub group: GroupId,
    /// Animated slot rectangle, including the hide/peek shift.
    pub rect: Rect,
    pub blend: f64,
    pub apps: Vec<AppId>,
    /// App-icon overlay rectangles (up to three apps).
    pub icons: Vec<Rect>,
}

pub struct Stage {
    pub cfg: StageConfig,
    usable: Rect,
    enabled: bool,
    groups: Vec<Group>,
    center: Option<GroupId>,
    /// Most recently left first.
    strip: Vec<GroupId>,
    windows: HashMap<WindowId, WindowInfo>,
    next_group: u64,
    hidden: AnimatedValue<f64>,
    hidden_target: bool,
    hot_since: Option<f64>,
    cold_since: Option<f64>,
    desktop_revealed: bool,
}

impl Stage {
    pub fn new(cfg: StageConfig, usable: Rect) -> Self {
        Self {
            cfg,
            usable,
            enabled: false,
            groups: Vec::new(),
            center: None,
            strip: Vec::new(),
            windows: HashMap::new(),
            next_group: 1,
            hidden: AnimatedValue::new(0.0),
            hidden_target: false,
            hot_since: None,
            cold_since: None,
            desktop_revealed: false,
        }
    }

    // ---- model ---------------------------------------------------------------------------

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn center_group(&self) -> Option<GroupId> {
        self.center
    }
    /// Strip groups, most recently left first (including those beyond the visible rows).
    pub fn strip_groups(&self) -> &[GroupId] {
        &self.strip
    }
    pub fn group_of(&self, window: WindowId) -> Option<GroupId> {
        self.windows.get(&window).map(|w| w.group)
    }
    pub fn group_windows(&self, group: GroupId) -> Vec<WindowId> {
        self.group(group).map(|g| g.windows.clone()).unwrap_or_default()
    }
    pub fn center_windows(&self) -> Vec<WindowId> {
        self.center.map(|c| self.group_windows(c)).unwrap_or_default()
    }
    pub fn group_apps(&self, group: GroupId) -> Vec<AppId> {
        let mut out = Vec::new();
        for w in self.group_windows(group) {
            let app = self.windows[&w].app;
            if !out.contains(&app) {
                out.push(app);
            }
        }
        out
    }

    fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }
    fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    fn new_group(&mut self, window: WindowId) -> GroupId {
        let id = GroupId(self.next_group);
        self.next_group += 1;
        let r = self.stage_area();
        self.groups.push(Group {
            id,
            windows: vec![window],
            slot: AnimatedValue::new(r.to_array()),
            slot_valid: false,
            blend: AnimatedValue::new(0.0),
        });
        id
    }

    /// Enable or disable the stage. Enabling builds groups from `windows` (most recently used
    /// first): the first becomes the centre group, the others fill the strip.
    pub fn set_enabled(&mut self, enabled: bool, windows: &[(WindowId, AppId, Rect)], now: f64) -> Vec<StageEvent> {
        let mut ev = Vec::new();
        self.groups.clear();
        self.strip.clear();
        self.windows.clear();
        self.center = None;
        self.desktop_revealed = false;
        self.enabled = enabled;
        if !enabled {
            ev.push(StageEvent::Disabled);
            ev.push(StageEvent::DesktopItemsVisible(true));
            return ev;
        }
        ev.push(StageEvent::Enabled);
        for (i, (w, app, frame)) in windows.iter().enumerate() {
            self.windows.insert(*w, WindowInfo { app: *app, group: GroupId(0), frame: *frame });
            let joined = self.find_group_for_app(*app).filter(|_| self.cfg.grouping == Grouping::AllWindows);
            match joined {
                Some(g) => {
                    self.windows.get_mut(w).unwrap().group = g;
                    self.group_mut(g).unwrap().windows.push(*w);
                }
                None => {
                    let g = self.new_group(*w);
                    self.windows.get_mut(w).unwrap().group = g;
                    ev.push(StageEvent::GroupCreated { group: g });
                    if i == 0 {
                        self.center = Some(g);
                        self.group_mut(g).unwrap().blend.set_immediately(1.0);
                    } else {
                        self.strip.push(g);
                    }
                }
            }
        }
        self.relayout_strip(now);
        ev.push(StageEvent::DesktopItemsVisible(self.desktop_items_visible()));
        ev
    }

    fn find_group_for_app(&self, app: AppId) -> Option<GroupId> {
        let has = |g: GroupId| self.windows.values().any(|w| w.group == g && w.app == app);
        self.center.filter(|c| has(*c)).or_else(|| self.strip.iter().copied().find(|g| has(*g)))
    }

    /// A window was mapped. Returns the resulting events (a swap when it takes the stage).
    pub fn window_opened(&mut self, window: WindowId, app: AppId, frame: Rect, now: f64) -> Vec<StageEvent> {
        if !self.enabled {
            return vec![];
        }
        if self.windows.contains_key(&window) {
            return self.window_activated(window, now);
        }
        let mut ev = Vec::new();
        let existing = if self.cfg.grouping == Grouping::AllWindows { self.find_group_for_app(app) } else { None };
        self.windows.insert(window, WindowInfo { app, group: GroupId(0), frame });
        let g = match existing {
            Some(g) => {
                self.windows.get_mut(&window).unwrap().group = g;
                self.group_mut(g).unwrap().windows.insert(0, window);
                g
            }
            None => {
                let g = self.new_group(window);
                self.windows.get_mut(&window).unwrap().group = g;
                ev.push(StageEvent::GroupCreated { group: g });
                g
            }
        };
        ev.extend(self.make_center(g, now));
        ev
    }

    /// A window was focused (strip click, switcher, dock): its group takes the stage.
    pub fn window_activated(&mut self, window: WindowId, now: f64) -> Vec<StageEvent> {
        let Some(g) = self.group_of(window) else { return vec![] };
        if let Some(gr) = self.group_mut(g) {
            gr.windows.retain(|w| *w != window);
            gr.windows.insert(0, window);
        }
        self.make_center(g, now)
    }

    /// Activate a whole group (strip thumbnail click).
    pub fn activate_group(&mut self, group: GroupId, now: f64) -> Vec<StageEvent> {
        if self.group(group).is_none() {
            return vec![];
        }
        self.make_center(group, now)
    }

    fn make_center(&mut self, g: GroupId, now: f64) -> Vec<StageEvent> {
        let old = self.center;
        let mut ev = Vec::new();
        if old == Some(g) {
            if self.desktop_revealed {
                self.desktop_revealed = false;
                ev.push(StageEvent::DesktopItemsVisible(self.desktop_items_visible()));
            }
            return ev;
        }
        let curve = self.cfg.swap_curve;
        self.strip.retain(|x| *x != g);
        if let Some(o) = old {
            self.send_to_strip(o, now);
        }
        self.center = Some(g);
        if let Some(gr) = self.group_mut(g) {
            gr.blend.animate_to(1.0, curve, now);
        }
        ev.push(StageEvent::Swap { incoming: g, outgoing: old });
        if self.desktop_revealed {
            self.desktop_revealed = false;
            ev.push(StageEvent::DesktopItemsVisible(self.desktop_items_visible()));
        }
        self.relayout_strip(now);
        ev
    }

    /// Move a group from the stage to the top of the strip; it shrinks out of the stage area.
    fn send_to_strip(&mut self, g: GroupId, now: f64) {
        let curve = self.cfg.swap_curve;
        let area = self.stage_area();
        self.strip.insert(0, g);
        if let Some(gr) = self.group_mut(g) {
            // a swap that is still running keeps its slot so the frames stay continuous
            let on_stage = gr.blend.value(now) >= 0.999 || !gr.slot_valid;
            gr.blend.animate_to(0.0, curve, now);
            if on_stage {
                gr.slot.set_immediately(area.to_array());
                gr.slot_valid = true;
            }
        }
    }

    pub fn window_closed(&mut self, window: WindowId, now: f64) -> Vec<StageEvent> {
        let Some(info) = self.windows.remove(&window) else { return vec![] };
        let g = info.group;
        let mut ev = Vec::new();
        if let Some(gr) = self.group_mut(g) {
            gr.windows.retain(|w| *w != window);
            if !gr.windows.is_empty() {
                return ev;
            }
        }
        self.groups.retain(|x| x.id != g);
        self.strip.retain(|x| *x != g);
        ev.push(StageEvent::GroupRemoved { group: g });
        if self.center == Some(g) {
            self.center = None;
            if self.cfg.promote_on_empty && !self.strip.is_empty() {
                let next = self.strip[0];
                ev.extend(self.make_center(next, now));
            } else {
                ev.push(StageEvent::CenterEmptied { group: None });
            }
        }
        self.relayout_strip(now);
        ev
    }

    pub fn set_window_frame(&mut self, window: WindowId, frame: Rect) {
        if let Some(w) = self.windows.get_mut(&window) {
            w.frame = frame;
        }
    }

    /// Move the centre group into the strip and leave the stage empty.
    pub fn empty_stage(&mut self, now: f64) -> Vec<StageEvent> {
        let mut ev = Vec::new();
        if let Some(c) = self.center.take() {
            self.send_to_strip(c, now);
            ev.push(StageEvent::CenterEmptied { group: Some(c) });
            self.relayout_strip(now);
        }
        if self.enabled && !self.desktop_revealed {
            self.desktop_revealed = true;
            ev.push(StageEvent::DesktopItemsVisible(true));
        }
        ev
    }

    /// Click on the desktop background.
    pub fn click_desktop(&mut self, now: f64) -> Vec<StageEvent> {
        if !self.enabled {
            return vec![];
        }
        match self.cfg.click_desktop {
            ClickDesktop::Nothing => vec![],
            ClickDesktop::EmptyStage => self.empty_stage(now),
        }
    }

    /// Whether desktop icons and widgets are shown.
    pub fn desktop_items_visible(&self) -> bool {
        !(self.enabled && self.cfg.hide_desktop_items && !self.desktop_revealed)
    }

    /// The strip column grown to the output edge on the left and by half the gap to the stage on
    /// the right.
    fn strip_drop_area(&self) -> Rect {
        let strip = self.strip_area();
        let right = strip.right() + self.cfg.stage_inset_from_strip / 2.0;
        Rect::new(self.usable.x, strip.y, (right - self.usable.x).max(0.0), strip.h)
    }

    /// What dropping a dragged strip group / stage window at `pointer` would do.
    pub fn drop_target(&self, pointer: Point) -> Option<DropTarget> {
        if !self.enabled {
            return None;
        }
        if self.stage_area().contains(pointer) {
            Some(DropTarget::Stage)
        } else if self.strip_drop_area().contains(pointer) {
            Some(DropTarget::Strip)
        } else {
            None
        }
    }

    /// Add a dragged strip group to the stage (merging it into the centre group; becomes the centre
    /// group when the stage is empty).
    pub fn add_to_stage(&mut self, group: GroupId, now: f64) -> Vec<StageEvent> {
        if self.group(group).is_none() || self.center == Some(group) {
            return vec![];
        }
        let Some(c) = self.center else { return self.make_center(group, now) };
        let moved = self.group(group).map(|g| g.windows.clone()).unwrap_or_default();
        let mut ev = Vec::new();
        self.strip.retain(|x| *x != group);
        self.groups.retain(|x| x.id != group);
        for w in &moved {
            self.windows.get_mut(w).unwrap().group = c;
            ev.push(StageEvent::WindowMoved { window: *w, from: group, to: c });
        }
        let cg = self.group_mut(c).unwrap();
        for w in moved.iter().rev() {
            cg.windows.insert(0, *w);
        }
        ev.push(StageEvent::GroupRemoved { group });
        self.relayout_strip(now);
        ev
    }

    /// Drop a strip group at `pointer`: onto the stage adds it, anywhere else does nothing.
    pub fn drop_group(&mut self, group: GroupId, pointer: Point, now: f64) -> Vec<StageEvent> {
        if self.drop_target(pointer) == Some(DropTarget::Stage) {
            self.add_to_stage(group, now)
        } else {
            vec![]
        }
    }

    /// Drop a stage window at `pointer`: onto the strip moves it into its own strip group.
    pub fn drop_window(&mut self, window: WindowId, pointer: Point, now: f64) -> Vec<StageEvent> {
        let Some(g) = self.group_of(window) else { return vec![] };
        if self.center != Some(g) || self.drop_target(pointer) != Some(DropTarget::Strip) {
            return vec![];
        }
        let mut ev = Vec::new();
        let remaining = self.group(g).map_or(0, |gr| gr.windows.len());
        if remaining == 1 {
            return self.empty_stage_keep_desktop(now);
        }
        self.group_mut(g).unwrap().windows.retain(|w| *w != window);
        let n = self.new_group(window);
        self.windows.get_mut(&window).unwrap().group = n;
        let frame = self.windows[&window].frame;
        let ng = self.group_mut(n).unwrap();
        ng.slot.set_immediately(frame.to_array());
        ng.slot_valid = true;
        self.strip.insert(0, n);
        ev.push(StageEvent::GroupCreated { group: n });
        ev.push(StageEvent::WindowMoved { window, from: g, to: n });
        self.relayout_strip(now);
        ev
    }

    fn empty_stage_keep_desktop(&mut self, now: f64) -> Vec<StageEvent> {
        let mut ev = Vec::new();
        if let Some(c) = self.center.take() {
            self.send_to_strip(c, now);
            ev.push(StageEvent::CenterEmptied { group: Some(c) });
            self.relayout_strip(now);
        }
        ev
    }

    // ---- layout ---------------------------------------------------------------------------

    /// (rows, slot height): as many rows as keep the slots at least the preferred minimum height
    /// (use of the verified preferred minimum), clamped to the configured range; the slot
    /// height is the nominal height capped by the maximum, shrunk so the rows fit.
    pub fn strip_metrics(&self) -> (usize, f64) {
        let c = &self.cfg;
        let avail = (self.usable.h - 2.0 * c.strip_vertical_padding).max(1.0);
        let nominal = c.strip_row_height.min(c.strip_max_item_height);
        let smallest = c.strip_preferred_min_item_height.min(nominal).max(1.0);
        let fit = ((avail + c.strip_row_gap) / (smallest + c.strip_row_gap)).floor().max(0.0) as usize;
        let rows = fit.clamp(c.strip_min_rows.min(c.strip_max_rows), c.strip_max_rows).max(1);
        let h = nominal
            .min((avail - c.strip_row_gap * (rows as f64 - 1.0)) / rows as f64)
            .max(c.strip_absolute_min_item_height.max(1.0));
        (rows, h)
    }

    pub fn rows(&self) -> usize {
        self.strip_metrics().0
    }

    /// Strip groups that currently have a slot.
    pub fn visible_strip(&self) -> &[GroupId] {
        &self.strip[..self.strip.len().min(self.rows())]
    }

    /// Resting slot rectangle (strip fully shown) of the `index`-th visible group.
    pub fn slot_rect(&self, index: usize) -> Rect {
        let c = &self.cfg;
        let (_, h) = self.strip_metrics();
        let n = self.visible_strip().len().max(1);
        let total = n as f64 * h + (n as f64 - 1.0) * c.strip_row_gap;
        let avail = (self.usable.h - 2.0 * c.strip_vertical_padding).max(1.0);
        let y0 = self.usable.y + c.strip_vertical_padding + (avail - total).max(0.0) * c.strip_vertical_align;
        Rect::new(self.usable.x + c.strip_edge_inset, y0 + index as f64 * (h + c.strip_row_gap), c.strip_width, h)
    }

    /// Full-height strip column with the strip shown.
    pub fn strip_area(&self) -> Rect {
        Rect::new(self.usable.x + self.cfg.strip_edge_inset, self.usable.y, self.cfg.strip_width, self.usable.h)
    }

    /// Area for the centre group's windows.
    pub fn stage_area(&self) -> Rect {
        if !self.enabled {
            return self.usable;
        }
        self.cfg.stage_frame(self.usable)
    }

    pub fn set_usable(&mut self, usable: Rect, now: f64) {
        self.usable = usable;
        self.relayout_strip(now);
    }

    fn relayout_strip(&mut self, now: f64) {
        let visible: Vec<GroupId> = self.visible_strip().to_vec();
        let curve = self.cfg.swap_curve;
        let targets: Vec<Rect> = (0..visible.len()).map(|i| self.slot_rect(i)).collect();
        for g in self.strip.clone() {
            let target = visible.iter().position(|v| *v == g).map(|i| targets[i]);
            let Some(gr) = self.group_mut(g) else { continue };
            match target {
                Some(t) if !gr.slot_valid => {
                    gr.slot.set_immediately(t.to_array());
                    gr.slot_valid = true;
                }
                Some(t) => {
                    if gr.slot.target() != t.to_array() {
                        gr.slot.animate_to(t.to_array(), curve, now);
                    }
                }
                None => gr.slot_valid = false,
            }
        }
    }

    /// Horizontal shift of the strip caused by hiding (negative = towards the left edge).
    pub fn strip_shift(&self, now: f64) -> f64 {
        let hidden = self.hidden.value(now).clamp(0.0, 1.0);
        let strip = self.strip_area();
        -(strip.right() - (self.usable.x + self.cfg.strip_peek_width)).max(0.0) * hidden
    }

    pub fn strip_hidden_fraction(&self, now: f64) -> f64 {
        self.hidden.value(now).clamp(0.0, 1.0)
    }

    pub fn is_strip_hidden_target(&self) -> bool {
        self.hidden_target
    }

    pub fn group_blend(&self, group: GroupId, now: f64) -> f64 {
        self.group(group).map_or(0.0, |g| g.blend.value(now))
    }

    fn icons_for(&self, group: GroupId, slot: Rect) -> (Vec<AppId>, Vec<Rect>) {
        let c = &self.cfg;
        let apps = self.group_apps(group);
        let shown = apps.len().min(3);
        let spacing = match shown {
            0 | 1 => 0.0,
            2 => c.icon_two_spacing,
            _ => c.icon_three_spacing,
        };
        let icons = (0..shown)
            .map(|k| Rect::new(slot.x + c.icon_left_offset + k as f64 * spacing, slot.bottom() - c.icon_bottom_offset - c.icon_size, c.icon_size, c.icon_size))
            .collect();
        (apps, icons)
    }

    /// Strip slots at `now` (animated, including the hide shift).
    pub fn strip_slots(&self, now: f64) -> Vec<StripSlot> {
        if !self.enabled {
            return vec![];
        }
        let dx = self.strip_shift(now);
        self.visible_strip()
            .iter()
            .filter_map(|g| {
                let gr = self.group(*g)?;
                let rect = Rect::from_array(gr.slot.value(now)).translate(dx, 0.0);
                let (apps, icons) = self.icons_for(*g, rect);
                Some(StripSlot { group: *g, rect, blend: gr.blend.value(now), apps, icons })
            })
            .collect()
    }

    pub fn hit_test_strip(&self, p: Point, now: f64) -> Option<GroupId> {
        self.strip_slots(now).into_iter().find(|s| s.rect.contains(p)).map(|s| s.group)
    }

    fn union_of(&self, group: GroupId) -> Option<Rect> {
        let g = self.group(group)?;
        g.windows.iter().map(|w| self.windows[w].frame).reduce(|a, b| a.union(&b))
    }

    /// Visual frames of every window of every group (`blend` 1 = stage frame, 0 = thumbnail).
    pub fn window_frames(&self, now: f64) -> Vec<(WindowId, Rect, f64)> {
        let mut out = Vec::new();
        let dx = self.strip_shift(now);
        for g in &self.groups {
            let blend = g.blend.value(now);
            let union = self.union_of(g.id);
            let slot = Rect::from_array(g.slot.value(now)).translate(dx, 0.0).inset(self.cfg.thumbnail_padding);
            for w in &g.windows {
                let stage = self.windows[w].frame;
                let thumb = union.map_or(stage, |u| thumbnail(slot, u, stage));
                out.push((*w, Rect::lerp(thumb, stage, blend), blend));
            }
        }
        out.sort_by_key(|(w, _, _)| *w);
        out
    }

    /// Resting placement of every window (sorted by id; empty while disabled): centre windows on
    /// the stage, the windows of a visible strip group as thumbnails in their slot's resting
    /// rectangle (shifted out while the strip is hidden), the others hidden. Unlike
    /// [`Stage::window_frames`] it does not depend on time: it is where a swap ends, not a frame of it.
    pub fn placements(&self) -> Vec<(WindowId, Placement)> {
        if !self.enabled {
            return vec![];
        }
        let dx = if self.hidden_target { -(self.strip_area().right() - (self.usable.x + self.cfg.strip_peek_width)).max(0.0) } else { 0.0 };
        let visible = self.visible_strip();
        let mut out = Vec::new();
        for g in &self.groups {
            let slot = visible.iter().position(|v| *v == g.id).map(|i| self.slot_rect(i).translate(dx, 0.0).inset(self.cfg.thumbnail_padding));
            let union = self.union_of(g.id);
            for w in &g.windows {
                let placement = match (self.center == Some(g.id), slot, union) {
                    (true, _, _) => Placement::Stage,
                    (false, Some(slot), Some(u)) => Placement::Strip(thumbnail(slot, u, self.windows[w].frame)),
                    (false, _, _) => Placement::Hidden,
                };
                out.push((*w, placement));
            }
        }
        out.sort_by_key(|(w, _)| *w);
        out
    }

    /// Frames that fit the group's windows into the stage area: relative layout preserved, uniformly
    /// shrunk when too large (never enlarged), centred. Used after a merge or a resize.
    pub fn fit_group(&self, group: GroupId) -> Vec<(WindowId, Rect)> {
        let Some(g) = self.group(group) else { return vec![] };
        let Some(u) = self.union_of(group) else { return vec![] };
        let area = self.stage_area();
        let s = (area.w / u.w).min(area.h / u.h).min(1.0);
        let (nw, nh) = (u.w * s, u.h * s);
        let (ox, oy) = (area.x + (area.w - nw) / 2.0, area.y + (area.h - nh) / 2.0);
        g.windows
            .iter()
            .map(|w| {
                let f = self.windows[w].frame;
                (*w, Rect::new(ox + (f.x - u.x) * s, oy + (f.y - u.y) * s, f.w * s, f.h * s))
            })
            .collect()
    }

    // ---- strip visibility -------------------------------------------------------------------

    /// Whether any centre window intrudes into the strip column.
    pub fn center_overlaps_strip(&self) -> bool {
        let strip = self.strip_area().inset(self.cfg.overlap_margin);
        self.center_windows().iter().any(|w| self.windows[w].frame.intersects(&strip))
    }

    fn set_hidden(&mut self, hidden: bool, now: f64, ev: &mut Vec<StageEvent>) {
        if self.hidden_target != hidden {
            self.hidden_target = hidden;
            self.hidden.animate_to(if hidden { 1.0 } else { 0.0 }, self.cfg.slide_curve, now);
            ev.push(if hidden { StageEvent::StripHidden } else { StageEvent::StripShown });
        }
    }

    /// Drive strip hide/peek from the pointer and the window positions. Call on pointer motion and
    /// every frame while `wants_update`.
    pub fn update(&mut self, pointer: Option<Point>, now: f64) -> Vec<StageEvent> {
        let mut ev = Vec::new();
        if !self.enabled {
            return ev;
        }
        let want_hidden = self.cfg.auto_hide || self.center_overlaps_strip();
        if !want_hidden {
            self.hot_since = None;
            self.cold_since = None;
            self.set_hidden(false, now, &mut ev);
            return ev;
        }
        let zone = Rect::new(self.usable.x, self.usable.y, self.cfg.reveal_zone_width.max(self.cfg.strip_peek_width), self.usable.h);
        let over_strip = !self.hidden_target && pointer.is_some_and(|p| self.strip_area().inset(-8.0).contains(p));
        let hot = pointer.is_some_and(|p| zone.contains(p)) || over_strip;
        if hot {
            self.cold_since = None;
            let since = *self.hot_since.get_or_insert(now);
            if self.hidden_target && now - since >= self.cfg.reveal_delay {
                self.set_hidden(false, now, &mut ev);
            }
        } else {
            self.hot_since = None;
            let since = *self.cold_since.get_or_insert(now);
            if !self.hidden_target && now - since >= self.cfg.hide_delay {
                self.set_hidden(true, now, &mut ev);
            }
        }
        ev
    }

    /// True while a timer or animation needs further `update` calls.
    pub fn wants_update(&self, now: f64) -> bool {
        self.hot_since.is_some() || self.cold_since.is_some() || self.is_animating(now)
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.hidden.is_animating(now) || self.groups.iter().any(|g| g.slot.is_animating(now) || g.blend.is_animating(now))
    }
}

/// Where `frame`, one window of a group whose frames span `union`, sits when the group is shrunk
/// (never grown) into `slot` with its relative layout kept and centred. A degenerate slot or union
/// leaves the frame as it is.
fn thumbnail(slot: Rect, union: Rect, frame: Rect) -> Rect {
    if union.w <= 0.0 || union.h <= 0.0 || slot.is_empty() {
        return frame;
    }
    let s = (slot.w / union.w).min(slot.h / union.h).min(1.0);
    let (cx, cy) = (slot.cx() + (frame.cx() - union.cx()) * s, slot.cy() + (frame.cy() - union.cy()) * s);
    Rect::new(cx - frame.w * s / 2.0, cy - frame.h * s / 2.0, frame.w * s, frame.h * s)
}
