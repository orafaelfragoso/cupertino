//! Auto-hide state machine (structure derived from the visibility settings of)
//! and the duration formula.
//!
//! The delay before a reveal (default 0.2 s; a hide has none) and the show/hide duration formula are as designed;
//! the state machine around them is. The offset is eased with the motion catalogue's cosine ease.
//!
//! ```text
//! Hidden ─hot→ WaitingToShow ─delay→ Showing ─done→ Shown
//! Shown ─cold→ WaitingToHide ─delay→ Hiding ─done→ Hidden
//! Hiding ─hot→ Showing (reverse in place, C¹) WaitingToShow ─cold→ Hidden
//! ```

use cupertino_kit::motion::{Animated, Easing};
use cupertino_wm::geometry::Rect;

use crate::config::{DockConfig, Orientation};

/// How long the pointer must have left the dock before it starts to slide away: none, the slide starts at once.
pub const HIDE_DELAY: f64 = 0.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HideState {
    Shown,
    WaitingToHide { since: f64 },
    Hiding,
    Hidden,
    WaitingToShow { since: f64 },
    Showing,
}

pub struct AutoHide {
    pub state: HideState,
    pub delay: f64,
    pub duration: f64,
    /// 0 = fully shown, 1 = fully hidden.
    pub offset: Animated<f64>,
}

impl AutoHide {
    pub fn new(delay: f64, duration: f64) -> Self {
        Self {
            state: HideState::Hidden,
            delay,
            duration,
            offset: Animated::new(1.0),
        }
    }

    /// Start in the shown state (e.g. auto-hide disabled).
    pub fn shown(delay: f64, duration: f64) -> Self {
        Self {
            state: HideState::Shown,
            delay,
            duration,
            offset: Animated::new(0.0),
        }
    }

    /// `hot`: the pointer is in the reveal zone (screen edge) or over the visible dock.
    pub fn update(&mut self, hot: bool, now: f64) {
        let curve = Easing::COSINE.over(self.duration);
        self.state = match (self.state, hot) {
            (HideState::Hidden, true) => HideState::WaitingToShow { since: now },
            (HideState::WaitingToShow { since }, true) if now - since >= self.delay => {
                self.offset.animate_to(0.0, curve, now);
                HideState::Showing
            }
            (HideState::WaitingToShow { .. }, false) => HideState::Hidden,
            (HideState::Showing, _) if !self.offset.is_animating(now) => HideState::Shown,
            (HideState::Showing, false) | (HideState::Shown, false) if HIDE_DELAY <= 0.0 => {
                self.offset.animate_to(1.0, curve, now);
                HideState::Hiding
            }
            (HideState::Showing, false) | (HideState::Shown, false) => {
                HideState::WaitingToHide { since: now }
            }
            (HideState::WaitingToHide { .. }, true) => HideState::Shown,
            (HideState::WaitingToHide { since }, false) if now - since >= HIDE_DELAY => {
                self.offset.animate_to(1.0, curve, now);
                HideState::Hiding
            }
            (HideState::Hiding, true) => {
                self.offset.animate_to(0.0, curve, now);
                HideState::Showing
            }
            (HideState::Hiding, false) if !self.offset.is_animating(now) => HideState::Hidden,
            (s, _) => s,
        };
    }

    pub fn hidden_fraction(&self, now: f64) -> f64 {
        self.offset.value(now).clamp(0.0, 1.0)
    }
}

/// Duration in milliseconds of the hide animation:
/// `floor(ln(|offset − (top + tile + bottom + fish)| · 0.5 + 1) + 1) · speed` where `offset` is the
/// current hidden offset `(top + tile + bottom + 10) · hidden_fraction`, `fish` the current
/// magnification amount in points (0 … large − tile) and `speed = time_modifier · 60`.
/// For the default 64 pt dock this is 240 ms, 300 ms with the magnification fully open.
pub fn autohide_hide_ms(cfg: &DockConfig, hidden_fraction: f64, current_fish: f64) -> f64 {
    let bar = cfg.bar_extent();
    let offset = (bar + 10.0) * hidden_fraction.clamp(0.0, 1.0);
    let dist = (offset - (bar + current_fish)).abs();
    ((dist * 0.5 + 1.0).ln() + 1.0).floor() * cfg.autohide_speed_ms()
}

/// Duration in milliseconds of the show animation: same form with
/// `dist = |X − (bar + fish − offset)|`, `X` the large size when the magnification is active under
/// the pointer, else the tile size.
pub fn autohide_show_ms(
    cfg: &DockConfig,
    hidden_fraction: f64,
    current_fish: f64,
    magnified_under_pointer: bool,
) -> f64 {
    let bar = cfg.bar_extent();
    let offset = (bar + 10.0) * hidden_fraction.clamp(0.0, 1.0);
    let x = if cfg.magnification && magnified_under_pointer {
        cfg.large_size
    } else {
        cfg.tile_size
    };
    let dist = (x - (bar + current_fish - offset)).abs();
    ((dist * 0.5 + 1.0).ln() + 1.0).floor() * cfg.autohide_speed_ms()
}

/// Strip at the dock edge of `screen` in which the pointer reveals a hidden dock.
pub fn reveal_zone(orientation: Orientation, screen: Rect, thickness: f64) -> Rect {
    match orientation {
        Orientation::Bottom => {
            Rect::new(screen.x, screen.bottom() - thickness, screen.w, thickness)
        }
        Orientation::Left => Rect::new(screen.x, screen.y, thickness, screen.h),
        Orientation::Right => Rect::new(screen.right() - thickness, screen.y, thickness, screen.h),
    }
}
