//! Composition of one output's scene into a [`SceneFrame`]. Everything that has a typed source is a scene item;
//! what the renderer still builds itself is kept in a frame-local list and referenced by position.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cupertino_geometry::{CornerStyle, Rect};
use cupertino_scene::{
    ContentDamage, FrameId, GlobalRect, LayerStack, Output, OutputId, OutputInvalidation,
    OutputRect, PhysicalSize, Placement, SceneFrame, SceneId, SceneItem, SceneKind, SceneLayer,
    Source,
};
use smithay::utils::{Logical, Rectangle};

use super::scene::SceneParams;
use super::CupertinoElement;

/// Straight opaque black; the render target clears with its own colour.
const UNSPECIFIED_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

static FRAMES: AtomicU64 = AtomicU64::new(0);

/// One drawable of a layer: a typed scene item, or an element the renderer still builds itself.
pub enum Piece {
    Item(SceneItem),
    Local(CupertinoElement),
}

impl<T: Into<CupertinoElement>> From<T> for Piece {
    fn from(element: T) -> Self {
        Self::Local(element.into())
    }
}

/// The items of an output by layer, with the frame-local elements they refer to.
#[derive(Default)]
pub struct Composition {
    items: LayerStack<SceneItem>,
    locals: Vec<Option<CupertinoElement>>,
}

/// A composed frame and the frame-local elements its [`Source::LocalPass`] items refer to.
pub struct Composed {
    pub frame: SceneFrame,
    pub locals: Vec<Option<CupertinoElement>>,
}

impl Composition {
    pub fn push(&mut self, layer: SceneLayer, piece: impl Into<Piece>) {
        match piece.into() {
            Piece::Item(item) => self.items.push(layer, item),
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
            FrameId(FRAMES.fetch_add(1, Ordering::Relaxed)),
            Duration::from_secs_f64(p.now.max(0.0)),
            UNSPECIFIED_CLEAR,
            OutputInvalidation::Incremental,
            self.items,
        );
        Composed {
            frame,
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
