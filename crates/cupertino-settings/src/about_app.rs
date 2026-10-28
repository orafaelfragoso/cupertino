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

pub fn run(activation_token: String) -> Result<()> {
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
                return Ok(instance::show_about(
                    zbus::connection::Builder::session()?,
                    &activation_token,
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
    if !activation_token.is_empty() {
        conn.activate_with_token(window, &activation_token)?;
    }
    let mut pressed = false;
    let mut info_request = None;
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
                    let path = KeyboardSettings::path();
                    let primary = primary_is_ctrl(path.as_deref());
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
                        && info_request.is_none()
                    {
                        info_request = Some(conn.request_activation_token(
                            window.surface(),
                            Some("cupertino-settings"),
                        )?);
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
                    if activate && info_request.is_none() {
                        info_request = Some(conn.request_activation_token(
                            window.surface(),
                            Some("cupertino-settings"),
                        )?);
                    }
                }
                Event::Pointer {
                    event: PointerEvent::Leave,
                    ..
                } => {
                    pressed = false;
                    presenter.request_redraw();
                }
                Event::ActivationToken { request, token } if info_request == Some(request) => {
                    info_request = None;
                    more_info(&token)?;
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
fn primary_is_ctrl(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return false;
    };
    match KeyboardSettings::load(path, None) {
        Ok((settings, problems)) if problems.is_empty() => {
            settings.modifier_profile == Some(ModifierProfile::ClassicPc)
        }
        result => {
            eprintln!("cupertino-settings: cannot read command-key choice from {}: {result:?}; using Super", path.display());
            false
        }
    }
}

fn more_info(token: &str) -> Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .env("XDG_ACTIVATION_TOKEN", token)
        .spawn()?;
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("cupertino-settings: More Info exited with {status}"),
        Err(e) => eprintln!("cupertino-settings: cannot wait for More Info: {e}"),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_keyboard_settings_keep_about_usable_with_the_standard_command_key() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("keyboard.conf");
        assert!(!primary_is_ctrl(Some(&path)));
        std::fs::write(&path, "modifier_profile = classic-pc\n").unwrap();
        assert!(primary_is_ctrl(Some(&path)));
        std::fs::write(&path, "modifier_profile = invalid\n").unwrap();
        assert!(!primary_is_ctrl(Some(&path)));
        assert!(!primary_is_ctrl(None));
    }
}
