//! The desktop items drawn to PNGs for look checks:
//! `cargo run -p cupertino-shell-desktop --example desktop_capture -- desktop/items [--state select|rename|drag]`
//! (crates/cupertino-kit/README.md).
//!
//! One surface, `items`: the desktop's items (640 × 400 at 1×) over a flat fill where the wallpaper would be, the
//! component's window ([`cupertino_shell_desktop::Desktop`]) drawn on the headless window system with the CPU renderer.
//! `--state` acts on `notes.txt`: `select` clicks it, `rename` clicks it and presses Return, so its name is edited,
//! and `drag` presses it and holds it [`DRAG`] away. The Desktop folder is made under the temporary directory
//! ([`ROOT`]) with a picture, a text file, a PDF and a folder; file types come from the system's MIME database, the
//! icons from the asset packs and the icon theme, and the picture shows its thumbnail, made in a cache under [`ROOT`].

#![expect(
    clippy::disallowed_methods,
    reason = "a capture runs alone: no UI loop waits on its files or its theme"
)]

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use cupertino_kit::core::RuntimeOptions;
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::platform::{
    KeyEvent, Keysym, Mods, MouseButton, PointerEvent, SystemAppearance, Timestamp, WindowId,
};
use cupertino_kit::render::{ImageResource, PaintOp, PixelFormat};
use cupertino_kit::test::capture::{self, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowEvent, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_shell_desktop::Desktop;

/// The folder under the temporary directory that holds the sample Desktop folder and the thumbnail cache.
const ROOT: &str = "cupertino-shell-desktop-capture";

/// The desktop's output, in points: the size of its one window.
const SCREEN: (f64, f64) = (640.0, 400.0);
/// What the desktop's items are drawn over where the wallpaper would be: a mid grey.
const FILL: [u8; 3] = [96, 112, 128];
/// The item the states act on.
const ITEM: &str = "notes.txt";
/// How far a held press on the item moves before the capture of `--state drag`, in points.
const DRAG: (f64, f64) = (-220.0, -60.0);
/// The sample picture, its size in pixels; the text file and the PDF; and the folder.
const PICTURE: &str = "IMG_0042.png";
const PICTURE_SIZE: (u32, u32) = (256, 176);
const TEXT: &str = "Plan for the week\n\n- Review the layout of the new folder views\n";
const PDF: (&str, u64) = ("Résumé 2026.pdf", 184_000);
const FOLDER: &str = "Drafts";
/// How long the listing and the thumbnail may take.
const LISTING: Duration = Duration::from_secs(5);
const THUMBNAILS: Duration = Duration::from_secs(20);
/// The work the runtime may do between two looks at the window.
const TURNS: u32 = 10_000;
/// The frames a window may ask for without resting or showing a steady state before the capture gives up: a guard
/// against a hang only.
const HANG_FRAMES: usize = 10_000;
/// The states `--state` takes.
const STATES: [&str; 3] = ["select", "rename", "drag"];
/// The Return key's keysym.
const RETURN: u32 = 0xff0d;

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// The sample picture: a sky over hills.
fn picture() -> Result<Vec<u8>, String> {
    let (w, h) = PICTURE_SIZE;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let hill = 0.62 + 0.12 * (fx * 6.0).sin();
            let [r, g, b] = if fy > hill {
                [0.25, 0.55 - 0.2 * fy, 0.25]
            } else {
                [0.45 + 0.4 * fy, 0.6 + 0.3 * fy, 0.95]
            };
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[c(r), c(g), c(b), 255]);
        }
    }
    let image = ImageResource {
        revision: 0,
        width: w,
        height: h,
        stride: w * 4,
        format: PixelFormat::Rgba8,
        pixels: pixels.into(),
    };
    cupertino_kit::image::encode_png(&image).map_err(|e| format!("{e:?}"))
}

/// The sample Desktop folder under `root`, its items written again in place.
fn desktop(root: &Path) -> Result<PathBuf, String> {
    let folder = root.join("Desktop");
    std::fs::create_dir_all(folder.join(FOLDER)).map_err(io)?;
    std::fs::write(folder.join(PICTURE), picture()?).map_err(io)?;
    std::fs::write(folder.join(ITEM), TEXT).map_err(io)?;
    std::fs::File::create(folder.join(PDF.0))
        .and_then(|f| f.set_len(PDF.1))
        .map_err(io)?;
    Ok(folder)
}

/// The options of a driver drawing in `look`: the Cupertino theme in that appearance.
fn options(look: Look) -> Result<RuntimeOptions, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: false,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    options.theme =
        Arc::new(ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?);
    Ok(options)
}

/// Run the window and present the frames it asks for until it rests, or presents the same pixels twice in a row (the
/// caret of a name being edited, at the capture's fixed clock); `false` when it does neither within [`HANG_FRAMES`].
fn settle(
    driver: &mut TestDriver<Desktop>,
    window: WindowId,
    turns: NonZeroU32,
) -> Result<bool, String> {
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let presented = |driver: &mut TestDriver<Desktop>| {
        driver
            .window_system_mut()
            .frames(window)
            .map(|frames| frames.len())
            .map_err(|e| fail(&e))
    };
    let before = presented(driver)?;
    let mut given = None;
    for _ in 0..HANG_FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let now = presented(driver)?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            return Ok(true);
        }
        let frames = system.frames(window).map_err(|e| fail(&e))?;
        if given == Some(now)
            || (now >= before + 2 && matches!(frames, [.., a, b] if a.pixels == b.pixels))
        {
            return Ok(true);
        }
        given = Some(now);
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    Ok(false)
}

/// Settle the window until `done` holds of it, waiting for a task on another thread (the listing, a thumbnail) to wake
/// the runtime between two looks; `guard` only guards against a hang.
fn wait_until(
    driver: &mut TestDriver<Desktop>,
    window: WindowId,
    turns: NonZeroU32,
    guard: Duration,
    what: &str,
    done: impl Fn(&TestDriver<Desktop>) -> bool,
) -> Result<(), String> {
    loop {
        settle(driver, window, turns)?;
        if done(driver) {
            return Ok(());
        }
        if !driver.window_system_mut().wait_for_wake(guard) {
            return Err(format!("{what} never came"));
        }
    }
}

/// The middle of the node named `name` in the window.
fn named_center(driver: &TestDriver<Desktop>, window: WindowId, name: &str) -> Option<Point> {
    let snapshot = driver.runtime().snapshot(window)?;
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds.center())
}

/// How many thumbnails the window draws: the images that are not square.
fn thumbnails_drawn(driver: &TestDriver<Desktop>, window: WindowId) -> usize {
    let Some(snapshot) = driver.runtime().snapshot(window) else {
        return 0;
    };
    snapshot
        .display_list
        .nodes
        .iter()
        .flat_map(|n| &n.ops)
        .filter(|op| match op {
            PaintOp::Image { dst, .. } => (dst.size.width - dst.size.height).abs() > 0.5,
            _ => false,
        })
        .count()
}

/// The pointer's `events` and the keys of `keys` in the window, each run to idle.
fn send(
    driver: &mut TestDriver<Desktop>,
    window: WindowId,
    events: Vec<WindowEvent>,
    turns: NonZeroU32,
) -> Result<(), String> {
    for event in events {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .map_err(|e| format!("the headless window system: {e:?}"))?;
        driver
            .run_until_idle(turns)
            .map_err(|e| format!("the headless window system: {e:?}"))?;
    }
    Ok(())
}

fn pointer(event: PointerEvent) -> WindowEvent {
    WindowEvent::Pointer(event, Timestamp(0))
}

/// A press or a release of the left button at `at`.
fn button(at: Point, pressed: bool) -> WindowEvent {
    pointer(PointerEvent::Button {
        position: at,
        button: MouseButton::Left,
        pressed,
        serial: 1,
        mods: Mods::NONE,
    })
}

/// The key of `keysym` pressed and let go.
fn key(keysym: u32) -> [WindowEvent; 2] {
    [true, false].map(|pressed| {
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        )
    })
}

/// What `state` does to the item at `at`.
fn acts(state: &str, at: Point) -> Vec<WindowEvent> {
    let click = vec![
        pointer(PointerEvent::Motion { position: at }),
        button(at, true),
        button(at, false),
    ];
    match state {
        "select" => click,
        "rename" => click.into_iter().chain(key(RETURN)).collect(),
        _ => {
            let to = Point::new(at.x + DRAG.0, at.y + DRAG.1);
            vec![
                pointer(PointerEvent::Motion { position: at }),
                button(at, true),
                pointer(PointerEvent::Motion { position: to }),
            ]
        }
    }
}

/// `over`, premultiplied, drawn over [`FILL`].
fn over_fill(mut picture: Pixmap) -> Pixmap {
    for px in picture.data_mut().as_chunks_mut::<4>().0 {
        let rest = 255 - u16::from(px[3]);
        for (c, fill) in px[..3].iter_mut().zip(FILL) {
            *c = (u16::from(*c) + (u16::from(fill) * rest).div_ceil(256)) as u8;
        }
        px[3] = 255;
    }
    picture
}

struct Capture {
    root: PathBuf,
    folder: PathBuf,
}

impl Capture {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(ROOT);
        let folder = desktop(&root)?;
        Ok(Capture { root, folder })
    }

    /// The items, drawn in `look` at `scale` over [`FILL`], after `state` acted on [`ITEM`].
    fn items(&self, look: Look, scale: f32, state: Option<&str>) -> Result<Pixmap, String> {
        let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
        let dirs = cupertino_apps::XdgDirs::from_env().map_err(|e| e.to_string())?;
        let types =
            cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
        let icons = FileIcons::discover(types.clone(), Default::default());
        cupertino_shell_desktop::prepare(
            self.folder.clone(),
            Size::new(SCREEN.0, SCREEN.1),
            Arc::new(types),
            icons,
            cupertino_icons::thumbnail::Thumbnailer::new(
                self.root.join("cache"),
                "cupertino-files",
            ),
        );
        let turns = NonZeroU32::new(TURNS).expect("a turn budget");
        let mut driver = TestDriver::<Desktop>::new(
            Capabilities::default(),
            options(look)?,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let window = driver.runtime().windows()[0].1;
        let system = driver.window_system_mut();
        system
            .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
            .map_err(|e| fail(&e))?;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        system
            .configure(window, Some(Size::new(SCREEN.0, SCREEN.1)), active)
            .map_err(|e| fail(&e))?;
        wait_until(
            &mut driver,
            window,
            turns,
            LISTING,
            "the listing",
            |driver| {
                [PICTURE, ITEM, PDF.0, FOLDER]
                    .iter()
                    .all(|name| named_center(driver, window, name).is_some())
            },
        )?;
        wait_until(
            &mut driver,
            window,
            turns,
            THUMBNAILS,
            "the thumbnail",
            |driver| thumbnails_drawn(driver, window) >= 1,
        )?;
        settle(&mut driver, window, turns)?;
        if let Some(state) = state {
            let at = named_center(&driver, window, ITEM).ok_or("the desktop shows no notes.txt")?;
            send(&mut driver, window, acts(state, at), turns)?;
        }
        if !settle(&mut driver, window, turns)? {
            return Err("the desktop keeps asking for frames".into());
        }
        capture::presented(driver.window_system_mut(), window).map(over_fill)
    }
}

impl App for Capture {
    const NAME: &'static str = "desktop";
    const STATE_FLAGS: &'static [&'static str] = &["state"];

    fn surfaces(&self) -> Vec<String> {
        vec!["items".into()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        std::iter::once(State::new())
            .chain(
                STATES
                    .into_iter()
                    .map(|value| vec![("state".to_string(), value.to_string())]),
            )
            .collect()
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        if r.surface != "items" {
            return Err(format!("{}: no such surface", r.surface));
        }
        match r.state.first() {
            None => self.items(r.look, r.scale, None),
            Some((_, value)) if STATES.contains(&value.as_str()) => {
                self.items(r.look, r.scale, Some(value))
            }
            Some((flag, value)) => Err(format!("--{flag} {value}: select, rename or drag")),
        }
    }
}

fn main() -> ExitCode {
    capture::run(Capture::new)
}
