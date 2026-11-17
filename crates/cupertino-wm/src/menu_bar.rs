//! Presentation of the menu bar, one per output: active / inactive dimming, auto-hide with a reveal
//! dwell and the area the bar reserves. The server owns this (the bar's frame, layers, auto-hide
//! and reveal); the bar content is drawn by the menu bar process.
//!
//! behaviour of the design: a bar on every
//! output, the inactive output's bar is dimmed, the bar auto-hides in full screen and reveals when
//! the pointer dwells at the top edge, with an option to hide it always, only on the desktop, or
//! only in full screen. The reveal dwell is unknown: [`BarSetConfig::reveal_dwell`] and
//! [`BarSetConfig::hide_delay`] are configurable placeholders (the dock's verified 0.15 s is reused
//! for the reveal). Slide and dim curves are approximate.

use std::collections::HashMap;

use cupertino_animation::{AnimatedValue, CubicBezier, Curve};

use crate::geometry::{Point, Rect};
use crate::ids::OutputId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoHide {
    Never,
    /// Hide only while an app is full screen on the output (the default behaviour).
    FullscreenOnly,
    /// Always hide until revealed.
    Always,
}

#[derive(Clone, Debug)]
pub struct BarSetConfig {
    /// Bar height in points. approximate 24 pt (larger on displays with a camera cut-out: set per
    /// output with [`MenuBarSet::set_bar_height`]).
    pub bar_height: f64,
    pub auto_hide: AutoHide,
    /// Seconds the pointer must rest at the top edge before the bar is revealed. unknown.
    pub reveal_dwell: f64,
    /// Seconds the pointer must stay below the bar before it hides again. unknown.
    pub hide_delay: f64,
    /// Distance from the top edge that counts as touching it. approximate.
    pub edge_thickness: f64,
    pub slide_curve: Curve,
    pub dim_curve: Curve,
}

impl Default for BarSetConfig {
    fn default() -> Self {
        let timed = Curve::Timed {
            duration: 0.2,
            bezier: CubicBezier::EASE_IN_EASE_OUT,
        };
        Self {
            bar_height: 33.0,
            auto_hide: AutoHide::FullscreenOnly,
            reveal_dwell: 0.15,
            hide_delay: 0.3,
            edge_thickness: 1.0,
            slide_curve: timed,
            dim_curve: timed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarEvent {
    Revealed {
        output: OutputId,
    },
    Hidden {
        output: OutputId,
    },
    /// The area reserved for the bar changed (auto-hide toggled): recompute usable areas.
    UsableAreaChanged {
        output: OutputId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarPresentation {
    pub output: OutputId,
    /// The bar rectangle including the slide offset (partly above the output while hiding).
    pub rect: Rect,
    /// 1 fully shown, 0 fully hidden.
    pub visible: f64,
    /// 0 for the active output, 1 for an inactive output (drives the inactive presentation state).
    pub inactive: f64,
    pub animating: bool,
}

struct OutputState {
    rect: Rect,
    height: f64,
    fullscreen: bool,
    revealed: bool,
    menu_open: bool,
    dwell_since: Option<f64>,
    leave_since: Option<f64>,
    visible: AnimatedValue<f64>,
    inactive: AnimatedValue<f64>,
}

pub struct MenuBarSet {
    pub cfg: BarSetConfig,
    outputs: HashMap<OutputId, OutputState>,
    order: Vec<OutputId>,
    active: Option<OutputId>,
}

impl MenuBarSet {
    pub fn new(cfg: BarSetConfig) -> Self {
        Self {
            cfg,
            outputs: HashMap::new(),
            order: Vec::new(),
            active: None,
        }
    }

    pub fn add_output(&mut self, id: OutputId, rect: Rect, now: f64) {
        let height = self.cfg.bar_height;
        let inactive = if self.active.is_some_and(|a| a != id) {
            1.0
        } else {
            0.0
        };
        self.outputs.insert(
            id,
            OutputState {
                rect,
                height,
                fullscreen: false,
                revealed: false,
                menu_open: false,
                dwell_since: None,
                leave_since: None,
                visible: AnimatedValue::new(1.0),
                inactive: AnimatedValue::new(inactive),
            },
        );
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        if self.active.is_none() {
            self.active = Some(id);
            if let Some(o) = self.outputs.get_mut(&id) {
                o.inactive = AnimatedValue::new(0.0);
            }
        }
        self.retarget_visible(id, now);
    }

    pub fn remove_output(&mut self, id: OutputId, now: f64) {
        self.outputs.remove(&id);
        self.order.retain(|o| *o != id);
        if self.active == Some(id) {
            let next = self.order.first().copied();
            self.set_active_output(next, now);
        }
    }

    pub fn set_output_rect(&mut self, id: OutputId, rect: Rect) {
        if let Some(o) = self.outputs.get_mut(&id) {
            o.rect = rect;
        }
    }

    /// Override the bar height of one output (displays with a camera cut-out).
    pub fn set_bar_height(&mut self, id: OutputId, height: f64) {
        if let Some(o) = self.outputs.get_mut(&id) {
            o.height = height;
        }
    }

    pub fn outputs(&self) -> &[OutputId] {
        &self.order
    }

    pub fn active_output(&self) -> Option<OutputId> {
        self.active
    }

    /// The output that has keyboard focus / the focused app's window; all others are dimmed.
    pub fn set_active_output(&mut self, active: Option<OutputId>, now: f64) {
        self.active = active;
        let curve = self.cfg.dim_curve;
        for (id, o) in self.outputs.iter_mut() {
            let target = if Some(*id) == active { 0.0 } else { 1.0 };
            if (o.inactive.target() - target).abs() > f64::EPSILON {
                o.inactive.animate_to(target, curve, now);
            }
        }
    }

    pub fn set_fullscreen(&mut self, id: OutputId, fullscreen: bool, now: f64) -> Vec<BarEvent> {
        let Some(o) = self
            .outputs
            .get_mut(&id)
            .filter(|o| o.fullscreen != fullscreen)
        else {
            return Vec::new();
        };
        let before = Self::hidden_target_of(&self.cfg, o);
        o.fullscreen = fullscreen;
        o.revealed = false;
        o.dwell_since = None;
        o.leave_since = None;
        let mut ev = Vec::new();
        if before != Self::hidden_target_of(&self.cfg, o) {
            ev.push(BarEvent::UsableAreaChanged { output: id });
        }
        self.retarget_visible(id, now);
        ev
    }

    pub fn set_auto_hide(&mut self, policy: AutoHide, now: f64) -> Vec<BarEvent> {
        self.cfg.auto_hide = policy;
        let ids = self.order.clone();
        let mut ev = Vec::new();
        for id in ids {
            if let Some(o) = self.outputs.get_mut(&id) {
                o.revealed = false;
                o.dwell_since = None;
                o.leave_since = None;
            }
            ev.push(BarEvent::UsableAreaChanged { output: id });
            self.retarget_visible(id, now);
        }
        ev
    }

    /// A menu is open on `id`: keep the bar shown.
    pub fn set_menu_open(&mut self, id: OutputId, open: bool, now: f64) {
        if let Some(o) = self.outputs.get_mut(&id) {
            o.menu_open = open;
            if !open {
                o.leave_since = Some(now);
            }
        }
        self.retarget_visible(id, now);
    }

    fn hidden_target_of(cfg: &BarSetConfig, o: &OutputState) -> bool {
        match cfg.auto_hide {
            AutoHide::Never => false,
            AutoHide::Always => true,
            AutoHide::FullscreenOnly => o.fullscreen,
        }
    }

    fn visible_target(&self, id: OutputId) -> Option<bool> {
        let o = self.outputs.get(&id)?;
        Some(!Self::hidden_target_of(&self.cfg, o) || o.revealed || o.menu_open)
    }

    fn retarget_visible(&mut self, id: OutputId, now: f64) {
        let Some(shown) = self.visible_target(id) else {
            return;
        };
        let curve = self.cfg.slide_curve;
        if let Some(o) = self.outputs.get_mut(&id) {
            let target = if shown { 1.0 } else { 0.0 };
            if (o.visible.target() - target).abs() > f64::EPSILON {
                o.visible.animate_to(target, curve, now);
            }
        }
    }

    fn at_top_edge(&self, id: OutputId, p: Point) -> bool {
        let Some(o) = self.outputs.get(&id) else {
            return false;
        };
        if !o.rect.contains(p) || p.y - o.rect.y >= self.cfg.edge_thickness {
            return false;
        }
        let probe = Point::new(p.x, o.rect.y - 0.5);
        !self
            .outputs
            .values()
            .any(|other| other.rect.contains(probe))
    }

    /// Feed the pointer position (global coordinates).
    pub fn pointer_moved(&mut self, pointer: Point, now: f64) -> Vec<BarEvent> {
        let mut ev = Vec::new();
        let ids = self.order.clone();
        for id in ids {
            let hidden_target = match self.outputs.get(&id) {
                Some(o) => Self::hidden_target_of(&self.cfg, o),
                None => continue,
            };
            if !hidden_target {
                continue;
            }
            let at_edge = self.at_top_edge(id, pointer);
            let height = self.outputs[&id].height;
            let in_bar = self.outputs[&id].rect.contains(pointer)
                && pointer.y - self.outputs[&id].rect.y < height;
            let o = self.outputs.get_mut(&id).expect("output");
            if !o.revealed {
                match (at_edge, o.dwell_since) {
                    (true, None) => o.dwell_since = Some(now),
                    (false, Some(_)) => o.dwell_since = None,
                    _ => {}
                }
            } else if in_bar || at_edge {
                o.leave_since = None;
            } else if o.leave_since.is_none() {
                o.leave_since = Some(now);
            }
        }
        ev.extend(self.tick(now));
        ev
    }

    /// Reveal or hide the auto-hidden bar of `id` on behalf of an external dwell timer (an edge
    /// reveal component): the bar's own dwell state is cleared.
    pub fn set_revealed(&mut self, id: OutputId, revealed: bool, now: f64) -> Vec<BarEvent> {
        let Some(o) = self.outputs.get_mut(&id) else {
            return Vec::new();
        };
        o.dwell_since = None;
        o.leave_since = None;
        if o.revealed == revealed {
            return Vec::new();
        }
        o.revealed = revealed;
        self.retarget_visible(id, now);
        vec![if revealed {
            BarEvent::Revealed { output: id }
        } else {
            BarEvent::Hidden { output: id }
        }]
    }

    /// Fire reveal / hide timers.
    pub fn tick(&mut self, now: f64) -> Vec<BarEvent> {
        let mut ev = Vec::new();
        let ids = self.order.clone();
        for id in ids {
            let (reveal, hide) = {
                let Some(o) = self.outputs.get(&id) else {
                    continue;
                };
                let reveal = !o.revealed
                    && o.dwell_since
                        .is_some_and(|s| now - s >= self.cfg.reveal_dwell - 1e-9);
                let hide = o.revealed
                    && !o.menu_open
                    && o.leave_since
                        .is_some_and(|s| now - s >= self.cfg.hide_delay - 1e-9);
                (reveal, hide)
            };
            if reveal {
                let o = self.outputs.get_mut(&id).expect("output");
                o.revealed = true;
                o.dwell_since = None;
                o.leave_since = None;
                ev.push(BarEvent::Revealed { output: id });
                self.retarget_visible(id, now);
            } else if hide {
                let o = self.outputs.get_mut(&id).expect("output");
                o.revealed = false;
                o.leave_since = None;
                ev.push(BarEvent::Hidden { output: id });
                self.retarget_visible(id, now);
            }
        }
        ev
    }

    pub fn next_deadline(&self) -> Option<f64> {
        self.outputs
            .values()
            .filter_map(|o| match (o.revealed, o.menu_open) {
                (false, _) => o.dwell_since.map(|s| s + self.cfg.reveal_dwell),
                (true, false) => o.leave_since.map(|s| s + self.cfg.hide_delay),
                (true, true) => None,
            })
            .reduce(f64::min)
    }

    pub fn is_revealed(&self, id: OutputId) -> bool {
        self.outputs.get(&id).is_some_and(|o| o.revealed)
    }

    /// Whether the bar currently takes room from windows on `id`.
    pub fn reserves_space(&self, id: OutputId) -> bool {
        self.outputs
            .get(&id)
            .is_some_and(|o| !Self::hidden_target_of(&self.cfg, o))
    }

    /// Height of the bar of `id` in points, whether it shows or is auto-hidden.
    pub fn bar_height(&self, id: OutputId) -> Option<f64> {
        self.outputs.get(&id).map(|o| o.height)
    }

    /// Height the bar takes from windows on `id`: its height, or 0 while it is auto-hidden.
    pub fn reserved_height(&self, id: OutputId) -> Option<f64> {
        let o = self.outputs.get(&id)?;
        Some(if Self::hidden_target_of(&self.cfg, o) {
            0.0
        } else {
            o.height
        })
    }

    /// The output rectangle minus the area reserved by the bar.
    pub fn usable_area(&self, id: OutputId) -> Option<Rect> {
        let o = self.outputs.get(&id)?;
        let h = self.reserved_height(id)?;
        Some(Rect::new(
            o.rect.x,
            o.rect.y + h,
            o.rect.w,
            (o.rect.h - h).max(0.0),
        ))
    }

    pub fn presentation(&self, id: OutputId, now: f64) -> Option<BarPresentation> {
        let o = self.outputs.get(&id)?;
        let visible = o.visible.value(now).clamp(0.0, 1.0);
        let y = o.rect.y - o.height * (1.0 - visible);
        Some(BarPresentation {
            output: id,
            rect: Rect::new(o.rect.x, y, o.rect.w, o.height),
            visible,
            inactive: o.inactive.value(now).clamp(0.0, 1.0),
            animating: o.visible.is_animating(now) || o.inactive.is_animating(now),
        })
    }

    /// Where the title bar (`height` points) of the full-screen window of `id` shows: below the bar it is revealed
    /// with, sliding in and out as one piece. `None` while hidden, and when the bar does not auto-hide there.
    pub fn fullscreen_titlebar(&self, id: OutputId, height: f64, now: f64) -> Option<Rect> {
        let o = self.outputs.get(&id)?;
        if !o.fullscreen || !Self::hidden_target_of(&self.cfg, o) {
            return None;
        }
        let visible = o.visible.value(now).clamp(0.0, 1.0);
        if visible <= 0.0 {
            return None;
        }
        let top = o.rect.y - (o.height + height) * (1.0 - visible) + o.height;
        Some(Rect::new(o.rect.x, top, o.rect.w, height))
    }
}
