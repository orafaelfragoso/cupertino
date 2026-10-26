//! What the panel shows: the modules in grid order, the reading behind each tile and the battery.
//!
//! Pure logic; nothing here talks to a backend. A reading that could not be taken is `Unavailable` with the reason, so a
//! failing backend disables its tile instead of showing a default.

/// Below this level a volume counts as muted ("mute when `v < 0.001`").
pub const MUTE_BELOW: f64 = 0.001;

/// The speaker glyph changes at a third and at two thirds.
const LOW_BELOW: f64 = 1.0 / 3.0;
const MEDIUM_BELOW: f64 = 2.0 / 3.0;

/// Fill width of the battery glyph at full charge, in points (`clamp(round(level * 19), 2, 19)).
pub const BATTERY_FILL_MAX: f64 = 19.0;
/// Narrowest battery fill, so an empty battery still shows a sliver.
pub const BATTERY_FILL_MIN: f64 = 2.0;
/// At or below this level the battery fill is red (`level <= 0.1`).
pub const BATTERY_LOW_LEVEL: f64 = 0.1;

/// A module of the panel, in grid order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    WiFi,
    Bluetooth,
    FileSharing,
    FocusMode,
    Stage,
    ScreenMirroring,
    Display,
    Sound,
    NowPlaying,
}

impl Module {
    /// Every module in grid order: the connectivity tile's three rows, the focus mode tile, the stage and screen
    /// mirroring tiles, then the display, sound and now playing rows.
    pub const ALL: [Module; 9] =
        [Module::WiFi, Module::Bluetooth, Module::FileSharing, Module::FocusMode, Module::Stage, Module::ScreenMirroring, Module::Display, Module::Sound, Module::NowPlaying];

    /// The tile's title.
    pub fn title(self) -> &'static str {
        match self {
            Module::WiFi => "Wi-Fi",
            Module::Bluetooth => "Bluetooth",
            Module::FileSharing => "File Sharing",
            Module::FocusMode => "Focus Mode",
            Module::Stage => "Stage",
            Module::ScreenMirroring => "Mirroring",
            Module::Display => "Display",
            Module::Sound => "Sound",
            Module::NowPlaying => "Now Playing",
        }
    }

    /// Why the module has no function yet, for the modules whose backend does not exist.
    pub fn unsupported(self) -> Option<&'static str> {
        match self {
            Module::FileSharing => Some("no file sharing service exists yet"),
            Module::Stage => Some("the stage process is not wired"),
            Module::ScreenMirroring => Some("no screen mirroring service exists yet"),
            Module::NowPlaying => Some("no media player service exists yet"),
            Module::WiFi | Module::Bluetooth | Module::FocusMode | Module::Display | Module::Sound => None,
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

    /// Whether there is a value, with the value dropped: what a slider row needs to know.
    fn as_usable(&self) -> Reading<bool> {
        match self {
            Reading::Ready(_) => Reading::Ready(true),
            Reading::Unavailable(reason) => Reading::Unavailable(reason.clone()),
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

/// The battery.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    /// Charge, 0 to 100.
    pub percent: u8,
    pub charging: bool,
}

impl Battery {
    /// Charge from 0 to 1.
    pub fn level(&self) -> f64 {
        f64::from(self.percent) / 100.0
    }

    /// Width of the fill in points: `clamp(round(level * 19), 2, 19).
    pub fn fill_width(&self) -> f64 {
        (self.level() * BATTERY_FILL_MAX).round().clamp(BATTERY_FILL_MIN, BATTERY_FILL_MAX)
    }

    /// Whether the fill is red (`level <= 0.1`).
    pub fn is_low(&self) -> bool {
        self.level() <= BATTERY_LOW_LEVEL
    }
}

/// What a tile shows. For the display and sound rows, `On` means the slider works.
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
        "symbol.speaker"
    } else {
        "symbol.speaker.high"
    }
}

/// The readings behind the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    pub wifi: Reading<WifiStatus>,
    pub bluetooth: Reading<bool>,
    pub focus_mode: Reading<bool>,
    /// Display brightness, 0 to 1.
    pub brightness: Reading<f64>,
    pub volume: Reading<VolumeState>,
    /// `None` on a machine without a battery.
    pub battery: Option<Battery>,
}

const NOT_READ: &str = "not read yet";

impl Controls {
    /// Nothing read yet: every backed module is disabled until the first reading.
    pub fn unread() -> Self {
        let unread = || NOT_READ.to_string();
        Self {
            wifi: Reading::Unavailable(unread()),
            bluetooth: Reading::Unavailable(unread()),
            focus_mode: Reading::Unavailable(unread()),
            brightness: Reading::Unavailable(unread()),
            volume: Reading::Unavailable(unread()),
            battery: None,
        }
    }

    /// The state of `module`'s tile.
    pub fn tile(&self, module: Module) -> TileState {
        if let Some(reason) = module.unsupported() {
            return TileState::Disabled(reason.to_string());
        }
        let switch = |reading: &Reading<bool>| match reading {
            Reading::Ready(true) => TileState::On,
            Reading::Ready(false) => TileState::Off,
            Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
        };
        match module {
            Module::WiFi => match &self.wifi {
                Reading::Ready(status) => switch(&Reading::Ready(status.enabled)),
                Reading::Unavailable(reason) => TileState::Disabled(reason.clone()),
            },
            Module::Bluetooth => switch(&self.bluetooth),
            Module::FocusMode => switch(&self.focus_mode),
            Module::Display => switch(&self.brightness.as_usable()),
            Module::Sound => switch(&self.volume.as_usable()),
            Module::FileSharing | Module::Stage | Module::ScreenMirroring | Module::NowPlaying => unreachable!("unsupported modules returned above"),
        }
    }

    /// The second line of a connectivity or focus mode tile: the network name, or On / Off; "Unavailable" for a
    /// disabled tile (the reason is in [`TileState::Disabled`]).
    pub fn subtitle(&self, module: Module) -> &str {
        match (module, self.tile(module)) {
            (_, TileState::Disabled(_)) => "Unavailable",
            (Module::WiFi, TileState::On) => match &self.wifi {
                Reading::Ready(WifiStatus { network: Some(network), .. }) => network,
                _ => "On",
            },
            (_, TileState::On) => "On",
            (_, TileState::Off) => "Off",
        }
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
            (Slider::Sound, _, Reading::Ready(volume)) => *volume = VolumeState { level: fraction, muted: fraction < MUTE_BELOW },
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Controls {
        Controls {
            wifi: Reading::Ready(WifiStatus { enabled: true, network: Some("Home".into()) }),
            bluetooth: Reading::Ready(false),
            focus_mode: Reading::Unavailable("no service".into()),
            brightness: Reading::Ready(0.25),
            volume: Reading::Ready(VolumeState { level: 0.5, muted: false }),
            battery: Some(Battery { percent: 80, charging: false }),
        }
    }

    #[test]
    fn modules_are_in_grid_order() {
        let names: Vec<_> = Module::ALL.iter().map(|m| m.title()).collect();
        assert_eq!(names, ["Wi-Fi", "Bluetooth", "File Sharing", "Focus Mode", "Stage", "Mirroring", "Display", "Sound", "Now Playing"]);
    }

    #[test]
    fn tiles_follow_their_readings() {
        let controls = ready();
        assert_eq!(controls.tile(Module::WiFi), TileState::On);
        assert_eq!(controls.tile(Module::Bluetooth), TileState::Off);
        assert_eq!(controls.tile(Module::FocusMode), TileState::Disabled("no service".into()));
        assert_eq!(controls.tile(Module::Display), TileState::On);
        assert_eq!(controls.tile(Module::Stage), TileState::Disabled("the stage process is not wired".into()));
    }

    #[test]
    fn subtitles_name_the_network_or_the_switch() {
        let controls = ready();
        assert_eq!((controls.subtitle(Module::WiFi), controls.subtitle(Module::Bluetooth), controls.subtitle(Module::FocusMode)), ("Home", "Off", "Unavailable"));
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
        assert_eq!(controls.volume, Reading::Ready(VolumeState { level: 0.0005, muted: true }));
    }

    #[test]
    fn a_disabled_slider_does_not_move() {
        let mut controls = Controls::unread();
        assert!(!controls.set_fraction(Slider::Display, 0.5));
        assert_eq!(controls.fraction(Slider::Display), None);
    }

    #[test]
    fn the_battery_fill_is_clamped_between_two_and_nineteen_points() {
        let width = |percent| Battery { percent, charging: false }.fill_width();
        assert_eq!((width(0), width(5), width(50), width(100)), (2.0, 2.0, 10.0, 19.0));
    }

    #[test]
    fn the_battery_is_low_at_ten_percent() {
        assert!(Battery { percent: 10, charging: false }.is_low());
        assert!(!Battery { percent: 11, charging: false }.is_low());
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
