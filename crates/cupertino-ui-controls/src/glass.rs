//! The glass surfaces of a window's chrome, each drawn through one named role.
//!
//! Every glass surface a control draws names its [`GlassRole`]; [`GlassRole::material`] says what draws that role. A
//! role drawn by the compositor asks for the theme's material under the node ([`PaintCx::material`]) and paints the
//! surface's opaque fill where the window cannot show it (no desktop effects, Reduce Transparency, Increase
//! Contrast). A glass role asks for glass under the node ([`PaintCx::glass`]), which the compositor keeps with
//! Reduce Transparency and Increase Contrast, and joins the role's container. A painted role draws a translucent body
//! with a fine rim over what the window shows below it, as a glass role does where the desktop shows no glass. Moving
//! a role to another material is the one line of its arm in [`GlassRole::material`].
//!
//! Glass lies under everything the window paints, so a window with glass over its content pane has the compositor
//! draw that pane's background too ([`Surface::WindowBackground`]): the glass then reads the window, not the desktop
//! behind it.
//!
//! A glass button is glass wherever the compositor's glass can show, filling its node, without Reduce Transparency: on a
//! material the window shows (a sidebar, a translucent window background), and over the window's own content, where
//! the button draws in the window's raised layer ([`Widget::raised`](cupertino_ui_core::Widget::raised)) with its glass
//! between that layer and the content, so the glass shows the content under it. Where the window system has no such
//! layer, over content that would hide glass under it, and with Reduce Transparency it paints its body, as it does
//! where the desktop shows no glass.
//!
//! Roles are either content surfaces, which hold a window's content (the sidebar panel), or floating controls over
//! that content (a toolbar group, a translucent button, a held thumb's lens); see [`GlassRole::floating`]. Glass is
//! never laid on glass: a role whose node already lies on glass ([`PaintCx::on_glass`]) draws none of its own, the
//! glass under it serving both.

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_core::{Glass, PaintCx};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, Surface};

/// A glass surface of the window's chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlassRole {
    /// A floating group of toolbar controls: a capsule of buttons, a menu button, the circular search button.
    ToolbarGroup,
    /// The sidebar panel of a full-height-sidebar window, from its top edge to its bottom edge.
    SidebarPanel,
    /// An action button drawn on glass of its own, a shaded body with a fine bright rim, as against the quiet fill of
    /// an ordinary button.
    GlassButton,
    /// The lens a switch's thumb or a slider's knob becomes while held, over the track it slides along.
    ThumbLens,
}

/// What draws a [`GlassRole`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlassMaterial {
    /// The compositor's material of a theme surface, under the node.
    Compositor(Surface),
    /// Regular glass under the node ([`PaintCx::glass`]), joined with the nodes of the same role in one container
    /// ([`GlassRole::container`]).
    Glass,
    /// A translucent body and rim the toolkit paints over the window's own content.
    Painted,
}

impl GlassRole {
    /// What draws each role. A floating toolbar group is glass over the content pane's background, which the
    /// compositor draws below it; the sidebar panel lies at the window's edge over what is behind the window, which
    /// the compositor's sidebar material shows. A translucent button is glass of its own where that glass can show
    /// ([`paint_glass`]) and paints its body elsewhere. A thumb's lens lies over the track it slides along, which the
    /// window paints and which would hide the compositor's glass, so the toolkit paints it.
    pub const fn material(self) -> GlassMaterial {
        match self {
            GlassRole::ToolbarGroup | GlassRole::GlassButton => GlassMaterial::Glass,
            GlassRole::SidebarPanel => GlassMaterial::Compositor(Surface::Sidebar),
            GlassRole::ThumbLens => GlassMaterial::Painted,
        }
    }

    /// Whether the role is a floating control over the window's content rather than a surface holding content.
    pub const fn floating(self) -> bool {
        match self {
            GlassRole::SidebarPanel => false,
            GlassRole::ToolbarGroup | GlassRole::GlassButton | GlassRole::ThumbLens => true,
        }
    }

    /// The glass container the role's nodes join and its blend width in points: nodes of one window in the same
    /// container blend into one shape where they come closer than the width.
    pub const fn container(self) -> (u32, f64) {
        match self {
            GlassRole::ToolbarGroup => (TOOLBAR_CONTAINER, TOOLBAR_SPACING),
            GlassRole::SidebarPanel | GlassRole::GlassButton | GlassRole::ThumbLens => (0, 0.0),
        }
    }
}

/// The glass container of a window's floating toolbar controls.
const TOOLBAR_CONTAINER: u32 = 1;
/// How close two toolbar controls come before their glass blends into one shape: the room between neighbouring
/// controls, so at rest each keeps its own outline and they merge only as one grows towards the next.
const TOOLBAR_SPACING: f64 = crate::toolbar::TOOLBAR_GAP;

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
/// Returns true when the compositor draws it, or when the node already lies on glass, which then serves it: glass is
/// never doubled. A glass button asks for glass only where it can show ([`shows_button_glass`]).
pub fn paint_glass(cx: &mut PaintCx<'_>, role: GlassRole, rect: Rect, radius: f64) -> bool {
    if cx.on_glass() {
        return true;
    }
    if role == GlassRole::GlassButton && !shows_button_glass(cx, rect) {
        paint_button_body(cx, rect, radius);
        return false;
    }
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
        GlassMaterial::Glass => {
            let (container, spacing) = role.container();
            if cx.glass(
                Glass::regular()
                    .radius(radius)
                    .container(container, spacing),
            ) {
                // with Increase Contrast the toolkit lays a firmer rim over the compositor's glass
                paint_edge(cx, rect, radius);
                return true;
            }
            // without the desktop's glass the toolkit paints its own
            match role {
                GlassRole::GlassButton => paint_button_body(cx, rect, radius),
                _ => paint_body(cx, rect, radius),
            }
            false
        }
        GlassMaterial::Painted => {
            match role {
                GlassRole::GlassButton => paint_button_body(cx, rect, radius),
                _ => paint_body(cx, rect, radius),
            }
            false
        }
    }
}

/// Whether a glass button's own glass would show: it lies on a material the window shows, or it draws in the window's
/// raised layer, over the window's content, which then shows through the glass rather than covering it; Reduce
/// Transparency is off, which keeps the painted body; and its body fills the node, the shape the compositor draws the
/// glass in.
fn shows_button_glass(cx: &PaintCx<'_>, rect: Rect) -> bool {
    let size = cx.size();
    let fills = rect.min_x().abs() < 1e-6
        && rect.min_y().abs() < 1e-6
        && (rect.width() - size.width).abs() < 1e-6
        && (rect.height() - size.height).abs() < 1e-6;
    (cx.on_material() || cx.in_raised_layer())
        && !cx.env().theme.preferences.reduce_transparency
        && fills
}

/// The rim the toolkit lays over a floating control's glass with Increase Contrast in the light appearance, at its sides
/// and at its top and bottom: a dark line on the outermost device pixel of the shape, strongest across the middle of the
/// sides. At rest the compositor's glass draws its own outline, which follows the brightness of the face and of what
/// lies around it.
const EDGE_RIM_CONTRAST_SIDES: f32 = 0.5;
const EDGE_RIM_CONTRAST_ENDS: f32 = 0.3;

/// The firmer rim the toolkit lays over the compositor's glass of a floating control with Increase Contrast in the
/// light appearance ([`EDGE_RIM_CONTRAST_SIDES`]); the dark appearance's glass stands out by its face, and at rest the
/// glass's own outline serves.
fn paint_edge(cx: &mut PaintCx<'_>, rect: Rect, radius: f64) {
    let theme = &cx.env().theme;
    if theme.preferences.appearance.is_dark() || !theme.preferences.appearance.is_high_contrast() {
        return;
    }
    let (sides, ends) = (EDGE_RIM_CONTRAST_SIDES, EDGE_RIM_CONTRAST_ENDS);
    let scale = cx.env().scale.max(1.0);
    let px = 1.0 / scale;
    let black = |a| Color::new(0.0, 0.0, 0.0, a);
    let stop = |offset, color| cupertino_ui_render::GradientStop { offset, color };
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rect.inset(Insets::uniform(px / 2.0)),
            radius: nested_radius(radius, px / 2.0),
        },
        width: px,
        paint: Paint::Linear {
            start: cupertino_geometry::Point::new(rect.min_x(), rect.min_y()),
            end: cupertino_geometry::Point::new(rect.min_x(), rect.max_y()),
            stops: vec![
                stop(0.0, black(ends)),
                stop(0.5, black(sides)),
                stop(1.0, black(ends)),
            ],
        },
    });
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

/// A translucent button's body: a share of white over the dark appearance, nearly white over the light one; with
/// Reduce Transparency a darker opaque body in the dark appearance and a white one in the light.
const BUTTON_BODY_DARK: f32 = 0.13;
const BUTTON_BODY_LIGHT: f32 = 0.82;
const BUTTON_OPAQUE_DARK: f32 = 0.19;
/// The fine rim just inside the body's edge, bright at the top and the bottom and faint across the middle: white in
/// the dark appearance, black in the light one, stronger with Increase Contrast.
const BUTTON_RIM_DARK: f32 = 0.38;
const BUTTON_RIM_LIGHT: f32 = 0.10;
const BUTTON_RIM_HIGH_CONTRAST: f32 = 0.70;
/// What the rim keeps across the middle of the sides.
const BUTTON_RIM_MIDDLE: f32 = 0.3;
/// The soft shade under the body in the light appearance.
const BUTTON_SHADE_LIGHT: f32 = 0.12;

/// The colours of a translucent button's body and rim in `theme`.
pub(crate) fn button_body_colors(theme: &cupertino_ui_theme::ThemeTokens) -> (Color, Color) {
    let dark = theme.preferences.appearance.is_dark();
    let contrast = theme.preferences.appearance.is_high_contrast();
    let opaque = theme.preferences.reduce_transparency;
    let body = match (dark, opaque) {
        (true, false) => Color::new(1.0, 1.0, 1.0, BUTTON_BODY_DARK),
        (true, true) => Color::new(
            BUTTON_OPAQUE_DARK,
            BUTTON_OPAQUE_DARK,
            BUTTON_OPAQUE_DARK + 0.02,
            1.0,
        ),
        (false, false) => Color::new(1.0, 1.0, 1.0, BUTTON_BODY_LIGHT),
        (false, true) => Color::new(1.0, 1.0, 1.0, 1.0),
    };
    let tone = if dark { 1.0 } else { 0.0 };
    let rim = if contrast {
        BUTTON_RIM_HIGH_CONTRAST
    } else if dark {
        BUTTON_RIM_DARK
    } else {
        BUTTON_RIM_LIGHT
    };
    (body, Color::new(tone, tone, tone, rim))
}

/// A translucent button's body: in the light appearance a soft shade first, then the body and its rim, brightest at
/// the top and bottom edges.
fn paint_button_body(cx: &mut PaintCx<'_>, rect: Rect, radius: f64) {
    let theme = cx.env().theme.clone();
    let (body, rim) = button_body_colors(&theme);
    let scale = cx.env().scale.max(1.0);
    let shape = Shape::RoundedRect { rect, radius };
    if !theme.preferences.appearance.is_dark() {
        cx.push(PaintOp::PushLayer {
            opacity: 1.0,
            effects: vec![cupertino_ui_render::Effect::DropShadow {
                offset: cupertino_geometry::Point::new(0.0, 0.5),
                blur: SHADE_BLUR,
                color: Color::new(0.0, 0.0, 0.0, BUTTON_SHADE_LIGHT),
            }],
        });
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, 1.0)),
        });
        cx.push(PaintOp::PopLayer);
    }
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(body),
    });
    let px = 1.0 / scale;
    let middle = Color {
        a: rim.a * BUTTON_RIM_MIDDLE,
        ..rim
    };
    let stop = |offset, color| cupertino_ui_render::GradientStop { offset, color };
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rect.inset(Insets::uniform(px / 2.0)),
            radius: nested_radius(radius, px / 2.0),
        },
        width: px,
        paint: Paint::Linear {
            start: cupertino_geometry::Point::new(rect.min_x(), rect.min_y()),
            end: cupertino_geometry::Point::new(rect.min_x(), rect.max_y()),
            stops: vec![stop(0.0, rim), stop(0.5, middle), stop(1.0, rim)],
        },
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
        assert_eq!(GlassRole::ToolbarGroup.material(), GlassMaterial::Glass);
        assert_eq!(
            GlassRole::SidebarPanel.material(),
            GlassMaterial::Compositor(Surface::Sidebar)
        );
        assert_eq!(GlassRole::GlassButton.material(), GlassMaterial::Glass);
        assert_eq!(GlassRole::ThumbLens.material(), GlassMaterial::Painted);
    }

    #[test]
    fn content_surfaces_and_floating_controls_are_told_apart() {
        assert!(!GlassRole::SidebarPanel.floating());
        for role in [
            GlassRole::ToolbarGroup,
            GlassRole::GlassButton,
            GlassRole::ThumbLens,
        ] {
            assert!(role.floating(), "{role:?}");
            assert_eq!(role.container().0 == 0, role != GlassRole::ToolbarGroup);
        }
    }
}
