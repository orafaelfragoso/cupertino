use crate::look;
use crate::{Text, TextRole};
use cupertino_geometry::Point;
use cupertino_ui_core::{
    Change, Environment, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, Wrap,
};
use cupertino_ui_theme::{ColorRole, StateVariant, ThemeTokens};

impl Text {
    /// The content set in its role: the role's size everywhere, and its weight where a run keeps the regular one.
    fn styled(&self, theme: &ThemeTokens) -> AttributedText {
        let scale = &theme.type_scale;
        let role = match self.role {
            TextRole::Body => scale.body,
            TextRole::Caption => scale.caption,
            TextRole::Section => scale.section,
            TextRole::Title => scale.title,
        };
        let mut content = self.content.clone();
        for (_, style) in &mut content.runs {
            style.size = role.size;
            if style.weight == FontWeight::REGULAR {
                style.weight = FontWeight(role.weight);
            }
        }
        content
    }

    /// Laid out within `width` when it wraps, else on one line that ends in an ellipsis past `width`, its lines against
    /// the layout's leading edge; `None` if the fonts cannot shape it.
    pub(crate) fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        let style = ParagraphStyle {
            align: match env.direction {
                LayoutDirection::Ltr => TextAlign::Left,
                LayoutDirection::Rtl => TextAlign::Right,
            },
            wrap: if self.wrap { Wrap::Word } else { Wrap::None },
            ellipsis: !self.wrap,
            ..ParagraphStyle::default()
        };
        let paragraph = env.text.paragraph(&self.styled(&env.theme), &style).ok()?;
        Some(if self.wrap {
            paragraph.layout(width.is_finite().then_some(width as f32))
        } else {
            look::one_line(&paragraph, width)
        })
    }
}

/// Where a layout laid out for a node `width` wide is drawn: against the node's leading edge.
pub(crate) fn leading_origin(
    layout: &ParagraphLayout,
    width: f64,
    direction: LayoutDirection,
) -> Point {
    match direction {
        LayoutDirection::Ltr => Point::ZERO,
        LayoutDirection::Rtl => Point::new(
            width - layout.lines().first().map_or(0.0, |line| line.rect.width()),
            0.0,
        ),
    }
}

impl<M: 'static> Widget<M> for Text {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        // the colours are paint; what is set, in which role and whether it wraps, is size
        if self.content != old.content || self.role != old.role || self.wrap != old.wrap {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        match self.laid_out(cx.env(), constraints.width.max()) {
            Some(layout) => Measured {
                size: layout.size(),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => Measured {
                size: Default::default(),
                baseline: None,
            },
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx.env(), cx.size().width) else {
            return;
        };
        let (role, variant) = if self.selected {
            (
                ColorRole::ControlText,
                StateVariant {
                    selected: true,
                    window_active: cx.window_active(),
                    ..StateVariant::NORMAL
                },
            )
        } else if self.secondary {
            (ColorRole::SecondaryText, StateVariant::NORMAL)
        } else {
            (ColorRole::Text, StateVariant::NORMAL)
        };
        let color = cx.env().theme.colors.get(role, variant);
        let origin = leading_origin(&layout, cx.size().width, cx.env().direction);
        look::glyphs(cx, &layout, origin, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.content.text.clone());
    }
}
