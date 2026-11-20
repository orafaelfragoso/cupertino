//! The roles of a shell program's windows ([`crate::Surface`]): each window's surface gets the role its program's
//! `surface` names when it opens (a layer surface, the menu bar or the dock of an output, a lock surface), persistent
//! ones are persisted before their first buffer, and holes follow the window's surface layers. What the shell's
//! objects say becomes the program's [`Event`]s; the requests the program queued go out before the loop waits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use cupertino_kit::core::WindowKey;
use cupertino_kit::geometry::Rect;
use cupertino_kit::prelude::WindowSpec;
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::cupertino_window_manager_v1::CupertinoWindowManagerV1;
use cupertino_protocols::client::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1;
use cupertino_protocols::client::overview::cupertino_overview_v1::CupertinoOverviewV1;
use cupertino_protocols::client::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1;
use cupertino_protocols::client::transaction::cupertino_hole_v1::CupertinoHoleV1;
use cupertino_protocols::client::transaction::cupertino_transaction_manager_v1::CupertinoTransactionManagerV1;
use cupertino_protocols::client::transaction::cupertino_transaction_v1::CupertinoTransactionV1;
use cupertino_shell_client::{
    Anchor as WireAnchor, ControlAuthority, ControlEvent, Dock, DockChange, IdleNotifier,
    IdleWatch, KeyboardInteractivity, Layer as WireLayer, LayerShell, LayerSurface,
    LayerSurfaceConfig, Margin as WireMargin, MenuBar, Session, SessionChange, SessionLock,
    SessionLockManager, SessionState, Shell, ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles};
use cupertino_wayland_client::{Connection, Error as WireError, ExtensionEvent, SurfaceId};

use super::globals;
use super::protocols::{OverviewData, Quiet, WindowManagerData, Wire};
use super::Shared;
use crate::context::{self, Inbox};
use crate::link::ComponentLink;
use crate::outputs::{Output, OutputId, Transform};
use crate::{
    consent, lock, session, transaction, ComponentName, Event, Hole, Keyboard, Layer, LayerSpec,
    Role, Surface,
};

/// What the roles are set up with.
pub(crate) struct Binding {
    /// The component the loop runs; `None` for a program of its own.
    pub component: Option<ComponentName>,
    pub shared: Rc<RefCell<Shared>>,
    /// The record of when the loop left its wait, that a host's watchdog reads.
    pub busy: Option<Arc<Mutex<Option<Instant>>>>,
}

/// The shell's globals on the connection, those it offers.
#[derive(Default)]
pub(super) struct Globals {
    pub layer_shell: Option<LayerShell>,
    pub shell: Option<Shell>,
    /// The runtime's own binding of `cupertino_shell_v1`, for the window manager role.
    pub window_managers: Option<CupertinoShellV1>,
    /// The session observer: appearance, display options, the lock and the person's requests.
    pub session: Option<Session>,
    pub lock_manager: Option<SessionLockManager>,
    pub idle: Option<IdleNotifier>,
    pub transactions: Option<CupertinoTransactionManagerV1>,
    pub overviews: Option<CupertinoOverviewManagerV1>,
    /// Bound when the program becomes the consent authority (`consent::bind`).
    pub consent: Option<ControlAuthority>,
    /// Bound when the program follows the windows on screen (`capture::windows`).
    pub windows: Option<CupertinoScreenshotV1>,
}

/// A hole of a window's surface layer (`cupertino_hole_v1`), with what it was last told.
pub(super) struct HoleObject {
    pub proxy: CupertinoHoleV1,
    pub number: u64,
    pub hole: Hole,
}

/// One window of the program, with what its role made of it.
pub(super) struct Win {
    pub key: WindowKey,
    /// Its surface; a toplevel's is known from its first commit.
    pub surface: Option<SurfaceId>,
    pub layer: Option<LayerSurface>,
    /// The placement last given to its layer surface.
    pub spec: Option<LayerSpec>,
    pub menu_bar: Option<MenuBar>,
    pub dock: Option<Dock>,
    /// Its holes, by the key of the surface layer that is the hole.
    pub holes: HashMap<u64, HoleObject>,
    /// The entry of the dock's application switcher selected last.
    pub switcher: usize,
    /// The tile each minimized window of the dock goes to and comes back from.
    pub minimize_targets: HashMap<u32, Rect>,
}

impl Win {
    fn new(key: WindowKey) -> Self {
        Self {
            key,
            surface: None,
            layer: None,
            spec: None,
            menu_bar: None,
            dock: None,
            holes: HashMap::new(),
            switcher: 0,
            minimize_targets: HashMap::new(),
        }
    }
}

/// The roles of a shell program's windows and its other objects on the connection.
pub(crate) struct ShellRoles {
    pub(super) component: Option<ComponentName>,
    shared: Rc<RefCell<Shared>>,
    pub(super) inbox: Arc<Inbox>,
    link: Option<Arc<ComponentLink>>,
    busy: Option<Arc<Mutex<Option<Instant>>>>,
    pub(super) globals: Globals,
    observed: SessionState,
    /// The key of the window about to open.
    opening: Option<WindowKey>,
    pub(super) windows: Vec<Win>,
    ready_sent: bool,
    /// The session lock the program holds or asked for.
    pub(super) lock: Option<SessionLock>,
    pub(super) idle_watches: HashMap<u64, IdleWatch>,
    pub(super) window_managers: HashMap<OutputId, CupertinoWindowManagerV1>,
    pub(super) overviews: HashMap<OutputId, CupertinoOverviewV1>,
    pub(super) transactions: HashMap<transaction::Id, CupertinoTransactionV1>,
    /// Windows staged in a transaction before their first commit: staged with it.
    pub(super) pending_stages: Vec<(transaction::Id, WindowKey)>,
    /// The window the person answers consent on, until its surface is known.
    pub(super) consent_window: Option<WindowKey>,
    /// The consent runs by the number the program knows them by, and the epoch the server does.
    pub(super) consent_runs: HashMap<u32, u64>,
    pub(super) next_run: u32,
    /// The app of each consent request, which the server names before the request.
    pub(super) consent_apps: HashMap<u64, consent::App>,
    pub(super) next_hole: u64,
    pub(super) captures: super::capture::Capturing,
}

impl ShellRoles {
    /// Binds the shell's globals `conn` offers, observes the session, and follows the outputs.
    pub(crate) fn bind(conn: &mut Connection, binding: Binding) -> ShellRoles {
        let (inbox, link) =
            context::with_ctx(|ctx| (Arc::clone(&ctx.inbox), ctx.link.clone())).unwrap_or_default();
        let shell = Shell::bind(conn);
        let session = shell.as_ref().map(|shell| shell.session(conn));
        let component = binding.component;
        let window_managers = (component == Some(ComponentName::WINDOWMANAGER))
            .then(|| {
                conn.bind_extension::<CupertinoShellV1, _>(
                    globals::SHELL_SESSION_VERSION..=globals::SHELL_DOCK_VERSION,
                    Quiet,
                )
            })
            .flatten();
        let overviews = (component == Some(ComponentName::OVERVIEW))
            .then(|| {
                conn.bind_extension::<CupertinoOverviewManagerV1, _>(
                    globals::OVERVIEW_VERSION..=globals::OVERVIEW_VERSION,
                    Quiet,
                )
            })
            .flatten();
        let globals = Globals {
            layer_shell: LayerShell::bind(conn),
            shell,
            window_managers,
            session,
            lock_manager: SessionLockManager::bind(conn),
            idle: IdleNotifier::bind(conn),
            transactions: conn.bind_extension::<CupertinoTransactionManagerV1, _>(
                globals::TRANSACTION_VERSION..=globals::TRANSACTION_VERSION,
                Quiet,
            ),
            overviews,
            consent: None,
            windows: None,
        };
        let mut roles = ShellRoles {
            component,
            shared: binding.shared,
            inbox,
            link,
            busy: binding.busy,
            globals,
            observed: SessionState::default(),
            opening: None,
            windows: Vec::new(),
            ready_sent: false,
            lock: None,
            idle_watches: HashMap::new(),
            window_managers: HashMap::new(),
            overviews: HashMap::new(),
            transactions: HashMap::new(),
            pending_stages: Vec::new(),
            consent_window: None,
            consent_runs: HashMap::new(),
            next_run: 1,
            consent_apps: HashMap::new(),
            next_hole: 1,
            captures: super::capture::Capturing::bind(conn),
        };
        roles.follow_outputs(conn, false);
        roles
    }

    /// The outputs as the program reads them ([`crate::outputs::all`]), and the window manager and overview objects of
    /// each; `tell` sends the program [`Event::Outputs`].
    fn follow_outputs(&mut self, conn: &Connection, tell: bool) {
        let outputs: Vec<Output> = conn.outputs().into_iter().map(output).collect();
        let present: Vec<OutputId> = outputs.iter().map(|o| o.id).collect();
        let _ = context::with_ctx_mut(|ctx| ctx.outputs = outputs);
        self.window_managers.retain(|id, wm| {
            let keep = present.contains(id);
            if !keep {
                wm.destroy();
            }
            keep
        });
        self.overviews.retain(|id, overview| {
            let keep = present.contains(id);
            if !keep {
                overview.destroy();
            }
            keep
        });
        let qh = conn.queue_handle();
        for &id in &present {
            let Some(wl_output) = conn.wl_output(wire_output(id)) else {
                continue;
            };
            if let Some(shell) = &self.globals.window_managers {
                self.window_managers.entry(id).or_insert_with(|| {
                    shell.get_window_manager(&wl_output, &qh, WindowManagerData::new(id))
                });
            }
            if let Some(manager) = &self.globals.overviews {
                self.overviews.entry(id).or_insert_with(|| {
                    manager.get_overview(&wl_output, &qh, OverviewData::new(id))
                });
            }
        }
        if tell {
            self.inbox.push(Event::Outputs);
        }
    }

    pub(super) fn win_of_key(&mut self, key: &WindowKey) -> Option<&mut Win> {
        self.windows.iter_mut().find(|w| &w.key == key)
    }

    fn win_of_surface(&mut self, surface: SurfaceId) -> Option<&mut Win> {
        self.windows.iter_mut().find(|w| w.surface == Some(surface))
    }

    /// The surface the program's last view gave `key`; a window it did not describe is Kit's own.
    fn surface_of(&self, key: &WindowKey) -> Surface {
        self.shared
            .borrow()
            .surfaces
            .get(key)
            .cloned()
            .unwrap_or(Surface {
                role: Role::Window,
                persistent: None,
                holes: Vec::new(),
            })
    }

    /// A layer surface for `key`, with the menu bar or dock role `role` asks for, persisted under `persistent`.
    fn create_layer(
        &mut self,
        conn: &mut Connection,
        key: WindowKey,
        role: &Role,
        persistent: Option<&'static str>,
    ) -> Result<SurfaceId, WireError> {
        let (Role::Layer(spec) | Role::MenuBar(spec) | Role::Dock(spec)) = role else {
            return Err(WireError::Protocol("not a layer role"));
        };
        let unsupported = |what| WireError::Protocol(what);
        let layer_shell = self
            .globals
            .layer_shell
            .as_ref()
            .ok_or_else(|| unsupported("the server offers no layer shell"))?;
        let mut config = layer_config(spec);
        let role_output = match role {
            Role::MenuBar(_) | Role::Dock(_) => {
                let output = config
                    .output
                    .or_else(|| conn.outputs().first().map(|o| o.id))
                    .ok_or_else(|| unsupported("the server has no output"))?;
                config.output = Some(output);
                Some(output)
            }
            _ => None,
        };
        let layer = layer_shell
            .layer_surface(conn, &config)
            .map_err(transport)?;
        let surface = layer.surface();
        let mut win = Win::new(key);
        win.surface = Some(surface);
        win.spec = Some(spec.clone());
        if let (Some(output), Some(shell)) = (role_output, &self.globals.shell) {
            match role {
                Role::MenuBar(_) => {
                    win.menu_bar = Some(shell.menu_bar(conn, surface, output).map_err(transport)?);
                }
                _ => win.dock = Some(shell.dock(conn, surface, output).map_err(transport)?),
            }
        }
        win.layer = Some(layer);
        if let (Some(persistent), Some(link)) = (persistent, &self.link) {
            // before the surface's first buffer, which the window's first frame attaches
            link.persist(&conn.wl_surface(surface)?, persistent);
        }
        self.windows.push(win);
        Ok(surface)
    }

    /// A lock surface of `output` for `key`, under the session lock the program holds.
    fn create_lock(
        &mut self,
        conn: &mut Connection,
        key: WindowKey,
        output: OutputId,
    ) -> Result<SurfaceId, WireError> {
        let lock = self.lock.as_ref().ok_or(WireError::Protocol(
            "a lock surface without the session lock",
        ))?;
        let surface = lock
            .lock_surface(conn, wire_output(output))
            .map_err(transport)?;
        let mut win = Win::new(key);
        win.surface = Some(surface);
        self.windows.push(win);
        Ok(surface)
    }

    /// Sends `ready` once every persistent window of the first view presented a frame.
    fn ready_if_shown(&mut self) {
        if self.ready_sent {
            return;
        }
        let shown = self
            .shared
            .borrow()
            .awaiting
            .as_ref()
            .is_some_and(|awaiting| awaiting.is_empty());
        if shown {
            self.ready_sent = true;
            if let Some(link) = &self.link {
                link.ready();
            }
        }
    }

    /// `key` presented a frame, or closed: the first view's persistent windows wait for it no more.
    fn shown(&mut self, key: &WindowKey) {
        if let Some(awaiting) = self.shared.borrow_mut().awaiting.as_mut() {
            awaiting.remove(key);
        }
        self.ready_if_shown();
    }

    /// The placement `spec` asks of `win`'s layer surface, applied with its next commit where it changed.
    fn place(win: &mut Win, spec: &LayerSpec) {
        let (Some(layer), Some(applied)) = (win.layer.as_mut(), win.spec.as_ref()) else {
            return;
        };
        if applied == spec {
            return;
        }
        if applied.anchor != spec.anchor || applied.size != spec.size {
            layer.set_placement(wire_anchor(spec.anchor), whole_size(spec));
        }
        if applied.margin != spec.margin {
            layer.set_margin(wire_margin(spec));
        }
        if applied.exclusive_zone != spec.exclusive_zone {
            layer.set_exclusive_zone(spec.exclusive_zone);
        }
        if applied.keyboard != spec.keyboard {
            layer.set_keyboard_interactivity(wire_keyboard(spec.keyboard));
        }
        win.spec = Some(spec.clone());
    }

    /// The holes `surface` returns for the window, on its surface layers `layers`: made, changed or destroyed with
    /// the window's next frame.
    fn follow_holes(
        &mut self,
        conn: &Connection,
        key: &WindowKey,
        holes: &[Hole],
        layers: &[(u64, SurfaceId)],
    ) {
        let Some(manager) = self.globals.transactions.clone() else {
            return;
        };
        let qh = conn.queue_handle();
        let mut next_hole = self.next_hole;
        let Some(win) = self.win_of_key(key) else {
            return;
        };
        win.holes.retain(|layer, object| {
            let keep =
                holes.iter().any(|h| h.layer == *layer) && layers.iter().any(|(l, _)| l == layer);
            if !keep {
                object.proxy.destroy();
            }
            keep
        });
        for hole in holes {
            let Some(&(_, layer_surface)) = layers.iter().find(|(l, _)| *l == hole.layer) else {
                continue;
            };
            let (source, id) = hole_source(hole.source);
            match win.holes.get_mut(&hole.layer) {
                Some(object) => {
                    if object.hole.source != hole.source {
                        object.proxy.set_source(source, id);
                    }
                    if object.hole.claim != hole.claim {
                        object.proxy.set_claim(u32::from(hole.claim));
                    }
                    object.hole = *hole;
                }
                None => {
                    let Ok(wl) = conn.wl_surface(layer_surface) else {
                        continue;
                    };
                    let number = next_hole;
                    next_hole += 1;
                    let proxy = manager.get_hole(&wl, &qh, super::protocols::HoleData(number));
                    proxy.set_source(source, id);
                    proxy.set_claim(u32::from(hole.claim));
                    win.holes.insert(
                        hole.layer,
                        HoleObject {
                            proxy,
                            number,
                            hole: *hole,
                        },
                    );
                }
            }
        }
        self.next_hole = next_hole;
    }

    /// What the shell said, as the program's event; `None` when the event is not one for the program.
    fn shell_event(&mut self, event: &ShellEvent) -> Option<RoleEvent> {
        match event {
            &ShellEvent::Configure { surface, size } => {
                let win = self.win_of_surface(surface)?;
                let size = match &win.layer {
                    Some(layer) => match layer.configured(size) {
                        Ok(size) => size,
                        // a configure without a size closes the window
                        Err(_) => return Some(self.closed(surface)),
                    },
                    None => size,
                };
                Some(RoleEvent::Configure { surface, size })
            }
            &ShellEvent::Closed(surface) => {
                self.win_of_surface(surface)?;
                Some(self.closed(surface))
            }
            &ShellEvent::Lock { generation, locked } => {
                if self.lock.as_ref().map(SessionLock::generation) != Some(generation) {
                    return None;
                }
                if locked {
                    self.inbox.push(Event::Lock(lock::Event::Locked));
                } else {
                    if let Some(lock) = self.lock.take() {
                        lock.destroy();
                    }
                    self.inbox.push(Event::Lock(lock::Event::Finished));
                }
                None
            }
            &ShellEvent::Idle(idle) => self.session(session::Change::Idle(idle)),
            &ShellEvent::LockRequested { explicit } => {
                self.session(session::Change::LockRequested { explicit })
            }
            ShellEvent::LogOutRequested => self.session(session::Change::LogOutRequested),
            &ShellEvent::AppActivation { serial, result } => {
                let result = match result {
                    cupertino_shell_client::AppActivation::Activated => {
                        session::Activation::Activated
                    }
                    cupertino_shell_client::AppActivation::Resident => {
                        session::Activation::Resident
                    }
                    cupertino_shell_client::AppActivation::NotRunning => {
                        session::Activation::NotRunning
                    }
                };
                self.session(session::Change::AppActivation { serial, result })
            }
            ShellEvent::MenuBar { surface, update } => {
                let key = self.win_of_surface(*surface)?.key.clone();
                self.inbox.push(Event::MenuBar(key, update.clone()));
                None
            }
            ShellEvent::Dock { surface, update } => {
                let win = self.win_of_surface(*surface)?;
                for change in update {
                    if let DockChange::SwitcherChanged { selected, .. } = change {
                        win.switcher = *selected;
                    }
                }
                let key = win.key.clone();
                self.inbox.push(Event::Dock(key, update.clone()));
                None
            }
            // the session observer's state is taken before (`SessionState::apply`)
            ShellEvent::SessionLock { .. } => None,
        }
    }

    fn session(&self, change: session::Change) -> Option<RoleEvent> {
        self.inbox.push(Event::Session(change));
        None
    }

    /// The server closed or dismissed `surface`: the program hears it, and its window closes.
    fn closed(&mut self, surface: SurfaceId) -> RoleEvent {
        if let Some(key) = self.win_of_surface(surface).map(|w| w.key.clone()) {
            self.inbox.push(Event::Closed(key.clone()));
            self.shown(&key);
        }
        RoleEvent::Closed(surface)
    }

    /// What the consent authority said, as the program's event.
    fn consent_event(&mut self, event: &ControlEvent) {
        match event {
            ControlEvent::ConsentApp { epoch, app } => {
                self.consent_apps.insert(
                    *epoch,
                    consent::App {
                        desktop_entry: app.desktop,
                        identity: app.identity.clone(),
                        name: app.name.clone(),
                        services: app.services,
                        remembered: app.remembered,
                    },
                );
            }
            ControlEvent::ConsentRequested {
                epoch,
                capabilities,
                output,
                resuming,
            } => {
                let run = self.run_of(*epoch);
                self.inbox.push(Event::Consent(consent::Event::Requested(
                    consent::Request {
                        run,
                        capabilities: *capabilities,
                        output: output.clone(),
                        resuming: *resuming,
                        app: self.consent_apps.remove(epoch),
                    },
                )));
            }
            ControlEvent::ConsentChanged { epoch, phase } => {
                let run = self.run_of(*epoch);
                self.inbox.push(Event::Consent(consent::Event::Changed {
                    run,
                    phase: *phase,
                }));
            }
            _ => {}
        }
    }

    /// The program's number for the consent run of `epoch`.
    fn run_of(&mut self, epoch: u64) -> consent::Run {
        if let Some((&run, _)) = self.consent_runs.iter().find(|(_, &e)| e == epoch) {
            return consent::Run::new(run);
        }
        let run = self.next_run;
        self.next_run = self.next_run.wrapping_add(1).max(1);
        self.consent_runs.insert(run, epoch);
        consent::Run::new(run)
    }

    /// What the runtime's own protocol objects said, as the program's event.
    fn wire_event(&mut self, event: &Wire) {
        let event = match event {
            Wire::WindowManager(output, update) => Event::WindowManager(*output, update.clone()),
            Wire::Overview(output, update) => Event::Overview(*output, update.clone()),
            Wire::Transaction(event) => {
                if let transaction::Event::Finished { id, .. } = event {
                    if let Some(object) = self.transactions.remove(id) {
                        object.destroy();
                    }
                }
                Event::Transaction(*event)
            }
            &Wire::HoleClosed(number) => {
                let Some((window, layer)) = self.windows.iter().find_map(|w| {
                    w.holes
                        .iter()
                        .find(|(_, h)| h.number == number)
                        .map(|(layer, _)| (w.key.clone(), *layer))
                }) else {
                    return;
                };
                Event::HoleClosed { window, layer }
            }
            Wire::Windows(windows) => {
                Event::Capture(crate::capture::Event::Windows(windows.clone()))
            }
        };
        self.inbox.push(event);
    }

    fn set_busy(&self, busy: Option<Instant>) {
        if let Some(record) = &self.busy {
            *record.lock().unwrap_or_else(PoisonError::into_inner) = busy;
        }
    }
}

impl SurfaceRoles for ShellRoles {
    fn opening(&mut self, key: Option<&WindowKey>) {
        self.opening = key.cloned();
    }

    fn create(
        &mut self,
        conn: &mut Connection,
        _: &WindowSpec,
    ) -> Option<Result<SurfaceId, WireError>> {
        let key = self.opening.take()?;
        let surface = self.surface_of(&key);
        match &surface.role {
            Role::Window => {
                if surface.persistent.is_some() {
                    tracing::warn!(?key, "only a layer surface is persisted; the window is not");
                }
                self.windows.push(Win::new(key));
                None
            }
            Role::Lock(output) => Some(self.create_lock(conn, key, *output)),
            role => Some(self.create_layer(conn, key, role, surface.persistent)),
        }
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(change) = self.observed.apply(event) {
            if let SessionChange::Lock { locked } = change {
                let _ = context::with_ctx_mut(|ctx| ctx.locked = locked);
                self.inbox
                    .push(Event::Session(session::Change::Locked(locked)));
            }
            // the window system follows the appearance and the display options itself
            return None;
        }
        if let Some(event) = event.downcast_ref::<ShellEvent>() {
            return self.shell_event(event);
        }
        if let Some(event) = event.downcast_ref::<ControlEvent>() {
            self.consent_event(event);
            return None;
        }
        if let Some(event) = event.downcast_ref::<Wire>() {
            self.wire_event(event);
            return None;
        }
        self.captures.event(event, &self.inbox);
        None
    }

    fn before_commit(
        &mut self,
        conn: &mut Connection,
        key: &WindowKey,
        surface: SurfaceId,
        layers: &mut dyn Iterator<Item = (u64, SurfaceId)>,
        new_frame: bool,
    ) {
        let layers: Vec<(u64, SurfaceId)> = layers.collect();
        if self.win_of_key(key).is_none() {
            self.windows.push(Win::new(key.clone()));
        }
        if let Some(win) = self.win_of_key(key) {
            win.surface.get_or_insert(surface);
        }
        self.stage_pending(conn, key, surface);
        if !new_frame {
            return;
        }
        let described = self.surface_of(key);
        if let (Role::Layer(spec) | Role::MenuBar(spec) | Role::Dock(spec), Some(win)) =
            (&described.role, self.win_of_key(key))
        {
            Self::place(win, spec);
        }
        self.follow_holes(conn, key, &described.holes, &layers);
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        if let Some(i) = self.windows.iter().position(|w| w.surface == Some(surface)) {
            let win = self.windows.remove(i);
            for (_, hole) in win.holes {
                hole.proxy.destroy();
            }
        }
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        self.follow_outputs(conn, true);
    }

    fn prepare(&mut self, conn: &mut Connection) {
        if let Some(link) = &self.link {
            link.dispatch_pending();
        }
        for request in context::drain_outgoing() {
            self.execute(conn, request);
        }
        self.captures.pump(conn);
        self.ready_if_shown();
        self.set_busy(None);
    }

    fn woke(&mut self) {
        self.set_busy(Some(Instant::now()));
    }

    fn frame_done(&mut self, surface: SurfaceId) {
        if let Some(key) = self.win_of_surface(surface).map(|w| w.key.clone()) {
            self.shown(&key);
        }
    }
}

/// The program's description of an output.
fn output(info: cupertino_wayland_client::OutputInfo) -> Output {
    use cupertino_wayland_client::OutputTransform as Wire;
    Output {
        id: OutputId::from_raw(info.id.registry_name()),
        name: info.name.clone(),
        description: info.description.clone(),
        geometry: info.geometry.map(|g| {
            Rect::new(
                f64::from(g.x),
                f64::from(g.y),
                f64::from(g.width),
                f64::from(g.height),
            )
        }),
        scale: info
            .effective_scale()
            .unwrap_or_else(|| info.scale.factor()),
        transform: match info.transform {
            Wire::Normal => Transform::Normal,
            Wire::Rotated90 => Transform::Rotated90,
            Wire::Rotated180 => Transform::Rotated180,
            Wire::Rotated270 => Transform::Rotated270,
            Wire::Flipped => Transform::Flipped,
            Wire::Flipped90 => Transform::Flipped90,
            Wire::Flipped180 => Transform::Flipped180,
            Wire::Flipped270 => Transform::Flipped270,
        },
    }
}

/// The transport's id of the output the program names `output`.
pub(super) fn wire_output(output: OutputId) -> cupertino_wayland_client::OutputId {
    cupertino_wayland_client::OutputId::from_registry_name(output.raw())
}

fn transport(e: cupertino_shell_client::Error) -> WireError {
    match e {
        cupertino_shell_client::Error::Transport(e) => e,
        _ => WireError::Protocol("the shell refused the surface's role"),
    }
}

fn layer_config(spec: &LayerSpec) -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        namespace: spec.namespace.to_owned(),
        output: spec.output.map(wire_output),
        layer: match spec.layer {
            Layer::Background => WireLayer::Background,
            Layer::Bottom => WireLayer::Bottom,
            Layer::Top => WireLayer::Top,
            Layer::Overlay => WireLayer::Overlay,
        },
        anchor: wire_anchor(spec.anchor),
        size: whole_size(spec),
        margin: wire_margin(spec),
        exclusive_zone: spec.exclusive_zone,
        keyboard: wire_keyboard(spec.keyboard),
    }
}

fn wire_anchor(anchor: crate::Anchor) -> WireAnchor {
    let mut wire = WireAnchor::empty();
    for (on, edge) in [
        (anchor.top, WireAnchor::Top),
        (anchor.bottom, WireAnchor::Bottom),
        (anchor.left, WireAnchor::Left),
        (anchor.right, WireAnchor::Right),
    ] {
        if on {
            wire |= edge;
        }
    }
    wire
}

/// The size asked for in whole points, rounded up; 0 stretches between anchored edges.
fn whole_size(spec: &LayerSpec) -> (u32, u32) {
    let whole = |extent: f64| extent.max(0.0).ceil() as u32;
    (whole(spec.size.width), whole(spec.size.height))
}

fn wire_margin(spec: &LayerSpec) -> WireMargin {
    WireMargin {
        top: spec.margin.top,
        right: spec.margin.right,
        bottom: spec.margin.bottom,
        left: spec.margin.left,
    }
}

fn wire_keyboard(keyboard: Keyboard) -> KeyboardInteractivity {
    match keyboard {
        Keyboard::None => KeyboardInteractivity::None,
        Keyboard::OnDemand => KeyboardInteractivity::OnDemand,
        Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
    }
}

fn hole_source(
    source: crate::HoleSource,
) -> (
    cupertino_protocols::client::transaction::cupertino_hole_v1::Source,
    u32,
) {
    use cupertino_protocols::client::transaction::cupertino_hole_v1::Source;
    match source {
        crate::HoleSource::Window(id) => (Source::Window, id.0),
        crate::HoleSource::Thumbnail(id) => (Source::Thumbnail, id.0),
        crate::HoleSource::Workspace(id) => (Source::Workspace, id.0),
    }
}
