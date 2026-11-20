//! The roots of the windows' content: a banner's strip, which follows the pointer for hover and the swipe, and the
//! centre's root, which closes the centre.

use cupertino_kit::core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{MouseButton, PointerEvent};

use super::{close_rect, no_measure, Input, CENTRE_TITLE};

/// The key of the centre's root, which takes the focus as the centre opens.
const CENTRE_ROOT: &str = "centre-root";

/// The key of the centre's root: the focus moves there as the centre opens ([`Command::MoveFocus`]), so the list
/// hears the keyboard leave its window.
///
/// [`Command::MoveFocus`]: cupertino_kit::core::Command::MoveFocus
pub fn centre_root_key() -> Key {
    Key::from(CENTRE_ROOT)
}

/// A banner's window content: the card and the pointer handling of the banner, hover and swipe.
pub(super) struct Strip<M> {
    pub(super) id: u32,
    /// The card's resting rect at this frame, window coordinates.
    pub(super) card: Rect,
    pub(super) map: fn(Input) -> M,
    pub(super) children: Vec<Element<M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct StripState {
    /// The x of the press that may become a swipe.
    press: Option<f64>,
    hovered: bool,
}

impl<M: 'static> Strip<M> {
    fn hover(&self, state: &mut StripState, over: bool, cx: &mut EventCx<'_, M>) {
        if over != state.hovered && state.press.is_none() {
            state.hovered = over;
            cx.emit((self.map)(Input::Hover(over.then_some(self.id))));
        }
    }

    /// Whether `position` is over the card or its close button.
    fn over(&self, position: Point) -> bool {
        self.card.contains(position) || close_rect(self.card).contains(position)
    }
}

impl<M: 'static> Widget<M> for Strip<M> {
    type State = StripState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> StripState {
        StripState::default()
    }

    fn update(&self, old: &Self, _: &mut StripState) -> Change {
        if self.card != old.card {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &StripState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, state: &mut StripState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match *event {
            // hover follows the pointer before any child sees it
            Event::Pointer(
                PointerEvent::Enter { position } | PointerEvent::Motion { position },
                _,
            ) if cx.phase() == Phase::Capture || cx.node() == cx.target() => {
                if let Some(start) = state.press {
                    cx.emit((self.map)(Input::SwipeMove(self.id, position.x - start)));
                    return Propagation::Stop;
                }
                self.hover(state, self.over(position), cx);
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                self.hover(state, false, cx);
                Propagation::Continue
            }
            // a press no child took: on the card it begins a swipe or a click
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture && self.card.contains(position) => {
                state.press = Some(position.x);
                cx.capture_pointer();
                cx.emit((self.map)(Input::SwipeBegin(self.id)));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.press.is_some() => {
                state.press = None;
                cx.release_pointer();
                cx.emit((self.map)(Input::SwipeEnd(self.id)));
                self.hover(state, self.over(position), cx);
                Propagation::Stop
            }
            Event::Cancel if state.press.is_some() => {
                state.press = None;
                cx.emit((self.map)(Input::SwipeCancel(self.id)));
                self.hover(state, false, cx);
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &StripState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &StripState, _: &mut SemanticsCx<'_>) {}
}

/// The centre's window content: a press no card, button or menu took, and Escape close the Clear menu, then the
/// centre. It holds the focus while the list is open ([`centre_root_key`]), so the list closes when another surface
/// takes the keyboard from it: another shell panel opened, as a rule.
pub(super) struct CentreRoot<M> {
    pub(super) map: fn(Input) -> M,
    pub(super) menu_open: bool,
    /// A card is revealed: the pointer moving over no card, or leaving the window, hides its buttons again.
    pub(super) revealed: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for CentreRoot<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.menu_open != old.menu_open || self.revealed != old.revealed {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                cx.emit((self.map)(Input::CloseCentre));
                Propagation::Stop
            }
            Event::Dismiss => {
                cx.emit((self.map)(Input::CloseCentre));
                Propagation::Stop
            }
            // the window lost the keyboard: the focus on the root heard it, or the focus on a card within it
            Event::WindowActive(false) => {
                cx.emit((self.map)(Input::PressOutside));
                Propagation::Continue
            }
            Event::FocusWithin { gained: false, .. } if !cx.window_active() => {
                cx.emit((self.map)(Input::PressOutside));
                Propagation::Continue
            }
            // a card stops the motions over it: this one is over none
            Event::Pointer(PointerEvent::Motion { .. } | PointerEvent::Leave, _)
                if self.revealed && cx.phase() != Phase::Capture =>
            {
                cx.emit((self.map)(Input::Reveal(None)));
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(CENTRE_TITLE);
        cx.dismissible();
    }
}
