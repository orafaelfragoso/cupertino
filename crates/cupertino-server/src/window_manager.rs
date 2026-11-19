//! Server side of the window manager process (`cupertino_shell_v1.get_window_manager`,
//! `cupertino_window_manager_v1`): the process owns the stage policy (which group is on the stage,
//! the strip of recent groups) and commits where every arranged window rests; the server keeps the
//! frame clock. It animates each window from where it is drawn to its committed placement on three
//! tracks with the stage model's springs (frame, alpha and corner radius), draws strip
//! windows as their live content scaled into the thumbnail rectangle (the overview's path, no
//! capture), keeps strip and hidden windows out of hit testing and reports presses on strip
//! thumbnails. A window manager that goes away (the process
//! died) takes its placements with it: every window is drawn and hit at its own frame again at once.

use std::collections::{BTreeMap, HashMap};

use cupertino_animation::{AnimatedValue, Curve};
use cupertino_protocols::server::cupertino_shell_v1::{CupertinoShellV1, Error as ShellError};
use cupertino_protocols::server::cupertino_window_manager_v1::{
    self, CupertinoWindowManagerV1, Error, Placement as WirePlacement, WindowState,
};
use cupertino_wm::stage::StageConfig;
use cupertino_wm::{OutputId, Rect, WindowId};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource, WEnum,
};
use smithay::utils::{Logical, Point};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::authorization::{authorized, refuse, require, role, Permit, ShellRole};
use crate::menu_bar::cut_text;
use crate::state::{Cupertino, WindowEntry};

/// Where a placed window is headed. A window on the stage has no entry once its animation ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// Back to its own frame.
    Stage,
    Strip(Rect),
    Hidden,
}

struct Placed {
    output: OutputId,
    target: Target,
    /// Frame track: x, y, width and height of the rectangle the window is drawn at.
    frame: AnimatedValue<[f64; 4]>,
    /// Alpha track: 0 for a hidden window, 1 on the stage and in the strip.
    alpha: AnimatedValue<f64>,
    /// Corner track: 0 at the window's own corner radius, 1 at the strip's.
    rounding: AnimatedValue<f64>,
    /// Drawn with the strip, below every stage window: towards the strip, or fading out of it.
    strip_layer: bool,
}

impl Placed {
    fn is_animating(&self, now: f64) -> bool {
        self.frame.is_animating(now)
            || self.alpha.is_animating(now)
            || self.rounding.is_animating(now)
    }
}

/// How a window is drawn at a moment, given its own (natural) rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// No placement: drawn and hit as every window without a window manager.
    Natural,
    Hidden,
    /// Scaled into `rect` at `alpha` (times its own opacity), its corners `rounding` of the way from
    /// the window's radius to the strip's ([`WindowManagers::corner_radius`]); `strip` while it is,
    /// becomes or fades out of a strip thumbnail (drawn below the stage windows and not hit).
    Placed {
        rect: Rect,
        strip: bool,
        alpha: f64,
        rounding: f64,
    },
}

/// Springs and strip radius of the placement tracks.
#[derive(Clone, Copy, Debug)]
struct Motion {
    frame: Curve,
    alpha: Curve,
    corner: Curve,
    strip_corner_radius: f64,
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
}

/// The window managers (at most one per output) and the placements they committed.
pub struct WindowManagers {
    managers: Vec<Manager>,
    placed: HashMap<WindowId, Placed>,
    motion: Motion,
}

impl WindowManagers {
    /// A placement change moves the frame with `stage.swap_curve`, the alpha with
    /// `stage.alpha_curve` and the corner radius towards `stage.strip_corner_radius` with
    /// `stage.corner_curve`.
    pub fn new(stage: &StageConfig) -> Self {
        let motion = Motion {
            frame: stage.swap_curve,
            alpha: stage.alpha_curve,
            corner: stage.corner_curve,
            strip_corner_radius: stage.strip_corner_radius,
        };
        Self {
            managers: Vec::new(),
            placed: HashMap::new(),
            motion,
        }
    }

    /// How window `id` is drawn at `now`. A hidden window fades out where it was drawn and is not
    /// drawn once the fade ends.
    pub fn look(&self, id: WindowId, now: f64) -> Look {
        let Some(p) = self.placed.get(&id) else {
            return Look::Natural;
        };
        match p.target {
            Target::Hidden if !p.alpha.is_animating(now) => return Look::Hidden,
            Target::Stage if !p.is_animating(now) => return Look::Natural,
            Target::Hidden | Target::Strip(_) | Target::Stage => {}
        }
        let rect = Rect::from_array(p.frame.value(now));
        Look::Placed {
            rect,
            strip: p.strip_layer,
            alpha: p.alpha.value(now),
            rounding: p.rounding.value(now),
        }
    }

    /// Visual corner radius, in points, of a placed window whose own radius is `window` at
    /// `rounding` ([`Look::Placed`]).
    pub fn corner_radius(&self, window: f64, rounding: f64) -> f64 {
        window + (self.motion.strip_corner_radius - window) * rounding
    }

    /// Whether window `id` is headed for the strip (a press on it is a strip press).
    fn in_strip(&self, id: WindowId) -> bool {
        self.placed
            .get(&id)
            .is_some_and(|p| matches!(p.target, Target::Strip(_)))
    }

    /// Whether the pointer passes through window `id`: strip thumbnails and hidden windows take no
    /// input of their clients.
    pub fn passes_input(&self, id: WindowId) -> bool {
        self.placed
            .get(&id)
            .is_some_and(|p| p.target != Target::Stage)
    }

    /// Whether the stage of `output` is on as far as the server can tell: a window there is placed
    /// in the strip or hidden. A stage holding only its centre group places nothing off the stage
    /// and is not seen (the protocol carries no stage state).
    pub fn stage_active(&self, output: OutputId) -> bool {
        self.placed
            .values()
            .any(|p| p.output == output && p.target != Target::Stage)
    }

    pub fn wants_tick(&self, now: f64) -> bool {
        self.placed.values().any(|p| p.is_animating(now))
    }

    /// Keep window `id`, while it heads back to the stage, headed for its own frame `natural`: when
    /// the frame changed since the commit (a client that resized during the swap) the frame track
    /// turns towards the new one, keeping its value and velocity.
    fn follow_natural(&mut self, id: WindowId, natural: Rect, now: f64) {
        let frame = self.motion.frame;
        let Some(p) = self.placed.get_mut(&id) else {
            return;
        };
        if p.target == Target::Stage
            && p.is_animating(now)
            && p.frame.target() != natural.to_array()
        {
            p.frame.animate_to(natural.to_array(), frame, now);
        }
    }

    /// Drop the entries of windows that arrived back on the stage.
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

    /// Move window `id` towards `target`, starting from where it is drawn now: the frame, alpha
    /// and corner tracks each keep their value and velocity and spring to the target's; the frame
    /// heads for the window's own rectangle `natural`, which [`Cupertino::tick_window_managers`]
    /// keeps following. A hidden window stops where it is and fades out; it comes back in its new
    /// place, fading in with the corners of that place. False when it was already headed there.
    fn retarget(
        &mut self,
        id: WindowId,
        output: OutputId,
        target: Target,
        natural: Rect,
        now: f64,
    ) -> bool {
        let current = self.look(id, now);
        let motion = self.motion;
        let entry = self.placed.entry(id).or_insert_with(|| Placed {
            output,
            target: Target::Stage,
            frame: AnimatedValue::new(natural.to_array()),
            alpha: AnimatedValue::new(1.0),
            rounding: AnimatedValue::new(0.0),
            strip_layer: false,
        });
        if entry.target == target {
            return false;
        }
        let rounding = match target {
            Target::Stage => 0.0,
            Target::Strip(_) => 1.0,
            Target::Hidden => entry.rounding.target(),
        };
        match current {
            Look::Natural => entry.frame.set_immediately(natural.to_array()),
            // a hidden window appears in its slot instead of flying in from where it was hidden
            Look::Hidden => entry.frame.set_immediately(
                if let Target::Strip(rect) = target {
                    rect
                } else {
                    natural
                }
                .to_array(),
            ),
            Look::Placed { .. } => {}
        }
        match target {
            Target::Strip(rect) => entry.frame.animate_to(rect.to_array(), motion.frame, now),
            Target::Stage => entry
                .frame
                .animate_to(natural.to_array(), motion.frame, now),
            Target::Hidden => entry.frame.cancel(now),
        }
        if current == Look::Hidden {
            entry.rounding.set_immediately(rounding);
        } else {
            entry.rounding.animate_to(rounding, motion.corner, now);
        }
        entry.strip_layer = match target {
            Target::Strip(_) => true,
            Target::Stage => false,
            Target::Hidden => matches!(current, Look::Placed { strip: true, .. }),
        };
        entry.alpha.animate_to(
            if target == Target::Hidden { 0.0 } else { 1.0 },
            motion.alpha,
            now,
        );
        entry.target = target;
        entry.output = output;
        true
    }
}

/// Per-object data: `None` for an object the server rejected with a protocol error; it stays inert.
pub struct WindowManagerData {
    output: Option<OutputId>,
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
            init.init(id, WindowManagerData { output: None });
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
            init.init(id, WindowManagerData { output: None });
            shell.post_error(ShellError::Role, reason);
            return;
        }
        let out = target.expect("a rejected output returned above");
        let resource = init.init(id, WindowManagerData { output: Some(out) });
        self.window_managers.managers.push(Manager {
            resource,
            output: out,
            sent: None,
            pending: BTreeMap::new(),
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

    /// Advance the placements at `now`: windows heading back to the stage follow their own frame,
    /// and the entries of those that arrived are dropped.
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
                self.window_managers.follow_natural(id, natural, now);
            }
        }
        self.window_managers.tick(now);
    }

    /// `commit`: retarget every window whose pending placement differs from its current one.
    fn commit_placements(&mut self, output: OutputId) {
        let now = self.now();
        let Some(m) = self
            .window_managers
            .managers
            .iter_mut()
            .find(|m| m.output == output)
        else {
            return;
        };
        let pending = std::mem::take(&mut m.pending);
        let arranged = m
            .sent
            .as_ref()
            .map(|f| f.windows.keys().copied().collect::<Vec<u32>>())
            .unwrap_or_default();
        for (id, target) in pending {
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

    /// A left press at `pos` on a strip thumbnail with nothing above it: tell the window manager
    /// of that output. True when the press was consumed.
    pub fn window_manager_press(&mut self, pos: Point<f64, Logical>) -> bool {
        if self.window_managers.placed.is_empty() || self.window_target_under(pos).is_some() {
            return false;
        }
        let Some(out) = self.output_at(pos) else {
            return false;
        };
        let (output, rect) = (out.id, out.rect);
        let rel = pos - Point::from((rect.x, rect.y));
        {
            let map = layer_map_for_output(&out.output);
            if map.layer_under(Layer::Overlay, rel).is_some()
                || map.layer_under(Layer::Top, rel).is_some()
            {
                return false;
            }
        }
        let now = self.now();
        let p = cupertino_wm::Point::new(pos.x, pos.y);
        let hit = crate::render::scene::windows_on_output(
            &self.wm,
            output,
            rect.w,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            now,
        )
        .into_iter()
        .rev()
        .find_map(|(id, _, _)| match self.window_managers.look(id, now) {
            Look::Placed { rect, .. } if self.window_managers.in_strip(id) && rect.contains(p) => {
                Some(id)
            }
            _ => None,
        });
        let Some(id) = hit else { return false };
        let Some(m) = self.window_managers.of_output(output) else {
            return false;
        };
        m.resource.strip_pressed(id.0 as u32);
        m.resource.done();
        true
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
            cupertino_window_manager_v1::Request::SetPlacement {
                id,
                placement,
                x,
                y,
                width,
                height,
            } => {
                let target = match placement {
                    WEnum::Value(WirePlacement::Stage) => Target::Stage,
                    WEnum::Value(WirePlacement::Hidden) => Target::Hidden,
                    WEnum::Value(WirePlacement::Strip) if width > 0 && height > 0 => {
                        Target::Strip(Rect::new(
                            f64::from(x),
                            f64::from(y),
                            f64::from(width),
                            f64::from(height),
                        ))
                    }
                    WEnum::Value(WirePlacement::Strip) => {
                        resource.post_error(
                            Error::InvalidPlacement,
                            format!("strip rectangle {width}x{height} has no area"),
                        );
                        return;
                    }
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
            cupertino_window_manager_v1::Request::Commit => state.commit_placements(output),
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
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The role ends with the object, also when its client dies: every window is back at its own
    /// frame at once.
    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &CupertinoWindowManagerV1,
        data: &WindowManagerData,
    ) {
        let Some(output) = data.output else { return };
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
}
