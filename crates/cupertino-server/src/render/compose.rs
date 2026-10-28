//! Composition of one output's scene into a [`SceneFrame`]. Everything that has a typed source is a scene item;
//! what the renderer still builds itself is kept in a frame-local list and referenced by position.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cupertino_geometry::{CornerStyle, Point, Rect};
use cupertino_scene::{
    Clip, ContentDamage, FrameId, GlobalRect, LayerStack, Output, OutputId, OutputInvalidation,
    OutputPoint, OutputRect, PhysicalSize, Placement, ResourceKey, SceneFrame, SceneId, SceneItem,
    SceneKind, SceneLayer, Source, SurfaceId, SurfaceRole,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Physical, Rectangle};

type DevicePoint = smithay::utils::Point<i32, Physical>;

use super::scene::SceneParams;
use super::CupertinoElement;

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
}

/// One drawable of a layer: a typed scene item, a surface tree, or an element the renderer still builds itself.
pub enum Piece {
    Item(SceneItem),
    Tree(Box<Tree>),
    Local(CupertinoElement),
}

impl<T: Into<CupertinoElement>> From<T> for Piece {
    fn from(element: T) -> Self {
        Self::Local(element.into())
    }
}

/// The items of an output by layer, with the resources they refer to.
pub struct Composition {
    id: FrameId,
    scale: f64,
    items: LayerStack<SceneItem>,
    surfaces: Vec<WlSurface>,
    locals: Vec<Option<CupertinoElement>>,
}

/// A composed frame and the resources its items refer to: surface trees by [`SurfaceId`], and the
/// frame-local elements of its [`Source::LocalPass`] items.
pub struct Composed {
    pub frame: SceneFrame,
    pub surfaces: Vec<WlSurface>,
    pub locals: Vec<Option<CupertinoElement>>,
}

impl Composition {
    /// A new frame for an output drawn at `scale`.
    pub fn new(scale: f64) -> Self {
        Self {
            id: FrameId(FRAMES.fetch_add(1, Ordering::Relaxed)),
            scale,
            items: LayerStack::new(),
            surfaces: Vec::new(),
            locals: Vec::new(),
        }
    }

    pub fn push(&mut self, layer: SceneLayer, piece: impl Into<Piece>) {
        match piece.into() {
            Piece::Item(item) => self.items.push(layer, item),
            Piece::Tree(tree) => {
                let index = tree.surface.id().protocol_id() as u64;
                let id = SurfaceId(ResourceKey {
                    slot: self.surfaces.len() as u64,
                    generation: self.id.0,
                });
                self.surfaces.push(tree.surface);
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
            Piece::Local(element) => {
                let index = self.locals.len();
                self.locals.push(Some(element));
                self.items.push(
                    layer,
                    item(
                        SceneKind::LocalPass,
                        index as u64,
                        Source::LocalPass { index },
                        1.0,
                    ),
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
        let frame = SceneFrame::new(
            Output {
                id: OutputId(p.output_id.0),
                rect: GlobalRect(from_rect(p.rect)),
                physical_size: PhysicalSize {
                    width: p.output_px.0,
                    height: p.output_px.1,
                },
                scale: p.scale,
                transform: transform(p.output.current_transform()),
            },
            self.id,
            Duration::from_secs_f64(p.now.max(0.0)),
            UNSPECIFIED_CLEAR,
            OutputInvalidation::Incremental,
            self.items,
        );
        Composed {
            frame,
            surfaces: self.surfaces,
            locals: self.locals,
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
