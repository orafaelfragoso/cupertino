//! The shell of a window in the full-height-sidebar style: the sidebar panel from the window's top edge to its bottom
//! edge, whose 52 pt top holds the compositor's window control buttons and moves the window, then a header slot and
//! the sidebar's content; beside it the content pane, which runs to the window's top edge under its floating toolbar.

use crate::glass::{nested_radius, paint_glass, GlassRole};
use crate::split::{split_element, DividerLook, DIVIDER};
use crate::toolbar::TOOLBAR_ROW;
use crate::window_drag::DragState;
use crate::{Handler, SplitLayout, SplitView};
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
    WindowBackground,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, StateVariant, Surface};

/// The window's corner radius, which the sidebar panel's corners follow.
pub const WINDOW_CORNER_RADIUS: f64 = 16.0;
/// The background of a small window that is a single panel, such as an About window or a prompt for permission: the
/// sidebar's glass under the whole window, its corners the window's, so it reads as the same glass as a sidebar
/// window's panel. Where the desktop's effects offer no glass it is the theme's opaque window colour in that shape.
pub const PANEL_WINDOW_BACKGROUND: WindowBackground = WindowBackground::Surface {
    surface: Surface::Sidebar,
    radius: WINDOW_CORNER_RADIUS,
    alpha: 1.0,
};
/// How far the sidebar panel lies inside the window's edges. The panel meets the top, leading and bottom edges, its
/// corners there the window's own; a panel inset from them would have [`nested_radius`] corners.
pub const SIDEBAR_PANEL_INSET: f64 = 0.0;
/// How far in from the panel's edges its warmer band reaches, and the band's colour at the edge, which fades out
/// towards the inside.
const EDGE_BAND: f64 = 20.0;
const EDGE_BAND_COLOR: [u8; 3] = [222, 214, 207];

/// The warm band along the leading and bottom edges of a panel `size` big.
fn paint_edge_band(cx: &mut PaintCx<'_>, size: Size) {
    use cupertino_geometry::Point;
    use cupertino_ui_render::GradientStop;
    use cupertino_ui_theme::Color;
    let edge = Color::from_rgba8([
        EDGE_BAND_COLOR[0],
        EDGE_BAND_COLOR[1],
        EDGE_BAND_COLOR[2],
        255,
    ]);
    let stops = |inner: Color| {
        vec![
            GradientStop {
                offset: 0.0,
                color: edge,
            },
            GradientStop {
                offset: 1.0,
                color: inner,
            },
        ]
    };
    let clear = edge.with_alpha(0.0);
    let band = EDGE_BAND.min(size.width).min(size.height);
    cx.push(PaintOp::Fill {
        shape: Shape::Rect(Rect::new(0.0, 0.0, band, size.height)),
        paint: Paint::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(band, 0.0),
            stops: stops(clear),
        },
    });
    cx.push(PaintOp::Fill {
        shape: Shape::Rect(Rect::new(band, size.height - band, size.width - band, band)),
        paint: Paint::Linear {
            start: Point::new(0.0, size.height),
            end: Point::new(0.0, size.height - band),
            stops: stops(clear),
        },
    });
}

/// The top of the sidebar panel that holds the window control buttons: the toolbar row's height.
pub const SIDEBAR_TOP: f64 = TOOLBAR_ROW;

/// A window laid out in the full-height-sidebar style, for a window opened with `DecorationMode::Sidebar`: the sidebar
/// panel beside the content pane, divided by a [`SplitView`]. The panel's glass is the [`GlassRole::SidebarPanel`]
/// role; the content pane paints the window background up to the window's top edge, so the window itself paints none
/// (`WindowBackground::None`). The top of the panel is the window's drag region, as a toolbar is: the compositor's
/// window control buttons lie there and take their presses first. Put a [`crate::NavigationStack`] in `content` for
/// the floating toolbar. Build its element with `Element::from`.
pub struct SidebarWindow<M> {
    /// The divider's accessible name.
    pub label: String,
    pub layout: SplitLayout,
    pub min_sidebar: f64,
    pub min_content: f64,
    pub on_change: Option<Handler<SplitLayout, M>>,
    /// Below the window control buttons, above `sidebar`: a search field, say.
    pub sidebar_header: Option<Element<M>>,
    /// The sidebar's content; it takes the height the top and the header leave.
    pub sidebar: Element<M>,
    /// The pane beside the sidebar.
    pub content: Element<M>,
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn column(insets: Insets) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        flex: Flex::grow(1.0),
        insets,
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

impl<M: 'static> From<SidebarWindow<M>> for Element<M> {
    fn from(window: SidebarWindow<M>) -> Self {
        let mut children = vec![Element::new(SidebarTop)];
        children.extend(window.sidebar_header);
        children.push(window.sidebar.fill());
        let panel = Element::new(Panel {
            inset: SIDEBAR_PANEL_INSET,
            children: vec![Element::new(PanelBody { children })],
        });
        let content = Element::new(ContentPane {
            children: vec![Element::new(UnderLine), window.content.fill()],
        });
        // where the sidebar's glass meets the content a fine rim, not a separator line
        split_element(
            SplitView {
                label: window.label,
                axis: Axis::Horizontal,
                leading: panel,
                trailing: content,
                layout: window.layout,
                min_leading: window.min_sidebar,
                min_trailing: window.min_content,
                on_change: window.on_change,
            },
            DividerLook::SidebarRim,
        )
    }
}

/// The sidebar pane: the panel inside its inset.
struct Panel<M> {
    inset: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Panel<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(Insets::uniform(self.inset))
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.inset != old.inset {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The panel's glass under the top, the header and the sidebar.
struct PanelBody<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for PanelBody<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(Insets::ZERO)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let radius = nested_radius(WINDOW_CORNER_RADIUS, SIDEBAR_PANEL_INSET);
        // a panel that meets the window's edges takes the window's corners, which the compositor cuts
        let radius = if SIDEBAR_PANEL_INSET > 0.0 {
            radius
        } else {
            0.0
        };
        let shown = paint_glass(
            cx,
            GlassRole::SidebarPanel,
            Rect::new(0.0, 0.0, size.width, size.height),
            radius,
        );
        // over a light window's panel, along its leading and bottom edges, a warmer band where more of the desktop
        // shows through
        let prefs = cx.env().theme.preferences;
        if shown
            && cx.window_active()
            && !prefs.appearance.is_dark()
            && !prefs.appearance.is_high_contrast()
            && !prefs.reduce_transparency
        {
            paint_edge_band(cx, size);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The top of the sidebar panel: room for the window control buttons, and the window's drag region.
struct SidebarTop;

impl<M: 'static> Widget<M> for SidebarTop {
    type State = DragState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(SIDEBAR_TOP),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> DragState {
        DragState::default()
    }

    fn update(&self, _: &Self, _: &mut DragState) -> Change {
        Change::None
    }

    fn measure(&self, _: &DragState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, drag: &mut DragState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        drag.event(event, cx)
    }

    fn paint(&self, _: &DragState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &DragState, _: &mut SemanticsCx<'_>) {}
}

/// The room under the divider's line at the content pane's leading edge, which the line covers.
struct UnderLine;

impl<M: 'static> Widget<M> for UnderLine {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(DIVIDER),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(DIVIDER, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: cupertino_geometry::Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The content pane: the window background from the window's top edge to its bottom edge, drawn by the compositor
/// where the desktop can ([`Surface::WindowBackground`]) and painted where it cannot. It begins under the divider's
/// line, so its background meets the sidebar's glass, and its content beside the line.
struct ContentPane<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ContentPane<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        // where the desktop can, the compositor draws the background, so the toolbar's glass over it reads the window
        // and not the desktop behind it
        if cx.material(Surface::WindowBackground, 0.0, 1.0) {
            return;
        }
        let state = StateVariant {
            window_active: cx.window_active(),
            ..StateVariant::NORMAL
        };
        let size = cx.size();
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::WindowBackground, state);
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_window_is_the_sidebar_glass_with_the_window_corners_and_opaque() {
        let WindowBackground::Surface {
            surface,
            radius,
            alpha,
        } = PANEL_WINDOW_BACKGROUND
        else {
            panic!("a panel window asks for a compositor surface");
        };
        assert_eq!(
            GlassRole::SidebarPanel.material(),
            crate::GlassMaterial::Compositor(surface),
            "the same glass as a sidebar window's panel"
        );
        assert_eq!((radius, alpha), (WINDOW_CORNER_RADIUS, 1.0));
    }
}
