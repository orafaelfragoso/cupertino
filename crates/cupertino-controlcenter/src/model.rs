//! What the panel shows: the modules in reading order, the reading behind each tile, what is playing and the lists of
//! the detail views.
//!
//! Pure logic; nothing here talks to a backend. A reading that could not be taken is `Unavailable` with the reason, so a
//! failing backend disables its tile instead of showing a default.

/// Below this level a volume counts as muted ("mute when `v < 0.001`").
pub const MUTE_BELOW: f64 = 0.001;

/// The speaker glyph changes at a third and at two thirds.
const LOW_BELOW: f64 = 1.0 / 3.0;
const MEDIUM_BELOW: f64 = 2.0 / 3.0;

/// A module of the panel, in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    WiFi,
    NowPlaying,
    Bluetooth,
    Microphone,
    DockHiding,
    LockScreen,
    Mute,
    KeyboardBrightness,
    FocusMode,
    Display,
    Sound,
}

impl Module {
    /// Every module in reading order: the connectivity capsules beside the media tile, the round buttons, the focus
    /// capsule, then the display and sound modules.
    pub const ALL: [Module; 11] = [
        Module::WiFi,
        Module::NowPlaying,
        Module::Bluetooth,
        Module::Microphone,
        Module::DockHiding,
        Module::LockScreen,
        Module::Mute,
        Module::KeyboardBrightness,
        Module::FocusMode,
        Module::Display,
        Module::Sound,
    ];

    /// The tile's title.
    pub fn title(self) -> &'static str {
        match self {
            Module::WiFi => "Wi-Fi",
            Module::NowPlaying => "Now Playing",
            Module::Bluetooth => "Bluetooth",
            Module::Microphone => "Microphone",
            Module::DockHiding => "Dock Hiding",
            Module::LockScreen => "Lock Screen",
            Module::Mute => "Mute",
            Module::KeyboardBrightness => "Keyboard Brightness",
            Module::FocusMode => "Focus",
            Module::Display => "Display",
            Module::Sound => "Sound",
        }
    }

    /// The detail view the module opens, for the modules that have one.
    pub fn detail(self) -> Option<Detail> {
        match self {
            Module::WiFi => Some(Detail::WiFi),
            Module::Bluetooth => Some(Detail::Bluetooth),
            Module::Display => Some(Detail::Display),
            Module::Sound => Some(Detail::Sound),
            _ => None,
        }
    }
}

/// A module's detail view: the module grown over the panel with its list or controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Detail {
    WiFi,
    Bluetooth,
    Display,
    Sound,
}

impl Detail {
    pub const ALL: [Detail; 4] = [
        Detail::WiFi,
        Detail::Bluetooth,
        Detail::Display,
        Detail::Sound,
    ];

    /// The module the detail belongs to.
    pub fn module(self) -> Module {
        match self {
            Detail::WiFi => Module::WiFi,
            Detail::Bluetooth => Module::Bluetooth,
            Detail::Display => Module::Display,
            Detail::Sound => Module::Sound,
        }
    }

    /// The heading of the detail's list.
    pub fn heading(self) -> &'static str {
        match self {
            Detail::WiFi => "Networks",
            Detail::Bluetooth => "Devices",
            Detail::Display => "Keyboard",
            Detail::Sound => "Output",
        }
    }

    /// The settings page of the module, which the view's last row opens.
    pub fn settings_pane(self) -> &'static str {
        match self {
            Detail::WiFi => "wifi",
            Detail::Bluetooth => "bluetooth",
            Detail::Display => "displays",
            Detail::Sound => "sound",
        }
    }

    /// The label of that last row.
    pub fn settings_label(self) -> &'static str {
        match self {
            Detail::WiFi => "Wi-Fi Settings…",
            Detail::Bluetooth => "Bluetooth Settings…",
            Detail::Display => "Displays Settings…",
            Detail::Sound => "Sound Settings…",
        }
    }
}

/// The two sliders of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slider {
    Display,
    Sound,
}

impl Slider {
    /// The module the slider belongs to.
    pub fn module(self) -> Module {
        match self {
            Slider::Display => Module::Display,
            Slider::Sound => Module::Sound,
        }
    }
}

/// A reading of a backend, or why there is none.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<T> {
    Ready(T),
    /// The backend failed or does not exist; the text is shown to the user and logged.
    Unavailable(String),
}

impl<T> Reading<T> {
    /// The value, when there is one.
    pub fn ready(&self) -> Option<&T> {
        match self {
            Reading::Ready(value) => Some(value),
            Reading::Unavailable(_) => None,
        }
    }
}

/// The Wi-Fi radio.
#[derive(Clone, Debug, PartialEq)]
pub struct WifiStatus {
    pub enabled: bool,
    /// The network the radio is connected to.
    pub network: Option<String>,
}

/// The output volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    /// Level, 0 to 1.
    pub level: f64,
    /// Whether the output is muted. False on a card without a mute switch, where level 0 is the only way to silence it.
    pub muted: bool,
}

/// What a media player is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub title: String,
    pub artist: Option<String>,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_play_pause: bool,
}

/// A button of the media tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaCommand {
    Previous,
    PlayPause,
    Next,
}

/// A network of the last scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub ssid: String,
    /// Signal bars, 1 to 3; `None` when the driver gave no strength.
    pub bars: Option<u8>,
    pub secured: bool,
    /// The service holds its credentials.
    pub known: bool,
    pub connected: bool,
}

impl Network {
    /// Whether a press can join it: it is open, or the service knows its passphrase. Joining a new secured network
    /// needs the passphrase, which the network settings ask for.
    pub fn joinable(&self) -> bool {
        !self.connected && (!self.secured || self.known)
    }

    /// Signal bars of a strength in dBm: 3 from -60, 2 from -70, else 1.
    pub fn bars_of(dbm: i32) -> u8 {
        if dbm >= -60 {
            3
        } else if dbm >= -70 {
            2
        } else {
            1
        }
    }
}

/// A Bluetooth device the adapter knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// The identifier the service takes.
    pub id: String,
    pub name: String,
    pub connected: bool,
}

/// A sound output.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub id: u32,
    pub name: String,
    /// Applications play to it now.
    pub current: bool,
}

/// What a tile shows. For the display and sound modules, `On` means the slider works.
#[derive(Clone, Debug, PartialEq)]
pub enum TileState {
    On,
    Off,
    Disabled(String),
}

/// The asset-directory symbol for a speaker at `volume`.
pub fn speaker_icon(volume: &VolumeState) -> &'static str {
    if volume.muted || volume.level < MUTE_BELOW {
        "symbol.speaker.slash"
    } else if volume.level < LOW_BELOW {
        "symbol.speaker.low"
    } else if volume.level < MEDIUM_BELOW {
        "symbol.speaker.medium"
    } else {
        "symbol.speaker.high"
    }
}

/// The readings behind the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    pub wifi: Reading<WifiStatus>,
    pub bluetooth: Reading<bool>,
    /// Whether the default input records (it is not muted).
    pub microphone: Reading<bool>,
    /// Whether the dock hides until the pointer reaches its edge.
    pub dock_hiding: Reading<bool>,
    /// Whether the keyboard backlight is on.
    pub keyboard: Reading<bool>,
    pub focus_mode: Reading<bool>,
    /// Display brightness, 0 to 1.
    pub brightness: Reading<f64>,
    pub volume: Reading<VolumeState>,
    /// What the media player plays; `None` when no player plays anything.
    pub media: Reading<Option<Media>>,
    /// The lists of the detail views; read while their detail is open.
    pub networks: Reading<Vec<Network>>,
    pub devices: Reading<Vec<Device>>,
    pub outputs: Reading<Vec<Output>>,
}

const NOT_READ: &str = "not read yet";

impl Controls {
    /// Nothing read yet: every backed module is disabled until the first reading.
    pub fn unread() -> Self {
        let unread = || NOT_READ.to_string();
        Self {
            wifi: Reading::Unavailable(unread()),
            bluetooth: Reading::Unavailable(unread()),
            microphone: Reading::Unavailable(unread()),
            dock_hiding: Reading::Unavailable(unread()),
            keyboard: Reading::Unavailable(unread()),
            focus_mode: Reading::Unavailable(unread()),
            brightness: Reading::Unavailable(unread()),
            volume: Reading::Unavailable(unread()),
            media: Reading::Unavailable(unread()),
            networks: Reading::Unavailable(unread()),
            devices: Reading::Unavailable(unread()),
            outputs: Reading::Unavailable(unread()),
        }
    }

    /// The state of `module`'s tile.
    pub fn tile(&self, module: Module) -> TileState {
        let switch = |reading: &Reading<bool>| match reading {
            Reading::Ready(true) => TileState::On,
            Reading::Ready(false) => TileState::Off,
            Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
        };
        let usable = |ready: bool, reason: Option<&String>| match reason {
            Some(reason) if !ready => TileState::Disabled(reason.clone()),
            _ => TileState::On,
        };
        match module {
            Module::WiFi => match &self.wifi {
                Reading::Ready(status) => switch(&Reading::Ready(status.enabled)),
                Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
            },
            Module::Bluetooth => switch(&self.bluetooth),
            Module::Microphone => switch(&self.microphone),
            Module::DockHiding => switch(&self.dock_hiding),
            Module::KeyboardBrightness => switch(&self.keyboard),
            Module::FocusMode => switch(&self.focus_mode),
            Module::Mute => match &self.volume {
                Reading::Ready(volume) => switch(&Reading::Ready(volume.muted)),
                Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
            },
            // an action rather than a switch: it works until the session refuses it
            Module::LockScreen => TileState::Off,
            Module::NowPlaying => match &self.media {
                Reading::Ready(Some(media)) if media.playing => TileState::On,
                Reading::Ready(_) => TileState::Off,
                Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
            },
            Module::Display => usable(
                self.brightness.ready().is_some(),
                unavailable(&self.brightness),
            ),
            Module::Sound => usable(self.volume.ready().is_some(), unavailable(&self.volume)),
        }
    }

    /// The tile's title: the focus capsule names the mode that is on.
    pub fn title(&self, module: Module) -> &'static str {
        match (module, self.tile(module)) {
            (Module::FocusMode, TileState::On) => "Do Not Disturb",
            _ => module.title(),
        }
    }

    /// The second line of a capsule: the network name, or On / Off; "Unavailable" for a disabled tile (the reason is
    /// in [`TileState::Disabled`]); `None` for the focus capsule while no mode is on, which shows its title alone.
    pub fn subtitle(&self, module: Module) -> Option<&str> {
        Some(match (module, self.tile(module)) {
            (_, TileState::Disabled(_)) => "Unavailable",
            (Module::FocusMode, TileState::Off) => return None,
            (Module::WiFi, TileState::On) => match &self.wifi {
                Reading::Ready(WifiStatus {
                    network: Some(network),
                    ..
                }) => network,
                _ => "On",
            },
            (Module::Microphone, TileState::Off) => "Muted",
            (_, TileState::On) => "On",
            (_, TileState::Off) => "Off",
        })
    }

    /// The position of `slider`, 0 to 1, when its backend works.
    pub fn fraction(&self, slider: Slider) -> Option<f64> {
        match slider {
            Slider::Display => self.brightness.ready().copied(),
            Slider::Sound => self.volume.ready().map(|volume| volume.level),
        }
    }

    /// Move `slider` to `fraction` (clamped to 0 to 1) as the pointer drags it; the backend is written separately.
    /// Returns false when the slider is disabled and nothing changed.
    pub fn set_fraction(&mut self, slider: Slider, fraction: f64) -> bool {
        let fraction = fraction.clamp(0.0, 1.0);
        match (slider, &mut self.brightness, &mut self.volume) {
            (Slider::Display, Reading::Ready(level), _) => *level = fraction,
            (Slider::Sound, _, Reading::Ready(volume)) => {
                *volume = VolumeState {
                    level: fraction,
                    muted: fraction < MUTE_BELOW,
                }
            }
            _ => return false,
        }
        true
    }
}

fn unavailable<T>(reading: &Reading<T>) -> Option<&String> {
    match reading {
        Reading::Ready(_) => None,
        Reading::Unavailable(reason) => Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Controls {
        Controls {
            wifi: Reading::Ready(WifiStatus {
                enabled: true,
                network: Some("Home".into()),
            }),
            bluetooth: Reading::Ready(false),
            microphone: Reading::Ready(false),
            dock_hiding: Reading::Ready(true),
            keyboard: Reading::Unavailable("no keyboard backlight".into()),
            focus_mode: Reading::Unavailable("no service".into()),
            brightness: Reading::Ready(0.25),
            volume: Reading::Ready(VolumeState {
                level: 0.5,
                muted: false,
            }),
            media: Reading::Ready(None),
            networks: Reading::Ready(Vec::new()),
            devices: Reading::Ready(Vec::new()),
            outputs: Reading::Ready(Vec::new()),
        }
    }

    #[test]
    fn modules_are_in_reading_order() {
        let names: Vec<_> = Module::ALL.iter().map(|m| m.title()).collect();
        assert_eq!(
            names,
            [
                "Wi-Fi",
                "Now Playing",
                "Bluetooth",
                "Microphone",
                "Dock Hiding",
                "Lock Screen",
                "Mute",
                "Keyboard Brightness",
                "Focus",
                "Display",
                "Sound"
            ]
        );
    }

    #[test]
    fn the_modules_with_a_list_or_more_controls_open_a_detail() {
        let with: Vec<_> = Module::ALL.iter().filter_map(|m| m.detail()).collect();
        assert_eq!(with, Detail::ALL);
        assert!(Detail::ALL.iter().all(|d| d.module().detail() == Some(*d)));
    }

    #[test]
    fn tiles_follow_their_readings() {
        let controls = ready();
        assert_eq!(controls.tile(Module::WiFi), TileState::On);
        assert_eq!(controls.tile(Module::Bluetooth), TileState::Off);
        assert_eq!(controls.tile(Module::DockHiding), TileState::On);
        assert_eq!(
            controls.tile(Module::FocusMode),
            TileState::Disabled("no service".into())
        );
        assert_eq!(controls.tile(Module::Display), TileState::On);
        assert_eq!(controls.tile(Module::Mute), TileState::Off);
        assert_eq!(controls.tile(Module::NowPlaying), TileState::Off);
    }

    #[test]
    fn subtitles_name_the_network_or_the_switch() {
        let controls = ready();
        assert_eq!(
            (
                controls.subtitle(Module::WiFi),
                controls.subtitle(Module::Bluetooth),
                controls.subtitle(Module::Microphone),
                controls.subtitle(Module::FocusMode)
            ),
            (
                Some("Home"),
                Some("Off"),
                Some("Muted"),
                Some("Unavailable")
            )
        );
    }

    #[test]
    fn the_focus_capsule_shows_its_title_alone_until_a_mode_is_on() {
        let mut controls = ready();
        controls.focus_mode = Reading::Ready(false);
        assert_eq!(
            (
                controls.title(Module::FocusMode),
                controls.subtitle(Module::FocusMode)
            ),
            ("Focus", None)
        );
        controls.focus_mode = Reading::Ready(true);
        assert_eq!(
            (
                controls.title(Module::FocusMode),
                controls.subtitle(Module::FocusMode)
            ),
            ("Do Not Disturb", Some("On"))
        );
    }

    #[test]
    fn dragging_a_slider_moves_its_reading() {
        let mut controls = ready();
        assert!(controls.set_fraction(Slider::Display, 0.75));
        assert_eq!(controls.fraction(Slider::Display), Some(0.75));
        assert!(controls.set_fraction(Slider::Sound, 1.5));
        assert_eq!(controls.fraction(Slider::Sound), Some(1.0));
    }

    #[test]
    fn dragging_the_volume_to_the_end_mutes_it() {
        let mut controls = ready();
        controls.set_fraction(Slider::Sound, 0.0005);
        assert_eq!(
            controls.volume,
            Reading::Ready(VolumeState {
                level: 0.0005,
                muted: true
            })
        );
        assert_eq!(controls.tile(Module::Mute), TileState::On);
    }

    #[test]
    fn a_disabled_slider_does_not_move() {
        let mut controls = Controls::unread();
        assert!(!controls.set_fraction(Slider::Display, 0.5));
        assert_eq!(controls.fraction(Slider::Display), None);
    }

    #[test]
    fn a_network_is_joined_by_a_press_when_it_is_open_or_known() {
        let network = |secured, known, connected| Network {
            ssid: "n".into(),
            bars: Some(2),
            secured,
            known,
            connected,
        };
        assert!(network(false, false, false).joinable());
        assert!(network(true, true, false).joinable());
        assert!(!network(true, false, false).joinable());
        assert!(!network(false, false, true).joinable(), "already joined");
        assert_eq!(
            [-50, -60, -65, -70, -80].map(Network::bars_of),
            [3, 3, 2, 2, 1]
        );
    }

    #[test]
    fn the_speaker_icon_changes_at_a_third_and_two_thirds() {
        let icon = |level, muted| speaker_icon(&VolumeState { level, muted });
        assert_eq!(icon(0.0, false), "symbol.speaker.slash");
        assert_eq!(icon(0.9, true), "symbol.speaker.slash");
        assert_eq!(icon(0.33, false), "symbol.speaker.low");
        assert_eq!(icon(0.34, false), "symbol.speaker");
        assert_eq!(icon(0.67, false), "symbol.speaker.high");
    }
}
