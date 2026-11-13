//! Glue between the dock's Kit application ([`cupertino_dock::kit`]) and the session: the windows get their layer
//! surfaces from [`DockRoles`] by their `app_id` (`<prefix>@<output>`), the bar and every edge strip take the dock role
//! of their output, what the server tells those roles and the session observer arrives as messages, and [`AppHost`]
//! sends the dock's requests, places the bar, gives the surfaces the keyboard, starts programs, keeps the pins and
//! loads the icons. Every surface is drawn by CupertinoKit.

use cupertino_apps::exec::CommandSpec;
use cupertino_apps::launch::{spawn, OPEN_COMMAND};
use cupertino_assets::Assets;
use cupertino_defaults::dock::DockSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_dock::actions::{ActionMap, Trigger};
use cupertino_dock::conf::{
    default_pins, dock_config, load_pins, session_file_manager, store_pins, TERMINALS,
};
use cupertino_dock::config::Orientation;
use cupertino_dock::desktop::DesktopIndex;
use cupertino_dock::icons::Icons;
use cupertino_dock::kit::{
    self, AppAction, DockEvent, Exited, Host, IconKey, IconLoad, Message, Model, OutputInfo, Part,
    Placement, Request, Sized, WindowIcon,
};
use cupertino_dock::overview_ui::{Mode, Workspace};
use cupertino_dock::session::Session;
use cupertino_dock::view::{
    edge, IconSet, BAR_APP_ID, DRAG_APP_ID, EDGE_APP_ID, FORCE_QUIT_APP_ID, OVERVIEW_APP_ID,
    SWITCHER_APP_ID,
};
use cupertino_dock::Dock;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Application, Command, Window};
use cupertino_kit::platform::WindowSpec;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_modifiers::Profile;
use cupertino_shell_client::{
    Anchor, AppCommand, Corner, Dock as DockRole, DockChange, DockWindowIcon, Gesture,
    KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    OverviewMode, SessionChange, SessionState, Shell, ShellEvent, WindowIconImage,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, Event, ExtensionEvent, OutputId, SurfaceId,
};
use cupertino_wm::geometry::Rect;
use std::cell::RefCell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The name the process reports problems under.
const PROCESS: &str = "cupertino-dock";

/// Command line of the process.
pub struct Options {
    /// Terminal emulator that runs entries with `Terminal=true`; the first installed of the common ones when absent.
    pub terminal: Option<String>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options { terminal: None };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--terminal" => {
                    options.terminal = Some(args.next().ok_or("--terminal needs a program")?)
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

/// What a layer surface of the process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Bar,
    Edge,
    Drag,
    ForceQuit,
    Switcher,
    Overview,
}

impl Kind {
    /// The kind and the output key an `app_id` names.
    fn of(app_id: &str) -> Option<(Kind, u32)> {
        let (prefix, output) = app_id.rsplit_once('@')?;
        let kind = match prefix {
            BAR_APP_ID => Kind::Bar,
            EDGE_APP_ID => Kind::Edge,
            DRAG_APP_ID => Kind::Drag,
            FORCE_QUIT_APP_ID => Kind::ForceQuit,
            SWITCHER_APP_ID => Kind::Switcher,
            OVERVIEW_APP_ID => Kind::Overview,
            _ => return None,
        };
        Some((kind, output.parse().ok()?))
    }
}

struct Surface {
    kind: Kind,
    output: u32,
    layer: LayerSurface,
    /// The dock role of the bar and of the edge strips.
    role: Option<DockRole>,
}

/// The process's layer surfaces, shared by the roles and the host.
#[derive(Default)]
struct Surfaces {
    all: Vec<Surface>,
    /// Where the bar goes, as the model last said.
    placement: Option<Placement>,
    /// The surface that has the keyboard, as the model last said.
    keyboard: Option<Part>,
}

impl Surfaces {
    fn of_surface(&self, surface: SurfaceId) -> Option<&Surface> {
        self.all.iter().find(|s| s.layer.surface() == surface)
    }

    fn of_kind(&mut self, kind: Kind) -> Option<&mut Surface> {
        self.all.iter_mut().find(|s| s.kind == kind)
    }

    /// The dock role of the output with key `output`: its bar's or its edge strip's.
    fn role(&self, output: u32) -> Option<&DockRole> {
        self.all
            .iter()
            .filter(|s| s.output == output)
            .find_map(|s| s.role.as_ref())
    }
}

/// The edges a surface along the dock's edge is anchored to, and its size for `extent` points deep.
fn along(orientation: Orientation, extent: u32) -> (Anchor, (u32, u32)) {
    match orientation {
        Orientation::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, extent)),
        Orientation::Left => (Anchor::Left | Anchor::Top | Anchor::Bottom, (extent, 0)),
        Orientation::Right => (Anchor::Right | Anchor::Top | Anchor::Bottom, (extent, 0)),
    }
}

const ALL_EDGES: Anchor = Anchor::Top
    .union(Anchor::Bottom)
    .union(Anchor::Left)
    .union(Anchor::Right);

/// Gives the windows their layer surfaces and forwards what the server tells the dock roles and the session observer.
struct DockRoles {
    layer_shell: LayerShell,
    shell: Shell,
    surfaces: Rc<RefCell<Surfaces>>,
    observed: SessionState,
    messages: async_channel::Sender<Message>,
}

impl DockRoles {
    fn send(&self, message: Message) {
        // the receiver is gone only while the process exits
        let _ = self.messages.try_send(message);
    }

    fn config(&self, kind: Kind, output: OutputId, spec: &WindowSpec) -> LayerSurfaceConfig {
        let whole = |v: f64| v.ceil().max(1.0) as u32;
        let (placement, holder) = {
            let surfaces = self.surfaces.borrow();
            (surfaces.placement, surfaces.keyboard)
        };
        // a surface made while the model gives it the keyboard takes it at once
        let keyboard_of = |part: Part| {
            if holder == Some(part) {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            }
        };
        let key = output_key(output);
        let orientation = placement.map_or(Orientation::Bottom, |p| p.orientation);
        let (layer, anchor, size, exclusive_zone, keyboard) = match kind {
            Kind::Bar => {
                let extent = placement.map_or(whole(spec.size.height), |p| whole(p.extent));
                let (anchor, size) = along(orientation, extent);
                let zone = placement.map_or(0, |p| p.exclusive_zone);
                (Layer::Top, anchor, size, zone, keyboard_of(Part::Bar(key)))
            }
            Kind::Edge => {
                let (anchor, size) = along(orientation, whole(edge::DEPTH));
                // -1: the strip lies on the very edge, whatever reserves room there
                (Layer::Top, anchor, size, -1, KeyboardInteractivity::None)
            }
            // the whole output, so the window's coordinates are the output's
            Kind::Drag => (
                Layer::Overlay,
                ALL_EDGES,
                (0, 0),
                -1,
                KeyboardInteractivity::None,
            ),
            // no anchor: the server centres the surface on the output, ignoring the zones
            Kind::ForceQuit => (
                Layer::Overlay,
                Anchor::empty(),
                (whole(spec.size.width), whole(spec.size.height)),
                -1,
                KeyboardInteractivity::Exclusive,
            ),
            // the server keeps the keys of the switch
            Kind::Switcher => (
                Layer::Overlay,
                Anchor::empty(),
                (whole(spec.size.width), whole(spec.size.height)),
                -1,
                KeyboardInteractivity::None,
            ),
            // over the dock bar and the menu bar, so the window's coordinates are the output's (the server's area is
            // in them); the keyboard while the overview is open
            Kind::Overview => (
                Layer::Overlay,
                ALL_EDGES,
                (0, 0),
                -1,
                keyboard_of(Part::Overview(key)),
            ),
        };
        LayerSurfaceConfig {
            namespace: match kind {
                Kind::Bar => BAR_APP_ID,
                Kind::Edge => EDGE_APP_ID,
                Kind::Drag => DRAG_APP_ID,
                Kind::ForceQuit => FORCE_QUIT_APP_ID,
                Kind::Switcher => SWITCHER_APP_ID,
                Kind::Overview => OVERVIEW_APP_ID,
            }
            .to_string(),
            output: Some(output),
            layer,
            anchor,
            size,
            margin: Margin::default(),
            exclusive_zone,
            keyboard,
        }
    }

    fn forward(&self, surface: SurfaceId, update: &[DockChange]) {
        let Some(output) = self.surfaces.borrow().of_surface(surface).map(|s| s.output) else {
            return;
        };
        let events = update.iter().map(dock_event).collect();
        self.send(Message::Dock { output, events });
    }
}

fn output_key(id: OutputId) -> u32 {
    id.registry_name()
}

/// The compositor's outputs that have a logical size, in its order.
fn outputs(conn: &Connection) -> Vec<OutputInfo> {
    conn.outputs()
        .into_iter()
        .filter_map(|o| {
            let g = o.geometry?;
            Some(OutputInfo {
                key: output_key(o.id),
                width: f64::from(g.width),
                height: f64::from(g.height),
                scale: o.effective_scale().unwrap_or_else(|| o.scale.factor()),
            })
        })
        .collect()
}

fn wm_rect(r: &cupertino_geometry::Rect) -> Rect {
    Rect::new(r.x(), r.y(), r.width(), r.height())
}

fn trigger_of_corner(corner: Corner) -> Trigger {
    match corner {
        Corner::TopLeft => Trigger::CornerTopLeft,
        Corner::TopRight => Trigger::CornerTopRight,
        Corner::BottomLeft => Trigger::CornerBottomLeft,
        Corner::BottomRight => Trigger::CornerBottomRight,
        // the enum is non-exhaustive for the client; the toolkit drops values it does not know
        _ => Trigger::CornerTopLeft,
    }
}

fn trigger_of_gesture(gesture: Gesture) -> Trigger {
    match gesture {
        Gesture::SwipeUp => Trigger::SwipeUp,
        Gesture::SwipeDown => Trigger::SwipeDown,
        Gesture::Pinch => Trigger::Pinch,
        Gesture::Spread => Trigger::Spread,
        _ => Trigger::SwipeUp,
    }
}

/// One change of the server's as the model takes it.
fn dock_event(change: &DockChange) -> DockEvent {
    match change.clone() {
        DockChange::Window(w) => DockEvent::Window {
            id: w.id,
            app_id: w.app_id,
            title: w.title,
            minimized: w.minimized,
        },
        DockChange::WindowIcon(icon) => DockEvent::WindowIcon(WindowIcon {
            id: icon.id,
            name: icon.name,
            image: icon.image.map(|i| (i.size, i.argb)),
        }),
        DockChange::WindowThumbnail(t) => DockEvent::WindowThumbnail {
            id: t.id,
            image: t
                .image
                .and_then(|i| cupertino_dock::minimized::image(i.width, i.height, &i.argb)),
        },
        DockChange::WindowClosed(id) => DockEvent::WindowClosed(id),
        DockChange::Launching(app_id) => DockEvent::Launching(app_id),
        DockChange::ToggleAutoHide => DockEvent::ToggleAutoHide,
        DockChange::ToggleAppGrid => DockEvent::ToggleAppGrid,
        DockChange::SwitcherChanged {
            output,
            apps,
            selected,
        } => DockEvent::Switcher {
            output: output_key(output),
            apps,
            selected,
        },
        DockChange::SwitcherClosed { committed } => DockEvent::SwitcherClosed { committed },
        DockChange::OverviewChanged {
            mode,
            area,
            windows,
            workspaces,
        } => DockEvent::Overview {
            mode: match mode {
                OverviewMode::All => Mode::All,
                OverviewMode::App(_) => Mode::App,
            },
            area: wm_rect(&area),
            windows: windows.iter().map(|w| (w.id, wm_rect(&w.frame))).collect(),
            workspaces: workspaces
                .iter()
                .map(|w| Workspace {
                    id: w.id,
                    current: w.current,
                })
                .collect(),
        },
        DockChange::OverviewClosed => DockEvent::OverviewClosed,
        DockChange::HotCorner(corner) => DockEvent::Trigger(trigger_of_corner(corner)),
        DockChange::Gesture(gesture) => DockEvent::Trigger(trigger_of_gesture(gesture)),
        DockChange::Attention(app_id) => DockEvent::Attention(app_id),
        DockChange::Unresponsive {
            app_id,
            unresponsive,
        } => DockEvent::Unresponsive {
            app_id,
            unresponsive,
        },
        DockChange::ForceQuitDialog => DockEvent::ForceQuitDialog,
        DockChange::Engage => DockEvent::Engage,
        DockChange::AppResident { app_id, resident } => DockEvent::Resident { app_id, resident },
    }
}

impl SurfaceRoles for DockRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let (kind, key) = Kind::of(&spec.app_id)?;
        let Some(output) = conn
            .outputs()
            .into_iter()
            .map(|o| o.id)
            .find(|&id| output_key(id) == key)
        else {
            return Some(Err(WaylandError::Protocol("the dock's output went away")));
        };
        let config = self.config(kind, output, spec);
        let layer = match self.layer_shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => return Some(Err(WaylandError::Protocol("no layer surface for the dock"))),
        };
        let surface = layer.surface();
        if matches!(kind, Kind::Bar | Kind::Edge) {
            // the server allows one dock per output, and the toolkit makes a window before it closes the one it
            // replaces (the bar moving here, a strip where the bar was): that one gives its role up first
            for s in self.surfaces.borrow_mut().all.iter_mut() {
                if s.output == key {
                    s.role = None;
                }
            }
        }
        let role = match kind {
            Kind::Bar | Kind::Edge => match self.shell.dock(conn, surface, output) {
                Ok(role) => Some(role),
                Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
                Err(_) => return Some(Err(WaylandError::Protocol("no dock role"))),
            },
            _ => None,
        };
        self.surfaces.borrow_mut().all.push(Surface {
            kind,
            output: key,
            layer,
            role,
        });
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(change) = self.observed.apply(event) {
            self.send(match change {
                SessionChange::Appearance { dark } => Message::Dark(dark),
                SessionChange::DisplayOptions(display) => {
                    Message::ReduceMotion(display.reduce_motion)
                }
                SessionChange::Lock { locked } => Message::Locked(locked),
            });
            // the window system follows the appearance and the lock too
            return None;
        }
        match event.downcast_ref::<ShellEvent>()? {
            &ShellEvent::Configure { surface, size } => {
                let surfaces = self.surfaces.borrow();
                let s = surfaces.of_surface(surface)?;
                // a configure without a size closes the window, which the model answers
                let Ok(size) = s.layer.configured(size) else {
                    return Some(RoleEvent::Closed(surface));
                };
                let sized = match s.kind {
                    Kind::Bar => Some(Sized::Bar(s.output)),
                    _ => None,
                };
                drop(surfaces);
                if let Some(sized) = sized {
                    self.send(Message::Sized(sized, f64::from(size.0), f64::from(size.1)));
                }
                Some(RoleEvent::Configure { surface, size })
            }
            // its output went away
            &ShellEvent::Closed(surface) => {
                self.surfaces.borrow().of_surface(surface)?;
                Some(RoleEvent::Closed(surface))
            }
            ShellEvent::Dock { surface, update } => {
                self.forward(*surface, update);
                None
            }
            _ => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        self.surfaces
            .borrow_mut()
            .all
            .retain(|s| s.layer.surface() != surface);
    }

    fn keyboard_focus(&mut self, surface: SurfaceId, focused: bool) {
        let output = self
            .surfaces
            .borrow()
            .of_surface(surface)
            .filter(|s| s.kind == Kind::Bar)
            .map(|s| s.output);
        if let (Some(output), false) = (output, focused) {
            self.send(Message::KeyboardLeft(output));
        }
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        self.send(Message::Outputs(outputs(conn)));
    }
}

/// Whether `program` is an executable file on `PATH` (or at the path it names).
fn installed(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// A premultiplied pixmap as a Kit image.
fn image(pixmap: &tiny_skia::Pixmap) -> ImageSource {
    ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 0,
            width: pixmap.width(),
            height: pixmap.height(),
            stride: pixmap.width() * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixmap.data()),
        },
    }
}

/// `dock.conf` at `path` (nothing set when it does not exist). Lines that are not understood are reported; with
/// `last_good` their keys keep its values.
fn load_dock_settings(path: &Path, last_good: Option<&DockSettings>) -> Result<DockSettings> {
    let (settings, problems) = DockSettings::load(path, last_good)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for problem in problems {
        eprintln!(
            "{PROCESS}: {} line {}: {}",
            path.display(),
            problem.line,
            problem.message
        );
    }
    Ok(settings)
}

struct AppHost {
    surfaces: Rc<RefCell<Surfaces>>,
    start: Instant,
    icons: Icons,
    /// The icons loaded in the background ([`Host::preload`]), on a thread of their own; made on the first load.
    preloader: Arc<Mutex<Option<Icons>>>,
    assets: Assets,
    /// The file types, for a stack's icons; loaded by the first listing, on its thread.
    mime: Arc<std::sync::OnceLock<cupertino_apps::MimeDatabase>>,
    /// Where the pins are stored (`None` when neither `XDG_CONFIG_HOME` nor `HOME` is set).
    pins_path: Option<PathBuf>,
    conf_path: Option<PathBuf>,
    terminal: Option<String>,
}

impl AppHost {
    fn request_on(&self, role: &DockRole, request: Request) -> std::result::Result<(), String> {
        let shape = |r: std::result::Result<(), cupertino_shell_client::Error>| {
            r.map_err(|e| e.to_string())
        };
        let rect = |r: &Rect| cupertino_geometry::Rect::new(r.x, r.y, r.w, r.h);
        match request {
            Request::ActivateApp(app_id) => role.activate_app(&app_id),
            Request::RestoreWindow { id, tile } => shape(role.restore_window(id, &rect(&tile)))?,
            Request::MinimizeTarget(target) => shape(role.set_minimize_target(&rect(&target)))?,
            Request::App(app_id, action) => role.app_command(
                &app_id,
                match action {
                    AppAction::ShowWindows => AppCommand::ShowWindows,
                    AppAction::Hide => AppCommand::Hide,
                    AppAction::Quit => AppCommand::Quit,
                },
            ),
            Request::ForceQuitApp(app_id) => role.force_quit_app(&app_id),
            Request::RunAction(name) => role.run_action(&name),
            Request::SwitcherSelect(i) => shape(role.switcher_select(i))?,
            Request::SwitcherActivate(i) => shape(role.switcher_activate(i))?,
            Request::SwitcherCancel => role.switcher_cancel(),
            Request::OverviewFrame(id, frame) => shape(role.overview_frame(id, &rect(&frame)))?,
            Request::OverviewSelect(id) => role.overview_select(id),
            Request::OverviewClose => role.overview_close(),
            Request::SwitchWorkspace(id) => role.switch_workspace(id),
            Request::AddWorkspace => role.add_workspace(),
            Request::RemoveWorkspace(id) => role.remove_workspace(id),
            Request::MoveWorkspace { id, index } => shape(role.move_workspace(id, index))?,
        }
        Ok(())
    }
}

impl Host for AppHost {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn wake(&mut self, after: f64, generation: u64) -> Vec<Command<Message>> {
        let after = Duration::from_secs_f64(after.max(0.0));
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(after).await;
            Message::Wake(generation)
        }))]
    }

    fn request(&mut self, output: u32, request: Request) {
        let surfaces = self.surfaces.borrow();
        // before the role exists (the minimize target of a bar not created yet) the next change sends it again
        let Some(role) = surfaces.role(output) else {
            return;
        };
        if let Err(e) = self.request_on(role, request) {
            eprintln!("{PROCESS}: a request to the server failed: {e}");
        }
    }

    fn place_bar(&mut self, placement: Placement) {
        let mut surfaces = self.surfaces.borrow_mut();
        surfaces.placement = Some(placement);
        let (anchor, size) = along(placement.orientation, placement.extent.max(1.0) as u32);
        for s in surfaces
            .all
            .iter_mut()
            .filter(|s| s.kind == Kind::Bar && s.output == placement.output)
        {
            s.layer.set_placement(anchor, size);
            s.layer.set_exclusive_zone(placement.exclusive_zone);
        }
        let (anchor, size) = along(placement.orientation, edge::DEPTH as u32);
        for s in surfaces.all.iter_mut().filter(|s| s.kind == Kind::Edge) {
            s.layer.set_placement(anchor, size);
        }
    }

    fn keyboard(&mut self, part: Part, on: bool) {
        let mut surfaces = self.surfaces.borrow_mut();
        if on {
            surfaces.keyboard = Some(part);
        } else if surfaces.keyboard == Some(part) {
            surfaces.keyboard = None;
        }
        let of =
            |kind: Kind, output: u32| move |s: &&mut Surface| s.kind == kind && s.output == output;
        let surface = match part {
            Part::Bar(output) => surfaces.all.iter_mut().find(of(Kind::Bar, output)),
            Part::Overview(output) => surfaces.all.iter_mut().find(of(Kind::Overview, output)),
        };
        if let Some(s) = surface {
            s.layer.set_keyboard_interactivity(if on {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            });
        }
    }

    fn resize_force_quit(&mut self, width: f64, height: f64) {
        if let Some(s) = self.surfaces.borrow_mut().of_kind(Kind::ForceQuit) {
            s.layer
                .set_placement(Anchor::empty(), (width.ceil() as u32, height.ceil() as u32));
        }
    }

    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> Option<Exited> {
        let spec = CommandSpec::from_argv(argv)?;
        let spec = match token {
            Some(token) => spec.with_activation_token(&token),
            None => spec,
        };
        spawn_watched(spec.to_command())
    }

    fn open(&mut self, target: &str) {
        let mut command = Process::new(OPEN_COMMAND);
        command.arg(target);
        spawn(PROCESS, command);
    }

    fn shell(&mut self, command_line: &str) {
        let mut command = Process::new("sh");
        command.args(["-c", command_line]);
        spawn(PROCESS, command);
    }

    /// A failure goes to stderr: the dock keeps the order it shows, and the next change writes the file again.
    fn store_pins(&mut self, pins: &[String]) {
        let Some(path) = &self.pins_path else { return };
        if let Err(e) = store_pins(path, pins) {
            eprintln!("{PROCESS}: cannot write {}: {e}", path.display());
        }
    }

    fn read_conf(&mut self, last_good: &DockSettings) -> Option<DockSettings> {
        let path = self.conf_path.as_ref()?;
        match load_dock_settings(path, Some(last_good)) {
            Ok(settings) => Some(settings),
            Err(e) => {
                eprintln!("{PROCESS}: {e}; the dock keeps its settings");
                None
            }
        }
    }

    fn terminal(&mut self) -> Option<String> {
        self.terminal.clone().or_else(|| {
            TERMINALS
                .iter()
                .find(|t| installed(t))
                .map(|t| t.to_string())
        })
    }

    /// The launcher's usage store, ranked now: the dock only reads it.
    fn recent_apps(&mut self) -> Vec<String> {
        let Some(path) = cupertino_apps::frecency::default_path() else {
            return Vec::new();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        cupertino_apps::frecency::ranked_apps(&cupertino_apps::frecency::Store::read(&path), now)
            .into_iter()
            .map(|app| app.id)
            .collect()
    }

    fn dbus_activate(&mut self, bus_name: &str, token: Option<String>) -> Option<kit::BusCall> {
        Some(Box::pin(cupertino_dock::activate::activate_in_thread(
            PROCESS,
            bus_name.to_owned(),
            token,
        )))
    }

    fn icon(&mut self, desktop: &DesktopIndex, icon: &IconKey, side: f64) -> IconSet {
        kit::ICON_SCALES
            .iter()
            .map(|scale: &f64| {
                let px = (side * scale).ceil().max(1.0) as u32;
                let pixmap = match icon {
                    IconKey::App(key) => self.icons.app(desktop, key, px),
                    IconKey::Named(name) => self.icons.named(name, px),
                    IconKey::Trash(full) => self.icons.trash(*full, px),
                };
                image(pixmap)
            })
            .collect()
    }

    /// The user's downloads folder (`XDG_DOWNLOAD_DIR` of `user-dirs.dirs`, else `~/Downloads`), when it exists.
    fn folders(&mut self) -> Vec<(String, String)> {
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            return Vec::new();
        };
        let home = PathBuf::from(home);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|c| !c.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from);
        let user_dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).ok();
        let downloads = cupertino_dock::stack::downloads_dir(user_dirs.as_deref(), &home);
        if !downloads.is_dir() {
            return Vec::new();
        }
        vec![(
            downloads.to_string_lossy().into_owned(),
            cupertino_dock::stack::DOWNLOADS_ICON.to_owned(),
        )]
    }

    /// Listed on a thread of its own when the stack opens ([`cupertino_dock::stack::list_folder`]).
    fn folder_entries(&mut self, folder: &str) -> kit::FolderListing {
        let (sender, receiver) = async_channel::bounded(1);
        let folder = PathBuf::from(folder);
        let mime = Arc::clone(&self.mime);
        std::thread::spawn(move || {
            let mime = mime.get_or_init(|| {
                let dirs = cupertino_apps::XdgDirs::from_env()
                    .map(|d| d.all_data_dirs())
                    .unwrap_or_default();
                cupertino_apps::MimeDatabase::load(&dirs).unwrap_or_default()
            });
            let entries = cupertino_dock::stack::list_folder(&folder, |path, is_dir| {
                let ty = mime.file_type(path, is_dir);
                cupertino_icons::document::document_icon_name(mime, ty).to_owned()
            });
            let _ = sender.send_blocking(entries);
        });
        Box::pin(async move { receiver.recv().await.unwrap_or_default() })
    }

    /// An application's icon loads from the theme name of its entry; one without, or whose name the theme lacks, is
    /// left out, for the model to load on the spot (its window's own icon lives here, on the model's thread).
    fn preload(&mut self, desktop: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        let jobs: Vec<(IconKey, f64, Preload)> = icons
            .into_iter()
            .filter_map(|(key, side)| {
                let job = match &key {
                    IconKey::App(app) => Preload::Themed(desktop.entry_icon(app)?.to_owned()),
                    IconKey::Named(name) => Preload::Named(name.clone()),
                    IconKey::Trash(full) => Preload::Trash(*full),
                };
                Some((key, side, job))
            })
            .collect();
        let (sender, receiver) = async_channel::bounded(1);
        let preloader = Arc::clone(&self.preloader);
        let assets = self.assets.clone();
        std::thread::spawn(move || {
            let mut slot = preloader.lock().unwrap_or_else(|e| e.into_inner());
            let icons = slot.get_or_insert_with(|| Icons::background(&assets));
            let loaded: Vec<(IconKey, u32, IconSet)> = jobs
                .into_iter()
                .filter_map(|(key, side, job)| {
                    let set = kit::ICON_SCALES
                        .iter()
                        .map(|scale| {
                            let px = (side * scale).ceil().max(1.0) as u32;
                            let pixmap = match &job {
                                Preload::Themed(name) => icons.themed(name, px)?,
                                Preload::Named(name) => icons.named(name, px),
                                Preload::Trash(full) => icons.trash(*full, px),
                            };
                            Some(image(pixmap))
                        })
                        .collect::<Option<IconSet>>()?;
                    Some((key, side.ceil().max(1.0) as u32, set))
                })
                .collect();
            let _ = sender.send_blocking(loaded);
        });
        Some(Box::pin(async move {
            receiver.recv().await.unwrap_or_default()
        }))
    }

    fn window_icon(&mut self, apps: [String; 2], icon: WindowIcon) {
        self.icons.window_icon(
            apps,
            DockWindowIcon {
                id: icon.id,
                name: icon.name,
                image: icon
                    .image
                    .map(|(size, argb)| WindowIconImage { size, argb }),
            },
        );
    }

    fn window_closed(&mut self, id: u32) {
        self.icons.window_closed(id);
    }

    fn trash_full(&mut self) -> bool {
        self.icons.trash_full()
    }

    /// Emptied on a thread of its own: a big trash takes a while to delete.
    fn empty_trash(&mut self) -> kit::BusCall {
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let ok = cupertino_trash::Trash::new().empty().is_ok();
            let _ = sender.send_blocking(ok);
        });
        Box::pin(async move { receiver.recv().await.unwrap_or(false) })
    }
}

/// An icon loaded in the background ([`Host::preload`]).
enum Preload {
    /// The theme's icon of this name, or none.
    Themed(String),
    /// The theme's icon of this name, else the generic application icon.
    Named(String),
    /// The trash, full or empty.
    Trash(bool),
}

/// Run `command` detached from the dock's loop, as [`spawn`] does: what ends when the process exits, in whether it
/// failed, its exit reaped on a thread; `None` when it did not start. An unsuccessful exit is reported on standard
/// error. An exit that cannot be read counts as no failure.
fn spawn_watched(mut command: Process) -> Option<Exited> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{PROCESS}: cannot run {program}: {e}");
            return None;
        }
    };
    let (sender, receiver) = async_channel::bounded::<bool>(1);
    std::thread::spawn(move || {
        let failed = match child.wait() {
            Ok(status) if !status.success() => {
                eprintln!("{PROCESS}: {program} exited with {status}");
                true
            }
            Ok(_) => false,
            Err(e) => {
                eprintln!("{PROCESS}: {program}: {e}");
                false
            }
        };
        let _ = sender.send_blocking(failed);
    });
    Some(Box::pin(
        async move { receiver.recv().await.unwrap_or(false) },
    ))
}

/// The model and the channel the session's messages arrive on.
pub struct App {
    model: Model,
    heard: async_channel::Receiver<Message>,
}

fn listen(heard: &async_channel::Receiver<Message>) -> Command<Message> {
    let heard = heard.clone();
    Command::Perform(Box::pin(async move {
        heard.recv().await.unwrap_or(Message::Unheard)
    }))
}

impl Application for App {
    type Model = App;
    type Message = Message;
    const RESIDENT: bool = true;

    /// Not used: the model needs the host and the connection [`run`] sets up, and starts through `run_on`.
    fn init() -> (App, Vec<Command<Message>>) {
        unreachable!("the dock starts only through `run`, which hands the model to `run_on`")
    }

    fn update(app: &mut App, message: Message) -> Vec<Command<Message>> {
        let heard = message.is_heard();
        let mut commands = kit::update(&mut app.model, message);
        if heard {
            commands.push(listen(&app.heard));
        }
        commands
    }

    fn view(app: &App) -> Vec<Window<Message>> {
        kit::view(&app.model)
    }
}

/// Watch `dock.conf` at `path`; a change is told as [`Message::ConfChanged`]. A watch that cannot be set up is
/// reported: the file then applies at the next start.
fn watch_conf(
    path: Option<&PathBuf>,
    messages: async_channel::Sender<Message>,
) -> Option<ConfigWatcher> {
    let path = path?;
    let watch = ConfigWatcher::new(vec![path.clone()], move |event| match event {
        WatchEvent::Changed(_) => {
            let _ = messages.send_blocking(Message::ConfChanged);
        }
        WatchEvent::Error(e) => eprintln!("{PROCESS}: watching dock.conf failed: {e}"),
    });
    match watch {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!(
                "{PROCESS}: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            None
        }
    }
}

/// Watch the home trash's `files` folder (made when missing); an entry put there or taken away is told as
/// [`Message::TrashChanged`], so the trash's icon follows at once. A watch that cannot be set up leaves the
/// once-a-second look the model takes anyway.
fn watch_trash(messages: async_channel::Sender<Message>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let files = cupertino_trash::Trash::new().home_trash().join("files");
    if let Err(e) = std::fs::create_dir_all(&files) {
        eprintln!("{PROCESS}: cannot make {}: {e}", files.display());
        return None;
    }
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !e.kind.is_access()) {
            let _ = messages.send_blocking(Message::TrashChanged);
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{PROCESS}: cannot watch the trash: {e}");
            return None;
        }
    };
    match watcher.watch(&files, notify::RecursiveMode::NonRecursive) {
        Ok(()) => Some(watcher),
        Err(e) => {
            eprintln!("{PROCESS}: cannot watch {}: {e}", files.display());
            None
        }
    }
}

/// Listen on the session bus, on a thread of its own, for the launcher entry signals with which applications set the count
/// on their badges ([`cupertino_dock::badge`]). Without a session bus the tiles show no badges.
fn listen_badges(messages: async_channel::Sender<Message>) {
    use cupertino_dock::badge::{update, LAUNCHER_ENTRY_INTERFACE, LAUNCHER_ENTRY_UPDATE};
    use std::collections::HashMap;
    use zbus::message::Type;
    use zbus::zvariant::{OwnedValue, Value};
    let listen = move || -> zbus::Result<()> {
        let session = zbus::blocking::Connection::session()?;
        let rule = zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(LAUNCHER_ENTRY_INTERFACE)?
            .member(LAUNCHER_ENTRY_UPDATE)?
            .build();
        let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &session, None)?;
        let number = |v: &Value<'_>| match *v {
            Value::I64(n) => Some(n),
            Value::I32(n) => Some(i64::from(n)),
            Value::U32(n) => Some(i64::from(n)),
            Value::U64(n) => i64::try_from(n).ok(),
            Value::I16(n) => Some(i64::from(n)),
            Value::U16(n) => Some(i64::from(n)),
            Value::U8(n) => Some(i64::from(n)),
            _ => None,
        };
        for message in signals {
            let Ok(message) = message else { break };
            let Ok((uri, properties)) = message
                .body()
                .deserialize::<(String, HashMap<String, OwnedValue>)>()
            else {
                continue;
            };
            let count = properties.get("count").and_then(|v| number(v));
            let visible = properties.get("count-visible").and_then(|v| match **v {
                Value::Bool(b) => Some(b),
                _ => None,
            });
            if let Some(update) = update(&uri, count, visible) {
                if messages.send_blocking(Message::Badge(update)).is_err() {
                    break;
                }
            }
        }
        Ok(())
    };
    let spawned = std::thread::Builder::new()
        .name("badges".into())
        .spawn(move || {
            if let Err(e) = listen() {
                eprintln!("{PROCESS}: cannot hear the applications' badges: {e}");
            }
        });
    if let Err(e) = spawned {
        eprintln!("{PROCESS}: cannot listen for the applications' badges: {e}");
    }
}

/// Connect, take the dock role of the first output and serve until the connection fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let layer_shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let shell = Shell::bind(conn).ok_or("the server lacks cupertino_shell_v1")?;
    let (observer, mut observed, early) = shell.observe(conn)?;
    for event in early {
        if let Event::Extension(event) = event {
            observed.apply(&event);
        }
    }
    let outputs = outputs(conn);
    let display_height = outputs.first().ok_or("the server has no output")?.height;

    let desktop = DesktopIndex::load_from(&cupertino_defaults::data_dirs());
    let conf_path = DockSettings::path();
    let file = match &conf_path {
        Some(path) => Some(load_dock_settings(path, None)?),
        None => None,
    };
    let cfg = dock_config(file.as_ref(), display_height);
    let actions = file
        .as_ref()
        .map_or_else(ActionMap::default, |f| f.actions.clone());
    // the pins the dock stored (a drag, Keep in Dock) win over those of `dock.conf`, which the dock never rewrites
    let pins_path = cupertino_defaults::config_file("CUPERTINO_DOCK_PINS", "dock-pins");
    let stored = match &pins_path {
        Some(path) => {
            load_pins(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
        }
        None => None,
    };
    let pins: Vec<String> = match stored.or_else(|| {
        file.as_ref()
            .map(|f| f.pinned.clone())
            .filter(|p| !p.is_empty())
    }) {
        Some(pins) => pins,
        None => {
            let terminals: Vec<&str> = options
                .terminal
                .iter()
                .map(String::as_str)
                .chain(TERMINALS.iter().copied())
                .collect();
            // four after the built-in file manager
            default_pins(&desktop, &terminals, 4)
        }
    };
    let mut dock = Dock::new(cfg);
    if let Some(file_manager) = session_file_manager(&desktop) {
        dock.set_built_in(&file_manager);
    }
    dock.set_pinned(&pins.iter().map(String::as_str).collect::<Vec<_>>());
    let session = Session::new(dock, desktop);

    let (messages, heard) = async_channel::unbounded();
    let _conf_watch = watch_conf(conf_path.as_ref(), messages.clone());
    let _trash_watch = watch_trash(messages.clone());
    listen_badges(messages.clone());
    let surfaces = Rc::new(RefCell::new(Surfaces::default()));
    let initial = (
        observed.dark,
        observed.display.reduce_motion,
        observed.locked,
    );
    let system = system.with_roles(DockRoles {
        layer_shell,
        shell,
        surfaces: Rc::clone(&surfaces),
        observed,
        messages,
    });
    let host = AppHost {
        surfaces,
        start: Instant::now(),
        icons: Icons::new(&assets),
        preloader: Arc::default(),
        assets: assets.clone(),
        mime: Arc::default(),
        pins_path,
        conf_path,
        terminal: options.terminal.clone(),
    };
    let mut model = Model::new(session, actions, file.unwrap_or_default(), Box::new(host));
    model.set_unresponsive(fixture_unresponsive());
    let mut commands = vec![listen(&heard)];
    for message in [
        Message::Dark(initial.0),
        Message::ReduceMotion(initial.1),
        Message::Locked(initial.2),
        Message::Outputs(outputs),
    ] {
        commands.extend(kit::update(&mut model, message));
    }
    let app = App { model, heard };
    // the observer lives as long as the process
    let _observer = observer;
    cupertino_kit::app::run_on::<App>(system, (app, commands))?;
    Err("the dock's surfaces were closed".into())
}

/// The application ids that the screenshot fixtures declare unresponsive.
#[cfg(cupertino_capture_fixtures)]
fn fixture_unresponsive() -> std::collections::HashSet<String> {
    std::env::var("CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE")
        .map(|ids| ids.split(',').map(str::to_owned).collect())
        .unwrap_or_default()
}

#[cfg(not(cupertino_capture_fixtures))]
fn fixture_unresponsive() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}
