//! The `cupertino-portal` process, which the server starts for the session. See the README.

use std::process::ExitCode;

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

fn run() -> Result<()> {
    let mut conn = Connection::connect()?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let _observer = effects.appearance(&conn);
    let mut appearance = Appearance::default();
    let bus = zbus::blocking::Connection::session()?;
    let settings = Service::serve(&bus, appearance)?;
    bus.request_name(BUS_NAME)?;
    loop {
        for event in conn.wait(None)? {
            let Event::Extension(event) = event else {
                continue;
            };
            let next = match event.downcast_ref::<AppEvent>() {
                Some(AppEvent::Appearance { dark }) => Appearance {
                    dark: *dark,
                    ..appearance
                },
                Some(AppEvent::DisplayOptions(options)) => Appearance {
                    increase_contrast: options.increase_contrast,
                    reduce_motion: options.reduce_motion,
                    ..appearance
                },
                _ => continue,
            };
            settings.set(next)?;
            appearance = next;
        }
    }
}
