//! Server side of `cupertino_overview_manager_v1` and `cupertino_overview_v1`: feeds an output's layout area, its
//! windows, its workspaces and the person's requests to open or leave the overview (`toggle`) to the overview
//! component, which draws the overview with holes and transactions. The server draws no overview of its own.

use std::collections::HashMap;

use cupertino_protocols::server::overview::cupertino_overview_manager_v1::{
    self, CupertinoOverviewManagerV1, Error as ManagerError,
};
use cupertino_protocols::server::overview::cupertino_overview_v1::{
    self, CupertinoOverviewV1, Mode as OverviewMode, WindowState, WorkspaceState,
};
use cupertino_wm::{Command, LifecycleState, OutputId, WindowId, WorkspaceId, WorkspaceKind};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::authorization::{authorized, ShellRole};
use crate::state::Cupertino;
use crate::transaction::QueuedWindowChange;

/// Protocol version offered for `cupertino_overview_manager_v1`.
pub const OVERVIEW_PROTOCOL_VERSION: u32 = 1;

/// Maximum UTF-8 byte length of `app_id` and `title` sent over `cupertino_overview_v1`.
pub const MAX_TEXT_BYTES: usize = 1024;

/// Cut `text` to at most [`MAX_TEXT_BYTES`] on a UTF-8 character boundary.
fn clip_text(text: &str) -> &str {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SentWindow {
    app_id: String,
    title: String,
    rect: (i32, i32, i32, i32),
    state: WindowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SentWorkspace {
    id: u32,
    state: WorkspaceState,
}

struct OverviewObserver {
    output: OutputId,
    resource: CupertinoOverviewV1,
    last_area: Option<(i32, i32, i32, i32)>,
    last_windows: HashMap<WindowId, SentWindow>,
    last_workspaces: Vec<SentWorkspace>,
}

/// User data on a `cupertino_overview_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct OverviewData {
    pub output: OutputId,
}

/// Server-side registry of `cupertino_overview_v1` observers per output.
pub struct Overviews {
    _global: GlobalId,
    observers: Vec<OverviewObserver>,
}

impl Overviews {
    pub fn new(dh: &DisplayHandle) -> Self {
        let global = dh.create_global::<Cupertino, CupertinoOverviewManagerV1, ()>(
            OVERVIEW_PROTOCOL_VERSION,
            (),
        );
        Self {
            _global: global,
            observers: Vec::new(),
        }
    }

    /// Whether an overview component holds `cupertino_overview_v1` for `output`.
    pub fn has_observer(&self, output: OutputId) -> bool {
        self.observers.iter().any(|o| o.output == output)
    }

    /// Remove any overview observer owned by a disconnected client.
    pub fn client_disconnected(&mut self, client_id: &ClientId) {
        self.observers
            .retain(|o| o.resource.client().is_some_and(|c| c.id() != *client_id));
    }

    /// Remove any overview observer bound to `output`.
    pub fn remove_output(&mut self, output: OutputId) {
        self.observers.retain(|o| o.output != output);
    }
}

impl Cupertino {
    /// Compute the overview layout area for `output`: from the output's top edge down to the bottom
    /// of its usable area, across the usable width (clear of the dock, over the menu bar).
    fn overview_v1_area(&self, output: OutputId) -> Option<(i32, i32, i32, i32)> {
        let out = self.output_entry(output)?;
        let usable = self.usable_area(output);
        let top = out.rect.y;
        let bottom = usable.bottom();
        Some((
            usable.x.round() as i32,
            top.round() as i32,
            usable.w.round().max(0.0) as i32,
            (bottom - top).round().max(0.0) as i32,
        ))
    }

    /// Snapshot the overview window set on `output`: top-level windows without a parent on the
    /// output's current workspace or sticky, minimized included, of non-hidden applications.
    fn overview_v1_windows(&self, output: OutputId) -> HashMap<WindowId, SentWindow> {
        let current_ws = self.wm.current_workspace(output);
        let key = self.wm.key_window();
        let mut map = HashMap::new();
        for entry in &self.windows {
            if !entry.mapped {
                continue;
            }
            let Some(w) = self.wm.window(entry.id) else {
                continue;
            };
            if w.output != output || w.parent.is_some() {
                continue;
            }
            if !w.sticky && Some(w.workspace) != current_ws {
                continue;
            }
            if self.wm.app(w.app).is_some_and(|a| a.hidden) {
                continue;
            }
            let mut state = WindowState::empty();
            if w.state() == LifecycleState::Minimized {
                state |= WindowState::Minimized;
            }
            if key == Some(entry.id) {
                state |= WindowState::Active;
            }
            let app_id = self.app_identity_of(w.app).unwrap_or_default();
            let geom = w.geometry();
            let rect = (
                geom.x.round() as i32,
                geom.y.round() as i32,
                geom.w.round().max(0.0) as i32,
                geom.h.round().max(0.0) as i32,
            );
            map.insert(
                entry.id,
                SentWindow {
                    app_id: clip_text(&app_id).to_owned(),
                    title: clip_text(&w.title).to_owned(),
                    rect,
                    state,
                },
            );
        }
        map
    }

    fn overview_v1_workspaces(&self, output: OutputId) -> Vec<SentWorkspace> {
        let current = self.wm.current_workspace(output);
        self.wm
            .workspaces(output)
            .iter()
            .map(|ws| {
                let mut state = WorkspaceState::empty();
                if Some(ws.id) == current {
                    state |= WorkspaceState::Current;
                }
                if matches!(ws.kind, WorkspaceKind::FullScreen { .. }) {
                    state |= WorkspaceState::FullScreen;
                }
                SentWorkspace {
                    id: ws.id.0 as u32,
                    state,
                }
            })
            .collect()
    }

    /// Push incremental area, window, and workspace updates to all `cupertino_overview_v1` observers.
    pub fn refresh_overview_v1(&mut self) {
        let outputs: Vec<OutputId> = self.overviews.observers.iter().map(|o| o.output).collect();
        for (idx, output) in outputs.into_iter().enumerate() {
            let area = self.overview_v1_area(output);
            let windows = self.overview_v1_windows(output);
            let workspaces = self.overview_v1_workspaces(output);
            let Some(obs) = self.overviews.observers.get_mut(idx) else {
                continue;
            };
            let mut sent_any = false;
            if let Some(a) = area {
                if obs.last_area != Some(a) {
                    obs.resource.area(a.0, a.1, a.2, a.3);
                    obs.last_area = Some(a);
                    sent_any = true;
                }
            }
            let closed: Vec<WindowId> = obs
                .last_windows
                .keys()
                .copied()
                .filter(|id| !windows.contains_key(id))
                .collect();
            for id in closed {
                obs.resource.window_closed(id.0 as u32);
                obs.last_windows.remove(&id);
                sent_any = true;
            }
            for (id, w) in windows {
                if obs.last_windows.get(&id) != Some(&w) {
                    obs.resource.window(
                        id.0 as u32,
                        w.app_id.clone(),
                        w.title.clone(),
                        w.rect.0,
                        w.rect.1,
                        w.rect.2,
                        w.rect.3,
                        w.state,
                    );
                    obs.last_windows.insert(id, w);
                    sent_any = true;
                }
            }
            if obs.last_workspaces != workspaces {
                for ws in &workspaces {
                    obs.resource.workspace(ws.id, ws.state);
                }
                obs.last_workspaces = workspaces;
                sent_any = true;
            }
            if sent_any {
                obs.resource.done();
            }
        }
    }

    /// Send `toggle(mode, app_id)` + `done()` to the `cupertino_overview_v1` observer of `output` when one exists
    /// and the session is not locked. Returns `true` if delivered to a component observer.
    pub fn toggle_overview_v1(&self, output: OutputId, mode: OverviewMode, app_id: &str) -> bool {
        if self.session_lock.is_active() {
            return false;
        }
        let Some(obs) = self.overviews.observers.iter().find(|o| o.output == output) else {
            return false;
        };
        obs.resource.toggle(mode, clip_text(app_id).to_owned());
        obs.resource.done();
        true
    }
}

impl GlobalDispatch<CupertinoOverviewManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoOverviewManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::Overview)
    }
}

impl Dispatch<CupertinoOverviewManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoOverviewManagerV1,
        request: cupertino_overview_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_overview_manager_v1::Request::Destroy => {}
            cupertino_overview_manager_v1::Request::GetOverview { id, output } => {
                if !authorized(client, ShellRole::Overview) {
                    resource.post_error(
                        ManagerError::Permission,
                        "get_overview requires the overview role",
                    );
                    return;
                }
                let Some(out_id) = Output::from_resource(&output)
                    .and_then(|o| state.output_by_wl(&o).map(|e| e.id))
                else {
                    resource.post_error(ManagerError::Role, "output is gone");
                    return;
                };
                if state.overviews.has_observer(out_id) {
                    resource.post_error(ManagerError::Role, "output already has an overview");
                    return;
                }
                let obj = init.init(id, OverviewData { output: out_id });
                let area = state.overview_v1_area(out_id);
                let windows = state.overview_v1_windows(out_id);
                let workspaces = state.overview_v1_workspaces(out_id);
                if let Some(a) = area {
                    obj.area(a.0, a.1, a.2, a.3);
                }
                for (wid, w) in &windows {
                    obj.window(
                        wid.0 as u32,
                        w.app_id.clone(),
                        w.title.clone(),
                        w.rect.0,
                        w.rect.1,
                        w.rect.2,
                        w.rect.3,
                        w.state,
                    );
                }
                for ws in &workspaces {
                    obj.workspace(ws.id, ws.state);
                }
                obj.done();
                state.overviews.observers.push(OverviewObserver {
                    output: out_id,
                    resource: obj,
                    last_area: area,
                    last_windows: windows,
                    last_workspaces: workspaces,
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoOverviewV1, OverviewData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoOverviewV1,
        request: cupertino_overview_v1::Request,
        data: &OverviewData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if state.session_lock.is_active() {
            return;
        }
        let output = data.output;
        match request {
            cupertino_overview_v1::Request::Destroy => {
                state
                    .overviews
                    .observers
                    .retain(|o| &o.resource != resource);
            }
            cupertino_overview_v1::Request::ActivateWindow { id } => {
                let window = WindowId(u64::from(id));
                if !state
                    .transactions
                    .enqueue_window_change(&client.id(), QueuedWindowChange::ActivateWindow(window))
                {
                    if let Some(w) = state.wm.window(window) {
                        if w.state() == LifecycleState::Minimized {
                            state.command(Command::Restore { window });
                        }
                        state.focus_window_by_input(window);
                    }
                }
            }
            cupertino_overview_v1::Request::SwitchWorkspace { id } => {
                let workspace = WorkspaceId(u64::from(id));
                if !state.transactions.enqueue_window_change(
                    &client.id(),
                    QueuedWindowChange::SwitchWorkspace { output, workspace },
                ) {
                    state.command(Command::SwitchWorkspace { output, workspace });
                }
            }
            cupertino_overview_v1::Request::AddWorkspace => {
                state.command(Command::AddWorkspace { output });
            }
            cupertino_overview_v1::Request::RemoveWorkspace { id } => {
                state.command(Command::RemoveWorkspace {
                    workspace: WorkspaceId(u64::from(id)),
                });
            }
            cupertino_overview_v1::Request::MoveWorkspace { id, index } => {
                state.command(Command::MoveWorkspace {
                    workspace: WorkspaceId(u64::from(id)),
                    index: index as usize,
                });
            }
            cupertino_overview_v1::Request::MoveWindow { id, workspace } => {
                let window = WindowId(u64::from(id));
                let ws = WorkspaceId(u64::from(workspace));
                if !state.transactions.enqueue_window_change(
                    &client.id(),
                    QueuedWindowChange::MoveWindowToWorkspace {
                        window,
                        workspace: ws,
                    },
                ) {
                    state.command(Command::MoveWindowToWorkspace {
                        window,
                        workspace: ws,
                    });
                }
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoOverviewV1, _: &OverviewData) {
        state
            .overviews
            .observers
            .retain(|o| &o.resource != resource);
    }
}
