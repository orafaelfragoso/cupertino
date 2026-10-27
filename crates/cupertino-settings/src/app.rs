//! Glue between the settings window model and the toolkit: one application window with the unified title bar style
//! (the surface is the whole window; the server draws the window control buttons over the sidebar and the material
//! under it), the sidebar, the search field with its results popup, history, the keyboard of `keys`, and the panes:
//! their controls commit on the moments of §7.2 and write through `cupertino-defaults` or call `org.cupertino.Session1`.

use std::error::Error;

use cupertino_assets::Assets;
use cupertino_client::{
    Backdrop, Button, Client, Event, Modifiers as KeyModifiers, PointerEvent, SurfaceId,
    TitlebarStyle, ToplevelConfig,
};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_geometry::Rect as PopupRect;
use cupertino_settings::agent::{Agent, AgentEvent};
use cupertino_settings::controls::{self, Control, Press, Value};
use cupertino_settings::form::{self, RowSpec, SectionLayout, SectionSpec};
use cupertino_settings::history::History;
use cupertino_settings::instance;
use cupertino_settings::keys::{self, Action, Focus, Mods, SearchKey};
use cupertino_settings::live::Live;
use cupertino_settings::paint::{menu_rows, popup_hit, Painter, Palette, PaneView, WindowState};
use cupertino_settings::panes::{self, Call, ControlId, Effect, Section, Session};
use cupertino_settings::search::{popup, SearchField};
use cupertino_settings::services::Services;
use cupertino_settings::settings::{Paths, Settings};
use cupertino_settings::sheet::{Answer, Outcome, Sheet, SheetKey};
use cupertino_settings::sidebar::{self, RowSize};
use cupertino_settings::window::{self, metrics, Hit, WindowLayout};
use cupertino_settings::{PaneId, Point, Size};
use cupertino_theme::{Appearance, Theme, WindowCornerSize};

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

/// The session bus service that applies the appearance (the server's, `quit.rs`).
const SESSION_NAME: &str = "org.cupertino.Session1";
const SESSION_PATH: &str = "/org/cupertino/Session1";

/// A pop-up button's open menu of choices.
struct ChoiceMenu {
    surface: SurfaceId,
    backdrop: Backdrop,
    id: ControlId,
    choices: Vec<String>,
    selected: usize,
    hovered: Option<usize>,
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
    surface: SurfaceId,
    backdrop: Backdrop,
    /// The size it was created or last moved with.
    size: (f64, f64),
    hovered: Option<usize>,
}

struct App {
    client: Client,
    window: SurfaceId,
    sidebar: Backdrop,
    theme: Theme,
    palette: Palette,
    painter: Painter,
    groups: Vec<Vec<PaneId>>,
    history: History<PaneId>,
    search: SearchField,
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
    /// The open pairing question's answer, sent when its sheet decides.
    agent_answer: Option<(u64, async_channel::Sender<Answer>)>,
    /// Whether this window's pairing agent is registered.
    agent_registered: bool,
    /// Watches the settings files while it lives; `None` when the watch could not be set up.
    _files: Option<ConfigWatcher>,
    /// Owns `org.cupertino.Settings1` while it lives.
    _instance: zbus::Connection,
}

/// Something for the window loop from another thread.
enum Background {
    ShowPane(PaneId),
    Agent(AgentEvent),
    /// A pairing ended.
    Paired(std::result::Result<(), String>),
    /// A settings file changed on disk.
    Files(WatchEvent),
}

/// Hands a [`Background`] to the window loop and wakes it.
type Notify = std::sync::Arc<dyn Fn(Background) + Send + Sync>;

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
pub fn run(pane: PaneId) -> Result<()> {
    // one process per session: a second start hands its pane to the first and ends (§8); before the window, so it
    // needs no display
    let (sender, background) = std::sync::mpsc::channel();
    let waker = std::sync::Arc::new(std::sync::OnceLock::<cupertino_client::Waker>::new());
    let wake = waker.clone();
    let notify: Notify = std::sync::Arc::new(move |event| {
        // the window loop owns the receiver until it ends; what arrives after that has no window. What arrives before
        // the window exists waits in the channel for the loop's first turn.
        if sender.send(event).is_ok() {
            if let Some(waker) = wake.get() {
                waker.wake();
            }
        }
    });
    let to_loop = notify.clone();
    let instance = match instance::serve(zbus::connection::Builder::session()?, move |pane| {
        to_loop(Background::ShowPane(pane))
    }) {
        Ok(connection) => connection,
        Err(zbus::Error::NameTaken) => {
            return Ok(instance::show(
                zbus::connection::Builder::session()?,
                pane.key(),
            )?)
        }
        Err(e) => return Err(format!("cannot claim {}: {e}", instance::BUS_NAME).into()),
    };
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let painter = Painter::new(&assets)?;
    let mut client = Client::connect()?;
    assert!(waker.set(client.waker()).is_ok(), "the waker is set once");
    let size = window::default_size(WIDTH_FACTOR);
    let window = client.toplevel(&ToplevelConfig {
        app_id: APP_ID.into(),
        title: pane.pane().title.into(),
        size: (size.w as u32, size.h as u32),
        // the width is fixed by the content; the height's minimum is approximate (O6)
        min_size: (size.w as u32, metrics::MIN_HEIGHT as u32),
        titlebar: TitlebarStyle::Unified,
    })?;
    let sidebar = client.backdrop(window)?;
    sidebar.set_material(SIDEBAR_MATERIAL);
    // the dock's default tile size depends on the height of the display it sits on
    let display_height = client
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
        palette: palette(&theme, client.is_dark(), client.display_options())?,
        client,
        window,
        sidebar,
        theme,
        painter,
        groups: sidebar::groups(),
        history: History::new(pane),
        search: SearchField::default(),
        results: None,
        focus: Focus::Sidebar,
        layout: None,
        pressed_row: None,
        showing: pane,
        last_titlebar_press: None,
        paths,
        settings,
        errors: Vec::new(),
        content_pane: pane,
        scroll: 0.0,
        form: Vec::new(),
        form_height: 0.0,
        drag: None,
        menu: None,
        dark: false,
        bus: None,
        services: Services::new(),
        live: Live::default(),
        sheet: None,
        level: None,
        start: std::time::Instant::now(),
        refreshed: f64::NEG_INFINITY,
        scanned: f64::NEG_INFINITY,
        background,
        notify,
        agent_answer: None,
        agent_registered: false,
        _files: files,
        _instance: instance,
    };
    app.dark = app.client.is_dark();
    app.enter_pane(None, pane)?;
    loop {
        let timeout = app.timeout();
        for event in app.client.wait(Some(timeout))? {
            if !app.handle(event)? {
                app.leave_pane(app.content_pane);
                return Ok(());
            }
        }
        while let Ok(event) = app.background.try_recv() {
            app.background_event(event)?;
        }
        app.tick()?;
    }
}

impl App {
    /// React to one event; `false` once the window is to close.
    fn handle(&mut self, event: Event) -> Result<bool> {
        match event {
            // a draw follows every configure and scale change
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(surface) if Some(surface) == self.results.as_ref().map(|r| r.surface) => {
                self.draw_results()?
            }
            Event::Draw(surface) if Some(surface) == self.menu.as_ref().map(|m| m.surface) => {
                self.draw_menu()?
            }
            Event::Draw(_) => self.draw()?,
            Event::Closed(_) => return Ok(false),
            Event::Pointer { surface, event }
                if Some(surface) == self.results.as_ref().map(|r| r.surface) =>
            {
                self.results_pointer(event)?
            }
            Event::Pointer { surface, event }
                if Some(surface) == self.menu.as_ref().map(|m| m.surface) =>
            {
                self.menu_pointer(event)?
            }
            Event::Pointer { event, .. } => self.pointer(event)?,
            Event::Key {
                key,
                pressed: true,
                mods,
                character,
                ..
            } => self.key(key, mods, character)?,
            Event::Key { .. } | Event::KeyboardLeave(_) => {}
            Event::Appearance { dark } => {
                self.dark = dark;
                self.palette = palette(&self.theme, dark, self.client.display_options())?;
                self.close_menu()?;
                self.redraw()?;
            }
            Event::DisplayOptions(options) => {
                self.palette = palette(&self.theme, self.client.is_dark(), options)?;
                self.redraw()?;
            }
            // the server dismissed the popup (a click outside it): the field keeps its text
            Event::PopupDone(surface) => {
                // a click on another client or the desktop ends the search, so the field is never open without its
                // popup. The click itself reaches what it hit (the grab forwards it); a click in this window keeps the
                // grab and reaches the window directly.
                if self.results.as_ref().is_some_and(|r| r.surface == surface) {
                    self.search.clear();
                    self.close_results()?;
                    self.client.request_redraw(self.window)?;
                }
                if self.menu.as_ref().is_some_and(|m| m.surface == surface) {
                    self.close_menu()?;
                }
            }
            Event::OutputsChanged | Event::SessionLock { .. } | Event::ActivationToken { .. } => {}
            Event::MenuBar { .. } | Event::Dock { .. } => {
                return Err(
                    "an event for a menu bar or dock arrived, and this process has none".into(),
                )
            }
        }
        Ok(true)
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Until the next refresh, or the level slider's waiting write.
    fn timeout(&self) -> std::time::Duration {
        let now = self.now();
        let mut due = self.refreshed + REFRESH;
        if let Some((_, _, _, w)) = &self.level {
            if let Some(t) = w.due() {
                due = due.min(t);
            }
        }
        std::time::Duration::from_secs_f64((due - now).clamp(0.0, REFRESH))
            + std::time::Duration::from_millis(1)
    }

    /// Run what is due: the level slider's waiting write, the refresh of the live readings, the Wi-Fi scan.
    fn tick(&mut self) -> Result<()> {
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
            PaneId::Bluetooth => {
                let keep = self.live.bluetooth.ready().cloned();
                self.live.bluetooth = self.services.bluetooth(keep.as_ref());
            }
            _ => {}
        }
        if self.live != before {
            self.client.request_redraw(self.window)?;
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
            self.client.request_redraw(self.window)?;
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

    fn redraw(&mut self) -> Result<()> {
        self.client.request_redraw(self.window)?;
        if let Some(r) = &self.results {
            self.client.request_redraw(r.surface)?;
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let (width, height) = self.client.size(self.window)?;
        let active = self.client.toplevel_activated(self.window)?;
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
                rows: sec
                    .rows
                    .iter()
                    .map(|r| RowSpec {
                        control: &r.control,
                        error: r.error.is_some() || r.disabled.is_some(),
                    })
                    .collect(),
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
        let App {
            client,
            window,
            painter,
            palette,
            history,
            search,
            focus,
            showing,
            scroll,
            sheet,
            ..
        } = self;
        let state = WindowState {
            selected: *showing,
            has_battery,
            active,
            history,
            search,
            search_focused: *focus == Focus::Search,
        };
        let view = PaneView {
            sections: &sections,
            layout: &form,
            scroll: *scroll,
            dragging,
        };
        let content = layout.content;
        client.draw(*window, |pm, scale| {
            painter.paint_window(pm, scale.factor(), &layout, &state, &view, palette);
            if let Some(sheet) = sheet.as_ref() {
                painter.paint_sheet(
                    pm,
                    scale.factor(),
                    sheet,
                    &sheet.layout(&content),
                    &content,
                    palette,
                );
            }
        })?;
        self.layout = Some(layout);
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
            client,
            painter,
            palette,
            search,
            results,
            ..
        } = self;
        let r = results.as_ref().expect("checked above");
        client.draw(r.surface, |pm, scale| {
            painter.paint_popup(pm, scale.factor(), search, &rows, r.hovered, palette)
        })?;
        Ok(())
    }

    /// Show `pane`: select its row, push it on the history and name the window after it.
    fn go(&mut self, pane: PaneId) -> Result<()> {
        self.history.push(pane);
        self.show_current()
    }

    fn show_current(&mut self) -> Result<()> {
        self.close_menu()?;
        let pane = self.history.current();
        self.showing = pane;
        self.client
            .set_title(self.window, pane.title(self.live.has_battery()))?;
        self.client.request_redraw(self.window)?;
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
                    Hit::Search => self.set_focus(Focus::Search)?,
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
                self.client.request_redraw(self.window)?;
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
                    self.client.request_redraw(self.window)?;
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
                self.client.request_redraw(self.window)?;
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
            self.client.toplevel_toggle_maximized(self.window)?;
        } else {
            self.client.toplevel_move(self.window)?;
        }
        Ok(())
    }

    fn set_focus(&mut self, focus: Focus) -> Result<()> {
        if self.focus != focus {
            if self.focus == Focus::Search {
                self.search.deselect();
            }
            self.focus = focus;
            self.client.request_redraw(self.window)?;
        }
        Ok(())
    }

    fn key(&mut self, code: u32, mods: KeyModifiers, character: Option<char>) -> Result<()> {
        if let Some(sheet) = &mut self.sheet {
            let key = match code {
                keys::code::ESCAPE => SheetKey::Escape,
                keys::code::ENTER | keys::code::KEYPAD_ENTER => SheetKey::Return,
                keys::code::BACKSPACE => SheetKey::Backspace,
                _ => match character.filter(|c| !c.is_control()) {
                    Some(c) => SheetKey::Type(c),
                    None => return Ok(()),
                },
            };
            let outcome = sheet.key(key);
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
            Action::ShowAll => self.go(sidebar::rows()[0])?,
            Action::Previous => self.step_row(-1)?,
            Action::Next => self.step_row(1)?,
            Action::First => self.go(sidebar::rows()[0])?,
            Action::Last => self.go(*sidebar::rows().last().expect("the sidebar has rows"))?,
            Action::FocusNext => self.set_focus(keys::cycle(self.focus, true))?,
            Action::FocusPrevious => self.set_focus(keys::cycle(self.focus, false))?,
            Action::FocusSidebar => self.set_focus(Focus::Sidebar)?,
            Action::Search(k) => self.search_key(k)?,
            Action::None => {}
        }
        Ok(())
    }

    /// The row `step` rows from the current one, within the sidebar.
    fn step_row(&mut self, step: isize) -> Result<()> {
        let rows = sidebar::rows();
        let i = rows
            .iter()
            .position(|p| *p == self.history.current())
            .expect("the current pane is a row");
        let to = i.saturating_add_signed(step).min(rows.len() - 1);
        self.go(rows[to])
    }

    fn search_key(&mut self, k: SearchKey) -> Result<()> {
        match k {
            SearchKey::Type(c) => self.search.type_char(c),
            SearchKey::Backspace => self.search.backspace(),
            SearchKey::SelectAll => self.search.select_all(),
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
        self.client.request_redraw(self.window)?;
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
            Some(r) if r.size == size => self.client.request_redraw(r.surface)?,
            Some(r) => {
                r.size = size;
                self.client.reposition_popup(r.surface, &rect)?;
                r.backdrop
                    .set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), popup::RADIUS)?;
            }
            None => {
                let surface = self.client.popup(self.window, &rect)?;
                let backdrop = self.client.backdrop(surface)?;
                backdrop.set_material(POPUP_MATERIAL);
                backdrop.set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), popup::RADIUS)?;
                self.results = Some(Results {
                    surface,
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
            self.client.destroy_surface(r.surface)?;
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
                    self.client.request_redraw(r.surface)?;
                }
            }
            PointerEvent::Leave => {
                r.hovered = None;
                self.client.request_redraw(r.surface)?;
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
                display: self.client.display_options(),
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
                        self.client.request_redraw(self.window)?;
                        return Ok(());
                    }
                    Some(Press::Commit(value)) => return self.commit(row.id, value),
                    Some(Press::Drag(shown)) => {
                        self.drag = Some(SliderDrag {
                            section: si,
                            row: ri,
                            shown,
                        });
                        self.client.request_redraw(self.window)?;
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
            let e = format!("The settings could not be read: {e}.");
            eprintln!("cupertino-settings: {e}");
            self.errors.push((id, e));
            self.client.request_redraw(self.window)?;
            return Ok(());
        }
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
                    self.sheet = Some(Sheet::passphrase(&ssid));
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
                Effect::Store(file) => self
                    .settings
                    .store(&self.paths, file)
                    .map_err(|e| e.to_string()),
                Effect::SetAppearance { dark } => self.set_appearance(dark),
                Effect::SetAccessibility { key, value } => self.set_accessibility(key, &value),
            };
            if let Err(e) = outcome {
                eprintln!("cupertino-settings: {e}");
                self.settings = before;
                self.errors.push((id, e));
                break;
            }
        }
        self.client.request_redraw(self.window)?;
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
        self.client.request_redraw(self.window)?;
        Ok(())
    }

    /// Something from a bus thread: a pane to show, a question of the pairing agent, the end of a pairing.
    fn background_event(&mut self, event: Background) -> Result<()> {
        match event {
            Background::ShowPane(pane) => self.go(pane)?,
            Background::Agent(AgentEvent::Prompt {
                id,
                device,
                prompt,
                answer,
            }) => {
                // a new question replaces an open one: its pairing is over (the daemon pairs one device at a time)
                self.sheet = Some(Sheet::pairing(id, &device, prompt));
                self.agent_answer = answer.map(|a| (id, a));
                self.client.request_redraw(self.window)?;
            }
            Background::Agent(AgentEvent::Cancel(id)) => {
                if self.sheet.as_ref().and_then(Sheet::pairing_id) == Some(id) {
                    self.sheet = None;
                    self.agent_answer = None;
                    self.client.request_redraw(self.window)?;
                }
            }
            Background::Files(WatchEvent::Changed(_)) => {
                if let Err(e) = self.settings.reload(&self.paths) {
                    eprintln!("cupertino-settings: the settings could not be read again: {e}");
                }
                self.client.request_redraw(self.window)?;
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
                self.client.request_redraw(self.window)?;
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
        .map_err(|e| format!("The appearance could not be changed: {e}."))?;
        self.dark = dark;
        Ok(())
    }

    /// The session bus, connected on first use.
    fn bus(&mut self) -> std::result::Result<&zbus::blocking::Connection, String> {
        if self.bus.is_none() {
            self.bus = Some(
                zbus::blocking::Connection::session()
                    .map_err(|e| format!("The session bus is not available: {e}."))?,
            );
        }
        Ok(self.bus.as_ref().expect("connected above"))
    }

    /// Ask the server to set one key of `accessibility.conf` (`org.cupertino.Session1.SetAccessibility`), then read the
    /// file it stored again; the display options themselves arrive from the session.
    fn set_accessibility(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        let bus = self.bus()?;
        bus.call_method(
            Some(SESSION_NAME),
            SESSION_PATH,
            Some(SESSION_NAME),
            "SetAccessibility",
            &(key, value),
        )
        .map_err(|e| format!("The setting could not be changed: {e}."))?;
        if let Some(path) = &self.paths.accessibility {
            let (stored, _) = cupertino_defaults::accessibility::AccessibilitySettings::load(path)
                .map_err(|e| format!("The changed setting could not be read back: {e}."))?;
            self.settings.accessibility = stored;
        }
        Ok(())
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
        let surface = self.client.popup(self.window, &rect)?;
        let backdrop = self.client.backdrop(surface)?;
        backdrop.set_material(POPUP_MATERIAL);
        backdrop.set_shape(&PopupRect::new(0.0, 0.0, size.0, size.1), 6.0)?;
        self.menu = Some(ChoiceMenu {
            surface,
            backdrop,
            id,
            choices,
            selected,
            hovered: None,
        });
        Ok(())
    }

    fn close_menu(&mut self) -> Result<()> {
        if let Some(m) = self.menu.take() {
            drop(m.backdrop);
            self.client.destroy_surface(m.surface)?;
        }
        Ok(())
    }

    fn draw_menu(&mut self) -> Result<()> {
        let App {
            client,
            painter,
            palette,
            menu,
            ..
        } = self;
        let Some(m) = menu.as_ref() else {
            return Ok(());
        };
        client.draw(m.surface, |pm, scale| {
            painter.paint_menu(
                pm,
                scale.factor(),
                &m.choices,
                m.selected,
                m.hovered,
                palette,
            )
        })?;
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
                    self.client.request_redraw(m.surface)?;
                }
            }
            PointerEvent::Leave => {
                m.hovered = None;
                self.client.request_redraw(m.surface)?;
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
