//! Glue between the server, the menu controllers and the session bus: one bar per output, its
//! popups, input routing and timers. The decisions live in the library modules (`feed`,
//! `routing`, `popups`, `raster`); this file only connects them to the toolkit.

use cupertino_apps::launch::{self, Launch, OPEN_COMMAND};
use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_assets::Assets;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_geometry::Rect as GRect;
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_menubar::clock::{clock_text, local_time, next_tick, separators_off, ClockSettings};
use cupertino_menubar::controller::{MenuController, Mode, Outcome};
use cupertino_menubar::desktop::app_name;
use cupertino_menubar::extras::{self, ExtraAction, ExtraKind, Extras, SessionAction};
use cupertino_menubar::extras_bus::{ExtrasBus, SessionAgent};
use cupertino_menubar::feed::{status_icon, status_icon_px, BarFeed, Shared, StatusState};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::layout::{BarMetrics, MenuMetrics};
use cupertino_menubar::popups::{self, PopupOp};
use cupertino_menubar::raster::{BarText, MenuPalette, Style, POPUP_RADIUS};
use cupertino_menubar::recent::RecentSources;
use cupertino_menubar::routing::{self, menu_key, Action, Context};
use cupertino_menubar::worker::{self, Worker};
use cupertino_protocol_app::{AppEffects, Backdrop};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    MenuBar, Session as SessionObserver, SessionChange, SessionState, Shell, ShellEvent,
};
use cupertino_text_field::{KeyInput, Keymap};
use cupertino_theme::Theme;
use cupertino_ui_wayland::{popup, Ready, ShmPresenter};
use cupertino_wayland_client::{
    Button, Connection, Event, KeyboardEvent, OutputGeometry, OutputId, PointerEvent, Popup,
    SurfaceId, TokenRequest,
};
use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
/// Opacity factor of an item symbol whose radio is off or has no link. approximate (the design does not say).
const DIMMED_ALPHA: f32 = 0.4;

const NAMESPACE: &str = "cupertino-menubar";

/// The name the process reports problems under.
const PROCESS: &str = "cupertino-menubar";

/// The popup surface of one open menu level.
struct PopupSurface {
    popup: Popup,
    presenter: ShmPresenter,
    backdrop: Backdrop,
    /// On whole points, in the coordinates of the bar surface.
    rect: Rect,
}

/// The bar of one output.
struct Bar {
    output: OutputId,
    geometry: OutputGeometry,
    layer: LayerSurface,
    presenter: ShmPresenter,
    /// Width of the surface from its last configure.
    width: u32,
    menu_bar: MenuBar,
    backdrop: Backdrop,
    controller: MenuController,
    text: BarText,
    feed: BarFeed,
    /// One per open menu level, the first a popup of the bar.
    popups: Vec<PopupSurface>,
    keyboard: KeyboardInteractivity,
    scale: f64,
}

/// What a surface of the process is.
#[derive(Clone, Copy)]
enum Part {
    Bar,
    Popup(usize),
}

struct App {
    conn: Connection,
    layer_shell: LayerShell,
    effects: AppEffects,
    shell: Shell,
    _observer: SessionObserver,
    /// The session's appearance (for the text palette).
    observed: SessionState,
    worker: Worker,
    events: Receiver<worker::Event>,
    style: Style,
    text: BarText,
    shared: Shared,
    bars: Vec<Bar>,
    icons: ThemeIcons,
    /// The template symbols of the extras.
    symbols: SystemIcons,
    data_dirs: Vec<PathBuf>,
    start: Instant,
    clock: ClockSettings,
    /// `menubar.conf` as last read, the last good values of the next read.
    conf: MenuBarSettings,
    /// Reports of the watch on `menubar.conf`; the watch lives as long as the process (`None` without one).
    conf_changes: Receiver<WatchEvent>,
    _conf_watch: Option<ConfigWatcher>,
    /// Unix seconds at which the clock text is due again.
    next_clock: f64,
    /// The minute of the last reading of the extras' services.
    extras_minute: Option<i64>,
    extras: ExtrasBus,
    extras_events: Receiver<Extras>,
    /// Identifies the next bar's menu in the shared sources.
    next_key: u64,
    /// Launches waiting for their activation token, by request.
    pending: HashMap<TokenRequest, Launch>,
    /// Where Recent Items is read from; `None` when the session's directories are not known.
    recent: Option<RecentSources>,
    /// The editing chords of the Help menu's search field.
    keymap: Keymap,
}

/// `menubar.conf` from the configuration directory (`CUPERTINO_MENUBAR_CONFIG` overrides the path); every key unset
/// when there is none. Lines that are not understood are reported; with `last_good` their keys keep its values.
fn load_conf(last_good: Option<&MenuBarSettings>) -> Result<MenuBarSettings> {
    let Some(path) = MenuBarSettings::path() else {
        return Ok(MenuBarSettings::UNSET);
    };
    let (settings, problems) = MenuBarSettings::load(&path, last_good)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for problem in problems {
        eprintln!(
            "cupertino-menubar: {} line {}: {}",
            path.display(),
            problem.line,
            problem.message
        );
    }
    Ok(settings)
}

/// Watch `menubar.conf`; its changes wake the loop. A watch that cannot be set up is reported: the file then applies at
/// the next start.
fn watch_conf(conn: &Connection) -> (Option<ConfigWatcher>, Receiver<WatchEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some(path) = MenuBarSettings::path() else {
        return (None, rx);
    };
    let waker = conn.waker();
    let watch = ConfigWatcher::new(vec![path.clone()], move |event| {
        // the receiver is gone only while the process exits
        if tx.send(event).is_ok() {
            waker.wake();
        }
    });
    match watch {
        Ok(w) => (Some(w), rx),
        Err(e) => {
            eprintln!(
                "cupertino-menubar: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            (None, rx)
        }
    }
}

/// Connect, claim the bus names and serve until the connection or the bus fails.
pub fn run() -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let conf = load_conf(None)?;
    let bar = BarMetrics {
        height: theme.shapes.menu_bar_height,
        selection_corner_radius: theme.shapes.menu_bar_selection_corner_radius,
        ..BarMetrics::default()
    };
    let text = BarText::new(theme.tokens.typography(), &assets)?;

    let mut conn = Connection::connect()?;
    let layer_shell = LayerShell::bind(&mut conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let shell = Shell::bind(&mut conn).ok_or("the server lacks cupertino_shell_v1")?;
    let (observer, observed, early) = shell.observe(&mut conn)?;
    let (conf_watch, conf_changes) = watch_conf(&conn);
    let style = Style {
        palette: MenuPalette::new(observed.dark),
        bar,
        menu: MenuMetrics::default(),
    };
    let session = zbus::block_on(zbus::Connection::session())
        .map_err(|e| format!("cannot connect to the session bus: {e}"))?;
    let waker = conn.waker();
    let (worker, events) = Worker::start(session.clone(), move || waker.wake())?;
    let waker = conn.waker();
    let (extras, extras_events) = ExtrasBus::start(session, move || waker.wake());
    let recent = match XdgDirs::from_env() {
        Ok(dirs) => Some(RecentSources::new(
            &dirs,
            Locale::from_env(),
            Visibility::from_env(),
        )),
        Err(e) => {
            eprintln!("{PROCESS}: no Recent Items: {e}");
            None
        }
    };
    let mut app = App {
        conn,
        layer_shell,
        effects,
        shell,
        _observer: observer,
        observed,
        worker,
        events,
        style,
        text,
        shared: Shared::default(),
        bars: Vec::new(),
        icons: ThemeIcons::new(&assets),
        symbols: SystemIcons::required(&assets, &[], &extras::SYMBOL_NAMES),
        data_dirs: cupertino_defaults::data_dirs(),
        start: Instant::now(),
        clock: conf.clock,
        conf,
        conf_changes,
        _conf_watch: conf_watch,
        next_clock: f64::NEG_INFINITY,
        extras_minute: None,
        extras,
        extras_events,
        next_key: 0,
        pending: HashMap::new(),
        recent,
        keymap: Keymap::load(),
    };
    app.reconcile_outputs()?;
    for event in early {
        app.handle(event)?;
    }
    let mut timeout = app.housekeeping()?;
    loop {
        app.present()?;
        for event in app.conn.wait(timeout)? {
            app.handle(event)?;
        }
        app.drain_worker()?;
        app.reload_conf();
        timeout = app.housekeeping()?;
    }
}

fn geometry_rect(r: Rect) -> GRect {
    GRect::new(r.x, r.y, r.w, r.h)
}

impl App {
    /// Seconds on the monotonic clock the controllers are driven with.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn locate(&self, surface: SurfaceId) -> Option<(usize, Part)> {
        self.bars.iter().enumerate().find_map(|(i, b)| {
            if b.layer.surface() == surface {
                Some((i, Part::Bar))
            } else {
                b.popups
                    .iter()
                    .position(|p| p.popup.surface() == surface)
                    .map(|level| (i, Part::Popup(level)))
            }
        })
    }

    // ---- outputs -------------------------------------------------------------------------

    /// Create the bars of new outputs, drop those of outputs that went away and follow the geometry
    /// of the others.
    fn reconcile_outputs(&mut self) -> Result<()> {
        let outputs = self.conn.outputs();
        let mut i = 0;
        while i < self.bars.len() {
            if outputs.iter().any(|o| o.id == self.bars[i].output) {
                i += 1;
            } else {
                self.destroy_bar(i)?;
            }
        }
        for info in outputs {
            // an output the server has not described yet gets its bar when it has
            let Some(geometry) = info.geometry else {
                continue;
            };
            match self.bars.iter().position(|b| b.output == info.id) {
                Some(i) => {
                    if self.bars[i].geometry != geometry {
                        self.bars[i].geometry = geometry;
                        self.set_screen(i);
                        self.sync_popups(i)?;
                    }
                }
                None => self.create_bar(info.id, geometry)?,
            }
        }
        Ok(())
    }

    fn create_bar(&mut self, output: OutputId, geometry: OutputGeometry) -> Result<()> {
        let layer = self.layer_shell.layer_surface(
            &mut self.conn,
            &LayerSurfaceConfig {
                namespace: NAMESPACE.to_string(),
                output: Some(output),
                layer: Layer::Top,
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                size: (0, self.style.bar.height.ceil() as u32),
                margin: Margin::default(),
                // the server reserves the bar's space
                exclusive_zone: 0,
                keyboard: KeyboardInteractivity::None,
            },
        )?;
        let menu_bar = self.shell.menu_bar(&self.conn, layer.surface(), output)?;
        let edits = menu_bar.can_edit();
        let backdrop = self.effects.backdrop(&self.conn, layer.surface())?;
        backdrop.set_material("MenuBar");
        let text = self.text.for_output();
        let mut controller = MenuController::new(
            Rect::new(
                0.0,
                0.0,
                f64::from(geometry.width),
                f64::from(geometry.height),
            ),
            Box::new(text.clone()),
        );
        controller.bar_metrics = self.style.bar.clone();
        let key = self.next_key;
        self.next_key += 1;
        self.bars.push(Bar {
            output,
            geometry,
            presenter: ShmPresenter::new(layer.surface()),
            layer,
            width: geometry.width,
            menu_bar,
            backdrop,
            controller,
            text,
            feed: BarFeed::new(key, edits),
            popups: Vec::new(),
            keyboard: KeyboardInteractivity::None,
            scale: 1.0,
        });
        if self.bars.len() == 1 {
            self.worker.send(worker::Request::AppMenuRegistrar(true))?;
        }
        Ok(())
    }

    fn destroy_bar(&mut self, i: usize) -> Result<()> {
        let mut bar = self.bars.remove(i);
        if self.bars.is_empty() {
            self.worker.send(worker::Request::AppMenuRegistrar(false))?;
        }
        while let Some(popup) = bar.popups.pop() {
            drop(popup.backdrop);
            self.conn.destroy(popup.popup.surface())?;
        }
        drop(bar.backdrop);
        drop(bar.menu_bar);
        self.conn.destroy(bar.layer.surface())?;
        Ok(())
    }

    /// The screen the controller lays menus out on: the bar's width and the output's height.
    fn set_screen(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        bar.controller.set_output(Rect::new(
            0.0,
            0.0,
            f64::from(bar.width),
            f64::from(bar.geometry.height),
        ));
    }

    // ---- events --------------------------------------------------------------------------

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Frame(frame) => {
                if let Some(presenter) =
                    presenters(&mut self.bars).find(|p| p.surface() == frame.surface)
                {
                    presenter.frame_done();
                }
            }
            Event::PopupConfigure { popup, configure } => {
                if let Some(presenter) =
                    presenters(&mut self.bars).find(|p| p.surface() == popup.surface())
                {
                    presenter.configured((configure.width, configure.height));
                }
            }
            Event::PopupDone(popup) => {
                // the server dismissed the grab: the menus are over
                if let Some((i, _)) = self.locate(popup.surface()) {
                    let outcome = self.bars[i].controller.close();
                    self.apply(i, outcome)?;
                }
            }
            Event::Pointer { surface, event } => self.pointer(surface, event)?,
            Event::Keyboard {
                surface,
                event:
                    KeyboardEvent::Key {
                        code,
                        pressed: true,
                        character,
                        mods,
                        ..
                    },
            } => {
                if let Some((i, _)) = self.locate(surface) {
                    let now = self.now();
                    let key = menu_key(code);
                    let input =
                        KeyInput::new(code, mods.shift, mods.ctrl, mods.alt, mods.logo, character);
                    let controller = &mut self.bars[i].controller;
                    let outcome =
                        match controller.search_key(key, &mut self.keymap, input, Instant::now()) {
                            Some(outcome) => Some(outcome),
                            None => key.map(|key| controller.key(key, now)),
                        };
                    if let Some(outcome) = outcome {
                        self.apply(i, outcome)?;
                    }
                }
            }
            Event::Keyboard {
                surface,
                event: KeyboardEvent::Leave,
            } => {
                // the bar held the keyboard for the shortcut and lost it to something else
                if let Some((i, Part::Bar)) = self.locate(surface) {
                    if self.bars[i].controller.mode() == Mode::TitleFocus {
                        let outcome = self.bars[i].controller.close();
                        self.apply(i, outcome)?;
                    }
                }
            }
            Event::OutputsChanged => self.reconcile_outputs()?,
            Event::ActivationToken { request, token } => self.complete_launch(request, &token),
            Event::Extension(event) => match self.observed.apply(&event) {
                Some(SessionChange::Appearance { dark }) => {
                    self.style.palette = MenuPalette::new(dark);
                    self.reload_extra_icons();
                    self.redraw_all();
                }
                // the bar's and the menus' materials follow the options in the server; the text palette has no
                // recorded contrast variant
                Some(SessionChange::DisplayOptions(_)) => {}
                // the server draws the lock screen: only the menus this process owns have to go
                Some(SessionChange::Lock { locked: true }) => {
                    for i in 0..self.bars.len() {
                        let outcome = self.bars[i].controller.close();
                        self.apply(i, outcome)?;
                    }
                }
                Some(SessionChange::Lock { locked: false }) => {}
                None => match event.downcast_ref::<ShellEvent>() {
                    Some(&ShellEvent::Configure { surface, size }) => {
                        if let Some((i, Part::Bar)) = self.locate(surface) {
                            let bar = &mut self.bars[i];
                            let size = bar.layer.configured(size)?;
                            bar.presenter.configured(size);
                            bar.width = size.0;
                            self.set_screen(i);
                            self.sync_popups(i)?;
                        }
                    }
                    Some(&ShellEvent::Closed(surface)) => {
                        if let Some((i, Part::Bar)) = self.locate(surface) {
                            let output = self.bars[i].output;
                            self.destroy_bar(i)?;
                            if self.conn.outputs().iter().any(|o| o.id == output) {
                                return Err("the server closed the menu bar of an output that is still present"
                                    .into());
                            }
                        }
                    }
                    Some(ShellEvent::MenuBar { surface, update }) => {
                        if let Some((i, Part::Bar)) = self.locate(*surface) {
                            let data_dirs = &self.data_dirs;
                            self.bars[i]
                                .feed
                                .apply(update, &mut self.shared, |id| app_name(id, data_dirs));
                            if let Some(agent) = update.session_agent {
                                self.extras.session_agent(match agent {
                                    cupertino_shell_client::SessionAgent::None => {
                                        SessionAgent::None
                                    }
                                    cupertino_shell_client::SessionAgent::Process(pid) => {
                                        SessionAgent::Process(pid)
                                    }
                                    cupertino_shell_client::SessionAgent::Any => SessionAgent::Any,
                                })?;
                            }
                            if update.engage {
                                let outcome = self.bars[i].controller.focus_bar();
                                self.apply(i, outcome)?;
                            }
                            self.refresh()?;
                        }
                    }
                    Some(ShellEvent::Dock { .. }) => {
                        return Err("a dock event arrived but this process has no dock".into())
                    }
                    // the session's requests are for the session agent
                    _ => {}
                },
            },
            // the scale is read when polling the presenters; the bar has no window and no clipboard
            _ => {}
        }
        Ok(())
    }

    /// Draw every surface that can take a frame, until none can. A bar whose scale changed measures its menus at
    /// the new one before anything is drawn.
    fn present(&mut self) -> Result<()> {
        loop {
            let (mut scaled, mut ready) = (Vec::new(), Vec::new());
            for presenter in presenters(&mut self.bars) {
                loop {
                    match presenter.poll(&self.conn)? {
                        Ready::ScaleChanged(scale) => scaled.push((presenter.surface(), scale)),
                        Ready::Draw => break ready.push(presenter.surface()),
                        Ready::Idle => break,
                    }
                }
            }
            if scaled.is_empty() && ready.is_empty() {
                return Ok(());
            }
            for (surface, scale) in scaled {
                if let Some((i, Part::Bar)) = self.locate(surface) {
                    // menus are measured at the scale they are drawn at
                    self.bars[i].scale = scale.factor();
                    self.bars[i].text.set_scale(self.bars[i].scale);
                    self.set_screen(i);
                    self.sync_popups(i)?;
                    self.reload_icons();
                }
            }
            for surface in ready {
                self.draw(surface)?;
            }
        }
    }

    fn pointer(&mut self, surface: SurfaceId, event: PointerEvent) -> Result<()> {
        let Some((i, part)) = self.locate(surface) else {
            return Ok(());
        };
        // the controller works in bar-surface coordinates; a popup's events are relative to its corner
        let origin = match part {
            Part::Bar => (0.0, 0.0),
            Part::Popup(level) => (
                self.bars[i].popups[level].rect.x,
                self.bars[i].popups[level].rect.y,
            ),
        };
        let at = |p: cupertino_geometry::Point| Point::new(p.x + origin.0, p.y + origin.1);
        let now = self.now();
        let outcome = match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                self.bars[i].controller.pointer_move(at(p), now)
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => self.bars[i].controller.pointer_press(at(position), now),
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                position,
            } => self.bars[i].controller.pointer_release(at(position), now),
            // the controller keeps its selection until the pointer shows up elsewhere
            PointerEvent::Leave | PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {
                return Ok(())
            }
        };
        self.apply(i, outcome)
    }

    // ---- bus -----------------------------------------------------------------------------

    /// Apply the changes of `menubar.conf`: the file is read again with the current values as the last good ones, and a
    /// new clock setting redraws the clock now. A file that cannot be read keeps the current values (reported).
    fn reload_conf(&mut self) {
        while let Ok(event) = self.conf_changes.try_recv() {
            match event {
                WatchEvent::Error(e) => {
                    eprintln!("cupertino-menubar: watching menubar.conf failed: {e}")
                }
                WatchEvent::Changed(_) => match load_conf(Some(&self.conf)) {
                    Ok(conf) => {
                        self.conf = conf;
                        if self.clock != conf.clock {
                            self.clock = conf.clock;
                            self.next_clock = f64::NEG_INFINITY;
                        }
                    }
                    Err(e) => eprintln!("cupertino-menubar: {e}; the clock keeps its settings"),
                },
            }
        }
    }

    fn drain_worker(&mut self) -> Result<()> {
        loop {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(worker::WorkerGone.into()),
            };
            match event {
                worker::Event::Layout { source, root } => self.shared.apply_layout(&source, root),
                worker::Event::LayoutFailed { source } => self.shared.layout_failed(&source),
                worker::Event::StatusUpdated { item, menu } => {
                    let px = self.icon_px();
                    let icons = &mut self.icons;
                    let icon = status_icon(&item, px, |name, px| icons.load(name, px));
                    self.shared.set_status(StatusState {
                        item: *item,
                        icon,
                        menu,
                    });
                    self.redraw_all();
                }
                worker::Event::StatusRemoved { id } => {
                    self.shared.remove_status(&id);
                    self.redraw_all();
                }
            }
        }
        loop {
            match self.extras_events.try_recv() {
                Ok(extras) => {
                    self.shared.extras = extras;
                    self.reload_extra_icons();
                    self.redraw_all();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(worker::WorkerGone.into()),
            }
        }
        self.refresh()
    }

    /// Draw the extras' symbols in the bar's text colour (dimmed ones at [`DIMMED_ALPHA`]).
    fn reload_extra_icons(&mut self) {
        let px = self.icon_px();
        let text = self.style.palette.text;
        let symbols = &mut self.symbols;
        let extras = &self.shared.extras;
        self.shared.extra_icons = ExtraKind::ORDER
            .into_iter()
            .filter_map(|kind| {
                let glyph = extras.glyph(kind)?;
                let color = [
                    text[0],
                    text[1],
                    text[2],
                    if glyph.dimmed {
                        text[3] * DIMMED_ALPHA
                    } else {
                        text[3]
                    },
                ];
                Some((kind, Some(symbols.symbol(glyph.symbol, px, color).clone())))
            })
            .collect();
    }

    /// Device pixels of status icons: sharp on the sharpest output.
    fn icon_px(&self) -> u32 {
        status_icon_px(self.bars.iter().map(|b| b.scale).fold(1.0, f64::max))
    }

    fn reload_icons(&mut self) {
        let px = self.icon_px();
        for status in &mut self.shared.status {
            let icons = &mut self.icons;
            status.icon = status_icon(&status.item, px, |name, px| icons.load(name, px));
        }
        self.reload_extra_icons();
    }

    // ---- content and timers --------------------------------------------------------------

    /// Give every bar the content its feed describes, load menus it needs and forget the rest.
    fn refresh(&mut self) -> Result<()> {
        for i in 0..self.bars.len() {
            let content = self.bars[i].feed.content(&self.shared);
            if self.bars[i].controller.content() != &content {
                let outcome = self.bars[i].controller.set_content(content);
                self.apply(i, outcome)?;
            }
            if let Some(request) = self.bars[i].feed.request_layout(&mut self.shared) {
                self.worker.send(request)?;
            }
        }
        let in_use: Vec<_> = self
            .bars
            .iter()
            .map(|b| b.feed.source(&self.shared))
            .collect();
        self.shared.prune_layouts(&in_use);
        Ok(())
    }

    /// Update the clock, read the extras' services once a minute, run due menu timers and return how long the loop may
    /// sleep.
    fn housekeeping(&mut self) -> Result<Option<Duration>> {
        let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        if unix >= self.next_clock {
            self.next_clock = next_tick(unix, &self.clock);
            self.shared.clock = clock_text(
                &local_time(unix.floor() as i64),
                &self.clock,
                separators_off(unix),
            );
            self.refresh()?;
        }
        // the services announce their changes; the duration rows change with the hour and announce nothing
        let minute = (unix / 60.0).floor() as i64;
        if self.extras_minute != Some(minute) {
            self.extras_minute = Some(minute);
            self.extras.refresh()?;
        }
        let now = self.now();
        let mut next = Duration::from_secs_f64((self.next_clock - unix).max(0.0));
        for i in 0..self.bars.len() {
            if self.bars[i]
                .controller
                .next_deadline()
                .is_some_and(|d| d <= now)
            {
                let outcome = self.bars[i].controller.tick(now);
                self.apply(i, outcome)?;
            }
            if let Some(deadline) = self.bars[i].controller.next_deadline() {
                next = next.min(Duration::from_secs_f64((deadline - now).max(0.0)));
            }
        }
        Ok(Some(next))
    }

    // ---- launches ------------------------------------------------------------------------

    /// Fetch an activation token for a launch from bar `i`; it starts when the token arrives.
    fn begin_launch(&mut self, i: usize, launch: Launch) {
        match self
            .conn
            .request_activation_token(self.bars[i].layer.surface(), None)
        {
            Ok(request) => {
                self.pending.insert(request, launch);
            }
            Err(e) => eprintln!("{PROCESS}: cannot launch {}: {e}", launch_name(&launch)),
        }
    }

    /// Start the launch that waited for `token` (it uses the token to take the focus).
    fn complete_launch(&mut self, request: TokenRequest, token: &str) {
        let Some(launch) = self.pending.remove(&request) else {
            eprintln!("{PROCESS}: an activation token arrived for no launch");
            return;
        };
        match launch.command(token) {
            Some(spec) if spec.terminal => eprintln!(
                "{PROCESS}: {} runs in a terminal, and the menu bar has none",
                launch_name(&launch)
            ),
            Some(spec) => {
                launch::spawn(PROCESS, spec.to_command());
            }
            None => eprintln!("{PROCESS}: {} has no command", launch_name(&launch)),
        }
    }

    /// Open `file` from bar `i` with the default application of its type, or through [`OPEN_COMMAND`] when no
    /// installed application handles it. The associations are read at each open, so changed defaults apply at once.
    fn open_document(&mut self, i: usize, file: &Path) {
        let apps = XdgDirs::from_env().and_then(|dirs| {
            let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
            MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env())
                .map_err(|e| e.to_string())
        });
        match apps.map(|apps| launch::open_with_default(&apps, file)) {
            Ok(Some(launch)) => self.begin_launch(i, launch),
            Ok(None) => {
                let mut command = Command::new(OPEN_COMMAND);
                command.arg(file);
                launch::spawn(PROCESS, command);
            }
            Err(e) => eprintln!("{PROCESS}: cannot open {}: {e}", file.display()),
        }
    }

    /// Read Recent Items for the system menu that is opening; the bars show it at their next refresh.
    fn read_recent_items(&mut self) {
        if let Some(sources) = &self.recent {
            let now = unix_now();
            self.shared.recent = sources.read(now);
        }
    }

    /// Clear Menu: Recent Items lists only what is used from now on.
    fn clear_recent_items(&mut self) {
        if let Some(sources) = &self.recent {
            let now = unix_now();
            if let Err(e) = sources.clear(now) {
                eprintln!("{PROCESS}: cannot write {}: {e}", sources.cleared.display());
            }
            self.shared.recent = Default::default();
        }
    }

    // ---- effects -------------------------------------------------------------------------

    /// Carry out what the controller of bar `i` asked for, then bring popups and keyboard in line.
    fn apply(&mut self, i: usize, outcome: Outcome) -> Result<()> {
        let actions: Vec<Action> = {
            let bar = &self.bars[i];
            let cx = Context {
                feed: &bar.feed,
                shared: &self.shared,
                layout: bar.controller.layout(),
                origin: (bar.geometry.x, bar.geometry.y),
            };
            outcome
                .effects
                .into_iter()
                .filter_map(|e| routing::resolve(e, &cx))
                .collect()
        };
        for action in actions {
            match action {
                Action::Redraw => self.redraw(i),
                Action::SetMenuOpen(open) => self.bars[i].menu_bar.set_menu_open(open),
                Action::Request(request) => self.worker.send(request)?,
                Action::WindowCommand(command) => self.bars[i].menu_bar.window_command(command),
                Action::MoveToWorkspace(workspace) => {
                    self.bars[i].menu_bar.move_to_workspace(workspace)
                }
                Action::ActivateWindow(id) => self.bars[i].menu_bar.activate_window(id),
                Action::Edit(action) => self.bars[i].menu_bar.edit(action),
                Action::Launch(launch) => self.begin_launch(i, launch),
                Action::OpenDocument(file) => self.open_document(i, &file),
                Action::SystemMenuOpening {
                    refresh_capabilities,
                } => {
                    self.read_recent_items();
                    if refresh_capabilities {
                        self.bars[i].menu_bar.session_action(client_session_action(
                            SessionAction::RefreshCapabilities,
                        ));
                    }
                }
                Action::ClearRecentItems => self.clear_recent_items(),
                Action::ShowAbout => {
                    eprintln!("cupertino-menubar: about requested (there is no about panel yet)")
                }
                Action::Extra(ExtraAction::Session(action)) => self.bars[i]
                    .menu_bar
                    .session_action(client_session_action(action)),
                Action::Extra(action) => self.extras.act(action)?,
            }
        }
        self.sync_popups(i)?;
        self.sync_keyboard(i);
        Ok(())
    }

    /// Make the popup surfaces of bar `i` match the open menu levels.
    fn sync_popups(&mut self, i: usize) -> Result<()> {
        let desired: Vec<Rect> = self.bars[i]
            .controller
            .levels()
            .iter()
            .map(|l| popups::snap(l.layout.rect))
            .collect();
        let current: Vec<Rect> = self.bars[i].popups.iter().map(|p| p.rect).collect();
        for op in popups::plan(&current, &desired) {
            match op {
                PopupOp::Destroy { level } => {
                    let popup = self.bars[i]
                        .popups
                        .pop()
                        .filter(|_| level == self.bars[i].popups.len())
                        .ok_or("popups are destroyed deepest first")?;
                    drop(popup.backdrop);
                    self.conn.destroy(popup.popup.surface())?;
                }
                PopupOp::Create { level, rect } => {
                    let bar = &self.bars[i];
                    let parent = if level == 0 {
                        bar.layer.surface()
                    } else {
                        bar.popups[level - 1].popup.surface()
                    };
                    let popup = self
                        .conn
                        .popup(parent, &popup::placement(&geometry_rect(rect), true)?)?;
                    let backdrop = self.effects.backdrop(&self.conn, popup.surface())?;
                    backdrop.set_material("Menu");
                    self.bars[i].popups.push(PopupSurface {
                        popup,
                        presenter: ShmPresenter::new(popup.surface()),
                        backdrop,
                        rect: desired[level],
                    });
                }
                PopupOp::Reposition { level, rect } => self.conn.reposition_popup(
                    self.bars[i].popups[level].popup,
                    &popup::placement(&geometry_rect(rect), true)?,
                )?,
            }
        }
        for (popup, rect) in self.bars[i].popups.iter_mut().zip(desired) {
            popup.rect = rect;
        }
        Ok(())
    }

    /// The bar takes the keyboard exclusively while a title is focused by the shortcut (the server
    /// gives a layer the keyboard only for exclusive interactivity); open menus get it through
    /// their popup grab.
    fn sync_keyboard(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        let wanted = if bar.controller.mode() == Mode::TitleFocus {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        if bar.keyboard != wanted {
            bar.keyboard = wanted;
            bar.layer.set_keyboard_interactivity(wanted);
            // the change applies with the next commit
            bar.presenter.request_redraw();
        }
    }

    // ---- drawing -------------------------------------------------------------------------

    fn redraw(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        bar.presenter.request_redraw();
        for popup in &mut bar.popups {
            popup.presenter.request_redraw();
        }
    }

    fn redraw_all(&mut self) {
        (0..self.bars.len()).for_each(|i| self.redraw(i));
    }

    fn draw(&mut self, surface: SurfaceId) -> Result<()> {
        // the surface may be gone since it was found ready
        let Some((i, part)) = self.locate(surface) else {
            return Ok(());
        };
        let bar = &mut self.bars[i];
        let presenter = match part {
            Part::Bar => &bar.presenter,
            Part::Popup(level) => &bar.popups[level].presenter,
        };
        let (width, height) = presenter
            .size()
            .ok_or("a surface was ready to draw before its first configure")?;
        let size = GRect::new(0.0, 0.0, f64::from(width), f64::from(height));
        match part {
            Part::Bar => {
                bar.backdrop.set_shape(&size, 0.0)?;
                let (c, text, style, shared) =
                    (&bar.controller, &bar.text, &self.style, &self.shared);
                bar.presenter.draw(&mut self.conn, None, |pm, scale| {
                    style.paint_bar(
                        text,
                        pm,
                        c.layout(),
                        c.content(),
                        c.highlighted_slot(),
                        shared,
                        scale.factor(),
                    )
                })?;
            }
            Part::Popup(level) => {
                // a level that closed since stays blank until its surface is destroyed
                let Some(open) = bar.controller.levels().get(level) else {
                    return Ok(());
                };
                let popup = &mut bar.popups[level];
                popup.backdrop.set_shape(&size, POPUP_RADIUS)?;
                let (text, style, origin) = (&bar.text, &self.style, (popup.rect.x, popup.rect.y));
                let field = bar.controller.search_field();
                popup.presenter.draw(&mut self.conn, None, |pm, scale| {
                    style.paint_popup(
                        text,
                        pm,
                        &open.layout,
                        open.selected,
                        field,
                        origin,
                        scale.factor(),
                    )
                })?;
            }
        }
        Ok(())
    }
}

/// Seconds since the Unix epoch (0 for a clock set before it).
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// What a launch starts, for messages.
fn launch_name(launch: &Launch) -> String {
    match launch {
        Launch::Program(argv) => argv.join(" "),
        Launch::App(app) => app.id.clone(),
        Launch::Open { app, file } => format!("{} with {}", file.display(), app.id),
    }
}

/// Every presenter of the bars: each bar's, then those of its open menu levels.
fn presenters(bars: &mut [Bar]) -> impl Iterator<Item = &mut ShmPresenter> {
    bars.iter_mut().flat_map(|bar| {
        let Bar {
            presenter, popups, ..
        } = bar;
        std::iter::once(presenter).chain(popups.iter_mut().map(|p| &mut p.presenter))
    })
}

fn client_session_action(action: SessionAction) -> cupertino_shell_client::SessionAction {
    match action {
        SessionAction::Sleep => cupertino_shell_client::SessionAction::Sleep,
        SessionAction::Restart => cupertino_shell_client::SessionAction::Restart,
        SessionAction::ShutDown => cupertino_shell_client::SessionAction::ShutDown,
        SessionAction::Lock => cupertino_shell_client::SessionAction::Lock,
        SessionAction::LogOut => cupertino_shell_client::SessionAction::LogOut,
        SessionAction::SwitchUser => cupertino_shell_client::SessionAction::SwitchUser,
        SessionAction::RefreshCapabilities => {
            cupertino_shell_client::SessionAction::RefreshCapabilities
        }
    }
}
