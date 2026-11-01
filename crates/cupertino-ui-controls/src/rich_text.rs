use crate::look::paint_focus_ring;
use crate::press::{Keys, Outcome, Press};
use crate::text::leading_origin;
use crate::{RichText, Text};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    Action, Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx,
    Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::CursorShape;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, ParagraphLayout, TextRange};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::cell::RefCell;
use std::ops::Range;

/// How far below the baseline a link's underline sits, in points.
const UNDERLINE_GAP: f64 = 1.5;
/// The corner radius of the focus ring around a link's text.
const RING_RADIUS: f64 = 3.0;

/// `content` with its style runs cut at every link edge, so each shaped run is wholly inside or outside a link.
fn cut_at(content: &AttributedText, links: &[Range<usize>]) -> AttributedText {
    let edges: Vec<usize> = links
        .iter()
        .flat_map(|link| [link.start, link.end])
        .collect();
    let runs = content
        .runs
        .iter()
        .flat_map(|(range, style)| {
            let mut at: Vec<usize> = edges
                .iter()
                .copied()
                .filter(|&edge| range.0.start < edge && edge < range.0.end)
                .collect();
            at.push(range.0.start);
            at.push(range.0.end);
            at.sort_unstable();
            at.dedup();
            at.windows(2)
                .map(|pair| (TextRange(pair[0]..pair[1]), style.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    AttributedText {
        text: content.text.clone(),
        runs,
    }
}

impl<M: Clone + 'static> Widget<M> for RichText<M> {
    type State = ();

    /// The paragraph, with one node per link layered over it.
    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let ranges: Vec<Range<usize>> = self.links.iter().map(|link| link.range.clone()).collect();
        let text = Text {
            content: cut_at(&self.content, &ranges),
            role: self.role,
            wrap: self.wrap,
            secondary: self.secondary,
            selected: false,
        };
        let mut children = vec![Element::new(Body {
            text: text.clone(),
            links: ranges,
        })];
        children.extend(self.links.drain(..).map(|link| {
            Element::new(Link {
                text: text.clone(),
                range: link.range,
                url: link.url,
                on_open: link.on_open,
            })
        }));
        children
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The paragraph: the text in its colour, the links in the link colour over an underline.
struct Body {
    text: Text,
    links: Vec<Range<usize>>,
}

impl Body {
    fn in_link(&self, range: &Range<usize>) -> bool {
        self.links
            .iter()
            .any(|link| link.start <= range.start && range.end <= link.end)
    }
}

impl<M: 'static> Widget<M> for Body {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Layout
        } else if self.links != old.links {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        <Text as Widget<M>>::measure(&self.text, &(), constraints, cx)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.text.laid_out(cx.env(), cx.size().width) else {
            return;
        };
        let colors = &cx.env().theme.colors;
        let text_role = if self.text.secondary {
            ColorRole::SecondaryText
        } else {
            ColorRole::Text
        };
        let text = colors.get(text_role, StateVariant::NORMAL);
        let link = colors.get(ColorRole::Link, StateVariant::NORMAL);
        let device_pixel = 1.0 / cx.env().scale;
        let origin = leading_origin(&layout, cx.size().width, cx.env().direction);
        for line in layout.lines() {
            for run in &line.runs {
                let color = if self.in_link(&run.range.0) {
                    link
                } else {
                    text
                };
                if let Some(color) = color {
                    cx.push(PaintOp::Glyphs {
                        origin: origin + run.origin,
                        run: run.run.clone(),
                        color,
                    });
                }
            }
        }
        let Some(link) = link else {
            return;
        };
        for range in &self.links {
            for rect in underlines(&layout, range.clone()) {
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(Rect::new(
                        origin.x + rect.min_x(),
                        ((rect.min_y() + UNDERLINE_GAP) / device_pixel).round() * device_pixel,
                        rect.width(),
                        device_pixel,
                    )),
                    paint: Paint::Solid(link),
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        <Text as Widget<M>>::semantics(&self.text, &(), cx);
    }
}

/// Where `range` is drawn, one zero-height rectangle per piece on its line's baseline.
fn underlines(layout: &ParagraphLayout, range: Range<usize>) -> Vec<Rect> {
    let rects = layout.selection_rects(range);
    rects
        .into_iter()
        .filter_map(|rect| {
            let line = layout.lines().iter().find(|line| {
                line.rect.min_y() <= rect.center().y && rect.center().y < line.rect.max_y()
            })?;
            Some(Rect::new(rect.min_x(), line.baseline, rect.width(), 0.0))
        })
        .collect()
}

/// A link's node: stretched over the paragraph, it takes the pointer only over its own text.
struct Link<M> {
    text: Text,
    range: Range<usize>,
    url: String,
    on_open: Option<M>,
}

#[derive(Default)]
struct LinkState {
    press: Press,
    /// Where the link's text was last laid out, in node coordinates.
    rects: RefCell<Vec<Rect>>,
}

impl<M: Clone + 'static> Widget<M> for Link<M> {
    type State = LinkState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> LinkState {
        LinkState::default()
    }

    /// Its rects come from measure, so new text or a new range lays it out again.
    fn update(&self, old: &Self, _: &mut LinkState) -> Change {
        if self.text != old.text || self.range != old.range {
            Change::Layout
        } else {
            Change::None
        }
    }

    /// Stretched over the paragraph, it is last measured at the paragraph's width, where it finds its text.
    fn measure(
        &self,
        state: &LinkState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        let width = constraints.width.max();
        *state.rects.borrow_mut() = self
            .text
            .laid_out(cx.env(), width)
            .map(|layout| {
                let origin = leading_origin(&layout, width, cx.env().direction);
                layout
                    .selection_rects(self.range.clone())
                    .into_iter()
                    .map(|rect| rect.translate(origin.x, origin.y))
                    .collect()
            })
            .unwrap_or_default();
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, state: &mut LinkState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let hovered = state.press.hovered();
        let outcome = state.press.event(event, true, Keys::Return, cx);
        if state.press.hovered() != hovered {
            cx.set_cursor(state.press.hovered().then_some(CursorShape::Pointer));
        }
        match outcome {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                if let Some(message) = &self.on_open {
                    cx.emit(message.clone());
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, state: &LinkState, cx: &mut PaintCx<'_>) {
        for rect in state.rects.borrow().iter() {
            paint_focus_ring(cx, *rect, RING_RADIUS);
        }
    }

    fn semantics(&self, state: &LinkState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Link);
        cx.name(
            self.text
                .content
                .text
                .get(self.range.clone())
                .unwrap_or_default(),
        );
        cx.description(self.url.clone());
        cx.focusable();
        cx.action(Action::Click);
        if let Some(bounds) = state
            .rects
            .borrow()
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
        {
            cx.bounds(bounds);
        }
    }

    fn hit(&self, state: &LinkState, point: Point) -> bool {
        state.rects.borrow().iter().any(|rect| rect.contains(point))
    }
}
