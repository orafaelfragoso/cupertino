//! Server-owned scope and transforms. Publications never grant permission or name another client's surface.
use super::*;
use cupertino_protocols::server::semantics::{
    cupertino_semantic_surface_v1::{self as publisher, CupertinoSemanticSurfaceV1},
    cupertino_semantics_v1::{self as manager, CupertinoSemanticsV1},
};
use cupertino_semantics as wire;
use smithay::desktop::{layer_map_for_output, PopupManager};
use std::os::fd::{AsFd, OwnedFd};

/// One native action may be outstanding per controller, with no automatic replay after timeout.
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Each connection may publish at most this many live native surfaces.
const MAX_PUBLISHERS: usize = 64;
pub(super) struct SemanticState {
    _global: GlobalId,
    next: u64,
    next_action: u64,
    revision: u64,
    pubs: Vec<Publication>,
    pending: Vec<Pending>,
    layout: Vec<wire::Window>,
}
struct Publication {
    resource: CupertinoSemanticSurfaceV1,
    surface: WlSurface,
    handle: u64,
    epoch: Option<u64>,
    snapshot: Option<wire::Snapshot>,
}
struct Pending {
    id: u64,
    epoch: u64,
    serial: u32,
    source: CupertinoSemanticSurfaceV1,
    controller: CupertinoControlSessionV1,
    deadline: Instant,
}
impl SemanticState {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<Cupertino, CupertinoSemanticsV1, ()>(1, ()),
            next: 1,
            next_action: 1,
            revision: 1,
            pubs: Vec::new(),
            pending: Vec::new(),
            layout: Vec::new(),
        }
    }
}
fn split(n: u64) -> (u32, u32) {
    ((n >> 32) as u32, n as u32)
}
impl Cupertino {
    fn semantic_run(&self) -> Option<usize> {
        if self.session_lock.is_active()
            || self.login_screen
            || self.prompt_shown()
            || !self.control.authority_alive()
        {
            return None;
        }
        self.control.runs.iter().position(|r| {
            r.resource.version() >= 3
                && r.model.phase() == Phase::Active
                && r.capabilities & 1 != 0
                && r.resource
                    .client()
                    .is_some_and(|c| authorized(&c, ShellRole::DesktopControl))
        })
    }
    fn semantic_root_meta(
        &self,
        surface: &WlSurface,
        index: usize,
        handle: u64,
    ) -> Option<wire::Window> {
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
            return Some(wire::Window {
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
            });
        }
        let map = layer_map_for_output(&output);
        let layer = map.layers().find(|l| l.wl_surface() == surface)?;
        let g = map.layer_geometry(layer)?;
        if layer.bbox().size.w == 0 || layer.bbox().size.h == 0 {
            return None;
        }
        Some(wire::Window {
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
        })
    }
    fn semantic_meta(&self, p: &Publication, index: usize) -> Option<wire::Window> {
        if !p.surface.is_alive() {
            return None;
        }
        let root = self.shell_surface_root(&p.surface);
        let mut meta = self.semantic_root_meta(&root, index, p.handle)?;
        if root != p.surface {
            let (popup, offset) = PopupManager::popups_for_surface(&root)
                .find(|(popup, _)| popup.wl_surface() == &p.surface)?;
            let g = popup.geometry();
            meta.origin[0] += f64::from(offset.x - g.loc.x) * meta.scale[0];
            meta.origin[1] += f64::from(offset.y - g.loc.y) * meta.scale[1];
        }
        Some(meta)
    }
    fn semantics_changed(&mut self) {
        self.control.semantics.revision = self
            .control
            .semantics
            .revision
            .checked_add(1)
            .expect("semantic revision exhausted");
        if let Some(i) = self.semantic_run() {
            let (h, l) = split(self.control.semantics.revision);
            self.control.runs[i].resource.semantic_changed(h, l);
            if let Some(revision) = self.control.runs[i].revision.checked_add(1) {
                self.control.runs[i].revision = revision;
                self.control_geometry(i);
                self.control_changed(i, 0);
            } else {
                self.control_revoke(i, Revocation::Stopped);
            }
        }
    }
    pub(super) fn refresh_semantics(&mut self) {
        let active = self.semantic_run();
        let mut layout = Vec::new();
        if let Some(i) = active {
            for entry in &self.windows {
                if let Some(m) = entry
                    .id
                    .0
                    .checked_mul(2)
                    .and_then(|h| self.semantic_root_meta(entry.surface(), i, h))
                {
                    layout.push(m);
                }
            }
            for p in &self.control.semantics.pubs {
                if !self.windows.iter().any(|w| w.surface() == &p.surface) {
                    if let Some(m) = self.semantic_meta(p, i) {
                        layout.push(m);
                    }
                }
            }
        }
        if layout != self.control.semantics.layout {
            self.control.semantics.layout = layout;
            self.semantics_changed();
        }
        let epochs: Vec<_> = self
            .control
            .semantics
            .pubs
            .iter()
            .map(|p| {
                active
                    .filter(|i| self.semantic_meta(p, *i).is_some())
                    .map(|i| self.control.runs[i].model.epoch().get())
            })
            .collect();
        let mut changed = false;
        for (p, epoch) in self.control.semantics.pubs.iter_mut().zip(epochs) {
            if p.epoch != epoch {
                p.epoch = epoch;
                p.snapshot = None;
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
            let valid = active.is_some_and(|i| {
                self.control.runs[i].model.epoch().get() == p.epoch
                    && self.control.runs[i].resource == p.controller
            }) && self
                .control
                .semantics
                .pubs
                .iter()
                .any(|s| s.resource == p.source && s.epoch == Some(p.epoch));
            if valid && Instant::now() < p.deadline {
                self.control.semantics.pending.push(p);
                continue;
            }
            if let Some(i) = self
                .control
                .runs
                .iter()
                .position(|r| r.resource == p.controller)
            {
                self.control_finish(i, p.serial, Err(session::Refusal::UnknownOutcome));
            }
        }
    }
    pub(super) fn control_observe(
        &mut self,
        index: usize,
        serial: u32,
    ) -> Result<(), session::Refusal> {
        if self.semantic_run() != Some(index) {
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
                if let Some(p) = self.control.semantics.pubs.iter().find(|p| {
                    p.surface == *entry.surface()
                        && p.epoch == Some(self.control.runs[index].model.epoch().get())
                }) {
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
        if self.semantic_run() != Some(index) || self.control.runs[index].capabilities & 16 == 0 {
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
        if !self.control.semantics.pending.is_empty() {
            return Err(session::Refusal::Busy);
        }
        let id = self.control.semantics.next_action;
        self.control.semantics.next_action =
            id.checked_add(1).ok_or(session::Refusal::Unsupported)?;
        let epoch = self.control.runs[index].model.epoch().get();
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
        source.perform(file.as_fd(), len);
        self.control.semantics.pending.push(Pending {
            id,
            epoch,
            serial,
            source,
            controller: self.control.runs[index].resource.clone(),
            deadline: Instant::now() + ACTION_TIMEOUT,
        });
        Ok(true)
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
                        state.semantics_changed();
                    }
                    _ => {
                        p.snapshot = None;
                        resource.post_error(
                            manager::Error::InvalidPayload,
                            "invalid, stale or unsealed semantic publication",
                        );
                    }
                }
            }
            publisher::Request::Unavailable { epoch_hi, epoch_lo } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                let p = &mut state.control.semantics.pubs[i];
                if p.epoch == Some(epoch) {
                    p.snapshot = None;
                    state.semantics_changed();
                }
            }
            publisher::Request::Complete { payload, length } => {
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
