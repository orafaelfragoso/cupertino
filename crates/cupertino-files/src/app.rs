//! Glue between the browser window model and the toolkit: one application window with the unified
//! title bar style (the surface is the whole window; the server draws the window control buttons over
//! it, the window moves and zooms from the toolbar strip), input routed to [`Browser`], frames drawn by
//! [`WindowPainter`] with the theme's file icons, files handed to their default applications (`mimeapps.list`).
//!
//! Keys (reference chords with Super as the primary modifier): Return and Super+Down open the
//! selection, Super+Up goes to the enclosing folder, Super+[ and Super+] go back and forward,
//! Super+1 and Super+2 switch to the icon and the list view, Super+A selects everything; arrows,
//! Home and End move the selection (Shift extends it), Tab and typing select by name. A double
//! click opens; Super+click toggles an item, Shift+click selects a range and Ctrl+click is the
//! secondary click (the context menu).
//!
//! A right click opens the context menu (`context_menu`): one popup per open level, navigable by pointer and by
//! the arrow keys, Return and Escape. Super+Backspace moves the selection to the trash. A click on the name of the
//! only selected item, or Rename in the menu, edits the name in place (Return commits, Escape cancels). Pressing on an
//! item and moving more than 5 pt drags the selection: onto a folder of the window or a place of the sidebar it
//! moves there (Alt copies, Super moves, copy across volumes); onto the trash place it goes to the trash.

use cupertino_apps::{launch, Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_assets::Assets;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_files::browser::{Activation, Browser};
use cupertino_files::context_menu::{
    common_apps, AppChoice, Command as MenuCommand, Key as MenuKey, Menu, MenuItem,
    Mods as MenuMods, Nav, Outcome,
};
use cupertino_files::menu_bar::{self, MenuAction, MenuState};
use cupertino_files::menu_spec::{self, Background, Subject};
use cupertino_files::metrics::{
    drag, marquee as marquee_metrics, menu as menu_metrics, timing, window as window_metrics,
};
use cupertino_files::name_edit::NameEdit;
use cupertino_files::open::{command_with, default_app_id, open_command, open_with};
use cupertino_files::operations;
use cupertino_files::places::{
    removable_holding, session_places, Place, PlaceKind, SessionPlaces, Volume,
};
use cupertino_files::recents;
use cupertino_files::rename::{RenameEvent, RenameTrigger};
use cupertino_files::render::{Palette, Renderer};
use cupertino_files::selection::Modifiers;
use cupertino_files::tags::{self, TagColor};
use cupertino_files::trash::Trash;
use cupertino_files::udisks::{Device, Report, Volumes};
use cupertino_files::window::{
    metrics, status_text, Bars, DropOutline, Frame, Hit, Move, SearchWidth, ToolbarItem, ViewMode,
    WindowPainter, WindowState, SEARCH_MAX_LEN,
};
use cupertino_files::{Entry, Kind, Point, Size, SortSpec};
use cupertino_geometry::Rect as PopupRect;
use cupertino_icons::document::FileIcons;
use cupertino_protocol_app::{AppEffects, AppEvent, Appearance as AppearanceObserver, Backdrop};
use cupertino_text_field::{KeyInput, Keymap, Response, TextField, TextFieldKeys};
use cupertino_theme::{Appearance, Theme, WindowCornerSize};
use cupertino_ui_menus::WindowMenus;
use cupertino_ui_wayland::clipboard::Clipboard;
use cupertino_ui_wayland::{copy_rects, popup, Damage, Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Connection, Decorations, Event, KeyboardEvent, Modifiers as KeyModifiers,
    PointerEvent, Popup, SurfaceId, TokenRequest, Toplevel, ToplevelConfig, WindowConfigure,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tiny_skia::{Color, Pixmap};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Identifier of the window for the server (`xdg_toplevel.set_app_id`).
const APP_ID: &str = "cupertino-files";

/// The material the server puts behind a context menu's popups.
const MENU_MATERIAL: &str = "Menu";

/// The material the server puts behind the sidebar (`Mica: Sidebar`, recipe of the design tokens).
const SIDEBAR_MATERIAL: &str = "Sidebar";

/// Two presses on the same item within this interval open it (default).
/// One frame of an animation (seconds).
const FRAME_SECONDS: f64 = 1.0 / 60.0;

const DOUBLE_CLICK: Duration =
    Duration::from_millis((timing::DOUBLE_CLICK_INTERVAL * 1000.0) as u64);

struct App {
    conn: Connection,
    effects: AppEffects,
    /// Reports the session's appearance and display options while it lives.
    _appearance: AppearanceObserver,
    /// The session's display options, as last reported.
    display: DisplayOptions,
    clipboard: Clipboard,
    window: Window,
    /// The sidebar's material, below the window's surface.
    sidebar: Backdrop,
    theme: Theme,
    palette: Palette,
    painter: WindowPainter,
    browser: Browser,
    /// The session's places, then a row per removable file system.
    places: Vec<Place>,
    standard_places: usize,
    /// The removable media; `None` without a system bus.
    volumes: Option<Volumes>,
    /// The removable file systems, in the order of their rows.
    devices: Vec<Device>,
    /// The block objects a mount or an eject is running for.
    busy: HashSet<zbus::zvariant::OwnedObjectPath>,
    home: PathBuf,
    mode: ViewMode,
    /// The path bar and status bar shown (hidden by default).
    bars: Bars,
    /// Space left on the folder's file system for the status bar, read on entering the folder and when its listing
    /// changes.
    available: Option<u64>,
    /// The toolbar search field (the browser shows the folder's items that match its text), and whether it has the
    /// keyboard.
    search: TextField,
    search_focused: bool,
    /// How wide the search field is now: it opens while focused or holding text.
    search_width: SearchWidth,
    /// The frame drawn last: clicks and arrow keys are resolved against it.
    frame: Option<Frame>,
    scroll: f64,
    /// Vertical scroll since the last frame, applied when it draws.
    pending_scroll: f64,
    /// The item of the last press that did not open, and when, for double clicks.
    last_press: Option<(usize, Instant)>,
    /// When the toolbar strip was last pressed, for the double click that zooms the window.
    last_titlebar_press: Option<Instant>,
    /// Files waiting for the activation token being fetched for them.
    pending: HashMap<TokenRequest, Launch>,
    start: Instant,
    /// The argv prefix for applications that run in a terminal (`--terminal`).
    terminal: Option<Vec<String>>,
    /// The open context menu.
    menu: Option<OpenMenu>,
    /// The name being edited in place, with the path of the item it renames.
    edit: Option<(NameEdit, PathBuf)>,
    /// The user's modifier profile and editing chords, for the name editor.
    keymap: Keymap,
    /// Whether the last frame showed the name editor's caret; a blink redraws when it changes.
    caret_drawn: bool,
    rename: RenameTrigger,
    /// A new folder to edit the name of once the listing shows it.
    edit_when_listed: Option<PathBuf>,
    /// The left button is down on an item.
    drag: Option<Drag>,
    /// The left button is down on empty space.
    marquee: Option<Marquee>,
    /// The menus published to the desktop's menu bar; `None` when the server or the session bus takes none.
    menus: Option<WindowMenus>,
    /// What the published menus were made from.
    menu_state: Option<MenuState>,
    /// The folder of the trash's items, where Put Back applies.
    trash_files: PathBuf,
    /// Close Window was chosen.
    closing: bool,
}

/// An item waiting for its activation token and what opens it.
struct Launch {
    path: PathBuf,
    is_dir: bool,
    /// Desktop entry id of the application chosen in Open With; `None` for the default application.
    with: Option<String>,
}

/// An open context menu with a popup surface (and its backdrop) per level.
struct OpenMenu {
    menu: Menu,
    levels: Vec<MenuLevel>,
}

/// The popup of one open level of a menu.
struct MenuLevel {
    popup: Popup,
    presenter: ShmPresenter,
    backdrop: Backdrop,
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

/// The presenters of the window and of the open menu's popups.
fn presenters<'a>(
    window: &'a mut Window,
    menu: &'a mut Option<OpenMenu>,
) -> impl Iterator<Item = &'a mut ShmPresenter> {
    std::iter::once(&mut window.presenter).chain(
        menu.iter_mut()
            .flat_map(|open| open.levels.iter_mut().map(|level| &mut level.presenter)),
    )
}

/// The value an extension event of the public protocols carries.
fn app_event(event: &Event) -> Option<AppEvent> {
    match event {
        Event::Extension(event) => event.downcast_ref::<AppEvent>().cloned(),
        _ => None,
    }
}

/// A press on an item that becomes a drag once the pointer moves far enough.
struct Drag {
    press: Point,
    /// The dragged items: set when the drag starts.
    sources: Vec<PathBuf>,
    started: bool,
    target: Option<DropTarget>,
}

/// A rubber band being dragged from empty space.
struct Marquee {
    /// Where it started, in content coordinates (it stays on the content when that scrolls).
    origin: Point,
    /// The pointer, in window points.
    at: Point,
    /// When the next auto-scroll step is due (seconds on the app clock).
    next_scroll: f64,
}

/// Where a drag would drop.
#[derive(Clone, PartialEq)]
struct DropTarget {
    path: PathBuf,
    /// The listing item or sidebar place marked with the drop look.
    outline: DropOutline,
    /// The trash place: dropping trashes.
    trash: bool,
}

/// A rectangle for the client's popups.
fn popup_rect(r: &cupertino_files::Rect) -> PopupRect {
    PopupRect::new(r.x, r.y, r.w, r.h)
}

/// The palette of the session's appearance.
fn palette(theme: &Theme, dark: bool) -> Palette {
    Palette::from_theme(
        theme,
        if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        },
    )
}

/// The window title for a folder: the place name of the trash folder (`<trash>/files`) and of Recents, else
/// the folder's name, the path for the root.
fn title(dir: &Path, places: &[Place]) -> String {
    if let Some(place) = places
        .iter()
        .find(|p| matches!(p.kind, PlaceKind::Trash | PlaceKind::Recents) && p.path == dir)
    {
        return place.name.clone();
    }
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Connect, open the window on `folder` and serve it until it is closed; `terminal` runs the applications
/// with `Terminal=true`.
pub fn run(folder: PathBuf, terminal: Option<Vec<String>>) -> Result<()> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let dirs = XdgDirs::from_env()?;
    let mut conn =
        Connection::connect().map_err(|e| format!("cannot connect to the display server: {e}"))?;
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
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let SessionPlaces {
        places,
        folder_icons,
    } = session_places(&home, &dirs.data_dirs)?;
    let folders = folder_icons.into_iter().collect();
    // one database for the icons and for the Kind column, so both name the same type
    let types = Arc::new(MimeDatabase::load(&dirs.all_data_dirs())?);
    let icons = FileIcons::new(&assets, (*types).clone(), folders);
    let waker = conn.waker();
    let browser = Browser::open(
        folder.clone(),
        SortSpec::default(),
        dirs.data_home.join(recents::XBEL_NAME),
        Some(types),
        Arc::new(move || waker.wake()),
    )
    .map_err(|e| format!("cannot open {}: {e}", folder.display()))?;
    let (width, height) = window_metrics::DEFAULT_SIZE;
    let window = Window::open(
        &mut conn,
        &effects,
        title(&folder, &places),
        (width as u32, height as u32),
        (
            metrics::WINDOW_MIN.0.ceil() as u32,
            metrics::WINDOW_MIN.1.ceil() as u32,
        ),
    )?;
    let sidebar = effects.backdrop(&conn, window.toplevel.surface())?;
    sidebar.set_material(SIDEBAR_MATERIAL);
    let waker = conn.waker();
    let volumes = zbus::blocking::Connection::system()
        .and_then(|system| {
            Volumes::watch(
                system,
                zbus::blocking::Connection::session().ok(),
                Arc::new(move || waker.wake()),
            )
        })
        .map_err(|e| eprintln!("cupertino-files: no removable media: {e}"))
        .ok();
    let mut app = App {
        palette: palette(&theme, dark),
        conn,
        effects,
        _appearance: appearance,
        display,
        clipboard: Clipboard::default(),
        window,
        sidebar,
        theme,
        painter: WindowPainter::new(Renderer::new(icons, &assets)?),
        browser,
        standard_places: places.len(),
        places,
        volumes,
        devices: Vec::new(),
        busy: HashSet::new(),
        home,
        mode: ViewMode::Icons,
        bars: Bars::HIDDEN,
        available: None,
        search: TextField::with_max_len(SEARCH_MAX_LEN),
        search_focused: false,
        search_width: SearchWidth::default(),
        frame: None,
        scroll: 0.0,
        pending_scroll: 0.0,
        last_press: None,
        last_titlebar_press: None,
        pending: HashMap::new(),
        start: Instant::now(),
        terminal,
        menu: None,
        edit: None,
        keymap: Keymap::new(
            cupertino_keyboard::binding::user_profile(),
            cupertino_keyboard::binding::load_default().0,
        ),
        caret_drawn: false,
        rename: RenameTrigger::default(),
        edit_when_listed: None,
        drag: None,
        marquee: None,
        menus: None,
        menu_state: None,
        trash_files: Trash::new().home_trash().join("files"),
        closing: false,
    };
    app.publish_menus();
    app.update_available();
    let mut events = early;
    loop {
        for event in events {
            if !app.handle(event)? {
                return Ok(());
            }
        }
        app.sync_menus()?;
        if app.closing {
            return Ok(());
        }
        app.sync_menu_mods()?;
        app.tick()?;
        app.refresh()?;
        app.present()?;
        events = app.conn.wait(app.timeout())?;
    }
}

/// The default-application resolver of the session's directories and desktops.
fn applications() -> Result<MimeApps> {
    let dirs = XdgDirs::from_env()?;
    let db = MimeDatabase::load(&dirs.all_data_dirs())?;
    Ok(MimeApps::load(
        &dirs,
        db,
        &Locale::from_env(),
        &Visibility::from_env(),
    )?)
}

impl App {
    /// React to one event; `false` once the window is to close.
    fn handle(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Configure { configure, .. } => self.window.configure(&configure)?,
            Event::PopupConfigure { popup, configure } => {
                if let Some(level) = self.menu_level(popup.surface()) {
                    let open = self.menu.as_mut().expect("a level is of the open menu");
                    open.levels[level]
                        .presenter
                        .configured((configure.width, configure.height));
                }
            }
            Event::Frame(frame) => {
                if let Some(presenter) = presenters(&mut self.window, &mut self.menu)
                    .find(|p| p.surface() == frame.surface)
                {
                    presenter.frame_done();
                }
            }
            Event::Close(_) => return Ok(false),
            Event::Pointer { surface, event, .. } => match self.menu_level(surface) {
                Some(level) => self.menu_pointer(level, event)?,
                None => self.pointer(event)?,
            },
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
            Event::Keyboard { .. } => {}
            Event::Selection(offer) => self.clipboard.selection_changed(offer),
            Event::ActivationToken { request, token } => self.launch(request, &token)?,
            Event::Extension(_) => match app_event(&event) {
                Some(AppEvent::Appearance { dark }) => {
                    self.palette = palette(&self.theme, dark);
                    self.window.redraw();
                }
                // the display options reach the title bar and materials through the server
                Some(AppEvent::DisplayOptions(options)) => {
                    // the drop look follows Increase Contrast
                    if options.increase_contrast != self.display.increase_contrast {
                        self.window.redraw();
                    }
                    self.display = options;
                }
                Some(event @ (AppEvent::MenuBar { .. } | AppEvent::MenuReader { .. })) => {
                    if let Some(menus) = &mut self.menus {
                        menus.handle(&event);
                    }
                }
                Some(AppEvent::ScrollCount { .. } | AppEvent::SessionLock { .. }) | None => {}
            },
            // the server dismissed the menu (a click outside it); popups this process destroyed itself end no menu
            Event::PopupDone(popup) => {
                if self.menu_level(popup.surface()).is_some() {
                    self.close_menu()?;
                }
            }
            // the scale is read when polling the presenters; the window has no output-bound geometry
            Event::ScaleChanged { .. }
            | Event::OutputsChanged
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

    /// Draw every surface that can take a frame, until none can.
    fn present(&mut self) -> Result<()> {
        loop {
            let mut ready = Vec::new();
            for presenter in presenters(&mut self.window, &mut self.menu) {
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
                } else if let Some(level) = self.menu_level(surface) {
                    self.draw_menu(level)?;
                }
            }
        }
    }

    /// Take what the folder's model queued; draw again when the listing changed.
    fn refresh(&mut self) -> Result<()> {
        self.poll_volumes()?;
        let dir = self.browser.dir().to_path_buf();
        let polled = self.browser.poll();
        for e in &polled.errors {
            eprintln!("cupertino-files: {}: {e}", self.browser.dir().display());
        }
        if self.browser.dir() != dir {
            self.entered()?;
        } else if polled.changed {
            self.update_available();
            self.follow_edit();
            self.window.redraw();
        }
        Ok(())
    }

    /// Take what the removable media reported: the rows follow the devices, a window on a file system that went away
    /// goes home, and a finished mount goes to its folder.
    fn poll_volumes(&mut self) -> Result<()> {
        let Some(volumes) = &self.volumes else {
            return Ok(());
        };
        let reports = volumes.poll();
        if reports.is_empty() {
            return Ok(());
        }
        let mut go = None;
        for report in reports {
            match report {
                Report::Devices(devices) => {
                    self.busy
                        .retain(|object| devices.iter().any(|d| &d.object == object));
                    self.devices = devices;
                }
                Report::Mounted { object, at } => {
                    self.busy.remove(&object);
                    go = Some(at);
                }
                Report::Done { object } => {
                    let drive = self
                        .devices
                        .iter()
                        .find(|d| d.object == object)
                        .map(|d| d.drive.clone());
                    let devices = &self.devices;
                    self.busy.retain(|o| {
                        *o != object
                            && !devices
                                .iter()
                                .any(|d| &d.object == o && Some(&d.drive) == drive.as_ref())
                    });
                }
            }
        }
        let held = removable_holding(self.browser.dir(), &self.places);
        self.places.truncate(self.standard_places);
        let rows: Vec<Place> = self
            .devices
            .iter()
            .map(|d| Place::removable(&d.name, d.mount.as_deref(), self.busy.contains(&d.object)))
            .collect();
        self.places.extend(rows);
        self.window.redraw();
        if let Some(at) = go {
            return self.go(at);
        }
        let mut mounted = self.devices.iter().filter_map(|d| d.mount.as_deref());
        if held.is_some() && !mounted.any(|m| self.browser.dir().starts_with(m)) {
            return self.go(self.home.clone());
        }
        Ok(())
    }

    /// Mount the removable file system of place `index`.
    fn mount(&mut self, index: usize) {
        let (Some(volumes), Some(device)) = (
            &self.volumes,
            index
                .checked_sub(self.standard_places)
                .and_then(|i| self.devices.get(i)),
        ) else {
            return;
        };
        self.busy.insert(device.object.clone());
        volumes.mount(device);
        self.places[index] = Place::removable(&device.name, None, true);
        self.window.redraw();
    }

    /// Eject the drive of the removable file system of place `index`: a window on one of its file systems goes home
    /// first, so it holds none of them.
    fn eject(&mut self, index: usize) -> Result<()> {
        let Some(device) = index
            .checked_sub(self.standard_places)
            .and_then(|i| self.devices.get(i))
            .filter(|d| !self.busy.contains(&d.object))
            .cloned()
        else {
            return Ok(());
        };
        let filesystems: Vec<Device> = self
            .devices
            .iter()
            .filter(|d| d.drive == device.drive && d.mount.is_some())
            .cloned()
            .collect();
        if filesystems
            .iter()
            .filter_map(|d| d.mount.as_deref())
            .any(|m| self.browser.dir().starts_with(m))
        {
            self.go(self.home.clone())?;
        }
        let Some(volumes) = &self.volumes else {
            return Ok(());
        };
        for fs in &filesystems {
            self.busy.insert(fs.object.clone());
        }
        self.busy.insert(device.object.clone());
        volumes.eject(&device, filesystems);
        for (place, d) in self.places[self.standard_places..]
            .iter_mut()
            .zip(&self.devices)
        {
            if self.busy.contains(&d.object) {
                *place = Place::removable(&d.name, d.mount.as_deref(), true);
            }
        }
        self.window.redraw();
        Ok(())
    }

    /// The listing changed: keep the name editor on its item (it ends with the item), and start the editor of a new
    /// folder once the listing has it.
    fn follow_edit(&mut self) {
        let entries = self.browser.entries();
        if let Some((edit, path)) = self.edit.as_mut() {
            match entries.iter().position(|e| &e.path == path) {
                Some(index) => edit.set_index(index),
                None => self.edit = None,
            }
        }
        if let Some(index) = self
            .edit_when_listed
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
        {
            self.edit_when_listed = None;
            self.begin_edit(index);
        }
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// How long the loop may sleep: until the next timer of the menu or of the click-and-wait rename.
    fn timeout(&self) -> Option<Duration> {
        let due = [
            self.menu.as_ref().and_then(|open| open.menu.deadline()),
            self.rename.deadline(),
            self.search_width
                .animating(self.now())
                .then(|| self.now() + FRAME_SECONDS),
            self.marquee.as_ref().map(|m| m.next_scroll),
            self.search_focused
                .then(|| self.search.next_blink(Instant::now()))
                .flatten()
                .map(|d| self.now() + d.as_secs_f64()),
            self.edit
                .as_ref()
                .and_then(|(e, _)| e.field().next_blink(Instant::now()))
                .map(|d| self.now() + d.as_secs_f64()),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)?;
        Some(Duration::from_secs_f64((due - self.now()).max(0.0)) + Duration::from_millis(1))
    }

    /// Run the timers that are due.
    fn tick(&mut self) -> Result<()> {
        let now = self.now();
        if self.search_width.animating(now) {
            self.window.redraw();
        } else if self.caret_shown() != self.caret_drawn {
            match self.caret_area() {
                Some(areas) => areas.into_iter().for_each(|a| self.window.redraw_rect(a)),
                None => self.window.redraw(),
            }
        }
        if self.menu.as_mut().is_some_and(|open| open.menu.poll(now)) {
            self.sync_menu()?;
        }
        self.marquee_autoscroll(now)?;
        if let Some(RenameEvent::Begin(index)) = self.rename.poll(now) {
            self.begin_edit(index);
            self.window.redraw();
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let dy = std::mem::take(&mut self.pending_scroll);
        let (width, height) = self.window.size()?;
        let active = self.window.activated;
        let edit_caret = active && self.edit_caret();
        let search_caret = active && self.search_caret();
        let now = self.now();
        let animate = !self.display.reduce_motion;
        self.search_width
            .set_open(self.search_focused || !self.search.is_empty(), now, animate);
        let search_width = self.search_width.width(now);
        self.caret_drawn = edit_caret || search_caret;
        let reveal = self
            .browser
            .take_focus_moved()
            .then(|| self.browser.selection().focus())
            .flatten();
        let size = Size {
            w: f64::from(width),
            h: f64::from(height),
        };
        let App {
            conn,
            window,
            sidebar,
            painter,
            browser,
            places,
            mode,
            bars,
            available,
            search,
            search_focused,
            palette,
            scroll,
            edit,
            drag,
            marquee,
            display,
            frame: prev_frame,
            ..
        } = self;
        let drop_target = drag
            .as_ref()
            .and_then(|d| d.target.as_ref())
            .map(|t| t.outline);
        let title = title(browser.dir(), places);
        let status = status_text(browser.item_count(), *available);
        let state = WindowState {
            browser,
            places,
            mode: *mode,
            active,
            search,
            search_focused: *search_focused,
            search_caret,
            title: &title,
            now: SystemTime::now(),
            edit: edit.as_ref().map(|(e, _)| e),
            edit_caret,
            drop_target,
            increase_contrast: display.increase_contrast,
            marquee: marquee.as_ref().map(|m| (m.origin, m.at)),
            status: &status,
        };
        let mut drawn = None;
        let mut shaped = Ok(());
        // a scroll moves the whole content view
        if dy != 0.0 || reveal.is_some() {
            match prev_frame {
                Some(f) => window.damage.add(popup_rect(&f.window.content)),
                None => window.damage.add_whole(),
            }
        }
        let damage = window.damage.take();
        let scratch = &mut window.scratch;
        window
            .presenter
            .draw_with(conn, damage.as_deref(), |pm, scale, cleared| {
                let mut frame = painter.frame(
                    size,
                    state.places,
                    *bars,
                    search_width,
                    state.browser.dir(),
                    state.mode,
                    state.browser.outline(),
                    *scroll + dy,
                    scale.factor() as f32,
                );
                if let Some(index) = reveal {
                    frame.reveal(index);
                }
                // the material follows the sidebar, which gives way when the window narrows. A shape has one radius: the
                // window's, so the outer corners match the window; the inner ones lie under the opaque content
                let radius = WindowCornerSize::Regular.radius_pt();
                let area = frame.window.sidebar;
                shaped = sidebar.set_shape(
                    &popup_rect(&cupertino_files::Rect::new(
                        area.x,
                        area.y,
                        area.w + radius,
                        area.h,
                    )),
                    radius,
                );
                let factor = scale.factor() as f32;
                match cleared {
                    None => painter.paint(pm, factor, &frame, &state, palette),
                    // the frame is drawn into a scratch pixmap, its content view only in the rows the damage crosses,
                    // and only the damaged rectangles of it replace what the pixmap holds, which makes the pixels the
                    // same as those of a whole redraw
                    Some(rects) => {
                        let whole = match scratch.take() {
                            Some(p) if (p.width(), p.height()) == (pm.width(), pm.height()) => p,
                            _ => {
                                Pixmap::new(pm.width(), pm.height()).expect("the pixmap has a size")
                            }
                        };
                        let mut whole = whole;
                        whole.fill(Color::TRANSPARENT);
                        let rows: Vec<(u32, u32)> =
                            rects.iter().map(|r| (r.y, r.y + r.height)).collect();
                        painter.paint_rows(&mut whole, factor, &frame, &state, palette, &rows);
                        copy_rects(pm, &whole, rects);
                        *scratch = Some(whole);
                    }
                }
                drawn = Some(frame);
            })?;
        shaped?;
        let frame = drawn.expect("draw runs the paint closure");
        self.scroll = frame.scroll;
        self.frame = Some(frame);
        self.painter.follow_search_caret(&mut self.search);
        // a scroll or a resize moves the content under a band that the pointer is not moving
        if self.marquee_apply() {
            self.window.redraw();
        }
        Ok(())
    }

    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        match event {
            // a Control-click is the secondary click, as on the design
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } if self.conn.modifiers().ctrl => self.context(Point {
                x: position.x,
                y: position.y,
            }),
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => self.press(Point {
                x: position.x,
                y: position.y,
            }),
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } => self.release(),
            PointerEvent::Button {
                button: Button::Right,
                pressed: true,
                position,
            } => self.context(Point {
                x: position.x,
                y: position.y,
            }),
            PointerEvent::Motion(position) => self.drag_motion(Point {
                x: position.x,
                y: position.y,
            }),
            // one value per frame however many wheel or touchpad events arrive before it
            PointerEvent::Scroll { dy, .. } => {
                self.rename.cancel();
                self.pending_scroll += dy;
                self.window.presenter.request_redraw();
                Ok(())
            }
            PointerEvent::Enter(_) | PointerEvent::Leave | PointerEvent::Button { .. } => Ok(()),
        }
    }

    fn press(&mut self, at: Point) -> Result<()> {
        // a press ends the edit of a name, except one in the editor itself or one that finds the name invalid
        if self.edit.is_some() && self.in_editor(at) {
            return self.editor_press(at);
        }
        if self.edit.is_some() && !self.end_edit()? {
            return Ok(());
        }
        self.drag = None;
        // nothing to hit before the first frame
        let Some(frame) = &self.frame else {
            return Ok(());
        };
        let held = self.conn.modifiers();
        let mods = Modifiers {
            shift: held.shift,
            primary: held.logo,
        };
        let hit = frame.hit(at);
        if !matches!(hit, Hit::Item(_)) {
            self.last_press = None;
        }
        // a press anywhere else takes the keyboard from the search field, which keeps its text
        if hit == Hit::Toolbar(ToolbarItem::Search) {
            return self.search_press(at);
        }
        if self.search_focused {
            self.search_focused = false;
            self.search.blur();
            self.window.redraw();
        }
        match hit {
            Hit::Back => {
                let outcome = self.browser.back();
                self.navigated(outcome, "the previous folder")
            }
            Hit::Forward => {
                let outcome = self.browser.forward();
                self.navigated(outcome, "the next folder")
            }
            Hit::Toolbar(item) => self.press_toolbar(
                item,
                frame
                    .window
                    .items
                    .iter()
                    .find(|(i, _)| *i == item)
                    .map(|(_, r)| *r)
                    .expect("a hit item is laid out"),
            ),
            Hit::Place(i) => match self.places[i].kind {
                PlaceKind::Removable(Volume::Unmounted) => {
                    self.mount(i);
                    Ok(())
                }
                PlaceKind::Removable(Volume::Busy) => Ok(()),
                _ => {
                    let path = self.places[i].path.clone();
                    self.go(path)
                }
            },
            Hit::Eject(i) => self.eject(i),
            Hit::PathSegment(i) => {
                let path = frame.path[i].path.clone();
                self.go(path)
            }
            Hit::Column(id) => {
                let column = self
                    .painter
                    .list
                    .columns
                    .iter()
                    .find(|c| c.id == id)
                    .ok_or("a header of a column that is not shown")?;
                self.browser.sort_by(id.sort_key(), column.ascending);
                self.window.redraw();
                Ok(())
            }
            Hit::Disclosure(index) => {
                match self.browser.toggle_expanded(index) {
                    Ok(_) => {}
                    Err(e) => eprintln!("cupertino-files: cannot list the folder: {e}"),
                }
                self.window.redraw();
                Ok(())
            }
            Hit::Item(index) => {
                let sole =
                    self.browser.selection().len() == 1 && self.browser.selection().contains(index);
                let on_name = frame.name_frame(index).is_some_and(|r| r.contains(at));
                self.rename
                    .click(index, on_name, sole, self.start.elapsed().as_secs_f64());
                let now = Instant::now();
                let double = self.last_press.is_some_and(|(last, at)| {
                    last == index && now.duration_since(at) <= DOUBLE_CLICK
                });
                self.last_press = (!double).then_some((index, now));
                if double {
                    return self.open_selection();
                }
                self.drag = Some(Drag {
                    press: at,
                    sources: Vec::new(),
                    started: false,
                    target: None,
                });
                let before = self.selected_indices();
                if self.browser.press(index, mods) {
                    self.redraw_items(&before);
                }
                Ok(())
            }
            Hit::Empty => {
                self.marquee = Some(Marquee {
                    origin: frame.to_content(at),
                    at,
                    next_scroll: self.start.elapsed().as_secs_f64()
                        + marquee_metrics::AUTOSCROLL_INTERVAL,
                });
                self.browser.rubber_band_begin(mods);
                self.window.redraw();
                Ok(())
            }
            Hit::Titlebar => self.press_titlebar(),
            Hit::Chrome => Ok(()),
        }
    }

    /// A press on the toolbar strip: a double click zooms the window (or restores it), a single press moves it.
    fn press_titlebar(&mut self) -> Result<()> {
        let now = Instant::now();
        let double = self
            .last_titlebar_press
            .is_some_and(|last| now.duration_since(last) <= DOUBLE_CLICK);
        self.last_titlebar_press = (!double).then_some(now);
        if double {
            self.conn
                .set_maximized(self.window.toplevel, !self.window.maximized)?;
        } else {
            self.conn.start_move(self.window.toplevel)?;
        }
        Ok(())
    }

    fn key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        if self.menu.is_some() {
            return self.menu_key(code, mods, character);
        }
        self.rename.cancel();
        if self.edit.is_some() {
            return self.edit_key(code, mods, character);
        }
        if self.search_focused {
            return self.search_key(code, mods, character);
        }
        if code == key::ESCAPE && self.drag.as_ref().is_some_and(|d| d.started) {
            self.drag = None;
            self.window.redraw();
            return Ok(());
        }
        let changed = if mods.logo && !mods.ctrl && !mods.alt {
            match (code, character) {
                (key::BACKSPACE, _) => return self.trash_selection(),
                (key::UP, _) => {
                    let outcome = self.browser.parent();
                    return self.navigated(outcome, "the enclosing folder");
                }
                (key::DOWN, _) => return self.open_selection(),
                (_, Some('[')) => {
                    let outcome = self.browser.back();
                    return self.navigated(outcome, "the previous folder");
                }
                (_, Some(']')) => {
                    let outcome = self.browser.forward();
                    return self.navigated(outcome, "the next folder");
                }
                // the keys of the menu bar's items (crate::menu_bar)
                (_, Some('n')) => return self.menu_action(MenuAction::NewWindow),
                (_, Some('N')) => return self.menu_action(MenuAction::NewFolder),
                (_, Some('o')) => return self.menu_action(MenuAction::Open),
                (_, Some('d')) => return self.menu_action(MenuAction::Duplicate),
                (_, Some('r')) => return self.menu_action(MenuAction::ShowOriginal),
                (_, Some('w')) => return self.menu_action(MenuAction::CloseWindow),
                (_, Some('e')) => return self.menu_action(MenuAction::Eject),
                // the design's View menu shortcut for the status bar
                (_, Some('/')) => return self.toggle_bar(false),
                (_, Some('1')) => return self.set_mode(ViewMode::Icons),
                (_, Some('2')) => return self.set_mode(ViewMode::List),
                (_, Some('a')) => {
                    self.browser.select_all();
                    true
                }
                _ => false,
            }
        } else if mods.logo && mods.ctrl && !mods.alt && matches!(character, Some('n' | 'a')) {
            return self.menu_action(if character == Some('n') {
                MenuAction::NewFolderWithSelection
            } else {
                MenuAction::MakeAlias
            });
        } else if mods.logo && mods.alt && !mods.ctrl && matches!(character, Some('p' | 'P')) {
            // the design's View menu shortcut for the path bar
            return self.toggle_bar(true);
        } else if mods.logo || mods.ctrl || mods.alt {
            false
        } else {
            match code {
                key::ENTER | key::KEYPAD_ENTER => return self.open_selection(),
                key::UP => self.step(Move::Up, mods.shift),
                key::DOWN => self.step(Move::Down, mods.shift),
                key::LEFT | key::RIGHT if self.mode == ViewMode::List => {
                    match self.browser.disclose_focus(code == key::RIGHT) {
                        Ok(changed) => changed,
                        Err(e) => {
                            eprintln!("cupertino-files: cannot list the folder: {e}");
                            false
                        }
                    }
                }
                key::LEFT => self.step(Move::Left, mods.shift),
                key::RIGHT => self.step(Move::Right, mods.shift),
                key::HOME => self.step(Move::First, mods.shift),
                key::END => self.step(Move::Last, mods.shift),
                key::TAB => self.browser.tab(mods.shift),
                _ => match character {
                    Some(c) => self.browser.type_text(
                        c.encode_utf8(&mut [0; 4]),
                        self.start.elapsed().as_secs_f64(),
                    ),
                    None => false,
                },
            }
        };
        if changed {
            self.window.redraw();
        }
        Ok(())
    }

    /// Move the focus by a key in the frame drawn last. The items it changes are drawn here, so a caller has nothing
    /// more to draw: the result is always `false`.
    fn step(&mut self, key: Move, extend: bool) -> bool {
        let Some(frame) = &self.frame else {
            return false;
        };
        match frame.content.step(
            self.browser.selection().focus(),
            self.browser.entries().len(),
            key,
        ) {
            Some(index) => {
                let before = self.selected_indices();
                self.browser.move_focus(index, extend);
                self.redraw_items(&before);
                false
            }
            None => false,
        }
    }

    /// The indices of the selected items.
    fn selected_indices(&self) -> Vec<usize> {
        self.browser.selection().indices().collect()
    }

    /// The selection changed from `before`: draw the items that were selected and those that are now. A scroll to the
    /// focus is added when the frame is drawn.
    fn redraw_items(&mut self, before: &[usize]) {
        let Some(frame) = &self.frame else {
            self.window.redraw();
            return;
        };
        let mut items = before.to_vec();
        items.extend(self.browser.selection().indices());
        items.sort_unstable();
        items.dedup();
        let areas: Vec<PopupRect> = items
            .into_iter()
            .filter_map(|i| cupertino_files::window::item_area(frame, i))
            .collect();
        if areas.is_empty() {
            self.window.redraw();
        }
        for area in areas {
            self.window.redraw_rect(area);
        }
    }

    fn set_mode(&mut self, mode: ViewMode) -> Result<()> {
        if self.mode != mode {
            self.mode = mode;
            self.scroll = 0.0;
            if mode != ViewMode::List {
                self.browser.collapse_all();
            }
            self.window.redraw();
        }
        Ok(())
    }

    fn go(&mut self, to: PathBuf) -> Result<()> {
        let target = to.display().to_string();
        let outcome = self.browser.go(to).map(|()| true);
        self.navigated(outcome, &target)
    }

    /// Follow up a navigation: a new folder starts at the top with its name in the title bar; a folder that
    /// cannot be opened leaves the window as it is and is reported.
    fn navigated(&mut self, outcome: io::Result<bool>, target: &str) -> Result<()> {
        match outcome {
            Ok(true) => self.entered(),
            Ok(false) => Ok(()),
            Err(e) => {
                eprintln!("cupertino-files: cannot open {target}: {e}");
                Ok(())
            }
        }
    }

    fn entered(&mut self) -> Result<()> {
        self.scroll = 0.0;
        self.last_press = None;
        self.rename.cancel();
        self.edit = None;
        self.edit_when_listed = None;
        self.drag = None;
        self.marquee = None;
        self.close_menu()?;
        self.conn.set_title(
            self.window.toplevel,
            &title(self.browser.dir(), &self.places),
        )?;
        // another folder ends the search (the browser shows it whole)
        self.search.set_text("");
        self.search_focused = false;
        self.update_available();
        self.window.redraw();
        Ok(())
    }

    /// Read the space left for the status bar; a failure is reported and the bar shows the item count only.
    fn update_available(&mut self) {
        self.available = match self.browser.available_space() {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!(
                    "cupertino-files: cannot read the free space of {}: {e}",
                    self.browser.dir().display()
                );
                None
            }
        };
    }

    /// Show or hide one of the bars under the folder's view.
    fn toggle_bar(&mut self, path: bool) -> Result<()> {
        if path {
            self.bars.path = !self.bars.path;
        } else {
            self.bars.status = !self.bars.status;
        }
        self.window.redraw();
        Ok(())
    }

    /// Open the selection: files with their default applications, each once its activation token arrives, and
    /// the first selected folder in this window.
    fn open_selection(&mut self) -> Result<()> {
        let Activation { folder, files } = self.browser.activation();
        // the token names the default application, so the dock bounces its tile
        let apps = if files.is_empty() {
            None
        } else {
            Some(applications()?)
        };
        for path in files {
            let id = apps
                .as_ref()
                .and_then(|apps| default_app_id(apps, &path, false));
            let Some(request) = self.request_token(id.as_deref(), &path) else {
                continue;
            };
            self.pending.insert(
                request,
                Launch {
                    path,
                    is_dir: false,
                    with: None,
                },
            );
        }
        match folder {
            Some(folder) => self.go(folder),
            None => Ok(()),
        }
    }

    /// Open every selected item with the application of desktop entry `id`, each once its activation token arrives.
    fn open_selection_with(&mut self, id: &str) -> Result<()> {
        let items: Vec<(PathBuf, bool)> = self
            .selected_entries()
            .map(|e| (e.path.clone(), e.is_dir))
            .collect();
        for (path, is_dir) in items {
            let Some(request) = self.request_token(Some(id), &path) else {
                continue;
            };
            self.pending.insert(
                request,
                Launch {
                    path,
                    is_dir,
                    with: Some(id.to_string()),
                },
            );
        }
        Ok(())
    }

    /// Ask for the activation token that opens `path`, naming the application `app_id` when known; a token that cannot
    /// be asked for is reported and that item is not opened.
    fn request_token(&mut self, app_id: Option<&str>, path: &Path) -> Option<TokenRequest> {
        match self
            .conn
            .request_activation_token(self.window.toplevel.surface(), app_id)
        {
            Ok(request) => Some(request),
            Err(e) => {
                eprintln!("cupertino-files: cannot open {}: {e}", path.display());
                None
            }
        }
    }

    fn launch(&mut self, request: TokenRequest, token: &str) -> Result<()> {
        let Launch { path, is_dir, with } = self
            .pending
            .remove(&request)
            .ok_or("an activation token arrived for no request of this process")?;
        let terminal = self.terminal.as_deref();
        // the associations are read at each open, so changed defaults and new applications apply at once
        let spec = applications().and_then(|apps| match &with {
            None => Ok(open_command(&apps, &path, is_dir, terminal)?),
            Some(id) => {
                let item = open_with(&apps, &path, is_dir)
                    .into_iter()
                    .find(|item| &item.entry.id == id);
                let item =
                    item.ok_or_else(|| format!("{id} no longer opens {}", path.display()))?;
                Ok(command_with(item.entry, &path, terminal)?)
            }
        });
        match spec {
            Ok(spec) => {
                launch::spawn(APP_ID, spec.with_activation_token(token).to_command());
            }
            Err(e) => eprintln!("cupertino-files: cannot open {}: {e}", path.display()),
        }
        Ok(())
    }

    fn selected_entries(&self) -> impl Iterator<Item = &Entry> {
        let entries = self.browser.entries();
        self.browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
    }

    /// The applications that open every selected item (Open With).
    fn selection_apps(&self) -> Vec<AppChoice> {
        let apps = match applications() {
            Ok(apps) => apps,
            Err(e) => {
                eprintln!("cupertino-files: cannot read the default applications: {e}");
                return Vec::new();
            }
        };
        let lists: Vec<Vec<AppChoice>> = self
            .selected_entries()
            .map(|e| {
                open_with(&apps, &e.path, e.is_dir)
                    .into_iter()
                    .map(|item| AppChoice {
                        id: item.entry.id.clone(),
                        name: item.entry.name.clone(),
                        is_default: item.is_default,
                    })
                    .collect()
            })
            .collect();
        common_apps(&lists)
    }

    /// A right click: select the item under it unless it is selected, and open the menu of the selection, or of the folder.
    fn context(&mut self, at: Point) -> Result<()> {
        if self.edit.is_some() && !self.end_edit()? {
            return Ok(());
        }
        self.drag = None;
        self.marquee = None;
        let Some(frame) = &self.frame else {
            return Ok(());
        };
        let in_trash = self.browser.dir() == Trash::new().home_trash().join("files");
        let items = match frame.hit(at) {
            Hit::Item(index) | Hit::Disclosure(index) => {
                if !self.browser.selection().contains(index) {
                    self.browser.press(index, Modifiers::NONE);
                    self.browser.end_drag();
                }
                let subject = self.subject(in_trash);
                menu_spec::item_menu(&subject, &self.selection_apps())
            }
            Hit::Empty => {
                self.browser.press_empty(Modifiers::NONE);
                menu_spec::background_menu(&Background {
                    view: self.mode,
                    sort: self.browser.sort(),
                    in_trash,
                })
            }
            _ => return Ok(()),
        };
        self.window.redraw();
        self.open_menu(items, at)
    }

    /// A press on toolbar item `item` at `r`: a popup opens its menu under it, the search field takes the keyboard.
    fn press_toolbar(&mut self, item: ToolbarItem, r: cupertino_files::Rect) -> Result<()> {
        let below = Point {
            x: r.x,
            y: r.bottom() + metrics::POPUP_GAP,
        };
        match item {
            ToolbarItem::View => self.open_menu(menu_spec::view_menu(self.mode), below),
            ToolbarItem::Tags => {
                let has_selection = !self.browser.selection().is_empty();
                let on = if has_selection {
                    self.subject(false).tags_on
                } else {
                    Vec::new()
                };
                self.open_menu(menu_spec::tags_menu(&on, has_selection), below)
            }
            ToolbarItem::Action => {
                let in_trash = self.browser.dir() == Trash::new().home_trash().join("files");
                let items = if self.browser.selection().is_empty() {
                    menu_spec::background_menu(&Background {
                        view: self.mode,
                        sort: self.browser.sort(),
                        in_trash,
                    })
                } else {
                    menu_spec::item_menu(&self.subject(in_trash), &self.selection_apps())
                };
                self.open_menu(items, below)
            }
            ToolbarItem::Search => {
                unreachable!("a press on the search field goes to `search_press`")
            }
            ToolbarItem::GroupBy => unreachable!("a disabled item is not hit"),
        }
    }

    /// A key while the search field has the keyboard: the text field edits the query and the listing follows it;
    /// Escape clears it and gives the keyboard back, Return and Tab give it back keeping the query.
    fn search_key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        let before = self.search.text().to_string();
        match code {
            key::ESCAPE => {
                self.search.set_text("");
                self.search_focused = false;
            }
            key::ENTER | key::KEYPAD_ENTER | key::TAB => {
                self.search.blur();
                self.search_focused = false;
            }
            _ => {
                let input =
                    KeyInput::new(code, mods.shift, mods.ctrl, mods.alt, mods.logo, character);
                match self.search.key(&mut self.keymap, input, Instant::now()) {
                    Response::Ignored => return Ok(()),
                    Response::Moved | Response::Edited => {}
                    Response::Copy(text) | Response::Cut(text) => {
                        if let Err(e) = self.clipboard.set_text(&mut self.conn, text) {
                            eprintln!("cupertino-files: copy: {e}");
                        }
                    }
                    Response::Paste => match self.clipboard.text(&mut self.conn) {
                        Ok(Some(text)) => self.search.paste(&text),
                        Ok(None) => {}
                        Err(e) => eprintln!("cupertino-files: paste: {e}"),
                    },
                }
            }
        }
        if self.search.text() != before {
            self.browser.set_filter(self.search.text());
            self.scroll = 0.0;
            self.update_available();
        }
        self.window.redraw();
        Ok(())
    }

    /// A press on the search field gives it the keyboard and places the caret there (Shift extends; a double press
    /// selects a word, a triple press the whole query).
    fn search_press(&mut self, at: Point) -> Result<()> {
        self.search_focused = true;
        if let Some(index) = self.painter.search_index(&self.search, at) {
            let extend = self.conn.modifiers().shift;
            self.search.press(index, extend, Instant::now());
        }
        self.window.redraw();
        Ok(())
    }

    /// What the menu of the selection depends on.
    fn subject(&self, in_trash: bool) -> Subject {
        let selected: Vec<&Entry> = self.selected_entries().collect();
        let tags_on = TagColor::ALL
            .into_iter()
            .filter(|c| {
                selected
                    .iter()
                    .all(|e| e.tags.iter().any(|t| t.eq_ignore_ascii_case(c.title())))
            })
            .collect();
        Subject {
            count: selected.len(),
            name: selected.first().map(|e| e.name.clone()).unwrap_or_default(),
            all_folders: selected.iter().all(|e| e.is_dir),
            single_link: selected.len() == 1 && selected[0].kind == Kind::Symlink,
            in_trash,
            tags_on,
        }
    }

    /// The modifiers held decide which alternate rows of an open menu show.
    fn sync_menu_mods(&mut self) -> Result<()> {
        let held = self.conn.modifiers();
        let mods = MenuMods {
            shift: held.shift,
            ctrl: held.ctrl,
            alt: held.alt,
        };
        if self
            .menu
            .as_mut()
            .is_some_and(|open| open.menu.set_mods(mods))
        {
            self.sync_menu()?;
        }
        Ok(())
    }

    fn open_menu(&mut self, items: Vec<MenuItem>, at: Point) -> Result<()> {
        self.close_menu()?;
        let (width, height) = self.window.size()?;
        let bounds = cupertino_files::Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        // the menu is laid out in points; the popups draw it at the output scale with the same optical size
        self.painter.renderer.text.set_scale(1.0);
        let text = RefCell::new(&mut self.painter.renderer.text);
        let menu = Menu::open(items, at, bounds, &|label| {
            f64::from(
                text.borrow_mut()
                    .measure(label, menu_metrics::TEXT_SIZE as f32, 400, 0.0),
            )
        });
        self.menu = Some(OpenMenu {
            menu,
            levels: Vec::new(),
        });
        self.sync_menu()?;
        self.sync_menu_mods()
    }

    /// Give the menu one popup per open level, destroying those of closed levels deepest first, and draw them.
    fn sync_menu(&mut self) -> Result<()> {
        let Some(open) = self.menu.as_mut() else {
            return Ok(());
        };
        let wanted = open.menu.levels();
        while open.levels.len() > wanted {
            let MenuLevel {
                popup, backdrop, ..
            } = open
                .levels
                .pop()
                .expect("the loop runs while a level is open");
            drop(backdrop);
            self.conn.destroy(popup.surface())?;
        }
        while open.levels.len() < wanted {
            let level = open.levels.len();
            let parent = level
                .checked_sub(1)
                .map_or(self.window.toplevel.surface(), |below| {
                    open.levels[below].popup.surface()
                });
            let placement = popup::placement(&popup_rect(&open.menu.rect_in_parent(level)), true)?;
            let popup = self.conn.popup(parent, &placement)?;
            let backdrop = self.effects.backdrop(&self.conn, popup.surface())?;
            backdrop.set_material(MENU_MATERIAL);
            let own = open.menu.layout(level).rect;
            backdrop.set_shape(
                &PopupRect::new(0.0, 0.0, own.w, own.h),
                menu_metrics::RADIUS,
            )?;
            open.levels.push(MenuLevel {
                popup,
                presenter: ShmPresenter::new(popup.surface()),
                backdrop,
            });
        }
        for level in &mut open.levels {
            level.presenter.request_redraw();
        }
        Ok(())
    }

    fn close_menu(&mut self) -> Result<()> {
        if let Some(mut open) = self.menu.take() {
            while let Some(MenuLevel {
                popup, backdrop, ..
            }) = open.levels.pop()
            {
                drop(backdrop);
                self.conn.destroy(popup.surface())?;
            }
        }
        Ok(())
    }

    /// The level of the open menu that `surface` is the popup of.
    fn menu_level(&self, surface: SurfaceId) -> Option<usize> {
        self.menu
            .as_ref()?
            .levels
            .iter()
            .position(|level| level.popup.surface() == surface)
    }

    fn draw_menu(&mut self, level: usize) -> Result<()> {
        let App {
            conn,
            painter,
            palette,
            menu,
            ..
        } = self;
        let Some(OpenMenu { menu, levels }) = menu.as_mut() else {
            return Ok(());
        };
        levels[level].presenter.draw(conn, None, |pm, scale| {
            painter.paint_menu(pm, scale.factor() as f32, menu, level, palette)
        })?;
        Ok(())
    }

    /// Pointer input on the popup of `level`: hovering highlights, a left press picks.
    fn menu_pointer(&mut self, level: usize, event: PointerEvent) -> Result<()> {
        let now = self.now();
        let Some(open) = self.menu.as_mut() else {
            return Ok(());
        };
        let origin = open.menu.layout(level).rect;
        let at = |x: f64, y: f64| Point {
            x: x + origin.x,
            y: y + origin.y,
        };
        let (mut changed, mut outcome) = (false, Outcome::Nothing);
        match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                let row = open.menu.hit(level, at(p.x, p.y));
                changed = open.menu.hover(level, row, now);
            }
            // the row that holds an open submenu stays highlighted while the pointer is in it
            PointerEvent::Leave if level + 1 == open.menu.levels() => {
                changed = open.menu.hover(level, None, now)
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                let p = at(position.x, position.y);
                if let Some(row) = open.menu.hit(level, p) {
                    outcome = open.menu.press_at(level, row, p);
                }
            }
            PointerEvent::Leave | PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {}
        }
        self.menu_outcome(outcome, changed)
    }

    /// Arrow keys, Return, Space, Escape, Super+. and typed text while a menu is open; every other key is swallowed.
    fn menu_key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        let key = match code {
            key::UP => MenuKey::Nav(Nav::Up),
            key::DOWN => MenuKey::Nav(Nav::Down),
            key::LEFT => MenuKey::Nav(Nav::Left),
            key::RIGHT => MenuKey::Nav(Nav::Right),
            key::ENTER | key::KEYPAD_ENTER => MenuKey::Nav(Nav::Enter),
            key::ESCAPE => MenuKey::Cancel,
            _ if mods.logo && !mods.ctrl && !mods.alt && character == Some('.') => MenuKey::Cancel,
            _ if mods.logo || mods.ctrl || mods.alt => return Ok(()),
            _ if character == Some(' ') => MenuKey::Space,
            _ => match character.filter(|c| !c.is_control()) {
                Some(c) => MenuKey::Type(c),
                None => return Ok(()),
            },
        };
        let now = self.now();
        let outcome = self
            .menu
            .as_mut()
            .map_or(Outcome::Nothing, |open| open.menu.press_key(key, now));
        self.menu_outcome(outcome, false)
    }

    fn menu_outcome(&mut self, outcome: Outcome, changed: bool) -> Result<()> {
        match outcome {
            Outcome::Run(command) => {
                self.close_menu()?;
                self.run(command)
            }
            Outcome::Close => self.close_menu(),
            Outcome::Changed => self.sync_menu(),
            Outcome::Nothing if changed => self.sync_menu(),
            Outcome::Nothing => Ok(()),
        }
    }

    fn run(&mut self, command: MenuCommand) -> Result<()> {
        match command {
            MenuCommand::Open => self.open_selection(),
            MenuCommand::OpenWith(id) => self.open_selection_with(&id),
            MenuCommand::OpenInNewWindow => {
                self.open_in_new_windows();
                Ok(())
            }
            MenuCommand::ShowOriginal => self.show_original(),
            MenuCommand::PutBack => {
                self.put_back();
                Ok(())
            }
            MenuCommand::MoveToTrash => self.trash_selection(),
            MenuCommand::Rename => {
                let only = self
                    .browser
                    .selection()
                    .indices()
                    .next()
                    .filter(|_| self.browser.selection().len() == 1);
                if let Some(index) = only {
                    self.begin_edit(index);
                    self.window.redraw();
                }
                Ok(())
            }
            MenuCommand::NewFolder => {
                match operations::new_folder(self.browser.dir()) {
                    Ok(path) => self.edit_new_folder(path),
                    Err(e) => eprintln!(
                        "cupertino-files: cannot create a folder in {}: {e}",
                        self.browser.dir().display()
                    ),
                }
                Ok(())
            }
            MenuCommand::NewFolderWithSelection => {
                let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
                match operations::new_folder_with(self.browser.dir(), &paths) {
                    Ok(path) => self.edit_new_folder(path),
                    Err(e) => eprintln!(
                        "cupertino-files: cannot move the selection into a new folder: {e}"
                    ),
                }
                Ok(())
            }
            MenuCommand::Duplicate => {
                let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
                for (path, result) in paths.iter().zip(operations::duplicate(&paths)) {
                    if let Err(e) = result {
                        eprintln!("cupertino-files: cannot duplicate {}: {e}", path.display());
                    }
                }
                Ok(())
            }
            MenuCommand::MakeAlias => {
                let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
                for path in paths {
                    if let Err(e) = operations::make_alias(&path) {
                        eprintln!(
                            "cupertino-files: cannot make an alias of {}: {e}",
                            path.display()
                        );
                    }
                }
                Ok(())
            }
            MenuCommand::SortBy(key, ascending) => {
                self.browser.set_sort(key, ascending);
                self.window.redraw();
                Ok(())
            }
            MenuCommand::ToggleTag(color) => {
                self.toggle_tag(color);
                self.window.redraw();
                Ok(())
            }
            MenuCommand::SetView(mode) => self.set_mode(mode),
        }
    }

    /// A folder was made: the listing selects it when it shows it, and its name is edited then.
    fn edit_new_folder(&mut self, path: PathBuf) {
        self.browser.select_when_appears(path.clone());
        self.edit_when_listed = Some(path);
    }

    /// Start a window of this program on each selected folder.
    fn open_in_new_windows(&self) {
        let folders: Vec<PathBuf> = self
            .selected_entries()
            .filter(|e| e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        self.open_windows(folders);
    }

    /// Open a window of its own for each of `folders`.
    fn open_windows(&self, folders: Vec<PathBuf>) {
        for folder in folders {
            let mut command = match std::env::current_exe() {
                Ok(exe) => Command::new(exe),
                Err(e) => {
                    return eprintln!(
                        "cupertino-files: cannot find this program to start a window: {e}"
                    )
                }
            };
            if let Some(program) = self.terminal.as_ref().and_then(|terminal| terminal.first()) {
                command.arg("--terminal").arg(program);
            }
            command.arg(folder);
            launch::spawn(APP_ID, command);
        }
    }

    /// What the menu bar's items depend on now.
    fn current_menu_state(&self) -> MenuState {
        let in_trash = self.browser.dir() == self.trash_files;
        let subject = self.subject(in_trash);
        let history = self.browser.history();
        MenuState {
            count: subject.count,
            all_folders: subject.count > 0 && subject.all_folders,
            single_link: subject.single_link,
            in_trash,
            path_bar: self.bars.path,
            status_bar: self.bars.status,
            full_screen: self.window.fullscreen,
            can_go_back: history.back_target().is_some(),
            can_go_forward: history.forward_target().is_some(),
            has_parent: self.browser.dir().parent().is_some(),
            eject: removable_holding(self.browser.dir(), &self.places)
                .filter(|&i| self.places[i].ejectable())
                .map(|i| self.places[i].name.clone()),
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
        let menus = self.menus.as_mut().expect("checked above");
        let last = self.menu_state.clone().unwrap_or_default();
        if state != last {
            let shape = |s: &MenuState| {
                (
                    s.in_trash,
                    s.path_bar,
                    s.status_bar,
                    s.full_screen,
                    s.eject.clone(),
                )
            };
            if shape(&state) != shape(&last) {
                menus.update(&menu_bar::menu_bar(&state));
            }
            let before = menu_bar::enabled(&last);
            for (i, (action, on)) in menu_bar::enabled(&state).into_iter().enumerate() {
                if shape(&state) != shape(&last) || before[i].1 != on {
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

    /// Carry out an item of the menu bar, or its key.
    fn menu_action(&mut self, action: MenuAction) -> Result<()> {
        match action {
            MenuAction::CloseWindow => self.closing = true,
            MenuAction::NewWindow => self.open_windows(vec![self.browser.dir().to_path_buf()]),
            MenuAction::NewFolder => self.run(MenuCommand::NewFolder)?,
            MenuAction::NewFolderWithSelection => self.run(MenuCommand::NewFolderWithSelection)?,
            MenuAction::Open => self.open_selection()?,
            MenuAction::OpenInNewWindow => self.run(MenuCommand::OpenInNewWindow)?,
            MenuAction::MoveToTrash => self.trash_selection()?,
            MenuAction::PutBack => self.run(MenuCommand::PutBack)?,
            MenuAction::Rename => self.run(MenuCommand::Rename)?,
            MenuAction::Duplicate => self.run(MenuCommand::Duplicate)?,
            MenuAction::MakeAlias => self.run(MenuCommand::MakeAlias)?,
            MenuAction::ShowOriginal => self.run(MenuCommand::ShowOriginal)?,
            MenuAction::Eject => {
                if let Some(i) = removable_holding(self.browser.dir(), &self.places) {
                    self.eject(i)?;
                }
            }
            MenuAction::SelectAll => {
                self.browser.select_all();
                self.window.redraw();
            }
            MenuAction::View(mode) => self.set_mode(mode)?,
            MenuAction::SortBy(key, ascending) => self.run(MenuCommand::SortBy(key, ascending))?,
            MenuAction::TogglePathBar => self.toggle_bar(true)?,
            MenuAction::ToggleStatusBar => self.toggle_bar(false)?,
            MenuAction::FullScreen => {
                let fullscreen = !self.window.fullscreen;
                self.conn.set_fullscreen(self.window.toplevel, fullscreen)?;
            }
            MenuAction::Back => {
                let outcome = self.browser.back();
                self.navigated(outcome, "the previous folder")?;
            }
            MenuAction::Forward => {
                let outcome = self.browser.forward();
                self.navigated(outcome, "the next folder")?;
            }
            MenuAction::EnclosingFolder => {
                let outcome = self.browser.parent();
                self.navigated(outcome, "the enclosing folder")?;
            }
        }
        Ok(())
    }

    /// Go to the folder of the selected link's target and select the target.
    fn show_original(&mut self) -> Result<()> {
        let Some(link) = self.selected_entries().next().map(|e| e.path.clone()) else {
            return Ok(());
        };
        let target = match std::fs::canonicalize(&link) {
            Ok(target) => target,
            Err(e) => {
                eprintln!(
                    "cupertino-files: the original of {} is not there: {e}",
                    link.display()
                );
                return Ok(());
            }
        };
        let Some(parent) = target.parent() else {
            return Ok(());
        };
        // after the step: going to another folder ends a wait
        self.go(parent.to_path_buf())?;
        self.browser.select_when_appears(target);
        Ok(())
    }

    /// Put the selected items of the trash back where they came from.
    fn put_back(&mut self) {
        let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
        let trash = Trash::new();
        let listed = match trash.list() {
            Ok(listed) => listed,
            Err(e) => return eprintln!("cupertino-files: cannot read the trash: {e}"),
        };
        for path in paths {
            match listed.iter().find(|item| item.file_path() == path) {
                Some(item) => {
                    if let Err(e) = trash.restore(item) {
                        eprintln!("cupertino-files: cannot put {} back: {e}", path.display());
                    }
                }
                None => eprintln!("cupertino-files: {} has no trash record", path.display()),
            }
        }
    }

    /// Add the tag of `color` to every selected item, or take it off when they all have it.
    fn toggle_tag(&mut self, color: TagColor) {
        let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
        let all_have = self
            .selected_entries()
            .all(|e| e.tags.iter().any(|t| t.eq_ignore_ascii_case(color.title())));
        for path in paths {
            let result = if all_have {
                tags::remove_tag(&path, color.title())
            } else {
                tags::add_tag(&path, color.title())
            };
            if let Err(e) = result {
                eprintln!(
                    "cupertino-files: cannot change the tags of {}: {e}",
                    path.display()
                );
            }
        }
    }

    /// Move the selected items to the trash (freedesktop Trash 1.0); the listing drops them when the watcher reports it.
    fn trash_selection(&mut self) -> Result<()> {
        let paths: Vec<PathBuf> = self.selected_entries().map(|e| e.path.clone()).collect();
        trash_all(&paths);
        Ok(())
    }

    /// Edit the name of item `index` in place, if it is the only selected item.
    fn begin_edit(&mut self, index: usize) {
        if self.browser.selection().len() != 1 || !self.browser.selection().contains(index) {
            return;
        }
        if let Some(entry) = self.browser.entries().get(index) {
            self.edit = Some((
                NameEdit::new(index, &entry.name, entry.is_dir),
                entry.path.clone(),
            ));
        }
    }

    /// Whether `at` is on the name editor.
    fn in_editor(&self, at: Point) -> bool {
        let (Some((edit, _)), Some(frame)) = (&self.edit, &self.frame) else {
            return false;
        };
        frame.name_frame(edit.index()).is_some_and(|r| {
            cupertino_files::Rect::new(r.x - 6.0, r.y - 4.0, r.w + 12.0, r.h + 8.0).contains(at)
        })
    }

    /// Commit the edited name. `false` when the name is not valid: it is reported and the editor stays open. A rename
    /// that changes the extension is not confirmed (there is no dialog to ask in).
    fn end_edit(&mut self) -> Result<bool> {
        let Some((edit, path)) = self.edit.take() else {
            return Ok(true);
        };
        self.window.redraw();
        match edit.commit() {
            Ok(None) => {}
            Ok(Some(name)) => match operations::rename_item(&path, &name) {
                Ok(_) => self.browser.select_when_appears(path.with_file_name(&name)),
                Err(e) => eprintln!(
                    "cupertino-files: cannot rename {} to {name}: {e}",
                    path.display()
                ),
            },
            Err(e) => {
                eprintln!("cupertino-files: {:?} is not a name: {e:?}", edit.text());
                self.edit = Some((edit, path));
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Keys while a name is edited: Escape cancels, Return commits, the rest goes to the text field.
    fn edit_key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        match code {
            key::ESCAPE => self.edit = None,
            key::ENTER | key::KEYPAD_ENTER => {
                self.end_edit()?;
            }
            _ => {
                let Some((edit, _)) = self.edit.as_mut() else {
                    return Ok(());
                };
                let input =
                    KeyInput::new(code, mods.shift, mods.ctrl, mods.alt, mods.logo, character);
                match edit
                    .field_mut()
                    .key(&mut self.keymap, input, Instant::now())
                {
                    Response::Ignored => return Ok(()),
                    Response::Moved | Response::Edited => {}
                    Response::Copy(text) | Response::Cut(text) => {
                        if let Err(e) = self.clipboard.set_text(&mut self.conn, text) {
                            eprintln!("cupertino-files: copy: {e}");
                        }
                    }
                    Response::Paste => match self.clipboard.text(&mut self.conn) {
                        Ok(Some(text)) => edit.field_mut().paste(&text),
                        Ok(None) => {}
                        Err(e) => eprintln!("cupertino-files: paste: {e}"),
                    },
                }
            }
        }
        self.window.redraw();
        Ok(())
    }

    /// A press in the name editor places the caret there (Shift extends; a double press selects a word, a triple
    /// press the whole name).
    fn editor_press(&mut self, at: Point) -> Result<()> {
        let extend = self.conn.modifiers().shift;
        if let Some((edit, _)) = self.edit.as_mut() {
            if let Some(index) = self.painter.editor_index(edit.field(), at) {
                edit.field_mut().press(index, extend, Instant::now());
                self.window.redraw();
            }
        }
        Ok(())
    }

    /// Whether the search field's caret shows now: it has the keyboard (no name is being edited), nothing is selected
    /// and the blink is on.
    fn search_caret(&self) -> bool {
        self.search_focused && self.edit.is_none() && self.search.caret_visible(Instant::now())
    }

    /// Whether a caret shows now, the name editor's or the search field's.
    fn caret_shown(&self) -> bool {
        self.edit_caret() || self.search_caret()
    }

    /// Where a caret blink changes the window, `None` when that is not known.
    fn caret_area(&self) -> Option<Vec<PopupRect>> {
        cupertino_files::window::caret_areas(
            self.frame.as_ref()?,
            self.edit.as_ref().map(|(e, _)| e.index()),
        )
    }

    /// Whether the name editor's caret shows now (nothing selected, the blink on).
    fn edit_caret(&self) -> bool {
        self.edit
            .as_ref()
            .is_some_and(|(e, _)| e.field().caret_visible(Instant::now()))
    }

    /// The pointer moved with the left button down on an item: past the start distance the selection is dragged, and
    /// the folder or place under the pointer is the target.
    fn drag_motion(&mut self, at: Point) -> Result<()> {
        if self.search.is_dragging() {
            if let Some(index) = self.painter.search_index(&self.search, at) {
                self.search.drag_to(index);
                self.window.redraw();
            }
            return Ok(());
        }
        if let Some((edit, _)) = self.edit.as_mut().filter(|(e, _)| e.field().is_dragging()) {
            if let Some(index) = self.painter.editor_index(edit.field(), at) {
                edit.field_mut().drag_to(index);
                self.window.redraw();
            }
            return Ok(());
        }
        if let Some(marquee) = self.marquee.as_mut() {
            let (origin, from) = (marquee.origin, marquee.at);
            marquee.at = at;
            let before = self.selected_indices();
            let selection = self.marquee_apply();
            self.redraw_marquee(origin, from, at, selection.then_some(&before[..]));
            return Ok(());
        }
        let Some(drag) = &self.drag else {
            return Ok(());
        };
        if !drag.started && (at.x - drag.press.x).hypot(at.y - drag.press.y) < drag::START_DISTANCE
        {
            return Ok(());
        }
        let sources = if drag.started {
            drag.sources.clone()
        } else {
            self.selected_entries().map(|e| e.path.clone()).collect()
        };
        let target = self.drop_target(at, &sources);
        if let Some(drag) = self.drag.as_mut() {
            if !drag.started {
                self.rename.cancel();
            }
            let outlined = |t: &Option<DropTarget>| t.as_ref().map(|t| t.outline);
            let moved = (outlined(&drag.target) != outlined(&target))
                .then(|| [outlined(&drag.target), outlined(&target)]);
            (drag.started, drag.sources, drag.target) = (true, sources, target);
            if let Some(outlines) = moved {
                self.redraw_outlines(outlines.into_iter().flatten());
            }
        }
        Ok(())
    }

    /// The rubber band from `origin` moved from `from` to `to`: draw where it was and where it is, and the items whose
    /// selection it changed from `before`.
    fn redraw_marquee(&mut self, origin: Point, from: Point, to: Point, before: Option<&[usize]>) {
        let Some(frame) = &self.frame else {
            self.window.redraw();
            return;
        };
        let areas: Vec<PopupRect> = [from, to]
            .into_iter()
            .filter_map(|at| cupertino_files::window::marquee_area(frame, origin, at))
            .collect();
        for area in areas {
            self.window.redraw_rect(area);
        }
        if let Some(before) = before {
            self.redraw_items(before);
        }
    }

    /// The drop look left or reached `outlines`: draw them.
    fn redraw_outlines(&mut self, outlines: impl Iterator<Item = DropOutline>) {
        let Some(frame) = &self.frame else {
            self.window.redraw();
            return;
        };
        let areas: Vec<PopupRect> = outlines
            .map(|o| match o {
                DropOutline::Item(i) => cupertino_files::window::item_area(frame, i),
                DropOutline::Place(i) => cupertino_files::window::place_area(frame, i),
            })
            .collect::<Option<_>>()
            .unwrap_or_default();
        if areas.is_empty() {
            self.window.redraw();
        }
        for area in areas {
            self.window.redraw_rect(area);
        }
    }

    /// Select what the band touches in the frame drawn last; `true` when the selection changed.
    fn marquee_apply(&mut self) -> bool {
        let (Some(marquee), Some(frame)) = (&self.marquee, &self.frame) else {
            return false;
        };
        let held = self.conn.modifiers();
        let band =
            cupertino_files::Rect::from_corners(marquee.origin, frame.to_content(marquee.at));
        let hits = frame.hits_in(&band);
        self.browser.rubber_band_update(
            &hits,
            Modifiers {
                shift: held.shift,
                primary: held.logo,
            },
        )
    }

    /// While the pointer is held within the edge zone of the content, scroll toward it a step that grows with the depth
    /// into the zone, once per interval.
    fn marquee_autoscroll(&mut self, now: f64) -> Result<()> {
        let (Some(marquee), Some(frame)) = (self.marquee.as_mut(), &self.frame) else {
            return Ok(());
        };
        if now < marquee.next_scroll {
            return Ok(());
        }
        marquee.next_scroll = now + marquee_metrics::AUTOSCROLL_INTERVAL;
        let content = frame.window.content;
        let top = content.y + frame.content.fixed_top();
        let ramp = |depth: f64| {
            marquee_metrics::MAX_STEP * depth.clamp(0.0, marquee_metrics::EDGE_ZONE)
                / marquee_metrics::EDGE_ZONE
        };
        let step = if marquee.at.y < top + marquee_metrics::EDGE_ZONE {
            -ramp(top + marquee_metrics::EDGE_ZONE - marquee.at.y)
        } else if marquee.at.y > content.bottom() - marquee_metrics::EDGE_ZONE {
            ramp(marquee.at.y - (content.bottom() - marquee_metrics::EDGE_ZONE))
        } else {
            0.0
        };
        if step != 0.0
            && ((step < 0.0 && frame.scroll > 0.0)
                || (step > 0.0 && frame.scroll < frame.max_scroll()))
        {
            self.pending_scroll += step;
            self.window.redraw();
        }
        Ok(())
    }

    /// The folder item or sidebar place at `at` that `sources` can be dropped on: not one of them.
    fn drop_target(&self, at: Point, sources: &[PathBuf]) -> Option<DropTarget> {
        let frame = self.frame.as_ref()?;
        let trash_files = Trash::new().home_trash().join("files");
        let (path, outline) = match frame.hit(at) {
            Hit::Item(i) | Hit::Disclosure(i) => {
                let entry = self.browser.entries().get(i)?;
                (
                    entry.is_dir.then(|| entry.path.clone())?,
                    DropOutline::Item(i),
                )
            }
            Hit::Place(i) => (
                self.places.get(i)?.folder()?.to_path_buf(),
                DropOutline::Place(i),
            ),
            _ => return None,
        };
        let trash = path == trash_files;
        (!sources.contains(&path)).then_some(DropTarget {
            path,
            outline,
            trash,
        })
    }

    /// The left button went up: end a drag by dropping on its target, else finish the click.
    fn release(&mut self) -> Result<()> {
        self.search.release();
        if let Some((edit, _)) = self.edit.as_mut() {
            edit.field_mut().release();
        }
        if self.marquee.take().is_some() {
            self.browser.rubber_band_end();
            self.window.redraw();
            return Ok(());
        }
        match self.drag.take() {
            Some(drag) if drag.started => {
                self.browser.end_drag();
                self.window.redraw();
                self.drop_items(drag)
            }
            _ => {
                if self.browser.release() {
                    self.window.redraw();
                }
                Ok(())
            }
        }
    }

    /// Drop the dragged items on the target: into the trash for the trash place, else moved or copied (Alt copies,
    /// Super moves, otherwise by volume).
    fn drop_items(&mut self, drag: Drag) -> Result<()> {
        let Some(target) = drag.target else {
            return Ok(());
        };
        if target.trash {
            trash_all(&drag.sources);
            return Ok(());
        }
        let same_volume = drag
            .sources
            .iter()
            .map(|source| operations::same_volume(source, &target.path))
            .collect::<io::Result<Vec<_>>>();
        let same_volume = match same_volume {
            Ok(each) => each.into_iter().all(|same| same),
            Err(e) => {
                eprintln!(
                    "cupertino-files: cannot drop on {}: {e}",
                    target.path.display()
                );
                return Ok(());
            }
        };
        let held = self.conn.modifiers();
        let action = operations::drop_action(same_volume, held.alt, held.logo);
        for (source, result) in
            drag.sources
                .iter()
                .zip(operations::drop_into(&drag.sources, &target.path, action))
        {
            if let Err(e) = result {
                eprintln!(
                    "cupertino-files: cannot drop {} on {}: {e}",
                    source.display(),
                    target.path.display()
                );
            }
        }
        Ok(())
    }
}

/// Move each of `paths` to the trash; a failure is reported and the others still go.
fn trash_all(paths: &[PathBuf]) {
    let trash = Trash::new();
    for path in paths {
        if let Err(e) = trash.trash(path) {
            eprintln!(
                "cupertino-files: cannot move {} to the trash: {e}",
                path.display()
            );
        }
    }
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
}
