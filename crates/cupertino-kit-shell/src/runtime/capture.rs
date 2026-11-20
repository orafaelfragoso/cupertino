//! Captures of outputs and windows for a program with the `capture` role (`shell-host.md#consent-and-capture`): each
//! capture is a session of `ext_image_copy_capture_v1` on the source the program names, into a buffer in shared
//! memory, read once its frame is ready. A single capture ([`crate::capture::once`]) ends with its first frame; a
//! followed one ([`crate::capture::follow`]) asks for the next frame after each.
//!
//! A ready frame is read on Kit's I/O pool ([`cupertino_kit::task::io`]), not on the UI thread: the pixels are copied
//! out of shared memory, put in the toolkit's byte order, cut to the capture's region and turned upright by the
//! transform the frame was sent in. The connection's waker wakes the loop when the read is done, and the next
//! [`Capturing::pump`] hands the image to the program; a capture asks for its next frame only after that, so its
//! frames arrive in order.

use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use cupertino_kit::core::Command;
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::image::OutputTransform;
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

/// How long reading a ready frame on the I/O pool should take: copying, reordering and turning the pixels of a large
/// output (about 33 million pixels at 8K) from memory. A read past it is reported as overdue and goes on.
const READ_DEADLINE: Duration = Duration::from_secs(2);

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
    /// The frame was copied, in a buffer holding the source's picture turned by `transform`.
    Ready {
        key: Key,
        transform: OutputTransform,
    },
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

/// User data of a frame: its capture, and the transform its buffer holds the picture in (`transform`, sent before
/// `ready`).
struct FrameData {
    key: Key,
    transform: Mutex<OutputTransform>,
}

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

/// The image a ready frame gives, read on the I/O pool.
type Reading = Pin<Box<dyn Future<Output = Result<ImageResource, Error>> + Send>>;

/// What a read of a ready frame needs, moved to the I/O pool.
struct Frame {
    file: File,
    size: (u32, u32),
    format: wl_shm::Format,
    transform: OutputTransform,
    region: Option<Rect>,
    /// The size of the source in logical points; see [`Capture::logical`].
    logical: Option<(f64, f64)>,
}

/// Wakes the connection's loop when a read finishes, so the next [`Capturing::pump`] takes its image.
struct LoopWaker(cupertino_wayland_client::Waker);

impl Wake for LoopWaker {
    fn wake(self: Arc<Self>) {
        self.0.wake();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.wake();
    }
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
    /// The read of the last ready frame, until it gives its image.
    reading: Option<Reading>,
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
    /// What a finished read wakes: the connection's loop.
    waker: Waker,
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
            waker: Waker::from(Arc::new(LoopWaker(conn.waker()))),
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
                reading: None,
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

    /// Takes `event` when it is one of the captures': a ready frame starts its read, which [`Capturing::pump`] hands
    /// to the program; false for any other.
    pub(super) fn event(&mut self, event: &ExtensionEvent) -> bool {
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
            Wire::Ready { key, transform } => {
                if let Some(capture) = self.captures.get_mut(key) {
                    if let Some(copy) = capture.copy.take() {
                        // the server wrote the frame: the pixels are in the file, which the read takes
                        copy.frame.destroy();
                        copy.buffer.destroy();
                        capture.reading = Some(start_read(Frame {
                            file: copy.file,
                            size: copy.size,
                            format: copy.format,
                            transform: *transform,
                            region: capture.options.region,
                            logical: capture.logical,
                        }));
                    }
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

    /// Hands the program the images of the reads that finished, then asks for the frames the captures want, into
    /// buffers of the constraints the server gave.
    pub(super) fn pump(&mut self, conn: &Connection, inbox: &Inbox) {
        self.take_reads(inbox);
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let qh = conn.queue_handle();
        let mut failed = Vec::new();
        for (key, capture) in &mut self.captures {
            if !capture.wanted || capture.copy.is_some() || capture.reading.is_some() {
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

    /// Delivers the image of every read that finished; a capture whose read finished wants its next frame.
    fn take_reads(&mut self, inbox: &Inbox) {
        let mut context = Context::from_waker(&self.waker);
        let mut read = Vec::new();
        for (key, capture) in &mut self.captures {
            let Some(reading) = capture.reading.as_mut() else {
                continue;
            };
            if let Poll::Ready(image) = reading.as_mut().poll(&mut context) {
                capture.reading = None;
                capture.wanted = true;
                read.push((*key, image));
            }
        }
        for (key, image) in read {
            self.revision += 1;
            let revision = self.revision;
            let image = image.map(|mut image| {
                image.revision = revision;
                image
            });
            self.deliver(key, image, inbox);
        }
    }
}

/// Starts reading `frame` on Kit's I/O pool; the read begins when the returned future is first polled.
fn start_read(frame: Frame) -> Reading {
    let Command::Perform(reading) =
        cupertino_kit::task::io(READ_DEADLINE, move || read(frame), |image| image)
    else {
        unreachable!("an I/O task is performed")
    };
    reading
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
    let frame = session.create_frame(
        qh,
        FrameData {
            key,
            transform: Mutex::new(OutputTransform::Normal),
        },
    );
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

/// The copied frame as an upright image of the capture's region: read from shared memory, in the toolkit's byte order,
/// the region's pixels cut from the buffer and turned upright. Runs on the I/O pool.
fn read(frame: Frame) -> Result<ImageResource, Error> {
    let stride = (frame.size.0 * BYTES_PER_PIXEL) as usize;
    let area = area_in_buffer(&frame)
        .ok_or_else(|| Error::Protocol("the region covers none of the source".into()))?;
    let (x, y, w, h) = area;
    let row = (w * BYTES_PER_PIXEL) as usize;
    let mut pixels = vec![0u8; row * h as usize];
    for (line, out) in (y..y + h).zip(pixels.chunks_exact_mut(row)) {
        let offset = line as usize * stride + (x * BYTES_PER_PIXEL) as usize;
        frame
            .file
            .read_exact_at(out, offset as u64)
            .map_err(|e| Error::Protocol(format!("capture buffer: {e}")))?;
    }
    let opaque = frame.format == wl_shm::Format::Xrgb8888;
    // `argb8888` is B, G, R, A in memory on a little-endian machine
    for pixel in pixels.as_chunks_mut::<{ BYTES_PER_PIXEL as usize }>().0 {
        pixel.swap(0, 2);
        if opaque {
            pixel[3] = u8::MAX;
        }
    }
    let cut = ImageResource {
        revision: 0,
        width: w,
        height: h,
        stride: w * BYTES_PER_PIXEL,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: pixels.into(),
    };
    if frame.transform == OutputTransform::Normal {
        return Ok(cut);
    }
    cupertino_kit::image::orient(&cut, frame.transform)
        .map_err(|e| Error::Protocol(format!("capture buffer: {e}")))
}

/// The pixels of the buffer (x, y, width, height) that the frame's region covers, the whole buffer without one. The
/// region is in the source's logical points, over its upright picture: its edges are rounded to whole upright pixels at
/// as many pixels per point as the upright picture has over the logical size (one without a logical size), cut to the
/// picture, and found in the buffer that holds the picture turned. `None` when nothing of the source is left.
fn area_in_buffer(frame: &Frame) -> Option<(u32, u32, u32, u32)> {
    let Some(region) = frame.region else {
        return Some((0, 0, frame.size.0, frame.size.1));
    };
    let (uw, uh) = frame.transform.turn_pixels(frame.size);
    let (uw, uh) = (f64::from(uw), f64::from(uh));
    let (lw, lh) = frame
        .logical
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .unwrap_or((uw, uh));
    let (sx, sy) = (uw / lw, uh / lh);
    let px = |v: f64, s: f64, max: f64| (v * s).round().clamp(0.0, max);
    let upright = Rect::from_ltrb(
        px(region.min_x(), sx, uw),
        px(region.min_y(), sy, uh),
        px(region.max_x(), sx, uw),
        px(region.max_y(), sy, uh),
    );
    if upright.width() < 1.0 || upright.height() < 1.0 {
        return None;
    }
    let r = frame.transform.buffer_rect(&upright, Size::new(uw, uh));
    Some((
        r.min_x() as u32,
        r.min_y() as u32,
        r.width() as u32,
        r.height() as u32,
    ))
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
        let mut transform = data
            .transform
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match event {
            frame::Event::Transform {
                transform: WEnum::Value(wire),
            } => {
                *transform = OutputTransform::from_wayland(u32::from(wire)).unwrap_or_default();
            }
            frame::Event::Ready => {
                let transform = *transform;
                state.emit(Wire::Ready {
                    key: data.key,
                    transform,
                });
            }
            frame::Event::Failed { .. } => state.emit(Wire::Failed(data.key)),
            _ => {}
        }
    }
}
