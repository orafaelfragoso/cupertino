//! A choice drawn as a picture: a small mock of a look (light, dark or both) with its name under it, or a round dot of
//! a colour. The chosen one has a ring in the accent colour. One Tab stop; a click, Space or Return chooses it.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{GradientStop, Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphStyle, TextAlign, TextStyle};
use cupertino_ui_theme::{Color, ColorRole, StateVariant};

/// A look the desktop can have, as a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwatchLook {
    Light,
    Dark,
    /// Light and dark side by side: the look follows the time of day.
    Auto,
}

/// What a [`Swatch`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwatchFill {
    /// A mock of a window in this look, 66 by 45 points, with a caption.
    Look(SwatchLook),
    /// A dot of this sRGB colour, 22 points across.
    Color([u8; 3]),
    /// A dot of every accent colour in turn.
    Multicolor,
}

/// One choice of a row of choices drawn as pictures. `label` is its name, shown under a look's mock and read to
/// assistive technology for every kind. Build its element with `Element::new`.
pub struct Swatch<M> {
    pub label: String,
    pub fill: SwatchFill,
    pub selected: bool,
    pub enabled: bool,
    pub on_press: Option<M>,
}

/// The mock's size, its corners and the gap between its ring and itself.
const LOOK_SIZE: Size = Size::new(66.0, 45.0);
const LOOK_RADIUS: f64 = 6.0;
/// The dot's diameter.
const DOT: f64 = 22.0;
/// The ring of the chosen one: its width, and the room it takes around the picture.
const RING_WIDTH: f64 = 3.0;
const DOT_RING_WIDTH: f64 = 2.0;
const RING_GAP: f64 = 1.0;
/// Between the ring's room and the caption, and the caption's size and line height.
const CAPTION_GAP: f64 = 4.0;
const CAPTION_SIZE: f32 = 11.0;
const CAPTION_HEIGHT: f64 = 14.0;
/// The mock's colours: window, sidebar strip, a row of text and the accent pill, for the light and the dark look.
const MOCK_LIGHT: [[u8; 3]; 4] = [
    [246, 246, 248],
    [226, 226, 231],
    [190, 190, 196],
    [0, 122, 255],
];
const MOCK_DARK: [[u8; 3]; 4] = [[44, 44, 48], [34, 34, 38], [96, 96, 104], [0, 122, 255]];
/// A hairline round the mock, so a light one stands apart from a light window.
const MOCK_RIM_ALPHA: f32 = 0.16;
/// The colours of the multicolour dot, from one edge to the other.
const SPECTRUM: [[u8; 3]; 6] = [
    [224, 56, 62],
    [247, 130, 27],
    [255, 199, 38],
    [98, 186, 70],
    [0, 122, 255],
    [149, 61, 150],
];

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba8([c[0], c[1], c[2], 255])
}

impl<M> Swatch<M> {
    fn is_look(&self) -> bool {
        matches!(self.fill, SwatchFill::Look(_))
    }

    /// The room the ring takes on each side.
    fn room(&self) -> f64 {
        if self.is_look() {
            RING_GAP + RING_WIDTH
        } else {
            DOT_RING_WIDTH + RING_GAP
        }
    }

    fn picture(&self) -> Rect {
        let room = self.room();
        match self.fill {
            SwatchFill::Look(_) => Rect::new(room, room, LOOK_SIZE.width, LOOK_SIZE.height),
            _ => Rect::new(room, room, DOT, DOT),
        }
    }
}

/// A mock of a window in `colors` (window, sidebar strip, text, pill) filling `rect`.
fn mock(cx: &mut PaintCx<'_>, rect: Rect, colors: &[[u8; 3]; 4]) {
    let fill = |cx: &mut PaintCx<'_>, r: Rect, radius: f64, c: [u8; 3]| {
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect { rect: r, radius },
            paint: Paint::Solid(rgb(c)),
        });
    };
    fill(cx, rect, 0.0, colors[0]);
    let strip = Rect::new(
        rect.min_x(),
        rect.min_y(),
        rect.width() * 0.3,
        rect.height(),
    );
    fill(cx, strip, 0.0, colors[1]);
    // the window's buttons, as three dots
    for i in 0..3 {
        let dot = Rect::new(
            rect.min_x() + 4.0 + 4.5 * f64::from(i),
            rect.min_y() + 4.0,
            3.0,
            3.0,
        );
        fill(cx, dot, 1.5, colors[2]);
    }
    // a selected row in the strip, and lines of text in the page
    let pill = Rect::new(
        rect.min_x() + 3.0,
        rect.min_y() + 14.0,
        strip.width() - 6.0,
        6.0,
    );
    fill(cx, pill, 2.0, colors[3]);
    let page = rect.inset(Insets {
        top: 7.0,
        left: strip.width() + 6.0,
        bottom: 0.0,
        right: 6.0,
    });
    for (i, width) in [1.0, 0.75, 0.9].into_iter().enumerate() {
        let line = Rect::new(
            page.min_x(),
            page.min_y() + 6.0 * i as f64,
            page.width() * width,
            2.5,
        );
        fill(cx, line, 1.25, colors[2]);
    }
}

impl<M: Clone + 'static> Widget<M> for Swatch<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.fill != old.fill {
            Change::Layout
        } else if self.selected != old.selected
            || self.enabled != old.enabled
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        let picture = self.picture();
        let room = self.room();
        let caption = if self.is_look() {
            CAPTION_GAP + CAPTION_HEIGHT
        } else {
            0.0
        };
        Measured {
            size: Size::new(
                picture.width() + 2.0 * room,
                picture.height() + 2.0 * room + caption,
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

    fn paint(&self, _: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let picture = self.picture();
        let text = theme.colors.get(ColorRole::Text, StateVariant::NORMAL);
        let radius = match self.fill {
            SwatchFill::Look(_) => LOOK_RADIUS,
            _ => DOT / 2.0,
        };
        cx.push(PaintOp::PushClip(Shape::RoundedRect {
            rect: picture,
            radius,
        }));
        match self.fill {
            SwatchFill::Look(SwatchLook::Light) => mock(cx, picture, &MOCK_LIGHT),
            SwatchFill::Look(SwatchLook::Dark) => mock(cx, picture, &MOCK_DARK),
            SwatchFill::Look(SwatchLook::Auto) => {
                let half = picture.width() / 2.0;
                let left = Rect::new(picture.min_x(), picture.min_y(), half, picture.height());
                let right = Rect::new(
                    picture.min_x() + half,
                    picture.min_y(),
                    half,
                    picture.height(),
                );
                for (side, colors) in [(left, &MOCK_LIGHT), (right, &MOCK_DARK)] {
                    cx.push(PaintOp::PushClip(Shape::Rect(side)));
                    mock(cx, picture, colors);
                    cx.push(PaintOp::PopClip);
                }
            }
            SwatchFill::Color(c) => look::fill(cx, picture, radius, Some(rgb(c))),
            SwatchFill::Multicolor => {
                let last = (SPECTRUM.len() - 1) as f32;
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(picture),
                    paint: Paint::Linear {
                        start: Point::new(picture.min_x(), picture.min_y()),
                        end: Point::new(picture.max_x(), picture.max_y()),
                        stops: SPECTRUM
                            .iter()
                            .enumerate()
                            .map(|(i, c)| GradientStop {
                                offset: i as f32 / last,
                                color: rgb(*c),
                            })
                            .collect(),
                    },
                });
            }
        }
        cx.push(PaintOp::PopClip);
        // a hairline round the picture
        look::stroke(
            cx,
            picture.inset(Insets::uniform(0.25)),
            radius - 0.25,
            0.5,
            look::faded(text, MOCK_RIM_ALPHA),
        );
        let ring_width = if self.is_look() {
            RING_WIDTH
        } else {
            DOT_RING_WIDTH
        };
        if self.selected {
            let accent = theme
                .colors
                .get(ColorRole::SelectedControlFill, StateVariant::NORMAL);
            let outer = picture.outset(RING_GAP + ring_width / 2.0);
            look::stroke(
                cx,
                outer,
                radius + RING_GAP + ring_width / 2.0,
                ring_width,
                accent,
            );
        }
        if self.is_look() {
            let style = TextStyle {
                weight: FontWeight(if self.selected {
                    700
                } else {
                    theme.type_scale.body.weight
                }),
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
                let width = picture.width() + 2.0 * self.room();
                let layout = look::one_line(&paragraph, width);
                let x = (width - layout.size().width) / 2.0;
                let y = picture.max_y() + self.room() + CAPTION_GAP;
                look::glyphs(cx, &layout, Point::new(x, y), text);
            }
        }
        paint_focus_ring(
            cx,
            picture.outset(RING_GAP + ring_width),
            radius + RING_GAP + ring_width,
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
