//! Server side of the dock (`cupertino_shell_v1.get_dock`, `cupertino_dock_v1`): the dock is a
//! layer surface of the dock process, which owns its presentation (tiles, magnification, bounces,
//! auto-hide) and its reserved area (the layer's exclusive zone). The server tells every dock which
//! windows exist, takes its activation and restore requests, and reads the slot a minimizing window
//! flies to from the committed `set_minimize_target`. The surface's geometry is the dock's rectangle.
//! The application switcher's panel is the dock's too: the server keeps the keys and the choice
//! (`input.rs`, `cupertino-wm`) and tells one dock the panel's output (the active output
//! when the switch opened), the list and the selection ([`Cupertino::switcher_changed`]), then
//! every dock that it ended ([`Docks::switcher_closed`]); a dock that binds mid-switch on the
//! panel's output gets the state at once. `switcher_select` is the pointer's pick and
//! `switcher_activate` / `switcher_cancel` its release on the panel; a press anywhere else is the
//! server's ([`SWITCHER_NAMESPACE`]). Refused activations reach every dock as `attention`. The
//! app-grid action opens the launcher's application browser, which the server asks for on the
//! session bus (`Cupertino::toggle_applications`); a hot corner or gesture the dock maps to it
//! comes back as that action. Docks are no longer sent `toggle_app_grid`. The overview is shared: the server owns
//! the session (`shell.rs`: transition, frames, Escape, click and workspace-switch fallbacks) and
//! tells the dock of its output the window set, the workspaces and the layout area
//! ([`Cupertino::send_overview_to_docks`]); the dock lays the windows out (`overview_frame`), draws
//! the workspaces bar and sends picks and workspace edits. Hot corners and gestures the server
//! detects go to the dock to be mapped ([`Docks::hot_corner`], [`Docks::gesture`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Seek, Write};
use std::os::fd::AsFd;
use std::sync::Arc;

use cupertino_overview::OverviewMode;
use cupertino_protocols::server::cupertino_dock_v1::{
    self, Corner, CupertinoDockV1, Gesture, WindowState,
};
use cupertino_protocols::server::cupertino_session_v1::Activation as AppActivation;
use cupertino_protocols::server::cupertino_shell_v1::{CupertinoShellV1, Error as ShellError};
use cupertino_wm::{Command, LifecycleState, OutputId, Rect, WindowId};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::WEnum;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource,
};
use smithay::wayland::compositor::{with_states, Cacheable};

use crate::app_effects_v1::surface_backdrops;
use crate::authorization::{authorized, refuse, require, role, Permit, ShellRole};
use crate::dock_thumbnail::WindowThumbnail;
use crate::menu_bar::cut_text;
use crate::state::Cupertino;
use crate::toplevel_icon::WindowIcon;

/// Where a minimizing window lands: the dock's `set_minimize_target`, in surface-local logical
/// coordinates, double-buffered with the dock surface.
#[derive(Clone, Copy, Debug, Default)]
pub struct DockState {
    minimize_target: Option<Rect>,
}

impl Cacheable for DockState {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// What a dock was told about one window.
#[derive(Clone, Debug)]
struct WindowInfo {
    app_id: String,
    title: String,
    state: WindowState,
    /// The window's copied icon: the same `Arc` until the window commits another.
    icon: Arc<WindowIcon>,
    /// The picture of the window while it is minimized, once drawn.
    thumbnail: Option<Arc<WindowThumbnail>>,
}

impl WindowInfo {
    fn same_window_event(&self, other: &WindowInfo) -> bool {
        (&self.app_id, &self.title, self.state) == (&other.app_id, &other.title, other.state)
    }
}

/// Shell version of `cupertino_dock_v1.window_icon`.
const WINDOW_ICON_SINCE: u32 = 17;

/// Shell version of `cupertino_dock_v1.engage`.
const ENGAGE_SINCE: u32 = 18;

/// Shell version of `cupertino_dock_v1.app_resident`.
const APP_RESIDENT_SINCE: u32 = 19;

/// Shell version of `cupertino_dock_v1.window_thumbnail`.
const WINDOW_THUMBNAIL_SINCE: u32 = 22;

/// Layer-shell namespace of the dock's switcher panel: a press there is the dock's, a press
/// anywhere else cancels the switch.
pub const SWITCHER_NAMESPACE: &str = "cupertino-dock-switcher";

/// The open application switcher: what the panel's dock is shown, and the output it is drawn on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SwitcherState {
    apps: Vec<String>,
    selected: usize,
    output: OutputId,
}

struct Dock {
    resource: CupertinoDockV1,
    surface: WlSurface,
    output: OutputId,
    /// The client's object of the dock's output.
    wl_output: WlOutput,
    /// Windows the dock knows by id; `None` until the first group.
    sent: Option<BTreeMap<u32, WindowInfo>>,
    /// The resident applications the dock was told of.
    residents_sent: BTreeSet<String>,
}

impl Dock {
    /// The `unresponsive` event for `app_id`: the one place the application id is cut for it.
    fn send_unresponsive(&self, app_id: &str, unresponsive: bool) {
        self.resource
            .unresponsive(cut_text(app_id).to_owned(), u32::from(unresponsive));
    }
}

/// The docks of the dock process, at most one per output.
#[derive(Default)]
pub struct Docks {
    docks: Vec<Dock>,
    /// `None` while no switch is under way, also without any dock.
    switcher: Option<SwitcherState>,
    /// The applications each connected application client has shown a window of: they stay resident while it is
    /// connected, windows or not ([`Cupertino::resident_apps`]).
    residents: HashMap<ClientId, BTreeSet<String>>,
    /// The pictures of the minimized windows, by window id.
    thumbnails: HashMap<u32, Arc<WindowThumbnail>>,
    /// Minimized windows that get no picture: protected from capture, or drawing them failed.
    no_thumbnail: BTreeSet<u32>,
}

impl Docks {
    /// A client that showed windows disconnected: its applications are resident no longer. Whether it had any.
    pub fn client_gone(&mut self, client: &ClientId) -> bool {
        self.residents.remove(client).is_some()
    }
}

impl Docks {
    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.docks.iter().any(|d| &d.surface == surface)
    }

    pub fn remove_output(&mut self, output: OutputId) {
        self.docks.retain(|d| d.output != output);
    }

    fn of_output(&self, output: OutputId) -> Option<&Dock> {
        self.docks.iter().find(|d| d.output == output)
    }

    /// Tell every dock the server started a program.
    pub fn launching(&self, app_id: &str) {
        for dock in &self.docks {
            dock.resource.launching(cut_text(app_id).to_owned());
            dock.resource.done();
        }
    }

    /// Tell every dock that `app_id` stopped answering its pings (`unresponsive` true) or answers again.
    pub fn unresponsive(&self, app_id: &str, unresponsive: bool) {
        for dock in self.docks.iter().filter(|d| d.resource.version() >= 4) {
            dock.send_unresponsive(app_id, unresponsive);
            dock.resource.done();
        }
    }

    /// Tell every dock the Force Quit shortcut was pressed: it opens or closes its dialog.
    pub fn force_quit_dialog(&self) {
        for dock in self.docks.iter().filter(|d| d.resource.version() >= 4) {
            dock.resource.force_quit_dialog();
            dock.resource.done();
        }
    }

    /// The output the open switch's panel is drawn on.
    pub fn switcher_output(&self) -> Option<OutputId> {
        self.switcher.as_ref().map(|s| s.output)
    }

    /// The dock that draws the panel on `output`: that output's own, else the first.
    fn panel_dock(&self, output: OutputId) -> Option<&Dock> {
        self.of_output(output).or(self.docks.first())
    }

    /// Tell every dock that `app_id` asked for attention.
    pub fn attention(&self, app_id: &str) {
        for dock in &self.docks {
            dock.resource.attention(cut_text(app_id).to_owned());
            dock.resource.done();
        }
    }

    /// The switch ended: tell every dock, whether the selection was committed or cancelled.
    pub fn switcher_closed(&mut self, committed: bool) {
        self.switcher = None;
        for dock in &self.docks {
            dock.resource.switcher_closed(u32::from(committed));
            dock.resource.done();
        }
    }

    /// The Focus Dock shortcut: the dock of `output`, else the first, takes the keyboard. Docks below version 18 are not
    /// told.
    pub fn engage(&self, output: Option<OutputId>) {
        if let Some(dock) = output
            .and_then(|o| self.of_output(o))
            .or(self.docks.first())
            .filter(|d| d.resource.version() >= ENGAGE_SINCE)
        {
            dock.resource.engage();
            dock.resource.done();
        }
    }

    /// Tell every dock the auto-hide shortcut was pressed.
    pub fn toggle_auto_hide(&self) {
        for dock in &self.docks {
            dock.resource.toggle_auto_hide();
            dock.resource.done();
        }
    }

    /// The overview of `output` is leaving: its dock drops the chrome.
    pub fn overview_closed(&self, output: OutputId) {
        if let Some(dock) = self.of_output(output) {
            dock.resource.overview_closed();
            dock.resource.done();
        }
    }

    /// A corner of `output` fired: its dock maps the corner to an action.
    pub fn hot_corner(&self, output: OutputId, corner: Corner) {
        if let Some(dock) = self.of_output(output) {
            dock.resource.hot_corner(corner);
            dock.resource.done();
        }
    }

    /// A gesture committed on `output`: its dock maps it to an action. False without a dock there.
    pub fn gesture(&self, output: OutputId, kind: Gesture) -> bool {
        let Some(dock) = self.of_output(output) else {
            return false;
        };
        dock.resource.gesture(kind);
        dock.resource.done();
        true
    }
}

/// The open overview as one dock sees it, in its output's coordinates.
struct OverviewGroup {
    mode: cupertino_dock_v1::OverviewMode,
    app_id: String,
    area: Rect,
    windows: Vec<(u32, Rect)>,
    workspaces: Vec<(u32, bool)>,
}

/// One group: the overview's windows, workspaces and area.
fn send_overview(resource: &CupertinoDockV1, g: &OverviewGroup) {
    for (id, r) in &g.windows {
        resource.overview_window(
            *id,
            r.x.round() as i32,
            r.y.round() as i32,
            r.w.round() as i32,
            r.h.round() as i32,
        );
    }
    for (id, current) in &g.workspaces {
        resource.overview_workspace(*id, u32::from(*current));
    }
    let a = g.area;
    resource.overview_changed(
        g.mode,
        cut_text(&g.app_id).to_owned(),
        a.x.round() as i32,
        a.y.round() as i32,
        a.w.round() as i32,
        a.h.round() as i32,
    );
    resource.done();
}

/// `window_icon` for window `id`, its pixels in a sealed memfd; nothing when the file cannot be made.
fn send_window_icon(resource: &CupertinoDockV1, id: u32, icon: &WindowIcon) {
    let (size, argb) = icon
        .image
        .as_ref()
        .map_or((0, &[][..]), |i| (i.size, i.argb.as_slice()));
    match sealed_file(argb) {
        Ok(file) => resource.window_icon(
            id,
            icon.name.clone().unwrap_or_default(),
            size,
            file.as_fd(),
        ),
        Err(e) => tracing::warn!(error = %e, "making the file of a window icon failed"),
    }
}

/// `window_thumbnail` for window `id`, its pixels in a sealed memfd, or an empty one for none; nothing when the file
/// cannot be made.
fn send_window_thumbnail(resource: &CupertinoDockV1, id: u32, thumbnail: Option<&WindowThumbnail>) {
    let (width, height, argb) =
        thumbnail.map_or((0, 0, &[][..]), |t| (t.width, t.height, t.argb.as_slice()));
    match sealed_file(argb) {
        Ok(file) => resource.window_thumbnail(id, width, height, file.as_fd()),
        Err(e) => tracing::warn!(error = %e, "making the file of a window picture failed"),
    }
}

/// A memfd holding `bytes`, sealed against writes and resizing.
fn sealed_file(bytes: &[u8]) -> std::io::Result<std::fs::File> {
    use rustix::fs::{fcntl_add_seals, memfd_create, MemfdFlags, SealFlags};
    let fd = memfd_create(
        "cupertino-window-icon",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes)?;
    file.rewind()?;
    fcntl_add_seals(
        &file,
        SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE | SealFlags::SEAL,
    )?;
    Ok(file)
}

/// One group: the panel's output, the switcher's list and its selection.
fn send_switcher(resource: &CupertinoDockV1, output: &WlOutput, state: &SwitcherState) {
    resource.switcher_output(output);
    for app in &state.apps {
        resource.switcher_entry(cut_text(app).to_owned());
    }
    resource.switcher_changed(state.selected as u32);
    resource.done();
}

/// Per-object data of a dock. `output` is `None` for an object the server rejected with a protocol
/// error: it stays inert until the client is disconnected.
pub struct DockData {
    output: Option<OutputId>,
}

impl Cupertino {
    /// Drop the docks whose connection no longer holds the Dock role.
    pub(crate) fn release_revoked_docks(&mut self) {
        let revoked: Vec<_> = self
            .docks
            .docks
            .iter()
            .filter(|d| {
                d.resource
                    .client()
                    .is_some_and(|c| !authorized(&c, ShellRole::Dock))
            })
            .map(|d| (d.resource.clone(), d.output))
            .collect();
        for (resource, output) in revoked {
            self.docks.docks.retain(|d| d.resource != resource);
            self.dock_gone(output);
        }
    }

    /// `cupertino_shell_v1.get_dock`: validate the role, register the dock and send its first group.
    pub fn create_dock(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::Dock>>,
        id: New<CupertinoDockV1>,
        surface: WlSurface,
        output: &WlOutput,
        init: &mut DataInit<'_, Self>,
    ) {
        if permit.is_none() {
            init.init(id, DockData { output: None });
            refuse(shell, ShellRole::Dock);
            return;
        }
        let target = Output::from_resource(output)
            .and_then(|o| self.output_by_wl(&o).map(|e| (e.id, e.output.clone())));
        let rejection = match &target {
            None => Some("the output is gone".to_string()),
            Some((_, o))
                if layer_map_for_output(o)
                    .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                    .is_none() =>
            {
                Some("the surface is not a layer surface of the output".to_string())
            }
            Some(_) if self.docks.has_surface(&surface) || self.menu_bars.has_surface(&surface) => {
                Some("the surface already has a shell role".to_string())
            }
            Some((out, _)) if self.docks.of_output(*out).is_some() => {
                Some("the output already has a dock".to_string())
            }
            Some(_) => None,
        };
        if let Some(reason) = rejection {
            init.init(id, DockData { output: None });
            shell.post_error(ShellError::Role, reason);
            return;
        }
        let (out, _) = target.expect("a rejected target returned above");
        let resource = init.init(id, DockData { output: Some(out) });
        self.docks.docks.push(Dock {
            resource,
            surface,
            output: out,
            wl_output: output.clone(),
            sent: None,
            residents_sent: BTreeSet::new(),
        });
        self.refresh_usable_area(out);
        self.refresh_docks();
        // a dock that binds while an application does not answer learns it at once
        if let Some(dock) = self
            .docks
            .docks
            .last()
            .filter(|d| d.resource.version() >= 4)
        {
            let unresponsive = self.unresponsive_apps();
            for app_id in &unresponsive {
                dock.send_unresponsive(app_id, true);
            }
            if !unresponsive.is_empty() {
                dock.resource.done();
            }
        }
        // a dock that restarts mid-switch on the panel's output draws the panel from where the switch stands
        if let Some(state) = self.docks.switcher.clone().filter(|s| s.output == out) {
            self.send_switcher_to_panel_dock(&state);
        }
        // and one that restarts mid-overview lays it out from where it stands
        if let Some(group) = self
            .overview_group()
            .filter(|_| self.shell.overview_on(out).is_some())
        {
            if let Some(dock) = self.docks.docks.last() {
                send_overview(&dock.resource, &group);
            }
        }
    }

    /// The switcher opened or its selection moved: the panel's dock gets the whole list again. The
    /// panel stays on the active output of when the switch opened (`crate::displays::placement`).
    pub fn switcher_changed(&mut self, apps: Vec<String>, selected: usize) {
        let output = match self.docks.switcher_output() {
            Some(output) => output,
            None => match self.active_output() {
                Some(o) => o,
                // no output, no panel: the switch runs on the keys alone
                None => return,
            },
        };
        let state = SwitcherState {
            apps,
            selected,
            output,
        };
        self.send_switcher_to_panel_dock(&state);
        self.docks.switcher = Some(state);
    }

    /// Send the switcher group to the dock that draws the panel, naming the panel's output by the
    /// dock client's own object for it (its dock's output when the client has none).
    fn send_switcher_to_panel_dock(&self, state: &SwitcherState) {
        let Some(dock) = self.docks.panel_dock(state.output) else {
            return;
        };
        let bound = (dock.output != state.output)
            .then(|| {
                let client = dock.resource.client()?;
                self.output_entry(state.output)?
                    .output
                    .client_outputs(&client)
                    .next()
            })
            .flatten();
        send_switcher(
            &dock.resource,
            bound.as_ref().unwrap_or(&dock.wl_output),
            state,
        );
    }

    /// Whether `surface` belongs to the dock's switcher panel (a layer surface of
    /// [`SWITCHER_NAMESPACE`], its subsurfaces included).
    pub fn is_switcher_panel(&self, surface: &WlSurface) -> bool {
        self.outputs.iter().any(|o| {
            layer_map_for_output(&o.output)
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .is_some_and(|l| l.namespace() == SWITCHER_NAMESPACE)
        })
    }

    /// The layer surface of a dock went away: the role ends with it.
    pub fn dock_surface_gone(&mut self, surface: &WlSurface) {
        let outputs: Vec<OutputId> = self
            .docks
            .docks
            .iter()
            .filter(|d| &d.surface == surface)
            .map(|d| d.output)
            .collect();
        self.docks.docks.retain(|d| &d.surface != surface);
        for output in outputs {
            self.dock_gone(output);
        }
    }

    /// The dock of `output` is gone: its reservation goes, and an overview it was laying out
    /// closes so the windows come back.
    fn dock_gone(&mut self, output: OutputId) {
        self.refresh_usable_area(output);
        if self.shell.overview_on(output).is_some() {
            self.close_overview();
        }
    }

    /// The open overview for the dock of its output, in the output's coordinates.
    fn overview_group(&self) -> Option<OverviewGroup> {
        let s = self.shell.overview.as_ref().filter(|s| !s.closing)?;
        let origin = self.output_entry(s.output)?.rect;
        let local = |r: Rect| r.translate(-origin.x, -origin.y);
        let (mode, app_id) = match s.mode {
            OverviewMode::Overview => (cupertino_dock_v1::OverviewMode::All, String::new()),
            OverviewMode::AppOverview(app) => (
                cupertino_dock_v1::OverviewMode::App,
                self.wm
                    .app(app)
                    .map(|a| a.identity.clone())
                    .unwrap_or_default(),
            ),
        };
        let current = self.wm.current_workspace(s.output);
        Some(OverviewGroup {
            mode,
            app_id,
            area: local(s.area),
            windows: s
                .windows
                .iter()
                .map(|w| (w.id.0 as u32, local(w.frame)))
                .collect(),
            workspaces: self
                .wm
                .workspaces(s.output)
                .iter()
                .map(|w| (w.id.0 as u32, Some(w.id) == current))
                .collect(),
        })
    }

    /// The Focus Dock shortcut: the dock of the active output takes the keyboard on its first tile, asking for
    /// exclusive keyboard interactivity. Nothing is sent while the session is locked or a lock is pending.
    pub fn focus_dock(&mut self) {
        if self.session_lock.is_active() {
            return;
        }
        // a dock a press released takes the keyboard again when engaged again
        self.released_layer = None;
        let output = self.active_output();
        self.docks.engage(output);
    }

    /// Send the open overview (its windows, workspaces and area) to the dock of its output. Call
    /// when it opens and whenever its window set or the workspaces change.
    pub fn send_overview_to_docks(&self) {
        let Some(group) = self.overview_group() else {
            return;
        };
        let output = self
            .shell
            .overview
            .as_ref()
            .map(|s| s.output)
            .expect("a group exists only for an open overview");
        if let Some(dock) = self.docks.of_output(output) {
            tracing::info!(
                output = output.0,
                windows = group.windows.len(),
                workspaces = group.workspaces.len(),
                "overview sent"
            );
            send_overview(&dock.resource, &group);
        }
    }

    /// Windows a dock lists, by id: every mapped main window of an application. A window with a parent (a dialog, a
    /// portal's chooser over another application) belongs to its parent's application and is not listed, and neither
    /// is a window of the server's own connections or of a shell process.
    fn dock_feed(&self) -> BTreeMap<u32, WindowInfo> {
        let key = self.wm.key_window();
        self.wm
            .windows()
            .filter(|w| {
                !matches!(
                    w.state(),
                    LifecycleState::Created | LifecycleState::Closing | LifecycleState::Destroyed
                )
            })
            .filter(|w| w.parent.is_none() && !self.is_shell_window(w.id))
            .map(|w| {
                let mut state = WindowState::empty();
                state.set(
                    WindowState::Minimized,
                    w.state() == LifecycleState::Minimized,
                );
                state.set(WindowState::Active, key == Some(w.id));
                let app_id = self
                    .wm
                    .app(w.app)
                    .map(|a| a.identity.clone())
                    .unwrap_or_default();
                (
                    w.id.0 as u32,
                    WindowInfo {
                        app_id: cut_text(&app_id).to_owned(),
                        title: cut_text(&w.title).to_owned(),
                        state,
                        icon: self.entry(w.id).map(|e| e.icon()).unwrap_or_default(),
                        thumbnail: self.docks.thumbnails.get(&(w.id.0 as u32)).cloned(),
                    },
                )
            })
            .collect()
    }

    /// Whether window `id` belongs to one of the server's own connections or to a shell process.
    fn is_shell_window(&self, id: WindowId) -> bool {
        self.entry(id)
            .and_then(|e| e.surface().client())
            .and_then(|c| {
                c.get_data::<crate::state::ClientState>()
                    .map(|d| d.internal || !d.capabilities.is_empty())
            })
            .unwrap_or(false)
    }

    /// Note the application of every mapped window of an application client as resident for that client, and have
    /// the client's disconnection reported ([`Cupertino::forget_gone_processes`]). The server's own connections and
    /// the shell processes are not applications, and neither is the X11 satellite: it shows the windows of every X11
    /// application on its one connection, which outlives each of them, so an X11 application is in the dock while it
    /// has a window.
    fn note_residents(&mut self) {
        let satellite = self
            .xwayland
            .as_ref()
            .and_then(crate::xwayland::XWayland::pid);
        for entry in self.windows.iter().filter(|e| e.mapped) {
            let Some(client) = entry.surface().client() else {
                continue;
            };
            let Some(data) = client.get_data::<crate::state::ClientState>() else {
                continue;
            };
            if data.internal || !data.capabilities.is_empty() {
                continue;
            }
            if satellite.is_some()
                && (self.process_of)(&self.display_handle, &client).map(|(pid, _)| pid) == satellite
            {
                continue;
            }
            let Some(identity) = self
                .wm
                .window(entry.id)
                .and_then(|w| self.wm.app(w.app))
                .map(|a| a.identity.clone())
                .filter(|i| !i.is_empty())
            else {
                continue;
            };
            data.gone.get_or_init(|| self.gone_clients.clone());
            self.docks
                .residents
                .entry(client.id())
                .or_default()
                .insert(identity);
        }
    }

    /// The applications a connected client has shown a window of, with or without windows now.
    pub fn resident_apps(&self) -> BTreeSet<String> {
        self.docks
            .residents
            .values()
            .flatten()
            .map(|a| cut_text(a).to_owned())
            .collect()
    }

    /// Send every dock what changed in the window list and in the resident applications since its last group. Call
    /// after anything they derive from may have changed: mapping, closing, titles, minimizing, focus, a client
    /// disconnecting.
    pub fn refresh_docks(&mut self) {
        self.note_residents();
        if self.docks.docks.is_empty() {
            return;
        }
        let feed = self.dock_feed();
        let residents = self.resident_apps();
        for dock in &mut self.docks.docks {
            let mut changed = dock.sent.is_none();
            let known = dock.sent.take().unwrap_or_default();
            let icons = dock.resource.version() >= WINDOW_ICON_SINCE;
            let thumbnails = dock.resource.version() >= WINDOW_THUMBNAIL_SINCE;
            for (id, info) in &feed {
                let old = known.get(id);
                if !old.is_some_and(|o| o.same_window_event(info)) {
                    dock.resource
                        .window(*id, info.app_id.clone(), info.title.clone(), info.state);
                    changed = true;
                }
                let new_icon = old.map_or(!info.icon.is_empty(), |o| {
                    !Arc::ptr_eq(&o.icon, &info.icon) && o.icon != info.icon
                });
                if icons && new_icon {
                    send_window_icon(&dock.resource, *id, &info.icon);
                    changed = true;
                }
                let new_thumbnail = match (old.and_then(|o| o.thumbnail.as_ref()), &info.thumbnail)
                {
                    (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                    (None, None) => false,
                    (Some(_), None) | (None, Some(_)) => true,
                };
                if thumbnails && new_thumbnail {
                    send_window_thumbnail(&dock.resource, *id, info.thumbnail.as_deref());
                    changed = true;
                }
            }
            for id in known.keys().filter(|id| !feed.contains_key(id)) {
                dock.resource.window_closed(*id);
                changed = true;
            }
            if dock.resource.version() >= APP_RESIDENT_SINCE {
                for app in residents.difference(&dock.residents_sent) {
                    dock.resource.app_resident(app.clone(), 1);
                    changed = true;
                }
                for app in dock.residents_sent.difference(&residents) {
                    dock.resource.app_resident(app.clone(), 0);
                    changed = true;
                }
                dock.residents_sent = residents.clone();
            }
            if changed {
                dock.resource.done();
            }
            dock.sent = Some(feed.clone());
        }
    }

    /// Draw the picture of each minimized window of output `idx` that has none yet, while a dock takes pictures, and
    /// forget the pictures of the windows that are no longer minimized; tell the docks when that changed anything.
    pub fn capture_dock_thumbnails(
        &mut self,
        renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
        idx: usize,
    ) {
        let minimized: BTreeSet<u32> = self
            .wm
            .windows()
            .filter(|w| w.state() == LifecycleState::Minimized)
            .map(|w| w.id.0 as u32)
            .collect();
        let before = self.docks.thumbnails.len();
        self.docks.thumbnails.retain(|id, _| minimized.contains(id));
        self.docks.no_thumbnail.retain(|id| minimized.contains(id));
        let mut changed = self.docks.thumbnails.len() != before;
        let wanted = self
            .docks
            .docks
            .iter()
            .any(|d| d.resource.version() >= WINDOW_THUMBNAIL_SINCE);
        if let (true, Some(output)) = (wanted, self.outputs.get(idx).map(|o| o.id)) {
            let pending: Vec<WindowId> = self
                .wm
                .windows()
                .filter(|w| w.state() == LifecycleState::Minimized && w.output == output)
                .filter(|w| w.parent.is_none())
                .map(|w| w.id)
                .filter(|id| {
                    let key = id.0 as u32;
                    !self.docks.thumbnails.contains_key(&key)
                        && !self.docks.no_thumbnail.contains(&key)
                })
                .collect();
            for id in pending {
                let key = id.0 as u32;
                let protected = self
                    .entry(id)
                    .is_none_or(|e| crate::control::protected_surface(e.surface()));
                if protected || self.is_shell_window(id) {
                    self.docks.no_thumbnail.insert(key);
                    continue;
                }
                match self
                    .render_window_rgba(renderer, idx, id)
                    .ok()
                    .and_then(|(w, h, rgba)| crate::dock_thumbnail::from_rgba(w, h, &rgba))
                {
                    Some(t) => {
                        self.docks.thumbnails.insert(key, Arc::new(t));
                        changed = true;
                    }
                    None => {
                        self.docks.no_thumbnail.insert(key);
                    }
                }
            }
        }
        if changed {
            self.refresh_docks();
        }
    }

    /// A dock surface's rectangle in global logical coordinates, once the layer map placed it.
    fn dock_geometry(&self, dock: &Dock) -> Option<Rect> {
        let out = self.output_entry(dock.output)?;
        let map = layer_map_for_output(&out.output);
        let geo =
            map.layer_geometry(map.layer_for_surface(&dock.surface, WindowSurfaceType::TOPLEVEL)?)?;
        Some(Rect::new(
            out.rect.x + f64::from(geo.loc.x),
            out.rect.y + f64::from(geo.loc.y),
            f64::from(geo.size.w),
            f64::from(geo.size.h),
        ))
    }

    /// The dock's rectangle on `output` in global logical coordinates: the area of its committed
    /// backdrops (the bar), not the whole layer surface, which also covers magnification room.
    /// `None` without a dock or before it committed a backdrop.
    pub fn dock_rect(&self, output: OutputId) -> Option<Rect> {
        let dock = self.docks.of_output(output)?;
        let geo = self.dock_geometry(dock)?;
        surface_backdrops(&dock.surface, (geo.x, geo.y).into())
            .iter()
            .map(|b| Rect::new(b.area.loc.x, b.area.loc.y, b.area.size.w, b.area.size.h))
            .reduce(|a, b| a.union(&b))
    }

    /// Slot the dock of the window's output reserved for a window minimizing now, in global
    /// logical coordinates; `None` without a dock or before it set a target.
    pub fn minimize_target(&self, id: WindowId) -> Option<Rect> {
        let output = self.wm.window(id)?.output;
        let dock = self.docks.of_output(output)?;
        let local = with_states(&dock.surface, |s| {
            s.cached_state.get::<DockState>().current().minimize_target
        })?;
        let geo = self.dock_geometry(dock)?;
        Some(Rect::new(
            geo.x + local.x,
            geo.y + local.y,
            local.w,
            local.h,
        ))
    }

    /// `restore_window`: the window flies out of its tile `tile` (surface-local) back to its frame.
    fn dock_restore_window(&mut self, output: OutputId, id: u32, tile: Rect) {
        let window = WindowId(u64::from(id));
        if !self
            .wm
            .window(window)
            .is_some_and(|w| w.state().is_minimized())
        {
            return;
        }
        if let Some(geo) = self
            .docks
            .of_output(output)
            .and_then(|d| self.dock_geometry(d))
        {
            self.command(Command::SetMinimizeTarget {
                window,
                target: Rect::new(geo.x + tile.x, geo.y + tile.y, tile.w, tile.h),
            });
        }
        self.command(Command::Restore { window });
    }

    /// `activate_app`: raise the application's windows and focus the frontmost.
    fn dock_activate_app(&mut self, app_id: &str) {
        if let Some(app) = self.wm.app_for_identity(app_id) {
            self.command(Command::ActivateApp { app });
        }
    }

    /// Whether the application `identity` is the one of desktop file ID `id` (without `.desktop`): it is the ID, or its
    /// desktop entry has it, without regard to case.
    fn is_desktop_app(&self, identity: &str, id: &str) -> bool {
        identity.eq_ignore_ascii_case(id)
            || self
                .desktop
                .find(identity)
                .is_some_and(|e| e.id.eq_ignore_ascii_case(id))
    }

    /// `cupertino_session_v1.activate_app` (the launcher opening an application): bring the application of desktop
    /// file ID `desktop_id` forward the way the dock's `activate_app` does (the same [`Command::ActivateApp`], so the
    /// same choice of window, restore of a minimized one and unhiding), when it shows a window the dock would list.
    /// An application matches when its identity is the ID, or the desktop entry of its identity has that ID, without
    /// regard to case; of several, the most recently active. Without a window, whether it is resident
    /// ([`Self::resident_apps`]) or not running; nothing is started.
    pub fn activate_desktop_app(&mut self, desktop_id: &str) -> AppActivation {
        let id = desktop_id.strip_suffix(".desktop").unwrap_or(desktop_id);
        if id.is_empty() {
            return AppActivation::NotRunning;
        }
        let shown = |app| {
            self.wm.windows().any(|w| {
                w.app == app
                    && !matches!(
                        w.state(),
                        LifecycleState::Created
                            | LifecycleState::Closing
                            | LifecycleState::Destroyed
                    )
            })
        };
        let app = self.wm.app_mru().iter().copied().find(|&app| {
            shown(app)
                && self
                    .wm
                    .app(app)
                    .is_some_and(|a| self.is_desktop_app(&a.identity, id))
        });
        if let Some(app) = app {
            self.command(Command::ActivateApp { app });
            return AppActivation::Activated;
        }
        self.note_residents();
        if self
            .resident_apps()
            .iter()
            .any(|identity| self.is_desktop_app(identity, id))
        {
            AppActivation::Resident
        } else {
            AppActivation::NotRunning
        }
    }
}

impl Dispatch<CupertinoDockV1, DockData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        dock: &CupertinoDockV1,
        request: cupertino_dock_v1::Request,
        data: &DockData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(output) = data.output else { return };
        if !require(client, ShellRole::Dock, dock) {
            return;
        }
        match request {
            cupertino_dock_v1::Request::ActivateApp { app_id } => state.dock_activate_app(&app_id),
            cupertino_dock_v1::Request::ForceQuitApp { app_id } => state.force_quit_app(&app_id),
            cupertino_dock_v1::Request::RestoreWindow {
                id,
                x,
                y,
                width,
                height,
            } => {
                state.dock_restore_window(
                    output,
                    id,
                    Rect::new(
                        f64::from(x),
                        f64::from(y),
                        f64::from(width),
                        f64::from(height),
                    ),
                );
            }
            cupertino_dock_v1::Request::SetMinimizeTarget {
                x,
                y,
                width,
                height,
            } => {
                let Some(dock) = state.docks.of_output(output) else {
                    return;
                };
                // an empty slot is no target: the effect cannot fly into nothing
                let target = (width > 0 && height > 0).then(|| {
                    Rect::new(
                        f64::from(x),
                        f64::from(y),
                        f64::from(width),
                        f64::from(height),
                    )
                });
                with_states(&dock.surface, |s| {
                    s.cached_state.get::<DockState>().pending().minimize_target = target
                });
            }
            cupertino_dock_v1::Request::SwitcherSelect { index } => {
                state.command(Command::SwitcherSelect {
                    index: index as usize,
                })
            }
            cupertino_dock_v1::Request::SwitcherActivate { index } => {
                if state
                    .wm
                    .switcher()
                    .is_some_and(|s| (index as usize) < s.items.len())
                {
                    state.command(Command::SwitcherSelect {
                        index: index as usize,
                    });
                    state.command(Command::SwitcherCommit);
                }
            }
            cupertino_dock_v1::Request::SwitcherCancel => state.command(Command::SwitcherCancel),
            cupertino_dock_v1::Request::OverviewFrame {
                id,
                x,
                y,
                width,
                height,
            } => {
                let Some(origin) = state.output_entry(output).map(|o| o.rect) else {
                    return;
                };
                if width <= 0 || height <= 0 {
                    return;
                }
                state.dock_overview_frame(
                    output,
                    id,
                    Rect::new(
                        origin.x + f64::from(x),
                        origin.y + f64::from(y),
                        f64::from(width),
                        f64::from(height),
                    ),
                );
            }
            cupertino_dock_v1::Request::OverviewSelect { id } => {
                state.overview_select(WindowId(u64::from(id)));
            }
            cupertino_dock_v1::Request::OverviewClose => {
                if state.shell.overview_on(output).is_some() {
                    state.close_overview();
                }
            }
            cupertino_dock_v1::Request::SwitchWorkspace { id } => {
                state.dock_switch_workspace(output, id)
            }
            cupertino_dock_v1::Request::AddWorkspace => {
                state.command(Command::AddWorkspace { output })
            }
            cupertino_dock_v1::Request::RemoveWorkspace { id } => {
                state.command(Command::RemoveWorkspace {
                    workspace: cupertino_wm::WorkspaceId(u64::from(id)),
                })
            }
            cupertino_dock_v1::Request::MoveWorkspace { id, index } => {
                state.command(Command::MoveWorkspace {
                    workspace: cupertino_wm::WorkspaceId(u64::from(id)),
                    index: index as usize,
                })
            }
            cupertino_dock_v1::Request::RunAction { name } => state.dock_run_action(&name),
            cupertino_dock_v1::Request::AppCommand {
                app_id,
                command: WEnum::Value(command),
            } => state.dock_app_command(output, &app_id, command),
            cupertino_dock_v1::Request::AppCommand {
                command: WEnum::Unknown(v),
                ..
            } => tracing::warn!(v, "dock sent an unknown app command"),
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The role ends with the object; the dock's reservation goes with it.
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoDockV1, data: &DockData) {
        let Some(output) = data.output else { return };
        state.docks.docks.retain(|d| &d.resource != resource);
        state.dock_gone(output);
    }
}
