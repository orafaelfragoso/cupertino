//! A window's drag region: a left press there that no control inside took moves the window once the pointer travels
//! past the drag start distance with the button held, and a second press soon after the first zooms the window
//! (maximizes it, or restores it). Controls inside keep their presses, since they stop them on the way up; a disabled
//! control or a label lets the press through to the region. The region has no role and no Tab stop of its own: the
//! keyboard moves and zooms windows with the compositor's shortcuts and the menu bar's Window menu.

use crate::WindowDragArea;
use cupertino_geometry::{Point, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{MouseButton, PointerEvent, Timestamp};
use cupertino_ui_text::MULTI_CLICK_INTERVAL;

/// How far, in points along either axis, the pointer travels from the press with the button held before the window
/// starts to move; a shorter wobble stays a click.
pub(crate) const DRAG_START: f64 = 5.0;
/// The longest time between two presses on the region, in nanoseconds, that makes them a double click: the toolkit's
/// one double click interval, which text selection also uses.
pub(crate) const DOUBLE_CLICK: u64 = MULTI_CLICK_INTERVAL.as_nanos() as u64;

/// The press being held and the last press's time.
#[derive(Debug, Default)]
pub(crate) struct DragState {
    /// Where the held press went down, until the window starts to move or the button is released.
    held: Option<Point>,
    /// When the last single press went down, for the double click.
    last_press: Option<Timestamp>,
}

impl DragState {
    /// Feeds one event that reached the region; presses that a control below took never arrive.
    pub(crate) fn event<M>(&mut self, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    position,
                    ..
                },
                time,
            ) => {
                let double = self
                    .last_press
                    .is_some_and(|last| time.0.saturating_sub(last.0) <= DOUBLE_CLICK);
                if double {
                    self.last_press = None;
                    self.held = None;
                    cx.zoom_window();
                } else {
                    self.last_press = Some(time);
                    self.held = Some(position);
                    cx.capture_pointer();
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => match self.held {
                Some(press)
                    if (position.x - press.x).abs() > DRAG_START
                        || (position.y - press.y).abs() > DRAG_START =>
                {
                    self.held = None;
                    // a move is no click: the next press starts over
                    self.last_press = None;
                    cx.move_window();
                    Propagation::Stop
                }
                Some(_) => Propagation::Stop,
                None => Propagation::Continue,
            },
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if cx.has_pointer_capture() => {
                self.held = None;
                cx.release_pointer();
                Propagation::Stop
            }
            Event::Cancel => {
                self.held = None;
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }
}

impl<M: 'static> From<WindowDragArea<M>> for Element<M> {
    fn from(area: WindowDragArea<M>) -> Self {
        Element::new(Area {
            children: vec![area.content],
        })
    }
}

/// The region around its content, the size of the content.
struct Area<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Area<M> {
    type State = DragState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> DragState {
        DragState::default()
    }

    fn update(&self, _: &Self, _: &mut DragState) -> Change {
        Change::None
    }

    fn measure(&self, _: &DragState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, state: &mut DragState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        state.event(event, cx)
    }

    fn paint(&self, _: &DragState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &DragState, _: &mut SemanticsCx<'_>) {}
}
