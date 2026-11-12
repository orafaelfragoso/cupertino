//! The screens as CupertinoKit draws them: the password screen of the lock screen and the greeter, the greeter's name
//! screen, user picker and power menu, and the session agent's confirmation and interrupt panels. Each function builds
//! a window's content from the state machines' state; what the person does comes back as an [`Input`], which the
//! process maps into its own messages.
//!
//! The password field is CupertinoKit's secure field: its text lives only in the field's zeroed buffer, assistive
//! technology sees a password field with a protected value, and diagnostics see bullets. The field is keyed by
//! [`LockScreen::field`](crate::model::LockScreen::field): a submission or Escape makes a new field, so the old one's
//! buffer is dropped and zeroed, and the new one takes the focus on every output.

#[cfg(test)]
mod tests;
mod widgets;

use std::fmt;
use std::sync::Arc;

use cupertino_kit::controls::{
    BareField, BareSecureField, Button, ButtonRole, ButtonStyle, Canvas, CheckBox, Image, ImageFit,
    ImageSource, Menu as KitMenu, MenuAnchor, MenuItem, SecureField, Stack, Text, TextChange,
    TextField, TextRole,
};
use cupertino_kit::core::{
    Checked, CommandRegistry, Element, Key, Live, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{
    AttributedText, ContentPurpose, FontWeight, SecureBuffer, TextStyle, TextSystem,
};
use cupertino_kit::theme::{Color, Surface};

use crate::interrupt::{self, Choice};
use crate::layout::{
    Layout, AVATAR_DIAMETER, HELP_SIZE, NAME_BASELINE_GAP, NAME_MAX_WIDTH, NAME_SIZE, TILE_WIDTH,
    USER_VIEW_BORDER,
};
use crate::lock_notices::{self, Notice};
use crate::model::{Key as PanelKey, Password, MAX_PASSWORD_BYTES};
use crate::panel::{Decision, Frames, Kind, Panel};
use crate::picker::{Direction, Grid, Picker, Tile};
use crate::power::{self, Menu};
use crate::user::{initials, User};
use widgets::{
    Adornment, AlertBody, Avatar, Backdrop, Fade, FieldFrame, Label, Look, NoticeCard, Place,
    Pressable, Slot, ESCAPE, WHITE,
};

/// The `app_id` of a lock surface's window, followed by `.` and the output's registry name: the surface roles give it
/// the output's lock surface.
pub const LOCK_APP_ID: &str = "cupertino-login.lock";
/// The `app_id` of a greeter window, followed by `.` and the output's registry name: a full-output layer surface.
pub const GREETER_APP_ID: &str = "cupertino-login.greeter";
/// The `app_id` of the confirmation panel's and of the interrupt panel's window: a centred layer surface.
pub const PANEL_APP_ID: &str = "cupertino-login.panel";
pub const INTERRUPT_APP_ID: &str = "cupertino-login.interrupt";

/// What assistive technology calls the lock screen and the login screen.
pub const LOCKED_TITLE: &str = "Locked";
pub const GREETER_TITLE: &str = "Login";
/// The password field's name and placeholder.
pub const PASSWORD_LABEL: &str = "Password";
pub const PASSWORD_PLACEHOLDER: &str = "Enter Password";
/// The name field of the "Other…" tile: its name and placeholder (approximate text).
pub const NAME_LABEL: &str = "Name";
pub const NAME_PLACEHOLDER: &str = "Name";
/// The "Other…" tile's label and the mark in its disc (approximate).
pub const OTHER_LABEL: &str = "Other…";
const OTHER_GLYPH: &str = "…";
/// Shown and announced after a wrong password, until the next edit.
pub const DENIED: &str = "Incorrect password. Try again.";
/// What assistive technology hears of the Caps Lock badge and of the spinner.
pub const CAPS_LOCK_ON: &str = "Caps Lock is on";
pub const CHECKING: &str = "Checking the password";
/// The password help line (approximate: this system has no hint to show, so the line points to whoever can reset the
/// password).
pub const PASSWORD_HELP: &str = "Forgot your password? Ask an administrator to reset it.";
/// The power item's name, and the description of a picker tile whose user has a session.
pub const POWER_LABEL: &str = "Power";
pub const LOGGED_IN: &str = "Logged in";

/// Weights of the clock and the names (medium and semibold).
const WEIGHT_MEDIUM: FontWeight = FontWeight(500);
const WEIGHT_SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;
/// Size of the field's notices and the help (the field's text size).
const NOTICE_SIZE: f64 = 13.0;
/// Size of the text typed in the field and of its placeholder.
const FIELD_TEXT_SIZE: f64 = 13.0;
/// The password field's placeholder: white at half opacity.
const PLACEHOLDER_COLOR: Color = Color::new(1.0, 1.0, 1.0, 0.5);
/// Widest a notice draws, and how many lines it may take.
const NOTICE_MAX_WIDTH: f64 = 544.0;
const NOTICE_LINES: f64 = 3.0;
/// White at this alpha for the notices and the help.
const NOTICE_ALPHA: f32 = 0.8;
/// Space between the screen's edges and the help line.
const HELP_MARGIN: f64 = 40.0;
/// Initials size as a fraction of the avatar diameter (approximate).
const INITIALS_SIZE: f64 = 0.4;
/// The field's trailing slot for the Caps Lock badge and the spinner.
const SLOT_WIDTH: f64 = 18.0;
/// Panel title and info: the title's weight; the icon disc's fill (approximate, a neutral grey).
const PANEL_ICON_FILL: Color = Color::new(0.60, 0.62, 0.66, 1.0);
/// Corner radius of a panel (a regular window's).
const PANEL_RADIUS: f64 = 12.0;
/// Text size the panels' title and lines are measured at; the roles set the size they draw at.
const PANEL_TITLE_SIZE: f32 = 13.0;
/// Corner radius of the confirmation panel's alert.
const PANEL_ALERT_RADIUS: f64 = 26.0;
/// The room a check box's box and the gap after it take before its label: where the label of the confirmation panel's
/// checkbox wraps.
const CHECKBOX_LABEL_INSET: f64 = 22.0;

/// Ascent and descent of the interface font as fractions of the em: what places text by its baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub ascent: f64,
    pub descent: f64,
}

/// Metrics of a typical sans face, for a text system that cannot resolve a font.
const FALLBACK_METRICS: Metrics = Metrics {
    ascent: 0.97,
    descent: 0.24,
};

impl Metrics {
    /// The theme font's metrics in `text`.
    pub fn of(text: &TextSystem) -> Metrics {
        text.metrics(&TextStyle::new(PANEL_TITLE_SIZE))
            .map_or(FALLBACK_METRICS, |m| Metrics {
                ascent: f64::from(m.ascent),
                descent: f64::from(m.descent),
            })
    }

    /// The box of one line of `size` points whose baseline is at `baseline`, `width` wide and centred on `center_x`.
    fn line(self, center_x: f64, width: f64, baseline: f64, size: f64, lines: f64) -> Rect {
        Rect::new(
            center_x - width / 2.0,
            baseline - self.ascent * size,
            width,
            (self.ascent + self.descent) * size * lines,
        )
    }
}

/// A password the person submitted, borrowed from the field's buffer into one zeroed-on-drop copy. It never prints:
/// its `Debug` says only that it is there.
#[derive(Clone)]
pub struct Typed(Arc<Password>);

impl Typed {
    /// A copy of what `buffer` holds.
    pub fn of(buffer: &SecureBuffer) -> Typed {
        Typed(Arc::new(buffer.with_secret(crate::model::password)))
    }

    /// For tests and the state machine: the typed text.
    pub fn secret(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Typed(..)")
    }
}

/// What the person did on a screen.
#[derive(Clone, Debug)]
pub enum Input {
    /// A key, a press or a move of the pointer: shows a hidden interface and restarts the idle time.
    Activity,
    /// The password changed.
    Edited,
    /// Escape on the password or name screen.
    Escape,
    /// Return in the password field.
    Submit(Typed),
    /// The name changed.
    Name(TextChange),
    /// Return in the name field.
    SubmitName,
    /// The pointer or the focus came to a picker tile.
    Select(usize),
    /// A picker tile was clicked, or Return or Space chose it.
    Choose(usize),
    /// An arrow key on the picker.
    Step(Direction),
    /// The power item, or Tab: open or close its menu.
    TogglePower,
    /// The power menu closed without a choice.
    ClosePower,
    /// A row of the power menu.
    Power(usize),
    /// A button or key of the confirmation panel.
    Decide(Decision),
    /// The confirmation panel's "Reopen windows" checkbox was set to this, by a click, Space or assistive technology.
    Reopen(bool),
    /// A choice on the native desktop-control consent panel.
    DesktopControl(ControlChoice),
    /// A button or key of the interrupt panel.
    Interrupt(Choice),
}

/// The clock text the screens show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    pub date: String,
    pub time: String,
}

/// The field of the password screen, or of the name screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    /// The password field of `generation` ([`LockScreen::field`](crate::model::LockScreen::field)); `enabled` while
    /// the screen takes a password, `busy` while an attempt is checked
    /// ([`LockScreen::checking`](crate::model::LockScreen::checking)).
    Password {
        generation: u64,
        enabled: bool,
        busy: bool,
    },
    /// The name typed for "Other…" or for an administrator.
    Name { text: String, revision: u64 },
}

/// Everything a password or name screen shows.
pub struct Screen<'a> {
    pub size: Size,
    pub metrics: Metrics,
    pub clock: &'a Clock,
    /// Whose password is asked; `None` on the name screen, which shows the "Other…" disc.
    pub user: Option<&'a User>,
    pub field: Field,
    pub caps_lock: bool,
    /// The spinner's frame while an attempt has been checked for a while.
    pub spinner: Option<u32>,
    /// The last attempt was a wrong password: the outline and [`DENIED`].
    pub denied: bool,
    pub notice: Option<&'a str>,
    pub help: bool,
    /// Of everything but the wallpaper.
    pub opacity: f64,
    /// The field's horizontal offset.
    pub shake: f64,
    /// The interface is hidden: the next key or press only shows it.
    pub hidden: bool,
    /// The greeter's power item.
    pub power: Option<&'a Menu>,
    /// The notifications the lock screen shows, newest first, as the notification daemon gave them
    /// ([`crate::lock_notices`]); none on the greeter.
    pub notices: &'a [Notice],
}

fn label(
    text: impl Into<String>,
    size: f64,
    weight: FontWeight,
    rect: Rect,
    metrics: Metrics,
) -> Label {
    Label {
        text: text.into(),
        size,
        weight,
        color: Some(WHITE),
        rect,
        ascent: metrics.ascent * size,
        wrap: false,
        live: None,
    }
}

/// The avatar of `user` at `rect` (the "Other…" disc without one), its picture or initials inside.
fn avatar<M: 'static>(user: Option<&User>, rect: Rect, metrics: Metrics) -> Element<M> {
    let diameter = rect.width();
    let (filled, logged_in, inside) = match user.and_then(|u| u.picture.as_ref().map(|p| (u, p))) {
        Some((user, picture)) => (
            false,
            user.logged_in,
            Element::new(Image {
                sources: vec![picture.clone()],
                size: Size::new(diameter, diameter),
                fit: ImageFit::Cover,
                radius: diameter / 2.0,
                label: None,
            }),
        ),
        None => {
            let text = user.map_or_else(|| OTHER_GLYPH.to_owned(), |u| initials(&u.display));
            let size = diameter * INITIALS_SIZE;
            let baseline = diameter / 2.0 + (metrics.ascent - metrics.descent) * size / 2.0;
            let mut initials = label(
                text,
                size,
                WEIGHT_MEDIUM,
                metrics.line(diameter / 2.0, diameter, baseline, size, 1.0),
                metrics,
            );
            initials.live = None;
            (
                true,
                user.is_some_and(|u| u.logged_in),
                Element::new(Decorative(initials)),
            )
        }
    };
    Element::new(Avatar {
        rect,
        filled,
        logged_in,
        children: vec![inside],
    })
}

/// A label assistive technology skips: the initials repeat the name beside them.
struct Decorative(Label);

impl<M: 'static> cupertino_kit::core::Widget<M> for Decorative {
    type State = ();

    fn layout(&self) -> cupertino_kit::layout::LayoutSpec {
        <Label as cupertino_kit::core::Widget<M>>::layout(&self.0)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, state: &mut ()) -> cupertino_kit::core::Change {
        <Label as cupertino_kit::core::Widget<M>>::update(&self.0, &old.0, state)
    }

    fn measure(
        &self,
        state: &(),
        constraints: cupertino_kit::layout::Constraints,
        cx: &mut cupertino_kit::core::MeasureCx<'_>,
    ) -> cupertino_kit::layout::Measured {
        <Label as cupertino_kit::core::Widget<M>>::measure(&self.0, state, constraints, cx)
    }

    fn event(
        &self,
        _: &mut (),
        _: &cupertino_kit::core::Event,
        _: &mut cupertino_kit::core::EventCx<'_, M>,
    ) -> cupertino_kit::core::Propagation {
        cupertino_kit::core::Propagation::Continue
    }

    fn paint(&self, state: &(), cx: &mut cupertino_kit::core::PaintCx<'_>) {
        <Label as cupertino_kit::core::Widget<M>>::paint(&self.0, state, cx)
    }

    fn semantics(&self, _: &(), cx: &mut cupertino_kit::core::SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// The keys every screen with a field takes from the focus: Escape, and Tab for the power menu when there is one.
fn field_keys<M: 'static>(
    power: bool,
    map: fn(Input) -> M,
) -> Box<dyn Fn(Keysym, Option<char>) -> Option<M>> {
    const TAB: Keysym = Keysym(0xff09);
    Box::new(move |keysym, _| match keysym {
        ESCAPE => Some(map(Input::Escape)),
        TAB if power => Some(map(Input::TogglePower)),
        _ => None,
    })
}

/// The power item at `layout.power_item`, keyed for its menu to hang under.
fn power_item<M: Clone + 'static>(menu: &Menu, layout: &Layout, map: fn(Input) -> M) -> Element<M> {
    Element::new(Pressable {
        rect: layout.power_item,
        look: Look::Power {
            open: menu.is_open(),
        },
        label: POWER_LABEL.to_owned(),
        description: None,
        on_press: map(Input::TogglePower),
        on_focus: None,
        on_hover: None,
        children: Vec::new(),
    })
    .key(POWER_KEY)
}

/// The element key of the power item, which the menu hangs under.
pub const POWER_KEY: &str = "power";

/// The clock at the top of a screen.
fn clock<M: 'static>(
    clock: &Clock,
    layout: &Layout,
    width: f64,
    metrics: Metrics,
) -> [Element<M>; 2] {
    let line = |text: &str, baseline, size, weight| {
        Element::new(label(
            text,
            size,
            weight,
            metrics.line(layout.center_x, width, baseline, size, 1.0),
            metrics,
        ))
    };
    [
        line(
            &clock.date,
            layout.date_baseline,
            layout.date_size,
            WEIGHT_MEDIUM,
        ),
        line(
            &clock.time,
            layout.time_baseline,
            layout.time_size,
            WEIGHT_SEMIBOLD,
        ),
    ]
}

/// The password screen of the lock screen and the greeter, or the greeter's name screen.
pub fn screen<M: Clone + 'static>(
    screen: &Screen<'_>,
    title: &str,
    map: fn(Input) -> M,
) -> Element<M> {
    let metrics = screen.metrics;
    let (width, height) = (screen.size.width, screen.size.height);
    let layout = Layout::new(width, height, metrics.descent);
    let mut children: Vec<Element<M>> = clock(screen.clock, &layout, width, metrics).into();
    children.push(avatar(screen.user, layout.avatar, metrics));
    let name = screen.user.map_or(OTHER_LABEL, |u| u.display.as_str());
    children.push(Element::new(label(
        name,
        NAME_SIZE,
        WEIGHT_MEDIUM,
        metrics.line(
            layout.center_x,
            NAME_MAX_WIDTH,
            layout.name_baseline,
            NAME_SIZE,
            1.0,
        ),
        metrics,
    )));

    let pill = layout.pill.translate(screen.shake, 0.0);
    let (field, key) = match &screen.field {
        Field::Password {
            generation,
            enabled,
            busy,
        } => (
            Element::new(BareSecureField {
                field: SecureField {
                    label: PASSWORD_LABEL.to_owned(),
                    placeholder: PASSWORD_PLACEHOLDER.to_owned(),
                    enabled: *enabled,
                    on_change: Some(Box::new(move |_| map(Input::Edited))),
                    on_submit: Some(Box::new(move |buffer| {
                        map(Input::Submit(Typed::of(buffer)))
                    })),
                    busy: *busy,
                    // the outline shows it and the notice below says it, once
                    invalid: screen.denied,
                    // the field refuses what the authenticator would not take, so a submission is never cut
                    max_len: Some(MAX_PASSWORD_BYTES),
                    initial: false,
                },
                size: FIELD_TEXT_SIZE,
                weight: Some(WEIGHT_MEDIUM.0),
                placeholder_color: Some(PLACEHOLDER_COLOR),
            }),
            format!("password-{generation}"),
        ),
        Field::Name { text, revision } => (
            Element::new(BareField {
                field: TextField {
                    label: NAME_LABEL.to_owned(),
                    placeholder: NAME_PLACEHOLDER.to_owned(),
                    value: text.clone(),
                    revision: *revision,
                    purpose: ContentPurpose::Name,
                    enabled: true,
                    on_change: Some(Box::new(move |change| map(Input::Name(change)))),
                    on_submit: Some(map(Input::SubmitName)),
                    selection: None,
                    initial: true,
                },
                size: FIELD_TEXT_SIZE,
            }),
            "name".to_owned(),
        ),
    };
    children.push(
        Element::new(FieldFrame {
            rect: pill,
            outline: screen.denied,
            children: vec![field],
        })
        .key(key),
    );
    let slot = match (screen.spinner, screen.caps_lock) {
        (Some(frame), _) => Some((Adornment::Spinner(frame), CHECKING)),
        (None, true) => Some((Adornment::CapsLock, CAPS_LOCK_ON)),
        (None, false) => None,
    };
    if let Some((adornment, label)) = slot {
        children.push(Element::new(Slot {
            rect: Rect::new(
                pill.max_x() - SLOT_WIDTH,
                pill.y(),
                SLOT_WIDTH,
                pill.height(),
            ),
            adornment,
            label,
        }));
    }

    let notices = screen
        .denied
        .then_some(DENIED)
        .into_iter()
        .chain(screen.notice);
    let line_height = (metrics.ascent + metrics.descent) * NOTICE_SIZE;
    for (i, notice) in notices.enumerate() {
        let baseline = layout.notice_baseline + i as f64 * NOTICE_LINES * line_height;
        children.push(Element::new(Label {
            color: Some(Color::new(1.0, 1.0, 1.0, NOTICE_ALPHA)),
            wrap: true,
            live: Some(Live::Assertive),
            ..label(
                notice,
                NOTICE_SIZE,
                FontWeight::REGULAR,
                metrics.line(
                    layout.center_x,
                    NOTICE_MAX_WIDTH,
                    baseline,
                    NOTICE_SIZE,
                    NOTICE_LINES,
                ),
                metrics,
            )
        }));
    }
    if screen.help {
        children.push(Element::new(Label {
            color: Some(Color::new(1.0, 1.0, 1.0, NOTICE_ALPHA)),
            ..label(
                PASSWORD_HELP,
                HELP_SIZE,
                FontWeight::REGULAR,
                metrics.line(
                    layout.center_x,
                    (width - 2.0 * HELP_MARGIN).max(0.0),
                    layout.help_baseline,
                    HELP_SIZE,
                    1.0,
                ),
                metrics,
            )
        }));
    }
    if let Some(menu) = screen.power {
        children.push(power_item(menu, &layout, map));
    }
    children.extend(notice_cards(screen.notices, screen.size, metrics));
    Element::new(Backdrop {
        wallpaper: Some(screen.size),
        hidden: screen.hidden,
        on_activity: Some(map(Input::Activity)),
        keys: field_keys(screen.power.is_some(), map),
        label: title.to_owned(),
        description: screen.user.map(|u| u.display.clone()),
        role: Role::Group,
        modal: false,
        children: vec![Element::new(Fade {
            opacity: screen.opacity,
            children,
        })],
    })
}

/// The cards of the lock screen's notifications in a column at the top right, and the line that counts those not
/// shown. A card shows only what the daemon sent: an application and "Notification", or a title and body it allowed.
fn notice_cards<M: 'static>(
    notices: &[Notice],
    size: Size,
    metrics: Metrics,
) -> Option<Element<M>> {
    if notices.is_empty() {
        return None;
    }
    let (cards, more) = lock_notices::shown(notices);
    let mut children: Vec<Element<M>> = cards
        .iter()
        .map(|n| {
            Element::new(NoticeCard {
                app: n.app.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
                speech: n.speech(),
                ascent: metrics.ascent,
                descent: metrics.descent,
            })
            .key(format!("notice-{}", n.id))
        })
        .collect();
    if more > 0 {
        let size = lock_notices::APP_SIZE;
        let height = ((metrics.ascent + metrics.descent) * size).ceil();
        children.push(Element::new(Label {
            color: Some(Color::new(1.0, 1.0, 1.0, NOTICE_ALPHA)),
            ..label(
                lock_notices::more_text(more),
                size,
                FontWeight::REGULAR,
                Rect::new(0.0, 0.0, lock_notices::CARD_WIDTH, height),
                metrics,
            )
        }));
    }
    let column = lock_notices::column(size.width, size.height);
    Some(Element::new(Place::text(column, children)).key("notices"))
}

/// Everything the user picker shows.
pub struct PickerScreen<'a> {
    pub size: Size,
    pub metrics: Metrics,
    pub clock: &'a Clock,
    pub picker: &'a Picker,
    pub max_per_page: usize,
    pub power: Option<&'a Menu>,
}

/// The element key of picker tile `index`, which the focus follows.
pub fn tile_key(index: usize) -> Key {
    Key::from(format!("tile-{index}"))
}

/// The greeter's user picker: the page of tiles with the selected one, under the clock.
pub fn picker<M: Clone + 'static>(screen: &PickerScreen<'_>, map: fn(Input) -> M) -> Element<M> {
    let metrics = screen.metrics;
    let (width, height) = (screen.size.width, screen.size.height);
    let layout = Layout::new(width, height, metrics.descent);
    let mut children: Vec<Element<M>> = clock(screen.clock, &layout, width, metrics).into();
    let picker = screen.picker;
    let page = picker.page(screen.max_per_page);
    let grid = Grid::for_count(screen.max_per_page.min(picker.tiles().len()));
    let frames = layout.tiles(grid, page.len());
    let avatar_center = Point::new(TILE_WIDTH / 2.0, USER_VIEW_BORDER + AVATAR_DIAMETER / 2.0);
    let name_baseline = 2.0 * USER_VIEW_BORDER + AVATAR_DIAMETER + NAME_BASELINE_GAP;
    for (frame, index) in frames.into_iter().zip(page) {
        let user = match &picker.tiles()[index] {
            Tile::Account(user) => Some(user),
            Tile::Other => None,
        };
        let name = user.map_or(OTHER_LABEL, |u| u.display.as_str());
        let tile = Pressable {
            rect: frame,
            look: Look::Tile {
                selected: index == picker.selected_index(),
                avatar_center,
            },
            label: name.to_owned(),
            description: user.filter(|u| u.logged_in).map(|_| LOGGED_IN.to_owned()),
            on_press: map(Input::Choose(index)),
            on_focus: Some(map(Input::Select(index))),
            on_hover: Some(map(Input::Select(index))),
            children: vec![
                avatar(
                    user,
                    Rect::new(
                        avatar_center.x - AVATAR_DIAMETER / 2.0,
                        USER_VIEW_BORDER,
                        AVATAR_DIAMETER,
                        AVATAR_DIAMETER,
                    ),
                    metrics,
                ),
                Element::new(Decorative(label(
                    name,
                    NAME_SIZE,
                    WEIGHT_MEDIUM,
                    metrics.line(
                        avatar_center.x,
                        NAME_MAX_WIDTH,
                        name_baseline,
                        NAME_SIZE,
                        1.0,
                    ),
                    metrics,
                ))),
            ],
        };
        children.push(Element::new(tile).key(tile_key(index)));
    }
    if let Some(menu) = screen.power {
        children.push(power_item(menu, &layout, map));
    }
    let power = screen.power.is_some();
    let selected = picker.selected_index();
    Element::new(Backdrop {
        wallpaper: Some(screen.size),
        hidden: false,
        on_activity: None,
        keys: Box::new(move |keysym, _| {
            const TAB: Keysym = Keysym(0xff09);
            Some(map(match keysym {
                Keysym(0xff51) => Input::Step(Direction::Left),
                Keysym(0xff52) => Input::Step(Direction::Up),
                Keysym(0xff53) => Input::Step(Direction::Right),
                Keysym(0xff54) => Input::Step(Direction::Down),
                Keysym(0xff0d) | Keysym(0xff8d) => Input::Choose(selected),
                TAB if power => Input::TogglePower,
                _ => return None,
            }))
        }),
        label: GREETER_TITLE.to_owned(),
        description: None,
        role: Role::Group,
        modal: false,
        children: vec![Element::new(Place {
            initial: true,
            ..Place::new(
                Rect::new(0.0, 0.0, width, height),
                vec![Element::new(Fade {
                    opacity: 1.0,
                    children,
                })],
            )
        })],
    })
}

/// The power menu's popup windows while it is open, under the power item of the window `parent`.
pub fn power_menu<M: Clone + 'static>(
    parent: WindowKey,
    menu: &Menu,
    map: fn(Input) -> M,
) -> Vec<Window<M>> {
    if !menu.is_open() {
        return Vec::new();
    }
    let key = WindowKey(Key::from(format!("{:?}-power-menu", parent.0)));
    KitMenu {
        parent,
        anchor: MenuAnchor::Below(Key::from(POWER_KEY)),
        label: POWER_LABEL.to_owned(),
        items: menu
            .items()
            .iter()
            .enumerate()
            .map(|(row, action)| MenuItem::Item {
                label: power::Action::title(*action, menu.will_prompt()).to_owned(),
                enabled: true,
                checked: false,
                on_select: map(Input::Power(row)),
            })
            .collect(),
        current: None,
        open: Vec::new(),
        on_open: Box::new(move |_| map(Input::Activity)),
        on_close: map(Input::ClosePower),
    }
    .windows(key)
}

/// A full-screen window of `size` with `content`: a lock surface or a greeter surface by `app_id`.
pub fn full_screen<M: Clone>(
    key: WindowKey,
    app_id: String,
    title: &str,
    size: Size,
    content: Element<M>,
    on_close: Option<M>,
) -> Window<M> {
    Window {
        key,
        spec: WindowSpec {
            title: title.to_owned(),
            app_id,
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close,
        background: WindowBackground::None,
        content,
        popup: None,
        menus: None,
    }
}

/// How many lines `text` takes at `size` and `weight`, wrapped at `width`.
fn lines(text_system: &TextSystem, text: &str, size: f32, weight: FontWeight, width: f64) -> u32 {
    let content = AttributedText::plain(
        text,
        TextStyle {
            weight,
            ..TextStyle::new(size)
        },
    );
    let style = cupertino_kit::text::ParagraphStyle {
        wrap: cupertino_kit::text::Wrap::Word,
        ..Default::default()
    };
    text_system.paragraph(&content, &style).map_or(1, |p| {
        p.layout(Some(width as f32)).lines().len().max(1) as u32
    })
}

/// The confirmation panel's frames for `kind`, its title, info line and checkbox label measured with `text_system`: the
/// info line takes as many lines as its longest countdown message.
pub fn panel_frames(kind: Kind, text_system: &TextSystem) -> Frames {
    let wrap = crate::panel::wrap_width(kind);
    let title = lines(
        text_system,
        kind.text().title,
        PANEL_TITLE_SIZE,
        FontWeight::BOLD,
        wrap,
    );
    let info = (0..=crate::panel::COUNTDOWN_SECONDS as u32)
        .filter_map(|seconds| kind.countdown_message(seconds))
        .map(|message| {
            lines(
                text_system,
                &message,
                PANEL_TITLE_SIZE,
                FontWeight::REGULAR,
                wrap,
            )
        })
        .max()
        .unwrap_or(1);
    let checkbox = lines(
        text_system,
        crate::panel::REOPEN_LABEL,
        PANEL_TITLE_SIZE,
        FontWeight::REGULAR,
        wrap - CHECKBOX_LABEL_INSET,
    );
    kind.frames(title, info, checkbox)
}

/// What the panels' keys mean: the model's keys, by keysym.
fn panel_key(keysym: Keysym, text: Option<char>) -> Option<PanelKey> {
    match keysym {
        Keysym(0xff0d) | Keysym(0xff8d) => Some(PanelKey::Return),
        ESCAPE => Some(PanelKey::Escape),
        _ => text.filter(|c| !c.is_control()).map(PanelKey::Char),
    }
}

fn panel_text<M: 'static>(
    text: &str,
    role: TextRole,
    weight: FontWeight,
    secondary: bool,
) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(
            text,
            TextStyle {
                weight,
                ..TextStyle::new(PANEL_TITLE_SIZE)
            },
        ),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

fn icon_disc<M: 'static>(rect: Rect) -> Element<M> {
    Element::new(Place::new(
        rect,
        vec![Element::new(Canvas {
            size: Some(rect.size),
            label: String::new(),
            paint: Box::new(move |cx| {
                let side = rect.width();
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(0.0, 0.0, side, side),
                        radius: side / 2.0,
                    },
                    paint: Paint::Solid(PANEL_ICON_FILL),
                })
            }),
            children: Vec::new(),
        })],
    ))
}

fn panel_window<M: Clone>(
    key: WindowKey,
    app_id: &str,
    title: &str,
    size: Size,
    content: Element<M>,
    on_close: M,
) -> Window<M> {
    Window {
        background: WindowBackground::Surface {
            surface: Surface::Popover,
            radius: PANEL_RADIUS,
            alpha: 1.0,
        },
        ..full_screen(key, app_id.to_owned(), title, size, content, Some(on_close))
    }
}

/// The confirmation panel, an alert on glass with no icon: its title, the countdown line, the "Reopen windows" checkbox
/// at `reopen` (log out, restart and shut down), and its buttons as capsules, the default one focused. Return does the
/// default action, Escape cancels, Space flips the checkbox, and the power-button panel's letters choose.
pub fn panel<M: Clone + 'static>(
    key: WindowKey,
    kind: Kind,
    seconds_left: Option<u32>,
    reopen: bool,
    frames: &Frames,
    map: fn(Input) -> M,
) -> Window<M> {
    let text = kind.text();
    let size = Size::new(frames.width, frames.height);
    let mut children = vec![
        Element::new(AlertBody {
            width: size.width,
            height: Some(size.height),
            radius: PANEL_ALERT_RADIUS,
            padding: Insets::ZERO,
            children: Vec::new(),
        }),
        Element::new(Place::text(
            frames.title,
            vec![panel_text(
                text.title,
                TextRole::Body,
                FontWeight::BOLD,
                false,
            )],
        )),
    ];
    if let (Some(info), Some(frame)) = (
        seconds_left.and_then(|s| kind.countdown_message(s)),
        frames.info,
    ) {
        children.push(Element::new(Place::text(
            frame,
            vec![panel_text(
                &info,
                TextRole::Body,
                FontWeight::REGULAR,
                false,
            )],
        )));
    }
    if let Some(checkbox) = frames.checkbox {
        children.push(
            // against the text column's leading edge, its label wrapping in the column
            Element::new(Place::text(
                checkbox.frame,
                vec![Element::new(CheckBox {
                    label: checkbox.label.to_owned(),
                    checked: if reopen {
                        Checked::True
                    } else {
                        Checked::False
                    },
                    enabled: true,
                    on_toggle: Some(Box::new(move |on| map(Input::Reopen(on)))),
                })],
            ))
            .key("reopen"),
        );
    }
    let default = Decision::Do(kind.default_action());
    for button in &frames.buttons {
        children.push(
            Element::new(Place {
                initial: button.decision == kind.focused(),
                ..Place::filled(
                    button.frame,
                    vec![Element::new(Button {
                        label: button.label.to_owned(),
                        role: if button.decision == default {
                            ButtonRole::Default
                        } else {
                            ButtonRole::Normal
                        },
                        style: ButtonStyle::Pill,
                        enabled: true,
                        busy: false,
                        on_press: Some(map(Input::Decide(button.decision))),
                    })],
                )
            })
            .key(button.label),
        );
    }
    let content = Element::new(Backdrop {
        wallpaper: None,
        hidden: false,
        on_activity: None,
        keys: Box::new(move |keysym, text| {
            let key = panel_key(keysym, text)?;
            let panel = Panel::new(kind, 0.0, reopen);
            if panel.toggles(key) {
                return Some(map(Input::Reopen(!reopen)));
            }
            panel
                .press(key)
                .map(|decision| map(Input::Decide(decision)))
        }),
        label: text.title.to_owned(),
        description: None,
        role: Role::AlertDialog,
        modal: true,
        children,
    });
    Window {
        background: WindowBackground::None,
        ..full_screen(
            key,
            PANEL_APP_ID.to_owned(),
            text.title,
            size,
            content,
            Some(map(Input::Decide(Decision::Cancel))),
        )
    }
}

pub const CONTROL_APP_ID: &str = "org.cupertino.login.desktop-control";
/// The consent panel's surface: the alert's width, and a height that holds the alert with the longest name, path and
/// list of rights, the alert centred in it and the rest clear.
pub const CONTROL_WIDTH: f64 = 260.0;
pub const CONTROL_HEIGHT: f64 = 520.0;
/// The alert: its corner radius and its padding; the icon and the text sit further in than the buttons.
const CONTROL_RADIUS: f64 = 26.0;
const CONTROL_PADDING: Insets = Insets {
    top: 22.0,
    left: 16.0,
    bottom: 16.0,
    right: 16.0,
};
const CONTROL_TEXT_INSET: f64 = 6.0;
/// The app's icon, and the room under it, under the title and above the buttons.
const CONTROL_ICON: f64 = 64.0;
const CONTROL_ICON_GAP: f64 = 14.0;
const CONTROL_TITLE_GAP: f64 = 10.0;
const CONTROL_BUTTONS_GAP: f64 = 16.0;
/// The two capsule buttons side by side, this far apart, filling the alert's width between its insets.
const CONTROL_BUTTON_GAP: f64 = 8.0;
const CONTROL_BUTTON_HEIGHT: f64 = 28.0;
const CONTROL_BUTTON_WIDTH: f64 = (CONTROL_WIDTH - 16.0 - 16.0 - CONTROL_BUTTON_GAP) / 2.0;
/// The size of the surface: the consent alert while a request waits, the status panel with Stop while a run is on.
pub fn control_size(phase: u32) -> Size {
    if phase == 1 {
        Size::new(CONTROL_WIDTH, CONTROL_HEIGHT)
    } else {
        Size::new(interrupt::WIDTH, interrupt::HEIGHT)
    }
}
/// Characters of an app's name the panel shows; a longer name is cut with an ellipsis.
const CONTROL_NAME_CHARS: usize = 48;
/// Characters of an executable's path the panel shows; a longer path keeps its end, which names the program.
const CONTROL_PATH_CHARS: usize = 120;
/// Characters of an output's name the panel shows.
const CONTROL_OUTPUT_CHARS: usize = 64;
/// Service bits of an app's request: desktop control and screen capture.
const SERVICE_CONTROL: u32 = 1;
const SERVICE_CAPTURE: u32 = 2;

/// What the consent panel asks the person.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlRequest {
    /// The output the run would control.
    pub output: String,
    /// The capability bits the run asks for: observe 1, capture 2, pointer 4, keyboard 8, native controls 16.
    pub capabilities: u32,
    /// The app asking; `None` for a controller that does not name it, whose answer holds for this run only.
    pub app: Option<RequestingApp>,
}

/// The app a consent request comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestingApp {
    /// The desktop entry's name, or the executable's file name.
    pub name: String,
    /// The desktop entry id, or the executable's absolute path.
    pub identity: String,
    /// Whether `identity` is a desktop entry id.
    pub desktop: bool,
    /// The services asked: desktop control 1, screen capture 2.
    pub services: u32,
    /// Whether the answer is remembered for the app.
    pub remembered: bool,
    /// The app's icon at the sizes there are; none draws the plain disc.
    pub icon: Vec<ImageSource>,
}

/// The person's choice on the consent panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlChoice {
    /// Allow, remembered for the app.
    Allow,
    /// Don't Allow, remembered for the app.
    DontAllow,
    /// The panel was closed without a choice (Escape): this request only.
    Dismiss,
}

/// `text` without control or bidirectional formatting characters, cut at `chars` characters with an ellipsis; `keep_end`
/// keeps the end instead of the start.
fn clean(text: &str, chars: usize, keep_end: bool) -> String {
    let kept: Vec<char> = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    if kept.len() <= chars {
        return kept.into_iter().collect();
    }
    if keep_end {
        std::iter::once('\u{2026}')
            .chain(kept[kept.len() + 1 - chars..].iter().copied())
            .collect()
    } else {
        kept[..chars - 1]
            .iter()
            .copied()
            .chain(std::iter::once('\u{2026}'))
            .collect()
    }
}

/// The title and the information text of the consent panel for `request`.
pub fn control_text(request: &ControlRequest) -> (String, String) {
    let services = request.app.as_ref().map_or_else(
        || {
            let mut services = 0;
            if request.capabilities & !2 != 0 {
                services |= SERVICE_CONTROL;
            }
            if request.capabilities & 2 != 0 {
                services |= SERVICE_CAPTURE;
            }
            services
        },
        |app| app.services,
    );
    let rights: Vec<_> = [
        (1, SERVICE_CONTROL, "inspect the display"),
        (2, SERVICE_CAPTURE, "capture the display"),
        (4, SERVICE_CONTROL, "use the pointer"),
        (8, SERVICE_CONTROL, "use the keyboard"),
        (16, SERVICE_CONTROL, "use native controls"),
    ]
    .into_iter()
    .filter(|(bit, service, _)| request.capabilities & bit != 0 && services & service != 0)
    .map(|(_, _, label)| label)
    .collect();
    let display = clean(&request.output, CONTROL_OUTPUT_CHARS, false);
    let Some(app) = &request.app else {
        return (
            "Allow desktop control?".to_owned(),
            format!(
                "Allow the desktop controller to {} on {display}. This choice applies to this request only. \
                 Ctrl+Alt+Escape stops it.",
                rights.join(", ")
            ),
        );
    };
    let name = clean(&app.name, CONTROL_NAME_CHARS, false);
    let wants = match (
        services & SERVICE_CONTROL != 0,
        services & SERVICE_CAPTURE != 0,
    ) {
        (true, true) => "control and capture the desktop",
        (false, true) => "capture the screen",
        _ => "control the desktop",
    };
    let title = format!("Allow \u{201c}{name}\u{201d} to {wants}?");
    let mut info = format!("It will be able to {} on {display}.", rights.join(", "));
    if !app.desktop {
        info.push_str(&format!(
            " The program is {}.",
            clean(&app.identity, CONTROL_PATH_CHARS, true)
        ));
    }
    info.push_str(if app.remembered {
        " Your choice is remembered for this app; change it any time in Settings \u{203a} Privacy."
    } else {
        " This choice applies to this request only."
    });
    info.push_str(" Ctrl+Alt+Escape stops control.");
    (title, info)
}

/// The native consent panel, an alert on glass: the app's icon, the question, the rights and the display, and Don't
/// Allow beside Allow. Neither button has a default-key binding (Return does nothing); Don't Allow initially has the
/// focus, and Escape closes the panel without a choice.
pub fn desktop_control<M: Clone + 'static>(
    key: WindowKey,
    request: &ControlRequest,
    map: fn(Input) -> M,
) -> Window<M> {
    let (title, info) = control_text(request);
    let icon_rect = Rect::new(0.0, 0.0, CONTROL_ICON, CONTROL_ICON);
    let icon = match request.app.as_ref().filter(|a| !a.icon.is_empty()) {
        Some(app) => Element::new(Place::new(
            icon_rect,
            vec![Element::new(Image {
                sources: app.icon.clone(),
                size: icon_rect.size,
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            })],
        )),
        None => icon_disc(icon_rect),
    };
    let stack = |axis, spacing, align, padding, children| {
        Element::new(Stack {
            axis,
            padding,
            spacing,
            align,
            children,
        })
    };
    let text = stack(
        Axis::Vertical,
        CONTROL_TITLE_GAP,
        Align::Stretch,
        Insets::ZERO,
        vec![
            panel_text(&title, TextRole::Body, FontWeight::BOLD, false),
            panel_text(&info, TextRole::Body, FontWeight::REGULAR, false),
        ],
    );
    let head = stack(
        Axis::Vertical,
        CONTROL_ICON_GAP,
        Align::Stretch,
        Insets {
            top: 0.0,
            left: CONTROL_TEXT_INSET,
            bottom: 0.0,
            right: CONTROL_TEXT_INSET,
        },
        vec![
            stack(
                Axis::Horizontal,
                0.0,
                Align::Start,
                Insets::ZERO,
                vec![icon],
            ),
            text,
        ],
    );
    let buttons = [
        ("Don't Allow", ControlChoice::DontAllow),
        ("Allow", ControlChoice::Allow),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (label, choice))| {
        Element::new(Place {
            initial: index == 0,
            ..Place::filled(
                Rect::new(0.0, 0.0, CONTROL_BUTTON_WIDTH, CONTROL_BUTTON_HEIGHT),
                vec![Element::new(Button {
                    label: label.to_owned(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Pill,
                    enabled: true,
                    busy: false,
                    on_press: Some(map(Input::DesktopControl(choice))),
                })],
            )
        })
        .key(label)
    })
    .collect();
    let body = stack(
        Axis::Vertical,
        CONTROL_BUTTONS_GAP,
        Align::Stretch,
        Insets::ZERO,
        vec![
            head,
            stack(
                Axis::Horizontal,
                CONTROL_BUTTON_GAP,
                Align::Center,
                Insets::ZERO,
                buttons,
            ),
        ],
    );
    let cancel = map(Input::DesktopControl(ControlChoice::Dismiss));
    let close = cancel.clone();
    let size = Size::new(CONTROL_WIDTH, CONTROL_HEIGHT);
    let content = Element::new(Backdrop {
        wallpaper: None,
        hidden: false,
        on_activity: None,
        keys: Box::new(move |sym, _| (sym == ESCAPE).then(|| cancel.clone())),
        label: title.clone(),
        description: None,
        role: Role::AlertDialog,
        modal: true,
        children: vec![Element::new(Place::new(
            Rect::new(0.0, 0.0, size.width, size.height),
            vec![Element::new(AlertBody {
                width: CONTROL_WIDTH,
                height: None,
                radius: CONTROL_RADIUS,
                padding: CONTROL_PADDING,
                children: vec![body],
            })],
        ))],
    });
    Window {
        background: WindowBackground::None,
        ..full_screen(
            key,
            CONTROL_APP_ID.to_owned(),
            &title,
            size,
            content,
            Some(close),
        )
    }
}

/// The interrupt panel for the application `app`: Cancel and Force Quit, Cancel focused. Neither is the default:
/// Return does nothing, so a key held from typing does not kill the application.
pub fn interrupt<M: Clone + 'static>(key: WindowKey, app: &str, map: fn(Input) -> M) -> Window<M> {
    let frames = interrupt::Frames::new();
    let title = interrupt::title(app);
    let mut children = vec![
        icon_disc(frames.icon),
        Element::new(Place::text(
            frames.title,
            vec![panel_text(&title, TextRole::Body, FontWeight::BOLD, false)],
        )),
        Element::new(Place::text(
            frames.info,
            vec![panel_text(
                &interrupt::info(app),
                TextRole::Caption,
                FontWeight::REGULAR,
                true,
            )],
        )),
    ];
    for button in &frames.buttons {
        children.push(
            Element::new(Place {
                initial: button.choice == Choice::Cancel,
                ..Place::filled(
                    button.frame,
                    vec![Element::new(Button {
                        label: button.label.to_owned(),
                        role: ButtonRole::Normal,
                        style: ButtonStyle::Bordered,
                        enabled: true,
                        busy: false,
                        on_press: Some(map(Input::Interrupt(button.choice))),
                    })],
                )
            })
            .key(button.label),
        );
    }
    let content = Element::new(Backdrop {
        wallpaper: None,
        hidden: false,
        on_activity: None,
        keys: Box::new(move |keysym, text| {
            panel_key(keysym, text)
                .and_then(interrupt::press)
                .map(|choice| map(Input::Interrupt(choice)))
        }),
        label: title.clone(),
        description: None,
        role: Role::AlertDialog,
        modal: true,
        children,
    });
    panel_window(
        key,
        INTERRUPT_APP_ID,
        &title,
        Size::new(interrupt::WIDTH, interrupt::HEIGHT),
        content,
        map(Input::Interrupt(Choice::Cancel)),
    )
}
