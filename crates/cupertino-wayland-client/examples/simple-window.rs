//! A window drawn through shm on any compositor with `xdg_wm_base`. It follows configures, scale changes and pointer
//! and key input; with client-side decorations it draws a title bar to move the window by and a border to resize it
//! from.
//!
//! `simple-window [--frames N]`: with `--frames` it animates and exits after N presented frames, printing what it
//! showed on the last line (`frames N size WxH scale S decorations D`). Escape or the close request ends it too.

use cupertino_wayland_client::{
    BufferFormat, Button, Connection, Decorations, Error, Event, KeyboardEvent, PointerEvent,
    ResizeEdge, Toplevel, ToplevelConfig, WindowConfigure,
};
use std::process::ExitCode;

/// Size of the window when the compositor leaves it to the client, in logical points.
const PREFERRED_SIZE: (u32, u32) = (320, 240);
/// Smallest size the window takes, in logical points.
const MIN_SIZE: (u32, u32) = (160, 120);
/// Height of the client-drawn title bar, in logical points.
const TITLE_BAR: f64 = 24.0;
/// Width of the client-drawn resize border, in logical points.
const BORDER: f64 = 6.0;
/// Evdev code of the Escape key.
const KEY_ESC: u32 = 1;

const BACKGROUND: [u8; 4] = [0xee, 0xee, 0xee, 0xff];
const TITLE_BAR_COLOR: [u8; 4] = [0xc8, 0xc8, 0xc8, 0xff];
const POINTER_COLOR: [u8; 4] = [0x30, 0x60, 0xd0, 0xff];
const STRIPE_COLOR: [u8; 4] = [0x50, 0xa0, 0x50, 0xff];

struct Window {
    toplevel: Toplevel,
    configure: Option<WindowConfigure>,
    pointer: Option<(f64, f64)>,
    frames_left: Option<u32>,
    presented: u32,
    frame_pending: bool,
    dirty: bool,
    shown: Option<((u32, u32), f64, Decorations)>,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let frames = match (
        args.next().as_deref(),
        args.next().map(|n| n.parse::<u32>()),
    ) {
        (None, _) => None,
        (Some("--frames"), Some(Ok(n))) if n > 0 => Some(n),
        _ => {
            eprintln!("usage: simple-window [--frames N]");
            return ExitCode::from(2);
        }
    };
    match run(frames) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("simple-window: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(frames: Option<u32>) -> Result<(), Error> {
    let mut conn = Connection::connect()?;
    eprintln!(
        "simple-window: capabilities {:?}",
        conn.capabilities().iter().collect::<Vec<_>>()
    );
    let toplevel = conn.toplevel(&ToplevelConfig {
        app_id: "simple-window".into(),
        title: "Simple window".into(),
        min_size: Some(MIN_SIZE),
        max_size: None,
        decorations: Decorations::Server,
    })?;
    let mut w = Window {
        toplevel,
        configure: None,
        pointer: None,
        frames_left: frames,
        presented: 0,
        frame_pending: false,
        dirty: false,
        shown: None,
    };
    loop {
        for event in conn.wait(None)? {
            if !handle(&mut conn, &mut w, event)? {
                return Ok(());
            }
        }
        if w.dirty && !w.frame_pending && w.configure.is_some() {
            draw(&mut conn, &mut w)?;
        }
    }
}

/// React to one event; `false` ends the program.
fn handle(conn: &mut Connection, w: &mut Window, event: Event) -> Result<bool, Error> {
    match event {
        Event::Configure { configure, .. } => {
            w.configure = Some(configure);
            w.dirty = true;
        }
        Event::ScaleChanged { .. } => w.dirty = true,
        Event::Frame(_) => {
            w.frame_pending = false;
            w.presented += 1;
            if let Some(left) = w.frames_left.as_mut() {
                *left -= 1;
                if *left == 0 {
                    report(w);
                    return Ok(false);
                }
                w.dirty = true;
            }
        }
        Event::Close(_) => return Ok(false),
        Event::Keyboard {
            event:
                KeyboardEvent::Key {
                    code: KEY_ESC,
                    pressed: true,
                    ..
                },
            ..
        } => return Ok(false),
        Event::Pointer { event, .. } => pointer(conn, w, event)?,
        _ => {}
    }
    Ok(true)
}

fn pointer(conn: &mut Connection, w: &mut Window, event: PointerEvent) -> Result<(), Error> {
    match event {
        PointerEvent::Enter(p) | PointerEvent::Motion(p) => w.pointer = Some((p.x, p.y)),
        PointerEvent::Leave => w.pointer = None,
        PointerEvent::Button {
            button: Button::Left,
            pressed: true,
            position,
        } if decorations(w) == Decorations::Client => {
            let size = shown_size(w);
            match resize_edge(position.x, position.y, size) {
                Some(edge) => conn.start_resize(w.toplevel, edge)?,
                None if position.y < TITLE_BAR => conn.start_move(w.toplevel)?,
                None => {}
            }
        }
        _ => {}
    }
    w.dirty = true;
    Ok(())
}

fn decorations(w: &Window) -> Decorations {
    w.configure.map_or(Decorations::Client, |c| c.decorations)
}

fn shown_size(w: &Window) -> (f64, f64) {
    let (width, height) = w.shown.map_or(PREFERRED_SIZE, |(s, _, _)| s);
    (f64::from(width), f64::from(height))
}

/// The border edge under a point of a window of `size`, if any.
fn resize_edge(x: f64, y: f64, (width, height): (f64, f64)) -> Option<ResizeEdge> {
    let (left, right) = (x < BORDER, x >= width - BORDER);
    let (top, bottom) = (y < BORDER, y >= height - BORDER);
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (_, true, true, _) => ResizeEdge::TopRight,
        (true, _, _, true) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (true, ..) => ResizeEdge::Left,
        (_, true, ..) => ResizeEdge::Right,
        (_, _, true, _) => ResizeEdge::Top,
        (_, _, _, true) => ResizeEdge::Bottom,
        _ => return None,
    })
}

fn draw(conn: &mut Connection, w: &mut Window) -> Result<(), Error> {
    let decorations = decorations(w);
    let mut lease = match conn.lease(w.toplevel.surface(), PREFERRED_SIZE, BufferFormat::Xrgb8888) {
        Ok(lease) => lease,
        // the compositor still holds the buffers: the next frame callback brings one back
        Err(Error::NoFreeBuffer) => return Ok(()),
        Err(e) => return Err(e),
    };
    let layout = lease.layout();
    let factor = lease.scale().factor();
    let logical = lease.logical_size();
    let phase = w.presented;
    let pointer = w.pointer;
    let width = layout.width() as usize;
    let title_bar = if decorations == Decorations::Client {
        (TITLE_BAR * factor).round() as usize
    } else {
        0
    };
    let stripe = phase as usize * 4 % width.max(1);
    let pixels = lease.pixels_mut();
    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let near_pointer = pointer.is_some_and(|(px_x, px_y)| {
                let (dx, dy) = (x as f64 / factor - px_x, y as f64 / factor - px_y);
                dx * dx + dy * dy < 36.0
            });
            let rgba = if near_pointer {
                POINTER_COLOR
            } else if y < title_bar {
                TITLE_BAR_COLOR
            } else if x.abs_diff(stripe) < (4.0 * factor) as usize {
                STRIPE_COLOR
            } else {
                BACKGROUND
            };
            *px = [rgba[2], rgba[1], rgba[0], rgba[3]];
        }
    }
    lease.present()?;
    w.frame_pending = true;
    w.dirty = false;
    w.shown = Some((logical, factor, decorations));
    Ok(())
}

fn report(w: &Window) {
    let Some(((width, height), scale, decorations)) = w.shown else {
        return;
    };
    let decorations = match decorations {
        Decorations::Server => "server",
        Decorations::Client => "client",
    };
    println!(
        "frames {} size {width}x{height} scale {scale} decorations {decorations}",
        w.presented
    );
}
