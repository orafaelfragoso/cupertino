//! Glue between the overlay model and the toolkit surface: applies the commands of the bus interface, draws the
//! overlay over a server-rendered backdrop and paces the hold and the fade.

use cupertino_alsa::DEVICE_DIR;
use cupertino_assets::Assets;
use cupertino_osd::backend::{
    AlsaVolume, MemoryBacklight, MemoryVolume, SessionMicrophone, SessionVolume, Volume,
};
use cupertino_osd::control;
use cupertino_osd::controller::{Command, Controller};
use cupertino_osd::layout::{self, BOTTOM_MARGIN, CORNER_RADIUS, WINDOW_SIZE};
use cupertino_osd::model::{Phase, View};
use cupertino_osd::render::{Palette, Renderer, MATERIAL};
use cupertino_osd::system::{DisplayBacklight, KeyboardBacklight};
use cupertino_pipewire::Wpctl;
use cupertino_protocol_app::{AppEffects, Backdrop};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    Session, SessionChange, SessionState, Shell, ShellEvent,
};
use cupertino_theme::{Appearance, Theme};
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{Connection, Event, OutputId};
use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tiny_skia::{PixmapPaint, Transform};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surface for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-osd";

/// Levels of the in-memory backends of `--fake-backends` (arbitrary, for trying the overlay).
const FAKE_LEVEL: f64 = 0.5;

/// Command line of the process.
pub struct Options {
    /// Change in-memory levels instead of the machine's volume and backlights.
    pub fake_backends: bool,
    /// Sound card whose master volume is used instead of the default PipeWire sink.
    pub alsa_card: Option<u32>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            fake_backends: false,
            alsa_card: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fake-backends" => options.fake_backends = true,
                "--alsa-card" => {
                    let card = args.next().ok_or("--alsa-card needs a card number")?;
                    options.alsa_card = Some(
                        card.parse()
                            .map_err(|_| format!("{card:?} is not a card number"))?,
                    );
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

struct App {
    conn: Connection,
    effects: AppEffects,
    _session: Session,
    session: SessionState,
    theme: Theme,
    assets: Assets,
    /// The 200 x 200 window, mapped all the time; it draws nothing and takes no input while hidden.
    layer: LayerSurface,
    presenter: ShmPresenter,
    output: OutputId,
    controller: Controller,
    commands: Receiver<Command>,
    renderer: Renderer,
    start: Instant,
    /// The phase last drawn, to redraw when it changes.
    phase: Phase,
    backdrop: Option<Backdrop>,
}

/// The first output the server has described.
fn first_output(conn: &Connection) -> Result<OutputId> {
    Ok(conn
        .outputs()
        .into_iter()
        .find(|o| o.geometry.is_some())
        .ok_or("the server has no output")?
        .id)
}

fn controller(options: &Options) -> Controller {
    if options.fake_backends {
        return Controller::new(
            Box::new(MemoryVolume::new(FAKE_LEVEL, false)),
            Box::new(MemoryVolume::new(FAKE_LEVEL, false)),
            Box::new(MemoryBacklight::new(FAKE_LEVEL)),
            Box::new(MemoryBacklight::new(FAKE_LEVEL)),
        );
    }
    let alsa = AlsaVolume::new(PathBuf::from(DEVICE_DIR), options.alsa_card);
    let volume: Box<dyn Volume> = match options.alsa_card {
        Some(_) => Box::new(alsa),
        None => Box::new(SessionVolume::new(Wpctl::default(), alsa)),
    };
    Controller::new(
        volume,
        Box::new(SessionMicrophone::new(Wpctl::default())),
        Box::new(DisplayBacklight::new()),
        Box::new(KeyboardBacklight::new()),
    )
}

/// The renderer for the session's appearance and display options.
fn overlay_renderer(theme: &Theme, assets: &Assets, session: &SessionState) -> Result<Renderer> {
    let appearance = Appearance::of_session(session.dark, session.display.increase_contrast);
    Ok(Renderer::new(
        Palette::from_theme(theme, appearance, session.display.reduce_transparency)?,
        theme.tokens.typography().clone(),
        assets,
    )?)
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let mut conn = Connection::connect()?;
    let layer_shell = LayerShell::bind(&mut conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let shell = Shell::bind(&mut conn).ok_or("the server lacks cupertino_shell_v1")?;
    let (session_observer, session, early) = shell.observe(&mut conn)?;
    let renderer = overlay_renderer(&theme, &assets, &session)?;
    let waker = conn.waker();
    let (sender, commands) = mpsc::channel();
    // the bus thread only queues; the loop below carries the commands out, so the effects never block the bus
    let sender = Mutex::new(sender);
    let _control = control::serve(zbus::connection::Builder::session()?, move |command| {
        sender
            .lock()
            .expect("the sender lock is never poisoned")
            .send(command)
            .expect("the receiver lives as long as the process");
        waker.wake();
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another overlay owns org.cupertino.Osd1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;

    let output = first_output(&conn)?;
    let layer = layer_shell.layer_surface(
        &mut conn,
        &LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output: Some(output),
            layer: Layer::Overlay,
            // anchored to the bottom only: centred horizontally, BOTTOM_MARGIN above the edge
            anchor: Anchor::Bottom,
            size: (WINDOW_SIZE, WINDOW_SIZE),
            margin: Margin {
                bottom: BOTTOM_MARGIN,
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        },
    )?;
    let mut app = App {
        presenter: ShmPresenter::new(layer.surface()),
        conn,
        effects,
        _session: session_observer,
        session,
        theme,
        assets,
        layer,
        output,
        controller: controller(options),
        commands,
        renderer,
        start: Instant::now(),
        phase: Phase::Hidden,
        backdrop: None,
    };
    for event in early {
        app.handle(event)?;
    }
    loop {
        app.present()?;
        let now = app.now();
        let timeout = app.controller.until_fade(now).map(Duration::from_secs_f64);
        for event in app.conn.wait(timeout)? {
            app.handle(event)?;
        }
        app.housekeeping();
    }
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Frame(frame) if frame.surface == self.layer.surface() => {
                self.presenter.frame_done()
            }
            Event::OutputsChanged => {
                if !self.conn.outputs().iter().any(|o| o.id == self.output) {
                    return Err("the output of the overlay went away".into());
                }
            }
            Event::Extension(event) => match self.session.apply(&event) {
                Some(SessionChange::Appearance { .. } | SessionChange::DisplayOptions(_)) => {
                    self.renderer = overlay_renderer(&self.theme, &self.assets, &self.session)?;
                    self.presenter.request_redraw();
                }
                // a lock leaves the overlay alone, which the server hides
                Some(SessionChange::Lock { .. }) => {}
                None => match event.downcast_ref::<ShellEvent>() {
                    Some(&ShellEvent::Configure { surface, size })
                        if surface == self.layer.surface() =>
                    {
                        self.presenter.configured(self.layer.configured(size)?);
                    }
                    Some(ShellEvent::Closed(_)) => {
                        return Err("the server closed the overlay surface".into())
                    }
                    _ => {}
                },
            },
            // the surface takes no input and has no popups; the presenter reads scale changes itself
            _ => {}
        }
        Ok(())
    }

    /// Draw while the presenter can take a frame.
    fn present(&mut self) -> Result<()> {
        loop {
            match self.presenter.poll(&self.conn)? {
                Ready::Idle => return Ok(()),
                // a draw follows every scale change
                Ready::ScaleChanged(_) => {}
                Ready::Draw => self.draw()?,
            }
        }
    }

    /// Carry out the queued commands (a failure is logged and shows nothing) and redraw when something changed or the
    /// lifetime moved to its next phase.
    fn housekeeping(&mut self) {
        let mut changed = false;
        while let Ok(command) = self.commands.try_recv() {
            match self.controller.apply(self.now(), command) {
                Ok(()) => changed = true,
                Err(e) => eprintln!("cupertino-osd: {e}"),
            }
        }
        let phase = self.controller.phase(self.now());
        if changed || phase != self.phase {
            self.phase = phase;
            self.presenter.request_redraw();
        }
    }

    /// Draw the overlay with the backdrop of its material, or clear the surface when it is gone.
    fn draw(&mut self) -> Result<()> {
        let now = self.now();
        let Some(View { overlay, alpha }) = self.controller.view(now) else {
            self.backdrop = None;
            self.conn
                .set_input_region(self.layer.surface(), Some(&[]))?;
            self.presenter.draw(&mut self.conn, None, |_, _| {})?;
            return Ok(());
        };
        if self.backdrop.is_none() {
            let backdrop = self.effects.backdrop(&self.conn, self.layer.surface())?;
            backdrop.set_material(MATERIAL);
            self.backdrop = Some(backdrop);
        }
        let backdrop = self.backdrop.as_ref().expect("created above");
        backdrop.set_shape(&layout::window(), CORNER_RADIUS)?;
        backdrop.set_alpha(alpha.clamp(0.0, 1.0))?;
        // the window ignores the mouse
        self.conn
            .set_input_region(self.layer.surface(), Some(&[]))?;
        let renderer = &mut self.renderer;
        self.presenter.draw(&mut self.conn, None, |pixmap, scale| {
            let rendered = renderer.render(&overlay, scale.factor() as f32);
            let paint = PixmapPaint {
                opacity: alpha as f32,
                ..PixmapPaint::default()
            };
            pixmap.draw_pixmap(0, 0, rendered.as_ref(), &paint, Transform::identity(), None);
        })?;
        // frame callbacks pace the fade
        if self.controller.phase(now) == Phase::Fading {
            self.presenter.request_redraw();
        }
        Ok(())
    }
}
