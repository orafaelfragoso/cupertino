//! The session agent as a component of the shell host: its connections, what the server asks of it
//! (`org.cupertino.SessionAgent1` and the session's log-out request), the panels on overlay layer surfaces of the
//! server's first output, the power key's hold and the countdown on the loop's clock, the session menu it publishes and
//! the work at login. Every bus call is awaited on the loop and every file is read on Kit's I/O pool.

mod actions;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_apps::exec::CommandSpec;
use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::core::{
    Application, Command as Do, Key as WindowName, RendererClass, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::text::TextSystem;
use cupertino_kit_shell::bus::{self, Message as Published};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::session::Change;
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role,
    Surface,
};
use zbus::zvariant::OwnedFd;

use crate::capabilities::{Capabilities, Inputs};
use crate::control::{self, Request};
use crate::interrupt::{Reason, Stage};
use crate::low_battery::LowBattery;
use crate::panel::{Action, Frames, Kind, Panel, Placement};
use crate::power_button::{Effect, PowerButton};
use crate::session::Interrupt;
use crate::startup::{self, Planned};
use crate::system::{self, Battery};
use crate::view::{self, Input, InterruptView};
use crate::{autostart, restore};

/// The layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-shell-agent";
/// `PowerButtonSleepsSystem`: whether a release of the power key between the debounce and the hold time sleeps the
/// system. Off, the design's default; `cupertino-login` took it as an option no configuration set.
const POWER_BUTTON_SLEEPS_SYSTEM: bool = false;
/// How long a file task at login or at the end of the session should take before it is reported overdue.
const FILE_DEADLINE: Duration = Duration::from_secs(2);

/// The session agent.
pub struct Agent;

/// What the bus streams the component follows say.
#[derive(Clone, Debug)]
pub enum Heard {
    /// The server asked for it on `org.cupertino.SessionAgent1`.
    Request(Request),
    /// `org.cupertino.Session1`'s word on the applications it quits.
    Interrupt(Interrupt),
    Battery(Battery),
    /// The session manager's name changed owner: what the menu may offer is read again.
    ManagerChanged,
    /// A followed stream ended, and why.
    Ended(&'static str, String),
}

/// A confirmation panel on screen.
struct PanelState {
    panel: Panel,
    /// Where its parts sit; its size is the window's.
    frames: Frames,
    placement: Placement,
    keyboard: Keyboard,
    /// Its window's number: a new panel is a new window.
    number: u64,
}

/// A confirmed log out, restart or shut down that `org.cupertino.Session1` carries out, quitting the applications first.
struct Quitting {
    /// What follows once no application is left.
    then: Action,
    /// The interrupt panel on screen.
    interrupt: Option<Interrupted>,
}

/// The interrupt panel on screen: the application it asks about, why it is still there, and which question it asks.
struct Interrupted {
    app: String,
    reason: Reason,
    stage: Stage,
    placement: Placement,
    keyboard: Keyboard,
    number: u64,
}

/// How long the component's end waits for its bus connections to close and their tasks to end: a bus that answers
/// closes them at once, and one that does not is not waited for longer.
const CLOSE_WAIT: Duration = Duration::from_millis(500);

impl Drop for Model {
    /// The connections' executors are ticked by the component's loop, which has ended: a task still on one (its
    /// reader, the object server of `org.cupertino.SessionAgent1`) would keep its connection, the connection its
    /// executor and so the task, for as long as the host runs. So the connections are closed and their executors run
    /// until every task has ended, at most [`CLOSE_WAIT`].
    fn drop(&mut self) {
        let connections: Vec<zbus::Connection> = [self.session.take(), self.system.take()]
            .into_iter()
            .flatten()
            .collect();
        if connections.is_empty() {
            return;
        }
        let closing = async {
            for connection in &connections {
                let _ = connection.clone().close().await;
            }
            for connection in &connections {
                let executor = connection.executor().clone();
                while !executor.is_empty() {
                    executor.tick().await;
                }
            }
        };
        let waited = async_io::Timer::after(CLOSE_WAIT);
        close(futures_util::future::select(
            std::pin::pin!(closing),
            std::pin::pin!(waited),
        ));
    }
}

/// Wait for `closing` on the component's thread, whose loop has ended.
#[expect(
    clippy::disallowed_methods,
    reason = "the component's loop has ended: nothing on this thread waits to be drawn, and the wait is bounded"
)]
fn close(closing: impl std::future::Future) {
    async_io::block_on(closing);
}

/// What the component holds.
pub struct Model {
    start: Instant,
    session: Option<zbus::Connection>,
    system: Option<zbus::Connection>,
    /// Where the followed streams and `org.cupertino.SessionAgent1` hand what they heard, and where it comes out.
    heard: (async_channel::Sender<Heard>, async_channel::Receiver<Heard>),
    /// logind's ID of the session, once known; login items and restore wait for it.
    session_id: Option<String>,
    /// The file of the "Open my windows again" choice and list ([`restore`]); none without a state or home directory.
    restore_path: Option<PathBuf>,
    /// The choice as last read or stored.
    reopen: bool,
    user_name: Option<String>,
    /// What the menu rests on, as last read, and whether a reading is under way or asked for again.
    inputs: Option<Inputs>,
    reading: bool,
    read_again: bool,
    locked: bool,
    /// The server's first output and its size, which the panels show on.
    output: Option<(OutputId, Size)>,
    text: Arc<TextSystem>,
    panel: Option<PanelState>,
    windows: u64,
    power_button: PowerButton,
    /// Held for the component's life: logind does not power off on the power key, the agent decides what it does.
    power_key: Option<Arc<OwnedFd>>,
    /// A confirmed end of the session is under way: the power-button panel does not show over it.
    logging_out: bool,
    quitting: Option<Quitting>,
    low_battery: LowBattery,
    /// The wake-up asked for, by its number and time on the model's clock.
    wake: (u64, Option<f64>),
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// Whether `org.cupertino.SessionAgent1` is served.
    Served(Result<(), String>),
    System(Option<zbus::Connection>),
    SystemClosed,
    SessionId(Option<String>),
    Heard(Heard),
    Inputs(Inputs),
    UserName(String),
    ReopenChoice(bool),
    PowerKeyBlocked(Result<Arc<OwnedFd>, String>),
    LoginItems(Planned),
    Reopen(Vec<(String, CommandSpec)>),
    Locked(bool),
    LogOutRequested,
    Panel(Input),
    Interrupt(Input),
    /// The wake-up of this number came.
    Wake(u64),
    Outputs,
    Closed(WindowKey),
    /// What a call started by an action came back with.
    Done(actions::Done),
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Publish the session menu's state once what it rests on is known.
    fn publish(&self) {
        let (Some(inputs), Some(user_name)) = (&self.inputs, &self.user_name) else {
            return;
        };
        let caps = Capabilities::new(inputs);
        bus::publish(Published::SessionMenu {
            user_name: user_name.clone(),
            lock: caps.lock,
            log_out: caps.log_out,
            switch_user: caps.switch_user,
            sleep: caps.sleep,
            restart: caps.restart,
            shut_down: caps.shut_down,
        });
    }

    /// Read what the menu may offer, once at a time; one asked for during a reading follows it.
    fn read_inputs(&mut self) -> Vec<Do<Message>> {
        let Some(system) = self.system.clone() else {
            return Vec::new();
        };
        if self.reading {
            self.read_again = true;
            return Vec::new();
        }
        self.reading = true;
        vec![Do::Perform(Box::pin(async move {
            Message::Inputs(system::inputs(&system).await)
        }))]
    }

    fn inputs_read(&mut self, inputs: Inputs) -> Vec<Do<Message>> {
        self.reading = false;
        if self.inputs != Some(inputs) {
            for reason in Capabilities::reasons(&inputs) {
                eprintln!("cupertino-shell-agent: menu item off: {reason}");
            }
            self.inputs = Some(inputs);
            self.publish();
        }
        if std::mem::take(&mut self.read_again) {
            return self.read_inputs();
        }
        Vec::new()
    }

    /// The window number of the next panel.
    fn next_window(&mut self) -> u64 {
        self.windows += 1;
        self.windows
    }

    /// A panel of `size` centred on the first output.
    fn centred(&self, size: Size) -> Placement {
        let room = self.output.map_or((0.0, 0.0), |(_, s)| (s.width, s.height));
        Placement::centred(room, (size.width.ceil(), size.height.ceil()))
    }

    /// Seconds until the component next has something to do by itself: the panel's countdown or a held power key.
    fn next_wakeup(&self, now: f64) -> Option<f64> {
        let panel = self.panel.as_ref().and_then(|p| p.panel.next_wakeup(now));
        let power_key = self.power_button.next_wakeup(now);
        [panel, power_key].into_iter().flatten().reduce(f64::min)
    }

    /// Carry out what the clock asks for: the power key held long enough, the countdown run out.
    fn housekeeping(&mut self, now: f64) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        if let Some(effect) = self.power_button.tick(now) {
            commands.extend(self.power_effect(now, effect));
        }
        if let Some(decision) = self.panel.as_ref().and_then(|p| p.panel.tick(now)) {
            commands.extend(self.decide(decision));
        }
        commands
    }

    /// The timer for the next wake-up, when it is not the one asked for already.
    fn schedule(&mut self, now: f64) -> Option<Do<Message>> {
        let at = self.next_wakeup(now).map(|wait| now + wait);
        if at == self.wake.1 {
            return None;
        }
        self.wake = (self.wake.0 + 1, at);
        let wait = at? - now;
        Some(Do::After {
            delay: Duration::from_secs_f64(wait.max(0.0)),
            message: Message::Wake(self.wake.0),
        })
    }

    /// Show the confirmation panel for `kind` on the first output, in place of one that is open. Not on a locked
    /// session, which has nobody to see it.
    fn show_panel(&mut self, now: f64, kind: Kind) -> Vec<Do<Message>> {
        if self.locked {
            eprintln!("cupertino-shell-agent: panel request ignored: the session is locked");
            return Vec::new();
        }
        let mut commands = Vec::new();
        if self.panel.is_some() {
            commands.extend(self.decide(crate::panel::Decision::Cancel));
        }
        if self.output.is_none() {
            eprintln!("cupertino-shell-agent: panel request ignored: the server has no output");
            return commands;
        }
        let frames = view::panel_frames(kind, &self.text);
        let placement = self.centred(Size::new(frames.width, frames.height));
        let number = self.next_window();
        self.panel = Some(PanelState {
            panel: Panel::new(kind, now, self.reopen),
            frames,
            placement,
            keyboard: Keyboard::Exclusive,
            number,
        });
        eprintln!("cupertino-shell-agent: panel shown: {}", kind.log_name());
        commands
    }

    /// The power key went down or up. The lock screen swallows both.
    fn power_key(&mut self, now: f64, pressed: bool) -> Vec<Do<Message>> {
        if self.locked {
            self.power_button.reset();
            return Vec::new();
        }
        eprintln!(
            "cupertino-shell-agent: power key {}",
            if pressed { "down" } else { "up" }
        );
        if pressed {
            self.power_button.press(now);
            return Vec::new();
        }
        match self.power_button.release(now) {
            Some(effect) => self.power_effect(now, effect),
            None => Vec::new(),
        }
    }

    /// Carry out what the hold timing asks for. The panel does not show over another panel or a running log-out.
    fn power_effect(&mut self, now: f64, effect: Effect) -> Vec<Do<Message>> {
        match effect {
            Effect::Sleep => {
                eprintln!("cupertino-shell-agent: power button: sleep");
                self.sleep()
            }
            Effect::ShowPanel if self.panel.is_some() || self.logging_out => {
                eprintln!("cupertino-shell-agent: power button hold ignored: a panel or a log-out is running");
                Vec::new()
            }
            Effect::ShowPanel => self.show_panel(now, Kind::PowerButton),
        }
    }

    /// The session locked or unlocked: a lock cancels the panels, since the lock screen takes the keyboard, and the
    /// key held.
    fn set_locked(&mut self, locked: bool) -> Vec<Do<Message>> {
        self.locked = locked;
        if !locked {
            return Vec::new();
        }
        self.power_button.reset();
        let mut commands = Vec::new();
        if self.panel.is_some() {
            commands.extend(self.decide(crate::panel::Decision::Cancel));
        }
        if self.interrupt_shown() {
            commands.extend(self.choose(crate::interrupt::Choice::Cancel));
        }
        commands
    }

    /// The server asked on `org.cupertino.SessionAgent1`.
    fn request(&mut self, now: f64, request: Request) -> Vec<Do<Message>> {
        match request {
            Request::Panel(kind) => self.show_panel(now, kind),
            Request::PowerKey { pressed } => self.power_key(now, pressed),
            Request::RefreshCapabilities => self.read_inputs(),
        }
    }

    fn heard(&mut self, now: f64, heard: Heard) -> Vec<Do<Message>> {
        match heard {
            Heard::Request(request) => self.request(now, request),
            Heard::Interrupt(interrupt) => self.quit_signal(interrupt),
            Heard::Battery(report) => self.battery(report),
            Heard::ManagerChanged => self.read_inputs(),
            Heard::Ended(what, why) => {
                eprintln!("cupertino-shell-agent: {what} not followed any more: {why}");
                Vec::new()
            }
        }
    }

    /// The system bus is there: the session's ID, the power key, what the menu may offer, the battery and the session
    /// manager.
    fn system_connected(&mut self, system: zbus::Connection) -> Vec<Do<Message>> {
        self.system = Some(system.clone());
        let mut commands = vec![
            {
                let system = system.clone();
                Do::Perform(Box::pin(async move {
                    Message::SessionId(system::session_id(&system).await)
                }))
            },
            {
                let system = system.clone();
                Do::Perform(Box::pin(async move {
                    Message::PowerKeyBlocked(
                        system::block_power_key(&system)
                            .await
                            .map(Arc::new)
                            .map_err(|e| e.to_string()),
                    )
                }))
            },
            follow(&self.heard.0, "the battery", {
                let system = system.clone();
                move |send| async move {
                    system::follow_battery(&system, |b| send(Heard::Battery(b))).await
                }
            }),
            follow(&self.heard.0, "the session manager's name", {
                let system = system.clone();
                move |send| async move {
                    system::follow_manager(&system, || send(Heard::ManagerChanged)).await
                }
            }),
        ];
        commands.extend(self.read_inputs());
        commands
    }

    /// The session is known: its login items, then the applications to reopen.
    fn session_known(&mut self, id: Option<String>) -> Vec<Do<Message>> {
        let Some(id) = id else {
            eprintln!("cupertino-shell-agent: no login session: login items and reopened applications wait for one");
            return Vec::new();
        };
        self.session_id = Some(id.clone());
        vec![cupertino_kit::task::io(
            FILE_DEADLINE,
            move || startup::plan_login_items(&id),
            Message::LoginItems,
        )]
    }

    /// Start the login items one after the other, then plan the applications to reopen, which they leave out.
    fn start_login_items(&mut self, planned: Planned) -> Vec<Do<Message>> {
        let started: HashSet<String> = planned
            .items
            .iter()
            .filter(|item| start_login_item(item))
            .map(|item| item.id.clone())
            .collect();
        let (path, id) = (
            self.restore_path.clone(),
            self.session_id.clone().unwrap_or_default(),
        );
        vec![cupertino_kit::task::io(
            FILE_DEADLINE,
            move || startup::plan_reopen(path.as_deref(), &id, &started),
            Message::Reopen,
        )]
    }
}

/// Run one login item through the host, which reaps it. False when it did not start.
fn start_login_item(item: &autostart::Item) -> bool {
    let mut command = std::process::Command::new(&item.argv[0]);
    command
        .args(&item.argv[1..])
        .stdin(std::process::Stdio::null());
    if let Some(dir) = &item.dir {
        command.current_dir(dir);
    }
    let started = cupertino_kit_shell::host::spawn(command);
    if started {
        eprintln!("cupertino-shell-agent: login item started: {}", item.id);
    } else {
        eprintln!(
            "cupertino-shell-agent: login item {} did not start",
            item.id
        );
    }
    started
}

/// Start each application planned to reopen through the host.
fn reopen(planned: Vec<(String, CommandSpec)>) {
    for (id, spec) in planned {
        let mut command = spec.to_command();
        command.stdin(std::process::Stdio::null());
        if cupertino_kit_shell::host::spawn(command) {
            eprintln!("cupertino-shell-agent: application reopened: {id}");
        } else {
            eprintln!("cupertino-shell-agent: application not reopened: {id}: it did not start");
        }
    }
}

/// Follow a stream on the loop for as long as the component runs: `run` gets what hands a piece of news to the
/// component and returns why its stream ended, which the component then hears.
fn follow<F, Fut>(heard: &async_channel::Sender<Heard>, what: &'static str, run: F) -> Do<Message>
where
    F: FnOnce(Box<dyn Fn(Heard) + Send + Sync>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = String> + Send + 'static,
{
    let heard = heard.clone();
    let send = heard.clone();
    Do::Perform(Box::pin(async move {
        // the component holds the receiver for as long as the future runs
        let why = run(Box::new(move |news| drop(send.try_send(news)))).await;
        Message::Heard(Heard::Ended(what, why))
    }))
}

/// The next piece of news the streams and the control interface handed over.
fn listen(heard: &async_channel::Receiver<Heard>) -> Do<Message> {
    let heard = heard.clone();
    Do::Perform(Box::pin(async move {
        match heard.recv().await {
            Ok(news) => Message::Heard(news),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The server's first output with its size, if it has one.
fn first_output() -> Option<(OutputId, Size)> {
    outputs::all()
        .into_iter()
        .find_map(|o| o.geometry.map(|g| (o.id, g.size)))
}

fn panel_key(number: u64) -> WindowKey {
    WindowKey(WindowName::from(format!("panel-{number}")))
}

fn interrupt_key(number: u64) -> WindowKey {
    WindowKey(WindowName::from(format!("interrupt-{number}")))
}

impl Application for Agent {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Panels shown now and then: a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let heard = async_channel::unbounded();
        let mut commands = bus::session(|c| Message::Session(c.ok()), Message::SessionClosed);
        commands.extend(kit_bus::connect(
            Bus::System,
            |c| Message::System(c.ok()),
            Message::SystemClosed,
        ));
        commands.push(listen(&heard.1));
        let restore_path = restore::state_path_from_env();
        let path = restore_path.clone();
        commands.push(cupertino_kit::task::io(
            FILE_DEADLINE,
            move || startup::reopen_choice(path.as_deref()),
            Message::ReopenChoice,
        ));
        commands.push(cupertino_kit::task::io(
            FILE_DEADLINE,
            startup::user_name,
            Message::UserName,
        ));
        let model = Model {
            start: Instant::now(),
            session: None,
            system: None,
            heard,
            session_id: None,
            restore_path,
            reopen: restore::REOPEN_DEFAULT,
            user_name: None,
            inputs: None,
            reading: false,
            read_again: false,
            locked: cupertino_kit_shell::session::locked(),
            output: first_output(),
            text: Arc::clone(&cupertino_kit_shell::environment().text),
            panel: None,
            windows: 0,
            power_button: PowerButton::new(POWER_BUTTON_SLEEPS_SYSTEM),
            power_key: None,
            logging_out: false,
            quitting: None,
            low_battery: LowBattery::new(),
            wake: (0, None),
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        let now = model.now();
        let mut commands = match message {
            Message::Session(Some(connection)) => {
                model.session = Some(connection.clone());
                let requests = model.heard.0.clone();
                vec![
                    Do::Perform(Box::pin(async move {
                        let (sender, receiver) = async_channel::unbounded();
                        let served = control::serve(&connection, sender).await;
                        if served.is_ok() {
                            // the interface's calls, handed on for as long as the component runs
                            while let Ok(request) = receiver.recv().await {
                                drop(requests.try_send(Heard::Request(request)));
                            }
                        }
                        Message::Served(served.map_err(|e| e.to_string()))
                    })),
                    follow(&model.heard.0, "the Session1 signals", {
                        let session = model.session.clone().expect("set above");
                        move |send| async move {
                            crate::session::follow_interrupts(&session, |i| {
                                send(Heard::Interrupt(i))
                            })
                            .await
                        }
                    }),
                ]
            }
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the agent serves nobody: the host starts it again
                eprintln!("cupertino-shell-agent: no session bus connection");
                vec![Do::Exit]
            }
            Message::Served(Ok(())) => Vec::new(),
            Message::Served(Err(e)) => {
                eprintln!(
                    "cupertino-shell-agent: {} is not served: {e}",
                    control::BUS_NAME
                );
                vec![Do::Exit]
            }
            Message::System(Some(system)) => model.system_connected(system),
            Message::System(None) => {
                // nothing of the system is known: the menu offers what needs none of it
                eprintln!("cupertino-shell-agent: no system bus connection");
                model.inputs_read(Inputs {
                    can_suspend: crate::capabilities::Answer::NotAvailable,
                    can_reboot: crate::capabilities::Answer::NotAvailable,
                    can_power_off: crate::capabilities::Answer::NotAvailable,
                    session_manager: false,
                })
            }
            Message::SystemClosed => {
                model.system = None;
                Vec::new()
            }
            Message::SessionId(id) => model.session_known(id),
            Message::Heard(heard) => {
                let mut commands = model.heard(now, heard);
                if !matches!(commands.last(), Some(Do::Exit)) {
                    commands.push(listen(&model.heard.1));
                }
                commands
            }
            Message::Inputs(inputs) => model.inputs_read(inputs),
            Message::UserName(name) => {
                model.user_name = Some(name);
                model.publish();
                Vec::new()
            }
            Message::ReopenChoice(reopen) => {
                model.reopen = reopen;
                Vec::new()
            }
            Message::PowerKeyBlocked(Ok(fd)) => {
                model.power_key = Some(fd);
                Vec::new()
            }
            Message::PowerKeyBlocked(Err(e)) => {
                eprintln!("cupertino-shell-agent: logind handles the power key itself: {e}");
                Vec::new()
            }
            Message::LoginItems(planned) => model.start_login_items(planned),
            Message::Reopen(planned) => {
                reopen(planned);
                Vec::new()
            }
            Message::Locked(locked) => model.set_locked(locked),
            Message::LogOutRequested => model.show_panel(now, Kind::LogOut),
            Message::Panel(input) => model.panel_input(input),
            Message::Interrupt(input) => model.interrupt_input(input),
            Message::Wake(number) => {
                if number == model.wake.0 {
                    model.wake.1 = None;
                }
                Vec::new()
            }
            Message::Outputs => {
                model.output = first_output();
                Vec::new()
            }
            Message::Closed(key) => model.closed(&key),
            Message::Done(done) => model.done(done),
        };
        commands.extend(model.housekeeping(now));
        commands.extend(model.schedule(now));
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let now = model.now();
        let mut windows = Vec::new();
        if let Some(panel) = &model.panel {
            windows.push(view::panel(
                panel_key(panel.number),
                panel.panel.kind(),
                panel.panel.seconds_left(now),
                panel.panel.reopen(),
                &panel.frames,
                Message::Panel,
            ));
        }
        if let Some(quitting) = &model.quitting {
            if let Some(shown) = &quitting.interrupt {
                windows.push(view::interrupt(
                    interrupt_key(shown.number),
                    InterruptView {
                        app: &shown.app,
                        reason: shown.reason,
                        stage: shown.stage,
                        action: quitting.then,
                        text: &model.text,
                    },
                    Message::Interrupt,
                ));
            }
        }
        windows
    }
}

impl Program for Agent {
    /// Each panel on the overlay layer of the server's first output, anchored to its top-left corner so that its
    /// margins place it where it stands, holding the keyboard until it lets go.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let (size, placement, keyboard) = match (&model.panel, model.interrupt()) {
            (Some(panel), _) if &panel_key(panel.number) == key => (
                Size::new(panel.frames.width, panel.frames.height),
                panel.placement,
                panel.keyboard,
            ),
            (_, Some((shown, height))) => (
                Size::new(crate::interrupt::WIDTH, height),
                shown.placement,
                shown.keyboard,
            ),
            _ => (
                Size::ZERO,
                Placement::centred((0.0, 0.0), (0.0, 0.0)),
                Keyboard::None,
            ),
        };
        let (left, top) = placement.origin();
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: NAMESPACE,
                output: model.output.map(|(id, _)| id),
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    left: true,
                    ..Anchor::default()
                },
                size: Size::new(size.width.ceil(), size.height.ceil()),
                margin: Margin {
                    top,
                    left,
                    ..Margin::default()
                },
                exclusive_zone: -1,
                keyboard,
            }),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Session(Change::Locked(locked)) => Some(Message::Locked(locked)),
            Event::Session(Change::LogOutRequested) => Some(Message::LogOutRequested),
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(key) => Some(Message::Closed(key)),
            _ => None,
        }
    }
}

impl Component for Agent {
    const NAME: ComponentName = ComponentName::AGENT;
}
