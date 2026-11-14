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
    AlphaMode, BorderFalloff, Clip, GlassId, GlassPass, ImageId, ImageMapping, ImageRole,
    MaterialPass, MaterialStage, Placement, RecipeId, ResourceKey, RoundedRect, Shadow, SnapshotId,
    Source, SurfaceId, SurfaceRole, VibrantBlend,
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
    ActivationFade,
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
    #[cfg(test)]
    Test,
}

impl SceneKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Wallpaper => "wallpaper",
            Self::LockBackdrop => "lock-backdrop",
            Self::WallpaperImage => "wallpaper-image",
            Self::ActivationFade => "activation-fade",
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
            #[cfg(test)]
            Self::Test => "test",
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::{Point, Rect};

    fn item(index: u64, damage: ContentDamage) -> SceneItem {
        SceneItem {
            id: SceneId {
                kind: SceneKind::Test,
                index,
            },
            revision: 3,
            source: Source::SurfaceTree {
                id: SurfaceId(ResourceKey {
                    slot: index,
                    generation: 1,
                }),
                role: SurfaceRole::WindowContent,
            },
            placement: Placement {
                origin: OutputPoint(Point::ZERO),
                pivot: OutputPoint(Point::ZERO),
                scale_x: 1.0,
                scale_y: 1.0,
            },
            opacity: 1.0,
            clip: Clip::None,
            damage,
        }
    }

    #[test]
    fn frame_keeps_order_and_does_not_inflate_content_damage() {
        let partial = ContentDamage::Regions(DamageRegions::Buffer(vec![BufferRect {
            x: 4,
            y: 8,
            width: 12,
            height: 16,
        }]));
        let mut items = LayerStack::new();
        items.push(SceneLayer::Wallpaper, item(0, ContentDamage::Full));
        items.push(SceneLayer::Windows, item(1, ContentDamage::Unchanged));
        let mut image = item(2, partial.clone());
        image.source = Source::Image {
            id: ImageId(ResourceKey {
                slot: 2,
                generation: 1,
            }),
            mapping: ImageMapping {
                buffer_size: (100, 100),
                buffer_scale: 1.25,
                transform: Transform::Normal,
                crop: Some(BufferCrop(Rect::new(0.25, 0.5, 90.0, 90.0))),
                alpha: AlphaMode::Premultiplied,
            },
            role: ImageRole::Standalone,
            size: (90, 90),
        };
        items.push(SceneLayer::Windows, image);
        items.push(
            SceneLayer::Cursor,
            item(
                3,
                ContentDamage::Regions(DamageRegions::CommittedSurfaceTree),
            ),
        );
        let frame = SceneFrame::new(
            Output {
                id: OutputId(1),
                rect: GlobalRect(Rect::new(0.0, 0.0, 800.0, 600.0)),
                physical_size: PhysicalSize {
                    width: 1000,
                    height: 750,
                },
                scale: 1.25,
                transform: Transform::Normal,
            },
            FrameId(10),
            Duration::from_millis(125),
            [0.0, 0.0, 0.0, 1.0],
            OutputInvalidation::Incremental,
            items,
        );
        assert_eq!(
            frame
                .items()
                .iter()
                .map(|(_, i)| i.id.index)
                .collect::<Vec<_>>(),
            vec![3, 1, 2, 0]
        );
        assert_eq!(frame.items()[1].1.damage, ContentDamage::Unchanged);
        assert_eq!(frame.items()[2].1.damage, partial);
        assert_eq!(frame.items()[3].1.damage, ContentDamage::Full);
        assert_eq!(frame.items()[1].1.revision, 3);
        assert_eq!(frame.invalidation, OutputInvalidation::Incremental);
    }
    fn material(index: u64, stage: MaterialStage) -> SceneItem {
        let mut i = item(index, ContentDamage::Full);
        i.source = Source::Material(MaterialPass {
            recipe: RecipeId(ResourceKey {
                slot: 0,
                generation: 1,
            }),
            rect: OutputRect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            radius: 0.0,
            style: cupertino_geometry::CornerStyle::Circular,
            stage,
            tint: [0.0; 3],
        });
        i
    }

    fn frame_with(items: LayerStack<SceneItem>) -> SceneFrame {
        SceneFrame::new(
            Output {
                id: OutputId(1),
                rect: GlobalRect(Rect::new(0.0, 0.0, 800.0, 600.0)),
                physical_size: PhysicalSize {
                    width: 800,
                    height: 600,
                },
                scale: 1.0,
                transform: Transform::Normal,
            },
            FrameId(1),
            Duration::ZERO,
            [0.0; 4],
            OutputInvalidation::Incremental,
            items,
        )
    }

    #[test]
    fn composite_with_its_capture_below_is_accepted() {
        let mut items = LayerStack::new();
        items.push(
            SceneLayer::BackdropCapture,
            material(1, MaterialStage::Capture { owner: 7 }),
        );
        items.push(
            SceneLayer::LayerTop,
            material(2, MaterialStage::Composite { owner: 7 }),
        );
        assert_eq!(frame_with(items).items().len(), 2);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "needs its capture below")]
    fn composite_above_nothing_is_rejected() {
        let mut items = LayerStack::new();
        items.push(
            SceneLayer::LayerTop,
            material(2, MaterialStage::Composite { owner: 7 }),
        );
        items.push(
            SceneLayer::BackdropCapture,
            material(1, MaterialStage::Capture { owner: 8 }),
        );
        frame_with(items);
    }
}
