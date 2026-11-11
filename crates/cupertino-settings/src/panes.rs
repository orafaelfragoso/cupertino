//! What each pane shows and what its controls write (§3, §6.3). Every control names
//! the key or call that backs it ([`Backend`]); a control without one is not drawn (ST-09). The row order inside a pane
//! is Original (by frequency of use) until O4 recovers the design's.

use cupertino_defaults::accessibility::{
    AccessibilitySettings, ColorFilter, DisplayOption, DisplayOptions, SYSTEM_KEYS,
};
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
use crate::controls::{Control, Tile, Value, MUTE_BELOW};
use crate::desktop_control;
use crate::live::{
    display_name, duration_text, BatteryState, Bluetooth, Live, LoginScreen, Mouse, Reading, Wifi,
};
use crate::mapping;
use crate::pane::PaneId;
use crate::screen_sharing;
use crate::settings::{File, Settings};

/// A control of a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlId {
    /// A read-only row of a live reading (battery, network devices): it changes nothing.
    Reading,
    Appearance,
    ShowScrollBars,
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
    /// Ask the server to set a key of `accessibility.conf` to a value as the file writes it.
    SetAccessibility { key: &'static str, value: String },
    /// Ask sessiond to make [`login_screen_options`] the machine's accessibility defaults.
    SetSystemAccessibility,
    /// A call on a live service.
    Service(Call),
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
    /// The change is refused for this reason; nothing changed.
    Refuse(String),
}

/// A call on a live service.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    SetDoNotDisturb(bool),
    SetDoNotDisturbFor(String),
    SetVolume {
        level: f64,
        muted: bool,
    },
    /// Make the PipeWire sink with this id the output applications play to.
    SetOutput(u32),
    SetBrightness(f64),
    /// Set the display on `connector` to `scale` panel pixels per point (an output configuration of the server).
    SetDisplayScale {
        connector: String,
        scale: f64,
    },
    SetWirelessEnabled(bool),
    /// Join a network; `None`: ask for the passphrase first (a secured network that is not saved).
    Join {
        ssid: String,
        passphrase: Option<crate::sheet::Secret>,
    },
    Disconnect,
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
    ("Notification Centre", "notification-center"),
    ("Control Panel", "control-panel"),
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

fn row(id: ControlId, label: impl Into<String>, control: Control, backend: Backend) -> Row {
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
    let note = if pair {
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
        PaneId::Appearance => vec![Section {
            header: None,
            rows: vec![
                row(
                    ControlId::Appearance,
                    "Appearance",
                    Control::Segmented {
                        segments: vec!["Light", "Dark"],
                        selected: usize::from(session.dark),
                    },
                    Backend::SetAppearance,
                ),
                // automatically: beside the content while a mouse is connected, else over it while it scrolls
                row(
                    ControlId::ShowScrollBars,
                    "Show scroll bars",
                    popup(
                        SCROLL_BARS.iter().map(|(l, _)| *l),
                        index_of(&SCROLL_BARS, settings.show_scroll_bars()),
                    ),
                    Backend::Key(File::Compositor, "show_scroll_bars"),
                ),
            ],
            footer: None,
        }],
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
            vec![
                Section {
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
                    ],
                    footer: file_footer(File::Dock),
                },
                Section {
                    header: header("Hot Corners"),
                    rows: corners,
                    footer: file_footer(File::Dock),
                },
            ]
        }
        PaneId::ControlPanel => {
            let c = settings.menubar.clock;
            let key = |k| Backend::Key(File::MenuBar, k);
            vec![Section {
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
            }]
        }
        PaneId::Keyboard => {
            let rate =
                mapping::rate_position(settings.repeat_delay_ms(), settings.repeat_interval_ms());
            let delay = delay_position(settings);
            vec![
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
            ]
        }
        PaneId::Mouse => {
            let key = |k| Backend::Key(File::Compositor, k);
            let mut sections = vec![Section {
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
                    row(
                        ControlId::MouseNatural,
                        "Natural scrolling",
                        switch(settings.mouse_natural_scrolling()),
                        key("mouse_natural_scrolling"),
                    ),
                ],
                footer: file_footer(File::Compositor),
            }];
            sections.extend(live.mice.ready().and_then(|m| resolution_section(m)));
            sections
        }
        PaneId::Wallpaper => wallpaper_sections(settings, live),
        PaneId::Trackpad => {
            let key = |k| Backend::Key(File::Compositor, k);
            vec![Section {
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
                    row(
                        ControlId::TrackpadNatural,
                        "Natural scrolling",
                        switch(settings.natural_scrolling()),
                        key("natural_scrolling"),
                    ),
                ],
                footer: file_footer(File::Compositor),
            }]
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
                    ],
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
        PaneId::Focus => focus_sections(&live.focus),
        PaneId::Notifications => notifications_sections(settings, live),
        PaneId::Sound => {
            let svc = Backend::Service("master");
            let (level, muted, reason) = match &live.volume {
                Reading::Ready(v) => (v.level, v.muted, None),
                other => (0.0, false, other.reason().map(str::to_string)),
            };
            let mut rows = Vec::new();
            if live.outputs.len() > 1 {
                rows.push(row(
                    ControlId::Output,
                    "Output device",
                    Control::Popup {
                        choices: live.outputs.iter().map(|o| o.name.clone()).collect(),
                        selected: live.outputs.iter().position(|o| o.default).unwrap_or(0),
                    },
                    svc,
                ));
            }
            rows.extend([
                Row {
                    disabled: reason.clone(),
                    ..row(
                        ControlId::Volume,
                        "Output volume",
                        Control::Level { value: level },
                        svc,
                    )
                },
                Row {
                    disabled: reason,
                    ..row(ControlId::Mute, "Mute", switch(muted), svc)
                },
            ]);
            vec![Section {
                header: header("Output"),
                rows,
                footer: None,
            }]
        }
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
                            m.refresh_mhz
                                .map(|r| text("Refresh Rate", about::refresh_text(r))),
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
        PaneId::Wifi => wifi_sections(&live.wifi),
        PaneId::Network => network_sections(&live.wifi),
        PaneId::Bluetooth => bluetooth_sections(&live.bluetooth),
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
    Some(row(
        ControlId::DisplaySize(index),
        "Desktop Size",
        Control::Popup { choices, selected },
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
            rows: vec![
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
            ],
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
    if !known.is_empty() {
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
    power.error = b.failure.clone();
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
    let device_row = |(i, d): (usize, &crate::live::BtDevice)| {
        let control = if d.pairing {
            Control::Text {
                value: "Pairing…".into(),
            }
        } else if d.paired {
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
        row(
            ControlId::BluetoothDevice(i),
            d.title().to_string(),
            control,
            svc(if d.paired { "Connect" } else { "Pair" }),
        )
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
    out.push(Section {
        header: header("Nearby Devices"),
        rows: nearby,
        footer: None,
    });
    out.retain(|s| !s.rows.is_empty());
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
        ControlId::Appearance => vec![Effect::SetAppearance {
            dark: index_of(value) == 1,
        }],
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
            vec![Effect::Service(Call::SetVolume {
                level,
                muted: level < MUTE_BELOW,
            })]
        }
        ControlId::Mute => {
            let level = live.volume.ready().map_or(0.0, |v| v.level);
            vec![Effect::Service(Call::SetVolume {
                level,
                muted: bool_of(value),
            })]
        }
        ControlId::Output => match live.outputs.get(index_of(value)) {
            Some(output) => vec![Effect::Service(Call::SetOutput(output.id))],
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
        ControlId::BluetoothDevice(i) => {
            match live.bluetooth.ready().and_then(|b| b.devices.get(i)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use cupertino_defaults::compositor::CompositorSettings;
    use cupertino_defaults::dock::DockSettings;
    use cupertino_defaults::keyboard::KeyboardSettings;
    use cupertino_defaults::menubar::{ClockSettings, MenuBarSettings};

    fn unset() -> Settings {
        Settings {
            dock: DockSettings::default(),
            compositor: CompositorSettings::default(),
            menubar: MenuBarSettings {
                clock: ClockSettings::UNSET,
            },
            keyboard: KeyboardSettings::default(),
            power: cupertino_defaults::power::PowerSettings::default(),
            accessibility: cupertino_defaults::accessibility::AccessibilitySettings::default(),
            display_height: 1080.0,
            clipboard: crate::clipboard::List::default(),
            screen_sharing: crate::screen_sharing::Grants::default(),
            desktop_control: crate::desktop_control::Answers::default(),
            notifications: cupertino_defaults::notifications::NotificationSettings::default(),
        }
    }

    const LIGHT: Session = Session {
        dark: false,
        display: DisplayOptions {
            reduce_motion: false,
            reduce_transparency: false,
            increase_contrast: false,
            differentiate_without_color: false,
        },
    };

    fn control(pane: PaneId, id: ControlId, s: &Settings) -> Control {
        sections(pane, s, LIGHT, &Live::default())
            .into_iter()
            .flat_map(|sec| sec.rows)
            .find(|r| r.id == id)
            .expect("the control is in the pane")
            .control
    }

    #[test]
    fn every_control_names_the_key_or_call_behind_it() {
        let s = unset();
        for pane in crate::pane::PANES.iter().map(|p| p.id) {
            for r in sections(pane, &s, LIGHT, &Live::default())
                .iter()
                .flat_map(|sec| &sec.rows)
            {
                match r.backend {
                    Backend::SetAppearance => assert_eq!(r.id, ControlId::Appearance),
                    Backend::SetAccessibility(key)
                    | Backend::Key(_, key)
                    | Backend::Service(key) => assert!(!key.is_empty(), "{:?}", r.id),
                }
            }
        }
    }

    #[test]
    fn without_a_battery_only_the_power_adapter_timers_show() {
        let ids = |pane, battery| {
            let live = Live {
                battery: crate::live::Reading::Ready(battery),
                ..Live::default()
            };
            sections(pane, &unset(), LIGHT, &live)
                .iter()
                .flat_map(|s| &s.rows)
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(PaneId::LockScreen, None),
            [ControlId::DisplaySleepAc, ControlId::RequirePassword]
        );
        assert_eq!(ids(PaneId::Battery, None), [ControlId::SystemSleepAc]);
        let battery = crate::live::Battery {
            percent: 80.0,
            state: crate::live::BatteryState::Discharging,
            time_to_empty: 0,
            time_to_full: 0,
        };
        assert_eq!(
            ids(PaneId::LockScreen, Some(battery)),
            [
                ControlId::DisplaySleepBattery,
                ControlId::DisplaySleepAc,
                ControlId::RequirePassword
            ]
        );
        assert!(ids(PaneId::Battery, Some(battery))
            .ends_with(&[ControlId::SystemSleepBattery, ControlId::SystemSleepAc]));
    }

    #[test]
    fn the_displays_pane_shows_each_connected_display_and_brightness_only_with_a_backlight() {
        use crate::live::{Monitor, Reading};
        let monitor = Live {
            displays: Reading::Ready(vec![Monitor {
                title: "Studio 27".into(),
                mode: (2560, 1440),
                refresh_mhz: Some(59_951),
                scale: 1.25,
                connector: Some("DP-1".into()),
                physical_mm: None,
                logical: Some((2048, 1152)),
                rotated: false,
            }]),
            brightness: Reading::Unavailable("This display has no brightness control".into()),
            ..Live::default()
        };
        let shown = |live: &Live| {
            sections(PaneId::Displays, &unset(), LIGHT, live)
                .into_iter()
                .map(|sec| {
                    let rows: Vec<(String, Control)> =
                        sec.rows.into_iter().map(|r| (r.label, r.control)).collect();
                    (sec.header, rows)
                })
                .collect::<Vec<_>>()
        };
        let text = |label: &str, value: &str| {
            (
                label.to_string(),
                Control::Text {
                    value: value.into(),
                },
            )
        };
        assert_eq!(
            shown(&monitor),
            [(
                Some("Studio 27".to_string()),
                vec![
                    text("Resolution", "2560\u{202f}×\u{202f}1440"),
                    text("Refresh Rate", "59.95\u{a0}Hz"),
                    text("Scale", "1.25x"),
                ]
            )]
        );
        let laptop = Live {
            brightness: Reading::Ready(0.5),
            ..monitor
        };
        assert_eq!(
            shown(&laptop)[1],
            (
                Some("Brightness".to_string()),
                vec![("Brightness".to_string(), Control::Level { value: 0.5 })]
            )
        );
    }

    fn built_in_panel(logical: (u32, u32)) -> crate::live::Monitor {
        crate::live::Monitor {
            title: "Built-in Display".into(),
            mode: (3024, 1964),
            refresh_mhz: Some(120_000),
            scale: 3024.0 / f64::from(logical.0),
            connector: Some("eDP-1".into()),
            physical_mm: Some((302, 196)),
            logical: Some(logical),
            rotated: false,
        }
    }

    fn displays(monitors: Vec<crate::live::Monitor>) -> Live {
        Live {
            displays: crate::live::Reading::Ready(monitors),
            ..Live::default()
        }
    }

    fn size_row(live: &Live) -> Option<Row> {
        sections(PaneId::Displays, &unset(), LIGHT, live)
            .into_iter()
            .flat_map(|s| s.rows)
            .find(|r| matches!(r.id, ControlId::DisplaySize(_)))
    }

    #[test]
    fn a_built_in_panel_offers_its_sizes_from_bigger_text_to_more_room() {
        let row = size_row(&displays(vec![built_in_panel((1512, 982))])).expect("the size row");
        assert_eq!(row.label, "Desktop Size");
        let nb = '\u{202f}';
        assert_eq!(
            row.control,
            Control::Popup {
                choices: vec![
                    format!("1024{nb}×{nb}665 (Bigger)"),
                    format!("1147{nb}×{nb}745"),
                    format!("1352{nb}×{nb}878"),
                    format!("1512{nb}×{nb}982 (Default)"),
                    format!("1800{nb}×{nb}1169 (More Room)"),
                ],
                selected: 3,
            }
        );
        let larger = size_row(&displays(vec![built_in_panel((1147, 745))])).unwrap();
        assert!(matches!(larger.control, Control::Popup { selected: 1, .. }));
        // the size row comes first, the readings after it
        let rows: Vec<String> = sections(
            PaneId::Displays,
            &unset(),
            LIGHT,
            &displays(vec![built_in_panel((1512, 982))]),
        )
        .remove(0)
        .rows
        .into_iter()
        .map(|r| r.label)
        .collect();
        assert_eq!(
            rows,
            ["Desktop Size", "Resolution", "Refresh Rate", "Scale"]
        );
    }

    #[test]
    fn a_turned_display_shows_its_sizes_turned_and_the_one_it_shows_chosen() {
        // a 27-inch 4K monitor turned upright, at 1.5: the desktop is 1440 x 2560 points
        let upright = crate::live::Monitor {
            title: "Studio 27".into(),
            mode: (3840, 2160),
            refresh_mhz: Some(60_000),
            scale: 1.5,
            connector: Some("DP-1".into()),
            physical_mm: Some((597, 336)),
            logical: Some((1440, 2560)),
            rotated: true,
        };
        let live = displays(vec![upright.clone()]);
        let list = upright.catalogue();
        let at = list
            .choices
            .iter()
            .position(|c| c.scale == 1.5)
            .expect("1.5 is one of the choices");
        let Control::Popup { choices, selected } = size_row(&live).unwrap().control else {
            panic!("a popup");
        };
        assert_eq!(
            choices.len(),
            list.choices.len(),
            "no extra size: {choices:?}"
        );
        assert_eq!(selected, at);
        assert!(
            choices[at].starts_with(&format!("1440{nb}×{nb}2560", nb = '\u{202f}')),
            "{}",
            choices[at]
        );
        assert_eq!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(at),
                &mut unset(),
                &live
            ),
            [],
            "the size shown already"
        );
    }

    #[test]
    fn choosing_a_size_sets_its_exact_scale_on_its_display() {
        let live = displays(vec![built_in_panel((1512, 982))]);
        let mut settings = unset();
        assert_eq!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(4),
                &mut settings,
                &live
            ),
            [Effect::Service(Call::SetDisplayScale {
                connector: "eDP-1".into(),
                scale: 3024.0 / 1800.0,
            })]
        );
        assert_eq!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(0),
                &mut settings,
                &live
            ),
            [Effect::Service(Call::SetDisplayScale {
                connector: "eDP-1".into(),
                scale: 3024.0 / 1024.0,
            })]
        );
        assert_eq!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(3),
                &mut settings,
                &live
            ),
            [],
            "the size shown already"
        );
        assert_eq!(
            change(
                ControlId::DisplaySize(1),
                Value::Index(0),
                &mut settings,
                &live
            ),
            []
        );
        let anonymous = displays(vec![crate::live::Monitor {
            connector: None,
            ..built_in_panel((1512, 982))
        }]);
        assert!(matches!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(0),
                &mut settings,
                &anonymous
            )[..],
            [Effect::Refuse(_)]
        ));
    }

    #[test]
    fn a_size_set_elsewhere_is_an_extra_choice_and_a_display_of_unknown_size_has_none() {
        let custom = displays(vec![crate::live::Monitor {
            scale: 1.5,
            ..built_in_panel((2016, 1309))
        }]);
        let row = size_row(&custom).unwrap();
        let Control::Popup { choices, selected } = row.control else {
            panic!("a popup");
        };
        assert_eq!((choices.len(), selected), (6, 5));
        assert_eq!(choices[5], format!("2016{nb}×{nb}1309", nb = '\u{202f}'));
        assert_eq!(
            change(
                ControlId::DisplaySize(0),
                Value::Index(5),
                &mut unset(),
                &custom
            ),
            [],
            "choosing it again keeps it"
        );
        let unknown = displays(vec![crate::live::Monitor {
            physical_mm: None,
            ..built_in_panel((1512, 982))
        }]);
        assert_eq!(size_row(&unknown), None);
        // an external monitor offers the bucket list around its own size
        let external = displays(vec![crate::live::Monitor {
            title: "Studio 27".into(),
            mode: (3840, 2160),
            refresh_mhz: Some(60_000),
            scale: 1.5,
            connector: Some("DP-1".into()),
            physical_mm: Some((597, 336)),
            logical: Some((2560, 1440)),
            rotated: false,
        }]);
        let Control::Popup { choices, selected } = size_row(&external).unwrap().control else {
            panic!("a popup");
        };
        assert_eq!(
            choices[selected],
            format!("2560{nb}×{nb}1440 (Default)", nb = '\u{202f}')
        );
    }

    #[test]
    fn without_a_battery_of_its_own_the_battery_pane_has_no_battery_level() {
        let live = Live {
            hardware: Some(crate::hardware::Hardware::default()),
            battery: crate::live::Reading::Unavailable("The power service is not running".into()),
            ..Live::default()
        };
        assert_eq!(live.has_battery(), Some(false));
        assert_eq!(
            sections(PaneId::Battery, &unset(), LIGHT, &live)
                .iter()
                .flat_map(|s| &s.rows)
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            [ControlId::SystemSleepAc]
        );
    }

    #[test]
    fn every_commit_has_at_most_one_effect() {
        use crate::live::{Bluetooth, BtDevice, Focus, Network, Reading, Volume, Wifi};
        let network = |ssid: &str, security: &str, saved, connected| Network {
            ssid: ssid.into(),
            name: ssid.into(),
            signal: -50,
            security: security.into(),
            saved,
            connected,
        };
        let device = |id: &str, paired| BtDevice {
            id: id.into(),
            address: id.into(),
            name: None,
            icon: "phone".into(),
            paired,
            connected: paired,
            pairing: false,
        };
        let live = Live {
            pictures: crate::live::Pictures::default(),
            mice: Reading::Unknown,
            focus: Reading::Ready(Focus {
                on: false,
                lifetimes: vec![("hour".into(), "For 1 hour".into())],
            }),
            volume: Reading::Ready(Volume {
                level: 0.5,
                muted: false,
            }),
            outputs: Vec::new(),
            displays: Reading::Ready(Vec::new()),
            brightness: Reading::Ready(0.5),
            battery: Reading::Ready(None),
            hardware: None,
            wifi: Reading::Ready(Wifi {
                enabled: true,
                networks: vec![
                    network("a", "wpa2", true, true),
                    network("b", "open", false, false),
                    network("c", "wpa2", false, false),
                ],
                ..Wifi::default()
            }),
            bluetooth: Reading::Ready(Bluetooth {
                available: true,
                powered: true,
                devices: vec![device("p", true), device("n", false)],
                ..Bluetooth::default()
            }),
            login_screen: Some(crate::live::LoginScreen::default()),
            apps: Vec::new(),
            notifying_apps: vec![("org.example.Mail".into(), "Mail".into())],
            screen_reader_installed: true,
        };
        for pane in crate::pane::PANES.iter().map(|p| p.id) {
            for r in sections(pane, &unset(), LIGHT, &live)
                .iter()
                .flat_map(|sec| &sec.rows)
            {
                // every value the control can commit
                let values: Vec<Value> = match &r.control {
                    Control::Switch { .. } => vec![Value::Bool(false), Value::Bool(true)],
                    Control::Segmented { segments, .. } => {
                        (0..segments.len()).map(Value::Index).collect()
                    }
                    Control::Popup { choices, .. } => {
                        (0..choices.len()).map(Value::Index).collect()
                    }
                    Control::Buttons { titles } => (0..titles.len()).map(Value::Index).collect(),
                    Control::Tiles { tiles, .. } => (0..tiles.len()).map(Value::Index).collect(),
                    Control::Preview { .. } => Vec::new(),
                    Control::Button { .. } => vec![Value::Index(0)],
                    Control::ButtonAndSwitch { .. } => {
                        vec![Value::Index(0), Value::Bool(false), Value::Bool(true)]
                    }
                    Control::Slider { stops: Some(n), .. } => (0..*n).map(Value::Index).collect(),
                    Control::Slider { stops: None, .. } | Control::Level { .. } => vec![
                        Value::Fraction(0.0),
                        Value::Fraction(0.5),
                        Value::Fraction(1.0),
                    ],
                    Control::Text { .. } => Vec::new(),
                };
                for value in values {
                    let effects = change(r.id, value, &mut unset(), &live);
                    assert!(effects.len() <= 1, "{:?} = {value:?}: {effects:?}", r.id);
                }
            }
        }
    }

    #[test]
    fn unset_files_show_the_reference_defaults() {
        let s = unset();
        assert_eq!(
            control(PaneId::Mouse, ControlId::MouseSpeed, &s),
            tracking(0.6875),
            "stop 3"
        );
        assert_eq!(
            control(PaneId::Mouse, ControlId::MouseAcceleration, &s),
            Control::Switch { on: true }
        );
        assert_eq!(
            control(PaneId::Trackpad, ControlId::TrackpadAcceleration, &s),
            Control::Switch { on: true }
        );
        assert_eq!(
            control(PaneId::DesktopDock, ControlId::AutoHide, &s),
            Control::Switch { on: false }
        );
        assert_eq!(
            control(PaneId::DesktopDock, ControlId::DockPosition, &s),
            Control::Segmented {
                segments: vec!["Bottom", "Left", "Right"],
                selected: 0
            }
        );
        let Control::Slider { value, .. } = control(PaneId::Keyboard, ControlId::RepeatRate, &s)
        else {
            panic!("a slider")
        };
        assert_eq!(value, 6.0 / 7.0, "83 ms: position 6 of 0…7");
        let Control::Slider { value, .. } = control(PaneId::Keyboard, ControlId::RepeatDelay, &s)
        else {
            panic!("a slider")
        };
        assert_eq!(value, 3.0 / 5.0, "500 ms: position 4 of 1…6");
        let Control::Popup { selected, .. } = control(
            PaneId::DesktopDock,
            ControlId::HotCorner(Trigger::CornerTopLeft),
            &s,
        ) else {
            panic!("a popup")
        };
        assert_eq!(selected, 0, "no corner does anything");
    }

    #[test]
    fn show_scroll_bars_starts_automatic_and_writes_the_compositor_file() {
        let mut s = unset();
        let Control::Popup { choices, selected } =
            control(PaneId::Appearance, ControlId::ShowScrollBars, &s)
        else {
            panic!("a popup")
        };
        assert_eq!(choices, ["Automatically", "When scrolling", "Always"]);
        assert_eq!(selected, 0);
        for (i, name) in ScrollBarsName::ALL.into_iter().enumerate() {
            assert_eq!(
                change(
                    ControlId::ShowScrollBars,
                    Value::Index(i),
                    &mut s,
                    &Live::default()
                ),
                [Effect::Store(File::Compositor)]
            );
            assert_eq!(s.compositor.show_scroll_bars, Some(name));
            let Control::Popup { selected, .. } =
                control(PaneId::Appearance, ControlId::ShowScrollBars, &s)
            else {
                panic!("a popup")
            };
            assert_eq!(selected, i);
        }
    }

    #[test]
    fn changes_write_the_mapped_values_to_their_file() {
        let mut s = unset();
        assert_eq!(
            change(
                ControlId::MouseSpeed,
                Value::Index(9),
                &mut s,
                &Live::default()
            ),
            [Effect::Store(File::Compositor)]
        );
        assert_eq!(s.compositor.mouse_speed, Some(3.0));
        assert_eq!(
            change(
                ControlId::TrackpadAcceleration,
                Value::Bool(false),
                &mut s,
                &Live::default()
            ),
            [Effect::Store(File::Compositor)]
        );
        assert_eq!(s.compositor.trackpad_acceleration, Some(false));
        change(
            ControlId::DockSize,
            Value::Fraction(0.5),
            &mut s,
            &Live::default(),
        );
        assert_eq!(s.dock.tile_size, Some(72.0));
        change(
            ControlId::Magnification,
            Value::Fraction(0.0),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.dock.magnification, s.dock.large_size),
            (Some(false), None),
            "Off keeps the magnified size"
        );
        change(
            ControlId::Magnification,
            Value::Fraction(0.125),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.dock.magnification, s.dock.large_size),
            (Some(true), Some(79.0)),
            "the Small mark magnifies the 72 pt tiles"
        );
        change(
            ControlId::DockSize,
            Value::Fraction(1.0),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.dock.tile_size, s.dock.large_size),
            (Some(128.0), Some(128.0)),
            "a larger dock keeps the magnified size at least its own"
        );
        change(
            ControlId::HotCorner(Trigger::CornerBottomRight),
            Value::Index(3),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            s.dock.actions.get(Trigger::CornerBottomRight),
            &ActionTarget::Server("show-desktop".into())
        );
        change(
            ControlId::Clock24,
            Value::Bool(true),
            &mut s,
            &Live::default(),
        );
        assert_eq!(s.menubar.clock.hour_cycle, HourCycle::H24);
        assert_eq!(
            change(
                ControlId::Appearance,
                Value::Index(1),
                &mut s,
                &Live::default()
            ),
            [Effect::SetAppearance { dark: true }]
        );
        assert_eq!(
            (
                control(PaneId::Mouse, ControlId::MouseNatural, &s),
                control(PaneId::Trackpad, ControlId::TrackpadNatural, &s)
            ),
            (Control::Switch { on: false }, Control::Switch { on: true }),
            "a wheel scrolls the view, a trackpad the content"
        );
        change(
            ControlId::MouseNatural,
            Value::Bool(true),
            &mut s,
            &Live::default(),
        );
        change(
            ControlId::TrackpadNatural,
            Value::Bool(false),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (
                s.compositor.mouse_natural_scrolling,
                s.compositor.natural_scrolling
            ),
            (Some(true), Some(false)),
            "each has its own value"
        );
    }

    #[test]
    fn repeat_off_keeps_the_delay_slider_and_turning_it_on_restores_it() {
        let mut s = unset();
        change(
            ControlId::RepeatDelay,
            Value::Index(1),
            &mut s,
            &Live::default(),
        );
        assert_eq!(s.keyboard.repeat_delay_ms, Some(1567), "position 2 of 1…6");
        change(
            ControlId::RepeatRate,
            Value::Index(0),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.keyboard.repeat_delay_ms, s.keyboard.repeat_delay_saved_ms),
            (Some(0), Some(1567)),
            "off saves the delay"
        );
        let Control::Slider { value, .. } = control(PaneId::Keyboard, ControlId::RepeatDelay, &s)
        else {
            panic!("a slider")
        };
        assert_eq!(
            value,
            1.0 / 5.0,
            "the delay slider keeps its place while repeat is off"
        );
        change(
            ControlId::RepeatDelay,
            Value::Index(4),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.keyboard.repeat_delay_ms, s.keyboard.repeat_delay_saved_ms),
            (Some(0), Some(417)),
            "moving it while off moves the saved delay"
        );
        change(
            ControlId::RepeatRate,
            Value::Index(7),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            (s.keyboard.repeat_delay_ms, s.keyboard.repeat_interval_ms),
            (Some(417), Some(33)),
            "on again with the saved delay"
        );
    }

    #[test]
    fn a_corner_action_outside_the_list_shows_as_an_extra_item_and_is_kept() {
        let mut s = unset();
        s.dock
            .actions
            .set(Trigger::CornerTopRight, ActionTarget::Exec("foot".into()));
        let Control::Popup { choices, selected } = control(
            PaneId::DesktopDock,
            ControlId::HotCorner(Trigger::CornerTopRight),
            &s,
        ) else {
            panic!("a popup")
        };
        assert_eq!(
            (choices.len(), selected, choices[selected].as_str()),
            (10, 9, "exec:foot")
        );
        assert!(change(
            ControlId::HotCorner(Trigger::CornerTopRight),
            Value::Index(9),
            &mut s,
            &Live::default()
        )
        .is_empty());
        assert_eq!(
            s.dock.actions.get(Trigger::CornerTopRight),
            &ActionTarget::Exec("foot".into())
        );
    }

    #[test]
    fn a_sleep_timer_outside_the_list_is_an_extra_item_kept_until_another_is_chosen() {
        let mut s = unset();
        s.power.ac.display_sleep = Timer::After(std::time::Duration::from_secs(420));
        let Control::Popup { choices, selected } =
            control(PaneId::LockScreen, ControlId::DisplaySleepAc, &s)
        else {
            panic!("a popup")
        };
        assert_eq!((choices[selected].as_str(), selected), ("7 minutes", 13));
        assert!(
            change(
                ControlId::DisplaySleepAc,
                Value::Index(13),
                &mut s,
                &Live::default()
            )
            .is_empty(),
            "choosing it again keeps it"
        );
        assert_eq!(
            change(
                ControlId::DisplaySleepBattery,
                Value::Index(12),
                &mut s,
                &Live::default()
            ),
            [Effect::Store(File::Power)]
        );
        assert_eq!(
            (s.power.battery.display_sleep, s.power.ac.display_sleep),
            (
                Timer::Never,
                Timer::After(std::time::Duration::from_secs(420))
            ),
            "another control's write keeps it"
        );
        change(
            ControlId::DisplaySleepAc,
            Value::Index(4),
            &mut s,
            &Live::default(),
        );
        assert_eq!(
            s.power.ac.display_sleep,
            Timer::After(std::time::Duration::from_secs(600)),
            "replaced only by the user's choice"
        );
        let Control::Popup { choices, selected } =
            control(PaneId::LockScreen, ControlId::RequirePassword, &s)
        else {
            panic!("a popup")
        };
        assert_eq!(choices[selected], "5 minutes", "the 300 s default");
    }

    #[test]
    fn the_steady_text_cursor_follows_the_accessibility_setting_and_asks_the_server() {
        let mut s = unset();
        let shown = |s: &Settings| {
            sections(PaneId::Accessibility, s, LIGHT, &Live::default())
                .into_iter()
                .flat_map(|sec| sec.rows)
                .find(|r| r.id == ControlId::SteadyCaret)
                .map(|r| r.control)
        };
        assert_eq!(shown(&s), Some(switch(false)));
        s.accessibility.steady_caret = true;
        assert_eq!(shown(&s), Some(switch(true)));
        assert_eq!(
            change(
                ControlId::SteadyCaret,
                Value::Bool(false),
                &mut s,
                &Live::default()
            ),
            [Effect::SetAccessibility {
                key: "steady_caret",
                value: "off".into()
            }]
        );
    }

    #[test]
    fn accessibility_display_options_go_to_the_server_and_the_rest_to_their_files() {
        let mut s = unset();
        assert_eq!(
            change(
                ControlId::ReduceMotion,
                Value::Bool(true),
                &mut s,
                &Live::default()
            ),
            [Effect::SetAccessibility {
                key: "reduce_motion",
                value: "on".into()
            }]
        );
        assert_eq!(
            change(
                ControlId::ColorFilter,
                Value::Index(1),
                &mut s,
                &Live::default()
            ),
            [Effect::SetAccessibility {
                key: "color_filter",
                value: "grayscale".into()
            }]
        );
        assert_eq!(
            change(
                ControlId::ColorFilterIntensity,
                Value::Fraction(1.0),
                &mut s,
                &Live::default()
            ),
            [Effect::SetAccessibility {
                key: "color_filter_intensity",
                value: "1.000".into()
            }]
        );
        assert_eq!(
            change(
                ControlId::CursorSize,
                Value::Fraction(1.0),
                &mut s,
                &Live::default()
            ),
            [Effect::Store(File::Compositor)]
        );
        assert_eq!(s.compositor.cursor_size, Some(4.0));
        change(
            ControlId::StickyKeys,
            Value::Bool(true),
            &mut s,
            &Live::default(),
        );
        assert_eq!(s.keyboard.sticky_keys, Some(true));
        assert!(
            !sections(PaneId::Accessibility, &s, LIGHT, &Live::default())
                .iter()
                .flat_map(|sec| &sec.rows)
                .any(|r| r.id == ControlId::ColorFilterIntensity),
            "no intensity without a filter"
        );
        s.accessibility.color_filter = Some(ColorFilter::Grayscale);
        let Control::Slider { value, .. } =
            control(PaneId::Accessibility, ControlId::ColorFilterIntensity, &s)
        else {
            panic!("a slider")
        };
        assert_eq!(value, 1.0, "grayscale's default intensity 1.0");
    }

    #[test]
    fn the_pointer_colours_start_white_around_black_and_keep_a_colour_of_the_file() {
        let mut s = unset();
        let popup = |id, s: &Settings| match control(PaneId::Accessibility, id, s) {
            Control::Popup { choices, selected } => (choices, selected),
            other => panic!("a pop-up, not {other:?}"),
        };
        let (choices, outline) = popup(ControlId::CursorOutline, &s);
        assert_eq!((choices[outline].as_str(), choices.len()), ("White", 8));
        let (choices, fill) = popup(ControlId::CursorFill, &s);
        assert_eq!(choices[fill], "Black");
        assert_eq!(
            change(
                ControlId::CursorFill,
                Value::Index(6),
                &mut s,
                &Live::default()
            ),
            [Effect::Store(File::Compositor)]
        );
        assert_eq!(s.compositor.cursor_fill, Some([30, 110, 235, 255]));
        assert_eq!(
            s.compositor.cursor_outline, None,
            "the outline is untouched"
        );
        s.compositor.cursor_outline = Some([1, 2, 3, 128]);
        let (choices, outline) = popup(ControlId::CursorOutline, &s);
        assert_eq!((choices[outline].as_str(), choices.len()), ("#01020380", 9));
        assert!(change(
            ControlId::CursorOutline,
            Value::Index(8),
            &mut s,
            &Live::default()
        )
        .is_empty());
        assert_eq!(s.compositor.cursor_outline, Some([1, 2, 3, 128]));
    }

    fn login_live(login: Option<LoginScreen>) -> Live {
        Live {
            login_screen: login,
            ..Live::default()
        }
    }

    fn login_row(live: &Live) -> Row {
        sections(PaneId::Accessibility, &unset(), LIGHT, live)
            .into_iter()
            .flat_map(|s| s.rows)
            .find(|r| r.id == ControlId::LoginScreen)
            .expect("the Login Screen row")
    }

    #[test]
    fn the_screen_reader_switch_shows_the_server_s_value_and_asks_the_server() {
        let reader = |settings: &Settings, installed: bool| {
            let live = Live {
                screen_reader_installed: installed,
                ..Live::default()
            };
            sections(PaneId::Accessibility, settings, LIGHT, &live)
                .into_iter()
                .flat_map(|s| s.rows)
                .find(|r| r.id == ControlId::ScreenReader)
                .expect("the Screen reader row")
        };
        let mut s = unset();
        let row = reader(&s, true);
        assert_eq!(
            (row.label.as_str(), &row.control, row.disabled.as_deref()),
            ("Screen reader", &Control::Switch { on: false }, None)
        );
        assert_eq!(row.detail.as_deref(), Some(SCREEN_READER_DETAIL));
        assert_eq!(row.backend, Backend::SetAccessibility("screen_reader"));
        assert_eq!(
            reader(&s, false).disabled.as_deref(),
            Some(NO_SCREEN_READER),
            "without a reader the switch says why it is off"
        );
        s.accessibility.screen_reader = true;
        assert_eq!(reader(&s, true).control, Control::Switch { on: true });
        for (on, value) in [(true, "on"), (false, "off")] {
            assert_eq!(
                change(
                    ControlId::ScreenReader,
                    Value::Bool(on),
                    &mut s,
                    &Live::default()
                ),
                [Effect::SetAccessibility {
                    key: "screen_reader",
                    value: value.into()
                }],
                "the server starts or stops the reader and stores the key"
            );
        }
    }

    #[test]
    fn the_login_screen_section_follows_display_only_when_sessiond_allows_it() {
        let headers = |live: &Live| {
            sections(PaneId::Accessibility, &unset(), LIGHT, live)
                .into_iter()
                .map(|s| s.header.unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            headers(&login_live(None)),
            ["Display", "Screen Reader", "Pointer", "Keyboard"],
            "not allowed, not answered, or no sessiond"
        );
        let live = login_live(Some(LoginScreen::default()));
        assert_eq!(
            headers(&live),
            [
                "Display",
                "Login Screen",
                "Screen Reader",
                "Pointer",
                "Keyboard"
            ]
        );
        let row = login_row(&live);
        assert_eq!(row.label, "Login screen and new users");
        assert_eq!(
            row.control,
            Control::Button {
                title: "Use Mine".into()
            }
        );
    }

    #[test]
    fn use_my_settings_sends_only_the_allow_listed_keys_with_the_values_shown() {
        let mut s = unset();
        s.accessibility.invert_colors = true;
        s.accessibility.display.reduce_motion = false;
        let session = Session {
            display: DisplayOptions {
                reduce_motion: true,
                increase_contrast: true,
                ..DisplayOptions::default()
            },
            ..LIGHT
        };
        assert_eq!(
            login_screen_options(session, &s),
            HashMap::from([("reduce_motion".to_string(), "on".to_string())]),
            "the session's value, and no invert or any other key"
        );
        assert_eq!(
            login_screen_options(LIGHT, &s),
            HashMap::from([("reduce_motion".to_string(), "off".to_string())])
        );
        let shown = login_live(Some(LoginScreen::default()));
        assert_eq!(
            change(ControlId::LoginScreen, Value::Index(0), &mut s, &shown),
            [Effect::SetSystemAccessibility]
        );
        assert!(
            change(
                ControlId::LoginScreen,
                Value::Index(0),
                &mut s,
                &login_live(None)
            )
            .is_empty(),
            "no section, no write"
        );
    }

    #[test]
    fn while_saving_the_row_says_saving_ignores_presses_and_the_reply_brings_the_button_back() {
        let mut s = unset();
        let saving = login_live(Some(LoginScreen {
            current: None,
            saving: true,
        }));
        assert_eq!(
            login_row(&saving).control,
            Control::Text {
                value: "Saving…".into()
            }
        );
        assert!(change(ControlId::LoginScreen, Value::Index(0), &mut s, &saving).is_empty());
        let written = cupertino_defaults::accessibility::SystemAccessibility::from_text(
            "reduce_motion = on\n",
        )
        .0
        .settings;
        let saved = login_live(Some(LoginScreen {
            current: Some(written),
            saving: false,
        }));
        assert_eq!(
            login_row(&saved).control,
            Control::Button {
                title: "Use Mine".into()
            }
        );
        let state = sections(PaneId::Accessibility, &s, LIGHT, &saved)[1]
            .rows
            .iter()
            .find(|r| r.label == "Login screen now")
            .map(|r| r.control.clone());
        assert_eq!(
            state,
            Some(Control::Text {
                value: "Reduce motion on".into()
            }),
            "the state row shows the file as written"
        );
    }

    #[test]
    fn the_login_screen_state_is_read_from_the_machines_file_and_is_default_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let current = crate::settings::system_accessibility(&dir.path().join("accessibility.conf"));
        assert_eq!(current, None);
        assert_eq!(login_screen_state(current.as_ref()), "Default settings");
        let off = cupertino_defaults::accessibility::SystemAccessibility::from_text(
            "reduce_motion = off\n",
        )
        .0
        .settings;
        assert_eq!(login_screen_state(Some(&off)), "Reduce motion off");
        let on = cupertino_defaults::accessibility::SystemAccessibility::from_text(
            "reduce_motion = on\n",
        )
        .0
        .settings;
        assert_eq!(login_screen_state(Some(&on)), "Reduce motion on");
        let section = login_screen_section(&LoginScreen::default());
        assert_eq!(section.footer.as_deref(), Some(LOGIN_SCREEN_FOOTER));
        assert!(
            LOGIN_SCREEN_FOOTER.len() <= 80,
            "one line at the narrowest window"
        );
    }

    #[test]
    fn the_keyboard_modifier_rows_show_the_defaults_and_keep_an_action_they_do_not_offer() {
        let mut s = unset();
        assert_eq!(
            control(PaneId::Keyboard, ControlId::CapsLockDelay, &s),
            Control::Switch { on: true },
            "75 ms by default"
        );
        let Control::Popup { choices, selected } = control(PaneId::Keyboard, ControlId::FnKey, &s)
        else {
            panic!("a popup")
        };
        assert_eq!(choices[selected], "Change Input Source", "");
        s.keyboard.fn_key = Some(FnKey::Emoji);
        let Control::Popup { choices, selected } = control(PaneId::Keyboard, ControlId::FnKey, &s)
        else {
            panic!("a popup")
        };
        assert_eq!((choices.len(), choices[selected].as_str()), (3, "emoji"));
        change(
            ControlId::CapsLockDelay,
            Value::Bool(false),
            &mut s,
            &Live::default(),
        );
        assert_eq!(s.keyboard.caps_lock_delay_ms, Some(0));
    }

    fn rows(pane: PaneId, live: &Live) -> Vec<Row> {
        sections(pane, &unset(), LIGHT, live)
            .into_iter()
            .flat_map(|sec| sec.rows)
            .collect()
    }

    #[test]
    fn a_missing_daemon_disables_the_switch_with_the_reason_and_keeps_the_pane() {
        let live = Live {
            wifi: Reading::Unavailable("Wi-Fi is not available".into()),
            bluetooth: Reading::Unavailable("Bluetooth is not available".into()),
            ..Live::default()
        };
        let wifi = rows(PaneId::Wifi, &live);
        assert_eq!(
            (
                wifi.len(),
                wifi[0].label.as_str(),
                wifi[0].disabled.as_deref()
            ),
            (1, "Wi-Fi", Some("Wi-Fi is not available"))
        );
        assert_eq!(
            rows(PaneId::Bluetooth, &live)[0].disabled.as_deref(),
            Some("Bluetooth is not available")
        );
        assert_eq!(
            rows(PaneId::Sound, &Live::default())[0].disabled.as_deref(),
            Some("Checking…"),
            "before the first read"
        );
        // a daemon without a Wi-Fi device
        let no_device = Live {
            wifi: Reading::Ready(Wifi {
                enabled: true,
                ..Wifi::default()
            }),
            ..Live::default()
        };
        assert_eq!(
            rows(PaneId::Wifi, &no_device)[0].disabled.as_deref(),
            Some("Wi-Fi is not available")
        );
    }

    #[test]
    fn focus_rows_are_the_lifetimes_in_order_and_call_the_centre() {
        let focus = crate::live::Focus {
            on: false,
            lifetimes: vec![
                ("hour".into(), "For 1 hour".into()),
                ("tonight".into(), "Until this evening".into()),
            ],
        };
        let live = Live {
            focus: Reading::Ready(focus),
            ..Live::default()
        };
        let r = rows(PaneId::Focus, &live);
        assert_eq!(
            r.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            ["Do Not Disturb", "For 1 hour", "Until this evening"]
        );
        let mut s = unset();
        assert_eq!(
            change(ControlId::DoNotDisturb, Value::Bool(true), &mut s, &live),
            [Effect::Service(Call::SetDoNotDisturb(true))]
        );
        assert_eq!(
            change(ControlId::FocusFor(1), Value::Index(0), &mut s, &live),
            [Effect::Service(Call::SetDoNotDisturbFor("tonight".into()))]
        );
    }

    #[test]
    fn wifi_lists_known_and_other_networks_and_joins_a_saved_one_without_a_passphrase() {
        let net = |ssid: &str, security: &str, saved: bool, connected: bool| crate::live::Network {
            ssid: ssid.into(),
            name: ssid.into(),
            signal: -40,
            security: security.into(),
            saved,
            connected,
        };
        let device = crate::live::Device {
            index: 3,
            name: "wlan0".into(),
            kind: "wifi".into(),
            state: "connected".into(),
            ssid: "home".into(),
            failure: String::new(),
            address: "10.0.0.2".into(),
        };
        let wifi = Wifi {
            enabled: true,
            devices: vec![device],
            state: "connected".into(),
            networks: vec![
                net("home", "wpa2", true, true),
                net("cafe", "open", false, false),
                net("office", "wpa2", false, false),
            ],
            joining: None,
            failure: None,
        };
        let live = Live {
            wifi: Reading::Ready(wifi),
            ..Live::default()
        };
        let secs = sections(PaneId::Wifi, &unset(), LIGHT, &live);
        assert_eq!(
            secs.iter().map(|s| s.header.clone()).collect::<Vec<_>>(),
            [
                None,
                Some("Known Networks".into()),
                Some("Other Networks".into())
            ]
        );
        assert_eq!(
            secs[2]
                .rows
                .iter()
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>(),
            ["cafe", "office (secured)"],
            "GetNetworks order"
        );
        let mut s = unset();
        assert_eq!(
            change(ControlId::WifiNetwork(0), Value::Index(0), &mut s, &live),
            [Effect::Service(Call::Disconnect)]
        );
        assert_eq!(
            change(ControlId::WifiNetwork(1), Value::Index(0), &mut s, &live),
            [Effect::Service(Call::Join {
                ssid: "cafe".into(),
                passphrase: Some(crate::sheet::Secret::new())
            })],
            "open: an empty passphrase"
        );
        assert_eq!(
            change(ControlId::WifiNetwork(2), Value::Index(0), &mut s, &live),
            [Effect::Service(Call::Join {
                ssid: "office".into(),
                passphrase: None
            })],
            "secured and unsaved: the sheet asks"
        );
        assert_eq!(
            rows(PaneId::Network, &live)[0].control,
            Control::Text {
                value: "connected · home · 10.0.0.2".into()
            }
        );
    }

    #[test]
    fn bluetooth_groups_paired_and_nearby_devices_and_forgets_only_after_confirmation() {
        let dev = |id: &str, paired: bool, connected: bool| crate::live::BtDevice {
            id: id.into(),
            address: id.into(),
            name: Some(format!("dev {id}")),
            icon: "headphones".into(),
            paired,
            connected,
            pairing: false,
        };
        let bt = Bluetooth {
            available: true,
            powered: true,
            discovering: true,
            devices: vec![dev("a", true, true), dev("b", false, false)],
            agent_elsewhere: false,
            failure: None,
        };
        let live = Live {
            bluetooth: Reading::Ready(bt),
            ..Live::default()
        };
        let secs = sections(PaneId::Bluetooth, &unset(), LIGHT, &live);
        assert_eq!(
            secs.iter().map(|s| s.header.clone()).collect::<Vec<_>>(),
            [
                None,
                Some("My Devices".into()),
                Some("Nearby Devices".into())
            ]
        );
        let mut s = unset();
        assert_eq!(
            change(
                ControlId::BluetoothDevice(0),
                Value::Index(0),
                &mut s,
                &live
            ),
            [Effect::Service(Call::DisconnectDevice("a".into()))]
        );
        assert_eq!(
            change(
                ControlId::BluetoothDevice(0),
                Value::Index(1),
                &mut s,
                &live
            ),
            [Effect::Service(Call::Remove {
                id: "a".into(),
                confirmed: false
            })],
            "Forget asks first"
        );
        assert_eq!(
            change(
                ControlId::BluetoothDevice(1),
                Value::Index(0),
                &mut s,
                &live
            ),
            [Effect::Service(Call::Pair("b".into()))]
        );
    }

    #[test]
    fn a_volume_below_the_threshold_mutes_and_the_mute_switch_keeps_the_level() {
        let live = Live {
            volume: Reading::Ready(crate::live::Volume {
                level: 0.6,
                muted: false,
            }),
            ..Live::default()
        };
        let mut s = unset();
        assert_eq!(
            change(ControlId::Volume, Value::Fraction(0.0005), &mut s, &live),
            [Effect::Service(Call::SetVolume {
                level: 0.0005,
                muted: true
            })]
        );
        assert_eq!(
            change(ControlId::Volume, Value::Fraction(0.3), &mut s, &live),
            [Effect::Service(Call::SetVolume {
                level: 0.3,
                muted: false
            })]
        );
        assert_eq!(
            change(ControlId::Mute, Value::Bool(true), &mut s, &live),
            [Effect::Service(Call::SetVolume {
                level: 0.6,
                muted: true
            })]
        );
    }

    #[test]
    fn the_sound_pane_lists_the_outputs_when_there_is_more_than_one_and_a_choice_sets_that_sink() {
        use crate::live::Output;
        let output = |id, name: &str, default| Output {
            id,
            name: name.into(),
            default,
        };
        let live = Live {
            outputs: vec![
                output(52, "Speakers", false),
                output(53, "Headphones", true),
            ],
            ..Live::default()
        };
        let first = &rows(PaneId::Sound, &live)[0];
        assert_eq!(first.id, ControlId::Output);
        assert_eq!(
            first.control,
            Control::Popup {
                choices: vec!["Speakers".into(), "Headphones".into()],
                selected: 1
            }
        );
        let mut s = unset();
        assert_eq!(
            change(ControlId::Output, Value::Index(0), &mut s, &live),
            [Effect::Service(Call::SetOutput(52))]
        );
        assert_eq!(
            change(ControlId::Output, Value::Index(7), &mut s, &live),
            []
        );
        let one = Live {
            outputs: vec![output(52, "Speakers", true)],
            ..Live::default()
        };
        assert_eq!(rows(PaneId::Sound, &one)[0].id, ControlId::Volume);
    }

    #[test]
    fn a_pane_shows_the_restart_footer_only_for_a_file_its_process_does_not_reload() {
        let s = unset();
        assert!(
            sections(PaneId::DesktopDock, &s, LIGHT, &Live::default())
                .iter()
                .all(|sec| sec.footer.is_none()),
            "every owner reloads its file"
        );
        assert!(File::ALL.iter().all(|f| f.reloads()));
        assert_eq!(
            restart_footer(File::Dock, false).as_deref(),
            Some("Takes effect when the dock restarts.")
        );
        assert_eq!(
            restart_footer(File::Keyboard, false).as_deref(),
            Some("Takes effect when the window server restarts.")
        );
        assert_eq!(restart_footer(File::Dock, true), None);
    }

    #[test]
    fn the_resolution_rows_list_at_most_32_mice_with_clean_names_of_at_most_128_characters() {
        let mice: Vec<Mouse> = (0..40)
            .map(|i| Mouse {
                key: format!("1:{i}:Mouse"),
                name: format!("Mouse\u{1b}[31m\n{}", "x".repeat(200)),
                dpi: 1000,
                stored: false,
            })
            .collect();
        let live = Live {
            mice: Reading::Ready(mice),
            ..Live::default()
        };
        let sections = sections(PaneId::Mouse, &unset(), LIGHT, &live);
        let rows = &sections[1].rows;
        assert_eq!(rows.len(), 32);
        assert!(rows.iter().all(|r| !r.label.chars().any(char::is_control)));
        assert!(rows
            .iter()
            .all(|r| r.label.chars().count() <= crate::live::MAX_NAME + 5));
    }

    fn picture(name: &str, dark: bool) -> WallpaperFile {
        WallpaperFile {
            name: name.to_string(),
            light: format!("/pics/{name}.jpg").into(),
            dark: dark.then(|| format!("/pics/{name}-dark.jpg").into()),
            credit: None,
        }
    }

    fn wallpaper_live() -> Live {
        Live {
            pictures: crate::live::Pictures {
                bundled: vec![picture("snow-ridge", false), picture("sunrise_bay", true)],
                folders: vec![("/home/u/Art".into(), vec![picture("mine", false)])],
                default: Some("/pics/snow-ridge.jpg".into()),
                ..Default::default()
            },
            ..Live::default()
        }
    }

    fn tiles_of(section: &Section) -> (Vec<(String, bool)>, Option<usize>) {
        match &section.rows[0].control {
            Control::Tiles { tiles, selected } => (
                tiles.iter().map(|t| (t.title.clone(), t.pair)).collect(),
                *selected,
            ),
            other => panic!("a grid, got {other:?}"),
        }
    }

    #[test]
    fn the_wallpaper_pane_lists_the_packs_pictures_then_each_folder_and_marks_the_shown_one() {
        let live = wallpaper_live();
        let mut settings = unset();
        let sections = sections(PaneId::Wallpaper, &settings, LIGHT, &live);
        assert_eq!(
            sections
                .iter()
                .map(|s| s.header.as_deref())
                .collect::<Vec<_>>(),
            [Some("Current"), Some("Pictures"), Some("Art"), None, None]
        );
        assert_eq!(
            tiles_of(&sections[1]),
            (
                vec![
                    ("Snow ridge".to_string(), false),
                    ("Sunrise bay".to_string(), true)
                ],
                Some(0)
            ),
            "with nothing stored the pack's default picture is the shown one"
        );
        settings.compositor.wallpaper = Some("/pics/sunrise_bay.jpg".into());
        settings.compositor.wallpaper_dark = Some("/pics/sunrise_bay-dark.jpg".into());
        let sections = super::sections(PaneId::Wallpaper, &settings, LIGHT, &live);
        assert_eq!(tiles_of(&sections[1]).1, Some(1));
        assert_eq!(tiles_of(&sections[2]).1, None);
        settings.compositor.wallpaper_dark = None;
        let sections = super::sections(PaneId::Wallpaper, &settings, LIGHT, &live);
        assert_eq!(
            tiles_of(&sections[1]).1,
            None,
            "a pair is shown only while both of its files are stored"
        );
    }

    #[test]
    fn choosing_a_picture_stores_its_light_file_and_its_dark_one_or_forgets_the_old_one() {
        let live = wallpaper_live();
        let mut settings = unset();
        settings.compositor.wallpaper_dark = Some("/old-dark.jpg".into());
        let effects = change(
            ControlId::Wallpaper(0),
            Value::Index(0),
            &mut settings,
            &live,
        );
        assert_eq!(effects, [Effect::Store(File::Compositor)]);
        assert_eq!(
            settings.compositor.wallpaper.as_deref(),
            Some("/pics/snow-ridge.jpg")
        );
        assert_eq!(
            settings.compositor.wallpaper_dark, None,
            "a single picture serves both appearances"
        );
        change(
            ControlId::Wallpaper(0),
            Value::Index(1),
            &mut settings,
            &live,
        );
        assert_eq!(
            settings.compositor.wallpaper.as_deref(),
            Some("/pics/sunrise_bay.jpg")
        );
        assert_eq!(
            settings.compositor.wallpaper_dark.as_deref(),
            Some("/pics/sunrise_bay-dark.jpg")
        );
        change(
            ControlId::Wallpaper(1),
            Value::Index(0),
            &mut settings,
            &live,
        );
        assert_eq!(
            settings.compositor.wallpaper.as_deref(),
            Some("/pics/mine.jpg")
        );
        assert!(change(
            ControlId::Wallpaper(2),
            Value::Index(0),
            &mut settings,
            &live
        )
        .is_empty());
        assert!(change(
            ControlId::Wallpaper(0),
            Value::Index(9),
            &mut settings,
            &live
        )
        .is_empty());
    }

    #[test]
    fn a_credited_picture_shows_its_title_and_photographer_and_the_grid_makes_room() {
        let mut live = wallpaper_live();
        live.pictures.bundled[0].credit = Some(cupertino_assets::Credit {
            title: "Trees covering snow".into(),
            author: "Ryan Pohanic".into(),
        });
        let sections = sections(PaneId::Wallpaper, &unset(), LIGHT, &live);
        let Control::Tiles { tiles, .. } = &sections[1].rows[0].control else {
            panic!("a grid");
        };
        assert_eq!(tiles[0].title, "Trees covering snow");
        assert_eq!(tiles[0].credit.as_deref(), Some("Ryan Pohanic"));
        assert_eq!(
            tiles[1].title, "Sunrise bay",
            "an uncredited picture is named by its file"
        );
        assert_eq!(tiles[1].credit, None);
        assert!(
            crate::controls::tile_caption_height(tiles)
                > crate::controls::tile_caption_height(&tiles[1..])
        );
    }

    #[test]
    fn the_wallpaper_pane_shows_the_current_picture_and_offers_removing_a_folder_and_adding_more() {
        let live = wallpaper_live();
        let mut settings = unset();
        settings.compositor.wallpaper = Some("/pics/sunrise_bay.jpg".into());
        settings.compositor.wallpaper_dark = Some("/pics/sunrise_bay-dark.jpg".into());
        settings.compositor.wallpaper_folders = Some(vec!["/home/u/Art".into(), "/mnt/x".into()]);
        let sections = sections(PaneId::Wallpaper, &settings, LIGHT, &live);
        assert_eq!(sections[0].header.as_deref(), Some("Current"));
        let Control::Preview { image, title, note } = &sections[0].rows[0].control else {
            panic!("the preview");
        };
        assert_eq!(image.as_deref(), Some(Path::new("/pics/sunrise_bay.jpg")));
        assert_eq!(title, "Sunrise bay");
        assert!(note
            .as_deref()
            .is_some_and(|n| n.contains("Light and dark")));
        let remove = sections
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.id == ControlId::WallpaperRemoveFolder(0))
            .expect("a remove row for the folder");
        assert_eq!(remove.label, "/home/u/Art");
        let add = sections.last().unwrap().rows.last().unwrap();
        assert_eq!(add.id, ControlId::WallpaperAdd);
        let Control::Buttons { titles } = &add.control else {
            panic!("buttons");
        };
        assert_eq!(titles, &["Add Folder\u{2026}", "Add Photo\u{2026}"]);
        let ask = |i| {
            change(
                ControlId::WallpaperAdd,
                Value::Index(i),
                &mut unset(),
                &live,
            )
        };
        assert_eq!(ask(0), [Effect::AskPicture { folder: true }]);
        assert_eq!(ask(1), [Effect::AskPicture { folder: false }]);
    }

    #[test]
    fn removing_a_folder_forgets_only_that_one_and_the_last_removal_clears_the_key() {
        let live = Live {
            pictures: crate::live::Pictures {
                folders: vec![("/a".into(), vec![]), ("/b".into(), vec![])],
                ..Default::default()
            },
            ..Live::default()
        };
        let mut settings = unset();
        settings.compositor.wallpaper_folders = Some(vec!["/a".into(), "/b".into()]);
        let effects = change(
            ControlId::WallpaperRemoveFolder(0),
            Value::Index(0),
            &mut settings,
            &live,
        );
        assert_eq!(effects, [Effect::Store(File::Compositor)]);
        assert_eq!(
            settings.compositor.wallpaper_folders,
            Some(vec!["/b".to_string()])
        );
        change(
            ControlId::WallpaperRemoveFolder(1),
            Value::Index(0),
            &mut settings,
            &live,
        );
        assert_eq!(settings.compositor.wallpaper_folders, None);
        assert!(change(
            ControlId::WallpaperRemoveFolder(5),
            Value::Index(0),
            &mut settings,
            &live
        )
        .is_empty());
    }

    #[test]
    fn a_group_whose_listing_has_not_come_back_says_loading_and_then_its_pictures() {
        let mut live = Live {
            pictures: crate::live::Pictures {
                folders: vec![("/mnt/slow".into(), vec![])],
                loading: [0, 1].into(),
                ..Default::default()
            },
            ..Live::default()
        };
        let text =
            |live: &Live, section: usize| match &sections(PaneId::Wallpaper, &unset(), LIGHT, live)
                [section]
                .rows[0]
                .control
            {
                Control::Text { value } => value.clone(),
                other => panic!("a note, got {other:?}"),
            };
        assert_eq!(text(&live, 1), "Loading\u{2026}");
        assert_eq!(text(&live, 2), "Loading\u{2026}");
        live.pictures.loading.clear();
        assert_eq!(text(&live, 1), "No pictures");
        live.pictures.bundled = vec![picture("one", false)];
        live.pictures.folders[0].1 = vec![picture("two", false)];
        let sections = sections(PaneId::Wallpaper, &unset(), LIGHT, &live);
        assert!(matches!(sections[1].rows[0].control, Control::Tiles { .. }));
        assert!(matches!(sections[2].rows[0].control, Control::Tiles { .. }));
    }

    #[test]
    fn an_app_menu_closes_when_the_list_of_apps_changes_under_it() {
        let s = unset();
        let mail = Live {
            notifying_apps: vec![("org.example.Mail".into(), "Mail".into())],
            ..Live::default()
        };
        let both = Live {
            notifying_apps: vec![
                ("org.example.Mail".into(), "Mail".into()),
                ("org.example.Bank".into(), "Bank".into()),
            ],
            ..Live::default()
        };
        let (before, after) = (notification_apps(&s, &mail), notification_apps(&s, &both));
        // Bank sorts before Mail: the menu opened on Mail's row 0 would now set Bank's
        assert_eq!(before[0].1, "Mail");
        assert_eq!(after[0].1, "Bank");
        for open in [
            ControlId::NotificationAppPreviews(0),
            ControlId::NotificationAppLockScreen(0),
        ] {
            assert!(!menu_survives(open, &before, &after));
            assert!(menu_survives(open, &after, &after));
        }
        assert!(menu_survives(
            ControlId::NotificationsPreviews,
            &before,
            &after
        ));
    }

    #[test]
    fn the_notifications_pane_lists_the_default_then_each_app_with_default_first() {
        use cupertino_defaults::notifications::AppOverride;
        let mut s = unset();
        assert!(s.notifications.set_app(
            "org.example.Mail",
            AppOverride {
                show_on_lock_screen: Some(false),
                show_previews: None,
            },
        ));
        let live = Live {
            notifying_apps: vec![
                ("org.example.Chat".into(), "Chat".into()),
                ("org.example.Mail".into(), "Mail".into()),
                // an id the file cannot hold is not listed
                ("bad=id".into(), "Bad".into()),
            ],
            ..Live::default()
        };
        let shown = sections(PaneId::Notifications, &s, LIGHT, &live);
        let headers: Vec<_> = shown.iter().map(|s| s.header.as_deref()).collect();
        assert_eq!(
            headers,
            [
                Some("Lock Screen"),
                Some("Application Notifications"),
                Some("Chat"),
                Some("Mail")
            ]
        );
        assert_eq!(
            shown[0].rows.iter().map(|r| &r.control).collect::<Vec<_>>(),
            [&Control::Switch { on: true }, &popup(PREVIEW_CHOICES, 1),],
            "unset: on the lock screen, previews when unlocked"
        );
        assert!(shown[1].rows.is_empty(), "a title over the apps");
        let popup_of = |section: &Section, row: usize| match &section.rows[row].control {
            Control::Popup { choices, selected } => (choices.clone(), *selected),
            other => panic!("a pop-up, not {other:?}"),
        };
        assert_eq!(
            popup_of(&shown[2], 0),
            (vec!["Default (On)".into(), "On".into(), "Off".into()], 0)
        );
        assert_eq!(popup_of(&shown[3], 0).1, 2, "Mail: off");
        assert_eq!(
            popup_of(&shown[3], 1),
            (
                vec![
                    "Default (When Unlocked)".into(),
                    "Always".into(),
                    "When Unlocked".into(),
                    "Never".into()
                ],
                0
            )
        );
        // the commit acts on the app shown at that index
        assert_eq!(
            change(
                ControlId::NotificationAppPreviews(0),
                Value::Index(3),
                &mut s,
                &live
            ),
            [Effect::Store(File::Notifications)]
        );
        assert_eq!(
            s.notifications.policy("org.example.Chat").show_previews,
            Previews::Never
        );
        assert_eq!(
            change(
                ControlId::NotificationAppPreviews(9),
                Value::Index(1),
                &mut s,
                &live
            ),
            [],
            "no app at that index"
        );
        let none = sections(PaneId::Notifications, &unset(), LIGHT, &Live::default());
        assert_eq!(none.len(), 2);
        assert_eq!(none[1].rows[0].label, "No applications");
        let many = Live {
            notifying_apps: (0..MAX_NOTIFICATION_APPS + 9)
                .map(|i| (format!("org.example.App{i}"), format!("App {i}")))
                .collect(),
            ..Live::default()
        };
        assert_eq!(
            notification_apps(&unset(), &many).len(),
            MAX_NOTIFICATION_APPS
        );
    }
}
