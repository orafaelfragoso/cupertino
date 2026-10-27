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

use crate::item::{plain_text, ItemId, MenuItem, Shortcut, ShortcutModifier, Toggle};

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
}

/// What a menu row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraAction {
    SetWifiEnabled(bool),
    /// Join a network without a passphrase (open, or with saved credentials).
    JoinNetwork(String),
    SetBluetoothPowered(bool),
    ConnectDevice(String),
    DisconnectDevice(String),
    SetDoNotDisturb(bool),
    /// Do not disturb for the duration row with this identifier.
    DoNotDisturbFor(String),
    /// An entry of the system menu.
    Session(SessionAction),
}

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

/// Which entries of the system menu the session can carry out; a missing key reads as `true`.
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

    /// A row whose label is shown as given (network and device names keep their underscores).
    fn row(&mut self, label: &str, enabled: bool, toggle: Toggle, action: Option<ExtraAction>) {
        let id = ItemId(self.next_id());
        self.rows.push(MenuItem {
            label: label.to_owned(),
            enabled,
            toggle,
            ..MenuItem::new(id.0, "")
        });
        if let Some(action) = action {
            self.actions.push((id, action));
        }
    }

    /// A row that lists the keys that do the same (`Shortcut` text of a menu row).
    fn row_with_shortcut(
        &mut self,
        label: &str,
        enabled: bool,
        shortcut: Shortcut,
        action: ExtraAction,
    ) {
        self.row(label, enabled, Toggle::None, Some(action));
        if let Some(row) = self.rows.last_mut() {
            row.shortcut = Some(shortcut);
        }
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
                    wifi_menu(&mut menu, wifi);
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

    /// The system menu: Sleep, Restart…, Shut Down…, Lock Screen and Log Out <name>…, then the login window entry. The
    /// ellipsis entries open the agent's confirmation panel. Without a session agent (none runs, or the owner of its
    /// name is not the one the server started) only Lock Screen is left: the server locks without the agent.
    pub fn session_menu(&self) -> ExtraMenu {
        let mut menu = ExtraMenu::default();
        let lock = |caps: &SessionCaps, menu: &mut ExtraMenu| {
            menu.row_with_shortcut(
                "Lock Screen",
                caps.lock,
                Shortcut {
                    modifiers: vec![ShortcutModifier::Ctrl, ShortcutModifier::Primary],
                    key: "Q".to_owned(),
                },
                ExtraAction::Session(SessionAction::Lock),
            );
        };
        let Some(session) = self.session.as_ref() else {
            lock(&SessionCaps::default(), &mut menu);
            return menu;
        };
        let caps = session.caps;
        let action = |a| Some(ExtraAction::Session(a));
        menu.row(
            "Sleep",
            caps.sleep,
            Toggle::None,
            action(SessionAction::Sleep),
        );
        menu.row(
            "Restart\u{2026}",
            caps.restart,
            Toggle::None,
            action(SessionAction::Restart),
        );
        menu.row(
            "Shut Down\u{2026}",
            caps.shut_down,
            Toggle::None,
            action(SessionAction::ShutDown),
        );
        menu.separator();
        lock(&caps, &mut menu);
        let name = display_name(&session.user_name);
        let log_out = if name.is_empty() {
            "Log Out\u{2026}".to_owned()
        } else {
            format!("Log Out {name}\u{2026}")
        };
        menu.row_with_shortcut(
            &log_out,
            caps.log_out,
            Shortcut {
                modifiers: vec![ShortcutModifier::Shift, ShortcutModifier::Primary],
                key: "Q".to_owned(),
            },
            ExtraAction::Session(SessionAction::LogOut),
        );
        if caps.switch_user {
            menu.separator();
            menu.row(
                "Login Window\u{2026}",
                true,
                Toggle::None,
                action(SessionAction::SwitchUser),
            );
        }
        menu
    }

    /// The menu as the bar's status menu: a submenu titled with the item.
    pub fn status_menu(&self, kind: ExtraKind) -> MenuItem {
        MenuItem::submenu(0, kind.title(), self.menu(kind).rows)
    }
}

fn wifi_menu(menu: &mut ExtraMenu, wifi: &WifiState) {
    menu.row(
        "Wi-Fi",
        true,
        Toggle::Check(wifi.enabled),
        Some(ExtraAction::SetWifiEnabled(!wifi.enabled)),
    );
    if !wifi.enabled {
        return;
    }
    let sections: [(&str, &str, bool); 2] = [
        ("Known Network", "Known Networks", true),
        ("Other Network", "Other Networks", false),
    ];
    for (singular, plural, known) in sections {
        let mut networks: Vec<&WifiNetwork> = wifi
            .networks
            .iter()
            .filter(|n| n.known == known && !n.ssid.is_empty())
            .collect();
        if networks.is_empty() {
            continue;
        }
        // stable, ascending by name
        networks.sort_by(|a, b| a.ssid.cmp(&b.ssid));
        menu.separator();
        menu.header(if networks.len() == 1 {
            singular
        } else {
            plural
        });
        for network in networks {
            // without a passphrase prompt in the bar, only networks that need none can be joined from here
            let joinable = network.known || network.security == OPEN || network.security == OWE;
            let action = (joinable && !network.connected)
                .then(|| ExtraAction::JoinNetwork(network.ssid.clone()));
            menu.row(
                &network.ssid,
                joinable,
                Toggle::Check(network.connected),
                action,
            );
        }
    }
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
    use super::*;

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

    #[test]
    fn the_system_menu_lists_the_session_entries_with_the_name_the_chords_and_one_action_each() {
        let menu = session("Rafael", SessionCaps::default()).session_menu();
        assert_eq!(
            entries(&menu),
            [
                "Sleep",
                "Restart\u{2026}",
                "Shut Down\u{2026}",
                "Lock Screen",
                "Log Out Rafael\u{2026}",
                "Login Window\u{2026}"
            ]
        );
        let row = |label: &str| menu.rows.iter().find(|r| r.label == label).unwrap();
        assert_eq!(
            row("Lock Screen").shortcut.as_ref().map(Shortcut::display),
            Some("Ctrl+Super+Q".to_string())
        );
        assert_eq!(
            row("Log Out Rafael\u{2026}")
                .shortcut
                .as_ref()
                .map(Shortcut::display),
            Some("Shift+Super+Q".to_string())
        );
        for (label, action) in [
            ("Sleep", SessionAction::Sleep),
            ("Restart\u{2026}", SessionAction::Restart),
            ("Shut Down\u{2026}", SessionAction::ShutDown),
            ("Lock Screen", SessionAction::Lock),
            ("Log Out Rafael\u{2026}", SessionAction::LogOut),
            ("Login Window\u{2026}", SessionAction::SwitchUser),
        ] {
            assert_eq!(
                menu.action(row(label).id),
                Some(&ExtraAction::Session(action)),
                "{label}"
            );
        }
        assert!(menu
            .action(menu.rows.iter().find(|r| r.separator).unwrap().id)
            .is_none());
    }

    #[test]
    fn the_system_menu_greys_what_the_session_cannot_do_and_leaves_out_the_login_window_when_it_cannot_switch(
    ) {
        let caps = SessionCaps {
            sleep: false,
            ..SessionCaps::default()
        };
        let menu = session("", caps).session_menu();
        assert_eq!(
            entries(&menu),
            [
                "Sleep",
                "Restart\u{2026}",
                "Shut Down\u{2026}",
                "Lock Screen",
                "Log Out\u{2026}",
                "Login Window\u{2026}"
            ],
            "no name: plain Log Out"
        );
        assert!(!menu.rows[0].enabled, "Sleep is greyed");
        assert!(menu.rows[1].enabled);
        let alone = SessionCaps {
            switch_user: false,
            ..SessionCaps::default()
        };
        let menu = session("Rafael", alone).session_menu();
        assert!(!entries(&menu).iter().any(|l| l.contains("Login Window")));
    }

    #[test]
    fn without_a_session_agent_the_system_menu_still_locks() {
        let menu = Extras::default().session_menu();
        assert_eq!(entries(&menu), ["Lock Screen"]);
        assert!(menu.rows[0].enabled);
        assert_eq!(
            menu.action(menu.rows[0].id),
            Some(&ExtraAction::Session(SessionAction::Lock))
        );
    }

    #[test]
    fn the_name_in_log_out_is_plain_text_so_underscores_and_ampersands_are_not_mnemonics() {
        let menu = session("Ana_&Co", SessionCaps::default()).session_menu();
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
        let menu = session(&dirty, SessionCaps::default()).session_menu();
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
        let blank = session("\u{1b}\u{202e} ", SessionCaps::default()).session_menu();
        assert!(
            blank.rows.iter().any(|r| r.label == "Log Out\u{2026}"),
            "nothing left: plain Log Out"
        );
    }
}
