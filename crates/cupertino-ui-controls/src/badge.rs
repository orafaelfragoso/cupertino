use crate::look;
use crate::Badge;
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    Change, ControlState, Environment, Event, EventCx, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::ColorRole;

/// The pill's height and its narrowest width, a circle for one digit. Approximate design values.
const PILL: f64 = 16.0;
const SIDE_PADDING: f64 = 5.0;
const TEXT_SIZE: f32 = 11.0;
/// The dot variant, as the dock's running indicator.
const DOT: f64 = 4.0;

fn layout(env: &Environment, text: &str) -> Option<ParagraphLayout> {
    let style = TextStyle {
        weight: FontWeight::SEMIBOLD,
        ..TextStyle::new(TEXT_SIZE)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

impl<M: 'static> Widget<M> for Badge {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let size = match &self.text {
            Some(text) if text.chars().count() <= 1 => Size::new(PILL, PILL),
            Some(text) => {
                let width = layout(cx.env(), text).map_or(0.0, |l| l.size().width);
                Size::new((width + 2.0 * SIDE_PADDING).max(PILL), PILL)
            }
            None => Size::new(DOT, DOT),
        };
        Measured {
            size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let bounds = Rect::new(0.0, 0.0, size.width, size.height);
        let Some(text) = &self.text else {
            let dot = look::color(&theme, ColorRole::SecondaryText, &state);
            look::fill(cx, bounds, size.height / 2.0, dot);
            return;
        };
        let fill = look::color(&theme, ColorRole::DestructiveFill, &state);
        look::fill(cx, bounds, size.height / 2.0, fill);
        if let Some(text) = layout(cx.env(), text) {
            let label = text.size();
            let origin = Point::new(
                (size.width - label.width) / 2.0,
                (size.height - label.height) / 2.0,
            );
            let color = look::color(&theme, ColorRole::DestructiveText, &state);
            look::glyphs(cx, &text, origin, color);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.description(self.description.clone());
    }
}
