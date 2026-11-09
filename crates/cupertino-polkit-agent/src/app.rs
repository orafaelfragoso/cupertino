//! The `cupertino-polkit-agent` process: the agent on the system bus, the helper of the prompt on screen, and the
//! prompt itself on the server's connection: a scrim on the overlay layer of every output and the panel above them,
//! which takes the keyboard exclusively. While the session is locked or isn't the active one nothing is shown.

use std::error::Error;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_polkit_agent::access::{clicked, prompt_tree};
use cupertino_polkit_agent::agent::{self, Event as Polkit, User};
use cupertino_polkit_agent::conversation::Conversation;
use cupertino_polkit_agent::helper::{HelperError, Line, Route};
use cupertino_polkit_agent::model::{Agent, Effect, Key};
use cupertino_polkit_agent::panel::{
    Command, Content, Control, Frames, Panel, PanelKey, Status, View, PADDING, WIDTH,
};
use cupertino_polkit_agent::render::{scrim_color, scrim_opacity, Painter, Palette};
use cupertino_polkit_agent::surfaces;
use cupertino_polkit_agent::users::{self, display_names};
use cupertino_protocol_app::{AppEffects, AppEvent, Appearance as AppearanceObserver};
use cupertino_shell_client::{Anchor, LayerShell, LayerSurface, ShellEvent};
use cupertino_theme::{Appearance, Theme};
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{AccessibilityAdapter, Action};
use cupertino_ui_platform::WindowId;
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Connection, Event, KeyboardEvent, OutputId, PointerEvent, SurfaceId,
};

use crate::logind::Session;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The window assistive technology knows the panel as.
const PANEL_WINDOW: WindowId = WindowId(1);
/// How often the spinner turns a spoke while the answer is checked.
const SPINNER_STEP: Duration = Duration::from_millis(83);

/// What the other threads tell the loop.
enum Message {
    Polkit(Polkit),
    /// A line of the helper started as `run`, or how it ended.
    Helper {
        run: u64,
        line: std::result::Result<Line, HelperError>,
    },
    Active(std::result::Result<bool, String>),
}

/// Sends to the loop and wakes it.
#[derive(Clone)]
struct Notifier {
    sender: Sender<Message>,
    waker: cupertino_wayland_client::Waker,
}

impl Notifier {
    fn send(&self, message: Message) {
        // the loop is gone only while the process ends
        if self.sender.send(message).is_ok() {
            self.waker.wake();
        }
    }
}

/// Wakes the loop from assistive technology's thread.
struct WakeLoop(cupertino_wayland_client::Waker);

impl Wake for WakeLoop {
    fn wake(self: Arc<Self>) {
        self.0.wake();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.wake();
    }
}

/// The prompt on screen: its surfaces and what the panel shows that the model does not hold.
struct Shown {
    cookie: String,
    scrims: Vec<(OutputId, LayerSurface, ShmPresenter)>,
    layer: LayerSurface,
    presenter: ShmPresenter,
    panel: Panel,
    frames: Frames,
    message: Vec<String>,
    /// polkitd's users with the names the prompt gives them.
    users: Vec<(User, String)>,
    details: Vec<(&'static str, String)>,
    since: Instant,
    /// When the answer went to the helper, for the spinner.
    checking: Option<Instant>,
}

impl Shown {
    fn surfaces(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.scrims
            .iter()
            .map(|(_, layer, _)| layer.surface())
            .chain([self.layer.surface()])
    }

    /// Every surface had its first configure: the scrims and the panel are mapped together.
    fn configured(&self) -> bool {
        self.presenter.size().is_some() && self.scrims.iter().all(|(_, _, p)| p.size().is_some())
    }

    fn fading(&self, reduce_motion: bool) -> f64 {
        scrim_opacity(self.since.elapsed(), reduce_motion)
    }
}

struct App {
    conn: Connection,
    layer_shell: LayerShell,
    _effects: AppEffects,
    _appearance: AppearanceObserver,
    theme: Theme,
    painter: Painter,
    dark: bool,
    increase_contrast: bool,
    reduce_motion: bool,
    locked: bool,
    active: bool,
    conversation: Conversation,
    messages: Receiver<Message>,
    shown: Option<Shown>,
    access: AtspiAdapter,
    access_active: bool,
}

/// Connect, register with polkitd and serve until a connection fails.
pub fn run() -> Result<()> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let painter = Painter::new(
        Palette::from_tokens(&theme.tokens, Appearance::Light)?,
        &assets,
    )?;
    let routes = Route::find();
    if routes.is_empty() {
        return Err("polkit's agent helper is not installed".into());
    }
    // the connection the server made for this process holds the prompt's role only
    let mut conn = Connection::connect()?;
    let layer_shell = LayerShell::bind(&mut conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let effects = AppEffects::bind(&mut conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    if !effects.tells_session_lock() {
        return Err(
            "the server's cupertino_app_effects_v1 does not tell the session's lock".into(),
        );
    }
    let appearance = effects.appearance(&conn);
    let (sender, messages) = mpsc::channel();
    let notifier = Notifier {
        sender,
        waker: conn.waker(),
    };
    let access = AtspiAdapter::new(Waker::from(Arc::new(WakeLoop(conn.waker()))));

    let bus = zbus::blocking::Connection::system()?;
    let session = Session::find(&bus)?;
    let watcher = notifier.clone();
    session.watch_active(move |active| watcher.send(Message::Active(active)))?;
    let active = session.active()?;
    let to_loop = notifier.clone();
    agent::serve(&bus, &session.id, &locale(), move |event| {
        to_loop.send(Message::Polkit(event))
    })?;
    eprintln!(
        "cupertino-polkit-agent: registered with polkitd for session {}",
        session.id
    );
    // Safety: `getuid` has no preconditions.
    let me = User(unsafe { libc::getuid() });
    let helpers = std::sync::Mutex::new(notifier);
    let mut app = App {
        conn,
        layer_shell,
        _effects: effects,
        _appearance: appearance,
        theme,
        painter,
        dark: false,
        increase_contrast: false,
        reduce_motion: false,
        // until the server tells, which it does as soon as the observer is created
        locked: true,
        active,
        conversation: Conversation::new(Agent::new(me), routes, move |run, line| {
            // the thread of each helper run sends through the one notifier
            helpers.lock().unwrap().send(Message::Helper { run, line })
        }),
        messages,
        shown: None,
        access,
        access_active: false,
    };
    app.apply_away()?;
    loop {
        app.present()?;
        let timeout = app.timeout();
        for event in app.conn.wait(timeout)? {
            app.handle(event)?;
        }
        while let Ok(message) = app.messages.try_recv() {
            app.message(message)?;
        }
        app.follow_access()?;
        app.sync()?;
    }
}

/// The locale polkitd translates the action's message into.
fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "C".to_owned())
}

fn panel_key(
    code: u32,
    mods: cupertino_wayland_client::Modifiers,
    character: Option<char>,
) -> Option<PanelKey> {
    Some(match code {
        key::TAB => PanelKey::Tab { back: mods.shift },
        key::UP => PanelKey::Up,
        key::DOWN => PanelKey::Down,
        key::SPACE => PanelKey::Space,
        key::ENTER | key::KEYPAD_ENTER => PanelKey::Typed(Key::Return),
        key::ESCAPE => PanelKey::Typed(Key::Escape),
        key::BACKSPACE => PanelKey::Typed(Key::Backspace),
        _ if mods.ctrl || mods.alt || mods.logo => return None,
        _ => PanelKey::Typed(Key::Char(character?)),
    })
}

impl App {
    fn appearance(&self) -> Appearance {
        Appearance::of_session(self.dark, self.increase_contrast)
    }

    fn away(&self) -> bool {
        self.locked || !self.active
    }

    fn apply_away(&mut self) -> Result<()> {
        let away = self.away();
        let effects = self.conversation.agent_mut().set_away(away);
        self.apply(effects)
    }

    /// How long the loop may sleep: while the scrim fades or the spinner turns, until the next frame of it.
    fn timeout(&self) -> Option<Duration> {
        let shown = self.shown.as_ref()?;
        if shown.fading(self.reduce_motion) < 1.0 {
            return Some(Duration::from_millis(16));
        }
        shown.checking.map(|_| SPINNER_STEP)
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Frame(frame) => {
                if let Some(presenter) = self.presenter_mut(frame.surface) {
                    presenter.frame_done();
                }
            }
            Event::Keyboard {
                surface,
                event:
                    KeyboardEvent::Key {
                        code,
                        pressed: true,
                        mods,
                        character,
                        ..
                    },
                ..
            } if self.owns(surface) => {
                if let Some(command) = panel_key(code, mods, character)
                    .and_then(|k| self.shown.as_mut().and_then(|s| s.panel.key(k)))
                {
                    self.command(command)?;
                }
                // Caps Lock changes the line under the field
                self.relayout();
            }
            Event::Pointer {
                surface,
                event:
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed: true,
                        position,
                    },
                ..
            } => {
                let control = self
                    .shown
                    .as_ref()
                    .filter(|s| s.layer.surface() == surface)
                    .and_then(|s| s.frames.control_at(position));
                if let Some(control) = control {
                    self.click(control)?;
                }
            }
            Event::OutputsChanged => self.arrange_scrims()?,
            Event::Extension(event) => {
                if let Some(event) = event.downcast_ref::<AppEvent>() {
                    self.app_event(event)?;
                } else if let Some(event) = event.downcast_ref::<ShellEvent>() {
                    self.shell_event(event)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn app_event(&mut self, event: &AppEvent) -> Result<()> {
        match event {
            AppEvent::Appearance { dark } => {
                self.dark = *dark;
                self.repaint()?;
            }
            AppEvent::DisplayOptions(options) => {
                self.increase_contrast = options.increase_contrast;
                self.reduce_motion = options.reduce_motion;
                self.repaint()?;
            }
            AppEvent::SessionLock { locked } => {
                self.locked = *locked;
                self.access.session_locked(*locked);
                self.apply_away()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn shell_event(&mut self, event: &ShellEvent) -> Result<()> {
        let Some(shown) = &mut self.shown else {
            return Ok(());
        };
        match *event {
            ShellEvent::Configure { surface, size } => {
                if shown.layer.surface() == surface {
                    shown.presenter.configured(shown.layer.configured(size)?);
                } else if let Some((_, layer, presenter)) = shown
                    .scrims
                    .iter_mut()
                    .find(|(_, l, _)| l.surface() == surface)
                {
                    presenter.configured(layer.configured(size)?);
                }
            }
            // the server closes the panel only when it can't show it: the request is dismissed
            ShellEvent::Closed(surface) if shown.layer.surface() == surface => {
                eprintln!("cupertino-polkit-agent: the server closed the prompt");
                let effects = self.conversation.agent_mut().dismiss();
                self.apply(effects)?;
            }
            ShellEvent::Closed(surface) => {
                if let Some(i) = shown
                    .scrims
                    .iter()
                    .position(|(_, l, _)| l.surface() == surface)
                {
                    shown.scrims.remove(i);
                    self.conn.destroy(surface)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn message(&mut self, message: Message) -> Result<()> {
        match message {
            Message::Polkit(Polkit::Begin(request, reply)) => {
                self.conversation.begin(request, reply);
                Ok(())
            }
            Message::Polkit(Polkit::Cancel(cookie)) => {
                self.conversation.cancel(&cookie);
                Ok(())
            }
            Message::Helper { run, line } => {
                // a failed attempt gives the cleared field the keyboard
                if self.conversation.is_current(run) && matches!(line, Ok(Line::Failure)) {
                    if let Some(shown) = &mut self.shown {
                        shown.panel.focus = Control::Field;
                    }
                }
                self.conversation.helper_line(run, line);
                Ok(())
            }
            Message::Active(Ok(active)) => {
                self.active = active;
                self.apply_away()
            }
            Message::Active(Err(error)) => Err(error.into()),
        }
    }

    /// Carry out what the model asks, and what that leads to.
    fn apply(&mut self, effects: Vec<Effect>) -> Result<()> {
        self.conversation.apply(effects);
        Ok(())
    }

    fn command(&mut self, command: Command) -> Result<()> {
        let effects = match command {
            Command::Field(key) => self.conversation.agent_mut().key(key),
            Command::Authenticate => {
                if self
                    .conversation
                    .agent()
                    .prompt()
                    .is_some_and(|p| p.can_answer())
                {
                    self.conversation.agent_mut().key(Key::Return)
                } else {
                    Vec::new()
                }
            }
            Command::Cancel => self.conversation.agent_mut().dismiss(),
            Command::User { back } => {
                let next = self.conversation.agent().prompt().and_then(|prompt| {
                    let users = &prompt.request.users;
                    let i = users.iter().position(|u| *u == prompt.user)?;
                    let n = users.len();
                    Some(users[if back { (i + n - 1) % n } else { (i + 1) % n }])
                });
                match next {
                    Some(user) => self.conversation.agent_mut().choose_user(user),
                    None => Vec::new(),
                }
            }
            Command::ToggleDetails | Command::Focused => Vec::new(),
        };
        self.apply(effects)?;
        self.relayout();
        Ok(())
    }

    fn click(&mut self, control: Control) -> Result<()> {
        let Some(shown) = &mut self.shown else {
            return Ok(());
        };
        let command = shown.panel.click(control);
        self.command(command)
    }

    fn owns(&self, surface: SurfaceId) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|s| s.surfaces().any(|id| id == surface))
    }

    fn presenter_mut(&mut self, surface: SurfaceId) -> Option<&mut ShmPresenter> {
        let shown = self.shown.as_mut()?;
        if shown.layer.surface() == surface {
            return Some(&mut shown.presenter);
        }
        shown
            .scrims
            .iter_mut()
            .find(|(_, l, _)| l.surface() == surface)
            .map(|(_, _, p)| p)
    }

    /// Show the model's prompt, or take the prompt away when the model has none.
    fn sync(&mut self) -> Result<()> {
        let wanted = self
            .conversation
            .agent()
            .prompt()
            .map(|p| p.request.cookie.clone());
        if self.shown.as_ref().map(|s| &s.cookie) == wanted.as_ref() {
            return Ok(());
        }
        if let Some(shown) = self.shown.take() {
            for surface in shown.surfaces().collect::<Vec<_>>() {
                self.conn.destroy(surface)?;
            }
            self.access.window_closed(PANEL_WINDOW);
        }
        if wanted.is_some() {
            self.show()?;
        }
        Ok(())
    }

    fn show(&mut self) -> Result<()> {
        let prompt = self
            .conversation
            .agent()
            .prompt()
            .expect("a prompt to show");
        let request = &prompt.request;
        let message = self.painter.wrap(&request.message, WIDTH - 2.0 * PADDING);
        let users: Vec<(User, String)> = request
            .users
            .iter()
            .copied()
            .zip(display_names(
                &request
                    .users
                    .iter()
                    .map(|u| (*u, users::account(*u)))
                    .collect::<Vec<_>>(),
            ))
            .collect();
        let details = request.detail_rows();
        let cookie = request.cookie.clone();
        let panel = Panel::new(users.len() > 1);
        let frames = self.frames(&panel, message.len(), 0, 2);
        let mut scrims = Vec::new();
        for output in self
            .conn
            .outputs()
            .into_iter()
            .filter(|o| o.geometry.is_some())
        {
            scrims.push(self.scrim(output.id)?);
        }
        // created after the scrims, so it is stacked above them; without an output the server puts it on the active one
        let layer = self.layer_shell.layer_surface(
            &mut self.conn,
            &surfaces::panel((frames.width as u32, frames.height as u32)),
        )?;
        self.shown = Some(Shown {
            cookie,
            scrims,
            presenter: ShmPresenter::new(layer.surface()),
            layer,
            panel,
            frames,
            message,
            users,
            details,
            since: Instant::now(),
            checking: None,
        });
        self.relayout();
        Ok(())
    }

    /// A scrim over all of `output`; a click on it does nothing.
    fn scrim(&mut self, output: OutputId) -> Result<(OutputId, LayerSurface, ShmPresenter)> {
        let layer = self
            .layer_shell
            .layer_surface(&mut self.conn, &surfaces::scrim(output))?;
        let presenter = ShmPresenter::new(layer.surface());
        Ok((output, layer, presenter))
    }

    /// Keep exactly one scrim per output the server has described.
    fn arrange_scrims(&mut self) -> Result<()> {
        let Some(shown) = &mut self.shown else {
            return Ok(());
        };
        let outputs: Vec<OutputId> = self
            .conn
            .outputs()
            .into_iter()
            .filter(|o| o.geometry.is_some())
            .map(|o| o.id)
            .collect();
        let mut gone = Vec::new();
        shown.scrims.retain(|(output, layer, _)| {
            let keep = outputs.contains(output);
            if !keep {
                gone.push(layer.surface());
            }
            keep
        });
        let new: Vec<OutputId> = outputs
            .into_iter()
            .filter(|o| !shown.scrims.iter().any(|(output, _, _)| output == o))
            .collect();
        for surface in gone {
            self.conn.destroy(surface)?;
        }
        for output in new {
            let scrim = self.scrim(output)?;
            if let Some(shown) = &mut self.shown {
                shown.scrims.push(scrim);
            }
        }
        Ok(())
    }

    fn frames(
        &mut self,
        panel: &Panel,
        message_lines: usize,
        status_lines: usize,
        details: usize,
    ) -> Frames {
        let (cancel_title, authenticate_title) = self.painter.button_titles();
        Frames::new(&Content {
            message_lines,
            users: panel.users(),
            status_lines,
            details: panel.details.then_some(details),
            cancel_title,
            authenticate_title,
        })
    }

    /// Lay the panel out again for what it shows now, resize its surface when that changed, and draw it.
    fn relayout(&mut self) {
        let caps_lock = self.conn.modifiers().caps_lock;
        let Some(prompt) = self.conversation.agent().prompt() else {
            return;
        };
        let status_lines = Status::of(prompt, caps_lock).len();
        let checking = prompt.phase == cupertino_polkit_agent::model::Phase::Checking;
        let Some(shown) = &self.shown else {
            return;
        };
        let (panel, message_lines, details) = (
            shown.panel.clone(),
            shown.message.len(),
            shown.details.len(),
        );
        let frames = self.frames(&panel, message_lines, status_lines, details);
        let shown = self.shown.as_mut().expect("checked");
        if frames.height != shown.frames.height {
            shown
                .layer
                .set_placement(Anchor::empty(), (frames.width as u32, frames.height as u32));
        }
        shown.frames = frames;
        shown.checking = match (checking, shown.checking) {
            (true, None) => Some(Instant::now()),
            (true, since) => since,
            (false, _) => None,
        };
        shown.presenter.request_redraw();
    }

    /// The appearance changed: new colours everywhere.
    fn repaint(&mut self) -> Result<()> {
        self.painter.palette = Palette::from_tokens(&self.theme.tokens, self.appearance())?;
        if let Some(shown) = &mut self.shown {
            shown.presenter.request_redraw();
            for (_, _, presenter) in &mut shown.scrims {
                presenter.request_redraw();
            }
        }
        Ok(())
    }

    /// Draw what can take a frame, once every surface of the prompt is configured.
    fn present(&mut self) -> Result<()> {
        let Some(shown) = &mut self.shown else {
            return Ok(());
        };
        if !shown.configured() {
            return Ok(());
        }
        let opacity = shown.fading(self.reduce_motion);
        let palette = self.painter.palette;
        for (_, _, presenter) in &mut shown.scrims {
            if opacity < 1.0 {
                presenter.request_redraw();
            }
            loop {
                match presenter.poll(&self.conn)? {
                    Ready::Idle => break,
                    Ready::ScaleChanged(_) => {}
                    // one colour over the output: the compositor stretches one pixel of it, so the fade costs no
                    // frame of the output's size
                    Ready::Draw => {
                        presenter.draw_solid(&mut self.conn, scrim_color(&palette, opacity))?
                    }
                }
            }
        }
        if shown.checking.is_some() {
            shown.presenter.request_redraw();
        }
        let Some(prompt) = self.conversation.agent().prompt() else {
            return Ok(());
        };
        let caps_lock = self.conn.modifiers().caps_lock;
        let status = Status::of(prompt, caps_lock);
        let user = (shown.users.len() > 1)
            .then(|| shown.users.iter().find(|(u, _)| *u == prompt.user))
            .flatten()
            .map(|(_, name)| name.as_str());
        let elapsed = shown
            .checking
            .map_or(0.0, |since| since.elapsed().as_secs_f64());
        let view = View {
            frames: &shown.frames,
            panel: &shown.panel,
            prompt,
            message: &shown.message,
            user,
            status: &status,
            details: &shown.details,
        };
        let mut drew = false;
        loop {
            match shown.presenter.poll(&self.conn)? {
                Ready::Idle => break,
                Ready::ScaleChanged(_) => {}
                Ready::Draw => {
                    let painter = &mut self.painter;
                    shown
                        .presenter
                        .draw(&mut self.conn, None, |pixmap, scale| {
                            painter.draw(&view, pixmap, scale.factor(), elapsed)
                        })?;
                    drew = true;
                }
            }
        }
        if drew && self.access.is_active() {
            let tree = prompt_tree(PANEL_WINDOW, &view);
            self.access.window_focus(PANEL_WINDOW, true);
            self.access.update(&tree);
        }
        Ok(())
    }

    /// Tell assistive technology the panel again when it starts listening, and do what it asks: a click is a click on
    /// that control, focus on the field gives it the keyboard.
    fn follow_access(&mut self) -> Result<()> {
        let active = self.access.is_active();
        if active && !self.access_active {
            if let Some(shown) = &mut self.shown {
                shown.presenter.request_redraw();
            }
        }
        self.access_active = active;
        let mut requests = Vec::new();
        self.access.drain_actions(&mut requests);
        for request in requests {
            match request.action {
                Action::Click => {
                    if let Some(control) = clicked(request.target) {
                        self.click(control)?;
                    }
                }
                Action::Focus => self.click(Control::Field)?,
                _ => {}
            }
        }
        Ok(())
    }
}
