//! The glass surfaces of a window's chrome, each drawn through one named role.
//!
//! Every glass surface a control draws names its [`GlassRole`]; [`GlassRole::material`] says what draws that role. A
//! role drawn by the compositor asks for the theme's material under the node ([`PaintCx::material`]) and paints the
//! surface's opaque fill where the window cannot show it (no desktop effects, Reduce Transparency, Increase
//! Contrast). A painted role draws a translucent body with a fine rim over what the window shows below it. Moving a
//! role to another material is the one line of its arm in [`GlassRole::material`].

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_core::PaintCx;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, Surface};

/// A glass surface of the window's chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlassRole {
    /// A floating group of toolbar controls: a capsule of buttons, a menu button, the circular search button.
    ToolbarGroup,
    /// The sidebar panel of a full-height-sidebar window, from its top edge to its bottom edge.
    SidebarPanel,
}

/// What draws a [`GlassRole`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlassMaterial {
    /// The compositor's material of a theme surface, under the node.
    Compositor(Surface),
    /// A translucent body and rim the toolkit paints over the window's own content.
    Painted,
}

impl GlassRole {
    /// What draws each role. A floating toolbar group lies over the window's own content, which a material under the
    /// window cannot show through, so it is painted; the sidebar panel lies at the window's edge over what is behind
    /// the window, which the compositor's sidebar material shows.
    pub const fn material(self) -> GlassMaterial {
        match self {
            GlassRole::ToolbarGroup => GlassMaterial::Painted,
            GlassRole::SidebarPanel => GlassMaterial::Compositor(Surface::Sidebar),
        }
    }
}

/// The corner radius of a shape inset `inset` points inside a shape with corner radius `outer`, so both corners share
/// a centre: `outer − inset`, never below zero.
pub fn nested_radius(outer: f64, inset: f64) -> f64 {
    (outer - inset).max(0.0)
}

/// The painted body over the window's content: a share of white in the dark appearance, of white over a light one.
const BODY_DARK: f32 = 0.133;
const BODY_LIGHT: f32 = 0.72;
/// The rim one device pixel wide just inside the body's edge: a light line in the dark appearance, a dark one in
/// the light appearance.
const RIM_DARK: f32 = 0.22;
const RIM_LIGHT: f32 = 0.09;
/// The shade below a painted body in the light appearance, where white over white needs it to stand out.
const SHADE_LIGHT: f32 = 0.10;
const SHADE_BLUR: f64 = 3.0;

/// Paints the glass of `role` over the whole node, `rect` being the node's bounds and `radius` its corner radius.
/// Returns true when the compositor draws it.
pub fn paint_glass(cx: &mut PaintCx<'_>, role: GlassRole, rect: Rect, radius: f64) -> bool {
    match role.material() {
        GlassMaterial::Compositor(surface) => {
            if cx.material(surface, radius, 1.0) {
                return true;
            }
            if let Some(fill) = cx.env().theme.surface(surface).fill {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect { rect, radius },
                    paint: Paint::Solid(fill),
                });
            }
            false
        }
        GlassMaterial::Painted => {
            paint_body(cx, rect, radius);
            false
        }
    }
}

/// The painted glass: in the light appearance a soft shade first, then the body and its rim.
fn paint_body(cx: &mut PaintCx<'_>, rect: Rect, radius: f64) {
    let theme = &cx.env().theme;
    let dark = theme.preferences.appearance.is_dark();
    let opaque =
        theme.preferences.reduce_transparency || theme.preferences.appearance.is_high_contrast();
    let scale = cx.env().scale.max(1.0);
    let shape = Shape::RoundedRect { rect, radius };
    if !dark {
        cx.push(PaintOp::PushLayer {
            opacity: 1.0,
            effects: vec![cupertino_ui_render::Effect::DropShadow {
                offset: cupertino_geometry::Point::new(0.0, 0.5),
                blur: SHADE_BLUR,
                color: Color::new(0.0, 0.0, 0.0, SHADE_LIGHT),
            }],
        });
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, 1.0)),
        });
        cx.push(PaintOp::PopLayer);
    }
    let body = match (dark, opaque) {
        (true, false) => Color::new(1.0, 1.0, 1.0, BODY_DARK),
        (true, true) => Color::new(0.235, 0.243, 0.271, 1.0),
        (false, false) => Color::new(1.0, 1.0, 1.0, BODY_LIGHT),
        (false, true) => Color::new(1.0, 1.0, 1.0, 1.0),
    };
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(body),
    });
    let px = 1.0 / scale;
    let rim = if dark {
        Color::new(1.0, 1.0, 1.0, RIM_DARK)
    } else {
        Color::new(0.0, 0.0, 0.0, RIM_LIGHT)
    };
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rect.inset(Insets::uniform(px / 2.0)),
            radius: nested_radius(radius, px / 2.0),
        },
        width: px,
        paint: Paint::Solid(rim),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_shape_shares_its_corner_centre_with_its_container() {
        assert_eq!(nested_radius(18.0, 4.0), 14.0);
        assert_eq!(nested_radius(16.0, 0.0), 16.0);
        assert_eq!(nested_radius(3.0, 4.0), 0.0, "never below zero");
    }

    #[test]
    fn each_role_has_its_material() {
        assert_eq!(GlassRole::ToolbarGroup.material(), GlassMaterial::Painted);
        assert_eq!(
            GlassRole::SidebarPanel.material(),
            GlassMaterial::Compositor(Surface::Sidebar)
        );
    }
}
