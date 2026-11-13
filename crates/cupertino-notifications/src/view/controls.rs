//! The controls a card of the centre shows while the pointer or the keyboard is on it: the close circle at its
//! top-leading corner (which a collapsed group's card grows into a capsule), the chevron that takes the age's place,
//! and the pill of its first action over its bottom-trailing corner.

use cupertino_kit::core::{
    Action, Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, Flex, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{MouseButton, PointerEvent};
use cupertino_kit::render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{FontWeight, TextAlign};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::{
    color, Input, Label, CLOSE_DIAMETER, CLOSE_REACH_X, CLOSE_REACH_Y, KEYPAD_ENTER, OUTLINE_WIDTH,
    RETURN, SPACE,
};

/// The chevron's size: it points right while a card is folded and down while it is unfolded.
const CHEVRON_LONG: f64 = 8.5;
const CHEVRON_SHORT: f64 = 5.0;
/// The chevron's centre, from the card's trailing and top edges.
const CHEVRON_INSET: f64 = 16.0;
/// The side of the square that takes the pointer for the chevron.
const CHEVRON_HIT: f64 = 24.0;
/// The action pill's size and its distance from the card's trailing and bottom edges.
pub const PILL_WIDTH: f64 = 63.5;
pub const PILL_HEIGHT: f64 = 22.0;
pub const PILL_INSET: f64 = 10.0;
/// The capsule a collapsed group's close circle grows into.
pub const CLEAR_ALL_WIDTH: f64 = 64.5;
pub const CLEAR_ALL_HEIGHT: f64 = 22.5;
/// The type size of the labels of the capsule and the pill.
const LABEL_SIZE: f64 = 12.0;
/// Room kept at either side of the pill's label.
const PILL_PADDING: f64 = 8.0;

/// What the control is, and so its shape and where it lies on the card.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    /// The close circle. A collapsed group's card gives it a `label`: it is the capsule that clears the group while
    /// `wide`.
    Close { label: Option<String>, wide: bool },
    /// The chevron that unfolds the card or expands its group (`open` while it is unfolded).
    Chevron { open: bool },
    /// The pill of the card's first action.
    Pill { label: String },
}

/// One control of a card, at `rect` in the card's frame.
pub(super) struct CardControl<M> {
    pub id: u32,
    pub kind: Kind,
    /// What assistive technology calls it.
    pub name: String,
    pub message: Input,
    pub map: fn(Input) -> M,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ControlState {
    armed: bool,
}

/// Where the close circle (or capsule) of a card lies in the card's frame.
pub fn close_frame(wide: bool) -> Rect {
    if wide {
        Rect::new(
            -CLOSE_REACH_X,
            -CLOSE_REACH_Y - (CLEAR_ALL_HEIGHT - CLOSE_DIAMETER) / 2.0,
            CLEAR_ALL_WIDTH,
            CLEAR_ALL_HEIGHT,
        )
    } else {
        Rect::new(
            -CLOSE_REACH_X,
            -CLOSE_REACH_Y,
            CLOSE_DIAMETER,
            CLOSE_DIAMETER,
        )
    }
}

impl<M> CardControl<M> {
    /// Where the control lies in a card `card` large.
    fn frame(&self, card: Size) -> Rect {
        match &self.kind {
            Kind::Close { wide, .. } => close_frame(*wide),
            Kind::Chevron { .. } => Rect::new(
                card.width - CHEVRON_INSET - CHEVRON_HIT / 2.0,
                CHEVRON_INSET - CHEVRON_HIT / 2.0,
                CHEVRON_HIT,
                CHEVRON_HIT,
            ),
            Kind::Pill { .. } => Rect::new(
                card.width - PILL_INSET - PILL_WIDTH,
                card.height - PILL_INSET - PILL_HEIGHT,
                PILL_WIDTH,
                PILL_HEIGHT,
            ),
        }
    }

    /// The element for a control in a card `card` large.
    pub fn element(self, card: Size) -> Element<M>
    where
        M: Clone + 'static,
    {
        let key = match self.kind {
            Kind::Close { .. } => "close",
            Kind::Chevron { .. } => "chevron",
            Kind::Pill { .. } => "pill",
        };
        let frame = self.frame(card);
        Element::new(Frame {
            frame,
            control: self,
        })
        .key(key)
    }
}

/// A control with the rect it was placed at, which the card's size decided.
struct Frame<M> {
    frame: Rect,
    control: CardControl<M>,
}

impl<M: Clone + 'static> Frame<M> {
    fn emit(&self, cx: &mut EventCx<'_, M>) {
        cx.emit((self.control.map)(self.control.message.clone()));
    }

    /// Tell the list the capsule is wanted (`true`) or not, when the control is one that grows.
    fn grow(&self, wanted: bool, cx: &mut EventCx<'_, M>) {
        if let Kind::Close {
            label: Some(_),
            wide,
        } = &self.control.kind
        {
            if *wide != wanted {
                let id = self.control.id;
                cx.emit((self.control.map)(Input::CloseWide(wanted.then_some(id))));
            }
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Frame<M> {
    type State = ControlState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.frame.width()),
            height: SizeSpec::Fixed(self.frame.height()),
            position: self.frame.origin,
            flex: Flex::NONE,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> ControlState {
        ControlState::default()
    }

    fn update(&self, old: &Self, _: &mut ControlState) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.control.kind != old.control.kind || self.control.name != old.control.name {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ControlState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut ControlState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            // the pointer over the circle asks for the capsule; the motion goes on to the card, which stays revealed
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _) => {
                self.grow(true, cx);
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                state.armed = false;
                self.grow(false, cx);
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                    position,
                    ..
                },
                _,
            ) => {
                if pressed {
                    state.armed = true;
                    cx.capture_pointer();
                } else if std::mem::take(&mut state.armed) {
                    cx.release_pointer();
                    if cx.hit(position) {
                        self.emit(cx);
                    }
                }
                Propagation::Stop
            }
            Event::Key(key, _) if key.pressed && cx.node() == cx.target() => match key.keysym {
                RETURN | KEYPAD_ENTER | SPACE => {
                    self.emit(cx);
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            Event::Focus { gained, visible } if cx.node() == cx.target() => {
                self.grow(gained && visible, cx);
                cx.request_paint();
                Propagation::Continue
            }
            Event::Cancel => {
                state.armed = false;
                Propagation::Continue
            }
            Event::Action(ref request) if request.action == Action::Click => {
                self.emit(cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &ControlState, cx: &mut PaintCx<'_>) {
        let (Some(fill), Some(mark)) = (
            color(cx, ColorRole::ControlFill),
            color(cx, ColorRole::SecondaryText),
        ) else {
            return;
        };
        let size = self.frame.size;
        let focus = cx.is_focused() && cx.focus_visible();
        let ring = |cx: &mut PaintCx<'_>, rect: Rect, radius: f64| {
            if !focus {
                return;
            }
            let theme = cx.env().theme.clone();
            if let Some(indicator) = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                let grow = theme.focus.width / 2.0 + 1.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(grow),
                        radius: radius + grow,
                    },
                    width: theme.focus.width,
                    paint: Paint::Solid(indicator),
                });
            }
        };
        let platter = |cx: &mut PaintCx<'_>, rect: Rect| {
            let shape = Shape::RoundedRect {
                rect,
                radius: rect.height() / 2.0,
            };
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(fill),
            });
            if let Some(edge) = color(cx, ColorRole::Separator) {
                cx.push(PaintOp::Stroke {
                    shape,
                    width: OUTLINE_WIDTH,
                    paint: Paint::Solid(edge),
                });
            }
        };
        let own = Rect::from_origin_size(Point::ZERO, size);
        match &self.control.kind {
            Kind::Close { label, wide } => {
                platter(cx, own);
                if let (true, Some(label)) = (*wide, label) {
                    text(cx, own, label, FontWeight::SEMIBOLD, ColorRole::Text);
                } else {
                    let (a, b) = (size.width * 0.32, size.width * 0.68);
                    let (c, d) = (size.height * 0.32, size.height * 0.68);
                    cx.push(PaintOp::Stroke {
                        shape: Shape::Path {
                            verbs: vec![
                                PathVerb::MoveTo(Point::new(a, c)),
                                PathVerb::LineTo(Point::new(b, d)),
                                PathVerb::MoveTo(Point::new(b, c)),
                                PathVerb::LineTo(Point::new(a, d)),
                            ],
                            fill_rule: Default::default(),
                        },
                        width: 1.5,
                        paint: Paint::Solid(mark),
                    });
                }
                ring(cx, own, own.height() / 2.0);
            }
            Kind::Pill { label } => {
                platter(cx, own);
                text(
                    cx,
                    own.inset(cupertino_kit::geometry::Insets {
                        left: PILL_PADDING,
                        right: PILL_PADDING,
                        top: 0.0,
                        bottom: 0.0,
                    }),
                    label,
                    FontWeight(500),
                    ColorRole::Text,
                );
                ring(cx, own, own.height() / 2.0);
            }
            Kind::Chevron { open } => {
                let (cx0, cy0) = (size.width / 2.0, size.height / 2.0);
                let (long, short) = (CHEVRON_LONG / 2.0, CHEVRON_SHORT / 2.0);
                let verbs = if *open {
                    vec![
                        PathVerb::MoveTo(Point::new(cx0 - long, cy0 - short)),
                        PathVerb::LineTo(Point::new(cx0, cy0 + short)),
                        PathVerb::LineTo(Point::new(cx0 + long, cy0 - short)),
                    ]
                } else {
                    vec![
                        PathVerb::MoveTo(Point::new(cx0 - short, cy0 - long)),
                        PathVerb::LineTo(Point::new(cx0 + short, cy0)),
                        PathVerb::LineTo(Point::new(cx0 - short, cy0 + long)),
                    ]
                };
                cx.push(PaintOp::Stroke {
                    shape: Shape::Path {
                        verbs,
                        fill_rule: Default::default(),
                    },
                    width: 1.5,
                    paint: Paint::Solid(mark),
                });
                ring(cx, own, 8.0);
            }
        }
    }

    fn semantics(&self, _: &ControlState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.control.name.clone());
        cx.focusable();
        cx.action(Action::Click);
    }
}

/// `label` centred in `rect`, one line, cut with an ellipsis where it does not fit.
fn text(cx: &mut PaintCx<'_>, rect: Rect, label: &str, weight: FontWeight, role: ColorRole) {
    let Some(ink) = color(cx, role) else {
        return;
    };
    let laid = Label {
        text: label.to_owned(),
        size: LABEL_SIZE,
        weight: weight.0,
        secondary: false,
        align: TextAlign::Center,
        rect: None,
        hidden: true,
        halo: false,
    };
    let Some(layout) = laid.laid_out(cx.env(), rect.width()) else {
        return;
    };
    let top = rect.y() + ((rect.height() - layout.size().height) / 2.0).round();
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: Point::new(rect.x(), top) + run.origin,
                run: run.run.clone(),
                color: ink,
            });
        }
    }
}
