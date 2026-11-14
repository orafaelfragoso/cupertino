//! The `cupertino-screenshot` process: one capture from a shortcut. `--mode screen` captures every output at once;
//! `--mode selection` and `--mode toolbar` put an overlay on every output (and the toolbar) and run the
//! [`Controller`] until it captures or is cancelled. Its windows are CupertinoKit's on layer surfaces ([`Roles`]); the
//! capture thread ([`capture`]) copies the outputs once those surfaces are gone, and the images go to files
//! ([`deliver`]) or to the clipboard.

use std::collections::HashSet;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_geometry::Rect;
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::WindowSpec;
use cupertino_modifiers::Profile;
use cupertino_screenshot::controller::{Controller, Effect};
use cupertino_screenshot::image;
use cupertino_screenshot::session::Style;
use cupertino_screenshot::settings::Settings;
use cupertino_screenshot::values::{THUMBNAIL_MARGIN, THUMBNAIL_SHADOW_OUTSET, TOOLBAR_BOTTOM};
use cupertino_screenshot::view::{self, Input, CLIPBOARD_APP_ID, THUMBNAIL_APP_ID, TOOLBAR_APP_ID};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};

use crate::capture::{self, Capturer, Job, Reply, Worker};
use crate::deliver;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Layer-shell namespace of the overlay and the toolbar; the server starts no second process while one is mapped.
const OVERLAY_NAMESPACE: &str = "cupertino-screenshot";
/// Namespaces of the thumbnail and of the pixel that takes the keyboard focus the clipboard needs. Neither is the
/// overlay's, so a new shortcut works while they are up.
const THUMBNAIL_NAMESPACE: &str = "cupertino-screenshot-thumbnail";
const CLIPBOARD_NAMESPACE: &str = "cupertino-screenshot-clipboard";

/// How the process was started (the server appends these per shortcut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    /// Capture every output at once.
    Screen,
    /// The crosshair selection.
    Selection,
    /// The toolbar.
    Toolbar,
}

/// Command line of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub mode: StartMode,
    /// Send the capture to the clipboard instead of a file.
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

/// An output the overlay covers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Screen {
    id: OutputId,
    rect: Rect,
}

fn screens(conn: &Connection) -> Vec<Screen> {
    conn.outputs()
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
        .collect()
}

/// What the application and the roles share: the job that waits for the tool's surfaces to go, and the capture
/// thread it goes to.
struct Gate {
    worker: Worker,
    /// The overlay and toolbar surfaces that are up.
    live: HashSet<SurfaceId>,
    /// A capture waits until `live` is empty, so the destroy requests reach the server before it.
    capture: Option<Job>,
    /// The PNG to offer once the clipboard pixel's surface exists.
    clipboard: Option<Vec<u8>>,
    /// Keyboard enters the capture thread heard.
    enters: Arc<AtomicU32>,
}

impl Gate {
    fn capture(&mut self, job: Job) {
        if self.live.is_empty() {
            self.worker.send(job);
        } else {
            self.capture = Some(job);
        }
    }
}

type Shared = Arc<Mutex<Gate>>;

fn lock(gate: &Shared) -> std::sync::MutexGuard<'_, Gate> {
    gate.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives the tool's windows their layer surfaces: an overlay on each output, over everything and the exclusive
/// zones; the toolbar at the bottom of the first output, above the dock's zone; the thumbnail in its bottom-right
/// corner; the clipboard pixel. The direct shortcut's first overlay takes the keyboard, otherwise the toolbar does,
/// and the clipboard pixel does while the offer is made; the thumbnail never does.
struct Roles {
    shell: LayerShell,
    outputs: Vec<OutputId>,
    screens: Vec<Screen>,
    style: Option<Style>,
    layers: Vec<LayerSurface>,
    gate: Shared,
    requests: async_channel::Sender<Message>,
}

impl Roles {
    fn config(&self, spec: &WindowSpec) -> Option<LayerSurfaceConfig> {
        let first = self.outputs.first().copied();
        let size = (
            spec.size.width.ceil() as u32,
            spec.size.height.ceil() as u32,
        );
        if let Some(index) = view::overlay_index(&spec.app_id) {
            let keys = self.style == Some(Style::Simple) && index == 0;
            return Some(LayerSurfaceConfig {
                namespace: OVERLAY_NAMESPACE.into(),
                output: Some(*self.outputs.get(index)?),
                layer: Layer::Overlay,
                anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                size: (0, 0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: if keys {
                    KeyboardInteractivity::Exclusive
                } else {
                    KeyboardInteractivity::None
                },
            });
        }
        Some(match spec.app_id.as_str() {
            // created after the overlays, so it is stacked above them
            TOOLBAR_APP_ID => LayerSurfaceConfig {
                namespace: OVERLAY_NAMESPACE.into(),
                output: first,
                layer: Layer::Overlay,
                anchor: Anchor::Bottom,
                size,
                margin: Margin {
                    bottom: TOOLBAR_BOTTOM,
                    ..Margin::default()
                },
                exclusive_zone: 0,
                keyboard: KeyboardInteractivity::Exclusive,
            },
            // the picture sits THUMBNAIL_MARGIN inside the corner; its surface carries the shadow's room around it,
            // so the margins are smaller by that much
            THUMBNAIL_APP_ID => {
                let margin = (THUMBNAIL_MARGIN - THUMBNAIL_SHADOW_OUTSET) as i32;
                LayerSurfaceConfig {
                    namespace: THUMBNAIL_NAMESPACE.into(),
                    output: first,
                    layer: Layer::Overlay,
                    anchor: Anchor::Bottom | Anchor::Right,
                    size,
                    margin: Margin {
                        right: margin,
                        bottom: margin,
                        ..Margin::default()
                    },
                    exclusive_zone: 0,
                    keyboard: KeyboardInteractivity::None,
                }
            }
            CLIPBOARD_APP_ID => LayerSurfaceConfig {
                namespace: CLIPBOARD_NAMESPACE.into(),
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor::Top | Anchor::Left,
                size: (1, 1),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::Exclusive,
            },
            _ => return None,
        })
    }

    fn layer(&self, surface: SurfaceId) -> Option<&LayerSurface> {
        self.layers.iter().find(|l| l.surface() == surface)
    }
}

impl SurfaceRoles for Roles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let config = self.config(spec)?;
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => {
                return Some(Err(WaylandError::Protocol(
                    "the screenshot's output is gone",
                )))
            }
        };
        let surface = layer.surface();
        let mut gate = lock(&self.gate);
        if config.namespace == OVERLAY_NAMESPACE {
            gate.live.insert(surface);
        } else if spec.app_id == CLIPBOARD_APP_ID {
            // the enter this surface brings comes after the count now
            let after = gate.enters.load(Ordering::SeqCst);
            if let Some(png) = gate.clipboard.take() {
                gate.worker.send(Job::Clipboard { png, after });
            }
        }
        drop(gate);
        self.layers.push(layer);
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = self.layer(surface)?;
                Some(match layer.configured(size) {
                    Ok(size) => RoleEvent::Configure { surface, size },
                    Err(_) => RoleEvent::Closed(surface),
                })
            }
            ShellEvent::Closed(surface) => {
                self.layer(surface)?;
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    /// The last overlay or toolbar surface is gone: the capture waiting for it goes to the capture thread.
    fn destroyed(&mut self, surface: SurfaceId) {
        self.layers.retain(|l| l.surface() != surface);
        let mut gate = lock(&self.gate);
        if gate.live.remove(&surface) && gate.live.is_empty() {
            if let Some(job) = gate.capture.take() {
                gate.worker.send(job);
            }
        }
    }

    /// The description the process connected with also reports as a change; only a real one ends the run.
    fn outputs_changed(&mut self, conn: &Connection) {
        if screens(conn) != self.screens {
            let _ = self.requests.send_blocking(Message::OutputsChanged);
        }
    }
}

pub struct App {
    controller: Controller,
    start: Instant,
    gate: Shared,
    screens: Vec<Screen>,
    settings_path: PathBuf,
    replies: async_channel::Receiver<Reply>,
    requests: async_channel::Receiver<Message>,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    /// How the run ended, for [`run`] to return.
    result: Arc<Mutex<Option<std::result::Result<(), String>>>>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    Reply(Reply),
    Timer(u64),
    OutputsChanged,
    /// The windows are gone: end the process.
    Exit,
    /// The capture thread or the roles are gone.
    Unheard,
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Carry out the controller's effects.
    fn effects(&mut self, effects: Vec<Effect>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for effect in effects {
            let done = match effect {
                Effect::StoreSettings => self
                    .controller
                    .settings()
                    .store(&self.settings_path)
                    .map_err(|e| format!("{}: {e}", self.settings_path.display())),
                Effect::Capture { target, pointer } => {
                    let screens = self.screens.iter().map(|s| (s.id, s.rect)).collect();
                    lock(&self.gate).capture(Job::Capture {
                        target,
                        pointer,
                        screens,
                    });
                    Ok(())
                }
                Effect::Write => deliver::directory(self.controller.settings())
                    .and_then(|dir| deliver::write_files(&dir, self.controller.images()))
                    .map_err(|e| e.to_string()),
                Effect::Offer => match image::png(&self.controller.images()[0]) {
                    Ok(png) => {
                        lock(&self.gate).clipboard = Some(png);
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                Effect::Exit(result) => {
                    commands.push(self.exit(result));
                    return commands;
                }
            };
            if let Err(e) = done {
                commands.push(self.exit(Err(e)));
                return commands;
            }
        }
        commands
    }

    /// End the run with `result` once the windows the update closes are gone.
    fn exit(&mut self, result: std::result::Result<(), String>) -> Command<Message> {
        let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert(result);
        Command::Perform(Box::pin(async { Message::Exit }))
    }

    /// The timer for the controller's next deadline.
    fn schedule(&mut self) -> Vec<Command<Message>> {
        self.timer += 1;
        let now = self.now();
        let Some(at) = self.controller.deadline(now) else {
            return Vec::new();
        };
        let wait = Duration::from_secs_f64((at - now).max(0.0));
        let timer = self.timer;
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(wait).await;
            Message::Timer(timer)
        }))]
    }
}

fn listen<T: Send + 'static>(
    from: &async_channel::Receiver<T>,
    to: fn(T) -> Message,
) -> Command<Message> {
    let from = from.clone();
    Command::Perform(Box::pin(async move {
        from.recv().await.map_or(Message::Unheard, to)
    }))
}

fn input(input: Input) -> Message {
    Message::Input(input)
}

impl Application for App {
    type Model = App;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The toolbar is small and the selection overlay is shown for a moment, mostly at rest: a GPU context would only
    /// cost memory.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

    /// [`run`] starts the process from its connection; there is no run without one.
    fn init() -> (App, Vec<Command<Message>>) {
        unreachable!("cupertino-screenshot starts from its connection")
    }

    fn update(model: &mut App, message: Message) -> Vec<Command<Message>> {
        let now = model.now();
        let mut commands = match message {
            Message::Input(input) => {
                let effects = model.controller.input(input, now);
                model.effects(effects)
            }
            Message::Reply(reply) => {
                let mut commands = vec![listen(&model.replies, Message::Reply)];
                let effects = match reply {
                    Reply::Windows(windows) => {
                        model.controller.set_windows(windows);
                        Vec::new()
                    }
                    Reply::Shots(shots) => model.controller.shots(shots, now),
                    Reply::Offered => {
                        model.controller.offered();
                        Vec::new()
                    }
                    Reply::ClipboardDone(result) => model.controller.clipboard_done(result),
                };
                commands.extend(model.effects(effects));
                commands
            }
            Message::Timer(timer) if timer == model.timer => {
                let effects = model.controller.tick(now);
                model.effects(effects)
            }
            Message::Timer(_) => return Vec::new(),
            Message::OutputsChanged => {
                vec![model.exit(Err("the outputs changed during the screenshot".into()))]
            }
            Message::Exit => return vec![Command::Exit],
            Message::Unheard => vec![model.exit(Err("the capture thread stopped".into()))],
        };
        commands.extend(model.schedule());
        commands
    }

    fn view(model: &App) -> Vec<Window<Message>> {
        model.controller.windows(model.now(), input)
    }
}

/// Connect, run the capture the options ask for and deliver it.
pub fn run(options: &Options) -> Result<()> {
    let settings_path =
        cupertino_defaults::config_file("CUPERTINO_SCREENSHOT_CONFIG", "screenshot.conf")
            .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    let (settings, problems) = Settings::load(&settings_path)?;
    for p in problems {
        eprintln!("cupertino-screenshot: {}: {p}", settings_path.display());
    }
    // the toolkit and the capture thread share the connection the server started the process with
    let wayland = wayland_client::Connection::connect_to_env()?;
    let mut conn = Connection::from_wayland(wayland.clone())?;
    let shell = LayerShell::bind(&mut conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let screens = screens(&conn);
    if screens.is_empty() {
        return Err("the server has no output".into());
    }
    let capturer = Capturer::new(&wayland)?;
    let (reply_to, replies) = async_channel::unbounded();
    let enters = Arc::new(AtomicU32::new(0));
    let worker = capture::spawn(capturer, reply_to, Arc::clone(&enters))?;
    let gate = Arc::new(Mutex::new(Gate {
        worker,
        live: HashSet::new(),
        capture: None,
        clipboard: None,
        enters,
    }));
    let style = match options.mode {
        StartMode::Screen => None,
        StartMode::Selection => Some(Style::Simple),
        StartMode::Toolbar => Some(Style::Interactive),
    };
    let (request_to, requests) = async_channel::unbounded();
    // the roles go with the window system; a channel closed then would wake a loop that is gone
    let keep_open = request_to.clone();
    let system =
        WaylandWindowSystem::new_as(conn, Profile::cupertino(), App::RENDERER).with_roles(Roles {
            shell,
            outputs: screens.iter().map(|s| s.id).collect(),
            screens: screens.clone(),
            style,
            layers: Vec::new(),
            gate: Arc::clone(&gate),
            requests: request_to,
        });
    let (controller, effects) = Controller::new(
        style,
        settings,
        screens.iter().map(|s| s.rect).collect(),
        options.clipboard,
    );
    let result = Arc::new(Mutex::new(None));
    let mut app = App {
        controller,
        start: Instant::now(),
        gate,
        screens,
        settings_path,
        replies,
        requests,
        timer: 0,
        result: Arc::clone(&result),
    };
    let mut commands = vec![
        listen(&app.replies, Message::Reply),
        listen(&app.requests, |m| m),
    ];
    commands.extend(app.effects(effects));
    commands.extend(app.schedule());
    let ran = cupertino_kit::app::run_on::<App>(system, (app, commands));
    drop(keep_open);
    ran?;
    let outcome = result.lock().unwrap_or_else(|e| e.into_inner()).take();
    match outcome {
        Some(Ok(())) => Ok(()),
        Some(Err(problem)) => Err(problem.into()),
        None => Err("the screenshot's windows went away".into()),
    }
}

#[cfg(test)]
mod renderer_class {
    use super::*;

    /// The toolbar is small and the overlay redraws only its selection.
    #[test]
    fn the_toolbar_and_the_overlay_draw_on_the_cpu() {
        assert_eq!(
            <App as Application>::RENDERER,
            cupertino_kit::core::RendererClass::Cpu
        );
    }
}
