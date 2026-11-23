//! The panel's text and symbols: labels in the theme's type scale and tones, and the rounded fills under them.

use super::*;

/// Which size of the theme's type scale a label takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TypeSize {
    Body,
    Caption,
}

/// How a label is coloured. Over glass the label colours are the glass's foreground: a title, a tile's second line and
/// a primary label go to black on light glass and to near white on dark glass, a secondary label shifts the face less.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    /// A tile's title: the primary label colour.
    Title,
    /// A tile's second line: the primary label colour as the title's, so the state under it reads as well as the title
    /// over a bright backdrop.
    Line,
    /// The label colour at its full strength.
    Primary,
    /// The secondary label colour of the theme.
    Secondary,
}

/// One label of the panel, set in its own size and weight and drawn in its tone. The kit's static text sets every
/// label in the size of its role; the panel's second lines and headings are smaller than its titles.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Label {
    pub(super) content: String,
    pub(super) size: TypeSize,
    pub(super) weight: u16,
    pub(super) tone: Tone,
    /// Wraps onto more lines where its block is too narrow; else one line that ends in an ellipsis.
    pub(super) wrap: bool,
}

impl Label {
    pub(super) fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        let scale = &env.theme.type_scale;
        let size = match self.size {
            TypeSize::Body => scale.body.size,
            TypeSize::Caption => scale.caption.size,
        };
        let content = AttributedText::plain(
            &self.content,
            TextStyle {
                weight: FontWeight(self.weight),
                ..TextStyle::new(size)
            },
        );
        let style = ParagraphStyle {
            align: match env.direction {
                LayoutDirection::Ltr => TextAlign::Left,
                LayoutDirection::Rtl => TextAlign::Right,
            },
            wrap: if self.wrap { Wrap::Word } else { Wrap::None },
            ellipsis: !self.wrap,
            ..ParagraphStyle::default()
        };
        let paragraph = env.text.paragraph(&content, &style).ok()?;
        let width = width.is_finite().then_some(width as f32);
        if self.wrap {
            return Some(paragraph.layout(width));
        }
        let line = paragraph.layout(None);
        Some(match width {
            Some(width) if line.size().width > f64::from(width) => {
                let cut = paragraph.layout(Some(width));
                paragraph.layout(Some(cut.size().width as f32))
            }
            _ => line,
        })
    }

    /// The colour of the label where it is painted.
    pub(super) fn color(&self, cx: &PaintCx<'_>) -> Color {
        let colors = &cx.env().theme.colors;
        let tone = if dark(cx) { 1.0 } else { 0.0 };
        let fallback = |a: f32| Color::new(tone, tone, tone, a);
        let label = colors
            .get(ColorRole::Text, StateVariant::NORMAL)
            .unwrap_or(fallback(1.0));
        let secondary = colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            .unwrap_or(fallback(0.55));
        match self.tone {
            Tone::Title | Tone::Line | Tone::Primary => label,
            Tone::Secondary => secondary,
        }
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.content != old.content
            || self.size != old.size
            || self.weight != old.weight
            || self.wrap != old.wrap
        {
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
                size: Size::ZERO,
                baseline: None,
            },
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let width = cx.size().width;
        let Some(layout) = self.laid_out(cx.env(), width) else {
            return;
        };
        let origin = match cx.env().direction {
            LayoutDirection::Ltr => Point::ZERO,
            LayoutDirection::Rtl => Point::new(
                width - layout.lines().first().map_or(0.0, |line| line.rect.width()),
                0.0,
            ),
        };
        let color = self.color(cx);
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.content.clone());
    }
}

/// A label on one line.
pub(super) fn text<M: 'static>(
    content: &str,
    size: TypeSize,
    weight: u16,
    tone: Tone,
) -> Element<M> {
    Element::new(Label {
        content: content.to_owned(),
        size,
        weight,
        tone,
        wrap: false,
    })
}

/// A label that wraps onto more lines where its block is too narrow, as a capsule's title and state do.
pub(super) fn wrapping<M: 'static>(
    content: &str,
    size: TypeSize,
    weight: u16,
    tone: Tone,
) -> Element<M> {
    Element::new(Label {
        content: content.to_owned(),
        size,
        weight,
        tone,
        wrap: true,
    })
}

pub(super) fn icon<M: 'static>(name: &str, size: f64, tint: Option<ColorRole>) -> Element<M> {
    Element::new(Icon {
        name: name.to_owned(),
        size,
        direction: IconDirection::Fixed,
        label: None,
        tint,
    })
}

pub(super) fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, color: Color) {
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect { rect, radius },
        paint: Paint::Solid(color),
    });
}
