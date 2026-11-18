//! What each pane shows and what its controls write (§3, §6.3). Every control names
//! the key or call that backs it ([`Backend`]); a control without one is not drawn (ST-09). The row order inside a pane
//! is Original (by frequency of use) until O4 recovers the design's.

use cupertino_defaults::accessibility::{
    AccessibilitySettings, ColorFilter, DisplayOption, DisplayOptions, SYSTEM_KEYS,
};
use cupertino_defaults::appearance::Accent;
use cupertino_defaults::compositor::{MinimizeEffectName, ScrollBarsName};
use cupertino_defaults::desktop_control::{App, Decision, Service};
use cupertino_defaults::dock::{ActionTarget, Position, Trigger};
use cupertino_defaults::keyboard::{FnKey, ModifierProfile};
use cupertino_defaults::menubar::{HourCycle, ShowDate};
use cupertino_defaults::notifications::{valid_app_id, Previews};
use cupertino_defaults::power::Timer;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cupertino_assets::WallpaperFile;

use crate::about;
use crate::clipboard::{self, State};
use crate::controls::{Control, SwatchKind, Tile, Value, MUTE_BELOW};
use crate::desktop_control;
use crate::live::{
    display_name, duration_text, BatteryState, Bluetooth, Live, LoginScreen, Mouse, Reading, Wifi,
};
use crate::mapping;
use crate::pane::PaneId;
use crate::pane_extras;
use crate::screen_sharing;
use crate::settings::{File, Settings};

/// A control of a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlId {
    /// A read-only row of a live reading (battery, network devices): it changes nothing.
    Reading,
    Appearance,
    ShowScrollBars,
    /// What a click in a scroll bar's track off the knob does.
    ScrollClick,
    /// The time the dark look starts, and the time the light look starts, while the look follows the time of day.
    AutoDarkFrom,
    AutoLightFrom,
    AccentColor,
    HighlightColor,
    SidebarIconSize,
    /// What a double click on a window's title bar does.
    DoubleClickTitle,
    /// The menu bar hides until the pointer reaches the top edge.
    MenuBarAutoHide,
    /// Dragging a window to a screen edge tiles it.
    EdgeTiling,
    /// Tiled windows keep a margin around and between them.
    TileMargins,
    /// How much denser every material is made.
    MaterialDensity,
    /// Windows take a slight tint of the wallpaper's colour.
    TintWindows,
    /// The solid colours the desktop can show instead of a picture, by index of `WALLPAPER_COLORS`.
    WallpaperColor,
    MinimizeIntoIcon,
    LaunchBounce,
    RunningIndicators,
    StackStyle,
    DefaultBrowser,
    MenuBarBackground,
    ShowClock,
    ShowWifi,
    ShowBluetooth,
    ShowBattery,
    ShowFocus,
    RecentDocuments,
    RecentApps,
    /// A status icon of `pane_extras::tray_list`, by index: whether it shows in the bar.
    TrayItem(usize),
    /// A program of `Live::login_items`, by index: whether it opens at login.
    LoginItem(usize),
    DockSize,
    Magnification,
    DockPosition,
    MinimizeEffect,
    Recents,
    AutoHide,
    HotCorner(Trigger),
    ClockSeconds,
    ClockDayOfWeek,
    ClockDate,
    ClockAmPm,
    ClockFlash,
    Clock24,
    RepeatRate,
    RepeatDelay,
    MouseSpeed,
    /// The pictures of group `n` of `Live::pictures`: 0 the packs', then one per added folder.
    Wallpaper(usize),
    /// Take the added folder `n` of `Live::pictures` off the list.
    WallpaperRemoveFolder(usize),
    /// The buttons that add a folder (0) or a picture (1).
    WallpaperAdd,
    /// A mouse of `Live::mice`, by index: its resolution.
    MouseResolution(usize),
    MouseAcceleration,
    MouseNatural,
    TrackpadSpeed,
    TrackpadAcceleration,
    TrackpadNatural,
    ModifierProfile,
    CapsLockDelay,
    CapsLockSwitch,
    FnKey,
    AccentPicker,
    ReduceMotion,
    /// The text cursor stays shown instead of blinking (`steady_caret`).
    SteadyCaret,
    ReduceTransparency,
    IncreaseContrast,
    DifferentiateWithoutColor,
    InvertColors,
    DisplayContrast,
    ColorFilter,
    ColorFilterIntensity,
    /// Use Mine: the person's allow-listed display options become the machine's defaults.
    LoginScreen,
    /// The session's screen reader on or off (`screen_reader`, which the server runs).
    ScreenReader,
    CursorSize,
    CursorOutline,
    CursorFill,
    ShakeToLocate,
    StickyKeys,
    StickyKeysShortcut,
    SlowKeys,
    MouseKeys,
    MouseKeysShortcut,
    DisplaySleepBattery,
    DisplaySleepAc,
    RequirePassword,
    SystemSleepBattery,
    SystemSleepAc,
    DoNotDisturb,
    /// A row of `FocusLifetimes`, by index.
    FocusFor(usize),
    Volume,
    Mute,
    /// The size a display of `Live::displays` looks like, by index: one of its "looks like" choices.
    DisplaySize(usize),
    /// The sound output, by index of `Live::outputs`.
    Output,
    Brightness,
    WifiPower,
    /// A network of `GetNetworks`, by index: Join or Disconnect.
    WifiNetwork(usize),
    BluetoothPower,
    /// A device of `Devices`, by index: its buttons.
    BluetoothDevice(usize),
    /// An entry of `data-control.conf`, by index: Remove, or Allow Again and Remove once its file changed.
    ClipboardApp(usize),
    /// A line of `data-control.conf` that allows nothing, by index: Remove.
    ClipboardUnusable(usize),
    /// The buttons that add an application (0) or a program (1).
    ClipboardAdd,
    /// A grant of `screen-sharing.conf`, by index: Forget.
    ScreenSharingGrant(usize),
    /// An app of `desktop-control.conf` for a service, by index among that service's: its switch (on allows, off
    /// denies) and Forget.
    DesktopControlApp(Service, usize),
    /// Whether notifications show on the lock screen, for every application without its own choice.
    NotificationsLockScreen,
    /// How much of a notification shows, for every application without its own choice.
    NotificationsPreviews,
    /// An application of [`notification_apps`], by index: its own choice of [`ControlId::NotificationsLockScreen`].
    NotificationAppLockScreen(usize),
    /// An application of [`notification_apps`], by index: its own choice of [`ControlId::NotificationsPreviews`].
    NotificationAppPreviews(usize),
    /// A control of the networks, battery, sound, display, search and notification options.
    More(crate::pane_more::More),
}

/// What a control's value reaches the system through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// `org.cupertino.Session1.SetAppearance`.
    SetAppearance,
    /// `org.cupertino.Session1.SetAccessibility` with this key of `accessibility.conf` (the server is its only writer).
    SetAccessibility(&'static str),
    /// A key of a settings file.
    Key(File, &'static str),
    /// A method or property of a live service (named as on the bus).
    Service(&'static str),
}

/// One row: the label, the control and its backend, the error of the last refused change, and why the control is
/// disabled (a missing daemon or device, §6.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: ControlId,
    pub label: String,
    pub control: Control,
    pub backend: Backend,
    pub error: Option<String>,
    pub disabled: Option<String>,
    /// The application icon before the label: its icon name, the generic application icon when that has none.
    pub icon: Option<String>,
    /// A second line under the label in secondary text, when there is no error or disabled reason.
    pub detail: Option<String>,
}

/// One section of a pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub header: Option<String>,
    pub rows: Vec<Row>,
    pub footer: Option<String>,
}

/// What a committed change asks the process to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Write the file (its typed settings were changed in place).
    Store(File),
    /// Ask the server for the appearance.
    SetAppearance { dark: bool },
    /// Store `appearance.conf` (its automatic switch changed), then ask the server for the look `dark` when one is
    /// due now.
    SetTheme { dark: Option<bool> },
    /// Turn the login item `id` on or off.
    SetLoginItem { id: String, enable: bool },
    /// Make the browser with desktop file `id` the default.
    SetDefaultBrowser { id: String },
    /// Ask the server to set a key of `accessibility.conf` to a value as the file writes it.
    SetAccessibility { key: &'static str, value: String },
    /// Ask sessiond to make [`login_screen_options`] the machine's accessibility defaults.
    SetSystemAccessibility,
    /// A call on a live service.
    Service(Call),
    /// A session Audio1 operation, queued without waiting for the bus.
    Audio(AudioChange),
    /// Ask for a folder (`folder`) or a picture in the chooser.
    AskPicture { folder: bool },
    /// Ask for a resolution of the mouse `key` called `name` in a sheet.
    AskResolution { key: String, name: String },
    /// Ask before `program`, shown as `name`, can no longer use the clipboard.
    ConfirmUnallow { program: PathBuf, name: String },
    /// Choose an installed application (`program` false) or a program file to allow to use the clipboard.
    AskClipboardApp { program: bool },
    /// Take the screen-sharing grant with this id out of `screen-sharing.conf`, as the file holds it now
    /// ([`Settings::forget_grant`](crate::settings::Settings::forget_grant)): the portal adds to the file while the
    /// pane is open, so the pane's own list is never written back.
    ForgetGrant(String),
    /// Record this answer of an app in `desktop-control.conf`, as the file holds it now
    /// ([`Settings::decide_app`](crate::settings::Settings::decide_app)): the compositor adds to the file while the
    /// pane is open, so the pane's own list is never written back.
    DecideApp {
        service: Service,
        app: App,
        decision: Decision,
    },
    /// Take the answer of an app out of `desktop-control.conf`, so it asks again
    /// ([`Settings::forget_app`](crate::settings::Settings::forget_app)).
    ForgetApp { service: Service, app: App },
    /// Show the battery charts for the last ten days (true) or the last day.
    ChartSpan(bool),
    /// Show this tab of the page.
    Tab(usize),
    /// Ask for a folder the file search leaves out.
    AskExclusion,
    /// The change is refused for this reason; nothing changed.
    Refuse(String),
}

/// A change to the default audio devices.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioChange {
    SetVolume {
        level: f64,
        muted: bool,
    },
    /// Make the PipeWire sink with this id the output applications play to.
    SetOutput(u32),
    SetInput(u32),
    SetInputVolume {
        level: f64,
        muted: bool,
    },
    SetBalance(f64),
}

/// A call on a live service.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    SetDoNotDisturb(bool),
    SetDoNotDisturbFor(String),
    SetBrightness(f64),
    /// Set the display on `connector` to `scale` panel pixels per point (an output configuration of the server).
    SetDisplayScale {
        connector: String,
        scale: f64,
    },
    /// Set the refresh rate, in millihertz, of the display on `connector` at its current size.
    SetDisplayRefresh {
        connector: String,
        mhz: u32,
    },
    /// Move the display on `connector` so its top-left corner lies at (`x`, `y`) in the desktop.
    SetDisplayPosition {
        connector: String,
        x: i32,
        y: i32,
    },
    SetWirelessEnabled(bool),
    /// Join a network; `None`: ask for the passphrase first (a secured network that is not saved).
    Join {
        ssid: String,
        passphrase: Option<crate::sheet::Secret>,
    },
    Disconnect,
    /// Take a saved network, by its key, off the machine.
    ForgetNetwork(String),
    /// Let the machine join a saved network by itself, or stop it.
    SetAutoJoin {
        ssid: String,
        on: bool,
    },
    /// Connect the VPN with this identifier, or disconnect it.
    SetVpn {
        id: String,
        on: bool,
    },
    /// Turn the power-saving profile on or off.
    SetLowPower(bool),
    SetKeyboardBacklight(f64),
    SetBluetoothPowered(bool),
    Pair(String),
    Connect(String),
    DisconnectDevice(String),
    /// Forget a device; `confirmed` once the confirmation sheet said so (§6.4).
    Remove {
        id: String,
        confirmed: bool,
    },
}

impl Call {
    /// The call changes the displays through the display server.
    pub fn sets_displays(&self) -> bool {
        matches!(
            self,
            Call::SetDisplayScale { .. }
                | Call::SetDisplayRefresh { .. }
                | Call::SetDisplayPosition { .. }
        )
    }
}

/// The session state the panes show besides the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub dark: bool,
    /// The display options in effect (`cupertino_session_v1`).
    pub display: DisplayOptions,
}

/// The hot-corner choices: label and the dock's action (names from the server's and the dock's actions; the design's
/// list, names; default none).
pub const CORNER_ACTIONS: [(&str, &str); 9] = [
    ("None", "none"),
    ("Overview", "overview"),
    ("App Overview", "app-overview"),
    ("Show Desktop", "show-desktop"),
    ("App Grid", "app-grid"),
    ("Launcher", "launcher"),
    ("Lock Screen", "lock"),
    ("Notification Center", "notification-center"),
    ("Control Panel", "control-panel"),
];

/// The gestures of the trackpad that run an action, with the keys of the dock's file.
const GESTURES: [(Trigger, &str, &str); 4] = [
    (Trigger::SwipeUp, "Swipe up with three fingers", "swipe_up"),
    (
        Trigger::SwipeDown,
        "Swipe down with three fingers",
        "swipe_down",
    ),
    (Trigger::Pinch, "Pinch with four fingers", "pinch"),
    (Trigger::Spread, "Spread with four fingers", "spread"),
];

const CORNERS: [(Trigger, &str, &str); 4] = [
    (Trigger::CornerTopLeft, "Top Left", "corner_top_left"),
    (Trigger::CornerTopRight, "Top Right", "corner_top_right"),
    (
        Trigger::CornerBottomLeft,
        "Bottom Left",
        "corner_bottom_left",
    ),
    (
        Trigger::CornerBottomRight,
        "Bottom Right",
        "corner_bottom_right",
    ),
];

/// When applications show their scroll bars.
const SCROLL_BARS: [(&str, ScrollBarsName); 3] = [
    ("Automatically", ScrollBarsName::Automatic),
    ("When scrolling", ScrollBarsName::WhenScrolling),
    ("Always", ScrollBarsName::Always),
];

/// The minimize effects offered: Warp, Scale, Funnel (enum, `plain` is not offered).
const EFFECTS: [(&str, MinimizeEffectName); 3] = [
    ("Warp", MinimizeEffectName::Warp),
    ("Scale", MinimizeEffectName::Scale),
    ("Funnel", MinimizeEffectName::Funnel),
];
const POSITIONS: [(&str, Position); 3] = [
    ("Bottom", Position::Bottom),
    ("Left", Position::Left),
    ("Right", Position::Right),
];
const DATES: [(&str, ShowDate); 3] = [
    ("When Space Allows", ShowDate::WhenSpaceAllows),
    ("Always", ShowDate::Always),
    ("Never", ShowDate::Never),
];
/// The modifier profiles (labels approximate).
const PROFILES: [(&str, ModifierProfile); 2] = [
    ("Super is the command key", ModifierProfile::Cupertino),
    ("Ctrl is the command key", ModifierProfile::ClassicPc),
];
/// The fn key's tap actions offered: emoji and dictation have nothing to open yet, so they are not offered; a stored one
/// shows as an extra item (the spec's disabled rows are approximate this way).
const FN_ACTIONS: [(&str, FnKey); 2] = [
    ("Do Nothing", FnKey::DoNothing),
    ("Change Input Source", FnKey::ChangeInputSource),
];
/// The colour filters (types; labels from the design pane's popup, §6.3). The tint keeps its hue
/// when one is stored, else 0 (approximate: no colour well yet).
const FILTERS: [&str; 6] = [
    "None",
    "Grayscale",
    "Red/Green (Protanopia)",
    "Green/Red (Deuteranopia)",
    "Blue/Yellow (Tritanopia)",
    "Colour Tint",
];
/// The colour filter intensity slider's range (range).
const INTENSITY_MIN: f64 = 0.25;
/// The cursor size slider's range (the server's clamp).
const CURSOR_MIN: f64 = 1.0;
const CURSOR_MAX: f64 = 4.0;
/// The caps-lock delay: 75 ms or off (default).
const CAPS_LOCK_DELAY_MS: u32 = 75;

pub(crate) fn row(
    id: ControlId,
    label: impl Into<String>,
    control: Control,
    backend: Backend,
) -> Row {
    Row {
        id,
        label: label.into(),
        control,
        backend,
        error: None,
        disabled: None,
        icon: None,
        detail: None,
    }
}

/// Clipboard Access (the entries of `data-control.conf`, the lines that allow nothing, and the add buttons), then
/// Screen Sharing (the grants of `screen-sharing.conf`), then Desktop Control and Screen Capture (the answers of
/// `desktop-control.conf`).
fn privacy_sections(settings: &Settings, live: &Live) -> Vec<Section> {
    let list = &settings.clipboard;
    let backend = Backend::Key(File::DataControl, "allow");
    let button = |title: &str| Control::Button {
        title: title.to_string(),
    };
    let none = || Control::Text {
        value: String::new(),
    };
    let mut rows = Vec::new();
    if let Some(e) = &list.unreadable {
        rows.push(Row {
            error: Some(e.clone()),
            ..row(ControlId::Reading, CLIPBOARD_UNREADABLE, none(), backend)
        });
    }
    for (i, entry) in list.conf.allowed.iter().enumerate() {
        let (name, icon) = clipboard::identity(&entry.program, &live.apps);
        let (error, control) = match clipboard::state(entry) {
            State::Allowed => (None, button("Remove")),
            State::Missing => (Some("Not found"), button("Remove")),
            State::Changed => (
                Some("Changed since you allowed it"),
                Control::Buttons {
                    titles: vec!["Allow Again".into(), "Remove".into()],
                },
            ),
        };
        rows.push(Row {
            error: error.map(str::to_string),
            icon: Some(icon.unwrap_or_default()),
            detail: Some(home_relative(&entry.program)),
            ..row(ControlId::ClipboardApp(i), name, control, backend)
        });
    }
    for (i, line) in list.unusable.iter().enumerate() {
        rows.push(Row {
            error: Some(line.message.clone()),
            ..row(
                ControlId::ClipboardUnusable(i),
                line.text.clone(),
                button("Remove"),
                backend,
            )
        });
    }
    if rows.is_empty() {
        rows.push(row(ControlId::Reading, "No applications", none(), backend));
    }
    let mut add = row(
        ControlId::ClipboardAdd,
        "Add an application",
        Control::Buttons {
            titles: vec!["Add App\u{2026}".to_string(), "Browse\u{2026}".to_string()],
        },
        backend,
    );
    // a refused file has no entries to remove; nothing may be added over it
    if list.unreadable.is_some() {
        add.disabled = Some(CLIPBOARD_UNREADABLE.to_string());
    }
    vec![
        Section {
            header: header("Clipboard Access"),
            rows,
            footer: None,
        },
        Section {
            header: None,
            rows: vec![add],
            footer: Some(CLIPBOARD_FOOTER.to_string()),
        },
        screen_sharing_section(settings, live),
        desktop_control_section(Service::Control, settings, live),
        desktop_control_section(Service::Capture, settings, live),
    ]
}

/// Desktop Control or Screen Capture: each app that asked for `service`, in the order the file lists them, with its
/// switch (on: allowed, off: denied) and Forget.
fn desktop_control_section(service: Service, settings: &Settings, live: &Live) -> Section {
    let answers = &settings.desktop_control;
    let backend = Backend::Key(File::DesktopControl, "allow");
    let none = || Control::Text {
        value: String::new(),
    };
    let mut rows = Vec::new();
    if let Some(e) = &answers.unreadable {
        rows.push(Row {
            error: Some(e.clone()),
            ..row(ControlId::Reading, CLIPBOARD_UNREADABLE, none(), backend)
        });
    }
    for (i, entry) in answers.conf.service(service).enumerate() {
        let shown = desktop_control::identity(&entry.app, &live.apps);
        rows.push(Row {
            icon: Some(shown.icon.unwrap_or_default()),
            detail: shown.detail,
            // nothing may be written over a file that is refused
            disabled: answers
                .unreadable
                .as_ref()
                .map(|_| CLIPBOARD_UNREADABLE.to_string()),
            ..row(
                ControlId::DesktopControlApp(service, i),
                shown.name,
                Control::ButtonAndSwitch {
                    title: "Forget".to_string(),
                    on: entry.decision == Decision::Allow,
                },
                backend,
            )
        });
    }
    let (empty, footer) = match service {
        Service::Control => (DESKTOP_CONTROL_EMPTY, DESKTOP_CONTROL_FOOTER),
        Service::Capture => (SCREEN_CAPTURE_EMPTY, SCREEN_CAPTURE_FOOTER),
    };
    if rows.is_empty() {
        rows.push(Row {
            detail: Some(empty.to_string()),
            ..row(ControlId::Reading, "No applications", none(), backend)
        });
    }
    Section {
        header: header(desktop_control::title(service)),
        rows,
        footer: Some(footer.to_string()),
    }
}

/// Screen Sharing: each app that may share a display again without asking, with Forget.
fn screen_sharing_section(settings: &Settings, live: &Live) -> Section {
    let grants = &settings.screen_sharing;
    let backend = Backend::Key(File::ScreenSharing, "grant");
    let none = || Control::Text {
        value: String::new(),
    };
    let mut rows = Vec::new();
    if let Some(e) = &grants.unreadable {
        rows.push(Row {
            error: Some(e.clone()),
            ..row(ControlId::Reading, CLIPBOARD_UNREADABLE, none(), backend)
        });
    }
    for (i, grant) in grants.conf.grants.iter().enumerate() {
        let (name, icon) = screen_sharing::identity(&grant.app_id, &live.apps);
        rows.push(Row {
            icon: Some(icon.unwrap_or_default()),
            detail: Some(format!("Display {}", grant.display)),
            ..row(
                ControlId::ScreenSharingGrant(i),
                name,
                Control::Button {
                    title: "Forget".to_string(),
                },
                backend,
            )
        });
    }
    if rows.is_empty() {
        rows.push(row(ControlId::Reading, "No applications", none(), backend));
    }
    Section {
        header: header("Screen Sharing"),
        rows,
        footer: Some(SCREEN_SHARING_FOOTER.to_string()),
    }
}

const CLIPBOARD_UNREADABLE: &str = "The list can't be read";
/// The footer of Screen Sharing: what a remembered app can do, and what Forget does.
const SCREEN_SHARING_FOOTER: &str =
    "These apps can share the display shown without asking you again. Forget one to be asked next time.";
/// The footer of Desktop Control: what an app that is on can do, and what its switch and Forget do.
const DESKTOP_CONTROL_FOOTER: &str =
    "Apps that are on can see your windows and their controls and use the pointer and \
     keyboard. Turning one off stops it at once; Forget one to be asked next time.";
/// The footer of Screen Capture: what an app that is on can do, and what its switch and Forget do.
const SCREEN_CAPTURE_FOOTER: &str =
    "Apps that are on can take pictures of your screen. Turning one off stops it at \
     once; Forget one to be asked next time.";
/// The second line of an empty Desktop Control list: when an app shows there.
const DESKTOP_CONTROL_EMPTY: &str = "Apps show here after they ask to control the desktop.";
/// The second line of an empty Screen Capture list: when an app shows there.
const SCREEN_CAPTURE_EMPTY: &str = "Apps show here after they ask to capture the screen.";
/// One line of footer: what an allowed application can do (wording approximate).
const CLIPBOARD_FOOTER: &str = "These apps can read and change what you copy, at any time.";

/// A picture's name as a title: separators as spaces, the first letter capital.
fn picture_title(name: &str) -> String {
    let spaced = name.replace(['-', '_'], " ");
    let mut chars = spaced.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The grid of `files` as a section; `selected` is the picture the desktop shows.
fn picture_section(
    group: usize,
    header: Option<String>,
    files: &[WallpaperFile],
    loading: bool,
    settings: &Settings,
    default: Option<&Path>,
    unreadable: &std::collections::BTreeMap<std::path::PathBuf, crate::controls::Unreadable>,
) -> Section {
    // with nothing stored the pack's default picture shows, whichever appearance files it has
    let stored = settings.compositor.wallpaper.as_deref().map(Path::new);
    let shown_dark = settings.compositor.wallpaper_dark.as_deref().map(Path::new);
    let selected = files.iter().position(|f| match stored {
        Some(light) => f.light == light && f.dark.as_deref() == shown_dark,
        None => Some(f.light.as_path()) == default,
    });
    let control = if files.is_empty() {
        Control::Text {
            value: if loading {
                "Loading\u{2026}"
            } else {
                "No pictures"
            }
            .to_string(),
        }
    } else {
        Control::Tiles {
            tiles: files
                .iter()
                .map(|f| Tile {
                    title: f
                        .credit
                        .as_ref()
                        .map_or_else(|| picture_title(&f.name), |c| c.title.clone()),
                    image: f.light.clone(),
                    pair: f.dark.is_some(),
                    unreadable: unreadable.get(&f.light).copied(),
                    credit: f.credit.as_ref().map(|c| c.author.clone()),
                })
                .collect(),
            selected,
        }
    };
    Section {
        header,
        rows: vec![row(
            ControlId::Wallpaper(group),
            if files.is_empty() { "Pictures" } else { "" },
            control,
            Backend::Key(File::Compositor, "wallpaper"),
        )],
        footer: None,
    }
}

/// The picture the desktop shows now, large, with its name and what is special about it.
fn current_section(settings: &Settings, live: &Live) -> Section {
    let pictures = &live.pictures;
    let stored = settings.compositor.wallpaper.as_deref().map(Path::new);
    let light = stored.or(pictures.default.as_deref());
    let known = light.and_then(|light| {
        pictures
            .bundled
            .iter()
            .chain(pictures.folders.iter().flat_map(|(_, files)| files))
            .find(|f| f.light == light)
    });
    let title = match (known, light) {
        (Some(file), _) => file
            .credit
            .as_ref()
            .map_or_else(|| picture_title(&file.name), |c| c.title.clone()),
        (None, Some(path)) => path
            .file_stem()
            .map_or_else(String::new, |n| picture_title(&n.to_string_lossy())),
        (None, None) => "Default".to_string(),
    };
    let pair = settings.compositor.wallpaper_dark.is_some()
        || (stored.is_none() && known.is_some_and(|f| f.dark.is_some()));
    let unreadable = light.and_then(|light| pictures.unreadable.get(light).copied());
    let note = if let Some(why) = unreadable {
        Some(why.note().to_string())
    } else if pair {
        Some("Light and dark: follows the appearance.".to_string())
    } else {
        known
            .and_then(|f| f.credit.as_ref())
            .map(|c| format!("By {}", c.author))
            .or_else(|| light.is_none().then(|| "The built-in picture.".to_string()))
    };
    Section {
        header: header("Current"),
        rows: vec![row(
            ControlId::Reading,
            "",
            Control::Preview {
                image: light.map(Path::to_path_buf),
                title,
                note,
                unreadable,
            },
            Backend::Key(File::Compositor, "wallpaper"),
        )],
        footer: None,
    }
}

/// `path` with the home folder written `~`.
pub fn home_relative(path: &Path) -> String {
    match std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn wallpaper_sections(settings: &Settings, live: &Live) -> Vec<Section> {
    let pictures = &live.pictures;
    let default = pictures.default.as_deref();
    let mut sections = vec![
        current_section(settings, live),
        picture_section(
            0,
            header("Pictures"),
            &pictures.bundled,
            pictures.loading.contains(&0),
            settings,
            default,
            &pictures.unreadable,
        ),
    ];
    for (i, (folder, files)) in pictures.folders.iter().enumerate() {
        let name = folder.file_name().map_or_else(
            || folder.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        sections.push(picture_section(
            i + 1,
            Some(name),
            files,
            pictures.loading.contains(&(i + 1)),
            settings,
            None,
            &pictures.unreadable,
        ));
        sections.push(Section {
            header: None,
            rows: vec![row(
                ControlId::WallpaperRemoveFolder(i),
                home_relative(folder),
                Control::Button {
                    title: "Remove".to_string(),
                },
                Backend::Key(File::Compositor, "wallpaper_folders"),
            )],
            footer: None,
        });
    }
    sections.push(pane_extras::wallpaper_colors_section(settings));
    sections.push(Section {
        header: None,
        rows: vec![row(
            ControlId::WallpaperAdd,
            "Add pictures",
            Control::Buttons {
                titles: vec![
                    "Add Folder\u{2026}".to_string(),
                    "Add Photo\u{2026}".to_string(),
                ],
            },
            Backend::Key(File::Compositor, "wallpaper_folders"),
        )],
        footer: None,
    });
    sections
}

/// The resolutions the pop-up offers besides the automatic one and a typed one (dots per inch).
pub const RESOLUTIONS: [u32; 6] = [400, 800, 1200, 1600, 3200, 6400];

/// A choice of a mouse's resolution pop-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionChoice {
    /// Forget the stored resolution: the hardware database's, else 1000.
    Automatic,
    Dpi(u32),
    /// Ask for a number in a sheet.
    Custom,
}

/// The items of `mouse`'s resolution pop-up in order: Automatic, the presets, a stored value that is none of them,
/// Custom.
pub fn resolution_choices(mouse: &Mouse) -> Vec<ResolutionChoice> {
    let mut items = vec![ResolutionChoice::Automatic];
    items.extend(RESOLUTIONS.map(ResolutionChoice::Dpi));
    if mouse.stored && !RESOLUTIONS.contains(&mouse.dpi) {
        items.push(ResolutionChoice::Dpi(mouse.dpi));
    }
    items.push(ResolutionChoice::Custom);
    items
}

fn resolution_control(mouse: &Mouse) -> Control {
    let items = resolution_choices(mouse);
    let selected = items
        .iter()
        .position(|c| match c {
            ResolutionChoice::Automatic => !mouse.stored,
            ResolutionChoice::Dpi(d) => mouse.stored && *d == mouse.dpi,
            ResolutionChoice::Custom => false,
        })
        .expect("the automatic item or the stored value is in the list");
    let choices = items
        .iter()
        .map(|c| match c {
            ResolutionChoice::Automatic if !mouse.stored => {
                format!("Automatic ({} dpi)", mouse.dpi)
            }
            ResolutionChoice::Automatic => "Automatic".to_string(),
            ResolutionChoice::Dpi(d) => format!("{d} dpi"),
            ResolutionChoice::Custom => "Custom\u{2026}".to_string(),
        })
        .collect();
    Control::Popup { choices, selected }
}

/// The most mice the pane lists.
const MAX_MICE: usize = 32;

/// The resolution rows of the connected mice: one row "Mouse resolution" for one mouse, else a section header and a
/// row named after each mouse (a name another mouse has too gets its number).
fn resolution_section(mice: &[Mouse]) -> Option<Section> {
    let mice = &mice[..mice.len().min(MAX_MICE)];
    if mice.is_empty() {
        return None;
    }
    let rows = mice
        .iter()
        .enumerate()
        .map(|(i, mouse)| {
            let name = display_name(&mouse.name);
            let label = if mice.len() == 1 {
                "Mouse resolution".to_string()
            } else if mice
                .iter()
                .filter(|m| display_name(&m.name) == name)
                .count()
                > 1
            {
                let n = mice[..=i]
                    .iter()
                    .filter(|m| display_name(&m.name) == name)
                    .count();
                format!("{name} ({n})")
            } else {
                name
            };
            // keyed by the row's index into `Live::mice`; when the server pushes the list as it changes, key it by
            // `Mouse::key` instead, so a mouse that goes away does not hand its row to another one under the pointer
            row(
                ControlId::MouseResolution(i),
                label,
                resolution_control(mouse),
                Backend::Key(File::Compositor, "mouse_dpi"),
            )
        })
        .collect();
    Some(Section {
        header: (mice.len() > 1).then(|| "Mouse resolution".to_string()),
        rows,
        footer: Some(MOUSE_RESOLUTION_FOOTER.to_string()),
    })
}

/// What the mouse resolution section's footnote says. It wraps to the content's width.
pub const MOUSE_RESOLUTION_FOOTER: &str =
    "Match this to your mouse's DPI setting so tracking speed feels the same on every mouse.";

fn header(text: &str) -> Option<String> {
    Some(text.to_string())
}

/// The colours the pointer's outline and fill pop-ups offer, opaque sRGB.
pub const POINTER_COLORS: [(&str, [u8; 4]); 8] = [
    ("White", [255, 255, 255, 255]),
    ("Black", [0, 0, 0, 255]),
    ("Red", [230, 40, 40, 255]),
    ("Orange", [245, 140, 20, 255]),
    ("Yellow", [250, 210, 20, 255]),
    ("Green", [40, 180, 70, 255]),
    ("Blue", [30, 110, 235, 255]),
    ("Purple", [150, 70, 200, 255]),
];
/// The pointer's outline and fill when the file sets none: a white outline around a black fill.
const POINTER_OUTLINE: [u8; 4] = [255, 255, 255, 255];
const POINTER_FILL: [u8; 4] = [0, 0, 0, 255];

/// A pointer colour pop-up showing `stored` (else `default`): the named colours, and a colour the file holds that is
/// none of them as one more item, kept until the user picks another.
fn pointer_color(stored: Option<[u8; 4]>, default: [u8; 4]) -> Control {
    let color = stored.unwrap_or(default);
    let mut choices: Vec<String> = POINTER_COLORS.iter().map(|(n, _)| n.to_string()).collect();
    let selected = match POINTER_COLORS.iter().position(|(_, c)| *c == color) {
        Some(i) => i,
        None => {
            let [r, g, b, a] = color;
            choices.push(format!("#{r:02x}{g:02x}{b:02x}{a:02x}"));
            POINTER_COLORS.len()
        }
    };
    Control::Popup { choices, selected }
}

fn switch(on: bool) -> Control {
    Control::Switch { on }
}

fn popup(choices: impl IntoIterator<Item = &'static str>, selected: usize) -> Control {
    Control::Popup {
        choices: choices.into_iter().map(str::to_string).collect(),
        selected,
    }
}

/// The index of `value` among the second items of `table`.
fn index_of<T: PartialEq + Copy>(table: &[(&str, T)], value: T) -> usize {
    table
        .iter()
        .position(|(_, v)| *v == value)
        .expect("every value the files hold is in its control's table")
}

/// The Show scroll bars row, shared by the Appearance pane.
pub(crate) fn scroll_bars_row(settings: &Settings) -> Row {
    // automatically: beside the content while a mouse is connected, else over it while it scrolls
    row(
        ControlId::ShowScrollBars,
        "Show scroll bars",
        popup(
            SCROLL_BARS.iter().map(|(l, _)| *l),
            index_of(&SCROLL_BARS, settings.show_scroll_bars()),
        ),
        Backend::Key(File::Compositor, "show_scroll_bars"),
    )
}

/// The corner's action as a choice: its index, or `None` for an action the list does not offer (a command, a server
/// action of its own), which is kept until the user picks another.
fn corner_choice(action: &ActionTarget) -> Option<usize> {
    let name = match action {
        ActionTarget::None => "none",
        ActionTarget::AppGrid => "app-grid",
        ActionTarget::Server(name) => name.as_str(),
        ActionTarget::Exec(_) => return None,
    };
    CORNER_ACTIONS.iter().position(|(_, a)| *a == name)
}

/// The footer of a section backed by `file`: "Takes effect when … restarts." while its process does not apply the
/// file at run time (`reloads`), else none (§2, ST-17).
pub fn restart_footer(file: File, reloads: bool) -> Option<String> {
    (!reloads).then(|| format!("Takes effect when {} restarts.", file.owner()))
}

/// The sections of `pane` with the values in `settings`, `session` and `live`.
pub fn sections(pane: PaneId, settings: &Settings, session: Session, live: &Live) -> Vec<Section> {
    let file_footer = |file: File| restart_footer(file, file.reloads());
    match pane {
        PaneId::Appearance => crate::pane_extras::appearance_sections(settings, session.dark),
        PaneId::Account => crate::pane_extras::account_sections(live),
        PaneId::General => crate::pane_extras::general_sections(live),
        PaneId::DesktopDock => {
            let dock = Backend::Key;
            let mut corners: Vec<Row> = Vec::new();
            for (trigger, label, key) in CORNERS {
                let action = settings.dock.actions.get(trigger);
                let mut choices: Vec<String> =
                    CORNER_ACTIONS.iter().map(|(l, _)| l.to_string()).collect();
                let selected = match corner_choice(action) {
                    Some(i) => i,
                    // a value outside the list shows as an extra selected item (as §6.3 does for the sleep timers)
                    None => {
                        choices.push(cupertino_defaults::dock::action_text(action));
                        choices.len() - 1
                    }
                };
                corners.push(row(
                    ControlId::HotCorner(trigger),
                    label,
                    Control::Popup { choices, selected },
                    dock(File::Dock, key),
                ));
            }
            let mut sections = vec![Section {
                header: header("Dock"),
                rows: vec![
                    row(
                        ControlId::DockSize,
                        "Size",
                        Control::Slider {
                            value: mapping::dock_value(settings.tile_size()),
                            stops: None,
                            marks: &mapping::DOCK_SIZE_MARKS,
                            labels: ("Small", "Large"),
                        },
                        dock(File::Dock, "tile_size"),
                    ),
                    row(
                        ControlId::Magnification,
                        "Magnification",
                        Control::Slider {
                            value: mapping::magnification_value(
                                settings.magnification(),
                                settings.large_size(),
                                settings.tile_size(),
                            ),
                            stops: None,
                            marks: &mapping::MAGNIFICATION_MARKS,
                            labels: ("Off", "Large"),
                        },
                        dock(File::Dock, "large_size"),
                    ),
                    row(
                        ControlId::DockPosition,
                        "Position on screen",
                        Control::Segmented {
                            segments: POSITIONS.iter().map(|(l, _)| *l).collect(),
                            selected: index_of(&POSITIONS, settings.position()),
                        },
                        dock(File::Dock, "position"),
                    ),
                    row(
                        ControlId::MinimizeEffect,
                        "Minimize windows using",
                        popup(
                            EFFECTS.iter().map(|(l, _)| *l),
                            index_of(&EFFECTS, settings.minimize_effect()),
                        ),
                        dock(File::Compositor, "minimize_effect"),
                    ),
                    row(
                        ControlId::AutoHide,
                        "Automatically hide and show the Dock",
                        switch(settings.autohide()),
                        dock(File::Dock, "autohide"),
                    ),
                    row(
                        ControlId::Recents,
                        "Show suggested and recent apps in Dock",
                        switch(settings.recents()),
                        dock(File::Dock, "recents"),
                    ),
                ]
                .into_iter()
                .chain(crate::pane_extras::dock_dock_rows(settings))
                .collect(),
                footer: file_footer(File::Dock),
            }];
            sections.push(crate::pane_extras::windows_section(settings));
            sections.extend(crate::pane_extras::browser_section(live));
            sections.push(Section {
                header: header("Hot Corners"),
                rows: corners,
                footer: file_footer(File::Dock),
            });
            sections
        }
        PaneId::ControlPanel => crate::pane_extras::control_panel_sections(settings),
        PaneId::MenuBar => {
            let c = settings.menubar.clock;
            let key = |k| Backend::Key(File::MenuBar, k);
            let clock = Section {
                header: header("Clock Options"),
                rows: vec![
                    row(
                        ControlId::ClockDate,
                        "Show date",
                        popup(DATES.iter().map(|(l, _)| *l), index_of(&DATES, c.show_date)),
                        key("clock.show_date"),
                    ),
                    row(
                        ControlId::ClockDayOfWeek,
                        "Show the day of the week",
                        switch(c.show_day_of_week),
                        key("clock.show_day_of_week"),
                    ),
                    row(
                        ControlId::Clock24,
                        "Use a 24-hour clock",
                        switch(c.hour_cycle == HourCycle::H24),
                        key("clock.hour_cycle"),
                    ),
                    row(
                        ControlId::ClockAmPm,
                        "Show AM/PM",
                        switch(c.show_am_pm),
                        key("clock.show_am_pm"),
                    ),
                    row(
                        ControlId::ClockFlash,
                        "Flash the time separators",
                        switch(c.flash_separators),
                        key("clock.flash_separators"),
                    ),
                    row(
                        ControlId::ClockSeconds,
                        "Display the time with seconds",
                        switch(c.show_seconds),
                        key("clock.show_seconds"),
                    ),
                ],
                footer: file_footer(File::MenuBar),
            };
            let mut sections = crate::pane_extras::menu_bar_head_sections(settings);
            sections.push(clock);
            sections.extend(crate::pane_extras::menu_bar_tail_sections(settings, live));
            sections
        }
        PaneId::Keyboard => {
            let rate =
                mapping::rate_position(settings.repeat_delay_ms(), settings.repeat_interval_ms());
            let delay = delay_position(settings);
            let mut out = vec![
                Section {
                    header: header("Key Repeat"),
                    rows: vec![
                        row(
                            ControlId::RepeatRate,
                            "Key repeat rate",
                            Control::Slider {
                                value: Control::stop_value(rate, mapping::RATE_POSITIONS),
                                stops: Some(mapping::RATE_POSITIONS),
                                marks: &[],
                                labels: ("Off", "Fast"),
                            },
                            Backend::Key(File::Keyboard, "repeat_interval"),
                        ),
                        row(
                            ControlId::RepeatDelay,
                            "Delay until repeat",
                            Control::Slider {
                                value: Control::stop_value(
                                    delay - mapping::DELAY_FIRST,
                                    mapping::DELAY_LAST,
                                ),
                                stops: Some(mapping::DELAY_LAST),
                                marks: &[],
                                labels: ("Long", "Short"),
                            },
                            Backend::Key(File::Keyboard, "repeat_delay"),
                        ),
                    ],
                    footer: file_footer(File::Keyboard),
                },
                Section {
                    header: header("Modifier Keys"),
                    rows: vec![
                        row(
                            ControlId::ModifierProfile,
                            "Command key",
                            choice_popup(
                                &PROFILES,
                                settings
                                    .keyboard
                                    .modifier_profile
                                    .unwrap_or(ModifierProfile::Cupertino),
                                |p| p.name().to_string(),
                            ),
                            Backend::Key(File::Keyboard, "modifier_profile"),
                        ),
                        row(
                            ControlId::CapsLockDelay,
                            "Delay caps lock",
                            switch(
                                settings
                                    .keyboard
                                    .caps_lock_delay_ms
                                    .unwrap_or(CAPS_LOCK_DELAY_MS)
                                    > 0,
                            ),
                            Backend::Key(File::Keyboard, "caps_lock_delay"),
                        ),
                        row(
                            ControlId::CapsLockSwitch,
                            "Caps lock switches input sources",
                            switch(settings.keyboard.caps_lock_switch.unwrap_or(false)),
                            Backend::Key(File::Keyboard, "caps_lock_switch"),
                        ),
                        row(
                            ControlId::FnKey,
                            "Press fn key to",
                            choice_popup(
                                &FN_ACTIONS,
                                settings.keyboard.fn_key.unwrap_or(FnKey::ChangeInputSource),
                                |k| k.name().to_string(),
                            ),
                            Backend::Key(File::Keyboard, "fn_key"),
                        ),
                        row(
                            ControlId::AccentPicker,
                            "Show accents for held keys",
                            switch(settings.keyboard.accent_picker.unwrap_or(true)),
                            Backend::Key(File::Keyboard, "accent_picker"),
                        ),
                    ],
                    footer: file_footer(File::Keyboard),
                },
            ];
            out.extend(crate::pane_more::keyboard_sections(settings, live));
            out
        }
        PaneId::Mouse => {
            let key = |k| Backend::Key(File::Compositor, k);
            let mut sections = vec![crate::pane_more::tabs(
                &["Point & Click", "Scrolling", "Devices"],
                live,
            )];
            sections.push(match live.tab {
                0 => Section {
                    header: None,
                    rows: vec![
                        row(
                            ControlId::MouseSpeed,
                            "Tracking speed",
                            tracking(settings.mouse_speed()),
                            key("mouse_speed"),
                        ),
                        row(
                            ControlId::MouseAcceleration,
                            "Pointer acceleration",
                            switch(settings.mouse_acceleration()),
                            key("mouse_acceleration"),
                        ),
                    ],
                    footer: file_footer(File::Compositor),
                },
                1 => Section {
                    header: None,
                    rows: vec![row(
                        ControlId::MouseNatural,
                        "Natural scrolling",
                        switch(settings.mouse_natural_scrolling()),
                        key("mouse_natural_scrolling"),
                    )],
                    footer: file_footer(File::Compositor),
                },
                _ => match live.mice.ready().and_then(|m| resolution_section(m)) {
                    Some(section) => section,
                    None => Section {
                        header: None,
                        rows: vec![row(
                            ControlId::Reading,
                            "Mouse",
                            Control::Text {
                                value: "No mouse is connected".to_string(),
                            },
                            Backend::Service("Mice"),
                        )],
                        footer: None,
                    },
                },
            });
            sections
        }
        PaneId::Wallpaper => wallpaper_sections(settings, live),
        PaneId::Trackpad => {
            let key = |k| Backend::Key(File::Compositor, k);
            let mut sections = vec![crate::pane_more::tabs(
                &["Point & Click", "Scroll & Zoom", "More Gestures"],
                live,
            )];
            sections.push(match live.tab {
                0 => Section {
                    header: None,
                    rows: vec![
                        row(
                            ControlId::TrackpadSpeed,
                            "Tracking speed",
                            tracking(settings.trackpad_speed()),
                            key("trackpad_speed"),
                        ),
                        row(
                            ControlId::TrackpadAcceleration,
                            "Pointer acceleration",
                            switch(settings.trackpad_acceleration()),
                            key("trackpad_acceleration"),
                        ),
                    ],
                    footer: file_footer(File::Compositor),
                },
                1 => Section {
                    header: None,
                    rows: vec![row(
                        ControlId::TrackpadNatural,
                        "Natural scrolling",
                        switch(settings.natural_scrolling()),
                        key("natural_scrolling"),
                    )],
                    footer: file_footer(File::Compositor),
                },
                _ => Section {
                    header: None,
                    rows: GESTURES
                        .iter()
                        .map(|(trigger, label, key)| {
                            let action = settings.dock.actions.get(*trigger);
                            let mut choices: Vec<String> =
                                CORNER_ACTIONS.iter().map(|(l, _)| l.to_string()).collect();
                            let selected = match corner_choice(action) {
                                Some(i) => i,
                                None => {
                                    choices.push(cupertino_defaults::dock::action_text(action));
                                    choices.len() - 1
                                }
                            };
                            row(
                                ControlId::HotCorner(*trigger),
                                *label,
                                Control::Popup { choices, selected },
                                Backend::Key(File::Dock, key),
                            )
                        })
                        .collect(),
                    footer: file_footer(File::Dock),
                },
            });
            sections
        }
        PaneId::Accessibility => {
            let a = &settings.accessibility;
            let d = session.display;
            let set = Backend::SetAccessibility;
            let mut display = vec![
                row(
                    ControlId::ReduceMotion,
                    display_label(DisplayOption::ReduceMotion),
                    switch(d.reduce_motion),
                    set("reduce_motion"),
                ),
                row(
                    ControlId::ReduceTransparency,
                    display_label(DisplayOption::ReduceTransparency),
                    switch(d.reduce_transparency),
                    set("reduce_transparency"),
                ),
                row(
                    ControlId::IncreaseContrast,
                    display_label(DisplayOption::IncreaseContrast),
                    switch(d.increase_contrast),
                    set("increase_contrast"),
                ),
                row(
                    ControlId::DifferentiateWithoutColor,
                    display_label(DisplayOption::DifferentiateWithoutColor),
                    switch(d.differentiate_without_color),
                    set("differentiate_without_color"),
                ),
                row(
                    ControlId::InvertColors,
                    "Invert colours",
                    switch(a.invert_colors),
                    set("invert_colors"),
                ),
                row(
                    ControlId::DisplayContrast,
                    "Display contrast",
                    Control::Slider {
                        value: a.display_contrast.clamp(0.0, 1.0),
                        stops: None,
                        marks: &[],
                        labels: ("Normal", "Maximum"),
                    },
                    set("display_contrast"),
                ),
                row(
                    ControlId::ColorFilter,
                    "Colour filter",
                    popup(FILTERS, filter_index(a.color_filter)),
                    set("color_filter"),
                ),
            ];
            if let Some((_, intensity)) = a.filter() {
                let v = ((intensity - INTENSITY_MIN) / (1.0 - INTENSITY_MIN)).clamp(0.0, 1.0);
                display.push(row(
                    ControlId::ColorFilterIntensity,
                    "Intensity",
                    Control::Slider {
                        value: v,
                        stops: None,
                        marks: &[],
                        labels: ("Low", "High"),
                    },
                    set("color_filter_intensity"),
                ));
            }
            // last, so the rows above keep their places
            display.push(row(
                ControlId::SteadyCaret,
                "Steady text cursor",
                switch(a.steady_caret),
                set(cupertino_defaults::accessibility::STEADY_CARET),
            ));
            let pointer = |k| Backend::Key(File::Compositor, k);
            let cursor = ((settings.compositor.cursor_size.unwrap_or(CURSOR_MIN) - CURSOR_MIN)
                / (CURSOR_MAX - CURSOR_MIN))
                .clamp(0.0, 1.0);
            let keys = |k| Backend::Key(File::Keyboard, k);
            let k = &settings.keyboard;
            let mut out = vec![
                Section {
                    header: header("Display"),
                    rows: display,
                    footer: None,
                },
                screen_reader_section(a.screen_reader, live.screen_reader_installed),
                Section {
                    header: header("Pointer"),
                    rows: vec![
                        row(
                            ControlId::CursorSize,
                            "Pointer size",
                            Control::Slider {
                                value: cursor,
                                stops: None,
                                marks: &[],
                                labels: ("Normal", "Large"),
                            },
                            pointer("cursor_size"),
                        ),
                        row(
                            ControlId::CursorOutline,
                            "Pointer outline color",
                            pointer_color(settings.compositor.cursor_outline, POINTER_OUTLINE),
                            pointer("cursor_outline"),
                        ),
                        row(
                            ControlId::CursorFill,
                            "Pointer fill color",
                            pointer_color(settings.compositor.cursor_fill, POINTER_FILL),
                            pointer("cursor_fill"),
                        ),
                        row(
                            ControlId::ShakeToLocate,
                            "Shake mouse pointer to locate",
                            switch(settings.compositor.shake_to_locate.unwrap_or(true)),
                            pointer("shake_to_locate"),
                        ),
                    ],
                    footer: file_footer(File::Compositor),
                },
                Section {
                    header: header("Keyboard"),
                    rows: vec![
                        row(
                            ControlId::StickyKeys,
                            "Sticky keys",
                            switch(k.sticky_keys.unwrap_or(false)),
                            keys("sticky_keys"),
                        ),
                        row(
                            ControlId::StickyKeysShortcut,
                            "Press Shift five times to toggle sticky keys",
                            switch(k.sticky_keys_shortcut.unwrap_or(false)),
                            keys("sticky_keys_shortcut"),
                        ),
                        row(
                            ControlId::SlowKeys,
                            "Slow keys",
                            switch(k.slow_keys.unwrap_or(false)),
                            keys("slow_keys"),
                        ),
                        row(
                            ControlId::MouseKeys,
                            "Mouse keys",
                            switch(k.mouse_keys.unwrap_or(false)),
                            keys("mouse_keys"),
                        ),
                        row(
                            ControlId::MouseKeysShortcut,
                            "Press Alt five times to toggle mouse keys",
                            switch(k.mouse_keys_shortcut.unwrap_or(false)),
                            keys("mouse_keys_shortcut"),
                        ),
                    ]
                    .into_iter()
                    .chain(crate::pane_more::key_delay_rows(settings))
                    .collect(),
                    footer: file_footer(File::Keyboard),
                },
            ];
            if let Some(login) = &live.login_screen {
                out.insert(1, login_screen_section(login));
            }
            out
        }
        PaneId::Privacy => privacy_sections(settings, live),
        PaneId::LockScreen => {
            let p = &settings.power;
            let key = |k| Backend::Key(File::Power, k);
            let ac = timer_popup(&mapping::SLEEP_CHOICES, p.ac.display_sleep);
            // without a battery only the power-adapter timer applies (§3)
            let mut rows = match live.has_battery() {
                Some(false) => vec![row(
                    ControlId::DisplaySleepAc,
                    "Turn display off when inactive",
                    ac,
                    key("display_sleep_ac"),
                )],
                Some(true) | None => vec![
                    row(
                        ControlId::DisplaySleepBattery,
                        "Turn display off on battery when inactive",
                        timer_popup(&mapping::SLEEP_CHOICES, p.battery.display_sleep),
                        key("display_sleep_battery"),
                    ),
                    row(
                        ControlId::DisplaySleepAc,
                        "Turn display off on power adapter when inactive",
                        ac,
                        key("display_sleep_ac"),
                    ),
                ],
            };
            rows.push(row(
                ControlId::RequirePassword,
                "Require password after the display is turned off",
                timer_popup(&mapping::PASSWORD_CHOICES, p.require_password_after),
                key("require_password_after"),
            ));
            vec![Section {
                header: None,
                rows,
                footer: file_footer(File::Power),
            }]
        }
        PaneId::Battery => {
            let p = &settings.power;
            let key = |k| Backend::Key(File::Power, k);
            let level = match &live.battery {
                _ if live.has_battery() == Some(false) => Vec::new(),
                Reading::Ready(Some(b)) => {
                    let mut rows = vec![
                        row(
                            ControlId::Reading,
                            "Charge",
                            Control::Text {
                                value: format!("{:.0} %", b.percent),
                            },
                            Backend::Service("Percentage"),
                        ),
                        row(
                            ControlId::Reading,
                            "State",
                            Control::Text {
                                value: b.state.text().to_string(),
                            },
                            Backend::Service("State"),
                        ),
                    ];
                    let time = match b.state {
                        BatteryState::Charging if b.time_to_full > 0 => {
                            Some(("Time until full", b.time_to_full))
                        }
                        BatteryState::Discharging if b.time_to_empty > 0 => {
                            Some(("Time remaining", b.time_to_empty))
                        }
                        _ => None,
                    };
                    if let Some((label, t)) = time {
                        rows.push(row(
                            ControlId::Reading,
                            label,
                            Control::Text {
                                value: duration_text(t),
                            },
                            Backend::Service(if label == "Time remaining" {
                                "TimeToEmpty"
                            } else {
                                "TimeToFull"
                            }),
                        ));
                    }
                    vec![Section {
                        header: header("Battery Level"),
                        rows,
                        footer: None,
                    }]
                }
                // no battery: the pane is Energy Saver, the timers only
                Reading::Ready(None) => Vec::new(),
                other => vec![Section {
                    header: header("Battery Level"),
                    rows: vec![row(
                        ControlId::Reading,
                        "Charge",
                        Control::Text {
                            value: other.reason().unwrap_or_default().to_string(),
                        },
                        Backend::Service("Percentage"),
                    )],
                    footer: None,
                }],
            };
            let mut out = level;
            out.extend(crate::pane_more::battery_sections(
                &live.battery_extras,
                live.chart_days,
                live.has_battery() == Some(true),
            ));
            out.push(Section {
                header: header("System Sleep"),
                // without a battery only the power-adapter timer applies (§3)
                rows: match live.has_battery() {
                    Some(false) => vec![row(
                        ControlId::SystemSleepAc,
                        "Put the computer to sleep after",
                        timer_popup(&mapping::SLEEP_CHOICES, p.ac.system_sleep),
                        key("system_sleep_ac"),
                    )],
                    Some(true) | None => vec![
                        row(
                            ControlId::SystemSleepBattery,
                            "Put the computer to sleep on battery after",
                            timer_popup(&mapping::SLEEP_CHOICES, p.battery.system_sleep),
                            key("system_sleep_battery"),
                        ),
                        row(
                            ControlId::SystemSleepAc,
                            "Put the computer to sleep on power adapter after",
                            timer_popup(&mapping::SLEEP_CHOICES, p.ac.system_sleep),
                            key("system_sleep_ac"),
                        ),
                    ],
                },
                footer: file_footer(File::Power),
            });
            out
        }
        PaneId::Focus => {
            let mut out = focus_sections(&live.focus);
            out.push(crate::pane_more::focus_critical_section(settings));
            out
        }
        PaneId::Notifications => notifications_sections(settings, live),
        PaneId::Sound => crate::pane_more::sound_sections(live),
        PaneId::Displays => {
            let text = |label: &str, value: String| {
                row(
                    ControlId::Reading,
                    label,
                    Control::Text { value },
                    Backend::Service("wl_output"),
                )
            };
            let mut out: Vec<Section> = match &live.displays {
                Reading::Ready(monitors) => monitors
                    .iter()
                    .enumerate()
                    .map(|(i, m)| Section {
                        header: Some(m.title.clone()),
                        rows: [
                            display_size_row(i, m),
                            Some(text("Resolution", about::mode_text(m.mode))),
                            crate::pane_more::refresh_row(i, m).or_else(|| {
                                m.refresh_mhz
                                    .map(|r| text("Refresh Rate", about::refresh_text(r)))
                            }),
                            crate::pane_more::arrange_row(i, monitors),
                            Some(text("Scale", about::scale_text(m.scale))),
                        ]
                        .into_iter()
                        .flatten()
                        .collect(),
                        footer: None,
                    })
                    .collect(),
                other => vec![Section {
                    header: header("Display"),
                    rows: vec![text(
                        "Resolution",
                        other.reason().unwrap_or_default().to_string(),
                    )],
                    footer: None,
                }],
            };
            // only a display with a backlight has a brightness to set
            if let Reading::Ready(value) = live.brightness {
                out.push(Section {
                    header: header("Brightness"),
                    rows: vec![row(
                        ControlId::Brightness,
                        "Brightness",
                        Control::Level { value },
                        Backend::Service("SetBrightness"),
                    )],
                    footer: None,
                });
            }
            out
        }
        PaneId::Wifi => {
            let mut out = wifi_sections(&live.wifi);
            out.extend(crate::pane_more::wifi_details(live));
            out
        }
        PaneId::Network => {
            let mut out = network_sections(&live.wifi);
            out.extend(crate::pane_more::interface_sections(live));
            out
        }
        PaneId::Vpn => crate::pane_more::vpn_sections(live),
        PaneId::Bluetooth => bluetooth_sections(&live.bluetooth),
        PaneId::Search => crate::pane_more::search_sections(settings),
        PaneId::Users => crate::pane_more::users_sections(live),
    }
}

/// The label of a "looks like" choice of `monitor`: its size in points as the display shows it, with the default, the
/// one with the biggest text and the one with the most room named.
pub fn display_size_label(
    monitor: &crate::live::Monitor,
    list: &cupertino_displays::Catalogue,
    index: usize,
) -> String {
    let size = about::mode_text(monitor.choice_logical(&list.choices[index]));
    let note = if list.default == Some(index) {
        Some("Default")
    } else if index == 0 {
        Some("Bigger")
    } else if index + 1 == list.choices.len() {
        Some("More Room")
    } else {
        None
    };
    match note {
        Some(note) => format!("{size} ({note})"),
        None => size,
    }
}

/// The most sizes that show as pictures; more show as a pop-up.
const MAX_SIZE_TILES: usize = 7;

/// The "looks like" choices of display `index` of `Live::displays`, from the biggest text to the most room, the one
/// it shows selected. A size that is none of them (a scale set elsewhere) is an extra item at the end. `None` when the
/// display has no choices (its physical size is unknown, or it is too small to scale).
fn display_size_row(index: usize, monitor: &crate::live::Monitor) -> Option<Row> {
    let list = monitor.catalogue();
    if list.choices.is_empty() {
        return None;
    }
    let mut choices: Vec<String> = (0..list.choices.len())
        .map(|i| display_size_label(monitor, &list, i))
        .collect();
    let shown = monitor.logical.map(|logical| {
        let at = list
            .choices
            .iter()
            .position(|c| monitor.choice_logical(c) == logical);
        (at, logical)
    });
    let selected = match shown {
        Some((Some(i), _)) => i,
        Some((None, logical)) => {
            choices.push(about::mode_text(logical));
            choices.len() - 1
        }
        None => list.default.unwrap_or(0),
    };
    // a few sizes that are all the display's own show as pictures, from the biggest text to the most room
    let control =
        if choices.len() == list.choices.len() && (2..=MAX_SIZE_TILES).contains(&choices.len()) {
            Control::Swatches {
                items: (0..list.choices.len())
                    .map(|i| {
                        (
                            about::mode_text(monitor.choice_logical(&list.choices[i])),
                            SwatchKind::Scale(i as u8),
                        )
                    })
                    .collect(),
                selected,
            }
        } else {
            Control::Popup { choices, selected }
        };
    Some(row(
        ControlId::DisplaySize(index),
        "Desktop Size",
        control,
        Backend::Service("zwlr_output_manager_v1"),
    ))
}

fn seconds(t: Timer) -> Option<u64> {
    t.duration().map(|d| d.as_secs())
}

fn timer_of(seconds: Option<u64>) -> Timer {
    seconds.map_or(Timer::Never, |s| {
        Timer::After(std::time::Duration::from_secs(s))
    })
}

/// A timer pop-up over `listed` with `stored` selected.
fn timer_popup(listed: &[Option<u64>], stored: Timer) -> Control {
    let c = mapping::timer_choices(listed, seconds(stored));
    Control::Popup {
        choices: c.labels,
        selected: c.selected,
    }
}

/// The colour filter's choice index.
fn filter_index(f: Option<ColorFilter>) -> usize {
    match f {
        None => 0,
        Some(ColorFilter::Grayscale) => 1,
        Some(ColorFilter::Protanopia) => 2,
        Some(ColorFilter::Deuteranopia) => 3,
        Some(ColorFilter::Tritanopia) => 4,
        Some(ColorFilter::Tint { .. }) => 5,
    }
}

/// The value `accessibility.conf` writes for filter choice `i`, a tint keeping `hue`.
fn filter_text(i: usize, hue: f64) -> String {
    match i {
        0 => "off".to_string(),
        1 => "grayscale".to_string(),
        2 => "protanopia".to_string(),
        3 => "deuteranopia".to_string(),
        4 => "tritanopia".to_string(),
        _ => format!("tint {hue}"),
    }
}

/// A pop-up over the labels of `table` with `value` selected, a stored value outside it shown as an extra item named by
/// `extra`.
fn choice_popup<T: PartialEq + Copy>(
    table: &[(&'static str, T)],
    value: T,
    extra: impl Fn(T) -> String,
) -> Control {
    let mut choices: Vec<String> = table.iter().map(|(l, _)| l.to_string()).collect();
    let selected = match table.iter().position(|(_, v)| *v == value) {
        Some(i) => i,
        None => {
            choices.push(extra(value));
            choices.len() - 1
        }
    };
    Control::Popup { choices, selected }
}

/// The label of a display option's row; the Login Screen footer names the option by it too.
fn display_label(option: DisplayOption) -> &'static str {
    match option {
        DisplayOption::ReduceMotion => "Reduce motion",
        DisplayOption::ReduceTransparency => "Reduce transparency",
        DisplayOption::IncreaseContrast => "Increase contrast",
        DisplayOption::DifferentiateWithoutColor => "Differentiate without colour",
    }
}

/// The display options the machine's defaults may set ([`SYSTEM_KEYS`]), in the Display section's order.
fn system_options() -> Vec<DisplayOption> {
    DisplayOption::ALL
        .into_iter()
        .filter(|o| SYSTEM_KEYS.contains(&o.key()))
        .collect()
}

/// What the Screen reader row says under its label: the chord that turns the reader on or off anywhere.
pub const SCREEN_READER_DETAIL: &str =
    "Reads the screen aloud. Super+Alt+S turns it on or off, also at the login screen.";
/// Why the Screen reader switch is disabled without a reader.
pub const NO_SCREEN_READER: &str = "No screen reader is installed.";

/// The session's screen reader (`screen_reader` of `accessibility.conf`, which the server starts and stops), after the
/// display options and their login screen defaults; disabled with the reason when none is installed.
fn screen_reader_section(on: bool, installed: bool) -> Section {
    let mut reader = row(
        ControlId::ScreenReader,
        "Screen reader",
        switch(on),
        Backend::SetAccessibility(cupertino_defaults::accessibility::SCREEN_READER),
    );
    reader.detail = Some(SCREEN_READER_DETAIL.to_string());
    if !installed {
        reader.disabled = Some(NO_SCREEN_READER.to_string());
    }
    Section {
        header: header("Screen Reader"),
        rows: vec![reader],
        footer: None,
    }
}

/// What the section's footer says: the limit of the machine's defaults (the row's label says whom they reach).
const LOGIN_SCREEN_FOOTER: &str =
    "People who have changed an accessibility setting keep their own settings.";

/// What the machine's file holds now, for the login screen: each allowed option with its state, or `Default settings`
/// when there is no file or it was refused.
pub fn login_screen_state(current: Option<&AccessibilitySettings>) -> String {
    match current {
        Some(a) => system_options()
            .iter()
            .map(|o| {
                let state = if a.display.get(*o) { "on" } else { "off" };
                format!("{} {state}", display_label(*o))
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "Default settings".to_string(),
    }
}

/// What Use Mine sends: the [`SYSTEM_KEYS`] with the person's values, the display options as the session
/// applies them (the Display rows'), never another key.
pub fn login_screen_options(session: Session, settings: &Settings) -> HashMap<String, String> {
    AccessibilitySettings {
        display: session.display,
        ..settings.accessibility
    }
    .entries()
    .into_iter()
    .filter(|(key, _)| SYSTEM_KEYS.contains(key))
    .map(|(key, value)| (key.to_string(), value))
    .collect()
}

/// The machine's defaults for the login screen and new users, offered to whoever sessiond lets set them: Use My
/// Settings, "Saving…" while the write waits for its reply.
fn login_screen_section(login: &LoginScreen) -> Section {
    let control = if login.saving {
        Control::Text {
            value: "Saving…".into(),
        }
    } else {
        Control::Button {
            title: "Use Mine".into(),
        }
    };
    Section {
        header: header("Login Screen"),
        rows: vec![
            row(
                ControlId::LoginScreen,
                "Login screen and new users",
                control,
                Backend::Service("SetSystemAccessibility"),
            ),
            row(
                ControlId::Reading,
                "Login screen now",
                Control::Text {
                    value: login_screen_state(login.current.as_ref()),
                },
                Backend::Service("SystemAccessibility"),
            ),
        ],
        footer: Some(LOGIN_SCREEN_FOOTER.to_string()),
    }
}

/// The most applications the Notifications pane lists.
pub const MAX_NOTIFICATION_APPS: usize = 64;

/// The choices of "Show previews", in the order of [`Previews::ALL`].
const PREVIEW_CHOICES: [&str; 3] = ["Always", "When Unlocked", "Never"];

fn previews_index(p: Previews) -> usize {
    Previews::ALL
        .iter()
        .position(|q| *q == p)
        .expect("every value is in Previews::ALL")
}

/// Under the lock screen's settings: what a notification shows there.
const NOTIFICATIONS_FOOTER: &str =
    "Without previews, a notification shows only its app and the word Notification.";
/// Under the applications' title: which applications are listed and what Default means.
const NOTIFICATION_APPS_FOOTER: &str =
    "Apps appear here once they send a notification. Default follows the settings above.";

/// The applications the Notifications pane lists, as (settings id, name): those `notifications.conf` holds a choice of
/// and those whose notifications the centre holds now, each once, by name then id, at most [`MAX_NOTIFICATION_APPS`]. The
/// name is the desktop entry's, else the one the centre gave, else the id.
pub fn notification_apps(settings: &Settings, live: &Live) -> Vec<(String, String)> {
    let named = |id: &str, given: Option<&str>| {
        let file = format!("{id}.desktop");
        live.apps
            .iter()
            .find(|i| i.entry.id == file)
            .map(|i| i.entry.name.clone())
            .or_else(|| given.map(str::to_owned))
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| id.to_owned())
    };
    let given = |id: &str| {
        live.notifying_apps
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, name)| name.as_str())
    };
    let mut apps: Vec<(String, String)> = settings
        .notifications
        .apps
        .keys()
        .map(|id| (id.clone(), named(id, given(id))))
        .collect();
    for (id, name) in &live.notifying_apps {
        if valid_app_id(id) && !apps.iter().any(|(k, _)| k == id) {
            apps.push((id.clone(), named(id, Some(name))));
        }
    }
    // applications of one name by id, so that their order (and the numbers that tell their pop-ups apart) does not
    // change with which of them holds a choice or with the order the centre lists them in
    apps.sort_by(|a, b| crate::sidebar::standard_compare(&a.1, &b.1).then_with(|| a.0.cmp(&b.0)));
    apps.truncate(MAX_NOTIFICATION_APPS);
    apps
}

/// Whether the pop-up menu `open` still belongs to the row it opened on once the applications of
/// [`notification_apps`] went from `before` to `after`. An application's rows are addressed by its place in that list,
/// so a menu open on one closes when the list changes under it, and a choice never lands on another application.
pub fn menu_survives(
    open: ControlId,
    before: &[(String, String)],
    after: &[(String, String)],
) -> bool {
    match open {
        ControlId::NotificationAppLockScreen(_) | ControlId::NotificationAppPreviews(_) => {
            before == after
        }
        _ => true,
    }
}

/// The lock screen's settings for every application, then each listed application's own choices, where Default
/// follows the former (`notifications.conf`; the pane's layout is Original).
fn notifications_sections(settings: &Settings, live: &Live) -> Vec<Section> {
    let n = &settings.notifications;
    let key = |k| Backend::Key(File::Notifications, k);
    let on_off = |on: bool| if on { "On" } else { "Off" };
    let mut out = vec![Section {
        header: header("Lock Screen"),
        rows: vec![
            row(
                ControlId::NotificationsLockScreen,
                "Show on lock screen",
                switch(n.show_on_lock_screen),
                key("show_on_lock_screen"),
            ),
            row(
                ControlId::NotificationsPreviews,
                "Show previews",
                popup(PREVIEW_CHOICES, previews_index(n.show_previews)),
                key("show_previews"),
            ),
        ],
        footer: Some(NOTIFICATIONS_FOOTER.to_string()),
    }];
    let apps = notification_apps(settings, live);
    out.push(Section {
        header: header("Application Notifications"),
        rows: if apps.is_empty() {
            vec![row(
                ControlId::Reading,
                "No applications",
                Control::Text {
                    value: String::new(),
                },
                key("app"),
            )]
        } else {
            Vec::new()
        },
        footer: Some(NOTIFICATION_APPS_FOOTER.to_string()),
    });
    for (i, (id, name)) in apps.iter().enumerate() {
        let own = n.apps.get(id).copied().unwrap_or_default();
        let lock_choices = vec![
            format!("Default ({})", on_off(n.show_on_lock_screen)),
            "On".to_string(),
            "Off".to_string(),
        ];
        let lock_selected = match own.show_on_lock_screen {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        let preview_choices = std::iter::once(format!(
            "Default ({})",
            PREVIEW_CHOICES[previews_index(n.show_previews)]
        ))
        .chain(PREVIEW_CHOICES.iter().map(|c| c.to_string()))
        .collect();
        let preview_selected = own.show_previews.map_or(0, |p| 1 + previews_index(p));
        out.push(Section {
            header: Some(name.clone()),
            rows: crate::pane_more::app_rows(i, &own)
                .into_iter()
                .chain([
                    row(
                        ControlId::NotificationAppLockScreen(i),
                        "Show on lock screen",
                        Control::Popup {
                            choices: lock_choices,
                            selected: lock_selected,
                        },
                        key("app.<id>.show_on_lock_screen"),
                    ),
                    row(
                        ControlId::NotificationAppPreviews(i),
                        "Show previews",
                        Control::Popup {
                            choices: preview_choices,
                            selected: preview_selected,
                        },
                        key("app.<id>.show_previews"),
                    ),
                ])
                .collect(),
            footer: None,
        });
    }
    out
}

/// Do Not Disturb and its durations (`NotificationCenter1`; the pane's layout is Original, §3).
fn focus_sections(focus: &Reading<crate::live::Focus>) -> Vec<Section> {
    let svc = Backend::Service;
    match focus {
        Reading::Ready(f) => {
            let mut out = vec![Section {
                header: None,
                rows: vec![row(
                    ControlId::DoNotDisturb,
                    "Do Not Disturb",
                    switch(f.on),
                    svc("SetDoNotDisturb"),
                )],
                footer: None,
            }];
            if !f.lifetimes.is_empty() {
                let rows = f
                    .lifetimes
                    .iter()
                    .enumerate()
                    .map(|(i, (_, title))| {
                        row(
                            ControlId::FocusFor(i),
                            title.clone(),
                            Control::Button {
                                title: "Turn On".into(),
                            },
                            svc("SetDoNotDisturbFor"),
                        )
                    })
                    .collect();
                out.push(Section {
                    header: header("Turn On For"),
                    rows,
                    footer: None,
                });
            }
            out
        }
        other => vec![Section {
            header: None,
            rows: vec![Row {
                disabled: other.reason().map(str::to_string),
                ..row(
                    ControlId::DoNotDisturb,
                    "Do Not Disturb",
                    switch(false),
                    svc("SetDoNotDisturb"),
                )
            }],
            footer: None,
        }],
    }
}

/// The radio switch, then the saved and the other networks (§6.4; GetNetworks order, strongest first).
fn wifi_sections(wifi: &Reading<Wifi>) -> Vec<Section> {
    let svc = Backend::Service;
    let Reading::Ready(w) = wifi else {
        return vec![Section {
            header: None,
            rows: vec![Row {
                disabled: wifi.reason().map(str::to_string),
                ..row(
                    ControlId::WifiPower,
                    "Wi-Fi",
                    switch(false),
                    svc("WirelessEnabled"),
                )
            }],
            footer: None,
        }];
    };
    let mut power = row(
        ControlId::WifiPower,
        "Wi-Fi",
        switch(w.enabled),
        svc("WirelessEnabled"),
    );
    if !w.has_wifi_device() {
        power.disabled = Some("Wi-Fi is not available".into());
    }
    power.error = w.failure.clone();
    let mut out = vec![Section {
        header: None,
        rows: vec![power],
        footer: None,
    }];
    if !w.enabled || !w.has_wifi_device() {
        return out;
    }
    let network_row = |(i, n): (usize, &crate::live::Network)| {
        let control = if w.joining.as_deref() == Some(n.ssid.as_str()) {
            Control::Text {
                value: "Connecting…".into(),
            }
        } else if n.connected {
            Control::Button {
                title: "Disconnect".into(),
            }
        } else {
            Control::Button {
                title: "Join".into(),
            }
        };
        let label = if n.security == "open" {
            n.name.clone()
        } else {
            format!("{} (secured)", n.name)
        };
        row(
            ControlId::WifiNetwork(i),
            label,
            control,
            svc(if n.connected { "Disconnect" } else { "Connect" }),
        )
    };
    let known: Vec<Row> = w
        .networks
        .iter()
        .enumerate()
        .filter(|(_, n)| n.saved)
        .map(network_row)
        .collect();
    let other: Vec<Row> = w
        .networks
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.saved)
        .map(network_row)
        .collect();
    if !w.known.is_empty() {
        out.push(crate::pane_more::known_section(w));
    } else if !known.is_empty() {
        out.push(Section {
            header: header("Known Networks"),
            rows: known,
            footer: None,
        });
    }
    if !other.is_empty() {
        out.push(Section {
            header: header("Other Networks"),
            rows: other,
            footer: None,
        });
    }
    out
}

/// The devices of `Network1`, read only, under the daemon's state (§6.4).
fn network_sections(wifi: &Reading<Wifi>) -> Vec<Section> {
    let Reading::Ready(w) = wifi else {
        return vec![Section {
            header: None,
            rows: vec![Row {
                disabled: wifi.reason().map(str::to_string),
                ..row(
                    ControlId::WifiPower,
                    "Network",
                    Control::Text {
                        value: String::new(),
                    },
                    Backend::Service("Devices"),
                )
            }],
            footer: None,
        }];
    };
    let rows = w
        .devices
        .iter()
        .map(|d| {
            let mut value = d.state.clone();
            for part in [&d.ssid, &d.address, &d.failure] {
                if !part.is_empty() {
                    value = format!("{value} · {part}");
                }
            }
            row(
                ControlId::Reading,
                format!("{} ({})", d.name, d.kind),
                Control::Text { value },
                Backend::Service("Devices"),
            )
        })
        .collect();
    vec![Section {
        header: Some(format!("Status: {}", w.state)),
        rows,
        footer: None,
    }]
}

/// What the nearby list says under it while discovery runs.
pub const BLUETOOTH_SEARCHING: &str = "Searching for devices…";

/// The power switch, the paired devices under My Devices and the others under Nearby Devices (§6.4).
fn bluetooth_sections(bt: &Reading<Bluetooth>) -> Vec<Section> {
    let svc = Backend::Service;
    let Reading::Ready(b) = bt else {
        return vec![Section {
            header: None,
            rows: vec![Row {
                disabled: bt.reason().map(str::to_string),
                ..row(
                    ControlId::BluetoothPower,
                    "Bluetooth",
                    switch(false),
                    svc("Powered"),
                )
            }],
            footer: None,
        }];
    };
    let mut power = row(
        ControlId::BluetoothPower,
        "Bluetooth",
        switch(b.powered),
        svc("Powered"),
    );
    if !b.available {
        power.disabled = Some("Bluetooth is not available".into());
    }
    // a failure on a device no longer listed shows here, as one of the switch's does
    let unlisted = b
        .device_failure
        .as_ref()
        .filter(|f| !b.devices.iter().any(|d| d.id == f.id))
        .map(crate::live::DeviceFailure::text);
    power.error = b.failure.clone().or(unlisted);
    let footer = b
        .agent_elsewhere
        .then(|| "Pairing is handled by another application.".to_string());
    let mut out = vec![Section {
        header: None,
        rows: vec![power],
        footer,
    }];
    if !b.available || !b.powered {
        return out;
    }
    // each row: the device's name (or kind and address), its link state under it, its actions at the right; while it
    // pairs or connects the actions wait, and a failed action says so in place of the state
    let device_row = |(i, d): (usize, &crate::live::BtDevice)| {
        let control = if d.paired {
            Control::Buttons {
                titles: vec![
                    if d.connected {
                        "Disconnect".into()
                    } else {
                        "Connect".into()
                    },
                    "Forget…".into(),
                ],
            }
        } else {
            Control::Button {
                title: "Pair".into(),
            }
        };
        let state = d.state();
        let mut r = row(
            ControlId::BluetoothDevice(i),
            d.title(),
            control,
            svc(if d.paired { "Connect" } else { "Pair" }),
        );
        r.detail = Some(state.label().to_string());
        if state.busy() {
            r.disabled = Some(state.label().to_string());
        }
        r.error = b
            .device_failure
            .as_ref()
            .filter(|f| f.id == d.id)
            .map(crate::live::DeviceFailure::text);
        r
    };
    let mine: Vec<Row> = b
        .devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.paired)
        .map(device_row)
        .collect();
    let nearby: Vec<Row> = b
        .devices
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.paired)
        .map(device_row)
        .collect();
    out.push(Section {
        header: header("My Devices"),
        rows: mine,
        footer: None,
    });
    // while discovery runs the nearby list says so, even before it finds anything
    out.push(Section {
        header: header("Nearby Devices"),
        rows: nearby,
        footer: b.discovering.then(|| BLUETOOTH_SEARCHING.to_string()),
    });
    out.retain(|s| !s.rows.is_empty() || s.footer.is_some());
    out
}

/// The tracking-speed slider: ten stops.
fn tracking(speed: f64) -> Control {
    Control::Slider {
        value: Control::stop_value(mapping::tracking_stop(speed), mapping::TRACKING_STOPS.len()),
        stops: Some(mapping::TRACKING_STOPS.len()),
        marks: &[],
        labels: ("Slow", "Fast"),
    }
}

/// The delay slider's position: from the delay in effect, or from the saved one while repeat is off (the slider keeps
/// its position, `repeat_delay_saved`), else the default.
fn delay_position(settings: &Settings) -> usize {
    match (
        settings.repeat_delay_ms(),
        settings.keyboard.repeat_delay_saved_ms,
    ) {
        (0, Some(saved)) => mapping::delay_position(saved),
        (0, None) => mapping::DELAY_DEFAULT,
        (d, _) => mapping::delay_position(d),
    }
}

/// Apply a committed `value` of control `id` to `settings` with the live state `live`; the effects say what to write or
/// call. A commit has at most one effect: the window undoes a failed commit by restoring the settings it had before.
pub fn change(id: ControlId, value: Value, settings: &mut Settings, live: &Live) -> Vec<Effect> {
    let store = |f| vec![Effect::Store(f)];
    let bool_of = |v: Value| match v {
        Value::Bool(b) => b,
        other => panic!("{id:?} is a switch, not {other:?}"),
    };
    let index_of = |v: Value| match v {
        Value::Index(i) => i,
        other => panic!("{id:?} is a choice, not {other:?}"),
    };
    let fraction_of = |v: Value| match v {
        Value::Fraction(f) => f,
        other => panic!("{id:?} is a slider, not {other:?}"),
    };
    match id {
        ControlId::Appearance => {
            // Auto follows the schedule; Light and Dark turn it off and set the look at once
            let choice = index_of(value);
            settings.appearance.auto = choice == 0;
            let dark = match choice {
                0 => settings.appearance.dark_at(crate::system::local_minutes()),
                other => other == 2,
            };
            vec![Effect::SetTheme { dark: Some(dark) }]
        }
        ControlId::AutoDarkFrom => {
            settings.appearance.auto_dark_from =
                pane_extras::time_of_choice(index_of(value), settings.appearance.auto_dark_from);
            vec![Effect::SetTheme {
                dark: Some(settings.appearance.dark_at(crate::system::local_minutes())),
            }]
        }
        ControlId::AutoLightFrom => {
            settings.appearance.auto_light_from =
                pane_extras::time_of_choice(index_of(value), settings.appearance.auto_light_from);
            vec![Effect::SetTheme {
                dark: Some(settings.appearance.dark_at(crate::system::local_minutes())),
            }]
        }
        ControlId::AccentColor => {
            let Some(accent) = Accent::ALL.get(index_of(value)) else {
                return Vec::new();
            };
            settings.appearance.accent = *accent;
            store(File::Appearance)
        }
        ControlId::HighlightColor => {
            let Some(h) = pane_extras::highlight_choices()
                .get(index_of(value))
                .copied()
            else {
                return Vec::new();
            };
            settings.appearance.highlight = h;
            store(File::Appearance)
        }
        ControlId::SidebarIconSize => {
            let Some((_, size)) = pane_extras::SIDEBAR_ICON_CHOICES.get(index_of(value)) else {
                return Vec::new();
            };
            settings.appearance.sidebar_icons = *size;
            store(File::Appearance)
        }
        ControlId::ScrollClick => {
            let Some((_, click)) = pane_extras::SCROLL_CLICK_CHOICES.get(index_of(value)) else {
                return Vec::new();
            };
            settings.appearance.scroll_click = *click;
            store(File::Appearance)
        }
        ControlId::MenuBarAutoHide => {
            settings.compositor.menu_bar_auto_hide = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::EdgeTiling => {
            settings.compositor.edge_tiling = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::TileMargins => {
            settings.compositor.tile_margins = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::MaterialDensity => {
            settings.compositor.material_density = Some((fraction_of(value) * 20.0).round() / 20.0);
            store(File::Compositor)
        }
        ControlId::TintWindows => {
            settings.appearance.tint_windows = bool_of(value);
            store(File::Appearance)
        }
        ControlId::WallpaperColor => {
            let Some((_, color)) = pane_extras::WALLPAPER_COLORS.get(index_of(value)) else {
                return Vec::new();
            };
            settings.compositor.wallpaper_color = Some([color[0], color[1], color[2], 255]);
            store(File::Compositor)
        }
        ControlId::DoubleClickTitle => {
            let Some((_, action)) = pane_extras::TITLE_ACTION_CHOICES.get(index_of(value)) else {
                return Vec::new();
            };
            settings.appearance.titlebar_double_click = *action;
            store(File::Appearance)
        }
        ControlId::MinimizeIntoIcon => {
            settings.dock.minimize_to_app = Some(bool_of(value));
            store(File::Dock)
        }
        ControlId::LaunchBounce => {
            settings.dock.bounce = Some(bool_of(value));
            store(File::Dock)
        }
        ControlId::RunningIndicators => {
            settings.dock.indicators = Some(bool_of(value));
            store(File::Dock)
        }
        ControlId::StackStyle => {
            let Some((_, style)) = pane_extras::STACK_CHOICES.get(index_of(value)) else {
                return Vec::new();
            };
            settings.dock.stack = Some(*style);
            store(File::Dock)
        }
        ControlId::DefaultBrowser => match live.browsers.0.get(index_of(value)) {
            Some(browser) => vec![Effect::SetDefaultBrowser {
                id: browser.id.clone(),
            }],
            None => Vec::new(),
        },
        ControlId::LoginItem(i) => match live.login_items.get(i) {
            Some(item) => vec![Effect::SetLoginItem {
                id: item.id.clone(),
                enable: bool_of(value),
            }],
            None => Vec::new(),
        },
        ControlId::MenuBarBackground => {
            settings.menubar.background = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ShowClock => {
            settings.menubar.shown.clock = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ShowWifi => {
            settings.menubar.shown.wifi = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ShowBluetooth => {
            settings.menubar.shown.bluetooth = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ShowBattery => {
            settings.menubar.shown.battery = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ShowFocus => {
            settings.menubar.shown.focus = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::RecentDocuments => {
            settings.menubar.recent_documents =
                pane_extras::count_of_choice(index_of(value), settings.menubar.recent_documents);
            store(File::MenuBar)
        }
        ControlId::RecentApps => {
            settings.menubar.recent_apps =
                pane_extras::count_of_choice(index_of(value), settings.menubar.recent_apps);
            store(File::MenuBar)
        }
        ControlId::TrayItem(i) => {
            let list = pane_extras::tray_list(settings, live);
            let Some((id, _)) = list.get(i) else {
                return Vec::new();
            };
            let hidden = &mut settings.menubar.hidden_tray;
            hidden.retain(|h| h != id);
            if !bool_of(value) {
                hidden.push(id.clone());
            }
            store(File::MenuBar)
        }
        ControlId::MouseResolution(i) => {
            let Some(mouse) = live.mice.ready().and_then(|m| m.get(i)) else {
                return Vec::new();
            };
            let dpi = match resolution_choices(mouse).get(index_of(value)) {
                Some(ResolutionChoice::Automatic) => None,
                Some(ResolutionChoice::Dpi(d)) => Some(*d),
                Some(ResolutionChoice::Custom) => {
                    return vec![Effect::AskResolution {
                        key: mouse.key.clone(),
                        name: display_name(&mouse.name),
                    }]
                }
                None => return Vec::new(),
            };
            match settings.compositor.set_mouse_dpi(&mouse.key, dpi) {
                Ok(()) => store(File::Compositor),
                Err(_) => Vec::new(),
            }
        }
        ControlId::Wallpaper(group) => {
            let files = match group {
                0 => Some(&live.pictures.bundled),
                n => live.pictures.folders.get(n - 1).map(|(_, files)| files),
            };
            let Some(file) = files.and_then(|f| f.get(index_of(value))) else {
                return Vec::new();
            };
            settings.compositor.wallpaper_color = None;
            settings.compositor.wallpaper = Some(file.light.to_string_lossy().into_owned());
            settings.compositor.wallpaper_dark =
                file.dark.as_ref().map(|d| d.to_string_lossy().into_owned());
            store(File::Compositor)
        }
        ControlId::WallpaperRemoveFolder(i) => {
            let Some((folder, _)) = live.pictures.folders.get(i) else {
                return Vec::new();
            };
            let folder = folder.to_string_lossy();
            let left: Vec<String> = settings
                .compositor
                .wallpaper_folders
                .iter()
                .flatten()
                .filter(|f| f.as_str() != folder)
                .cloned()
                .collect();
            settings.compositor.wallpaper_folders = Some(left).filter(|l| !l.is_empty());
            store(File::Compositor)
        }
        ControlId::WallpaperAdd => vec![Effect::AskPicture {
            folder: index_of(value) == 0,
        }],
        ControlId::DockSize => {
            let magnified = mapping::magnification_value(
                settings.magnification(),
                settings.large_size(),
                settings.tile_size(),
            );
            settings.dock.tile_size = Some(mapping::dock_px(fraction_of(value)));
            // the magnification keeps its place on its slider, so it stays larger than the new tiles
            if magnified > 0.0 {
                settings.dock.large_size =
                    mapping::magnification(magnified, settings.tile_size()).1;
            }
            store(File::Dock)
        }
        ControlId::Magnification => {
            let (on, large) = mapping::magnification(fraction_of(value), settings.tile_size());
            settings.dock.magnification = Some(on);
            if large.is_some() {
                settings.dock.large_size = large;
            }
            store(File::Dock)
        }
        ControlId::DockPosition => {
            settings.dock.position = Some(POSITIONS[index_of(value)].1);
            store(File::Dock)
        }
        ControlId::MinimizeEffect => {
            settings.compositor.minimize_effect = Some(EFFECTS[index_of(value)].1);
            store(File::Compositor)
        }
        ControlId::ShowScrollBars => {
            settings.compositor.show_scroll_bars = Some(SCROLL_BARS[index_of(value)].1);
            store(File::Compositor)
        }
        ControlId::AutoHide => {
            settings.dock.autohide = Some(bool_of(value));
            store(File::Dock)
        }
        ControlId::Recents => {
            settings.dock.recents = Some(bool_of(value));
            store(File::Dock)
        }
        ControlId::HotCorner(trigger) => match CORNER_ACTIONS.get(index_of(value)) {
            Some((_, action)) => {
                let target = cupertino_defaults::dock::parse_action(action)
                    .expect("the corner table holds valid actions");
                settings.dock.actions.set(trigger, target);
                store(File::Dock)
            }
            // the extra item of an action the list does not offer: choosing it again changes nothing
            None => Vec::new(),
        },
        ControlId::ClockSeconds => {
            settings.menubar.clock.show_seconds = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ClockDayOfWeek => {
            settings.menubar.clock.show_day_of_week = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ClockDate => {
            settings.menubar.clock.show_date = DATES[index_of(value)].1;
            store(File::MenuBar)
        }
        ControlId::ClockAmPm => {
            settings.menubar.clock.show_am_pm = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::ClockFlash => {
            settings.menubar.clock.flash_separators = bool_of(value);
            store(File::MenuBar)
        }
        ControlId::Clock24 => {
            settings.menubar.clock.hour_cycle = if bool_of(value) {
                HourCycle::H24
            } else {
                HourCycle::H12
            };
            store(File::MenuBar)
        }
        ControlId::RepeatRate => {
            let p = index_of(value);
            let was_off = settings.repeat_delay_ms() == 0;
            let delay = delay_position(settings);
            if p == 0 && !was_off {
                settings.keyboard.repeat_delay_saved_ms = Some(settings.repeat_delay_ms());
            }
            let (d, interval) = mapping::rate_write(p, delay, settings.repeat_interval_ms());
            settings.keyboard.repeat_delay_ms = Some(d);
            settings.keyboard.repeat_interval_ms = Some(interval);
            store(File::Keyboard)
        }
        ControlId::RepeatDelay => {
            let d = mapping::delay_ms(index_of(value) + mapping::DELAY_FIRST);
            if settings.repeat_delay_ms() == 0 {
                // repeat is off: the delay slider keeps its position for when it is turned on again
                settings.keyboard.repeat_delay_saved_ms = Some(d);
            } else {
                settings.keyboard.repeat_delay_ms = Some(d);
            }
            store(File::Keyboard)
        }
        ControlId::MouseSpeed => {
            settings.compositor.mouse_speed = Some(mapping::TRACKING_STOPS[index_of(value)]);
            store(File::Compositor)
        }
        ControlId::TrackpadSpeed => {
            settings.compositor.trackpad_speed = Some(mapping::TRACKING_STOPS[index_of(value)]);
            store(File::Compositor)
        }
        ControlId::TrackpadAcceleration => {
            settings.compositor.trackpad_acceleration = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::MouseAcceleration => {
            settings.compositor.mouse_acceleration = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::ModifierProfile => match PROFILES.get(index_of(value)) {
            Some((_, p)) => {
                settings.keyboard.modifier_profile = Some(*p);
                store(File::Keyboard)
            }
            None => Vec::new(),
        },
        ControlId::FnKey => match FN_ACTIONS.get(index_of(value)) {
            Some((_, k)) => {
                settings.keyboard.fn_key = Some(*k);
                store(File::Keyboard)
            }
            None => Vec::new(),
        },
        ControlId::CapsLockDelay => {
            settings.keyboard.caps_lock_delay_ms = Some(if bool_of(value) {
                CAPS_LOCK_DELAY_MS
            } else {
                0
            });
            store(File::Keyboard)
        }
        ControlId::CapsLockSwitch => {
            settings.keyboard.caps_lock_switch = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::AccentPicker => {
            settings.keyboard.accent_picker = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::StickyKeys => {
            settings.keyboard.sticky_keys = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::StickyKeysShortcut => {
            settings.keyboard.sticky_keys_shortcut = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::SlowKeys => {
            settings.keyboard.slow_keys = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::MouseKeys => {
            settings.keyboard.mouse_keys = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::MouseKeysShortcut => {
            settings.keyboard.mouse_keys_shortcut = Some(bool_of(value));
            store(File::Keyboard)
        }
        ControlId::ReduceMotion
        | ControlId::SteadyCaret
        | ControlId::ReduceTransparency
        | ControlId::IncreaseContrast
        | ControlId::DifferentiateWithoutColor
        | ControlId::InvertColors => {
            let key = match id {
                ControlId::ReduceMotion => "reduce_motion",
                ControlId::SteadyCaret => cupertino_defaults::accessibility::STEADY_CARET,
                ControlId::ReduceTransparency => "reduce_transparency",
                ControlId::IncreaseContrast => "increase_contrast",
                ControlId::DifferentiateWithoutColor => "differentiate_without_color",
                _ => "invert_colors",
            };
            vec![Effect::SetAccessibility {
                key,
                value: if bool_of(value) { "on" } else { "off" }.to_string(),
            }]
        }
        ControlId::ScreenReader => vec![Effect::SetAccessibility {
            key: cupertino_defaults::accessibility::SCREEN_READER,
            value: if bool_of(value) { "on" } else { "off" }.to_string(),
        }],
        ControlId::DisplayContrast => vec![Effect::SetAccessibility {
            key: "display_contrast",
            value: format!("{:.3}", fraction_of(value)),
        }],
        ControlId::ColorFilter => {
            let hue = match settings.accessibility.color_filter {
                Some(ColorFilter::Tint { hue }) => hue,
                _ => 0.0,
            };
            vec![Effect::SetAccessibility {
                key: "color_filter",
                value: filter_text(index_of(value), hue),
            }]
        }
        ControlId::ColorFilterIntensity => vec![Effect::SetAccessibility {
            key: "color_filter_intensity",
            value: format!(
                "{:.3}",
                INTENSITY_MIN + (1.0 - INTENSITY_MIN) * fraction_of(value)
            ),
        }],
        ControlId::LoginScreen => match &live.login_screen {
            Some(login) if !login.saving => vec![Effect::SetSystemAccessibility],
            _ => Vec::new(),
        },
        ControlId::CursorSize => {
            settings.compositor.cursor_size = Some(
                ((CURSOR_MIN + (CURSOR_MAX - CURSOR_MIN) * fraction_of(value)) * 100.0).round()
                    / 100.0,
            );
            store(File::Compositor)
        }
        ControlId::ShakeToLocate => {
            settings.compositor.shake_to_locate = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::CursorOutline | ControlId::CursorFill => {
            // the extra item of a colour the file holds is already the value
            let Some((_, color)) = POINTER_COLORS.get(index_of(value)) else {
                return Vec::new();
            };
            let slot = if id == ControlId::CursorOutline {
                &mut settings.compositor.cursor_outline
            } else {
                &mut settings.compositor.cursor_fill
            };
            *slot = Some(*color);
            store(File::Compositor)
        }
        ControlId::DisplaySleepBattery
        | ControlId::DisplaySleepAc
        | ControlId::RequirePassword
        | ControlId::SystemSleepBattery
        | ControlId::SystemSleepAc => {
            let p = &mut settings.power;
            let (listed, slot): (&[Option<u64>], &mut Timer) = match id {
                ControlId::DisplaySleepBattery => {
                    (&mapping::SLEEP_CHOICES, &mut p.battery.display_sleep)
                }
                ControlId::DisplaySleepAc => (&mapping::SLEEP_CHOICES, &mut p.ac.display_sleep),
                ControlId::SystemSleepBattery => {
                    (&mapping::SLEEP_CHOICES, &mut p.battery.system_sleep)
                }
                ControlId::SystemSleepAc => (&mapping::SLEEP_CHOICES, &mut p.ac.system_sleep),
                _ => (&mapping::PASSWORD_CHOICES, &mut p.require_password_after),
            };
            match listed.get(index_of(value)) {
                Some(seconds) => {
                    *slot = timer_of(*seconds);
                    store(File::Power)
                }
                // the extra item of a stored value outside the list: choosing it again keeps it
                None => Vec::new(),
            }
        }
        ControlId::NotificationsLockScreen => {
            settings.notifications.show_on_lock_screen = bool_of(value);
            store(File::Notifications)
        }
        ControlId::NotificationsPreviews => match Previews::ALL.get(index_of(value)) {
            Some(p) => {
                settings.notifications.show_previews = *p;
                store(File::Notifications)
            }
            None => Vec::new(),
        },
        ControlId::NotificationAppLockScreen(i) | ControlId::NotificationAppPreviews(i) => {
            let Some((app, _)) = notification_apps(settings, live).into_iter().nth(i) else {
                return Vec::new();
            };
            let mut own = settings
                .notifications
                .apps
                .get(&app)
                .copied()
                .unwrap_or_default();
            let choice = index_of(value);
            if matches!(id, ControlId::NotificationAppLockScreen(_)) {
                own.show_on_lock_screen = match choice {
                    0 => None,
                    1 => Some(true),
                    2 => Some(false),
                    _ => return Vec::new(),
                };
            } else {
                own.show_previews = match choice {
                    0 => None,
                    c => match Previews::ALL.get(c - 1) {
                        Some(p) => Some(*p),
                        None => return Vec::new(),
                    },
                };
            }
            if !settings.notifications.set_app(&app, own) {
                return vec![Effect::Refuse(
                    "This app's settings can't be saved.".to_string(),
                )];
            }
            store(File::Notifications)
        }
        ControlId::DoNotDisturb => vec![Effect::Service(Call::SetDoNotDisturb(bool_of(value)))],
        ControlId::FocusFor(i) => match live.focus.ready().and_then(|f| f.lifetimes.get(i)) {
            Some((id, _)) => vec![Effect::Service(Call::SetDoNotDisturbFor(id.clone()))],
            None => Vec::new(),
        },
        ControlId::Volume => {
            let level = fraction_of(value);
            // a level below the mute threshold mutes; any other level unmutes
            vec![Effect::Audio(AudioChange::SetVolume {
                level,
                muted: level < MUTE_BELOW,
            })]
        }
        ControlId::Mute => {
            let level = live.volume.ready().map_or(0.0, |v| v.level);
            vec![Effect::Audio(AudioChange::SetVolume {
                level,
                muted: bool_of(value),
            })]
        }
        ControlId::Output => match live.outputs.get(index_of(value)) {
            Some(output) => vec![Effect::Audio(AudioChange::SetOutput(output.id))],
            None => Vec::new(),
        },
        ControlId::Brightness => vec![Effect::Service(Call::SetBrightness(fraction_of(value)))],
        ControlId::DisplaySize(i) => {
            let Some(monitor) = live.displays.ready().and_then(|d| d.get(i)) else {
                return Vec::new();
            };
            let list = monitor.catalogue();
            let Some(choice) = list.choices.get(index_of(value)) else {
                // the extra item of a size that is none of the choices: choosing it again keeps it
                return Vec::new();
            };
            if monitor.logical == Some(monitor.choice_logical(choice)) {
                return Vec::new();
            }
            match &monitor.connector {
                Some(connector) => vec![Effect::Service(Call::SetDisplayScale {
                    connector: connector.clone(),
                    scale: choice.scale,
                })],
                None => vec![Effect::Refuse(
                    "This display does not say which it is, so its size can't be changed."
                        .to_string(),
                )],
            }
        }
        ControlId::WifiPower => vec![Effect::Service(Call::SetWirelessEnabled(bool_of(value)))],
        ControlId::WifiNetwork(i) => match live.wifi.ready().and_then(|w| w.networks.get(i)) {
            Some(n) if n.connected => vec![Effect::Service(Call::Disconnect)],
            Some(n) => vec![Effect::Service(Call::Join {
                ssid: n.ssid.clone(),
                passphrase: (!n.needs_passphrase()).then(crate::sheet::Secret::new),
            })],
            None => Vec::new(),
        },
        ControlId::BluetoothPower => {
            vec![Effect::Service(Call::SetBluetoothPowered(bool_of(value)))]
        }
        ControlId::Reading => Vec::new(),
        ControlId::More(more) => crate::pane_more::change(more, value, settings, live),
        ControlId::BluetoothDevice(i) => {
            match live.bluetooth.ready().and_then(|b| b.devices.get(i)) {
                // its actions wait while it pairs or connects
                Some(d) if d.state().busy() => Vec::new(),
                Some(d) if !d.paired => vec![Effect::Service(Call::Pair(d.id.clone()))],
                Some(d) => match index_of(value) {
                    0 if d.connected => vec![Effect::Service(Call::DisconnectDevice(d.id.clone()))],
                    0 => vec![Effect::Service(Call::Connect(d.id.clone()))],
                    _ => vec![Effect::Service(Call::Remove {
                        id: d.id.clone(),
                        confirmed: false,
                    })],
                },
                None => Vec::new(),
            }
        }
        ControlId::ClipboardApp(i) => {
            let Some(entry) = settings.clipboard.conf.allowed.get(i) else {
                return Vec::new();
            };
            if clipboard::state(entry) == State::Changed && index_of(value) == 0 {
                return match cupertino_defaults::data_control::Entry::for_program(&entry.program) {
                    Ok(again) => {
                        settings.clipboard.conf.allowed[i] = again;
                        store(File::DataControl)
                    }
                    Err(reason) => vec![Effect::Refuse(reason)],
                };
            }
            vec![Effect::ConfirmUnallow {
                program: entry.program.clone(),
                name: clipboard::identity(&entry.program, &live.apps).0,
            }]
        }
        ControlId::ClipboardUnusable(i) => {
            if i >= settings.clipboard.unusable.len() {
                return Vec::new();
            }
            settings.clipboard.unusable.remove(i);
            store(File::DataControl)
        }
        ControlId::ClipboardAdd => vec![Effect::AskClipboardApp {
            program: index_of(value) == 1,
        }],
        // forgetting only takes a permission away, so it asks nothing first; a share running now goes on
        ControlId::ScreenSharingGrant(i) => {
            let Some(grant) = settings.screen_sharing.conf.grants.get(i) else {
                return Vec::new();
            };
            let id = grant.id.clone();
            // the row goes at once; the file loses the grant through the effect
            settings.screen_sharing.conf.revoke(&id);
            vec![Effect::ForgetGrant(id)]
        }
        // the row shows the change at once; the file gets it through the effect, and the compositor ends a session the
        // app no longer may hold
        ControlId::DesktopControlApp(service, i) => {
            let answers = &mut settings.desktop_control;
            if answers.unreadable.is_some() {
                return vec![Effect::Refuse(CLIPBOARD_UNREADABLE.to_string())];
            }
            let Some(app) = answers.conf.service(service).nth(i).map(|e| e.app.clone()) else {
                return Vec::new();
            };
            match value {
                Value::Bool(on) => {
                    let decision = if on { Decision::Allow } else { Decision::Deny };
                    // an app read from the file can always be written back; a line that cannot hold it is refused
                    if answers.conf.set(service, &app, decision).is_err() {
                        return vec![Effect::Refuse(
                            "This app's answer can't be saved.".to_string(),
                        )];
                    }
                    vec![Effect::DecideApp {
                        service,
                        app,
                        decision,
                    }]
                }
                Value::Index(0) => {
                    answers.conf.forget(service, &app);
                    vec![Effect::ForgetApp { service, app }]
                }
                _ => Vec::new(),
            }
        }
        ControlId::MouseNatural => {
            settings.compositor.mouse_natural_scrolling = Some(bool_of(value));
            store(File::Compositor)
        }
        ControlId::TrackpadNatural => {
            settings.compositor.natural_scrolling = Some(bool_of(value));
            store(File::Compositor)
        }
    }
}
