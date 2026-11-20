//! The launcher as a component of the shell host (`cupertino-shell`): the panel's window and the catcher under it on
//! layer surfaces, `org.cupertino.Launcher1` on the component's own session bus connection ([`control`]), the
//! applications, the launch history, `launcher.conf` and the terminal read on Kit's I/O pool, and everything the panel
//! asks of the session carried out off the UI thread ([`host`]).
//!
//! The panel opens on the output a `TogglePanel` publication of the host bus names, else on the server's active output
//! (the one the person works on). It takes the exclusive keyboard while it is shown and gives it back as it closes; the
//! server closes its surfaces with their output, which hides it at once, and so does the session lock. Its surfaces are
//! transient: when the component stops they go with it, and the keyboard returns to the window that had it.
//!
//! What a restart loses: the panel's query, results and scroll positions, and a launch made in the last moment whose
//! history write had not finished. The launch history, `launcher.conf` and the file index are read again as the
//! component starts.

mod host;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_apps::catalogue::{ApplicationWatch, Catalogue, Change, Timing, REREAD_DEADLINE};
use cupertino_apps::DesktopEntry;
use cupertino_defaults::launcher::LauncherSettings;
use cupertino_defaults::terminal::{self, DEFAULT_TERMINALS};
use cupertino_defaults::watch::ConfigWatcher;
use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::SystemAppearance;
use cupertino_kit::task;
use cupertino_kit_shell::bus::{self, Message as Published, Panel, Topic};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::session::{self, Activation};
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role,
    Surface,
};

use crate::control::{self, Call};
use crate::engine::{Launcher as Engine, LauncherConfig};
use crate::files::FileSearchConfig;
use crate::frecency::Frecency;
use crate::kit::{self, Message, Model, Running};
use crate::view::{self, Input, SHADOW_TOP, TOP_FRACTION};

pub use host::ComponentHost;

/// The layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-launcher";
/// How long reading a settings file or the launch history should take: one small file.
const READ_DEADLINE: Duration = Duration::from_secs(2);
/// How long finding the terminal should take: a look along `PATH`.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(2);

/// The launcher.
pub struct Launcher;

/// A value that cannot be cloned, carried by a message that must be: the first to take it has it.
pub struct Handed<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handed<T> {
    fn clone(&self) -> Self {
        Handed(Arc::clone(&self.0))
    }
}

impl<T> fmt::Debug for Handed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Handed")
    }
}

impl<T> Handed<T> {
    fn new(value: T) -> Self {
        Handed(Arc::new(Mutex::new(Some(value))))
    }

    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// What the component hears that the panel's model does not: its buses, the server and the session.
#[derive(Clone, Debug)]
pub enum Shell {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// Whether `org.cupertino.Launcher1` is served.
    Served(Result<(), String>),
    /// A call of the bus interface.
    Call(Call),
    /// Open the search field (`false`) or the application browser (`true`), or close what is shown: on this output, or
    /// the server's active one.
    Toggle(bool, Option<OutputId>),
    Appearance(SystemAppearance),
    /// The server's outputs changed.
    Outputs,
    /// The server closed this window.
    Closed(WindowKey),
    /// The terminal that runs entries with `Terminal=true`, when one is installed.
    Terminal(Option<String>),
    /// The applications as the component starts, and what follows their folders from now on.
    Catalogue(Vec<DesktopEntry>, Option<Handed<ApplicationWatch>>),
    /// The applications changed in their folders: all of them, as they are now.
    Installed(Vec<DesktopEntry>),
    /// `launcher.conf` as it is now, and what watches it from now on when it was just set up.
    Settings(LauncherSettings, Option<Handed<ConfigWatcher>>),
    /// `launcher.conf` changed.
    SettingsChanged,
    /// The launch history was written, or why not was reported.
    Saved,
    /// A read that a newer one replaced before it started: nothing to tell.
    Replaced,
}

impl From<Shell> for Message {
    fn from(shell: Shell) -> Self {
        Message::Shell(shell)
    }
}

/// What the component holds: the panel's model and what carries it.
pub struct State {
    model: Model,
    /// Where `org.cupertino.Launcher1` hands its calls, and where they come out.
    calls: (async_channel::Sender<Call>, async_channel::Receiver<Call>),
    /// Where the applications' changes arrive.
    installed: (
        async_channel::Sender<Vec<DesktopEntry>>,
        async_channel::Receiver<Vec<DesktopEntry>>,
    ),
    /// Where the changes of `launcher.conf` are told.
    settings_changed: (async_channel::Sender<()>, async_channel::Receiver<()>),
    /// `launcher.conf` as last read: a line not understood keeps its value.
    settings: Option<LauncherSettings>,
    /// The output the panel opened on; `None` is the server's active output.
    output: Option<OutputId>,
    /// What the host bus was last told: whether the panel is shown, and where.
    told: Option<(bool, Option<OutputId>)>,
    /// The output size the model was last told.
    size: Option<Size>,
    /// The component's session bus connection, which serves `org.cupertino.Launcher1`.
    session: Option<zbus::Connection>,
}

/// The top margin of the panel's window on an output `height` points tall: the panel at [`TOP_FRACTION`] of the
/// height, less the room above it for its shadow.
pub fn top_margin(height: f64) -> i32 {
    (height * TOP_FRACTION - SHADOW_TOP).round().max(0.0) as i32
}

/// The next item of `receiver` as a message; it never ends while the model holds the sender.
fn hear<T: Send + 'static>(
    receiver: &async_channel::Receiver<T>,
    message: impl FnOnce(T) -> Shell + Send + 'static,
) -> Command<Message> {
    let receiver = receiver.clone();
    Command::Perform(Box::pin(async move {
        match receiver.recv().await {
            Ok(item) => Message::Shell(message(item)),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// `launcher.conf` as it is now: a file that cannot be read keeps `last` (the defaults at start). For the I/O pool.
fn load_settings(last: Option<&LauncherSettings>) -> LauncherSettings {
    let Some(path) = LauncherSettings::path() else {
        return last.cloned().unwrap_or(LauncherSettings::UNSET);
    };
    match LauncherSettings::load(&path, last) {
        Ok((settings, _)) => settings,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LauncherSettings::UNSET,
        Err(e) => {
            eprintln!("cupertino-launcher: cannot read {}: {e}", path.display());
            last.cloned().unwrap_or(LauncherSettings::UNSET)
        }
    }
}

/// Read `launcher.conf` and start watching it. Without a watch its changes apply at the next start.
fn first_settings() -> Command<Message> {
    task::io(
        READ_DEADLINE,
        || {
            let settings = load_settings(None);
            let watch = LauncherSettings::path().and_then(|path| {
                ConfigWatcher::new(vec![path.clone()])
                    .map_err(|e| {
                        eprintln!(
                            "cupertino-launcher: cannot watch {}: {e}; changes apply at the next start",
                            path.display()
                        );
                    })
                    .ok()
            });
            (settings, watch)
        },
        |(settings, watch)| Message::Shell(Shell::Settings(settings, watch.map(Handed::new))),
    )
}

/// Read the applications of the session and set up the watch of their folders.
fn first_catalogue() -> Command<Message> {
    task::io(
        REREAD_DEADLINE,
        || {
            let catalogue = Catalogue::from_env();
            let entries = catalogue.entries().to_vec();
            let watch = ApplicationWatch::new(catalogue, Timing::default())
                .map_err(|e| {
                    eprintln!(
                        "cupertino-launcher: cannot watch the application folders: {e}; applications installed                          from now on show after a restart"
                    );
                })
                .ok();
            (entries, watch)
        },
        |(entries, watch)| Message::Shell(Shell::Catalogue(entries, watch.map(Handed::new))),
    )
}

/// The task that follows the application folders while the launcher runs: the folders are read again on the I/O pool,
/// never on the UI thread, and each change goes to `installed`. It never ends.
fn follow_applications(
    mut watch: ApplicationWatch,
    installed: async_channel::Sender<Vec<DesktopEntry>>,
) -> Command<Message> {
    Command::Perform(Box::pin(async move {
        loop {
            let Command::Perform(read) = task::io(
                REREAD_DEADLINE,
                move || {
                    let changes = watch.reread();
                    (watch, changes)
                },
                |read| read,
            ) else {
                return std::future::pending().await;
            };
            let changes;
            (watch, changes) = read.await;
            for change in changes {
                match change {
                    Change::Applications(entries) => {
                        let _ = installed.try_send(entries);
                    }
                    Change::Error(e) => {
                        eprintln!("cupertino-launcher: watching the applications: {e}");
                    }
                }
            }
            if let Err(e) = watch.quiet().await {
                eprintln!(
                    "cupertino-launcher: the application folders are no longer watched: {e}; applications                      installed from now on show after a restart"
                );
                return std::future::pending().await;
            }
        }
    }))
}

/// The terminal of the session: the first of the default entries that is installed. For the I/O pool.
fn find_terminal() -> Command<Message> {
    task::io(
        TERMINAL_DEADLINE,
        || terminal::resolve_installed(&DEFAULT_TERMINALS.map(String::from)),
        |found| Message::Shell(Shell::Terminal(found)),
    )
}

impl State {
    /// The size of the output the panel is on: the one it opened on, else the server's only output, else the first one
    /// the server described. The server puts a panel without an output on its active output, which the component is
    /// not told; on several outputs of different sizes the first one's size places it.
    fn output_size(&self) -> Option<Size> {
        let all = outputs::all();
        let chosen = self
            .output
            .and_then(|id| all.iter().find(|o| o.id == id))
            .and_then(|o| o.geometry);
        chosen
            .or_else(|| all.iter().find_map(|o| o.geometry))
            .map(|g| Size::new(g.width(), g.height()))
    }

    /// Tell the model the output's size when it changed.
    fn measure(&mut self) -> Vec<Command<Message>> {
        let size = self.output_size();
        if size == self.size {
            return Vec::new();
        }
        self.size = size;
        match size {
            Some(size) => kit::update(&mut self.model, Message::Output(size)),
            None => Vec::new(),
        }
    }

    /// Open the search field or the browser, or close what is shown. A panel that opens from hidden goes on `output`
    /// when the server has it, else on the server's active output.
    fn toggle(&mut self, browse: bool, output: Option<OutputId>) -> Vec<Command<Message>> {
        if !self.model.presenting() {
            self.output = output.filter(|id| outputs::get(*id).is_some());
        }
        let mut commands = self.measure();
        let message = if browse {
            Message::Browse
        } else {
            Message::Toggle
        };
        commands.extend(kit::update(&mut self.model, message));
        commands
    }

    /// Tell the host bus whether the panel is shown and where, when that changed.
    fn announce(&mut self) {
        let shown = self.model.nav.is_visible();
        let output = self.output.filter(|_| shown);
        if self.told == Some((shown, output)) {
            return;
        }
        bus::publish(Published::PanelShown {
            panel: Panel::Launcher,
            shown,
            output,
        });
        self.told = Some((shown, output));
    }

    fn shell(&mut self, shell: Shell) -> Vec<Command<Message>> {
        match shell {
            Shell::Session(Some(connection)) => {
                // the connection lives while the component holds it
                self.session = Some(connection.clone());
                let calls = self.calls.0.clone();
                vec![Command::Perform(Box::pin(async move {
                    let served = control::serve(&connection, calls).await;
                    Message::Shell(Shell::Served(served.map_err(|e| match e {
                        zbus::Error::NameTaken => {
                            format!("another launcher owns {}", control::BUS_NAME)
                        }
                        e => e.to_string(),
                    })))
                }))]
            }
            Shell::Session(None) | Shell::SessionClosed => {
                // without its bus connection the component serves nobody: the host starts it again
                eprintln!("cupertino-launcher: no session bus connection");
                vec![Command::Exit]
            }
            Shell::Served(Ok(())) | Shell::Saved | Shell::Replaced => Vec::new(),
            Shell::Served(Err(e)) => {
                eprintln!(
                    "cupertino-launcher: {} is not served: {e}",
                    control::BUS_NAME
                );
                vec![Command::Exit]
            }
            Shell::Call(call) => {
                let mut commands = vec![hear(&self.calls.1, Shell::Call)];
                match call {
                    Call::Toggle => commands.extend(self.toggle(false, None)),
                    Call::Applications => commands.extend(self.toggle(true, None)),
                }
                commands
            }
            Shell::Toggle(browse, output) => self.toggle(browse, output),
            Shell::Appearance(appearance) => {
                let mut commands = kit::update(
                    &mut self.model,
                    Message::ReduceMotion(appearance.reduce_motion),
                );
                commands.extend(kit::update(
                    &mut self.model,
                    Message::SteadyCaret(appearance.steady_caret),
                ));
                commands
            }
            Shell::Outputs => {
                let mut commands = Vec::new();
                // the server closes the surfaces of an output it drops; a panel on one that went goes at once
                if self.output.is_some_and(|id| outputs::get(id).is_none()) {
                    self.output = None;
                    commands.extend(kit::update(&mut self.model, Message::Gone));
                }
                commands.extend(self.measure());
                commands
            }
            Shell::Closed(key) => {
                let message = if key == view::panel_window_key() || key == view::catcher_key() {
                    Message::Gone
                } else {
                    Message::from(Input::CloseMenu)
                };
                kit::update(&mut self.model, message)
            }
            Shell::Terminal(found) => {
                if let Some(program) = found {
                    self.model.launcher.config.terminal_command =
                        terminal::command_prefix(&program);
                }
                Vec::new()
            }
            Shell::Catalogue(entries, watch) => {
                let mut commands = kit::update(&mut self.model, Message::Installed(entries));
                if let Some(watch) = watch.and_then(|w| w.take()) {
                    commands.push(follow_applications(watch, self.installed.0.clone()));
                }
                commands
            }
            Shell::Installed(entries) => {
                let mut commands = vec![hear(&self.installed.1, Shell::Installed)];
                commands.extend(kit::update(&mut self.model, Message::Installed(entries)));
                commands
            }
            Shell::Settings(settings, watch) => {
                self.settings = Some(settings.clone());
                let mut commands = kit::update(&mut self.model, Message::Settings(settings));
                if let Some(watch) = watch.and_then(|w| w.take()) {
                    let changed = self.settings_changed.0.clone();
                    commands.push(Command::Perform(Box::pin(task::follow_settings(
                        watch,
                        move |_| {
                            // the model holds the receiver for as long as it lives
                            let _ = changed.try_send(());
                        },
                    ))));
                }
                commands
            }
            Shell::SettingsChanged => {
                let last = self.settings.clone();
                vec![
                    hear(&self.settings_changed.1, |()| Shell::SettingsChanged),
                    task::io(
                        READ_DEADLINE,
                        move || load_settings(last.as_ref()),
                        |settings| Message::Shell(Shell::Settings(settings, None)),
                    ),
                ]
            }
        }
    }
}

impl Application for Launcher {
    type Model = State;
    type Message = Message;
    const RESIDENT: bool = true;
    /// A frame for every letter typed: on the CPU its raster passes the animation budget, on the GPU it is within it.
    const RENDERER: RendererClass = RendererClass::Gpu;

    fn init() -> (State, Vec<Command<Message>>) {
        bus::follow(&[Topic::Panels]);
        let icons = cupertino_kit_shell::environment().icons;
        let engine = Engine::new(LauncherConfig::default(), Vec::new(), Frecency::in_memory());
        let mut model = Model::new(engine, Box::new(ComponentHost::new(icons)));
        model.files = FileSearchConfig::home();
        model.settled = cupertino_kit_shell::test_clock::motion_settled();
        // the server shows a component's windows' parts as layers of their own, with glass: the compositor runs the
        // panel's open and close, and the panel's window keeps the largest size the panel takes, so the folds and
        // unfolds it runs never wait for the window to be configured again
        model.compositor_layers = true;
        model.window_held = true;
        let state = State {
            model,
            calls: async_channel::unbounded(),
            installed: async_channel::unbounded(),
            settings_changed: async_channel::bounded(1),
            settings: None,
            output: None,
            told: None,
            size: None,
            session: None,
        };
        let mut commands = bus::session(
            |connection| Message::Shell(Shell::Session(connection.ok())),
            Message::Shell(Shell::SessionClosed),
        );
        commands.extend([
            hear(&state.calls.1, Shell::Call),
            hear(&state.installed.1, Shell::Installed),
            hear(&state.settings_changed.1, |()| Shell::SettingsChanged),
            find_terminal(),
            first_catalogue(),
            first_settings(),
            task::io(READ_DEADLINE, Frecency::load_default, Message::History),
        ]);
        (state, commands)
    }

    fn update(state: &mut State, message: Message) -> Vec<Command<Message>> {
        let commands = match message {
            Message::Shell(shell) => state.shell(shell),
            message => kit::update(&mut state.model, message),
        };
        state.announce();
        commands
    }

    fn view(state: &State) -> Vec<Window<Message>> {
        kit::view(&state.model)
    }
}

impl Program for Launcher {
    /// The panel: on the overlay layer, anchored to the top only (so centred across the output) at [`TOP_FRACTION`] of
    /// the output's height, with the exclusive keyboard while it is shown. The catcher: on the top layer under it, over
    /// the whole output, with no keyboard. The view options menu: a popup of the panel's window.
    fn surface(state: &State, window: &WindowKey) -> Surface {
        let spec = |layer, anchor, size, top, keyboard| LayerSpec {
            namespace: NAMESPACE,
            output: state.output,
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
        let role = if *window == view::panel_window_key() {
            let height = state.size.map_or(0.0, |s| s.height);
            let keyboard = if state.model.nav.is_visible() {
                Keyboard::Exclusive
            } else {
                Keyboard::None
            };
            Role::Layer(spec(
                Layer::Overlay,
                Anchor {
                    top: true,
                    ..Anchor::default()
                },
                state.model.extent(),
                top_margin(height),
                keyboard,
            ))
        } else if *window == view::catcher_key() {
            Role::Layer(spec(
                Layer::Top,
                Anchor {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true,
                },
                Size::new(0.0, 0.0),
                0,
                Keyboard::None,
            ))
        } else {
            Role::Window
        };
        Surface {
            role,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        let shell = match event {
            Event::Session(session::Change::Locked(true)) => return Some(Message::Locked),
            Event::Session(session::Change::AppActivation { serial, result }) => {
                let running = match result {
                    Activation::Activated => Running::Raised,
                    Activation::Resident => Running::Resident,
                    Activation::NotRunning => Running::NotRunning,
                };
                return Some(Message::Opened(u64::from(serial), running));
            }
            Event::Appearance(appearance) => Shell::Appearance(appearance),
            Event::Outputs => Shell::Outputs,
            Event::Closed(key) => Shell::Closed(key),
            Event::Bus {
                message: Published::TogglePanel { panel, output },
                ..
            } => match panel {
                Panel::Launcher => Shell::Toggle(false, output),
                Panel::Applications => Shell::Toggle(true, output),
                _ => return None,
            },
            _ => return None,
        };
        Some(Message::Shell(shell))
    }
}

impl Component for Launcher {
    const NAME: ComponentName = ComponentName::LAUNCHER;
}
