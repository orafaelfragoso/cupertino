//! The screens as CupertinoKit draws them: the password screen of the lock screen and the greeter, the greeter's name
//! screen, user picker and power menu, and the session agent's confirmation and interrupt panels. Each function builds
//! a window's content from the state machines' state; what the person does comes back as an [`Input`], which the
//! process maps into its own messages.
//!
//! The password field is CupertinoKit's secure field: its text lives only in the field's zeroed buffer, assistive
//! technology sees a password field with a protected value, and diagnostics see bullets. The field is keyed by
//! [`LockScreen::field`](crate::model::LockScreen::field): a submission or Escape makes a new field, so the old one's
//! buffer is dropped and zeroed, and the new one takes the focus on every output.

mod panel_parts;
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
    AttributedText, ContentPurpose, FontWeight, SecureBuffer, TextAlign, TextStyle, TextSystem,
};
use cupertino_kit::theme::Color;

use crate::interrupt::{self, Choice, Reason, Stage};
use crate::layout::{
    Layout, BUTTON_SIZE, HINT_LINES, HINT_SIZE, HINT_WIDTH, LOCK_MESSAGE_SIZE, LOCK_MESSAGE_WIDTH,
    NAME_MAX_WIDTH, NAME_SIZE, STATUS_BAR_HEIGHT, SUBMIT_GAP, SUB_HINT_SIZE, SUB_HINT_WIDTH,
};
use crate::lock_notices::{self, Notice};
use crate::model::{Key as PanelKey, Password, MAX_PASSWORD_BYTES, SHAKE_AMPLITUDE};
use crate::panel::{Decision, Frames, Kind, Panel};
use crate::picker::{Direction, Grid, Jump, Picker, Tile};
use crate::power::{self, Menu};
use crate::status::{StatusItems, WifiState};
use crate::user::{initials, User};
use crate::wallpaper::Source;
pub use widgets::Access;
use widgets::{
    Adornment, AlertBody, Avatar, Backdrop, Fade, FieldFrame, FieldMaterial, FieldTint, GlassText,
    Glyph, GlyphKind, Ground, Label, Look, NoticeCard, Place, Prepare, Pressable, Shift, Slot,
    StatusItem, StatusKind, StatusRow, ESCAPE, OPTION_RETURN, WHITE,
};

/// The `app_id` of a lock surface's window, followed by `.` and the output's registry name: the surface roles give it
/// the output's lock surface.
pub const LOCK_APP_ID: &str = "cupertino-login.lock";
/// The `app_id` of a greeter window, followed by `.` and the output's registry name: a full-output layer surface.
pub const GREETER_APP_ID: &str = "cupertino-login.greeter";
/// The `app_id` of the confirmation panel's and of the interrupt panel's window: a layer surface above the windows,
/// centred when it appears, that moves when its background is dragged and takes the keyboard while it is used.
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
/// Panel title and info: the title's weight; the icon disc's fill (approximate, a neutral grey).
const PANEL_ICON_FILL: Color = Color::new(0.60, 0.62, 0.66, 1.0);
/// Text size the panels' title and lines are measured at; the roles set the size they draw at.
const PANEL_TITLE_SIZE: f32 = 13.0;
/// Corner radius of the alert of the confirmation and interrupt panels.
const PANEL_ALERT_RADIUS: f64 = 26.0;

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
    /// A button or key of the confirmation panel.
    Decide(Decision),
    /// The confirmation panel's "Open my windows again" checkbox was set to this, by a click, Space or assistive technology.
    Reopen(bool),
    /// A choice on the native desktop-control consent panel.
    DesktopControl(ControlChoice),
    /// A button or key of the interrupt panel.
    Interrupt(Choice),
    /// The background of a session panel was dragged: move the panel by this many points.
    MovePanel(Point),
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

/// The clock at the top of a screen, a part of its own just its size.
/// The date in solid white and the time below it in glass letters made from `ground`'s picture.
fn clock<M: 'static>(
    clock: &Clock,
    layout: &Layout,
    metrics: Metrics,
    opacity: f64,
    ground: &Ground,
) -> Element<M> {
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
                text: clock.time.clone(),
                size: layout.time_size,
                weight: WEIGHT_TIME,
                rect: within(time, rect.origin),
                ascent: metrics.ascent * layout.time_size,
                ground: ground.clone(),
                origin: rect.origin,
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
/// The key of the picker's tiles, a part of their own, and of what makes the password screen's material ahead of it.
const TILES_PART: &str = "tiles-part";
const PREPARE_PART: &str = "prepare-part";

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

/// The ground of a screen of `size` on `wallpaper` with its dimming at `dim`.
fn ground(wallpaper: &Source, size: Size, dim: f64) -> Ground {
    Ground {
        source: wallpaper.clone(),
        size,
        dim,
    }
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
    let ground = ground(screen.wallpaper, screen.size, screen.dim);
    // the date and the large time, unless the settings leave them out
    let mut parts: Vec<Element<M>> = screen
        .large_clock
        .then(|| clock(screen.clock, &layout, metrics, screen.opacity, &ground))
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
    let name_box = layout
        .pill
        .translate(0.0, -(layout.pill.height() + CREDENTIAL_GAP));
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
                        material: Some(FieldMaterial {
                            ground: ground.clone(),
                            strip: strip_of(rest),
                            origin: button.origin,
                        }),
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
                material: Some(FieldMaterial {
                    ground: ground.clone(),
                    strip: strip_of(name_box),
                    origin: name_rect.origin,
                }),
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
            material: Some(FieldMaterial {
                ground: ground.clone(),
                strip,
                origin: pill.origin,
            }),
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
        &ground,
    ));
    Element::new(Backdrop {
        ground: Some(ground),
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
    ground: &Ground,
) -> Option<Element<M>> {
    if notices.is_empty() {
        return None;
    }
    let (cards, more) = lock_notices::shown(notices);
    let column = lock_notices::column(size.width, size.height);
    // the material of the cards covers the room the shown cards may take at most, made once for all of them
    let tallest =
        lock_notices::card_height(lock_notices::BODY_LINES, metrics.ascent, metrics.descent)
            + lock_notices::CARD_GAP;
    let strip = Rect::new(
        column.x(),
        column.y(),
        column.width(),
        (tallest * cards.len() as f64).min(column.height()),
    );
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
                    ground: ground.clone(),
                    strip,
                    origin: column.origin,
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
    let ground = ground(screen.wallpaper, screen.size, screen.dim);
    // the date and the large time, unless the settings leave them out
    let mut parts: Vec<Element<M>> = screen
        .large_clock
        .then(|| clock(screen.clock, &layout, metrics, screen.opacity, &ground))
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
    // the password screen's field and back button stand on material made while the picker rests
    parts.push(
        Element::new(Prepare {
            ground: ground.clone(),
            strips: vec![strip_of(layout.pill), strip_of(layout.back_button)],
        })
        .key(PREPARE_PART),
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
        ground: Some(ground),
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

/// How many pictures of a whole screen this thread made from their wallpaper: once per screen size, scale and
/// wallpaper, and once more as the dimming has come in. What a frame of a typed character, a hover or a fade draws
/// leaves it as it is.
pub fn pictures_made() -> u64 {
    widgets::pictures_made()
}

/// Drop the pictures the screens keep: when the lock screen ends or the greeter hands over to a session, so a screen's
/// memory is given back.
pub fn forget_pictures() {
    widgets::forget_pictures();
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

/// The style the panels' title, lines and checkbox label are measured in; a countdown also sets its digits at one width
/// each (`tnum`), so the line keeps its length and does not shift as the number changes.
fn panel_style(weight: FontWeight, tabular: bool) -> TextStyle {
    TextStyle {
        weight,
        features: if tabular {
            vec![cupertino_kit::text::FontFeature {
                tag: *b"tnum",
                value: 1,
            }]
        } else {
            Vec::new()
        },
        ..TextStyle::new(PANEL_TITLE_SIZE)
    }
}

/// How many lines `text` takes in `style`, wrapped at `width`.
fn lines_in(text_system: &TextSystem, text: &str, style: TextStyle, width: f64) -> u32 {
    let content = AttributedText::plain(text, style);
    let paragraph = cupertino_kit::text::ParagraphStyle {
        wrap: cupertino_kit::text::Wrap::Word,
        ..Default::default()
    };
    text_system.paragraph(&content, &paragraph).map_or(1, |p| {
        p.layout(Some(width as f32)).lines().len().max(1) as u32
    })
}

/// How wide `text` is on one line in `style`.
fn line_width(text_system: &TextSystem, text: &str, style: TextStyle) -> f64 {
    let content = AttributedText::plain(text, style);
    text_system
        .paragraph(&content, &Default::default())
        .map_or(0.0, |p| p.layout(None).size().width)
}

/// The confirmation panel's frames for `kind`, its checkbox label, title and info line measured with `text_system`:
/// the panel as wide as its checkbox row needs, and the info line as many lines as its longest countdown message.
pub fn panel_frames(kind: Kind, text_system: &TextSystem) -> Frames {
    let label = line_width(
        text_system,
        crate::panel::REOPEN_LABEL,
        panel_style(FontWeight::REGULAR, false),
    );
    let width = crate::panel::width_for(label);
    let wrap = crate::panel::wrap_width(width);
    let title = lines_in(
        text_system,
        kind.text().title,
        panel_style(FontWeight::BOLD, false),
        wrap,
    );
    let info = (0..=crate::panel::COUNTDOWN_SECONDS as u32)
        .filter_map(|seconds| kind.countdown_message(seconds))
        .map(|message| {
            lines_in(
                text_system,
                &message,
                panel_style(FontWeight::REGULAR, true),
                wrap,
            )
        })
        .max()
        .unwrap_or(1);
    kind.frames(width, title, info)
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
    styled_panel_text(text, role, panel_style(weight, false), secondary)
}

fn styled_panel_text<M: 'static>(
    text: &str,
    role: TextRole,
    style: TextStyle,
    secondary: bool,
) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, style),
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

/// The picture `icon` of a session panel at `rect`: our own glyph on a grey disc.
fn panel_icon<M: 'static>(rect: Rect, icon: crate::panel::Icon) -> Element<M> {
    Element::new(Place::new(
        rect,
        vec![Element::new(Canvas {
            size: Some(rect.size),
            label: String::new(),
            paint: Box::new(move |cx| {
                for op in panel_parts::icon_ops(icon, rect.width()) {
                    cx.push(op);
                }
            }),
            children: Vec::new(),
        })],
    ))
}

/// The confirmation panel, an alert on glass: the picture of its action, its title, the countdown line (its digits
/// of one width), the "Open my windows again" checkbox at `reopen`, which has the focus, and its buttons as capsules. Return
/// does the default action, Escape cancels, Space flips the checkbox, and the power-button panel's letters choose. A
/// press on its background that no control takes drags it ([`Input::MovePanel`]); it is no modal dialog, so the rest
/// of the session stays usable while it counts down.
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
        panel_icon(frames.icon, kind.icon()),
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
            vec![styled_panel_text(
                &info,
                TextRole::Body,
                panel_style(FontWeight::REGULAR, true),
                false,
            )],
        )));
    }
    let checkbox = frames.checkbox;
    children.push(
        // against the text column's leading edge; it has the focus when the panel appears
        Element::new(Place {
            initial: true,
            ..Place::text(
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
            )
        })
        .key("reopen"),
    );
    let default = Decision::Do(kind.default_action());
    for button in &frames.buttons {
        children.push(
            Element::new(Place::filled(
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
            ))
            .key(button.label),
        );
    }
    let content = Element::new(Backdrop {
        ground: None,
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
        modal: false,
        children: vec![Element::new(panel_parts::Movable {
            on_move: Box::new(move |by| map(Input::MovePanel(by))),
            children,
        })],
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
        ground: None,
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

/// What the interrupt panel shows: the application, why it is still there, the question asked and what it stops, and
/// the text system its words are measured with.
#[derive(Clone, Copy)]
pub struct InterruptView<'a> {
    pub app: &'a str,
    pub reason: Reason,
    pub stage: Stage,
    pub action: crate::panel::Action,
    pub text: &'a TextSystem,
}

/// The interrupt panel's frames, its title and info line wrapped in its text column as `text` lays them out.
pub fn interrupt_frames(shown: &InterruptView<'_>) -> interrupt::Frames {
    let InterruptView {
        app,
        reason,
        stage,
        action,
        text,
    } = *shown;
    let lines = |words: &str, weight| {
        lines_in(
            text,
            words,
            panel_style(weight, false),
            interrupt::WRAP_WIDTH,
        )
    };
    interrupt::Frames::new(
        reason,
        stage,
        lines(
            &interrupt::title(reason, stage, action, app),
            FontWeight::BOLD,
        ),
        lines(
            &interrupt::info(reason, stage, action, app),
            FontWeight::REGULAR,
        ),
    )
}

/// The interrupt panel, on the glass of the confirmation it interrupts: the picture of what it stops, why, and its
/// buttons as capsules: Try Again (the default), Cancel and, for an application that failed or hangs, Force Quit, which
/// asks again before it kills. On that second question neither button is the default and Cancel has the focus, so a
/// Return held from typing kills nothing. It moves when its background is dragged and is no modal dialog.
pub fn interrupt<M: Clone + 'static>(
    key: WindowKey,
    shown: InterruptView<'_>,
    map: fn(Input) -> M,
) -> Window<M> {
    let frames = interrupt_frames(&shown);
    let InterruptView {
        app,
        reason,
        stage,
        action,
        ..
    } = shown;
    let title = interrupt::title(reason, stage, action, app);
    let icon = match action {
        crate::panel::Action::Restart => crate::panel::Icon::Restart,
        crate::panel::Action::ShutDown => crate::panel::Icon::Power,
        crate::panel::Action::LogOut | crate::panel::Action::Sleep => crate::panel::Icon::LogOut,
    };
    let mut children = vec![
        Element::new(AlertBody {
            width: interrupt::WIDTH,
            height: Some(frames.height),
            radius: PANEL_ALERT_RADIUS,
            padding: Insets::ZERO,
            children: Vec::new(),
        }),
        panel_icon(frames.icon, icon),
        Element::new(Place::text(
            frames.title,
            vec![panel_text(&title, TextRole::Body, FontWeight::BOLD, false)],
        )),
        Element::new(Place::text(
            frames.info,
            vec![panel_text(
                &interrupt::info(reason, stage, action, app),
                TextRole::Caption,
                FontWeight::REGULAR,
                true,
            )],
        )),
    ];
    // the first question focuses its default button; the force quit question focuses Cancel
    let focused = frames
        .buttons
        .iter()
        .find(|b| b.default)
        .or_else(|| frames.buttons.iter().find(|b| b.choice == Choice::Back))
        .map(|b| b.choice);
    let mut question = Vec::new();
    for button in &frames.buttons {
        question.push(
            Element::new(Place {
                initial: Some(button.choice) == focused,
                ..Place::filled(
                    button.frame,
                    vec![Element::new(Button {
                        label: button.label.to_owned(),
                        role: if button.default {
                            ButtonRole::Default
                        } else {
                            ButtonRole::Normal
                        },
                        style: ButtonStyle::Pill,
                        enabled: true,
                        busy: false,
                        on_press: Some(map(Input::Interrupt(button.choice))),
                    })],
                )
            })
            .key(format!("{stage:?}-{}", button.label)),
        );
    }
    // each question its own focus scope, so the second one starts at its Cancel and the first one's focus comes back
    // after it
    children
        .push(Element::new(panel_parts::Question { children: question }).key(format!("{stage:?}")));
    let content = Element::new(Backdrop {
        ground: None,
        hidden: false,
        on_activity: None,
        keys: Box::new(move |keysym, text| {
            panel_key(keysym, text)
                .and_then(|key| interrupt::press(stage, key))
                .map(|choice| map(Input::Interrupt(choice)))
        }),
        label: title.clone(),
        description: None,
        role: Role::AlertDialog,
        modal: false,
        children: vec![Element::new(panel_parts::Movable {
            on_move: Box::new(move |by| map(Input::MovePanel(by))),
            children,
        })],
    });
    Window {
        background: WindowBackground::None,
        ..full_screen(
            key,
            INTERRUPT_APP_ID.to_owned(),
            &title,
            Size::new(interrupt::WIDTH, frames.height),
            content,
            Some(map(Input::Interrupt(match stage {
                Stage::Ask => Choice::Cancel,
                Stage::ConfirmForceQuit => Choice::Back,
            }))),
        )
    }
}
