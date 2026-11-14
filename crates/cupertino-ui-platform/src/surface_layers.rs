//! Subtrees of a window drawn into surfaces of their own that the compositor places, fades, scales and blurs on its
//! clock ([`WindowSystem::present_surface_layers`](crate::WindowSystem::present_surface_layers)), so a part of a
//! window can move without the window drawing a frame for it.
//!
//! A widget marks its subtree a surface layer with a key and a look ([`SurfaceLayerLook`]); the runtime paints the
//! subtree into a frame of its own, at its layout size, and hands the frames to the window system with the window's.
//! The layer's properties start at its layout: its place, its size, opaque, unscaled, its look's corner radius and
//! sharp. The application changes them, at once or along a curve, with [`WindowRequest::Layer`](crate::WindowRequest);
//! input and the published semantics keep the layout.

use std::sync::Arc;

use cupertino_geometry::Rect;
use cupertino_ui_render::CpuFrame;

use crate::GlassStyle;

/// A property of a surface layer ([`WindowRequest::Layer`](crate::WindowRequest)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerProperty {
    /// Its left edge in the window, in points.
    X,
    /// Its top edge in the window, in points.
    Y,
    /// Its width in points: its glass and its mask follow; its frame is drawn at its layout size, from its top left.
    Width,
    /// Its height in points, as its width.
    Height,
    /// From 0 (hidden) to 1, its glass with it.
    Opacity,
    /// Scales all the layer draws about its centre, its glass with it.
    Scale,
    /// The radius of its glass and its mask's corners, in points.
    CornerRadius,
    /// The standard deviation of a Gaussian blur of its frame, in its points before its scale; its glass is never
    /// blurred.
    ContentBlur,
}

/// One change of a surface layer: `property` goes to `to` at once (`curve` `None`), or along `curve` from what is
/// shown when the change is first drawn, keeping its velocity; `velocity` (units per second) starts a spring from rest.
/// With `samples` the property moves through them instead ([`LayerChange::through`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerChange {
    pub property: LayerProperty,
    pub to: f64,
    pub curve: Option<cupertino_animation::LayerCurve>,
    pub velocity: f64,
    /// The property's values at even steps of a timed `curve`'s duration (its Bézier unused), the first at its start
    /// and the last, `to`, at its end, in a straight line between two: a motion the application works out itself (a
    /// product of two springs, a clamped ramp) carried as it is. It starts at the first value, not from what is shown.
    pub samples: Option<Arc<[f32]>>,
}

impl LayerChange {
    /// `property` set to `to` at once (`curve` `None`) or animated along `curve`.
    pub fn new(
        property: LayerProperty,
        to: f64,
        curve: Option<cupertino_animation::LayerCurve>,
    ) -> LayerChange {
        LayerChange {
            property,
            to,
            curve,
            velocity: 0.0,
            samples: None,
        }
    }

    /// `property` moved through `values` (at least two) over `duration` seconds.
    pub fn through(property: LayerProperty, duration: f64, values: Arc<[f32]>) -> LayerChange {
        LayerChange {
            property,
            to: values.last().map_or(0.0, |&v| f64::from(v)),
            curve: Some(cupertino_animation::LayerCurve::Timed {
                duration,
                bezier: cupertino_animation::CubicBezier::new(0.0, 0.0, 1.0, 1.0),
            }),
            velocity: 0.0,
            samples: Some(values),
        }
    }
}

/// How a surface layer looks besides its frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceLayerLook {
    /// The glass the compositor draws under the layer's frame, in its shape; `None` draws none.
    pub glass: Option<GlassStyle>,
    /// The glass group the layer's glass joins, and the distance in points over which the group's shapes blend: the
    /// glass of the layers of one group is drawn as one shape. 0 joins none.
    pub group: (u32, f64),
    /// The corner radius the layer starts with, in points.
    pub radius: f64,
    /// Cuts the frame to the layer's shape (its size and corner radius as shown).
    pub masks: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_through_samples_rests_at_its_last_over_a_timed_curve_of_its_duration() {
        let change = LayerChange::through(LayerProperty::Scale, 0.25, [1.1, 0.98, 1.0].into());
        assert_eq!(change.to, 1.0, "where it rests");
        assert_eq!(change.velocity, 0.0);
        assert!(matches!(
            change.curve,
            Some(cupertino_animation::LayerCurve::Timed { duration, .. }) if duration == 0.25
        ));
        assert_eq!(change.samples.as_deref(), Some(&[1.1_f32, 0.98, 1.0][..]));
        let set = LayerChange::new(LayerProperty::Opacity, 0.5, None);
        assert_eq!((set.to, set.curve, set.samples), (0.5, None, None));
    }
}

/// A marked subtree's frame as the window system shows it ([`WindowSystem::present_surface_layers`]).
///
/// [`WindowSystem::present_surface_layers`]: crate::WindowSystem::present_surface_layers
#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayer<'a> {
    /// The application's name for the layer, unique within its window.
    pub key: u64,
    /// The layer's layout, in the window's points: where it starts, and the size its frame is drawn at.
    pub rect: Rect,
    pub look: SurfaceLayerLook,
    /// Its frame, `rect`'s size at the window's scale; `None` when it has not changed since it was last shown.
    pub frame: Option<&'a CpuFrame>,
}
