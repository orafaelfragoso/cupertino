//! The dock as a component of the shell host (`cupertino-shell`): the bar on the `dock` role of the output it is on, a
//! persistent surface the server holds with its last picture and its reserved space while the component restarts
//! (D19); an edge strip on the `dock` role of every other output, so the hot corners, gestures and the switcher of
//! every output reach the dock; and the held tile, the Force Quit dialog, the trash's alerts and the switcher's panels
//! on layer surfaces of their own, the menus, names and stacks as Kit popups of the bar. Every window is drawn by
//! [`crate::kit`]'s model, which this module connects to the shell: the dock roles' feed, the outputs, the appearance
//! and the lock arrive as shell events, the dock's requests go out through the role of the window they concern, and a
//! minimized window's tile shows the window's picture in a hole the server fills.
//!
//! Nothing here waits on the UI thread: the desktop entries, `dock.conf`, the pins, the icons, the folders, the
//! launcher's history and the trash are read and written on Kit's I/O pool ([`tasks`]); the applications' badges and
//! the D-Bus activation are calls on the component's session connection. Every instance reads its start anew and keeps
//! nothing for the next: while the server holds the bar's picture of the instance before, the restarted dock shows no
//! window until its bar has all it draws at hand ([`kit::Model::ready_to_show`]), so its first picture takes the held
//! one's place whole.

mod host;
mod tasks;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::SystemAppearance;
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    bus, dock, session, Anchor, Component, ComponentName, Event, Hole, HoleSource, Keyboard, Layer,
    LayerSpec, Margin, Program, Role, ServerWindowId, Surface,
};

use crate::actions::ActionMap;
use crate::conf::dock_config;
use crate::config::Orientation;
use crate::desktop::DesktopIndex;
use crate::kit::{self, DockEvent, Message, OutputInfo, Placed, WindowIcon};
use crate::session::Session;
use crate::view::{
    self, edge, ALERT_APP_ID, BAR_APP_ID, DRAG_APP_ID, EDGE_APP_ID, FORCE_QUIT_APP_ID,
    SWITCHER_APP_ID,
};

use host::{Link, ShellHost};
pub use tasks::Startup;

/// The key the server holds the bar's picture and the space it reserves under while the component restarts: one bar.
const HOLD: &str = "dock";
/// The name the component reports problems under.
const PROCESS: &str = "cupertino-dock";

/// The dock.
pub struct Dock;

/// A value a task hands the model once, through a message that has to be cloneable.
pub struct Handed<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handed<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for Handed<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handed")
    }
}

impl<T> Handed<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, the first time it is asked for.
    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// What the shell told the component, which it takes before the model ([`Message::Shell`]).
#[derive(Clone, Debug)]
pub enum Heard {
    /// A group of changes of the dock role of the window with this key.
    Dock(WindowKey, dock::Update),
    /// An output appeared, went away or changed.
    Outputs,
    Appearance(SystemAppearance),
    /// What the dock reads at start, read on the I/O pool.
    Loaded(Handed<Startup>),
    /// The component's session connection, or none.
    Bus(Option<zbus::Connection>),
    BusClosed,
    /// A message of the watches and the bus listener ([`Model::told`]).
    Told(Box<Message>),
}

/// What the dock reads at start, which its model is made from: the desktop entries, `dock.conf`, the pins and the
/// file manager.
pub struct Read {
    desktop: DesktopIndex,
    conf: Option<DockSettings>,
    pins: Vec<String>,
    file_manager: Option<String>,
}

/// The dock's model as the shell runs it.
pub struct Model {
    /// `None` until what the dock reads at start arrived and an output is there.
    dock: Option<kit::Model>,
    /// The server holds the bar's picture of the instance before (D19): no window shows until the bar has all it draws
    /// at hand, so the bar's first picture takes the held one's place whole.
    held: bool,
    /// The start's reading, while no output is there to make the model on.
    startup: Option<Read>,
    link: Rc<RefCell<Link>>,
    /// The outputs' keys in the model, by the shell's id, in the order they were seen.
    keys: Vec<(OutputId, u32)>,
    dark: bool,
    reduce_motion: bool,
    locked: bool,
    /// What the watches and the bus listener tell, and the sender they tell it with.
    tell: async_channel::Sender<Message>,
    told: async_channel::Receiver<Message>,
    /// The watch on the home trash; dropping it ends the watch.
    trash_watch: Option<notify::RecommendedWatcher>,
    /// The watch on the `applications` directories; dropping it ends the watch.
    desktop_watch: Option<notify::RecommendedWatcher>,
}

impl Model {
    /// The model's key of the output `id`, given it the first time.
    fn key_of(&mut self, id: OutputId) -> u32 {
        if let Some(&(_, key)) = self.keys.iter().find(|(o, _)| *o == id) {
            return key;
        }
        let key = self.keys.iter().map(|&(_, k)| k + 1).max().unwrap_or(0);
        self.keys.push((id, key));
        key
    }

    /// The shell's id of the output with the model's `key`.
    fn output_id(&self, key: u32) -> Option<OutputId> {
        self.keys
            .iter()
            .find(|&&(_, k)| k == key)
            .map(|&(id, _)| id)
    }

    /// The outputs that have a size, in the shell's order.
    fn outputs(&mut self) -> Vec<OutputInfo> {
        outputs::all()
            .into_iter()
            .filter_map(|o| {
                let g = o.geometry?;
                Some(OutputInfo {
                    key: self.key_of(o.id),
                    width: g.size.width,
                    height: g.size.height,
                    scale: o.scale,
                })
            })
            .collect()
    }

    /// The next message of the watches and the bus listener.
    fn told(&self) -> Command<Message> {
        let told = self.told.clone();
        Command::Perform(Box::pin(async move {
            match told.recv().await {
                Ok(message) => Message::Shell(Heard::Told(Box::new(message))),
                // the model holds a sender
                Err(_) => std::future::pending().await,
            }
        }))
    }

    /// Make the dock's model from what it read on the first output, when there is one.
    fn make(&mut self, read: Read) -> Vec<Command<Message>> {
        let outputs = self.outputs();
        let Some(first) = outputs.first() else {
            self.startup = Some(read);
            return Vec::new();
        };
        let cfg = dock_config(read.conf.as_ref(), first.height);
        let actions = read
            .conf
            .as_ref()
            .map_or_else(ActionMap::default, |f| f.actions.clone());
        let mut bar = crate::dock::Dock::new(cfg);
        if let Some(file_manager) = &read.file_manager {
            bar.set_built_in(file_manager);
        }
        bar.set_pinned(&read.pins.iter().map(String::as_str).collect::<Vec<_>>());
        let session = Session::new(bar, read.desktop);
        let host = ShellHost::new(Rc::clone(&self.link));
        let mut dock = kit::Model::new(
            session,
            actions,
            read.conf.unwrap_or_default(),
            Box::new(host),
        );
        let mut commands = Vec::new();
        for message in [
            Message::Dark(self.dark),
            Message::ReduceMotion(self.reduce_motion),
            Message::Locked(self.locked),
            Message::Outputs(outputs),
        ] {
            commands.extend(kit::update(&mut dock, message));
        }
        self.dock = Some(dock);
        commands
    }

    /// The start's reading arrived: the host's paths and programs follow it, the watches start, and the dock's model is
    /// made from it.
    fn loaded(&mut self, startup: Startup) -> Vec<Command<Message>> {
        let Startup {
            read,
            terminal,
            folders,
            home,
            trash,
            conf_watch,
            trash_watch,
            desktop_watch,
        } = startup;
        {
            let mut link = self.link.borrow_mut();
            link.terminal = terminal;
            link.folders = folders;
            link.home = home;
            link.trash = Some(trash);
        }
        self.trash_watch = trash_watch;
        let mut commands: Vec<Command<Message>> = conf_watch
            .map(|watch| tasks::follow_conf(watch, self.tell.clone()))
            .into_iter()
            .collect();
        if let Some((watcher, pokes)) = desktop_watch {
            self.desktop_watch = Some(watcher);
            commands.push(tasks::follow_desktop(pokes, self.tell.clone()));
        }
        commands.extend(self.make(read));
        commands
    }

    /// What the shell told the component.
    fn heard(&mut self, heard: Heard) -> Vec<Command<Message>> {
        match heard {
            Heard::Dock(window, update) => {
                let Some(dock) = self.dock.as_mut() else {
                    return Vec::new();
                };
                let Some(output) = dock.role_output(&window) else {
                    return Vec::new();
                };
                let events = update
                    .into_iter()
                    .filter_map(|change| dock_event(change, output))
                    .collect();
                kit::update(dock, Message::Dock { output, events })
            }
            Heard::Outputs => {
                let outputs = self.outputs();
                match self.dock.as_mut() {
                    Some(dock) => kit::update(dock, Message::Outputs(outputs)),
                    None => match self.startup.take() {
                        Some(read) => self.make(read),
                        None => Vec::new(),
                    },
                }
            }
            Heard::Appearance(appearance) => {
                self.dark = appearance.dark;
                self.reduce_motion = appearance.reduce_motion;
                let Some(dock) = self.dock.as_mut() else {
                    return Vec::new();
                };
                let mut commands = kit::update(dock, Message::Dark(appearance.dark));
                commands.extend(kit::update(
                    dock,
                    Message::ReduceMotion(appearance.reduce_motion),
                ));
                commands
            }
            Heard::Loaded(startup) => match startup.take() {
                Some(startup) => self.loaded(startup),
                None => Vec::new(),
            },
            Heard::Bus(Some(connection)) => {
                self.link.borrow_mut().bus = Some(connection.clone());
                vec![tasks::listen_badges(connection, self.tell.clone())]
            }
            // without the session connection the tiles show no badges, and an application is started anew instead of
            // asked for a window
            Heard::Bus(None) | Heard::BusClosed => {
                eprintln!(
                    "{PROCESS}: no session bus connection: no badges and no D-Bus activation"
                );
                self.link.borrow_mut().bus = None;
                Vec::new()
            }
            Heard::Told(message) => {
                let mut commands = vec![self.told()];
                if matches!(*message, Message::DesktopChanged) {
                    commands.push(tasks::reindex_desktop());
                    return commands;
                }
                if let Some(dock) = self.dock.as_mut() {
                    commands.extend(kit::update(dock, *message));
                }
                commands
            }
        }
    }
}

/// The hot corner of the server's as the dock's actions name it.
fn trigger_of_corner(corner: dock::Corner) -> Option<crate::actions::Trigger> {
    use crate::actions::Trigger;
    Some(match corner {
        dock::Corner::TopLeft => Trigger::CornerTopLeft,
        dock::Corner::TopRight => Trigger::CornerTopRight,
        dock::Corner::BottomLeft => Trigger::CornerBottomLeft,
        dock::Corner::BottomRight => Trigger::CornerBottomRight,
        // the enum is open to later versions: a corner the dock does not know does nothing
        _ => return None,
    })
}

/// The touchpad gesture of the server's as the dock's actions name it.
fn trigger_of_gesture(gesture: dock::Gesture) -> Option<crate::actions::Trigger> {
    use crate::actions::Trigger;
    Some(match gesture {
        dock::Gesture::SwipeUp => Trigger::SwipeUp,
        dock::Gesture::SwipeDown => Trigger::SwipeDown,
        dock::Gesture::Pinch => Trigger::Pinch,
        dock::Gesture::Spread => Trigger::Spread,
        _ => return None,
    })
}

/// One change of the dock role of the output with key `output` as the model takes it. The switcher's panel goes on
/// the output whose role was told: the server tells the role of the panel's output.
fn dock_event(change: dock::Change, output: u32) -> Option<DockEvent> {
    Some(match change {
        dock::Change::Window(w) => DockEvent::Window {
            id: w.id,
            app_id: w.app_id,
            title: w.title,
            minimized: w.minimized,
        },
        dock::Change::WindowIcon(icon) => DockEvent::WindowIcon(WindowIcon {
            id: icon.id,
            name: icon.name,
            image: icon.image.map(|i| (i.size, i.argb)),
        }),
        dock::Change::WindowClosed(id) => DockEvent::WindowClosed(id),
        dock::Change::Launching(app_id) => DockEvent::Launching(app_id),
        dock::Change::ToggleAutoHide => DockEvent::ToggleAutoHide,
        dock::Change::ToggleAppGrid => DockEvent::ToggleAppGrid,
        dock::Change::SwitcherChanged { apps, selected, .. } => DockEvent::Switcher {
            output,
            apps,
            selected,
        },
        dock::Change::SwitcherClosed { committed } => DockEvent::SwitcherClosed { committed },
        dock::Change::HotCorner(corner) => DockEvent::Trigger(trigger_of_corner(corner)?),
        dock::Change::Gesture(gesture) => DockEvent::Trigger(trigger_of_gesture(gesture)?),
        dock::Change::Attention(app_id) => DockEvent::Attention(app_id),
        dock::Change::Unresponsive {
            app_id,
            unresponsive,
        } => DockEvent::Unresponsive {
            app_id,
            unresponsive,
        },
        dock::Change::ForceQuitDialog => DockEvent::ForceQuitDialog,
        dock::Change::Engage => DockEvent::Engage,
        dock::Change::AppResident { app_id, resident } => DockEvent::Resident { app_id, resident },
    })
}

impl Application for Dock {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The bar rests most of the time, and its other surfaces are small or draw little (the held tile's window covers
    /// the output but draws one tile): the CPU, which costs the host no GPU context. The magnification and the bounces
    /// repaint the bar every frame while they last; a long stretch of them moves the dock's drawing to the GPU.
    const RENDERER: RendererClass = RendererClass::ByUse;

    fn init() -> (Model, Vec<Command<Message>>) {
        let preferences = cupertino_kit_shell::environment().theme.preferences;
        let (tell, told) = async_channel::unbounded();
        let model = Model {
            dock: None,
            held: cupertino_kit_shell::holds().iter().any(|key| key == HOLD),
            startup: None,
            link: Rc::new(RefCell::new(Link::new())),
            keys: Vec::new(),
            dark: preferences.appearance.is_dark(),
            reduce_motion: preferences.reduce_motion,
            locked: session::locked(),
            tell: tell.clone(),
            told,
            trash_watch: None,
            desktop_watch: None,
        };
        let mut commands = bus::session(
            |connection| Message::Shell(Heard::Bus(connection.ok())),
            Message::Shell(Heard::BusClosed),
        );
        commands.push(model.told());
        commands.push(tasks::startup(tell));
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let commands = match message {
            Message::Shell(heard) => model.heard(heard),
            Message::Locked(locked) => {
                model.locked = locked;
                match model.dock.as_mut() {
                    Some(dock) => kit::update(dock, Message::Locked(locked)),
                    None => Vec::new(),
                }
            }
            message => match model.dock.as_mut() {
                Some(dock) => kit::update(dock, message),
                None => Vec::new(),
            },
        };
        if model.held {
            model.held = !model.dock.as_ref().is_some_and(kit::Model::ready_to_show);
        }
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        if model.held {
            return Vec::new();
        }
        model.dock.as_ref().map(kit::view).unwrap_or_default()
    }
}

/// The edges a surface along the dock's edge anchors to, and its size: as long as the output, `depth` deep.
fn along(orientation: Orientation, depth: f64) -> (Anchor, Size) {
    let depth = depth.ceil().max(1.0);
    match orientation {
        Orientation::Bottom => (
            Anchor {
                top: false,
                bottom: true,
                left: true,
                right: true,
            },
            Size::new(0.0, depth),
        ),
        Orientation::Left => (
            Anchor {
                top: true,
                bottom: true,
                left: true,
                right: false,
            },
            Size::new(depth, 0.0),
        ),
        Orientation::Right => (
            Anchor {
                top: true,
                bottom: true,
                left: false,
                right: true,
            },
            Size::new(depth, 0.0),
        ),
    }
}

/// Every edge: a surface over the whole output, whose coordinates are the output's.
const ALL_EDGES: Anchor = Anchor {
    top: true,
    bottom: true,
    left: true,
    right: true,
};

/// A layer surface on `output` of the plane `layer`.
fn layer(namespace: &'static str, output: OutputId, layer: Layer) -> LayerSpec {
    LayerSpec {
        namespace,
        output: Some(output),
        layer,
        anchor: Anchor::default(),
        size: Size::new(0.0, 0.0),
        margin: Margin::default(),
        // over whatever reserves room
        exclusive_zone: -1,
        keyboard: Keyboard::None,
    }
}

/// A transient surface of `role`.
fn transient(role: Role) -> Surface {
    Surface {
        role,
        persistent: None,
        holes: Vec::new(),
    }
}

impl Program for Dock {
    /// The bar on the `dock` role of its output, along its edge on the top layer, reserving the bar's depth and taking
    /// the keyboard while Focus Dock holds it, persistent; an edge strip on the `dock` role of every other output; the
    /// held tile over its whole output, the dialogs and the switcher's panel centred on theirs, on the overlay. The
    /// menus, the names and the stacks are Kit's popups of the bar.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let placed = model.dock.as_ref().and_then(|dock| dock.placed(key));
        let output = |o: u32| model.output_id(o);
        let surface = match placed {
            Some(Placed::Bar {
                placement,
                keyboard,
            }) => output(placement.output).map(|id| {
                let (anchor, size) = along(placement.orientation, placement.extent);
                let holes = model
                    .dock
                    .as_ref()
                    .map(|dock| dock.pictures())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(layer, window)| Hole {
                        layer,
                        source: HoleSource::Thumbnail(ServerWindowId(window)),
                        claim: false,
                    })
                    .collect();
                Surface {
                    role: Role::Dock(LayerSpec {
                        anchor,
                        size,
                        // the bar's space, which the server keeps while it holds the bar (D19)
                        exclusive_zone: placement.exclusive_zone,
                        keyboard: if keyboard {
                            Keyboard::Exclusive
                        } else {
                            Keyboard::None
                        },
                        ..layer(BAR_APP_ID, id, Layer::Top)
                    }),
                    persistent: Some(HOLD),
                    holes,
                }
            }),
            Some(Placed::Edge {
                output: o,
                orientation,
            }) => output(o).map(|id| {
                let (anchor, size) = along(orientation, edge::DEPTH);
                transient(Role::Dock(LayerSpec {
                    anchor,
                    size,
                    ..layer(EDGE_APP_ID, id, Layer::Top)
                }))
            }),
            Some(Placed::Drag { output: o }) => output(o).map(|id| {
                transient(Role::Layer(LayerSpec {
                    anchor: ALL_EDGES,
                    ..layer(DRAG_APP_ID, id, Layer::Overlay)
                }))
            }),
            // no anchor: the server centres the surface on the output
            Some(Placed::Dialog { output: o, size }) => output(o).map(|id| {
                let namespace = if *key == view::force_quit::key() {
                    FORCE_QUIT_APP_ID
                } else {
                    ALERT_APP_ID
                };
                transient(Role::Layer(LayerSpec {
                    size: Size::new(size.width.ceil(), size.height.ceil()),
                    keyboard: Keyboard::Exclusive,
                    ..layer(namespace, id, Layer::Overlay)
                }))
            }),
            // the server keeps the keys of the switch
            Some(Placed::Panel { output: o, size }) => output(o).map(|id| {
                transient(Role::Layer(LayerSpec {
                    size: Size::new(size.width.ceil(), size.height.ceil()),
                    ..layer(SWITCHER_APP_ID, id, Layer::Overlay)
                }))
            }),
            None => None,
        };
        surface.unwrap_or_else(|| transient(Role::Window))
    }

    /// Once the bar shows: while the server holds its picture of the instance before, not before the bar has all it
    /// draws at hand.
    fn showing(model: &Model) -> bool {
        !model.held
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Dock(window, update) => Some(Message::Shell(Heard::Dock(window, update))),
            Event::Outputs => Some(Message::Shell(Heard::Outputs)),
            Event::Appearance(appearance) => Some(Message::Shell(Heard::Appearance(appearance))),
            Event::Session(session::Change::Locked(locked)) => Some(Message::Locked(locked)),
            // a window the server closed hears it through its `on_close`; a minimized window's closing comes with the
            // feed
            _ => None,
        }
    }
}

impl Component for Dock {
    const NAME: ComponentName = ComponentName::DOCK;
}
