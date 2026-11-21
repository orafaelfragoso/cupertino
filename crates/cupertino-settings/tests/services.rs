//! The services Settings reads and changes through the server, on a private bus with stand-ins of the server's
//! interfaces: the displays of `org.cupertino.Displays1` (D14) and the appearance and accessibility of
//! `org.cupertino.Session1` (D13), what a refusal tells the pane's row, and a server that is not on the bus, which
//! Settings starts without.
#![allow(
    clippy::disallowed_methods,
    reason = "a test drives its private bus and stand-ins from its own thread"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use cupertino_defaults::dock::default_tile_size;
use cupertino_settings::live::{Monitor, Reading};
use cupertino_settings::panes::Call;
use cupertino_settings::services::{
    monitors, Bus, DisplayRecord, Services, DISPLAYS_NAME, DISPLAYS_PATH, SESSION_NAME,
    SESSION_PATH,
};
use cupertino_settings::settings::{dock_display_height, Paths, Settings, DEFAULT_DISPLAY_HEIGHT};
use zbus::blocking::connection::Builder;
use zbus::fdo;

/// The calls a stand-in was made, as `Method(arguments)`.
type Calls = Arc<Mutex<Vec<String>>>;

/// The connector the displays stand-in refuses every change of.
const REFUSED: &str = "HDMI-A-1";

struct Displays {
    records: Vec<DisplayRecord>,
    calls: Calls,
}

impl Displays {
    fn change(&self, call: String, connector: &str) -> fdo::Result<()> {
        self.calls.lock().unwrap().push(call);
        if connector == REFUSED {
            return Err(fdo::Error::InvalidArgs("no such display".into()));
        }
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.Displays1")]
impl Displays {
    fn list_displays(&self) -> Vec<DisplayRecord> {
        self.records.clone()
    }

    fn set_scale(&self, connector: &str, scale: f64) -> fdo::Result<()> {
        self.change(format!("SetScale({connector}, {scale})"), connector)
    }

    fn set_refresh_rate(&self, connector: &str, mhz: u32) -> fdo::Result<()> {
        self.change(format!("SetRefreshRate({connector}, {mhz})"), connector)
    }

    fn set_position(&self, connector: &str, x: i32, y: i32) -> fdo::Result<()> {
        self.change(format!("SetPosition({connector}, {x}, {y})"), connector)
    }
}

struct Session {
    accessibility: HashMap<String, String>,
    calls: Calls,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session {
    #[zbus(property)]
    fn accessibility(&self) -> HashMap<String, String> {
        self.accessibility.clone()
    }

    #[zbus(property)]
    fn appearance(&self) -> String {
        "dark".into()
    }

    fn set_appearance(&self, name: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("SetAppearance({name})"));
    }

    fn set_accessibility(&self, key: &str, value: &str) -> fdo::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("SetAccessibility({key}, {value})"));
        if key == "bogus" {
            return Err(fdo::Error::InvalidArgs("Unknown key".into()));
        }
        Ok(())
    }
}

/// A private bus, with the server's stand-ins on it when `server`, and the calls they were made.
struct Fixture {
    bus: Child,
    address: String,
    calls: Calls,
    _server: Option<zbus::blocking::Connection>,
}

impl Fixture {
    fn new(server: bool) -> Self {
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let calls = Calls::default();
        let server = server.then(|| {
            let displays = Displays {
                records: vec![
                    record(
                        "eDP-1",
                        true,
                        (0, 0),
                        (2880, 1800),
                        2.0,
                        "normal",
                        (300, 190),
                    ),
                    record(REFUSED, true, (1440, 0), (1920, 1080), 1.0, "90", (0, 0)),
                ],
                calls: Arc::clone(&calls),
            };
            let session = Session {
                accessibility: HashMap::from([("reduce_motion".into(), "true".into())]),
                calls: Arc::clone(&calls),
            };
            Builder::address(address.as_str())
                .unwrap()
                .name(DISPLAYS_NAME)
                .unwrap()
                .name(SESSION_NAME)
                .unwrap()
                .serve_at(DISPLAYS_PATH, displays)
                .unwrap()
                .serve_at(SESSION_PATH, session)
                .unwrap()
                .build()
                .unwrap()
        });
        Fixture {
            bus,
            address,
            calls,
            _server: server,
        }
    }

    /// Services with both buses at the private bus.
    fn services(&self) -> Services {
        Services::on(Bus::address(&self.address), Bus::address(&self.address))
    }

    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.bus.kill();
        let _ = self.bus.wait();
    }
}

/// A display record of `ListDisplays`, with two modes at its current size and one at another.
fn record(
    connector: &str,
    enabled: bool,
    (x, y): (i32, i32),
    (width, height): (i32, i32),
    scale: f64,
    transform: &str,
    mm: (u32, u32),
) -> DisplayRecord {
    (
        connector.into(),
        format!("{connector} panel"),
        enabled,
        x,
        y,
        width,
        height,
        60_000,
        scale,
        transform.into(),
        vec![
            (width, height, 120_000),
            (width, height, 60_000),
            (width, height, 60_000),
            (640, 480, 75_000),
        ],
        mm,
    )
}

/// What a display shows: its size in points, whether it is turned, its refresh rates and its physical size.
type Shown = ((u32, u32), bool, Vec<u32>, Option<(u32, u32)>);
/// Where a display is: its connector, its position and its size in points.
type Placed = (Option<String>, Option<(i32, i32)>, Option<(u32, u32)>);

#[test]
fn displays1_records_become_the_displays_that_are_on() {
    // (record, its display, or None when it is left out)
    let cases: Vec<(DisplayRecord, Option<Shown>)> = vec![
        (
            record(
                "eDP-1",
                true,
                (0, 0),
                (2880, 1800),
                2.0,
                "normal",
                (300, 190),
            ),
            Some(((1440, 900), false, vec![60_000, 120_000], Some((300, 190)))),
        ),
        (
            record("DP-1", true, (0, 0), (1920, 1080), 1.5, "90", (0, 0)),
            Some(((720, 1280), true, vec![60_000, 120_000], None)),
        ),
        (
            record(
                "DP-2",
                true,
                (0, 0),
                (1920, 1080),
                f64::NAN,
                "flipped-270",
                (600, 340),
            ),
            Some(((1080, 1920), true, vec![60_000, 120_000], Some((600, 340)))),
        ),
        (
            record("DP-3", false, (0, 0), (1920, 1080), 1.0, "normal", (0, 0)),
            None,
        ),
        (
            record("DP-4", true, (0, 0), (0, 0), 1.0, "normal", (0, 0)),
            None,
        ),
    ];
    for (record, expected) in cases {
        let connector = record.0.clone();
        let found: Vec<Monitor> = monitors(vec![record]);
        let found = found.first().map(|m| {
            (
                m.logical.unwrap(),
                m.rotated,
                m.refresh_rates.clone(),
                m.physical_mm,
            )
        });
        assert_eq!(found, expected, "{connector}");
    }
}

#[test]
fn displays_are_read_from_displays1_and_unavailable_without_the_server() {
    let served = Fixture::new(true);
    let Reading::Ready(found) = served.services().displays() else {
        panic!("the displays are read");
    };
    let found: Vec<Placed> = found
        .iter()
        .map(|m| (m.connector.clone(), m.position, m.logical))
        .collect();
    assert_eq!(
        found,
        vec![
            (Some("eDP-1".into()), Some((0, 0)), Some((1440, 900))),
            (Some(REFUSED.into()), Some((1440, 0)), Some((1080, 1920))),
        ]
    );

    let empty = Fixture::new(false);
    assert_eq!(
        empty.services().displays(),
        Reading::Unavailable("The display server does not answer".into())
    );
}

#[test]
fn settings_start_without_displays1_with_the_docks_tile_size_for_the_usual_display() {
    // (the server is on the bus, the height the Dock's default tile size is for)
    for (server, height) in [(true, 900.0), (false, DEFAULT_DISPLAY_HEIGHT)] {
        let fixture = Fixture::new(server);
        let displays = fixture.services().displays();
        assert_eq!(dock_display_height(&displays), height, "server {server}");
        let settings = Settings::load(&Paths::default(), dock_display_height(&displays))
            .expect("settings load without files");
        assert_eq!(
            settings.tile_size(),
            default_tile_size(height),
            "server {server}"
        );
    }
}

#[test]
fn display_changes_are_calls_of_displays1_and_a_refusal_is_the_rows_sentence() {
    let fixture = Fixture::new(true);
    let mut services = fixture.services();
    let at = |connector: &str| connector.to_owned();
    // (the change, the call the server was made, what the row is told)
    let cases: Vec<(Call, &str, Result<(), String>)> = vec![
        (
            Call::SetDisplayScale {
                connector: at("eDP-1"),
                scale: 1.5,
            },
            "SetScale(eDP-1, 1.5)",
            Ok(()),
        ),
        (
            Call::SetDisplayRefresh {
                connector: at("eDP-1"),
                mhz: 120_000,
            },
            "SetRefreshRate(eDP-1, 120000)",
            Ok(()),
        ),
        (
            Call::SetDisplayPosition {
                connector: at("eDP-1"),
                x: -1440,
                y: 120,
            },
            "SetPosition(eDP-1, -1440, 120)",
            Ok(()),
        ),
        (
            Call::SetDisplayScale {
                connector: at(REFUSED),
                scale: 2.0,
            },
            "SetScale(HDMI-A-1, 2)",
            Err("The display size could not be changed.".into()),
        ),
        (
            Call::SetDisplayRefresh {
                connector: at(REFUSED),
                mhz: 60_000,
            },
            "SetRefreshRate(HDMI-A-1, 60000)",
            Err("The refresh rate could not be changed.".into()),
        ),
        (
            Call::SetDisplayPosition {
                connector: at(REFUSED),
                x: 0,
                y: 0,
            },
            "SetPosition(HDMI-A-1, 0, 0)",
            Err("The display could not be moved.".into()),
        ),
    ];
    for (call, made, told) in cases {
        let job = services
            .long_call(&call)
            .expect("a display change runs as a task of its own")
            .expect("the server is on the bus");
        assert_eq!(job(), told, "{call:?}");
        assert_eq!(fixture.calls(), vec![made.to_owned()], "{call:?}");
    }
}

#[test]
fn appearance_and_accessibility_go_through_session1() {
    let fixture = Fixture::new(true);
    let mut services = fixture.services();
    assert_eq!(services.dark(), Some(true));
    assert_eq!(
        services.accessibility(),
        Reading::Ready(HashMap::from([("reduce_motion".into(), "true".into())]))
    );
    type Change = fn(&mut Services) -> Result<(), String>;
    // (the change, the call the server was made, what the row is told)
    let cases: [(Change, &str, Result<(), String>); 4] = [
        (|s| s.set_appearance(true), "SetAppearance(dark)", Ok(())),
        (|s| s.set_appearance(false), "SetAppearance(light)", Ok(())),
        (
            |s| s.set_accessibility("increase_contrast", "true"),
            "SetAccessibility(increase_contrast, true)",
            Ok(()),
        ),
        (
            |s| s.set_accessibility("bogus", "1"),
            "SetAccessibility(bogus, 1)",
            Err("The setting could not be changed: Unknown key".into()),
        ),
    ];
    for (change, made, told) in cases {
        assert_eq!(change(&mut services), told, "{made}");
        assert_eq!(fixture.calls(), vec![made.to_owned()]);
    }
}

#[test]
fn session1_not_on_the_bus_reads_as_unknown_and_refuses_changes() {
    let fixture = Fixture::new(false);
    let mut services = fixture.services();
    assert_eq!(services.dark(), None);
    assert!(matches!(services.accessibility(), Reading::Unavailable(_)));
    assert!(services.set_appearance(true).is_err());
    assert!(services.set_accessibility("reduce_motion", "true").is_err());
}
