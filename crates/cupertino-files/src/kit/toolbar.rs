//! The toolbar's pop-up buttons: View, Tags and Action, each a symbol with a small chevron that opens its menu below
//! it ([`super::Popup`]). A press, Space, Return or an assistive Click opens the menu; the button reads as a pop-up
//! button named for what it does.
//!
//! The path bar's folders ([`PathSegment`]): each its icon and name, which a click or an assistive Click goes to.

use cupertino_kit::controls::{Icon, IconDirection, Text, TextRole};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{ColorRole, InteractionVariant, StateVariant};

use super::Message;

/// The side of a toolbar button's symbol, and of the chevron after it, in points.
const SYMBOL: f64 = 16.0;
const CHEVRON: f64 = 9.0;
/// Between the symbol and the chevron.
const CHEVRON_GAP: f64 = 3.0;
/// The corner radius of the fill a hovered or pressed button shows.
const RADIUS: f64 = 5.0;
/// The width of the keyboard focus ring.
const FOCUS_RING: f64 = 2.0;
/// A path bar folder's icon and name, and the gap between them, in points.
const PATH_ICON: f64 = 16.0;
const PATH_TEXT: f32 = 11.0;
const PATH_GAP: f64 = 4.0;
const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);

/// A toolbar button that opens a menu.
pub(super) struct ToolButton {
    /// What assistive technology calls it.
    pub label: String,
    pub symbol: String,
    /// The chevron that says it opens a menu: down, or up and down for a choice.
    pub chevron: &'static str,
    pub size: Size,
    pub on_press: Message,
}

/// The pointer over the button, and the press held on it.
#[derive(Default)]
pub(super) struct ToolState {
    hovered: bool,
    pressed: bool,
}

fn icon(symbol: &str, size: f64) -> Element<Message> {
    tinted(symbol, size, None)
}

fn tinted(symbol: &str, size: f64, tint: Option<ColorRole>) -> Element<Message> {
    Element::new(Icon {
        name: symbol.into(),
        size,
        direction: IconDirection::Fixed,
        label: None,
        tint,
    })
}

impl Widget<Message> for ToolButton {
    type State = ToolState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CHEVRON_GAP,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        vec![icon(&self.symbol, SYMBOL), icon(self.chevron, CHEVRON)]
    }

    fn state(&self) -> ToolState {
        ToolState::default()
    }

    fn update(&self, old: &Self, _: &mut ToolState) -> Change {
        if self.size != old.size {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ToolState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut ToolState,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Enter { .. }, _) => {
                state.hovered = true;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                state.hovered = false;
                state.pressed = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                    ..
                },
                _,
            ) => {
                // the menu opens on the press, as a pop-up button's does
                state.pressed = *pressed;
                if *pressed {
                    cx.emit(self.on_press.clone());
                }
                cx.request_paint();
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
            Event::Cancel => {
                state.pressed = false;
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &ToolState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let theme = cx.env().theme.clone();
        let colors = &theme.colors;
        if state.hovered || state.pressed {
            let variant = StateVariant {
                interaction: if state.pressed {
                    InteractionVariant::Pressed
                } else {
                    InteractionVariant::Hover
                },
                ..StateVariant::NORMAL
            };
            if let Some(fill) = colors.get(ColorRole::ControlFill, variant) {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: area,
                        radius: RADIUS,
                    },
                    paint: Paint::Solid(fill),
                });
            }
        }
        if cx.is_focused() && cx.focus_visible() {
            let ring = colors.get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
            if let Some(ring) = ring {
                let half = FOCUS_RING / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(
                            half,
                            half,
                            size.width - FOCUS_RING,
                            size.height - FOCUS_RING,
                        ),
                        radius: RADIUS - half,
                    },
                    paint: Paint::Solid(ring),
                    width: FOCUS_RING,
                });
            }
        }
    }

    fn semantics(&self, _: &ToolState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.states(AccessibleStates::default());
        cx.action(Action::Click);
        cx.focusable();
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
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.action(Action::Click);
    }
}
