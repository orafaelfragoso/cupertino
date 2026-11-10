//! Native desktop-control grants, trusted session consent and input routing. Version 1 remains discovery-only.
mod input;
mod semantics;
use crate::authorization::{authorized, require, ShellRole};
use crate::state::Cupertino;
use cupertino_control_model::{
    Capabilities, Epoch, Grant, Handle, InputOwners, Operation, Phase, Revocation, Scope, Session,
};
use cupertino_protocols::server::control::cupertino_control_authority_v1::{
    self as authority, CupertinoControlAuthorityV1,
};
use cupertino_protocols::server::control::cupertino_control_session_v1::{
    self as session, CupertinoControlSessionV1,
};
use cupertino_protocols::server::control::cupertino_control_v1::{
    self as manager, CupertinoControlV1,
};
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::Transform;
use smithay::wayland::compositor::with_states;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Current version; version 1 keeps discovery only and version 2 retains its input/capture ABI.
const VERSION: u32 = 3;
/// At most one session per private connection, across manager bindings.
const MAX_SESSIONS_PER_CONNECTION: usize = 1;
/// Number of completed request outcomes retained to suppress retries.
const REPLY_HISTORY: usize = 64;
/// Fresh physical input must have reached the consent surface within this interval.
const CONSENT_FRESHNESS: Duration = Duration::from_secs(3);
/// Bound serial provenance storage while the person interacts with consent.
const USER_SERIALS: usize = 32;
/// Operations implemented by version 3, adding semantic actions to version 2 input/capture.
const AVAILABLE: u32 = 31;

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
    revision: u32,
    last_serial: u32,
    consent_since: Instant,
    replies: VecDeque<(u32, Result<(), session::Refusal>)>,
}
impl Control {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<Cupertino, CupertinoControlV1, ()>(VERSION, ()),
            _authority: display.create_global::<Cupertino, CupertinoControlAuthorityV1, ()>(1, ()),
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
    fn control_changed(&self, index: usize, serial: u32) {
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
    pub(crate) fn suspend_controls(&mut self) {
        let mut changed = false;
        for i in 0..self.control.runs.len() {
            let epoch = self.control.runs[i].model.epoch();
            if self.control.runs[i].model.phase() == Phase::Active {
                changed = true;
                let _ = self.control.runs[i].model.suspend(epoch);
                self.control.runs[i].consent_since = Instant::now();
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
            } else if r.model.phase() != Phase::Inactive && !self.control.authority_alive() {
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
            if self.control.runs[i].model.phase() == Phase::Active {
                if self.prompt_shown() {
                    self.suspend_controls();
                    break;
                }
                let output = self.control.runs[i]
                    .output
                    .as_ref()
                    .and_then(WeakOutput::upgrade);
                if let Some(geometry) = output.and_then(|o| self.control_output_geometry(&o)) {
                    if self.control.runs[i].geometry.as_ref() != Some(&geometry) {
                        if let Some(revision) = self.control.runs[i].revision.checked_add(1) {
                            self.control.runs[i].revision = revision;
                            self.control.runs[i].geometry = Some(geometry);
                            self.control_geometry(i);
                            self.control_changed(i, 0);
                        } else {
                            self.control_revoke(i, Revocation::Stopped);
                        }
                    }
                }
            }
        }
        self.refresh_semantics();
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
        if !self.control.authority_alive() {
            return Err(session::Refusal::NoAuthority);
        }
        if bits == 0
            || bits & !AVAILABLE != 0
            || (bits & 16 != 0 && bits & 1 == 0)
            || (self.control.runs[index].resource.version() < 3 && bits & 16 != 0)
        {
            return Err(session::Refusal::Unsupported);
        }
        if self.control.runs.iter().enumerate().any(|(i, r)| {
            i != index
                && matches!(
                    r.model.phase(),
                    Phase::Active | Phase::AwaitingConsent | Phase::Suspended
                )
        }) {
            return Err(session::Refusal::Busy);
        }
        if bits & 4 != 0
            && self
                .control
                .owners
                .has_buttons(cupertino_control_model::InputSource::Physical(0))
        {
            return Err(session::Refusal::Busy);
        }
        let geometry = self
            .control_output_geometry(&output)
            .ok_or(session::Refusal::OutsideScope)?;
        let run = &mut self.control.runs[index];
        let epoch = run.model.epoch();
        let grant = Grant {
            scope: Scope::Output(Handle {
                id: epoch.get(),
                generation: epoch.get(),
            }),
            capabilities: capabilities(bits),
        };
        run.model
            .request(epoch, grant)
            .map_err(|_| session::Refusal::Busy)?;
        run.consent_since = Instant::now();
        run.output = Some(output.downgrade());
        run.grant = Some(grant);
        run.capabilities = bits;
        run.geometry = Some(geometry);
        run.revision = 1;
        self.control_ask(index, false);
        self.control_changed(index, 0);
        Ok(())
    }
    fn control_ask(&self, index: usize, resuming: bool) {
        let r = &self.control.runs[index];
        if let Some(a) = &self.control.authority {
            let epoch = r.model.epoch().get();
            let name = r
                .output
                .as_ref()
                .and_then(WeakOutput::upgrade)
                .map(|o| o.name())
                .unwrap_or_default();
            a.requested(
                (epoch >> 32) as u32,
                epoch as u32,
                r.capabilities,
                name,
                u32::from(resuming),
            );
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
            && self.control.authority_alive()
            && authorized(client, ShellRole::DesktopControl)
            && self.control.runs.iter().any(|r| {
                r.resource.client().as_ref() == Some(client)
                    && r.model.epoch().get() == epoch
                    && r.model.phase() == Phase::Active
                    && r.capabilities & 2 != 0
                    && r.output.as_ref().and_then(WeakOutput::upgrade).as_ref() == Some(output)
                    && self.control_output_geometry(output).is_some()
            })
    }
    pub(crate) fn control_capture_epoch(&self, client: &Client, output: &Output) -> Option<u64> {
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
                if state
                    .control
                    .runs
                    .iter()
                    .filter(|r| {
                        r.resource.is_alive() && r.resource.client().as_ref() == Some(client)
                    })
                    .count()
                    >= MAX_SESSIONS_PER_CONNECTION
                {
                    init.init(id, SessionData { epoch: None });
                    resource.post_error(manager::Error::SessionExists, "a session already exists");
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
                    last_serial: 0,
                    consent_since: Instant::now(),
                    replies: VecDeque::new(),
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
            session::Request::Resume { .. } => {
                if state.control.runs[i].model.phase() != Phase::Suspended || state.prompt_shown() {
                    Err(session::Refusal::Blocked)
                } else {
                    state.control_ask(i, true);
                    Ok(())
                }
            }
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
                let p = state.control.runs[i].model.phase();
                if !matches!(p, Phase::AwaitingConsent | Phase::Suspended) {
                    return;
                }
                if allow == 0 {
                    if p == Phase::AwaitingConsent {
                        let e = state.control.runs[i].model.epoch();
                        let _ = state.control.runs[i].model.deny(e);
                    } else {
                        state.control_revoke(i, Revocation::Stopped);
                    }
                } else {
                    if allow != 1 || state.session_lock.is_active() || state.prompt_shown() {
                        return;
                    }
                    if state.control.runs[i].capabilities & 4 != 0
                        && state
                            .control
                            .owners
                            .has_buttons(cupertino_control_model::InputSource::Physical(0))
                        && state.pointer.current_focus().as_ref()
                            != state.control.consent_surface.as_ref()
                    {
                        return;
                    }
                    let fresh = state
                        .control
                        .user_serials
                        .iter()
                        .position(|(s, surface, time)| {
                            *s == serial
                                && time.elapsed() <= CONSENT_FRESHNESS
                                && *time >= state.control.runs[i].consent_since
                                && state.control.consent_surface.as_ref() == Some(surface)
                                && surface.client().as_ref() == Some(client)
                        });
                    let Some(n) = fresh else { return };
                    state.control.user_serials.remove(n);
                    state.control.user_serials.clear();
                    let run = &mut state.control.runs[i];
                    let e = run.model.epoch();
                    let accepted = if p == Phase::AwaitingConsent {
                        run.model.approve(e, run.grant.expect("pending grant"))
                    } else {
                        run.model.resume(e)
                    };
                    if accepted.is_err() {
                        return;
                    }
                    state.control_geometry(i);
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
            state.revoke_controls(Revocation::AuthorityDisconnected);
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
