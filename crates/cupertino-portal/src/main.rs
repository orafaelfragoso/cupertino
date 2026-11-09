//! The `cupertino-portal` process, which the server starts for the session. See the README.

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use cupertino_defaults::screen_sharing::ScreenSharingConf;
use cupertino_diagnostic_events as diag;
use cupertino_portal::app_chooser::AppChooser;
use cupertino_portal::capture::Capture;
use cupertino_portal::dialog::{inbox, Dialogs};
use cupertino_portal::file_chooser::FileChooser;
use cupertino_portal::pick_overlay::PickOverlay;
use cupertino_portal::screen_cast::ScreenCast;
use cupertino_portal::screenshot::{self, Screenshot};
use cupertino_portal::settings::{Appearance, Service};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_protocol_app::{AppEffects, AppEvent};
use cupertino_wayland_client::{Connection, Event};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    // first: the panic hook and the user's event store are in place for everything below
    diag::init(diag::component!(), "portal", diag::Sink::UserStore);
    // `run` only returns on failure, which makes the server restart the process
    if let Err(e) = run() {
        eprintln!("cupertino-portal: {e}");
        diag::exit_error(&e);
    }
    ExitCode::FAILURE
}

/// `appearance` after the appearance events among `events`.
fn fold(mut appearance: Appearance, events: &[Event]) -> Appearance {
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

/// The file-chooser, app-chooser, screen-sharing and screenshot windows; a dialog that cannot be shown ends the process.
fn dialogs() {
    if let Err(e) = cupertino_kit::app::run::<Dialogs>() {
        eprintln!("cupertino-portal: {e}");
        diag::exit_error(&e);
    }
    std::process::exit(1);
}

fn run() -> Result<()> {
    let mut conn = Connection::connect()?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let _observer = effects.appearance(&conn);
    // the session is published once the name is owned, so the first reply must already carry the session's appearance
    conn.roundtrip()?;
    let (requests, received) = mpsc::channel();
    let (shoots, shots) = mpsc::channel();
    // without the capture globals the other interfaces are still served
    let mut capture = Capture::bind(&mut conn, received, shots)
        .inspect_err(|e| eprintln!("cupertino-portal: no screen sharing or screenshots: {e}"))
        .ok();
    let (picks, picked) = mpsc::channel();
    // without a layer shell PickColor answers that it failed
    let mut picker = capture
        .is_some()
        .then(|| PickOverlay::bind(&mut conn, picked))
        .flatten();
    if capture.is_some() && picker.is_none() {
        eprintln!("cupertino-portal: no colour picking: the server lacks zwlr_layer_shell_v1");
    }
    let first = conn.wait(Some(Duration::ZERO))?;
    let mut appearance = fold(Appearance::default(), &first);
    if let Some(capture) = &mut capture {
        capture.update(&conn, &first);
    }
    let bus = zbus::blocking::Connection::session()?;
    let settings = Service::serve(&bus, appearance)?;
    bus.object_server()
        .at(PORTAL_PATH, FileChooser::new(|event| inbox().push(event)))?;
    bus.object_server().at(
        PORTAL_PATH,
        AppChooser::new(cupertino_apps::entry::xdg_application_dirs(), |event| {
            inbox().push(event)
        }),
    )?;
    if let Some(capture) = &capture {
        let sources = capture.sources();
        let waker = conn.waker();
        ScreenCast::new(
            move || sources.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            |event| inbox().push(event),
            move |request| {
                let _ = requests.send(request);
                waker.wake();
            },
        )
        .with_applications(cupertino_apps::entry::xdg_application_dirs())
        .with_grants(ScreenSharingConf::path())
        .serve(&bus)?;
        let waker = conn.waker();
        let mut screenshot = Screenshot::new(
            cupertino_apps::entry::xdg_application_dirs(),
            screenshot::pictures,
            |event| inbox().push(event),
            move |shoot| {
                let _ = shoots.send(shoot);
                waker.wake();
            },
        );
        if picker.is_some() {
            let waker = conn.waker();
            screenshot = screenshot.with_picker(move |event| {
                let _ = picks.send(event);
                waker.wake();
            });
        }
        bus.object_server().at(PORTAL_PATH, screenshot)?;
    }
    std::thread::Builder::new()
        .name("dialogs".to_owned())
        .spawn(dialogs)?;
    bus.request_name(BUS_NAME)?;
    loop {
        let events = conn.wait(capture.as_ref().and_then(Capture::timeout))?;
        if let Some(picker) = &mut picker {
            picker.update(&mut conn, &events);
        }
        if let Some(capture) = &mut capture {
            capture.update(&conn, &events);
        }
        let next = fold(appearance, &events);
        if next != appearance {
            settings.set(next)?;
            appearance = next;
        }
    }
}
