//! Glue between the settings window model and the toolkit: one application window with the unified title bar style
//! (the surface is the whole window; the server draws the window control buttons over the sidebar and the material
//! under it), the sidebar, the search field with its results popup, history, the keyboard of `keys`, and the panes:
//! their controls commit on the moments of §7.2 and write through `cupertino-defaults` or call `org.cupertino.Session1`.

use std::error::Error;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_defaults::accessibility::{DisplayOptions, SYSTEM_ACCESSIBILITY};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_filepicker::Mode;
use cupertino_geometry::Rect as PopupRect;
use cupertino_protocol_app::{AppEffects, AppEvent, Appearance as AppearanceObserver, Backdrop};
use cupertino_settings::agent::{Agent, AgentEvent};
use cupertino_settings::controls::{self, Control, Press, Value};
use cupertino_settings::form::{self, SectionLayout, SectionSpec};
use cupertino_settings::history::History;
use cupertino_settings::inflight::Reads;
use cupertino_settings::instance;
use cupertino_settings::keys::{self, Action, Focus, Mods, SearchKey};
use cupertino_settings::live::{display_name, Live, LoginScreen, Pictures, Reading};
use cupertino_settings::menu;
use cupertino_settings::menu_bar::{self, MenuAction, MenuState};
use cupertino_settings::paint::{
    caret_area, menu_rows, popup_hit, row_area, scroll_areas, search_text_area, Painter, Palette,
    PaneView, WindowState,
};
use cupertino_settings::panes::{self, Call, ControlId, Effect, Section, Session};
use cupertino_settings::search::{popup, SearchField};
use cupertino_settings::services::{self, Services, SESSION_NAME, SESSION_PATH};
use cupertino_settings::settings::{system_accessibility, File, Paths, Settings};
use cupertino_settings::sheet::{Answer, Outcome, Sheet, SheetKey};
use cupertino_settings::sidebar::{self, RowSize};
use cupertino_settings::window::{self, metrics, Hit, WindowLayout};
use cupertino_settings::{PaneId, Point, Size};
use cupertino_theme::{Appearance, Theme, WindowCornerSize};
use cupertino_ui_menus::WindowMenus;
use cupertino_ui_text::{KeyInput, Keymap, Response, TextBuffer, TextFieldKeys};
use cupertino_ui_wayland::clipboard::Clipboard;
use cupertino_ui_wayland::popup::placement;
use cupertino_ui_wayland::{copy_rects, Damage, Ready, ShmPresenter};
use cupertino_wayland_client::{
    Button, Capability, Connection, Decorations, Event, KeyboardEvent, Modifiers as KeyModifiers,
    PointerEvent, Popup, SurfaceId, Toplevel, ToplevelConfig, WindowConfigure,
};
use tiny_skia::{Color, Pixmap};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Identifier of the window for the server and the name of its desktop entry.
const APP_ID: &str = "cupertino-settings";

/// The material the server puts behind the sidebar: the behind-window `Sidebar` recipe, the same as the file
/// manager's (that the settings sidebar is that split-view item is there).
const SIDEBAR_MATERIAL: &str = "Sidebar";

/// The material behind the results popup (approximate, §5: the menu's).
const POPUP_MATERIAL: &str = "Menu";

/// The interface width factor: 1 for English (approximate).
const WIDTH_FACTOR: f64 = 1.0;

/// A pop-up button's open menu of choices.
struct ChoiceMenu {
    popup: Popup,
    presenter: ShmPresenter,
    backdrop: Backdrop,
    id: ControlId,
    choices: Vec<String>,
    selected: usize,
    hovered: Option<usize>,
    type_ahead: menu::TypeAhead,
}

/// A slider being dragged: its section and row and the value it shows.
#[derive(Clone, Copy)]
struct SliderDrag {
    section: usize,
    row: usize,
    shown: f64,
}

/// The open results popup.
struct Results {
    popup: Popup,
    presenter: ShmPresenter,
    backdrop: Backdrop,
    /// The size it was created or last moved with.
    size: (f64, f64),
    hovered: Option<usize>,
}

/// The application window: its role, its frames and the states of its last configure.
struct Window {
    toplevel: Toplevel,
    presenter: ShmPresenter,
    /// What changed since the last frame; a frame draws all of the window unless every change said where.
    damage: Damage,
    /// The whole frame, drawn before the damaged rectangles of it are copied into the presenter's pixmap.
    scratch: Option<Pixmap>,
    /// The size asked for, kept until a configure sets one.
    requested: (u32, u32),
    activated: bool,
    maximized: bool,
    fullscreen: bool,
}

impl Window {
    /// Open a window with the server's title bar in the unified style; `size` and `min_size` in points.
    fn open(
        conn: &mut Connection,
        effects: &AppEffects,
        title: String,
        size: (u32, u32),
        min_size: (u32, u32),
    ) -> Result<Window> {
        let toplevel = conn.toplevel(&ToplevelConfig {
            app_id: APP_ID.into(),
            title,
            min_size: Some(min_size),
            max_size: None,
            decorations: Decorations::Server,
            foreign_parent: None,
        })?;
        effects.unify_titlebar(conn, toplevel.surface())?;
        Ok(Window {
            toplevel,
            presenter: ShmPresenter::new(toplevel.surface()),
            damage: Damage::new(),
            scratch: None,
            requested: size,
            activated: false,
            maximized: false,
            fullscreen: false,
        })
    }

    /// Draw all of the window again as soon as the surface can take a frame.
    fn redraw(&mut self) {
        self.damage.add_whole();
        self.presenter.request_redraw();
    }

    /// Draw `rect` (points) of the window again as soon as the surface can take a frame.
    fn redraw_rect(&mut self, rect: PopupRect) {
        self.damage.add(rect);
        self.presenter.request_redraw();
    }

    /// Take a configure: its states, and its size where it sets one.
    fn configure(&mut self, configure: &WindowConfigure) -> Result<()> {
        // the toolkit draws no title bar of its own: an undecorated window could not be moved or closed
        if configure.decorations != Decorations::Server {
            return Err("the server refused server-side decorations".into());
        }
        self.activated = configure.states.activated;
        self.maximized = configure.states.maximized;
        self.fullscreen = configure.states.fullscreen;
        let current = self.presenter.size().unwrap_or(self.requested);
        self.presenter
            .configured(configured_size(configure.size, current));
        Ok(())
    }

    /// The logical size of the last configure.
    fn size(&self) -> Result<(u32, u32)> {
        Ok(self
            .presenter
            .size()
            .ok_or("the window has no size before its first configure")?)
    }
}

/// The size a window takes from its configure: a zero or missing extent leaves it to the client, which keeps the
/// `current` one.
fn configured_size(asked: Option<(u32, u32)>, current: (u32, u32)) -> (u32, u32) {
    let (width, height) = asked.unwrap_or((0, 0));
    (
        if width == 0 { current.0 } else { width },
        if height == 0 { current.1 } else { height },
    )
}

/// The presenters of the window, the results popup and the open menu.
fn presenters<'a>(
    window: &'a mut Window,
    results: &'a mut Option<Results>,
    menu: &'a mut Option<ChoiceMenu>,
) -> impl Iterator<Item = &'a mut ShmPresenter> {
    std::iter::once(&mut window.presenter)
        .chain(results.iter_mut().map(|r| &mut r.presenter))
        .chain(menu.iter_mut().map(|m| &mut m.presenter))
}

/// The value an extension event of the public protocols carries.
fn app_event(event: &Event) -> Option<AppEvent> {
    match event {
        Event::Extension(event) => event.downcast_ref::<AppEvent>().cloned(),
        _ => None,
    }
}

struct App {
    conn: Connection,
    effects: AppEffects,
    /// Reports the session's appearance and display options while it lives.
    _appearance: AppearanceObserver,
    /// The session's appearance and display options, as last reported.
    session_dark: bool,
    display: DisplayOptions,
    clipboard: Clipboard,
    window: Window,
    sidebar: Backdrop,
    theme: Theme,
    palette: Palette,
    painter: Painter,
    groups: Vec<Vec<PaneId>>,
    /// The panes no service backs, left out of the sidebar and the search.
    hidden: Vec<PaneId>,
    history: History<PaneId>,
    search: SearchField,
    /// The user's modifier profile and editing chords, for the search field.
    keymap: Keymap,
    /// The x of each caret position of the search text as last drawn (points from the text's start).
    search_offsets: Vec<f64>,
    /// Whether the last frame showed the search field's caret; a blink redraws when it changes.
    caret_drawn: bool,
    results: Option<Results>,
    focus: Focus,
    layout: Option<WindowLayout>,
    /// A press on a sidebar row: it is selected at once and pushed on the history when the button is released over it.
    pressed_row: Option<PaneId>,
    /// The row shown selected while it is pressed.
    showing: PaneId,
    last_titlebar_press: Option<std::time::Instant>,
    paths: Paths,
    settings: Settings,
    /// The text of the last refused change of a control, shown under its row until it changes again.
    errors: Vec<(ControlId, String)>,
    /// Problems already written to the log, so a repeating one is logged once per run.
    reported: std::collections::HashSet<String>,
    /// The pane the content column shows, its scroll offset and its layout as last drawn.
    content_pane: PaneId,
    scroll: f64,
    form: Vec<SectionLayout>,
    form_height: f64,
    drag: Option<SliderDrag>,
    menu: Option<ChoiceMenu>,
    /// The appearance shown: the session's, or the one just asked for until the session reports it.
    dark: bool,
    bus: Option<zbus::blocking::Connection>,
    services: Services,
    live: Live,
    sheet: Option<Sheet>,
    /// A level slider being dragged: its section and row, the value shown and its writer.
    level: Option<(usize, usize, f64, controls::LevelWriter)>,
    start: std::time::Instant,
    /// When the live readings were last refreshed, and when the Wi-Fi device last scanned.
    refreshed: f64,
    scanned: f64,
    /// What arrived from the bus threads.
    background: std::sync::mpsc::Receiver<Background>,
    notify: Notify,
    assets: Assets,
    /// The pictures whose thumbnail is asked for, so each is made once.
    thumbnails_asked: std::collections::HashSet<std::path::PathBuf>,
    /// The listings running on threads: one that is running is not started again, and one asked for meanwhile is read
    /// after it. The chooser's listings carry their request.
    listings: Reads<ListingKey, Option<cupertino_filepicker::Request>>,
    /// The open pairing question's answer, sent when its sheet decides.
    agent_answer: Option<(u64, async_channel::Sender<Answer>)>,
    /// Whether this window's pairing agent is registered.
    agent_registered: bool,
    /// The openings of the Accessibility pane so far: an answer to `CanSetSystemAccessibility` counts for its own only.
    login_opening: u64,
    /// A Use Mine write is waiting for sessiond's reply; it outlives the opening it was pressed in.
    login_saving: bool,
    /// Watches the settings files while it lives; `None` when the watch could not be set up.
    _files: Option<ConfigWatcher>,
    /// Owns `org.cupertino.Settings1` while it lives.
    _instance: zbus::Connection,
    /// The menus published to the desktop's menu bar; `None` when the server or the session bus takes none.
    menus: Option<WindowMenus>,
    /// What the published menus were made from.
    menu_state: Option<MenuState>,
    /// Quit was chosen.
    closing: bool,
}

/// Something for the window loop from another thread.
enum Background {
    ShowPane(PaneId, String),
    Agent(AgentEvent),
    /// A pairing ended.
    Paired(std::result::Result<(), String>),
    /// A settings file changed on disk.
    Files(WatchEvent),
    /// A listing of the Wallpaper pane's pictures: group 0 the packs', then one per added folder.
    Pictures {
        key: ListingKey,
        files: Vec<cupertino_assets::WallpaperFile>,
    },
    /// A folder the chooser asked for was read.
    Listing(cupertino_filepicker::Listing),
    /// The thumbnail of a picture of the Wallpaper pane is made.
    Thumbnail {
        image: std::path::PathBuf,
        thumbnail: std::path::PathBuf,
    },
    /// Whether the Login Screen section shows, asked at the Accessibility pane's opening `opening`.
    LoginScreen {
        opening: u64,
        allowed: bool,
    },
    /// sessiond answered Use Mine.
    LoginScreenSaved(std::result::Result<(), String>),
}

/// What a listing thread reads: the packs' pictures, the pictures of an added folder, or a folder of the chooser.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ListingKey {
    Packs,
    Pictures(std::path::PathBuf),
    Chooser(std::path::PathBuf, bool, cupertino_filepicker::Mode),
}

/// The outcome of an activation request, `None` when the compositor has no activation protocol: the window then shows
/// without being raised.
pub(crate) fn unless_unsupported<T>(
    result: std::result::Result<T, cupertino_wayland_client::Error>,
) -> std::result::Result<Option<T>, cupertino_wayland_client::Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(cupertino_wayland_client::Error::Unsupported(Capability::Activation)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Hands a [`Background`] to the window loop and wakes it.
type Notify = std::sync::Arc<dyn Fn(Background) + Send + Sync>;

/// Background events waiting for the window loop.
const BACKGROUND_QUEUE: usize = 64;
/// Where Choose Program… opens (approximate).
const PROGRAM_CHOOSER_START: &str = "/usr/bin";

/// The window loop owns the receiver until it ends; what arrives after that has no window. What arrives before the
/// window exists waits for the loop's first turn. A full queue makes a worker thread wait. What comes from a bus is
/// dropped instead, so a caller can neither grow the queue nor block a bus executor: a dropped pane request shows nothing,
/// a dropped agent prompt refuses the pairing.
fn background_channel(
    wake: impl Fn() + Send + Sync + 'static,
) -> (Notify, std::sync::mpsc::Receiver<Background>) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(BACKGROUND_QUEUE);
    let notify: Notify = std::sync::Arc::new(move |event| {
        let queued = match event {
            Background::ShowPane(..) | Background::Agent(_) => sender.try_send(event).is_ok(),
            event => sender.send(event).is_ok(),
        };
        if queued {
            wake();
        }
    });
    (notify, receiver)
}

/// The live readings are read again this often as a safety net (the control panel's 1 s, approximate, §6.4).
const REFRESH: f64 = 1.0;
/// The Wi-Fi pane scans this often while it is shown and the radio is on (approximate, §6.4).
const SCAN: f64 = 10.0;

/// The palette of the session's appearance and display options.
fn palette(theme: &Theme, dark: bool, options: DisplayOptions) -> Result<Palette> {
    let appearance = match (dark, options.increase_contrast) {
        (false, false) => Appearance::Light,
        (true, false) => Appearance::Dark,
        (false, true) => Appearance::LightHighContrast,
        (true, true) => Appearance::DarkHighContrast,
    };
    Ok(Palette::from_theme(theme, appearance)?)
}

/// Connect, open the window on `pane` and serve it until it is closed.
pub fn run(pane: PaneId, activation_token: String) -> Result<()> {
    // one process per session: a second start hands its pane to the first and ends (§8); before the window, so it
    // needs no display
    let waker = std::sync::Arc::new(std::sync::OnceLock::<cupertino_wayland_client::Waker>::new());
    let wake = waker.clone();
    let (notify, background) = background_channel(move || {
        if let Some(waker) = wake.get() {
            waker.wake();
        }
    });
    let to_loop = notify.clone();
    let on_show = move |pane, token| to_loop(Background::ShowPane(pane, token));
    let Some(instance) = instance::serve_or_hand_over(
        || instance::serve(zbus::connection::Builder::session()?, on_show.clone()),
        || {
            instance::show(
                zbus::connection::Builder::session()?,
                pane.key(),
                &activation_token,
            )
        },
    )
    .map_err(|e| format!("cannot claim or reach {}: {e}", instance::BUS_NAME))?
    else {
        return Ok(());
    };
    let mut services = Services::new();
    let hidden = services.detect().hidden();
    let pane = if hidden.contains(&pane) {
        sidebar::rows(&hidden)[0]
    } else {
        pane
    };
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let painter = Painter::new(&assets)?;
    let mut conn =
        Connection::connect().map_err(|e| format!("cannot connect to the display server: {e}"))?;
    assert!(waker.set(conn.waker()).is_ok(), "the waker is set once");
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    // the session's appearance at connect is read before the window draws
    let appearance = effects.appearance(&conn);
    conn.roundtrip()?;
    let (mut dark, mut display) = (false, DisplayOptions::default());
    let early: Vec<Event> = conn
        .wait(Some(Duration::ZERO))?
        .into_iter()
        .filter(|event| match app_event(event) {
            Some(AppEvent::Appearance { dark: d }) => {
                dark = d;
                false
            }
            Some(AppEvent::DisplayOptions(options)) => {
                display = options;
                false
            }
            _ => true,
        })
        .collect();
    let size = window::default_size(WIDTH_FACTOR);
    let window = Window::open(
        &mut conn,
        &effects,
        pane.pane().title.into(),
        (size.w as u32, size.h as u32),
        // the width is fixed by the content; the height's minimum is approximate (O6)
        (size.w as u32, metrics::MIN_HEIGHT as u32),
    )?;
    if !activation_token.is_empty() {
        unless_unsupported(conn.activate_with_token(window.toplevel, &activation_token))?;
    }
    let sidebar = effects.backdrop(&conn, window.toplevel.surface())?;
    sidebar.set_material(SIDEBAR_MATERIAL);
    // the dock's default tile size depends on the height of the display it sits on
    let display_height = conn
        .outputs()
        .into_iter()
        .find_map(|o| o.geometry.map(|g| f64::from(g.height)))
        .ok_or("the server has no output")?;
    let paths = Paths::from_env();
    let settings = Settings::load(&paths, display_height)?;
    // another writer's change shows at once (the dock's menu, the server, an editor); a commit re-reads the files anyway
    let files: Vec<std::path::PathBuf> = [
        &paths.dock,
        &paths.compositor,
        &paths.menubar,
        &paths.keyboard,
        &paths.power,
        &paths.accessibility,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    let to_loop = notify.clone();
    let files = match ConfigWatcher::new(files, move |event| to_loop(Background::Files(event))) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("cupertino-settings: cannot watch the settings files: {e}; the panes show changes of other writers at the next commit");
            None
        }
    };
    let mut app = App {
        palette: palette(&theme, dark, display)?,
        conn,
        effects,
        _appearance: appearance,
        session_dark: dark,
        display,
        clipboard: Clipboard::default(),
        window,
        sidebar,
        theme,
        painter,
        groups: sidebar::groups(&hidden),
        history: History::new(pane),
        search: {
            let mut search = SearchField::default();
            search.set_hidden(&hidden);
            search
        },
        hidden,
        keymap: Keymap::new(
            cupertino_keyboard::binding::user_profile(),
            cupertino_keyboard::binding::load_default().0,
        ),
        search_offsets: Vec::new(),
        caret_drawn: false,
        results: None,
        focus: Focus::Sidebar,
        layout: None,
        pressed_row: None,
        showing: pane,
        last_titlebar_press: None,
        paths,
        settings,
        errors: Vec::new(),
        reported: std::collections::HashSet::new(),
        content_pane: pane,
        scroll: 0.0,
        form: Vec::new(),
        form_height: 0.0,
        drag: None,
        menu: None,
        dark,
        bus: None,
        services,
        live: Live::default(),
        sheet: None,
        level: None,
        start: std::time::Instant::now(),
        refreshed: f64::NEG_INFINITY,
        scanned: f64::NEG_INFINITY,
        background,
        notify,
        assets,
        thumbnails_asked: std::collections::HashSet::new(),
        listings: Reads::default(),
        agent_answer: None,
        agent_registered: false,
        login_opening: 0,
        login_saving: false,
        _files: files,
        _instance: instance,
        menus: None,
        menu_state: None,
        closing: false,
    };
    app.sync_accessibility();
    app.enter_pane(None, pane)?;
    app.publish_menus();
    let mut events = early;
    loop {
        for event in events {
            if !app.handle(event)? {
                app.leave_pane(app.content_pane);
                return Ok(());
            }
        }
        app.sync_menus()?;
        if app.closing {
            app.leave_pane(app.content_pane);
            return Ok(());
        }
        while let Ok(event) = app.background.try_recv() {
            app.background_event(event)?;
        }
        app.tick()?;
        app.present()?;
        events = app.conn.wait(Some(app.timeout()))?;
    }
}

impl App {
    /// React to one event; `false` once the window is to close.
    fn handle(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Configure { configure, .. } => self.window.configure(&configure)?,
            Event::PopupConfigure { popup, configure } => {
                let size = (configure.width, configure.height);
                if let Some(r) = self.results.as_mut().filter(|r| r.popup == popup) {
                    r.presenter.configured(size);
                }
                if let Some(m) = self.menu.as_mut().filter(|m| m.popup == popup) {
                    m.presenter.configured(size);
                }
            }
            Event::Frame(frame) => {
                if let Some(presenter) =
                    presenters(&mut self.window, &mut self.results, &mut self.menu)
                        .find(|p| p.surface() == frame.surface)
                {
                    presenter.frame_done();
                }
            }
            Event::Close(_) => return Ok(false),
            Event::Pointer { surface, event, .. } if Some(surface) == self.results_surface() => {
                self.results_pointer(event)?
            }
            Event::Pointer { surface, event, .. } if Some(surface) == self.menu_surface() => {
                self.menu_pointer(event)?
            }
            Event::Pointer { event, .. } => self.pointer(event)?,
            Event::Keyboard {
                event:
                    KeyboardEvent::Key {
                        code,
                        pressed: true,
                        mods,
                        character,
                        ..
                    },
                ..
            } => self.key(code, mods, character)?,
            Event::Keyboard { .. } | Event::Touch { .. } => {}
            Event::Selection(offer) => self.clipboard.selection_changed(offer),
            Event::Extension(_) => match app_event(&event) {
                Some(AppEvent::Appearance { dark }) => {
                    self.session_dark = dark;
                    self.dark = dark;
                    self.palette = palette(&self.theme, dark, self.display)?;
                    self.close_menu()?;
                    self.redraw();
                }
                Some(AppEvent::DisplayOptions(options)) => {
                    self.display = options;
                    self.palette = palette(&self.theme, self.session_dark, options)?;
                    self.redraw();
                }
                Some(event @ (AppEvent::MenuBar { .. } | AppEvent::MenuReader { .. })) => {
                    if let Some(menus) = &mut self.menus {
                        menus.handle(&event);
                    }
                }
                Some(AppEvent::ScrollCount { .. } | AppEvent::SessionLock { .. }) | None => {}
            },
            // the server dismissed the popup (a click outside it): the field keeps its text
            Event::PopupDone(popup) => {
                // a click on another client or the desktop ends the search, so the field is never open without its
                // popup. The click itself reaches what it hit (the grab forwards it); a click in this window keeps the
                // grab and reaches the window directly.
                if self.results.as_ref().is_some_and(|r| r.popup == popup) {
                    self.search.clear();
                    self.close_results()?;
                    self.window.redraw();
                }
                if self.menu.as_ref().is_some_and(|m| m.popup == popup) {
                    self.close_menu()?;
                }
            }
            // the scale is read when polling the presenters
            Event::ScaleChanged { .. }
            | Event::OutputsChanged
            | Event::ActivationToken { .. }
            | Event::TextInput { .. }
            | Event::PopupRepositioned { .. }
            | Event::Presented(_)
            | Event::Discarded(_)
            | Event::Drag { .. }
            | Event::DragSource { .. }
            | Event::Received { .. } => {}
        }
        Ok(true)
    }

    fn results_surface(&self) -> Option<SurfaceId> {
        self.results.as_ref().map(|r| r.popup.surface())
    }

    fn menu_surface(&self) -> Option<SurfaceId> {
        self.menu.as_ref().map(|m| m.popup.surface())
    }

    /// Draw every surface that can take a frame, until none can.
    fn present(&mut self) -> Result<()> {
        loop {
            let mut ready = Vec::new();
            for presenter in presenters(&mut self.window, &mut self.results, &mut self.menu) {
                loop {
                    match presenter.poll(&self.conn)? {
                        // a draw follows every scale change
                        Ready::ScaleChanged(_) => {}
                        Ready::Draw => break ready.push(presenter.surface()),
                        Ready::Idle => break,
                    }
                }
            }
            if ready.is_empty() {
                return Ok(());
            }
            for surface in ready {
                if surface == self.window.toplevel.surface() {
                    self.draw()?;
                } else if Some(surface) == self.results_surface() {
                    self.draw_results()?;
                } else if Some(surface) == self.menu_surface() {
                    self.draw_menu()?;
                }
            }
        }
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Until the next refresh, the level slider's waiting write, or the search field's caret blink.
    fn timeout(&self) -> std::time::Duration {
        let now = self.now();
        let mut due = self.refreshed + REFRESH;
        if let Some((_, _, _, w)) = &self.level {
            if let Some(t) = w.due() {
                due = due.min(t);
            }
        }
        if self.focus == Focus::Search {
            if let Some(blink) = self.search.field().next_blink(Instant::now()) {
                due = due.min(now + blink.as_secs_f64());
            }
        }
        std::time::Duration::from_secs_f64((due - now).clamp(0.0, REFRESH))
            + std::time::Duration::from_millis(1)
    }

    /// Run what is due: the level slider's waiting write, the refresh of the live readings, the Wi-Fi scan.
    fn tick(&mut self) -> Result<()> {
        if self.focus == Focus::Search
            && self.search.field().caret_visible(Instant::now()) != self.caret_drawn
        {
            match self.layout.as_ref().map(caret_area) {
                Some(a) => self.window.redraw_rect(PopupRect::new(a.x, a.y, a.w, a.h)),
                None => self.window.redraw(),
            }
        }
        let now = self.now();
        if let Some((section, row, _, mut writer)) = self.level.take() {
            let write = writer.tick(now);
            self.level = Some((section, row, self.level_shown(), writer));
            if let Some(v) = write {
                self.level_write(section, row, v)?;
            }
        }
        if now - self.refreshed >= REFRESH {
            self.refresh(now)?;
        }
        Ok(())
    }

    fn level_shown(&self) -> f64 {
        self.level.as_ref().map_or(0.0, |l| l.2)
    }

    /// Read the live services of the pane shown again; draw again when something changed. The battery is read on the
    /// first refresh too, whatever the pane: whether there is one names the Battery row.
    fn refresh(&mut self, now: f64) -> Result<()> {
        self.refreshed = now;
        let hidden = self.services.detect().hidden();
        if hidden != self.hidden {
            self.groups = sidebar::groups(&hidden);
            self.search.set_hidden(&hidden);
            self.hidden = hidden;
            self.window.redraw();
        }
        if self.hidden.contains(&self.history.current()) {
            return self.go(self.rows()[0]);
        }
        let before = self.live.clone();
        if self.content_pane == PaneId::Battery
            || self.live.battery == cupertino_settings::live::Reading::Unknown
        {
            self.live.battery = self.services.battery();
        }
        match self.content_pane {
            PaneId::Focus => self.live.focus = self.services.focus(),
            PaneId::Sound => {
                if self.level.is_none() {
                    self.live.volume = self.services.volume();
                }
            }
            PaneId::Displays => {
                if self.level.is_none() {
                    self.live.brightness = self.services.brightness();
                }
            }
            PaneId::Wifi | PaneId::Network => {
                let keep = self.live.wifi.ready().cloned();
                self.live.wifi = self.services.wifi(keep.as_ref());
                let radio_on = self
                    .live
                    .wifi
                    .ready()
                    .is_some_and(|w| w.enabled && w.has_wifi_device());
                if self.content_pane == PaneId::Wifi && radio_on && now - self.scanned >= SCAN {
                    self.scanned = now;
                    if let Err(e) = self.services.scan() {
                        eprintln!("cupertino-settings: Wi-Fi scan: {e}");
                    }
                }
            }
            PaneId::Mouse => self.live.mice = self.services.mice(),
            PaneId::Bluetooth => {
                let keep = self.live.bluetooth.ready().cloned();
                self.live.bluetooth = self.services.bluetooth(keep.as_ref());
            }
            _ => {}
        }
        if self.live != before {
            self.window.redraw();
        }
        Ok(())
    }

    /// The content now shows `to` (`from` before): start what the new pane needs and stop what the old one started.
    fn enter_pane(&mut self, from: Option<PaneId>, to: PaneId) -> Result<()> {
        if let Some(from) = from.filter(|f| *f != to) {
            self.leave_pane(from);
        }
        if from == Some(to) {
            return Ok(());
        }
        self.scanned = f64::NEG_INFINITY;
        if to == PaneId::Wallpaper {
            self.load_pictures();
        }
        if to == PaneId::Accessibility {
            self.ask_login_screen();
        }
        if to == PaneId::Privacy {
            self.settings.reload_clipboard(&self.paths);
            let (locale, visibility) = (
                cupertino_apps::entry::Locale::from_env(),
                cupertino_apps::entry::Visibility::from_env(),
            );
            let entries = cupertino_apps::entry::index_dirs(
                &cupertino_apps::entry::xdg_application_dirs(),
                &locale,
                &visibility,
            );
            let path_dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default();
            self.live.apps = cupertino_settings::clipboard::installed(entries, &path_dirs);
        }
        self.refresh(self.now())?;
        if to == PaneId::Bluetooth && self.live.bluetooth.ready().is_some_and(|b| b.available) {
            let notify = self.notify.clone();
            match self.services.register_agent(Agent {
                on_event: Box::new(move |event| notify(Background::Agent(event))),
            }) {
                Ok(ours) => {
                    self.agent_registered = ours;
                    if let cupertino_settings::live::Reading::Ready(b) = &mut self.live.bluetooth {
                        b.agent_elsewhere = !ours;
                    }
                }
                Err(e) => eprintln!("cupertino-settings: Bluetooth pairing agent: {e}"),
            }
            // discovery needs a powered adapter; the pane shows why otherwise
            if self.live.bluetooth.ready().is_some_and(|b| b.powered) {
                if let Err(e) = self.services.discovery(true) {
                    eprintln!("cupertino-settings: Bluetooth discovery: {e}");
                }
            }
            self.window.redraw();
        }
        Ok(())
    }

    fn leave_pane(&mut self, pane: PaneId) {
        if pane == PaneId::Bluetooth && self.live.bluetooth.ready().is_some_and(|b| b.discovering) {
            if let Err(e) = self.services.discovery(false) {
                eprintln!("cupertino-settings: Bluetooth discovery: {e}");
            }
        }
        if pane == PaneId::Bluetooth && self.agent_registered {
            self.agent_registered = false;
            if let Err(e) = self.services.unregister_agent() {
                eprintln!("cupertino-settings: Bluetooth pairing agent: {e}");
            }
        }
    }

    fn redraw(&mut self) {
        self.window.redraw();
        if let Some(r) = &mut self.results {
            r.presenter.request_redraw();
        }
    }

    fn draw(&mut self) -> Result<()> {
        let (width, height) = self.window.size()?;
        let active = self.window.activated;
        let layout = WindowLayout::new(
            Size {
                w: f64::from(width),
                h: f64::from(height),
            },
            WIDTH_FACTOR,
            RowSize::default(),
            &self.groups,
            self.history.can_go_back() || self.history.can_go_forward(),
        );
        // the material covers the sidebar column at the window's full height; a shape has one radius, the window's, so
        // the outer corners match the window and the inner ones lie under the opaque content
        let radius = WindowCornerSize::Regular.radius_pt();
        let s = layout.sidebar;
        self.sidebar
            .set_shape(&PopupRect::new(s.x, s.y, s.w + radius, s.h), radius)?;
        // another pane starts at its top
        if self.content_pane != self.history.current() {
            let from = self.content_pane;
            self.content_pane = self.history.current();
            self.scroll = 0.0;
            self.drag = None;
            self.level = None;
            self.sheet = None;
            self.agent_answer = None;
            self.errors.clear();
            self.enter_pane(Some(from), self.content_pane)?;
        }
        let sections = self.sections();
        let specs: Vec<SectionSpec<'_>> = sections
            .iter()
            .map(|sec| SectionSpec {
                header: sec.header.is_some(),
                rows: sec.rows.iter().map(|r| r.spec()).collect(),
                footer: sec.footer.is_some(),
            })
            .collect();
        let (form, form_height) =
            form::layout(&specs, layout.content.x, layout.content.w, layout.content.y);
        self.scroll = self
            .scroll
            .clamp(0.0, (form_height - layout.content.h).max(0.0));
        let dragging = self
            .drag
            .map(|d| (d.section, d.row, d.shown))
            .or(self.level.as_ref().map(|l| (l.0, l.1, l.2)));
        let has_battery = self.live.has_battery();
        let search_focused = self.focus == Focus::Search;
        let caret = search_focused && active && self.search.field().caret_visible(Instant::now());
        let App {
            conn,
            window,
            painter,
            palette,
            history,
            search,
            search_offsets,
            showing,
            scroll,
            sheet,
            ..
        } = self;
        let view = PaneView {
            sections: &sections,
            layout: &form,
            scroll: *scroll,
            dragging,
        };
        let content = layout.content;
        let damage = window.damage.take();
        let scratch = &mut window.scratch;
        window
            .presenter
            .draw_with(conn, damage.as_deref(), |pm, scale, cleared| {
                let offsets =
                    painter.search_offsets(search.field(), scale.factor() as f32, palette);
                let area = search_text_area(&layout.search, !search.text().is_empty());
                let caret_x = caret_x(search.field(), &offsets);
                let text_width = offsets.last().copied().unwrap_or(0.0);
                search.edit(|f| f.follow_caret(caret_x, text_width, area.w));
                *search_offsets = offsets;
                let state = WindowState {
                    selected: *showing,
                    has_battery,
                    active,
                    history,
                    search,
                    search_focused,
                    search_caret: caret,
                };
                let factor = scale.factor() as f32;
                // the frame is drawn whole; where only rectangles of the pixmap were cleared, those replace what it holds,
                // which makes the pixels the same as those of a whole redraw
                let mut whole = cleared.map(|_| match scratch.take() {
                    Some(p) if (p.width(), p.height()) == (pm.width(), pm.height()) => p,
                    _ => Pixmap::new(pm.width(), pm.height()).expect("the pixmap has a size"),
                });
                if let Some(whole) = whole.as_mut() {
                    whole.fill(Color::TRANSPARENT);
                }
                let target = whole.as_mut().unwrap_or(pm);
                painter.paint_window(target, factor, &layout, &state, &view, palette);
                if let Some(sheet) = sheet.as_ref() {
                    painter.paint_sheet(
                        target,
                        factor,
                        sheet,
                        &sheet.layout(&content),
                        &content,
                        palette,
                    );
                }
                if let (Some(whole), Some(rects)) = (whole, cleared) {
                    copy_rects(pm, &whole, rects);
                    *scratch = Some(whole);
                }
            })?;
        self.layout = Some(layout);
        self.caret_drawn = caret;
        self.form = form;
        self.form_height = form_height;
        Ok(())
    }

    fn draw_results(&mut self) -> Result<()> {
        let Some(r) = &self.results else {
            return Ok(());
        };
        let rows = popup::rows(self.search.results().len(), r.size.0, RowSize::default());
        let App {
            conn,
            painter,
            palette,
            search,
            results,
            ..
        } = self;
        let Results {
            presenter, hovered, ..
        } = results.as_mut().expect("checked above");
        presenter.draw(conn, None, |pm, scale| {
            painter.paint_popup(pm, scale.factor() as f32, search, &rows, *hovered, palette)
        })?;
        Ok(())
    }

    /// Show `pane`: select its row, push it on the history and name the window after it. A hidden pane shows the first
    /// row instead.
    fn go(&mut self, pane: PaneId) -> Result<()> {
        let pane = if self.hidden.contains(&pane) {
            self.rows()[0]
        } else {
            pane
        };
        self.history.push(pane);
        self.show_current()
    }

    fn show_current(&mut self) -> Result<()> {
        self.close_menu()?;
        let pane = self.history.current();
        self.showing = pane;
        self.conn
            .set_title(self.window.toplevel, pane.title(self.live.has_battery()))?;
        self.window.redraw();
        Ok(())
    }

    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        if self.menu.is_some() && matches!(event, PointerEvent::Button { pressed: true, .. }) {
            // a press outside the open menu closes it and reaches nothing underneath
            return self.close_menu();
        }
        let Some(layout) = &self.layout else {
            return Ok(());
        };
        match event {
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } if self.sheet.is_some() => {
                let sheet = self.sheet.as_mut().expect("checked by the guard");
                let l = sheet.layout(&layout.content);
                let outcome = sheet.press(
                    &l,
                    Point {
                        x: position.x,
                        y: position.y,
                    },
                );
                self.sheet_outcome(outcome)?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                let p = Point {
                    x: position.x,
                    y: position.y,
                };
                match layout.hit(p) {
                    Hit::Back => {
                        if self.history.back().is_some() {
                            self.show_current()?;
                        }
                    }
                    Hit::Forward => {
                        if self.history.forward().is_some() {
                            self.show_current()?;
                        }
                    }
                    Hit::Search => {
                        let area = search_text_area(&layout.search, !self.search.text().is_empty());
                        self.set_focus(Focus::Search)?;
                        let index = self.search.field().index_at(
                            &self.search_offsets,
                            p.x - area.x + self.search.field().scroll(),
                        );
                        let extend = self.conn.modifiers().shift;
                        self.search.edit(|f| f.press(index, extend, Instant::now()));
                        self.window.redraw();
                    }
                    Hit::Row(pane) => {
                        // the selection fill shows on the press; the history push waits for the release (approximate)
                        self.pressed_row = Some(pane);
                        self.showing = pane;
                        self.set_focus(Focus::Sidebar)?;
                    }
                    Hit::Titlebar => self.press_titlebar()?,
                    Hit::Content => {
                        self.set_focus(Focus::Pane)?;
                        self.content_press(p)?;
                    }
                    Hit::Sidebar => {}
                }
            }
            PointerEvent::Motion(position) | PointerEvent::Enter(position)
                if self.sheet.is_some() =>
            {
                let sheet = self.sheet.as_mut().expect("checked by the guard");
                let l = sheet.layout(&layout.content);
                let p = Point {
                    x: position.x,
                    y: position.y,
                };
                if sheet.hover(&l, Some(p)) {
                    self.window.redraw();
                }
            }
            PointerEvent::Leave if self.sheet.is_some() => {
                let sheet = self.sheet.as_mut().expect("checked by the guard");
                let l = sheet.layout(&layout.content);
                if sheet.hover(&l, None) {
                    self.window.redraw();
                }
            }
            PointerEvent::Scroll { dy, position, .. } if self.sheet.is_some() => {
                let sheet = self.sheet.as_mut().expect("checked by the guard");
                let l = sheet.layout(&layout.content);
                let p = Point {
                    x: position.x,
                    y: position.y,
                };
                if sheet.scroll(&l, p, dy) {
                    self.window.redraw();
                }
            }
            PointerEvent::Motion(position) if self.search.field().is_dragging() => {
                let area = search_text_area(&layout.search, !self.search.text().is_empty());
                let index = self.search.field().index_at(
                    &self.search_offsets,
                    position.x - area.x + self.search.field().scroll(),
                );
                self.search.edit(|f| f.drag_to(index));
                self.window.redraw();
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } if self.search.field().is_dragging() => self.search.edit(TextBuffer::release),
            PointerEvent::Motion(position) if self.level.is_some() => {
                let (section, row, _, mut writer) =
                    self.level.take().expect("checked by the guard");
                let frame = self.form[section].rows[row]
                    .control
                    .offset(0.0, -self.scroll);
                let v = controls::level_value(&frame, position.x);
                let write = writer.moved(v, self.now());
                self.level = Some((section, row, v, writer));
                if let Some(v) = write {
                    self.level_write(section, row, v)?;
                }
                self.window.redraw();
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } if self.level.is_some() => {
                let (section, row, shown, mut writer) =
                    self.level.take().expect("checked by the guard");
                let v = writer.released(shown);
                self.level_write(section, row, v)?;
            }
            PointerEvent::Motion(position) => {
                if let Some(mut d) = self.drag {
                    let (Control::Slider { stops, .. }, frame) = self.control_at(d.section, d.row)
                    else {
                        unreachable!("a drag is on a slider")
                    };
                    d.shown = controls::drag(stops, &frame, position.x);
                    self.drag = Some(d);
                    let area = row_area(&self.form[d.section].rows[d.row], self.scroll);
                    self.window
                        .redraw_rect(PopupRect::new(area.x, area.y, area.w, area.h));
                }
            }
            PointerEvent::Scroll { dy, position, .. }
                if layout.content.contains(Point {
                    x: position.x,
                    y: position.y,
                }) =>
            {
                self.scroll =
                    (self.scroll + dy).clamp(0.0, (self.form_height - layout.content.h).max(0.0));
                for a in scroll_areas(layout) {
                    self.window.redraw_rect(PopupRect::new(a.x, a.y, a.w, a.h));
                }
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } if self.drag.is_some() => {
                let d = self.drag.take().expect("checked by the guard");
                let (Control::Slider { stops, .. }, _) = self.control_at(d.section, d.row) else {
                    unreachable!("a drag is on a slider")
                };
                let id = self.sections()[d.section].rows[d.row].id;
                self.commit(id, controls::release(stops, d.shown))?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                position,
            } => {
                if let Some(pane) = self.pressed_row.take() {
                    match layout.hit(Point {
                        x: position.x,
                        y: position.y,
                    }) {
                        Hit::Row(over) if over == pane => self.go(pane)?,
                        // released elsewhere: the selection goes back to the current pane
                        _ => self.show_current()?,
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A press on the toolbar strip moves the window; a second one within the double-click interval zooms it.
    fn press_titlebar(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        // the double-click interval (default 0.5 s, the pointer settings of)
        let double = self
            .last_titlebar_press
            .is_some_and(|last| now.duration_since(last).as_secs_f64() <= 0.5);
        self.last_titlebar_press = (!double).then_some(now);
        if double {
            self.conn
                .set_maximized(self.window.toplevel, !self.window.maximized)?;
        } else {
            self.conn.start_move(self.window.toplevel)?;
        }
        Ok(())
    }

    fn set_focus(&mut self, focus: Focus) -> Result<()> {
        if self.focus != focus {
            if self.focus == Focus::Search {
                self.search.edit(TextBuffer::blur);
            }
            if focus == Focus::Search {
                // the keyboard brings the whole text selected; a click places the caret after this
                self.search.edit(TextBuffer::select_all);
            }
            self.focus = focus;
            self.window.redraw();
        }
        Ok(())
    }

    /// The modifiers of a key as the window's chords read them: the command key is `logo` whatever key the profile
    /// makes it (Ctrl under `classic-pc`).
    fn window_mods(&self, mods: KeyModifiers) -> KeyModifiers {
        let m = keys::command_mods(&self.keymap, mods.shift, mods.ctrl, mods.alt, mods.logo);
        KeyModifiers {
            logo: m.logo,
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            ..mods
        }
    }

    fn key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        let physical = mods;
        let mods = self.window_mods(mods);
        if self.menu.is_some() {
            return self.menu_key(code, mods, character);
        }
        if let Some(sheet) = &mut self.sheet {
            let key = match code {
                keys::code::ESCAPE => SheetKey::Escape,
                keys::code::ENTER | keys::code::KEYPAD_ENTER if mods.logo => SheetKey::Choose,
                keys::code::ENTER | keys::code::KEYPAD_ENTER => SheetKey::Return,
                keys::code::BACKSPACE => SheetKey::Backspace,
                keys::code::UP if mods.logo => SheetKey::Parent,
                keys::code::UP => SheetKey::Up,
                keys::code::DOWN => SheetKey::Down,
                keys::code::HOME => SheetKey::Home,
                keys::code::END => SheetKey::End,
                _ if mods.logo => return Ok(()),
                _ => match character.filter(|c| !c.is_control()) {
                    Some(c) => SheetKey::Type(c),
                    None => return Ok(()),
                },
            };
            let outcome = sheet.key_at(key, self.start.elapsed().as_secs_f64());
            return self.sheet_outcome(outcome);
        }
        let mods = Mods {
            logo: mods.logo,
            shift: mods.shift,
            ctrl: mods.ctrl,
            alt: mods.alt,
        };
        match keys::route(self.focus, code, mods, character) {
            Action::Back => {
                if self.history.back().is_some() {
                    self.show_current()?;
                }
            }
            Action::Forward => {
                if self.history.forward().is_some() {
                    self.show_current()?;
                }
            }
            Action::FocusSearch => self.set_focus(Focus::Search)?,
            Action::ShowAll => self.go(self.rows()[0])?,
            Action::Previous => self.step_row(-1)?,
            Action::Next => self.step_row(1)?,
            Action::First => self.go(self.rows()[0])?,
            Action::Last => self.go(*self.rows().last().expect("the sidebar has rows"))?,
            Action::FocusNext => self.set_focus(keys::cycle(self.focus, true))?,
            Action::FocusPrevious => self.set_focus(keys::cycle(self.focus, false))?,
            Action::FocusSidebar => self.set_focus(Focus::Sidebar)?,
            Action::Search(k) => self.search_key(
                k,
                KeyInput::new(
                    code,
                    physical.shift,
                    physical.ctrl,
                    physical.alt,
                    physical.logo,
                    character,
                ),
            )?,
            Action::None => {}
        }
        Ok(())
    }

    /// The sidebar's rows, top to bottom.
    fn rows(&self) -> Vec<PaneId> {
        self.groups.iter().flatten().copied().collect()
    }

    /// The row `step` rows from the current one, within the sidebar.
    fn step_row(&mut self, step: isize) -> Result<()> {
        let rows = self.rows();
        let i = rows
            .iter()
            .position(|p| *p == self.history.current())
            .expect("the current pane is a row");
        let to = i.saturating_add_signed(step).min(rows.len() - 1);
        self.go(rows[to])
    }

    /// What the menu bar's items depend on now.
    fn current_menu_state(&self) -> MenuState {
        MenuState {
            search_focused: self.focus == Focus::Search,
            has_selection: !self.search.field().selected_text().is_empty(),
            full_screen: self.window.fullscreen,
        }
    }

    /// Publish the window's menus to the desktop's menu bar.
    fn publish_menus(&mut self) {
        let state = self.current_menu_state();
        self.menus = WindowMenus::publish(
            &self.conn,
            &self.effects,
            self.window.toplevel.surface(),
            APP_ID,
            Box::new(self.keymap.profile().clone()),
            &menu_bar::menu_bar(&state),
        );
        if let Some(menus) = &self.menus {
            for (action, on) in menu_bar::enabled(&state) {
                menus.set_enabled(&action.name(), on);
            }
            self.menu_state = Some(state);
        }
    }

    /// Bring the published menus up to date and carry out the items chosen in the menu bar.
    fn sync_menus(&mut self) -> Result<()> {
        if self.menus.is_none() {
            return Ok(());
        }
        let state = self.current_menu_state();
        let last = self.menu_state.unwrap_or_default();
        let menus = self.menus.as_mut().expect("checked above");
        if state != last {
            if state.full_screen != last.full_screen {
                menus.update(&menu_bar::menu_bar(&state));
            }
            let before = menu_bar::enabled(&last);
            for (i, (action, on)) in menu_bar::enabled(&state).into_iter().enumerate() {
                if before[i].1 != on {
                    menus.set_enabled(&action.name(), on);
                }
            }
            self.menu_state = Some(state);
        }
        let chosen: Vec<MenuAction> = menus
            .activations()
            .filter_map(|activation| MenuAction::from_name(&activation.action))
            .collect();
        for action in chosen {
            self.menu_action(action)?;
        }
        Ok(())
    }

    /// Carry out an item of the menu bar: the editing items act on the search field.
    fn menu_action(&mut self, action: MenuAction) -> Result<()> {
        match action {
            MenuAction::Quit => self.closing = true,
            MenuAction::Find => self.set_focus(Focus::Search)?,
            MenuAction::FullScreen => {
                let fullscreen = !self.window.fullscreen;
                self.conn.set_fullscreen(self.window.toplevel, fullscreen)?;
            }
            MenuAction::Undo
            | MenuAction::Redo
            | MenuAction::Cut
            | MenuAction::Copy
            | MenuAction::Paste
            | MenuAction::SelectAll => {
                if self.focus != Focus::Search {
                    return Ok(());
                }
                let selected = self.search.field().selected_text().to_owned();
                match action {
                    MenuAction::Undo => {
                        self.search.edit(TextBuffer::undo);
                    }
                    MenuAction::Redo => {
                        self.search.edit(TextBuffer::redo);
                    }
                    MenuAction::Cut | MenuAction::Copy if !selected.is_empty() => {
                        if let Err(e) = self.clipboard.set_text(&mut self.conn, selected) {
                            eprintln!("cupertino-settings: copy: {e}");
                        }
                        if action == MenuAction::Cut {
                            self.search.edit(|f| f.paste(""));
                        }
                    }
                    MenuAction::Paste => match self.clipboard.text(&mut self.conn) {
                        Ok(Some(text)) => self.search.edit(|f| f.paste(&text)),
                        Ok(None) => {}
                        Err(e) => eprintln!("cupertino-settings: paste: {e}"),
                    },
                    MenuAction::SelectAll => self.search.edit(TextBuffer::select_all),
                    _ => {}
                }
                self.sync_results()?;
                self.window.redraw();
            }
        }
        Ok(())
    }

    fn search_key(&mut self, k: SearchKey, input: KeyInput) -> Result<()> {
        match k {
            SearchKey::Edit => {
                let keymap = &mut self.keymap;
                let response = self.search.edit(|f| f.key(keymap, input, Instant::now()));
                match response {
                    Response::Ignored => return Ok(()),
                    Response::Moved | Response::Edited => {}
                    Response::Copy(text) | Response::Cut(text) => {
                        if let Err(e) = self.clipboard.set_text(&mut self.conn, text) {
                            eprintln!("cupertino-settings: copy: {e}");
                        }
                    }
                    Response::Paste => match self.clipboard.text(&mut self.conn) {
                        Ok(Some(text)) => self.search.edit(|f| f.paste(&text)),
                        Ok(None) => {}
                        Err(e) => eprintln!("cupertino-settings: paste: {e}"),
                    },
                }
            }
            SearchKey::Up => self.search.step(-1),
            SearchKey::Down => self.search.step(1),
            SearchKey::Clear => self.search.clear(),
            SearchKey::Choose => {
                if let Some(hit) = self.search.choose() {
                    self.go(hit.pane)?;
                }
            }
        }
        self.sync_results()?;
        self.window.redraw();
        Ok(())
    }

    /// Open, resize or close the results popup to follow the field.
    fn sync_results(&mut self) -> Result<()> {
        if !self.search.is_open() {
            return self.close_results();
        }
        let Some(layout) = &self.layout else {
            return Ok(());
        };
        let size = popup::size(
            WIDTH_FACTOR,
            self.search.results().len(),
            RowSize::default(),
        );
        // pinned under the search field, left edges aligned (approximate, §5)
        let rect = PopupRect::new(layout.search.x, layout.search.bottom(), size.0, size.1);
        match &mut self.results {
            Some(r) if r.size == size => r.presenter.request_redraw(),
            Some(r) => {
                r.size = size;
                self.conn
                    .reposition_popup(r.popup, &placement(&rect, true)?)?;
                r.backdrop
                    .set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), popup::RADIUS)?;
            }
            None => {
                let popup = self
                    .conn
                    .popup(self.window.toplevel.surface(), &placement(&rect, true)?)?;
                let backdrop = self.effects.backdrop(&self.conn, popup.surface())?;
                backdrop.set_material(POPUP_MATERIAL);
                backdrop.set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), popup::RADIUS)?;
                self.results = Some(Results {
                    popup,
                    presenter: ShmPresenter::new(popup.surface()),
                    backdrop,
                    size,
                    hovered: None,
                });
            }
        }
        Ok(())
    }

    fn close_results(&mut self) -> Result<()> {
        if let Some(r) = self.results.take() {
            drop(r.backdrop);
            self.conn.destroy(r.popup.surface())?;
        }
        Ok(())
    }

    fn results_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let Some(r) = &mut self.results else {
            return Ok(());
        };
        let rows = popup::rows(self.search.results().len(), r.size.0, RowSize::default());
        match event {
            PointerEvent::Motion(p) | PointerEvent::Enter(p) => {
                let hovered = popup_hit(&rows, self.search.results(), p.x, p.y);
                if hovered != r.hovered {
                    r.hovered = hovered;
                    r.presenter.request_redraw();
                }
            }
            PointerEvent::Leave => {
                r.hovered = None;
                r.presenter.request_redraw();
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                if let Some(i) = popup_hit(&rows, self.search.results(), position.x, position.y) {
                    self.search.highlight(i);
                    if let Some(hit) = self.search.choose() {
                        self.close_results()?;
                        self.go(hit.pane)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ---- the pane's controls ------------------------------------------------------------------------------------

    /// The current pane's sections with the values in effect and the errors of refused changes.
    fn sections(&self) -> Vec<Section> {
        let mut sections = panes::sections(
            self.history.current(),
            &self.settings,
            Session {
                dark: self.dark,
                display: self.display,
            },
            &self.live,
        );
        for row in sections.iter_mut().flat_map(|s| &mut s.rows) {
            if let Some((_, e)) = self.errors.iter().find(|(id, _)| *id == row.id) {
                row.error = Some(e.clone());
            }
        }
        sections
    }

    /// The control of row `row` of section `section` and its frame on the window as last drawn.
    fn control_at(&self, section: usize, row: usize) -> (Control, cupertino_settings::Rect) {
        let control = self.sections()[section].rows[row].control.clone();
        (
            control,
            self.form[section].rows[row]
                .control
                .offset(0.0, -self.scroll),
        )
    }

    /// A press in the content column at `p`: on a control, what it does (§7.2).
    fn content_press(&mut self, p: Point) -> Result<()> {
        let sections = self.sections();
        for (si, (section, l)) in sections.iter().zip(&self.form).enumerate() {
            for (ri, (row, rl)) in section.rows.iter().zip(&l.rows).enumerate() {
                let frame = rl.control.offset(0.0, -self.scroll);
                if row.disabled.is_some() {
                    continue;
                }
                match controls::press(&row.control, &frame, p) {
                    None => continue,
                    Some(Press::Level(v)) => {
                        // a zero-distance drag: the press writes at once
                        let mut writer = controls::LevelWriter::default();
                        let write = writer.moved(v, self.now());
                        self.level = Some((si, ri, v, writer));
                        if let Some(v) = write {
                            self.level_write(si, ri, v)?;
                        }
                        self.window.redraw();
                        return Ok(());
                    }
                    Some(Press::Commit(value)) => return self.commit(row.id, value),
                    Some(Press::Drag(shown)) => {
                        self.drag = Some(SliderDrag {
                            section: si,
                            row: ri,
                            shown,
                        });
                        self.window.redraw();
                        return Ok(());
                    }
                    Some(Press::OpenMenu) => {
                        let Control::Popup { choices, selected } = &row.control else {
                            unreachable!("only a pop-up button opens a menu")
                        };
                        return self.open_menu(row.id, choices.clone(), *selected, &frame);
                    }
                }
            }
        }
        Ok(())
    }

    /// Commit `value` of control `id`: change the settings and do what that asks; a refusal restores the control and
    /// shows the error under its row.
    fn commit(&mut self, id: ControlId, value: Value) -> Result<()> {
        self.errors.retain(|(e, _)| *e != id);
        // read-modify-write: the files as they are now, so the store changes the user's key only and puts back no
        // value another writer changed since the last read
        if let Err(e) = self.settings.reload(&self.paths) {
            eprintln!("cupertino-settings: the settings could not be read: {e}");
            self.errors
                .push((id, "The settings could not be read.".to_string()));
            self.window.redraw();
            return Ok(());
        }
        self.sync_accessibility();
        let before = self.settings.clone();
        let effects = panes::change(id, value, &mut self.settings, &self.live);
        // restoring `before` on a failure undoes the whole commit only while a commit has one effect
        assert!(effects.len() <= 1, "{id:?} made {} effects", effects.len());
        for effect in effects {
            let outcome = match effect {
                Effect::Service(Call::Join {
                    ssid,
                    passphrase: None,
                }) => {
                    let name = self
                        .live
                        .wifi
                        .ready()
                        .and_then(|w| w.networks.iter().find(|n| n.ssid == ssid))
                        .map_or_else(|| display_name(&ssid), |n| n.name.clone());
                    self.sheet = Some(Sheet::passphrase(&ssid, &name));
                    Ok(())
                }
                Effect::Service(Call::Remove {
                    id,
                    confirmed: false,
                }) => {
                    let name = self
                        .live
                        .bluetooth
                        .ready()
                        .and_then(|b| b.devices.iter().find(|d| d.id == id))
                        .map_or(id.clone(), |d| d.title().to_string());
                    self.sheet = Some(Sheet::forget(&id, &name));
                    Ok(())
                }
                Effect::Service(call) => self.service_call(&call),
                Effect::AskPicture { folder } => {
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                    let start = home
                        .as_ref()
                        .map(|h| h.join("Pictures"))
                        .filter(|p| p.is_dir())
                        .or(home)
                        .unwrap_or_else(|| std::path::PathBuf::from("/"));
                    let mode = if folder { Mode::Folder } else { Mode::Photo };
                    self.sheet = Some(Sheet::picker_pending(&start, mode));
                    self.drive_picker();
                    Ok(())
                }
                Effect::AskResolution { key, name } => {
                    self.sheet = Some(Sheet::resolution(&key, &name));
                    Ok(())
                }
                Effect::ConfirmUnallow { program, name } => {
                    self.sheet = Some(Sheet::unallow(&program, &name));
                    Ok(())
                }
                Effect::AskClipboardApp { program: true } => {
                    self.sheet = Some(Sheet::picker_pending(
                        std::path::Path::new(PROGRAM_CHOOSER_START),
                        Mode::Program,
                    ));
                    self.drive_picker();
                    Ok(())
                }
                Effect::AskClipboardApp { program: false } => {
                    let apps = cupertino_settings::clipboard::apps(
                        &self.live.apps,
                        &self.settings.clipboard,
                        sidebar::standard_compare,
                    );
                    self.sheet = Some(Sheet::apps(apps));
                    Ok(())
                }
                Effect::Refuse(reason) => Err(reason),
                Effect::Store(file) => {
                    let stored = self
                        .settings
                        .store(&self.paths, file)
                        .map_err(|e| e.to_string());
                    if file == File::DataControl && stored.is_ok() {
                        self.settings.reload_clipboard(&self.paths);
                    }
                    stored
                }
                Effect::SetAppearance { dark } => self.set_appearance(dark),
                Effect::SetAccessibility { key, value } => self.set_accessibility(key, &value),
                Effect::SetSystemAccessibility => self.save_login_screen(),
            };
            if let Err(e) = outcome {
                eprintln!("cupertino-settings: {e}");
                self.settings = before;
                self.errors.push((id, e));
                break;
            }
        }
        self.window.redraw();
        Ok(())
    }

    /// Make `call` on its service; a Join waits for the device until it connects or fails. The readings are refreshed
    /// after it, so the control shows the service's state, not the asked one.
    fn service_call(&mut self, call: &Call) -> std::result::Result<(), String> {
        if let Call::Pair(id) = call {
            let notify = self.notify.clone();
            return self
                .services
                .pair(id, move |result| notify(Background::Paired(result)));
        }
        let result = self.services.call(call);
        if let (Ok(()), Call::Join { ssid, .. }) = (&result, call) {
            if let cupertino_settings::live::Reading::Ready(w) = &mut self.live.wifi {
                w.joining = Some(ssid.clone());
                w.failure = None;
            }
        }
        let now = self.now();
        self.refresh(now).map_err(|e| e.to_string())?;
        result
    }

    /// Ask sessiond whether the Login Screen section shows at this opening of the Accessibility pane: it stays hidden
    /// until the answer, and the refusal shown under it at the last opening goes.
    fn ask_login_screen(&mut self) {
        self.live.login_screen = None;
        self.errors.retain(|(id, _)| *id != ControlId::LoginScreen);
        self.login_opening += 1;
        let opening = self.login_opening;
        let notify = self.notify.clone();
        // a failure is logged and leaves the section hidden
        let _ = self.services.can_set_system_accessibility(move |allowed| {
            notify(Background::LoginScreen { opening, allowed })
        });
    }

    /// Use Mine: send the person's allow-listed options to sessiond; the row shows "Saving…" until the reply.
    fn save_login_screen(&mut self) -> std::result::Result<(), String> {
        let session = Session {
            dark: self.dark,
            display: self.display,
        };
        let options = panes::login_screen_options(session, &self.settings);
        let notify = self.notify.clone();
        self.services
            .set_system_accessibility(options, move |result| {
                notify(Background::LoginScreenSaved(result))
            })?;
        self.login_saving = true;
        if let Some(login) = &mut self.live.login_screen {
            login.saving = true;
        }
        Ok(())
    }

    /// What a sheet's key or press decided.
    fn sheet_outcome(&mut self, outcome: Outcome) -> Result<()> {
        match outcome {
            Outcome::Stay => {}
            Outcome::Cancel => {
                self.sheet = None;
                self.agent_answer = None;
            }
            Outcome::Answer(answer) => {
                self.sheet = None;
                if let Some((_, sender)) = self.agent_answer.take() {
                    // the daemon may have ended the pairing meanwhile: then nobody waits for the answer
                    let _ = sender.try_send(answer);
                }
            }
            Outcome::Resolution { key, dpi } => {
                self.sheet = None;
                if let Err(e) = self.set_mouse_dpi(&key, dpi) {
                    eprintln!("cupertino-settings: {e}");
                }
            }
            Outcome::Unallow(program) => {
                self.sheet = None;
                if let Err(e) = self.settings.unallow_clipboard(&self.paths, &program) {
                    eprintln!("cupertino-settings: {e}");
                    self.errors.push((ControlId::ClipboardAdd, e));
                }
            }
            // the sheet stays with the reason until the program is allowed
            Outcome::Program(program) => match self.settings.allow_clipboard(&self.paths, &program)
            {
                Ok(()) => self.sheet = None,
                Err(e) => {
                    if let Some(sheet) = &mut self.sheet {
                        sheet.failed(&e);
                    }
                }
            },
            Outcome::Picked { path, folder } => {
                self.sheet = None;
                let added = if folder {
                    self.settings.add_wallpaper_folder(&self.paths, &path)
                } else {
                    self.settings.set_wallpaper(&self.paths, &path)
                };
                match added {
                    Ok(()) => self.load_pictures(),
                    Err(e) => eprintln!("cupertino-settings: {e}"),
                }
            }
            Outcome::Do(call) => match self.service_call(&call) {
                Ok(()) => self.sheet = None,
                // the passphrase sheet stays with the error (§7.2); a failed removal closes it and shows the error
                Err(e) => match &mut self.sheet {
                    Some(sheet) if matches!(call, Call::Join { .. }) => sheet.failed(&e),
                    _ => {
                        self.sheet = None;
                        eprintln!("cupertino-settings: {e}");
                        if let cupertino_settings::live::Reading::Ready(b) =
                            &mut self.live.bluetooth
                        {
                            b.failure = Some(e);
                        }
                    }
                },
            },
        }
        self.window.redraw();
        Ok(())
    }

    /// List the pictures of the packs and of the added folders, and start the thumbnails of those without one.
    fn load_pictures(&mut self) {
        let folders: Vec<std::path::PathBuf> = self
            .settings
            .compositor
            .wallpaper_folders
            .iter()
            .flatten()
            .map(std::path::PathBuf::from)
            .collect();
        self.live.pictures = Pictures {
            bundled: Vec::new(),
            folders: folders.iter().map(|f| (f.clone(), Vec::new())).collect(),
            default: self.assets.wallpaper_path(false),
            loading: (0..=folders.len()).collect(),
        };
        let current: Vec<std::path::PathBuf> = self
            .settings
            .compositor
            .wallpaper
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        self.ask_thumbnails(current);
        // each listing on a thread of its own: a folder on a hung mount shows "Loading…" and holds nothing else up
        self.start_listing(ListingKey::Packs, None);
        for folder in folders {
            self.start_listing(ListingKey::Pictures(folder), None);
        }
    }

    /// Start the read of `key` on a thread unless one is running (a folder on a dead mount keeps one thread, however
    /// often it is visited); it is read once more when the running one is answered if it was asked for meanwhile.
    fn start_listing(&mut self, key: ListingKey, request: Option<cupertino_filepicker::Request>) {
        if let Some(request) = self.listings.ask(key.clone(), request) {
            self.run_listing(key, request);
        }
    }

    /// Run the read of `key` on a thread; its answer comes back as a [`Background`] event.
    fn run_listing(&mut self, key: ListingKey, request: Option<cupertino_filepicker::Request>) {
        let notify = self.notify.clone();
        let spawned_key = key.clone();
        let job: Box<dyn FnOnce() -> Background + Send> = match key.clone() {
            ListingKey::Packs => {
                let assets = self.assets.clone();
                Box::new(move || Background::Pictures {
                    files: assets.wallpaper_files(),
                    key,
                })
            }
            ListingKey::Pictures(folder) => Box::new(move || Background::Pictures {
                files: cupertino_assets::pictures_in(&folder),
                key,
            }),
            ListingKey::Chooser(..) => {
                let Some(request) = request else {
                    self.listings.abandon(&key);
                    return;
                };
                Box::new(move || Background::Listing(request.run()))
            }
        };
        let spawned = std::thread::Builder::new()
            .name("settings-listing".into())
            .spawn(move || notify(job()));
        if let Err(e) = spawned {
            eprintln!("cupertino-settings: cannot list a folder: {e}");
            self.listings.abandon(&spawned_key);
        }
    }

    /// Make the thumbnails of `images` that have none and were not asked for yet, one after the other on a thread.
    fn ask_thumbnails(&mut self, images: Vec<std::path::PathBuf>) {
        let wanted: Vec<std::path::PathBuf> = images
            .into_iter()
            .filter(|image| {
                !self.painter.has_thumbnail(image) && self.thumbnails_asked.insert(image.clone())
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let notify = self.notify.clone();
        let spawned = std::thread::Builder::new()
            .name("wallpaper-thumbnails".into())
            .spawn(move || {
                let cache = cupertino_thumbnails::Thumbnailer::from_env("cupertino-settings");
                for image in wanted {
                    match cache.get_or_create(&image, cupertino_thumbnails::ThumbSize::Large) {
                        Ok(thumbnail) => notify(Background::Thumbnail { image, thumbnail }),
                        Err(e) => {
                            eprintln!(
                                "cupertino-settings: no thumbnail of {}: {e}",
                                image.display()
                            )
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("cupertino-settings: cannot start the thumbnails: {e}");
        }
    }

    /// Hand the chooser's next listing to a thread; its answer comes back as [`Background::Listing`].
    fn drive_picker(&mut self) {
        let Some(request) = self.sheet.as_mut().and_then(Sheet::picker_request) else {
            return;
        };
        let (dir, hidden, mode) = request.key();
        let key = ListingKey::Chooser(dir, hidden, mode);
        self.start_listing(key, Some(request));
    }

    /// Store the typed resolution of the mouse `key`; the server applies the file.
    fn set_mouse_dpi(&mut self, key: &str, dpi: u32) -> std::result::Result<(), String> {
        self.settings.set_mouse_dpi(&self.paths, key, dpi)
    }

    /// Something from a bus thread: a pane to show, a question of the pairing agent, the end of a pairing.
    fn background_event(&mut self, event: Background) -> Result<()> {
        match event {
            Background::ShowPane(pane, token) => {
                self.go(pane)?;
                if !token.is_empty() {
                    unless_unsupported(
                        self.conn.activate_with_token(self.window.toplevel, &token),
                    )?;
                }
            }
            Background::Agent(AgentEvent::Prompt {
                id,
                device,
                prompt,
                answer,
            }) => {
                // a new question replaces an open one: its pairing is over (the daemon pairs one device at a time)
                self.sheet = Some(Sheet::pairing(id, &device, prompt));
                self.agent_answer = answer.map(|a| (id, a));
                self.window.redraw();
            }
            Background::Agent(AgentEvent::Cancel(id)) => {
                if self.sheet.as_ref().and_then(Sheet::pairing_id) == Some(id) {
                    self.sheet = None;
                    self.agent_answer = None;
                    self.window.redraw();
                }
            }
            Background::Pictures { key, files } => {
                if let Some(request) = self.listings.answered(&key) {
                    self.run_listing(key.clone(), request);
                }
                let images: Vec<std::path::PathBuf> =
                    files.iter().map(|f| f.light.clone()).collect();
                let pictures = &mut self.live.pictures;
                // the pane may list other folders by now: the answer goes to the one it is for, if still listed
                let group = match &key {
                    ListingKey::Packs => Some(0),
                    ListingKey::Pictures(folder) => pictures
                        .folders
                        .iter()
                        .position(|(f, _)| f == folder)
                        .map(|i| i + 1),
                    ListingKey::Chooser(..) => None,
                };
                if let Some(group) = group {
                    pictures.loading.remove(&group);
                    match group {
                        0 => pictures.bundled = files,
                        n => pictures.folders[n - 1].1 = files,
                    }
                    self.ask_thumbnails(images);
                    self.window.redraw();
                }
            }
            Background::Listing(listing) => {
                let (dir, hidden, mode) = listing.key();
                let key = ListingKey::Chooser(dir, hidden, mode);
                if let Some(request) = self.listings.answered(&key) {
                    self.run_listing(key, request);
                }
                if let Some(sheet) = self.sheet.as_mut() {
                    sheet.apply_listing(listing);
                    self.window.redraw();
                }
            }
            Background::Thumbnail { image, thumbnail } => {
                if self.painter.set_thumbnail(&image, &thumbnail) {
                    self.window.redraw();
                }
            }
            Background::LoginScreen { opening, allowed } => {
                if opening == self.login_opening {
                    self.live.login_screen = allowed.then(|| LoginScreen {
                        current: system_accessibility(std::path::Path::new(SYSTEM_ACCESSIBILITY)),
                        saving: self.login_saving,
                    });
                    self.window.redraw();
                }
            }
            Background::LoginScreenSaved(result) => {
                self.login_saving = false;
                if let Some(login) = &mut self.live.login_screen {
                    login.saving = false;
                    login.current =
                        system_accessibility(std::path::Path::new(SYSTEM_ACCESSIBILITY));
                }
                if let Err(e) = result {
                    self.errors.push((ControlId::LoginScreen, e));
                }
                self.window.redraw();
            }
            Background::Files(WatchEvent::Changed(_)) => {
                if let Err(e) = self.settings.reload(&self.paths) {
                    eprintln!("cupertino-settings: the settings could not be read again: {e}");
                }
                self.sync_accessibility();
                self.window.redraw();
            }
            Background::Files(WatchEvent::Error(e)) => {
                eprintln!("cupertino-settings: watching the settings files: {e}")
            }
            Background::Paired(result) => {
                if let (Err(e), cupertino_settings::live::Reading::Ready(b)) =
                    (result, &mut self.live.bluetooth)
                {
                    b.failure = Some(e);
                }
                let now = self.now();
                self.refresh(now)?;
                self.window.redraw();
            }
        }
        Ok(())
    }

    /// Write the level slider of `section`/`row` at `v`.
    fn level_write(&mut self, section: usize, row: usize, v: f64) -> Result<()> {
        let id = self.sections()[section].rows[row].id;
        for effect in panes::change(id, Value::Fraction(v), &mut self.settings, &self.live) {
            if let Effect::Service(call) = effect {
                if let Err(e) = self.services.call(&call) {
                    eprintln!("cupertino-settings: {e}");
                    self.errors.retain(|(e, _)| *e != id);
                    self.errors.push((id, e));
                }
            }
        }
        Ok(())
    }

    /// Ask the server for the appearance (`org.cupertino.Session1.SetAppearance`); the control shows it at once and the
    /// session's report replaces it.
    fn set_appearance(&mut self, dark: bool) -> std::result::Result<(), String> {
        let name = if dark { "dark" } else { "light" };
        let bus = self.bus()?;
        bus.call_method(
            Some(SESSION_NAME),
            SESSION_PATH,
            Some(SESSION_NAME),
            "SetAppearance",
            &(name,),
        )
        .map_err(|e| {
            format!(
                "The appearance could not be changed: {}",
                services::failure(&e)
            )
        })?;
        self.dark = dark;
        Ok(())
    }

    /// The session bus, connected on first use.
    fn bus(&mut self) -> std::result::Result<&zbus::blocking::Connection, String> {
        if self.bus.is_none() {
            self.bus = Some(zbus::blocking::Connection::session().map_err(|e| {
                services::log_once(&format!("the session bus: {e}"));
                "The session is not available.".to_string()
            })?);
        }
        Ok(self.bus.as_ref().expect("connected above"))
    }

    /// Ask the server to set one key of `accessibility.conf` (`org.cupertino.Session1.SetAccessibility`) and show what
    /// the server now holds; the display options themselves arrive from the session.
    fn set_accessibility(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        let bus = self.bus()?;
        bus.call_method(
            Some(SESSION_NAME),
            SESSION_PATH,
            Some(SESSION_NAME),
            "SetAccessibility",
            &(key, value),
        )
        .map_err(|e| {
            format!(
                "The setting could not be changed: {}",
                services::failure(&e)
            )
        })?;
        self.sync_accessibility();
        Ok(())
    }

    /// The accessibility settings as the server holds them (`org.cupertino.Session1.Accessibility`): the server is
    /// their only writer, and it keeps them in memory only when it has no file to store them in, so the file is not
    /// the truth. Without the property the file's values stay.
    fn sync_accessibility(&mut self) {
        let problems = match self.services.accessibility() {
            Reading::Ready(held) => self.settings.apply_session_accessibility(&held),
            Reading::Unavailable(reason) => {
                vec![format!(
                    "the session's accessibility settings cannot be read: {reason}"
                )]
            }
            Reading::Unknown => Vec::new(),
        };
        for problem in problems {
            if self.reported.insert(problem.clone()) {
                eprintln!("cupertino-settings: {problem}");
            }
        }
    }

    /// Open the menu of a pop-up button in `frame` under it.
    fn open_menu(
        &mut self,
        id: ControlId,
        choices: Vec<String>,
        selected: usize,
        frame: &cupertino_settings::Rect,
    ) -> Result<()> {
        self.close_menu()?;
        let (_, size) = menu_rows(choices.len());
        let rect = PopupRect::new(frame.x, frame.bottom() + 2.0, size.0, size.1);
        let popup = self
            .conn
            .popup(self.window.toplevel.surface(), &placement(&rect, true)?)?;
        let backdrop = self.effects.backdrop(&self.conn, popup.surface())?;
        backdrop.set_material(POPUP_MATERIAL);
        backdrop.set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), 6.0)?;
        self.menu = Some(ChoiceMenu {
            popup,
            presenter: ShmPresenter::new(popup.surface()),
            backdrop,
            id,
            choices,
            selected,
            hovered: None,
            type_ahead: menu::TypeAhead::default(),
        });
        Ok(())
    }

    fn close_menu(&mut self) -> Result<()> {
        if let Some(m) = self.menu.take() {
            drop(m.backdrop);
            self.conn.destroy(m.popup.surface())?;
        }
        Ok(())
    }

    fn draw_menu(&mut self) -> Result<()> {
        let App {
            conn,
            painter,
            palette,
            menu,
            ..
        } = self;
        let Some(ChoiceMenu {
            presenter,
            choices,
            selected,
            hovered,
            ..
        }) = menu.as_mut()
        else {
            return Ok(());
        };
        presenter.draw(conn, None, |pm, scale| {
            painter.paint_menu(
                pm,
                scale.factor() as f32,
                choices,
                *selected,
                *hovered,
                palette,
            )
        })?;
        Ok(())
    }

    /// A key with a menu open: it moves, chooses or cancels, and reaches nothing underneath.
    fn menu_key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        let now = self.now();
        let Some(m) = &mut self.menu else {
            return Ok(());
        };
        let mods = Mods {
            logo: mods.logo,
            shift: mods.shift,
            ctrl: mods.ctrl,
            alt: mods.alt,
        };
        let from = m.hovered.unwrap_or(m.selected);
        match menu::press(
            &m.choices,
            from,
            &mut m.type_ahead,
            now,
            code,
            mods,
            character,
        ) {
            menu::Outcome::Highlight(i) => {
                m.hovered = Some(i);
                m.presenter.request_redraw();
            }
            menu::Outcome::Choose(i) => {
                let id = m.id;
                self.close_menu()?;
                self.commit(id, Value::Index(i))?;
            }
            menu::Outcome::Cancel => self.close_menu()?,
            menu::Outcome::Ignore => {}
        }
        Ok(())
    }

    fn menu_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let Some(m) = &mut self.menu else {
            return Ok(());
        };
        let (rows, _) = menu_rows(m.choices.len());
        let at = |x: f64, y: f64| rows.iter().position(|r| r.contains(Point { x, y }));
        match event {
            PointerEvent::Motion(p) | PointerEvent::Enter(p) => {
                let hovered = at(p.x, p.y);
                if hovered != m.hovered {
                    m.hovered = hovered;
                    m.presenter.request_redraw();
                }
            }
            PointerEvent::Leave => {
                m.hovered = None;
                m.presenter.request_redraw();
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                if let Some(i) = at(position.x, position.y) {
                    let id = m.id;
                    self.close_menu()?;
                    self.commit(id, Value::Index(i))?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// The x of `field`'s caret among `offsets` (one per [`TextBuffer::boundaries`]).
fn caret_x(field: &TextBuffer, offsets: &[f64]) -> f64 {
    field
        .boundaries()
        .iter()
        .position(|b| *b == field.caret())
        .and_then(|i| offsets.get(i))
        .copied()
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_or_missing_extent_in_a_configure_keeps_the_current_size() {
        assert_eq!(configured_size(None, (920, 436)), (920, 436));
        assert_eq!(configured_size(Some((0, 0)), (920, 436)), (920, 436));
        assert_eq!(configured_size(Some((1200, 0)), (920, 436)), (1200, 436));
        assert_eq!(configured_size(Some((640, 480)), (920, 436)), (640, 480));
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

    #[test]
    fn an_agent_prompt_at_a_full_queue_is_refused_without_waiting() {
        let (notify, _background) = background_channel(|| {});
        for _ in 0..BACKGROUND_QUEUE {
            notify(Background::Paired(Ok(())));
        }
        let (answer, reply) = async_channel::bounded(1);
        let agent = notify.clone();
        let (done, returned) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            agent(Background::Agent(AgentEvent::Prompt {
                id: 1,
                device: "Keyboard".into(),
                prompt: cupertino_settings::sheet::Prompt::Confirm {
                    passkey: Some(123456),
                },
                answer: Some(answer),
            }));
            done.send(()).unwrap();
        });
        returned
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a bus executor never waits for room");
        assert!(reply.recv_blocking().is_err(), "a dropped prompt refuses");
    }

    #[test]
    fn pane_requests_cannot_grow_the_background_queue_and_workers_wait_for_room() {
        let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = wakes.clone();
        let (notify, background) = background_channel(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        for _ in 0..BACKGROUND_QUEUE * 4 {
            notify(Background::ShowPane(PaneId::Wifi, String::new()));
        }
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            BACKGROUND_QUEUE
        );

        let worker = notify.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            worker(Background::Paired(Ok(())));
            done.send(()).unwrap();
        });
        assert!(finished
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());
        assert!(matches!(background.recv(), Ok(Background::ShowPane(..))));
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker's event is queued once there is room");
        let queued: Vec<_> = background.try_iter().collect();
        assert_eq!(queued.len(), BACKGROUND_QUEUE);
        assert!(matches!(queued.last(), Some(Background::Paired(Ok(())))));
    }
}
