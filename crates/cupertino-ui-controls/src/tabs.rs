use crate::choice::{items, no_measure, paint_segments, segment_width, Look, SEGMENT_RADIUS};
use crate::look;
use crate::Tabs;
use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_core::{
    Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{FillRule, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::{ColorRole, ControlFamily};

const PANEL_RADIUS: f64 = 6.0;
const PANEL_PADDING: f64 = 12.0;
const EDGE: f64 = 1.0;

impl<M: Clone + 'static> From<Tabs<M>> for Element<M> {
    fn from(tabs: Tabs<M>) -> Self {
        let name = tabs.tabs.get(tabs.selected).cloned().unwrap_or_default();
        let tabs_labels = tabs.tabs.clone();
        let row = Element::new(TabRow {
            label: tabs.label,
            children: items(
                Look::Tab,
                &tabs.tabs,
                Some(tabs.selected),
                Axis::Horizontal,
                true,
                tabs.on_select.as_ref(),
            ),
            tabs: tabs.tabs,
            selected: tabs.selected,
        });
        // keyed by the tab, so a newly selected tab's content mounts fresh
        let panel = Element::new(Panel {
            name,
            children: vec![tabs.content],
        })
        .key(tabs.selected as u64);
        Element::new(TabView {
            tabs: tabs_labels,
            children: vec![row, panel],
        })
    }
}

/// The row over the panel; it paints the panel's frame, from the row's middle down, around the row's chrome.
struct TabView<M> {
    tabs: Vec<String>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for TabView<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
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

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.tabs != old.tabs {
            Change::Paint
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

    /// The frame starts at the chrome's middle, on a device pixel, and leaves the chrome alone: a translucent row
    /// shows the window behind it, not the panel.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = env.theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let height = look::geometry(&theme, ControlFamily::Segmented).height;
        let width = segment_width(&env, &self.tabs) * self.tabs.len() as f64;
        let chrome = Rect::new((size.width - width) / 2.0, 0.0, width, height);
        let top = (chrome.center().y * env.scale).round() / env.scale;
        let frame = Rect::new(0.0, top, size.width, size.height - top);
        let mut verbs = rounded(Rect::new(0.0, 0.0, size.width, size.height), 0.0);
        verbs.extend(rounded(chrome, SEGMENT_RADIUS));
        cx.push(PaintOp::PushClip(Shape::Path {
            verbs,
            fill_rule: FillRule::EvenOdd,
        }));
        look::fill(
            cx,
            frame,
            PANEL_RADIUS,
            look::color(&theme, ColorRole::ControlFill, &state),
        );
        look::stroke(
            cx,
            frame.inset(Insets::uniform(EDGE / 2.0)),
            PANEL_RADIUS - EDGE / 2.0,
            EDGE,
            look::color(&theme, ColorRole::ControlBorder, &state),
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A closed rounded rectangle, its corners as cubic quarter circles.
pub(crate) fn rounded(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // the control-point distance for a quarter circle
    let k = radius * 0.552_284_749_8;
    let (x0, y0, x1, y1) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let p = Point::new;
    vec![
        PathVerb::MoveTo(p(x0 + radius, y0)),
        PathVerb::LineTo(p(x1 - radius, y0)),
        PathVerb::CubicTo(
            p(x1 - radius + k, y0),
            p(x1, y0 + radius - k),
            p(x1, y0 + radius),
        ),
        PathVerb::LineTo(p(x1, y1 - radius)),
        PathVerb::CubicTo(
            p(x1, y1 - radius + k),
            p(x1 - radius + k, y1),
            p(x1 - radius, y1),
        ),
        PathVerb::LineTo(p(x0 + radius, y1)),
        PathVerb::CubicTo(
            p(x0 + radius - k, y1),
            p(x0, y1 - radius + k),
            p(x0, y1 - radius),
        ),
        PathVerb::LineTo(p(x0, y0 + radius)),
        PathVerb::CubicTo(
            p(x0, y0 + radius - k),
            p(x0 + radius - k, y0),
            p(x0 + radius, y0),
        ),
        PathVerb::Close,
    ]
}

/// The tabs, centred over the panel: one Tab stop, at the selected tab.
struct TabRow<M> {
    label: String,
    tabs: Vec<String>,
    selected: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for TabRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            align_self: Some(Align::Center),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.tabs != old.tabs {
            Change::Layout
        } else if self.selected != old.selected {
            Change::Paint
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        paint_segments(cx, &self.tabs, Some(self.selected), true, false);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TabList);
        cx.name(self.label.clone());
        cx.item_count(self.tabs.len());
        cx.roving_group();
    }
}

/// The selected tab's content.
struct Panel<M> {
    name: String,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Panel<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform(PANEL_PADDING),
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name {
            Change::Paint
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

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TabPanel);
        cx.name(self.name.clone());
    }
}
