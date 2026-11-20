//! The menu bar and its menus drawn to PNGs for look checks:
//! `cargo run -p cupertino-menubar --example menubar_capture -- menubar/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! The surfaces are `bar` (the bar alone), `menu` (the File menu open under its title, with its submenu open as a
//! second level), `system` (the system menu), `wifi`, `bluetooth`, `battery` and `focus` (the menus of those items),
//! `prompt` (the Wi-Fi join prompt of a secured network, its field empty) and `help` (the Help menu with text in its
//! search field and the results). Each picture is one canvas: the bar along the top and every popup at its place over a
//! plain wallpaper. `--wallpaper light|dark` (default `light`) names the wallpaper; the bar is see-through, so its
//! text takes the tone of that wallpaper as it does in the session. The content, its times and the clock are fixed, so
//! a command gives the same pixels on every run. The fonts, symbols and theme are the system's, as the process's. Each
//! picture is written as one PNG; there is no contact sheet.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::menubar::ShownItems;
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{self, Look, Options, Pixmap, State};
use cupertino_kit::test::{
    Capabilities, HeadlessWindowSystem, ManualClock, TestDriver, WindowEvent, WindowStates,
};
use cupertino_kit::text::{KeyInput, Keymap, TextSystem};
use cupertino_kit::theme::ThemeTokens;
use cupertino_menubar::controller::{Key, MenuController};
use cupertino_menubar::extras::{
    BatteryState, BluetoothDevice, BluetoothState, ExtraKind, FocusLifetime, FocusState,
    Passphrase, SessionCaps, SessionState, WifiLink, WifiNetwork, WifiState,
};
use cupertino_menubar::fallback::{self, HELP_MENU, HELP_SEARCH, SEARCH_PLACEHOLDER};
use cupertino_menubar::feed::{BarFeed, Shared, StatusState};
use cupertino_menubar::geom;
use cupertino_menubar::icons::Loader;
use cupertino_menubar::item::{ItemId, MenuItem, Shortcut, Toggle};
use cupertino_menubar::layout::{panel, BarContent, BarMetrics, SlotKey, TextMeasure};
use cupertino_menubar::popups;
use cupertino_menubar::tone::{Backdrop, BarTone, SlotTones};
use cupertino_menubar::view::{
    self, BarFont, BarId, BarScene, KitMeasure, PromptPhase, PromptScene, RowIcons,
};

/// The app's part of the surface and file names.
const NAME: &str = "menubar";
/// The state flags, without their dashes.
const STATE_FLAGS: &[&str] = &["wallpaper"];
const SURFACES: [&str; 9] = [
    "bar",
    "menu",
    "system",
    "wifi",
    "bluetooth",
    "battery",
    "focus",
    "prompt",
    "help",
];
const WALLPAPERS: [&str; 2] = ["light", "dark"];
/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 16;

/// The width of the output the bar spans, in points.
const OUTPUT_WIDTH: f64 = 1512.0;
/// The height the menus may take, in points: the output's height the controller keeps its popups inside.
const OUTPUT_HEIGHT: f64 = 900.0;
/// How much wallpaper shows below the lowest window of the picture, in points.
const WALLPAPER_BELOW: f64 = 24.0;
/// The least wallpaper a picture shows under the bar, in points.
const WALLPAPER_UNDER_BAR: f64 = 48.0;
/// The wallpaper of the `light` and the `dark` picture: a light grey-blue and a dark slate, as 8-bit sRGB. The bar's
/// text tone follows the luminance of the one in use.
const WALLPAPER_LIGHT: [u8; 3] = [196, 210, 228];
const WALLPAPER_DARK: [u8; 3] = [36, 44, 58];

/// The time the first press of a capture happens, in seconds on the controller's clock.
const PRESS_AT: f64 = 1.0;
/// How long a click's press lasts, in seconds: short enough to leave the menu open.
const CLICK_FOR: f64 = 0.05;
/// How long the controller waits before a submenu opens under a resting selection, with margin, in seconds.
const DWELL: f64 = 1.0;
/// The most keys a capture presses to select a row.
const MOST_KEYS: usize = 32;

/// The clock the bar shows: fixed.
const CLOCK: &str = "Fri 20 Nov  10:42";
/// The name of the active application, whose bold title opens the app menu.
const APP_NAME: &str = "Files";
/// The signed-in person's name the system menu's Log Out row carries.
const USER_NAME: &str = "Sam Rivera";
/// The network the join prompt asks the passphrase of.
const PROMPT_SSID: &str = "Harbour Cafe";
/// What the Help search field holds in the `help` picture, with the evdev code of each letter.
const HELP_TEXT: [(char, u32); 2] = [('c', 46), ('o', 24)];

thread_local! {
    /// The scene the next capture's windows show.
    static NEXT: RefCell<Option<Scene>> = const { RefCell::new(None) };
}

/// Everything the bar's windows show, owned.
struct Scene {
    controller: MenuController,
    shared: Shared,
    font: BarFont,
    tone: BarTone,
    solid: bool,
    size: Size,
    icons: Rc<RowIcons>,
    /// Told when a paint asks for a row icon that is not loaded yet.
    icons_wanted: async_channel::Receiver<()>,
    prompt: Option<(geom::Point, Passphrase)>,
}

/// What the capture's windows are told: the view's input, which changes nothing, or that a paint asked for row icons.
#[derive(Clone, Debug)]
enum Shown {
    Input,
    IconsWanted,
}

/// The next time a paint asks for row icons.
fn icons_wanted(wanted: &async_channel::Receiver<()>) -> Command<Shown> {
    let wanted = wanted.clone();
    Command::Perform(Box::pin(async move {
        let _ = wanted.recv().await;
        Shown::IconsWanted
    }))
}

/// The windows of [`NEXT`].
struct Capture;

impl Application for Capture {
    type Model = Scene;
    type Message = Shown;

    fn init() -> (Scene, Vec<Command<Shown>>) {
        let scene = NEXT
            .with_borrow_mut(Option::take)
            .expect("a scene for the capture");
        let listen = icons_wanted(&scene.icons_wanted);
        (scene, vec![listen])
    }

    /// The row icons a paint asked for are loaded the way the component loads them, here at once.
    fn update(scene: &mut Scene, message: Shown) -> Vec<Command<Shown>> {
        match message {
            Shown::Input => Vec::new(),
            Shown::IconsWanted => {
                let loader = Loader::default();
                for request in scene.icons.take_wanted() {
                    scene
                        .icons
                        .loaded(request.key, loader.row_icon(&request).as_ref());
                }
                vec![icons_wanted(&scene.icons_wanted)]
            }
        }
    }

    fn view(scene: &Scene) -> Vec<Window<Shown>> {
        view::windows(
            &BarScene {
                bar: BarId(0),
                controller: &scene.controller,
                shared: &scene.shared,
                font: scene.font,
                tone: scene.tone,
                slot_tones: &SlotTones::default(),
                solid: scene.solid,
                size: scene.size,
                open_panels: &[],
                icons: &scene.icons,
                prompt: scene.prompt.as_ref().map(|(at, text)| PromptScene {
                    ssid: PROMPT_SSID,
                    at: *at,
                    show: false,
                    text,
                    phase: PromptPhase::Editing,
                }),
            },
            |_| Shown::Input,
        )
    }
}

/// The font, the metrics and the text measure of the bar and its menus, made from the menu bar's design and the text
/// system as the component makes them.
fn bar_type(text: &Arc<TextSystem>) -> (BarFont, BarMetrics, Box<dyn TextMeasure>) {
    let design = cupertino_kit_shell::design::menu_bar();
    let metrics = BarMetrics {
        height: design.height,
        selection_corner_radius: design.selection_radius,
        selection_inset_y: BarMetrics::item_inset_y(design.height),
        ..BarMetrics::default()
    };
    let font = BarFont::of(&design);
    let measure = KitMeasure {
        text: Arc::clone(text),
        font,
    };
    (font, metrics, Box::new(measure))
}

/// The value of state flag `flag`, or the first of `values`; an error unless it is one of them.
fn flag<'a>(state: &'a State, flag: &str, values: &[&'a str]) -> Result<&'a str, String> {
    match state.iter().find(|(f, _)| f == flag) {
        None => Ok(values[0]),
        Some((_, value)) if values.contains(&value.as_str()) => Ok(value),
        Some((_, value)) => Err(format!("--{flag} {value}: one of {}", values.join(", "))),
    }
}

/// A chord of an item of the sample menus.
fn chord(text: &str) -> Shortcut {
    Shortcut::from_chord(text).expect("a chord of the sample menus")
}

/// The menus of the active application: File (with shortcuts, a separator, a disabled row, a checked row and a
/// submenu), Edit, View, Go, Window and Help.
fn menus() -> Vec<MenuItem> {
    let file = vec![
        MenuItem::new(101, "New Folder").with_shortcut(chord("Primary+Shift+N")),
        MenuItem::new(102, "New Window").with_shortcut(chord("Primary+N")),
        MenuItem::new(103, "Open").with_shortcut(chord("Primary+O")),
        MenuItem::submenu(
            104,
            "Open Recent",
            vec![
                MenuItem::new(141, "Budget 2027.csv"),
                MenuItem::new(142, "Meeting notes.md"),
                MenuItem::new(143, "Trip photos"),
                MenuItem::separator(144),
                MenuItem::new(145, "Clear Menu"),
            ],
        ),
        MenuItem::separator(105),
        MenuItem::new(106, "Close Window").with_shortcut(chord("Primary+W")),
        MenuItem::new(107, "Rename").disabled(),
        MenuItem::separator(108),
        MenuItem::new(109, "Show Hidden Files").with_toggle(Toggle::Check(true)),
        MenuItem::new(110, "Get Info").with_shortcut(chord("Primary+I")),
    ];
    let edit = vec![
        MenuItem::new(201, "Undo").with_shortcut(chord("Primary+Z")),
        MenuItem::new(202, "Redo").with_shortcut(chord("Primary+Shift+Z")),
        MenuItem::separator(203),
        MenuItem::new(204, "Cut").with_shortcut(chord("Primary+X")),
        MenuItem::new(205, "Copy").with_shortcut(chord("Primary+C")),
        MenuItem::new(206, "Paste").with_shortcut(chord("Primary+V")),
        MenuItem::new(207, "Select All").with_shortcut(chord("Primary+A")),
    ];
    let view = vec![
        MenuItem::new(301, "as Icons").with_toggle(Toggle::Radio(true)),
        MenuItem::new(302, "as List").with_toggle(Toggle::Radio(false)),
        MenuItem::new(303, "as Columns").with_toggle(Toggle::Radio(false)),
        MenuItem::separator(304),
        MenuItem::new(305, "Show Preview").with_shortcut(chord("Primary+Shift+P")),
    ];
    let go = vec![
        MenuItem::new(401, "Back").with_shortcut(chord("Primary+Left")),
        MenuItem::new(402, "Forward").with_shortcut(chord("Primary+Right")),
        MenuItem::separator(403),
        MenuItem::new(404, "Home").with_shortcut(chord("Primary+Shift+H")),
        MenuItem::new(405, "Connect to Server\u{2026}").with_shortcut(chord("Primary+K")),
    ];
    let window = vec![
        MenuItem::new(501, "Minimize").with_shortcut(chord("Primary+M")),
        MenuItem::new(502, "Zoom"),
        MenuItem::separator(503),
        MenuItem::new(504, "Compress Window"),
    ];
    let help = vec![
        MenuItem::new(HELP_SEARCH.0, SEARCH_PLACEHOLDER).disabled(),
        MenuItem::new(601, "Files Help"),
    ];
    vec![
        MenuItem::submenu(1, "File", file),
        MenuItem::submenu(2, "Edit", edit),
        MenuItem::submenu(3, "View", view),
        MenuItem::submenu(4, "Go", go),
        MenuItem::submenu(5, "Window", window),
        MenuItem::submenu(HELP_MENU.0, "Help", help),
    ]
}

/// The services' state: Wi-Fi on and joined, Bluetooth on, the battery at 76 % on battery, Focus on, a session, and one
/// third-party status item.
fn services(shared: &mut Shared) {
    let network =
        |ssid: &str, dbm: i32, security: &str, known: bool, connected: bool| WifiNetwork {
            ssid: ssid.to_owned(),
            signal_dbm: Some(dbm),
            security: security.to_owned(),
            known,
            connected,
        };
    let device = |id: &str, name: &str, paired: bool, connected: bool| BluetoothDevice {
        id: id.to_owned(),
        address: format!("00:11:22:33:44:{id}"),
        name: Some(name.to_owned()),
        paired,
        connected,
    };
    let lifetime = |id: &str, title: &str| FocusLifetime {
        id: id.to_owned(),
        title: title.to_owned(),
    };
    let extras = &mut shared.extras;
    extras.wifi = Some(WifiState {
        enabled: true,
        networks: vec![
            network("Home Network", -48, "wpa2", true, true),
            network(PROMPT_SSID, -61, "wpa2", false, false),
            network("Library Guest", -70, "open", false, false),
            network("Neighbour 5G", -78, "wpa3", true, false),
        ],
    });
    extras.wifi_list_open = true;
    extras.wifi_link = WifiLink {
        state: "connected".to_owned(),
        ssid: "Home Network".to_owned(),
        ..WifiLink::default()
    };
    extras.battery = Some(BatteryState {
        percent: 76,
        charging: false,
        on_battery: true,
    });
    extras.bluetooth = Some(BluetoothState {
        powered: true,
        devices: vec![
            device("01", "Magic Keyboard", true, true),
            device("02", "Trackpad", true, false),
            device("03", "Headphones", true, false),
        ],
    });
    extras.focus = Some(FocusState {
        active: true,
        lifetimes: vec![
            lifetime("hour", "For 1 hour"),
            lifetime("evening", "Until this evening"),
            lifetime("off", "Until I turn it off"),
        ],
    });
    extras.session = Some(SessionState {
        user_name: USER_NAME.to_owned(),
        caps: SessionCaps::default(),
    });
    // one third-party status item, drawn as a dot: it has no icon
    let item = cupertino_menubar::dbus::sni::StatusItem {
        id: "org.example.Sync/StatusNotifierItem".to_owned(),
        service: "org.example.Sync".to_owned(),
        title: "Sync".to_owned(),
        ..Default::default()
    };
    shared.set_status(StatusState {
        item,
        icon: None,
        menu: None,
    });
}

/// What the bar shows: the application's menus, the bar's own items, one third-party item and the clock.
fn content(shared: &mut Shared) -> BarContent {
    shared.conf.shown = ShownItems {
        clock: true,
        wifi: true,
        battery: true,
        bluetooth: true,
        focus: true,
    };
    shared.clock = CLOCK.to_owned();
    services(shared);
    let feed = BarFeed::new(0, false, false);
    let mut content = feed.content(shared);
    content.app_name = APP_NAME.to_owned();
    content.app_menu = fallback::app_menu(APP_NAME, &shared.chords);
    content.menus = menus();
    content
}

/// The centre of the slot `key` of the bar.
fn centre(controller: &MenuController, key: &SlotKey) -> Result<geom::Point, String> {
    let slot = controller
        .layout()
        .slot(key)
        .ok_or_else(|| format!("{key:?}: the bar has no such slot"))?;
    Ok(geom::Point::new(
        slot.rect.x + slot.rect.w / 2.0,
        slot.rect.y + slot.rect.h / 2.0,
    ))
}

/// Click the slot `key`: its menu opens and stays open.
fn open(controller: &mut MenuController, key: &SlotKey) -> Result<(), String> {
    let at = centre(controller, key)?;
    controller.pointer_press(at, PRESS_AT);
    controller.pointer_release(at, PRESS_AT + CLICK_FOR);
    if controller.open_slot() != Some(key) {
        return Err(format!("{key:?}: the menu did not open"));
    }
    Ok(())
}

/// Select the row labelled `label` of the first level with the keyboard.
fn select(controller: &mut MenuController, label: &str) -> Result<(), String> {
    for _ in 0..MOST_KEYS {
        let selected = controller.levels()[0]
            .selected_item()
            .map(|item| item.label.as_str());
        if selected == Some(label) {
            return Ok(());
        }
        controller.key(Key::Down, PRESS_AT + 1.0);
    }
    Err(format!("{label}: no row of the menu"))
}

/// Type [`HELP_TEXT`] into the Help menu's search field.
fn type_help(controller: &mut MenuController) -> Result<(), String> {
    let mut keymap = Keymap::new(
        Profile::cupertino(),
        cupertino_kit::text::binding_file::load_default().0,
    );
    for (character, code) in HELP_TEXT {
        let input = KeyInput::new(code, false, false, false, false, Some(character));
        controller
            .search_key(None, &mut keymap, input, Instant::now())
            .ok_or("the search field takes no key")?;
    }
    Ok(())
}

/// The scene of `surface`: the controller driven to where the surface shows, with fixed times.
fn scene(surface: &str, text: &Arc<TextSystem>, tone: (BarTone, bool)) -> Result<Scene, String> {
    let (font, metrics, measure) = bar_type(text);
    let height = metrics.height;
    let mut shared = Shared::default();
    let content = content(&mut shared);
    let output = geom::Rect::new(0.0, 0.0, OUTPUT_WIDTH, OUTPUT_HEIGHT);
    let mut controller = MenuController::new(output, measure);
    controller.bar_metrics = metrics;
    controller.menu_metrics.shortcut_style = shared.chords.style();
    controller.cfg.flash_choice = false;
    controller.set_content(content);
    let status = |kind: ExtraKind| SlotKey::Status(kind.slot_id().to_owned());
    let mut prompt = None;
    match surface {
        "bar" => {}
        "menu" => {
            open(&mut controller, &SlotKey::Menu(ItemId(1)))?;
            select(&mut controller, "Open Recent")?;
            controller.key(Key::Right, PRESS_AT + 1.0);
            controller.tick(PRESS_AT + 1.0 + DWELL);
            if controller.levels().len() < 2 {
                return Err("the submenu did not open".to_owned());
            }
        }
        "system" => open(&mut controller, &SlotKey::System)?,
        "wifi" => open(&mut controller, &status(ExtraKind::Wifi))?,
        "bluetooth" => open(&mut controller, &status(ExtraKind::Bluetooth))?,
        "battery" => open(&mut controller, &status(ExtraKind::Battery))?,
        "focus" => open(&mut controller, &status(ExtraKind::Focus))?,
        "prompt" => {
            // where the Wi-Fi panel's top-left corner is: its right edge level with the Wi-Fi item's, a little below
            let right = controller
                .layout()
                .slot(&status(ExtraKind::Wifi))
                .map_or(OUTPUT_WIDTH - 8.0, |slot| slot.rect.x + slot.rect.w);
            let at = geom::Point::new((right - panel::WIDTH).max(8.0), height + panel::GAP);
            prompt = Some((at, Passphrase::new("")));
        }
        "help" => {
            open(&mut controller, &SlotKey::Menu(HELP_MENU))?;
            type_help(&mut controller)?;
        }
        _ => {
            return Err(format!(
                "{NAME}/{surface}: not a surface (`capture {NAME}` lists them)"
            ))
        }
    }
    let (wanted, icons_wanted) = async_channel::bounded(1);
    Ok(Scene {
        controller,
        shared,
        font,
        tone: tone.0,
        solid: tone.1,
        size: Size::new(OUTPUT_WIDTH, height),
        icons: Rc::new(RowIcons::new(wanted)),
        icons_wanted,
        prompt,
    })
}

/// The wallpaper's colour and the bar's tone over it under `display`, and whether the bar is solid.
fn wallpaper(name: &str, display: DisplayOptions) -> ([u8; 3], BarTone, bool) {
    let rgb = if name == "dark" {
        WALLPAPER_DARK
    } else {
        WALLPAPER_LIGHT
    };
    let backdrop = Backdrop::Bands(vec![rgb.map(|c| u16::from(c) * 257)]);
    (
        rgb,
        BarTone::fresh(Some(backdrop.luminance()), display),
        cupertino_menubar::tone::solid_bar(display),
    )
}

/// Where each window after the bar has its top-left corner, in the bar's points: the open levels, then the prompt.
fn places(scene: &Scene) -> Vec<geom::Point> {
    let mut places: Vec<geom::Point> = scene
        .controller
        .levels()
        .iter()
        .map(|level| {
            let rect = popups::snap(level.layout.rect);
            geom::Point::new(rect.x, rect.y)
        })
        .collect();
    places.extend(scene.prompt.as_ref().map(|(at, _)| *at));
    places
}

/// The height of the picture in points: the bar, and the lowest window with some wallpaper under it.
fn picture_height(bar: Size, sizes: &[Size], places: &[geom::Point]) -> f64 {
    let lowest = places
        .iter()
        .zip(sizes)
        .map(|(at, size)| at.y + size.height)
        .fold(0.0, f64::max);
    (bar.height + WALLPAPER_UNDER_BAR).max(lowest + WALLPAPER_BELOW)
}

/// Run `window` at `scale`, `size` points large (or as placed), until it rests.
fn settle(
    driver: &mut TestDriver<Capture>,
    window: WindowId,
    scale: f32,
    size: Option<Size>,
) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let system = driver.window_system_mut();
    system
        .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    let size = match size {
        Some(size) => Some(size),
        None => system
            .placement(window)
            .map_err(|e| fail(&e))?
            .map(|placement| placement.size),
    };
    system
        .configure(window, size, WindowStates::default())
        .map_err(|e| fail(&e))?;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            return Ok(());
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    Ok(())
}

/// One picture of `surface` in `state` and `look`, at `scale` pixels a point.
// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(surface: &str, state: &State, look: Look, scale: f32) -> Result<Pixmap, String> {
    let wallpaper_name = flag(state, "wallpaper", &WALLPAPERS)?;
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
    let display = DisplayOptions {
        reduce_motion: false,
        reduce_transparency: look.reduce_transparency,
        increase_contrast: look.increase_contrast,
        differentiate_without_color: false,
    };
    let (rgb, tone, solid) = wallpaper(wallpaper_name, display);
    let scene = scene(surface, &options.text, (tone, solid))?;
    let places = places(&scene);
    let bar_size = scene.size;
    NEXT.set(Some(scene));
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Capture>::new(
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
    let windows: Vec<WindowId> = driver.runtime().windows().iter().map(|w| w.1).collect();
    if windows.len() != places.len() + 1 {
        return Err(format!(
            "the model shows {} windows, {} expected",
            windows.len(),
            places.len() + 1
        ));
    }
    settle(&mut driver, windows[0], scale, Some(bar_size))?;
    for window in &windows[1..] {
        settle(&mut driver, *window, scale, None)?;
    }
    let system: &HeadlessWindowSystem = driver.runtime().window_system();
    let pictures: Vec<Pixmap> = windows
        .iter()
        .map(|window| capture::presented(system, *window))
        .collect::<Result<_, _>>()?;
    let points = |pixels: u32| f64::from(pixels) / f64::from(scale);
    let sizes: Vec<Size> = pictures[1..]
        .iter()
        .map(|p| Size::new(points(p.width()), points(p.height())))
        .collect();
    let height = picture_height(bar_size, &sizes, &places);
    compose(rgb, &pictures, &places, scale, (bar_size.width, height))
}

/// The picture: the wallpaper `rgb`, the bar at the top and each popup at its place over it, `size` points large.
fn compose(
    rgb: [u8; 3],
    pictures: &[Pixmap],
    places: &[geom::Point],
    scale: f32,
    size: (f64, f64),
) -> Result<Pixmap, String> {
    let px = |points: f64| (points * f64::from(scale)).round() as u32;
    let mut canvas = Pixmap::new(px(size.0), px(size.1)).ok_or("the picture is empty")?;
    for pixel in canvas.data_mut().as_chunks_mut::<4>().0 {
        *pixel = [rgb[0], rgb[1], rgb[2], 255];
    }
    let at = |p: &geom::Point| (px(p.x) as i32, px(p.y) as i32);
    let origins = std::iter::once((0, 0)).chain(places.iter().map(at));
    for (picture, origin) in pictures.iter().zip(origins) {
        over(&mut canvas, picture, origin);
    }
    Ok(canvas)
}

/// `picture` drawn over `canvas` with its top-left corner at `origin`: premultiplied source-over, each channel
/// `s + (d * (255 - sa) + 255) / 256` as the renderer's 8-bit pipeline rounds it.
fn over(canvas: &mut Pixmap, picture: &Pixmap, origin: (i32, i32)) {
    let (width, height) = (canvas.width() as i32, canvas.height() as i32);
    let source = picture.data();
    let target = canvas.data_mut();
    for y in 0..picture.height() as i32 {
        let ty = origin.1 + y;
        if !(0..height).contains(&ty) {
            continue;
        }
        for x in 0..picture.width() as i32 {
            let tx = origin.0 + x;
            if !(0..width).contains(&tx) {
                continue;
            }
            let s = (y * picture.width() as i32 + x) as usize * 4;
            let t = (ty * width + tx) as usize * 4;
            let inverse = 255 - u16::from(source[s + 3]);
            for c in 0..4 {
                let d = u16::from(target[t + c]);
                target[t + c] = (u16::from(source[s + c]) + ((d * inverse + 255) >> 8)) as u8;
            }
        }
    }
}

/// Writes `image` as the PNG `name` in `out`.
// the example writes its pictures itself, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn save(image: &Pixmap, out: &Path, name: &str) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    let path = out.join(name);
    image
        .save_png(&path)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

/// Carries out `options`.
fn run(options: &Options) -> Result<(), String> {
    let Some(surface) = options.surface.as_deref() else {
        for surface in SURFACES {
            println!("{NAME}/{surface}");
        }
        return Ok(());
    };
    if options.sheet {
        return Err(
            "--sheet: this example writes one PNG per picture and no contact sheet".to_owned(),
        );
    }
    if options.size.is_some() {
        return Err("--size: the windows take the size of what they show".to_owned());
    }
    for &dark in &options.dark {
        let look = Look {
            dark,
            increase_contrast: options.increase_contrast,
            reduce_transparency: options.reduce_transparency,
        };
        let image = draw(surface, &options.state, look, options.scale)?;
        let name = capture::file_name(NAME, surface, &options.state, look, options.scale);
        save(&image, &options.out, &name)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let result = capture::assets()
        .and_then(|_| capture::parse(NAME, STATE_FLAGS, std::env::args().skip(1)))
        .and_then(|options| run(&options));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("capture: {e}");
            ExitCode::from(2)
        }
    }
}
