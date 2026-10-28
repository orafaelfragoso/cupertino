//! The independent About window on the public client transport.

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use cupertino_assets::Assets;
use cupertino_defaults::keyboard::{KeyboardSettings, ModifierProfile};
use cupertino_protocol_app::{AppEffects, AppEvent};
use cupertino_settings::about::{About, Row};
use cupertino_settings::instance;
use cupertino_settings::paint::{Painter, Palette};
use cupertino_theme::{Appearance, Theme};
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Connection, Decorations, Event, KeyboardEvent, PointerEvent, ToplevelConfig,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn run() -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let waker = std::sync::Arc::new(std::sync::OnceLock::<cupertino_wayland_client::Waker>::new());
    let wake = waker.clone();
    let _instance =
        match instance::serve_about(zbus::connection::Builder::session()?, move |token| {
            // A pending activation already represents all repeat launches; never queue an unbounded number of calls.
            if sender.try_send(token).is_ok() {
                if let Some(waker) = wake.get() {
                    waker.wake();
                }
            }
        }) {
            Ok(connection) => connection,
            Err(zbus::Error::NameTaken) => {
                let token = match std::env::var("XDG_ACTIVATION_TOKEN") {
                    Ok(token) => token,
                    Err(std::env::VarError::NotPresent) => String::new(),
                    Err(e) => return Err(e.into()),
                };
                return Ok(instance::show_about(
                    zbus::connection::Builder::session()?,
                    &token,
                )?);
            }
            Err(e) => return Err(e.into()),
        };
    let mut about = About::read(Path::new("/"));
    let theme = Theme::load()?;
    let mut painter = Painter::new(&Assets::discover())?;
    let mut conn = Connection::connect()?;
    assert!(waker.set(conn.waker()).is_ok(), "the waker is set once");
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let _appearance = effects.appearance(&conn);
    conn.roundtrip()?;
    let mut events = conn.wait(Some(Duration::ZERO))?;
    let mut dark = false;
    let mut contrast = false;
    for output in conn.outputs() {
        if let Some((w, h)) = output.mode.filter(|(w, h)| *w > 0 && *h > 0) {
            let description = output.description.or(output.name);
            if let Some(description) = description.filter(|s| !s.trim().is_empty()) {
                let value = format!("{description} {w} × {h} @ {}x", output.scale.factor());
                let before_kernel = about
                    .rows
                    .iter()
                    .position(|r| r.label == "Kernel")
                    .unwrap_or(about.rows.len());
                about.rows.insert(
                    before_kernel,
                    Row {
                        label: "Displays",
                        value,
                    },
                );
            }
        }
    }
    let size = about.size();
    let window = conn.toplevel(&ToplevelConfig {
        app_id: "cupertino-settings".into(),
        title: "About This Computer".into(),
        min_size: Some(size),
        max_size: Some(size),
        decorations: Decorations::Server,
    })?;
    let mut presenter = ShmPresenter::new(window.surface());
    let mut pressed = false;
    loop {
        for event in events {
            match event {
                Event::Configure { configure, .. } => {
                    if configure.decorations != Decorations::Server {
                        return Err("the server refused server-side decorations".into());
                    }
                    presenter.configured(size);
                }
                Event::Frame(_) => presenter.frame_done(),
                Event::Close(_) => return Ok(()),
                Event::Keyboard {
                    event:
                        KeyboardEvent::Key {
                            code,
                            pressed: true,
                            mods,
                            repeat,
                            ..
                        },
                    ..
                } => {
                    if code == key::ESCAPE {
                        return Ok(());
                    }
                    let primary = primary_is_ctrl()?;
                    let command = if primary {
                        mods.ctrl && !mods.logo
                    } else {
                        mods.logo && !mods.ctrl
                    };
                    if code == 17 && command && !mods.alt && !mods.shift {
                        return Ok(());
                    }
                    if !repeat
                        && !mods.ctrl
                        && !mods.logo
                        && !mods.alt
                        && matches!(code, key::ENTER | key::KEYPAD_ENTER | key::SPACE)
                    {
                        more_info()?;
                    }
                }
                Event::Pointer {
                    event:
                        PointerEvent::Button {
                            button: Button::Left,
                            pressed: down,
                            position,
                        },
                    ..
                } => {
                    let inside = about.button().contains(cupertino_settings::Point {
                        x: position.x,
                        y: position.y,
                    });
                    let activate = !down && pressed && inside;
                    pressed = down && inside;
                    presenter.request_redraw();
                    if activate {
                        more_info()?;
                    }
                }
                Event::Pointer {
                    event: PointerEvent::Leave,
                    ..
                } => {
                    pressed = false;
                    presenter.request_redraw();
                }
                Event::Extension(event) => match event.downcast_ref::<AppEvent>() {
                    Some(AppEvent::Appearance { dark: value }) => {
                        dark = *value;
                        presenter.request_redraw();
                    }
                    Some(AppEvent::DisplayOptions(options)) => {
                        contrast = options.increase_contrast;
                        presenter.request_redraw();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        while let Ok(token) = receiver.try_recv() {
            if token.is_empty() {
                conn.activate(window)?;
            } else {
                conn.activate_with_token(window, &token)?;
            }
        }
        let appearance = match (dark, contrast) {
            (false, false) => Appearance::Light,
            (true, false) => Appearance::Dark,
            (false, true) => Appearance::LightHighContrast,
            (true, true) => Appearance::DarkHighContrast,
        };
        let palette = Palette::from_theme(&theme, appearance)?;
        loop {
            match presenter.poll(&conn)? {
                Ready::ScaleChanged(_) => {}
                Ready::Idle => break,
                Ready::Draw => presenter.draw(&mut conn, None, |pm, scale| {
                    painter.paint_about(pm, scale.factor() as f32, &about, &palette)
                })?,
            }
        }
        events = conn.wait(None)?;
    }
}

/// Read the command-key choice on input so changing profiles also updates the close chord live.
fn primary_is_ctrl() -> Result<bool> {
    let Some(path) = KeyboardSettings::path() else {
        return Ok(false);
    };
    let (settings, problems) = KeyboardSettings::load(&path, None)?;
    if !problems.is_empty() {
        return Err(format!("invalid keyboard settings: {problems:?}").into());
    }
    Ok(settings.modifier_profile == Some(ModifierProfile::ClassicPc))
}

fn more_info() -> Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?).spawn()?;
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("cupertino-settings: More Info exited with {status}"),
        Err(e) => eprintln!("cupertino-settings: cannot wait for More Info: {e}"),
    });
    Ok(())
}
