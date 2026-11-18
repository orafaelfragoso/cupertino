//! Commits sent from another thread than the connection's: a [`Commit`] holds what one present of a surface sends
//! (the configure it answers, the buffer, how it is scaled, the sync points, the frame callback and feedback), made on
//! the connection's thread, and sends it from any thread. The surface's configure serials are shared with the
//! connection behind a lock that is held only while the commit's requests go out, so a configure that arrives
//! meanwhile waits for it and is acknowledged by the frame after. Events still arrive on the connection's queue.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection as WlConnection, QueueHandle};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1;
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface;

use super::{clamp_i32, ExternalRole, State, SurfaceId};
use crate::buffer::PixelRect;
use crate::configure::Serials;
use crate::Error;

/// What a surface shares with the commits made for it.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    pub(crate) serials: Serials,
    /// The surface was destroyed: a commit made before sends nothing.
    pub(crate) gone: bool,
    /// The viewport shows a crop of the buffer: the frame committed last was magnified
    /// ([`BufferLease::set_zoom`](crate::BufferLease::set_zoom)). Only a commit sent changes it, so one dropped unsent
    /// leaves the crop for the next to unset.
    pub(crate) zoomed: bool,
}

pub(crate) type SharedState = Arc<Mutex<Shared>>;

pub(crate) fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who acknowledges a configure of the surface.
#[derive(Clone)]
pub(crate) enum Ack {
    Xdg(XdgSurface),
    External(Arc<dyn ExternalRole>),
}

impl Ack {
    fn send(&self, serial: u32) {
        match self {
            Ack::Xdg(xdg) => xdg.ack_configure(serial),
            Ack::External(role) => role.ack_configure(serial),
        }
    }
}

/// How the buffer is scaled onto the surface.
pub(crate) enum Scaling {
    /// By the surface's viewport: the crop of the buffer it shows (pixels; `None` for all of it) and the destination
    /// in points.
    Viewport {
        viewport: WpViewport,
        crop: Option<(f64, f64, f64, f64)>,
        destination: (u32, u32),
    },
    /// By a whole buffer scale.
    Buffer(i32),
}

/// One present of a surface, ready to be sent from any thread ([`Connection::gpu_commit`](crate::Connection::gpu_commit),
/// [`BufferLease::detach`](crate::BufferLease::detach)). It answers the configure that was the surface's latest when
/// it was made: [`Commit::commit`] acknowledges that configure, unless a later one already was, then attaches the
/// buffer and commits, in that order. A commit dropped unsent sends nothing.
pub struct Commit {
    pub(crate) surface: SurfaceId,
    pub(crate) wl: WlSurface,
    pub(crate) shared: SharedState,
    /// The configure the frame answers and who acknowledges it; `None` for a subsurface, which answers none and
    /// shows with its parent's commit (whose frame callback and feedback stand for both).
    pub(crate) ack: Option<(Ack, u32)>,
    pub(crate) buffer: WlBuffer,
    pub(crate) layout: (u32, u32),
    pub(crate) scaling: Scaling,
    pub(crate) sync: Option<(
        WpLinuxDrmSyncobjSurfaceV1,
        WpLinuxDrmSyncobjTimelineV1,
        u64,
        u64,
    )>,
    pub(crate) qh: QueueHandle<State>,
    pub(crate) presentation: Option<WpPresentation>,
    pub(crate) conn: WlConnection,
}

impl Commit {
    /// The surface it commits.
    pub fn surface(&self) -> SurfaceId {
        self.surface
    }

    /// The configure it answers; `None` for a subsurface.
    pub fn serial(&self) -> Option<u32> {
        self.ack.as_ref().map(|(_, serial)| *serial)
    }

    /// Send the present, telling the compositor only `damage` (buffer pixels) changed since the frame before; `None`
    /// for all of it. A surface destroyed since the commit was made is left alone.
    pub fn commit(self, damage: Option<&[PixelRect]>) -> Result<(), Error> {
        {
            let mut shared = lock(&self.shared);
            if shared.gone {
                return Ok(());
            }
            if let Some((ack, serial)) = &self.ack {
                if shared.serials.answer(*serial) {
                    ack.send(*serial);
                }
            }
            if let Some((object, timeline, acquire, release)) = &self.sync {
                object.set_acquire_point(timeline, (acquire >> 32) as u32, *acquire as u32);
                object.set_release_point(timeline, (release >> 32) as u32, *release as u32);
            }
            self.wl.attach(Some(&self.buffer), 0, 0);
            match &self.scaling {
                Scaling::Viewport {
                    viewport,
                    crop,
                    destination,
                } => {
                    self.wl.set_buffer_scale(1);
                    match *crop {
                        Some((x, y, w, h)) => {
                            viewport.set_source(x, y, w, h);
                            shared.zoomed = true;
                        }
                        // the crop the last frame sent goes
                        None if shared.zoomed => {
                            viewport.set_source(-1.0, -1.0, -1.0, -1.0);
                            shared.zoomed = false;
                        }
                        None => {}
                    }
                    viewport.set_destination(clamp_i32(destination.0), clamp_i32(destination.1));
                }
                Scaling::Buffer(n) => self.wl.set_buffer_scale(*n),
            }
            match damage {
                None => self.wl.damage_buffer(0, 0, i32::MAX, i32::MAX),
                Some(rects) => {
                    let (width, height) = self.layout;
                    for r in rects {
                        let (x1, y1) = (
                            r.x.saturating_add(r.width).min(width),
                            r.y.saturating_add(r.height).min(height),
                        );
                        if r.x < x1 && r.y < y1 {
                            self.wl.damage_buffer(
                                clamp_i32(r.x),
                                clamp_i32(r.y),
                                clamp_i32(x1 - r.x),
                                clamp_i32(y1 - r.y),
                            );
                        }
                    }
                }
            }
            if self.ack.is_some() {
                self.wl.frame(&self.qh, self.surface);
                if let Some(presentation) = &self.presentation {
                    presentation.feedback(&self.wl, &self.qh, self.surface);
                }
            }
            self.wl.commit();
        }
        flush(&self.conn)
    }
}

/// Flush what was sent; a full socket is flushed by the next call.
fn flush(conn: &WlConnection) -> Result<(), Error> {
    match conn.flush() {
        Ok(()) => Ok(()),
        Err(wayland_client::backend::WaylandError::Io(e))
            if e.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
