//! Arming and activation shared by buttons and switches. A pointer or touch press arms the control, release over it
//! activates once and release elsewhere disarms it; Space arms and activates on release, Return activates on press.
//! A cancel (focus, capture or surface lost) disarms without activating. Touch has no hover.

use cupertino_ui_core::{Action, ActivationSource, Event, EventCx, Phase};
use cupertino_ui_platform::{Keysym, MouseButton, PointerEvent, TouchEvent};

const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Press {
    hovered: bool,
    pointer: bool,
    touch: Option<i32>,
    key: bool,
    /// The armed pointer or touch is over the control.
    inside: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ignored,
    /// The look changed; repaint.
    Changed,
    Activated(ActivationSource),
}

impl Press {
    pub(crate) fn hovered(&self) -> bool {
        self.hovered
    }

    /// Armed and, for a pointer or touch, still over the control.
    pub(crate) fn pressed(&self) -> bool {
        self.key || ((self.pointer || self.touch.is_some()) && self.inside)
    }

    fn armed(&self) -> bool {
        self.pointer || self.touch.is_some() || self.key
    }

    fn disarm(&mut self) -> Outcome {
        let was = self.armed();
        self.pointer = false;
        self.touch = None;
        self.key = false;
        self.inside = false;
        if was {
            Outcome::Changed
        } else {
            Outcome::Ignored
        }
    }

    /// Feeds one event; a disabled control tracks hover and disarms but never arms or activates.
    pub(crate) fn event<M>(
        &mut self,
        event: &Event,
        enabled: bool,
        cx: &mut EventCx<'_, M>,
    ) -> Outcome {
        if cx.phase() == Phase::Capture {
            return Outcome::Ignored;
        }
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                self.hovered = true;
                if self.pointer {
                    self.inside = true;
                }
                Outcome::Changed
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                self.hovered = false;
                self.inside = false;
                Outcome::Changed
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if enabled && !self.armed() => {
                self.pointer = true;
                self.inside = true;
                cx.capture_pointer();
                Outcome::Changed
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if self.pointer => {
                let inside = cx.hit(position);
                if inside == self.inside {
                    return Outcome::Ignored;
                }
                self.inside = inside;
                Outcome::Changed
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if self.pointer => {
                cx.release_pointer();
                let inside = cx.hit(position);
                self.disarm();
                if enabled && inside {
                    Outcome::Activated(ActivationSource::Pointer)
                } else {
                    Outcome::Changed
                }
            }
            Event::Touch(TouchEvent::Down { id, .. }, _) if enabled && !self.armed() => {
                self.touch = Some(id);
                self.inside = true;
                Outcome::Changed
            }
            Event::Touch(TouchEvent::Motion { id, position }, _) if self.touch == Some(id) => {
                let inside = cx.hit(position);
                if inside == self.inside {
                    return Outcome::Ignored;
                }
                self.inside = inside;
                Outcome::Changed
            }
            Event::Touch(TouchEvent::Up { id }, _) if self.touch == Some(id) => {
                let inside = self.inside;
                self.disarm();
                if enabled && inside {
                    Outcome::Activated(ActivationSource::Touch)
                } else {
                    Outcome::Changed
                }
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => self.disarm(),
            Event::Focus { gained: false, .. } if self.key => self.disarm(),
            Event::Key(key, _) if cx.node() == cx.target() => match key.keysym {
                SPACE if key.pressed && !key.repeat && enabled && !self.armed() => {
                    self.key = true;
                    Outcome::Changed
                }
                SPACE if !key.pressed && self.key => {
                    self.disarm();
                    if enabled {
                        Outcome::Activated(ActivationSource::Keyboard)
                    } else {
                        Outcome::Changed
                    }
                }
                RETURN | KP_ENTER if key.pressed && !key.repeat && enabled && !self.armed() => {
                    Outcome::Activated(ActivationSource::Keyboard)
                }
                ESCAPE if key.pressed && self.key => self.disarm(),
                _ => Outcome::Ignored,
            },
            Event::Action(ref request) if request.action == Action::Click && enabled => {
                Outcome::Activated(ActivationSource::Accessibility)
            }
            _ => Outcome::Ignored,
        }
    }
}
