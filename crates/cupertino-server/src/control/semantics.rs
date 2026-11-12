//! Server-owned scope and transforms. Publications never grant permission or name another client's surface.
use super::*;
use cupertino_protocols::server::semantics::{
    cupertino_semantic_surface_v1::{self as publisher, CupertinoSemanticSurfaceV1},
    cupertino_semantics_v1::{self as manager, CupertinoSemanticsV1},
};
use cupertino_semantics as wire;
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::utils::{Serial, SERIAL_COUNTER};
use std::os::fd::{AsFd, OwnedFd};

/// One native action may be outstanding per run and per publication, with no automatic replay after timeout.
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Each connection may publish at most this many live native surfaces.
const MAX_PUBLISHERS: usize = 64;
/// The publisher version from which an action comes with a serial its popup may grab with.
const GRAB_SERIAL_VERSION: u32 = 2;
/// How many of the latest action grab serials are remembered after their action ends, so that a request taking an input
/// serial (a move, a resize, an activation token, a second grab) still knows them for what they are.
const SPENT_GRAB_SERIALS: usize = 64;
/// How long an action's grab serial stays good after the application answers the action as applied, so that a popup
/// the action's work shows a little later (a folder's stack once it is listed) still grabs.
const ANSWERED_GRAB_GRACE: Duration = Duration::from_secs(1);
pub(super) struct SemanticState {
    _global: GlobalId,
    next: u64,
    next_action: u64,
    revision: u64,
    pubs: Vec<Publication>,
    pending: Vec<Pending>,
    layout: Vec<wire::Window>,
    /// The epoch publications are subscribed under while any run observes, shared by the runs that see them; a fresh
    /// one is taken each time observation starts after no run observed, so no run sees what was published before.
    subscription: Option<u64>,
    /// Every action grab serial handed out lately, the latest last, live or not: none is ever an input serial.
    grab_serials: VecDeque<Serial>,
    /// The grab serials still good.
    grants: Vec<GrabGrant>,
}
/// The popup grab an action's publisher may take while it handles the action: once, with `serial`, for a popup whose
/// chain hangs from `root`, from its own connection. It goes when it is used, when the run that asked for the action
/// stops observing, when the application answers the action as anything but applied, and at `deadline`: the action's
/// own until it is answered, [`ANSWERED_GRAB_GRACE`] after an applied answer.
struct GrabGrant {
    serial: Serial,
    /// The action it came with.
    action: u64,
    client: ClientId,
    root: WlSurface,
    controller: CupertinoControlSessionV1,
    deadline: Instant,
}
/// What an action grab serial allows a popup grab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionGrab {
    /// The serial is not one an action handed out.
    NotAction,
    /// The action's grant: it is used up now.
    Granted,
    /// An action's serial, but spent, expired, withdrawn or for another client or window.
    Refused,
}
struct Publication {
    resource: CupertinoSemanticSurfaceV1,
    surface: WlSurface,
    handle: u64,
    /// The subscription the publication is under while a run sees it.
    epoch: Option<u64>,
    snapshot: Option<wire::Snapshot>,
    /// The newest publication that arrived and was not read yet (its descriptor and length): a newer one replaces it and
    /// closes its descriptor at once, so a burst is read once ([`Cupertino::flush_publications`]).
    unread: Option<(OwnedFd, u32)>,
}
struct Pending {
    id: u64,
    /// The subscription the action was sent under.
    epoch: u64,
    serial: u32,
    source: CupertinoSemanticSurfaceV1,
    controller: CupertinoControlSessionV1,
    deadline: Instant,
}
/// Where one window of a run's layout lies on the run's output: the rectangle input coordinates computed from an
/// observation fall in. Titles, focus and published content are not part of it, so they leave input revisions alone.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Placement {
    handle: u64,
    origin: [f64; 2],
    size: [f64; 2],
    scale: [f64; 2],
}
impl Placement {
    fn of(meta: &wire::Window, size: [f64; 2]) -> Self {
        Self {
            handle: meta.handle,
            origin: meta.origin,
            size,
            scale: meta.scale,
        }
    }
}
impl SemanticState {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display
                .create_global::<Cupertino, CupertinoSemanticsV1, ()>(GRAB_SERIAL_VERSION, ()),
            next: 1,
            next_action: 1,
            revision: 1,
            pubs: Vec::new(),
            pending: Vec::new(),
            layout: Vec::new(),
            subscription: None,
            grab_serials: VecDeque::new(),
            grants: Vec::new(),
        }
    }
}
fn split(n: u64) -> (u32, u32) {
    ((n >> 32) as u32, n as u32)
}
impl Cupertino {
    /// The runs that observe: active with the observe capability, in run order.
    fn semantic_runs(&self) -> Vec<usize> {
        if self.session_lock.is_active() || self.login_screen || self.prompt_shown() {
            return Vec::new();
        }
        (0..self.control.runs.len())
            .filter(|&i| {
                let r = &self.control.runs[i];
                r.resource.version() >= 3
                    && r.model.phase() == Phase::Active
                    && r.capabilities & 1 != 0
                    && r.resource
                        .client()
                        .is_some_and(|c| authorized(&c, ShellRole::DesktopControl))
            })
            .collect()
    }
    fn semantic_root_meta(
        &self,
        surface: &WlSurface,
        index: usize,
        handle: u64,
    ) -> Option<wire::Window> {
        self.semantic_root_placed(surface, index, handle)
            .map(|(meta, _)| meta)
    }
    /// [`Self::semantic_root_meta`] with the window's size on the output, in logical points.
    fn semantic_root_placed(
        &self,
        surface: &WlSurface,
        index: usize,
        handle: u64,
    ) -> Option<(wire::Window, [f64; 2])> {
        if protected_surface(surface)
            || crate::authorization::is_prompt(surface)
            || surface
                .client()
                .is_some_and(|c| authorized(&c, ShellRole::SessionLock))
        {
            return None;
        }
        let output = self.control.runs[index].output.as_ref()?.upgrade()?;
        let out = self.outputs.iter().find(|o| o.output == output)?;
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .is_some_and(|s| self.shell_surface_root(&s) == *surface);
        let list = crate::render::scene::windows_on_output(
            &self.wm,
            out.id,
            out.rect.w,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            self.now(),
        );
        for (id, slide, _) in list {
            let entry = self.entry(id)?;
            if entry.surface() != surface {
                continue;
            }
            let (mut wplan, _) = crate::render::scene::plan_for(
                &self.wm,
                entry,
                slide,
                self.now(),
                &self.fx,
                self.display_options(),
            )?;
            // Strip thumbnails are pictures, not live application targets.
            if self.window_managers.passes_input(id) {
                return None;
            }
            match self.window_managers.look(id, self.now()) {
                crate::window_manager::Look::Natural => {}
                crate::window_manager::Look::Hidden => return None,
                crate::window_manager::Look::Placed {
                    rect,
                    strip,
                    alpha,
                    rounding,
                } => {
                    if strip {
                        return None;
                    }
                    let corner = self
                        .window_managers
                        .corner_radius(self.config.window.corner_radius, rounding);
                    wplan = crate::render::scene::placed_plan(
                        entry,
                        rect,
                        wplan.opacity * alpha,
                        wplan.shadow,
                        corner,
                    );
                }
            }
            if wplan.opacity <= 0.0 {
                return None;
            }
            let geometry = entry.window.geometry();
            let ins = entry.insets();
            let window = self.wm.window(id)?;
            let meta = wire::Window {
                handle,
                app_id: self
                    .wm
                    .app(window.app)?
                    .identity
                    .chars()
                    .take(wire::MAX_TEXT)
                    .collect(),
                title: window.title.chars().take(wire::MAX_TEXT).collect(),
                origin: [
                    wplan.visual.x + (ins.left - f64::from(geometry.loc.x)) * wplan.scale_x,
                    wplan.visual.y + (ins.top - f64::from(geometry.loc.y)) * wplan.scale_y,
                ],
                scale: [wplan.scale_x, wplan.scale_y],
                focused,
                snapshot: None,
            };
            return Some((meta, [wplan.visual.w, wplan.visual.h]));
        }
        let map = layer_map_for_output(&output);
        let layer = map.layers().find(|l| l.wl_surface() == surface)?;
        let g = map.layer_geometry(layer)?;
        if layer.bbox().size.w == 0 || layer.bbox().size.h == 0 {
            return None;
        }
        let meta = wire::Window {
            handle,
            app_id: layer.namespace().chars().take(wire::MAX_TEXT).collect(),
            title: layer.namespace().chars().take(wire::MAX_TEXT).collect(),
            origin: [
                out.rect.x + f64::from(g.loc.x),
                out.rect.y + f64::from(g.loc.y),
            ],
            scale: [1.0, 1.0],
            focused,
            snapshot: None,
        };
        Some((meta, [f64::from(g.size.w), f64::from(g.size.h)]))
    }
    fn semantic_meta(&self, p: &Publication, index: usize) -> Option<wire::Window> {
        self.semantic_placed(p, index).map(|(meta, _)| meta)
    }
    /// [`Self::semantic_meta`] with the surface's size on the output, in logical points.
    fn semantic_placed(&self, p: &Publication, index: usize) -> Option<(wire::Window, [f64; 2])> {
        if !p.surface.is_alive() {
            return None;
        }
        let root = self.shell_surface_root(&p.surface);
        let (mut meta, mut size) = self.semantic_root_placed(&root, index, p.handle)?;
        if root != p.surface {
            let (popup, offset) = PopupManager::popups_for_surface(&root)
                .find(|(popup, _)| popup.wl_surface() == &p.surface)?;
            let g = popup.geometry();
            meta.origin[0] += f64::from(offset.x - g.loc.x) * meta.scale[0];
            meta.origin[1] += f64::from(offset.y - g.loc.y) * meta.scale[1];
            size = [
                f64::from(g.size.w) * meta.scale[0],
                f64::from(g.size.h) * meta.scale[1],
            ];
        }
        Some((meta, size))
    }
    /// Tell the observing runs that what they observe changed. Their geometry revisions stay: a new title, focus or
    /// publication leaves input coordinates where they were ([`Placement`]).
    fn semantics_changed(&mut self) {
        self.control.semantics.revision = self
            .control
            .semantics
            .revision
            .checked_add(1)
            .expect("semantic revision exhausted");
        let (h, l) = split(self.control.semantics.revision);
        for i in self.semantic_runs() {
            self.control.runs[i].resource.semantic_changed(h, l);
        }
    }
    /// Pending native replies have a real deadline even while every output is idle.
    pub(crate) fn control_deadline(&self) -> Option<f64> {
        self.control
            .semantics
            .pending
            .iter()
            .map(|p| p.deadline)
            .min()
            .map(|at| {
                // The loop schedules model time; native timeouts retain their wall-time duration with --time-scale.
                self.now()
                    + at.saturating_duration_since(Instant::now()).as_secs_f64() * self.time_scale
            })
    }
    /// Read the newest publication each publisher has sent since the last call and tell the observing runs once. An
    /// invalid, stale or unsealed one is a protocol error of its publisher, as when it arrived.
    pub(crate) fn flush_publications(&mut self) {
        let mut changed = false;
        for p in &mut self.control.semantics.pubs {
            let Some((payload, length)) = p.unread.take() else {
                continue;
            };
            match wire::read::<wire::Snapshot>(payload, length, wire::MAX_FRAME)
                .ok()
                .filter(|s| s.validate().is_ok())
            {
                Some(snapshot)
                    if p.snapshot
                        .as_ref()
                        .is_none_or(|old| snapshot.revision > old.revision) =>
                {
                    p.snapshot = Some(snapshot);
                    changed = true;
                }
                _ => {
                    p.snapshot = None;
                    p.resource.post_error(
                        manager::Error::InvalidPayload,
                        "invalid, stale or unsealed semantic publication",
                    );
                }
            }
        }
        if changed {
            self.semantics_changed();
        }
    }
    pub(super) fn refresh_semantics(&mut self) {
        let active = self.semantic_runs();
        if active.is_empty() {
            self.control.semantics.subscription = None;
        } else if self.control.semantics.subscription.is_none() {
            self.control.semantics.subscription = self.control.allocate_epoch().map(Epoch::get);
        }
        let subscription = self.control.semantics.subscription;
        let mut layout = Vec::new();
        let mut placements = Vec::with_capacity(active.len());
        for &i in &active {
            let mut placed = Vec::new();
            // toplevels in stacking order, so a restack moves the run's input revision on
            for &id in self.wm.stacking_order() {
                if let Some((m, size)) = self.entry(id).and_then(|entry| {
                    let handle = entry.id.0.checked_mul(2)?;
                    self.semantic_root_placed(entry.surface(), i, handle)
                }) {
                    placed.push(Placement::of(&m, size));
                    layout.push(m);
                }
            }
            for p in &self.control.semantics.pubs {
                if !self.windows.iter().any(|w| w.surface() == &p.surface) {
                    if let Some((m, size)) = self.semantic_placed(p, i) {
                        placed.push(Placement::of(&m, size));
                        layout.push(m);
                    }
                }
            }
            placements.push(placed);
        }
        if layout != self.control.semantics.layout {
            self.control.semantics.layout = layout;
            self.semantics_changed();
        }
        // Only where windows lie moves input revisions on: coordinates an agent computed from an observation point at
        // the same place while titles, focus and published content change.
        for (&i, placed) in active.iter().zip(placements) {
            let run = &mut self.control.runs[i];
            // a run revoked meanwhile, as by an exhausted revision, no longer observes
            if run.model.phase() != Phase::Active || run.placements.as_ref() == Some(&placed) {
                continue;
            }
            // the first layout a run sees is the one its first revision stands for
            if run.placements.replace(placed).is_some() {
                self.control_advance(i);
            }
        }
        let epochs: Vec<_> = self
            .control
            .semantics
            .pubs
            .iter()
            .map(|p| {
                subscription.filter(|_| active.iter().any(|&i| self.semantic_meta(p, i).is_some()))
            })
            .collect();
        let mut changed = false;
        for (p, epoch) in self.control.semantics.pubs.iter_mut().zip(epochs) {
            if p.epoch != epoch {
                p.epoch = epoch;
                p.snapshot = None;
                p.unread = None;
                let (h, l) = split(epoch.unwrap_or(0));
                p.resource.interest(h, l, u32::from(epoch.is_some()));
                changed = true;
            }
        }
        if changed {
            self.semantics_changed();
        }
        let pending = std::mem::take(&mut self.control.semantics.pending);
        for p in pending {
            let valid = active
                .iter()
                .any(|&i| self.control.runs[i].resource == p.controller)
                && self
                    .control
                    .semantics
                    .pubs
                    .iter()
                    .any(|s| s.resource == p.source && s.epoch == Some(p.epoch));
            if valid && Instant::now() < p.deadline {
                self.control.semantics.pending.push(p);
                continue;
            }
            // a withdrawn or timed-out action takes its grab serial with it
            self.control.semantics.grants.retain(|g| g.action != p.id);
            if let Some(i) = self
                .control
                .runs
                .iter()
                .position(|r| r.resource == p.controller)
            {
                self.control_finish(i, p.serial, Err(session::Refusal::UnknownOutcome));
            }
        }
        self.expire_action_grabs();
    }
    pub(super) fn control_observe(
        &mut self,
        index: usize,
        serial: u32,
    ) -> Result<(), session::Refusal> {
        self.flush_publications();
        if !self.semantic_runs().contains(&index) {
            return Err(session::Refusal::NotGranted);
        }
        let mut windows = Vec::new();
        let output = self.control.runs[index]
            .output
            .as_ref()
            .and_then(WeakOutput::upgrade)
            .ok_or(session::Refusal::OutsideScope)?;
        // Toplevel handles are even and belong to the compositor window's lifetime; popup/layer publisher handles are odd.
        for entry in &self.windows {
            let Some(handle) = entry.id.0.checked_mul(2) else {
                return Err(session::Refusal::Unsupported);
            };
            if let Some(mut meta) = self.semantic_root_meta(entry.surface(), index, handle) {
                if let Some(p) = self
                    .control
                    .semantics
                    .pubs
                    .iter()
                    .find(|p| p.surface == *entry.surface() && p.epoch.is_some())
                {
                    meta.snapshot = p.snapshot.clone();
                }
                windows.push(meta);
            }
        }
        for p in &self.control.semantics.pubs {
            if p.epoch.is_none() || self.windows.iter().any(|e| *e.surface() == p.surface) {
                continue;
            }
            if let Some(mut meta) = self.semantic_meta(p, index) {
                meta.snapshot = p.snapshot.clone();
                windows.push(meta);
            }
        }
        if windows.len() > wire::MAX_WINDOWS {
            return Err(session::Refusal::Unsupported);
        }
        let g = self
            .control_output_geometry(&output)
            .ok_or(session::Refusal::OutsideScope)?;
        let observation = wire::Observation {
            geometry: wire::OutputGeometry {
                revision: self.control.runs[index].revision,
                name: output.name(),
                x: g.rect.0,
                y: g.rect.1,
                width: g.rect.2,
                height: g.rect.3,
                scale: g.scale,
                transform: format!("{:?}", g.transform),
            },
            epoch: self.control.runs[index].model.epoch().get(),
            revision: self.control.semantics.revision,
            geometry_revision: self.control.runs[index].revision,
            windows,
        };
        let (file, len) = wire::seal(&observation, wire::MAX_OBSERVATION)
            .map_err(|_| session::Refusal::Unsupported)?;
        self.control.runs[index]
            .resource
            .observation(serial, file.as_fd(), len);
        Ok(())
    }
    pub(super) fn control_perform(
        &mut self,
        index: usize,
        serial: u32,
        fd: OwnedFd,
        len: u32,
    ) -> Result<bool, session::Refusal> {
        self.flush_publications();
        if !self.semantic_runs().contains(&index) || self.control.runs[index].capabilities & 16 == 0
        {
            return Err(session::Refusal::NotGranted);
        }
        let call: wire::Invocation =
            wire::read(fd, len, wire::MAX_FRAME).map_err(|_| session::Refusal::InvalidInput)?;
        let root = if call.window & 1 == 0 {
            self.windows
                .iter()
                .find(|e| e.id.0.checked_mul(2) == Some(call.window))
                .map(|e| e.surface().clone())
        } else {
            None
        };
        let pi = self
            .control
            .semantics
            .pubs
            .iter()
            .position(|p| {
                root.as_ref() == Some(&p.surface)
                    || (call.window & 1 != 0 && p.handle == call.window)
            })
            .ok_or(session::Refusal::Unsupported)?;
        let p = &self.control.semantics.pubs[pi];
        if self.semantic_meta(p, index).is_none() {
            return Err(session::Refusal::OutsideScope);
        }
        let snapshot = p.snapshot.as_ref().ok_or(session::Refusal::Busy)?;
        if snapshot.revision != call.revision {
            return Err(session::Refusal::StaleGeometry);
        }
        if !snapshot.permits(call.node, call.action, call.data.as_ref()) {
            return Err(session::Refusal::Blocked);
        }
        let controller = &self.control.runs[index].resource;
        if self
            .control
            .semantics
            .pending
            .iter()
            .any(|q| q.controller == *controller || q.source == p.resource)
        {
            return Err(session::Refusal::Busy);
        }
        let epoch = p.epoch.ok_or(session::Refusal::OutsideScope)?;
        let id = self.control.semantics.next_action;
        self.control.semantics.next_action =
            id.checked_add(1).ok_or(session::Refusal::Unsupported)?;
        let action = wire::ActionCall {
            id,
            epoch,
            revision: call.revision,
            node: call.node,
            action: call.action,
            data: call.data,
        };
        let (file, len) =
            wire::seal(&action, wire::MAX_FRAME).map_err(|_| session::Refusal::InvalidInput)?;
        let source = self.control.semantics.pubs[pi].resource.clone();
        if let Some(client) = source
            .client()
            .filter(|c| authorized(c, ShellRole::MenuBar))
        {
            if !self.control.injected_clients.contains(&client.id()) {
                self.control.injected_clients.push(client.id());
            }
        }
        let controller = self.control.runs[index].resource.clone();
        let deadline = Instant::now() + ACTION_TIMEOUT;
        // a menu the action shows grabs as one a press opened would, with a serial good for that alone
        if let Some(client) = source
            .client()
            .filter(|_| source.version() >= GRAB_SERIAL_VERSION)
        {
            let serial = SERIAL_COUNTER.next_serial();
            let root = self.shell_surface_root(&self.control.semantics.pubs[pi].surface);
            let semantics = &mut self.control.semantics;
            if semantics.grab_serials.len() == SPENT_GRAB_SERIALS {
                semantics.grab_serials.pop_front();
            }
            semantics.grab_serials.push_back(serial);
            semantics.grants.push(GrabGrant {
                serial,
                action: id,
                client: client.id(),
                root,
                controller: controller.clone(),
                deadline,
            });
            source.grab_serial(serial.into());
        }
        source.perform(file.as_fd(), len);
        self.control.semantics.pending.push(Pending {
            id,
            epoch,
            serial,
            source,
            controller,
            deadline,
        });
        Ok(true)
    }

    /// The action `action` was answered with `outcome`: an applied one leaves its grab serial good a moment longer, for
    /// the popup its work may still show; any other takes it away.
    fn action_answered(&mut self, action: u64, outcome: wire::Outcome) {
        let grants = &mut self.control.semantics.grants;
        if outcome == wire::Outcome::Applied {
            let until = Instant::now() + ANSWERED_GRAB_GRACE;
            for g in grants.iter_mut().filter(|g| g.action == action) {
                g.deadline = g.deadline.min(until);
            }
        } else {
            grants.retain(|g| g.action != action);
        }
    }

    /// Drop the grab serials no longer good: expired, of a window gone, or of a run that no longer observes (a lock,
    /// the login screen, the authentication prompt or the run's end withdraws them at once).
    fn expire_action_grabs(&mut self) {
        let runs: Vec<_> = self
            .semantic_runs()
            .into_iter()
            .map(|i| self.control.runs[i].resource.clone())
            .collect();
        let now = Instant::now();
        self.control
            .semantics
            .grants
            .retain(|g| now < g.deadline && g.root.is_alive() && runs.contains(&g.controller));
    }

    /// Whether `serial` is one an action handed out for a popup grab, live or spent: it is never an input serial, so a
    /// move, a resize, a drag or an activation token that names it is refused.
    pub(crate) fn is_action_grab_serial(&self, serial: Serial) -> bool {
        self.control.semantics.grab_serials.contains(&serial)
    }

    /// A popup grab of `client` for a popup chain hanging from `root` that names `serial`. An action's serial is
    /// [`ActionGrab::Granted`] once, while its grant lasts ([`GrabGrant`]), to the client that publishes the action's
    /// window and for a popup of that window; any other use of it is [`ActionGrab::Refused`].
    pub(crate) fn take_action_grab(
        &mut self,
        client: &ClientId,
        root: &WlSurface,
        serial: Serial,
    ) -> ActionGrab {
        if !self.is_action_grab_serial(serial) {
            return ActionGrab::NotAction;
        }
        self.expire_action_grabs();
        let root = self.shell_surface_root(root);
        let grants = &mut self.control.semantics.grants;
        match grants
            .iter()
            .position(|g| g.serial == serial && g.client == *client && g.root == root)
        {
            Some(i) => {
                grants.remove(i);
                ActionGrab::Granted
            }
            None => ActionGrab::Refused,
        }
    }
}
impl GlobalDispatch<CupertinoSemanticsV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        id: New<CupertinoSemanticsV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(id, ());
    }
}
impl Dispatch<CupertinoSemanticsV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoSemanticsV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let manager::Request::GetSurface { id, surface } = request {
            let valid = surface.client().as_ref() == Some(client)
                && !state
                    .control
                    .semantics
                    .pubs
                    .iter()
                    .any(|p| p.surface == surface)
                && state
                    .control
                    .semantics
                    .pubs
                    .iter()
                    .filter(|p| p.surface.client().as_ref() == Some(client))
                    .count()
                    < MAX_PUBLISHERS;
            let publisher = init.init(id, ());
            if !valid {
                resource.post_error(
                    manager::Error::InvalidSurface,
                    "one owned publisher per surface, within the connection limit",
                );
                return;
            }
            let Some(next) = state.control.semantics.next.checked_add(2) else {
                resource.post_error(manager::Error::InvalidSurface, "surface handles exhausted");
                return;
            };
            let handle = state.control.semantics.next;
            state.control.semantics.next = next;
            state.control.semantics.pubs.push(Publication {
                resource: publisher,
                surface,
                handle,
                epoch: None,
                snapshot: None,
                unread: None,
            });
            state.refresh_semantics();
            state.semantics_changed();
        }
    }
}
impl Dispatch<CupertinoSemanticSurfaceV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoSemanticSurfaceV1,
        request: publisher::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        state.refresh_control();
        let Some(i) = state
            .control
            .semantics
            .pubs
            .iter()
            .position(|p| p.resource == *resource)
        else {
            return;
        };
        match request {
            publisher::Request::Publish {
                epoch_hi,
                epoch_lo,
                payload,
                length,
            } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                let p = &mut state.control.semantics.pubs[i];
                if p.epoch != Some(epoch) {
                    return;
                }
                // read once per loop turn, the newest only
                p.unread = Some((payload, length));
            }
            publisher::Request::Unavailable { epoch_hi, epoch_lo } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                let p = &mut state.control.semantics.pubs[i];
                if p.epoch == Some(epoch) {
                    p.snapshot = None;
                    p.unread = None;
                    state.semantics_changed();
                }
            }
            publisher::Request::Complete { payload, length } => {
                // the reply is checked against the revision of the publication sent before it
                state.flush_publications();
                let Ok(reply) = wire::read::<wire::ActionReply>(payload, length, wire::MAX_FRAME)
                else {
                    return;
                };
                let Some(pi) = state.control.semantics.pending.iter().position(|p| {
                    p.id == reply.id && p.epoch == reply.epoch && p.source == *resource
                }) else {
                    return;
                };
                let p = state.control.semantics.pending.remove(pi);
                let valid_revision = reply.outcome != wire::Outcome::Applied
                    || state.control.semantics.pubs[i]
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.revision == reply.revision);
                let outcome = if valid_revision {
                    reply.outcome
                } else {
                    wire::Outcome::Stale
                };
                state.action_answered(p.id, outcome);
                if let Some(run) = state
                    .control
                    .runs
                    .iter()
                    .position(|r| r.resource == p.controller)
                {
                    if !valid_revision {
                        state.control_finish(run, p.serial, Err(session::Refusal::UnknownOutcome));
                        return;
                    }
                    if let Ok((file, len)) = wire::seal(&reply, wire::MAX_FRAME) {
                        p.controller.action_finished(p.serial, file.as_fd(), len);
                        state.control_finish(run, p.serial, Ok(()));
                    }
                }
            }
            _ => {}
        }
    }
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoSemanticSurfaceV1, _: &()) {
        state
            .control
            .semantics
            .pubs
            .retain(|p| p.resource != *resource);
        state.refresh_semantics();
        state.semantics_changed();
    }
}
