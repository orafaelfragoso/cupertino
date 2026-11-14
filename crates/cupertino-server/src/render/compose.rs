//! Composition of one output's scene into a [`SceneFrame`]. Everything that has a typed source is a scene item;
//! what the renderer still builds itself is kept in a frame-local list and referenced by position.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cupertino_effects::Mesh;
use cupertino_geometry::{CornerStyle, Point, Rect};
use cupertino_scene::{
    AlphaMode, BufferCrop, Clip, ContentDamage, FrameId, GlassId, GlassPass, GlobalRect, ImageId,
    ImageMapping, ImageRole, LayerStack, MaterialPass, MaterialStage, Output, OutputId,
    OutputInvalidation, OutputPoint, OutputRect, PhysicalSize, Placement, RecipeId, ResourceKey,
    SceneFrame, SceneId, SceneItem, SceneKind, SceneLayer, SnapshotId, Source, SurfaceId,
    SurfaceRole,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Physical, Rectangle};

type DevicePoint = smithay::utils::Point<i32, Physical>;

use super::material::glass::GlassContainer;
use super::material::GpuRecipe;
use super::scene::SceneParams;
use super::snapshots::{SnapshotLease, SnapshotOwner};

/// Straight opaque black; the render target clears with its own colour.
const UNSPECIFIED_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

static FRAMES: AtomicU64 = AtomicU64::new(0);

/// A surface tree to draw. Its place is given in device pixels, as the renderer rounds it, and becomes an item
/// of the frame once its surface has a handle in the frame's registry.
pub struct Tree {
    pub surface: WlSurface,
    pub kind: SceneKind,
    pub role: SurfaceRole,
    /// Where the tree's origin lies, in device pixels of the output.
    pub origin: DevicePoint,
    /// The point a window rescale is about, in device pixels.
    pub pivot: DevicePoint,
    pub scale: (f64, f64),
    pub opacity: f32,
    pub clip: Clip,
    /// The surfaces of the tree this piece draws, when it is a slice of it ([`TreeSlice`]); `None` draws all of it.
    pub only: Option<TreeSlice>,
}

/// The surfaces of a tree one piece draws, by the ids of their elements: a tree whose subsurfaces draw backdrops of
/// their own is drawn in slices with those backdrops between them.
#[derive(Clone, Debug)]
pub struct TreeSlice {
    pub ids: Rc<[smithay::backend::renderer::element::Id]>,
    /// The protocol id of the slice's first surface, which names its item apart from the other slices of the tree.
    pub index: u64,
}

/// What an image's pixels are: CPU pixels the renderer uploads, or chrome its GPU draws from a list of draws.
#[derive(Clone, Debug)]
pub enum ImageBuffer {
    Memory(MemoryRenderBuffer),
    Chrome(Rc<super::chrome::ChromeImage>),
}

impl From<MemoryRenderBuffer> for ImageBuffer {
    fn from(buffer: MemoryRenderBuffer) -> ImageBuffer {
        ImageBuffer::Memory(buffer)
    }
}

/// Pixels to draw, shown through `src` (buffer pixels) at `size` logical points.
pub struct Picture {
    pub buffer: ImageBuffer,
    pub kind: SceneKind,
    pub index: u64,
    pub role: ImageRole,
    pub buffer_px: (i32, i32),
    pub src: Rectangle<f64, Logical>,
    pub size: (i32, i32),
    pub placement: Placement,
    pub opacity: f32,
    pub clip: Clip,
}

/// A material pass over what lies below it. Its recipe gets a handle in the frame's registry once it is pushed.
pub struct Material {
    pub recipe: Rc<GpuRecipe>,
    pub kind: SceneKind,
    pub index: u64,
    pub rect: Rectangle<f64, Logical>,
    pub radius: f64,
    pub style: CornerStyle,
    pub stage: MaterialStage,
    pub tint: [f32; 3],
    pub opacity: f32,
    /// A rounded clip cuts the region to a window's outline as well; the material pass draws it as the region's mask.
    pub clip: Clip,
}

/// A glass container over what lies below it, drawn over the area it samples. Its container gets a handle in the
/// frame's registry once it is pushed.
pub struct Glass {
    pub glass: Rc<GlassContainer>,
    pub kind: SceneKind,
    pub index: u64,
    pub opacity: f32,
}

/// What keeps a stored snapshot alive for a frame: a lease on one that effect state owns, or the ownership of one the
/// frame captured for itself.
pub enum SnapshotKeep {
    Pinned(SnapshotLease),
    Owned(SnapshotOwner),
}

/// A window snapshot drawn through a mesh in output-local points. The pixels stay in the snapshot store; the piece
/// names them by handle and holds what keeps them alive for the frame.
pub struct MeshPiece {
    pub kind: SceneKind,
    pub index: u64,
    pub snapshot: SnapshotId,
    pub keep: SnapshotKeep,
    pub mesh: Mesh,
    pub opacity: f32,
    /// What besides the mesh and the opacity changes the look: the animation state that made the mesh.
    pub revision: u64,
}

/// The snapshot a frame's mesh item draws: the handle in the snapshot store, and what keeps it there.
pub struct FrameSnapshot {
    pub id: SnapshotId,
    pub keep: SnapshotKeep,
}

/// One drawable of a layer: a typed scene item, a surface tree, a picture, or an element the renderer still
/// builds itself.
pub enum Piece {
    Item(Box<SceneItem>),
    Tree(Box<Tree>),
    Image(Box<Picture>),
    Material(Box<Material>),
    Glass(Box<Glass>),
    Mesh(Box<MeshPiece>),
}

impl Piece {
    pub fn item(item: SceneItem) -> Self {
        Self::Item(Box::new(item))
    }
}

/// The items of an output by layer, with the resources they refer to.
pub struct Composition {
    id: FrameId,
    scale: f64,
    items: LayerStack<SceneItem>,
    resources: FrameResources,
}

/// What the typed handles of a frame's items resolve to: surface trees, images, recipes and snapshots, each by the
/// slot and generation the composition gave it. Owned by the renderer's adapter from the frame's composition until
/// the frame's elements are gone.
pub struct FrameResources {
    /// The frame these resources belong to: handles name slots of this frame's registries only.
    pub frame: FrameId,
    pub surfaces: Vec<WlSurface>,
    /// For each of `surfaces`, the slice of its tree the item draws; `None` for all of it.
    pub slices: Vec<Option<TreeSlice>>,
    pub images: Vec<ImageBuffer>,
    pub recipes: Vec<Rc<GpuRecipe>>,
    pub glass: Vec<Rc<GlassContainer>>,
    pub snapshots: Vec<Option<FrameSnapshot>>,
}

/// A composed frame and the resources its items refer to.
pub struct Composed {
    pub frame: SceneFrame,
    pub resources: FrameResources,
}

impl Composition {
    /// A new frame for an output drawn at `scale`.
    pub fn new(scale: f64) -> Self {
        let id = FrameId(FRAMES.fetch_add(1, Ordering::Relaxed));
        Self {
            id,
            scale,
            items: LayerStack::new(),
            resources: FrameResources {
                frame: id,
                surfaces: Vec::new(),
                slices: Vec::new(),
                images: Vec::new(),
                recipes: Vec::new(),
                glass: Vec::new(),
                snapshots: Vec::new(),
            },
        }
    }

    pub fn push(&mut self, layer: SceneLayer, piece: impl Into<Piece>) {
        match piece.into() {
            Piece::Item(item) => self.items.push(layer, *item),
            Piece::Mesh(mesh) => {
                let snapshot = SnapshotId(ResourceKey {
                    slot: self.resources.snapshots.len() as u64,
                    generation: self.id.0,
                });
                self.resources.snapshots.push(Some(FrameSnapshot {
                    id: mesh.snapshot,
                    keep: mesh.keep,
                }));
                self.items.push(
                    layer,
                    SceneItem {
                        id: SceneId {
                            kind: mesh.kind,
                            index: mesh.index,
                        },
                        revision: mesh.revision,
                        source: Source::TexturedMesh {
                            snapshot,
                            mesh: mesh.mesh,
                        },
                        placement: Placement::IDENTITY,
                        opacity: f64::from(mesh.opacity),
                        clip: Clip::None,
                        damage: ContentDamage::Full,
                    },
                );
            }
            Piece::Material(material) => {
                let recipe = RecipeId(ResourceKey {
                    slot: self.resources.recipes.len() as u64,
                    generation: self.id.0,
                });
                self.resources.recipes.push(material.recipe);
                self.items.push(
                    layer,
                    SceneItem {
                        id: SceneId {
                            kind: material.kind,
                            index: material.index,
                        },
                        revision: 0,
                        source: Source::Material(MaterialPass {
                            recipe,
                            rect: output_rect(material.rect),
                            radius: material.radius,
                            style: material.style,
                            stage: material.stage,
                            tint: material.tint,
                        }),
                        placement: Placement::IDENTITY,
                        opacity: f64::from(material.opacity),
                        clip: material.clip,
                        damage: ContentDamage::Full,
                    },
                );
            }
            Piece::Glass(glass) => {
                let Some(area) = glass.glass.sample_area() else {
                    return;
                };
                let id = GlassId(ResourceKey {
                    slot: self.resources.glass.len() as u64,
                    generation: self.id.0,
                });
                self.resources.glass.push(glass.glass);
                self.items.push(
                    layer,
                    SceneItem {
                        id: SceneId {
                            kind: glass.kind,
                            index: glass.index,
                        },
                        revision: 0,
                        source: Source::Glass(GlassPass {
                            glass: id,
                            rect: output_rect(area),
                        }),
                        placement: Placement::IDENTITY,
                        opacity: f64::from(glass.opacity),
                        clip: Clip::None,
                        damage: ContentDamage::Full,
                    },
                );
            }
            Piece::Image(picture) => {
                let id = ImageId(ResourceKey {
                    slot: self.resources.images.len() as u64,
                    generation: self.id.0,
                });
                self.resources.images.push(picture.buffer);
                self.items.push(
                    layer,
                    SceneItem {
                        id: SceneId {
                            kind: picture.kind,
                            index: picture.index,
                        },
                        revision: 0,
                        source: Source::Image {
                            id,
                            mapping: ImageMapping {
                                buffer_size: picture.buffer_px,
                                buffer_scale: 1.0,
                                transform: cupertino_scene::Transform::Normal,
                                crop: Some(BufferCrop(Rect::new(
                                    picture.src.loc.x,
                                    picture.src.loc.y,
                                    picture.src.size.w,
                                    picture.src.size.h,
                                ))),
                                alpha: AlphaMode::Premultiplied,
                            },
                            role: picture.role,
                            size: picture.size,
                        },
                        placement: picture.placement,
                        opacity: f64::from(picture.opacity),
                        clip: picture.clip,
                        damage: ContentDamage::Full,
                    },
                );
            }
            Piece::Tree(tree) => {
                // a slice is named by its first surface, so the slices of one tree stay apart
                let index = tree
                    .only
                    .as_ref()
                    .map_or(tree.surface.id().protocol_id() as u64, |s| s.index);
                let id = SurfaceId(ResourceKey {
                    slot: self.resources.surfaces.len() as u64,
                    generation: self.id.0,
                });
                self.resources.surfaces.push(tree.surface);
                self.resources.slices.push(tree.only);
                let device = |p: DevicePoint| {
                    OutputPoint(Point::new(
                        f64::from(p.x) / self.scale,
                        f64::from(p.y) / self.scale,
                    ))
                };
                self.items.push(
                    layer,
                    SceneItem {
                        id: SceneId {
                            kind: tree.kind,
                            index,
                        },
                        revision: 0,
                        source: Source::SurfaceTree {
                            id,
                            role: tree.role,
                        },
                        placement: Placement {
                            origin: device(tree.origin),
                            pivot: device(tree.pivot),
                            scale_x: tree.scale.0,
                            scale_y: tree.scale.1,
                        },
                        opacity: f64::from(tree.opacity),
                        clip: tree.clip,
                        damage: ContentDamage::Full,
                    },
                );
            }
        }
    }

    /// Add `pieces` to `layer`, in the order given (front to back within the layer).
    pub fn extend<P: Into<Piece>>(
        &mut self,
        layer: SceneLayer,
        pieces: impl IntoIterator<Item = P>,
    ) {
        for piece in pieces {
            self.push(layer, piece);
        }
    }

    /// Flatten into the frame of the output described by `p`.
    pub fn finish(self, p: &SceneParams<'_>) -> Composed {
        let output = Output {
            id: OutputId(p.output_id.0),
            rect: GlobalRect(from_rect(p.rect)),
            physical_size: PhysicalSize {
                width: p.output_px.0,
                height: p.output_px.1,
            },
            scale: p.scale,
            transform: transform(p.output.current_transform()),
        };
        self.finish_with(output, Duration::from_secs_f64(p.now.max(0.0)))
    }

    pub fn finish_with(self, output: Output, timestamp: Duration) -> Composed {
        let frame = SceneFrame::new(
            output,
            self.id,
            timestamp,
            UNSPECIFIED_CLEAR,
            OutputInvalidation::Incremental,
            self.items,
        );
        Composed {
            frame,
            resources: self.resources,
        }
    }
}

/// An item with the placement and clip of the source's own coordinates, drawn at `opacity`.
pub fn item(kind: SceneKind, index: u64, source: Source, opacity: f64) -> SceneItem {
    SceneItem {
        id: SceneId { kind, index },
        revision: 0,
        source,
        placement: Placement::IDENTITY,
        opacity,
        clip: cupertino_scene::Clip::None,
        damage: ContentDamage::Full,
    }
}

/// The image of a list of draws, drawn on the GPU ([`super::chrome`]), placed as [`image_piece`] places a buffer.
pub fn list_piece(
    image: &Rc<super::chrome::ChromeImage>,
    kind: SceneKind,
    px: (i32, i32),
    x: f64,
    y: f64,
    scale: f64,
    alpha: f32,
) -> Piece {
    picture_piece(
        ImageBuffer::Chrome(image.clone()),
        kind,
        px,
        x,
        y,
        scale,
        alpha,
    )
}

/// `buffer` of `px` device pixels with its top-left corner at output-local logical `(x, y)`; its logical size
/// is the pixel size divided by `scale`.
pub fn image_piece(
    buffer: &MemoryRenderBuffer,
    kind: SceneKind,
    px: (i32, i32),
    x: f64,
    y: f64,
    scale: f64,
    alpha: f32,
) -> Piece {
    picture_piece(buffer.clone().into(), kind, px, x, y, scale, alpha)
}

fn picture_piece(
    buffer: ImageBuffer,
    kind: SceneKind,
    px: (i32, i32),
    x: f64,
    y: f64,
    scale: f64,
    alpha: f32,
) -> Piece {
    Piece::Image(Box::new(Picture {
        buffer,
        kind,
        index: 0,
        role: ImageRole::Standalone,
        buffer_px: px,
        src: Rectangle::new((0.0, 0.0).into(), (f64::from(px.0), f64::from(px.1)).into()),
        size: (
            (f64::from(px.0) / scale).round() as i32,
            (f64::from(px.1) / scale).round() as i32,
        ),
        placement: Placement {
            origin: OutputPoint(Point::new(x, y)),
            ..Placement::IDENTITY
        },
        opacity: alpha,
        clip: Clip::None,
    }))
}

pub fn from_rect(r: cupertino_wm::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

pub fn output_rect(r: Rectangle<f64, Logical>) -> OutputRect {
    OutputRect(Rect::new(r.loc.x, r.loc.y, r.size.w, r.size.h))
}

pub fn logical_rect(r: OutputRect) -> Rectangle<f64, Logical> {
    Rectangle::new(
        (r.0.x(), r.0.y()).into(),
        (r.0.width(), r.0.height()).into(),
    )
}

pub fn corner_style(continuous: bool) -> CornerStyle {
    if continuous {
        CornerStyle::continuous()
    } else {
        CornerStyle::Circular
    }
}

fn transform(t: smithay::utils::Transform) -> cupertino_scene::Transform {
    use cupertino_scene::Transform as T;
    use smithay::utils::Transform as S;
    match t {
        S::Normal => T::Normal,
        S::_90 => T::Rotate90,
        S::_180 => T::Rotate180,
        S::_270 => T::Rotate270,
        S::Flipped => T::Flipped,
        S::Flipped90 => T::Flipped90,
        S::Flipped180 => T::Flipped180,
        S::Flipped270 => T::Flipped270,
    }
}
