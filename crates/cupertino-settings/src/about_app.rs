//! The independent About window on the public client transport.

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use cupertino_assets::Assets;
use cupertino_defaults::keyboard::{KeyboardSettings, ModifierProfile};
use cupertino_protocol_app::{AppEffects, AppEvent};
use cupertino_settings::about::{display_value, About, Display, Row};
use cupertino_settings::instance;
use cupertino_settings::paint::{Painter, Palette};
use cupertino_theme::{Appearance, Theme};
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Capability, Connection, Decorations, Event, KeyboardEvent, PointerEvent,
    SurfaceId, TokenRequest, ToplevelConfig,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn run(activation_token: String) -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let waker = std::sync::Arc::new(std::sync::OnceLock::<cupertino_wayland_client::Waker>::new());
    let wake = waker.clone();
    let on_show = move |token| {
        // A pending activation already represents all repeat launches; never queue an unbounded number of calls.
        if sender.try_send(token).is_ok() {
            if let Some(waker) = wake.get() {
                waker.wake();
            }
        }
    };
    let Some(_instance) = instance::serve_or_hand_over(
        || instance::serve_about(zbus::connection::Builder::session()?, on_show.clone()),
        || instance::show_about(zbus::connection::Builder::session()?, &activation_token),
    )?
    else {
        return Ok(());
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
        let Some(mode) = output.mode else { continue };
        let display = Display {
            make: output.make.as_deref(),
            model: output.model.as_deref(),
            description: output.description.as_deref(),
            name: output.name.as_deref(),
            mode,
            physical_size_mm: output.physical_size_mm,
            scale: output
                .effective_scale()
                .unwrap_or_else(|| output.scale.factor()),
        };
        if let Some(value) = display_value(&display) {
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
    let layout = painter.about_layout(&about);
    let size = layout.size();
    let window = conn.toplevel(&ToplevelConfig {
        app_id: "cupertino-settings".into(),
        title: "About This Computer".into(),
        min_size: Some(size),
        max_size: Some(size),
        decorations: Decorations::Server,
        foreign_parent: None,
    })?;
    let mut presenter = ShmPresenter::new(window.surface());
    if !activation_token.is_empty() {
        unless_unsupported(conn.activate_with_token(window, &activation_token))?;
    }
    let mut pressed = false;
    let mut info_request = None;
    let unraised_info = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                        info_request =
                            more_info_request(&mut conn, window.surface(), &unraised_info)?;
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
                    let inside = layout.button.contains(cupertino_settings::Point {
                        x: position.x,
                        y: position.y,
                    });
                    let activate = !down && pressed && inside;
                    pressed = down && inside;
                    presenter.request_redraw();
                    if activate && info_request.is_none() {
                        info_request =
                            more_info_request(&mut conn, window.surface(), &unraised_info)?;
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
                    more_info(&token, || {})?;
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
            unless_unsupported(if token.is_empty() {
                conn.activate(window)
            } else {
                conn.activate_with_token(window, &token)
            })?;
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
                    painter.paint_about(pm, scale.factor() as f32, &about, &layout, &palette)
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

/// Ask for the token that raises More Info. Without activation, More Info opens at once, unraised, and only when the one
/// opened that way has ended (`open`), so a held key or repeated clicks start one process.
fn more_info_request(
    conn: &mut Connection,
    surface: SurfaceId,
    open: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Option<TokenRequest>> {
    use std::sync::atomic::Ordering::SeqCst;
    let request =
        unless_unsupported(conn.request_activation_token(surface, Some("cupertino-settings")))?;
    if request.is_none() && !open.swap(true, SeqCst) {
        let ended = open.clone();
        if let Err(e) = more_info("", move || ended.store(false, SeqCst)) {
            open.store(false, SeqCst);
            return Err(e);
        }
    }
    Ok(request)
}

/// Start More Info; `ended` runs when its process exits.
fn more_info(token: &str, ended: impl FnOnce() + Send + 'static) -> Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .env("XDG_ACTIVATION_TOKEN", token)
        .spawn()?;
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("cupertino-settings: More Info exited with {status}"),
            Err(e) => eprintln!("cupertino-settings: cannot wait for More Info: {e}"),
        }
        ended();
    });
    Ok(())
}

/// The outcome of an activation request, `None` when the compositor has no activation protocol: the window then shows
/// without being raised.
fn unless_unsupported<T>(
    result: std::result::Result<T, cupertino_wayland_client::Error>,
) -> std::result::Result<Option<T>, cupertino_wayland_client::Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(cupertino_wayland_client::Error::Unsupported(Capability::Activation)) => Ok(None),
        Err(e) => Err(e),
    }
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

    #[test]
    fn only_a_missing_activation_protocol_skips_the_raise() {
        use cupertino_wayland_client::Error;
        assert!(matches!(unless_unsupported(Ok(7)), Ok(Some(7))));
        assert!(matches!(
            unless_unsupported::<()>(Err(Error::Unsupported(Capability::Activation))),
            Ok(None)
        ));
        assert!(matches!(
            unless_unsupported::<()>(Err(Error::Unsupported(Capability::Clipboard))),
            Err(Error::Unsupported(Capability::Clipboard))
        ));
        assert!(matches!(
            unless_unsupported::<()>(Err(Error::Protocol("bad token"))),
            Err(Error::Protocol(_))
        ));
    }
}
