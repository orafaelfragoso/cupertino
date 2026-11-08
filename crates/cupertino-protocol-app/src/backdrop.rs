//! Typed wrapper of `cupertino_backdrop_v1`: a material the server draws below a surface.

use crate::Error;
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::{
    CupertinoBackdropV1, GlassStyle as WireStyle,
};
use cupertino_geometry::Rect;
use wayland_client::Proxy;

/// The look of a glass backdrop ([`Backdrop::set_glass`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// The frosted glass of bars, menus, panels and controls.
    #[default]
    Regular,
    /// The clearer glass of controls over rich content, such as media.
    Clear,
}

impl GlassStyle {
    /// The material a server without glass draws instead.
    pub fn fallback_material(self) -> &'static str {
        "Menu"
    }
}

/// A server-rendered material region below the content of a surface. Every setter takes
/// effect with the next commit of the surface.
/// Dropping it removes the backdrop with that commit.
pub struct Backdrop {
    proxy: CupertinoBackdropV1,
}

/// Integer rectangle of a backdrop and its corner radius, as the wire carries them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: f64,
}

impl Shape {
    /// Round `rect` to whole logical points. Errors on a negative extent or radius or a
    /// non-finite value, which the server would reject as a protocol error.
    fn new(rect: &Rect, radius: f64) -> Result<Self, Error> {
        let values = [rect.x(), rect.y(), rect.width(), rect.height(), radius];
        if !values.iter().all(|v| v.is_finite())
            || rect.width().round() < 0.0
            || rect.height().round() < 0.0
            || radius < 0.0
        {
            return Err(Error::InvalidShape);
        }
        Ok(Self {
            x: rect.x().round() as i32,
            y: rect.y().round() as i32,
            width: rect.width().round() as i32,
            height: rect.height().round() as i32,
            radius,
        })
    }
}

impl Backdrop {
    pub(crate) fn new(proxy: CupertinoBackdropV1) -> Self {
        Self { proxy }
    }

    /// Name of a material recipe of the design tokens, for example `"Menu"`.
    pub fn set_material(&self, name: &str) {
        self.proxy.set_material(name.to_string());
    }

    /// Region in surface-local logical points with continuous corners of `radius` points; a
    /// zero width or height hides the backdrop.
    pub fn set_shape(&self, rect: &Rect, radius: f64) -> Result<(), Error> {
        let s = Shape::new(rect, radius)?;
        self.proxy.set_shape(s.x, s.y, s.width, s.height, s.radius);
        Ok(())
    }

    /// Draw the backdrop as glass of `style` instead of a material recipe. Backdrops of one surface with the same
    /// nonzero `container` join into one shape whose outlines blend over `spacing` points (0 joins them with a hard
    /// edge); container 0 keeps the backdrop on its own. A server before version 6 draws the backdrop with the
    /// material of [`GlassStyle::fallback_material`] instead.
    pub fn set_glass(&self, style: GlassStyle, container: u32, spacing: f64) -> Result<(), Error> {
        if !(spacing.is_finite() && spacing >= 0.0) {
            return Err(Error::InvalidGlass);
        }
        if self.proxy.version() < 6 {
            self.set_material(style.fallback_material());
            return Ok(());
        }
        let style = match style {
            GlassStyle::Regular => WireStyle::Regular,
            GlassStyle::Clear => WireStyle::Clear,
        };
        self.proxy.set_glass(style, container, spacing);
        Ok(())
    }

    /// Opacity of the whole backdrop, 0 to 1.
    pub fn set_alpha(&self, alpha: f64) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&alpha) {
            return Err(Error::InvalidAlpha);
        }
        self.proxy.set_alpha(alpha);
        Ok(())
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_rounds_to_whole_points() {
        let s = Shape::new(&Rect::new(10.4, 20.5, 343.6, 71.49), 12.0).unwrap();
        assert_eq!(
            (s.x, s.y, s.width, s.height, s.radius),
            (10, 21, 344, 71, 12.0)
        );
    }

    #[test]
    fn shape_rejects_what_the_server_rejects() {
        assert!(matches!(
            Shape::new(&Rect::new(0.0, 0.0, -1.0, 5.0), 1.0),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            Shape::new(&Rect::new(0.0, 0.0, 5.0, 5.0), -0.5),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            Shape::new(&Rect::new(f64::NAN, 0.0, 5.0, 5.0), 1.0),
            Err(Error::InvalidShape)
        ));
    }
}
