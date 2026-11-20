//! What the panel's effects do, through the services that own what it shows: the output and input volumes and the
//! sound outputs through `org.cupertino.Audio1` of `cupertino-audiod` and the media players through their standard
//! player interface (`org.mpris.MediaPlayer2.Player`) on the component's session bus connection; the display backlight
//! through `org.cupertino.Power1`, the keyboard backlight through `org.freedesktop.UPower.KbdBacklight` and the session
//! lock through logind on its system bus connection; the Wi-Fi radio and the Bluetooth adapter through whichever
//! service owns them (`cupertino-radios`); the dock's hiding and the stored choice of controls in their settings files.
//!
//! Nothing waits on the UI thread. The bus calls are made with zbus's async API, in futures the component's UI loop
//! polls; the radios' calls, which wait for their answers, and the settings files run on the toolkit's I/O pool. The
//! media players are other applications, so their calls end at a deadline the component sets ([`PLAYER_DEADLINE`]):
//! one that hangs costs a reading that long instead of the panel's media tile.

use std::collections::{HashMap, HashSet};
use std::future::{poll_fn, Future};
use std::pin::{pin, Pin};
use std::task::Poll;
use std::time::Duration;

use cupertino_defaults::control_panel::ControlPanelSettings;
use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::Command as Do;
use cupertino_kit::task;
use cupertino_radios::{refusal, unanswered, BluetoothBackend, NetworkBackend, CALL_TIMEOUT};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Connection;

use crate::controller::{Change, Listed, Playing, Read, Source};
use crate::model::{Device, Media, MediaCommand, Network, Output, VolumeState, WifiStatus};

/// The sound service: its name, object and interface (`docs/architecture/contracts.yaml`).
const AUDIO: Service = Service {
    name: "org.cupertino.Audio1",
    path: "/org/cupertino/Audio1",
    interface: "org.cupertino.Audio1",
};
/// The direction `Audio1` calls the default sink, and the default source.
const OUTPUT: &str = "output";
const INPUT: &str = "input";

/// The power daemon's display backlight.
const DISPLAY: Service = Service {
    name: "org.cupertino.Power1",
    path: "/org/cupertino/Power1",
    interface: "org.cupertino.Power1",
};
/// UPower's keyboard backlight.
const KEYBOARD: Service = Service {
    name: "org.freedesktop.UPower",
    path: "/org/freedesktop/UPower/KbdBacklight",
    interface: "org.freedesktop.UPower.KbdBacklight",
};
/// logind's manager, which names the session of a process.
const LOGIN: Service = Service {
    name: "org.freedesktop.login1",
    path: "/org/freedesktop/login1",
    interface: "org.freedesktop.login1.Manager",
};
/// The interface of a logind session, whose `Lock` asks the session's lock screen to come up.
const LOGIN_SESSION: &str = "org.freedesktop.login1.Session";
/// The bus daemon, which lists the media players.
const BUS: Service = Service {
    name: "org.freedesktop.DBus",
    path: "/org/freedesktop/DBus",
    interface: "org.freedesktop.DBus",
};
const PLAYER_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Lowest raw display brightness the slider reaches: 1, so the slider never turns the screen off (approximate, the
/// same floor as the overlay's key presses).
const DISPLAY_FLOOR: u32 = 1;

/// How long the media players have to answer a reading or a button: a call to the bus daemon and one to each player,
/// made together, from players that answer as they should (approximate).
pub const PLAYER_DEADLINE: Duration = Duration::from_millis(400);
/// How long a reading or a switch of a radio should take: the lookup of its service and its calls, each answered
/// within the radios' own wait.
const RADIO_DEADLINE: Duration = Duration::from_secs(2 * CALL_TIMEOUT.as_secs());
/// How long reading or writing a settings file should take: a small file on a local disk.
const FILE_DEADLINE: Duration = Duration::from_secs(2);

/// What a tile says when no service serves its radio.
const NO_NETWORK_SERVICE: &str = "no network service is running";
const NO_BLUETOOTH_SERVICE: &str = "no Bluetooth service is running";
/// What a tile says on a machine without the radio.
const NO_WIFI_DEVICE: &str = "there is no Wi-Fi device";
const NO_ADAPTER: &str = "there is no Bluetooth adapter";
/// What a tile says once its service did not answer.
const NETWORK_SILENT: &str = "the network service does not answer";
const BLUETOOTH_SILENT: &str = "the Bluetooth service does not answer";
/// What the keyboard tile says on a machine without a backlight, or when the service cannot tell.
const NO_KEYBOARD: &str = "there is no keyboard backlight";
/// What the settings tiles say when the session names no settings directory.
const NO_SETTINGS: &str = "there is no settings directory";
/// What a media player's call ends with at the deadline.
const PLAYER_SILENT: &str = "the media player does not answer";
/// What an effect ends with when its bus connection could not be made.
const NO_SESSION_BUS: &str = "no session bus connection";
const NO_SYSTEM_BUS: &str = "no system bus connection";

/// A bus object and its interface.
#[derive(Clone, Copy)]
struct Service {
    name: &'static str,
    path: &'static str,
    interface: &'static str,
}

impl Service {
    /// Calls `method` with `body` on `connection` and reads the answer as `T`.
    async fn call<B, T>(self, connection: &Connection, method: &str, body: &B) -> zbus::Result<T>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        connection
            .call_method(
                Some(self.name),
                self.path,
                Some(self.interface),
                method,
                body,
            )
            .await?
            .body()
            .deserialize()
    }
}

/// The connections the effects go through, each `None` while there is none.
#[derive(Clone, Debug, Default)]
pub struct Buses {
    pub session: Option<Connection>,
    pub system: Option<Connection>,
}

/// The bus connection an effect needs before it can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Through {
    Session,
    System,
}

/// The connection reading `source` needs; `None` for a settings file.
pub fn read_through(source: Source) -> Option<Through> {
    match source {
        Source::Microphone | Source::Volume | Source::Outputs | Source::Media => {
            Some(Through::Session)
        }
        Source::Wifi
        | Source::Networks
        | Source::Bluetooth
        | Source::Devices
        | Source::Brightness
        | Source::Keyboard => Some(Through::System),
        Source::DockHiding | Source::Layout => None,
    }
}

/// The connection `change` needs; `None` for a settings file.
pub fn change_through(change: &Change) -> Option<Through> {
    match change {
        Change::Lock => Some(Through::System),
        change => change.source().and_then(read_through),
    }
}

/// Where a media player's calls hear their deadline: the receiver closes when it has come.
pub type Deadline = async_channel::Receiver<()>;

/// Reads `source` and hands the reading to `done`; a bus call that has no connection fails at once. `deadline` ends
/// the calls of a media reading.
pub fn read<M: Send + 'static>(
    source: Source,
    buses: &Buses,
    deadline: Option<Deadline>,
    done: fn(Read) -> M,
) -> Do<M> {
    let failed = |reason: &str| {
        let read = Read::failed(source, reason.to_owned());
        Do::Perform(Box::pin(async move { done(read) }))
    };
    match source {
        Source::DockHiding => task::io(FILE_DEADLINE, || Read::DockHiding(dock_hiding()), done),
        Source::Layout => task::io(FILE_DEADLINE, || Read::Layout(layout()), done),
        Source::Wifi | Source::Networks | Source::Bluetooth | Source::Devices => {
            let Some(system) = buses.system.clone() else {
                return failed(NO_SYSTEM_BUS);
            };
            // the radios' calls wait for their answers, which the connection reads on the UI thread
            let system = zbus::blocking::Connection::from(system);
            task::io(RADIO_DEADLINE, move || read_radio(source, &system), done)
        }
        Source::Brightness | Source::Keyboard => {
            let Some(system) = buses.system.clone() else {
                return failed(NO_SYSTEM_BUS);
            };
            Do::Perform(Box::pin(async move {
                done(match source {
                    Source::Brightness => Read::Brightness(brightness(&system).await),
                    _ => Read::Keyboard(keyboard(&system).await),
                })
            }))
        }
        Source::Microphone | Source::Volume | Source::Outputs | Source::Media => {
            let Some(session) = buses.session.clone() else {
                return failed(NO_SESSION_BUS);
            };
            Do::Perform(Box::pin(async move {
                done(match source {
                    Source::Microphone => {
                        Read::Microphone(volume(&session, INPUT).await.map(|input| !input.muted))
                    }
                    Source::Volume => Read::Volume(volume(&session, OUTPUT).await),
                    Source::Outputs => Read::Outputs(outputs(&session).await),
                    _ => Read::Media(media(&session, deadline).await),
                })
            }))
        }
    }
}

/// Carries `change` out and hands its outcome to `done`: the volume reached for a volume change, else nothing, or why
/// it failed. `deadline` ends the call of a media button.
pub fn change<M: Send + 'static>(
    change: Change,
    buses: &Buses,
    deadline: Option<Deadline>,
    done: fn(Change, Result<Option<VolumeState>, String>) -> M,
) -> Do<M> {
    let asked = change.clone();
    let finish = move |result: Result<Option<VolumeState>, String>| done(asked, result);
    let through = change_through(&change);
    let connection = match through {
        Some(Through::Session) => buses.session.clone().ok_or(NO_SESSION_BUS),
        Some(Through::System) => buses.system.clone().ok_or(NO_SYSTEM_BUS),
        None => Err(""),
    };
    match (change, connection) {
        (Change::DockHiding(on), _) => task::io(FILE_DEADLINE, move || set_dock_hiding(on), finish),
        (Change::Layout(ids), _) => task::io(FILE_DEADLINE, move || store_layout(ids), finish),
        (_, Err(reason)) => {
            let reason = reason.to_owned();
            Do::Perform(Box::pin(async move { finish(Err(reason)) }))
        }
        (
            change @ (Change::Wifi(_)
            | Change::Bluetooth(_)
            | Change::Join(_)
            | Change::Connect(..)),
            Ok(system),
        ) => {
            let system = zbus::blocking::Connection::from(system);
            // BlueZ answers a connection once the link is up or has failed, which takes seconds
            let deadline = match change {
                Change::Connect(..) => cupertino_radios::LINK_TIMEOUT,
                _ => RADIO_DEADLINE,
            };
            task::io(deadline, move || switch_radio(&change, &system), finish)
        }
        (change, Ok(connection)) => Do::Perform(Box::pin(async move {
            finish(change_on_bus(change, &connection, deadline).await)
        })),
    }
}

/// A change made with an async call on the session or system bus.
async fn change_on_bus(
    change: Change,
    connection: &Connection,
    deadline: Option<Deadline>,
) -> Result<Option<VolumeState>, String> {
    match change {
        Change::Volume(target) => set_volume(connection, OUTPUT, target).await.map(Some),
        Change::Microphone(on) => {
            let input = volume(connection, INPUT).await?;
            set_volume(
                connection,
                INPUT,
                VolumeState {
                    level: input.level,
                    muted: !on,
                },
            )
            .await
            .map(|_| None)
        }
        Change::Output(id) => AUDIO
            .call::<_, ()>(connection, "SetDefault", &(OUTPUT, id))
            .await
            .map(|()| None)
            .map_err(|e| refusal(&e)),
        Change::Brightness(level) => set_brightness(connection, level).await.map(|()| None),
        Change::Keyboard(on) => set_keyboard(connection, on).await.map(|()| None),
        Change::Lock => lock(connection).await.map(|()| None),
        Change::Media { player, command } => press(connection, &player, command, deadline)
            .await
            .map(|()| None),
        // carried out on the I/O pool by [`change`]
        Change::Wifi(_)
        | Change::Bluetooth(_)
        | Change::Join(_)
        | Change::Connect(..)
        | Change::DockHiding(_)
        | Change::Layout(_) => Err(format!("{} is no bus call", change.what())),
    }
}

/// The volume of `direction` now.
async fn volume(audio: &Connection, direction: &str) -> Result<VolumeState, String> {
    let (level, muted): (f64, bool) = AUDIO
        .call(audio, "GetVolume", &(direction,))
        .await
        .map_err(|e| refusal(&e))?;
    Ok(VolumeState { level, muted })
}

/// Sets the volume of `direction` and gives the volume reached.
async fn set_volume(
    audio: &Connection,
    direction: &str,
    target: VolumeState,
) -> Result<VolumeState, String> {
    let (level, muted): (f64, bool) = AUDIO
        .call(
            audio,
            "SetVolume",
            &(direction, target.level.clamp(0.0, 1.0), target.muted),
        )
        .await
        .map_err(|e| refusal(&e))?;
    Ok(VolumeState { level, muted })
}

/// The sound outputs, the one applications play to marked.
async fn outputs(audio: &Connection) -> Result<Vec<Output>, String> {
    let devices: Vec<(u32, String, bool)> = AUDIO
        .call(audio, "ListDevices", &(OUTPUT,))
        .await
        .map_err(|e| refusal(&e))?;
    Ok(devices
        .into_iter()
        .map(|(id, name, current)| Output { id, name, current })
        .collect())
}

/// The display brightness, in the daemon's raw units mapped linearly onto 0 to 1.
async fn brightness(system: &Connection) -> Result<f64, String> {
    let current: u32 = DISPLAY
        .call(system, "GetBrightness", &())
        .await
        .map_err(|e| refusal(&e))?;
    let max = display_max(system).await?;
    Ok(f64::from(current) / f64::from(max))
}

async fn display_max(system: &Connection) -> Result<u32, String> {
    let max: u32 = DISPLAY
        .call(system, "GetMaxBrightness", &())
        .await
        .map_err(|e| refusal(&e))?;
    if max == 0 {
        return Err("the display backlight reports a maximum of 0".to_owned());
    }
    Ok(max)
}

/// Sets the display brightness; the level never goes below raw [`DISPLAY_FLOOR`].
async fn set_brightness(system: &Connection, level: f64) -> Result<(), String> {
    let max = display_max(system).await?;
    let raw = (level.clamp(0.0, 1.0) * f64::from(max)).round() as u32;
    DISPLAY
        .call::<_, ()>(
            system,
            "SetBrightness",
            &(raw.clamp(DISPLAY_FLOOR.min(max), max),),
        )
        .await
        .map_err(|e| refusal(&e))
}

/// Whether the keyboard backlight is on: any level above 0. A service that cannot tell reads as no backlight.
async fn keyboard(system: &Connection) -> Result<bool, String> {
    let max: i32 = KEYBOARD
        .call(system, "GetMaxBrightness", &())
        .await
        .map_err(|_| NO_KEYBOARD.to_owned())?;
    let level: i32 = KEYBOARD
        .call(system, "GetBrightness", &())
        .await
        .map_err(|_| NO_KEYBOARD.to_owned())?;
    if max <= 0 {
        return Err(NO_KEYBOARD.to_owned());
    }
    Ok(level > 0)
}

/// Switches the keyboard backlight on at its full level, as the keyboard's own key does on most machines, or off.
async fn set_keyboard(system: &Connection, on: bool) -> Result<(), String> {
    let max: i32 = KEYBOARD
        .call(system, "GetMaxBrightness", &())
        .await
        .map_err(|e| refusal(&e))?;
    KEYBOARD
        .call::<_, ()>(system, "SetBrightness", &(if on { max } else { 0 },))
        .await
        .map_err(|e| refusal(&e))
}

/// Locks the session the component's process belongs to: logind asks the session's lock screen to come up.
async fn lock(system: &Connection) -> Result<(), String> {
    let session: OwnedObjectPath = LOGIN
        .call(system, "GetSessionByPID", &(std::process::id(),))
        .await
        .map_err(|e| refusal(&e))?;
    system
        .call_method(
            Some(LOGIN.name),
            session.as_str(),
            Some(LOGIN_SESSION),
            "Lock",
            &(),
        )
        .await
        .map(|_| ())
        .map_err(|e| refusal(&e))
}

/// `call`'s outcome, or [`PLAYER_SILENT`] once `deadline` has come.
async fn within<T>(
    deadline: Option<&Deadline>,
    call: impl Future<Output = zbus::Result<T>>,
) -> Result<T, String> {
    let call = async { call.await.map_err(|e| refusal(&e)) };
    let Some(deadline) = deadline else {
        return call.await;
    };
    let expired = async {
        // the sender never sends: the receiver ends when the component drops it at the deadline
        let _ = deadline.recv().await;
        Err(PLAYER_SILENT.to_owned())
    };
    first(call, expired).await
}

/// The outcome of whichever of `a` and `b` ends first; `a` wins a tie.
async fn first<T>(a: impl Future<Output = T>, b: impl Future<Output = T>) -> T {
    let (mut a, mut b) = (pin!(a), pin!(b));
    poll_fn(move |cx| match a.as_mut().poll(cx) {
        Poll::Ready(value) => Poll::Ready(value),
        Poll::Pending => b.as_mut().poll(cx),
    })
    .await
}

/// The outcomes of `futures`, in their order, once every one has ended; they run together.
async fn all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut running: Vec<Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut ended: Vec<Option<F::Output>> = running.iter().map(|_| None).collect();
    poll_fn(move |cx| {
        let mut waiting = false;
        for (future, outcome) in running.iter_mut().zip(ended.iter_mut()) {
            if outcome.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(value) => *outcome = Some(value),
                    Poll::Pending => waiting = true,
                }
            }
        }
        if waiting {
            Poll::Pending
        } else {
            Poll::Ready(ended.iter_mut().filter_map(Option::take).collect())
        }
    })
    .await
}

/// What the media players play: the one playing, else the first by name that has a title; a player that does not
/// answer by the deadline is passed over.
async fn media(
    session: &Connection,
    deadline: Option<Deadline>,
) -> Result<Option<Playing>, String> {
    let deadline = deadline.as_ref();
    let names: Vec<String> = within(deadline, BUS.call(session, "ListNames", &())).await?;
    let mut players: Vec<String> = names
        .into_iter()
        .filter(|name| name.starts_with(PLAYER_PREFIX))
        .collect();
    players.sort();
    let readings = all(players
        .iter()
        .map(|name| player(session, name, deadline))
        .collect())
    .await;
    let mut shown = None;
    for (player, media) in players.into_iter().zip(readings) {
        let Some(media) = media else {
            continue;
        };
        if media.playing {
            return Ok(Some(Playing { player, media }));
        }
        shown.get_or_insert(Playing { player, media });
    }
    Ok(shown)
}

/// What the player of bus name `name` plays, when it has a title.
async fn player(session: &Connection, name: &str, deadline: Option<&Deadline>) -> Option<Media> {
    let properties: HashMap<String, OwnedValue> = within(deadline, async {
        session
            .call_method(
                Some(name),
                PLAYER_PATH,
                Some(PROPERTIES),
                "GetAll",
                &(PLAYER_INTERFACE,),
            )
            .await?
            .body()
            .deserialize()
    })
    .await
    .ok()?;
    let value = |key: &str| properties.get(key).and_then(|v| v.try_clone().ok());
    let metadata = HashMap::<String, OwnedValue>::try_from(value("Metadata")?).ok()?;
    let title = text(&metadata, "xesam:title")?;
    let artist = metadata
        .get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .map(|artists| artists.join(", "))
        .filter(|artists| !artists.is_empty());
    let status = String::try_from(value("PlaybackStatus")?).ok()?;
    let can = |key: &str| {
        value(key)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    };
    Some(Media {
        title,
        artist,
        playing: status == "Playing",
        can_previous: can("CanGoPrevious"),
        can_next: can("CanGoNext"),
        can_play_pause: can("CanPlay") || can("CanPause"),
    })
}

/// A non-empty string field of a bus dictionary.
fn text(fields: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
        .filter(|s| !s.is_empty())
}

/// Presses a button of the media tile on player `player`.
async fn press(
    session: &Connection,
    player: &str,
    command: MediaCommand,
    deadline: Option<Deadline>,
) -> Result<(), String> {
    let method = match command {
        MediaCommand::Previous => "Previous",
        MediaCommand::PlayPause => "PlayPause",
        MediaCommand::Next => "Next",
    };
    within(
        deadline.as_ref(),
        session.call_method(
            Some(player),
            PLAYER_PATH,
            Some(PLAYER_INTERFACE),
            method,
            &(),
        ),
    )
    .await
    .map(|_| ())
}

/// Why a radio's call failed, for a tile: `silent` when the service did not answer, else the service's or the bus's
/// words.
fn reason(error: &zbus::Error, silent: &str) -> String {
    if unanswered(error) {
        silent.to_owned()
    } else {
        refusal(error)
    }
}

/// The service that serves the network. For the I/O pool.
fn network(system: &zbus::blocking::Connection) -> Result<NetworkBackend, String> {
    let found = cupertino_radios::detect(system).map_err(|e| reason(&e, NETWORK_SILENT))?;
    found.network.ok_or_else(|| NO_NETWORK_SERVICE.to_owned())
}

/// The service that serves the Bluetooth adapter. For the I/O pool.
fn bluetooth(system: &zbus::blocking::Connection) -> Result<BluetoothBackend, String> {
    let found = cupertino_radios::detect(system).map_err(|e| reason(&e, BLUETOOTH_SILENT))?;
    found
        .bluetooth
        .ok_or_else(|| NO_BLUETOOTH_SERVICE.to_owned())
}

/// A reading of a radio. For the I/O pool: the radios' calls wait for their answers.
fn read_radio(source: Source, system: &zbus::blocking::Connection) -> Read {
    match source {
        Source::Wifi => Read::Wifi(wifi_status(system)),
        Source::Networks => Read::Networks(networks(system)),
        Source::Bluetooth => Read::Bluetooth(adapter(system).map(|adapter| adapter.powered)),
        _ => Read::Devices(adapter(system).map(devices)),
    }
}

/// The radio and the network of its first Wi-Fi device.
fn wifi_status(system: &zbus::blocking::Connection) -> Result<WifiStatus, String> {
    let backend = network(system)?;
    let link = backend
        .link(system)
        .map_err(|e| reason(&e, NETWORK_SILENT))?;
    if link.device.is_none() {
        return Err(NO_WIFI_DEVICE.to_owned());
    }
    Ok(WifiStatus {
        enabled: link.enabled,
        network: link.network().map(str::to_owned),
    })
}

/// The networks in range, after asking for a scan, whose list arrives with a later reading.
fn networks(system: &zbus::blocking::Connection) -> Result<Vec<Listed>, String> {
    let backend = network(system)?;
    // a refused or busy scan still leaves the last list to show
    let _ = backend.scan(system);
    let wifi = backend
        .wifi(system)
        .map_err(|e| reason(&e, NETWORK_SILENT))?;
    let mut listed: Vec<Listed> = Vec::new();
    for network in wifi.networks {
        // two networks that read the same could not be told apart in the list: the stronger one is shown
        if network.name.is_empty() || listed.iter().any(|(shown, _)| shown.ssid == network.name) {
            continue;
        }
        listed.push((
            Network {
                ssid: network.name,
                bars: (network.signal != i32::MIN).then(|| Network::bars_of(network.signal)),
                secured: network.security != "open",
                known: network.saved,
                connected: network.connected,
            },
            network.ssid,
        ));
    }
    Ok(listed)
}

/// The adapter and its devices.
fn adapter(system: &zbus::blocking::Connection) -> Result<cupertino_radios::Adapter, String> {
    let backend = bluetooth(system)?;
    let adapter = backend
        .adapter(system, &HashSet::new())
        .map_err(|e| reason(&e, BLUETOOTH_SILENT))?;
    if !adapter.available {
        return Err(NO_ADAPTER.to_owned());
    }
    Ok(adapter)
}

/// The paired devices of `adapter`, by name.
fn devices(adapter: cupertino_radios::Adapter) -> Vec<Device> {
    let mut devices: Vec<Device> = adapter
        .devices
        .into_iter()
        .filter(|d| d.paired)
        .map(|d| Device {
            name: d.title(),
            connected: d.connected,
            id: d.id,
        })
        .collect();
    devices.sort_by_key(|d| d.name.to_lowercase());
    devices
}

/// A switch of a radio, a join or a device's link. For the I/O pool.
fn switch_radio(
    change: &Change,
    system: &zbus::blocking::Connection,
) -> Result<Option<VolumeState>, String> {
    match change {
        Change::Wifi(on) => network(system)?
            .set_wireless_enabled(system, *on)
            .map_err(|e| reason(&e, NETWORK_SILENT)),
        Change::Join(key) => network(system)?
            // no passphrase: an open network, or one whose credentials the service holds
            .join(system, key, "")
            .map_err(|e| reason(&e, NETWORK_SILENT)),
        Change::Bluetooth(on) => bluetooth(system)?
            .set_powered(system, *on)
            .map_err(|e| reason(&e, BLUETOOTH_SILENT)),
        Change::Connect(id, true) => bluetooth(system)?
            .connect(system, id)
            .map_err(|e| reason(&e, BLUETOOTH_SILENT)),
        Change::Connect(id, false) => bluetooth(system)?
            .disconnect(system, id)
            .map_err(|e| reason(&e, BLUETOOTH_SILENT)),
        change => Err(format!("{} is no radio's change", change.what())),
    }
    .map(|()| None)
}

/// Whether the dock hides, the `autohide` key of its settings file. For the I/O pool.
fn dock_hiding() -> Result<bool, String> {
    let path = DockSettings::path().ok_or(NO_SETTINGS)?;
    let (settings, _) = DockSettings::load(&path, None).map_err(|e| e.to_string())?;
    Ok(settings.autohide.unwrap_or(false))
}

/// Makes the dock hide or stay, in its settings file, which the dock follows. For the I/O pool.
fn set_dock_hiding(on: bool) -> Result<Option<VolumeState>, String> {
    let path = DockSettings::path().ok_or(NO_SETTINGS)?;
    let (mut settings, _) = DockSettings::load(&path, None).map_err(|e| e.to_string())?;
    settings.autohide = Some(on);
    settings.store(&path).map_err(|e| e.to_string())?;
    Ok(None)
}

/// The stored choice of controls, the `controls` key of the panel's settings file. For the I/O pool.
fn layout() -> Result<Option<Vec<String>>, String> {
    let path = ControlPanelSettings::path().ok_or(NO_SETTINGS)?;
    let (settings, _) = ControlPanelSettings::load(&path, None).map_err(|e| e.to_string())?;
    Ok(settings.controls)
}

/// Stores the choice of controls. For the I/O pool.
fn store_layout(ids: Vec<String>) -> Result<Option<VolumeState>, String> {
    let path = ControlPanelSettings::path().ok_or(NO_SETTINGS)?;
    ControlPanelSettings {
        controls: Some(ids),
    }
    .store(&path)
    .map_err(|e| e.to_string())?;
    Ok(None)
}
