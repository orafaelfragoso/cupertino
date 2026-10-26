//! Glue between the controller and the toolkit surface: draws the card over a server-rendered backdrop, routes pointer
//! and keys to the controller, and keeps the outside-click catcher and the keyboard interactivity in step with the
//! panel's phase.

use cupertino_alsa::DEVICE_DIR;
use cupertino_assets::Assets;
use cupertino_client::{
    key, Anchor, Backdrop, Button, Client, DisplayOptions, Event, KeyboardInteractivity, Layer,
    LayerSurfaceConfig, Margin, OutputId, PointerEvent, SurfaceId,
};
use cupertino_controlcenter::backend::{
    AlsaVolume, MemoryBattery, MemoryBluetooth, MemoryBrightness, MemoryFocusMode, MemoryVolume,
    MemoryWifi, RecordingOverlay,
};
use cupertino_controlcenter::control;
use cupertino_controlcenter::controller::{Backends, Controller};
use cupertino_controlcenter::layout::{
    fraction_at, Hit, Layout, CARD_RADIUS, EDGE_MARGIN, SHADOW_INSETS,
};
use cupertino_controlcenter::lifecycle::Phase;
use cupertino_controlcenter::model::{Battery, Controls, Slider, VolumeState, WifiStatus};
use cupertino_controlcenter::render::{card_origin, surface_size, Palette, Renderer, MATERIAL};
use cupertino_controlcenter::system::{
    BtdBluetooth, NetdWifi, NotificationFocusMode, PowerdBattery, PowerdBrightness, SessionOverlay,
};
use cupertino_geometry::{Point, Rect};
use cupertino_theme::{Appearance, Theme};
use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tiny_skia::{PixmapPaint, Transform};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-controlcenter";

/// Readings of the in-memory backends of `--fake-backends` (arbitrary, for trying the panel).
const FAKE_NETWORK: &str = "Home Network";
const FAKE_BRIGHTNESS: f64 = 0.6;
const FAKE_VOLUME: f64 = 0.4;
const FAKE_BATTERY_PERCENT: u8 = 80;

/// Command line of the process.
pub struct Options {
    /// Use in-memory backends instead of the machine's radios, volume and backlight.
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
    /// The surface of the card and its shadow, mapped all the time; it draws nothing and takes no input while hidden.
    surface: SurfaceId,
    /// Full-output surface below the card that catches a press outside it; exists only while the panel is shown.
    catcher: Option<SurfaceId>,
    output: OutputId,
    controller: Controller,
    commands: Receiver<()>,
    renderer: Renderer,
    start: Instant,
    /// The phase the surfaces were last arranged for.
    phase: Phase,
    /// The readings last drawn, to redraw when they change.
    drawn: Option<Controls>,
    /// Layout of the last drawn frame, for hit testing.
    layout: Layout,
    /// The slider under the pointer's button, while it is held.
    drag: Option<Slider>,
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

fn backends(options: &Options) -> Backends {
    if options.fake_backends {
        return Backends {
            wifi: Box::new(MemoryWifi(WifiStatus {
                enabled: true,
                network: Some(FAKE_NETWORK.to_string()),
            })),
            bluetooth: Box::new(MemoryBluetooth(false)),
            focus_mode: Box::new(MemoryFocusMode(false)),
            brightness: Box::new(MemoryBrightness(FAKE_BRIGHTNESS)),
            volume: Box::new(MemoryVolume(VolumeState {
                level: FAKE_VOLUME,
                muted: false,
            })),
            battery: Box::new(MemoryBattery(Some(Battery {
                percent: FAKE_BATTERY_PERCENT,
                charging: false,
            }))),
            overlay: Box::new(RecordingOverlay::default()),
        };
    }
    Backends {
        wifi: Box::new(NetdWifi::new()),
        bluetooth: Box::new(BtdBluetooth::new()),
        focus_mode: Box::new(NotificationFocusMode::new()),
        brightness: Box::new(PowerdBrightness::new()),
        volume: Box::new(AlsaVolume::new(
            PathBuf::from(DEVICE_DIR),
            options.alsa_card,
        )),
        battery: Box::new(PowerdBattery::new()),
        overlay: Box::new(SessionOverlay::new()),
    }
}

/// The renderer for the session's appearance, in its high-contrast catalog under Increase Contrast.
fn panel_renderer(
    theme: &Theme,
    assets: &Assets,
    dark: bool,
    options: DisplayOptions,
) -> Result<Renderer> {
    let appearance = Appearance::of_session(dark, options.increase_contrast);
    Ok(Renderer::new(
        Palette::from_theme(theme, appearance)?,
        theme.tokens.typography().clone(),
        assets,
    )?)
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let mut client = Client::connect()?;
    let renderer = panel_renderer(&theme, &assets, client.is_dark(), client.display_options())?;
    let waker = client.waker();
    let (sender, commands) = mpsc::channel();
    // the bus thread only queues; the loop below carries the toggles out, so the effects never block the bus
    let sender = Mutex::new(sender);
    let _control = control::serve(zbus::connection::Builder::session()?, move || {
        sender
            .lock()
            .expect("the sender lock is never poisoned")
            .send(())
            .expect("the receiver lives as long as the process");
        waker.wake();
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another control panel owns org.cupertino.ControlPanel1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;

    let output = first_output(&client)?;
    let (width, height) = surface_size();
    let surface = client.layer_surface(&LayerSurfaceConfig {
        namespace: NAMESPACE.to_string(),
        output: Some(output),
        layer: Layer::Overlay,
        // top right, below the menu bar: the bar reserves no layer zone (the server keeps windows clear of it), so the
        // margin carries its height
        anchor: Anchor::Top | Anchor::Right,
        size: (width, height),
        margin: Margin {
            top: theme.shapes.menu_bar_height.ceil() as i32 + EDGE_MARGIN
                - SHADOW_INSETS.top as i32,
            right: EDGE_MARGIN - SHADOW_INSETS.right as i32,
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
        catcher: None,
        output,
        controller: Controller::new(backends(options)),
        commands,
        renderer,
        start: Instant::now(),
        phase: Phase::Hidden,
        drawn: None,
        layout: Layout::new(false),
        drag: None,
        backdrop: None,
    };
    loop {
        let timeout = app
            .controller
            .next_wakeup(app.now())
            .map(Duration::from_secs_f64);
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
            Event::Draw(surface) if Some(surface) == self.catcher => self.draw_catcher(surface)?,
            // of a catcher destroyed since
            Event::Draw(_) => {}
            Event::Closed(_) => return Err("the server closed a control panel surface".into()),
            Event::Pointer { surface, event } if surface == self.surface => self.pointer(event)?,
            // a left or right press outside the card dismisses it (global mouse monitor mask 0xa)
            Event::Pointer {
                surface,
                event:
                    PointerEvent::Button {
                        pressed: true,
                        button: Button::Left | Button::Right,
                        ..
                    },
            } if Some(surface) == self.catcher => self.controller.dismiss(self.now()),
            Event::Pointer { .. } => {}
            // Escape is not in the design; it is added (approximate)
            Event::Key {
                key: key::ESCAPE,
                pressed: true,
                ..
            } => self.controller.dismiss(self.now()),
            Event::Key { .. } => {}
            // the design dismisses when the panel resigns the key window (observer)
            Event::KeyboardLeave(_) => self.controller.dismiss(self.now()),
            Event::OutputsChanged => {
                if !self.client.outputs().iter().any(|o| o.id == self.output) {
                    return Err("the output of the control panel went away".into());
                }
            }
            Event::Appearance { .. } | Event::DisplayOptions(_) => {
                self.renderer = panel_renderer(
                    &self.theme,
                    &self.assets,
                    self.client.is_dark(),
                    self.client.display_options(),
                )?;
                self.client.request_redraw(self.surface)?;
            }
            // the server draws the lock screen: only the shown panel has to go
            Event::SessionLock { locked: true } => self.controller.dismiss(self.now()),
            Event::SessionLock { locked: false } => {}
            // the surfaces have no activation, popups or menu-bar role
            Event::ActivationToken { .. }
            | Event::PopupDone(_)
            | Event::MenuBar { .. }
            | Event::Dock { .. } => {}
        }
        Ok(())
    }

    /// Press a toggle or start, follow and end a slider drag.
    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        let now = self.now();
        if self.controller.phase(now) != Phase::Shown {
            return Ok(());
        }
        let origin = card_origin();
        let card = |p: Point| Point::new(p.x - origin.x, p.y - origin.y);
        match event {
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => match self.layout.hit(card(position)) {
                Hit::Toggle(module) => {
                    if self.controller.press(module) {
                        self.client.request_redraw(self.surface)?;
                    }
                }
                Hit::Slider(slider, fraction) => {
                    self.drag = Some(slider);
                    self.controller.slide(now, slider, fraction);
                    self.client.request_redraw(self.surface)?;
                }
                Hit::Panel | Hit::Outside => {}
            },
            PointerEvent::Motion(position) => {
                if let Some(slider) = self.drag {
                    let fraction = fraction_at(&self.layout.track(slider), card(position).x);
                    self.controller.slide(now, slider, fraction);
                    self.client.request_redraw(self.surface)?;
                }
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } => {
                if self.drag.take().is_some() {
                    self.controller.release(now);
                    self.client.request_redraw(self.surface)?;
                }
            }
            PointerEvent::Enter(_)
            | PointerEvent::Leave
            | PointerEvent::Button { .. }
            | PointerEvent::Scroll { .. } => {}
        }
        Ok(())
    }

    /// Carry out the queued toggles and the controller's timers, arrange the surfaces for the phase and redraw when
    /// something changed.
    fn housekeeping(&mut self) -> Result<()> {
        let now = self.now();
        while self.commands.try_recv().is_ok() {
            self.controller.toggle(now);
        }
        self.controller.tick(now);
        let phase = self.controller.phase(now);
        if phase != self.phase {
            self.arrange(phase)?;
        }
        if phase == Phase::Hidden {
            self.drag = None;
        }
        if phase != Phase::Hidden && self.drawn.as_ref() != Some(self.controller.controls()) {
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// Give the surfaces what `phase` needs: while shown, the keyboard (exclusive, the panel is the key window) and the catcher below the card; otherwise
    /// neither.
    fn arrange(&mut self, phase: Phase) -> Result<()> {
        self.phase = phase;
        if phase == Phase::Shown {
            self.catcher = Some(self.client.layer_surface(&LayerSurfaceConfig {
                namespace: NAMESPACE.to_string(),
                output: Some(self.output),
                layer: Layer::Top,
                anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                size: (0, 0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::None,
            })?);
            self.client
                .set_keyboard_interactivity(self.surface, KeyboardInteractivity::Exclusive)?;
        } else {
            if let Some(catcher) = self.catcher.take() {
                self.client.destroy_surface(catcher)?;
            }
            self.client
                .set_keyboard_interactivity(self.surface, KeyboardInteractivity::None)?;
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Draw the card with the backdrop of its material, or clear the surface when it is gone.
    fn draw(&mut self) -> Result<()> {
        let now = self.now();
        let phase = self.controller.phase(now);
        if phase == Phase::Hidden {
            self.backdrop = None;
            self.drawn = None;
            self.client.set_input_region(self.surface, &[])?;
            self.client.draw(self.surface, |_, _| {})?;
            return Ok(());
        }
        let opacity = self.controller.opacity(now);
        let controls = self.controller.controls().clone();
        self.layout = Layout::new(controls.battery.is_some());
        let origin = card_origin();
        let card = Rect::from_origin_size(origin, self.layout.size());
        if self.backdrop.is_none() {
            let backdrop = self.client.backdrop(self.surface)?;
            backdrop.set_material(MATERIAL);
            self.backdrop = Some(backdrop);
        }
        let backdrop = self.backdrop.as_ref().expect("created above");
        backdrop.set_shape(&card, CARD_RADIUS)?;
        backdrop.set_alpha(opacity)?;
        // the card takes input while shown; a press outside it reaches the catcher
        let region = if phase == Phase::Shown {
            vec![card]
        } else {
            Vec::new()
        };
        self.client.set_input_region(self.surface, &region)?;
        let renderer = &mut self.renderer;
        self.client.draw(self.surface, |pixmap, scale| {
            let rendered = renderer.render(&controls, scale.factor());
            let paint = PixmapPaint {
                opacity: opacity as f32,
                ..PixmapPaint::default()
            };
            pixmap.draw_pixmap(0, 0, rendered.as_ref(), &paint, Transform::identity(), None);
        })?;
        self.drawn = Some(controls);
        // frame callbacks pace the fade
        if phase == Phase::Hiding {
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// Make the catcher take the input of the whole output; it is drawn once and stays transparent.
    fn draw_catcher(&mut self, catcher: SurfaceId) -> Result<()> {
        let (width, height) = self.client.size(catcher)?;
        let region = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        self.client.set_input_region(catcher, &[region])?;
        self.client.draw(catcher, |_, _| {})?;
        Ok(())
    }
}
