//! Glue between the launcher's Kit application ([`cupertino_launcher::kit`]) and the session: the windows get their
//! layer surfaces from [`LauncherRoles`], `Toggle` arrives from the bus as a message, and [`AppHost`] starts what the
//! panel activates, builds the file index on a thread, resizes the panel's surface and loads the results' icons.

use cupertino_assets::Assets;
use cupertino_defaults::terminal::DEFAULT_TERMINALS;
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Application, Command, Window};
use cupertino_kit::platform::WindowSpec;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_launcher::control;
use cupertino_launcher::engine::{ItemKind, ResultItem};
use cupertino_launcher::exec::CommandSpec;
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::kit::{self, Host, Message, Model};
use cupertino_launcher::panel::Now;
use cupertino_launcher::view::{
    APP_ID, CATCHER_APP_ID, DOCUMENT_ICON, FOLDER_ICON, GENERIC_APPLICATION_ICON, HIDDEN_APP_ID,
    ICON_SIZE, SEARCH_SYMBOL, TOP_FRACTION, WIDTH,
};
use cupertino_launcher::{Launcher, LauncherConfig};
use cupertino_modifiers::Profile;
use cupertino_protocol_app::AppEvent;
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{Connection, Error as WaylandError, ExtensionEvent, SurfaceId};
use std::cell::RefCell;
use std::error::Error;
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-launcher";

/// The appear animation's frame interval.
const FRAME: Duration = Duration::from_micros(16_667);

/// Device pixels the results' icons are loaded at: their size at scale 2, so they stay sharp on a HiDPI output.
const ICON_PX: u32 = (ICON_SIZE as u32) * 2;

/// Command line of the process.
pub struct Options {
    /// Index the home directory for file results.
    pub files: bool,
    /// Terminal emulator that runs entries with `Terminal=true`; the launcher default when absent.
    pub terminal: Option<String>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            files: true,
            terminal: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-files" => options.files = false,
                "--terminal" => {
                    options.terminal = Some(args.next().ok_or("--terminal needs a program")?)
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

/// The layer surfaces of the launcher's windows, by their `app_id`.
#[derive(Default)]
struct Surfaces {
    panel: Option<LayerSurface>,
    /// The panel's top margin as last asked for.
    panel_margin: i32,
    catcher: Option<LayerSurface>,
    hidden: Option<LayerSurface>,
}

impl Surfaces {
    fn all(&self) -> impl Iterator<Item = &LayerSurface> {
        [&self.panel, &self.catcher, &self.hidden]
            .into_iter()
            .flatten()
    }
}

/// Gives the windows their layer surfaces, none of them on an output, so the server puts them on the output under the
/// pointer: the panel on the overlay layer, anchored to the top only (so centred horizontally) at one fifth of the
/// output's height, with the keyboard; the catcher over the whole output on the top layer, below the panel; the
/// hidden window a point that takes no input.
struct LauncherRoles {
    shell: LayerShell,
    surfaces: Rc<RefCell<Surfaces>>,
    /// Where a session lock is told to the model (the window system tells it only to the accessibility adapter).
    signals: async_channel::Sender<Signal>,
}

/// What reaches the model from outside the window system.
#[derive(Clone, Copy, Debug)]
enum Signal {
    /// `org.cupertino.Launcher1.Toggle`.
    Toggle,
    /// The session was locked.
    Locked,
}

/// The panel's top margin on an output `height` points tall.
fn top_margin(height: f64) -> i32 {
    (height * TOP_FRACTION).round() as i32
}

impl LauncherRoles {
    fn config(conn: &Connection, spec: &WindowSpec) -> Option<LayerSurfaceConfig> {
        let config = |layer, anchor, size, top, keyboard| LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output: None,
            layer,
            anchor,
            size,
            margin: Margin {
                top,
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard,
        };
        Some(match spec.app_id.as_str() {
            APP_ID => {
                // the output the server picks is not known before the configure: the first output's height guesses
                let height = conn
                    .outputs()
                    .into_iter()
                    .find_map(|o| o.geometry)
                    .map_or(0.0, |g| f64::from(g.height));
                config(
                    Layer::Overlay,
                    Anchor::Top,
                    (WIDTH as u32, spec.size.height.ceil() as u32),
                    top_margin(height),
                    KeyboardInteractivity::Exclusive,
                )
            }
            CATCHER_APP_ID => config(
                Layer::Top,
                Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                (0, 0),
                0,
                KeyboardInteractivity::None,
            ),
            HIDDEN_APP_ID => config(
                Layer::Overlay,
                Anchor::Top,
                (1, 1),
                0,
                KeyboardInteractivity::None,
            ),
            _ => return None,
        })
    }
}

impl SurfaceRoles for LauncherRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let config = Self::config(conn, spec)?;
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => return Some(Err(WaylandError::Protocol("no output for the launcher"))),
        };
        let surface = layer.surface();
        let mut surfaces = self.surfaces.borrow_mut();
        match spec.app_id.as_str() {
            APP_ID => {
                surfaces.panel_margin = config.margin.top;
                surfaces.panel = Some(layer);
            }
            CATCHER_APP_ID => surfaces.catcher = Some(layer),
            _ => {
                // the hidden window never takes the pointer
                if let Err(e) = conn.set_input_region(surface, Some(&[])) {
                    return Some(Err(e));
                }
                surfaces.hidden = Some(layer);
            }
        }
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(AppEvent::SessionLock { locked: true }) = event.downcast_ref::<AppEvent>() {
            // not a role event: the window system still tells the accessibility adapter
            let _ = self.signals.try_send(Signal::Locked);
            return None;
        }
        let surfaces = &mut *self.surfaces.borrow_mut();
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = surfaces.all().find(|l| l.surface() == surface)?;
                // a configure without a size closes the window, which the model answers
                let size = match layer.configured(size) {
                    Ok(size) => size,
                    Err(_) => return Some(RoleEvent::Closed(surface)),
                };
                // the catcher covers the output the server put both on: its height places the panel, whose margin
                // guessed from the first output, before the panel's first frame commits it
                if surfaces
                    .catcher
                    .as_ref()
                    .is_some_and(|c| c.surface() == surface)
                {
                    let margin = top_margin(f64::from(size.1));
                    if let Some(panel) = &surfaces.panel {
                        if margin != surfaces.panel_margin {
                            panel.set_margin(Margin {
                                top: margin,
                                ..Margin::default()
                            });
                            surfaces.panel_margin = margin;
                        }
                    }
                }
                Some(RoleEvent::Configure { surface, size })
            }
            // their output went away
            ShellEvent::Closed(surface) if surfaces.all().any(|l| l.surface() == surface) => {
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        let surfaces = &mut *self.surfaces.borrow_mut();
        for slot in [
            &mut surfaces.panel,
            &mut surfaces.catcher,
            &mut surfaces.hidden,
        ] {
            if slot.as_ref().is_some_and(|l| l.surface() == surface) {
                *slot = None;
            }
        }
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Run a command detached from the loop; its exit is reaped on a thread.
fn spawn(mut command: Process) {
    let program = command.get_program().to_string_lossy().into_owned();
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-launcher: {program}: {e}");
                }
            });
        }
        Err(e) => eprintln!("cupertino-launcher: cannot run {program}: {e}"),
    }
}

/// A premultiplied pixmap of the icon loaders as a Kit image.
fn image(width: u32, height: u32, pixels: &[u8]) -> ImageSource {
    ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 0,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        },
    }
}

struct AppHost {
    surfaces: Rc<RefCell<Surfaces>>,
    start: Instant,
    /// Applications' own icons (`Icon=`).
    themes: ThemeIcons,
    /// The generic application, folder and document icons.
    system: SystemIcons,
}

impl Host for AppHost {
    fn run(&mut self, spec: CommandSpec, token: Option<String>) {
        let spec = match token {
            Some(token) => spec.with_activation_token(&token),
            None => spec,
        };
        spawn(spec.to_command());
    }

    fn open(&mut self, target: &str) {
        let mut command = Process::new("xdg-open");
        command.arg(target);
        spawn(command);
    }

    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>> {
        let (sender, built) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(Arc::new(FileIndex::build(config)));
        });
        vec![Command::Perform(Box::pin(async move {
            built
                .recv()
                .await
                .map_or(Message::IndexFailed, Message::Indexed)
        }))]
    }

    fn resize(&mut self, height: f64) {
        if let Some(panel) = self.surfaces.borrow_mut().panel.as_mut() {
            panel.set_placement(Anchor::Top, (WIDTH as u32, height.ceil() as u32));
        }
    }

    fn frame(&mut self, frame: u64) -> Vec<Command<Message>> {
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(FRAME).await;
            Message::Frame(frame)
        }))]
    }

    fn icon(&mut self, item: &ResultItem) -> Option<ImageSource> {
        let raster = |system: &mut SystemIcons, name| {
            let pixmap = system.raster(name, ICON_PX);
            image(pixmap.width(), pixmap.height(), pixmap.data())
        };
        match item.kind {
            ItemKind::Application | ItemKind::ApplicationAction => Some(
                match item
                    .icon
                    .as_deref()
                    .and_then(|n| self.themes.load(n, ICON_PX))
                {
                    Some(pixmap) => image(pixmap.width(), pixmap.height(), pixmap.data()),
                    None => raster(&mut self.system, GENERIC_APPLICATION_ICON),
                },
            ),
            ItemKind::Folder => Some(raster(&mut self.system, FOLDER_ICON)),
            ItemKind::File => Some(raster(&mut self.system, DOCUMENT_ICON)),
            ItemKind::WebSearch | ItemKind::Calculator => None,
        }
    }

    fn now(&self) -> Now {
        Now {
            mono: self.start.elapsed().as_secs_f64(),
            unix: unix_now(),
        }
    }
}

/// The model and the channel `Toggle` and the session lock arrive on.
pub struct App {
    model: Model,
    signals: async_channel::Receiver<Signal>,
}

fn listen(signals: &async_channel::Receiver<Signal>) -> Command<Message> {
    let signals = signals.clone();
    Command::Perform(Box::pin(async move {
        signals
            .recv()
            .await
            .map_or(Message::Unheard, |signal| match signal {
                Signal::Toggle => Message::Toggle,
                Signal::Locked => Message::Locked,
            })
    }))
}

/// A launcher without a host or a bus; [`run`] starts the process's own.
struct Unhosted;

impl Host for Unhosted {
    fn run(&mut self, _: CommandSpec, _: Option<String>) {}
    fn open(&mut self, _: &str) {}
    fn index(&mut self, _: FileSearchConfig) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn resize(&mut self, _: f64) {}
    fn frame(&mut self, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn icon(&mut self, _: &ResultItem) -> Option<ImageSource> {
        None
    }
    fn now(&self) -> Now {
        Now {
            mono: 0.0,
            unix: unix_now(),
        }
    }
}

impl Application for App {
    type Model = App;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (App, Vec<Command<Message>>) {
        let launcher = Launcher::new(
            LauncherConfig::default(),
            Vec::new(),
            cupertino_launcher::Frecency::in_memory(),
        );
        let app = App {
            model: Model::new(launcher, Box::new(Unhosted)),
            signals: async_channel::unbounded().1,
        };
        let first = listen(&app.signals);
        (app, vec![first])
    }

    fn update(app: &mut App, message: Message) -> Vec<Command<Message>> {
        let signalled = matches!(message, Message::Toggle | Message::Locked);
        let mut commands = kit::update(&mut app.model, message);
        if signalled {
            commands.push(listen(&app.signals));
        }
        commands
    }

    fn view(app: &App) -> Vec<Window<Message>> {
        kit::view(&app.model)
    }
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let shell =
        LayerShell::bind(system.connection()).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let mut config = LauncherConfig::default();
    let terminal = options.terminal.clone().or_else(|| {
        let entries = DEFAULT_TERMINALS.map(String::from);
        cupertino_defaults::terminal::resolve_installed(&entries)
    });
    if let Some(terminal) = terminal {
        config.terminal_command = cupertino_defaults::terminal::command_prefix(&terminal);
    }
    let files = if options.files {
        Some(
            FileSearchConfig::home()
                .ok_or("HOME is not set (start with --no-files to skip file results)")?,
        )
    } else {
        None
    };
    let (sender, signals) = async_channel::unbounded();
    let toggles = sender.clone();
    let _control = control::serve(zbus::connection::Builder::session()?, move || {
        let _ = toggles.send_blocking(Signal::Toggle);
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another launcher owns org.cupertino.Launcher1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let surfaces = Rc::new(RefCell::new(Surfaces::default()));
    let system = system.with_roles(LauncherRoles {
        shell,
        surfaces: Rc::clone(&surfaces),
        signals: sender,
    });
    let assets = Assets::discover();
    let host = AppHost {
        surfaces,
        start: Instant::now(),
        themes: ThemeIcons::new(&assets),
        system: SystemIcons::required(
            &assets,
            &[GENERIC_APPLICATION_ICON, FOLDER_ICON, DOCUMENT_ICON],
            &[SEARCH_SYMBOL],
        ),
    };
    let mut model = Model::new(Launcher::from_system(config), Box::new(host));
    model.files = files;
    model.settled = cupertino_shell_client::test_clock::motion_settled();
    let app = App { model, signals };
    let first = listen(&app.signals);
    cupertino_kit::app::run_on::<App>(system, (app, vec![first]))?;
    Err("the launcher's surfaces were closed".into())
}
