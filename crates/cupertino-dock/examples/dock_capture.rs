//! The dock drawn to PNGs for look checks:
//! `cargo run -p cupertino-dock --example dock_capture -- dock/SURFACE [flags]` (crates/cupertino-kit/README.md).
//!
//! The surfaces drive the dock's model ([`cupertino_dock::kit::Model`]) with a fixture host and draw its windows as
//! the component does: `bar` (the pinned applications, the file manager and the trash, nothing running), `running`
//! (three of them running, with their indicators), `badges` (the running ones showing counts, one past 9999),
//! `magnified` (magnification on, the pointer over the middle tile), `switcher` (the application switcher's panel,
//! the second entry selected) and `force-quit` (the Force Quit dialog, one application not responding). Each picture
//! is the window over a plain wallpaper. The applications and their icons are fixtures (the asset packs' folder and
//! document icons stand in for applications' own), the clock is fixed and every motion has settled, so a command gives
//! the same pixels on every run.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use cupertino_assets::Assets;
use cupertino_defaults::dock::DockSettings;
use cupertino_dock::actions::ActionMap;
use cupertino_dock::badge::BadgeUpdate;
use cupertino_dock::conf::{dock_config, FILE_MANAGER};
use cupertino_dock::desktop::{parse_entry, DesktopIndex};
use cupertino_dock::dock::Dock;
use cupertino_dock::kit::{
    self, DockEvent, Host, IconKey, Message, Model, OutputInfo, Request, Started, WindowIcon,
    ICON_SCALES,
};
use cupertino_dock::session::Session;
use cupertino_dock::view::{
    IconSet, Input, Pointer, BAR_APP_ID, FORCE_QUIT_APP_ID, SWITCHER_APP_ID,
};
use cupertino_icons::SystemIcons;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Application, Command, Window, WindowKey};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::test::capture::{self, App, Look, Pixmap, Request as Capture, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowEvent, WindowStates};
use cupertino_kit::theme::ThemeTokens;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 16;

const SURFACES: [&str; 6] = [
    "bar",
    "running",
    "badges",
    "magnified",
    "switcher",
    "force-quit",
];

/// The output the dock is on: its registry name and its size in points.
const OUTPUT: OutputInfo = OutputInfo {
    key: 1,
    width: 1440.0,
    height: 900.0,
    scale: 1.0,
};

/// The fixture's applications: desktop file ID, name, and the asset packs' icon that stands in for its own.
const APPS: [(&str, &str, &str); 7] = [
    (FILE_MANAGER, "Files", "folder"),
    ("org.example.Mail", "Mail", "document.text"),
    ("org.example.Browser", "Browser", "folder.applications"),
    ("org.example.Music", "Music", "folder.music"),
    ("org.example.Photos", "Photos", "folder.pictures"),
    ("org.example.Notes", "Notes", "document.pdf"),
    ("org.example.Terminal", "Terminal", "document.executable"),
];
/// The pinned applications, in order after the file manager.
const PINS: [&str; 6] = [
    "org.example.Mail",
    "org.example.Browser",
    "org.example.Music",
    "org.example.Photos",
    "org.example.Notes",
    "org.example.Terminal",
];
/// The applications running in `running`, `badges` and `force-quit`, with their windows' titles.
const RUNNING: [(&str, &str); 3] = [
    ("org.example.Mail", "Inbox"),
    ("org.example.Music", "Library"),
    ("org.example.Terminal", "~"),
];
/// The counts `badges` shows: one digit, several, and one past what is drawn in full.
const BADGES: [(&str, u64); 3] = [
    ("org.example.Mail", 3),
    ("org.example.Music", 42),
    ("org.example.Terminal", 12_000),
];
/// The application that does not respond in `force-quit`.
const UNRESPONSIVE: &str = "org.example.Music";
/// The switcher's list, most recently used first, and the selected entry.
const SWITCHED: [&str; 4] = [
    "org.example.Mail",
    FILE_MANAGER,
    "org.example.Music",
    "org.example.Terminal",
];
const SWITCHER_SELECTED: usize = 1;

/// When the scene is set up and when it is drawn, in seconds on the model's clock: long enough after for every tile
/// to have come in, every indicator and the magnification to have ramped and the switcher to have shown.
const SET_UP_AT: f64 = 1.0;
const DRAWN_AT: f64 = 30.0;

/// The wallpaper under the windows: a light grey-blue and a dark slate, as 8-bit sRGB.
const WALLPAPER_LIGHT: [u8; 3] = [196, 210, 228];
const WALLPAPER_DARK: [u8; 3] = [36, 44, 58];

thread_local! {
    /// The model the next capture's runtime takes.
    static NEXT: RefCell<Option<Model>> = const { RefCell::new(None) };
}

/// What the dock needs of the session, answered with fixtures: a fixed clock, the asset packs' icons and nothing sent.
struct Fixture {
    clock: Rc<Cell<f64>>,
    icons: SystemIcons,
}

impl Fixture {
    /// The asset packs' icon that stands in for `icon`.
    fn name(icon: &IconKey) -> &'static str {
        match icon {
            IconKey::Trash(false) => "trash.empty",
            IconKey::Trash(true) => "trash.full",
            IconKey::Named(_) => "folder",
            IconKey::App(key) => APPS
                .iter()
                .find(|(id, ..)| id == key)
                .map_or("application.generic", |(.., icon)| icon),
        }
    }
}

impl Host for Fixture {
    fn now(&self) -> f64 {
        self.clock.get()
    }

    fn wake(&mut self, _: f64, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn request(&mut self, _: &WindowKey, _: Request) {}

    fn spawn(&mut self, _: Vec<String>, _: Option<String>) -> Started {
        Started::No
    }

    fn open(&mut self, _: &str) {}

    fn shell(&mut self, _: &str) {}

    fn store_pins(&mut self, _: &[String]) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn read_conf(&mut self, _: &DockSettings) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn terminal(&mut self) -> Option<String> {
        None
    }

    fn icon(&mut self, _: &DesktopIndex, icon: &IconKey, side: f64) -> Option<IconSet> {
        let name = Self::name(icon);
        let set = ICON_SCALES
            .iter()
            .map(|scale| {
                let px = (side * scale).ceil().max(1.0) as u32;
                let raster = self.icons.raster(name, px);
                ImageSource {
                    id: next_resource_id(),
                    image: ImageResource {
                        revision: 0,
                        width: raster.width(),
                        height: raster.height(),
                        stride: raster.width() * 4,
                        format: PixelFormat::Rgba8Premultiplied,
                        pixels: Arc::from(raster.data()),
                    },
                }
            })
            .collect();
        Some(set)
    }

    fn window_icon(&mut self, _: [String; 2], _: WindowIcon) {}

    fn window_closed(&mut self, _: u32) {}

    fn trash_full(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
}

/// The fixture's desktop entries.
fn desktop() -> DesktopIndex {
    let entries = APPS
        .iter()
        .filter_map(|(id, name, _)| {
            let text = format!(
                "[Desktop Entry]
Type=Application
Name={name}
Exec={id}
"
            );
            parse_entry(id, Path::new(&format!("/fixture/{id}.desktop")), &text)
        })
        .collect();
    DesktopIndex { entries }
}

/// Hand `message` to the model; what it asks to run is not run, the fixture having answered what it needs.
fn tell(model: &mut Model, message: Message) {
    drop(kit::update(model, message));
}

/// The dock's role telling the model `events`.
fn dock_events(model: &mut Model, events: Vec<DockEvent>) {
    tell(
        model,
        Message::Dock {
            output: OUTPUT.key,
            events,
        },
    );
}

/// The windows of [`RUNNING`] appearing, in the first group of the role, which lists every window.
fn run_apps(model: &mut Model) {
    let windows = RUNNING
        .iter()
        .zip(1..)
        .map(|(&(app_id, title), id)| DockEvent::Window {
            id,
            app_id: app_id.to_owned(),
            title: title.to_owned(),
            minimized: false,
        })
        .collect();
    dock_events(model, windows);
}

/// The model of `surface` in `look`, set up at [`SET_UP_AT`] and settled at [`DRAWN_AT`].
fn model(surface: &str, look: Look, assets: &Assets) -> Result<Model, String> {
    let clock = Rc::new(Cell::new(0.0));
    let mut cfg = dock_config(None, OUTPUT.height);
    cfg.magnification = surface == "magnified";
    let mut dock = Dock::new(cfg);
    dock.set_built_in(FILE_MANAGER);
    dock.set_pinned(&PINS);
    let host = Fixture {
        clock: Rc::clone(&clock),
        icons: SystemIcons::new(assets),
    };
    let mut model = Model::new(
        Session::new(dock, desktop()),
        ActionMap::default(),
        DockSettings::default(),
        Box::new(host),
    );
    for message in [
        Message::Dark(look.dark),
        Message::ReduceMotion(false),
        Message::Locked(false),
        Message::Outputs(vec![OUTPUT]),
    ] {
        tell(&mut model, message);
    }
    clock.set(SET_UP_AT);
    match surface {
        "bar" | "magnified" => dock_events(&mut model, Vec::new()),
        "running" => run_apps(&mut model),
        "badges" => {
            run_apps(&mut model);
            for (app, count) in BADGES {
                let update = BadgeUpdate {
                    app: app.to_owned(),
                    count: Some(count),
                    visible: Some(true),
                };
                tell(&mut model, Message::Badge(update));
            }
        }
        "switcher" => {
            dock_events(&mut model, Vec::new());
            dock_events(
                &mut model,
                vec![DockEvent::Switcher {
                    output: OUTPUT.key,
                    apps: SWITCHED.map(str::to_owned).to_vec(),
                    selected: SWITCHER_SELECTED,
                }],
            );
        }
        "force-quit" => {
            run_apps(&mut model);
            dock_events(
                &mut model,
                vec![
                    DockEvent::Unresponsive {
                        app_id: UNRESPONSIVE.to_owned(),
                        unresponsive: true,
                    },
                    DockEvent::ForceQuitDialog,
                ],
            );
        }
        _ => {
            return Err(format!(
                "dock/{surface}: not a surface (`capture dock` lists them)"
            ))
        }
    }
    clock.set(DRAWN_AT);
    tell(&mut model, Message::Input(Input::Frame));
    if surface == "magnified" {
        let screen = model.screen().ok_or("the dock has no bar")?;
        let layout = model.session.dock.layout(screen, DRAWN_AT);
        let middle = layout
            .tiles
            .get(layout.tiles.len() / 2)
            .ok_or("the bar has no tiles")?
            .rect
            .center();
        let at = cupertino_kit::geometry::Point::new(middle.x, middle.y);
        tell(&mut model, Message::Input(Input::Bar(Pointer::Move(at))));
        clock.set(DRAWN_AT * 2.0);
        tell(&mut model, Message::Input(Input::Frame));
    }
    Ok(model)
}

/// The dock's model as the capture's runtime runs it.
struct Shown;

impl Application for Shown {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = NEXT
            .with_borrow_mut(Option::take)
            .expect("a model for the capture");
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        kit::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        kit::view(model)
    }
}

/// The window prefix of the `app_id` (`<prefix>@<output>`) of the window `surface` shows.
fn window_of(surface: &str) -> &'static str {
    match surface {
        "switcher" => SWITCHER_APP_ID,
        "force-quit" => FORCE_QUIT_APP_ID,
        _ => BAR_APP_ID,
    }
}

struct DockCapture {
    assets: Assets,
}

impl App for DockCapture {
    const NAME: &'static str = "dock";
    const STATE_FLAGS: &'static [&'static str] = &[];

    fn surfaces(&self) -> Vec<String> {
        SURFACES.map(str::to_owned).to_vec()
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        vec![State::new()]
    }

    fn capture(&mut self, request: &Capture<'_>) -> Result<Pixmap, String> {
        if request.size.is_some() {
            return Err("--size: the windows take the size the dock gives them".to_owned());
        }
        let model = model(request.surface, request.look, &self.assets)?;
        NEXT.set(Some(model));
        draw(window_of(request.surface), request.look, request.scale)
    }
}

/// The last frame of the model's window whose `app_id` starts with `prefix`, at `scale` in `look`, over
/// the wallpaper.
// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(prefix: &str, look: Look, scale: f32) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: true,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    let tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    options.theme = Arc::new(tokens);
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Shown>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver
        .runtime_mut()
        .set_theme_for(Box::new(move |appearance: &SystemAppearance| {
            let preferences = cupertino_kit::test::preferences(appearance);
            Arc::new(ThemeTokens::cupertino(&theme, preferences).expect("the theme loaded once"))
        }));
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
    let system = driver.window_system_mut();
    let window: WindowId = system
        .windows()
        .into_iter()
        .find(|&w| {
            system
                .spec(w)
                .is_ok_and(|spec| spec.app_id.starts_with(&format!("{prefix}@")))
        })
        .ok_or_else(|| format!("the dock shows no {prefix} window"))?;
    let size = system.spec(window).map_err(|e| fail(&e))?.size;
    system
        .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    system
        .configure(window, Some(size), WindowStates::default())
        .map_err(|e| fail(&e))?;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            break;
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    let picture = capture::presented(driver.runtime().window_system(), window)?;
    let wallpaper = if look.dark {
        WALLPAPER_DARK
    } else {
        WALLPAPER_LIGHT
    };
    Ok(over_wallpaper(&picture, wallpaper))
}

/// `picture` drawn over the wallpaper `rgb`: premultiplied source-over, each channel
/// `s + (d * (255 - sa) + 255) / 256` as the renderer's 8-bit pipeline rounds it.
fn over_wallpaper(picture: &Pixmap, rgb: [u8; 3]) -> Pixmap {
    let mut canvas = picture.clone();
    for pixel in canvas.data_mut().as_chunks_mut::<4>().0 {
        let inverse = 255 - u16::from(pixel[3]);
        for (c, d) in pixel.iter_mut().zip([rgb[0], rgb[1], rgb[2], 255]) {
            *c = (u16::from(*c) + ((u16::from(d) * inverse + 255) >> 8)) as u8;
        }
    }
    canvas
}

fn main() -> ExitCode {
    capture::run(|| {
        Ok(DockCapture {
            assets: capture::assets()?,
        })
    })
}
