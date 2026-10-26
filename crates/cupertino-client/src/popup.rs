//! Geometry of `xdg_popup` surfaces.

use crate::client::outward_rect;
use crate::ClientError;
use cupertino_geometry::Rect;

/// Position and size of a popup in whole points, relative to the top-left corner of its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PopupGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl PopupGeometry {
    /// The smallest whole-point rectangle that contains `rect`. Errors on an empty or non-finite
    /// rectangle, which no popup can have.
    pub(crate) fn from_rect(rect: &Rect) -> Result<Self, ClientError> {
        let finite = [rect.x(), rect.y(), rect.width(), rect.height()]
            .iter()
            .all(|v| v.is_finite());
        let (x, y, width, height) = outward_rect(rect);
        if !finite || width <= 0 || height <= 0 {
            return Err(ClientError::InvalidShape);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_covers_the_rect_in_whole_points() {
        let g = PopupGeometry::from_rect(&Rect::new(10.4, 24.0, 153.2, 88.5)).unwrap();
        assert_eq!(
            g,
            PopupGeometry {
                x: 10,
                y: 24,
                width: 154,
                height: 89
            }
        );
    }

    #[test]
    fn empty_or_non_finite_rects_are_refused() {
        assert!(matches!(
            PopupGeometry::from_rect(&Rect::new(0.0, 0.0, 0.0, 10.0)),
            Err(ClientError::InvalidShape)
        ));
        assert!(matches!(
            PopupGeometry::from_rect(&Rect::new(f64::NAN, 0.0, 10.0, 10.0)),
            Err(ClientError::InvalidShape)
        ));
        assert!(matches!(
            PopupGeometry::from_rect(&Rect::new(0.0, 0.0, f64::INFINITY, 10.0)),
            Err(ClientError::InvalidShape)
        ));
    }
}
