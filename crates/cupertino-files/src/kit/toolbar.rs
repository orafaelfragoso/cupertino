//! The path bar's folders ([`PathSegment`]): each its icon and name, which a click, Space, Return or an assistive Click
//! goes to. Each folder is a Tab stop, after the folder's items, and the bar reads as a group named "Path"
//! ([`PathBar`]).

use cupertino_kit::controls::{Icon, IconDirection, Text, TextRole};
use cupertino_kit::core::{
    Action, Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::Message;

/// The corner radius of the fill a hovered or pressed button shows.
const RADIUS: f64 = 5.0;
/// The width of the keyboard focus ring.
const FOCUS_RING: f64 = 2.0;
/// A path bar folder's icon and name, and the gap between them, in points.
const PATH_ICON: f64 = 16.0;
const PATH_TEXT: f32 = 11.0;
const PATH_GAP: f64 = 4.0;
/// How far a path bar folder's focus ring lies outside its icon and name, so it covers neither.
const PATH_RING_OUTSET: f64 = 3.0;
const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);

fn tinted(symbol: &str, size: f64, tint: Option<ColorRole>) -> Element<Message> {
    Element::new(Icon {
        name: symbol.into(),
        size,
        direction: IconDirection::Fixed,
        label: None,
        tint,
    })
}

/// The keyboard focus ring, its outer edge `outset` points outside the node's bounds, in the theme's focus indicator
/// colour.
fn focus_ring(cx: &mut PaintCx<'_>, outset: f64) {
    let size = cx.size();
    let ring = cx
        .env()
        .theme
        .colors
        .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
    if let Some(ring) = ring {
        // the stroke is centred on its path, half of it on each side
        let edge = FOCUS_RING / 2.0 - outset;
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: Rect::new(
                    edge,
                    edge,
                    size.width - 2.0 * edge,
                    size.height - 2.0 * edge,
                ),
                radius: RADIUS - edge,
            },
            paint: Paint::Solid(ring),
            width: FOCUS_RING,
        });
    }
}

/// A folder of the path bar: its icon and its name. The folder shown reads in the label colour, the ones above it in
/// the secondary one; a place shown with its symbol in the accent colour (Recents) keeps that tint.
pub(super) struct PathSegment {
    pub name: String,
    pub symbol: String,
    pub accent: bool,
    /// The folder shown, the last segment.
    pub current: bool,
    pub on_press: Message,
}

impl Widget<Message> for PathSegment {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: PATH_GAP,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        let tint = self.accent.then_some(ColorRole::DefaultButtonFill);
        vec![
            tinted(&self.symbol, PATH_ICON, tint),
            Element::new(Text {
                content: AttributedText::plain(self.name.clone(), TextStyle::new(PATH_TEXT)),
                role: TextRole::Caption,
                wrap: false,
                secondary: !self.current,
                selected: false,
            }),
        ]
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.name, &self.symbol, self.current) != (&old.name, &old.symbol, old.current) {
            Change::Layout
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed && matches!(key.keysym, SPACE | RETURN | KP_ENTER) =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if cx.is_focused() && cx.focus_visible() {
            focus_ring(cx, PATH_RING_OUTSET);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.action(Action::Click);
        cx.focusable();
    }
}

/// What assistive technology calls the path bar.
const PATH_BAR_LABEL: &str = "Path";

/// The path bar's folders, read as one group.
pub(super) struct PathBar {
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for PathBar {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(PATH_BAR_LABEL);
    }
}
