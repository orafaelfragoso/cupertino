//! Native desktop-control grants, trusted session consent and input routing. Version 1 remains discovery-only.
//!
//! From version 4 each run names the app it comes from ([`identity`]), and what the person remembered for that app in
//! `desktop-control.conf` decides a request at once: a service turned off refuses it, all services turned on start it,
//! and only an app with an undecided service is asked on the consent panel, whose answer is remembered. A change to the
//! file that turns off a service a run uses revokes the run. Physical input never pauses a run: the person and the
//! controller use the desktop together, each source's held keys and buttons tracked apart ([`InputOwners`]).
//!
//! From version 5 one controller connection holds a session per agent it serves, each run with its own epoch, held
//! input and capture sources. Runs that need the person's answer are asked one at a time, in the order they asked.
mod identity;
mod input;
mod semantics;
use crate::authorization::{authorized, require, ShellRole};
use crate::state::Cupertino;
use cupertino_control_model::permission::{self, Services, Verdict};
use cupertino_control_model::{
    Capabilities, Epoch, Grant, Handle, InputOwners, Operation, Phase, Revocation, Scope, Session,
    MAX_RUNS_PER_CONTROLLER,
};
use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
use cupertino_protocols::server::control::cupertino_control_authority_v1::{
    self as authority, CupertinoControlAuthorityV1,
};
use cupertino_protocols::server::control::cupertino_control_session_v1::{
    self as session, CupertinoControlSessionV1,
};
use cupertino_protocols::server::control::cupertino_control_v1::{
    self as manager, CupertinoControlV1,
};
pub(crate) use semantics::ActionGrab;
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::Transform;
use smithay::wayland::compositor::with_states;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Current version; version 1 keeps discovery only, version 2 retains its input/capture ABI and version 3 its
/// semantic ABI, asked on every run.
const VERSION: u32 = 5;
/// The version from which one connection holds several sessions and their runs are independent.
const CONCURRENT_RUNS: u32 = 5;
/// Version of the consent authority; version 2 shows the app asking and remembers the answer.
const AUTHORITY_VERSION: u32 = 2;
/// The `allow` of an authority answer that refuses the run and remembers deny (version 2).
const ANSWER_DENY: u32 = 2;
/// Sessions a private connection may hold, across manager bindings, when it creates one through a manager older than
/// [`CONCURRENT_RUNS`]; a newer manager allows [`MAX_RUNS_PER_CONTROLLER`].
const SESSIONS_BEFORE_CONCURRENT_RUNS: usize = 1;
/// Number of completed request outcomes retained to suppress retries.
const REPLY_HISTORY: usize = 64;
/// Fresh physical input must have reached the consent surface within this interval.
const CONSENT_FRESHNESS: Duration = Duration::from_secs(3);
/// Bound serial provenance storage while the person interacts with consent.
const USER_SERIALS: usize = 32;
/// Operations implemented by version 3, adding semantic actions to version 2 input/capture.
const AVAILABLE: u32 = 31;

/// Why runs are suspended; each resumes by itself once its reason is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PauseReason {
    /// A protected session panel took the keyboard.
    ProtectedFocus,
    /// The authentication prompt is shown.
    AuthenticationPrompt,
}

/// The app a run comes from, as far as the controller named it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    /// Not named: a run of version 3 or older, asked every time and never remembered.
    Unnamed,
    /// Named and found.
    Named(Requester),
    /// Named, but the app could not be found: the run is refused.
    Unidentified,
}

/// An app a run comes from and how the consent panel names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Requester {
    pub(crate) app: App,
    /// The desktop entry's name, or the executable's file name.
    pub(crate) name: String,
}

pub struct Control {
    _global: GlobalId,
    _authority: GlobalId,
    next_epoch: u64,
    runs: Vec<Run>,
    authority: Option<CupertinoControlAuthorityV1>,
    consent_surface: Option<WlSurface>,
    user_serials: VecDeque<(u32, WlSurface, Instant)>,
    pub(crate) owners: InputOwners,
    pub(crate) dispatching: bool,
    pub(crate) physical_input: bool,
    injected_clients: Vec<ClientId>,
    semantics: semantics::SemanticState,
    /// What the person remembered for each app, as last read.
    store: DesktopControlConf,
    /// Where it is read and written; `None` (tests, no settings directory) remembers nothing.
    store_path: Option<PathBuf>,
}
pub struct SessionData {
    epoch: Option<Epoch>,
}
struct Protected(AtomicBool);
pub fn protected_surface(surface: &WlSurface) -> bool {
    with_states(surface, |s| {
        s.data_map
            .get::<Protected>()
            .is_some_and(|p| p.0.load(Ordering::Relaxed))
    })
}
#[derive(Clone, Debug, PartialEq)]
struct Geometry {
    rect: (f64, f64, f64, f64),
    scale: f64,
    transform: Transform,
}
struct Run {
    resource: CupertinoControlSessionV1,
    model: Session,
    output: Option<WeakOutput>,
    grant: Option<Grant>,
    capabilities: u32,
    geometry: Option<Geometry>,
    /// Advances when input coordinates computed earlier could point somewhere else: the output's geometry changed or,
    /// while the run observes, where its windows lie ([`semantics::Placement`]).
    revision: u32,
    /// Where the run's observed windows lie as last compared, in stacking order; none before the run first observes.
    placements: Option<Vec<semantics::Placement>>,
    last_serial: u32,
    consent_since: Instant,
    replies: VecDeque<(u32, Result<(), session::Refusal>)>,
    identity: Identity,
    /// Services the run holds from remembered answers: turning one off revokes the run.
    remembered: Services,
    /// Services the person is asked for while the run awaits consent.
    asked: Services,
    /// Whether the consent authority was sent this run's request; a run awaiting consent waits its turn until then.
    consent_shown: bool,
    /// Whether the next output capture source of the run's connection belongs to this run (`select_capture`).
    capture_selected: bool,
    /// Why the run is suspended, while it is.
    paused: Option<PauseReason>,
}
impl Control {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<Cupertino, CupertinoControlV1, ()>(VERSION, ()),
            _authority: display
                .create_global::<Cupertino, CupertinoControlAuthorityV1, ()>(AUTHORITY_VERSION, ()),
            next_epoch: 1,
            runs: Vec::new(),
            authority: None,
            consent_surface: None,
            user_serials: VecDeque::new(),
            owners: InputOwners::default(),
            dispatching: false,
            physical_input: false,
            injected_clients: Vec::new(),
            semantics: semantics::SemanticState::new(display),
            store: DesktopControlConf::default(),
            store_path: None,
        }
    }
    fn allocate_epoch(&mut self) -> Option<Epoch> {
        let epoch = Epoch::new(self.next_epoch)?;
        self.next_epoch = self.next_epoch.checked_add(1).unwrap_or(0);
        Some(epoch)
    }
    fn authority_alive(&self) -> bool {
        self.authority.as_ref().is_some_and(|a| {
            a.is_alive()
                && a.client()
                    .is_some_and(|c| authorized(&c, ShellRole::SessionLock))
        })
    }
}
fn phase(phase: Phase) -> session::Phase {
    match phase {
        Phase::Inactive => session::Phase::Inactive,
        Phase::AwaitingConsent => session::Phase::AwaitingConsent,
        Phase::Active => session::Phase::Active,
        Phase::Suspended => session::Phase::Suspended,
        Phase::Revoked => session::Phase::Revoked,
    }
}
fn capabilities(bits: u32) -> Capabilities {
    Capabilities::of(
        [
            (1, Operation::Observe),
            (2, Operation::Capture),
            (4, Operation::Pointer),
            (8, Operation::Keyboard),
            (16, Operation::Semantic),
        ]
        .into_iter()
        .filter_map(|(bit, op)| (bits & bit != 0).then_some(op)),
    )
}
/// The remembered file's service for a model service.
fn stored(service: permission::Service) -> Service {
    match service {
        permission::Service::Control => Service::Control,
        permission::Service::Capture => Service::Capture,
    }
}
/// What `store` remembers of `app` for `service`, as the model reads it.
fn remembered(
    store: &DesktopControlConf,
    app: &App,
    service: permission::Service,
) -> Option<permission::Decision> {
    store
        .decision(stored(service), app)
        .map(|decision| match decision {
            Decision::Allow => permission::Decision::Allow,
            Decision::Deny => permission::Decision::Deny,
        })
}
fn reply(resource: &CupertinoControlSessionV1, serial: u32, result: Result<(), session::Refusal>) {
    match result {
        Ok(()) => resource.completed(serial),
        Err(reason) => resource.refused(serial, reason),
    }
}
impl Cupertino {
    fn control_output_geometry(&self, output: &Output) -> Option<Geometry> {
        let o = self.outputs.iter().find(|o| o.output == *output)?;
        Some(Geometry {
            rect: (o.rect.x, o.rect.y, o.rect.w, o.rect.h),
            scale: o.scale,
            transform: output.current_transform(),
        })
    }
    fn control_changed(&mut self, index: usize, serial: u32) {
        self.control_status(index, serial);
        self.control_ask_next();
        self.refresh_capture_indicator();
    }
    /// Ask the person for the run that has waited longest, unless the consent panel already shows a request: the panel
    /// asks one request at a time.
    fn control_ask_next(&mut self) {
        let runs = &self.control.runs;
        let awaiting = |r: &&Run| r.model.phase() == Phase::AwaitingConsent;
        if runs.iter().filter(awaiting).any(|r| r.consent_shown) {
            return;
        }
        let Some(index) = runs
            .iter()
            .enumerate()
            .filter(|(_, r)| awaiting(r))
            .min_by_key(|(_, r)| r.consent_since)
            .map(|(i, _)| i)
        else {
            return;
        };
        let run = &mut self.control.runs[index];
        run.consent_shown = true;
        // the person's answer must come after the panel asked, not after the run started waiting
        run.consent_since = Instant::now();
        self.control_ask(index);
    }
    fn control_status(&self, index: usize, serial: u32) {
        let run = &self.control.runs[index];
        if run.resource.version() >= 2 {
            run.resource.status(serial, phase(run.model.phase()));
            if let Some(a) = self.control.authority.as_ref().filter(|a| a.is_alive()) {
                let epoch = run.model.epoch().get();
                a.changed(
                    (epoch >> 32) as u32,
                    epoch as u32,
                    phase(run.model.phase()) as u32,
                );
            }
        } else {
            run.resource.status(serial, session::Phase::Inactive);
        }
    }
    /// Advance run `index`'s geometry revision, so input computed from what it saw before is refused, and send it the
    /// new revision; a run whose revisions are exhausted is stopped.
    fn control_advance(&mut self, index: usize) {
        if let Some(revision) = self.control.runs[index].revision.checked_add(1) {
            self.control.runs[index].revision = revision;
            self.control_geometry(index);
            self.control_changed(index, 0);
        } else {
            self.control_revoke(index, Revocation::Stopped);
        }
    }
    fn control_geometry(&self, index: usize) {
        let r = &self.control.runs[index];
        if let (Some(g), Some(o)) = (&r.geometry, r.output.as_ref().and_then(WeakOutput::upgrade)) {
            r.resource.geometry(
                r.revision,
                g.rect.0.round() as i32,
                g.rect.1.round() as i32,
                g.rect.2.round() as i32,
                g.rect.3.round() as i32,
                g.scale,
                o.name(),
            );
        }
    }
    pub fn revoke_controls(&mut self, reason: Revocation) {
        for i in 0..self.control.runs.len() {
            self.control_revoke(i, reason);
        }
    }
    fn control_revoke(&mut self, index: usize, reason: Revocation) {
        if self.control.runs[index].model.phase() == Phase::Revoked {
            return;
        }
        let epoch = self.control.runs[index].model.epoch();
        self.control.runs[index].model.revoke(reason);
        self.control_cleanup_input(epoch);
        self.control_changed(index, 0);
        self.stop_captures();
        self.refresh_semantics();
    }
    /// Suspend the active runs while a protected surface or the authentication prompt is up; they resume by themselves
    /// when it is gone ([`Self::refresh_control`]).
    pub(crate) fn suspend_controls(&mut self, reason: PauseReason) {
        let mut changed = false;
        for i in 0..self.control.runs.len() {
            let epoch = self.control.runs[i].model.epoch();
            if self.control.runs[i].model.phase() == Phase::Active {
                tracing::info!(epoch = epoch.get(), ?reason, "desktop control paused");
                changed = true;
                let _ = self.control.runs[i].model.suspend(epoch);
                self.control.runs[i].paused = Some(reason);
                self.control_cleanup_input(epoch);
                self.control_changed(i, 0);
            }
        }
        if changed {
            self.stop_captures();
            self.refresh_semantics();
        }
    }
    /// Called each loop and before requests/copies: a held descriptor never outlives the actual processes or grant.
    pub(crate) fn refresh_control(&mut self) {
        self.control.injected_clients.retain(|id| {
            self.display_handle
                .backend_handle()
                .get_client_data(id.clone())
                .is_ok()
        });
        for i in 0..self.control.runs.len() {
            let r = &self.control.runs[i];
            if r.model.phase() == Phase::Revoked {
                continue;
            }
            let reason = if !r
                .resource
                .client()
                .is_some_and(|c| authorized(&c, ShellRole::DesktopControl))
            {
                Some(Revocation::ControllerDisconnected)
            } else if self.session_lock.is_active() || self.login_screen {
                Some(Revocation::Locked)
            } else if r.model.phase() == Phase::AwaitingConsent && !self.control.authority_alive() {
                // a run that is already active does not depend on the panel that could have asked for it
                Some(Revocation::AuthorityDisconnected)
            } else if r.output.is_some()
                && r.output
                    .as_ref()
                    .and_then(WeakOutput::upgrade)
                    .and_then(|o| self.control_output_geometry(&o))
                    .is_none()
            {
                Some(Revocation::Stopped)
            } else {
                None
            };
            if let Some(reason) = reason {
                self.control_revoke(i, reason);
                continue;
            }
            if self.control.runs[i].model.phase() == Phase::Suspended
                && self.control.runs[i].paused.is_some()
                && !self.control_blocked()
            {
                self.control_resume(i);
            }
            if self.control.runs[i].model.phase() == Phase::Active {
                if self.prompt_shown() {
                    self.suspend_controls(PauseReason::AuthenticationPrompt);
                    break;
                }
                let output = self.control.runs[i]
                    .output
                    .as_ref()
                    .and_then(WeakOutput::upgrade);
                if let Some(geometry) = output.and_then(|o| self.control_output_geometry(&o)) {
                    if self.control.runs[i].geometry.as_ref() != Some(&geometry) {
                        self.control.runs[i].geometry = Some(geometry);
                        self.control_advance(i);
                    }
                }
            }
        }
        self.refresh_semantics();
    }
    /// Whether something keeps a suspended run from resuming: the authentication prompt, or a protected session panel
    /// holding the keyboard.
    fn control_blocked(&self) -> bool {
        self.prompt_shown()
            || self
                .seat
                .get_keyboard()
                .and_then(|k| k.current_focus())
                .is_some_and(|s| self.control_sensitive_focus(&s))
    }
    /// Resume suspended run `index`: its grant still holds, so nobody is asked again.
    fn control_resume(&mut self, index: usize) {
        let run = &mut self.control.runs[index];
        let epoch = run.model.epoch();
        if run.model.resume(epoch).is_err() {
            return;
        }
        tracing::info!(epoch = epoch.get(), reason = ?run.paused, "desktop control resumed");
        run.paused = None;
        self.control_geometry(index);
        self.control_changed(index, 0);
    }
    /// Other session panels pause control; the consent panel itself may still be committing its old focus after approval.
    pub(crate) fn control_sensitive_focus(&self, surface: &WlSurface) -> bool {
        crate::authorization::is_prompt(surface)
            || (self.control.consent_surface.as_ref() != Some(surface)
                && surface
                    .client()
                    .is_some_and(|c| authorized(&c, ShellRole::SessionLock)))
    }
    /// Only backend-originated serials delivered to the registered authority surface can approve or resume.
    pub(crate) fn control_user_input(&mut self, serial: u32, surface: Option<WlSurface>) {
        if !self.control.physical_input || self.control.dispatching {
            return;
        }
        if let Some(client) = surface.as_ref().and_then(Resource::client) {
            self.control
                .injected_clients
                .retain(|id| *id != client.id());
        }
        if let Some(surface) = surface.filter(|s| self.control.consent_surface.as_ref() == Some(s))
        {
            if self.control.user_serials.len() == USER_SERIALS {
                self.control.user_serials.pop_front();
            }
            self.control
                .user_serials
                .push_back((serial, surface, Instant::now()));
        }
    }
    pub(crate) fn control_session_action_allowed(&self, client: &Client) -> bool {
        !self.control.injected_clients.contains(&client.id())
    }
    fn control_begin(
        &mut self,
        resource: &CupertinoControlSessionV1,
        serial: u32,
        epoch: u64,
    ) -> Option<usize> {
        self.refresh_control();
        let i = self
            .control
            .runs
            .iter()
            .position(|r| r.resource == *resource)?;
        let r = &mut self.control.runs[i];
        if serial == 0 {
            resource.post_error(session::Error::InvalidSerial, "serial zero is reserved");
            return None;
        }
        if r.model.epoch().get() != epoch {
            resource.refused(serial, session::Refusal::StaleEpoch);
            return None;
        }
        if let Some((_, result)) = r.replies.iter().find(|(s, _)| *s == serial) {
            reply(resource, serial, *result);
            return None;
        }
        if serial <= r.last_serial {
            resource.refused(serial, session::Refusal::UnknownOutcome);
            return None;
        }
        r.last_serial = serial;
        Some(i)
    }
    fn control_finish(&mut self, index: usize, serial: u32, result: Result<(), session::Refusal>) {
        let r = &mut self.control.runs[index];
        if r.replies.len() == REPLY_HISTORY {
            r.replies.pop_front();
        }
        r.replies.push_back((serial, result));
        reply(&r.resource, serial, result);
    }
    fn control_start(
        &mut self,
        index: usize,
        output: Output,
        bits: u32,
    ) -> Result<(), session::Refusal> {
        if self.session_lock.is_active() || self.login_screen || self.prompt_shown() {
            return Err(session::Refusal::Blocked);
        }
        if bits == 0
            || bits & !AVAILABLE != 0
            || (bits & 16 != 0 && bits & 1 == 0)
            || (self.control.runs[index].resource.version() < 3 && bits & 16 != 0)
        {
            return Err(session::Refusal::Unsupported);
        }
        let requester = match &self.control.runs[index].identity {
            Identity::Named(requester) => Some(requester.clone()),
            Identity::Unidentified => return Err(session::Refusal::Unidentified),
            Identity::Unnamed if self.control.runs[index].resource.version() >= 4 => {
                return Err(session::Refusal::Unidentified)
            }
            Identity::Unnamed => None,
        };
        let geometry = self
            .control_output_geometry(&output)
            .ok_or(session::Refusal::OutsideScope)?;
        let epoch = self.control.runs[index].model.epoch();
        let grant = Grant {
            scope: Scope::Output(Handle {
                id: epoch.get(),
                generation: epoch.get(),
            }),
            capabilities: capabilities(bits),
        };
        let needed = Services::needed_by(grant.capabilities);
        let verdict = match &requester {
            Some(r) => permission::decide(needed, |s| remembered(&self.control.store, &r.app, s)),
            None => Verdict::Ask(needed),
        };
        let asked = match verdict {
            Verdict::Denied => {
                tracing::info!(epoch = epoch.get(), app = ?requester.map(|r| r.app), "desktop control refused: the person turned it off for this app");
                return Err(session::Refusal::Denied);
            }
            Verdict::Allowed => Services::NONE,
            Verdict::Ask(asked) => asked,
        };
        // after the person's refusal, which neither a run that ended nor another run hides: a run that ended stays
        // ended, and a fresh session has a fresh run, asked for in its own right
        if self.control.runs[index].model.phase() == Phase::Revoked {
            return Err(session::Refusal::StaleEpoch);
        }
        // a run of an older version stands alone; newer runs are independent of each other
        let concurrent = |r: &Run| r.resource.version() >= CONCURRENT_RUNS;
        if self.control.runs.iter().enumerate().any(|(i, r)| {
            i != index
                && (!concurrent(r) || !concurrent(&self.control.runs[index]))
                && matches!(
                    r.model.phase(),
                    Phase::Active | Phase::AwaitingConsent | Phase::Suspended
                )
        }) {
            return Err(session::Refusal::Busy);
        }
        if !asked.is_empty() && !self.control.authority_alive() {
            return Err(session::Refusal::NoAuthority);
        }
        let run = &mut self.control.runs[index];
        run.model
            .request(epoch, grant)
            .map_err(|_| session::Refusal::Busy)?;
        run.consent_since = Instant::now();
        run.output = Some(output.downgrade());
        run.grant = Some(grant);
        run.capabilities = bits;
        run.geometry = Some(geometry);
        run.revision = 1;
        run.placements = None;
        run.asked = asked;
        run.consent_shown = false;
        // what is needed and not asked is allowed by the app's remembered answers
        run.remembered = needed.iter().filter(|s| !asked.contains(*s)).collect();
        if asked.is_empty() {
            run.model
                .approve(epoch, grant)
                .map_err(|_| session::Refusal::Busy)?;
            tracing::info!(epoch = epoch.get(), app = ?requester.map(|r| r.app), "desktop control allowed by the remembered answer");
            self.control_geometry(index);
        }
        // a run that must ask is asked now, or once the requests before it are answered ([`Self::control_ask_next`])
        self.control_changed(index, 0);
        Ok(())
    }
    /// Ask the person through the authority: the app first (version 2), then the request.
    fn control_ask(&self, index: usize) {
        let r = &self.control.runs[index];
        let Some(a) = &self.control.authority else {
            return;
        };
        let epoch = r.model.epoch().get();
        let (hi, lo) = ((epoch >> 32) as u32, epoch as u32);
        if let (Identity::Named(requester), true) = (&r.identity, a.version() >= 2) {
            let (kind, identity) = match &requester.app {
                App::Desktop(id) => (authority::AppKind::Desktop, id.clone()),
                App::Executable(path) => (
                    authority::AppKind::Executable,
                    path.to_string_lossy().into_owned(),
                ),
            };
            a.app(
                hi,
                lo,
                kind,
                identity,
                requester.name.clone(),
                r.asked.bits(),
                u32::from(self.control_rememberable(&requester.app)),
            );
        }
        let name = r
            .output
            .as_ref()
            .and_then(WeakOutput::upgrade)
            .map(|o| o.name())
            .unwrap_or_default();
        a.requested(hi, lo, r.capabilities, name, 0);
    }
    /// Whether an answer for `app` can be written to the remembered file.
    fn control_rememberable(&self, app: &App) -> bool {
        self.control.store_path.is_some() && app.is_rememberable()
    }
    /// Remember `decision` for `app` and each of `services`; whether it was written. When it was not, the answer holds
    /// for the run that asked only, which is logged.
    fn control_remember(&mut self, app: &App, services: Services, decision: Decision) -> bool {
        let Some(path) = self
            .control
            .store_path
            .clone()
            .filter(|_| app.is_rememberable())
        else {
            tracing::info!(
                ?app,
                ?decision,
                "desktop control: the answer applies to this run only, it cannot be remembered"
            );
            return false;
        };
        let written = DesktopControlConf::update(&path, |conf| {
            for service in services.iter() {
                conf.set(stored(service), app, decision)
                    .map_err(std::io::Error::other)?;
            }
            Ok(conf.clone())
        });
        match written {
            Ok(conf) => {
                tracing::info!(?app, ?decision, "desktop control: the answer is remembered");
                self.control.store = conf;
                true
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, ?app, ?decision, "desktop control: the answer cannot be remembered, it applies to this run only");
                false
            }
        }
    }
    /// Read the remembered answers from `path` and follow them: a run that lost a service it holds from them is revoked,
    /// and a run awaiting an answer for a service now turned off is refused. A file that is refused allows nothing.
    pub fn reload_desktop_control(&mut self, path: &Path) {
        self.control.store_path = Some(path.to_path_buf());
        self.control.store = match DesktopControlConf::load(path) {
            Ok((conf, problems)) => {
                for problem in problems {
                    tracing::warn!(path = %path.display(), line = problem.line, problem = problem.message, "desktop control: ignoring a line");
                }
                conf
            }
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "desktop control: the file is refused, no app is allowed");
                DesktopControlConf::default()
            }
        };
        for i in 0..self.control.runs.len() {
            let run = &self.control.runs[i];
            let Identity::Named(requester) = &run.identity else {
                continue;
            };
            let decision = |s| remembered(&self.control.store, &requester.app, s);
            let (epoch, phase) = (run.model.epoch(), run.model.phase());
            let withdrawn = matches!(phase, Phase::Active | Phase::Suspended)
                && permission::lost(run.remembered, decision);
            let refused = phase == Phase::AwaitingConsent
                && run
                    .asked
                    .iter()
                    .any(|s| decision(s) == Some(permission::Decision::Deny));
            if withdrawn {
                tracing::info!(epoch = epoch.get(), app = ?requester.app, "desktop control revoked: the person turned it off for this app");
                self.control_revoke(i, Revocation::Withdrawn);
            } else if refused {
                let _ = self.control.runs[i].model.deny(epoch);
                self.control_changed(i, 0);
            }
        }
    }
    /// Whether `serial` is fresh physical input the person gave the consent surface of `client` after run `index` asked;
    /// using it spends every serial held.
    fn control_fresh_consent(&mut self, client: &Client, index: usize, serial: u32) -> bool {
        let since = self.control.runs[index].consent_since;
        let fresh = self.control.user_serials.iter().any(|(s, surface, time)| {
            *s == serial
                && time.elapsed() <= CONSENT_FRESHNESS
                && *time >= since
                && self.control.consent_surface.as_ref() == Some(surface)
                && surface.client().as_ref() == Some(client)
        });
        if fresh {
            self.control.user_serials.clear();
        }
        fresh
    }
    /// Remember the person's answer for the services run `index` asked; what is written it then holds from the file.
    fn control_answered(&mut self, index: usize, decision: Decision) {
        let run = &self.control.runs[index];
        let Identity::Named(requester) = &run.identity else {
            return;
        };
        let (app, asked) = (requester.app.clone(), run.asked);
        if self.control_remember(&app, asked, decision) && decision == Decision::Allow {
            let run = &mut self.control.runs[index];
            run.remembered = run.remembered.union(asked);
        }
    }
    fn control_check_input(
        &self,
        index: usize,
        revision: u32,
        operation: Operation,
    ) -> Result<(), session::Refusal> {
        let r = &self.control.runs[index];
        let grant = r.grant.ok_or(session::Refusal::NotGranted)?;
        r.model
            .authorize(r.model.epoch(), grant.scope, operation)
            .map_err(|_| session::Refusal::NotGranted)?;
        if r.revision != revision {
            return Err(session::Refusal::StaleGeometry);
        }
        if self.prompt_shown() || self.session_lock.is_active() {
            return Err(session::Refusal::Blocked);
        }
        Ok(())
    }
    pub(crate) fn control_capture_allowed(
        &self,
        client: &Client,
        output: &Output,
        epoch: u64,
    ) -> bool {
        !self.session_lock.is_active()
            && !self.prompt_shown()
            && authorized(client, ShellRole::DesktopControl)
            && self.control.runs.iter().any(|r| {
                r.resource.client().as_ref() == Some(client)
                    && r.model.epoch().get() == epoch
                    && r.model.phase() == Phase::Active
                    && capabilities(r.capabilities).allows(Operation::Capture)
                    && r.output.as_ref().and_then(WeakOutput::upgrade).as_ref() == Some(output)
                    && self.control_output_geometry(output).is_some()
            })
    }
    /// Whether an active run may take images of the screen: the menu bar shows the capture indicator for it, as for
    /// any other capture, for as long as the run can capture.
    pub fn control_capturing(&self) -> bool {
        self.control.runs.iter().any(|r| {
            r.model.phase() == Phase::Active
                && capabilities(r.capabilities).allows(Operation::Capture)
        })
    }
    /// The run a new output capture source of `client` belongs to: the run that selected it (spending the selection),
    /// else the first of the client's runs that may capture `output`. `None` makes a source that never captures.
    pub(crate) fn control_capture_epoch(
        &mut self,
        client: &Client,
        output: &Output,
    ) -> Option<u64> {
        let selected = self
            .control
            .runs
            .iter_mut()
            .find(|r| r.capture_selected && r.resource.client().as_ref() == Some(client));
        if let Some(run) = selected {
            run.capture_selected = false;
            let epoch = run.model.epoch().get();
            return self
                .control_capture_allowed(client, output, epoch)
                .then_some(epoch);
        }
        self.control.runs.iter().find_map(|r| {
            let epoch = r.model.epoch().get();
            self.control_capture_allowed(client, output, epoch)
                .then_some(epoch)
        })
    }
}
impl GlobalDispatch<CupertinoControlV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        id: New<CupertinoControlV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let r = init.init(id, ());
        require(client, ShellRole::DesktopControl, &r);
    }
    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::DesktopControl)
    }
}
impl Dispatch<CupertinoControlV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoControlV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::Destroy => {}
            manager::Request::GetSession { id } => {
                if !require(client, ShellRole::DesktopControl, resource) {
                    init.init(id, SessionData { epoch: None });
                    return;
                }
                let allowed = if resource.version() >= CONCURRENT_RUNS {
                    MAX_RUNS_PER_CONTROLLER
                } else {
                    SESSIONS_BEFORE_CONCURRENT_RUNS
                };
                if state
                    .control
                    .runs
                    .iter()
                    .filter(|r| {
                        r.resource.is_alive() && r.resource.client().as_ref() == Some(client)
                    })
                    .count()
                    >= allowed
                {
                    init.init(id, SessionData { epoch: None });
                    resource.post_error(
                        manager::Error::SessionExists,
                        "the connection has as many sessions as allowed",
                    );
                    return;
                }
                let Some(epoch) = state.control.allocate_epoch() else {
                    init.init(id, SessionData { epoch: None });
                    resource.post_error(manager::Error::Exhausted, "run epochs exhausted");
                    return;
                };
                let r = init.init(id, SessionData { epoch: Some(epoch) });
                r.created(
                    (epoch.get() >> 32) as u32,
                    epoch.get() as u32,
                    if r.version() >= 3 {
                        AVAILABLE
                    } else if r.version() == 2 {
                        15
                    } else {
                        0
                    },
                );
                r.status(0, session::Phase::Inactive);
                state.control.runs.push(Run {
                    resource: r,
                    model: Session::new(epoch),
                    output: None,
                    grant: None,
                    capabilities: 0,
                    geometry: None,
                    revision: 0,
                    placements: None,
                    last_serial: 0,
                    consent_since: Instant::now(),
                    replies: VecDeque::new(),
                    identity: Identity::Unnamed,
                    remembered: Services::NONE,
                    asked: Services::NONE,
                    consent_shown: false,
                    capture_selected: false,
                    paused: None,
                });
            }
            _ => unreachable!(),
        }
    }
}
impl Dispatch<CupertinoControlSessionV1, SessionData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoControlSessionV1,
        request: session::Request,
        data: &SessionData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, session::Request::Destroy) {
            return;
        }
        if !require(client, ShellRole::DesktopControl, resource) {
            return;
        }
        if let session::Request::Query {
            serial,
            epoch_hi,
            epoch_lo,
        } = request
        {
            if serial == 0 {
                resource.post_error(session::Error::InvalidSerial, "serial zero is reserved");
                return;
            }
            if data
                .epoch
                .is_none_or(|e| e.get() != ((u64::from(epoch_hi) << 32) | u64::from(epoch_lo)))
            {
                resource.refused(serial, session::Refusal::StaleEpoch);
            } else if let Some(i) = state
                .control
                .runs
                .iter()
                .position(|r| r.resource == *resource)
            {
                state.refresh_control();
                state.control_changed(i, serial);
            }
            return;
        }
        if let session::Request::Identify {
            pid,
            start_hi,
            start_lo,
        } = request
        {
            let Some(i) = state
                .control
                .runs
                .iter()
                .position(|r| r.resource == *resource)
            else {
                return;
            };
            let run = &state.control.runs[i];
            if run.identity != Identity::Unnamed
                || matches!(
                    run.model.phase(),
                    Phase::AwaitingConsent | Phase::Active | Phase::Suspended
                )
            {
                resource.post_error(
                    session::Error::Identity,
                    "identify must come once, before start",
                );
                return;
            }
            if run.model.phase() == Phase::Revoked {
                // the run ended before the controller could name its peer, as when the session locked
                return;
            }
            let start = (u64::from(start_hi) << 32) | u64::from(start_lo);
            let epoch = run.model.epoch().get();
            state.control.runs[i].identity = match state.control_identify(pid, start) {
                Ok(requester) => {
                    tracing::info!(epoch, pid, app = ?requester.app, "desktop control: the run comes from this app");
                    Identity::Named(requester)
                }
                Err(reason) => {
                    tracing::warn!(
                        epoch,
                        pid,
                        ?reason,
                        "desktop control: the app of the run cannot be named, it is refused"
                    );
                    Identity::Unidentified
                }
            };
            return;
        }
        if matches!(request, session::Request::SelectCapture) {
            // the selection is the connection's: the last run to select owns its next source
            for run in &mut state.control.runs {
                if run.resource.client().as_ref() == Some(client) {
                    run.capture_selected = run.resource == *resource;
                }
            }
            return;
        }
        if matches!(request, session::Request::Stop) {
            if let Some(i) = state
                .control
                .runs
                .iter()
                .position(|r| r.resource == *resource)
            {
                state.control_revoke(i, Revocation::Stopped);
            }
            return;
        }
        let (serial, hi, lo) = match &request {
            session::Request::Start {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Motion {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Button {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Key {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Axis {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Observe {
                serial,
                epoch_hi,
                epoch_lo,
            }
            | session::Request::Perform {
                serial,
                epoch_hi,
                epoch_lo,
                ..
            }
            | session::Request::Resume {
                serial,
                epoch_hi,
                epoch_lo,
            } => (*serial, *epoch_hi, *epoch_lo),
            _ => return,
        };
        let Some(i) = state.control_begin(resource, serial, (u64::from(hi) << 32) | u64::from(lo))
        else {
            return;
        };
        let result = match request {
            session::Request::Observe { .. } => state.control_observe(i, serial),
            session::Request::Perform {
                payload, length, ..
            } => match state.control_perform(i, serial, payload, length) {
                Ok(true) => return,
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            },
            session::Request::Start {
                output,
                capabilities,
                ..
            } => {
                let bits = match capabilities {
                    WEnum::Value(v) => v.bits(),
                    WEnum::Unknown(v) => v,
                };
                Output::from_resource(&output)
                    .ok_or(session::Refusal::OutsideScope)
                    .and_then(|o| state.control_start(i, o, bits))
            }
            session::Request::Motion { revision, x, y, .. } => {
                state.control_motion(i, revision, x, y)
            }
            session::Request::Button {
                revision,
                code,
                state: pressed,
                ..
            } => state.control_button(i, revision, code, pressed),
            session::Request::Key {
                revision,
                code,
                state: pressed,
                ..
            } => state.control_key(i, revision, code, pressed),
            session::Request::Axis {
                revision,
                horizontal,
                vertical,
                ..
            } => state.control_axis(i, revision, horizontal, vertical),
            // a suspension whose reason is gone has resumed already ([`Cupertino::refresh_control`])
            session::Request::Resume { .. } => match state.control.runs[i].model.phase() {
                Phase::Active => Ok(()),
                Phase::Suspended if !state.control_blocked() => {
                    state.control_resume(i);
                    Ok(())
                }
                _ => Err(session::Refusal::Blocked),
            },
            _ => unreachable!(),
        };
        state.control_finish(i, serial, result);
    }
    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &CupertinoControlSessionV1,
        _: &SessionData,
    ) {
        if let Some(i) = state
            .control
            .runs
            .iter()
            .position(|r| r.resource == *resource)
        {
            state.control_revoke(i, Revocation::ControllerDisconnected);
            state.control.runs.remove(i);
        }
    }
}
impl GlobalDispatch<CupertinoControlAuthorityV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        id: New<CupertinoControlAuthorityV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let r = init.init(id, ());
        if !require(client, ShellRole::SessionLock, &r) {
            return;
        }
        if state.control.authority_alive() {
            r.post_error(authority::Error::Busy, "a consent authority already exists");
            return;
        }
        state.control.authority = Some(r);
    }
    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::SessionLock)
    }
}
impl Dispatch<CupertinoControlAuthorityV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoControlAuthorityV1,
        request: authority::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, authority::Request::Destroy) {
            return;
        }
        if !require(client, ShellRole::SessionLock, resource) {
            return;
        }
        if state.control.authority.as_ref() != Some(resource) {
            return;
        }
        match request {
            authority::Request::SetSurface { surface } => {
                if surface.client().as_ref() != Some(client) {
                    resource.post_error(
                        authority::Error::Permission,
                        "consent must use the authority's own surface",
                    );
                    return;
                }
                with_states(&surface, |s| {
                    s.data_map
                        .insert_if_missing(|| Protected(AtomicBool::new(true)));
                });
                state.control.consent_surface = Some(surface);
                state.control.user_serials.clear();
            }
            authority::Request::Answer {
                epoch_hi,
                epoch_lo,
                allow,
                serial,
            } => {
                state.refresh_control();
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                let Some(i) = state
                    .control
                    .runs
                    .iter()
                    .position(|r| r.model.epoch().get() == epoch)
                else {
                    return;
                };
                // a run still waiting its turn was not asked: nothing can answer it yet
                if state.control.runs[i].model.phase() != Phase::AwaitingConsent
                    || !state.control.runs[i].consent_shown
                {
                    return;
                }
                let e = state.control.runs[i].model.epoch();
                if allow == 1 {
                    if state.session_lock.is_active() || state.prompt_shown() {
                        return;
                    }
                    if capabilities(state.control.runs[i].capabilities).allows(Operation::Pointer)
                        && state
                            .control
                            .owners
                            .has_buttons(cupertino_control_model::InputSource::Physical(0))
                        && state.pointer.current_focus().as_ref()
                            != state.control.consent_surface.as_ref()
                    {
                        return;
                    }
                    if !state.control_fresh_consent(client, i, serial) {
                        return;
                    }
                    let run = &mut state.control.runs[i];
                    if run
                        .model
                        .approve(e, run.grant.expect("pending grant"))
                        .is_err()
                    {
                        return;
                    }
                    state.control_answered(i, Decision::Allow);
                    state.control_geometry(i);
                } else {
                    // a refusal is remembered only when the person chose it on the panel itself
                    let chosen = allow == ANSWER_DENY
                        && resource.version() >= 2
                        && state.control_fresh_consent(client, i, serial);
                    let _ = state.control.runs[i].model.deny(e);
                    if chosen {
                        state.control_answered(i, Decision::Deny);
                    }
                }
                state.control_changed(i, 0);
            }
            authority::Request::Stop { epoch_hi, epoch_lo } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                if let Some(i) = state
                    .control
                    .runs
                    .iter()
                    .position(|r| r.model.epoch().get() == epoch)
                {
                    state.control_revoke(i, Revocation::Stopped);
                }
            }
            _ => {}
        }
    }
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoControlAuthorityV1, _: &()) {
        if state.control.authority.as_ref() == Some(resource) {
            state.control.authority = None;
            state.control.consent_surface = None;
            state.control.user_serials.clear();
            // only the runs it was to ask for; a run already allowed does not depend on it
            for i in 0..state.control.runs.len() {
                if state.control.runs[i].model.phase() == Phase::AwaitingConsent {
                    state.control_revoke(i, Revocation::AuthorityDisconnected);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epoch_exhaustion_never_reuses_a_run_identity() {
        let display = smithay::reexports::wayland_server::Display::<Cupertino>::new().unwrap();
        let mut control = Control::new(&display.handle());
        control.next_epoch = u64::MAX;
        assert_eq!(control.allocate_epoch().unwrap().get(), u64::MAX);
        assert_eq!(control.allocate_epoch(), None);
        assert_eq!(control.allocate_epoch(), None);
    }
}
