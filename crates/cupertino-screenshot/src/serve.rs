//! The portal's service: `cupertino-screenshot --serve`, which the server starts for the portal
//! (`org.cupertino.Session1.StartScreenshot`) on a connection with the capture role. It serves
//! `org.cupertino.Screenshot1` ([`crate::control`]) on its own session bus connection: a colour pick shows the
//! outputs, copied when the call came, under an overlay on each ([`crate::color_picker`]); a screen-sharing choice is
//! a window of its own ([`crate::share_picker`]). It exits once it has had nothing to do for [`IDLE_EXIT`], and when
//! its bus connection goes.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cupertino_geometry::{Rect, Size};
use cupertino_kit::core::{
    next_resource_id, Application, Command, RendererClass, Window, WindowKey,
};
use cupertino_kit::render::ImageResource;
use cupertino_kit_shell::bus;
use cupertino_kit_shell::capture::{self, Source};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    Anchor, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};
use zbus::Connection;

use crate::color_picker::{self, Display, PickInput, Picked, Picking};
use crate::control::{self, Answer, Request};
use crate::share_picker::{self, Picker};

/// How long the service waits for its next call, once nothing is shown, before it exits; the portal starts it again
/// for the next one.
pub const IDLE_EXIT: Duration = Duration::from_secs(30);
/// Layer namespace of the colour picker's overlays: the capture overlay's ([`crate::shot::OVERLAY_NAMESPACE`]), so the
/// server starts no screenshot while a pick is shown, as while a capture's overlay is: the pick's overlay holds the
/// keyboard, and a capture's overlay over it would take a copy of the frozen screen.
pub const PICK_NAMESPACE: &str = crate::shot::OVERLAY_NAMESPACE;

/// How the service ended, for [`serve`] to return.
static OUTCOME: Mutex<Option<Result<(), String>>> = Mutex::new(None);

/// Serve `org.cupertino.Screenshot1` on the connection the server started the process with, until it is idle or its
/// bus connection goes.
pub fn serve() -> Result<(), String> {
    OUTCOME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let environment = cupertino_kit::app::environment().map_err(|e| e.to_string())?;
    cupertino_kit_shell::run::<Serve>(environment).map_err(|e| e.to_string())?;
    OUTCOME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or(Ok(()))
}

/// The service.
pub struct Serve;

/// The colour pick under way.
struct Pick {
    handle: String,
    reply: async_channel::Sender<Answer<[f64; 3]>>,
    /// The outputs as the pick copied them: their ids and places.
    outputs: Vec<(OutputId, Rect)>,
    /// Their pictures while they are copied.
    frames: Vec<Option<ImageResource>>,
    /// Made once every output is copied.
    picking: Option<Picking>,
}

/// A sharing choice on screen.
struct Share {
    handle: String,
    picker: Picker,
    reply: async_channel::Sender<Answer<(usize, bool)>>,
}

pub struct Model {
    calls: (
        async_channel::Sender<Request>,
        async_channel::Receiver<Request>,
    ),
    /// Kept for as long as the service runs: it owns the name.
    connection: Option<Connection>,
    pick: Option<Pick>,
    shares: Vec<Share>,
    next_share: u64,
    /// Counts the idle timers; one scheduled before the latest is ignored when it fires.
    idle: u64,
    /// The latest idle timer is running.
    idling: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Session(Option<Connection>),
    SessionClosed,
    Served(Result<(), String>),
    Call(Request),
    /// A copy of the pick's output, by its place among them.
    Frame(usize, Result<ImageResource, String>),
    Pick(PickInput),
    /// The overlay of the pick's output `0` presented its first frame.
    PickShown(usize),
    Share(share_picker::Input),
    ShareShown(u64),
    ShareArmed(u64),
    Outputs,
    Closed(WindowKey),
    Idle(u64),
}

/// The next call `org.cupertino.Screenshot1` accepted.
fn listen(calls: &async_channel::Receiver<Request>) -> Command<Message> {
    let calls = calls.clone();
    Command::Perform(Box::pin(async move {
        match calls.recv().await {
            Ok(request) => Message::Call(request),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The server's outputs that have a place in the layout.
fn layout() -> Vec<(OutputId, Rect)> {
    outputs::all()
        .into_iter()
        .filter_map(|o| o.geometry.map(|rect| (o.id, rect)))
        .collect()
}

fn end(message: &str) -> Vec<Command<Message>> {
    eprintln!("cupertino-screenshot: {message}");
    vec![Command::Exit]
}

impl Model {
    fn busy(&self) -> bool {
        self.pick.is_some() || !self.shares.is_empty()
    }

    /// The idle timer, started when nothing is shown and none runs.
    fn idle_timer(&mut self) -> Vec<Command<Message>> {
        if self.busy() {
            self.idle += 1;
            self.idling = false;
            return Vec::new();
        }
        if self.idling {
            return Vec::new();
        }
        self.idle += 1;
        self.idling = true;
        vec![Command::After {
            delay: IDLE_EXIT,
            message: Message::Idle(self.idle),
        }]
    }

    fn call(&mut self, request: Request) -> Vec<Command<Message>> {
        match request {
            Request::PickColor { handle, reply } => self.pick_color(handle, reply),
            Request::SelectSources { handle, ask, reply } => {
                let id = self.next_share;
                self.next_share += 1;
                self.shares.push(Share {
                    handle,
                    picker: Picker::new(id, ask),
                    reply,
                });
                vec![Command::OnPresented {
                    window: share_picker::window_key(id),
                    message: Message::ShareShown(id),
                }]
            }
            Request::Close { handle } => {
                // the dropped replies answer nothing the portal still waits for
                if self.pick.as_ref().is_some_and(|p| p.handle == handle) {
                    self.pick = None;
                }
                self.shares.retain(|s| s.handle != handle);
                Vec::new()
            }
        }
    }

    /// A colour pick starts with a copy of every output; one runs at a time.
    fn pick_color(
        &mut self,
        handle: String,
        reply: async_channel::Sender<Answer<[f64; 3]>>,
    ) -> Vec<Command<Message>> {
        if self.pick.is_some() {
            let _ = reply.try_send(Answer::Failed("a colour pick is already shown".into()));
            return Vec::new();
        }
        let outputs = layout();
        if outputs.is_empty() {
            let _ = reply.try_send(Answer::Failed("the server has no output".into()));
            return Vec::new();
        }
        let commands = outputs
            .iter()
            .enumerate()
            .map(|(i, (id, _))| {
                let options = capture::Options {
                    cursor: false,
                    region: None,
                };
                capture::once(Source::Output(*id), options, move |frame| {
                    Message::Frame(i, frame.map_err(|e| e.to_string()))
                })
            })
            .collect();
        self.pick = Some(Pick {
            handle,
            reply,
            frames: vec![None; outputs.len()],
            outputs,
            picking: None,
        });
        commands
    }

    /// Ends the pick with `answer`.
    fn picked(&mut self, answer: Answer<[f64; 3]>) {
        if let Some(pick) = self.pick.take() {
            let _ = pick.reply.try_send(answer);
        }
    }

    fn frame(
        &mut self,
        index: usize,
        frame: Result<ImageResource, String>,
    ) -> Vec<Command<Message>> {
        let Some(pick) = self.pick.as_mut() else {
            return Vec::new();
        };
        let image = match frame {
            Ok(image) => image,
            Err(e) => {
                self.picked(Answer::Failed(format!("an output was not copied: {e}")));
                return Vec::new();
            }
        };
        if let Some(slot) = pick.frames.get_mut(index) {
            *slot = Some(image);
        }
        if pick.frames.iter().any(Option::is_none) {
            return Vec::new();
        }
        let displays = std::mem::take(&mut pick.frames)
            .into_iter()
            .flatten()
            .zip(&pick.outputs)
            .map(|(image, (_, rect))| Display {
                rect: *rect,
                image,
                id: next_resource_id(),
            })
            .collect::<Vec<_>>();
        let count = displays.len();
        pick.picking = Some(Picking::new(displays));
        (0..count)
            .map(|i| Command::OnPresented {
                window: color_picker::overlay_key(i),
                message: Message::PickShown(i),
            })
            .collect()
    }

    fn pick_input(&mut self, input: PickInput) {
        let Some(picking) = self.pick.as_mut().and_then(|p| p.picking.as_mut()) else {
            return;
        };
        let picked = match input {
            PickInput::Moved(at) => {
                picking.moved(at);
                None
            }
            PickInput::Left => {
                picking.left();
                None
            }
            PickInput::Button { pressed, at } => picking.button(pressed, at, Instant::now()),
            PickInput::Escape => Some(Picked::Cancelled),
        };
        if let Some(picked) = picked {
            self.picked(picked.into());
        }
    }

    fn share(&mut self, input: share_picker::Input) {
        let id = share_picker::input_id(&input);
        let Some(at) = self.shares.iter().position(|s| s.picker.id == id) else {
            return;
        };
        if let Some(answer) = self.shares[at].picker.input(input) {
            let share = self.shares.remove(at);
            let _ = share.reply.try_send(match answer {
                Some(chosen) => Answer::Done(chosen),
                None => Answer::Cancelled,
            });
        }
    }

    /// The server closed window `key`: what it showed ends unanswered.
    fn closed(&mut self, key: &WindowKey) {
        let pick_key = self
            .pick
            .as_ref()
            .and_then(|p| p.picking.as_ref())
            .is_some_and(|p| (0..p.displays().len()).any(|i| color_picker::overlay_key(i) == *key));
        if pick_key {
            self.picked(Answer::Failed("the server closed the colour picker".into()));
        }
        if let Some(at) = self
            .shares
            .iter()
            .position(|s| share_picker::window_key(s.picker.id) == *key)
        {
            let share = self.shares.remove(at);
            let _ = share.reply.try_send(Answer::Cancelled);
        }
    }
}

impl Application for Serve {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The pickers are shown for a moment, mostly at rest: a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let calls = async_channel::unbounded();
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(listen(&calls.1));
        let mut model = Model {
            calls,
            connection: None,
            pick: None,
            shares: Vec::new(),
            next_share: 0,
            idle: 0,
            idling: false,
        };
        commands.extend(model.idle_timer());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let mut commands = match message {
            Message::Session(Some(connection)) => {
                model.connection = Some(connection.clone());
                let calls = model.calls.0.clone();
                vec![Command::Perform(Box::pin(async move {
                    Message::Served(
                        control::serve(&connection, calls)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                }))]
            }
            Message::Session(None) | Message::SessionClosed => {
                *OUTCOME.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(Err("no session bus connection".into()));
                return end("no session bus connection");
            }
            Message::Served(Ok(())) => Vec::new(),
            Message::Served(Err(e)) => {
                let e = format!("{} is not served: {e}", control::BUS_NAME);
                *OUTCOME.lock().unwrap_or_else(PoisonError::into_inner) = Some(Err(e.clone()));
                return end(&e);
            }
            Message::Call(request) => {
                let mut commands = model.call(request);
                commands.push(listen(&model.calls.1));
                commands
            }
            Message::Frame(index, frame) => model.frame(index, frame),
            Message::Pick(input) => {
                model.pick_input(input);
                Vec::new()
            }
            Message::PickShown(index) => {
                if let Some(picking) = model.pick.as_mut().and_then(|p| p.picking.as_mut()) {
                    picking.shown(index, Instant::now());
                }
                Vec::new()
            }
            Message::Share(input) => {
                model.share(input);
                Vec::new()
            }
            Message::ShareShown(id) => {
                let shown = model
                    .shares
                    .iter_mut()
                    .find(|s| s.picker.id == id)
                    .is_some_and(|s| s.picker.shown(Instant::now()));
                if shown {
                    vec![Command::After {
                        delay: color_picker::ARMING,
                        message: Message::ShareArmed(id),
                    }]
                } else {
                    Vec::new()
                }
            }
            Message::ShareArmed(id) => {
                if let Some(share) = model.shares.iter_mut().find(|s| s.picker.id == id) {
                    share.picker.arm();
                }
                Vec::new()
            }
            Message::Outputs => {
                // a click on an output without an overlay would reach the app under it
                let changed = model
                    .pick
                    .as_ref()
                    .is_some_and(|p| color_picker::layout_changed(&p.outputs, &layout()));
                if changed {
                    model.picked(Answer::Failed(
                        "the outputs changed during the colour pick".into(),
                    ));
                }
                Vec::new()
            }
            Message::Closed(key) => {
                model.closed(&key);
                Vec::new()
            }
            Message::Idle(idle) => {
                if idle == model.idle && !model.busy() {
                    return vec![Command::Exit];
                }
                Vec::new()
            }
        };
        commands.extend(model.idle_timer());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = Vec::new();
        if let Some(picking) = model.pick.as_ref().and_then(|p| p.picking.as_ref()) {
            for i in 0..picking.displays().len() {
                if let Some(view) = picking.view(i, i == 0) {
                    windows.push(color_picker::overlay(
                        color_picker::overlay_key(i),
                        view,
                        Message::Pick,
                    ));
                }
            }
        }
        windows.extend(
            model
                .shares
                .iter()
                .map(|s| share_picker::window(&s.picker, Message::Share)),
        );
        windows
    }
}

impl Program for Serve {
    /// An overlay on each output the pick copied, over everything and the exclusive zones, the first taking the
    /// keyboard for Escape; the sharing choice is a window.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let index = color_picker::overlay_index(key);
        let output = index.and_then(|i| model.pick.as_ref().and_then(|p| p.outputs.get(i)));
        match (index, output) {
            (Some(index), Some((output, _))) => Surface {
                role: Role::Layer(LayerSpec {
                    namespace: PICK_NAMESPACE,
                    output: Some(*output),
                    layer: Layer::Overlay,
                    anchor: Anchor {
                        top: true,
                        bottom: true,
                        left: true,
                        right: true,
                    },
                    size: Size::ZERO,
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: if index == 0 {
                        Keyboard::Exclusive
                    } else {
                        Keyboard::None
                    },
                }),
                persistent: None,
                holes: Vec::new(),
            },
            _ => Surface {
                role: Role::Window,
                persistent: None,
                holes: Vec::new(),
            },
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(key) => Some(Message::Closed(key)),
            _ => None,
        }
    }
}
