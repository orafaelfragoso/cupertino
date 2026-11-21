//! The `cupertino-portal` process, which the server starts for the session. See the README.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use cupertino_defaults::global_shortcuts::GlobalShortcutsConf;
use cupertino_defaults::screen_sharing::ScreenSharingConf;
use cupertino_diagnostic_events as diag;
use cupertino_portal::app_chooser::AppChooser;
use cupertino_portal::capture::Capture;
use cupertino_portal::dialogs::Dialogs;
use cupertino_portal::file_chooser::FileChooser;
use cupertino_portal::global_shortcuts::GlobalShortcuts;
use cupertino_portal::prompts::Prompts;
use cupertino_portal::screen_cast::ScreenCast;
use cupertino_portal::screenshot::{self, Screenshot};
use cupertino_portal::settings::{Appearance, Service};
use cupertino_portal::shortcut_keys::ShortcutKeys;
use cupertino_portal::toolkits::{self, ToolkitSettings};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_wayland_client::app::{AppEffects, AppEvent};
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
    let (keys_sender, keys_requested) = mpsc::channel();
    // without the global the portal serves no shortcuts, and the frontend asks another backend
    let mut shortcut_keys = ShortcutKeys::bind(&mut conn, keys_requested);
    if shortcut_keys.is_none() {
        eprintln!(
            "cupertino-portal: no global shortcuts: the server lacks cupertino_global_shortcuts_v1"
        );
    }
    let first = conn.wait(Some(Duration::ZERO))?;
    let mut appearance = fold(Appearance::default(), &first);
    if let Some(capture) = &mut capture {
        capture.update(&conn, &first);
    }
    let bus = zbus::blocking::Connection::session()?;
    // the consent questions are the prompts' and the choosers the shell's dialogs': asked on the session bus
    // connection, each a task of its executor
    let prompts = Prompts::new(bus.inner().clone());
    let dialogs = Dialogs::new(bus.inner().clone());
    let settings = Service::serve(&bus, appearance)?;
    let toolkits = toolkits::Writer::start(
        cupertino_defaults::config_home(),
        toolkits::written_file(),
        || std::process::Command::new("gsettings"),
    )?;
    toolkits.apply(ToolkitSettings::from(appearance));
    let files = dialogs.clone();
    bus.object_server().at(
        PORTAL_PATH,
        FileChooser::new(move |event| files.file(event)),
    )?;
    bus.object_server().at(
        PORTAL_PATH,
        AppChooser::new(
            cupertino_apps::entry::xdg_application_dirs(),
            move |event| dialogs.app(event),
        ),
    )?;
    if let Some(capture) = &capture {
        let sources = capture.sources();
        let waker = conn.waker();
        ScreenCast::new(
            move || sources.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            move |request| {
                let _ = requests.send(request);
                waker.wake();
            },
        )
        .with_applications(cupertino_apps::entry::xdg_application_dirs())
        .with_grants(ScreenSharingConf::path())
        .serve(&bus)?;
        let waker = conn.waker();
        let asking = prompts.clone();
        let screenshot = Screenshot::new(
            cupertino_apps::entry::xdg_application_dirs(),
            screenshot::pictures,
            move |event| asking.screenshot(event),
            move |shoot| {
                let _ = shoots.send(shoot);
                waker.wake();
            },
        );
        bus.object_server().at(PORTAL_PATH, screenshot)?;
    }
    let shortcuts = match shortcut_keys {
        Some(_) => {
            let waker = conn.waker();
            let asking = prompts.clone();
            let shortcuts = GlobalShortcuts::new(
                move |event| asking.shortcuts(event),
                move |keys| {
                    let _ = keys_sender.send(keys);
                    waker.wake();
                },
            )
            .with_applications(cupertino_apps::entry::xdg_application_dirs())
            .with_answers(GlobalShortcutsConf::path());
            let handle = shortcuts.handle();
            shortcuts.serve(&bus)?;
            Some(handle)
        }
        None => None,
    };
    if let (Some(keys), Some(handle)) = (&mut shortcut_keys, &shortcuts) {
        keys.update(&first, handle, &bus);
    }
    bus.request_name(BUS_NAME)?;
    loop {
        let events = conn.wait(capture.as_ref().and_then(Capture::timeout))?;
        if let Some(capture) = &mut capture {
            capture.update(&conn, &events);
        }
        if let (Some(keys), Some(handle)) = (&mut shortcut_keys, &shortcuts) {
            keys.update(&events, handle, &bus);
        }
        let next = fold(appearance, &events);
        if next != appearance {
            settings.set(next)?;
            if ToolkitSettings::from(next) != ToolkitSettings::from(appearance) {
                toolkits.apply(ToolkitSettings::from(next));
            }
            appearance = next;
        }
    }
}
