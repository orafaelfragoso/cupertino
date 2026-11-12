//! The sections of the panes added for the account, General, Appearance, Menu Bar and the Dock's windows and browser
//! options, and the choices their changes make (the rest of the panes are in [`crate::panes`]).

use cupertino_defaults::appearance::{
    time_text, Accent, Highlight, Minutes, ScrollClick, SidebarIcons, TitlebarAction,
};
use cupertino_defaults::dock::StackStyle;

use crate::controls::{Control, SwatchKind};
use crate::live::Live;
use crate::panes::{row, Backend, ControlId, Row, Section};
use crate::settings::{File, Settings};
use crate::system::size_text;

fn text_row(label: &str, value: impl Into<String>) -> Row {
    row(
        ControlId::Reading,
        label,
        Control::Text {
            value: value.into(),
        },
        Backend::Service("system"),
    )
}

fn section(header: Option<&str>, rows: Vec<Row>, footer: Option<String>) -> Section {
    Section {
        header: header.map(str::to_owned),
        rows,
        footer,
    }
}

fn popup_of(choices: Vec<String>, selected: usize) -> Control {
    Control::Popup { choices, selected }
}

/// The accent colour names and the highlight names, as the pop-ups and swatches show them.
pub fn accent_label(accent: Accent) -> &'static str {
    match accent {
        Accent::Multicolor => "Multicolour",
        Accent::Blue => "Blue",
        Accent::Purple => "Purple",
        Accent::Pink => "Pink",
        Accent::Red => "Red",
        Accent::Orange => "Orange",
        Accent::Yellow => "Yellow",
        Accent::Green => "Green",
        Accent::Graphite => "Graphite",
    }
}

/// The highlight choices: automatic, then every accent but the multicolour.
pub fn highlight_choices() -> Vec<Highlight> {
    std::iter::once(Highlight::Automatic)
        .chain(
            Accent::ALL
                .into_iter()
                .filter(|a| *a != Accent::Multicolor)
                .map(Highlight::Color),
        )
        .collect()
}

pub const SIDEBAR_ICON_CHOICES: [(&str, SidebarIcons); 3] = [
    ("Small", SidebarIcons::Small),
    ("Medium", SidebarIcons::Medium),
    ("Large", SidebarIcons::Large),
];

pub const SCROLL_CLICK_CHOICES: [(&str, ScrollClick); 2] = [
    ("Jump to the next page", ScrollClick::NextPage),
    ("Jump to the spot that's clicked", ScrollClick::JumpToSpot),
];

pub const TITLE_ACTION_CHOICES: [(&str, TitlebarAction); 3] = [
    ("Zoom", TitlebarAction::Zoom),
    ("Minimize", TitlebarAction::Minimize),
    ("Do Nothing", TitlebarAction::None),
];

pub const STACK_CHOICES: [(&str, StackStyle); 3] = [
    ("Automatic", StackStyle::Auto),
    ("Fan", StackStyle::Fan),
    ("Grid", StackStyle::Grid),
];

/// The counts the recent-items pop-ups offer.
pub const RECENT_COUNTS: [u32; 7] = [0, 5, 10, 15, 20, 30, 50];

/// A time-of-day pop-up on the hour, a stored time between hours shown as one more item.
fn time_popup(minutes: Minutes) -> Control {
    let mut choices: Vec<String> = (0..24).map(|h| time_text(h * 60)).collect();
    let selected = if minutes % 60 == 0 && minutes < 24 * 60 {
        (minutes / 60) as usize
    } else {
        choices.push(time_text(minutes));
        choices.len() - 1
    };
    popup_of(choices, selected)
}

/// The hour list's time at `index`, as minutes after midnight.
pub fn time_of_choice(index: usize, current: Minutes) -> Minutes {
    if index < 24 {
        index as Minutes * 60
    } else {
        current
    }
}

pub fn appearance_sections(settings: &Settings, dark: bool) -> Vec<Section> {
    let a = &settings.appearance;
    let theme = if a.auto {
        0
    } else if dark {
        2
    } else {
        1
    };
    let file = |key| Backend::Key(File::Appearance, key);
    let mut theme_rows = vec![row(
        ControlId::Appearance,
        "Appearance",
        Control::Swatches {
            items: vec![
                ("Auto".into(), SwatchKind::Auto),
                ("Light".into(), SwatchKind::Light),
                ("Dark".into(), SwatchKind::Dark),
            ],
            selected: theme,
        },
        file("auto"),
    )];
    if a.auto {
        theme_rows.push(row(
            ControlId::AutoDarkFrom,
            "Switch to dark at",
            time_popup(a.auto_dark_from),
            file("auto_dark_from"),
        ));
        theme_rows.push(row(
            ControlId::AutoLightFrom,
            "Switch to light at",
            time_popup(a.auto_light_from),
            file("auto_light_from"),
        ));
    }
    let accents: Vec<(String, SwatchKind)> = Accent::ALL
        .into_iter()
        .map(|accent| {
            let kind = match accent {
                Accent::Multicolor => SwatchKind::Multicolor,
                other => SwatchKind::Color(other.rgb(dark)),
            };
            (accent_label(accent).to_owned(), kind)
        })
        .collect();
    let highlights: Vec<String> = highlight_choices()
        .into_iter()
        .map(|h| match h {
            Highlight::Automatic => "Accent Colour".to_owned(),
            Highlight::Color(c) => accent_label(c).to_owned(),
        })
        .collect();
    let highlight = highlight_choices()
        .iter()
        .position(|h| *h == a.highlight)
        .unwrap_or(0);
    let scroll_click = SCROLL_CLICK_CHOICES
        .iter()
        .position(|(_, v)| *v == a.scroll_click)
        .unwrap_or(0);
    vec![
        section(None, theme_rows, None),
        section(
            Some("Materials"),
            vec![
                row(
                    ControlId::MaterialDensity,
                    "Transparency",
                    Control::Slider {
                        value: settings.compositor.material_density.unwrap_or(0.0),
                        stops: None,
                        marks: &[],
                        labels: ("Clear", "Dense"),
                    },
                    Backend::Key(File::Compositor, "material_density"),
                ),
                row(
                    ControlId::TintWindows,
                    "Tint window background with wallpaper colour",
                    Control::Switch { on: a.tint_windows },
                    file("tint_windows"),
                ),
            ],
            None,
        ),
        section(
            Some("Accent"),
            vec![
                row(
                    ControlId::AccentColor,
                    "Accent colour",
                    Control::Swatches {
                        items: accents,
                        selected: Accent::ALL.iter().position(|x| *x == a.accent).unwrap_or(0),
                    },
                    file("accent"),
                ),
                row(
                    ControlId::HighlightColor,
                    "Highlight colour",
                    popup_of(highlights, highlight),
                    file("highlight"),
                ),
                row(
                    ControlId::SidebarIconSize,
                    "Sidebar icon size",
                    popup_of(
                        SIDEBAR_ICON_CHOICES
                            .iter()
                            .map(|(l, _)| l.to_string())
                            .collect(),
                        SIDEBAR_ICON_CHOICES
                            .iter()
                            .position(|(_, v)| *v == a.sidebar_icons)
                            .unwrap_or(1),
                    ),
                    file("sidebar_icons"),
                ),
            ],
            None,
        ),
        section(
            Some("Scroll Bars"),
            vec![
                crate::panes::scroll_bars_row(settings),
                row(
                    ControlId::ScrollClick,
                    "Click in the scroll bar to",
                    popup_of(
                        SCROLL_CLICK_CHOICES
                            .iter()
                            .map(|(l, _)| l.to_string())
                            .collect(),
                        scroll_click,
                    ),
                    file("scroll_click"),
                ),
            ],
            None,
        ),
    ]
}

/// The extra rows of the Dock section and the windows and browser sections of Desktop & Dock.
pub fn dock_dock_rows(settings: &Settings) -> Vec<Row> {
    let d = &settings.dock;
    let dock = |key| Backend::Key(File::Dock, key);
    vec![
        row(
            ControlId::MinimizeIntoIcon,
            "Minimize windows into application icon",
            Control::Switch {
                on: d.minimize_to_app.unwrap_or(false),
            },
            dock("minimize_to_app"),
        ),
        row(
            ControlId::LaunchBounce,
            "Animate opening applications",
            Control::Switch {
                on: d.bounce.unwrap_or(true),
            },
            dock("bounce"),
        ),
        row(
            ControlId::RunningIndicators,
            "Show indicators for open applications",
            Control::Switch {
                on: d.indicators.unwrap_or(true),
            },
            dock("indicators"),
        ),
        row(
            ControlId::StackStyle,
            "Open folders as",
            popup_of(
                STACK_CHOICES.iter().map(|(l, _)| l.to_string()).collect(),
                STACK_CHOICES
                    .iter()
                    .position(|(_, v)| *v == d.stack.unwrap_or_default())
                    .unwrap_or(0),
            ),
            dock("stack"),
        ),
    ]
}

/// The solid colours the desktop can show instead of a picture: a name and an sRGB colour.
pub const WALLPAPER_COLORS: [(&str, [u8; 3]); 8] = [
    ("Slate", [0x3a, 0x3f, 0x4a]),
    ("Graphite", [0x1c, 0x1c, 0x1e]),
    ("Blue", [0x2f, 0x6f, 0xc4]),
    ("Teal", [0x2a, 0x8f, 0x8f]),
    ("Green", [0x4a, 0x9a, 0x5c]),
    ("Orange", [0xd9, 0x7a, 0x2b]),
    ("Pink", [0xcf, 0x5f, 0x8f]),
    ("Sand", [0xd8, 0xcf, 0xbf]),
];

/// The Colours section of the wallpaper pane: a press on a colour shows it alone as the desktop.
pub fn wallpaper_colors_section(settings: &Settings) -> Section {
    let chosen = settings
        .compositor
        .wallpaper_color
        .and_then(|c| {
            WALLPAPER_COLORS
                .iter()
                .position(|(_, rgb)| rgb[..] == c[..3])
        })
        .unwrap_or(usize::MAX);
    section(
        Some("Colours"),
        vec![row(
            ControlId::WallpaperColor,
            "Solid colour",
            Control::Swatches {
                items: WALLPAPER_COLORS
                    .iter()
                    .map(|(name, rgb)| (name.to_string(), SwatchKind::Color(*rgb)))
                    .collect(),
                selected: chosen,
            },
            Backend::Key(File::Compositor, "wallpaper_color"),
        )],
        Some("Choosing a picture shows the picture again.".into()),
    )
}

/// The Windows section of the Desktop & Dock pane.
pub fn windows_section(settings: &Settings) -> Section {
    section(
        Some("Windows"),
        vec![
            row(
                ControlId::EdgeTiling,
                "Tile windows by dragging them to the screen edges",
                Control::Switch {
                    on: settings.compositor.edge_tiling.unwrap_or(true),
                },
                Backend::Key(File::Compositor, "edge_tiling"),
            ),
            row(
                ControlId::TileMargins,
                "Margins around tiled windows",
                Control::Switch {
                    on: settings.compositor.tile_margins.unwrap_or(true),
                },
                Backend::Key(File::Compositor, "tile_margins"),
            ),
            row(
                ControlId::DoubleClickTitle,
                "Double-click a window's title bar to",
                popup_of(
                    TITLE_ACTION_CHOICES
                        .iter()
                        .map(|(l, _)| l.to_string())
                        .collect(),
                    TITLE_ACTION_CHOICES
                        .iter()
                        .position(|(_, v)| *v == settings.appearance.titlebar_double_click)
                        .unwrap_or(0),
                ),
                Backend::Key(File::Appearance, "titlebar_double_click"),
            ),
        ],
        None,
    )
}

pub fn browser_section(live: &Live) -> Option<Section> {
    let (browsers, default) = &live.browsers;
    if browsers.is_empty() {
        return None;
    }
    Some(section(
        Some("Default Browser"),
        vec![row(
            ControlId::DefaultBrowser,
            "Default web browser",
            popup_of(
                browsers.iter().map(|b| b.name.clone()).collect(),
                default.unwrap_or(0),
            ),
            Backend::Service("mimeapps.list"),
        )],
        None,
    ))
}

pub fn account_sections(live: &Live) -> Vec<Section> {
    let Some(info) = live.system.as_deref() else {
        return vec![section(None, vec![text_row("Account", "Checking…")], None)];
    };
    let account = &info.account;
    let mut rows = vec![
        text_row("Name", account.full_name.clone()),
        text_row("User name", account.username.clone()),
        text_row(
            "Account type",
            if account.administrator {
                "Administrator"
            } else {
                "Standard"
            },
        ),
        text_row("Home folder", crate::panes::home_relative(&account.home)),
    ];
    if let Some(host) = &info.hostname {
        rows.push(text_row("Computer", host.clone()));
    }
    let mut sections = vec![section(None, rows, None)];
    if let Some(login) = &info.login {
        let yes_no = |v: bool| if v { "On" } else { "Off" };
        sections.push(section(
            Some("Login Options"),
            vec![
                text_row(
                    "Automatic login",
                    login.auto_login.clone().unwrap_or_else(|| "Off".to_owned()),
                ),
                text_row(
                    "Show other users",
                    login.show_other_users.map_or("By default", yes_no),
                ),
                text_row(
                    "Hide administrator accounts",
                    yes_no(login.hide_admin_users),
                ),
                text_row(
                    "Show a password hint after",
                    format!("{} attempts", login.retries_until_hint),
                ),
            ],
            Some("The administrator sets these in the system's login configuration.".into()),
        ));
    }
    sections
}

pub fn general_sections(live: &Live) -> Vec<Section> {
    let Some(info) = live.system.as_deref() else {
        return vec![section(
            Some("About"),
            vec![text_row("Computer", "Checking…")],
            None,
        )];
    };
    let mut about = Vec::new();
    if let Some(host) = &info.hostname {
        about.push(text_row("Name", host.clone()));
    }
    if let Some(a) = &info.about {
        for r in &a.rows {
            about.push(text_row(r.label, r.value.clone()));
        }
        if let Some(os) = &a.os {
            about.insert(about.len().min(1), text_row("System", os.clone()));
        }
    }
    let mut update = Vec::new();
    if let Some(name) = &info.os_name {
        update.push(text_row("System", name.clone()));
    }
    if let Some(v) = &info.os_version {
        update.push(text_row("Version", v.clone()));
    }
    if let Some(b) = &info.os_build {
        update.push(text_row("Build", b.clone()));
    }
    update.push(text_row("Desktop", info.desktop_version.clone()));
    if let Some(c) = &info.commit {
        update.push(text_row("Revision", c.clone()));
    }
    let storage: Vec<Row> = info
        .mounts
        .iter()
        .map(|m| {
            let name = if m.target == std::path::Path::new("/") {
                "Startup disk".to_owned()
            } else {
                m.target.display().to_string()
            };
            let mut r = text_row(
                &name,
                format!("{} available of {}", size_text(m.free), size_text(m.total)),
            );
            r.detail = Some(format!("{} ({})", m.device, m.fs));
            r
        })
        .collect();
    let mut sections = vec![
        section(Some("About"), about, None),
        section(
            Some("Software Update"),
            update,
            Some("Updates arrive through this computer's package manager.".into()),
        ),
    ];
    if !storage.is_empty() {
        sections.push(section(Some("Storage"), storage, None));
    }
    let mut when = Vec::new();
    when.push(text_row(
        "Time zone",
        info.timezone.clone().unwrap_or_else(|| "Unknown".into()),
    ));
    sections.push(section(
        Some("Date & Time"),
        when,
        Some("The menu bar's clock is set in Menu Bar.".into()),
    ));
    sections.push(section(
        Some("Language & Region"),
        vec![text_row(
            "Language and region",
            info.locale.clone().unwrap_or_else(|| "Unknown".into()),
        )],
        None,
    ));
    let items: Vec<Row> = live
        .login_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            row(
                ControlId::LoginItem(i),
                item.name.clone(),
                Control::Switch { on: item.enabled },
                Backend::Service("autostart"),
            )
        })
        .collect();
    sections.push(section(
        Some("Login Items"),
        if items.is_empty() {
            vec![text_row("Open at login", "Nothing opens at login")]
        } else {
            items
        },
        Some("These programs open when you sign in.".into()),
    ));
    sections
}

/// The status icons the Menu Bar pane lists: those running now, then those only named in `menubar.conf`.
pub fn tray_list(settings: &Settings, live: &Live) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = live
        .tray
        .iter()
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect();
    for id in &settings.menubar.hidden_tray {
        if !list.iter().any(|(i, _)| i == id) {
            list.push((id.clone(), id.clone()));
        }
    }
    list
}

fn count_popup(value: u32) -> Control {
    let mut choices: Vec<String> = RECENT_COUNTS
        .iter()
        .map(|n| {
            if *n == 0 {
                "None".to_owned()
            } else {
                n.to_string()
            }
        })
        .collect();
    let selected = match RECENT_COUNTS.iter().position(|n| *n == value) {
        Some(i) => i,
        None => {
            choices.push(value.to_string());
            choices.len() - 1
        }
    };
    popup_of(choices, selected)
}

/// The count of the recent-items pop-up's choice `index`, `current` when it is the extra item.
pub fn count_of_choice(index: usize, current: u32) -> u32 {
    RECENT_COUNTS.get(index).copied().unwrap_or(current)
}

pub fn menu_bar_head_sections(settings: &Settings) -> Vec<Section> {
    let m = &settings.menubar;
    let key = |k| Backend::Key(File::MenuBar, k);
    vec![section(
        None,
        vec![
            row(
                ControlId::MenuBarAutoHide,
                "Automatically hide and show the menu bar",
                Control::Switch {
                    on: settings.compositor.menu_bar_auto_hide.unwrap_or(false),
                },
                Backend::Key(File::Compositor, "menu_bar_auto_hide"),
            ),
            row(
                ControlId::MenuBarBackground,
                "Show menu bar background",
                Control::Switch { on: m.background },
                key("show.background"),
            ),
            row(
                ControlId::ShowClock,
                "Show the clock",
                Control::Switch { on: m.shown.clock },
                key("show.clock"),
            ),
        ],
        None,
    )]
}

pub fn menu_bar_tail_sections(settings: &Settings, live: &Live) -> Vec<Section> {
    let m = &settings.menubar;
    let key = |k| Backend::Key(File::MenuBar, k);
    let mut sections = vec![section(
        Some("Recent Items"),
        vec![
            row(
                ControlId::RecentDocuments,
                "Recent documents",
                count_popup(m.recent_documents),
                key("recent.documents"),
            ),
            row(
                ControlId::RecentApps,
                "Recent applications",
                count_popup(m.recent_apps),
                key("recent.apps"),
            ),
        ],
        None,
    )];
    let tray = tray_list(settings, live);
    if !tray.is_empty() {
        sections.push(section(
            Some("Menu Bar Icons"),
            tray.iter()
                .enumerate()
                .map(|(i, (id, name))| {
                    row(
                        ControlId::TrayItem(i),
                        name.clone(),
                        Control::Switch {
                            on: !m.hidden_tray.contains(id),
                        },
                        key("tray.hidden"),
                    )
                })
                .collect(),
            None,
        ));
    }
    sections
}

pub fn control_panel_sections(settings: &Settings) -> Vec<Section> {
    let s = &settings.menubar.shown;
    let key = |k| Backend::Key(File::MenuBar, k);
    let sw = |id, label: &str, on, k| row(id, label, Control::Switch { on }, key(k));
    vec![section(
        Some("Show in Menu Bar"),
        vec![
            sw(ControlId::ShowWifi, "Wi-Fi", s.wifi, "show.wifi"),
            sw(
                ControlId::ShowBluetooth,
                "Bluetooth",
                s.bluetooth,
                "show.bluetooth",
            ),
            sw(ControlId::ShowBattery, "Battery", s.battery, "show.battery"),
            sw(ControlId::ShowFocus, "Focus", s.focus, "show.focus"),
        ],
        None,
    )]
}
