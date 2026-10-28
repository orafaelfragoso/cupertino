//! The `cupertino-screenshot` process: one capture from a shortcut. `--mode screen` captures every output at once;
//! `--mode selection` and `--mode toolbar` put an overlay on every output (and the toolbar) and run a [`Session`]
//! until it captures or is cancelled. The images then go to files ([`deliver`]) or to the clipboard.

use std::error::Error;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_geometry::{Point, Rect};
use cupertino_protocol_app::{AppEffects, Backdrop};
use cupertino_screenshot::image;
use cupertino_screenshot::render::{OverlayView, Painter, Palette, ToolbarView};
use cupertino_screenshot::session::{Key, Outcome, Pointer, Session, Style, Target};
use cupertino_screenshot::settings::{Location, Mode, Settings};
use cupertino_screenshot::toolbar::{self, Item, MenuRow, MENU_RADIUS, MENU_WIDTH};
use cupertino_screenshot::values::{
    TOOLBAR_BOTTOM, TOOLBAR_MATERIAL, TOOLBAR_RADIUS, TOOLBAR_SIZE,
};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    Session as SessionObserver, SessionState, Shell, ShellEvent,
};
use cupertino_theme::{Appearance, Theme};
use cupertino_ui_wayland::{popup, Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Connection, CursorShape, Event, ExtensionEvent, KeyboardEvent, OutputId,
    PointerEvent, Popup, SurfaceId,
};
use tiny_skia::Pixmap;

use crate::capture::Capturer;
use crate::deliver;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Layer-shell namespace of the overlay and the toolbar; the server starts no second process while one is mapped.
const OVERLAY_NAMESPACE: &str = "cupertino-screenshot";
/// Material of the Options menu, as the menu bar's menus.
const MENU_MATERIAL: &str = "Menu";
/// Gap between the Options button and its menu (approximate).
const MENU_GAP: f64 = 4.0;

/// How the process was started (the server appends these per shortcut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    /// Capture every output at once (the design's `-pd`).
    Screen,
    /// The crosshair selection (`-pdi`).
    Selection,
    /// The toolbar (`-pdiU`).
    Toolbar,
}

/// Command line of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub mode: StartMode,
    /// Send the capture to the clipboard instead of a file (the design's `c`).
    pub clipboard: bool,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut mode = None;
        let mut clipboard = false;
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--mode" => {
                    let value = args
                        .next()
                        .ok_or("--mode needs screen, selection or toolbar")?;
                    mode = Some(match value.as_str() {
                        "screen" => StartMode::Screen,
                        "selection" => StartMode::Selection,
                        "toolbar" => StartMode::Toolbar,
                        other => {
                            return Err(format!("{other:?} is not screen, selection or toolbar"))
                        }
                    });
                }
                "--clipboard" => clipboard = true,
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(Options {
            mode: mode.ok_or("--mode is required")?,
            clipboard,
        })
    }
}

/// One captured image.
pub struct Shot {
    pub image: Pixmap,
}

/// An output the overlay covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub id: OutputId,
    pub rect: Rect,
}

/// The toolkit side of the connection: the transport and the shell protocols the surfaces use.
pub struct Wayland {
    pub conn: Connection,
    pub layer_shell: LayerShell,
    pub effects: AppEffects,
    _session: SessionObserver,
    session: SessionState,
}

impl Wayland {
    fn new(conn: wayland_client::Connection) -> Result<Self> {
        let mut conn = Connection::from_wayland(conn)?;
        let layer_shell =
            LayerShell::bind(&mut conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
        let effects =
            AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
        let shell = Shell::bind(&mut conn).ok_or("the server lacks cupertino_shell_v1")?;
        // nothing is mapped yet: the other events are of the outputs, which are read after
        let (session_observer, session, _) = shell.observe(&mut conn)?;
        Ok(Self {
            conn,
            layer_shell,
            effects,
            _session: session_observer,
            session,
        })
    }

    pub fn appearance(&self) -> Appearance {
        if self.session.dark {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }

    /// Follow the session; the shell event when the extension event is one.
    pub fn extension<'a>(&mut self, event: &'a ExtensionEvent) -> Option<&'a ShellEvent> {
        match self.session.apply(event) {
            Some(_) => None,
            None => event.downcast_ref::<ShellEvent>(),
        }
    }
}

fn screens(conn: &Connection) -> Result<Vec<Screen>> {
    let screens: Vec<Screen> = conn
        .outputs()
        .into_iter()
        .filter_map(|o| {
            o.geometry.map(|g| Screen {
                id: o.id,
                rect: Rect::new(
                    f64::from(g.x),
                    f64::from(g.y),
                    f64::from(g.width),
                    f64::from(g.height),
                ),
            })
        })
        .collect();
    if screens.is_empty() {
        return Err("the server has no output".into());
    }
    Ok(screens)
}

/// Connect, run the capture the options ask for and deliver it.
pub fn run(options: &Options) -> Result<()> {
    let assets = Assets::discover();
    let settings_path =
        cupertino_defaults::config_file("CUPERTINO_SCREENSHOT_CONFIG", "screenshot.conf")
            .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    let (mut settings, problems) = Settings::load(&settings_path)?;
    for p in problems {
        eprintln!("cupertino-screenshot: {}: {p}", settings_path.display());
    }
    // the toolkit and the capture share the connection the server started the process with
    let conn = wayland_client::Connection::connect_to_env()?;
    let mut wl = Wayland::new(conn.clone())?;
    let mut capturer = Capturer::new(&conn)?;
    let screens = screens(&wl.conn)?;
    let (shots, clipboard) = match options.mode {
        StartMode::Screen => {
            let mut shots = Vec::new();
            for s in &screens {
                shots.push(Shot {
                    image: capturer.output(s.id, settings.show_pointer)?,
                });
            }
            (shots, options.clipboard)
        }
        StartMode::Selection | StartMode::Toolbar => {
            let theme = Theme::load()?;
            let painter = Painter::new(
                Palette::from_theme(&theme, wl.appearance())?,
                theme.tokens.typography().clone(),
                &assets,
            )?;
            let style = if options.mode == StartMode::Toolbar {
                Style::Interactive
            } else {
                Style::Simple
            };
            let mut overlay = Overlay::new(&mut wl, &screens, style, &settings, painter)?;
            let outcome = overlay.run(&mut wl, &mut capturer, &mut settings, &settings_path)?;
            if settings.show_pointer && matches!(outcome, Outcome::Capture { .. }) {
                // with the arrow back the server paints it into the capture
                overlay.show_arrow(&mut wl.conn)?;
            }
            overlay.close(&mut wl.conn)?;
            let Outcome::Capture { target, clipboard } = outcome else {
                return Ok(());
            };
            (
                capture(&mut capturer, &screens, &target, settings.show_pointer)?,
                clipboard || options.clipboard,
            )
        }
    };
    if clipboard || settings.location == Location::Clipboard {
        deliver::to_clipboard(&mut wl, &mut capturer, &shots[0])
    } else {
        deliver::to_files(&mut wl, &screens, &shots, &settings, &assets)
    }
}

/// Copy what `target` covers.
fn capture(
    capturer: &mut Capturer,
    screens: &[Screen],
    target: &Target,
    pointer: bool,
) -> Result<Vec<Shot>> {
    Ok(match target {
        Target::Screens => {
            let mut shots = Vec::new();
            for s in screens {
                shots.push(Shot {
                    image: capturer.output(s.id, pointer)?,
                });
            }
            shots
        }
        Target::Output(i) => vec![Shot {
            image: capturer.output(screens[*i].id, pointer)?,
        }],
        Target::Area { output, rect } => {
            let s = screens[*output];
            let whole = capturer.output(s.id, pointer)?;
            let area = image::area_pixels(rect, &s.rect, (whole.width(), whole.height()))
                .ok_or("the selection is outside its output")?;
            vec![Shot {
                image: whole
                    .clone_rect(area)
                    .ok_or("the selection is outside the captured image")?,
            }]
        }
        Target::Window(identifier) => vec![Shot {
            image: capturer.window(identifier, pointer)?,
        }],
    })
}

/// The toolbar surface and what it shows.
struct ToolbarSurface {
    layer: LayerSurface,
    presenter: ShmPresenter,
    backdrop: Option<Backdrop>,
    hovered: Option<Item>,
}

/// The Options menu popup.
struct Menu {
    popup: Popup,
    presenter: ShmPresenter,
    backdrop: Option<Backdrop>,
    rows: Vec<MenuRow>,
    hovered: Option<usize>,
}

/// The overlay surfaces, the toolbar and the session running on them.
struct Overlay {
    /// One full-output surface per screen, by index into `screens`.
    surfaces: Vec<(LayerSurface, ShmPresenter)>,
    screens: Vec<Screen>,
    toolbar: Option<ToolbarSurface>,
    menu: Option<Menu>,
    session: Session,
    painter: Painter,
    start: Instant,
    /// The pointer look last set, to set it only on change; the system pointer is hidden under the crosshair.
    look: Option<Pointer>,
}

impl Overlay {
    fn new(
        wl: &mut Wayland,
        screens: &[Screen],
        style: Style,
        settings: &Settings,
        painter: Painter,
    ) -> Result<Self> {
        let mut surfaces = Vec::new();
        for (i, s) in screens.iter().enumerate() {
            let layer = wl.layer_shell.layer_surface(
                &mut wl.conn,
                &LayerSurfaceConfig {
                    namespace: OVERLAY_NAMESPACE.into(),
                    output: Some(s.id),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                    size: (0, 0),
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    // one surface takes the keys for the whole overlay
                    keyboard: if i == 0 {
                        KeyboardInteractivity::Exclusive
                    } else {
                        KeyboardInteractivity::None
                    },
                },
            )?;
            let presenter = ShmPresenter::new(layer.surface());
            surfaces.push((layer, presenter));
        }
        let (session, toolbar) = match style {
            Style::Simple => (
                Session::new(
                    Style::Simple,
                    Mode::Selection,
                    screens.iter().map(|s| s.rect).collect(),
                    None,
                    0,
                ),
                None,
            ),
            Style::Interactive => {
                let remembered = settings.selection.filter(|_| settings.remember_selection);
                let session = Session::new(
                    Style::Interactive,
                    settings.mode,
                    screens.iter().map(|s| s.rect).collect(),
                    remembered,
                    settings.timer,
                );
                // created after the overlays, so it is stacked above them; on the first screen, above the dock's
                // exclusive zone, which stands in for the design's visible frame
                let layer = wl.layer_shell.layer_surface(
                    &mut wl.conn,
                    &LayerSurfaceConfig {
                        namespace: OVERLAY_NAMESPACE.into(),
                        output: Some(screens[0].id),
                        layer: Layer::Overlay,
                        anchor: Anchor::Bottom,
                        size: (TOOLBAR_SIZE.0 as u32, TOOLBAR_SIZE.1 as u32),
                        margin: Margin {
                            bottom: TOOLBAR_BOTTOM,
                            ..Margin::default()
                        },
                        exclusive_zone: 0,
                        keyboard: KeyboardInteractivity::None,
                    },
                )?;
                (
                    session,
                    Some(ToolbarSurface {
                        presenter: ShmPresenter::new(layer.surface()),
                        layer,
                        backdrop: None,
                        hovered: None,
                    }),
                )
            }
        };
        Ok(Self {
            surfaces,
            screens: screens.to_vec(),
            toolbar,
            menu: None,
            session,
            painter,
            start: Instant::now(),
            look: None,
        })
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Run until the session captures or is cancelled.
    fn run(
        &mut self,
        wl: &mut Wayland,
        capturer: &mut Capturer,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Outcome> {
        loop {
            self.present(wl)?;
            let now = self.now();
            let timeout = self
                .session
                .deadline(now)
                .map(|at| Duration::from_secs_f64((at - now).max(0.0)));
            let events = wl.conn.wait(timeout)?;
            capturer.dispatch()?;
            if let Some(windows) = capturer.take_windows() {
                self.session.set_windows(windows);
                self.redraw_overlays();
            }
            for event in events {
                if let Some(outcome) = self.handle(wl, event, settings, settings_path)? {
                    return Ok(outcome);
                }
            }
            let before = self.session.countdown(now);
            let now = self.now();
            if let Some(outcome) = self.session.tick(now) {
                return Ok(outcome);
            }
            if self.session.countdown(now) != before {
                self.redraw_toolbar();
            }
        }
    }

    fn presenters(&mut self) -> impl Iterator<Item = &mut ShmPresenter> {
        self.surfaces
            .iter_mut()
            .map(|(_, p)| p)
            .chain(self.toolbar.iter_mut().map(|t| &mut t.presenter))
            .chain(self.menu.iter_mut().map(|m| &mut m.presenter))
    }

    /// Draw every surface that can take a frame.
    fn present(&mut self, wl: &mut Wayland) -> Result<()> {
        let surfaces: Vec<SurfaceId> = self.presenters().map(|p| p.surface()).collect();
        for surface in surfaces {
            loop {
                let ready = match self.presenters().find(|p| p.surface() == surface) {
                    Some(presenter) => presenter.poll(&wl.conn)?,
                    None => break,
                };
                match ready {
                    Ready::Idle => break,
                    // a draw follows every scale change
                    Ready::ScaleChanged(_) => {}
                    Ready::Draw => self.draw(wl, surface)?,
                }
            }
        }
        Ok(())
    }

    fn redraw_overlays(&mut self) {
        for (_, p) in &mut self.surfaces {
            p.request_redraw();
        }
    }

    fn redraw_toolbar(&mut self) {
        if let Some(t) = &mut self.toolbar {
            t.presenter.request_redraw();
        }
    }

    /// Set the pointer look when it changed.
    fn set_look(&mut self, conn: &mut Connection, look: Pointer) -> Result<()> {
        if self.look != Some(look) {
            match look {
                Pointer::Crosshair => conn.hide_cursor()?,
                Pointer::Arrow => conn.set_cursor(CursorShape::Default)?,
            }
            self.look = Some(look);
        }
        Ok(())
    }

    /// Bring the arrow back where the overlay hid the pointer.
    fn show_arrow(&mut self, conn: &mut Connection) -> Result<()> {
        if self.look == Some(Pointer::Crosshair) {
            self.set_look(conn, Pointer::Arrow)?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        wl: &mut Wayland,
        event: Event,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Option<Outcome>> {
        let now = self.now();
        match event {
            Event::Frame(frame) => {
                if let Some(p) = self.presenters().find(|p| p.surface() == frame.surface) {
                    p.frame_done();
                }
            }
            Event::PopupConfigure { popup, configure } => {
                if let Some(m) = self.menu.as_mut().filter(|m| m.popup == popup) {
                    m.presenter.configured((configure.width, configure.height));
                }
            }
            // the description the process connected with also reports as a change; only a real one ends the overlay
            Event::OutputsChanged if screens(&wl.conn)? == self.screens => {}
            Event::OutputsChanged => return Err("the outputs changed during the screenshot".into()),
            Event::PopupDone(popup) if self.menu.as_ref().is_some_and(|m| m.popup == popup) => {
                self.close_menu(&mut wl.conn)?
            }
            Event::Keyboard {
                event:
                    KeyboardEvent::Key {
                        code,
                        pressed: true,
                        mods,
                        ..
                    },
                ..
            } => {
                let key = match code {
                    key::ESCAPE => Key::Escape,
                    key::SPACE => Key::Space,
                    key::ENTER | key::KEYPAD_ENTER => Key::Return,
                    _ => return Ok(None),
                };
                let outcome = self.session.key(key, mods.ctrl, now);
                self.redraw_overlays();
                self.redraw_toolbar();
                return Ok(outcome);
            }
            Event::Pointer { surface, event, .. } => {
                return self.pointer(wl, surface, event, settings, settings_path)
            }
            Event::Extension(event) => match wl.extension(&event) {
                Some(&ShellEvent::Configure { surface, size }) => self.configured(surface, size)?,
                Some(ShellEvent::Closed(_)) => {
                    return Err("the server closed a screenshot surface".into())
                }
                _ => {}
            },
            // the overlay asks for no activation and keeps its look for its short life
            _ => {}
        }
        Ok(None)
    }

    fn configured(&mut self, surface: SurfaceId, size: (u32, u32)) -> Result<()> {
        let layers = self.surfaces.iter_mut().map(|(l, p)| (&*l, p)).chain(
            self.toolbar
                .iter_mut()
                .map(|t| (&t.layer, &mut t.presenter)),
        );
        for (layer, presenter) in layers {
            if layer.surface() == surface {
                presenter.configured(layer.configured(size)?);
            }
        }
        Ok(())
    }

    fn pointer(
        &mut self,
        wl: &mut Wayland,
        surface: SurfaceId,
        event: PointerEvent,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Option<Outcome>> {
        let now = self.now();
        if let Some(i) = self
            .surfaces
            .iter()
            .position(|(l, _)| l.surface() == surface)
        {
            let origin = self.screens[i].rect.origin;
            let global = |p: Point| Point::new(p.x + origin.x, p.y + origin.y);
            let outcome = match event {
                PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                    self.session.pointer_moved(global(p));
                    None
                }
                PointerEvent::Button {
                    button: Button::Left,
                    pressed,
                    position,
                } => self
                    .session
                    .button(pressed, global(position), wl.conn.modifiers().ctrl, now),
                PointerEvent::Leave | PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {
                    None
                }
            };
            if matches!(event, PointerEvent::Enter(_)) {
                // a surface entered keeps the look its client set last; set it again
                self.look = None;
            }
            let look = self.session.pointer_look();
            self.set_look(&mut wl.conn, look)?;
            self.redraw_overlays();
            return Ok(outcome);
        }
        if self
            .toolbar
            .as_ref()
            .is_some_and(|t| t.layer.surface() == surface)
        {
            return self.toolbar_pointer(wl, event, settings, settings_path, now);
        }
        if self
            .menu
            .as_ref()
            .is_some_and(|m| m.popup.surface() == surface)
        {
            self.menu_pointer(&mut wl.conn, event, settings, settings_path)?;
        }
        Ok(None)
    }

    fn toolbar_pointer(
        &mut self,
        wl: &mut Wayland,
        event: PointerEvent,
        settings: &mut Settings,
        settings_path: &std::path::Path,
        now: f64,
    ) -> Result<Option<Outcome>> {
        if matches!(event, PointerEvent::Enter(_)) {
            self.look = None;
        }
        self.set_look(&mut wl.conn, Pointer::Arrow)?;
        let hovered = match event {
            PointerEvent::Enter(p)
            | PointerEvent::Motion(p)
            | PointerEvent::Button { position: p, .. } => toolbar::item_at(p),
            PointerEvent::Leave | PointerEvent::Scroll { .. } => None,
        };
        let toolbar = self
            .toolbar
            .as_mut()
            .expect("the toolbar received the event");
        if toolbar.hovered != hovered {
            toolbar.hovered = hovered;
            toolbar.presenter.request_redraw();
        }
        let PointerEvent::Button {
            button: Button::Left,
            pressed: false,
            ..
        } = event
        else {
            return Ok(None);
        };
        let outcome = match hovered {
            Some(Item::Close) => Some(Outcome::Cancel),
            Some(Item::Mode(mode)) => {
                self.session.set_mode(mode);
                settings.mode = self.session.mode();
                settings.store(settings_path)?;
                None
            }
            Some(Item::Options) => {
                self.open_menu(&mut wl.conn, settings)?;
                None
            }
            Some(Item::Capture) => self.session.capture(wl.conn.modifiers().ctrl, now),
            Some(
                Item::RecordScreen
                | Item::RecordWindow
                | Item::StopRecording
                | Item::RecordSelection,
            )
            | None => None,
        };
        self.redraw_overlays();
        self.redraw_toolbar();
        Ok(outcome)
    }

    fn menu_pointer(
        &mut self,
        conn: &mut Connection,
        event: PointerEvent,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<()> {
        let menu = self.menu.as_mut().expect("the menu received the event");
        let hovered = match event {
            PointerEvent::Enter(p)
            | PointerEvent::Motion(p)
            | PointerEvent::Button { position: p, .. } => toolbar::menu_row_at(&menu.rows, p),
            PointerEvent::Leave | PointerEvent::Scroll { .. } => None,
        };
        if menu.hovered != hovered {
            menu.hovered = hovered;
            menu.presenter.request_redraw();
        }
        if let (
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            },
            Some(row),
        ) = (event, hovered)
        {
            if let MenuRow::Choice { action, .. } = &menu.rows[row] {
                toolbar::apply(settings, action);
                settings.store(settings_path)?;
            }
            self.close_menu(conn)?;
        }
        Ok(())
    }

    fn open_menu(&mut self, conn: &mut Connection, settings: &Settings) -> Result<()> {
        if self.menu.is_some() {
            return self.close_menu(conn);
        }
        let toolbar = self
            .toolbar
            .as_ref()
            .expect("the Options button is on the toolbar");
        let rows = toolbar::menu(settings);
        let (_, height) = toolbar::menu_layout(&rows);
        let button = toolbar::items()
            .into_iter()
            .find(|(i, _)| *i == Item::Options)
            .map(|(_, r)| r)
            .expect("the toolbar has an Options button");
        // above the button: the toolbar sits at the bottom of the screen
        let rect = Rect::new(
            button.x(),
            toolbar::background().y() - MENU_GAP - height,
            MENU_WIDTH,
            height,
        );
        let popup = conn.popup(toolbar.layer.surface(), &popup::placement(&rect, true)?)?;
        self.menu = Some(Menu {
            popup,
            presenter: ShmPresenter::new(popup.surface()),
            backdrop: None,
            rows,
            hovered: None,
        });
        Ok(())
    }

    fn close_menu(&mut self, conn: &mut Connection) -> Result<()> {
        if let Some(menu) = self.menu.take() {
            drop(menu.backdrop);
            conn.destroy(menu.popup.surface())?;
        }
        Ok(())
    }

    /// Destroy every surface, so the capture that follows does not contain them.
    fn close(&mut self, conn: &mut Connection) -> Result<()> {
        self.close_menu(conn)?;
        if let Some(t) = self.toolbar.take() {
            drop(t.backdrop);
            conn.destroy(t.layer.surface())?;
        }
        for (layer, _) in self.surfaces.drain(..) {
            conn.destroy(layer.surface())?;
        }
        Ok(())
    }

    fn draw(&mut self, wl: &mut Wayland, surface: SurfaceId) -> Result<()> {
        if let Some(i) = self
            .surfaces
            .iter()
            .position(|(l, _)| l.surface() == surface)
        {
            let s = &self.session;
            let view = OverlayView {
                bounds: self.screens[i].rect,
                style: s.style(),
                mode: s.mode(),
                selection: s.selection(),
                sizing: s.sizing(),
                picked: s.picked().map(|w| w.frame),
                pointer: s.pointer(),
                crosshair: s.pointer_look() == Pointer::Crosshair,
            };
            let painter = &mut self.painter;
            self.surfaces[i].1.draw(&mut wl.conn, None, |pm, scale| {
                painter.overlay(pm, scale.factor(), &view)
            })?;
            return Ok(());
        }
        if let Some(t) = self
            .toolbar
            .as_mut()
            .filter(|t| t.layer.surface() == surface)
        {
            if t.backdrop.is_none() {
                let backdrop = wl.effects.backdrop(&wl.conn, surface)?;
                backdrop.set_material(TOOLBAR_MATERIAL);
                backdrop.set_shape(&toolbar::background(), TOOLBAR_RADIUS)?;
                t.backdrop = Some(backdrop);
            }
            wl.conn
                .set_input_region(surface, Some(&[toolbar::background()]))?;
            let view = ToolbarView {
                mode: self.session.mode(),
                hovered: t.hovered,
                countdown: self.session.countdown(self.start.elapsed().as_secs_f64()),
            };
            let painter = &mut self.painter;
            t.presenter.draw(&mut wl.conn, None, |pm, scale| {
                painter.toolbar(pm, scale.factor(), &view)
            })?;
            return Ok(());
        }
        if let Some(m) = self.menu.as_mut().filter(|m| m.popup.surface() == surface) {
            if m.backdrop.is_none() {
                let (_, height) = toolbar::menu_layout(&m.rows);
                let backdrop = wl.effects.backdrop(&wl.conn, surface)?;
                backdrop.set_material(MENU_MATERIAL);
                backdrop.set_shape(&Rect::new(0.0, 0.0, MENU_WIDTH, height), MENU_RADIUS)?;
                m.backdrop = Some(backdrop);
            }
            let painter = &mut self.painter;
            m.presenter.draw(&mut wl.conn, None, |pm, scale| {
                painter.menu(pm, scale.factor(), &m.rows, m.hovered)
            })?;
        }
        Ok(())
    }
}
