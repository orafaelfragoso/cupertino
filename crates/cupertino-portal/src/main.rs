//! The `cupertino-portal` process, which the server starts for the session. See the README.

use std::process::ExitCode;
use std::time::Duration;

use cupertino_portal::settings::{Appearance, Service};
use cupertino_portal::BUS_NAME;
use cupertino_protocol_app::{AppEffects, AppEvent};
use cupertino_wayland_client::{Connection, Event};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    // `run` only returns on failure, which makes the server restart the process
    if let Err(e) = run() {
        eprintln!("cupertino-portal: {e}");
    }
    ExitCode::FAILURE
}

/// `appearance` after the appearance events among `events`.
fn fold(mut appearance: Appearance, events: Vec<Event>) -> Appearance {
    for event in events {
        let Event::Extension(event) = event else {
            continue;
        };
        if let Some(event) = event.downcast_ref::<AppEvent>() {
            appearance = appearance.after(event);
        }
    }
    appearance
}

fn run() -> Result<()> {
    let mut conn = Connection::connect()?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let _observer = effects.appearance(&conn);
    // the session is published once the name is owned, so the first reply must already carry the session's appearance
    conn.roundtrip()?;
    let mut appearance = fold(Appearance::default(), conn.wait(Some(Duration::ZERO))?);
    let bus = zbus::blocking::Connection::session()?;
    let settings = Service::serve(&bus, appearance)?;
    bus.request_name(BUS_NAME)?;
    loop {
        let next = fold(appearance, conn.wait(None)?);
        if next != appearance {
            settings.set(next)?;
            appearance = next;
        }
    }
}
