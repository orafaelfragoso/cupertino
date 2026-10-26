//! Glue between the overlay model and the toolkit surface: applies the commands of the bus interface, draws the
//! overlay over a server-rendered backdrop and paces the hold and the fade.

use cupertino_alsa::DEVICE_DIR;
use cupertino_assets::Assets;
use cupertino_client::{
    Anchor, Backdrop, Client, DisplayOptions, Event, KeyboardInteractivity, Layer,
    LayerSurfaceConfig, Margin, OutputId, SurfaceId,
};
use cupertino_osd::backend::{AlsaVolume, Backlight, MemoryBacklight, MemoryVolume, Volume};
use cupertino_osd::control;
use cupertino_osd::controller::{Command, Controller};
use cupertino_osd::layout::{self, BOTTOM_MARGIN, CORNER_RADIUS, WINDOW_SIZE};
use cupertino_osd::model::{Phase, View};
use cupertino_osd::render::{Palette, Renderer, MATERIAL};
use cupertino_osd::system::{DisplayBacklight, KeyboardBacklight};
use cupertino_theme::{Appearance, Theme};
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
    /// Sound card whose master volume is used; the best master of all cards when absent.
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
    client: Client,
    theme: Theme,
    assets: Assets,
    /// The 200 x 200 window, mapped all the time; it draws nothing and takes no input while hidden.
    surface: SurfaceId,
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
fn first_output(client: &Client) -> Result<OutputId> {
    Ok(client
        .outputs()
        .into_iter()
        .find(|o| o.geometry.is_some())
        .ok_or("the server has no output")?
        .id)
}

fn controller(options: &Options) -> Controller {
    let (volume, display, keyboard): (Box<dyn Volume>, Box<dyn Backlight>, Box<dyn Backlight>) =
        if options.fake_backends {
            (
                Box::new(MemoryVolume::new(FAKE_LEVEL, false)),
                Box::new(MemoryBacklight::new(FAKE_LEVEL)),
                Box::new(MemoryBacklight::new(FAKE_LEVEL)),
            )
        } else {
            (
                Box::new(AlsaVolume::new(
                    PathBuf::from(DEVICE_DIR),
                    options.alsa_card,
                )),
                Box::new(DisplayBacklight::new()),
                Box::new(KeyboardBacklight::new()),
            )
        };
    Controller::new(volume, display, keyboard)
}

/// The renderer for the session's appearance and display options.
fn overlay_renderer(
    theme: &Theme,
    assets: &Assets,
    dark: bool,
    options: DisplayOptions,
) -> Result<Renderer> {
    let appearance = Appearance::of_session(dark, options.increase_contrast);
    Ok(Renderer::new(
        Palette::from_theme(theme, appearance, options.reduce_transparency)?,
        theme.tokens.typography().clone(),
        assets,
    )?)
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let mut client = Client::connect()?;
    let renderer = overlay_renderer(&theme, &assets, client.is_dark(), client.display_options())?;
    let waker = client.waker();
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

    let output = first_output(&client)?;
    let surface = client.layer_surface(&LayerSurfaceConfig {
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
    })?;
    let mut app = App {
        client,
        theme,
        assets,
        surface,
        output,
        controller: controller(options),
        commands,
        renderer,
        start: Instant::now(),
        phase: Phase::Hidden,
        backdrop: None,
    };
    loop {
        let now = app.now();
        let timeout = app.controller.until_fade(now).map(Duration::from_secs_f64);
        for event in app.client.wait(timeout)? {
            app.handle(event)?;
        }
        app.housekeeping()?;
    }
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            // a draw follows every configure and scale change
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(surface) if surface == self.surface => self.draw()?,
            Event::Draw(_) => {}
            Event::Closed(_) => return Err("the server closed the overlay surface".into()),
            Event::OutputsChanged => {
                if !self.client.outputs().iter().any(|o| o.id == self.output) {
                    return Err("the output of the overlay went away".into());
                }
            }
            Event::Appearance { .. } | Event::DisplayOptions(_) => {
                self.renderer = overlay_renderer(
                    &self.theme,
                    &self.assets,
                    self.client.is_dark(),
                    self.client.display_options(),
                )?;
                self.client.request_redraw(self.surface)?;
            }
            // the surface takes no input and has no popups or menu-bar role; a lock leaves the overlay alone, which the server hides
            Event::SessionLock { .. }
            | Event::Pointer { .. }
            | Event::Key { .. }
            | Event::KeyboardLeave(_)
            | Event::ActivationToken { .. }
            | Event::PopupDone(_)
            | Event::MenuBar { .. }
            | Event::Dock { .. } => {}
        }
        Ok(())
    }

    /// Carry out the queued commands (a failure is logged and shows nothing) and redraw when something changed or the
    /// lifetime moved to its next phase.
    fn housekeeping(&mut self) -> Result<()> {
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
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// Draw the overlay with the backdrop of its material, or clear the surface when it is gone.
    fn draw(&mut self) -> Result<()> {
        let now = self.now();
        let Some(View { overlay, alpha }) = self.controller.view(now) else {
            self.backdrop = None;
            self.client.set_input_region(self.surface, &[])?;
            self.client.draw(self.surface, |_, _| {})?;
            return Ok(());
        };
        if self.backdrop.is_none() {
            let backdrop = self.client.backdrop(self.surface)?;
            backdrop.set_material(MATERIAL);
            self.backdrop = Some(backdrop);
        }
        let backdrop = self.backdrop.as_ref().expect("created above");
        backdrop.set_shape(&layout::window(), CORNER_RADIUS)?;
        backdrop.set_alpha(alpha.clamp(0.0, 1.0))?;
        // the window ignores the mouse
        self.client.set_input_region(self.surface, &[])?;
        let renderer = &mut self.renderer;
        self.client.draw(self.surface, |pixmap, scale| {
            let rendered = renderer.render(&overlay, scale.factor());
            let paint = PixmapPaint {
                opacity: alpha as f32,
                ..PixmapPaint::default()
            };
            pixmap.draw_pixmap(0, 0, rendered.as_ref(), &paint, Transform::identity(), None);
        })?;
        // frame callbacks pace the fade
        if self.controller.phase(now) == Phase::Fading {
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }
}
