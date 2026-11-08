//! The shell of a window in the full-height-sidebar style: the sidebar panel from the window's top edge to its bottom
//! edge, whose 52 pt top holds the compositor's window control buttons and moves the window, then a header slot and
//! the sidebar's content; beside it the content pane, which runs to the window's top edge under its floating toolbar.

use crate::glass::{nested_radius, paint_glass, GlassRole};
use crate::toolbar::TOOLBAR_ROW;
use crate::window_drag::DragState;
use crate::{Handler, SplitLayout, SplitView};
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, StateVariant};

/// The window's corner radius, which the sidebar panel's corners follow.
pub const WINDOW_CORNER_RADIUS: f64 = 16.0;
/// How far the sidebar panel lies inside the window's edges. The panel meets the top, leading and bottom edges, its
/// corners there the window's own; a panel inset from them would have [`nested_radius`] corners.
pub const SIDEBAR_PANEL_INSET: f64 = 0.0;
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
            children: vec![window.content.fill()],
        });
        Element::from(SplitView {
            label: window.label,
            axis: Axis::Horizontal,
            leading: panel,
            trailing: content,
            layout: window.layout,
            min_leading: window.min_sidebar,
            min_trailing: window.min_content,
            on_change: window.on_change,
        })
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
        let radius = if SIDEBAR_PANEL_INSET > 0.0 { radius } else { 0.0 };
        paint_glass(
            cx,
            GlassRole::SidebarPanel,
            Rect::new(0.0, 0.0, size.width, size.height),
            radius,
        );
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

/// The content pane: the window background from the window's top edge to its bottom edge.
struct ContentPane<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ContentPane<M> {
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
