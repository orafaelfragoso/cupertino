//! Captures of outputs and windows for a program with the `capture` role (`shell-host.md#consent-and-capture`): each
//! capture is a session of `ext_image_copy_capture_v1` on the source the program names, into a buffer in shared
//! memory, read once its frame is ready. A single capture ([`crate::capture::once`]) ends with its first frame; a
//! followed one ([`crate::capture::follow`]) asks for the next frame after each.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex, PoisonError};

use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_wayland_client::{Connection, ExtensionEvent, State};
use rustix::fs::{memfd_create, MemfdFlags};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{
    self as handle, ExtForeignToplevelHandleV1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{
    self as list, ExtForeignToplevelListV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::{
    self as toplevel_sources, ExtForeignToplevelImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::{
    self as source, ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::{
    self as output_sources, ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{
    self as frame, ExtImageCopyCaptureFrameV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{
    self as manager, ExtImageCopyCaptureManagerV1, Options as WireOptions,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{
    self as session, ExtImageCopyCaptureSessionV1,
};

use super::roles::wire_output;
use crate::capture::{Event as CaptureEvent, Options, Source};
use crate::context::{self, Inbox};
use crate::{Error, Event};

/// Bytes per pixel of the shared-memory formats a capture is read in (`argb8888`, `xrgb8888`).
const BYTES_PER_PIXEL: u32 = 4;

/// A capture, by the program's id of it and whether it follows the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Key {
    Once(u32),
    Follow(u32),
}

/// What the capture objects received, for the roles to hand back here ([`Capturing::event`]).
#[derive(Debug)]
enum Wire {
    /// The session's buffer constraints: the size and the shared-memory formats a frame takes.
    Constraints {
        key: Key,
        size: (u32, u32),
        formats: Vec<wl_shm::Format>,
    },
    Ready(Key),
    Failed(Key),
    Stopped(Key),
}

/// A session's buffer size and shared-memory formats, as the server sends them before `done`.
type Constraints = (Option<(u32, u32)>, Vec<wl_shm::Format>);

/// User data of a session: the constraints the server sends before `done`.
struct SessionData {
    key: Key,
    constraints: Mutex<Constraints>,
}

/// User data of a frame.
struct FrameData(Key);

/// User data of the objects with no events the capture needs.
struct Quiet;

/// User data of the toplevel list: the handles it announced, shared with the captures that look them up.
#[derive(Clone, Default)]
struct Handles(Arc<Mutex<Vec<ExtForeignToplevelHandleV1>>>);

/// User data of a toplevel handle: the identifier the server gave it.
#[derive(Default)]
struct HandleData(Mutex<Option<String>>);

/// A frame being copied into shared memory.
struct Copy {
    frame: ExtImageCopyCaptureFrameV1,
    buffer: WlBuffer,
    file: File,
    size: (u32, u32),
    format: wl_shm::Format,
}

/// One capture's session and its frame in flight.
struct Capture {
    session: ExtImageCopyCaptureSessionV1,
    source: ExtImageCaptureSourceV1,
    options: Options,
    /// The size of the source in logical points, which a region is given in.
    logical: Option<(f64, f64)>,
    constraints: Option<((u32, u32), Vec<wl_shm::Format>)>,
    copy: Option<Copy>,
    /// The next frame is to be asked for.
    wanted: bool,
}

/// The program's captures and the globals they are made on.
pub(crate) struct Capturing {
    manager: Option<ExtImageCopyCaptureManagerV1>,
    outputs: Option<ExtOutputImageCaptureSourceManagerV1>,
    toplevels: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    list: Option<(ExtForeignToplevelListV1, Handles)>,
    shm: Option<WlShm>,
    captures: HashMap<Key, Capture>,
    revision: u64,
}

impl Capturing {
    /// Binds the capture globals `conn` offers: a connection without the `capture` role offers none.
    pub(crate) fn bind(conn: &mut Connection) -> Capturing {
        let manager = conn.bind_extension::<ExtImageCopyCaptureManagerV1, _>(1..=1, Quiet);
        let bound = manager.is_some();
        let handles = Handles::default();
        Capturing {
            outputs: bound
                .then(|| {
                    conn.bind_extension::<ExtOutputImageCaptureSourceManagerV1, _>(1..=1, Quiet)
                })
                .flatten(),
            toplevels: bound
                .then(|| {
                    conn.bind_extension::<ExtForeignToplevelImageCaptureSourceManagerV1, _>(
                        1..=1,
                        Quiet,
                    )
                })
                .flatten(),
            list: bound
                .then(|| conn.bind_extension::<ExtForeignToplevelListV1, _>(1..=1, handles.clone()))
                .flatten()
                .map(|list| (list, handles)),
            shm: bound
                .then(|| conn.bind_extension::<WlShm, _>(1..=1, Quiet))
                .flatten(),
            manager,
            captures: HashMap::new(),
            revision: 0,
        }
    }

    /// Starts capture `key` of `source`; a capture that cannot start ends at once with why.
    pub(super) fn start(&mut self, conn: &Connection, key: Key, source: Source, options: Options) {
        if let Err(e) = self.try_start(conn, key, &source, options) {
            self.end(key, Err(e));
        }
    }

    fn try_start(
        &mut self,
        conn: &Connection,
        key: Key,
        source: &Source,
        options: Options,
    ) -> Result<(), Error> {
        let missing = |what: &str| Error::Protocol(format!("the server offers no {what}"));
        let manager = self
            .manager
            .as_ref()
            .ok_or_else(|| missing("screen capture"))?;
        let qh = conn.queue_handle();
        let (source, logical) = match source {
            Source::Output(output) => {
                let sources = self
                    .outputs
                    .as_ref()
                    .ok_or_else(|| missing("output capture"))?;
                let info = conn
                    .outputs()
                    .into_iter()
                    .find(|o| o.id == wire_output(*output));
                let wl = conn
                    .wl_output(wire_output(*output))
                    .ok_or_else(|| Error::Protocol("no such output".into()))?;
                let logical = info
                    .and_then(|o| o.geometry)
                    .map(|g| (f64::from(g.width), f64::from(g.height)));
                (sources.create_source(&wl, &qh, Quiet), logical)
            }
            Source::Window(identifier) => {
                let sources = self
                    .toplevels
                    .as_ref()
                    .ok_or_else(|| missing("window capture"))?;
                let handle = self
                    .handle(identifier)
                    .ok_or_else(|| Error::Protocol(format!("no window {identifier}")))?;
                (sources.create_source(&handle, &qh, Quiet), None)
            }
        };
        let mut wire = WireOptions::empty();
        if options.cursor {
            wire |= WireOptions::PaintCursors;
        }
        let session = manager.create_session(
            &source,
            wire,
            &qh,
            SessionData {
                key,
                constraints: Mutex::default(),
            },
        );
        if let Some(old) = self.captures.insert(
            key,
            Capture {
                session,
                source,
                options,
                logical,
                constraints: None,
                copy: None,
                wanted: true,
            },
        ) {
            destroy(old);
        }
        Ok(())
    }

    /// The toplevel handle the server gave `identifier`.
    fn handle(&self, identifier: &str) -> Option<ExtForeignToplevelHandleV1> {
        let (_, handles) = self.list.as_ref()?;
        let handles = handles.0.lock().unwrap_or_else(PoisonError::into_inner);
        handles
            .iter()
            .find(|h| {
                h.data::<HandleData>().is_some_and(|data| {
                    data.0
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_deref()
                        == Some(identifier)
                })
            })
            .cloned()
    }

    /// Stops followed capture `id`.
    pub(super) fn stop(&mut self, key: Key) {
        if let Some(capture) = self.captures.remove(&key) {
            destroy(capture);
        }
    }

    /// Takes `event` when it is one of the captures' and hands what it finished to the program; false for any other.
    pub(super) fn event(&mut self, event: &ExtensionEvent, inbox: &Inbox) -> bool {
        let Some(wire) = event.downcast_ref::<Wire>() else {
            return false;
        };
        match wire {
            Wire::Constraints { key, size, formats } => {
                if let Some(capture) = self.captures.get_mut(key) {
                    capture.constraints = Some((*size, formats.clone()));
                    capture.wanted = true;
                }
            }
            Wire::Ready(key) => {
                let read = self.captures.get_mut(key).and_then(|capture| {
                    let copy = capture.copy.take()?;
                    capture.wanted = true;
                    Some((read_copy(&copy, capture), copy))
                });
                if let Some((image, copy)) = read {
                    copy.frame.destroy();
                    copy.buffer.destroy();
                    self.revision += 1;
                    let image = image.map(|mut image| {
                        image.revision = self.revision;
                        image
                    });
                    self.deliver(*key, image, inbox);
                }
            }
            Wire::Failed(key) => {
                if let Some(copy) = self.captures.get_mut(key).and_then(|c| c.copy.take()) {
                    copy.frame.destroy();
                    copy.buffer.destroy();
                }
                if let Some(capture) = self.captures.get_mut(key) {
                    // a followed capture asks again once the server sends new constraints
                    capture.wanted = false;
                }
                if let Key::Once(_) = key {
                    self.end(
                        *key,
                        Err(Error::Protocol("the frame was not captured".into())),
                    );
                }
            }
            Wire::Stopped(key) => {
                self.end(*key, Err(Error::Protocol("the capture was stopped".into())));
            }
        }
        true
    }

    fn deliver(&mut self, key: Key, image: Result<ImageResource, Error>, inbox: &Inbox) {
        match key {
            Key::Once(_) => self.end(key, image),
            Key::Follow(follow) => match image {
                Ok(image) => inbox.push(Event::Capture(CaptureEvent::Frame { follow, image })),
                Err(e) => tracing::warn!(follow, error = %e, "a captured frame was not read"),
            },
        }
    }

    /// Ends capture `key`, a single one with `result`.
    fn end(&mut self, key: Key, result: Result<ImageResource, Error>) {
        if let Some(capture) = self.captures.remove(&key) {
            destroy(capture);
        }
        match key {
            Key::Once(id) => {
                let _ = context::with_ctx_mut(|ctx| ctx.captures.finish(id, result));
            }
            Key::Follow(follow) => {
                if let Err(e) = result {
                    tracing::warn!(follow, error = %e, "the followed capture ended");
                }
            }
        }
    }

    /// Asks for the frames the captures want, into buffers of the constraints the server gave.
    pub(super) fn pump(&mut self, conn: &Connection) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let qh = conn.queue_handle();
        let mut failed = Vec::new();
        for (key, capture) in &mut self.captures {
            if !capture.wanted || capture.copy.is_some() {
                continue;
            }
            let Some((size, formats)) = capture.constraints.clone() else {
                continue;
            };
            match new_copy(&shm, &qh, &capture.session, *key, size, &formats) {
                Ok(copy) => {
                    capture.wanted = false;
                    capture.copy = Some(copy);
                }
                Err(e) => failed.push((*key, e)),
            }
        }
        for (key, e) in failed {
            self.end(key, Err(e));
        }
    }
}

fn destroy(capture: Capture) {
    if let Some(copy) = capture.copy {
        copy.frame.destroy();
        copy.buffer.destroy();
    }
    capture.session.destroy();
    capture.source.destroy();
}

/// A buffer of `size` in one of `formats` the capture reads, and the frame that copies into it.
fn new_copy(
    shm: &WlShm,
    qh: &QueueHandle<State>,
    session: &ExtImageCopyCaptureSessionV1,
    key: Key,
    size: (u32, u32),
    formats: &[wl_shm::Format],
) -> Result<Copy, Error> {
    let format = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]
        .into_iter()
        .find(|f| formats.contains(f))
        .ok_or_else(|| Error::Protocol("no shared-memory format the capture reads".into()))?;
    let (width, height) = size;
    let stride = width
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| Error::Protocol("the frame is too wide".into()))?;
    let bytes = u64::from(stride) * u64::from(height);
    let len = i32::try_from(bytes).map_err(|_| Error::Protocol("the frame is too big".into()))?;
    let io = |e: std::io::Error| Error::Protocol(format!("capture buffer: {e}"));
    let fd = memfd_create("cupertino-capture", MemfdFlags::CLOEXEC).map_err(|e| io(e.into()))?;
    let file = File::from(fd);
    file.set_len(bytes).map_err(io)?;
    let pool = shm.create_pool(file.as_fd(), len, qh, Quiet);
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        format,
        qh,
        Quiet,
    );
    pool.destroy();
    let frame = session.create_frame(qh, FrameData(key));
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, width as i32, height as i32);
    frame.capture();
    Ok(Copy {
        frame,
        buffer,
        file,
        size,
        format,
    })
}

/// The copied frame as an image, cropped to the capture's region.
fn read_copy(copy: &Copy, capture: &Capture) -> Result<ImageResource, Error> {
    let (width, height) = copy.size;
    let stride = (width * BYTES_PER_PIXEL) as usize;
    let mut pixels = vec![0u8; stride * height as usize];
    copy.file
        .read_exact_at(&mut pixels, 0)
        .map_err(|e| Error::Protocol(format!("capture buffer: {e}")))?;
    let opaque = copy.format == wl_shm::Format::Xrgb8888;
    // `argb8888` is B, G, R, A in memory on a little-endian machine
    for pixel in pixels.as_chunks_mut::<{ BYTES_PER_PIXEL as usize }>().0 {
        pixel.swap(0, 2);
        if opaque {
            pixel[3] = u8::MAX;
        }
    }
    let (x, y, w, h) = crop(capture, copy.size);
    let mut cropped = Vec::with_capacity((w * h * BYTES_PER_PIXEL) as usize);
    for row in y..y + h {
        let start = row as usize * stride + (x * BYTES_PER_PIXEL) as usize;
        cropped.extend_from_slice(&pixels[start..start + (w * BYTES_PER_PIXEL) as usize]);
    }
    Ok(ImageResource {
        revision: 0,
        width: w,
        height: h,
        stride: w * BYTES_PER_PIXEL,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: cropped.into(),
    })
}

/// The buffer pixels of the capture's region, the whole frame without one.
fn crop(capture: &Capture, (width, height): (u32, u32)) -> (u32, u32, u32, u32) {
    let Some(region) = capture.options.region else {
        return (0, 0, width, height);
    };
    let scale = capture
        .logical
        .filter(|(w, _)| *w > 0.0)
        .map_or(1.0, |(w, _)| f64::from(width) / w);
    let clamp = |v: f64, max: u32| (v.max(0.0).round() as u32).min(max);
    let x = clamp(region.x() * scale, width);
    let y = clamp(region.y() * scale, height);
    let w = clamp(region.width() * scale, width - x)
        .max(1)
        .min(width - x);
    let h = clamp(region.height() * scale, height - y)
        .max(1)
        .min(height - y);
    (x, y, w, h)
}

macro_rules! quiet {
    ($($interface:ty => $events:ty),* $(,)?) => {$(
        impl Dispatch<$interface, Quiet> for State {
            fn event(_: &mut Self, _: &$interface, _: $events, _: &Quiet, _: &WlConnection, _: &QueueHandle<Self>) {}
        }
    )*};
}

quiet!(
    ExtImageCopyCaptureManagerV1 => manager::Event,
    ExtOutputImageCaptureSourceManagerV1 => output_sources::Event,
    ExtForeignToplevelImageCaptureSourceManagerV1 => toplevel_sources::Event,
    ExtImageCaptureSourceV1 => source::Event,
    WlShm => wl_shm::Event,
    WlShmPool => wl_shm_pool::Event,
    WlBuffer => wl_buffer::Event,
);

impl Dispatch<ExtForeignToplevelListV1, Handles> for State {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: list::Event,
        handles: &Handles,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let list::Event::Toplevel { toplevel } = event {
            handles
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(toplevel);
        }
    }

    wayland_client::event_created_child!(State, ExtForeignToplevelListV1, [
        list::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, HandleData::default()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, HandleData> for State {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelHandleV1,
        event: handle::Event,
        data: &HandleData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            handle::Event::Identifier { identifier } => {
                *data.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(identifier);
            }
            handle::Event::Closed => {
                *data.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, SessionData> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: session::Event,
        data: &SessionData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut constraints = data
            .constraints
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match event {
            session::Event::BufferSize { width, height } => constraints.0 = Some((width, height)),
            session::Event::ShmFormat {
                format: WEnum::Value(format),
            } => constraints.1.push(format),
            session::Event::Done => {
                let (size, formats) = std::mem::take(&mut *constraints);
                drop(constraints);
                if let Some(size) = size {
                    state.emit(Wire::Constraints {
                        key: data.key,
                        size,
                        formats,
                    });
                }
            }
            session::Event::Stopped => {
                drop(constraints);
                state.emit(Wire::Stopped(data.key));
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: frame::Event,
        data: &FrameData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            frame::Event::Ready => state.emit(Wire::Ready(data.0)),
            frame::Event::Failed { .. } => state.emit(Wire::Failed(data.0)),
            _ => {}
        }
    }
}
