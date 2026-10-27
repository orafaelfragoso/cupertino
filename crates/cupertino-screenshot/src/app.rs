//! The `cupertino-screenshot` process: one capture from a shortcut. `--mode screen` captures every output at once;
//! `--mode selection` and `--mode toolbar` put an overlay on every output (and the toolbar) and run a [`Session`]
//! until it captures or is cancelled. The images then go to files ([`deliver`]) or to the clipboard.

use std::error::Error;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_client::{
    key, Anchor, Backdrop, Button, Client, Connection, CursorLook, Event, KeyboardInteractivity,
    Layer, LayerSurfaceConfig, Margin, OutputId, PointerEvent, SurfaceId,
};
use cupertino_geometry::{Point, Rect};
use cupertino_screenshot::image;
use cupertino_screenshot::render::{OverlayView, Painter, Palette, ToolbarView};
use cupertino_screenshot::session::{Key, Outcome, Pointer, Session, Style, Target};
use cupertino_screenshot::settings::{Location, Mode, Settings};
use cupertino_screenshot::toolbar::{self, Item, MenuRow, MENU_RADIUS, MENU_WIDTH};
use cupertino_screenshot::values::{
    TOOLBAR_BOTTOM, TOOLBAR_MATERIAL, TOOLBAR_RADIUS, TOOLBAR_SIZE,
};
use cupertino_theme::{Appearance, Theme};
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

fn screens(client: &Client) -> Result<Vec<Screen>> {
    let screens: Vec<Screen> = client
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
    let conn = Connection::connect_to_env()?;
    let mut client = Client::from_shell_connection(conn.clone())?;
    let mut capturer = Capturer::new(&conn)?;
    let screens = screens(&client)?;
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
            let appearance = if client.is_dark() {
                Appearance::Dark
            } else {
                Appearance::Light
            };
            let painter = Painter::new(
                Palette::from_theme(&theme, appearance)?,
                theme.tokens.typography().clone(),
                &assets,
            )?;
            let style = if options.mode == StartMode::Toolbar {
                Style::Interactive
            } else {
                Style::Simple
            };
            let mut overlay = Overlay::new(&mut client, &screens, style, &settings, painter)?;
            let outcome = overlay.run(&mut client, &mut capturer, &mut settings, &settings_path)?;
            if settings.show_pointer && matches!(outcome, Outcome::Capture { .. }) {
                // with the arrow back the server paints it into the capture
                overlay.show_arrow(&mut client)?;
            }
            overlay.close(&mut client)?;
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
        deliver::to_clipboard(&mut client, &mut capturer, &shots[0])
    } else {
        deliver::to_files(&mut client, &screens, &shots, &settings, &assets)
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
    surface: SurfaceId,
    backdrop: Option<Backdrop>,
    hovered: Option<Item>,
}

/// The Options menu popup.
struct Menu {
    surface: SurfaceId,
    backdrop: Option<Backdrop>,
    rows: Vec<MenuRow>,
    hovered: Option<usize>,
}

/// The overlay surfaces, the toolbar and the session running on them.
struct Overlay {
    /// One full-output surface per screen, by index into `screens`.
    surfaces: Vec<SurfaceId>,
    screens: Vec<Screen>,
    toolbar: Option<ToolbarSurface>,
    menu: Option<Menu>,
    session: Session,
    painter: Painter,
    start: Instant,
    /// The pointer look last set, to set it only on change.
    look: Option<CursorLook>,
}

impl Overlay {
    fn new(
        client: &mut Client,
        screens: &[Screen],
        style: Style,
        settings: &Settings,
        painter: Painter,
    ) -> Result<Self> {
        let mut surfaces = Vec::new();
        for (i, s) in screens.iter().enumerate() {
            surfaces.push(client.layer_surface(&LayerSurfaceConfig {
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
            })?);
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
                let surface = client.layer_surface(&LayerSurfaceConfig {
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
                })?;
                (
                    session,
                    Some(ToolbarSurface {
                        surface,
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
        client: &mut Client,
        capturer: &mut Capturer,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Outcome> {
        loop {
            let now = self.now();
            let timeout = self
                .session
                .deadline(now)
                .map(|at| Duration::from_secs_f64((at - now).max(0.0)));
            let events = client.wait(timeout)?;
            capturer.dispatch()?;
            if let Some(windows) = capturer.take_windows() {
                self.session.set_windows(windows);
                self.redraw_overlays(client)?;
            }
            for event in events {
                if let Some(outcome) = self.handle(client, event, settings, settings_path)? {
                    return Ok(outcome);
                }
            }
            let before = self.session.countdown(now);
            let now = self.now();
            if let Some(outcome) = self.session.tick(now) {
                return Ok(outcome);
            }
            if self.session.countdown(now) != before {
                self.redraw_toolbar(client)?;
            }
        }
    }

    fn redraw_overlays(&self, client: &mut Client) -> Result<()> {
        for s in &self.surfaces {
            client.request_redraw(*s)?;
        }
        Ok(())
    }

    fn redraw_toolbar(&self, client: &mut Client) -> Result<()> {
        if let Some(t) = &self.toolbar {
            client.request_redraw(t.surface)?;
        }
        Ok(())
    }

    /// Set the pointer look when it changed.
    fn set_look(&mut self, client: &mut Client, look: CursorLook) -> Result<()> {
        if self.look != Some(look) {
            client.set_cursor(look)?;
            self.look = Some(look);
        }
        Ok(())
    }

    /// Bring the arrow back where the overlay hid the pointer.
    fn show_arrow(&mut self, client: &mut Client) -> Result<()> {
        if self.look == Some(CursorLook::Hidden) {
            self.set_look(client, CursorLook::Arrow)?;
        }
        Ok(())
    }

    fn session_look(&self) -> CursorLook {
        match self.session.pointer_look() {
            Pointer::Crosshair => CursorLook::Hidden,
            Pointer::Arrow => CursorLook::Arrow,
        }
    }

    fn handle(
        &mut self,
        client: &mut Client,
        event: Event,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Option<Outcome>> {
        let now = self.now();
        match event {
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(surface) => self.draw(client, surface)?,
            Event::Closed(_) => return Err("the server closed a screenshot surface".into()),
            // the description the process connected with also reports as a change; only a real one ends the overlay
            Event::OutputsChanged if screens(client)? == self.screens => {}
            Event::OutputsChanged => return Err("the outputs changed during the screenshot".into()),
            Event::PopupDone(surface)
                if self.menu.as_ref().is_some_and(|m| m.surface == surface) =>
            {
                self.close_menu(client)?
            }
            Event::Key {
                key: code,
                pressed: true,
                mods,
                ..
            } => {
                let key = match code {
                    key::ESCAPE => Key::Escape,
                    key::SPACE => Key::Space,
                    key::ENTER | key::KEYPAD_ENTER => Key::Return,
                    _ => return Ok(None),
                };
                let outcome = self.session.key(key, mods.ctrl, now);
                self.redraw_overlays(client)?;
                self.redraw_toolbar(client)?;
                return Ok(outcome);
            }
            Event::Pointer { surface, event } => {
                return self.pointer(client, surface, event, settings, settings_path)
            }
            // the overlay has no menu-bar or dock role, asks for no activation and keeps its look for its short life
            Event::KeyboardLeave(_)
            | Event::Key { .. }
            | Event::PopupDone(_)
            | Event::ActivationToken { .. }
            | Event::MenuBar { .. }
            | Event::Dock { .. }
            | Event::Appearance { .. }
            | Event::DisplayOptions(_)
            | Event::SessionLock { .. } => {}
        }
        Ok(None)
    }

    fn pointer(
        &mut self,
        client: &mut Client,
        surface: SurfaceId,
        event: PointerEvent,
        settings: &mut Settings,
        settings_path: &std::path::Path,
    ) -> Result<Option<Outcome>> {
        let now = self.now();
        if let Some(i) = self.surfaces.iter().position(|s| *s == surface) {
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
                    .button(pressed, global(position), client.modifiers().ctrl, now),
                PointerEvent::Leave | PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {
                    None
                }
            };
            if matches!(event, PointerEvent::Enter(_)) {
                // a surface entered keeps the look its client set last; set it again
                self.look = None;
            }
            let look = self.session_look();
            self.set_look(client, look)?;
            self.redraw_overlays(client)?;
            return Ok(outcome);
        }
        if self.toolbar.as_ref().is_some_and(|t| t.surface == surface) {
            return self.toolbar_pointer(client, event, settings, settings_path, now);
        }
        if self.menu.as_ref().is_some_and(|m| m.surface == surface) {
            self.menu_pointer(client, event, settings, settings_path)?;
        }
        Ok(None)
    }

    fn toolbar_pointer(
        &mut self,
        client: &mut Client,
        event: PointerEvent,
        settings: &mut Settings,
        settings_path: &std::path::Path,
        now: f64,
    ) -> Result<Option<Outcome>> {
        if matches!(event, PointerEvent::Enter(_)) {
            self.look = None;
        }
        self.set_look(client, CursorLook::Arrow)?;
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
            client.request_redraw(toolbar.surface)?;
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
                self.open_menu(client, settings)?;
                None
            }
            Some(Item::Capture) => self.session.capture(client.modifiers().ctrl, now),
            Some(
                Item::RecordScreen
                | Item::RecordWindow
                | Item::StopRecording
                | Item::RecordSelection,
            )
            | None => None,
        };
        self.redraw_overlays(client)?;
        self.redraw_toolbar(client)?;
        Ok(outcome)
    }

    fn menu_pointer(
        &mut self,
        client: &mut Client,
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
            client.request_redraw(menu.surface)?;
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
            self.close_menu(client)?;
        }
        Ok(())
    }

    fn open_menu(&mut self, client: &mut Client, settings: &Settings) -> Result<()> {
        if self.menu.is_some() {
            return self.close_menu(client);
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
        let surface = client.popup(toolbar.surface, &rect)?;
        self.menu = Some(Menu {
            surface,
            backdrop: None,
            rows,
            hovered: None,
        });
        Ok(())
    }

    fn close_menu(&mut self, client: &mut Client) -> Result<()> {
        if let Some(menu) = self.menu.take() {
            drop(menu.backdrop);
            client.destroy_surface(menu.surface)?;
        }
        Ok(())
    }

    /// Destroy every surface, so the capture that follows does not contain them.
    fn close(&mut self, client: &mut Client) -> Result<()> {
        self.close_menu(client)?;
        if let Some(t) = self.toolbar.take() {
            drop(t.backdrop);
            client.destroy_surface(t.surface)?;
        }
        for s in self.surfaces.drain(..) {
            client.destroy_surface(s)?;
        }
        Ok(())
    }

    fn draw(&mut self, client: &mut Client, surface: SurfaceId) -> Result<()> {
        if let Some(i) = self.surfaces.iter().position(|s| *s == surface) {
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
            client.draw(surface, |pm, scale| {
                painter.overlay(pm, f64::from(scale.factor()), &view)
            })?;
            return Ok(());
        }
        if let Some(t) = self.toolbar.as_mut().filter(|t| t.surface == surface) {
            if t.backdrop.is_none() {
                let backdrop = client.backdrop(surface)?;
                backdrop.set_material(TOOLBAR_MATERIAL);
                backdrop.set_shape(&toolbar::background(), TOOLBAR_RADIUS)?;
                t.backdrop = Some(backdrop);
            }
            client.set_input_region(surface, &[toolbar::background()])?;
            let view = ToolbarView {
                mode: self.session.mode(),
                hovered: t.hovered,
                countdown: self.session.countdown(self.start.elapsed().as_secs_f64()),
            };
            let painter = &mut self.painter;
            client.draw(surface, |pm, scale| {
                painter.toolbar(pm, f64::from(scale.factor()), &view)
            })?;
            return Ok(());
        }
        if let Some(m) = self.menu.as_mut().filter(|m| m.surface == surface) {
            if m.backdrop.is_none() {
                let (_, height) = toolbar::menu_layout(&m.rows);
                let backdrop = client.backdrop(surface)?;
                backdrop.set_material(MENU_MATERIAL);
                backdrop.set_shape(&Rect::new(0.0, 0.0, MENU_WIDTH, height), MENU_RADIUS)?;
                m.backdrop = Some(backdrop);
            }
            let (rows, hovered) = (m.rows.clone(), m.hovered);
            let painter = &mut self.painter;
            client.draw(surface, |pm, scale| {
                painter.menu(pm, f64::from(scale.factor()), &rows, hovered)
            })?;
        }
        Ok(())
    }
}
