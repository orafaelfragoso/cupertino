//! A bar chart: a row of bars over a plot with horizontal guide lines, value labels at the right and labels along the
//! bottom. It reads, it takes no input. The bars' colour is the chart's; a bar can be set apart (dimmer) to mark a
//! span, such as the time a battery was charging.

use crate::look;
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    Change, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, ParagraphStyle, TextAlign, TextStyle};
use cupertino_ui_theme::{Color, ColorRole, StateVariant};

/// One bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartBar {
    /// How tall the bar is, 0 to 1 of the plot.
    pub value: f64,
    /// Drawn dimmer, to mark a span.
    pub marked: bool,
}

/// The colour of a chart's bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartColor {
    Green,
    Blue,
}

/// A bar chart. `label` is what assistive technology reads; `summary` says what the chart shows in words.
pub struct BarChart {
    pub label: String,
    pub summary: String,
    pub bars: Vec<ChartBar>,
    pub color: ChartColor,
    /// Labels at the right of the plot: the position from the bottom (0 to 1) and the text. Each has a guide line.
    pub value_labels: Vec<(f64, String)>,
    /// Labels under the plot: the position from the left (0 to 1) and the text.
    pub time_labels: Vec<(f64, String)>,
    /// The plot's height, in points.
    pub height: f64,
}

/// The room at the right for the value labels, and the height under the plot for the time labels.
const VALUE_ROOM: f64 = 36.0;
const TIME_ROOM: f64 = 18.0;
const LABEL_SIZE: f32 = 11.0;
/// The gap between two bars, as a share of a bar slot.
const BAR_GAP: f64 = 0.25;
/// How much dimmer a marked bar is.
const MARKED_SHARE: f32 = 0.45;
const GREEN: [u8; 3] = [52, 199, 89];
const BLUE: [u8; 3] = [10, 132, 255];

impl<M: 'static> Widget<M> for BarChart {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.height != old.height {
            Change::Layout
        } else if self.bars != old.bars
            || self.color != old.color
            || self.value_labels != old.value_labels
            || self.time_labels != old.time_labels
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(
                constraints.width.ideal().max(VALUE_ROOM * 2.0),
                self.height + TIME_ROOM,
            ),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let size = cx.size();
        let plot = Rect::new(0.0, 0.0, (size.width - VALUE_ROOM).max(1.0), self.height);
        let text = theme.colors.get(ColorRole::Text, StateVariant::NORMAL);
        let faint = look::faded(text, 0.18);
        let [r, g, b] = match self.color {
            ChartColor::Green => GREEN,
            ChartColor::Blue => BLUE,
        };
        let bar_color = Color::from_rgba8([r, g, b, 255]);
        let label = |cx: &mut PaintCx<'_>, content: &str, at: Point, centred: bool| {
            let style = TextStyle::new(LABEL_SIZE);
            let paragraph = env.text.paragraph(
                &AttributedText::plain(content, style),
                &ParagraphStyle {
                    align: TextAlign::Start,
                    ..ParagraphStyle::default()
                },
            );
            if let Ok(paragraph) = paragraph {
                let layout = look::one_line(&paragraph, f64::INFINITY);
                let width = layout.size().width;
                let x = if centred { at.x - width / 2.0 } else { at.x };
                look::glyphs(cx, &layout, Point::new(x, at.y), look::faded(text, 0.55));
            }
        };
        // the guide lines and their value labels
        for (position, content) in &self.value_labels {
            let y = plot.max_y() - position.clamp(0.0, 1.0) * plot.height();
            if let Some(color) = faint {
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(Rect::new(0.0, y - 0.25, plot.width(), 0.5)),
                    paint: Paint::Solid(color),
                });
            }
            label(cx, content, Point::new(plot.width() + 6.0, y - 7.0), false);
        }
        // the bars
        if !self.bars.is_empty() {
            let slot = plot.width() / self.bars.len() as f64;
            let width = (slot * (1.0 - BAR_GAP)).max(1.0);
            for (i, bar) in self.bars.iter().enumerate() {
                let height = (bar.value.clamp(0.0, 1.0) * plot.height()).max(if bar.value > 0.0 {
                    1.0
                } else {
                    0.0
                });
                if height <= 0.0 {
                    continue;
                }
                let color = if bar.marked {
                    look::faded(Some(bar_color), MARKED_SHARE)
                } else {
                    Some(bar_color)
                };
                look::fill(
                    cx,
                    Rect::new(
                        slot * i as f64 + (slot - width) / 2.0,
                        plot.max_y() - height,
                        width,
                        height,
                    ),
                    (width / 2.0).min(2.0),
                    color,
                );
            }
        }
        // the labels under the plot
        for (position, content) in &self.time_labels {
            let x = position.clamp(0.0, 1.0) * plot.width();
            label(cx, content, Point::new(x, plot.max_y() + 4.0), true);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Image);
        cx.name(self.label.clone());
        cx.describe_content(self.summary.clone());
    }
}
