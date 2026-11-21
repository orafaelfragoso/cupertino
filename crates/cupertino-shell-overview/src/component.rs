//! The overview as a component of the shell host (`cupertino-shell`): a surface over each output that shows
//! the output's windows in holes, the desktops band and the selection, opened and closed with transactions.
//!
//! - **The way in.** For every output whose `dock.conf` gives the swipe up the `overview` action (the swipe
//!   down `app-overview`), the component keeps a transaction armed for that gesture: its surface, staged and not
//!   shown, has a hole per window laid out at the window's own frame, claiming it, and layer animations that take
//!   each hole to its place in the overview and fade the chrome in. The fingers drive them; released, the gesture
//!   completes or reverts along [`SpringPreset::OVERVIEW_OPEN`] from its velocity. The server's toggle (the shortcut, a
//!   hot corner, a named action) opens the same surface with a transaction of its own.
//! - **Open.** The holes are laid out at their places; a change of the windows or of the band moves them there with
//!   [`SpringPreset::OVERVIEW_REFLOW`]. A window that closes has its hole faded by the server, which says so
//!   (`Event::HoleClosed`). Clicks and keys pick a window, leave, switch, add, remove and reorder desktops and
//!   move a window to another desktop, through `kit_shell::overview`.
//! - **The way out.** The reverse gesture takes a gesture-opened overview back; a pick, a desktop or a leave starts a
//!   transaction that takes the holes back to the windows' frames (the picked window activated at its start) and
//!   fades the chrome; at its end the surface goes and the windows show in their places again. Then the gesture is
//!   armed anew.
//!
//! The windows' frames are global; the component subtracts the output's origin. Under Reduce Motion the server
//! drives only the opacity of a gesture's layers, and a toggle fades the overview in and out with the windows at
//! their end places ([`Timing::OVERVIEW_FADE`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cupertino_defaults::dock::{ActionMap, ActionTarget, DockSettings, Trigger};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::core::{Application, Command as Do, Key, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::motion::{SpringPreset, Timing};
use cupertino_kit::platform::{LayerChange, LayerProperty, WindowRequest};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::transaction::{self, Gesture, Outcome};
use cupertino_kit_shell::{
    bus, overview, Anchor, Component, ComponentName, Event, Hole, HoleSource, Keyboard, Layer,
    LayerSpec, Margin, Program, Role, ServerWindowId, Surface, WorkspaceId,
};
use cupertino_wm::geometry::Rect as WmRect;

use crate::chrome::{self, HolePlace, OverviewInput, Scene, APP_ID, CHROME_LAYER};
use crate::layout::{Mode, OverviewUi, UiAction, WindowEntry, Workspace};

/// The most holes the server shows on one surface: the desktops' pictures take theirs first, the windows the rest.
pub const MAX_HOLES: usize = 64;

/// How long the windows must keep still before the gesture is armed again with their new frames: a window dragged
/// or resized changes its frame every frame, and each arming makes a surface anew.
const REARM_QUIET: Duration = Duration::from_millis(250);

/// How long a hole whose window left the overview's set stays, for the server to fade it if the window closed
/// (`Event::HoleClosed`): a window that went to another desktop has no fade, and its hole goes after this.
const DEPARTED_GRACE: Duration = Duration::from_millis(500);

/// The overview.
pub struct Overview;

/// A value a message hands over once, though messages are cloned.
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

    /// The value, the first time.
    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// The gestures' actions of `dock.conf` as read, with the watch on the file the first time.
    Conf(ActionMap, Handed<Option<ConfigWatcher>>),
    ConfChanged,
    /// The windows, desktops and area of an output, and the person's toggle.
    Feed(OutputId, overview::Update),
    Outputs,
    /// Reduce Motion.
    ReduceMotion(bool),
    Transaction(transaction::Event),
    HoleClosed(WindowKey, u64),
    Closed(WindowKey),
    Input(WindowKey, OverviewInput),
    /// A frame of the display, or a moment something changes by itself.
    Frame,
    /// The windows of the output kept still since the change numbered so.
    Rearm(OutputId, u64),
    /// The grace of a hole whose window left the set ended.
    Departed(WindowKey, u32),
}

/// What the server last said of an output.
#[derive(Default)]
struct Feed {
    area: Option<Rect>,
    windows: Vec<overview::Window>,
    workspaces: Vec<overview::Workspace>,
    /// Counts the changes of the windows, so a re-arm waits for the last.
    changes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Staged for a gesture, not shown.
    Armed,
    /// On its way in.
    Opening,
    Open,
    /// On its way out.
    Closing,
}

/// A window's hole: where it starts (the window's own frame) and its place in the overview, in the output's points.
#[derive(Clone, Copy, Debug)]
struct HoleState {
    start: WmRect,
    target: WmRect,
    minimized: bool,
    /// The window left the set: its hole waits for the server's fade.
    departed: bool,
}

/// One overview surface.
struct View {
    key: WindowKey,
    output: OutputId,
    /// The output's origin and size in global points.
    origin: (f64, f64),
    size: Size,
    mode: Mode,
    app_id: String,
    ui: OverviewUi,
    phase: Phase,
    /// The transaction of its way in or out.
    tx: Option<transaction::Id>,
    handle: Option<transaction::Handle>,
    holes: BTreeMap<u32, HoleState>,
}

impl View {
    /// Where hole `id` is laid out now: at its start until the way in ended, then at its place (a dragged window
    /// where the pointer took it).
    fn place(&self, id: u32, hole: &HoleState) -> WmRect {
        match self.phase {
            Phase::Armed | Phase::Opening => hole.start,
            Phase::Open | Phase::Closing => self
                .ui
                .window_drag()
                .filter(|d| d.0 == id && self.phase == Phase::Open)
                .map_or(hole.target, |d| d.1),
        }
    }
}

pub struct Model {
    start: Instant,
    session: Option<zbus::Connection>,
    actions: Option<ActionMap>,
    reduce_motion: bool,
    feeds: HashMap<OutputId, Feed>,
    views: Vec<View>,
    next_view: u64,
    /// Transactions of window changes alone (a window moved to another desktop), kept until they end.
    errands: Vec<transaction::Handle>,
    /// Where the watch on `dock.conf` says the file changed.
    conf_changes: (async_channel::Sender<()>, async_channel::Receiver<()>),
}

/// The gestures that open the overview, the mode each opens, the action name `dock.conf` gives it and the gesture
/// that takes it back.
const GESTURES: [(Trigger, Gesture, Mode, &str, Gesture); 2] = [
    (
        Trigger::SwipeUp,
        Gesture::SwipeUp,
        Mode::All,
        "overview",
        Gesture::SwipeDown,
    ),
    (
        Trigger::SwipeDown,
        Gesture::SwipeDown,
        Mode::App,
        "app-overview",
        Gesture::SwipeUp,
    ),
];

/// The gestures' actions of `dock.conf`, the defaults without the file; reads it, so it runs on the I/O pool.
fn read_actions() -> ActionMap {
    let Some(path) = DockSettings::path() else {
        return ActionMap::default();
    };
    match DockSettings::load(&path, None) {
        Ok((settings, _)) => settings.actions,
        Err(e) => {
            eprintln!("cupertino-overview: {}: {e}", path.display());
            ActionMap::default()
        }
    }
}

/// Read the gestures' actions on the I/O pool; with `watch`, set up the watch on the file too.
fn read_conf(watch: bool) -> Do<Message> {
    cupertino_kit::task::io(
        cupertino_kit::task::SETTINGS_READ_DEADLINE,
        move || {
            let watcher = if watch {
                DockSettings::path().and_then(|path| match ConfigWatcher::new(vec![path]) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        eprintln!("cupertino-overview: cannot watch dock.conf: {e}");
                        None
                    }
                })
            } else {
                None
            };
            (read_actions(), watcher)
        },
        |(actions, watcher)| Message::Conf(actions, Handed::new(watcher)),
    )
}

/// The next change the watch on `dock.conf` reports.
fn listen(changes: &async_channel::Receiver<()>) -> Do<Message> {
    let changes = changes.clone();
    Do::Perform(Box::pin(async move {
        match changes.recv().await {
            Ok(()) => Message::ConfChanged,
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// `r` on the model's rectangle.
fn wm(r: Rect) -> WmRect {
    WmRect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// The changes that take a layer to `r` along `timing`.
fn frame_changes(r: WmRect, timing: Option<Timing>) -> Vec<LayerChange> {
    [
        (LayerProperty::X, r.x),
        (LayerProperty::Y, r.y),
        (LayerProperty::Width, r.w),
        (LayerProperty::Height, r.h),
    ]
    .into_iter()
    .map(|(p, v)| LayerChange::new(p, v, timing))
    .collect()
}

/// The changes that fade a layer in from nothing, or out.
fn fade_changes(fade_in: bool, timing: Option<Timing>) -> Vec<LayerChange> {
    if fade_in {
        vec![
            LayerChange::new(LayerProperty::Opacity, 0.0, None),
            LayerChange::new(LayerProperty::Opacity, 1.0, timing),
        ]
    } else {
        vec![LayerChange::new(LayerProperty::Opacity, 0.0, timing)]
    }
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn view(&self, key: &WindowKey) -> Option<usize> {
        self.views.iter().position(|v| &v.key == key)
    }

    fn by_tx(&self, id: transaction::Id) -> Option<usize> {
        self.views.iter().position(|v| v.tx == Some(id))
    }

    /// Whether an overview of `output` is shown or on its way.
    fn showing(&self, output: OutputId) -> bool {
        self.views
            .iter()
            .any(|v| v.output == output && v.phase != Phase::Armed)
    }

    /// A surface for `mode` on `output` in `phase`, from the server's last word on the output; none
    /// without an area, an output geometry, or for the application mode without a window of the application.
    fn make_view(
        &mut self,
        output: OutputId,
        mode: Mode,
        app_id: &str,
        phase: Phase,
    ) -> Option<View> {
        let feed = self.feeds.get(&output)?;
        let area = feed.area?;
        let geometry = outputs::get(output)?.geometry?;
        let origin = (geometry.origin.x, geometry.origin.y);
        let local = |r: Rect| wm(r).translate(-origin.0, -origin.1);
        let workspaces: Vec<Workspace> = feed
            .workspaces
            .iter()
            .map(|w| Workspace {
                id: w.id.0,
                current: w.current,
            })
            .collect();
        let room = MAX_HOLES.saturating_sub(workspaces.len());
        let entries: Vec<WindowEntry> = feed
            .windows
            .iter()
            .filter(|w| match mode {
                Mode::All => !w.minimized,
                Mode::App => w.app_id == app_id,
            })
            .take(room)
            .map(|w| WindowEntry {
                id: w.id.0,
                frame: local(w.frame),
                title: w.title.clone(),
                minimized: w.minimized,
            })
            .collect();
        if mode == Mode::App && entries.is_empty() {
            return None;
        }
        let ui = OverviewUi::new(mode, local(area), &entries, workspaces);
        let holes = ui
            .frames()
            .into_iter()
            .filter_map(|(id, target)| {
                let entry = entries.iter().find(|e| e.id == id)?;
                Some((
                    id,
                    HoleState {
                        // a minimized window has no place on the desktop: its picture fades in at its place
                        start: if entry.minimized { target } else { entry.frame },
                        target,
                        minimized: entry.minimized,
                        departed: false,
                    },
                ))
            })
            .collect();
        self.next_view += 1;
        let key = WindowKey(Key::from(format!("overview-{}", self.next_view)));
        Some(View {
            key,
            output,
            origin,
            size: geometry.size,
            mode,
            app_id: app_id.to_owned(),
            ui,
            phase,
            // set by the caller with the transaction it submits
            tx: None,
            handle: None,
            holes,
        })
    }

    /// The layer animations of a way in (`opening`) or out of `view`: holes to their places or back to their
    /// windows, the chrome faded in or out. Under `fade_only` the holes go there at once and the overview fades.
    fn motion(view: &View, opening: bool, fade_only: bool) -> Vec<Do<Message>> {
        let (spatial, fade) = if fade_only {
            (None, Some(Timing::OVERVIEW_FADE))
        } else {
            let spring: Timing = SpringPreset::OVERVIEW_OPEN.into();
            (Some(spring), Some(spring))
        };
        let mut out = Vec::new();
        let mut layer = |key: u64, changes: Vec<LayerChange>| {
            out.push(Do::Window(
                view.key.clone(),
                WindowRequest::Layer { key, changes },
            ));
        };
        for (&id, hole) in &view.holes {
            let changes = if hole.minimized || fade_only {
                let mut c = fade_changes(opening, fade);
                c.extend(frame_changes(hole.target, None));
                c
            } else {
                frame_changes(if opening { hole.target } else { hole.start }, spatial)
            };
            layer(chrome::window_layer(id), changes);
        }
        layer(CHROME_LAYER, fade_changes(opening, fade));
        out
    }

    /// Arm the gestures of `output` that `dock.conf` gives the overview, unless an overview is shown there.
    fn arm(&mut self, output: OutputId) -> Vec<Do<Message>> {
        let Some(actions) = self.actions.clone() else {
            return Vec::new();
        };
        if self.showing(output) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (trigger, gesture, mode, action, reverse) in GESTURES {
            if actions.get(trigger) != &ActionTarget::Server(action.to_owned())
                || self
                    .views
                    .iter()
                    .any(|v| v.output == output && v.mode == mode)
            {
                continue;
            }
            let app_id = match mode {
                Mode::All => String::new(),
                Mode::App => match self
                    .feeds
                    .get(&output)
                    .and_then(|f| f.windows.iter().find(|w| w.active))
                {
                    Some(w) => w.app_id.clone(),
                    None => continue,
                },
            };
            let Some(mut view) = self.make_view(output, mode, &app_id, Phase::Armed) else {
                continue;
            };
            // the surface comes with this view: its first commit is kept for the gesture, with the way in
            let mut part = transaction::create(output);
            part.bind(gesture, SpringPreset::OVERVIEW_OPEN);
            part.bind_reverse(reverse);
            part.stage(&view.key);
            view.tx = Some(part.id());
            view.handle = Some(part.submit());
            out.extend(Self::motion(&view, true, false));
            self.views.push(view);
        }
        out
    }

    /// Give up the armed gestures of `output`: their surfaces go with their transactions.
    fn disarm(&mut self, output: OutputId) {
        self.views
            .retain(|v| !(v.output == output && v.phase == Phase::Armed));
    }

    /// The person's toggle: close the overview shown on `output`, else open one in `mode`.
    fn toggle(&mut self, output: OutputId, toggle: &overview::Toggle) -> Vec<Do<Message>> {
        if let Some(i) = self
            .views
            .iter()
            .position(|v| v.output == output && matches!(v.phase, Phase::Opening | Phase::Open))
        {
            return self.close(i, Leave::Back);
        }
        if self.showing(output) {
            return Vec::new();
        }
        let mode = match toggle.mode {
            overview::Mode::All => Mode::All,
            overview::Mode::App => Mode::App,
        };
        let Some(mut view) = self.make_view(output, mode, &toggle.app_id, Phase::Opening) else {
            return Vec::new();
        };
        self.disarm(output);
        let mut part = transaction::create(output);
        part.stage(&view.key);
        view.tx = Some(part.id());
        view.handle = Some(part.submit());
        let out = Self::motion(&view, true, self.reduce_motion);
        self.views.push(view);
        out
    }

    /// Take view `i` out: back to the windows' frames, with what `leave` asks carried out at the start.
    fn close(&mut self, i: usize, leave: Leave) -> Vec<Do<Message>> {
        let fade_only = self.reduce_motion;
        let view = &mut self.views[i];
        if matches!(view.phase, Phase::Armed | Phase::Closing) {
            return Vec::new();
        }
        view.phase = Phase::Closing;
        view.ui.close();
        let output = view.output;
        // the transaction that opened it, and the reverse gesture it kept, end here
        view.handle = None;
        let mut part = transaction::create(output);
        match leave {
            Leave::Back => {}
            Leave::Pick(id) => part.window_changes(|| {
                overview::activate_window(output, ServerWindowId(id));
            }),
            Leave::Desktop(id) => part.window_changes(|| {
                overview::switch_workspace(output, WorkspaceId(id));
            }),
        }
        part.stage(&view.key);
        view.tx = Some(part.id());
        view.handle = Some(part.submit());
        Self::motion(view, false, fade_only)
    }

    /// The server's word on `output`: a shown overview follows the windows and desktops, armed gestures are armed
    /// again once the windows keep still, and a toggle opens or closes.
    fn feed(&mut self, output: OutputId, update: overview::Update) -> Vec<Do<Message>> {
        let feed = self.feeds.entry(output).or_default();
        let changed = feed.area != update.area
            || feed.windows != update.windows
            || feed.workspaces != update.workspaces;
        feed.area = update.area;
        feed.windows = update.windows;
        feed.workspaces = update.workspaces;
        let mut out = Vec::new();
        if changed {
            feed.changes += 1;
            let changes = feed.changes;
            let armed = self
                .views
                .iter()
                .any(|v| v.output == output && v.phase == Phase::Armed);
            self.disarm(output);
            if armed {
                out.push(Do::After {
                    delay: REARM_QUIET,
                    message: Message::Rearm(output, changes),
                });
            } else {
                out.extend(self.arm(output));
            }
            out.extend(self.follow(output));
        }
        if let Some(toggle) = update.toggle {
            out.extend(self.toggle(output, &toggle));
        }
        out
    }

    /// A shown overview of `output` takes the server's windows and desktops: holes move to their new places, a
    /// new window gets one, a window that left keeps its hole for the server's fade.
    fn follow(&mut self, output: OutputId) -> Vec<Do<Message>> {
        let Some(feed) = self.feeds.get(&output) else {
            return Vec::new();
        };
        let reflow: Timing = SpringPreset::OVERVIEW_REFLOW.into();
        let mut out = Vec::new();
        for view in self
            .views
            .iter_mut()
            .filter(|v| v.output == output && v.phase != Phase::Armed)
        {
            let origin = view.origin;
            let local = |r: Rect| wm(r).translate(-origin.0, -origin.1);
            let room = MAX_HOLES.saturating_sub(feed.workspaces.len());
            let entries: Vec<WindowEntry> = feed
                .windows
                .iter()
                .filter(|w| match view.mode {
                    Mode::All => !w.minimized,
                    Mode::App => w.app_id == view.app_id,
                })
                .take(room)
                .map(|w| WindowEntry {
                    id: w.id.0,
                    frame: local(w.frame),
                    title: w.title.clone(),
                    minimized: w.minimized,
                })
                .collect();
            let now = self.start.elapsed().as_secs_f64();
            view.ui.set_windows(&entries, now);
            view.ui.set_workspaces(
                feed.workspaces
                    .iter()
                    .map(|w| Workspace {
                        id: w.id.0,
                        current: w.current,
                    })
                    .collect(),
            );
            out.extend(Self::reflow(view, &entries, reflow));
        }
        out
    }

    /// Move the holes of `view` to the layout's places: a moved one along `reflow`, a new one there at once, one
    /// whose window left waits for the server's fade.
    fn reflow(view: &mut View, entries: &[WindowEntry], reflow: Timing) -> Vec<Do<Message>> {
        let mut out = Vec::new();
        let frames = view.ui.frames();
        let shown = matches!(view.phase, Phase::Open);
        for (id, target) in &frames {
            let entry = entries.iter().find(|e| e.id == *id);
            match view.holes.get_mut(id) {
                Some(hole) => {
                    hole.departed = false;
                    if hole.target != *target {
                        hole.target = *target;
                        if shown {
                            out.push(Do::Window(
                                view.key.clone(),
                                WindowRequest::Layer {
                                    key: chrome::window_layer(*id),
                                    changes: frame_changes(*target, Some(reflow)),
                                },
                            ));
                        }
                    }
                    if let Some(e) = entry.filter(|e| !e.minimized) {
                        hole.start = e.frame;
                    }
                }
                None => {
                    let minimized = entry.is_some_and(|e| e.minimized);
                    view.holes.insert(
                        *id,
                        HoleState {
                            start: entry.filter(|e| !e.minimized).map_or(*target, |e| e.frame),
                            target: *target,
                            minimized,
                            departed: false,
                        },
                    );
                }
            }
        }
        for (&id, hole) in view.holes.iter_mut() {
            if !hole.departed && !frames.iter().any(|f| f.0 == id) {
                hole.departed = true;
                out.push(Do::After {
                    delay: DEPARTED_GRACE,
                    message: Message::Departed(view.key.clone(), id),
                });
            }
        }
        out
    }

    /// A transaction of a view moved on: the way in ended (open, or gone when reverted), the way out ended (gone).
    fn transaction(&mut self, event: transaction::Event) -> Vec<Do<Message>> {
        match event {
            transaction::Event::GestureBegan { id, .. } => {
                if let Some(i) = self.by_tx(id) {
                    let view = &mut self.views[i];
                    if view.phase == Phase::Armed {
                        view.phase = Phase::Opening;
                        let (output, key) = (view.output, view.key.clone());
                        // the other gesture's surface waits until this overview is gone
                        self.views.retain(|v| {
                            v.key == key || !(v.output == output && v.phase == Phase::Armed)
                        });
                    }
                }
                Vec::new()
            }
            transaction::Event::Finished { id, outcome } => {
                if let Some(i) = self.errands.iter().position(|h| h.id() == id) {
                    self.errands.remove(i);
                    return Vec::new();
                }
                let Some(i) = self.by_tx(id) else {
                    return Vec::new();
                };
                let view = &mut self.views[i];
                match (view.phase, outcome) {
                    (Phase::Armed | Phase::Opening, Outcome::Completed) => {
                        view.phase = Phase::Open;
                        Vec::new()
                    }
                    // a completed gesture's transaction stays for the reverse gesture
                    (Phase::Open, Outcome::Completed) => Vec::new(),
                    _ => {
                        let output = view.output;
                        self.views.remove(i);
                        self.arm(output)
                    }
                }
            }
            transaction::Event::Started { .. } | transaction::Event::GestureEnded { .. } => {
                Vec::new()
            }
        }
    }

    /// What the window of view `i` reported.
    fn input(&mut self, i: usize, input: OverviewInput) -> Vec<Do<Message>> {
        let now = self.now();
        let view = &mut self.views[i];
        if view.phase != Phase::Open {
            return Vec::new();
        }
        let dragged = view.ui.window_drag().map(|d| d.0);
        let action = chrome::action(&mut view.ui, input, now);
        let output = view.output;
        let mut out = Vec::new();
        // a dragged window dropped where nothing takes it goes back to its place
        if let Some(id) = dragged.filter(|_| view.ui.window_drag().is_none()) {
            if !matches!(action, UiAction::MoveWindow { .. }) {
                if let Some(hole) = view.holes.get(&id) {
                    out.push(Do::Window(
                        view.key.clone(),
                        WindowRequest::Layer {
                            key: chrome::window_layer(id),
                            changes: frame_changes(
                                hole.target,
                                Some(SpringPreset::OVERVIEW_REFLOW.into()),
                            ),
                        },
                    ));
                }
            }
        }
        if let Some(at) = view.ui.next_change(now) {
            out.push(Do::After {
                delay: Duration::from_secs_f64((at - now).max(0.0)),
                message: Message::Frame,
            });
        }
        match action {
            UiAction::None => {}
            UiAction::Select(id) => out.extend(self.close(i, Leave::Pick(id))),
            UiAction::Close => out.extend(self.close(i, Leave::Back)),
            UiAction::SwitchWorkspace(id) => out.extend(self.close(i, Leave::Desktop(id))),
            UiAction::AddWorkspace => overview::add_workspace(output),
            UiAction::RemoveWorkspace(id) => overview::remove_workspace(output, WorkspaceId(id)),
            UiAction::MoveWorkspace { id, index } => overview::move_workspace(
                output,
                WorkspaceId(id),
                u32::try_from(index).unwrap_or(u32::MAX),
            ),
            UiAction::MoveWindow { window, workspace } => {
                let mut part = transaction::create(output);
                part.window_changes(|| {
                    overview::move_window(output, ServerWindowId(window), WorkspaceId(workspace));
                });
                self.errands.push(part.submit());
            }
        }
        out
    }

    /// The outputs changed: views of an output that went, or moved, go; the gestures are armed on what is left.
    fn outputs(&mut self) -> Vec<Do<Message>> {
        let all = outputs::all();
        let geometry = |id: OutputId| all.iter().find(|o| o.id == id).and_then(|o| o.geometry);
        self.feeds.retain(|id, _| geometry(*id).is_some());
        self.views.retain(|v| {
            geometry(v.output)
                .is_some_and(|g| (g.origin.x, g.origin.y) == v.origin && g.size == v.size)
        });
        let outputs: Vec<OutputId> = self.feeds.keys().copied().collect();
        outputs.into_iter().flat_map(|o| self.arm(o)).collect()
    }
}

/// What a way out carries out at its start.
#[derive(Clone, Copy, Debug)]
enum Leave {
    Back,
    /// Activate this window.
    Pick(u32),
    /// Switch to this desktop.
    Desktop(u32),
}

impl Application for Overview {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The surface covers the output, but the server draws the windows and the desktops in its holes: the component
    /// draws only the band and the rings, so a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(read_conf(true));
        let model = Model {
            start: Instant::now(),
            session: None,
            actions: None,
            reduce_motion: false,
            feeds: HashMap::new(),
            views: Vec::new(),
            next_view: 0,
            errands: Vec::new(),
            conf_changes: async_channel::unbounded(),
        };
        commands.push(listen(&model.conf_changes.1));
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        match message {
            Message::Session(Some(connection)) => {
                // the connection carries the component's debug interface and its identity
                model.session = Some(connection);
                Vec::new()
            }
            Message::Session(None) | Message::SessionClosed => {
                // the overview calls nothing on the bus: without it, it still works
                model.session = None;
                Vec::new()
            }
            Message::Conf(actions, watcher) => {
                let mut out = Vec::new();
                if let Some(watch) = watcher.take().flatten() {
                    let changes = model.conf_changes.0.clone();
                    out.push(Do::Perform(Box::pin(cupertino_kit::task::follow_settings(
                        watch,
                        move |event| match event {
                            WatchEvent::Changed(_) => {
                                let _ = changes.try_send(());
                            }
                            WatchEvent::Error(e) => {
                                eprintln!("cupertino-overview: watching dock.conf failed: {e}");
                            }
                        },
                    ))));
                }
                if model.actions.as_ref() != Some(&actions) {
                    model.actions = Some(actions);
                    let outputs: Vec<OutputId> = model.feeds.keys().copied().collect();
                    for output in outputs {
                        model.disarm(output);
                        out.extend(model.arm(output));
                    }
                }
                out
            }
            Message::ConfChanged => vec![read_conf(false), listen(&model.conf_changes.1)],
            Message::Feed(output, update) => model.feed(output, update),
            Message::Outputs => model.outputs(),
            Message::ReduceMotion(on) => {
                model.reduce_motion = on;
                Vec::new()
            }
            Message::Transaction(event) => model.transaction(event),
            Message::HoleClosed(key, layer) => {
                if let (Some(i), Some(id)) = (model.view(&key), chrome::layer_window(layer)) {
                    model.views[i].holes.remove(&id);
                }
                Vec::new()
            }
            Message::Departed(key, id) => {
                if let Some(i) = model.view(&key) {
                    let holes = &mut model.views[i].holes;
                    if holes.get(&id).is_some_and(|h| h.departed) {
                        holes.remove(&id);
                    }
                }
                Vec::new()
            }
            Message::Closed(key) => {
                // the server closed the surface: its gestures are armed anew
                let Some(i) = model.view(&key) else {
                    return Vec::new();
                };
                let output = model.views.remove(i).output;
                model.arm(output)
            }
            Message::Input(key, input) => match model.view(&key) {
                Some(i) => model.input(i, input),
                None => Vec::new(),
            },
            // the view draws from the model's clock; this only makes it look again
            Message::Frame => Vec::new(),
            Message::Rearm(output, changes) => {
                if model
                    .feeds
                    .get(&output)
                    .is_some_and(|f| f.changes == changes)
                {
                    model.arm(output)
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let now = model.now();
        model
            .views
            .iter()
            .map(|view| {
                let holes = view
                    .holes
                    .iter()
                    .map(|(&id, hole)| HolePlace {
                        id,
                        rect: view.place(id, hole),
                    })
                    .collect();
                let scene = Scene {
                    key: view.key.clone(),
                    size: view.size,
                    ui: &view.ui,
                    holes,
                    reduce_motion: model.reduce_motion,
                    hole_fill: None,
                    chrome_opacity: 1.0,
                };
                let key = view.key.clone();
                let frame = (view.phase == Phase::Open && view.ui.is_animating(now))
                    .then_some(Message::Frame);
                chrome::window(
                    &scene,
                    now,
                    move |input| Message::Input(key.clone(), input),
                    frame,
                )
            })
            .collect()
    }
}

impl Program for Overview {
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let view = model.view(window).map(|i| &model.views[i]);
        let now = model.now();
        let mut holes = Vec::new();
        if let Some(view) = view {
            if let Some(bar) = view.ui.bar(now) {
                holes.extend(
                    bar.slots
                        .iter()
                        .filter(|s| s.thumb.w >= 1.0 && s.thumb.h >= 1.0)
                        .map(|s| Hole {
                            layer: chrome::workspace_layer(s.id.0 as u32),
                            source: HoleSource::Workspace(WorkspaceId(s.id.0 as u32)),
                            claim: false,
                        }),
                );
            }
            holes.extend(view.holes.iter().map(|(&id, hole)| Hole {
                layer: chrome::window_layer(id),
                source: if hole.minimized {
                    HoleSource::Thumbnail(ServerWindowId(id))
                } else {
                    HoleSource::Window(ServerWindowId(id))
                },
                claim: !hole.minimized,
            }));
        }
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: APP_ID,
                output: view.map(|v| v.output),
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
                keyboard: if view.is_some_and(|v| v.phase == Phase::Open) {
                    Keyboard::Exclusive
                } else {
                    Keyboard::None
                },
            }),
            persistent: None,
            holes,
        }
    }

    fn event(event: Event) -> Option<Message> {
        Some(match event {
            Event::Overview(output, update) => Message::Feed(output, update),
            Event::Outputs => Message::Outputs,
            Event::Appearance(appearance) => Message::ReduceMotion(appearance.reduce_motion),
            Event::Transaction(event) => Message::Transaction(event),
            Event::HoleClosed { window, layer } => Message::HoleClosed(window, layer),
            Event::Closed(window) => Message::Closed(window),
            _ => return None,
        })
    }
}

impl Component for Overview {
    const NAME: ComponentName = ComponentName::OVERVIEW;
}
