//! The real backends over D-Bus: the display and keyboard backlights through `cupertino-powerd` and the session lock
//! through logind (on the system bus); the overlay, the focus mode (the do-not-disturb switch of
//! `cupertino-notifications`) and the media players' standard player interface on the session bus. The Wi-Fi radio
//! and the Bluetooth adapter are in `radios`.
//! Each connection is made on the first use and again after a failure, so a daemon that starts later is picked up.
//! Properties are read from the service every time, never from a cache.

use std::collections::HashMap;
use std::time::Duration;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{connection, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::backend::{
    BackendError, Brightness, MediaPlayer, OverlayKind, OverlayNotifier, SessionLock, Switch,
};
use crate::model::{Media, MediaCommand};

const POWER_NAME: &str = "org.cupertino.Power1";
const POWER_PATH: &str = "/org/cupertino/Power1";
const UPOWER_NAME: &str = "org.freedesktop.UPower";
const KEYBOARD_PATH: &str = "/org/freedesktop/UPower/KbdBacklight";
const KEYBOARD_INTERFACE: &str = "org.freedesktop.UPower.KbdBacklight";
const LOGIND_NAME: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIND_SESSION: &str = "org.freedesktop.login1.Session";
const DBUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const PLAYER_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const OSD_NAME: &str = "org.cupertino.Osd1";
const OSD_PATH: &str = "/org/cupertino/Osd1";
const CENTRE_NAME: &str = "org.cupertino.NotificationCenter1";
const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";

/// Lowest raw display brightness a slider reaches: 1, so the slider never turns the screen off (approximate, the
/// same floor as the overlay's key presses; the design maps 0.0 to the panel's device minimum).
const DISPLAY_FLOOR: u32 = 1;

/// How long a media player has to answer one call. The players are other applications and are read on the panel's
/// own thread, so one that hangs costs a reading this long instead of holding the panel.
pub const PLAYER_TIMEOUT: Duration = Duration::from_millis(200);

/// One bus connection, made when first needed.
struct Bus {
    session: bool,
    /// The bus's address; `None` takes the system or session bus of the environment.
    address: Option<String>,
    /// How long a method call may wait for its answer; `None` waits as long as it takes.
    timeout: Option<Duration>,
    connection: Option<Connection>,
}

impl Bus {
    fn system() -> Self {
        Self {
            session: false,
            address: None,
            timeout: None,
            connection: None,
        }
    }

    fn session() -> Self {
        Self {
            session: true,
            ..Self::system()
        }
    }

    fn connect(&self) -> zbus::Result<Connection> {
        let builder = match (&self.address, self.session) {
            (Some(address), _) => connection::Builder::address(address.as_str())?,
            (None, true) => connection::Builder::session()?,
            (None, false) => connection::Builder::system()?,
        };
        match self.timeout {
            Some(timeout) => builder.method_timeout(timeout),
            None => builder,
        }
        .build()
    }

    fn proxy(
        &mut self,
        name: &'static str,
        path: &'static str,
        interface: &'static str,
    ) -> zbus::Result<Proxy<'static>> {
        self.proxy_to(name.to_owned(), path.to_owned(), interface)
    }

    /// [`Bus::proxy`] for a name or path known only at run time.
    fn proxy_to(
        &mut self,
        name: String,
        path: String,
        interface: &'static str,
    ) -> zbus::Result<Proxy<'static>> {
        if self.connection.is_none() {
            self.connection = Some(self.connect()?);
        }
        let connection = self.connection.as_ref().expect("connected above");
        Builder::new(connection)
            .destination(name)?
            .path(path)?
            .interface(interface)?
            .cache_properties(CacheProperties::No)
            .build()
    }

    /// Forget the connection after a failure so the next call connects again.
    fn failed<T, E: Into<BackendError>>(
        &mut self,
        result: Result<T, E>,
    ) -> Result<T, BackendError> {
        result.map_err(|e| {
            self.connection = None;
            e.into()
        })
    }
}

/// A string field of a bus dictionary.
fn text(fields: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
        .filter(|s| !s.is_empty())
}

/// The do-not-disturb switch of `org.cupertino.NotificationCenter1`: the `DoNotDisturb` property and `SetDoNotDisturb`.
pub struct NotificationFocusMode {
    bus: Bus,
}

impl NotificationFocusMode {
    /// A focus mode backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self {
            bus: Bus::session(),
        }
    }
}

impl Default for NotificationFocusMode {
    fn default() -> Self {
        Self::new()
    }
}

impl Switch for NotificationFocusMode {
    fn read(&mut self) -> Result<bool, BackendError> {
        let result = self
            .bus
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|proxy| proxy.get_property::<bool>("DoNotDisturb"));
        self.bus.failed(result)
    }

    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|proxy| proxy.call::<_, _, ()>("SetDoNotDisturb", &(on,)));
        self.bus.failed(result)
    }
}

/// The display backlight of `org.cupertino.Power1`, in the daemon's raw units mapped linearly onto 0 to 1.
pub struct PowerdBrightness {
    bus: Bus,
}

impl PowerdBrightness {
    /// A brightness backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for PowerdBrightness {
    fn default() -> Self {
        Self::new()
    }
}

impl Brightness for PowerdBrightness {
    fn read(&mut self) -> Result<f64, BackendError> {
        let result = self
            .bus
            .proxy(POWER_NAME, POWER_PATH, POWER_NAME)
            .and_then(|proxy| {
                let current: u32 = proxy.call("GetBrightness", &())?;
                let max: u32 = proxy.call("GetMaxBrightness", &())?;
                Ok(if max == 0 {
                    Err("the display backlight reports a maximum of 0".into())
                } else {
                    Ok(f64::from(current) / f64::from(max))
                })
            });
        self.bus.failed(result)?
    }

    fn set(&mut self, level: f64) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(POWER_NAME, POWER_PATH, POWER_NAME)
            .and_then(|proxy| {
                let max: u32 = proxy.call("GetMaxBrightness", &())?;
                if max == 0 {
                    return Ok(Err("the display backlight reports a maximum of 0".into()));
                }
                let raw = (level.clamp(0.0, 1.0) * f64::from(max)).round() as u32;
                proxy.call::<_, _, ()>(
                    "SetBrightness",
                    &(raw.clamp(DISPLAY_FLOOR.min(max), max),),
                )?;
                Ok(Ok(()))
            });
        self.bus.failed(result)?
    }
}

/// The keyboard backlight of the power daemon (`org.freedesktop.UPower.KbdBacklight`): on is any level above 0;
/// switching on goes to the full level, as the keyboard's own key does on most machines.
pub struct UpowerKeyboard {
    bus: Bus,
}

impl UpowerKeyboard {
    /// A keyboard backlight backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for UpowerKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Switch for UpowerKeyboard {
    fn read(&mut self) -> Result<bool, BackendError> {
        let result = self
            .bus
            .proxy(UPOWER_NAME, KEYBOARD_PATH, KEYBOARD_INTERFACE)
            .and_then(|proxy| {
                let max: i32 = proxy.call("GetMaxBrightness", &())?;
                Ok((max, proxy.call::<_, _, i32>("GetBrightness", &())?))
            });
        match self.bus.failed(result) {
            Ok((max, _)) if max <= 0 => Err("there is no keyboard backlight".into()),
            Ok((_, level)) => Ok(level > 0),
            Err(_) => Err("there is no keyboard backlight".into()),
        }
    }

    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(UPOWER_NAME, KEYBOARD_PATH, KEYBOARD_INTERFACE)
            .and_then(|proxy| {
                let max: i32 = proxy.call("GetMaxBrightness", &())?;
                proxy.call::<_, _, ()>("SetBrightness", &(if on { max } else { 0 },))
            });
        self.bus.failed(result)
    }
}

/// Locks this session through logind (`Session.Lock` on the session the process belongs to), which asks the session's
/// lock screen to come up.
pub struct LogindLock {
    bus: Bus,
}

impl LogindLock {
    /// A session lock; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for LogindLock {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionLock for LogindLock {
    fn lock(&mut self) -> Result<(), BackendError> {
        let session = self
            .bus
            .proxy(LOGIND_NAME, LOGIND_PATH, LOGIND_MANAGER)
            .and_then(|proxy| {
                proxy.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),))
            });
        let session = self.bus.failed(session)?;
        let result = self
            .bus
            .proxy_to(LOGIND_NAME.to_owned(), session.to_string(), LOGIND_SESSION)
            .and_then(|proxy| proxy.call::<_, _, ()>("Lock", &()));
        self.bus.failed(result)
    }
}

/// The media players of the session through their standard player interface (`org.mpris.MediaPlayer2.*`): the one
/// playing, else the first that has a title; the buttons go to the same player.
pub struct MprisPlayer {
    bus: Bus,
    /// The player the tile shows, from the last reading.
    player: Option<String>,
}

impl MprisPlayer {
    /// A media backend on the session bus; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self::on(Bus::session())
    }

    /// A media backend on the bus at `address`.
    pub fn at(address: &str) -> Self {
        Self::on(Bus {
            address: Some(address.to_owned()),
            ..Bus::session()
        })
    }

    /// Every call bounded by [`PLAYER_TIMEOUT`].
    fn on(bus: Bus) -> Self {
        Self {
            bus: Bus {
                timeout: Some(PLAYER_TIMEOUT),
                ..bus
            },
            player: None,
        }
    }

    fn player(&mut self, name: String) -> zbus::Result<Proxy<'static>> {
        self.bus
            .proxy_to(name, PLAYER_PATH.to_owned(), PLAYER_INTERFACE)
    }

    /// What player `name` has, if it has a title.
    fn media(&mut self, name: &str) -> Option<Media> {
        let proxy = self.player(name.to_owned()).ok()?;
        let metadata: HashMap<String, OwnedValue> = proxy.get_property("Metadata").ok()?;
        let title = text(&metadata, "xesam:title")?;
        let artist = metadata
            .get("xesam:artist")
            .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
            .map(|artists| artists.join(", "))
            .filter(|a| !a.is_empty());
        let status: String = proxy.get_property("PlaybackStatus").ok()?;
        let can = |property: &str| proxy.get_property::<bool>(property).unwrap_or(false);
        Some(Media {
            title,
            artist,
            playing: status == "Playing",
            can_previous: can("CanGoPrevious"),
            can_next: can("CanGoNext"),
            can_play_pause: can("CanPlay") || can("CanPause"),
        })
    }
}

impl Default for MprisPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaPlayer for MprisPlayer {
    fn read(&mut self) -> Result<Option<Media>, BackendError> {
        let names = self
            .bus
            .proxy(DBUS_NAME, DBUS_PATH, DBUS_NAME)
            .and_then(|proxy| proxy.call::<_, _, Vec<String>>("ListNames", &()));
        let mut names: Vec<String> = self
            .bus
            .failed(names)?
            .into_iter()
            .filter(|n| n.starts_with(PLAYER_PREFIX))
            .collect();
        names.sort();
        let mut first = None;
        for name in names {
            let Some(media) = self.media(&name) else {
                continue;
            };
            if media.playing {
                self.player = Some(name);
                return Ok(Some(media));
            }
            first.get_or_insert((name, media));
        }
        self.player = first.as_ref().map(|(name, _)| name.clone());
        Ok(first.map(|(_, media)| media))
    }

    fn command(&mut self, command: MediaCommand) -> Result<(), BackendError> {
        let name = self.player.clone().ok_or("no media player is open")?;
        let method = match command {
            MediaCommand::Previous => "Previous",
            MediaCommand::PlayPause => "PlayPause",
            MediaCommand::Next => "Next",
        };
        let result = self
            .player(name)
            .and_then(|proxy| proxy.call::<_, _, ()>(method, &()));
        self.bus.failed(result)
    }
}

/// The overlay's `Show` on the session bus.
pub struct SessionOverlay {
    bus: Bus,
}

impl SessionOverlay {
    /// An overlay notifier; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self {
            bus: Bus::session(),
        }
    }
}

impl Default for SessionOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayNotifier for SessionOverlay {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(OSD_NAME, OSD_PATH, OSD_NAME)
            .and_then(|proxy| proxy.call::<_, _, ()>("Show", &(kind.name(), level)));
        self.bus.failed(result)
    }
}
