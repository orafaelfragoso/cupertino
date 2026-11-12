//! The `cupertino-keyring-prompter` process: the prompter on the session bus, and the prompt on the server's
//! connection, drawn as the authentication prompt is: a scrim on the overlay layer of every output and the panel above
//! them, which takes the keyboard exclusively. While the session is locked or isn't the active one nothing is shown.

use std::error::Error;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_geometry::Rect;
use cupertino_polkit_agent::keyring::access::{keyring_tree, requested};
use cupertino_polkit_agent::keyring::model::{Caller, Phase};
use cupertino_polkit_agent::keyring::panel::{Command, Control, Frames, Panel, PanelKey};
use cupertino_polkit_agent::keyring::prompter::{self, Host, Owned, Request};
use cupertino_polkit_agent::keyring::render::Layout;
use cupertino_polkit_agent::panel::RADIUS;
use cupertino_polkit_agent::render::{
    panel_material, scrim_color, scrim_opacity, Painter, Palette,
};
use cupertino_polkit_agent::surfaces;
use cupertino_protocol_app::{AppEffects, AppEvent, Appearance as AppearanceObserver, Backdrop};
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
/// How often the spinner turns a spoke while the caller checks the answer.
const SPINNER_STEP: Duration = Duration::from_millis(83);

/// What the other threads tell the loop.
enum Message {
    Keyring(Request),
    Active(std::result::Result<bool, String>),
}

#[derive(Clone)]
struct Notifier {
    sender: Sender<Message>,
    waker: cupertino_wayland_client::Waker,
}

impl Notifier {
    fn send(&self, message: Message) {
        if self.sender.send(message).is_ok() {
            self.waker.wake();
        }
    }
}

struct WakeLoop(cupertino_wayland_client::Waker);

impl Wake for WakeLoop {
    fn wake(self: Arc<Self>) {
        self.0.wake();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.wake();
    }
}

/// The prompt on screen: its surfaces, its focus and its layout.
struct Shown {
    caller: Caller,
    asked: u32,
    scrims: Vec<(OutputId, LayerSurface, ShmPresenter)>,
    /// The compositor's glass under the panel; dropped before the panel's surface.
    backdrop: Backdrop,
    layer: LayerSurface,
    presenter: ShmPresenter,
    panel: Panel,
    layout: Layout,
    since: Instant,
    /// When the answer went to the caller, for the spinner.
    checking: Option<Instant>,
}

impl Shown {
    fn surfaces(&self) -> impl Iterator<Item = SurfaceId> + '_ {
        self.scrims
            .iter()
            .map(|(_, layer, _)| layer.surface())
            .chain([self.layer.surface()])
    }

    fn configured(&self) -> bool {
        self.presenter.size().is_some() && self.scrims.iter().all(|(_, _, p)| p.size().is_some())
    }
}

struct App {
    conn: Connection,
    layer_shell: LayerShell,
    effects: AppEffects,
    _appearance: AppearanceObserver,
    theme: Theme,
    painter: Painter,
    dark: bool,
    increase_contrast: bool,
    reduce_motion: bool,
    reduce_transparency: bool,
    locked: bool,
    active: bool,
    host: Host,
    messages: Receiver<Message>,
    shown: Option<Shown>,
    access: AtspiAdapter,
    access_active: bool,
}

/// Connect, take the prompter's names and serve until a connection fails.
pub fn run() -> Result<()> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let painter = Painter::new(
        Palette::from_tokens(&theme.tokens, Appearance::Light)?,
        &assets,
    )?;
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

    // logind tells whether the session is the active one; without it (a nested session) it counts as active
    let active = match zbus::blocking::Connection::system()
        .and_then(|system| Session::find(&system))
    {
        Ok(session) => {
            let watcher = notifier.clone();
            session.watch_active(move |active| watcher.send(Message::Active(active)))?;
            eprintln!(
                "cupertino-keyring-prompter: shown only while logind session {} is active",
                session.id
            );
            session.active()?
        }
        Err(error) => {
            eprintln!("cupertino-keyring-prompter: no logind session ({error}); the session counts as active");
            true
        }
    };
    let bus = zbus::blocking::Connection::session()?;
    let to_loop = notifier.clone();
    let owned = prompter::serve(&bus, move |request| to_loop.send(Message::Keyring(request)))?;
    match owned {
        Owned::Now => eprintln!("cupertino-keyring-prompter: serving the keyring's prompts"),
        Owned::Queued => eprintln!(
            "cupertino-keyring-prompter: another prompter holds the names; serving once it leaves"
        ),
    }
    let mut app = App {
        conn,
        layer_shell,
        effects,
        _appearance: appearance,
        theme,
        painter,
        dark: false,
        increase_contrast: false,
        reduce_motion: false,
        reduce_transparency: false,
        // until the server tells, which it does as soon as the observer is created
        locked: true,
        active,
        host: Host::new(bus),
        messages,
        shown: None,
        access,
        access_active: false,
    };
    app.apply_away();
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

fn panel_key(
    code: u32,
    mods: cupertino_wayland_client::Modifiers,
    character: Option<char>,
) -> Option<PanelKey> {
    Some(match code {
        key::TAB => PanelKey::Tab { back: mods.shift },
        key::SPACE => PanelKey::Space,
        key::ENTER | key::KEYPAD_ENTER => PanelKey::Return,
        key::ESCAPE => PanelKey::Escape,
        key::BACKSPACE => PanelKey::Backspace,
        _ if mods.ctrl || mods.alt || mods.logo => return None,
        _ => PanelKey::Char(character?),
    })
}

impl App {
    fn appearance(&self) -> Appearance {
        Appearance::of_session(self.dark, self.increase_contrast)
    }

    fn apply_away(&mut self) {
        let away = self.locked || !self.active;
        let effects = self.host.model.set_away(away);
        self.host.apply(effects);
    }

    fn timeout(&self) -> Option<Duration> {
        let shown = self.shown.as_ref()?;
        if scrim_opacity(shown.since.elapsed(), self.reduce_motion) < 1.0 {
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
                    self.command(command);
                }
                // Caps Lock changes the lines under the fields
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
                    .and_then(|s| s.layout.frames.control_at(position));
                if let Some(control) = control {
                    self.click(control);
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
                self.reduce_transparency = options.reduce_transparency;
                self.dress()?;
                self.repaint()?;
            }
            AppEvent::SessionLock { locked } => {
                self.locked = *locked;
                self.access.session_locked(*locked);
                self.apply_away();
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
            // the server closes the panel only when it can't show it: the question is cancelled
            ShellEvent::Closed(surface) if shown.layer.surface() == surface => {
                eprintln!("cupertino-keyring-prompter: the server closed the prompt");
                let effects = self.host.model.cancel();
                self.host.apply(effects);
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
            Message::Keyring(request) => {
                self.host.request(request);
                Ok(())
            }
            Message::Active(Ok(active)) => {
                self.active = active;
                self.apply_away();
                Ok(())
            }
            Message::Active(Err(error)) => Err(error.into()),
        }
    }

    fn command(&mut self, command: Command) {
        let effects = match command {
            Command::Type(field, key) => {
                self.host.model.key(field, key);
                Vec::new()
            }
            Command::ToggleChoice => {
                self.host.model.toggle_choice();
                Vec::new()
            }
            Command::Continue => self.host.model.answer(),
            Command::Cancel => self.host.model.cancel(),
            Command::Focused => Vec::new(),
        };
        // a mismatched confirmation gives the confirmation the keyboard again
        if let (Some(prompt), Some(shown)) = (self.host.model.shown(), &mut self.shown) {
            if prompt.mismatch && shown.panel.order().contains(&Control::Confirm) {
                shown.panel.focus = Control::Confirm;
            }
        }
        self.host.apply(effects);
        self.relayout();
    }

    fn click(&mut self, control: Control) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        let command = shown.panel.click(control);
        self.command(command);
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

    /// Show the model's prompt, give a new question a fresh panel, or take the prompt away when there is none.
    fn sync(&mut self) -> Result<()> {
        let wanted = self.host.model.shown().map(|p| (p.caller.clone(), p.asked));
        let current = self.shown.as_ref().map(|s| (s.caller.clone(), s.asked));
        if wanted == current {
            return Ok(());
        }
        if let (Some((caller, asked)), Some(shown)) = (&wanted, &mut self.shown) {
            if *caller == shown.caller {
                // the same caller asks again: the surfaces stay, the focus starts over
                let prompt = self.host.model.shown().expect("checked");
                shown.panel = Panel::new(prompt);
                shown.asked = *asked;
                self.relayout();
                return Ok(());
            }
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
        let caps_lock = self.conn.modifiers().caps_lock;
        let prompt = self.host.model.shown().expect("a prompt to show");
        let layout = Layout::new(&mut self.painter, prompt, caps_lock);
        let panel = Panel::new(prompt);
        let (caller, asked) = (prompt.caller.clone(), prompt.asked);
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
        let size = (layout.frames.width as u32, layout.frames.height as u32);
        let layer = self
            .layer_shell
            .layer_surface(&mut self.conn, &surfaces::panel(size))?;
        let backdrop = self.effects.backdrop(&self.conn, layer.surface())?;
        self.shown = Some(Shown {
            caller,
            asked,
            scrims,
            backdrop,
            presenter: ShmPresenter::new(layer.surface()),
            layer,
            panel,
            layout,
            since: Instant::now(),
            checking: None,
        });
        self.dress()?;
        self.relayout();
        Ok(())
    }

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

    /// Lay the panel out again for what it shows now, resize its surface when that changed, and draw it.
    fn relayout(&mut self) {
        let caps_lock = self.conn.modifiers().caps_lock;
        let Some(prompt) = self.host.model.shown() else {
            return;
        };
        let Some(shown) = &mut self.shown else {
            return;
        };
        let layout = Layout::new(&mut self.painter, prompt, caps_lock);
        if layout.frames.height != shown.layout.frames.height {
            let size = (layout.frames.width as u32, layout.frames.height as u32);
            shown.layer.set_placement(Anchor::empty(), size);
            if let Err(e) = shown
                .backdrop
                .set_shape(&panel_shape(&layout.frames), RADIUS)
            {
                eprintln!("cupertino-keyring-prompter: the panel's glass keeps its shape: {e}");
            }
        }
        shown.layout = layout;
        let checking = prompt.phase == Phase::Checking;
        shown.checking = match (checking, shown.checking) {
            (true, None) => Some(Instant::now()),
            (true, since) => since,
            (false, _) => None,
        };
        shown.presenter.request_redraw();
    }

    /// Give the panel the compositor's glass in its shape, or none where the session draws materials opaque.
    fn dress(&mut self) -> Result<()> {
        let material = panel_material(self.reduce_transparency, self.increase_contrast);
        self.painter.glass = material.is_some();
        if let Some(shown) = &mut self.shown {
            match material {
                Some(name) => {
                    shown.backdrop.set_material(name);
                    shown
                        .backdrop
                        .set_shape(&panel_shape(&shown.layout.frames), RADIUS)?;
                }
                None => shown
                    .backdrop
                    .set_shape(&Rect::new(0.0, 0.0, 0.0, 0.0), 0.0)?,
            }
            shown.presenter.request_redraw();
        }
        Ok(())
    }

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
        let opacity = scrim_opacity(shown.since.elapsed(), self.reduce_motion);
        let palette = self.painter.palette;
        for (_, _, presenter) in &mut shown.scrims {
            if opacity < 1.0 {
                presenter.request_redraw();
            }
            loop {
                match presenter.poll(&self.conn)? {
                    Ready::Idle => break,
                    Ready::ScaleChanged(_) => {}
                    Ready::Draw => {
                        presenter.draw_solid(&mut self.conn, scrim_color(&palette, opacity))?
                    }
                }
            }
        }
        if shown.checking.is_some() {
            shown.presenter.request_redraw();
        }
        let Some(prompt) = self.host.model.shown() else {
            return Ok(());
        };
        let elapsed = shown
            .checking
            .map_or(0.0, |since| since.elapsed().as_secs_f64());
        let view = shown.layout.view(&shown.panel, prompt);
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
                            painter.draw_keyring(&view, pixmap, scale.factor(), elapsed)
                        })?;
                    drew = true;
                }
            }
        }
        if drew && self.access.is_active() {
            let tree = keyring_tree(PANEL_WINDOW, &view);
            self.access.window_focus(PANEL_WINDOW, true);
            self.access.update(&tree);
        }
        Ok(())
    }

    /// Tell assistive technology the panel again when it starts listening, and do what it asks.
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
            let Some(control) = requested(request.target) else {
                continue;
            };
            match (request.action, control) {
                (Action::Focus, Control::Password | Control::Confirm)
                | (Action::Click, Control::Choice | Control::Cancel | Control::Continue) => {
                    self.click(control)
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The whole panel, in its own points.
fn panel_shape(frames: &Frames) -> Rect {
    Rect::new(0.0, 0.0, frames.width, frames.height)
}
