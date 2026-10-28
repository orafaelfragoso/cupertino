//! Coordinate spaces stay distinct until the renderer performs physical rounding.

use cupertino_geometry::{Point, Rect};

/// Global logical geometry cannot be passed as output-local geometry without a translation.
///
/// ```compile_fail,E0308
/// use cupertino_geometry::Rect;
/// use cupertino_scene::{GlobalRect, OutputRect};
/// let _: OutputRect = GlobalRect(Rect::ZERO);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalRect(pub Rect);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputRect(pub Rect);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputPoint(pub Point);

/// Source buffer pixels, before buffer scale, crop and transform are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Source crop in buffer pixels; viewport crops may have fractional edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BufferCrop(pub Rect);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalSize {
    pub width: i32,
    pub height: i32,
}

/// Output or source-buffer orientation. Flipped variants reflect horizontally before rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl GlobalRect {
    /// Translate global logical geometry into this output's logical space without rounding.
    pub fn on_output(self, output: GlobalRect) -> OutputRect {
        OutputRect(self.0.translate(-output.0.x(), -output.0.y()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_translation_preserves_fractional_geometry_and_negative_origins() {
        let output = GlobalRect(Rect::new(-1280.25, 100.5, 1280.0, 720.0));
        let window = GlobalRect(Rect::new(-1260.125, 110.75, 200.25, 100.5));
        assert_eq!(
            window.on_output(output),
            OutputRect(Rect::new(20.125, 10.25, 200.25, 100.5))
        );
    }
}
