//! The menu of a text field: Cut, Copy, Paste and Select All, opened by a secondary click, the Menu key, Shift+F10 or
//! a finger that rests on the field. The field opens it for itself ([`EventCx::open_popup`]), so every field has it
//! without its application declaring anything; it is a menu as any other, built from the same rows and panel.

use crate::menu::{self, Levels, Out};
use crate::{MenuAnchor, MenuItem, MenuStyle};
use cupertino_geometry::Point;
use cupertino_menu_model::tracking::TRACKING_INTERVAL;
use cupertino_ui_core::platform::{Keysym, Mods, MouseButton, PointerEvent, Timestamp, TouchEvent};
use cupertino_ui_core::{Event, EventCx, Key, OwnedWindow, WindowKey};

/// The key of the menu's window, unlike any an application gives its own.
const WINDOW: &str = "cupertino.text-menu";

/// What assistive technology calls the menu.
const LABEL: &str = "Edit";

const MENU: Keysym = Keysym(0xff67);
const F10: Keysym = Keysym(0xffc7);

/// How long a finger rests on a field before its menu opens there, in seconds, as on any region with a context menu.
const TOUCH_HOLD: f64 = TRACKING_INTERVAL;

/// How far, in points, a resting finger may stray before it is no longer a hold.
const TOUCH_SLOP: f64 = 8.0;

/// What a row of the menu does to its field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The rows from the top; a separator stands before Select All.
const ROWS: [(Command, &str); 4] = [
    (Command::Cut, "Cut"),
    (Command::Copy, "Copy"),
    (Command::Paste, "Paste"),
    (Command::SelectAll, "Select All"),
];

/// Which rows can act on the field as it is: the others show disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Can {
    pub(crate) cut: bool,
    pub(crate) copy: bool,
    pub(crate) paste: bool,
    pub(crate) select_all: bool,
}

impl Can {
    fn act(self, command: Command) -> bool {
        match command {
            Command::Cut => self.cut,
            Command::Copy => self.copy,
            Command::Paste => self.paste,
            Command::SelectAll => self.select_all,
        }
    }
}

/// The row the menu answered its field with ([`Event::PopupAnswer`]).
pub(crate) fn command(answer: u32) -> Option<Command> {
    ROWS.get(answer as usize).map(|row| row.0)
}

/// Opens the menu at `at`, in the coordinates of the field's window.
pub(crate) fn open<M: Clone + 'static>(at: Point, can: Can, cx: &mut EventCx<'_, M>) {
    cx.open_popup(Box::new(move || windows(at, can)));
}

fn windows<M: Clone + 'static>(at: Point, can: Can) -> Vec<OwnedWindow<M>> {
    let mut items = Vec::new();
    for (index, (command, label)) in ROWS.iter().enumerate() {
        if *command == Command::SelectAll {
            items.push(MenuItem::Separator);
        }
        items.push(MenuItem::Item {
            label: (*label).to_owned(),
            enabled: can.act(*command),
            checked: false,
            on_select: Out::Answer(index as u32),
        });
    }
    let key = WindowKey(Key::from(WINDOW));
    menu::levels(
        Levels {
            // the runtime opens it in the field's window
            parent: key.clone(),
            anchor: MenuAnchor::At(at),
            label: LABEL.to_owned(),
            items,
            current: None,
            open: Vec::new(),
            on_open: &|_| Out::Nothing,
            on_close: Out::Close,
        },
        key,
        MenuStyle::default(),
    )
    .into_iter()
    .map(|level| OwnedWindow {
        key: level.key,
        spec: level.spec,
        background: level.background,
        content: level.content,
        anchor: level.anchor,
        grab: true,
    })
    .collect()
}

/// How the menu was asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Opens {
    /// By the pointer or a finger, at this point of the window.
    At(Point),
    /// From the keyboard: it opens at the caret.
    Keyboard,
}

/// Watches a field's events for what opens its menu. It holds the finger resting on the field, if one is: where it
/// went down and when its rest opens the menu.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Opener {
    hold: Option<(i32, Point, Timestamp)>,
}

impl Opener {
    /// Whether `event` opens the menu, and how.
    pub(crate) fn opens<M>(&mut self, event: &Event, cx: &mut EventCx<'_, M>) -> Option<Opens> {
        if cx.node() != cx.target() {
            return None;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => Some(Opens::At(*position)),
            Event::Key(key, _)
                if key.pressed
                    && (key.keysym == MENU && key.mods.is_empty()
                        || key.keysym == F10 && key.mods == Mods::SHIFT) =>
            {
                Some(Opens::Keyboard)
            }
            Event::Touch(TouchEvent::Down { id, position }, _) if self.hold.is_none() => {
                let due = Timestamp(cx.now().0 + (TOUCH_HOLD * 1e9).round() as u64);
                self.hold = Some((*id, *position, due));
                None
            }
            Event::Touch(TouchEvent::Motion { id, position }, _) => {
                if let Some((_, from, _)) = self.hold.filter(|(held, ..)| held == id) {
                    if (position.x - from.x).hypot(position.y - from.y) > TOUCH_SLOP {
                        self.hold = None;
                    }
                }
                None
            }
            Event::Touch(TouchEvent::Up { id }, _) => {
                self.hold = self.hold.filter(|(held, ..)| held != id);
                None
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => {
                self.hold = None;
                None
            }
            Event::Wake(now) => {
                let (_, at, _) = self.hold.filter(|(.., due)| due <= now)?;
                self.hold = None;
                Some(Opens::At(at))
            }
            _ => None,
        }
    }

    /// Asks for the wake that ends a finger's rest, after the field's other wakes, which it replaces.
    pub(crate) fn arm<M>(&self, cx: &mut EventCx<'_, M>) {
        if let Some((.., due)) = self.hold {
            cx.wake_at(due);
        }
    }
}
