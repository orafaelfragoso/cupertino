//! Renderer-neutral desktop scene data. Composition samples models once; adapters resolve the
//! typed resources and retain their leases through rendering and presentation completion.

mod coordinates;
mod layers;
mod source;

pub use coordinates::{
    BufferCrop, BufferRect, GlobalRect, OutputPoint, OutputRect, PhysicalSize, Transform,
};
pub use layers::{LayerStack, SceneLayer};
pub use source::{
    AlphaMode, BorderFalloff, Clip, ContentBlur, GlassId, GlassPass, ImageId, ImageMapping,
    ImageRole, MaterialPass, MaterialStage, Placement, RecipeId, ResourceKey, RoundedRect, Shadow,
    SnapshotId, Source, SurfaceId, SurfaceRole, VibrantBlend,
};

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutputId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameId(pub u64);

/// What a composed item is, for stable identity. `key` is the renderer's cache-slot name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneKind {
    Wallpaper,
    LockBackdrop,
    WallpaperImage,
    PowerOverlay,
    OverviewDim,
    SnapPreview,
    Shadow,
    PopupRim,
    PopupOutline,
    PopupShadow,
    WindowContent,
    PopupContent,
    LayerContent,
    DragIcon,
    LockContent,
    CursorContent,
    TitleBar,
    Rim,
    /// The light rim just inside a window whose client draws its own frame, over its content.
    ClientRim,
    Outline,
    CursorImage,
    InputHud,
    ShortcutNotice,
    AccentPopup,
    MinimizeMesh,
    ClosingGhost,
    InputHudTint,
    InputHudMaterial,
    AccentMaterial,
    Backdrop,
    BackdropCapture,
    SheetDimming,
    DisplayFilter,
    InputHudShadow,
    AccentShadow,
}

impl SceneKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Wallpaper => "wallpaper",
            Self::LockBackdrop => "lock-backdrop",
            Self::WallpaperImage => "wallpaper-image",
            Self::PowerOverlay => "power-overlay",
            Self::OverviewDim => "overview-dim",
            Self::SnapPreview => "snap-preview",
            Self::Shadow => "shadow",
            Self::PopupRim => "popup-rim",
            Self::PopupOutline => "popup-outline",
            Self::PopupShadow => "popup-shadow",
            Self::WindowContent => "window-content",
            Self::PopupContent => "popup-content",
            Self::LayerContent => "layer-content",
            Self::DragIcon => "drag-icon",
            Self::LockContent => "lock-content",
            Self::CursorContent => "cursor-content",
            Self::TitleBar => "title-bar",
            Self::Rim => "rim",
            Self::ClientRim => "client-rim",
            Self::Outline => "outline",
            Self::CursorImage => "cursor-image",
            Self::InputHud => "input-hud",
            Self::ShortcutNotice => "shortcut-notice",
            Self::AccentPopup => "accent-popup",
            Self::MinimizeMesh => "minimize-mesh",
            Self::ClosingGhost => "closing-ghost",
            Self::InputHudTint => "input-hud-tint",
            Self::InputHudMaterial => "input-hud-material",
            Self::AccentMaterial => "accent-material",
            Self::Backdrop => "backdrop",
            Self::BackdropCapture => "backdrop-capture",
            Self::SheetDimming => "sheet-dimming",
            Self::DisplayFilter => "display-filter",
            Self::InputHudShadow => "input-hud-shadow",
            Self::AccentShadow => "accent-shadow",
        }
    }
}

/// Stable identity of a composed item, independent of the frame sequence and source revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneId {
    pub kind: SceneKind,
    pub index: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub id: OutputId,
    pub rect: GlobalRect,
    pub physical_size: PhysicalSize,
    pub scale: f64,
    pub transform: Transform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DamageRegions {
    Buffer(Vec<BufferRect>),
    /// Read each subsurface's committed damage from the leased surface tree, without widening it.
    CommittedSurfaceTree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentDamage {
    Unchanged,
    Full,
    Regions(DamageRegions),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputInvalidation {
    /// The renderer still accounts for commits, geometry, backdrop dependencies and buffer age.
    Incremental,
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneItem {
    pub id: SceneId,
    /// Generated-content revision; never substituted for committed surface-tree counters.
    pub revision: u64,
    pub source: Source,
    pub placement: Placement,
    pub opacity: f64,
    pub clip: Clip,
    /// Content changes only; moving, removing or changing a clip also invalidates visuals.
    pub damage: ContentDamage,
}

#[derive(Debug)]
pub struct SceneFrame {
    pub output: Output,
    pub id: FrameId,
    pub timestamp: Duration,
    /// Straight RGBA, matching the caller's target clear colour.
    pub clear: [f32; 4],
    pub invalidation: OutputInvalidation,
    items: Vec<(SceneLayer, SceneItem)>,
}

impl SceneFrame {
    /// Flatten once into front-to-back order; within each layer the caller supplies that order.
    pub fn new(
        output: Output,
        id: FrameId,
        timestamp: Duration,
        clear: [f32; 4],
        invalidation: OutputInvalidation,
        items: LayerStack<SceneItem>,
    ) -> Self {
        let items = items.flatten_with_layer();
        debug_assert!(
            captures_below_composites(&items),
            "a material composite needs its capture below it in the scene"
        );
        Self {
            output,
            id,
            timestamp,
            clear,
            invalidation,
            items,
        }
    }

    pub fn items(&self) -> &[(SceneLayer, SceneItem)] {
        &self.items
    }
}

/// `items` is front to back, so a composite's capture must come after it.
fn captures_below_composites(items: &[(SceneLayer, SceneItem)]) -> bool {
    items
        .iter()
        .enumerate()
        .all(|(at, (_, item))| match &item.source {
            Source::Material(MaterialPass {
                stage: MaterialStage::Composite { owner },
                ..
            }) => items[at + 1..].iter().any(|(_, below)| {
                matches!(
                    &below.source,
                    Source::Material(MaterialPass {
                        stage: MaterialStage::Capture { owner: o },
                        ..
                    }) if o == owner
                )
            }),
            _ => true,
        })
}
