//! The press response of interactive glass (the `interactive` modifier of `set_glass_variant`): while the primary
//! button pressed over a surface is held, the glass of its backdrops and layers under the press grows uniformly about
//! the centre of its shape along a spring on the compositor's clock, overshooting a little, and on the release it
//! lets go with a small undershoot. The corners are circular while the glass is deformed ([`DEFORMED`]); what the
//! glass filters is not changed, and a drag after the press does not stretch it. Under Reduce Motion the scene does not
//! ask, so the glass does not grow.
//!
//! One press is followed at a time, and the one it replaced lets go as it was; a press costs frames only once a frame
//! found interactive glass under it.

use std::cell::Cell;

use cupertino_animation::{LayerCurve, LayerProperty};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};

/// How much interactive glass grows under a press, uniformly about its centre: 160 × 60 points settle at
/// 162.24 × 60.84.
pub const PRESS_SCALE: f64 = 1.014;
/// The period in seconds of the undamped spring the glass grows and lets go along: fitted so that 160 points grow to
/// about 161.5 at 100 ms, peak near 162.4 at about 200 ms and are back at 162.25 by 300 ms.
pub const PRESS_RESPONSE: f64 = 0.33;
/// The damping ratio of that spring: the growth overshoots by about 7 % of itself, and the release undershoots as much
/// (to about 159.8 of 160 points) and is at rest by 500 ms.
pub const PRESS_DAMPING: f64 = 0.64;
/// How far from 1 the scale is when the glass counts as deformed, its corners then circular rather than continuous.
pub const DEFORMED: f64 = 1e-4;
/// How close, in logical points, a press lands to the previous one on the same surface for the two to be one glass's
/// (a double click): the second retargets the first's motion rather than starting another.
const SAME_PRESS_SLOP: f64 = 4.0;

/// The spring of the growth and of the release.
fn spring() -> LayerCurve {
    LayerCurve::Spring {
        response: PRESS_RESPONSE,
        damping: PRESS_DAMPING,
    }
}

/// A press and the scale it gives the glass under it.
#[derive(Debug)]
struct Press {
    /// Where the button went down, in global logical points.
    at: Point<f64, Logical>,
    /// The root of the surface tree pressed: a window's, a popup's or a layer shell surface's main surface.
    root: WlSurface,
    held: bool,
    scale: LayerProperty,
    /// Whether a frame drew interactive glass under this press: until one does, it asks for no frames, so a press
    /// beside any interactive glass costs none.
    hit: Cell<bool>,
}

impl Press {
    /// The scale of glass of a surface of the tree `owner` over `area` (global logical points) at `now`: grown when the
    /// press lies in the area, 1 otherwise.
    fn scale_of(&self, owner: &WlSurface, area: Rectangle<f64, Logical>, now: f64) -> Option<f64> {
        let under = self.root == *owner && area.contains(self.at);
        if under {
            self.hit.set(true);
        }
        under.then(|| self.scale.value(now))
    }
}

/// The press interactive glass follows, and the one before it while it lets go.
#[derive(Debug, Default)]
pub struct GlassPress {
    current: Option<Press>,
    previous: Option<Press>,
}

impl GlassPress {
    /// The primary button went down at `at` (global logical points) over `surface` at `now`.
    pub fn press(&mut self, at: Point<f64, Logical>, surface: &WlSurface, now: f64) {
        let root = root_of(surface);
        let again = self.current.as_ref().is_some_and(|p| {
            p.root == root && (p.at.x - at.x).hypot(p.at.y - at.y) <= SAME_PRESS_SLOP
        });
        if !again {
            self.previous = self
                .current
                .take()
                .filter(|p| p.scale.is_animating(now) || p.held);
            if let Some(p) = self.previous.as_mut().filter(|p| p.held) {
                p.held = false;
                p.scale.animate(1.0, spring(), now, 0.0);
            }
        }
        let press = self.current.get_or_insert_with(|| Press {
            at,
            root: root.clone(),
            held: false,
            scale: LayerProperty::new(1.0),
            hit: Cell::new(false),
        });
        press.held = true;
        press.scale.animate(PRESS_SCALE, spring(), now, 0.0);
    }

    /// The primary button went up at `now`: the glass lets go.
    pub fn release(&mut self, now: f64) {
        if let Some(p) = self.current.as_mut().filter(|p| p.held) {
            p.held = false;
            p.scale.animate(1.0, spring(), now, 0.0);
        }
    }

    /// The scale about its centre of interactive glass of a surface of the tree `owner` over `area` (global logical
    /// points) at `now`: grown or letting go under a press that went down in the area, 1 otherwise.
    pub fn scale_of(&self, owner: &WlSurface, area: Rectangle<f64, Logical>, now: f64) -> f64 {
        let owner = root_of(owner);
        [&self.current, &self.previous]
            .into_iter()
            .flatten()
            .find_map(|p| p.scale_of(&owner, area, now))
            .unwrap_or(1.0)
    }

    /// The scale the glass under the press shows at `now`, 1 without a press.
    pub fn scale(&self, now: f64) -> f64 {
        self.current.as_ref().map_or(1.0, |p| p.scale.value(now))
    }

    /// Whether interactive glass under a press is still moving at `now`, so outputs draw frames.
    pub fn wants_tick(&self, now: f64) -> bool {
        [&self.current, &self.previous]
            .into_iter()
            .flatten()
            .any(|p| p.hit.get() && p.scale.is_animating(now))
    }
}

/// The root of `surface`'s tree: the surface itself, or the main surface its subsurfaces lie on.
fn root_of(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
        root = parent;
    }
    root
}
