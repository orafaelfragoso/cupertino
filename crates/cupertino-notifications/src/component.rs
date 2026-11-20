//! The banners and the notification list as a component of the shell host (`cupertino-shell`): their windows on
//! layer surfaces, `org.freedesktop.Notifications` and `org.cupertino.NotificationCenter1` on the component's own
//! session bus connection ([`crate::control`]), the list kept in a file across a restart ([`crate::store`]), and the
//! host bus: the menu bar's toggle and do not disturb come in; whether the list is open and do not disturb go out, and
//! a component the server gave up is told to the person.
//!
//! Nothing here waits on the UI thread: the bus calls are answered from the model; the callers are named, the files
//! read and written and the desktop entries looked up on the I/O pool; the banners' lifetimes and motions are paced
//! by the UI loop's timers.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_kit::core::{Application, Command as Do, Key, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit_shell::bus::{self, Message as Published, Panel, Topic};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    session, Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program,
    Role, Surface,
};
use cupertino_peer::Caller;

use crate::center::{CenterConfig, LockItem, NotificationCenter, Wakeup};
use crate::control::{self, Call, ServeError, Served, Shown, Signal};
use crate::notification::{Expire, Hints, NotifyInput};
use crate::store::Restored;
use crate::view::{
    self, banner_key, centre_key, centre_root_key, CardMetrics, Fold, HostAction, Input, Ui,
    CENTRE_TITLE, SHADOW_MARGIN,
};

mod calls;
mod kept;

pub use calls::{Pending, Prepared};

/// The layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-notifications";
/// How often the model steps while something moves: a frame at 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);

/// The banners and the notification list.
pub struct Notifications;

/// A value a message hands to `update` once: messages are cloned, and what this one carries cannot be.
pub struct Handed<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handed<T> {
    fn clone(&self) -> Self {
        Handed(Arc::clone(&self.0))
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

#[derive(Clone)]
pub enum Message {
    Input(Input),
    /// A call of the bus interfaces.
    Call(Call),
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    Served(Served),
    /// The list an earlier run kept, if any, and the menu bar's height.
    Restored(Option<Restored>, f64),
    /// A `Notify` or `CloseNotification` call, its caller named.
    Prepared(Prepared),
    /// A caller was named for the call that waited for it.
    Judged,
    /// `notifications.conf` as it is now, and what watches it from now on when it was just set up.
    Policy(
        cupertino_defaults::notifications::NotificationSettings,
        Option<Handed<cupertino_defaults::watch::ConfigWatcher>>,
    ),
    /// `notifications.conf` changed.
    PolicyChanged,
    /// The list was written, or why not.
    Saved(Result<(), String>),
    Timer(u64),
    Token(Pending, Option<String>),
    /// Move the focus to this node, now that the view shows it: with its ring (`true`), or as a click moves it.
    Focus(Key, bool),
    Locked(bool),
    /// The session's Reduce Motion is on (`true`) or off, as at start and on every change.
    ReduceMotion(bool),
    Outputs,
    /// The server closed a banner's or the list's surface: its output went away.
    SurfaceClosed,
    Bus(Published),
    /// The signals stopped: the connection is gone.
    SignalsEnded,
}

/// What the bus and the host bus were last told.
#[derive(Default)]
struct Told {
    shown: Shown,
    /// The lock screen's list, as `LockScreenChanged` last said it changed.
    lock: Vec<LockItem>,
    /// Do not disturb as last published on the host bus; `None` before the first.
    dnd: Option<bool>,
    /// The list's presence as last published on the host bus.
    panel: Option<(bool, Option<OutputId>)>,
}

/// Where the bus stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Names {
    /// Waiting for the connection and the list an earlier run kept.
    Waiting,
    /// The names are being claimed.
    Claiming,
    /// `org.freedesktop.Notifications` is served.
    Owned,
    /// Another program serves notifications; this component shows none of its own.
    Idle,
}

/// What a window is, for its layer surface.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    /// Hears a press outside the open list on the output numbered so (`None`: the one the server chooses).
    Catcher(Option<u32>),
    /// The list, this wide.
    Centre(f64),
    /// A banner, this big with its shadow's room.
    Banner(Size),
}

pub struct Model {
    center: RefCell<NotificationCenter>,
    ui: Ui,
    metrics: CardMetrics,
    /// The start of the model's clock.
    start: Instant,
    /// Seconds since the Unix epoch at the model's 0.
    offset: f64,
    /// Where the interfaces hand their calls, and where they come out.
    calls: (async_channel::Sender<Call>, async_channel::Receiver<Call>),
    /// Where the watch of `notifications.conf` says it changed.
    policy: (async_channel::Sender<()>, async_channel::Receiver<()>),
    /// The signals to send, in order, for the emitter on the connection.
    signals: (
        async_channel::Sender<Signal>,
        async_channel::Receiver<Signal>,
    ),
    /// What the interfaces' properties say.
    shown: Arc<Mutex<Shown>>,
    told: Told,
    connection: Option<zbus::Connection>,
    names: Names,
    /// The list an earlier run kept was read (or there was none).
    restored: bool,
    /// The `Notify` and `CloseNotification` calls not yet carried out, in the order they came.
    queue: VecDeque<calls::Queued>,
    /// One of them is being prepared.
    preparing: bool,
    /// The sandboxed senders of the notifications that came from one.
    origins: HashMap<u32, Arc<Caller>>,
    store: kept::Store,
    /// The timer that counts; one set before it is ignored when it fires.
    timer: u64,
    /// The server's outputs, each with the number its press catcher is keyed by.
    outputs: Vec<(OutputId, u32)>,
    next_output: u32,
    /// The output the open list was asked for; `None` leaves it to the server.
    centre_output: Option<OutputId>,
    /// The generation of the banners' and the list's windows: one whose surface the server closed is made anew.
    surfaces: u64,
    /// What each window of the last view is.
    places: RefCell<HashMap<WindowKey, Place>>,
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn center(&mut self) -> &mut NotificationCenter {
        self.center.get_mut()
    }

    fn reduce_motion(&self) -> bool {
        self.center.borrow().config.motion.reduce_motion
    }

    /// The server's outputs with a place on screen; a new one takes the next number.
    fn follow_outputs(&mut self) {
        let mut numbered = Vec::new();
        for output in outputs::all().into_iter().filter(|o| o.geometry.is_some()) {
            let number = match self.outputs.iter().find(|(id, _)| *id == output.id) {
                Some(&(_, number)) => number,
                None => {
                    self.next_output += 1;
                    self.next_output
                }
            };
            numbered.push((output.id, number));
        }
        self.outputs = numbered;
        if let Some(output) = self.centre_output {
            if !self.outputs.iter().any(|(id, _)| *id == output) {
                self.centre_output = None;
            }
        }
    }

    fn output_numbered(&self, number: u32) -> Option<OutputId> {
        self.outputs
            .iter()
            .find(|(_, n)| *n == number)
            .map(|(id, _)| *id)
    }

    /// Open the list on `output` (`None`: where the server puts it), or close it when it is open.
    fn toggle(&mut self, output: Option<OutputId>) -> Vec<Do<Message>> {
        let now = self.now();
        let reduce = self.reduce_motion();
        if self.ui.centre_open {
            self.ui.close_centre(now, reduce);
            return Vec::new();
        }
        self.centre_output = output;
        // a list sliding out turns round from where it is
        self.ui.open_centre(now, reduce);
        // the keyboard goes to the list once it shows, so that its losing the keyboard closes it
        vec![later(Message::Focus(centre_root_key(), false))]
    }

    /// Carry out what the person did on the windows.
    fn input(&mut self, input: Input) -> Vec<Do<Message>> {
        let now = self.now();
        let action = view::apply(self.center.get_mut(), &mut self.ui, input, now);
        match action {
            Some(HostAction::Activate(id)) => {
                self.token(Pending::Activate(id)).into_iter().collect()
            }
            Some(HostAction::Invoke(id, key)) => {
                self.token(Pending::Invoke(id, key)).into_iter().collect()
            }
            Some(HostAction::OpenLink(href)) => {
                let mut command = std::process::Command::new("xdg-open");
                command.arg(&href);
                if !cupertino_kit_shell::host::spawn(command) {
                    eprintln!("cupertino-notifications: cannot run xdg-open for a link");
                }
                Vec::new()
            }
            // the card may be one the view builds only after this update
            Some(HostAction::Focus(key)) => vec![later(Message::Focus(key, true))],
            Some(HostAction::OutputGone(number)) => {
                self.outputs.retain(|&(_, n)| n != number);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// Ask for the activation token the action hands to the application; none when the notification is gone.
    fn token(&self, pending: Pending) -> Option<Do<Message>> {
        let center = self.center.borrow();
        let n = center.shown(center.get(pending.id())?).into_owned();
        let window = if self.ui.centre_shown(self.now()) {
            centre_key()
        } else {
            let width = center.config.layout.width;
            banner_key(&n, self.metrics.card(&n, width, Fold::Banner).height)
        };
        Some(Do::ActivationToken {
            window: in_generation(window, self.surfaces),
            app_id: n.desktop_entry.clone(),
            on_token: Box::new(move |token| Message::Token(pending, token)),
        })
    }

    /// Run the action that waited for the token; the signals carry it to the application.
    fn complete(&mut self, pending: Pending, token: Option<String>) {
        let now = self.now();
        let reduce = self.reduce_motion();
        // the notification may have expired while the token was fetched: nothing to run then
        match pending {
            Pending::Activate(id) => {
                self.center().activate(id, token, now);
                // a click on a card of the list closes it, now that its window gave the token
                self.ui.close_centre(now, reduce);
            }
            Pending::Invoke(id, key) => {
                self.center().invoke_action(id, &key, token, now);
            }
        }
    }

    /// A publication of the host bus.
    fn published(&mut self, message: Published) -> Vec<Do<Message>> {
        match message {
            Published::TogglePanel {
                panel: Panel::NotificationCenter,
                output,
            } => self.toggle(output),
            Published::SetDoNotDisturb { on, lifetime } => {
                let change = match (on, lifetime) {
                    (false, _) => Some(control::DndChange::Off),
                    (true, None) => Some(control::DndChange::On),
                    (true, Some(id)) => {
                        let lifetime = crate::focus::Lifetime::from_id(&id);
                        if lifetime.is_none() {
                            eprintln!("cupertino-notifications: {id:?} is not an offered duration");
                        }
                        lifetime.map(control::DndChange::For)
                    }
                };
                if let Some(change) = change {
                    self.set_dnd(change);
                }
                Vec::new()
            }
            Published::ComponentStopped { component } => {
                let now = self.now();
                self.center().notify(stopped(component), now);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The session was locked (`true`) or unlocked: the server shows the lock screen and hides every other surface,
    /// so the open list goes, and no window is open until the unlock.
    fn lock(&mut self, locked: bool) {
        let now = self.now();
        self.center().set_screen_locked(locked, now);
        if locked {
            self.ui = Ui::default();
        }
    }

    /// The list an earlier run kept goes back in, the banners go below the menu bar, and the bus is served.
    fn restore(&mut self, restored: Option<Restored>, menu_bar_height: f64) -> Vec<Do<Message>> {
        let now = self.now();
        let center = self.center.get_mut();
        let reduce = center.config.motion.reduce_motion;
        center.config = CenterConfig::below_menu_bar(menu_bar_height);
        center.config.motion.reduce_motion = reduce;
        if let Some(restored) = restored {
            center.restore(restored.kept, restored.next_id);
            // a duration that ended while the component was away is over
            let dnd = match restored.dnd {
                crate::DndMode::Until(t) if t <= now => crate::DndMode::Off,
                dnd => dnd,
            };
            center.set_dnd(dnd, now);
        }
        self.restored = true;
        self.serve()
    }

    /// Claim the names once the connection is there and the earlier run's list is back, so no id is given twice.
    fn serve(&mut self) -> Vec<Do<Message>> {
        let (Names::Waiting, true, Some(connection)) =
            (self.names, self.restored, self.connection.clone())
        else {
            return Vec::new();
        };
        self.names = Names::Claiming;
        let calls = self.calls.0.clone();
        let shown = Arc::clone(&self.shown);
        let capabilities = self.center.borrow().config.capabilities.clone();
        vec![Do::Perform(Box::pin(async move {
            Message::Served(control::serve(&connection, calls, shown, capabilities).await)
        }))]
    }

    /// How claiming the names went: without `org.freedesktop.Notifications` this component shows nothing, and one
    /// whose bus failed is started again by the host.
    fn served(&mut self, served: Served) -> Vec<Do<Message>> {
        match served.notifications {
            Ok(()) => {}
            Err(ServeError::Taken(name)) => {
                eprintln!("cupertino-notifications: another program owns {name}; no notification is shown here");
                self.names = Names::Idle;
                return Vec::new();
            }
            Err(ServeError::Bus(e)) => {
                eprintln!(
                    "cupertino-notifications: {} is not served: {e}",
                    control::BUS_NAME
                );
                return vec![Do::Exit];
            }
        }
        if let Some(Err(e)) = served.centre {
            // the banners and the list work without it; the menu bar and the control panel reach the list over the
            // host bus
            eprintln!(
                "cupertino-notifications: {} is not served: {e}",
                control::CENTRE_BUS_NAME
            );
        }
        self.names = Names::Owned;
        let Some(connection) = self.connection.clone() else {
            return Vec::new();
        };
        let signals = self.signals.1.clone();
        vec![Do::Perform(Box::pin(async move {
            control::emit(connection, signals).await;
            Message::SignalsEnded
        }))]
    }

    /// Expire and move the model, say on the bus what changed, keep the list, and set the timer for the next change.
    fn housekeeping(&mut self) -> Vec<Do<Message>> {
        let now = self.now();
        let wakeup = {
            let center = self.center.get_mut();
            // cleared cards that have left go from the model now
            self.ui.finish_removals(center, now);
            center.tick(now);
            center.next_wakeup(now)
        };
        // the list sliding, a group expanding or collapsing, a card leaving or the cards rising need every frame
        let wakeup = if self.ui.is_animating(now) {
            Some(Wakeup::Frame)
        } else {
            wakeup
        };
        let told = self.tell(now);
        let mut commands: Vec<Do<Message>> = self
            .store
            .keep(&self.center.borrow(), self.offset, self.restored, told)
            .into_iter()
            .collect();
        self.timer += 1;
        let delay = match wakeup {
            Some(Wakeup::Frame) => FRAME,
            Some(Wakeup::At(t)) => Duration::from_secs_f64((t - now).max(0.0)),
            None => return commands,
        };
        commands.push(Do::After {
            delay,
            message: Message::Timer(self.timer),
        });
        commands
    }

    /// Send the model's events, and say on the bus and the host bus what changed of do not disturb, the lock screen's
    /// list and the list's presence. True when the model had events to send.
    fn tell(&mut self, now: f64) -> bool {
        let serving = self.names == Names::Owned;
        let (events, dnd, lock) = {
            let center = self.center.get_mut();
            (
                center.take_events(),
                center.dnd_active(now),
                center.lock_screen(),
            )
        };
        let had_events = !events.is_empty();
        let mut signals: Vec<Signal> = events.into_iter().map(Signal::Event).collect();
        let centre = self.ui.centre_shown(now) && !self.center.borrow().screen_locked();
        let output = self.centre_output.filter(|_| centre);
        let shown = Shown {
            dnd,
            centre,
            output: output
                .and_then(outputs::get)
                .and_then(|o| o.name)
                .unwrap_or_default(),
        };
        if shown.dnd != self.told.shown.dnd {
            signals.push(Signal::DoNotDisturb);
        }
        if shown.centre != self.told.shown.centre {
            signals.push(Signal::CentreShown);
        }
        if shown.output != self.told.shown.output {
            signals.push(Signal::CentreOutput);
        }
        if lock != self.told.lock {
            signals.push(Signal::LockScreen);
            self.told.lock = lock;
        }
        if self.told.shown != shown {
            *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = shown.clone();
            self.told.shown = shown;
        }
        if serving {
            for signal in signals {
                // the emitter holds the receiver for as long as the connection lives
                let _ = self.signals.0.try_send(signal);
            }
        }
        if self.told.dnd != Some(dnd) {
            self.told.dnd = Some(dnd);
            let lifetimes = control::unix_now()
                .map(control::lifetimes)
                .unwrap_or_default();
            bus::publish(Published::DoNotDisturbChanged { on: dnd, lifetimes });
        }
        let panel = (centre, output);
        if self.told.panel != Some(panel) {
            self.told.panel = Some(panel);
            bus::publish(Published::PanelShown {
                panel: Panel::NotificationCenter,
                shown: centre,
                output,
            });
        }
        had_events
    }
}

/// `message` on the next turn of the loop, after the view that follows this update.
fn later(message: Message) -> Do<Message> {
    Do::Perform(Box::pin(async move { message }))
}

/// The next message on `channel`, as `message` makes it.
fn listen<T: Send + 'static>(
    channel: &async_channel::Receiver<T>,
    message: fn(T) -> Message,
) -> Do<Message> {
    let channel = channel.clone();
    Do::Perform(Box::pin(async move {
        match channel.recv().await {
            Ok(value) => message(value),
            // the model holds a sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The key of a banner's or the list's window in the generation `generation` of their surfaces: the view's own key
/// in the first, and another in each later one.
fn in_generation(key: WindowKey, generation: u64) -> WindowKey {
    match key.0 {
        Key::Str(name) if generation > 0 => WindowKey(Key::from(format!("{name}#{generation}"))),
        key => WindowKey(key),
    }
}

/// What the person is told when the server gave `component` up: it failed too often to be started again.
fn stopped(component: ComponentName) -> NotifyInput {
    let name = match component {
        ComponentName::MENUBAR => "The menu bar",
        ComponentName::DOCK => "The Dock",
        ComponentName::DESKTOP => "The desktop",
        ComponentName::WINDOWMANAGER => "The window manager",
        ComponentName::OVERVIEW => "The overview",
        ComponentName::LAUNCHER => "The launcher",
        ComponentName::CONTROLCENTER => "The control panel",
        ComponentName::OSD => "The on-screen displays",
        ComponentName::AGENT => "The session agent",
        ComponentName::DIALOGS => "The file and application choosers",
        _ => "A part of the desktop",
    };
    NotifyInput {
        app_name: "Cupertino".into(),
        replaces_id: 0,
        icon: String::new(),
        summary: format!("{name} stopped"),
        body: format!(
            "It failed too often to start again. \u{201c}cupertino shell restart {}\u{201d} starts it, as the next \
             login does.",
            component.as_str()
        ),
        actions: Vec::new(),
        expire: Expire::Default,
        hints: Hints::default(),
    }
}

/// What the window is, from what the view made of it.
fn place_of(window: &Window<Message>) -> Place {
    let spec = &window.spec;
    match view::catcher_output(&spec.app_id) {
        Some(number) => Place::Catcher(number),
        None if spec.title == CENTRE_TITLE => Place::Centre(spec.size.width),
        None => Place::Banner(spec.size),
    }
}

impl Application for Notifications {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Banners are small and shown now and then: a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let environment = cupertino_kit_shell::environment();
        let mut config = CenterConfig::default();
        config.motion.reduce_motion = environment.theme.preferences.reduce_motion;
        let mut model = Model {
            center: RefCell::new(NotificationCenter::new(config)),
            ui: Ui::default(),
            metrics: CardMetrics::new(environment.theme, environment.text),
            start: Instant::now(),
            offset: control::unix_now().map_or(0.0, |unix| unix as f64),
            calls: async_channel::unbounded(),
            policy: async_channel::unbounded(),
            signals: async_channel::unbounded(),
            shown: Arc::default(),
            told: Told::default(),
            connection: None,
            names: Names::Waiting,
            restored: false,
            queue: VecDeque::new(),
            preparing: false,
            origins: HashMap::new(),
            store: kept::Store::new(),
            timer: 0,
            outputs: Vec::new(),
            next_output: 0,
            centre_output: None,
            surfaces: 0,
            places: RefCell::default(),
        };
        model.follow_outputs();
        model.lock(session::locked());
        bus::follow(&[Topic::Host, Topic::Panels, Topic::DoNotDisturb]);
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(model.store.restore(model.offset));
        commands.push(calls::first_policy());
        commands.push(listen(&model.calls.1, Message::Call));
        commands.push(listen(&model.policy.1, |()| Message::PolicyChanged));
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        let mut commands = match message {
            Message::Input(input) => model.input(input),
            Message::Call(call) => {
                let mut commands = model.call(call);
                commands.push(listen(&model.calls.1, Message::Call));
                commands
            }
            Message::Session(Some(connection)) => {
                model.connection = Some(connection);
                model.serve()
            }
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the component serves nobody: the host starts it again
                eprintln!("cupertino-notifications: no session bus connection");
                return vec![Do::Exit];
            }
            Message::Served(served) => model.served(served),
            Message::Restored(restored, menu_bar_height) => {
                model.restore(restored, menu_bar_height)
            }
            Message::Prepared(prepared) => model.prepared(prepared),
            Message::Judged => Vec::new(),
            Message::Policy(settings, watch) => model.policy(settings, watch),
            Message::PolicyChanged => {
                vec![
                    model.reload_policy(),
                    listen(&model.policy.1, |()| Message::PolicyChanged),
                ]
            }
            Message::Saved(result) => {
                model.store.saved(result);
                Vec::new()
            }
            Message::Timer(timer) if timer != model.timer => return Vec::new(),
            Message::Timer(_) => Vec::new(),
            Message::Token(pending, token) => {
                model.complete(pending, token);
                Vec::new()
            }
            Message::Focus(key, true) => vec![Do::Focus(key)],
            Message::Focus(key, false) => vec![Do::MoveFocus(key)],
            Message::Locked(locked) => {
                model.lock(locked);
                Vec::new()
            }
            Message::ReduceMotion(reduce) => {
                model.center().config.motion.reduce_motion = reduce;
                Vec::new()
            }
            Message::Outputs => {
                model.follow_outputs();
                Vec::new()
            }
            Message::SurfaceClosed => {
                // new windows, on an output the server still has; while it has none there is no window
                model.surfaces += 1;
                Vec::new()
            }
            Message::Bus(message) => model.published(message),
            // the session connection's end is heard as `SessionClosed`
            Message::SignalsEnded => Vec::new(),
        };
        commands.extend(model.housekeeping());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut places = model.places.borrow_mut();
        places.clear();
        // the server closes at once every surface it is given while it has no output
        if model.outputs.is_empty() {
            return Vec::new();
        }
        let now = model.now();
        let mut center = model.center.borrow_mut();
        let numbers: Vec<u32> = model.outputs.iter().map(|&(_, n)| n).collect();
        // the windows that hear a press outside the open list come first: they lie under it
        let mut windows = view::catchers(&center, &model.ui, &numbers, Message::Input);
        windows.extend(
            view::windows(&mut center, &model.ui, &model.metrics, now, Message::Input)
                .into_iter()
                .map(|mut window| {
                    window.key = in_generation(window.key, model.surfaces);
                    // a surface the server closed is made again
                    window.on_close = Some(Message::SurfaceClosed);
                    window
                }),
        );
        for window in &windows {
            places.insert(window.key.clone(), place_of(window));
        }
        windows
    }
}

impl Program for Notifications {
    /// On the overlay layer at the right edge: a banner at the top below the menu bar, as big as its window; the list
    /// from top to bottom, on the output it was asked for, with the keyboard while it is open. While the list is open
    /// each output has a window on the top layer over all of it, under the list, which hears the press that closes it.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let place = model
            .places
            .borrow()
            .get(key)
            .copied()
            .unwrap_or(Place::Banner(Size::new(0.0, 0.0)));
        let spec = LayerSpec {
            namespace: NAMESPACE,
            output: None,
            layer: Layer::Overlay,
            anchor: Anchor {
                top: true,
                right: true,
                ..Anchor::default()
            },
            size: Size::new(0.0, 0.0),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: Keyboard::None,
        };
        let spec = match place {
            Place::Catcher(number) => LayerSpec {
                output: number.and_then(|n| model.output_numbered(n)),
                layer: Layer::Top,
                anchor: Anchor {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true,
                },
                ..spec
            },
            Place::Centre(width) => LayerSpec {
                output: model.centre_output,
                anchor: Anchor {
                    top: true,
                    right: true,
                    bottom: true,
                    left: false,
                },
                size: Size::new(width.ceil(), 0.0),
                keyboard: if model.ui.centre_open {
                    Keyboard::Exclusive
                } else {
                    Keyboard::None
                },
                ..spec
            },
            Place::Banner(size) => {
                let top_inset = model.center.borrow().config.layout.top_inset;
                LayerSpec {
                    size: Size::new(size.width.ceil(), size.height.ceil()),
                    margin: Margin {
                        top: (top_inset - SHADOW_MARGIN).max(0.0) as i32,
                        ..Margin::default()
                    },
                    ..spec
                }
            }
        };
        Surface {
            role: Role::Layer(spec),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Session(session::Change::Locked(locked)) => Some(Message::Locked(locked)),
            // the motions follow the session's Reduce Motion
            Event::Appearance(appearance) => Some(Message::ReduceMotion(appearance.reduce_motion)),
            Event::Outputs => Some(Message::Outputs),
            Event::Bus { message, .. } => Some(Message::Bus(message)),
            // a closed window's `on_close` makes it anew
            _ => None,
        }
    }
}

impl Component for Notifications {
    const NAME: ComponentName = ComponentName::NOTIFICATIONS;
}
