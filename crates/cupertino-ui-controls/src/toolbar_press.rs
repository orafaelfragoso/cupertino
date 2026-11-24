//! How a toolbar control answers a press: its glass grows a little about its centre while the press is held and goes
//! back when it lets go, and the pressed segment fills with the glass's interaction fill.
//!
//! The growth is a capsule's ([`CapsulePress`]): the capsule hears every press of its segments on the way down to them
//! (the capture phase), so one press of any enabled segment grows the whole capsule, its glass and the toolkit's outline
//! over it, as the system grows the whole platter. The glass widens by [`TOOLBAR_PRESS_GROWTH`] whatever its width, its
//! height in proportion, along [`SpringPreset::TOOLBAR_ITEM_PRESS`], and goes back along
//! [`SpringPreset::TOOLBAR_ITEM_RELEASE`]. A pressed segment takes the pointer, whose later events then go to it alone,
//! so the capsule learns of the release from the segment through the [`PressLink`] they share, asking every
//! [`POINTER_POLL`] while the press lasts. A drag off the control keeps it grown until the release. A segment that opens
//! a menu acts on the press and its menu takes the release, so its glass lets go by itself
//! [`TOOLBAR_MENU_PRESS_HOLD`] after the press. Under Reduce Motion, and where the window system does not scale glass,
//! nothing grows. What the capsule's segments draw (glyphs, the pills) keeps its place.

use std::cell::Cell;
use std::rc::Rc;

use cupertino_theme::animation::SpringPreset;
use cupertino_ui_core::platform::{Keysym, MouseButton, PointerEvent, Timestamp, TouchEvent};
use cupertino_ui_core::{Event, EventCx, PaintCx, Phase};

use crate::motion::{Curve, SpringTween, Tween};

/// How much wider a toolbar control's glass is drawn under a press, in points, about its centre; its height grows in
/// proportion, so the glass keeps its shape. A Finder recording (`video/frames/files-windows`) shows a 47-point menu
/// button, a 110-point group and a 160-point choice each widening by 2.2 to 2.5 points once settled (2.7 to 2.8 at the
/// widest), the 47-point one 2.1 points taller and the 160-point one about 1 point taller: the same growth at every
/// width, not the same share.
pub const TOOLBAR_PRESS_GROWTH: f64 = 2.5;
/// How long a press on a segment that opens a menu keeps the glass grown, in seconds: the menu takes the release, so the
/// glass lets go on its own. The recording's menu buttons (379.71 s, 389.87 s) let go 0.2 s after their press.
pub const TOOLBAR_MENU_PRESS_HOLD: f64 = 0.2;
/// How often a capsule grown by a pointer press asks its segment whether the press still holds, in seconds: one frame
/// at 60 Hz, so the glass starts back at most a frame after the release.
const POINTER_POLL: f64 = 1.0 / 60.0;
/// How long the pressed fill takes to come on, how long it waits after the release before it goes, and how long it then
/// takes to go, in seconds: a toolbar image button's press settles in about 46 ms, its release starts about 50 ms late
/// and is done about 79 ms after it.
const FILL_IN: f64 = 0.046;
const FILL_OUT_DELAY: f64 = 0.050;
const FILL_OUT: f64 = 0.029;

const SPACE: Keysym = Keysym(0x20);
const ESCAPE: Keysym = Keysym(0xff1b);

/// What a capsule and its segments share, built with them: whether one of the segments is armed by the pointer, which
/// the segment says and the capsule asks.
#[derive(Clone, Debug, Default)]
pub(crate) struct PressLink(Rc<Cell<bool>>);

impl PressLink {
    /// Says whether the segment is armed by the pointer.
    pub(crate) fn set(&self, armed: bool) {
        self.0.set(armed);
    }

    fn get(&self) -> bool {
        self.0.get()
    }
}

/// Two links are equal when they are the same one.
impl PartialEq for PressLink {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// What holds a capsule's glass grown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holder {
    /// The pointer, which the pressed segment took: the capsule asks the link.
    Pointer,
    Touch(i32),
    Space,
    /// A press that opened a menu, let go at the wake asked for it.
    Menu,
}

/// The node state of a toolbar capsule: what holds its glass grown, how far it has grown, from 0 at rest to 1 at
/// [`TOOLBAR_PRESS_GROWTH`], and the link to its segments of the last build. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CapsulePress {
    held: Option<Holder>,
    grow: SpringTween,
    link: PressLink,
}

/// What a capsule knows of the segment a press lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Landing {
    pub(crate) enabled: bool,
    pub(crate) menu: bool,
}

impl CapsulePress {
    /// At rest, asking `link` while a pointer press holds it.
    pub(crate) fn new(link: &PressLink) -> CapsulePress {
        CapsulePress {
            link: link.clone(),
            ..CapsulePress::default()
        }
    }

    /// Follows a rebuild: the capsule's segments now say what they hold through `link`.
    pub(crate) fn adopt(&mut self, link: &PressLink) {
        self.link = link.clone();
    }

    /// Feeds one event on its way down to a segment (`segment` says which a position in window coordinates lands on);
    /// it never stops the event. Returns whether the look changed.
    pub(crate) fn event<M>(
        &mut self,
        event: &Event,
        segment: impl Fn(cupertino_geometry::Point) -> Option<Landing>,
        cx: &mut EventCx<'_, M>,
    ) -> bool {
        match *event {
            Event::Wake(_) if self.held == Some(Holder::Menu) => self.release(),
            Event::Wake(now) if self.held == Some(Holder::Pointer) => {
                if self.link.get() {
                    cx.wake_at(after(now, POINTER_POLL));
                    false
                } else {
                    self.release()
                }
            }
            // a cancel or the focus leaving lets go of any hold but a menu's, however it reaches the capsule
            Event::Touch(TouchEvent::Cancel, _)
            | Event::Cancel
            | Event::FocusWithin { gained: false, .. } => {
                matches!(self.held, Some(h) if h != Holder::Menu) && self.release()
            }
            _ if cx.phase() != Phase::Capture => false,
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                at,
            ) if self.held.is_none() => match segment(position) {
                Some(Landing {
                    enabled: true,
                    menu,
                }) => {
                    let (holder, wait) = if menu {
                        (Holder::Menu, TOOLBAR_MENU_PRESS_HOLD)
                    } else {
                        (Holder::Pointer, POINTER_POLL)
                    };
                    cx.wake_at(after(cx.input_time(at), wait));
                    self.hold(holder)
                }
                _ => false,
            },
            Event::Touch(TouchEvent::Down { id, position }, _) if self.held.is_none() => {
                match segment(position) {
                    Some(Landing {
                        enabled: true,
                        menu: false,
                    }) => self.hold(Holder::Touch(id)),
                    _ => false,
                }
            }
            Event::Touch(TouchEvent::Up { id }, _) if self.held == Some(Holder::Touch(id)) => {
                self.release()
            }
            Event::Key(key, _) if key.keysym == SPACE && cx.target() != cx.node() => {
                // only an enabled segment takes the focus
                if key.pressed && !key.repeat && self.held.is_none() {
                    self.hold(Holder::Space)
                } else if !key.pressed && self.held == Some(Holder::Space) {
                    self.release()
                } else {
                    false
                }
            }
            Event::Key(key, _) if key.keysym == ESCAPE && key.pressed => {
                self.held == Some(Holder::Space) && self.release()
            }
            _ => false,
        }
    }

    fn hold(&mut self, holder: Holder) -> bool {
        self.held = Some(holder);
        self.grow.go(1.0, SpringPreset::TOOLBAR_ITEM_PRESS);
        true
    }

    fn release(&mut self) -> bool {
        self.held = None;
        self.grow.go(0.0, SpringPreset::TOOLBAR_ITEM_RELEASE);
        true
    }

    /// How many times its size a capsule `width` points wide draws its glass at the paint's time: grown by
    /// [`TOOLBAR_PRESS_GROWTH`] while pressed; 1 under Reduce Motion and where the window system does not scale glass.
    pub(crate) fn scale(&self, width: f64, cx: &mut PaintCx<'_>) -> f64 {
        if cx.env().theme.motion.reduce || !cx.glass_scale() || width <= 0.0 {
            return 1.0;
        }
        1.0 + TOOLBAR_PRESS_GROWTH / width * self.grow.sample(cx)
    }
}

/// `seconds` after `at`.
fn after(at: Timestamp, seconds: f64) -> Timestamp {
    Timestamp(at.0 + (seconds * 1e9) as u64)
}

/// The node state of a toolbar segment: how it is armed, and how far its pressed fill has come on, from 0 to 1. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SegmentPress {
    pub(crate) press: crate::press::Press,
    fill: Tween,
}

impl SegmentPress {
    /// The pressed fill follows the press: on over [`FILL_IN`] as it is pressed, and once it is not (let go, or the
    /// pointer dragged off it), gone over [`FILL_OUT`] after [`FILL_OUT_DELAY`]. Tells `link` whether the pointer arms it.
    pub(crate) fn follow(&mut self, was_pressed: bool, link: &PressLink) {
        link.set(self.press.pointer_armed());
        match (was_pressed, self.press.pressed()) {
            (false, true) => self.fill.go(1.0, FILL_IN, 0.0, Curve::EaseOut),
            (true, false) => self.fill.go(0.0, FILL_OUT, FILL_OUT_DELAY, Curve::EaseOut),
            _ => {}
        }
    }

    /// How far the pressed fill is on at the paint's time.
    pub(crate) fn fill(&self, cx: &mut PaintCx<'_>) -> f64 {
        self.fill.sample(cx)
    }
}
