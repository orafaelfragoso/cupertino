//! Subtrees of a window drawn into surfaces of their own that the compositor places, fades, scales and blurs on its
//! clock ([`WindowSystem::present_surface_layers`](crate::platform::WindowSystem::present_surface_layers)), so a part of a
//! window can move without the window drawing a frame for it.
//!
//! A widget marks its subtree a surface layer with a key and a look ([`SurfaceLayerLook`]); the runtime paints the
//! subtree into a frame of its own, at its layout size, and hands the frames to the window system with the window's.
//! The layer's properties start at its layout: its place, its size, opaque, unscaled, its look's corner radius and
//! sharp. The application changes them, at once or along a curve, with [`WindowRequest::Layer`](crate::platform::WindowRequest);
//! input and the published semantics keep the layout.

use std::sync::Arc;

use cupertino_geometry::Rect;
use cupertino_ui_render::CpuFrame;

use crate::platform::{GlassModifiers, GlassStyle, GlassVariant};

/// A property of a surface layer ([`WindowRequest::Layer`](crate::platform::WindowRequest)).
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
    pub curve: Option<cupertino_theme::animation::Timing>,
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
        curve: Option<cupertino_theme::animation::Timing>,
    ) -> LayerChange {
        LayerChange {
            property,
            to,
            curve,
            velocity: 0.0,
            samples: None,
        }
    }

    /// `property` moved through `values` (2 to [`MAX_LAYER_SAMPLES`]) over `duration` seconds.
    pub fn through(property: LayerProperty, duration: f64, values: Arc<[f32]>) -> LayerChange {
        LayerChange {
            property,
            to: values.last().map_or(0.0, |&v| f64::from(v)),
            curve: Some(cupertino_theme::animation::Easing::LINEAR.over(duration)),
            velocity: 0.0,
            samples: Some(values),
        }
    }
}

/// The most values a change through samples carries ([`LayerChange::through`]): as many as one request to the window
/// system carries.
pub const MAX_LAYER_SAMPLES: usize = cupertino_app_protocols::layers::MAX_SAMPLES;

/// The strongest magnification of a surface layer's glass lens ([`SurfaceLayerLook::lens`]): the compositor's limit.
pub const MAX_LAYER_LENS: f64 = cupertino_app_protocols::layers::LENS.1;

/// How a surface layer looks besides its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceLayerLook {
    /// The glass the compositor draws under the layer's frame, in its shape; `None` draws none.
    pub glass: Option<GlassStyle>,
    /// How much the layer's glass magnifies what it shows, about the centre of its bounds: 1, the default, shows it at
    /// its size, and the window system takes a value below 1 as 1 and one above [`MAX_LAYER_LENS`] as that. A held
    /// knob's lens magnifies the track it slides along. Nothing without glass.
    pub lens: f64,
    /// How the blur of the layer's glass varies from its top edge to its bottom edge ([`BlurRamp`]); the default
    /// blurs evenly. Nothing without glass.
    pub blur_ramp: BlurRamp,
    /// The glass group the layer's glass joins, and the distance in points over which the group's shapes blend: the
    /// glass of the layers of one group is drawn as one shape. 0 joins none.
    pub group: (u32, f64),
    /// The corner radius the layer starts with, in points.
    pub radius: f64,
    /// Cuts the frame to the layer's shape (its size and corner radius as shown).
    pub masks: bool,
    /// The line the compositor draws along the outside of the layer's glass, or of each shape of its group's glass cut
    /// by the others when the layer is the group's first ([`LayerEdge`]); its default draws none. Only where the window system draws edges
    /// ([`Capabilities::surface_layer_edges`](crate::platform::Capabilities::surface_layer_edges)).
    pub edge: LayerEdge,
    /// The system variant and modifiers the layer's glass is named by in place of [`SurfaceLayerLook::glass`]'s
    /// style, which a window system without variants draws instead; the compositor takes the modifiers' `active` from
    /// the window, and grows interactive glass under a press. Nothing without glass.
    pub variant: Option<(GlassVariant, GlassModifiers)>,
    /// A straight sRGB colour laid over the face of the layer's glass at its alpha; alpha 0, the default, lays none.
    /// Nothing without glass.
    pub tint: [f32; 4],
}

impl Default for SurfaceLayerLook {
    /// No glass, a lens that shows what is behind at its size, an even blur, no group, no corners, no mask, no edge,
    /// no variant and no tint.
    fn default() -> SurfaceLayerLook {
        SurfaceLayerLook {
            glass: None,
            lens: 1.0,
            blur_ramp: BlurRamp::EVEN,
            group: (0, 0.0),
            radius: 0.0,
            masks: false,
            edge: LayerEdge::NONE,
            variant: None,
            tint: [0.0; 4],
        }
    }
}

/// How the blur of a layer's glass varies down the layer: `top` times its style's blur along its top edge and `bottom`
/// times it along its bottom edge, smoothly in between, each from 0 (sharp) to 1 (the style's blur), as the soft edge
/// of a bar over content scrolled under it. The window system takes a weight outside 0 to 1 as the nearest of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurRamp {
    pub top: f64,
    pub bottom: f64,
}

impl BlurRamp {
    /// The style's blur all the way down.
    pub const EVEN: BlurRamp = BlurRamp {
        top: 1.0,
        bottom: 1.0,
    };
}

impl Default for BlurRamp {
    fn default() -> BlurRamp {
        BlurRamp::EVEN
    }
}

/// A line along the outside of a layer's glass: `width` points wide whatever the layer's scale, of the straight colour
/// `color` (red, green, blue and opacity, 0 to 1), drawn over what lies there and over the glass's own edge as a stroke
/// around the shape is. In a glass group it follows the outline of the group's joined shape as it is shown, so it is
/// left out where the shapes join. A width of 0 draws none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerEdge {
    pub width: f64,
    pub color: [f32; 4],
}

impl LayerEdge {
    /// No edge.
    pub const NONE: LayerEdge = LayerEdge {
        width: 0.0,
        color: [0.0; 4],
    };
}

/// A marked subtree's frame as the window system shows it ([`WindowSystem::present_surface_layers`]).
///
/// [`WindowSystem::present_surface_layers`]: crate::platform::WindowSystem::present_surface_layers
#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayer<'a> {
    /// The application's name for the layer, unique within its window.
    pub key: u64,
    /// The layer's layout, in the window's points: where it starts, and the size its frame is drawn at.
    pub rect: Rect,
    pub look: SurfaceLayerLook,
    /// Its frame, its size in whole points ([`surface_layer_size`]) at the window's scale; `None` when it has not
    /// changed since it was last shown.
    pub frame: Option<&'a CpuFrame>,
}

/// The size a surface layer laid out at `rect` is drawn at, in whole points: each side rounded, and at least one point.
/// Its frame and its buffers are this size at the window's scale, rounded to whole pixels as a window's are.
pub fn surface_layer_size(rect: Rect) -> (u32, u32) {
    let side = |points: f64| points.round().max(1.0) as u32;
    (side(rect.width()), side(rect.height()))
}
