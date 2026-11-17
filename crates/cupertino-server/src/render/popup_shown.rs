//! A popup's own presentation (`cupertino_layers_v1` version 3, [`crate::layers_v1::presentation`]): its offset, its
//! scale about the centre of its window geometry and its opacity, applied to the popup's tree, its subsurfaces' glass
//! and its backdrops, together with the rescale of the window or the presentation of the layer surface it belongs to.
//!
//! Each of those is a scale per axis and an offset ([`Shown`]), so they compose into one, which a tree draws through
//! its pivot and scale.

use smithay::utils::{Logical, Point, Rectangle};

use super::compose::Tree;
use crate::app_effects_v1::BackdropArea;
use crate::layers_v1::Presented;

/// `q` shown at `k * q + t` on each axis, output-local logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub k: (f64, f64),
    pub t: (f64, f64),
}

impl Shown {
    /// Scaling by `k` about `pivot`.
    pub fn about(pivot: Point<f64, Logical>, k: (f64, f64)) -> Shown {
        Shown {
            k,
            t: (pivot.x * (1.0 - k.0), pivot.y * (1.0 - k.1)),
        }
    }

    /// A presentation of a surface whose window geometry is `geometry` (output-local points): moved by its offset, then
    /// scaled about the moved centre.
    pub fn presented(shown: Presented, geometry: Rectangle<f64, Logical>) -> Shown {
        let c = Point::<f64, Logical>::from((
            geometry.loc.x + geometry.size.w / 2.0,
            geometry.loc.y + geometry.size.h / 2.0,
        ));
        let s = shown.scale;
        Shown {
            k: (s, s),
            t: (c.x * (1.0 - s) + shown.x, c.y * (1.0 - s) + shown.y),
        }
    }

    /// `self` and then `after`.
    pub fn then(self, after: Shown) -> Shown {
        Shown {
            k: (self.k.0 * after.k.0, self.k.1 * after.k.1),
            t: (
                after.k.0 * self.t.0 + after.t.0,
                after.k.1 * self.t.1 + after.t.1,
            ),
        }
    }

    pub fn point(&self, q: Point<f64, Logical>) -> Point<f64, Logical> {
        Point::from((self.k.0 * q.x + self.t.0, self.k.1 * q.y + self.t.1))
    }

    pub fn rect(&self, r: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        Rectangle::new(
            self.point(r.loc),
            (r.size.w * self.k.0, r.size.h * self.k.1).into(),
        )
    }

    /// Backdrops moved and scaled, their corners and their glass's blend with the shorter scale.
    pub fn areas(&self, mut areas: Vec<BackdropArea>) -> Vec<BackdropArea> {
        let k = self.k.0.min(self.k.1);
        for a in &mut areas {
            a.area = self.rect(a.area);
            a.radius *= k;
            if let Some(g) = &mut a.glass {
                g.spacing *= k;
            }
        }
        areas
    }

    /// `tree`, whose origin is where its surface lies unscaled, drawn as shown on an output of `scale`: through its pivot
    /// and scale, or moved on an axis it does not scale.
    pub fn tree(&self, mut tree: Tree, scale: f64) -> Tree {
        let (ox, px) = axis(self.k.0, self.t.0, tree.origin.x, scale);
        let (oy, py) = axis(self.k.1, self.t.1, tree.origin.y, scale);
        tree.origin = (ox, oy).into();
        tree.pivot = (px, py).into();
        tree.scale = self.k;
        tree
    }
}

/// The origin and pivot (device pixels) on one axis of a tree drawn at `origin` scaled by `k` and moved by `t` points on
/// an output of `scale`: the pivot is the fixed point of the scaling, and an axis only moved moves the origin.
fn axis(k: f64, t: f64, origin: i32, scale: f64) -> (i32, i32) {
    if (1.0 - k).abs() < 1e-6 {
        (origin + (t * scale).round() as i32, 0)
    } else {
        let pivot = (t * scale / (1.0 - k)).clamp(-1e9, 1e9);
        (origin, pivot.round() as i32)
    }
}
