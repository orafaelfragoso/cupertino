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
    /// Existing effect vertices in output-local logical points, with their original UVs.
    TexturedMesh {
        snapshot: SnapshotId,
        mesh: Mesh,
    },
    /// Content the renderer still builds itself, by position in its frame-local list. Temporary:
    /// removed in `wp03-scene-effects`, when materials and effect meshes become typed commands.
    LocalPass {
        index: usize,
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
}
