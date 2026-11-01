use crate::look::{self, paint_ring};
use crate::TextBox;
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx,
    Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_theme::ColorRole;

const EDGE: f64 = 1.0;
/// Keeps the content's square corners inside the rounded border.
const CONTENT_INSET: f64 = 2.0;

/// Whether the focus is inside the box, and whether it came from the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextBoxState {
    pub focused: bool,
    pub focus_visible: bool,
}

impl<M: 'static> Widget<M> for TextBox<M> {
    type State = TextBoxState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform(CONTENT_INSET),
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
        vec![std::mem::replace(
            &mut self.content,
            Element::new(crate::Spacer { min: 0.0 }),
        )]
    }

    fn state(&self) -> TextBoxState {
        TextBoxState::default()
    }

    fn update(&self, old: &Self, _: &mut TextBoxState) -> Change {
        if self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &TextBoxState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut TextBoxState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if let Event::FocusWithin { gained, visible } = *event {
            *state = TextBoxState {
                focused: gained,
                focus_visible: gained && visible,
            };
            cx.request_paint();
        }
        Propagation::Continue
    }

    fn paint(&self, state: &TextBoxState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let look = ControlState {
            enabled: self.enabled,
            focused: state.focused,
            focus_visible: state.focus_visible,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = theme.metrics.field_radius;
        look::fill(
            cx,
            rect,
            radius,
            look::color(&theme, ColorRole::TextBackground, &look),
        );
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            radius - EDGE / 2.0,
            EDGE,
            look::color(&theme, ColorRole::TextFieldBorder, &look),
        );
        if state.focus_visible && cx.window_active() {
            paint_ring(cx, rect, radius);
        }
    }

    fn semantics(&self, _: &TextBoxState, _: &mut SemanticsCx<'_>) {}
}
