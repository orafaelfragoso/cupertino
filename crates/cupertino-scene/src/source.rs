//! Typed resource references and paint data. Resource storage and leases belong to the server.

use cupertino_effects::Mesh;
use cupertino_geometry::{CornerStyle, Point};

use crate::{BufferCrop, OutputPoint, OutputRect, Transform};

/// A registry slot plus its incarnation; reusing a slot must change its generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub slot: u64,
    pub generation: u64,
}

/// A live tree cannot be replaced by a snapshot handle, even if their registry slots match.
///
/// ```compile_fail,E0308
/// use cupertino_scene::{ResourceKey, SnapshotId, SurfaceId};
/// let _: SurfaceId = SnapshotId(ResourceKey { slot: 7, generation: 2 });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub ResourceKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageId(pub ResourceKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SnapshotId(pub ResourceKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecipeId(pub ResourceKey);

/// A glass container of the frame: its shapes, style and blend width, resolved by the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlassId(pub ResourceKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    Straight,
    Premultiplied,
}

/// Metadata for an image or snapshot. Pixel storage is pinned by the frame resource lease.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageMapping {
    pub buffer_size: (i32, i32),
    pub buffer_scale: f64,
    pub transform: Transform,
    pub crop: Option<BufferCrop>,
    pub alpha: AlphaMode,
}

/// Translation and rescale are kept separate; the renderer retains its existing operation order.
/// Positions that the renderer places on whole device pixels are given already on that grid, as
/// device pixels divided by the output scale, so converting back reproduces them exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub origin: OutputPoint,
    pub pivot: OutputPoint,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl Placement {
    /// Drawn where its source says, at its own size.
    pub const IDENTITY: Self = Self {
        origin: OutputPoint(Point::ZERO),
        pivot: OutputPoint(Point::ZERO),
        scale_x: 1.0,
        scale_y: 1.0,
    };
}

/// Masks are evaluated before the item's rescale, in output-local logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clip {
    None,
    Rect(OutputRect),
    Rounded {
        rect: OutputRect,
        radius: f64,
        style: CornerStyle,
        /// The item is drawn unscaled, so the mask's interior can be reported as opaque.
        exact: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedRect {
    pub rect: OutputRect,
    pub radius: f64,
    pub style: CornerStyle,
    /// Straight RGBA; opacity is carried by the scene item.
    pub fill: [f32; 4],
    /// Width in logical points and straight RGBA colour.
    pub border: Option<(f64, [f32; 4])>,
    /// How the border's strength follows the direction of the edge; the default is the same all around.
    pub border_falloff: BorderFalloff,
}

/// How the strength of a rounded rectangle's border follows the direction of its edge. With `n` the unit outward
/// normal, the border is drawn at `floor + (1 − floor) · |n.x|^sides · |n.y|^ends`, where a power of 0 leaves its
/// factor out: `sides` keeps the border along the left and right edges and fades it towards the top and bottom,
/// `ends` the other way round. All zero (the default) draws it the same all around.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BorderFalloff {
    pub sides: f32,
    pub ends: f32,
    /// The share of the border left where the powers give nothing.
    pub floor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub area: OutputRect,
    pub caster: OutputRect,
    pub radius: f64,
    pub sigma: f64,
    pub density: f64,
    pub offset_y: f64,
    pub style: CornerStyle,
    pub cutout: bool,
}

/// Capture and composite share an owner, preserving the backdrop dependency in scene order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialStage {
    Full,
    Capture { owner: u64 },
    Composite { owner: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialPass {
    pub recipe: RecipeId,
    pub rect: OutputRect,
    pub radius: f64,
    pub style: CornerStyle,
    pub stage: MaterialStage,
    /// Average colour of the wallpaper below, which tints the material.
    pub tint: [f32; 3],
}

/// One glass container: its shapes joined into one outline over the backdrop below. `rect` is what of the backdrop
/// it reads, the shapes grown by the reach of the blur and the refraction; the pass draws nothing past it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassPass {
    pub glass: GlassId,
    pub rect: OutputRect,
}

/// Preserve live-surface import and scanout eligibility when choosing the renderer element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Rescaled about the placement pivot, masked when the item has a clip.
    WindowContent,
    /// Rescaled about the placement pivot with its window, never masked.
    Popup,
    /// Drawn at its placement as it is: layer-shell surfaces, drag icon, lock surface.
    Standalone,
    /// A client's pointer image, enlarged by the placement scale and eligible for the cursor plane.
    Cursor,
    /// A layer's content whose coverage is drawn in a colour taken from what lies behind it, rescaled and masked as
    /// window content.
    Vibrant(VibrantBlend),
    /// A layer's content blurred as one picture, transparent beyond its edges, by a Gaussian, then rescaled and masked
    /// as window content.
    Blurred(ContentBlur),
}

/// The blur of a layer's content: the standard deviation of its Gaussian in thousandths of a logical point, before
/// the placement's scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentBlur {
    pub sigma_milli: u32,
}

impl ContentBlur {
    /// The blur of `sigma` points, to a thousandth.
    pub fn of_points(sigma: f64) -> Self {
        Self {
            sigma_milli: (sigma.max(0.0) * 1000.0).round().min(f64::from(u32::MAX)) as u32,
        }
    }

    /// The standard deviation in logical points.
    pub fn sigma(self) -> f64 {
        f64::from(self.sigma_milli) / 1000.0
    }
}

/// How a vibrant layer mixes with what lies behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VibrantBlend {
    /// Over a dark face the coverage lifts what lies behind towards white, over a light one it darkens it.
    pub dark: bool,
    /// The plain label colour of the appearance instead (reduced transparency, increased contrast).
    pub plain: bool,
    /// Weight of the coverage in thousandths: the label's opacity.
    pub weight: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    /// Drawn at its placement as it is.
    Standalone,
    /// Rescaled about the placement pivot with its window, masked when the item has a clip.
    Decoration,
    /// The pointer image, eligible for the cursor plane.
    Cursor,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Subsurface flattening, buffer import and committed damage remain adapter-owned.
    SurfaceTree {
        id: SurfaceId,
        role: SurfaceRole,
    },
    Image {
        id: ImageId,
        mapping: ImageMapping,
        role: ImageRole,
        /// Size on the output in logical points; `mapping.crop` selects what of the buffer is shown in it.
        size: (i32, i32),
    },
    Snapshot {
        id: SnapshotId,
        mapping: ImageMapping,
    },
    RoundedRect(RoundedRect),
    Shadow(Shadow),
    /// A field of four straight RGBA colours: the procedural wallpaper's, or equal colours for a solid fill.
    /// Opaque when the item's opacity is 1.
    Wallpaper {
        rect: OutputRect,
        colours: [[f32; 4]; 4],
    },
    Material(MaterialPass),
    Glass(GlassPass),
    /// Existing effect vertices in output-local logical points, with their original UVs.
    TexturedMesh {
        snapshot: SnapshotId,
        mesh: Mesh,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn reused_resource_slots_do_not_alias_the_previous_generation() {
        let old = SnapshotId(ResourceKey {
            slot: 7,
            generation: 2,
        });
        let new = SnapshotId(ResourceKey {
            slot: 7,
            generation: 3,
        });
        assert_ne!(old, new);
        assert_eq!(HashSet::from([old, new]).len(), 2);
    }

    #[test]
    fn a_content_blur_keeps_its_deviation_to_a_thousandth_of_a_point() {
        assert_eq!(ContentBlur::of_points(2.5).sigma_milli, 2500);
        assert_eq!(ContentBlur::of_points(0.0004).sigma_milli, 0);
        assert_eq!(ContentBlur::of_points(-3.0).sigma_milli, 0);
        assert_eq!(ContentBlur::of_points(1.2344).sigma(), 1.234);
    }
}
