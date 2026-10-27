//! Where a capture goes: PNG files named after the local time, committed by the floating thumbnail when it is on; or
//! the clipboard, served until another selection replaces it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_geometry::{Point, Rect};
use cupertino_screenshot::filename::{self, LocalTime};
use cupertino_screenshot::image;
use cupertino_screenshot::render::{self, Painter, Palette};
use cupertino_screenshot::settings::{parse_user_dirs, Settings};
use cupertino_screenshot::thumbnail::{self, Phase, Thumbnail};
use cupertino_screenshot::values::{THUMBNAIL_MARGIN, THUMBNAIL_SHADOW_OUTSET};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerSurfaceConfig, Margin, ShellEvent,
};
use cupertino_theme::Theme;
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{Button, Event, PointerEvent};
use tiny_skia::Pixmap;

use crate::app::{Result, Screen, Shot, Wayland};
use crate::capture::Capturer;

/// Namespaces of the thumbnail and of the surface that takes the keyboard focus the clipboard needs. Neither is the
/// overlay's, so a new shortcut works while they are up.
const THUMBNAIL_NAMESPACE: &str = "cupertino-screenshot-thumbnail";
const CLIPBOARD_NAMESPACE: &str = "cupertino-screenshot-clipboard";
/// How long the server may take to focus the clipboard surface before the copy is given up.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(2);

/// The local wall-clock time now (`localtime_r`, POSIX).
fn local_now() -> Result<LocalTime> {
    // SAFETY: `time` with a null pointer only returns the time; `localtime_r` writes the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    })
}

/// The directory `settings` sends files to, created when missing.
fn directory(settings: &Settings) -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let config =
        cupertino_defaults::config_home().ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    let user_dirs = match std::fs::read_to_string(config.join("user-dirs.dirs")) {
        Ok(text) => parse_user_dirs(&text, &home),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let dir = settings
        .location
        .directory(&home, &user_dirs)
        .expect("the clipboard is not a directory");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Write every shot as a PNG under one time-stamped name, numbered when taken.
fn write_files(dir: &Path, shots: &[Shot]) -> Result<()> {
    let base = filename::base_name(&local_now()?);
    for shot in shots {
        let path = filename::free_path(dir, &base, |p| p.exists());
        let png = image::png(&shot.image)?;
        // a file that appeared since the name was chosen is not overwritten
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(&png)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// Save the shots: after the thumbnail commits when it is on, else at once.
pub fn to_files(
    wl: &mut Wayland,
    screens: &[Screen],
    shots: &[Shot],
    settings: &Settings,
    assets: &Assets,
) -> Result<()> {
    let dir = directory(settings)?;
    if !settings.show_thumbnail {
        return write_files(&dir, shots);
    }
    let first = &shots[0].image;
    let screen = (screens[0].rect.width(), screens[0].rect.height());
    let size = thumbnail::size(
        (f64::from(first.width()), f64::from(first.height())),
        screen,
    );
    let outset = THUMBNAIL_SHADOW_OUTSET;
    let surface_size = (
        (size.0 + 2.0 * outset).ceil() as u32,
        (size.1 + 2.0 * outset).ceil() as u32,
    );
    // the thumbnail's rectangle sits 16 pt inside the bottom-right corner; its surface carries the shadow
    // outset around it, so the margins are negative by that much. Anchored with an exclusive zone of 0 it stays
    // clear of the dock, which stands in for the design's move above the dock.
    let margin = (THUMBNAIL_MARGIN - outset) as i32;
    let layer = wl.layer_shell.layer_surface(
        &mut wl.conn,
        &LayerSurfaceConfig {
            namespace: THUMBNAIL_NAMESPACE.into(),
            output: Some(screens[0].id),
            layer: Layer::Overlay,
            anchor: Anchor::Bottom | Anchor::Right,
            size: surface_size,
            margin: Margin {
                right: margin,
                bottom: margin,
                ..Margin::default()
            },
            exclusive_zone: 0,
            keyboard: KeyboardInteractivity::None,
        },
    )?;
    let surface = layer.surface();
    let mut presenter = ShmPresenter::new(surface);
    let theme = Theme::load()?;
    let mut painter = Painter::new(
        Palette::from_theme(&theme, wl.appearance())?,
        theme.tokens.typography().clone(),
        assets,
    )?;
    let start = Instant::now();
    let now = || start.elapsed().as_secs_f64();
    let mut model = Thumbnail::new(size.0, now());
    let mut scaled: Option<(u32, Pixmap)> = None;
    let mut written = false;
    let image_rect = Rect::new(outset, outset, size.0, size.1);
    loop {
        loop {
            match presenter.poll(&wl.conn)? {
                Ready::Idle => break,
                // a draw follows every scale change
                Ready::ScaleChanged(_) => {}
                Ready::Draw => {
                    let phase = model.phase(now());
                    let look = match phase {
                        Phase::Shown(look) | Phase::Dismissing(look) => look,
                        Phase::Gone => continue,
                    };
                    wl.conn.set_input_region(surface, Some(&[image_rect]))?;
                    presenter.draw(&mut wl.conn, None, |pm, scale| {
                        let s = scale.factor();
                        let (w, h) = ((size.0 * s).round() as u32, (size.1 * s).round() as u32);
                        if scaled.as_ref().is_none_or(|(key, _)| *key != w) {
                            scaled = render::downscale(first, w.max(1), h.max(1)).map(|p| (w, p));
                        }
                        if let Some((_, image)) = &scaled {
                            painter.thumbnail(pm, s, image, size, look);
                        }
                    })?;
                    if matches!(phase, Phase::Dismissing(_)) || model.deadline().is_none() {
                        presenter.request_redraw();
                    }
                }
            }
        }
        let timeout = model
            .deadline()
            .map(|at| Duration::from_secs_f64((at - now()).max(0.0)));
        for event in wl.conn.wait(timeout)? {
            match event {
                Event::Frame(frame) if frame.surface == surface => presenter.frame_done(),
                Event::Pointer { surface: s, event } if s == surface => match event {
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed: true,
                        position,
                    } => model.press(position, now()),
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed: false,
                        ..
                    } => model.release(now()),
                    PointerEvent::Motion(p) => model.motion(Point::new(p.x, p.y), now()),
                    PointerEvent::Enter(_)
                    | PointerEvent::Leave
                    | PointerEvent::Button { .. }
                    | PointerEvent::Scroll { .. } => {}
                },
                Event::Extension(event) => match wl.extension(&event) {
                    Some(&ShellEvent::Configure { surface: s, size }) if s == surface => {
                        presenter.configured(layer.configured(size)?)
                    }
                    Some(&ShellEvent::Closed(s)) if s == surface => {
                        return Err("the server closed the thumbnail".into())
                    }
                    _ => {}
                },
                // the thumbnail takes no keys and has no popups or roles; its look is fixed for its few seconds
                _ => {}
            }
        }
        model.tick(now());
        if !written && model.committed().is_some() {
            write_files(&dir, shots)?;
            written = true;
        }
        match model.phase(now()) {
            Phase::Gone => {
                wl.conn.destroy(surface)?;
                return Ok(());
            }
            Phase::Dismissing(_) => presenter.request_redraw(),
            Phase::Shown(_) => {}
        }
    }
}

/// Offer the shot on the clipboard and serve it until it is replaced. The server takes a selection only from a client
/// with the keyboard focus, so a transparent 1 x 1 surface takes it for the moment of the offer.
pub fn to_clipboard(wl: &mut Wayland, capturer: &mut Capturer, shot: &Shot) -> Result<()> {
    let png = image::png(&shot.image)?;
    let before = wl.conn.input_serial();
    let layer = wl.layer_shell.layer_surface(
        &mut wl.conn,
        &LayerSurfaceConfig {
            namespace: CLIPBOARD_NAMESPACE.into(),
            output: None,
            layer: Layer::Overlay,
            anchor: Anchor::Top | Anchor::Left,
            size: (1, 1),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::Exclusive,
        },
    )?;
    let surface = layer.surface();
    let mut presenter = ShmPresenter::new(surface);
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    // the keyboard enter that follows the first commit brings a new serial
    while wl.conn.input_serial() == before || wl.conn.input_serial().is_none() {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("the server gave the clipboard surface no keyboard focus")?;
        for event in wl.conn.wait(Some(left))? {
            if let Event::Extension(event) = event {
                if let Some(&ShellEvent::Configure { surface: s, size }) = wl.extension(&event) {
                    if s == surface {
                        presenter.configured(layer.configured(size)?);
                    }
                }
            }
        }
        if presenter.poll(&wl.conn)? == Ready::Draw {
            wl.conn.set_input_region(surface, Some(&[]))?;
            presenter.draw(&mut wl.conn, None, |_, _| {})?;
        }
        capturer.dispatch()?;
    }
    capturer.offer_clipboard(png, wl.conn.input_serial().expect("checked above"))?;
    wl.conn.destroy(surface)?;
    capturer.serve_clipboard()
}
