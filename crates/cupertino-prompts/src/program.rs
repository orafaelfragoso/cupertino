//! The program on Kit's loop: the model of every prompt, the buses and futures they are served on, and the windows
//! that show them.
//!
//! - [`authentication`]: the polkit agent's prompts and polkit's helper.
//! - [`keyring`]: the keyring's prompts and the offer about the default keyring.
//! - [`consent`]: the portal's consent questions, asked over `org.cupertino.Prompts1`.
//! - [`desktop_control`]: the desktop-control consent question, asked by the server's consent authority.
//!
//! One prompt is on screen at a time, in an alert over a scrim on every output; the one shown first stays until it
//! ends, and the others wait: an authentication first, then the keyring, then the portal's consent questions in the
//! order they came, then desktop control. While the session is locked, or is not the active one of its seat, nothing is
//! shown: the authentication on screen is cancelled, the keyring's prompt is ended and every consent question is
//! dismissed, as their callers expect.

pub mod authentication;
pub mod consent;
pub mod desktop_control;
pub mod keyring;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Application, Command, Key, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::ScrollState;
use cupertino_kit::text::SecureBuffer;
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::session::{self, Change};
use cupertino_kit_shell::{
    Anchor, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};
use zeroize::Zeroizing;

use crate::keyring::model::Field;
use crate::keyring::offer::Outcome;
use crate::keyring::prompter::{Owned, Request};
use crate::logind;
use crate::polkit::helper::{Helper, Line, Route};
use crate::polkit::{Event as Polkit, User};
use crate::view;

use authentication::Authentication;
use consent::Consent;
use desktop_control::DesktopControl;
use keyring::Keyring;

/// The layer surfaces' namespaces, for the server.
pub const ALERT_NAMESPACE: &str = "cupertino-prompts";
pub const SCRIM_NAMESPACE: &str = "cupertino-prompts-scrim";

/// The prompts.
pub struct Prompts;

/// Text the person typed in a field, kept for the button that submits it. `Debug` never prints it, and its copy is
/// wiped when the last clone goes.
#[derive(Clone, Default)]
pub struct Secret(Arc<Zeroizing<String>>);

impl Secret {
    /// What `buffer` holds now.
    pub fn of(buffer: &SecureBuffer) -> Secret {
        buffer.with_secret(|text| Secret(Arc::new(Zeroizing::new(text.to_owned()))))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// A value a message carries over to the loop once, though messages are cloned.
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Handoff(Arc::clone(&self.0))
    }
}

impl<T> Handoff<T> {
    pub fn new(value: T) -> Handoff<T> {
        Handoff(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, the first time only.
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// What the futures of the program hear, handed to the loop one at a time ([`listen`]).
#[derive(Clone)]
pub enum News {
    /// Whether the session is the active one of its seat now.
    Active(bool),
    Polkit(Polkit),
    /// Whether polkitd took the agent's first registration.
    Registered(Result<(), String>),
    /// A line of the helper started as `run`, or why it can't be read.
    Helper {
        run: u64,
        line: Result<Line, String>,
    },
    Keyring(Request),
    /// How the prompter came to its names.
    Owned(Result<Owned, String>),
    /// Work of the offer for the I/O pool.
    Io(crate::keyring::asking::Io),
    /// A call of the portal.
    Consent(crate::consent::Request),
}

/// What the person does in an authentication prompt.
#[derive(Clone, Debug)]
pub enum AuthInput {
    /// The secret field holds this now.
    Secret(Secret),
    /// The field that shows what is typed holds this now.
    Text(String),
    /// Authenticate, or Return in the field: what it holds goes to the helper.
    Submit,
    /// Return in the secret field, with what it holds.
    SubmitSecret(Secret),
    Cancel,
    Details(bool),
    /// The users' menu opens or closes.
    Users(bool),
    /// The user at this index of the menu is chosen.
    User(usize),
}

/// What the person does in a keyring prompt.
#[derive(Clone, Debug)]
pub enum KeyringInput {
    Field(Field, Secret),
    Continue,
    Cancel,
    Choice(bool),
}

/// What the person does in a consent alert, and what its alert hears.
#[derive(Clone)]
pub enum ConsentInput {
    /// Take Screenshot, or Allow.
    Allow,
    /// Cancel, or Don't Allow.
    Deny,
    /// The shortcut list moved.
    Scroll(ScrollState),
    /// The alert of the call presented its first frame.
    Presented(u64),
    /// The arming delay of the call has passed.
    Armed(u64),
    /// The application icon `name` was read on the I/O pool; `None` when it resolves to none.
    Icon {
        name: String,
        sources: Option<Vec<ImageSource>>,
    },
}

/// What the person does in the desktop-control alert, and what the alert hears.
#[derive(Clone, Debug)]
pub enum DesktopControlInput {
    Allow,
    DontAllow,
    /// The alert of the run presented its first frame.
    Presented(cupertino_kit_shell::consent::Run),
    /// The arming delay of the run's alert has passed.
    Armed(cupertino_kit_shell::consent::Run),
    /// The icon of the run's app, read on the I/O pool.
    Icons {
        run: cupertino_kit_shell::consent::Run,
        icons: Vec<ImageSource>,
    },
}

#[derive(Clone)]
pub enum Message {
    /// The system bus connection, or none.
    System(Option<zbus::Connection>),
    SystemClosed,
    /// The session bus connection the offer and the Secret Service calls go on, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// The prompter's own session bus connection, which serves no object, or none.
    Prompter(Option<zbus::Connection>),
    PrompterClosed,
    /// The session bus connection that only asks who calls `org.cupertino.Prompts1`, or none.
    Lookup(Option<zbus::Connection>),
    LookupClosed,
    /// This process's logind session, or why there is none.
    Logind(Result<logind::Session, String>),
    /// polkit's helper, found on the I/O pool.
    Routes(Vec<Route>),
    /// The names of the users of the request of `cookie` and the rows of its details, read on the I/O pool.
    Looked {
        cookie: String,
        users: Vec<(User, String)>,
        details: Vec<(&'static str, String)>,
    },
    /// The helper started as `run`, or why it did not.
    HelperStarted {
        run: u64,
        helper: Handoff<Result<Helper, String>>,
    },
    /// An answer to the helper started as `run` was not written.
    AnswerFailed {
        run: u64,
        error: String,
    },
    Heard(News),
    /// A future that follows a bus ended, and why.
    Stopped(String),
    /// The offer about the default keyring ended.
    Offered(Outcome),
    Locked(bool),
    CapsLock(bool),
    Outputs,
    Closed(WindowKey),
    Authentication(AuthInput),
    Keyring(KeyringInput),
    Consent(ConsentInput),
    /// What the server's consent authority said.
    Authority(cupertino_kit_shell::consent::Event),
    DesktopControl(DesktopControlInput),
    /// Escape in the alert.
    Escape,
    /// A frame while the scrim fades in.
    Frame,
    /// Work on a pool ended with nothing to say.
    Idle,
}

/// Which prompt is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    Authentication,
    Keyring,
    Consent,
    DesktopControl,
}

/// The prompt on screen.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    front: Front,
    /// What it is about: the authentication's cookie, the keyring caller's name and path, the consent's call, the
    /// desktop-control run.
    about: String,
    /// The number of its windows, new for each prompt shown.
    serial: u64,
    since: Instant,
}

pub struct Model {
    heard: (async_channel::Sender<News>, async_channel::Receiver<News>),
    system: Option<zbus::Connection>,
    session_bus: Option<zbus::Connection>,
    lookup: Option<zbus::Connection>,
    /// This process's logind session: `None` until logind answered, `Some(None)` when it has none for it.
    login: Option<Option<logind::Session>>,
    locked: bool,
    /// What logind says; unknown counts as not active.
    active: Option<bool>,
    caps_lock: bool,
    outputs: Vec<OutputId>,
    shown: Option<Shown>,
    serials: u64,
    /// Scrims the server closed, left out until the next prompt.
    closed: HashSet<WindowKey>,
    auth: Authentication,
    keyring: Keyring,
    consent: Consent,
    desktop_control: DesktopControl,
}

/// The next news a future heard.
fn listen(heard: &async_channel::Receiver<News>) -> Command<Message> {
    let heard = heard.clone();
    Command::Perform(Box::pin(async move {
        match heard.recv().await {
            Ok(news) => Message::Heard(news),
            // the model holds a sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The outputs the server describes, where the scrims go.
fn described_outputs() -> Vec<OutputId> {
    outputs::all()
        .into_iter()
        .filter(|o| o.geometry.is_some())
        .map(|o| o.id)
        .collect()
}

impl Model {
    fn away(&self) -> bool {
        self.locked || self.active != Some(true)
    }

    /// The session went away or came back: the prompts follow.
    fn follow_away(&mut self) -> Vec<Command<Message>> {
        let away = self.away();
        let commands = self.auth.set_away(away);
        self.keyring.set_away(away);
        self.consent.set_away(away);
        self.desktop_control.set_away(away);
        commands
    }

    fn alert_key(serial: u64) -> WindowKey {
        WindowKey(Key::from(format!("prompt-{serial}")))
    }

    fn scrim_key(serial: u64, output: OutputId) -> WindowKey {
        WindowKey(Key::from(format!("scrim-{serial}-{output:?}")))
    }

    /// Show the prompt that should be on screen: the one shown stays while it lasts, else the authentication comes
    /// before the keyring, the keyring before the portal's consent questions, and they before desktop control.
    fn arrange(&mut self) {
        let about = |front: Front, model: &Model| match front {
            Front::Authentication => model.auth.shown_about(),
            Front::Keyring => model.keyring.shown_about(),
            Front::Consent => model.consent.shown_about(),
            Front::DesktopControl => model.desktop_control.shown_about(),
        };
        if let Some(shown) = &self.shown {
            if about(shown.front, self).as_ref() == Some(&shown.about) {
                return;
            }
        }
        let next = [
            Front::Authentication,
            Front::Keyring,
            Front::Consent,
            Front::DesktopControl,
        ]
        .into_iter()
        .find_map(|front| about(front, self).map(|about| (front, about)));
        self.shown = next.map(|(front, about)| {
            self.serials += 1;
            Shown {
                front,
                about,
                serial: self.serials,
                since: Instant::now(),
            }
        });
        self.closed.clear();
        self.auth.reset_view();
    }

    /// The alert on screen was closed by the server, which closes it only when it can't show it, or the person
    /// pressed Escape: the prompt is dismissed.
    fn dismiss(&mut self) -> Vec<Command<Message>> {
        match self.shown.as_ref().map(|s| s.front) {
            Some(Front::Authentication) => self.auth.dismiss(),
            Some(Front::Keyring) => {
                self.keyring.escape();
                Vec::new()
            }
            Some(Front::Consent) => {
                self.consent.dismiss();
                Vec::new()
            }
            Some(Front::DesktopControl) => {
                self.desktop_control.dismiss();
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// The commands of a bus connection that came: what is served on it starts.
    fn connected(&mut self) -> Vec<Command<Message>> {
        let mut commands =
            self.auth
                .start(self.system.as_ref(), self.login.as_ref(), &self.heard.0);
        let login = self
            .login
            .as_ref()
            .map(|l| l.as_ref().map_or(String::new(), |s| s.id.clone()));
        commands.extend(
            self.keyring
                .start_offer(self.session_bus.as_ref(), login, &self.heard.0),
        );
        commands.extend(self.consent.serve(
            self.session_bus.as_ref(),
            self.lookup.as_ref(),
            &self.heard.0,
        ));
        commands
    }

    fn shell(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Locked(locked) => {
                self.locked = locked;
                self.follow_away()
            }
            Message::CapsLock(on) => {
                self.caps_lock = on;
                Vec::new()
            }
            Message::Outputs => {
                self.outputs = described_outputs();
                Vec::new()
            }
            Message::Closed(key) => {
                let alert = self.shown.as_ref().map(|s| Model::alert_key(s.serial));
                if alert.as_ref() == Some(&key) {
                    eprintln!("cupertino-prompts: the server closed the prompt");
                    self.dismiss()
                } else {
                    self.closed.insert(key);
                    Vec::new()
                }
            }
            Message::Escape => self.dismiss(),
            _ => Vec::new(),
        }
    }

    fn buses(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::System(Some(connection)) => {
                self.system = Some(connection.clone());
                vec![Command::Perform(Box::pin(async move {
                    Message::Logind(logind::find(&connection).await.map_err(|e| e.to_string()))
                }))]
            }
            Message::Session(Some(connection)) => {
                self.session_bus = Some(connection);
                self.connected()
            }
            Message::Prompter(Some(connection)) => self.keyring.serve(connection, &self.heard.0),
            Message::Lookup(Some(connection)) => {
                self.lookup = Some(connection);
                self.connected()
            }
            Message::System(None) | Message::SystemClosed => {
                eprintln!("cupertino-prompts: no system bus connection");
                vec![Command::Exit]
            }
            Message::Session(None) | Message::SessionClosed => {
                eprintln!("cupertino-prompts: no session bus connection");
                vec![Command::Exit]
            }
            Message::Prompter(None) | Message::PrompterClosed => {
                eprintln!("cupertino-prompts: no session bus connection for the keyring's prompts");
                vec![Command::Exit]
            }
            Message::Lookup(None) | Message::LookupClosed => {
                eprintln!("cupertino-prompts: no session bus connection to ask who calls");
                vec![Command::Exit]
            }
            Message::Logind(Ok(found)) => {
                eprintln!("cupertino-prompts: in logind session {}", found.id);
                self.active = Some(found.active);
                let system = self.system.clone();
                let path = found.path.clone();
                self.login = Some(Some(found));
                let mut commands = self.follow_away();
                if let Some(system) = system {
                    let heard = self.heard.0.clone();
                    commands.push(Command::Perform(Box::pin(async move {
                        Message::Stopped(
                            logind::follow_active(&system, path, |active| {
                                let _ = heard.try_send(News::Active(active));
                            })
                            .await,
                        )
                    })));
                }
                commands.extend(self.connected());
                commands
            }
            Message::Logind(Err(error)) => {
                // a session without logind (a nested one) counts as active; polkitd registers no agent without one
                eprintln!(
                    "cupertino-prompts: no logind session ({error}); the session counts as active"
                );
                self.login = Some(None);
                self.active = Some(true);
                let mut commands = self.follow_away();
                commands.extend(self.connected());
                commands
            }
            _ => Vec::new(),
        }
    }

    fn heard(&mut self, news: News) -> Vec<Command<Message>> {
        let mut commands = vec![listen(&self.heard.1)];
        commands.extend(match news {
            News::Active(active) => {
                self.active = Some(active);
                self.follow_away()
            }
            News::Polkit(event) => self.auth.polkit(event),
            News::Registered(Ok(())) => {
                eprintln!("cupertino-prompts: registered with polkitd");
                Vec::new()
            }
            News::Registered(Err(error)) => {
                eprintln!("cupertino-prompts: polkitd did not take the agent: {error}");
                Vec::new()
            }
            News::Helper { run, line } => self.auth.helper_line(run, line),
            News::Keyring(request) => {
                self.keyring.request(request);
                Vec::new()
            }
            News::Owned(owned) => {
                let mut commands = self.keyring.owned(owned);
                commands.extend(self.connected());
                commands
            }
            News::Io(work) => vec![keyring::on_pool(work)],
            News::Consent(request) => self.consent.request(request),
        });
        commands
    }
}

impl Application for Prompts {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Small alerts shown now and then: a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let options = crate::starting();
        let heard = async_channel::unbounded();
        let mut commands = vec![listen(&heard.1)];
        commands.extend(kit_bus::connect(
            Bus::System,
            |connection| Message::System(connection.ok()),
            Message::SystemClosed,
        ));
        commands.extend(kit_bus::connect(
            Bus::Session,
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        ));
        commands.extend(kit_bus::connect(
            Bus::Session,
            |connection| Message::Prompter(connection.ok()),
            Message::PrompterClosed,
        ));
        commands.extend(kit_bus::connect(
            Bus::Session,
            |connection| Message::Lookup(connection.ok()),
            Message::LookupClosed,
        ));
        let (auth, found) = Authentication::new(options.helper);
        commands.extend(found);
        let model = Model {
            heard,
            system: None,
            session_bus: None,
            lookup: None,
            login: None,
            locked: session::locked(),
            active: None,
            caps_lock: false,
            outputs: described_outputs(),
            shown: None,
            serials: 0,
            closed: HashSet::new(),
            auth,
            keyring: Keyring::new(options.keyring_offer),
            consent: Consent::new(),
            desktop_control: DesktopControl::bind(),
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let mut commands = match message {
            Message::System(_)
            | Message::SystemClosed
            | Message::Session(_)
            | Message::SessionClosed
            | Message::Prompter(_)
            | Message::PrompterClosed
            | Message::Lookup(_)
            | Message::LookupClosed
            | Message::Logind(_) => model.buses(message),
            Message::Routes(routes) => {
                model.auth.found(routes);
                model.connected()
            }
            Message::Looked {
                cookie,
                users,
                details,
            } => {
                model.auth.looked(cookie, users, details);
                Vec::new()
            }
            Message::HelperStarted { run, helper } => {
                model.auth.helper_started(run, helper, &model.heard.0)
            }
            Message::AnswerFailed { run, error } => model.auth.answer_failed(run, &error),
            Message::Heard(news) => model.heard(news),
            Message::Stopped(why) => {
                eprintln!("cupertino-prompts: {why}");
                Vec::new()
            }
            Message::Offered(outcome) => {
                if outcome != Outcome::Quiet {
                    eprintln!(
                        "cupertino-prompts: the offer about the default keyring ended: {outcome:?}"
                    );
                }
                Vec::new()
            }
            Message::Authentication(input) => {
                let front = model.shown.as_ref().map(|s| s.front);
                if front == Some(Front::Authentication) {
                    model.auth.input(input)
                } else {
                    Vec::new()
                }
            }
            Message::Keyring(input) => {
                if model.shown.as_ref().map(|s| s.front) == Some(Front::Keyring) {
                    model.keyring.input(input);
                }
                Vec::new()
            }
            Message::Consent(input @ (ConsentInput::Allow | ConsentInput::Deny)) => {
                if model.shown.as_ref().map(|s| s.front) == Some(Front::Consent) {
                    model.consent.input(input)
                } else {
                    Vec::new()
                }
            }
            Message::Consent(input) => model.consent.input(input),
            Message::Authority(event) => model.desktop_control.heard(event),
            Message::DesktopControl(
                input @ (DesktopControlInput::Allow | DesktopControlInput::DontAllow),
            ) => {
                if model.shown.as_ref().map(|s| s.front) == Some(Front::DesktopControl) {
                    model.desktop_control.input(input)
                } else {
                    Vec::new()
                }
            }
            Message::DesktopControl(input) => model.desktop_control.input(input),
            Message::Frame | Message::Idle => Vec::new(),
            shell => model.shell(shell),
        };
        let before = model.shown.as_ref().map(|s| s.serial);
        model.arrange();
        if let Some(shown) = model.shown.as_ref() {
            if before != Some(shown.serial) {
                let window = Model::alert_key(shown.serial);
                match shown.front {
                    Front::Consent => commands.extend(model.consent.opened(window)),
                    Front::DesktopControl => {
                        commands.extend(model.desktop_control.opened(window));
                    }
                    Front::Authentication | Front::Keyring => {}
                }
            }
        }
        commands.extend(model.auth.look_up());
        commands.extend(model.focus(before));
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let Some(shown) = &model.shown else {
            return Vec::new();
        };
        let fading = Some(shown.since);
        let mut windows: Vec<Window<Message>> = model
            .outputs
            .iter()
            .map(|output| Model::scrim_key(shown.serial, *output))
            .filter(|key| !model.closed.contains(key))
            .map(|key| view::scrim(key, fading))
            .collect();
        // after the scrims, so it is stacked above them
        let key = Model::alert_key(shown.serial);
        match shown.front {
            Front::Authentication => {
                if let Some(prompt) = model.auth.view(model.caps_lock) {
                    windows.extend(view::authentication(key, &prompt));
                }
            }
            Front::Keyring => {
                if let Some(prompt) = model.keyring.view(model.caps_lock) {
                    windows.push(view::keyring(key, &prompt));
                }
            }
            Front::Consent => {
                if let Some(prompt) = model.consent.view() {
                    windows.push(view::consent::consent(key, &prompt));
                }
            }
            Front::DesktopControl => {
                if let Some(prompt) = model.desktop_control.view() {
                    windows.push(view::desktop_control::desktop_control(key, &prompt));
                }
            }
        }
        windows
    }
}

impl Model {
    /// The field of a new question takes the keyboard.
    fn focus(&mut self, before: Option<u64>) -> Vec<Command<Message>> {
        let field = match self.shown.as_ref().map(|s| s.front) {
            Some(Front::Authentication) => self.auth.new_question(),
            Some(Front::Keyring) => self.keyring.new_question(),
            Some(Front::Consent | Front::DesktopControl) | None => None,
        };
        // a window that just opened gives its field the focus itself
        match field {
            Some(field) if before == self.shown.as_ref().map(|s| s.serial) => {
                vec![Command::Focus(field)]
            }
            _ => Vec::new(),
        }
    }

    /// The output the alert goes on: desktop control's on the output the run would control, when the server describes
    /// it; any other on the active output (`None`).
    fn alert_output(&self) -> Option<OutputId> {
        if self.shown.as_ref().map(|s| s.front) != Some(Front::DesktopControl) {
            return None;
        }
        let name = self.desktop_control.output()?;
        outputs::all()
            .into_iter()
            .find(|o| o.geometry.is_some() && o.name.as_deref() == Some(name))
            .map(|o| o.id)
    }

    /// The size of the alert's surface for what it shows.
    fn alert_size(&self) -> Size {
        match self.shown.as_ref().map(|s| s.front) {
            Some(Front::Keyring) => view::KEYRING_SIZE,
            Some(Front::Consent) => self.consent.alert_size(),
            Some(Front::DesktopControl) => view::desktop_control::SIZE,
            _ => view::authentication_size(self.auth.details_shown()),
        }
    }
}

impl Program for Prompts {
    /// The scrims cover their outputs on the overlay layer and take no keys; the alert is centred above them on the
    /// active output (desktop control's on the output the run would control, when it is there) and takes the keyboard.
    /// A menu of the alert is Kit's popup.
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let shown = model.shown.as_ref();
        let alert = shown.map(|s| Model::alert_key(s.serial));
        let scrim = shown.and_then(|s| {
            model
                .outputs
                .iter()
                .find(|o| Model::scrim_key(s.serial, **o) == *window)
                .copied()
        });
        let role = if alert.as_ref() == Some(window) {
            Role::Layer(LayerSpec {
                namespace: ALERT_NAMESPACE,
                output: model.alert_output(),
                layer: Layer::Overlay,
                anchor: Anchor::default(),
                size: model.alert_size(),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::Exclusive,
            })
        } else if let Some(output) = scrim {
            Role::Layer(LayerSpec {
                namespace: SCRIM_NAMESPACE,
                output: Some(output),
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
                size: Size::new(0.0, 0.0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::None,
            })
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
        match event {
            Event::Session(Change::Locked(locked)) => Some(Message::Locked(locked)),
            Event::CapsLock(on) => Some(Message::CapsLock(on)),
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(key) => Some(Message::Closed(key)),
            Event::Consent(event) => Some(Message::Authority(event)),
            _ => None,
        }
    }
}
