//! The screens as CupertinoKit draws them: the password screen of the lock screen and the greeter, the greeter's name
//! screen, user picker and power menu, and the session agent's confirmation and interrupt panels. Each function builds
//! a window's content from the state machines' state; what the person does comes back as an [`Input`], which the
//! process maps into its own messages.
//!
//! The password field is CupertinoKit's secure field: its text lives only in the field's zeroed buffer, assistive
//! technology sees a password field with a protected value, and diagnostics see bullets. The field is keyed by
//! [`LockScreen::field`](crate::model::LockScreen::field): a submission or Escape makes a new field, so the old one's
//! buffer is dropped and zeroed, and the new one takes the focus on every output.

mod pictures;
mod wants;
mod widgets;

use std::fmt;
use std::sync::Arc;

use cupertino_kit::controls::{
    BareField, BareSecureField, Image, ImageFit, Menu as KitMenu, MenuAnchor, MenuItem,
    SecureField, TextChange, TextField,
};
use cupertino_kit::core::{
    CommandRegistry, Element, Key, Live, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_kit::text::{
    AttributedText, ContentPurpose, FontWeight, SecureBuffer, TextAlign, TextStyle, TextSystem,
};
use cupertino_kit::theme::Color;

use crate::layout::{
    Layout, BUTTON_SIZE, HINT_LINES, HINT_SIZE, HINT_WIDTH, LOCK_MESSAGE_SIZE, LOCK_MESSAGE_WIDTH,
    NAME_MAX_WIDTH, NAME_SIZE, STATUS_BAR_HEIGHT, SUBMIT_GAP, SUB_HINT_SIZE, SUB_HINT_WIDTH,
};
use crate::lock_notices::{self, Notice};
use crate::model::{Password, MAX_PASSWORD_BYTES, SHAKE_AMPLITUDE};
use crate::picker::{Direction, Grid, Jump, Picker, Tile};
use crate::power::{self, Menu};
use crate::status::{StatusItems, WifiState};
use crate::user::{initials, User};
use crate::wallpaper::Source;
use pictures::Glass;
pub use pictures::{Made, ScreenPictures, Shown, Wants};
use wants::notices_strip;
pub use widgets::Access;
use widgets::{
    Adornment, Avatar, Backdrop, Fade, FieldFrame, FieldMaterial, FieldTint, GlassText, Glyph,
    GlyphKind, Ground, Label, Look, NoticeCard, Place, Pressable, Shift, Slot, StatusItem,
    StatusKind, StatusRow, ESCAPE, OPTION_RETURN, WHITE,
};

/// The `app_id` of a lock surface's window, followed by `.` and the number the program gives the output while it is
/// there: the program's `surface` gives the window the output's lock surface.
pub const LOCK_APP_ID: &str = "cupertino-login.lock";
/// The `app_id` of a greeter window, followed by `.` and the number the program gives the output: a full-output layer
/// surface.
pub const GREETER_APP_ID: &str = "cupertino-login.greeter";

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
/// The password field's placeholder and its padlock's name when the account is locked.
pub const LOCKED_PLACEHOLDER: &str = "Locked";
pub const ACCOUNT_LOCKED: &str = "This account is locked";
/// The greeter's back button.
pub const BACK_LABEL: &str = "Back";
pub const CHECKING: &str = "Checking the password";
/// The password help line (approximate: this system has no hint to show, so the line points to whoever can reset the
/// password).
pub const PASSWORD_HELP: &str = "Forgot your password? Ask an administrator to reset it.";
/// The help button's name (approximate).
pub const HELP_LABEL: &str = "Password Help";
/// The power item's name, and the description of a picker tile whose user has a session.
pub const POWER_LABEL: &str = "Power";
pub const LOGGED_IN: &str = "Logged in";

/// Weights of the date and the names (medium) and of the time.
const WEIGHT_MEDIUM: FontWeight = FontWeight(500);
const WEIGHT_TIME: FontWeight = FontWeight(640);
/// Weight of the names in the picker's rows.
const WEIGHT_SEMIBOLD: FontWeight = FontWeight(600);
/// Size of the text typed in the field and of its placeholder.
const FIELD_TEXT_SIZE: f64 = 13.0;
/// The password field's placeholder: white at half opacity.
const PLACEHOLDER_COLOR: Color = Color::new(1.0, 1.0, 1.0, 0.5);
/// White at this alpha for the help line and the count of notifications not shown (approximate).
const NOTICE_ALPHA: f32 = 0.8;
/// Initials size as a fraction of the avatar diameter (approximate).
const INITIALS_SIZE: f64 = 0.4;
/// The field's trailing slot for the Caps Lock badge and the spinner, and the room after it (approximate).
const SLOT_WIDTH: f64 = 18.0;
const SLOT_INSET: f64 = 5.0;
/// The text insets of the name and the password fields where both show, before and after the text.
const CREDENTIAL_INSETS: Insets = Insets {
    top: 0.0,
    left: 15.0,
    bottom: 0.0,
    right: 15.0,
};
/// The gap between the name field and the password field under it where both show (approximate).
const CREDENTIAL_GAP: f64 = 8.0;
/// How far the avatar moves with the password group's shake, as a share of the group's offset (approximate).
const AVATAR_SHAKE_SHARE: f64 = 0.25;
/// How far the name's shadow reaches past its line: three deviations of its blur, rounded up. The user block's layer
/// holds it.
const SHADOW_REACH: f64 = 12.0;
/// Text size the panels' title and lines are measured at; the roles set the size they draw at.
const PANEL_TITLE_SIZE: f32 = 13.0;

/// Ascent and descent of the interface font as fractions of the em: what places text by its baseline; and the
/// password field's placeholder's width in points, which the field grows to hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub ascent: f64,
    pub descent: f64,
    pub placeholder: f64,
}

/// Metrics of a typical sans face, for a text system that cannot resolve a font.
const FALLBACK_METRICS: Metrics = Metrics {
    ascent: 0.97,
    descent: 0.24,
    placeholder: 90.0,
};

impl Metrics {
    /// The theme font's metrics in `text`.
    pub fn of(text: &TextSystem) -> Metrics {
        let placeholder = AttributedText::plain(
            PASSWORD_PLACEHOLDER,
            TextStyle {
                weight: WEIGHT_MEDIUM,
                ..TextStyle::new(FIELD_TEXT_SIZE as f32)
            },
        );
        let placeholder = text
            .paragraph(
                &placeholder,
                &cupertino_kit::text::ParagraphStyle::default(),
            )
            .ok()
            .and_then(|p| {
                p.layout(None)
                    .lines()
                    .iter()
                    .map(|line| line.rect.width())
                    .reduce(f64::max)
            })
            .unwrap_or(FALLBACK_METRICS.placeholder);
        text.metrics(&TextStyle::new(PANEL_TITLE_SIZE))
            .map_or(FALLBACK_METRICS, |m| Metrics {
                ascent: f64::from(m.ascent),
                descent: f64::from(m.descent),
                placeholder,
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
    /// The password changed; whether the field holds text now.
    Edited(bool),
    /// The help button: show or hide the password help.
    Help,
    /// Escape on the password or name screen.
    Escape,
    /// Return in the password field.
    Submit(Typed),
    /// The name changed.
    Name(TextChange),
    /// Return in the name field.
    SubmitName,
    /// The pointer came to a picker tile.
    Select(usize),
    /// The focus came to a picker tile without the keyboard (as the window opens): Return starts from it, and nothing
    /// shows on it. The keyboard bringing the focus to a tile selects it ([`Input::Select`]).
    Focused(usize),
    /// A picker tile was clicked, or Return or Space chose it.
    Choose(usize),
    /// An arrow key on the picker.
    Step(Direction),
    /// Home, End, Page Up or Page Down on the picker.
    Jump(Jump),
    /// Option+Return: the name and password screen in place of the picker, or the picker again.
    ToggleNames,
    /// The power item: open or close its menu.
    TogglePower,
    /// The accessibility item: open or close its menu.
    ToggleAccess,
    /// The accessibility menu closed.
    CloseAccess,
    /// A row of the accessibility menu: turn its option on or off.
    AccessOption(AccessOption),
    /// The power menu closed without a choice.
    ClosePower,
    /// A row of the power menu.
    Power(usize),
}

/// The clock text the screens show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    pub date: String,
    pub time: String,
}

/// The password field of the screen.
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
}

/// The name field shown above the password field: the name typed and the revision of it the field shows
/// ([`NameEntry`](crate::picker::NameEntry)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NameField<'a> {
    pub text: &'a str,
    pub revision: u64,
}

/// Everything a password or name screen shows.
pub struct Screen<'a> {
    pub size: Size,
    pub metrics: Metrics,
    pub clock: &'a Clock,
    /// Whose password is asked; `None` where a name is typed, which shows the "Other…" disc.
    pub user: Option<&'a User>,
    pub field: Field,
    /// A name field above the password field, for a name typed with its password: the name label does not show.
    pub name: Option<NameField<'a>>,
    /// The field holds text: the submit button shows.
    pub typed: bool,
    pub caps_lock: bool,
    /// The spinner's frame while an attempt has been checked for a while.
    pub spinner: Option<u32>,
    /// The last attempt was a wrong password: the outline.
    pub denied: bool,
    /// And [`DENIED`] says so under the field.
    pub denied_said: bool,
    pub notice: Option<&'a str>,
    /// The help button shows; `help_open`: and the help it opens.
    pub help: bool,
    pub help_open: bool,
    /// Of everything but the wallpaper.
    pub opacity: f64,
    /// Of the user block on top of `opacity`: the greeter's switch between its picker and this screen.
    pub block: f64,
    /// The field's horizontal offset.
    pub shake: f64,
    /// The interface is hidden: the next key or press only shows it.
    pub hidden: bool,
    /// The custom message under the user block, near the bottom ([`crate::login_options`]).
    pub lock_message: Option<&'a str>,
    /// The greeter's back button before the field: back to the picker.
    pub back: bool,
    /// The account is locked: the field takes no password and says so.
    pub locked: bool,
    /// Neither the avatar nor the name shows.
    pub hide_user: bool,
    /// The display options the field and its buttons follow.
    pub access: Access,
    /// The date and the large time show.
    pub large_clock: bool,
    /// Where the avatar starts while the screen comes in from the picker (`block` below 1): it moves from the chosen
    /// tile's avatar to its own place, at full opacity, as the rest fades in.
    pub avatar_from: Option<Rect>,
    /// The greeter's power item.
    pub power: Option<&'a Menu>,
    /// The status bar's other items.
    pub status: Status<'a>,
    /// The notifications the lock screen shows, newest first, as the notification daemon gave them
    /// ([`crate::lock_notices`]); none on the greeter.
    pub notices: &'a [Notice],
    /// What the screen stands on, and its dimming's opacity (0 to 1).
    pub wallpaper: &'a Source,
    pub dim: f64,
    /// The pictures the screen was last given ([`ScreenPictures::shown`]): its wallpaper's picture, its controls'
    /// material and its glass letters.
    pub pictures: &'a Shown,
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
        shadow: false,
        align: TextAlign::Center,
        shift: None,
    }
}

/// The password field's placeholder colour: white 0.5, black on an opaque light field.
fn placeholder_color(access: Access) -> Color {
    if access.reduce_transparency {
        Color::new(0.0, 0.0, 0.0, widgets::OPAQUE_PLACEHOLDER_ALPHA)
    } else {
        PLACEHOLDER_COLOR
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
            let initials = label(
                text,
                size,
                WEIGHT_MEDIUM,
                metrics.line(diameter / 2.0, diameter, baseline, size, 1.0),
                metrics,
            );
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

/// The keys every screen with a field takes from the focus: Escape, and Option+Return where a name is typed. Tab
/// moves between the screen's controls.
fn field_keys<M: 'static>(
    names: bool,
    map: fn(Input) -> M,
) -> Box<dyn Fn(Keysym, Option<char>) -> Option<M>> {
    Box::new(move |keysym, _| match keysym {
        ESCAPE => Some(map(Input::Escape)),
        OPTION_RETURN if names => Some(map(Input::ToggleNames)),
        _ => None,
    })
}

/// The status bar's power item at `layout.power_item`, keyed for its menu to hang under, in a part of its own.
fn power_item<M: Clone + 'static>(
    menu: &Menu,
    layout: &Layout,
    opacity: f64,
    map: fn(Input) -> M,
) -> Element<M> {
    let rect = layout.power_item;
    let item = Element::new(Pressable {
        rect: Rect::new(0.0, 0.0, rect.width(), rect.height()),
        look: Look::Power {
            open: menu.is_open(),
        },
        label: POWER_LABEL.to_owned(),
        description: None,
        on_press: map(Input::TogglePower),
        on_focus: None,
        on_hover: None,
        focusable: true,
        children: Vec::new(),
    })
    .key(POWER_KEY);
    Element::new(Fade::new(rect, opacity, vec![item])).key(POWER_PART)
}

/// The element key of the power item, which the menu hangs under.
pub const POWER_KEY: &str = "power";
/// The element key of the accessibility item, which its menu hangs under.
pub const ACCESS_KEY: &str = "accessibility";
/// What assistive technology calls the accessibility item.
pub const ACCESS_LABEL: &str = "Accessibility";

/// No status items.
pub static NO_STATUS: StatusItems = StatusItems {
    wifi: None,
    battery: None,
    input: None,
    host: None,
};

/// What the status bar shows besides the power item.
#[derive(Clone, Copy, Debug)]
pub struct Status<'a> {
    pub items: &'a StatusItems,
    /// The login screen's accessibility item: `Some(open)` while it shows, `open` while its menu is.
    pub access: Option<bool>,
}

impl Status<'_> {
    /// Nothing but the power item, if any.
    pub const NONE: Status<'static> = Status {
        items: &NO_STATUS,
        access: None,
    };
}

/// The status bar's items at the top right besides the power item: the accessibility item after it (the login screen),
/// and before them, 14 pt apart and centred in the bar, a line about the machine, the input source, the Wi-Fi radio and
/// the battery, each a part of its own.
fn status_bar<M: Clone + 'static>(
    status: Status<'_>,
    power: bool,
    width: f64,
    metrics: Metrics,
    opacity: f64,
    map: fn(Input) -> M,
) -> Vec<Element<M>> {
    let mut parts = Vec::new();
    let mut fixed = usize::from(power);
    if let Some(open) = status.access {
        let rect = Layout::status_item(width, fixed);
        fixed += 1;
        let item = Element::new(Pressable {
            rect: Rect::new(0.0, 0.0, rect.width(), rect.height()),
            look: Look::Access { open },
            label: ACCESS_LABEL.to_owned(),
            description: None,
            on_press: map(Input::ToggleAccess),
            on_focus: None,
            on_hover: None,
            focusable: true,
            children: Vec::new(),
        })
        .key(ACCESS_KEY);
        parts.push(Element::new(Fade::new(rect, opacity, vec![item])).key(ACCESS_PART));
    }
    let items = status.items;
    let item = |kind: StatusKind, label: String| {
        Element::new(StatusItem {
            kind,
            label,
            ascent: metrics.ascent,
            descent: metrics.descent,
        })
    };
    let mut row = Vec::new();
    if let Some(host) = &items.host {
        row.push(item(StatusKind::Text(host.clone()), host.clone()));
    }
    if let Some(input) = &items.input {
        row.push(item(
            StatusKind::Text(input.clone()),
            format!("{INPUT_LABEL}: {input}"),
        ));
    }
    if let Some(wifi) = items.wifi {
        let said = match wifi {
            WifiState::Off => WIFI_OFF,
            WifiState::NotConnected => WIFI_NOT_CONNECTED,
            WifiState::Connected { .. } => WIFI_CONNECTED,
        };
        row.push(item(StatusKind::Wifi(wifi), said.to_owned()));
    }
    if let Some(battery) = items.battery {
        row.push(item(
            StatusKind::Battery(battery),
            format!("{BATTERY_LABEL}: {:.0} %", battery.percentage),
        ));
    }
    if !row.is_empty() {
        // the row ends 14 pt before the first fixed item, or where the power item would end
        let first = Layout::status_item(width, fixed);
        let end = if fixed > 0 {
            Layout::status_item(width, fixed - 1).x() - STATUS_ROW_GAP
        } else {
            first.max_x()
        };
        let bar = Rect::new(0.0, 0.0, width, STATUS_BAR_HEIGHT);
        parts.push(
            Element::new(Fade::new(
                bar,
                opacity,
                vec![Element::new(StatusRow {
                    rect: Rect::new(0.0, 0.0, end.max(0.0), STATUS_BAR_HEIGHT),
                    children: row,
                })],
            ))
            .key(STATUS_PART),
        );
    }
    parts
}

/// The accessibility item's menu while it is open, under the item of the window `parent`: the display options the login
/// screen can change, each checked while it is on.
pub fn access_menu<M: Clone + 'static>(
    parent: WindowKey,
    open: bool,
    access: Access,
    reduce_motion: bool,
    map: fn(Input) -> M,
) -> Vec<Window<M>> {
    if !open {
        return Vec::new();
    }
    let key = WindowKey(Key::from(format!("{:?}-access-menu", parent.0)));
    let row = |label: &str, checked: bool, option: AccessOption| MenuItem::Item {
        label: label.to_owned(),
        enabled: true,
        checked,
        on_select: map(Input::AccessOption(option)),
    };
    KitMenu {
        parent,
        anchor: MenuAnchor::Below(Key::from(ACCESS_KEY)),
        label: ACCESS_LABEL.to_owned(),
        items: vec![
            row(
                INCREASE_CONTRAST,
                access.increase_contrast,
                AccessOption::IncreaseContrast,
            ),
            row(
                REDUCE_TRANSPARENCY,
                access.reduce_transparency,
                AccessOption::ReduceTransparency,
            ),
            row(REDUCE_MOTION, reduce_motion, AccessOption::ReduceMotion),
        ],
        current: None,
        open: Vec::new(),
        on_open: Box::new(move |_| map(Input::Activity)),
        on_close: map(Input::CloseAccess),
    }
    .windows(key)
}

/// A display option of the accessibility menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessOption {
    IncreaseContrast,
    ReduceTransparency,
    ReduceMotion,
}

/// The status items' words for assistive technology and the accessibility menu's rows.
const INPUT_LABEL: &str = "Input source";
const BATTERY_LABEL: &str = "Battery";
const WIFI_OFF: &str = "Wi-Fi: off";
const WIFI_NOT_CONNECTED: &str = "Wi-Fi: not connected";
const WIFI_CONNECTED: &str = "Wi-Fi: connected";
const INCREASE_CONTRAST: &str = "Increase Contrast";
const REDUCE_TRANSPARENCY: &str = "Reduce Transparency";
const REDUCE_MOTION: &str = "Reduce Motion";
/// The gap between the status bar's row and the items after it.
const STATUS_ROW_GAP: f64 = 14.0;

/// `rect` in the points of a part that starts at `origin`.
fn within(rect: Rect, origin: Point) -> Rect {
    rect.translate(-origin.x, -origin.y)
}

/// Where the clock at the top of a screen stands: the date's line, the time's glass letters (in the points of the
/// clock's part) and the part, just their size.
fn clock_parts(clock: &Clock, layout: &Layout, metrics: Metrics) -> (Rect, Glass, Rect) {
    let date = metrics.line(
        layout.center_x,
        layout.date_width(),
        layout.date_baseline,
        layout.date_size,
        1.0,
    );
    let time = metrics.line(
        layout.center_x,
        layout.time_width(),
        layout.time_baseline,
        layout.time_size,
        1.0,
    );
    let rect = date.union(&time);
    let glass = Glass {
        text: clock.time.clone(),
        size: layout.time_size,
        weight: WEIGHT_TIME,
        rect: within(time, rect.origin),
        ascent: metrics.ascent * layout.time_size,
        origin: rect.origin,
    };
    (date, glass, rect)
}

/// The clock at the top of a screen, a part of its own just its size.
/// The date in solid white and the time below it in glass letters made from the screen's picture.
fn clock<M: 'static>(
    clock: &Clock,
    layout: &Layout,
    metrics: Metrics,
    opacity: f64,
    pictures: &Shown,
) -> Element<M> {
    let (date, glass, rect) = clock_parts(clock, layout, metrics);
    Element::new(Fade::new(
        rect,
        opacity,
        vec![
            Element::new(label(
                &clock.date,
                layout.date_size,
                WEIGHT_MEDIUM,
                within(date, rect.origin),
                metrics,
            )),
            Element::new(GlassText {
                glass,
                letters: pictures.letters(),
            }),
        ],
    ))
    .key(CLOCK_PART)
}

/// The keys of the parts every full screen shares (the clock, the power item, the accessibility item, the row of status
/// items), so a switch between the picker and the password screen keeps each part as it is, with the pixels it drew.
const CLOCK_PART: &str = "clock-part";
const POWER_PART: &str = "power-part";
const ACCESS_PART: &str = "access-part";
const STATUS_PART: &str = "status-part";
const NOTICES_PART: &str = "notices-part";
const LOCK_MESSAGE_PART: &str = "lock-message-part";
/// The key of the picker's tiles, a part of their own.
const TILES_PART: &str = "tiles-part";

/// The strip of the screen whose control material a field or button at `rect` stands on: as wide as the shake takes it,
/// so the material is made once for the control at rest and is the screen's under it wherever it is.
fn strip_of(rect: Rect) -> Rect {
    rect.inset(Insets {
        top: 0.0,
        left: -SHAKE_AMPLITUDE,
        bottom: 0.0,
        right: -SHAKE_AMPLITUDE,
    })
}

/// The material of the screen's `strip` laid with its origin at `origin`, as the screen was last given it.
fn material_of(pictures: &Shown, strip: Rect, origin: Point) -> FieldMaterial {
    FieldMaterial {
        strip,
        origin,
        laid: pictures.material(strip),
    }
}

/// Where the name field stands, above the password field, where a name is typed with the password.
fn name_box(layout: &Layout) -> Rect {
    layout
        .pill
        .translate(0.0, -(layout.pill.height() + CREDENTIAL_GAP))
}

/// The layout of a screen of `size` measured by `metrics`.
fn layout_of(size: Size, metrics: Metrics) -> Layout {
    Layout::new(
        size.width,
        size.height,
        metrics.ascent,
        metrics.descent,
        metrics.placeholder,
    )
}

/// The password screen of the lock screen and the greeter, or the greeter's name screen.
pub fn screen<M: Clone + 'static>(
    screen: &Screen<'_>,
    title: &str,
    map: fn(Input) -> M,
) -> Element<M> {
    let metrics = screen.metrics;
    let (width, height) = (screen.size.width, screen.size.height);
    let layout = Layout::new(
        width,
        height,
        metrics.ascent,
        metrics.descent,
        metrics.placeholder,
    );
    let pictures = screen.pictures;
    // the date and the large time, unless the settings leave them out
    let mut parts: Vec<Element<M>> = screen
        .large_clock
        .then(|| clock(screen.clock, &layout, metrics, screen.opacity, pictures))
        .into_iter()
        .collect();

    // the user block: the avatar, the name, the field and its buttons and the hints under it, one part
    let name_line = metrics.line(
        layout.center_x,
        NAME_MAX_WIDTH,
        layout.name_baseline,
        NAME_SIZE,
        1.0,
    );
    // the hints: the main one says what the last attempt came to, the sub one the password help while it is open (or the
    // refusal when the main one says the authenticator's message)
    let main_hint = screen
        .notice
        .or(screen.denied_said.then_some(DENIED))
        .filter(|_| !screen.locked);
    let sub_hint = if screen.help_open {
        Some((PASSWORD_HELP, false))
    } else {
        (screen.notice.is_some() && screen.denied_said).then_some((DENIED, true))
    };
    let main_rect = metrics.line(
        layout.center_x,
        HINT_WIDTH,
        layout.hint_baseline,
        HINT_SIZE,
        HINT_LINES,
    );
    let one_line = layout.sub_hint_baseline(1.0);
    let wrapped_by = layout.sub_hint_baseline(HINT_LINES) - one_line;
    let sub_rect = metrics.line(
        layout.center_x,
        SUB_HINT_WIDTH,
        one_line,
        SUB_HINT_SIZE,
        1.0,
    );
    let sub_rect = Rect::new(
        sub_rect.x(),
        sub_rect.y(),
        sub_rect.width(),
        sub_rect.height() + wrapped_by,
    );
    // the whole password group shakes: the field, its buttons and the slot; the avatar a little, in phase
    let shake = screen.shake;
    let reach = SHAKE_AMPLITUDE;
    let group = layout
        .pill
        .union(&layout.help_button)
        .union(&layout.back_button);
    // the name field, where a name is typed with the password, stands above the password field
    let name_box = name_box(&layout);
    let block = layout
        .avatar
        .outset(AVATAR_SHAKE_SHARE * SHAKE_AMPLITUDE)
        .union(&name_line.outset(SHADOW_REACH))
        .union(&name_box.inset(Insets {
            top: 0.0,
            left: -reach,
            bottom: 0.0,
            right: -reach,
        }))
        .union(&group.inset(Insets {
            top: 0.0,
            left: -reach,
            bottom: 0.0,
            right: -reach,
        }))
        .union(&main_rect)
        .union(&sub_rect);
    let at = block.origin;
    let mut children: Vec<Element<M>> = Vec::new();
    if !screen.hide_user {
        // the avatar is a part of its own, which moves in from the picker at full opacity
        let moving = screen.avatar_from.filter(|_| screen.block < 1.0);
        let place = moving
            .map_or(layout.avatar, |from| {
                between(from, layout.avatar, screen.block)
            })
            .translate(AVATAR_SHAKE_SHARE * shake, 0.0);
        let area = place.outset(1.0);
        let opacity = screen.opacity * if moving.is_some() { 1.0 } else { screen.block };
        parts.push(
            Element::new(Fade::new(
                area,
                opacity,
                vec![avatar(screen.user, within(place, area.origin), metrics)],
            ))
            .key("avatar"),
        );
        // the name gives way to the password help while it shows, and to the name field
        if !screen.help_open && screen.name.is_none() {
            let name = screen.user.map_or(OTHER_LABEL, |u| u.display.as_str());
            children.push(Element::new(Label {
                shadow: true,
                ..label(
                    name,
                    NAME_SIZE,
                    FontWeight::REGULAR,
                    within(name_line, at),
                    metrics,
                )
            }));
        }
    }

    let pill = layout.pill.translate(shake, 0.0);
    let slot = match (screen.spinner, screen.caps_lock, screen.locked) {
        (_, _, true) => Some((Adornment::Locked, ACCOUNT_LOCKED)),
        (Some(frame), _, _) => Some((Adornment::Spinner(frame), CHECKING)),
        (None, true, _) => Some((Adornment::CapsLock, CAPS_LOCK_ON)),
        (None, false, false) => None,
    };
    // the submit button and the slot take the end of the field, which the text is centred clear of
    let submit = if screen.typed && !screen.locked {
        SUBMIT_GAP + BUTTON_SIZE
    } else {
        0.0
    };
    let slot_room = if slot.is_some() {
        SLOT_WIDTH + SLOT_INSET
    } else {
        0.0
    };
    let Field::Password {
        generation,
        enabled,
        busy,
    } = screen.field;
    // with a name field the two fields keep their text at the leading side, 15 pt in
    let both = screen.name.is_some();
    let field = Element::new(BareSecureField {
        field: SecureField {
            label: PASSWORD_LABEL.to_owned(),
            placeholder: if screen.locked {
                LOCKED_PLACEHOLDER
            } else {
                PASSWORD_PLACEHOLDER
            }
            .to_owned(),
            enabled: enabled && !screen.locked,
            on_change: Some(Box::new(move |buffer| {
                map(Input::Edited(!buffer.is_empty()))
            })),
            on_submit: Some(Box::new(move |buffer| {
                map(Input::Submit(Typed::of(buffer)))
            })),
            busy,
            // the hint under the field says it, once
            invalid: screen.denied,
            // the field refuses what the authenticator would not take, so a submission is never cut
            max_len: Some(MAX_PASSWORD_BYTES),
            initial: false,
        },
        size: FIELD_TEXT_SIZE,
        weight: Some(WEIGHT_MEDIUM.0),
        placeholder_color: Some(placeholder_color(screen.access)),
        centered: !both,
        submit_width: submit + slot_room,
    });
    let key = format!("password-{generation}");
    let insets = if both {
        Insets {
            right: 0.0,
            ..CREDENTIAL_INSETS
        }
    } else {
        Insets::ZERO
    };
    // the name field takes the focus while it is empty, the password field once a name is there
    let name_first = screen.name.is_some_and(|name| name.text.is_empty());
    let round_button =
        |button: Rect, look: Look, label: &str, on_press: Input, kind: GlyphKind, key: &str| {
            // the material is made once for the button at rest, wide enough for the shake, and laid where it is
            let rest = button;
            let button = button.translate(shake, 0.0);
            let local = Rect::new(0.0, 0.0, button.width(), button.height());
            Element::new(Pressable {
                rect: within(button, at),
                look,
                label: label.to_owned(),
                description: None,
                on_press: map(on_press),
                on_focus: None,
                on_hover: None,
                focusable: true,
                children: vec![
                    Element::new(FieldFrame {
                        rect: local,
                        insets: Insets::ZERO,
                        material: Some(material_of(pictures, strip_of(rest), button.origin)),
                        tint: FieldTint::of(screen.access, false),
                        initial: false,
                        submit: None,
                        children: Vec::new(),
                    }),
                    Element::new(Glyph {
                        rect: local,
                        kind,
                        tint: FieldTint::of(screen.access, false),
                    }),
                ],
            })
            .key(key.to_owned())
        };
    // Tab goes from the back button to the field and on to the help button
    if screen.back {
        children.push(round_button(
            layout.back_button,
            Look::Help { open: false },
            BACK_LABEL,
            Input::Escape,
            GlyphKind::Back,
            "back",
        ));
    }
    if let Some(name) = screen.name {
        let name_rect = name_box.translate(shake, 0.0);
        children.push(
            Element::new(FieldFrame {
                rect: within(name_rect, at),
                insets: CREDENTIAL_INSETS,
                material: Some(material_of(pictures, strip_of(name_box), name_rect.origin)),
                tint: FieldTint::of(screen.access, !enabled),
                initial: name_first,
                submit: None,
                children: vec![Element::new(BareField {
                    // no menu and no paste where the session asks who logs in
                    menu: false,
                    field: TextField {
                        label: NAME_LABEL.to_owned(),
                        placeholder: NAME_PLACEHOLDER.to_owned(),
                        value: name.text.to_owned(),
                        revision: name.revision,
                        purpose: ContentPurpose::Name,
                        enabled,
                        on_change: Some(Box::new(move |change| map(Input::Name(change)))),
                        on_submit: Some(map(Input::SubmitName)),
                        selection: None,
                        initial: false,
                    },
                    size: FIELD_TEXT_SIZE,
                    hold_caret: false,
                    blink_start: None,
                    caret_layer: None,
                    on_blink: None,
                })],
            })
            .key("name"),
        );
    }
    let strip = strip_of(layout.pill);
    children.push(
        Element::new(FieldFrame {
            rect: within(pill, at),
            insets,
            material: Some(material_of(pictures, strip, pill.origin)),
            tint: FieldTint::of(screen.access, !enabled || screen.locked),
            initial: !name_first,
            submit: (submit > 0.0).then(|| within(layout.submit, layout.pill.origin)),
            children: vec![field],
        })
        .key(key.clone()),
    );
    if let Some((adornment, label)) = slot {
        children.push(Element::new(Slot {
            rect: within(
                Rect::new(
                    pill.max_x() - submit - SLOT_INSET - SLOT_WIDTH,
                    pill.y(),
                    SLOT_WIDTH,
                    pill.height(),
                ),
                at,
            ),
            adornment,
            label,
        }));
    }
    if screen.help {
        children.push(round_button(
            layout.help_button,
            Look::Help {
                open: screen.help_open,
            },
            HELP_LABEL,
            Input::Help,
            GlyphKind::Help,
            "help",
        ));
    }
    if let Some(main) = main_hint {
        children.push(Element::new(Label {
            wrap: true,
            live: Some(Live::Assertive),
            ..label(
                main,
                HINT_SIZE,
                FontWeight::REGULAR,
                within(main_rect, at),
                metrics,
            )
        }));
    }
    if let Some((sub, live)) = sub_hint {
        children.push(Element::new(Label {
            live: live.then_some(Live::Assertive),
            shift: main_hint.map(|main| Shift {
                text: main.to_owned(),
                width: HINT_WIDTH,
                size: HINT_SIZE,
                by: wrapped_by,
            }),
            ..label(
                sub,
                SUB_HINT_SIZE,
                FontWeight::REGULAR,
                within(sub_rect, at),
                metrics,
            )
        }));
    }
    // the group is a focus scope keyed by its field, so a new field takes the focus and Tab goes round the group
    parts.push(
        Element::new(Fade {
            scope: true,
            ..Fade::new(block, screen.opacity * screen.block, children)
        })
        .key(format!("block-{key}")),
    );

    // the lock message near the bottom, out of the way of the password help
    if let Some(message) = screen.lock_message.filter(|_| !screen.help_open) {
        let line = metrics.line(
            layout.center_x,
            LOCK_MESSAGE_WIDTH,
            layout.lock_message_baseline,
            LOCK_MESSAGE_SIZE,
            1.0,
        );
        parts.push(
            Element::new(Fade::new(
                line,
                screen.opacity,
                vec![Element::new(Label {
                    wrap: false,
                    ..label(
                        message,
                        LOCK_MESSAGE_SIZE,
                        FontWeight::REGULAR,
                        within(line, line.origin),
                        metrics,
                    )
                })
                .key("lock-message")],
            ))
            .key(LOCK_MESSAGE_PART),
        );
    }
    if let Some(menu) = screen.power {
        parts.push(power_item(menu, &layout, screen.opacity, map));
    }
    parts.extend(status_bar(
        screen.status,
        screen.power.is_some(),
        width,
        metrics,
        screen.opacity,
        map,
    ));
    parts.extend(notice_cards(
        screen.notices,
        screen.size,
        metrics,
        screen.opacity,
        pictures,
    ));
    Element::new(Backdrop {
        ground: Some(Ground {
            size: screen.size,
            picture: pictures.ground(),
        }),
        hidden: screen.hidden,
        on_activity: Some(map(Input::Activity)),
        keys: field_keys(screen.name.is_some(), map),
        label: title.to_owned(),
        description: screen.user.map(|u| u.display.clone()),
        role: Role::Group,
        modal: false,
        children: parts,
    })
}

/// The cards of the lock screen's notifications in a column at the top right, and the line that counts those not
/// shown, a part of its own. A card shows only what the daemon sent: an application and "Notification", or a title and
/// body it allowed.
fn notice_cards<M: 'static>(
    notices: &[Notice],
    size: Size,
    metrics: Metrics,
    opacity: f64,
    pictures: &Shown,
) -> Option<Element<M>> {
    let strip = notices_strip(notices, size, metrics)?;
    let (cards, more) = lock_notices::shown(notices);
    let column = lock_notices::column(size.width, size.height);
    let mut children: Vec<Element<M>> = cards
        .iter()
        .enumerate()
        .map(|(index, n)| {
            Element::new(NoticeCard {
                app: n.app.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
                speech: n.speech(),
                ascent: metrics.ascent,
                descent: metrics.descent,
                material: Some(FieldMaterial {
                    strip,
                    origin: column.origin,
                    laid: pictures.notices(),
                }),
                above: cards[..index].iter().map(|c| c.body.clone()).collect(),
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
    let place = Element::new(Place::text(
        Rect::new(0.0, 0.0, column.width(), column.height()),
        children,
    ))
    .key("notices");
    Some(Element::new(Fade::new(column, opacity, vec![place])).key(NOTICES_PART))
}

/// Everything the user picker shows.
pub struct PickerScreen<'a> {
    pub size: Size,
    pub metrics: Metrics,
    pub clock: &'a Clock,
    pub picker: &'a Picker,
    pub max_per_page: usize,
    /// The rows lay out right to left.
    pub rtl: bool,
    /// The date and the large time show.
    pub large_clock: bool,
    /// While the picker comes back from a password screen (`block` below 1): the tile whose avatar moves from the
    /// password screen's avatar, here, to its place in the row, at full opacity, as the rest fades in.
    pub avatar_from: Option<(usize, Rect)>,
    pub power: Option<&'a Menu>,
    /// The status bar's other items.
    pub status: Status<'a>,
    /// Of everything but the wallpaper, and of the tiles on top of it (the switch from the password screen).
    pub opacity: f64,
    pub block: f64,
    /// What the screen stands on, and its dimming's opacity (0 to 1).
    pub wallpaper: &'a Source,
    pub dim: f64,
    /// The pictures the screen was last given ([`ScreenPictures::shown`]).
    pub pictures: &'a Shown,
}

/// The element key of picker tile `index`, which the focus follows.
pub fn tile_key(index: usize) -> Key {
    Key::from(format!("tile-{index}"))
}

/// The greeter's user picker: the page of tiles with the selected one, under the clock.
pub fn picker<M: Clone + 'static>(screen: &PickerScreen<'_>, map: fn(Input) -> M) -> Element<M> {
    let metrics = screen.metrics;
    let (width, height) = (screen.size.width, screen.size.height);
    let layout = Layout::new(
        width,
        height,
        metrics.ascent,
        metrics.descent,
        metrics.placeholder,
    );
    let pictures = screen.pictures;
    // the date and the large time, unless the settings leave them out
    let mut parts: Vec<Element<M>> = screen
        .large_clock
        .then(|| clock(screen.clock, &layout, metrics, screen.opacity, pictures))
        .into_iter()
        .collect();
    let picker = screen.picker;
    let page = picker.page(screen.max_per_page);
    let grid = Grid::for_count(screen.max_per_page.min(picker.tiles().len()));
    let frames = layout.tiles(grid, page.len(), screen.rtl);
    let row_avatar = layout.row_avatar(screen.rtl);
    let (name_x, name_width, name_baseline) = layout.row_name(screen.rtl);
    let area = frames
        .iter()
        .copied()
        .reduce(|a, b| a.union(&b))
        .unwrap_or(layout.avatar)
        .outset(SHADOW_REACH);
    let mut tiles = Vec::new();
    // the tile whose avatar moves back to it from the password screen, drawn in a part of its own
    let moving = screen.avatar_from.filter(|_| screen.block < 1.0);
    for (frame, index) in frames.into_iter().zip(page) {
        let user = match &picker.tiles()[index] {
            Tile::Account(user) => Some(user),
            Tile::Other => None,
        };
        let name = user.map_or(OTHER_LABEL, |u| u.display.as_str());
        let selected = picker.selection() == Some(index);
        let name_line = metrics.line(
            name_x + name_width / 2.0,
            name_width,
            name_baseline,
            NAME_SIZE,
            1.0,
        );
        let tile = Pressable {
            rect: within(frame, area.origin),
            look: Look::Tile { selected },
            label: name.to_owned(),
            description: user.filter(|u| u.logged_in).map(|_| LOGGED_IN.to_owned()),
            on_press: map(Input::Choose(index)),
            on_focus: Some(map(Input::Focused(index))),
            on_hover: Some(map(Input::Select(index))),
            focusable: true,
            children: vec![
                avatar(user, row_avatar, metrics),
                Element::new(Decorative(Label {
                    shadow: true,
                    align: if screen.rtl {
                        TextAlign::Right
                    } else {
                        TextAlign::Left
                    },
                    ..label(name, NAME_SIZE, WEIGHT_SEMIBOLD, name_line, metrics)
                })),
            ],
        };
        let mut tile = tile;
        if let Some((_, from)) = moving.filter(|(i, _)| *i == index) {
            tile.children.remove(0);
            let place = between(
                from,
                row_avatar.translate(frame.x(), frame.y()),
                screen.block,
            );
            let area = place.outset(1.0);
            parts.push(
                Element::new(Fade::new(
                    area,
                    screen.opacity,
                    vec![avatar(user, within(place, area.origin), metrics)],
                ))
                .key("avatar"),
            );
        }
        tiles.push(Element::new(tile).key(tile_key(index)));
    }
    parts.push(
        Element::new(Fade {
            initial: true,
            ..Fade::new(area, screen.opacity * screen.block, tiles)
        })
        .key(TILES_PART),
    );
    if let Some(menu) = screen.power {
        parts.push(power_item(menu, &layout, screen.opacity, map));
    }
    parts.extend(status_bar(
        screen.status,
        screen.power.is_some(),
        width,
        metrics,
        screen.opacity,
        map,
    ));
    let selected = picker.selected_index();
    Element::new(Backdrop {
        ground: Some(Ground {
            size: screen.size,
            picture: pictures.ground(),
        }),
        hidden: false,
        on_activity: None,
        keys: Box::new(move |keysym, _| {
            Some(map(match keysym {
                Keysym(0xff51) => Input::Step(Direction::Left),
                Keysym(0xff52) => Input::Step(Direction::Up),
                Keysym(0xff53) => Input::Step(Direction::Right),
                Keysym(0xff54) => Input::Step(Direction::Down),
                Keysym(0xff50) => Input::Jump(Jump::First),
                Keysym(0xff57) => Input::Jump(Jump::Last),
                Keysym(0xff55) => Input::Jump(Jump::PageBack),
                Keysym(0xff56) => Input::Jump(Jump::PageForward),
                OPTION_RETURN => Input::ToggleNames,
                Keysym(0xff0d) | Keysym(0xff8d) => Input::Choose(selected),
                _ => return None,
            }))
        }),
        label: GREETER_TITLE.to_owned(),
        description: None,
        role: Role::Group,
        modal: false,
        children: parts,
    })
}

/// The rectangle `t` of the way from `from` to `to` (0 to 1), its place and its size.
fn between(from: Rect, to: Rect, t: f64) -> Rect {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        mix(from.x(), to.x()),
        mix(from.y(), to.y()),
        mix(from.width(), to.width()),
        mix(from.height(), to.height()),
    )
}

/// Where the avatar of the password and name screens stands on a screen of `size`.
pub fn user_avatar(size: Size, metrics: Metrics) -> Rect {
    Layout::new(
        size.width,
        size.height,
        metrics.ascent,
        metrics.descent,
        metrics.placeholder,
    )
    .avatar
}

/// Where the avatar of tile `index` of `picker` stands on a screen of `size`, while its page shows.
pub fn tile_avatar(
    size: Size,
    metrics: Metrics,
    picker: &Picker,
    max_per_page: usize,
    rtl: bool,
    index: usize,
) -> Option<Rect> {
    let layout = Layout::new(
        size.width,
        size.height,
        metrics.ascent,
        metrics.descent,
        metrics.placeholder,
    );
    let page = picker.page(max_per_page);
    let grid = Grid::for_count(max_per_page.min(picker.tiles().len()));
    let frames = layout.tiles(grid, page.len(), rtl);
    let at = page.clone().position(|i| i == index)?;
    let frame = frames.get(at)?;
    Some(layout.row_avatar(rtl).translate(frame.x(), frame.y()))
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
