//! `zwlr_output_management_unstable_v1`, version 4: every head with its modes and state is listed
//! to each bound manager, changes are sent as they happen (followed by `done` with the new serial),
//! and a client applies or tests a whole configuration through [`Cupertino::configure_heads`].
//! Adaptive sync is reported off and cannot be turned on.

use std::sync::{Arc, Mutex};

use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::{
    zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, AdaptiveSyncState, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::Transform;

use super::{Head, HeadId, HeadMode, HeadState};
use crate::state::Cupertino;

pub const VERSION: u32 = 4;
/// `zwlr_output_mode_v1` stops at version 3.
const MODE_VERSION: u32 = 3;

/// A bound manager and the head and mode objects it was sent.
pub(crate) struct Manager {
    resource: ZwlrOutputManagerV1,
    heads: Vec<HeadObject>,
}

struct HeadObject {
    head: HeadId,
    resource: ZwlrOutputHeadV1,
    modes: Vec<(HeadMode, ZwlrOutputModeV1)>,
}

/// A configuration being put together, then applied or tested once.
pub struct ConfigurationData {
    serial: u32,
    inner: Mutex<Configuration>,
}

#[derive(Default)]
struct Configuration {
    /// Each head named, with what it is set to; `None` disables it.
    heads: Vec<(HeadId, Option<Arc<Mutex<PendingHead>>>)>,
    used: bool,
}

/// What a configuration sets on an enabled head; unset properties keep their current value.
#[derive(Default)]
struct PendingHead {
    mode: Option<HeadMode>,
    position: Option<(i32, i32)>,
    transform: Option<Transform>,
    scale: Option<f64>,
    adaptive_sync: Option<bool>,
}

pub struct ConfigurationHeadData {
    head: HeadId,
    pending: Arc<Mutex<PendingHead>>,
}

pub fn create_global(dh: &DisplayHandle) -> GlobalId {
    dh.create_global::<Cupertino, ZwlrOutputManagerV1, ()>(VERSION, ())
}

/// Send `mode`'s properties to a new mode object.
fn describe_mode(resource: &ZwlrOutputModeV1, mode: HeadMode, preferred: bool) {
    resource.size(mode.width, mode.height);
    if mode.refresh_mhz > 0 {
        resource.refresh(mode.refresh_mhz);
    }
    if preferred {
        resource.preferred();
    }
}

fn new_mode(
    dh: &DisplayHandle,
    client: &Client,
    head: &HeadObject,
    mode: HeadMode,
    preferred: bool,
) -> Option<ZwlrOutputModeV1> {
    let version = head.resource.version().min(MODE_VERSION);
    let resource = client
        .create_resource::<ZwlrOutputModeV1, (HeadId, HeadMode), Cupertino>(
            dh,
            version,
            (head.head, mode),
        )
        .ok()?;
    head.resource.mode(&resource);
    describe_mode(&resource, mode, preferred);
    Some(resource)
}

fn current_mode(head: &HeadObject, mode: HeadMode) {
    if let Some((_, resource)) = head.modes.iter().find(|(m, _)| *m == mode) {
        head.resource.current_mode(resource);
    }
}

/// The state events of a head: whether it is enabled, then for an enabled head its mode, position,
/// transform and scale, each when it differs from `before` (all of them when it was off or new).
fn describe_state(object: &HeadObject, now: &HeadState, before: Option<&HeadState>) {
    if before.map(|b| b.enabled) != Some(now.enabled) {
        object.resource.enabled(i32::from(now.enabled));
    }
    if !now.enabled {
        return;
    }
    let before = before.filter(|b| b.enabled);
    if before.is_none_or(|b| b.mode != now.mode) {
        current_mode(object, now.mode);
    }
    if before.is_none_or(|b| b.position != now.position) {
        object.resource.position(now.position.0, now.position.1);
    }
    if before.is_none_or(|b| b.transform != now.transform) {
        object
            .resource
            .transform(wl_output::Transform::from(now.transform));
    }
    if before.is_none_or(|b| b.scale != now.scale) {
        object.resource.scale(now.scale);
    }
}

/// Introduce `head` to a manager with everything it has.
fn announce(
    dh: &DisplayHandle,
    client: &Client,
    manager: &ZwlrOutputManagerV1,
    head: &Head,
) -> Option<HeadObject> {
    let resource = client
        .create_resource::<ZwlrOutputHeadV1, HeadId, Cupertino>(dh, manager.version(), head.id)
        .ok()?;
    manager.head(&resource);
    let info = &head.info;
    resource.name(info.connector.clone());
    resource.description(format!("{} {} ({})", info.make, info.model, info.connector));
    if info.physical_mm.0 > 0 && info.physical_mm.1 > 0 {
        resource.physical_size(info.physical_mm.0, info.physical_mm.1);
    }
    let mut object = HeadObject {
        head: head.id,
        resource,
        modes: Vec::new(),
    };
    for (i, mode) in info.modes.iter().enumerate() {
        if let Some(m) = new_mode(dh, client, &object, *mode, i == 0) {
            object.modes.push((*mode, m));
        }
    }
    if object.resource.version() >= 2 {
        object.resource.make(info.make.clone());
        object.resource.model(info.model.clone());
        if let Some(serial) = &info.serial {
            object.resource.serial_number(serial.clone());
        }
    }
    describe_state(&object, &head.state, None);
    if object.resource.version() >= 4 {
        object.resource.adaptive_sync(AdaptiveSyncState::Disabled);
    }
    Some(object)
}

/// Head `idx` is new or changed since it was last advertised: tell every manager.
pub(crate) fn head_changed(state: &mut Cupertino, idx: usize) {
    let dh = state.display_handle.clone();
    let head = &state.displays.heads[idx];
    for manager in &mut state.displays.managers {
        let Some(client) = manager.resource.client() else {
            continue;
        };
        let Some(object) = manager.heads.iter_mut().find(|o| o.head == head.id) else {
            if let Some(object) = announce(&dh, &client, &manager.resource, head) {
                manager.heads.push(object);
            }
            continue;
        };
        object.modes.retain(|(mode, resource)| {
            let keep = head.info.modes.contains(mode);
            if !keep {
                resource.finished();
            }
            keep
        });
        for mode in &head.info.modes {
            if !object.modes.iter().any(|(m, _)| m == mode) {
                if let Some(resource) = new_mode(&dh, &client, object, *mode, false) {
                    object.modes.push((*mode, resource));
                }
            }
        }
        describe_state(
            object,
            &head.state,
            head.advertised.as_ref().map(|(s, _)| s),
        );
    }
}

/// Head `id` went away: its objects are finished.
pub(crate) fn head_gone(state: &mut Cupertino, id: HeadId) {
    for manager in &mut state.displays.managers {
        manager.heads.retain(|o| {
            if o.head != id {
                return true;
            }
            for (_, mode) in &o.modes {
                mode.finished();
            }
            o.resource.finished();
            false
        });
    }
}

/// End a batch of changes on every manager.
pub(crate) fn done(state: &mut Cupertino) {
    for manager in &state.displays.managers {
        manager.resource.done(state.displays.serial);
    }
}

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let resource = init.init(resource, ());
        if state.session_lock.is_active() {
            // the displays are not for anyone to look at or change while the session is locked; `finished` destroys the
            // object, which cannot happen inside the bind
            state.loop_handle.insert_idle(move |_| resource.finished());
            return;
        }
        let heads = state
            .displays
            .heads
            .iter()
            .filter_map(|h| announce(dh, client, &resource, h))
            .collect();
        resource.done(state.displays.serial);
        state.displays.managers.push(Manager { resource, heads });
    }

    fn can_view(client: Client, _: &()) -> bool {
        crate::authorization::unsandboxed(&client)
    }
}

/// The session was locked: every manager is finished, so nothing about the displays reaches a client until it binds
/// again after the unlock.
pub(crate) fn session_locked(state: &mut Cupertino) {
    for manager in state.displays.managers.drain(..) {
        manager.resource.finished();
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                init.init(
                    id,
                    ConfigurationData {
                        serial,
                        inner: Mutex::new(Configuration::default()),
                    },
                );
            }
            zwlr_output_manager_v1::Request::Stop => {
                resource.finished();
                state.displays.managers.retain(|m| &m.resource != resource);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputManagerV1, _: &()) {
        state.displays.managers.retain(|m| &m.resource != resource);
    }
}

impl Dispatch<ZwlrOutputHeadV1, HeadId> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputHeadV1,
        request: zwlr_output_head_v1::Request,
        _: &HeadId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_head_v1::Request::Release = request {
            forget_head(state, resource);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputHeadV1, _: &HeadId) {
        forget_head(state, resource);
    }
}

fn forget_head(state: &mut Cupertino, resource: &ZwlrOutputHeadV1) {
    for manager in &mut state.displays.managers {
        manager.heads.retain(|o| &o.resource != resource);
    }
}

impl Dispatch<ZwlrOutputModeV1, (HeadId, HeadMode)> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputModeV1,
        request: zwlr_output_mode_v1::Request,
        _: &(HeadId, HeadMode),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_mode_v1::Request::Release = request {
            forget_mode(state, resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &ZwlrOutputModeV1,
        _: &(HeadId, HeadMode),
    ) {
        forget_mode(state, resource);
    }
}

fn forget_mode(state: &mut Cupertino, resource: &ZwlrOutputModeV1) {
    for manager in &mut state.displays.managers {
        for head in &mut manager.heads {
            head.modes.retain(|(_, m)| m != resource);
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ConfigurationData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &ConfigurationData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_v1::{Error, Request};
        let mut config = data.inner.lock().expect("configuration lock");
        let (head, test_only) = match request {
            Request::EnableHead { id, head } => (Some((head, Some(id))), false),
            Request::DisableHead { head } => (Some((head, None)), false),
            Request::Apply => (None, false),
            Request::Test => (None, true),
            Request::Destroy => return,
            _ => return,
        };
        if config.used {
            resource.post_error(
                Error::AlreadyUsed,
                "the configuration was already applied or tested",
            );
            return;
        }
        if let Some((head, enable)) = head {
            let id = *head
                .data::<HeadId>()
                .expect("head objects carry their head");
            if config.heads.iter().any(|(h, _)| *h == id) {
                resource.post_error(
                    Error::AlreadyConfiguredHead,
                    "the head is already in the configuration",
                );
                return;
            }
            let pending = enable.map(|new| {
                let pending = Arc::new(Mutex::new(PendingHead::default()));
                init.init(
                    new,
                    ConfigurationHeadData {
                        head: id,
                        pending: pending.clone(),
                    },
                );
                pending
            });
            config.heads.push((id, pending));
            return;
        }
        config.used = true;
        if state.session_lock.is_active() {
            resource.failed();
            return;
        }
        if data.serial != state.displays.serial {
            resource.cancelled();
            return;
        }
        if let Some(missing) = state
            .displays
            .heads
            .iter()
            .find(|h| !config.heads.iter().any(|(id, _)| *id == h.id))
        {
            resource.post_error(
                Error::UnconfiguredHead,
                format!("{} is not in the configuration", missing.info.connector),
            );
            return;
        }
        let mut wanted = Vec::with_capacity(config.heads.len());
        for (id, pending) in &config.heads {
            let Some(head) = state.displays.head(*id) else {
                resource.cancelled();
                return;
            };
            let mut s = head.state;
            s.enabled = pending.is_some();
            if let Some(pending) = pending {
                let p = pending.lock().expect("configuration head lock");
                if p.adaptive_sync == Some(true) {
                    tracing::info!(connector = %head.info.connector, "output configuration failed: adaptive sync is not supported");
                    resource.failed();
                    return;
                }
                s.mode = p.mode.unwrap_or(s.mode);
                s.position = p.position.unwrap_or(s.position);
                s.transform = p.transform.unwrap_or(s.transform);
                if let Some(scale) = p.scale {
                    s.scale = super::exact_choice_scale(head, s.mode, scale);
                }
            }
            wanted.push((*id, s));
        }
        drop(config);
        match state.configure_heads(&wanted, test_only) {
            Ok(()) => resource.succeeded(),
            Err(e) => {
                tracing::info!(error = %e, test_only, "output configuration failed");
                resource.failed();
            }
        }
        state.heads_changed();
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ConfigurationHeadData> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &ConfigurationHeadData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_head_v1::{Error, Request};
        let mut p = data.pending.lock().expect("configuration head lock");
        let already = |set: bool| {
            if set {
                resource.post_error(Error::AlreadySet, "the property was already set");
            }
            set
        };
        match request {
            Request::SetMode { mode } => {
                if already(p.mode.is_some()) {
                    return;
                }
                match mode.data::<(HeadId, HeadMode)>() {
                    Some((head, mode)) if *head == data.head => p.mode = Some(*mode),
                    _ => {
                        resource.post_error(Error::InvalidMode, "the mode belongs to another head")
                    }
                }
            }
            Request::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                if already(p.mode.is_some()) {
                    return;
                }
                if width <= 0 || height <= 0 || refresh < 0 {
                    resource.post_error(
                        Error::InvalidCustomMode,
                        format!("{width}x{height}@{refresh} is not a mode"),
                    );
                    return;
                }
                p.mode = Some(HeadMode {
                    width,
                    height,
                    refresh_mhz: refresh,
                });
            }
            Request::SetPosition { x, y } => {
                if !already(p.position.is_some()) {
                    p.position = Some((x, y));
                }
            }
            Request::SetTransform { transform } => {
                if already(p.transform.is_some()) {
                    return;
                }
                match transform {
                    WEnum::Value(t) => p.transform = Some(Transform::from(t)),
                    WEnum::Unknown(v) => resource.post_error(
                        Error::InvalidTransform,
                        format!("transform {v} is outside the enum"),
                    ),
                }
            }
            Request::SetScale { scale } => {
                if already(p.scale.is_some()) {
                    return;
                }
                if !(scale.is_finite() && scale > 0.0) {
                    resource.post_error(
                        Error::InvalidScale,
                        format!("scale {scale} is not above zero"),
                    );
                    return;
                }
                p.scale = Some(scale);
            }
            Request::SetAdaptiveSync { state } => {
                if already(p.adaptive_sync.is_some()) {
                    return;
                }
                match state {
                    WEnum::Value(s) => p.adaptive_sync = Some(s == AdaptiveSyncState::Enabled),
                    WEnum::Unknown(v) => resource.post_error(
                        Error::InvalidAdaptiveSyncState,
                        format!("adaptive sync state {v} is outside the enum"),
                    ),
                }
            }
            _ => {}
        }
    }
}
