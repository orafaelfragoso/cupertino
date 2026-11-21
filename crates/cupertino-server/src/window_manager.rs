//! Server side of the window manager (`cupertino_shell_v1.get_window_manager`, `cupertino_window_manager_v1`): the
//! window manager owns the stage policy (which group is on the stage, the strip of recent groups) and commits which
//! arranged windows are hidden; the server keeps the frame clock and fades a hidden window out where it is drawn and a
//! shown one back in at its own frame, with the stage model's alpha spring, and keeps hidden windows out of hit
//! testing. The strip is the window manager's own surface, whose holes claim its windows (`transaction.rs`); the server
//! draws no strip. A window manager that goes away takes its placements with it: every window is drawn and hit at its
//! own frame again at once. A shell component's window manager whose persistent surface is held keeps them held
//! instead (D19): until a new instance takes the hold over, its placement commits are kept, and `ready` or the hold
//! timeout ends the held placements.

use std::collections::{BTreeMap, HashMap};

use cupertino_animation::{AnimatedValue, Curve};
use cupertino_protocols::server::cupertino_shell_v1::{CupertinoShellV1, Error as ShellError};
use cupertino_protocols::server::cupertino_window_manager_v1::{
    self, CupertinoWindowManagerV1, Error, Placement as WirePlacement, WindowState,
};
use cupertino_wm::stage::StageConfig;
use cupertino_wm::{OutputId, Rect, WindowId};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource, WEnum,
};

use crate::authorization::{authorized, refuse, require, role, Permit, ShellRole};
use crate::host::{ComponentConnection, WmPlacementHold};
use crate::menu_bar::cut_text;
use crate::state::{Cupertino, WindowEntry};

/// Shell version from which `cupertino_window_manager_v1` has the messages of this server: version 29 removed the strip
/// placement, the rectangle of `set_placement` and `strip_pressed`, which moved the opcode of `done`, so an older
/// window manager speaks another wire and is left inert.
const WINDOW_MANAGER_MESSAGES_SINCE: u32 = 29;

/// The component whose holes are the stage strip's thumbnails.
const WINDOW_MANAGER_COMPONENT: &str = "windowmanager";

/// Where a placed window is headed. A window on the stage has no entry once its fade-in ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// Back at its own frame.
    Stage,
    Hidden,
}

struct Placed {
    output: OutputId,
    target: Target,
    /// The rectangle the window is drawn at while it fades: where it was when it was hidden, its own frame as it comes
    /// back.
    frame: Rect,
    /// Alpha track: 0 for a hidden window, 1 on the stage.
    alpha: AnimatedValue<f64>,
}

impl Placed {
    fn is_animating(&self, now: f64) -> bool {
        self.alpha.is_animating(now)
    }
}

/// How a window is drawn at a moment, given its own (natural) rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// No placement: drawn and hit as every window without a window manager.
    Natural,
    Hidden,
    /// Scaled into `rect` at `alpha` (times its own opacity) as it fades out or back in, with the window's own corners.
    Placed { rect: Rect, alpha: f64 },
}

/// What a window manager was told about one arranged window.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FeedWindow {
    app_id: String,
    frame: (i32, i32, i32, i32),
    active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Feed {
    usable: (i32, i32, i32, i32),
    windows: BTreeMap<u32, FeedWindow>,
}

struct Manager {
    resource: CupertinoWindowManagerV1,
    output: OutputId,
    /// `None` until the first group.
    sent: Option<Feed>,
    pending: BTreeMap<u32, Target>,
    /// Placements this manager committed while the output's placements were held for the instance before (D19):
    /// applied in its takeover frame, or when the hold ends.
    deferred: Option<BTreeMap<u32, Target>>,
}

/// The window managers (at most one per output) and the placements they committed.
pub struct WindowManagers {
    managers: Vec<Manager>,
    placed: HashMap<WindowId, Placed>,
    /// The spring of a window's fade out and back in.
    alpha: Curve,
}

impl WindowManagers {
    /// A hidden window fades out, and back in, with `stage.alpha_curve`.
    pub fn new(stage: &StageConfig) -> Self {
        Self {
            managers: Vec::new(),
            placed: HashMap::new(),
            alpha: stage.alpha_curve,
        }
    }

    /// How window `id` is drawn at `now`. A hidden window fades out where it was drawn and is not drawn once the fade
    /// ends.
    pub fn look(&self, id: WindowId, now: f64) -> Look {
        let Some(p) = self.placed.get(&id) else {
            return Look::Natural;
        };
        if !p.is_animating(now) {
            return match p.target {
                Target::Hidden => Look::Hidden,
                Target::Stage => Look::Natural,
            };
        }
        Look::Placed {
            rect: p.frame,
            alpha: p.alpha.value(now),
        }
    }

    /// Whether the pointer passes through window `id`: a hidden window takes no input of its client.
    pub fn passes_input(&self, id: WindowId) -> bool {
        self.placed
            .get(&id)
            .is_some_and(|p| p.target == Target::Hidden)
    }

    pub fn wants_tick(&self, now: f64) -> bool {
        self.placed.values().any(|p| p.is_animating(now))
    }

    /// Keep window `id`, while it fades back in on the stage, at its own frame `natural` (a client that resized
    /// meanwhile).
    fn follow_natural(&mut self, id: WindowId, natural: Rect) {
        if let Some(p) = self.placed.get_mut(&id) {
            if p.target == Target::Stage {
                p.frame = natural;
            }
        }
    }

    /// Drop the entries of windows that are back on the stage.
    pub fn tick(&mut self, now: f64) {
        self.placed
            .retain(|_, p| p.target != Target::Stage || p.is_animating(now));
    }

    pub fn remove_output(&mut self, output: OutputId) {
        self.managers.retain(|m| m.output != output);
        self.placed.retain(|_, p| p.output != output);
    }

    fn of_output(&self, output: OutputId) -> Option<&Manager> {
        self.managers.iter().find(|m| m.output == output)
    }

    /// Tell every window manager the stage shortcut was pressed.
    pub fn toggle_stage(&self) {
        for m in &self.managers {
            m.resource.toggle_stage();
            m.resource.done();
        }
    }

    /// Head window `id` for `target`: a hidden window stops where it is drawn and fades out; one shown again fades in
    /// at its own frame `natural`, from the alpha it has. False when it was already headed there.
    fn retarget(
        &mut self,
        id: WindowId,
        output: OutputId,
        target: Target,
        natural: Rect,
        now: f64,
    ) -> bool {
        let alpha = self.alpha;
        let entry = self.placed.entry(id).or_insert_with(|| Placed {
            output,
            target: Target::Stage,
            frame: natural,
            alpha: AnimatedValue::new(1.0),
        });
        if entry.target == target {
            return false;
        }
        entry.frame = natural;
        entry
            .alpha
            .animate_to(if target == Target::Hidden { 0.0 } else { 1.0 }, alpha, now);
        entry.target = target;
        entry.output = output;
        true
    }
}

/// Per-object data: `None` for an object the server rejected with a protocol error; it stays inert.
pub struct WindowManagerData {
    output: Option<OutputId>,
    /// The shell component whose connection made the object, if any: its placements can outlive the object (D19).
    component: Option<ComponentConnection>,
}

fn whole(r: Rect) -> (i32, i32, i32, i32) {
    (
        r.x.round() as i32,
        r.y.round() as i32,
        r.w.round() as i32,
        r.h.round() as i32,
    )
}

/// Send `resource` one group with what differs between `known` (its last group; `None` before
/// the first) and `feed`; nothing when nothing changed.
fn send_feed(resource: &CupertinoWindowManagerV1, known: Option<&Feed>, feed: &Feed) {
    let mut changed = known.is_none();
    if known.map(|k| k.usable) != Some(feed.usable) {
        let (x, y, w, h) = feed.usable;
        resource.usable_area(x, y, w, h);
        changed = true;
    }
    for (id, info) in &feed.windows {
        if known.and_then(|k| k.windows.get(id)) != Some(info) {
            let (x, y, w, h) = info.frame;
            let state = if info.active {
                WindowState::Active
            } else {
                WindowState::empty()
            };
            resource.window(*id, cut_text(&info.app_id).to_owned(), x, y, w, h, state);
            changed = true;
        }
    }
    if let Some(k) = known {
        for id in k.windows.keys().filter(|id| !feed.windows.contains_key(id)) {
            resource.window_closed(*id);
            changed = true;
        }
    }
    if changed {
        resource.done();
    }
}

impl Cupertino {
    /// Drop the window managers whose connection no longer holds the WindowManager role.
    pub(crate) fn release_revoked_window_managers(&mut self) {
        let revoked: Vec<_> = self
            .window_managers
            .managers
            .iter()
            .filter(|m| {
                m.resource
                    .client()
                    .is_some_and(|c| !authorized(&c, ShellRole::WindowManager))
            })
            .map(|m| (m.resource.clone(), m.output))
            .collect();
        for (resource, output) in revoked {
            self.window_managers
                .managers
                .retain(|m| m.resource != resource);
            self.window_managers
                .placed
                .retain(|_, p| p.output != output);
        }
        self.request_redraw();
    }

    /// Whether the stage of `output` is on as far as the server can tell: a window there is hidden, or in a hole of
    /// the window manager's strip. A stage holding only its centre group shows neither and is not seen (the protocol
    /// carries no stage state).
    pub fn stage_active(&self, output: OutputId) -> bool {
        self.window_managers
            .placed
            .values()
            .any(|p| p.output == output && p.target == Target::Hidden)
            || self.wm.windows().any(|w| {
                w.output == output
                    && self
                        .transactions
                        .active_claim_for(w.id)
                        .is_some_and(|hole| hole.component == WINDOW_MANAGER_COMPONENT)
            })
    }

    /// `cupertino_shell_v1.get_window_manager`: one per output; send the first group.
    pub fn create_window_manager(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::WindowManager>>,
        id: New<CupertinoWindowManagerV1>,
        output: &WlOutput,
        init: &mut DataInit<'_, Self>,
    ) {
        if permit.is_none() {
            init.init(
                id,
                WindowManagerData {
                    output: None,
                    component: None,
                },
            );
            refuse(shell, ShellRole::WindowManager);
            return;
        }
        let target =
            Output::from_resource(output).and_then(|o| self.output_by_wl(&o).map(|e| e.id));
        let rejection = match target {
            None => Some("the output is gone"),
            Some(out) if self.window_managers.of_output(out).is_some() => {
                Some("the output already has a window manager")
            }
            Some(_) => None,
        };
        if let Some(reason) = rejection {
            init.init(
                id,
                WindowManagerData {
                    output: None,
                    component: None,
                },
            );
            shell.post_error(ShellError::Role, reason);
            return;
        }
        if shell.version() < WINDOW_MANAGER_MESSAGES_SINCE {
            tracing::warn!(
                version = shell.version(),
                since = WINDOW_MANAGER_MESSAGES_SINCE,
                "a window manager bound below the shell version of this server's window manager messages is left inert"
            );
            init.init(
                id,
                WindowManagerData {
                    output: None,
                    component: None,
                },
            );
            return;
        }
        let out = target.expect("a rejected output returned above");
        let component = shell.client().and_then(|c| {
            c.get_data::<crate::state::ClientState>()
                .and_then(|s| s.component)
        });
        let resource = init.init(
            id,
            WindowManagerData {
                output: Some(out),
                component,
            },
        );
        self.window_managers.managers.push(Manager {
            resource,
            output: out,
            sent: None,
            pending: BTreeMap::new(),
            deferred: None,
        });
        self.refresh_window_managers();
    }

    /// The windows the window manager of `output` arranges, and the usable area.
    fn window_manager_feed(&self, output: OutputId) -> Feed {
        let key = self.wm.key_window();
        let current = self.wm.current_workspace(output);
        let windows = self
            .wm
            .windows()
            .filter(|w| {
                let s = w.state();
                w.output == output
                    && s.is_visible()
                    && !s.is_minimized()
                    && !s.is_dead()
                    && w.parent.is_none()
                    && (w.sticky || Some(w.workspace) == current)
                    && !self.wm.app(w.app).is_some_and(|a| a.hidden)
            })
            .map(|w| {
                let app_id = self
                    .wm
                    .app(w.app)
                    .map(|a| a.identity.clone())
                    .unwrap_or_default();
                (
                    w.id.0 as u32,
                    FeedWindow {
                        app_id,
                        frame: whole(w.geometry()),
                        active: key == Some(w.id),
                    },
                )
            })
            .collect();
        Feed {
            usable: whole(self.usable_area(output)),
            windows,
        }
    }

    /// Send every window manager what changed since its last group, and drop the placements of
    /// windows that left its set. Call after anything the feed derives from may have changed.
    pub fn refresh_window_managers(&mut self) {
        if self.window_managers.managers.is_empty() {
            return;
        }
        let feeds: Vec<Feed> = self
            .window_managers
            .managers
            .iter()
            .map(|m| self.window_manager_feed(m.output))
            .collect();
        let wms = &mut self.window_managers;
        for (m, feed) in wms.managers.iter_mut().zip(feeds) {
            send_feed(&m.resource, m.sent.as_ref(), &feed);
            let output = m.output;
            wms.placed
                .retain(|id, p| p.output != output || feed.windows.contains_key(&(id.0 as u32)));
            m.sent = Some(feed);
        }
    }

    /// The rectangle window `id` is drawn at without a placement.
    fn natural_rect(&self, entry: &WindowEntry, now: f64) -> Option<Rect> {
        crate::render::scene::plan_for(&self.wm, entry, 0.0, now, &self.fx, self.display_options())
            .map(|(plan, _)| plan.visual)
    }

    /// Advance the placements at `now`: windows fading back in on the stage follow their own frame, and the entries
    /// of those that are back are dropped.
    pub fn tick_window_managers(&mut self, now: f64) {
        let heading: Vec<WindowId> = self
            .window_managers
            .placed
            .iter()
            .filter(|(_, p)| p.target == Target::Stage)
            .map(|(id, _)| *id)
            .collect();
        for id in heading {
            if let Some(natural) = self.entry(id).and_then(|e| self.natural_rect(e, now)) {
                self.window_managers.follow_natural(id, natural);
            }
        }
        self.window_managers.tick(now);
    }

    /// `commit`: retarget every window whose pending placement differs from its current one. While the output's
    /// placements are held for the instance before (D19) the commit is kept for the takeover frame; the first commit
    /// after a takeover that brought none replaces the held placements.
    pub(crate) fn commit_placements(&mut self, output: OutputId) {
        let hold = self.hosts.wm_placement_hold(output);
        let Some(m) = self
            .window_managers
            .managers
            .iter_mut()
            .find(|m| m.output == output)
        else {
            return;
        };
        let pending = std::mem::take(&mut m.pending);
        match hold {
            Some(WmPlacementHold::Held) => {
                m.deferred.get_or_insert_with(BTreeMap::new).extend(pending);
            }
            Some(WmPlacementHold::AwaitingCommit) => {
                self.hosts.set_wm_placement_hold(output, None);
                self.apply_placements(output, pending, true);
            }
            None => self.apply_placements(output, pending, false),
        }
    }

    /// Retarget the windows of `output` to `targets`. With `replace`, every other hidden window of the output fades
    /// back in at its own frame: the placements of an instance before give way.
    fn apply_placements(
        &mut self,
        output: OutputId,
        targets: BTreeMap<u32, Target>,
        replace: bool,
    ) {
        let now = self.now();
        let arranged = self
            .window_managers
            .of_output(output)
            .and_then(|m| m.sent.as_ref())
            .map(|f| f.windows.keys().copied().collect::<Vec<u32>>())
            .unwrap_or_default();
        if replace {
            let returning: Vec<WindowId> = self
                .window_managers
                .placed
                .iter()
                .filter(|(id, p)| {
                    p.output == output
                        && p.target != Target::Stage
                        && !targets.contains_key(&(id.0 as u32))
                })
                .map(|(id, _)| *id)
                .collect();
            for window in returning {
                match self.entry(window).and_then(|e| self.natural_rect(e, now)) {
                    Some(natural) => {
                        self.window_managers
                            .retarget(window, output, Target::Stage, natural, now);
                    }
                    None => {
                        self.window_managers.placed.remove(&window);
                    }
                }
            }
        }
        for (id, target) in targets {
            // a window that left the set since the client decided: it may have just closed
            if !arranged.contains(&id) {
                continue;
            }
            let window = WindowId(u64::from(id));
            let Some(natural) = self.entry(window).and_then(|e| self.natural_rect(e, now)) else {
                continue;
            };
            if self
                .window_managers
                .retarget(window, output, target, natural, now)
            {
                let app = self
                    .wm
                    .window(window)
                    .and_then(|w| self.wm.app(w.app))
                    .map(|a| a.identity.as_str())
                    .unwrap_or_default();
                tracing::info!(window = id, app, placement = ?target, "stage placement");
            }
        }
        self.request_redraw();
    }

    /// The takeover frame of a window manager's held surface on `output` (D19): the placements the new instance
    /// committed replace the held ones; with none committed yet the held ones stay until its first `commit`.
    pub(crate) fn take_over_wm_placements(&mut self, output: OutputId) {
        if self.hosts.wm_placement_hold(output) != Some(WmPlacementHold::Held) {
            return;
        }
        let deferred = self
            .window_managers
            .managers
            .iter_mut()
            .find(|m| m.output == output)
            .and_then(|m| m.deferred.take());
        match deferred {
            Some(targets) => {
                self.hosts.set_wm_placement_hold(output, None);
                self.apply_placements(output, targets, true);
            }
            None => self
                .hosts
                .set_wm_placement_hold(output, Some(WmPlacementHold::AwaitingCommit)),
        }
    }

    /// End the placements held on `output` (`ready`, the hold timeout, the component stopped): each hidden window fades
    /// back in at its frame, and what a new instance committed meanwhile applies.
    pub(crate) fn end_held_wm_placements(&mut self, output: OutputId) {
        if self.hosts.wm_placement_hold(output).is_none() {
            return;
        }
        self.hosts.set_wm_placement_hold(output, None);
        let deferred = self
            .window_managers
            .managers
            .iter_mut()
            .find(|m| m.output == output)
            .and_then(|m| m.deferred.take())
            .unwrap_or_default();
        self.apply_placements(output, deferred, true);
    }
}

impl Dispatch<CupertinoWindowManagerV1, WindowManagerData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoWindowManagerV1,
        request: cupertino_window_manager_v1::Request,
        data: &WindowManagerData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(output) = data.output else { return };
        if !require(client, ShellRole::WindowManager, resource) {
            return;
        }
        match request {
            cupertino_window_manager_v1::Request::SetPlacement { id, placement } => {
                let target = match placement {
                    WEnum::Value(WirePlacement::Stage) => Target::Stage,
                    WEnum::Value(WirePlacement::Hidden) => Target::Hidden,
                    // the generated enum is non-exhaustive; a value it gains later is not handled here yet
                    WEnum::Value(other) => {
                        resource.post_error(
                            Error::InvalidPlacement,
                            format!("{other:?} is not a placement this server handles"),
                        );
                        return;
                    }
                    WEnum::Unknown(v) => {
                        resource
                            .post_error(Error::InvalidPlacement, format!("{v} is not a placement"));
                        return;
                    }
                };
                if let Some(m) = state
                    .window_managers
                    .managers
                    .iter_mut()
                    .find(|m| m.output == output)
                {
                    m.pending.insert(id, target);
                }
            }
            cupertino_window_manager_v1::Request::Commit => {
                if !state.transactions.enqueue_window_change(
                    &client.id(),
                    crate::transaction::QueuedWindowChange::CommitWindowManager(output),
                ) {
                    state.commit_placements(output);
                }
            }
            cupertino_window_manager_v1::Request::ActivateWindow { id } => {
                let window = WindowId(u64::from(id));
                if !state.transactions.enqueue_window_change(
                    &client.id(),
                    crate::transaction::QueuedWindowChange::ActivateWindow(window),
                ) && state.wm.window(window).is_some()
                {
                    state.focus_window_by_input(window);
                }
            }
            cupertino_window_manager_v1::Request::Destroy => {
                state
                    .window_managers
                    .managers
                    .retain(|m| &m.resource != resource);
                state
                    .window_managers
                    .placed
                    .retain(|_, p| p.output != output);
                state.request_redraw();
            }
            _ => {}
        }
    }

    /// The role ends with the object. When a shell component's connection ends while it persists a surface on the
    /// output (or a hold of it lasts there), its placements stay held until a new instance takes them over or the holds
    /// end (D19), whichever of the layer surface and this object ends first. A role the client let go of (`destroy`)
    /// took its placements already.
    fn destroyed(
        state: &mut Self,
        client: ClientId,
        resource: &CupertinoWindowManagerV1,
        data: &WindowManagerData,
    ) {
        let Some(output) = data.output else { return };
        let live = state
            .window_managers
            .managers
            .iter()
            .any(|m| &m.resource == resource);
        let keep_for_hold = live
            && data.component.is_some_and(|c| {
                !state.hosts.component_gave_up(c.name)
                    && state.hosts.component_persists_on(c.name, &client, output)
            });
        state
            .window_managers
            .managers
            .retain(|m| &m.resource != resource);
        if keep_for_hold {
            state.hosts.hold_wm_placements(output);
        } else if live {
            state
                .window_managers
                .placed
                .retain(|_, p| p.output != output);
        }
        state.request_redraw();
    }
}
