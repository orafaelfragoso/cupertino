//! A picture of a grid of pictures: a rounded thumbnail with its caption under it and, for the one in use, a neutral
//! ring around it. One Tab stop; a click, Space or Return chooses it.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::ThumbnailTile;
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{PaintOp, Sampling, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphStyle, TextAlign, TextStyle};
use cupertino_ui_theme::{Color, ColorRole, StateVariant};

/// Room around the picture for the ring of the chosen one, which lies outside it.
const RING_ROOM: f64 = 3.0;
/// The ring's width, and the gap between it and the picture, which shows the window's own colour.
const RING_WIDTH: f64 = 2.0;
const RING_GAP: f64 = 0.5;
/// The ring's grey over a light window and over a dark one, in 8-bit steps.
const RING_LIGHT: u8 = 215;
const RING_DARK: u8 = 140;
/// Between the picture's ring room and its caption, and the caption's size and line height.
const CAPTION_GAP: f64 = 3.0;
const CAPTION_SIZE: f32 = 11.0;
const CAPTION_HEIGHT: f64 = 14.0;
/// How much of the picture a press darkens, as a share of black.
const PRESS_SHADE: f32 = 0.12;
/// What the picture shows before its pixels are ready, as a share of the label colour.
const PLACEHOLDER_SHARE: f32 = 0.08;

impl<M> ThumbnailTile<M> {
    fn picture(&self) -> Rect {
        Rect::new(RING_ROOM, RING_ROOM, self.size.width, self.size.height)
    }
}

impl<M: Clone + 'static> Widget<M> for ThumbnailTile<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.size != old.size || self.caption != old.caption {
            Change::Layout
        } else if self.image != old.image
            || self.selected != old.selected
            || self.enabled != old.enabled
            || self.radius != old.radius
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        let caption = if self.caption {
            CAPTION_GAP + CAPTION_HEIGHT
        } else {
            0.0
        };
        Measured {
            size: Size::new(
                self.size.width + 2.0 * RING_ROOM,
                self.size.height + 2.0 * RING_ROOM + caption,
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.enabled, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(message) = &self.on_press {
                    cx.emit(message.clone());
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let picture = self.picture();
        match &self.image {
            Some(source) => {
                cx.push(PaintOp::PushClip(Shape::RoundedRect {
                    rect: picture,
                    radius: self.radius,
                }));
                cover(cx, source, picture);
                if press.pressed() {
                    cx.push(PaintOp::Fill {
                        shape: Shape::Rect(picture),
                        paint: cupertino_ui_render::Paint::Solid(Color::new(
                            0.0,
                            0.0,
                            0.0,
                            PRESS_SHADE,
                        )),
                    });
                }
                cx.push(PaintOp::PopClip);
            }
            None => {
                let tint = theme.colors.get(ColorRole::Text, StateVariant::NORMAL);
                look::fill(
                    cx,
                    picture,
                    self.radius,
                    look::faded(tint, PLACEHOLDER_SHARE),
                );
            }
        }
        if self.selected {
            let grey = if theme.preferences.appearance.is_dark() {
                RING_DARK
            } else {
                RING_LIGHT
            };
            let ring = Color::from_rgba8([grey, grey, grey, 255]);
            // the window's colour between the picture and the ring, then the ring outside it
            let gap = picture.outset(RING_GAP / 2.0);
            look::stroke(
                cx,
                gap,
                self.radius + RING_GAP / 2.0,
                RING_GAP,
                theme
                    .colors
                    .get(ColorRole::WindowBackground, StateVariant::NORMAL),
            );
            let outer = picture.outset(RING_GAP + RING_WIDTH / 2.0);
            look::stroke(
                cx,
                outer,
                self.radius + RING_GAP + RING_WIDTH / 2.0,
                RING_WIDTH,
                Some(ring),
            );
        }
        if self.caption {
            let style = TextStyle {
                weight: FontWeight(theme.type_scale.body.weight),
                ..TextStyle::new(CAPTION_SIZE)
            };
            let paragraph = env.text.paragraph(
                &AttributedText::plain(&self.label, style),
                &ParagraphStyle {
                    align: TextAlign::Center,
                    ..ParagraphStyle::default()
                },
            );
            if let Ok(paragraph) = paragraph {
                let width = self.size.width + 2.0 * RING_ROOM;
                let layout = look::one_line(&paragraph, width);
                let x = (width - layout.size().width) / 2.0;
                let y = picture.max_y() + RING_ROOM + CAPTION_GAP;
                let color = theme.colors.get(ColorRole::Text, StateVariant::NORMAL);
                look::glyphs(cx, &layout, Point::new(x, y), color);
            }
        }
        paint_focus_ring(
            cx,
            picture.outset(RING_GAP + RING_WIDTH),
            self.radius + RING_GAP + RING_WIDTH,
        );
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            selected: self.selected,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// Draws `source` over all of `area`, cropped to it and centred.
fn cover(cx: &mut PaintCx<'_>, source: &crate::ImageSource, area: Rect) {
    let (width, height) = (
        f64::from(source.image.width),
        f64::from(source.image.height),
    );
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let scale = (area.width() / width).max(area.height() / height);
    let size = Size::new(width * scale, height * scale);
    let dst = Rect::new(
        area.min_x() + (area.width() - size.width) / 2.0,
        area.min_y() + (area.height() - size.height) / 2.0,
        size.width,
        size.height,
    );
    cx.image(source.id, &source.image, dst, Sampling::Linear);
}
