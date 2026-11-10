//! Native semantics ride the existing transport queue; no accessibility service is involved.
use cupertino_app_protocols::client::semantics::{
    cupertino_semantic_surface_v1::{self, CupertinoSemanticSurfaceV1},
    cupertino_semantics_v1::CupertinoSemanticsV1,
};
use cupertino_semantics::{ActionCall, ActionReply, Outcome, Snapshot, MAX_FRAME};
use cupertino_wayland_client::{Connection, State, SurfaceId};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};

#[derive(Clone, Debug)]
pub struct SemanticWake;
#[derive(Default)]
struct Inner {
    epoch: u64,
    active: bool,
    sent: Option<(u64, u64)>,
    requests: Vec<ActionCall>,
}
#[derive(Clone)]
struct Shared(Arc<Mutex<Inner>>);
pub struct Semantics(CupertinoSemanticsV1);
pub struct SemanticSurface {
    proxy: CupertinoSemanticSurfaceV1,
    inner: Shared,
}
impl Semantics {
    pub fn bind(conn: &mut Connection) -> Option<Self> {
        conn.bind_extension(1..=1, crate::Quiet).map(Self)
    }
    pub fn surface(
        &self,
        conn: &Connection,
        surface: SurfaceId,
    ) -> Result<SemanticSurface, cupertino_wayland_client::Error> {
        let inner = Shared(Arc::new(Mutex::new(Inner::default())));
        let proxy = self.0.get_surface(
            &conn.wl_surface(surface)?,
            &conn.queue_handle(),
            inner.clone(),
        );
        Ok(SemanticSurface { proxy, inner })
    }
}
impl Drop for Semantics {
    fn drop(&mut self) {
        self.0.destroy();
    }
}
impl SemanticSurface {
    pub fn active(&self) -> bool {
        self.inner.0.lock().unwrap().active
    }
    pub fn requests(&self) -> Vec<ActionCall> {
        let mut s = self.inner.0.lock().unwrap();
        if !s.active {
            s.requests.clear();
        }
        std::mem::take(&mut s.requests)
    }
    pub fn publish(&self, snapshot: &Snapshot) {
        let mut s = self.inner.0.lock().unwrap();
        if !s.active || s.sent == Some((s.epoch, snapshot.revision)) {
            return;
        }
        let epoch = s.epoch;
        match snapshot
            .validate()
            .map_err(std::io::Error::other)
            .and_then(|_| cupertino_semantics::seal(snapshot, MAX_FRAME))
        {
            Ok((file, len)) => {
                self.proxy
                    .publish((epoch >> 32) as u32, epoch as u32, file.as_fd(), len)
            }
            Err(_) => self.proxy.unavailable((epoch >> 32) as u32, epoch as u32),
        }
        s.sent = Some((epoch, snapshot.revision));
    }
    pub fn complete(&self, call: &ActionCall, outcome: Outcome, revision: u64) {
        let s = self.inner.0.lock().unwrap();
        if !s.active || s.epoch != call.epoch {
            return;
        }
        if let Ok((file, len)) = cupertino_semantics::seal(
            &ActionReply {
                id: call.id,
                epoch: call.epoch,
                revision,
                outcome,
            },
            MAX_FRAME,
        ) {
            self.proxy.complete(file.as_fd(), len);
        }
    }
}
impl Drop for SemanticSurface {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}
impl Dispatch<CupertinoSemanticsV1, crate::Quiet> for State {
    fn event(
        _: &mut Self,
        _: &CupertinoSemanticsV1,
        _: <CupertinoSemanticsV1 as wayland_client::Proxy>::Event,
        _: &crate::Quiet,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<CupertinoSemanticSurfaceV1, Shared> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoSemanticSurfaceV1,
        event: cupertino_semantic_surface_v1::Event,
        shared: &Shared,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = shared.0.lock().unwrap();
        match event {
            cupertino_semantic_surface_v1::Event::Interest {
                epoch_hi,
                epoch_lo,
                active,
            } => {
                s.epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                s.active = active != 0;
                s.sent = None;
                s.requests.clear();
            }
            cupertino_semantic_surface_v1::Event::Perform { payload, length } => {
                if let Ok(call) =
                    cupertino_semantics::read::<ActionCall>(payload, length, MAX_FRAME)
                {
                    if s.active && call.epoch == s.epoch && s.requests.is_empty() {
                        s.requests.push(call);
                    }
                }
            }
            _ => return,
        }
        drop(s);
        state.emit(SemanticWake);
    }
}
