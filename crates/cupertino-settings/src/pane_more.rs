//! The sections of the panes that show the machine's networks, VPNs, battery history, sound inputs, display
//! arrangement, search options and accounts, and the changes their controls make (the rest of the panes are in
//! [`crate::panes`]).

use cupertino_defaults::notifications::AlertStyle;

use crate::controls::{Chart, Control, Value, MUTE_BELOW};
use crate::live::{
    BatteryExtras, ChargePoint, Interface, KnownNetwork, Live, Monitor, Reading, Vpn, Wifi,
};
use crate::panes::{row, Backend, Call, ControlId, Effect, Row, Section};
use crate::settings::{File, Settings};

/// A control of the panes of this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum More {
    /// A saved network, by index of `Wifi::known`: its buttons.
    Known(usize),
    /// A VPN connection, by index of `Live::vpns`: connected or not.
    Vpn(usize),
    LowPower,
    /// The span the battery charts show.
    ChartSpan,
    /// The page's tabs.
    Tab,
    InputDevice,
    InputVolume,
    InputMute,
    InputLevel,
    Balance,
    /// The refresh rate of a display, by index of `Live::displays`.
    Refresh(usize),
    /// Where a display lies against the first one, by index of `Live::displays`.
    Arrange(usize),
    KeyboardBacklight,
    SlowKeysDelay,
    MouseKeysDelay,
    /// A kind of result of the launcher: 0 applications, 1 folders, 2 files, 3 calculator, 4 actions, 5 top hit.
    SearchKind(usize),
    /// A folder the file search leaves out, by index.
    Exclude(usize),
    /// The button that adds a folder to those the file search leaves out.
    ExcludeAdd,
    /// An application of the notification list, by index: whether its notifications are accepted.
    AppAllow(usize),
    /// An application of the notification list, by index: how its alerts show.
    AppStyle(usize),
    /// Whether critical notifications come through Do Not Disturb.
    CriticalInFocus,
}

fn text(label: &str, value: impl Into<String>) -> Row {
    row(
        ControlId::Reading,
        label,
        Control::Text {
            value: value.into(),
        },
        Backend::Service("system"),
    )
}

fn section(header: Option<&str>, rows: Vec<Row>, footer: Option<&str>) -> Section {
    Section {
        header: header.map(str::to_owned),
        rows,
        footer: footer.map(str::to_owned),
    }
}

/// A row of tabs over the page; the pane's sections follow for the tab shown.
pub fn tabs(titles: &[&'static str], live: &Live) -> Section {
    section(
        None,
        vec![row(
            ControlId::More(More::Tab),
            "Page",
            Control::Segmented {
                segments: titles.to_vec(),
                selected: live.tab.min(titles.len().saturating_sub(1)),
            },
            Backend::Service("page"),
        )],
        None,
    )
}

// ---- Wi-Fi and Network --------------------------------------------------------------------------------------------

/// What a press of a saved network's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnownAction {
    Join,
    Disconnect,
    AutoJoin(bool),
    Forget,
}

fn known_actions(k: &KnownNetwork, can_forget: bool) -> Vec<KnownAction> {
    let mut out = Vec::new();
    if k.connected {
        out.push(KnownAction::Disconnect);
    } else if k.in_range {
        out.push(KnownAction::Join);
    }
    if let Some(on) = k.auto_join {
        out.push(KnownAction::AutoJoin(on));
    }
    if can_forget {
        out.push(KnownAction::Forget);
    }
    out
}

fn action_title(a: KnownAction) -> String {
    match a {
        KnownAction::Join => "Join".into(),
        KnownAction::Disconnect => "Disconnect".into(),
        KnownAction::AutoJoin(true) => "Auto-Join: On".into(),
        KnownAction::AutoJoin(false) => "Auto-Join: Off".into(),
        KnownAction::Forget => "Forget…".into(),
    }
}

/// The saved networks, in range or not, each with its buttons.
pub fn known_section(w: &Wifi) -> Section {
    let rows = w
        .known
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let actions = known_actions(k, w.can_forget);
            let mut r = row(
                ControlId::More(More::Known(i)),
                k.name.clone(),
                Control::Buttons {
                    titles: actions.into_iter().map(action_title).collect(),
                },
                Backend::Service("Known"),
            );
            r.detail = Some(
                if k.connected {
                    "Connected"
                } else if k.in_range {
                    "In range"
                } else {
                    "Not in range"
                }
                .to_string(),
            );
            if w.joining.as_deref() == Some(k.ssid.as_str()) {
                r.control = Control::Text {
                    value: "Connecting…".into(),
                };
            }
            r
        })
        .collect();
    section(Some("Known Networks"), rows, None)
}

fn security_text(security: &str) -> String {
    if security == "open" {
        "None".into()
    } else {
        security.to_uppercase()
    }
}

fn interface_rows(i: &Interface) -> Vec<Row> {
    let mut rows = vec![text("Status", i.state.clone())];
    if let Some(ip) = &i.ipv4 {
        rows.push(text("IP address", ip.clone()));
    }
    if let Some(gateway) = &i.gateway {
        rows.push(text("Router", gateway.clone()));
    }
    if !i.dns.is_empty() {
        rows.push(text("DNS", i.dns.join(", ")));
    }
    if let Some(mac) = &i.mac {
        rows.push(text("Hardware address", mac.clone()));
    }
    if let Some(speed) = i.speed {
        rows.push(text("Link speed", format!("{speed} Mbit/s")));
    }
    rows
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "ethernet" => "Ethernet",
        "wifi" => "Wi-Fi",
        _ => "Other",
    }
}

/// The details of the Wi-Fi connection that is up: the address, the router, the name servers and the security.
pub fn wifi_details(live: &Live) -> Vec<Section> {
    let Some(w) = live.wifi.ready() else {
        return Vec::new();
    };
    let Some(joined) = w.networks.iter().find(|n| n.connected) else {
        return Vec::new();
    };
    let mut rows = vec![text("Network", joined.name.clone())];
    if let Some(i) = live
        .interfaces
        .iter()
        .find(|i| i.kind == "wifi" && i.state == "up")
    {
        rows.extend(interface_rows(i).into_iter().skip(1));
    }
    rows.push(text("Security", security_text(&joined.security)));
    vec![section(Some("Network Details"), rows, None)]
}

/// One section for each interface the kernel lists.
pub fn interface_sections(live: &Live) -> Vec<Section> {
    live.interfaces
        .iter()
        .map(|i| {
            section(
                Some(&format!("{} ({})", kind_label(&i.kind), i.name)),
                interface_rows(i),
                None,
            )
        })
        .collect()
}

/// The VPN connections the network manager holds, each with a switch.
pub fn vpn_sections(live: &Live) -> Vec<Section> {
    match &live.vpns {
        Reading::Ready(list) if list.is_empty() => {
            vec![section(
                None,
                vec![text("VPN connections", "None")],
                Some("Add a VPN connection in the network manager; it shows here."),
            )]
        }
        Reading::Ready(list) => {
            let rows = list
                .iter()
                .enumerate()
                .map(|(i, v): (usize, &Vpn)| {
                    let mut r = row(
                        ControlId::More(More::Vpn(i)),
                        v.name.clone(),
                        Control::Switch { on: v.active },
                        Backend::Service("ActivateConnection"),
                    );
                    r.detail = Some(v.kind.clone());
                    if v.busy {
                        r.disabled = Some("Working…".into());
                    }
                    r
                })
                .collect();
            vec![section(Some("VPN Connections"), rows, None)]
        }
        other => vec![section(
            None,
            vec![Row {
                disabled: other.reason().map(str::to_string),
                ..text("VPN connections", "")
            }],
            None,
        )],
    }
}

// ---- Battery ------------------------------------------------------------------------------------------------------

/// The bars of the 24 hour chart: one for each half hour.
const DAY_BARS: usize = 48;
/// The bars of the ten day chart: one for each three hours.
const TEN_DAY_BARS: usize = 80;
const DAY_SECONDS: i64 = 86_400;

/// The time now in seconds since the epoch, and the local time's offset from UTC.
fn now_and_offset() -> (i64, i64) {
    // SAFETY: `time` accepts a null pointer; `localtime_r` fills the structure we pass it.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
        if libc::localtime_r(&now, tm.as_mut_ptr()).is_null() {
            return (now as i64, 0);
        }
        (now as i64, tm.assume_init().tm_gmtoff)
    }
}

fn hour_label(hour: i64) -> String {
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{h} {}", if hour < 12 { "AM" } else { "PM" })
}

fn weekday_label(days_since_epoch: i64) -> &'static str {
    // the epoch's day was a Thursday
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days_since_epoch.rem_euclid(7) as usize]
}

/// The chart of the recorded charge over the last day or the last ten days, ending at `now`.
pub fn charge_chart(history: &[ChargePoint], ten_days: bool, now: i64, offset: i64) -> Chart {
    let (bars, span) = if ten_days {
        (TEN_DAY_BARS, 10 * DAY_SECONDS)
    } else {
        (DAY_BARS, DAY_SECONDS)
    };
    let start = now - span;
    let step = span / bars as i64;
    let mut values: Vec<(f64, bool)> = vec![(0.0, false); bars];
    for p in history.iter().filter(|p| p.time >= start && p.time <= now) {
        let at = (((p.time - start) / step) as usize).min(bars - 1);
        values[at] = ((p.percent / 100.0).clamp(0.0, 1.0), p.charging);
    }
    let mut time_labels = Vec::new();
    if ten_days {
        // a label at each local midnight
        let mut midnight = (start + offset).div_euclid(DAY_SECONDS) * DAY_SECONDS - offset;
        while midnight <= now {
            if midnight > start {
                let day = (midnight + offset).div_euclid(DAY_SECONDS);
                time_labels.push((
                    (midnight - start) as f64 / span as f64,
                    weekday_label(day).to_string(),
                ));
            }
            midnight += DAY_SECONDS;
        }
    } else {
        // a label every six hours of the local clock
        let six = 6 * 3600;
        let mut tick = (start + offset).div_euclid(six) * six - offset;
        while tick <= now {
            if tick > start {
                let hour = (tick + offset).rem_euclid(DAY_SECONDS) / 3600;
                time_labels.push(((tick - start) as f64 / span as f64, hour_label(hour)));
            }
            tick += six;
        }
    }
    let last = history.last().map_or(String::new(), |p| {
        format!(" The latest reading is {:.0} per cent.", p.percent)
    });
    Chart {
        name: "Battery level".into(),
        summary: format!(
            "Battery level over the last {}.{last}",
            if ten_days { "ten days" } else { "24 hours" }
        ),
        bars: values,
        blue: false,
        value_labels: vec![
            (0.0, "0 %".into()),
            (0.5, "50 %".into()),
            (1.0, "100 %".into()),
        ],
        time_labels,
    }
}

/// Low Power Mode, the battery's health and the charge history, which the Battery pane shows among its sections.
pub fn battery_sections(
    extras: &BatteryExtras,
    ten_days: bool,
    with_battery: bool,
) -> Vec<Section> {
    let mut out = Vec::new();
    if let Some(on) = extras.low_power {
        out.push(section(
            None,
            vec![row(
                ControlId::More(More::LowPower),
                "Low power mode",
                Control::Switch { on },
                Backend::Service("ActiveProfile"),
            )],
            Some(
                "Lowers the speed of the processor and the screen to make the battery last longer.",
            ),
        ));
    }
    if with_battery {
        let mut rows = Vec::new();
        if let Some(health) = extras.health {
            rows.push(text("Maximum capacity", format!("{health:.0} %")));
        }
        if let Some(watts) = extras.rate_watts {
            rows.push(text("Power now", format!("{watts:.1} W")));
        }
        if !rows.is_empty() {
            out.push(section(Some("Battery Health"), rows, None));
        }
        let (now, offset) = now_and_offset();
        out.push(section(
            Some("Charge History"),
            vec![
                row(
                    ControlId::More(More::ChartSpan),
                    "Span",
                    Control::Segmented {
                        segments: vec!["Last 24 Hours", "Last 10 Days"],
                        selected: usize::from(ten_days),
                    },
                    Backend::Service("GetHistory"),
                ),
                row(
                    ControlId::Reading,
                    "Battery level",
                    Control::Chart(charge_chart(&extras.history, ten_days, now, offset)),
                    Backend::Service("GetHistory"),
                ),
            ],
            Some("The charge is recorded every five minutes; the darker bars were charging."),
        ));
    }
    out
}

// ---- Sound --------------------------------------------------------------------------------------------------------

/// The Sound pane: a tab for the outputs and one for the inputs.
pub fn sound_sections(live: &Live) -> Vec<Section> {
    let mut out = vec![tabs(&["Output", "Input"], live)];
    if live.tab == 0 {
        out.extend(sound_output(live));
    } else {
        out.extend(sound_input(live));
    }
    out
}

fn sound_output(live: &Live) -> Vec<Section> {
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
            ..row(ControlId::Mute, "Mute", Control::Switch { on: muted }, svc)
        },
    ]);
    if let Some(balance) = live.balance {
        rows.push(row(
            ControlId::More(More::Balance),
            "Balance",
            Control::Slider {
                value: (balance + 1.0) / 2.0,
                stops: None,
                marks: &[0.5],
                labels: ("Left", "Right"),
            },
            Backend::Service("balance"),
        ));
    }
    vec![section(Some("Output"), rows, None)]
}

fn sound_input(live: &Live) -> Vec<Section> {
    if live.inputs.is_empty() {
        return vec![section(
            Some("Input"),
            vec![text("Input device", "No sound input is available")],
            None,
        )];
    }
    let svc = Backend::Service("source");
    let (level, muted, reason) = match &live.input_volume {
        Reading::Ready(v) => (v.level, v.muted, None),
        other => (0.0, false, other.reason().map(str::to_string)),
    };
    let mut rows = Vec::new();
    if live.inputs.len() > 1 {
        rows.push(row(
            ControlId::More(More::InputDevice),
            "Input device",
            Control::Popup {
                choices: live.inputs.iter().map(|o| o.name.clone()).collect(),
                selected: live.inputs.iter().position(|o| o.default).unwrap_or(0),
            },
            svc,
        ));
    }
    rows.extend([
        Row {
            disabled: reason.clone(),
            ..row(
                ControlId::More(More::InputVolume),
                "Input volume",
                Control::Level { value: level },
                svc,
            )
        },
        Row {
            disabled: reason,
            ..row(
                ControlId::More(More::InputMute),
                "Mute",
                Control::Switch { on: muted },
                svc,
            )
        },
        row(
            ControlId::More(More::InputLevel),
            "Input level",
            Control::Meter {
                value: if muted { 0.0 } else { live.input_level },
            },
            svc,
        ),
    ]);
    vec![section(Some("Input"), rows, None)]
}

// ---- Displays -----------------------------------------------------------------------------------------------------

/// The refresh rates of a display that offers more than one, as a pop-up.
pub fn refresh_row(index: usize, m: &Monitor) -> Option<Row> {
    let current = m.refresh_mhz?;
    if m.refresh_rates.len() < 2 || m.connector.is_none() {
        return None;
    }
    let selected = m
        .refresh_rates
        .iter()
        .position(|r| r.abs_diff(current) < 500)?;
    Some(row(
        ControlId::More(More::Refresh(index)),
        "Refresh Rate",
        Control::Popup {
            choices: m
                .refresh_rates
                .iter()
                .map(|r| crate::about::refresh_text(*r))
                .collect(),
            selected,
        },
        Backend::Service("zwlr_output_manager_v1"),
    ))
}

const SIDES: [&str; 4] = ["Right of", "Left of", "Above", "Below"];

/// Where `m` would lie on side `side` of `base`.
fn placed(side: usize, base: &Monitor, m: &Monitor) -> Option<(i32, i32)> {
    let (bx, by) = base.position?;
    let (bw, bh) = base.logical?;
    let (mw, mh) = m.logical?;
    Some(match side {
        0 => (bx + bw as i32, by),
        1 => (bx - mw as i32, by),
        2 => (bx, by - mh as i32),
        _ => (bx, by + bh as i32),
    })
}

/// Where a display lies against the first one, as a pop-up; none for the first display itself.
pub fn arrange_row(index: usize, monitors: &[Monitor]) -> Option<Row> {
    let (base, m) = (monitors.first()?, monitors.get(index)?);
    if index == 0 || m.connector.is_none() {
        return None;
    }
    let here = m.position?;
    let mut choices: Vec<String> = SIDES
        .iter()
        .map(|s| format!("{s} {}", base.title))
        .collect();
    let selected = (0..SIDES.len())
        .find(|side| placed(*side, base, m) == Some(here))
        .unwrap_or_else(|| {
            choices.push(format!("At {}, {}", here.0, here.1));
            choices.len() - 1
        });
    Some(row(
        ControlId::More(More::Arrange(index)),
        "Position",
        Control::Popup { choices, selected },
        Backend::Service("zwlr_output_manager_v1"),
    ))
}

// ---- Keyboard and accessibility -----------------------------------------------------------------------------------

/// The delays of the slow keys filter, in milliseconds.
pub const SLOW_KEY_DELAYS: [u32; 5] = [100, 250, 500, 1000, 2000];
/// The delays before a held direction key of mouse keys repeats, in milliseconds.
pub const MOUSE_KEY_DELAYS: [u32; 4] = [500, 1000, 1500, 2000];
/// The delay slow keys use when the file sets none.
const SLOW_KEY_DEFAULT: u32 = 250;
/// The delay mouse keys use when the file sets none.
const MOUSE_KEY_DEFAULT: u32 = 1000;

fn delay_popup(list: &[u32], current: u32) -> Control {
    let mut choices: Vec<String> = list.iter().map(|ms| format!("{ms} ms")).collect();
    let selected = match list.iter().position(|ms| *ms == current) {
        Some(i) => i,
        None => {
            choices.push(format!("{current} ms"));
            choices.len() - 1
        }
    };
    Control::Popup { choices, selected }
}

/// The rows that set how long slow keys and mouse keys wait.
pub fn key_delay_rows(settings: &Settings) -> Vec<Row> {
    let k = &settings.keyboard;
    vec![
        row(
            ControlId::More(More::SlowKeysDelay),
            "Slow keys: accept a key after",
            delay_popup(
                &SLOW_KEY_DELAYS,
                k.slow_keys_delay_ms.unwrap_or(SLOW_KEY_DEFAULT),
            ),
            Backend::Key(File::Keyboard, "slow_keys_delay"),
        ),
        row(
            ControlId::More(More::MouseKeysDelay),
            "Mouse keys: start moving after",
            delay_popup(
                &MOUSE_KEY_DELAYS,
                k.mouse_keys_delay_ms.unwrap_or(MOUSE_KEY_DEFAULT),
            ),
            Backend::Key(File::Keyboard, "mouse_keys_delay"),
        ),
    ]
}

/// The keyboard's backlight, when it has one, and the input sources the keyboard file lists.
pub fn keyboard_sections(settings: &Settings, live: &Live) -> Vec<Section> {
    let mut out = Vec::new();
    if let Some(level) = live.keyboard_backlight {
        out.push(section(
            Some("Backlight"),
            vec![row(
                ControlId::More(More::KeyboardBacklight),
                "Keyboard brightness",
                Control::Level { value: level },
                Backend::Service("SetBrightness"),
            )],
            None,
        ));
    }
    if let Some(sources) = &settings.keyboard.input_sources {
        let rows: Vec<Row> = sources
            .iter()
            .map(|s| {
                text(
                    &s.id,
                    if s.decoded.is_some() {
                        "Decoded layout"
                    } else {
                        "System layout"
                    },
                )
            })
            .collect();
        if !rows.is_empty() {
            out.push(section(
                Some("Input Sources"),
                rows,
                Some("Edit input_sources in the keyboard file to change this list."),
            ));
        }
    }
    out
}

// ---- Search, users, notifications ---------------------------------------------------------------------------------

const SEARCH_KINDS: [&str; 6] = [
    "Applications",
    "Folders",
    "Files",
    "Calculator",
    "Actions",
    "Top hit",
];

/// The pane of the launcher's options.
pub fn search_sections(settings: &Settings) -> Vec<Section> {
    let l = &settings.launcher;
    let on = [
        l.applications,
        l.folders,
        l.files,
        l.calculator,
        l.actions,
        l.top_hit,
    ];
    let kinds = SEARCH_KINDS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            row(
                ControlId::More(More::SearchKind(i)),
                *label,
                Control::Switch { on: on[i] },
                Backend::Key(File::Launcher, "results"),
            )
        })
        .collect();
    let mut excluded: Vec<Row> = l
        .excluded
        .iter()
        .enumerate()
        .map(|(i, path)| {
            row(
                ControlId::More(More::Exclude(i)),
                crate::panes::home_relative(std::path::Path::new(path)),
                Control::Button {
                    title: "Remove".into(),
                },
                Backend::Key(File::Launcher, "files.exclude"),
            )
        })
        .collect();
    excluded.push(row(
        ControlId::More(More::ExcludeAdd),
        "Folders left out of file results",
        Control::Button {
            title: "Add Folder…".into(),
        },
        Backend::Key(File::Launcher, "files.exclude"),
    ));
    vec![
        section(
            Some("Results"),
            kinds,
            Some("Takes effect the next time the launcher opens."),
        ),
        section(Some("Files"), excluded, None),
    ]
}

/// The accounts of people on this machine, read only.
pub fn users_sections(live: &Live) -> Vec<Section> {
    let rows: Vec<Row> = live
        .users
        .iter()
        .map(|u| {
            let mut r = text(
                &u.name,
                format!(
                    "{}{}",
                    if u.administrator {
                        "Administrator"
                    } else {
                        "Standard"
                    },
                    if u.current { " · You" } else { "" }
                ),
            );
            r.detail = Some(u.username.clone());
            r
        })
        .collect();
    vec![section(
        Some("Users"),
        if rows.is_empty() {
            vec![text("Accounts", "None found")]
        } else {
            rows
        },
        Some("Accounts are managed with the system's own tools."),
    )]
}

/// The Allow and alert style rows of the application at `index`.
pub fn app_rows(index: usize, own: &cupertino_defaults::notifications::AppOverride) -> Vec<Row> {
    let key = |k| Backend::Key(File::Notifications, k);
    vec![
        row(
            ControlId::More(More::AppAllow(index)),
            "Allow notifications",
            Control::Switch {
                on: own.allow.unwrap_or(true),
            },
            key("app.<id>.allow"),
        ),
        row(
            ControlId::More(More::AppStyle(index)),
            "Alert style",
            Control::Popup {
                choices: vec!["Banners".into(), "None".into()],
                selected: match own.alert_style.unwrap_or(AlertStyle::Banners) {
                    AlertStyle::Banners => 0,
                    AlertStyle::None => 1,
                },
            },
            key("app.<id>.alert_style"),
        ),
    ]
}

/// The switch of the Focus pane that lets critical notifications through.
pub fn focus_critical_section(settings: &Settings) -> Section {
    section(
        Some("Exceptions"),
        vec![row(
            ControlId::More(More::CriticalInFocus),
            "Allow critical notifications",
            Control::Switch {
                on: settings.notifications.critical_during_dnd,
            },
            Backend::Key(File::Notifications, "critical_during_dnd"),
        )],
        None,
    )
}

// ---- Changes ------------------------------------------------------------------------------------------------------

/// The effects of a change of a control of this module.
pub fn change(more: More, value: Value, settings: &mut Settings, live: &Live) -> Vec<Effect> {
    let on = match value {
        Value::Bool(b) => b,
        _ => false,
    };
    let index = match value {
        Value::Index(i) => i,
        _ => 0,
    };
    let fraction = match value {
        Value::Fraction(f) => f,
        _ => 0.0,
    };
    let store = |f| vec![Effect::Store(f)];
    match more {
        More::Known(i) => {
            let Some(w) = live.wifi.ready() else {
                return Vec::new();
            };
            let Some(k) = w.known.get(i) else {
                return Vec::new();
            };
            match known_actions(k, w.can_forget).get(index) {
                Some(KnownAction::Join) => vec![Effect::Service(Call::Join {
                    ssid: k.ssid.clone(),
                    passphrase: Some(crate::sheet::Secret::new()),
                })],
                Some(KnownAction::Disconnect) => vec![Effect::Service(Call::Disconnect)],
                Some(KnownAction::AutoJoin(current)) => vec![Effect::Service(Call::SetAutoJoin {
                    ssid: k.ssid.clone(),
                    on: !current,
                })],
                Some(KnownAction::Forget) => {
                    vec![Effect::Service(Call::ForgetNetwork(k.ssid.clone()))]
                }
                None => Vec::new(),
            }
        }
        More::Vpn(i) => match live.vpns.ready().and_then(|v| v.get(i)) {
            Some(v) => vec![Effect::Service(Call::SetVpn {
                id: v.id.clone(),
                on,
            })],
            None => Vec::new(),
        },
        More::LowPower => vec![Effect::Service(Call::SetLowPower(on))],
        More::ChartSpan => vec![Effect::ChartSpan(index == 1)],
        More::Tab => vec![Effect::Tab(index)],
        More::InputDevice => match live.inputs.get(index) {
            Some(input) => vec![Effect::Service(Call::SetInput(input.id))],
            None => Vec::new(),
        },
        More::InputVolume => vec![Effect::Service(Call::SetInputVolume {
            level: fraction,
            muted: fraction < MUTE_BELOW,
        })],
        More::InputMute => {
            let level = live.input_volume.ready().map_or(0.0, |v| v.level);
            vec![Effect::Service(Call::SetInputVolume { level, muted: on })]
        }
        More::InputLevel => Vec::new(),
        More::Balance => vec![Effect::Service(Call::SetBalance(
            (fraction * 2.0 - 1.0).clamp(-1.0, 1.0),
        ))],
        More::Refresh(i) => {
            let Some(m) = live.displays.ready().and_then(|d| d.get(i)) else {
                return Vec::new();
            };
            match (&m.connector, m.refresh_rates.get(index)) {
                (Some(connector), Some(mhz)) => vec![Effect::Service(Call::SetDisplayRefresh {
                    connector: connector.clone(),
                    mhz: *mhz,
                })],
                _ => Vec::new(),
            }
        }
        More::Arrange(i) => {
            let Some(monitors) = live.displays.ready() else {
                return Vec::new();
            };
            let (Some(base), Some(m)) = (monitors.first(), monitors.get(i)) else {
                return Vec::new();
            };
            match (&m.connector, placed(index, base, m)) {
                (Some(connector), Some((x, y))) if index < SIDES.len() => {
                    vec![Effect::Service(Call::SetDisplayPosition {
                        connector: connector.clone(),
                        x,
                        y,
                    })]
                }
                _ => Vec::new(),
            }
        }
        More::KeyboardBacklight => vec![Effect::Service(Call::SetKeyboardBacklight(fraction))],
        More::SlowKeysDelay => match SLOW_KEY_DELAYS.get(index) {
            Some(ms) => {
                settings.keyboard.slow_keys_delay_ms = Some(*ms);
                store(File::Keyboard)
            }
            None => Vec::new(),
        },
        More::MouseKeysDelay => match MOUSE_KEY_DELAYS.get(index) {
            Some(ms) => {
                settings.keyboard.mouse_keys_delay_ms = Some(*ms);
                store(File::Keyboard)
            }
            None => Vec::new(),
        },
        More::SearchKind(i) => {
            let l = &mut settings.launcher;
            match i {
                0 => l.applications = on,
                1 => l.folders = on,
                2 => l.files = on,
                3 => l.calculator = on,
                4 => l.actions = on,
                5 => l.top_hit = on,
                _ => return Vec::new(),
            }
            store(File::Launcher)
        }
        More::Exclude(i) => {
            if i >= settings.launcher.excluded.len() {
                return Vec::new();
            }
            settings.launcher.excluded.remove(i);
            store(File::Launcher)
        }
        More::ExcludeAdd => vec![Effect::AskExclusion],
        More::AppAllow(i) | More::AppStyle(i) => {
            let Some((app, _)) = crate::panes::notification_apps(settings, live)
                .into_iter()
                .nth(i)
            else {
                return Vec::new();
            };
            let mut own = settings
                .notifications
                .apps
                .get(&app)
                .copied()
                .unwrap_or_default();
            if matches!(more, More::AppAllow(_)) {
                own.allow = (!on).then_some(false);
            } else {
                own.alert_style = match AlertStyle::ALL.get(index) {
                    Some(AlertStyle::Banners) => None,
                    Some(style) => Some(*style),
                    None => return Vec::new(),
                };
            }
            if !settings.notifications.set_app(&app, own) {
                return vec![Effect::Refuse(
                    "This app's settings can't be saved.".to_string(),
                )];
            }
            store(File::Notifications)
        }
        More::CriticalInFocus => {
            settings.notifications.critical_during_dnd = on;
            store(File::Notifications)
        }
    }
}
