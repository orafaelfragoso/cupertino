//! The slices of a held surface's picture (D19, [`super::capture_held_surface`]): each drawn over the bounds of its
//! pieces alone, so a slice holds no more pixels than it draws.

use super::*;
use crate::render::content_blur::{reach_px, sigma_px};

/// Device pixels a held slice reaches past the bounds of its pieces on every side: the renderer's rounding of where it
/// places them and the antialiasing of their edges.
const HELD_SLICE_MARGIN_PX: i32 = 1;

/// The pieces of one slice of a held surface's picture, drawn at `scale` into a picture of `size_px` device pixels,
/// moved to draw into the bounds of what they draw there, with those bounds (device pixels of the picture); the whole
/// picture, the pieces as they are, when the bounds of one of them are not known. `None` when they draw nothing.
pub(super) fn fitted(
    p: &SceneParams<'_>,
    pieces: Vec<Piece>,
    size_px: (i32, i32),
) -> Option<(Vec<Piece>, Rectangle<i32, Physical>)> {
    let whole = Rectangle::<i32, Physical>::from_size(size_px.into());
    let mut bounds: Option<Rectangle<i32, Physical>> = None;
    for piece in &pieces {
        match piece_bounds(piece, p.scale) {
            Drawn::Unknown => return Some((pieces, whole)),
            Drawn::Nothing => {}
            Drawn::Within(b) => bounds = Some(bounds.map_or(b, |a| a.merge(b))),
        }
    }
    let bounds = bounds?.intersection(whole)?;
    if bounds.is_empty() {
        return None;
    }
    if bounds == whole {
        return Some((pieces, whole));
    }
    let by = bounds.loc;
    let moved = pieces
        .into_iter()
        .map(|piece| moved(piece, by, p.scale))
        .collect();
    Some((moved, bounds))
}

/// Where a piece of a held slice draws.
enum Drawn {
    /// Not known: the piece may draw anywhere.
    Unknown,
    /// Nowhere: it lies outside its clip.
    Nothing,
    /// Inside these bounds (device pixels).
    Within(Rectangle<i32, Physical>),
}

/// Where `piece` draws at `scale`: a surface tree within its surfaces' bounds, placed, rescaled about its pivot, grown
/// by the reach of its blur and cut to its mask as the renderer draws it; a rounded rectangle within its rectangle and
/// border. Of any other piece the bounds are not known.
fn piece_bounds(piece: &Piece, scale: f64) -> Drawn {
    let grown = |r: Rectangle<f64, Physical>, reach: i32| {
        let reach = reach + HELD_SLICE_MARGIN_PX;
        let (x0, y0) = (
            r.loc.x.floor() as i32 - reach,
            r.loc.y.floor() as i32 - reach,
        );
        let (x1, y1) = (
            (r.loc.x + r.size.w).ceil() as i32 + reach,
            (r.loc.y + r.size.h).ceil() as i32 + reach,
        );
        Rectangle::<i32, Physical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
    };
    match piece {
        Piece::Tree(t) => {
            let b = smithay::desktop::utils::bbox_from_surface_tree(&t.surface, (0, 0))
                .to_f64()
                .to_physical(scale);
            let origin = t.origin.to_f64();
            let (mut x0, mut y0) = (origin.x + b.loc.x, origin.y + b.loc.y);
            let (mut x1, mut y1) = (x0 + b.size.w, y0 + b.size.h);
            let (rescaled, masked) = match t.role {
                SurfaceRole::Standalone | SurfaceRole::Cursor => (false, false),
                SurfaceRole::Popup => (true, false),
                SurfaceRole::WindowContent | SurfaceRole::Vibrant(_) | SurfaceRole::Blurred(_) => {
                    (true, true)
                }
            };
            if rescaled {
                let pivot = t.pivot.to_f64();
                let at = |v: f64, pivot: f64, k: f64| pivot + (v - pivot) * k;
                (x0, x1) = (at(x0, pivot.x, t.scale.0), at(x1, pivot.x, t.scale.0));
                (y0, y1) = (at(y0, pivot.y, t.scale.1), at(y1, pivot.y, t.scale.1));
            }
            let reach = match t.role {
                SurfaceRole::Blurred(blur) => sigma_px(blur, scale).map_or(0, |sigma| {
                    (f64::from(reach_px(sigma)) * t.scale.0.max(t.scale.1).max(1.0)).ceil() as i32
                }),
                _ => 0,
            };
            let drawn = grown(
                Rectangle::new(
                    (x0.min(x1), y0.min(y1)).into(),
                    ((x1 - x0).abs(), (y1 - y0).abs()).into(),
                ),
                reach,
            );
            match t.clip {
                Clip::Rounded { rect, .. } if masked => {
                    let r = rect.0;
                    let mask = grown(
                        Rectangle::new(
                            (r.x() * scale, r.y() * scale).into(),
                            (r.width() * scale, r.height() * scale).into(),
                        ),
                        0,
                    );
                    drawn
                        .intersection(mask)
                        .map_or(Drawn::Nothing, Drawn::Within)
                }
                _ => Drawn::Within(drawn),
            }
        }
        Piece::Item(item) => match &item.source {
            cupertino_scene::Source::RoundedRect(rr) => {
                let r = rr.rect.0;
                let border = rr.border.map_or(0, |(w, _)| (w * scale).ceil() as i32);
                Drawn::Within(grown(
                    Rectangle::new(
                        (r.x() * scale, r.y() * scale).into(),
                        (r.width() * scale, r.height() * scale).into(),
                    ),
                    border,
                ))
            }
            _ => Drawn::Unknown,
        },
        _ => Drawn::Unknown,
    }
}

/// `piece` moved by `-by` (device pixels at `scale`), to draw into a picture whose corner lies at `by`; only the
/// pieces whose bounds [`piece_bounds`] knows.
fn moved(piece: Piece, by: Point<i32, Physical>, scale: f64) -> Piece {
    let d = (f64::from(by.x) / scale, f64::from(by.y) / scale);
    let shifted = |r: cupertino_scene::OutputRect| {
        let r = r.0;
        cupertino_scene::OutputRect(cupertino_geometry::Rect {
            origin: cupertino_geometry::Point::new(r.x() - d.0, r.y() - d.1),
            size: r.size,
        })
    };
    let clip = |clip: Clip| match clip {
        Clip::None => Clip::None,
        Clip::Rect(r) => Clip::Rect(shifted(r)),
        Clip::Rounded {
            rect,
            radius,
            style,
            exact,
        } => Clip::Rounded {
            rect: shifted(rect),
            radius,
            style,
            exact,
        },
    };
    match piece {
        Piece::Tree(mut t) => {
            t.origin -= by;
            t.pivot -= by;
            t.clip = clip(t.clip);
            Piece::Tree(t)
        }
        Piece::Item(mut item) => {
            if let cupertino_scene::Source::RoundedRect(ref mut rr) = item.source {
                rr.rect = shifted(rr.rect);
            }
            item.clip = clip(item.clip);
            Piece::Item(item)
        }
        piece => piece,
    }
}
