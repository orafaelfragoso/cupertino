//! Typed resource references and paint data. Resource storage and leases belong to the server.

use cupertino_effects::Mesh;
use cupertino_geometry::CornerStyle;

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub origin: OutputPoint,
    pub pivot: OutputPoint,
    pub scale_x: f64,
    pub scale_y: f64,
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
    WindowContent,
    Popup,
    Standalone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    Standalone,
    Decoration,
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
    },
    Snapshot {
        id: SnapshotId,
        mapping: ImageMapping,
    },
    RoundedRect(RoundedRect),
    Shadow(Shadow),
    /// The four existing procedural-wallpaper colours, as straight RGBA.
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
    /// Content the renderer still builds itself, by position in its frame-local list. Removed
    /// when materials and effect meshes become typed commands.
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
