//! The window manager as a component of the shell host (`cupertino-shell`): the stage policy of every output
//! ([`Controller`], on the window manager role's feed) and the stage strip, a surface of its own along each output's
//! left edge ([`strip`](crate::strip)), whose thumbnails are holes claiming the strip's windows.
//!
//! - **The stage.** Off, the component places nothing and shows no strip. On (`windowmanager.conf`, or the server's
//!   `toggle_stage`), the windows of the most recently active application are on the stage, and the other
//!   applications' windows are thumbnails in the strip, one group per application; the groups beyond the strip's rows
//!   are hidden. A press on a thumbnail, or a window focused any other way, brings its group to the stage.
//! - **A change is one transaction.** The strip's surface is staged with the layer animations of its holes, and the
//!   placements (windows hidden, or shown again) and the activation are window changes of the same part, so the server
//!   starts them on one frame. A window going to the strip is taken over by a hole laid out where the window shows,
//!   which then moves to its slot along [`SpringPreset::STAGE_SWAP`]; a window coming back moves in its hole to its
//!   own frame, and the hole goes when the motion ended, so the window shows there again with no change on screen. A
//!   thumbnail fades in or out along [`SpringPreset::STAGE_FADE`] as its group gains or loses a row, the server hiding
//!   the window under it. Under Reduce Motion the holes move at once, and a window going to the strip fades in there.
//! - **A window that leaves.** Its hole stays until the server says it closed, which fades it out, or for
//!   [`DEPARTED_GRACE`] when it was minimized or moved; the strip closes up around it.
//! - **A restart.** The server holds the strip of the instance before, with its holes and the hidden windows. A new
//!   instance that hears the strip is held turns the stage on whatever the file says, lays its strip out at once with
//!   its placements committed in the update that first shows it, so it takes the hold over with nothing moving.
//!
//! The windows' frames are global; the component subtracts the output's origin.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use cupertino_kit::core::{Application, Command as Do, Key, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::motion::{SpringPreset, Timing};
use cupertino_kit::platform::{LayerChange, LayerProperty, WindowRequest};
use cupertino_kit_shell::bus::{self, Scene as BusScene};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::transaction::{self, MAX_DEADLINE};
use cupertino_kit_shell::window_manager::{self, Placement};
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Hole, HoleSource, Keyboard, Layer, LayerSpec, Margin,
    Program, Role, ServerWindowId, Surface,
};
use cupertino_wm::geometry::Rect as WmRect;
use cupertino_wm::stage::StageConfig;

use crate::conf;
use crate::controller::{Change, Controller, Request, Window as Arranged};
use crate::strip::{self, Scene, Thumb};

/// How long the hole of a window that left the arranged set waits for the server's word that the window closed: a
/// minimized window, or one moved to another desktop, shows nothing in it.
pub const DEPARTED_GRACE: Duration = Duration::from_millis(500);

/// The transactions a connection may have that have not ended (`shell-host.md`, Limits): past it a change applies
/// at once, outside a transaction.
const MAX_OPEN_TRANSACTIONS: usize = 16;

/// The window manager.
pub struct WindowManager;

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// Whether the stage is on at start.
    Setting(bool),
    /// The arranged windows and the usable area of an output, and the stage shortcut.
    Feed(OutputId, window_manager::Update),
    Outputs,
    /// Reduce Motion.
    ReduceMotion(bool),
    /// The left button went down at this point of the output's strip.
    Pressed(OutputId, Point),
    /// Assistive technology clicked the thumbnail of this window.
    Pick(OutputId, u32),
    /// The motions of the change numbered so ended on the output.
    Settled(OutputId, u64),
    /// The server says the window of the hole on this layer of the strip closed.
    HoleClosed(WindowKey, u64),
    /// The grace of the hole of a window that left the set ended.
    Departed(OutputId, u32),
    /// The server closed the strip's surface.
    Closed(WindowKey),
}

/// A thumbnail's hole, in the output's points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HoleState {
    /// Where its layer is laid out: where its last motion started, until that motion ended.
    layout: Rect,
    /// Where its layer rests once its motion ended.
    target: Rect,
    /// It goes once its motion ended: its window went back to the stage or was hidden, or the stage turned off.
    leaving: bool,
    /// Its window left the arranged set: it waits for the server's word or [`DEPARTED_GRACE`].
    departed: bool,
}

/// The stage of one output.
struct Output {
    id: OutputId,
    /// The output's origin in global points, and its height.
    origin: (f64, f64),
    height: f64,
    controller: Controller,
    /// The arranged windows as the server last described them.
    windows: BTreeMap<u32, window_manager::Window>,
    usable: Option<Rect>,
    /// The first feed was arranged.
    arranged: bool,
    /// The strip's window, while the stage is on or a hole is still on its way.
    strip: Option<WindowKey>,
    /// The strip window's width in points: the strip's column.
    width: f64,
    holes: BTreeMap<u32, HoleState>,
    /// Counts the changes, so the end of an earlier one's motion is told from the last's.
    generation: u64,
    /// The submitted transactions and the change each belongs to, kept until its motion ended: a creator that let go
    /// of one before it started would cancel it.
    handles: Vec<(u64, transaction::Handle)>,
}

pub struct Model {
    start: Instant,
    session: Option<zbus::Connection>,
    /// Whether the stage is on at start: `windowmanager.conf`, or a strip the server holds from the instance before;
    /// `None` until the file was read.
    stage_at_start: Option<bool>,
    reduce_motion: bool,
    outputs: HashMap<OutputId, Output>,
    /// What the server said before the setting was known, in order.
    waiting: Vec<(OutputId, window_manager::Update)>,
    next_strip: u64,
}

/// `r` on the model's rectangle.
fn wm(r: Rect) -> WmRect {
    WmRect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// The changes that take a layer to `r` along `timing`.
fn frame_changes(r: Rect, timing: Option<Timing>) -> Vec<LayerChange> {
    [
        (LayerProperty::X, r.origin.x),
        (LayerProperty::Y, r.origin.y),
        (LayerProperty::Width, r.size.width),
        (LayerProperty::Height, r.size.height),
    ]
    .into_iter()
    .map(|(p, v)| LayerChange::new(p, v, timing))
    .collect()
}

/// How long after a change its motions have ended: the slower spring's settling, after the latest start the
/// transaction may take.
fn settle() -> Duration {
    let swap: Timing = SpringPreset::STAGE_SWAP.into();
    let fade: Timing = SpringPreset::STAGE_FADE.into();
    Duration::from_secs_f64(swap.duration().max(fade.duration())) + MAX_DEADLINE
}

/// Send `requests` to the window manager of `output`, in order.
fn send(output: OutputId, requests: &[Request]) {
    for request in requests {
        match *request {
            Request::Place { id, hidden } => window_manager::place(
                output,
                ServerWindowId(id),
                if hidden {
                    Placement::Hidden
                } else {
                    Placement::Stage
                },
            ),
            Request::Commit => window_manager::commit(output),
            Request::Activate(id) => window_manager::activate_window(output, ServerWindowId(id)),
        }
    }
}

/// Read the stage setting on the I/O pool; `held` turns it on whatever the file says.
fn read_setting(held: bool) -> Do<Message> {
    cupertino_kit::task::io(
        cupertino_kit::task::SETTINGS_READ_DEADLINE,
        conf::read_stage,
        move |stage| Message::Setting(held || stage),
    )
}

impl Output {
    fn new(id: OutputId, origin: (f64, f64), height: f64, stage: bool) -> Self {
        Output {
            id,
            origin,
            height,
            controller: Controller::new(StageConfig::default(), stage),
            windows: BTreeMap::new(),
            usable: None,
            arranged: false,
            strip: None,
            width: 0.0,
            holes: BTreeMap::new(),
            generation: 0,
            handles: Vec::new(),
        }
    }

    /// `r`, a global rectangle, in the output's points.
    fn local(&self, r: WmRect) -> Rect {
        Rect::new(r.x - self.origin.0, r.y - self.origin.1, r.w, r.h)
    }

    /// The changes `update` brings against what the server said before; the holes of windows that left the set
    /// start waiting, each with its grace.
    fn changes(&mut self, update: window_manager::Update) -> (Vec<Change>, Vec<Do<Message>>) {
        let mut changes = Vec::new();
        if let Some(area) = update.usable_area.filter(|a| self.usable != Some(*a)) {
            self.usable = Some(area);
            changes.push(Change::UsableArea(wm(area)));
        }
        let windows: BTreeMap<u32, window_manager::Window> =
            update.windows.into_iter().map(|w| (w.id.0, w)).collect();
        for (id, w) in &windows {
            if self.windows.get(id) != Some(w) {
                changes.push(Change::Window(Arranged {
                    id: *id,
                    app_id: w.app_id.clone(),
                    frame: wm(w.frame),
                    active: w.active,
                }));
            }
        }
        let mut graces = Vec::new();
        for id in self.windows.keys().filter(|id| !windows.contains_key(id)) {
            changes.push(Change::WindowClosed(*id));
            if let Some(hole) = self.holes.get_mut(id) {
                hole.departed = true;
                graces.push(Do::After {
                    delay: DEPARTED_GRACE,
                    message: Message::Departed(self.id, *id),
                });
            }
        }
        self.windows = windows;
        if update.toggle_stage {
            changes.push(Change::ToggleStage);
        }
        (changes, graces)
    }

    /// The holes the controller's strip calls for, against the ones shown: the layer changes that take each where it
    /// goes. `was_hidden` are the windows hidden before the change; with `at_once` nothing moves (the first
    /// arrangement, which takes a held strip over).
    fn place_holes(
        &mut self,
        was_hidden: &BTreeSet<u32>,
        at_once: bool,
        reduce_motion: bool,
    ) -> Vec<(u64, Vec<LayerChange>)> {
        // under Reduce Motion a hole moves at once, and one that comes to the strip fades in there
        let swap: Option<Timing> = (!reduce_motion).then(|| SpringPreset::STAGE_SWAP.into());
        let fade: Timing = SpringPreset::STAGE_FADE.into();
        let wanted: BTreeMap<u32, Rect> = self
            .controller
            .strip()
            .into_iter()
            .map(|(id, r)| (id, self.local(r)))
            .collect();
        let natural: BTreeMap<u32, Rect> = self
            .windows
            .iter()
            .map(|(id, w)| (*id, self.local(wm(w.frame))))
            .collect();
        let mut layers = Vec::new();
        for (&id, &rect) in &wanted {
            match self.holes.get_mut(&id) {
                Some(hole) if hole.departed => {}
                Some(hole) => {
                    if hole.target != rect || hole.leaving {
                        let mut changes = frame_changes(rect, swap);
                        if hole.leaving {
                            changes.push(LayerChange::new(LayerProperty::Opacity, 1.0, Some(fade)));
                        }
                        hole.target = rect;
                        hole.leaving = false;
                        layers.push((strip::hole_layer(id), changes));
                    }
                }
                None => {
                    // a window on the stage is taken over where it shows; a hidden one fades in at its slot
                    let from = natural
                        .get(&id)
                        .copied()
                        .filter(|_| !at_once && !was_hidden.contains(&id));
                    let start = from.unwrap_or(rect);
                    self.holes.insert(
                        id,
                        HoleState {
                            layout: start,
                            target: rect,
                            leaving: false,
                            departed: false,
                        },
                    );
                    if at_once {
                        continue;
                    }
                    let mut changes = Vec::new();
                    if from.is_some() {
                        changes.extend(frame_changes(rect, swap));
                    }
                    if from.is_none() || reduce_motion {
                        changes.push(LayerChange::new(LayerProperty::Opacity, 0.0, None));
                        changes.push(LayerChange::new(LayerProperty::Opacity, 1.0, Some(fade)));
                    }
                    layers.push((strip::hole_layer(id), changes));
                }
            }
        }
        let hidden = self.controller.hidden();
        for (&id, hole) in &mut self.holes {
            if wanted.contains_key(&id) || hole.leaving || hole.departed {
                continue;
            }
            hole.leaving = true;
            match natural.get(&id).filter(|_| !hidden.contains(&id)) {
                // back to the window's own frame on the stage
                Some(&own) => {
                    hole.target = own;
                    layers.push((strip::hole_layer(id), frame_changes(own, swap)));
                }
                // out where it is: the server hides the window under it
                None => layers.push((
                    strip::hole_layer(id),
                    vec![LayerChange::new(LayerProperty::Opacity, 0.0, Some(fade))],
                )),
            }
        }
        layers
    }
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn by_strip(&self, key: &WindowKey) -> Option<OutputId> {
        self.outputs
            .values()
            .find(|o| o.strip.as_ref() == Some(key))
            .map(|o| o.id)
    }

    /// What the server said of `output`: the stage follows it, the strip with it.
    fn feed(&mut self, output: OutputId, update: window_manager::Update) -> Vec<Do<Message>> {
        let Some(stage) = self.stage_at_start else {
            self.waiting.push((output, update));
            return Vec::new();
        };
        let now = self.now();
        if !self.outputs.contains_key(&output) {
            let Some(geometry) = outputs::get(output).and_then(|o| o.geometry) else {
                return Vec::new();
            };
            // an output that comes later follows the stage as it is on the others
            let on = self
                .outputs
                .values()
                .next()
                .map_or(stage, |o| o.controller.is_enabled());
            let origin = (geometry.origin.x, geometry.origin.y);
            self.outputs.insert(
                output,
                Output::new(output, origin, geometry.size.height, on),
            );
        }
        let Some(out) = self.outputs.get_mut(&output) else {
            return Vec::new();
        };
        let (changes, mut commands) = out.changes(update);
        let was_hidden = out.controller.hidden().clone();
        let requests = out.controller.apply(&changes, now);
        commands.extend(self.arrange(output, &was_hidden, requests));
        commands
    }

    /// A change the person made on `output`'s strip.
    fn change(&mut self, output: OutputId, change: Change) -> Vec<Do<Message>> {
        let now = self.now();
        let Some(out) = self.outputs.get_mut(&output) else {
            return Vec::new();
        };
        let was_hidden = out.controller.hidden().clone();
        let requests = out.controller.apply(&[change], now);
        self.arrange(output, &was_hidden, requests)
    }

    /// Carry out what the controller asked for on `output` and move the holes to the strip it now has, as one
    /// transaction; the first arrangement of an output applies at once.
    fn arrange(
        &mut self,
        output: OutputId,
        was_hidden: &BTreeSet<u32>,
        mut requests: Vec<Request>,
    ) -> Vec<Do<Message>> {
        let reduce_motion = self.reduce_motion;
        let Some(out) = self.outputs.get_mut(&output) else {
            return Vec::new();
        };
        let first = !out.arranged;
        out.arranged = true;
        // the first commit replaces whatever the server holds of an instance before, even when this one places nothing
        if first && !requests.contains(&Request::Commit) {
            requests.push(Request::Commit);
        }
        let layers = out.place_holes(was_hidden, first, reduce_motion);
        if out.controller.is_enabled() {
            let column = out.controller.strip_column();
            out.width = (column.right() - out.origin.0).max(1.0);
        }
        if (out.controller.is_enabled() || !out.holes.is_empty()) && out.strip.is_none() {
            self.next_strip += 1;
            out.strip = Some(WindowKey(Key::from(format!("strip-{}", self.next_strip))));
        }
        if requests.is_empty() && layers.is_empty() {
            return Vec::new();
        }
        out.generation += 1;
        if first || out.handles.len() >= MAX_OPEN_TRANSACTIONS {
            send(output, &requests);
        } else {
            let mut part = transaction::create(output);
            if let Some(key) = &out.strip {
                part.stage(key);
            }
            if !requests.is_empty() {
                part.window_changes(|| send(output, &requests));
            }
            bus::publish(bus::Message::Transaction {
                id: part.id(),
                output,
                scene: BusScene::StageSwitch,
            });
            out.handles.push((out.generation, part.submit()));
        }
        let mut commands: Vec<Do<Message>> = match &out.strip {
            Some(key) => layers
                .into_iter()
                .map(|(layer, changes)| {
                    Do::Window(
                        key.clone(),
                        WindowRequest::Layer {
                            key: layer,
                            changes,
                        },
                    )
                })
                .collect(),
            None => Vec::new(),
        };
        commands.push(Do::After {
            delay: settle(),
            message: Message::Settled(output, out.generation),
        });
        commands
    }

    /// The motions of change `generation` on `output` ended: its transactions are let go of; when it is the last
    /// change, the holes on their way out go, the others are laid out where they rest, and a strip of a stage turned
    /// off goes.
    fn settled(&mut self, output: OutputId, generation: u64) {
        let Some(out) = self.outputs.get_mut(&output) else {
            return;
        };
        out.handles.retain(|(g, _)| *g > generation);
        if out.generation != generation {
            return;
        }
        out.holes.retain(|_, h| !h.leaving);
        for hole in out.holes.values_mut().filter(|h| !h.departed) {
            hole.layout = hole.target;
        }
        if !out.controller.is_enabled() && out.holes.is_empty() {
            out.strip = None;
        }
    }

    /// The thumbnail at `at` of `output`'s strip, if one rests there.
    fn thumb_at(&self, output: OutputId, at: Point) -> Option<u32> {
        let out = self.outputs.get(&output)?;
        out.holes
            .iter()
            .find(|(_, h)| !h.leaving && !h.departed && h.target.contains(at))
            .map(|(id, _)| *id)
    }

    /// The outputs changed: the stage of an output that went goes with its strip; the others follow their place.
    fn outputs_changed(&mut self) {
        let all = outputs::all();
        self.outputs.retain(|id, out| {
            let Some(geometry) = all.iter().find(|o| o.id == *id).and_then(|o| o.geometry) else {
                return false;
            };
            out.origin = (geometry.origin.x, geometry.origin.y);
            out.height = geometry.size.height;
            true
        });
    }
}

impl Application for WindowManager {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The strip draws nothing itself: the server draws the windows in its holes, so a GPU context would only cost
    /// memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        let held = cupertino_kit_shell::holds()
            .iter()
            .any(|key| key == strip::PERSIST_KEY);
        commands.push(read_setting(held));
        let model = Model {
            start: Instant::now(),
            session: None,
            stage_at_start: None,
            reduce_motion: false,
            outputs: HashMap::new(),
            waiting: Vec::new(),
            next_strip: 0,
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        match message {
            Message::Session(Some(connection)) => {
                // the connection carries the component's identity and its debug interface
                model.session = Some(connection);
                Vec::new()
            }
            Message::Session(None) | Message::SessionClosed => {
                // the window manager calls nothing on the bus: without it, it still works
                model.session = None;
                Vec::new()
            }
            Message::Setting(on) => {
                model.stage_at_start = Some(on);
                let waiting = std::mem::take(&mut model.waiting);
                waiting
                    .into_iter()
                    .flat_map(|(output, update)| model.feed(output, update))
                    .collect()
            }
            Message::Feed(output, update) => model.feed(output, update),
            Message::Outputs => {
                model.outputs_changed();
                Vec::new()
            }
            Message::ReduceMotion(on) => {
                model.reduce_motion = on;
                Vec::new()
            }
            Message::Pressed(output, at) => match model.thumb_at(output, at) {
                Some(id) => model.change(output, Change::StripPressed(id)),
                None => Vec::new(),
            },
            Message::Pick(output, id) => model.change(output, Change::StripPressed(id)),
            Message::Settled(output, generation) => {
                model.settled(output, generation);
                Vec::new()
            }
            Message::HoleClosed(key, layer) => {
                if let (Some(output), Some(id)) = (model.by_strip(&key), strip::layer_window(layer))
                {
                    if let Some(out) = model.outputs.get_mut(&output) {
                        out.holes.remove(&id);
                    }
                }
                Vec::new()
            }
            Message::Departed(output, id) => {
                if let Some(out) = model.outputs.get_mut(&output) {
                    if out.holes.get(&id).is_some_and(|h| h.departed) {
                        out.holes.remove(&id);
                    }
                }
                Vec::new()
            }
            Message::Closed(key) => {
                // the server closed the strip (its output went): the holes went with it
                if let Some(out) = model
                    .by_strip(&key)
                    .and_then(|output| model.outputs.get_mut(&output))
                {
                    out.strip = None;
                    out.holes.clear();
                }
                Vec::new()
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .outputs
            .values()
            .filter_map(|out| {
                let key = out.strip.clone()?;
                // thumbnails at rest below the ones on their way out, which pass over them
                let mut holes: Vec<(&u32, &HoleState)> = out.holes.iter().collect();
                holes.sort_by_key(|(_, h)| h.leaving);
                let thumbs = holes
                    .iter()
                    .map(|(&id, h)| Thumb {
                        id,
                        rect: h.layout,
                        name: out
                            .windows
                            .get(&id)
                            .map(|w| w.app_id.clone())
                            .unwrap_or_default(),
                        live: !h.leaving && !h.departed,
                    })
                    .collect();
                let input = out
                    .holes
                    .values()
                    .filter(|h| !h.leaving && !h.departed)
                    .map(|h| h.target)
                    .collect();
                let scene = Scene {
                    key,
                    size: Size::new(out.width, out.height),
                    thumbs,
                    input,
                    hole_fill: None,
                };
                let output = out.id;
                Some(strip::window(
                    &scene,
                    move |at| Message::Pressed(output, at),
                    |id| Message::Pick(output, id),
                ))
            })
            .collect()
    }
}

impl Program for WindowManager {
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let out = model
            .outputs
            .values()
            .find(|o| o.strip.as_ref() == Some(window));
        let holes = out.map_or_else(Vec::new, |o| {
            o.holes
                .keys()
                .map(|&id| Hole {
                    layer: strip::hole_layer(id),
                    source: HoleSource::Window(ServerWindowId(id)),
                    claim: true,
                })
                .collect()
        });
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: strip::NAMESPACE,
                output: out.map(|o| o.id),
                layer: Layer::Bottom,
                anchor: Anchor {
                    top: true,
                    bottom: true,
                    left: true,
                    right: false,
                },
                size: Size::new(out.map_or(1.0, |o| o.width), 0.0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::None,
            }),
            persistent: Some(strip::PERSIST_KEY),
            holes,
        }
    }

    fn event(event: Event) -> Option<Message> {
        Some(match event {
            Event::WindowManager(output, update) => Message::Feed(output, update),
            Event::Outputs => Message::Outputs,
            Event::Appearance(appearance) => Message::ReduceMotion(appearance.reduce_motion),
            Event::HoleClosed { window, layer } => Message::HoleClosed(window, layer),
            Event::Closed(window) => Message::Closed(window),
            _ => return None,
        })
    }

    /// Nothing counts as shown until the setting is read and every output's first arrangement is made, so a strip
    /// the server holds is taken over by the strip laid out at once.
    fn showing(model: &Model) -> bool {
        model.stage_at_start.is_some()
            && outputs::all()
                .iter()
                .filter(|o| o.geometry.is_some())
                .all(|o| model.outputs.get(&o.id).is_some_and(|out| out.arranged))
    }
}

impl Component for WindowManager {
    const NAME: ComponentName = ComponentName::WINDOWMANAGER;
}
