//! The mode buttons beside the search field, and the shortcut chips the field shows while the pointer is over one.
//!
//! A button is a round piece of the panel's glass, joined to the field's glass (and to its neighbours) where they come
//! close, so while the group settles the buttons bud out of the field's end as one shape and pinch apart. Its symbol
//! sharpens after it buds. Hovering a button shows the shortcut that opens its scope in the field's trailing end;
//! pressing it, or the shortcut, switches the launcher to the scope.

use super::browse::{fill, focus_ring, label, line, placed, role, share};
use super::{glyphs, Input, EDGE_ALPHA_DARK, EDGE_ALPHA_LIGHT, EDGE_WIDTH};
use crate::modes::{self, Mode};
use cupertino_kit::controls::{Icon, IconDirection};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{MouseButton, PointerEvent};
use cupertino_kit::render::{Effect, Paint, PaintOp, Shape};
use cupertino_kit::theme::{Color, ColorRole, Surface};

/// The glass container the field's enclosure and the buttons share, and how far their outlines blend into each other,
/// in points: more than the gap between a settled field and its first button is not blended.
pub const JOIN_CONTAINER: u32 = 1;
pub const JOIN_SPACING: f64 = 8.0;
/// How far a button's symbol is blurred while it is not yet sharp, in points.
const GLYPH_BLUR: f64 = 6.0;
/// A chip's fill: the label colour at this share.
const CHIP_FILL_SHARE: f32 = 0.08;
/// The width of the chip that names the modifier.
const MODIFIER_CHIP_WIDTH: f64 = 40.0;

/// One mode button: a circle of glass at `rect` (in the window), its symbol in the middle.
pub struct ModeButton<M> {
    pub mode: Mode,
    pub index: usize,
    pub rect: Rect,
    /// The panel's opacity.
    pub alpha: f64,
    /// How sharp its symbol is, from 0 to 1.
    pub sharp: f64,
    /// Whether the keys are on it.
    pub focus: bool,
    /// Whether it takes input (the panel is open and the buttons are in place).
    pub active: bool,
    pub children: Vec<Element<M>>,
}

/// A button's children: its symbol, blurred while it sharpens.
pub fn glyph<M: 'static>(mode: Mode, sharp: f64) -> Element<M> {
    Element::new(GlyphBox {
        blur: (1.0 - sharp.clamp(0.0, 1.0)) * GLYPH_BLUR,
        children: vec![Element::new(Icon {
            name: mode.symbol().to_owned(),
            size: modes::GLYPH_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: Some(ColorRole::SecondaryText),
        })],
    })
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ModeButton<M> {
    /// Whether the pointer is over it.
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, ContainerKind::Absolute)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, _: &mut bool) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.mode, self.index, self.alpha, self.focus, self.active)
            != (old.mode, old.index, old.alpha, old.focus, old.active)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn hit(&self, _: &bool, point: Point) -> bool {
        let r = self.rect.width() / 2.0;
        let (dx, dy) = (point.x - r, point.y - self.rect.height() / 2.0);
        self.active && dx * dx + dy * dy <= r * r
    }

    fn event(&self, hovered: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _)
                if !*hovered =>
            {
                *hovered = true;
                cx.emit(M::from(Input::ModeHover(Some(self.index))));
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if *hovered => {
                *hovered = false;
                cx.emit(M::from(Input::ModeHover(None)));
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::ModePress(self.index)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(M::from(Input::ModePress(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &bool, cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = size.width / 2.0;
        let theme = cx.env().theme.clone();
        let dark = theme.preferences.appearance.is_dark();
        let glass = cx.glass(
            Glass::large_panel()
                .capsule()
                .alpha(alpha)
                .container(JOIN_CONTAINER, JOIN_SPACING),
        );
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        let shape = Shape::RoundedRect { rect, radius };
        if !glass {
            if let Some(fill) = theme.surface(Surface::Menu).fill {
                cx.push(PaintOp::Fill {
                    shape: shape.clone(),
                    paint: Paint::Solid(fill),
                });
            }
        }
        let edge = if dark {
            EDGE_ALPHA_DARK
        } else {
            EDGE_ALPHA_LIGHT
        };
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: rect.outset(EDGE_WIDTH / 2.0),
                radius: radius + EDGE_WIDTH / 2.0,
            },
            width: EDGE_WIDTH,
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, edge)),
        });
        if self.focus {
            focus_ring(cx, rect, radius);
        }
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.mode.name().to_owned());
        cx.description(format!(
            "{}+{}",
            modes::MODIFIER_NAME,
            modes::digit(self.index)
        ));
        cx.states(AccessibleStates {
            selected: self.focus,
            ..AccessibleStates::default()
        });
        if self.active {
            cx.action(Action::Click);
        } else {
            cx.hidden();
        }
    }
}

/// A button's symbol, centred, blurred by `blur` points while it sharpens.
struct GlyphBox<M> {
    blur: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for GlyphBox<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: cupertino_kit::layout::Axis::Vertical,
                spacing: 0.0,
                align: cupertino_kit::layout::Align::Center,
                justify: cupertino_kit::layout::Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.blur != old.blur {
            Change::Paint
        } else {
            Change::None
        }
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.blur > 0.05 {
            cx.set_group(1.0, vec![Effect::Blur { radius: self.blur }]);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The two chips of a shortcut in the field's trailing end: the modifier and the button's digit. They draw only; the
/// button they belong to names the shortcut to assistive technology.
pub struct Chips {
    pub digit: char,
}

/// Chips' width: the modifier's, the gap and the digit's.
pub const CHIPS_WIDTH: f64 = MODIFIER_CHIP_WIDTH + modes::CHIP_GAP + modes::CHIP_DIGIT_WIDTH;

impl<M: 'static> Widget<M> for Chips {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(CHIPS_WIDTH),
            height: SizeSpec::Fixed(modes::CHIP_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.digit != old.digit {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(CHIPS_WIDTH, modes::CHIP_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let text = label(cx);
        let ink = role(cx, ColorRole::SecondaryText, share(text, 0.6));
        let chips = [
            (MODIFIER_CHIP_WIDTH, modes::MODIFIER_NAME.to_owned()),
            (modes::CHIP_DIGIT_WIDTH, self.digit.to_string()),
        ];
        let mut x = 0.0;
        for (width, name) in chips {
            let rect = Rect::new(x, 0.0, width, modes::CHIP_HEIGHT);
            fill(cx, rect, modes::CHIP_RADIUS, share(text, CHIP_FILL_SHARE));
            if let Some(l) = line(cx, &name, modes::CHIP_TEXT_SIZE, 500, width) {
                let (w, h) = (l.size().width, l.size().height);
                glyphs(
                    cx,
                    &l,
                    Point::new(x + (width - w) / 2.0, (modes::CHIP_HEIGHT - h) / 2.0),
                    ink,
                );
            }
            x += width + modes::CHIP_GAP;
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}
