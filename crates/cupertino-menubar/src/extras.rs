//! The bar's own status items ("extras"): Wi-Fi, battery, Bluetooth and Focus, drawn from the system services
//! (`cupertino-netd`, `cupertino-powerd`, `cupertino-btd`) and the notification process, with their menus.
//!
//! Pure: the process reads the services into an [`Extras`] (its `extras_bus` module) and this module turns it into
//! icons, menu rows and the action of each row.
//!
//! The
//!   personal hotspot, accessory and computer-to-computer sections have no source in the network service;
//! * the battery and Bluetooth row layouts are not mapped: their rows here are approximate, as are every
//!   icon name (freedesktop symbolic names) and the signal and charge bounds between icons.

use crate::fallback::Chords;
use crate::item::{plain_text, ItemId, MenuItem, PanelRow, Toggle};
use crate::layout::SlotKey;
use crate::recent::{RecentItems, RECENT_LIMIT};

/// Security name of a network that needs no passphrase in `org.cupertino.Network1.GetNetworks`.
const OPEN: &str = "open";
/// Enhanced open: encrypted, but joined without a passphrase.
const OWE: &str = "owe";

/// One of the bar's own status items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtraKind {
    Wifi,
    Battery,
    Bluetooth,
    Focus,
}

/// Every template symbol [`Extras::glyph`] can name, which the process requires of the asset directory at startup.
pub const SYMBOL_NAMES: [&str; 4] = [
    "symbol.wifi",
    "symbol.battery",
    "symbol.bluetooth",
    "symbol.moon",
];

impl ExtraKind {
    /// Nearest to the control panel first.
    pub const ORDER: [ExtraKind; 4] = [
        ExtraKind::Wifi,
        ExtraKind::Battery,
        ExtraKind::Bluetooth,
        ExtraKind::Focus,
    ];

    /// The status slot id; the prefix keeps it apart from bus names, which every other status item id starts with.
    pub fn slot_id(self) -> &'static str {
        match self {
            Self::Wifi => "extra:wifi",
            Self::Battery => "extra:battery",
            Self::Bluetooth => "extra:bluetooth",
            Self::Focus => "extra:focus",
        }
    }

    pub fn from_slot_id(id: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|kind| kind.slot_id() == id)
    }

    fn title(self) -> &'static str {
        match self {
            Self::Wifi => "Wi-Fi",
            Self::Battery => "Battery",
            Self::Bluetooth => "Bluetooth",
            Self::Focus => "Focus",
        }
    }
}

/// The display device of the power service (all batteries as one).
#[derive(Clone, Debug, PartialEq)]
pub struct BatteryState {
    /// 0 … 100.
    pub percent: u8,
    pub charging: bool,
    /// No line power is online (`OnBattery`).
    pub on_battery: bool,
}

/// A network of the last scan.
#[derive(Clone, Debug, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// dBm; `None` when the scan did not report it.
    pub signal_dbm: Option<i32>,
    /// The service's security name (`open`, `owe`, `wpa2`, …).
    pub security: String,
    /// Credentials are saved.
    pub known: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WifiState {
    pub enabled: bool,
    pub networks: Vec<WifiNetwork>,
}

/// A device of the Bluetooth service.
#[derive(Clone, Debug, PartialEq)]
pub struct BluetoothDevice {
    /// The id every method of the service takes.
    pub id: String,
    pub address: String,
    pub name: Option<String>,
    pub paired: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BluetoothState {
    pub powered: bool,
    pub devices: Vec<BluetoothDevice>,
}

/// A duration row of the notification process: its identifier and title.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusLifetime {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FocusState {
    pub active: bool,
    pub lifetimes: Vec<FocusLifetime>,
}

/// What the services report; `None` for one that is not running or has no device, whose item is not shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    pub wifi: Option<WifiState>,
    pub battery: Option<BatteryState>,
    pub bluetooth: Option<BluetoothState>,
    pub focus: Option<FocusState>,
    /// The session agent's menu entries; `None` without the agent (a greeter, another compositor): no system menu.
    pub session: Option<SessionState>,
    /// Whether Lock Screen works without the agent's entries: the server named a login process (its lock screen locks
    /// whoever owns the agent's bus name). It names none while no login process runs, and the lock then does nothing.
    pub lock_without_agent: bool,
    /// The panels of other processes that the bar's items open, while they are open.
    pub panels: OpenPanels,
    /// What the bar itself keeps of the Wi-Fi panel, not read from a service: whether the list of other networks is
    /// shown, and whether a scan the panel asked for is still under way.
    pub wifi_list_open: bool,
    pub wifi_scanning: bool,
}

/// The panels other processes show for the bar's items: the control panel and the notification centre's list. Each is
/// `Some` while it is open, with the connector name of the output it is on (`wl_output.name`), empty while the panel
/// does not know it yet. A panel whose process is not running is not open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenPanels {
    pub control_panel: Option<String>,
    pub centre: Option<String>,
}

impl OpenPanels {
    /// Whether the panel the item `slot` opens is open on the bar of the output named `output`, one of `bars` bars: on
    /// the bar of the panel's output, or, while the panel does not know its output yet, on the only bar there is.
    pub fn open_on(&self, slot: &SlotKey, output: Option<&str>, bars: usize) -> bool {
        let panel = match slot {
            SlotKey::ControlPanel => &self.control_panel,
            SlotKey::Clock => &self.centre,
            _ => return false,
        };
        match panel.as_deref() {
            None => false,
            Some("") => bars == 1,
            Some(on) => output == Some(on),
        }
    }
}

/// What a menu row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraAction {
    SetWifiEnabled(bool),
    /// Show or hide the list of other networks in the Wi-Fi panel; the bar carries it out itself.
    ToggleWifiList,
    /// Ask the network service to scan again.
    ScanWifi,
    /// Join a network without a passphrase (open, or with saved credentials).
    JoinNetwork(String),
    SetBluetoothPowered(bool),
    ConnectDevice(String),
    DisconnectDevice(String),
    SetDoNotDisturb(bool),
    /// Do not disturb for the duration row with this identifier.
    DoNotDisturbFor(String),
    /// An entry of the system menu the session agent carries out.
    Session(SessionAction),
    /// An entry of the system menu the bar carries out itself.
    System(SystemAction),
}

/// What an entry of the system menu that needs no session agent does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemAction {
    /// Open the Wi-Fi pane of the settings.
    WifiSettings,
    /// Open the About This Computer window of the control panel.
    About,
    /// Start the control panel.
    Settings,
    /// Show the server's Force Quit dialog.
    ForceQuit,
    /// Force quit the application with this id, the one the Alt alternate of Force Quit… names, without the dialog.
    ForceQuitApp(String),
    /// Start the application of Recent Items at this position.
    OpenRecentApp(usize),
    /// Open the document of Recent Items at this position with its default application.
    OpenRecentDocument(usize),
    /// Clear Menu of Recent Items.
    ClearRecentItems,
}

/// A row of the system menu. Each has a fixed id in -4000…-4099, so an open menu survives a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemRow {
    About = -4000,
    Settings = -4001,
    ForceQuit = -4002,
    Sleep = -4003,
    Restart = -4004,
    ShutDown = -4005,
    Lock = -4006,
    LogOut = -4007,
    LoginWindow = -4008,
    RecentItems = -4009,
    /// The Alt alternate of Force Quit…: Force Quit <app name>.
    ForceQuitApp = -4013,
}

impl SystemRow {
    pub fn id(self) -> ItemId {
        ItemId(self as i32)
    }
}

/// Separators of the system menu, each named by the row above it.
const SEPARATOR_AFTER_ABOUT: i32 = -4050;
const SEPARATOR_AFTER_SETTINGS: i32 = -4051;
const SEPARATOR_AFTER_FORCE_QUIT: i32 = -4052;
const SEPARATOR_AFTER_SHUT_DOWN: i32 = -4053;
const SEPARATOR_AFTER_LOG_OUT: i32 = -4054;
const SEPARATOR_AFTER_RECENT_ITEMS: i32 = -4055;

/// Rows of the Recent Items submenu. The application and document rows take the ids of their block by position
/// (-4060…-4069 and -4070…-4079); a click is resolved against the rows read when the menu opened.
pub const RECENT_APPS_HEADER: i32 = -4010;
pub const RECENT_DOCUMENTS_HEADER: i32 = -4011;
pub const CLEAR_RECENT_ITEMS: i32 = -4012;
const SEPARATOR_ABOVE_CLEAR_MENU: i32 = -4056;
pub const RECENT_APP_ROWS: i32 = -4060;
pub const RECENT_DOCUMENT_ROWS: i32 = -4070;

/// The most characters of an application or file name a Recent Items row shows.
const MAX_RECENT_LABEL_CHARS: usize = 64;

/// What an entry of the system menu asks of the session agent (`org.cupertino.Login1`, through the server).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    Sleep,
    /// The confirmation panel of a restart.
    Restart,
    ShutDown,
    Lock,
    LogOut,
    /// Lock the session and show the login window, where another person can log in.
    SwitchUser,
    /// The menu is about to open: ask the agent to read what the machine allows again; it answers by changing
    /// `Capabilities`. Not an entry of the menu.
    RefreshCapabilities,
}

/// Which entries of the system menu the session can carry out; a key the session agent does not report reads as `false`
/// (the default, all `true`, stands for an agent that reports every one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionCaps {
    pub sleep: bool,
    pub restart: bool,
    pub shut_down: bool,
    pub log_out: bool,
    pub switch_user: bool,
    pub lock: bool,
}

impl Default for SessionCaps {
    fn default() -> Self {
        SessionCaps {
            sleep: true,
            restart: true,
            shut_down: true,
            log_out: true,
            switch_user: true,
            lock: true,
        }
    }
}

/// The most characters of the application name the Force Quit alternate shows, the ellipsis included: the cap of the
/// person's name in Log Out <name>… and of the Recent Items names, so the row's width stays bounded.
const MAX_FORCE_QUIT_NAME_CHARS: usize = 64;

/// The active application as the Force Quit alternate names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrontApp<'a> {
    pub app_id: &'a str,
    /// The name the bold App menu title shows.
    pub name: &'a str,
    /// The server takes `force_quit_app` (`MenuBar::can_force_quit_app`).
    pub can_force_quit: bool,
}

/// Force Quit <name>, the name cut at its end with an ellipsis past [`MAX_FORCE_QUIT_NAME_CHARS`]; `None` for a name
/// with nothing to show.
fn force_quit_label(name: &str) -> Option<String> {
    let name = plain_text(name, usize::MAX);
    if name.is_empty() {
        return None;
    }
    if name.chars().count() <= MAX_FORCE_QUIT_NAME_CHARS {
        return Some(format!("Force Quit {name}"));
    }
    let head: String = name.chars().take(MAX_FORCE_QUIT_NAME_CHARS - 1).collect();
    Some(format!("Force Quit {}\u{2026}", head.trim_end()))
}

/// The most characters of the person's name the Log Out entry shows.
const MAX_NAME_CHARS: usize = 64;

/// A person's name as menu text (the name is the account's own, which its owner can set).
fn display_name(name: &str) -> String {
    plain_text(name, MAX_NAME_CHARS)
}

/// What the system menu is built from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionState {
    /// The signed-in person's name; the Log Out entry carries it.
    pub user_name: String,
    pub caps: SessionCaps,
}

/// A built menu: the rows and the action of each actionable row by its id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtraMenu {
    pub rows: Vec<MenuItem>,
    actions: Vec<(ItemId, ExtraAction)>,
}

impl ExtraMenu {
    pub fn action(&self, item: ItemId) -> Option<&ExtraAction> {
        self.actions
            .iter()
            .find(|(id, _)| *id == item)
            .map(|(_, action)| action)
    }

    fn next_id(&self) -> i32 {
        self.rows.len() as i32 + 1
    }

    /// A row with the id `id` whose label is shown as given (network, device and person names keep their underscores).
    fn push(
        &mut self,
        id: ItemId,
        label: &str,
        enabled: bool,
        toggle: Toggle,
        action: Option<ExtraAction>,
    ) -> &mut MenuItem {
        if let Some(action) = action {
            self.actions.push((id, action));
        }
        self.rows.push(MenuItem {
            label: label.to_owned(),
            enabled,
            toggle,
            ..MenuItem::new(id.0, "")
        });
        self.rows.last_mut().expect("a row was just pushed")
    }

    /// A row with the next positional id.
    fn row(&mut self, label: &str, enabled: bool, toggle: Toggle, action: Option<ExtraAction>) {
        self.push(ItemId(self.next_id()), label, enabled, toggle, action);
    }

    /// A row of the system menu at its fixed id, with the chord `chords` binds to it.
    fn system_row(
        &mut self,
        row: SystemRow,
        label: &str,
        enabled: bool,
        chords: &Chords,
        action: ExtraAction,
    ) {
        let item = self.push(row.id(), label, enabled, Toggle::None, Some(action));
        item.shortcut = chords.system.get(&row).cloned();
    }

    fn header(&mut self, label: &str) {
        self.row(label, false, Toggle::None, None);
    }

    fn separator(&mut self) {
        self.rows.push(MenuItem::separator(self.next_id()));
    }
}

/// What an item draws: a template symbol of the asset directory and whether it is dimmed (the radio is off or has no link).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtraGlyph {
    pub symbol: &'static str,
    pub dimmed: bool,
}

impl Extras {
    /// Whether going from these extras to `next` loses the session agent: an open system menu then closes, its session
    /// items doing nothing.
    pub fn loses_session(&self, next: &Extras) -> bool {
        self.session.is_some() && next.session.is_none()
    }

    /// The items to show, nearest to the control panel first.
    pub fn shown(&self) -> Vec<ExtraKind> {
        ExtraKind::ORDER
            .into_iter()
            .filter(|kind| match kind {
                ExtraKind::Wifi => self.wifi.is_some(),
                ExtraKind::Battery => self.battery.is_some(),
                ExtraKind::Bluetooth => self.bluetooth.is_some(),
                ExtraKind::Focus => self.focus.as_ref().is_some_and(|focus| focus.active),
            })
            .collect()
    }

    /// The template symbol of a shown item. One symbol per item: the asset directory has no per-level battery or
    /// signal-strength symbols, so the state shows only as `dimmed`.
    pub fn glyph(&self, kind: ExtraKind) -> Option<ExtraGlyph> {
        let (symbol, dimmed) = match kind {
            ExtraKind::Wifi => {
                let wifi = self.wifi.as_ref()?;
                (
                    "symbol.wifi",
                    !wifi.enabled || !wifi.networks.iter().any(|n| n.connected),
                )
            }
            ExtraKind::Battery => {
                self.battery.as_ref()?;
                ("symbol.battery", false)
            }
            ExtraKind::Bluetooth => ("symbol.bluetooth", !self.bluetooth.as_ref()?.powered),
            ExtraKind::Focus => {
                self.focus.as_ref()?;
                ("symbol.moon", false)
            }
        };
        Some(ExtraGlyph { symbol, dimmed })
    }

    /// The menu of an item; empty for one that is not shown.
    pub fn menu(&self, kind: ExtraKind) -> ExtraMenu {
        let mut menu = ExtraMenu::default();
        match kind {
            ExtraKind::Wifi => {
                if let Some(wifi) = &self.wifi {
                    wifi_menu(&mut menu, wifi, self.wifi_list_open, self.wifi_scanning);
                }
            }
            ExtraKind::Battery => {
                if let Some(battery) = &self.battery {
                    battery_menu(&mut menu, battery);
                }
            }
            ExtraKind::Bluetooth => {
                if let Some(bluetooth) = &self.bluetooth {
                    bluetooth_menu(&mut menu, bluetooth);
                }
            }
            ExtraKind::Focus => {
                if let Some(focus) = &self.focus {
                    focus_menu(&mut menu, focus);
                }
            }
        }
        menu
    }

    /// The system menu: About This Computer, Settings…, Recent Items (the submenu of `recent`, see [`recent_items`]),
    /// Force Quit…, then the session agent's Sleep, Restart…, Shut Down…, Lock Screen and Log Out <name>…, then the login
    /// window entry. The session's ellipsis entries open the agent's confirmation panel. Without a session agent (none
    /// runs, or the owner of its name is not the one the server started) the rows up to Force Quit… are followed only by
    /// Lock Screen, enabled while the server has named a login process: it locks without the agent. Force Quit…, Lock
    /// Screen and Log Out show the chord the server binds to them in `chords`. With an active application (`front`),
    /// Force Quit… has the Alt alternate Force Quit <name>, which force-quits it without the dialog: disabled when the
    /// server does not take the request, absent on the desktop. It shows no chord: none is bound.
    pub fn system_menu(
        &self,
        chords: &Chords,
        recent: &RecentItems,
        front: Option<FrontApp<'_>>,
    ) -> ExtraMenu {
        let mut menu = ExtraMenu::default();
        let session = |a| ExtraAction::Session(a);
        menu.system_row(
            SystemRow::About,
            "About This Computer",
            true,
            chords,
            ExtraAction::System(SystemAction::About),
        );
        menu.rows.push(MenuItem::separator(SEPARATOR_AFTER_ABOUT));
        menu.system_row(
            SystemRow::Settings,
            "Settings\u{2026}",
            true,
            chords,
            ExtraAction::System(SystemAction::Settings),
        );
        menu.rows
            .push(MenuItem::separator(SEPARATOR_AFTER_SETTINGS));
        let items = recent_items(recent);
        menu.actions.extend(items.actions);
        menu.rows.push(MenuItem::submenu(
            SystemRow::RecentItems.id().0,
            "Recent Items",
            items.rows,
        ));
        menu.rows
            .push(MenuItem::separator(SEPARATOR_AFTER_RECENT_ITEMS));
        menu.system_row(
            SystemRow::ForceQuit,
            "Force Quit\u{2026}",
            true,
            chords,
            ExtraAction::System(SystemAction::ForceQuit),
        );
        if let Some((front, label)) = front.and_then(|f| Some((f, force_quit_label(f.name)?))) {
            let id = SystemRow::ForceQuitApp.id();
            if front.can_force_quit {
                let action = SystemAction::ForceQuitApp(front.app_id.to_owned());
                menu.actions.push((id, ExtraAction::System(action)));
            }
            let alternate = MenuItem {
                label,
                enabled: front.can_force_quit,
                ..MenuItem::new(id.0, "")
            };
            if let Some(force_quit) = menu.rows.last_mut() {
                force_quit.alternate = Some(Box::new(alternate));
            }
        }
        menu.rows
            .push(MenuItem::separator(SEPARATOR_AFTER_FORCE_QUIT));
        let Some(state) = self.session.as_ref() else {
            menu.system_row(
                SystemRow::Lock,
                "Lock Screen",
                self.lock_without_agent,
                chords,
                session(SessionAction::Lock),
            );
            return menu;
        };
        let caps = state.caps;
        menu.system_row(
            SystemRow::Sleep,
            "Sleep",
            caps.sleep,
            chords,
            session(SessionAction::Sleep),
        );
        menu.system_row(
            SystemRow::Restart,
            "Restart\u{2026}",
            caps.restart,
            chords,
            session(SessionAction::Restart),
        );
        menu.system_row(
            SystemRow::ShutDown,
            "Shut Down\u{2026}",
            caps.shut_down,
            chords,
            session(SessionAction::ShutDown),
        );
        menu.rows
            .push(MenuItem::separator(SEPARATOR_AFTER_SHUT_DOWN));
        menu.system_row(
            SystemRow::Lock,
            "Lock Screen",
            caps.lock,
            chords,
            session(SessionAction::Lock),
        );
        let name = display_name(&state.user_name);
        let log_out = if name.is_empty() {
            "Log Out\u{2026}".to_owned()
        } else {
            format!("Log Out {name}\u{2026}")
        };
        menu.system_row(
            SystemRow::LogOut,
            &log_out,
            caps.log_out,
            chords,
            session(SessionAction::LogOut),
        );
        if caps.switch_user {
            menu.rows.push(MenuItem::separator(SEPARATOR_AFTER_LOG_OUT));
            menu.system_row(
                SystemRow::LoginWindow,
                "Login Window\u{2026}",
                true,
                chords,
                session(SessionAction::SwitchUser),
            );
        }
        menu
    }

    /// The menu as the bar's status menu: a submenu titled with the item.
    pub fn status_menu(&self, kind: ExtraKind) -> MenuItem {
        MenuItem::submenu(0, kind.title(), self.menu(kind).rows)
    }
}

/// The Recent Items submenu: a disabled Applications header and the applications, a disabled Documents header and the
/// documents, then Clear Menu. A section without rows keeps its header; with no rows at all the menu is a disabled
/// Clear Menu alone.
pub fn recent_items(recent: &RecentItems) -> ExtraMenu {
    let mut menu = ExtraMenu::default();
    let system = |a| Some(ExtraAction::System(a));
    if recent.is_empty() {
        menu.push(
            ItemId(CLEAR_RECENT_ITEMS),
            "Clear Menu",
            false,
            Toggle::None,
            None,
        );
        return menu;
    }
    menu.push(
        ItemId(RECENT_APPS_HEADER),
        "Applications",
        false,
        Toggle::None,
        None,
    );
    for (i, app) in recent.apps.iter().take(RECENT_LIMIT).enumerate() {
        menu.push(
            ItemId(RECENT_APP_ROWS - i as i32),
            &plain_text(&app.entry.name, MAX_RECENT_LABEL_CHARS),
            true,
            Toggle::None,
            system(SystemAction::OpenRecentApp(i)),
        );
    }
    menu.push(
        ItemId(RECENT_DOCUMENTS_HEADER),
        "Documents",
        false,
        Toggle::None,
        None,
    );
    for (i, document) in recent.documents.iter().take(RECENT_LIMIT).enumerate() {
        menu.push(
            ItemId(RECENT_DOCUMENT_ROWS - i as i32),
            &plain_text(&document.label(), MAX_RECENT_LABEL_CHARS),
            true,
            Toggle::None,
            system(SystemAction::OpenRecentDocument(i)),
        );
    }
    menu.rows
        .push(MenuItem::separator(SEPARATOR_ABOVE_CLEAR_MENU));
    menu.push(
        ItemId(CLEAR_RECENT_ITEMS),
        "Clear Menu",
        true,
        Toggle::None,
        system(SystemAction::ClearRecentItems),
    );
    menu
}

/// The most networks the panel lists.
const MAX_PANEL_NETWORKS: usize = 8;

/// Row ids of the Wi-Fi panel, fixed so the highlighted row stays the same when the rows around it change.
const WIFI_HEADER: i32 = 1;
const WIFI_RULE_AFTER_HEADER: i32 = 2;
const WIFI_SECTION: i32 = 3;
const WIFI_CURRENT: i32 = 4;
const WIFI_RULE_AFTER_CURRENT: i32 = 5;
const WIFI_DISCLOSURE: i32 = 6;
const WIFI_NOTE: i32 = 7;
const WIFI_RULE_BEFORE_SETTINGS: i32 = 8;
const WIFI_SETTINGS: i32 = 9;
/// The first id of the list's networks.
const WIFI_NETWORKS: i32 = 100;

/// The strength, 0 (not reported) to 3 bars, of a signal in dBm.
fn signal_bars(dbm: Option<i32>) -> u8 {
    match dbm {
        None => 0,
        Some(d) if d >= -55 => 3,
        Some(d) if d >= -67 => 2,
        Some(_) => 1,
    }
}

/// Whether joining the network asks for a passphrase or a certificate.
fn is_secured(security: &str) -> bool {
    security != OPEN && security != OWE
}

/// The Wi-Fi panel: the title with its switch, the network joined, the list of other networks (hidden until the row
/// above it is opened, and while Wi-Fi is off) and the row that opens the settings.
fn wifi_menu(menu: &mut ExtraMenu, wifi: &WifiState, list_open: bool, scanning: bool) {
    let panel_row = |menu: &mut ExtraMenu,
                     id: i32,
                     label: &str,
                     enabled: bool,
                     toggle: Toggle,
                     panel: PanelRow,
                     action: Option<ExtraAction>| {
        menu.push(ItemId(id), label, enabled, toggle, action).panel = Some(panel);
    };
    panel_row(
        menu,
        WIFI_HEADER,
        "Wi-Fi",
        true,
        Toggle::Check(wifi.enabled),
        PanelRow::Header {
            title: "Wi-Fi".to_owned(),
            on: wifi.enabled,
        },
        Some(ExtraAction::SetWifiEnabled(!wifi.enabled)),
    );
    menu.rows.push(MenuItem::separator(WIFI_RULE_AFTER_HEADER));
    if wifi.enabled {
        let named = |n: &&WifiNetwork| !n.ssid.is_empty();
        if let Some(current) = wifi.networks.iter().filter(named).find(|n| n.connected) {
            panel_row(
                menu,
                WIFI_SECTION,
                "Joined network",
                false,
                Toggle::None,
                PanelRow::Section {
                    title: "Joined network".to_owned(),
                },
                None,
            );
            panel_row(
                menu,
                WIFI_CURRENT,
                &current.ssid,
                false,
                Toggle::None,
                PanelRow::Current {
                    name: current.ssid.clone(),
                    secured: is_secured(&current.security),
                },
                None,
            );
            menu.rows.push(MenuItem::separator(WIFI_RULE_AFTER_CURRENT));
        }
        panel_row(
            menu,
            WIFI_DISCLOSURE,
            "Other networks",
            true,
            Toggle::None,
            PanelRow::Disclosure {
                title: "Other networks".to_owned(),
                expanded: list_open,
            },
            Some(ExtraAction::ToggleWifiList),
        );
        if list_open {
            let mut others: Vec<&WifiNetwork> = wifi
                .networks
                .iter()
                .filter(named)
                .filter(|n| !n.connected)
                .collect();
            // the strongest first, then by name
            others.sort_by(|a, b| {
                signal_bars(b.signal_dbm)
                    .cmp(&signal_bars(a.signal_dbm))
                    .then_with(|| a.ssid.cmp(&b.ssid))
            });
            others.dedup_by(|a, b| a.ssid == b.ssid);
            if others.is_empty() {
                let text = if scanning {
                    "Looking for networks\u{2026}"
                } else {
                    "No networks found"
                };
                panel_row(
                    menu,
                    WIFI_NOTE,
                    text,
                    false,
                    Toggle::None,
                    PanelRow::Note {
                        text: text.to_owned(),
                    },
                    None,
                );
            }
            for (i, network) in others.into_iter().take(MAX_PANEL_NETWORKS).enumerate() {
                let secured = is_secured(&network.security);
                // without a passphrase prompt in the bar, a network that needs one is finished in the settings
                let action = if network.known || !secured {
                    ExtraAction::JoinNetwork(network.ssid.clone())
                } else {
                    ExtraAction::System(SystemAction::WifiSettings)
                };
                panel_row(
                    menu,
                    WIFI_NETWORKS + i as i32,
                    &network.ssid,
                    true,
                    Toggle::None,
                    PanelRow::Network {
                        name: network.ssid.clone(),
                        secured,
                        strength: signal_bars(network.signal_dbm),
                    },
                    Some(action),
                );
            }
        }
    }
    menu.rows
        .push(MenuItem::separator(WIFI_RULE_BEFORE_SETTINGS));
    panel_row(
        menu,
        WIFI_SETTINGS,
        "Wi-Fi Settings\u{2026}",
        true,
        Toggle::None,
        PanelRow::Link {
            title: "Wi-Fi Settings\u{2026}".to_owned(),
        },
        Some(ExtraAction::System(SystemAction::WifiSettings)),
    );
}

fn battery_menu(menu: &mut ExtraMenu, battery: &BatteryState) {
    menu.header(&format!("Battery {}%", battery.percent));
    menu.header(if battery.on_battery {
        "Power Source: Battery"
    } else {
        "Power Source: Power Adapter"
    });
    if battery.charging {
        menu.header("Charging");
    }
}

fn bluetooth_menu(menu: &mut ExtraMenu, bluetooth: &BluetoothState) {
    menu.row(
        "Bluetooth",
        true,
        Toggle::Check(bluetooth.powered),
        Some(ExtraAction::SetBluetoothPowered(!bluetooth.powered)),
    );
    if !bluetooth.powered {
        return;
    }
    let mut devices: Vec<&BluetoothDevice> =
        bluetooth.devices.iter().filter(|d| d.paired).collect();
    if devices.is_empty() {
        return;
    }
    let name = |d: &BluetoothDevice| d.name.clone().unwrap_or_else(|| d.address.clone());
    devices.sort_by_key(|d| name(d));
    menu.separator();
    menu.header("Devices");
    for device in devices {
        let action = if device.connected {
            ExtraAction::DisconnectDevice(device.id.clone())
        } else {
            ExtraAction::ConnectDevice(device.id.clone())
        };
        menu.row(
            &name(device),
            true,
            Toggle::Check(device.connected),
            Some(action),
        );
    }
}

fn focus_menu(menu: &mut ExtraMenu, focus: &FocusState) {
    menu.row(
        "Do Not Disturb",
        true,
        Toggle::Check(focus.active),
        Some(ExtraAction::SetDoNotDisturb(!focus.active)),
    );
    if focus.lifetimes.is_empty() {
        return;
    }
    menu.separator();
    for lifetime in &focus.lifetimes {
        menu.row(
            &lifetime.title,
            true,
            Toggle::None,
            Some(ExtraAction::DoNotDisturbFor(lifetime.id.clone())),
        );
    }
}

#[cfg(test)]
mod tests {

    /// A panel's item is highlighted on the bar of the panel's output only; a panel that does not know its output yet
    /// highlights the item when there is one bar, and on no bar of several; a closed panel nowhere.
    #[test]
    fn an_open_panel_is_open_on_the_bar_of_its_output() {
        let panels = OpenPanels {
            control_panel: Some("DP-1".into()),
            centre: Some(String::new()),
        };
        assert!(panels.open_on(&SlotKey::ControlPanel, Some("DP-1"), 2));
        assert!(!panels.open_on(&SlotKey::ControlPanel, Some("HDMI-A-1"), 2));
        assert!(!panels.open_on(&SlotKey::ControlPanel, None, 1));
        assert!(panels.open_on(&SlotKey::Clock, Some("DP-1"), 1));
        assert!(panels.open_on(&SlotKey::Clock, None, 1));
        assert!(!panels.open_on(&SlotKey::Clock, Some("DP-1"), 2));
        assert!(!panels.open_on(&SlotKey::AppMenu, Some("DP-1"), 1));
        assert!(!panels.open_on(&SlotKey::Status("a".into()), Some("DP-1"), 1));
        let closed = OpenPanels::default();
        assert!(!closed.open_on(&SlotKey::ControlPanel, Some("DP-1"), 1));
        assert!(!closed.open_on(&SlotKey::Clock, None, 1));
    }

    #[test]
    fn only_an_agent_that_goes_away_closes_the_system_menu() {
        let with = Extras {
            session: Some(SessionState::default()),
            ..Extras::default()
        };
        let without = Extras::default();
        assert!(with.loses_session(&without));
        assert!(
            !without.loses_session(&with),
            "an agent that comes changes nothing open"
        );
        assert!(!with.loses_session(&with));
        assert!(!without.loses_session(&without));
    }

    use super::*;
    use crate::item::Shortcut;

    fn network(ssid: &str, security: &str, known: bool, connected: bool) -> WifiNetwork {
        WifiNetwork {
            ssid: ssid.into(),
            signal_dbm: Some(-60),
            security: security.into(),
            known,
            connected,
        }
    }

    fn labels(menu: &ExtraMenu) -> Vec<String> {
        menu.rows
            .iter()
            .map(|r| {
                if r.separator {
                    "---".to_owned()
                } else {
                    r.label.clone()
                }
            })
            .collect()
    }

    fn row<'a>(menu: &'a ExtraMenu, label: &str) -> &'a MenuItem {
        menu.rows
            .iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label:?}"))
    }

    fn wifi_fixture() -> Extras {
        let networks = vec![
            network("cafe_guest", "open", false, false),
            network("Home", "wpa2", true, true),
            network("Attic", "wpa2", false, false),
            network("Office", "wpa3", true, false),
            network("Lobby", "owe", false, false),
        ];
        Extras {
            wifi: Some(WifiState {
                enabled: true,
                networks,
            }),
            ..Extras::default()
        }
    }

    #[test]
    fn wifi_lists_known_then_other_networks_by_name() {
        let menu = wifi_fixture().menu(ExtraKind::Wifi);
        assert_eq!(
            labels(&menu),
            [
                "Wi-Fi",
                "---",
                "Known Networks",
                "Home",
                "Office",
                "---",
                "Other Networks",
                "Attic",
                "Lobby",
                "cafe_guest"
            ]
        );
        assert_eq!(row(&menu, "Home").toggle, Toggle::Check(true));
        assert!(
            !row(&menu, "Known Networks").enabled,
            "section titles are not rows to pick"
        );
    }

    #[test]
    fn wifi_joins_networks_that_need_no_passphrase() {
        let menu = wifi_fixture().menu(ExtraKind::Wifi);
        let action = |label| menu.action(row(&menu, label).id).cloned();
        assert_eq!(
            action("cafe_guest"),
            Some(ExtraAction::JoinNetwork("cafe_guest".into()))
        );
        assert_eq!(
            action("Lobby"),
            Some(ExtraAction::JoinNetwork("Lobby".into()))
        );
        assert_eq!(
            action("Office"),
            Some(ExtraAction::JoinNetwork("Office".into())),
            "saved credentials"
        );
        assert_eq!(action("Home"), None, "already joined");
        assert_eq!(action("Attic"), None);
        assert!(!row(&menu, "Attic").enabled);
        assert_eq!(action("Wi-Fi"), Some(ExtraAction::SetWifiEnabled(false)));
    }

    #[test]
    fn a_single_network_takes_the_singular_title_and_wifi_off_lists_none() {
        let mut extras = Extras {
            wifi: Some(WifiState {
                enabled: true,
                networks: vec![network("Home", "wpa2", true, false)],
            }),
            ..Extras::default()
        };
        assert_eq!(
            labels(&extras.menu(ExtraKind::Wifi)),
            ["Wi-Fi", "---", "Known Network", "Home"]
        );
        extras.wifi.as_mut().expect("set above").enabled = false;
        let menu = extras.menu(ExtraKind::Wifi);
        assert_eq!(labels(&menu), ["Wi-Fi"]);
        assert_eq!(
            menu.action(menu.rows[0].id),
            Some(&ExtraAction::SetWifiEnabled(true))
        );
        assert_eq!(
            extras.glyph(ExtraKind::Wifi),
            Some(ExtraGlyph {
                symbol: "symbol.wifi",
                dimmed: true
            })
        );
    }

    #[test]
    fn the_wifi_symbol_is_dimmed_without_a_joined_network() {
        let mut extras = wifi_fixture();
        assert_eq!(
            extras.glyph(ExtraKind::Wifi),
            Some(ExtraGlyph {
                symbol: "symbol.wifi",
                dimmed: false
            })
        );
        extras
            .wifi
            .as_mut()
            .expect("set above")
            .networks
            .iter_mut()
            .for_each(|n| n.connected = false);
        assert_eq!(
            extras.glyph(ExtraKind::Wifi),
            Some(ExtraGlyph {
                symbol: "symbol.wifi",
                dimmed: true
            })
        );
    }

    #[test]
    fn every_symbol_an_item_shows_is_in_the_required_list() {
        let extras = Extras {
            wifi: Some(WifiState {
                enabled: true,
                networks: Vec::new(),
            }),
            battery: Some(BatteryState {
                percent: 50,
                charging: false,
                on_battery: true,
            }),
            bluetooth: Some(BluetoothState {
                powered: true,
                devices: Vec::new(),
            }),
            focus: Some(FocusState {
                active: false,
                lifetimes: Vec::new(),
            }),
            session: None,
            lock_without_agent: false,
            panels: OpenPanels::default(),
        };
        for kind in ExtraKind::ORDER {
            let glyph = extras.glyph(kind).expect("every item is shown");
            assert!(SYMBOL_NAMES.contains(&glyph.symbol), "{}", glyph.symbol);
        }
    }

    #[test]
    fn battery_shows_the_percentage_and_the_power_source() {
        let mut extras = Extras {
            battery: Some(BatteryState {
                percent: 47,
                charging: false,
                on_battery: true,
            }),
            ..Extras::default()
        };
        let menu = extras.menu(ExtraKind::Battery);
        assert_eq!(labels(&menu), ["Battery 47%", "Power Source: Battery"]);
        assert!(menu.rows.iter().all(|r| !r.enabled));
        assert_eq!(
            extras.glyph(ExtraKind::Battery),
            Some(ExtraGlyph {
                symbol: "symbol.battery",
                dimmed: false
            })
        );
        extras.battery = Some(BatteryState {
            percent: 80,
            charging: true,
            on_battery: false,
        });
        assert_eq!(
            labels(&extras.menu(ExtraKind::Battery)),
            ["Battery 80%", "Power Source: Power Adapter", "Charging"]
        );
    }

    #[test]
    fn bluetooth_lists_paired_devices_with_connect_and_disconnect() {
        let device = |id: &str, name: Option<&str>, paired, connected| BluetoothDevice {
            id: id.into(),
            address: id.into(),
            name: name.map(Into::into),
            paired,
            connected,
        };
        let devices = vec![
            device("AA:01/bredr", Some("Mouse"), true, false),
            device("AA:02/bredr", Some("Headphones"), true, true),
            device("AA:03/le-random", None, false, false),
            device("AA:04/bredr", None, true, false),
        ];
        let extras = Extras {
            bluetooth: Some(BluetoothState {
                powered: true,
                devices,
            }),
            ..Extras::default()
        };
        let menu = extras.menu(ExtraKind::Bluetooth);
        assert_eq!(
            labels(&menu),
            [
                "Bluetooth",
                "---",
                "Devices",
                "AA:04/bredr",
                "Headphones",
                "Mouse"
            ]
        );
        assert_eq!(
            menu.action(row(&menu, "Headphones").id),
            Some(&ExtraAction::DisconnectDevice("AA:02/bredr".into()))
        );
        assert_eq!(
            menu.action(row(&menu, "Mouse").id),
            Some(&ExtraAction::ConnectDevice("AA:01/bredr".into()))
        );
        assert_eq!(
            menu.action(row(&menu, "Bluetooth").id),
            Some(&ExtraAction::SetBluetoothPowered(false))
        );
    }

    #[test]
    fn focus_shows_only_while_active_with_the_offered_durations() {
        let lifetimes = vec![
            FocusLifetime {
                id: "one-hour".into(),
                title: "For 1 hour".into(),
            },
            FocusLifetime {
                id: "evening".into(),
                title: "Until this evening".into(),
            },
        ];
        let mut extras = Extras {
            focus: Some(FocusState {
                active: false,
                lifetimes,
            }),
            ..Extras::default()
        };
        assert!(extras.shown().is_empty());
        extras.focus.as_mut().expect("set above").active = true;
        assert_eq!(extras.shown(), [ExtraKind::Focus]);
        let menu = extras.menu(ExtraKind::Focus);
        assert_eq!(
            labels(&menu),
            ["Do Not Disturb", "---", "For 1 hour", "Until this evening"]
        );
        assert_eq!(
            menu.action(row(&menu, "Do Not Disturb").id),
            Some(&ExtraAction::SetDoNotDisturb(false))
        );
        assert_eq!(
            menu.action(row(&menu, "Until this evening").id),
            Some(&ExtraAction::DoNotDisturbFor("evening".into()))
        );
    }

    #[test]
    fn items_show_in_order_and_only_with_their_service() {
        let mut extras = wifi_fixture();
        extras.bluetooth = Some(BluetoothState {
            powered: false,
            devices: Vec::new(),
        });
        assert_eq!(extras.shown(), [ExtraKind::Wifi, ExtraKind::Bluetooth]);
        assert_eq!(
            ExtraKind::from_slot_id(ExtraKind::Bluetooth.slot_id()),
            Some(ExtraKind::Bluetooth)
        );
        assert_eq!(ExtraKind::from_slot_id(":1.42"), None);
    }

    fn session(user_name: &str, caps: SessionCaps) -> Extras {
        Extras {
            session: Some(SessionState {
                user_name: user_name.to_string(),
                caps,
            }),
            ..Extras::default()
        }
    }

    fn entries(menu: &ExtraMenu) -> Vec<&str> {
        menu.rows
            .iter()
            .filter(|r| !r.separator)
            .map(|r| r.label.as_str())
            .collect()
    }

    /// Each row's label (`---` for a separator) and id.
    fn rows_and_ids(menu: &ExtraMenu) -> Vec<(String, i32)> {
        labels(menu)
            .into_iter()
            .zip(menu.rows.iter().map(|r| r.id.0))
            .collect()
    }

    fn system_menu(extras: &Extras) -> ExtraMenu {
        extras.system_menu(&Chords::default(), &RecentItems::default(), None)
    }

    #[test]
    fn the_system_menu_lists_its_rows_in_order_at_fixed_ids_with_one_action_each() {
        let menu = system_menu(&session("Rafael", SessionCaps::default()));
        let expected = [
            ("About This Computer", -4000),
            ("---", -4050),
            ("Settings\u{2026}", -4001),
            ("---", -4051),
            ("Recent Items", -4009),
            ("---", -4055),
            ("Force Quit\u{2026}", -4002),
            ("---", -4052),
            ("Sleep", -4003),
            ("Restart\u{2026}", -4004),
            ("Shut Down\u{2026}", -4005),
            ("---", -4053),
            ("Lock Screen", -4006),
            ("Log Out Rafael\u{2026}", -4007),
            ("---", -4054),
            ("Login Window\u{2026}", -4008),
        ]
        .map(|(label, id)| (label.to_owned(), id));
        assert_eq!(rows_and_ids(&menu), expected);
        let row = |label: &str| menu.rows.iter().find(|r| r.label == label).unwrap();
        for (label, action) in [
            (
                "About This Computer",
                ExtraAction::System(SystemAction::About),
            ),
            (
                "Settings\u{2026}",
                ExtraAction::System(SystemAction::Settings),
            ),
            (
                "Force Quit\u{2026}",
                ExtraAction::System(SystemAction::ForceQuit),
            ),
            ("Sleep", ExtraAction::Session(SessionAction::Sleep)),
            (
                "Restart\u{2026}",
                ExtraAction::Session(SessionAction::Restart),
            ),
            (
                "Shut Down\u{2026}",
                ExtraAction::Session(SessionAction::ShutDown),
            ),
            ("Lock Screen", ExtraAction::Session(SessionAction::Lock)),
            (
                "Log Out Rafael\u{2026}",
                ExtraAction::Session(SessionAction::LogOut),
            ),
            (
                "Login Window\u{2026}",
                ExtraAction::Session(SessionAction::SwitchUser),
            ),
        ] {
            assert!(row(label).enabled, "{label}");
            assert_eq!(menu.action(row(label).id), Some(&action), "{label}");
        }
        assert!(menu
            .rows
            .iter()
            .filter(|r| r.separator)
            .all(|r| menu.action(r.id).is_none()));

        let alone = SessionCaps {
            switch_user: false,
            ..SessionCaps::default()
        };
        let menu = system_menu(&session("Rafael", alone));
        assert_eq!(
            rows_and_ids(&menu),
            expected[..14],
            "no login window and no separator before it"
        );
    }

    #[test]
    fn the_system_menu_greys_what_the_session_cannot_do() {
        let caps = SessionCaps {
            sleep: false,
            lock: false,
            ..SessionCaps::default()
        };
        let menu = system_menu(&session("", caps));
        let enabled: Vec<(&str, bool)> = menu
            .rows
            .iter()
            .filter(|r| !r.separator)
            .map(|r| (r.label.as_str(), r.enabled))
            .collect();
        assert_eq!(
            enabled,
            [
                ("About This Computer", true),
                ("Settings\u{2026}", true),
                ("Recent Items", true),
                ("Force Quit\u{2026}", true),
                ("Sleep", false),
                ("Restart\u{2026}", true),
                ("Shut Down\u{2026}", true),
                ("Lock Screen", false),
                ("Log Out\u{2026}", true),
                ("Login Window\u{2026}", true),
            ],
            "no name: plain Log Out"
        );
    }

    #[test]
    fn without_a_session_agent_the_system_menu_offers_about_settings_force_quit_and_lock() {
        let named = Extras {
            lock_without_agent: true,
            ..Extras::default()
        };
        let menu = system_menu(&named);
        assert_eq!(
            rows_and_ids(&menu),
            [
                ("About This Computer", -4000),
                ("---", -4050),
                ("Settings\u{2026}", -4001),
                ("---", -4051),
                ("Recent Items", -4009),
                ("---", -4055),
                ("Force Quit\u{2026}", -4002),
                ("---", -4052),
                ("Lock Screen", -4006),
            ]
            .map(|(label, id)| (label.to_owned(), id))
        );
        assert!(menu.rows.iter().filter(|r| !r.separator).all(|r| r.enabled));
        assert_eq!(
            menu.action(SystemRow::Lock.id()),
            Some(&ExtraAction::Session(SessionAction::Lock))
        );
        assert_eq!(
            menu.action(SystemRow::ForceQuit.id()),
            Some(&ExtraAction::System(SystemAction::ForceQuit))
        );
        let nobody = system_menu(&Extras::default());
        assert_eq!(
            entries(&nobody),
            [
                "About This Computer",
                "Settings\u{2026}",
                "Recent Items",
                "Force Quit\u{2026}",
                "Lock Screen"
            ]
        );
        let lock = nobody.rows.last().unwrap();
        assert!(!lock.enabled, "no login process, so nothing locks");
        assert!(nobody.rows[0].enabled && nobody.rows[2].enabled && nobody.rows[4].enabled);
    }

    #[test]
    fn the_system_menu_shows_the_chords_the_server_binds_and_none_for_the_rest() {
        let extras = session("Rafael", SessionCaps::default());
        let shown = |chords: &Chords| -> Vec<(String, String)> {
            extras
                .system_menu(chords, &RecentItems::default(), None)
                .rows
                .iter()
                .filter_map(|r| Some((r.label.clone(), r.shortcut.as_ref()?.display())))
                .collect()
        };
        assert_eq!(shown(&Chords::default()), [], "nothing sent yet");
        let chords = Chords {
            system: [
                (SystemRow::ForceQuit, "Alt+Super+Escape"),
                (SystemRow::Lock, "Ctrl+Super+q"),
            ]
            .into_iter()
            .map(|(row, chord)| (row, Shortcut::from_chord(chord).unwrap()))
            .collect(),
            ..Chords::default()
        };
        assert_eq!(
            shown(&chords),
            [
                (
                    "Force Quit\u{2026}".to_owned(),
                    "Alt+Super+Escape".to_owned()
                ),
                ("Lock Screen".to_owned(), "Ctrl+Super+q".to_owned()),
            ],
            "Log Out is unbound"
        );
        let without_agent = Extras::default().system_menu(&chords, &RecentItems::default(), None);
        assert!(without_agent
            .rows
            .iter()
            .find(|r| r.id == SystemRow::Lock.id())
            .is_some_and(|r| r.shortcut.is_some()));
    }

    #[test]
    fn system_menu_ids_stay_when_the_name_or_the_capabilities_change() {
        let ids = |extras: &Extras| -> Vec<i32> {
            system_menu(extras).rows.iter().map(|r| r.id.0).collect()
        };
        let before = ids(&session("Rafael", SessionCaps::default()));
        let renamed = ids(&session("Ana", SessionCaps::default()));
        let greyed = ids(&session(
            "Rafael",
            SessionCaps {
                sleep: false,
                restart: false,
                log_out: false,
                ..SessionCaps::default()
            },
        ));
        assert_eq!(before, renamed);
        assert_eq!(before, greyed);
        let alone = ids(&session(
            "Rafael",
            SessionCaps {
                switch_user: false,
                ..SessionCaps::default()
            },
        ));
        assert_eq!(
            alone,
            before[..alone.len()],
            "rows that stay keep their ids"
        );
        assert!(before.iter().all(|id| (-4099..=-4000).contains(id)));
    }

    #[test]
    fn the_name_in_log_out_is_plain_text_so_underscores_and_ampersands_are_not_mnemonics() {
        let menu = system_menu(&session("Ana_&Co", SessionCaps::default()));
        let log_out = menu
            .rows
            .iter()
            .find(|r| r.label.starts_with("Log Out"))
            .unwrap();
        assert_eq!(log_out.label, "Log Out Ana_&Co\u{2026}");
        assert_eq!(log_out.mnemonic, None);
    }

    #[test]
    fn the_name_in_log_out_loses_control_and_direction_characters_and_is_cut_to_64_characters() {
        let dirty = format!("Ana\u{1b}[31m\n\u{202e}Bob{}", "x".repeat(100));
        let menu = system_menu(&session(&dirty, SessionCaps::default()));
        let log_out = menu
            .rows
            .iter()
            .find(|r| r.label.starts_with("Log Out"))
            .unwrap();
        let name = log_out
            .label
            .strip_prefix("Log Out ")
            .and_then(|l| l.strip_suffix('\u{2026}'))
            .unwrap();
        assert!(name.starts_with("Ana[31mBobxxx"), "{name:?}");
        assert_eq!(name.chars().count(), 64);
        assert!(!name.chars().any(|c| c.is_control() || c == '\u{202e}'));
        let blank = system_menu(&session("\u{1b}\u{202e} ", SessionCaps::default()));
        assert!(
            blank.rows.iter().any(|r| r.label == "Log Out\u{2026}"),
            "nothing left: plain Log Out"
        );
    }

    fn force_quit_row(front: Option<FrontApp<'_>>) -> (ExtraMenu, MenuItem) {
        let menu =
            Extras::default().system_menu(&Chords::default(), &RecentItems::default(), front);
        let row = menu
            .rows
            .iter()
            .find(|r| r.id == SystemRow::ForceQuit.id())
            .cloned()
            .unwrap();
        (menu, row)
    }

    const EDITOR: FrontApp<'static> = FrontApp {
        app_id: "org.example.editor",
        name: "Editor",
        can_force_quit: true,
    };

    #[test]
    fn force_quit_has_the_alt_alternate_force_quit_app_that_names_the_active_app() {
        let (menu, row) = force_quit_row(Some(EDITOR));
        let alternate = row.alternate.as_deref().unwrap();
        assert_eq!(
            (alternate.id, alternate.label.as_str()),
            (SystemRow::ForceQuitApp.id(), "Force Quit Editor")
        );
        assert!(alternate.enabled && alternate.shortcut.is_none());
        assert_eq!(
            menu.action(alternate.id),
            Some(&ExtraAction::System(SystemAction::ForceQuitApp(
                "org.example.editor".into()
            )))
        );
        assert_eq!(
            menu.action(row.id),
            Some(&ExtraAction::System(SystemAction::ForceQuit)),
            "Force Quit… still opens the dialog"
        );
        let shown = MenuItem::submenu(0, "", menu.rows.clone()).alternates_shown();
        assert!(shown
            .children
            .iter()
            .any(|r| r.label == "Force Quit Editor"));
    }

    #[test]
    fn the_force_quit_alternate_is_absent_on_the_desktop_and_disabled_below_version_13() {
        let (menu, row) = force_quit_row(None);
        assert!(row.alternate.is_none());
        assert_eq!(menu.action(SystemRow::ForceQuitApp.id()), None);
        let old_server = FrontApp {
            can_force_quit: false,
            ..EDITOR
        };
        let (menu, row) = force_quit_row(Some(old_server));
        assert!(!row.alternate.as_deref().unwrap().enabled);
        assert_eq!(menu.action(SystemRow::ForceQuitApp.id()), None);
    }

    #[test]
    fn a_long_app_name_is_cut_with_an_ellipsis_inside_the_force_quit_label() {
        let long = format!("Very{}", "Long".repeat(40));
        let (_, row) = force_quit_row(Some(FrontApp {
            name: &long,
            ..EDITOR
        }));
        let label = row.alternate.unwrap().label;
        let name = label.strip_prefix("Force Quit ").unwrap();
        assert_eq!(name.chars().count(), MAX_FORCE_QUIT_NAME_CHARS);
        assert!(name.ends_with('\u{2026}') && long.starts_with(name.trim_end_matches('\u{2026}')));
        let exact = "a".repeat(MAX_FORCE_QUIT_NAME_CHARS);
        let (_, row) = force_quit_row(Some(FrontApp {
            name: &exact,
            ..EDITOR
        }));
        assert_eq!(row.alternate.unwrap().label, format!("Force Quit {exact}"));
    }

    fn recent(apps: &[&str], documents: &[&str]) -> RecentItems {
        use crate::recent::{RecentApp, RecentDocument};
        let app = |name: &str| RecentApp {
            entry: cupertino_apps::DesktopEntry {
                id: format!("{name}.desktop"),
                name: name.into(),
                generic_name: String::new(),
                comment: String::new(),
                keywords: vec![],
                categories: vec![],
                icon: None,
                exec: name.into(),
                try_exec: None,
                terminal: false,
                path: None,
                startup_wm_class: None,
                actions: vec![],
                source: None,
            },
            last_used: 0.0,
        };
        RecentItems {
            apps: apps.iter().map(|name| app(name)).collect(),
            documents: documents
                .iter()
                .map(|name| RecentDocument {
                    path: std::path::Path::new("/home/ada").join(name),
                    used: None,
                })
                .collect(),
        }
    }

    fn recent_submenu(menu: &ExtraMenu) -> &MenuItem {
        menu.rows
            .iter()
            .find(|r| r.id == SystemRow::RecentItems.id())
            .expect("Recent Items is in the system menu")
    }

    #[test]
    fn recent_items_lists_applications_then_documents_at_their_ids_then_clear_menu() {
        let items = recent(
            &["Editor", "Web_Browser"],
            &["notes.md", "plan\u{202e}.txt"],
        );
        let extras = session("Rafael", SessionCaps::default());
        let menu = extras.system_menu(&Chords::default(), &items, None);
        let submenu = recent_submenu(&menu);
        assert!(submenu.has_submenu && submenu.enabled);
        let rows: Vec<(String, i32, bool)> = submenu
            .children
            .iter()
            .map(|r| {
                let label = if r.separator {
                    "---".into()
                } else {
                    r.label.clone()
                };
                (label, r.id.0, r.enabled)
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("Applications", -4010, false),
                ("Editor", -4060, true),
                ("Web_Browser", -4061, true),
                ("Documents", -4011, false),
                ("notes.md", -4070, true),
                ("plan.txt", -4071, true),
                ("---", -4056, false),
                ("Clear Menu", -4012, true),
            ]
            .map(|(label, id, enabled)| (label.to_owned(), id, enabled))
        );
        let action = |id: i32| menu.action(ItemId(id)).cloned();
        let system = |a| Some(ExtraAction::System(a));
        assert_eq!(action(-4061), system(SystemAction::OpenRecentApp(1)));
        assert_eq!(action(-4070), system(SystemAction::OpenRecentDocument(0)));
        assert_eq!(action(-4012), system(SystemAction::ClearRecentItems));
        assert_eq!(action(-4010), None, "headers do nothing");
    }

    #[test]
    fn an_empty_section_keeps_its_header_and_nothing_at_all_is_a_disabled_clear_menu() {
        let labels = |items: &RecentItems| -> Vec<(String, bool)> {
            recent_items(items)
                .rows
                .iter()
                .filter(|r| !r.separator)
                .map(|r| (r.label.clone(), r.enabled))
                .collect()
        };
        let owned = |rows: &[(&str, bool)]| -> Vec<(String, bool)> {
            rows.iter().map(|(l, e)| (l.to_string(), *e)).collect()
        };
        assert_eq!(
            labels(&recent(&[], &["a.txt"])),
            owned(&[
                ("Applications", false),
                ("Documents", false),
                ("a.txt", true),
                ("Clear Menu", true)
            ])
        );
        assert_eq!(
            labels(&recent(&["Editor"], &[])),
            owned(&[
                ("Applications", false),
                ("Editor", true),
                ("Documents", false),
                ("Clear Menu", true)
            ])
        );
        let empty = recent_items(&RecentItems::default());
        assert_eq!(
            labels(&RecentItems::default()),
            owned(&[("Clear Menu", false)])
        );
        assert_eq!(empty.rows[0].id.0, CLEAR_RECENT_ITEMS);
        assert_eq!(empty.action(ItemId(CLEAR_RECENT_ITEMS)), None);
    }

    #[test]
    fn recent_items_shows_at_most_ten_of_each_inside_the_system_menu_range() {
        let names: Vec<String> = (0..12).map(|i| format!("n{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let menu = recent_items(&recent(&names, &names));
        let ids: Vec<i32> = menu.rows.iter().map(|r| r.id.0).collect();
        assert_eq!(menu.rows.len(), 2 + 10 + 10 + 2);
        assert!(ids.contains(&-4069) && ids.contains(&-4079));
        assert!(ids.iter().all(|id| (-4099..=-4000).contains(id)));
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "every id once");
    }
}
