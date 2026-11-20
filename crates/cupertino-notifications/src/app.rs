//! Glue between the notification model, the D-Bus server and CupertinoKit: the windows of
//! `cupertino_notifications::view` get their layer surfaces from [`NotificationRoles`], the bus wakes the
//! application's loop through a channel, and timers pace the banners' lifetimes and animations.

use cupertino_defaults::notifications::NotificationSettings;
use cupertino_defaults::watch::ConfigWatcher;
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::core::{Application, Command, Key, RuntimeOptions, Window, WindowKey};
use cupertino_kit::platform::WindowSpec;
use cupertino_notifications::dbus::{CentrePresence, CentrePublisher, Server, ServerOptions};
use cupertino_notifications::view::{
    self, banner_key, centre_key, CardMetrics, Fold, HostAction, Input, Ui, CENTRE_TITLE,
    SHADOW_MARGIN,
};
use cupertino_notifications::{CenterConfig, StackLayout, Wakeup};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::app::AppEvent;
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-notifications";

/// The animations' frame interval.
const FRAME: Duration = Duration::from_micros(16_667);

/// Command line of the process.
pub struct Options {
    /// Take `org.freedesktop.Notifications` from a running daemon that allows replacement.
    pub replace_existing: bool,
}

impl Options {
    /// Parse the arguments; the unknown one is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            replace_existing: false,
        };
        for arg in args {
            match arg.as_str() {
                "--replace" => options.replace_existing = true,
                _ => return Err(arg),
            }
        }
        Ok(options)
    }
}

/// Another notification daemon owns `org.freedesktop.Notifications` and `--replace` was not given. This process has
/// nothing wrong: it leaves the name to the daemon that has it, and does not fail, so the server does not restart it.
#[derive(Debug)]
pub struct NameOwned;

impl std::fmt::Display for NameOwned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another notification daemon owns org.freedesktop.Notifications (start with --replace to take it over)")
    }
}

impl Error for NameOwned {}

/// What a failed start of the bus server is called.
fn start_error(e: zbus::Error) -> Box<dyn Error> {
    match e {
        zbus::Error::NameTaken => Box::new(NameOwned),
        e => Box::new(e),
    }
}

/// What the bus thread and the surface roles tell the application's loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The model changed on the bus.
    Changed,
    /// `ToggleCentre` was called.
    Toggle,
    /// The session was locked (`true`) or unlocked: the server draws the lock screen and hides every other surface,
    /// so the open centre goes and no window is open until the unlock.
    Lock(bool),
    /// The session's reduce-motion option.
    ReduceMotion(bool),
    /// `notifications.conf` changed.
    Policy,
    /// Another surface took the keyboard from the list at this opening of it (another shell panel opened, as a
    /// rule).
    Unfocused(u64),
    /// The compositor's outputs, by their numbers: a press outside the open list is heard on each.
    Outputs(Vec<u32>),
}

/// Whether the list losing the keyboard means another surface took it. The list asks for the exclusive keyboard while
/// it is open and lets it go as a close begins, so the leave that answers its own letting go is no reason to close;
/// one that comes while it holds the keyboard and still wants it is. An opening of the list that lost the keyboard
/// that way does not ask for it again: the compositor gives the keyboard to an exclusive surface at each of its
/// commits, so a list that kept asking would take it back from the panel that has it now.
#[derive(Debug, Default)]
struct ListKeyboard {
    /// The list heard the keyboard enter since it last asked for it or let it go.
    entered: bool,
    /// The opening of the list that lost the keyboard to another surface; 0 when none did.
    lost: u64,
}

impl ListKeyboard {
    /// The list asked for the keyboard, or let it go: what it hears next answers that.
    fn asked(&mut self) {
        self.entered = false;
    }

    /// Whether the list wants the keyboard at `opening`, the number of the opening it is at (0 while it is closed or
    /// closing).
    fn wants(&self, opening: u64) -> bool {
        opening != 0 && opening != self.lost
    }

    /// The list's surface took (`focused`) or lost the keyboard at `opening`. True when the list is to close, having
    /// lost the keyboard it held and wanted; that opening wants it no more.
    fn focus(&mut self, focused: bool, opening: u64) -> bool {
        if focused {
            self.entered = true;
            return false;
        }
        let lost = std::mem::take(&mut self.entered) && self.wants(opening);
        if lost {
            self.lost = opening;
        }
        lost
    }
}

/// The output a window that hears presses outside the list names for its surface: the one it is for while the
/// compositor still has it, else none, which leaves the choice to the compositor (an output that went a moment ago
/// refuses no surface that way).
fn catcher_surface_output(wanted: Option<u32>, outputs: &[u32]) -> Option<u32> {
    wanted.filter(|n| outputs.contains(n))
}

/// Gives the windows of the process their layer surfaces on the overlay layer, at the right edge of the output the
/// server chooses: a banner's at the top, as tall as its window, taking the pointer only on its card at rest; the
/// centre's from top to bottom, with the keyboard while it is open (Escape closes it). While the centre is open each
/// output has a transparent surface on the top layer over all of it, under the centre's, which takes the press that
/// closes the centre.
struct NotificationRoles {
    shell: LayerShell,
    layout: StackLayout,
    layers: HashMap<SurfaceId, LayerSurface>,
    notes: async_channel::Sender<Note>,
    /// The surface of the centre's list while it is open.
    centre: Option<SurfaceId>,
    /// Tells the bus whether the list is open and on which output.
    publisher: CentrePublisher,
    /// The opening of the list that wants the keyboard: a number each opening has to itself while the list is open, 0
    /// from the moment a close begins.
    opening: Arc<AtomicU64>,
    /// The keyboard was let go from the list's surface, which a close does at once.
    keys_released: bool,
    keyboard: ListKeyboard,
    /// The outputs the application was last told of.
    outputs: Option<Vec<u32>>,
}

/// The compositor's outputs by their numbers, in its order.
fn output_numbers(conn: &Connection) -> Vec<u32> {
    conn.outputs()
        .into_iter()
        .map(|o| o.id.registry_name())
        .collect()
}

/// What the bus says of the centre's list: open while its surface exists, on the output of the given connector name
/// once the surface was told it.
fn centre_presence(open: bool, output: Option<String>) -> CentrePresence {
    if !open {
        return CentrePresence::default();
    }
    CentrePresence {
        shown: true,
        output: output.unwrap_or_default(),
    }
}

impl NotificationRoles {
    /// The surface of a window that hears presses outside the list: the top layer, over the whole of `output`,
    /// without the keyboard.
    fn catcher_config(output: Option<OutputId>) -> LayerSurfaceConfig {
        LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output,
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
            size: (0, 0),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        }
    }

    fn config(&self, spec: &WindowSpec) -> LayerSurfaceConfig {
        let (width, height) = (
            spec.size.width.ceil() as u32,
            spec.size.height.ceil() as u32,
        );
        let centre = spec.title == CENTRE_TITLE;
        LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output: None,
            layer: Layer::Overlay,
            anchor: if centre {
                Anchor::Top | Anchor::Right | Anchor::Bottom
            } else {
                Anchor::Top | Anchor::Right
            },
            size: (width, if centre { 0 } else { height }),
            margin: Margin {
                top: if centre {
                    0
                } else {
                    (self.layout.top_inset - SHADOW_MARGIN).max(0.0) as i32
                },
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard: if centre {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            },
        }
    }
}

impl SurfaceRoles for NotificationRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let config = match view::catcher_output(&spec.app_id) {
            Some(wanted) => Self::catcher_config(
                catcher_surface_output(wanted, &output_numbers(conn))
                    .map(OutputId::from_registry_name),
            ),
            None if spec.app_id == view::APP_ID => self.config(spec),
            None => return None,
        };
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => {
                return Some(Err(WaylandError::Protocol(
                    "the notification surface's output is gone",
                )))
            }
        };
        let surface = layer.surface();
        self.layers.insert(surface, layer);
        if spec.app_id == view::APP_ID && spec.title == CENTRE_TITLE {
            self.centre = Some(surface);
            // a new surface asks for the keyboard
            self.keys_released = false;
            self.keyboard.asked();
        }
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        // the toolkit follows the appearance and the lock itself; the model needs the lock and reduce motion too
        match event.downcast_ref::<AppEvent>() {
            Some(&AppEvent::SessionLock { locked }) => {
                let _ = self.notes.try_send(Note::Lock(locked));
                return None;
            }
            Some(AppEvent::DisplayOptions(options)) => {
                let _ = self
                    .notes
                    .try_send(Note::ReduceMotion(options.reduce_motion));
                return None;
            }
            _ => {}
        }
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = self.layers.get(&surface)?;
                Some(match layer.configured(size) {
                    Ok(size) => RoleEvent::Configure { surface, size },
                    Err(_) => RoleEvent::Closed(surface),
                })
            }
            ShellEvent::Closed(surface) if self.layers.contains_key(&surface) => {
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        self.layers.remove(&surface);
        if self.centre == Some(surface) {
            self.centre = None;
            self.keys_released = false;
        }
    }

    /// Another surface took the keyboard from the open list: the list closes, as it does when another shell panel
    /// opens.
    fn keyboard_focus(&mut self, surface: SurfaceId, focused: bool) {
        if self.centre != Some(surface) {
            return;
        }
        let opening = self.opening.load(Ordering::Relaxed);
        if self.keyboard.focus(focused, opening) {
            // the list lets the keyboard go with its next commit, whatever it draws until it hears of the close
            if let Some(layer) = self.layers.get(&surface) {
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            }
            self.keys_released = true;
            self.keyboard.asked();
            let _ = self.notes.try_send(Note::Unfocused(opening));
        }
    }

    /// Tell the bus whether the centre's list is open and on which output, when that changed, and the application
    /// which outputs there are.
    fn prepare(&mut self, conn: &mut Connection) {
        let outputs = output_numbers(conn);
        if self.outputs.as_ref() != Some(&outputs) {
            let _ = self.notes.try_send(Note::Outputs(outputs.clone()));
            self.outputs = Some(outputs);
        }
        // a list on its way out lets the keyboard go to the window that had it; the next commit applies it
        if let Some(layer) = self.centre.and_then(|surface| self.layers.get(&surface)) {
            let wanted = self.keyboard.wants(self.opening.load(Ordering::Relaxed));
            if wanted == self.keys_released {
                layer.set_keyboard_interactivity(if wanted {
                    KeyboardInteractivity::Exclusive
                } else {
                    KeyboardInteractivity::None
                });
                self.keys_released = !wanted;
                self.keyboard.asked();
            }
        }
        let output = self.centre.and_then(|surface| {
            let on = conn.surface_outputs(surface).ok()?.into_iter().next()?;
            conn.outputs().into_iter().find(|o| o.id == on)?.name
        });
        let presence = centre_presence(self.centre.is_some(), output);
        if let Err(e) = self.publisher.publish(presence) {
            eprintln!("cupertino-notifications: cannot announce whether the centre is open: {e}");
        }
    }
}

/// A model action that waits for an activation token.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Activate(u32),
    Invoke(u32, String),
}

pub struct Notifications {
    server: Server,
    ui: Ui,
    notes: async_channel::Receiver<Note>,
    sender: async_channel::Sender<Note>,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    /// Measures the cards with the windows' text system; set once the runtime has its options.
    metrics: Option<CardMetrics>,
    /// Tells the roles which opening of the list wants the keyboard (it is open, not closing), 0 for none.
    opening: Arc<AtomicU64>,
    /// How many times the list was opened; the number of the opening it is at while it is open.
    openings: u64,
    /// The list was open when the roles were last told.
    was_open: bool,
    /// The compositor's outputs by their numbers: each has a window that hears presses outside the open list.
    outputs: Vec<u32>,
    /// The compositor said it has no output. There is no window then, as it closes at once every surface it is given;
    /// the notifications are taken and kept as ever, and the windows come back with an output.
    no_output: bool,
    /// The generation of the banners' and the list's windows: each one is a new window, so one whose surface the
    /// compositor closed (its output went away) is not kept.
    surfaces: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    Note(Note),
    Timer(u64),
    Token(Pending, Option<String>),
    /// Focus the node with this key, now that the view shows it.
    Focus(Key),
    /// The bus thread and the roles are gone.
    Unheard,
    /// The compositor closed the surface of a banner or of the list: its output went away.
    SurfaceClosed,
}

/// The key of a banner's or the list's window in the generation `generation` of their surfaces: the view's own key
/// in the first, and another in each later one.
fn in_generation(key: WindowKey, generation: u64) -> WindowKey {
    match key.0 {
        Key::Str(name) if generation > 0 => WindowKey(Key::from(format!("{name}#{generation}"))),
        key => WindowKey(key),
    }
}

fn listen(notes: &async_channel::Receiver<Note>) -> Command<Message> {
    let notes = notes.clone();
    Command::Perform(Box::pin(async move {
        notes.recv().await.map_or(Message::Unheard, Message::Note)
    }))
}

/// Open a link of a notification body with the default handler.
fn open_link(href: &str) {
    match std::process::Command::new("xdg-open").arg(href).spawn() {
        Ok(mut child) => {
            // reap the child without blocking the loop
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-notifications: xdg-open: {e}");
                }
            });
        }
        Err(e) => eprintln!("cupertino-notifications: cannot run xdg-open: {e}"),
    }
}

impl Notifications {
    /// Expire and animate the model, send its signals, and set the timer for its next change.
    fn housekeeping(&mut self) -> Vec<Command<Message>> {
        let now = self.server.now();
        let wakeup = {
            let mut c = self.server.center();
            // cleared cards that have left go from the model now
            self.ui.finish_removals(&mut c, now);
            c.tick(now);
            c.next_wakeup(now)
        };
        // the centre sliding, a group expanding or collapsing, a card leaving or the cards rising need every frame
        let wakeup = if self.ui.is_animating(now) {
            Some(Wakeup::Frame)
        } else {
            wakeup
        };
        if let Err(e) = self.server.flush_events() {
            eprintln!("cupertino-notifications: cannot send the signals: {e}");
        }
        if self.ui.centre_open && !self.was_open {
            self.openings += 1;
        }
        self.was_open = self.ui.centre_open;
        let opening = if self.ui.centre_open {
            self.openings
        } else {
            0
        };
        self.opening.store(opening, Ordering::Relaxed);
        self.timer += 1;
        let wait = match wakeup {
            Some(Wakeup::Frame) => FRAME,
            Some(Wakeup::At(t)) => Duration::from_secs_f64((t - now).max(0.0)),
            None => return Vec::new(),
        };
        let timer = self.timer;
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(wait).await;
            Message::Timer(timer)
        }))]
    }

    /// Ask for the activation token the action hands to the application; `None` when the notification is gone.
    fn token(&self, pending: Pending) -> Option<Command<Message>> {
        let id = match pending {
            Pending::Activate(id) | Pending::Invoke(id, _) => id,
        };
        let c = self.server.center();
        let n = c.shown(c.get(id)?).into_owned();
        let window = if self.ui.centre_shown(self.server.now()) {
            centre_key()
        } else {
            let width = c.config.layout.width;
            banner_key(
                &n,
                self.metrics.as_ref()?.card(&n, width, Fold::Banner).height,
            )
        };
        let window = in_generation(window, self.surfaces);
        Some(Command::ActivationToken {
            window,
            app_id: n.desktop_entry.clone(),
            on_token: Box::new(move |token| Message::Token(pending, token)),
        })
    }

    fn input(&mut self, input: Input) -> Vec<Command<Message>> {
        let now = self.server.now();
        let action = view::apply(&mut self.server.center(), &mut self.ui, input, now);
        match action {
            Some(HostAction::Activate(id)) => {
                self.token(Pending::Activate(id)).into_iter().collect()
            }
            Some(HostAction::Invoke(id, key)) => {
                self.token(Pending::Invoke(id, key)).into_iter().collect()
            }
            Some(HostAction::OpenLink(href)) => {
                open_link(&href);
                Vec::new()
            }
            // the card may be one the view builds only after this update
            Some(HostAction::Focus(key)) => {
                vec![Command::Perform(Box::pin(
                    async move { Message::Focus(key) },
                ))]
            }
            Some(HostAction::OutputGone(output)) => {
                self.outputs.retain(|&n| n != output);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// Run the action that waited for the token; the signals carry it to the application.
    fn complete(&mut self, pending: Pending, token: Option<String>) {
        let now = self.server.now();
        let mut c = self.server.center();
        // the notification may have expired while the token was fetched: nothing to run then
        match pending {
            Pending::Activate(id) => {
                c.activate(id, token, now);
                // a click on a card of the centre closes it, now that its window gave the token
                self.ui.close_centre(now, c.config.motion.reduce_motion);
            }
            Pending::Invoke(id, key) => {
                c.invoke_action(id, &key, token, now);
            }
        }
    }
}

/// `notifications.conf` as it is now: its lines that are not understood are reported, and keep the value of `last`.
/// A file that cannot be read keeps `last` (the defaults at start).
fn load_policy(last: Option<&NotificationSettings>) -> NotificationSettings {
    let kept = || last.cloned().unwrap_or_default();
    let Some(path) = NotificationSettings::path() else {
        return kept();
    };
    match NotificationSettings::load(&path, last) {
        Ok((settings, problems)) => {
            for p in problems {
                eprintln!(
                    "cupertino-notifications: {} line {}: {}",
                    path.display(),
                    p.line,
                    p.message
                );
            }
            settings
        }
        Err(e) => {
            eprintln!(
                "cupertino-notifications: cannot read {}: {e}",
                path.display()
            );
            kept()
        }
    }
}

/// The task of the loop that watches `notifications.conf`; each change is a [`Note::Policy`]. `None` when it cannot,
/// and the file then applies at the next start.
fn watch_policy(notes: async_channel::Sender<Note>) -> Option<Command<Message>> {
    let path = NotificationSettings::path()?;
    match ConfigWatcher::new(vec![path.clone()]) {
        Ok(watch) => Some(Command::Perform(Box::pin(
            cupertino_kit::task::follow_settings(watch, move |_| {
                let _ = notes.try_send(Note::Policy);
            }),
        ))),
        Err(e) => {
            eprintln!(
                "cupertino-notifications: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            None
        }
    }
}

fn start(options: &Options) -> Result<(Notifications, Vec<Command<Message>>)> {
    let mut config =
        CenterConfig::below_menu_bar(cupertino_assets::theme::load()?.shapes.menu_bar_height);
    // the toolkit's first frame has the session's options; the roles report changes
    config.motion.reduce_motion = false;
    let (sender, notes) = async_channel::unbounded();
    let wake = sender.clone();
    let server = Server::start(ServerOptions {
        config,
        wake: Some(Box::new(move || {
            let _ = wake.try_send(Note::Changed);
        })),
        replace_existing: options.replace_existing,
        ..ServerOptions::default()
    })
    .map_err(start_error)?;
    server.center().set_policy(load_policy(None));
    let policy_watch = watch_policy(sender.clone());
    let toggle = sender.clone();
    server.serve_centre_control(move || {
        let _ = toggle.try_send(Note::Toggle);
    })?;
    let first = listen(&notes);
    Ok((
        Notifications {
            server,
            ui: Ui::default(),
            notes,
            sender,
            timer: 0,
            metrics: None,
            opening: Arc::new(AtomicU64::new(0)),
            openings: 0,
            was_open: false,
            outputs: Vec::new(),
            no_output: false,
            surfaces: 0,
        },
        std::iter::once(first).chain(policy_watch).collect(),
    ))
}

impl Application for Notifications {
    type Model = Notifications;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Banners are small and shown now and then: a GPU context would only cost memory.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

    /// The process's daemon on the session bus; [`run`] starts it with the command line's options.
    fn init() -> (Notifications, Vec<Command<Message>>) {
        let options = cupertino_kit::app::options()
            .unwrap_or_else(|e| panic!("cupertino-notifications: {e}"));
        let (mut model, commands) = start(&Options {
            replace_existing: false,
        })
        .unwrap_or_else(|e| panic!("cupertino-notifications: {e}"));
        model.metrics = Some(metrics(&options));
        (model, commands)
    }

    fn update(model: &mut Notifications, message: Message) -> Vec<Command<Message>> {
        let mut commands = match message {
            Message::Input(input) => model.input(input),
            Message::Note(note) => {
                match note {
                    Note::Changed => {}
                    Note::Toggle => {
                        let now = model.server.now();
                        let reduce = model.server.center().config.motion.reduce_motion;
                        if model.ui.centre_open {
                            model.ui.close_centre(now, reduce);
                        } else {
                            // a centre sliding out turns round from where it is
                            model.ui.open_centre(now, reduce);
                        }
                    }
                    Note::Lock(locked) => {
                        let now = model.server.now();
                        model.server.center().set_screen_locked(locked, now);
                        if locked {
                            model.ui = Ui::default();
                        }
                    }
                    Note::ReduceMotion(on) => {
                        model.server.center().config.motion.reduce_motion = on
                    }
                    Note::Policy => {
                        let last = model.server.center().policy().clone();
                        let policy = load_policy(Some(&last));
                        model.server.center().set_policy(policy);
                    }
                    // a list opened again since is another opening, which has the keyboard
                    Note::Unfocused(opening) => {
                        if model.was_open && opening == model.openings {
                            let now = model.server.now();
                            let reduce = model.server.center().config.motion.reduce_motion;
                            model.ui.close_centre(now, reduce);
                        }
                    }
                    Note::Outputs(outputs) => {
                        model.no_output = outputs.is_empty();
                        model.outputs = outputs;
                    }
                }
                vec![listen(&model.notes)]
            }
            Message::Timer(timer) if timer != model.timer => return Vec::new(),
            Message::Timer(_) => Vec::new(),
            Message::Token(pending, token) => {
                model.complete(pending, token);
                Vec::new()
            }
            Message::Focus(key) => vec![Command::Focus(key)],
            Message::Unheard => {
                eprintln!("cupertino-notifications: the bus thread stopped");
                return vec![Command::Exit];
            }
            Message::SurfaceClosed => {
                // new windows, on an output the compositor still has; while it has none there is no window
                model.surfaces += 1;
                Vec::new()
            }
        };
        commands.extend(model.housekeeping());
        commands
    }

    fn view(model: &Notifications) -> Vec<Window<Message>> {
        let Some(metrics) = &model.metrics else {
            return Vec::new();
        };
        if model.no_output {
            return Vec::new();
        }
        let now = model.server.now();
        let mut center = model.server.center();
        // the windows that hear a press outside the open list come first: they lie under it
        let mut windows = view::catchers(&center, &model.ui, &model.outputs, Message::Input);
        windows.extend(
            view::windows(&mut center, &model.ui, metrics, now, Message::Input)
                .into_iter()
                .map(|mut window| {
                    window.key = in_generation(window.key, model.surfaces);
                    // a surface the compositor closed is made again; without this the close would end the process
                    window.on_close = Some(Message::SurfaceClosed);
                    window
                }),
        );
        windows
    }
}

/// The cards' metrics in the type and with the text system the runtime draws with.
fn metrics(options: &RuntimeOptions) -> CardMetrics {
    CardMetrics::new(Arc::clone(&options.theme), Arc::clone(&options.text))
}

/// Connect, claim the bus names and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let start = start(options)?;
    let mut system = WaylandWindowSystem::connect_as(Profile::cupertino(), Notifications::RENDERER)
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let layout = start.0.server.center().config.layout.clone();
    // the roles send on the bus thread's channel
    let sender = start.0.sender.clone();
    let publisher = start.0.server.centre_publisher();
    let system = system.with_roles(NotificationRoles {
        shell,
        layout,
        layers: HashMap::new(),
        notes: sender,
        centre: None,
        publisher,
        opening: Arc::clone(&start.0.opening),
        keys_released: false,
        keyboard: ListKeyboard::default(),
        outputs: None,
    });
    cupertino_kit::app::run_on_with::<Notifications>(system, move |options| {
        let (mut model, commands) = start;
        model.metrics = Some(metrics(options));
        (model, commands)
    })?;
    Err("the notification surfaces were closed".into())
}
