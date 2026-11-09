//! The capture side of the process, on its connection to the server, which offers the capture globals only to the
//! connection it started the process with. It lists the displays and the windows (`ext-foreign-toplevel-list-v1`) for
//! the chooser, and shares each chosen source as a PipeWire stream: one `ext-image-copy-capture-v1` session on the
//! source, each frame copied straight into a buffer of the stream through `wl_shm` on the buffer's memfd. A screenshot
//! copies one frame of every display into a memfd of its own once it is due.
//!
//! A frame is asked for once the consumer takes frames and a buffer is free, at most at the negotiated rate. The server
//! answers a frame when the source changed, so a still screen sends nothing; the frame's damage goes on the buffer.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_geometry::OutputTransform as Transform;
use cupertino_screencast::{BufferId, Format, Frame, Notice, Offer, Pixel, Region, Screencast, Size, State, StreamId};
use cupertino_wayland_client::{Connection, Event, OutputId, OutputInfo, OutputTransform, State as Wayland, Waker};
use rustix::fs::{memfd_create, MemfdFlags};
use tiny_skia::Pixmap;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{event_created_child, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};
use wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1};
use zbus::zvariant::OwnedObjectPath;

use crate::geometry::{self, Placement};
use crate::screen_cast::{self, Capture as Request, CursorMode, Running, Source, Started, Stream};
use crate::screenshot::{self, Failure, Output, Shoot, Shot};

/// The name of every stream's node.
const NODE_NAME: &str = "cupertino-screencast";

/// User data of the objects whose events nobody reads.
pub struct Quiet;
/// User data of the toplevel list and its handles.
pub struct Listed;
/// User data of a share's copy session and frames.
pub struct Tag(StreamId);
/// User data of a screenshot's copy sessions and frames: the screenshot, and the display among its copies.
pub struct Part(u64, usize);

/// What the objects of the capture received, handed on by [`Wayland::emit`].
enum Wire {
    Toplevel(
        ExtForeignToplevelHandleV1,
        ext_foreign_toplevel_handle_v1::Event,
    ),
    NewToplevel(ExtForeignToplevelHandleV1),
    Session(StreamId, ext_image_copy_capture_session_v1::Event),
    Frame(StreamId, ext_image_copy_capture_frame_v1::Event),
    StillSession(u64, usize, ext_image_copy_capture_session_v1::Event),
    StillFrame(u64, usize, ext_image_copy_capture_frame_v1::Event),
}

/// A window as the toplevel list describes it; `done` once its first group of properties arrived.
struct Toplevel {
    handle: ExtForeignToplevelHandleV1,
    identifier: Option<String>,
    title: String,
    app_id: String,
    done: bool,
}

/// What a copy session announced since its last `done`.
#[derive(Default)]
struct Constraints {
    size: Option<Size>,
    formats: Vec<wl_shm::Format>,
}

/// A buffer of a stream as the server knows it.
struct Buffer {
    id: BufferId,
    pool: WlShmPool,
    wl: WlBuffer,
}

/// Where a share's next frame is.
enum Pacing {
    /// Waiting until `next`; `held` is a buffer already taken from the stream.
    Idle {
        next: Instant,
        held: Option<BufferId>,
    },
    /// A buffer was asked for.
    Dequeuing,
    /// The server is copying a frame into `buffer`.
    Copying {
        buffer: BufferId,
        frame: ExtImageCopyCaptureFrameV1,
        started: Instant,
        damage: Vec<Region>,
        pts: Option<Duration>,
    },
}

struct Share {
    id: StreamId,
    session: OwnedObjectPath,
    source: Source,
    capture_source: ExtImageCaptureSourceV1,
    copy: ExtImageCopyCaptureSessionV1,
    constraints: Constraints,
    /// What the stream was opened with or last resized to.
    offer: Option<Offer>,
    state: State,
    /// Answered once the stream's node is known.
    started: Option<Started>,
    /// The stream as `Start` reports it, without its node.
    shown: Stream,
    _running: Running,
    buffers: Vec<Buffer>,
    pacing: Pacing,
}

impl Share {
    fn streaming(&self) -> Option<Format> {
        match self.state {
            State::Streaming(format) => Some(format),
            _ => None,
        }
    }
}

/// A frame of a display being copied into `file`.
struct Copying {
    frame: ExtImageCopyCaptureFrameV1,
    file: File,
    pool: WlShmPool,
    buffer: WlBuffer,
    size: Size,
    alpha: bool,
}

/// One display of a screenshot.
struct OutputCopy {
    position: (i32, i32),
    size: (i32, i32),
    /// The transform the server applied to the buffer's content, from the frame's `transform` event.
    turn: Transform,
    source: ExtImageCaptureSourceV1,
    session: ExtImageCopyCaptureSessionV1,
    constraints: Constraints,
    copying: Option<Copying>,
    image: Option<Pixmap>,
}

/// A screenshot: waiting until `due` while it has no copies, then copying every display.
struct Still {
    id: u64,
    due: Instant,
    shot: Shot,
    copies: Vec<OutputCopy>,
}

/// Why a share ends.
enum End {
    /// The session was closed.
    Stopped,
    /// The source went away or failed, or PipeWire did.
    Failed(String),
}

pub struct Capture {
    shm: WlShm,
    output_sources: ExtOutputImageCaptureSourceManagerV1,
    toplevel_sources: ExtForeignToplevelImageCaptureSourceManagerV1,
    copy: ExtImageCopyCaptureManagerV1,
    _toplevel_list: ExtForeignToplevelListV1,
    toplevels: Vec<Toplevel>,
    listed: Arc<Mutex<Vec<Source>>>,
    requests: mpsc::Receiver<Request>,
    notices: mpsc::Receiver<Notice>,
    notify: mpsc::Sender<Notice>,
    waker: Waker,
    qh: QueueHandle<Wayland>,
    screencast: Option<Screencast>,
    shares: Vec<Share>,
    next: u64,
    shoots: mpsc::Receiver<Shoot>,
    stills: Vec<Still>,
}

impl Capture {
    /// Bind the capture globals on `conn`, which [`Request`]s reach through `requests` and screenshots through
    /// `shoots` (each followed by a wake of the connection).
    pub fn bind(
        conn: &mut Connection,
        requests: mpsc::Receiver<Request>,
        shoots: mpsc::Receiver<Shoot>,
    ) -> Result<Capture, String> {
        fn missing(name: &str) -> String {
            format!("the server offers no {name} (was the process started by the server?)")
        }
        let shm = conn
            .bind_extension(1..=1, Quiet)
            .ok_or_else(|| missing("wl_shm"))?;
        let output_sources = conn
            .bind_extension(1..=1, Quiet)
            .ok_or_else(|| missing("ext_output_image_capture_source_manager_v1"))?;
        let toplevel_sources = conn
            .bind_extension(1..=1, Quiet)
            .ok_or_else(|| missing("ext_foreign_toplevel_image_capture_source_manager_v1"))?;
        let copy = conn
            .bind_extension(1..=1, Quiet)
            .ok_or_else(|| missing("ext_image_copy_capture_manager_v1"))?;
        let toplevel_list = conn
            .bind_extension(1..=1, Listed)
            .ok_or_else(|| missing("ext_foreign_toplevel_list_v1"))?;
        let (notify, notices) = mpsc::channel();
        Ok(Capture {
            shm,
            output_sources,
            toplevel_sources,
            copy,
            _toplevel_list: toplevel_list,
            toplevels: Vec::new(),
            listed: Arc::default(),
            requests,
            notices,
            notify,
            waker: conn.waker(),
            qh: conn.queue_handle(),
            screencast: None,
            shares: Vec::new(),
            next: 0,
            shoots,
            stills: Vec::new(),
        })
    }

    /// The displays and the windows to choose from, kept current by [`Capture::update`].
    pub fn sources(&self) -> Arc<Mutex<Vec<Source>>> {
        Arc::clone(&self.listed)
    }

    /// How long the connection may wait before a share's next frame or a screenshot is due.
    pub fn timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        let stills = self
            .stills
            .iter()
            .filter(|s| s.copies.is_empty())
            .map(|s| s.due.saturating_duration_since(now));
        self.shares
            .iter()
            .filter(|s| s.streaming().is_some())
            .filter_map(|s| match s.pacing {
                Pacing::Idle { next, .. } => Some(next.saturating_duration_since(now)),
                _ => None,
            })
            .chain(stills)
            .min()
    }

    /// Follow what `events` (one wait of the connection), the D-Bus side and PipeWire brought, and ask for the frames
    /// that are due.
    pub fn update(&mut self, conn: &Connection, events: &[Event]) {
        for event in events {
            if let Event::Extension(event) = event {
                if let Some(wire) = event.downcast_ref::<Wire>() {
                    self.wire(wire);
                }
            }
        }
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Request::Start {
                    session,
                    source,
                    cursor,
                    started,
                    running,
                } => self.start(conn, session, source, cursor, started, running),
                Request::Stop { session } => {
                    if let Some(i) = self.shares.iter().position(|s| s.session == session) {
                        self.end(i, End::Stopped);
                    }
                }
            }
        }
        while let Ok(notice) = self.notices.try_recv() {
            self.notice(notice);
        }
        while let Ok(Shoot { due, shot }) = self.shoots.try_recv() {
            self.stills.push(Still {
                id: self.next,
                due,
                shot,
                copies: Vec::new(),
            });
            self.next += 1;
        }
        self.list(&conn.outputs());
        self.pace();
        self.shoot(conn);
        let _ = conn.flush();
    }

    fn list(&self, outputs: &[OutputInfo]) {
        let displays = outputs.iter().map(|o| Source::Display {
            output: o.id.registry_name(),
            name: o.name.clone().unwrap_or_default(),
            description: o.description.clone().unwrap_or_default(),
        });
        let windows = self.toplevels.iter().filter(|t| t.done).filter_map(|t| {
            Some(Source::Window {
                identifier: t.identifier.clone()?,
                title: t.title.clone(),
                app_id: t.app_id.clone(),
            })
        });
        let sources: Vec<Source> = displays.chain(windows).collect();
        let mut listed = self.listed.lock().unwrap_or_else(|e| e.into_inner());
        if *listed != sources {
            *listed = sources;
        }
    }

    fn start(
        &mut self,
        conn: &Connection,
        session: OwnedObjectPath,
        source: Source,
        cursor: CursorMode,
        started: Started,
        running: Running,
    ) {
        if self.screencast.as_ref().is_none_or(|s| !s.is_connected()) {
            let (notify, waker) = (self.notify.clone(), self.waker.clone());
            match Screencast::connect(move |notice| {
                let _ = notify.send(notice);
                waker.wake();
            }) {
                Ok(screencast) => self.screencast = Some(screencast),
                Err(e) => return started.send(Err(e.to_string())),
            }
        }
        let qh = conn.queue_handle();
        let (capture_source, shown) = match &source {
            Source::Display { output, .. } => {
                let id = OutputId::from_registry_name(*output);
                let (Some(wl_output), Some(info)) = (
                    conn.wl_output(id),
                    conn.outputs().into_iter().find(|o| o.id == id),
                ) else {
                    return started.send(Err("the display went away".to_owned()));
                };
                let Placement { position, size } = placement(&info);
                (
                    self.output_sources.create_source(&wl_output, &qh, Quiet),
                    Stream {
                        node: 0,
                        source_type: source.source_type(),
                        size,
                        position: Some(position),
                    },
                )
            }
            Source::Window { identifier, .. } => {
                let Some(toplevel) = self
                    .toplevels
                    .iter()
                    .find(|t| t.identifier.as_deref() == Some(identifier))
                else {
                    return started.send(Err("the window closed".to_owned()));
                };
                (
                    self.toplevel_sources
                        .create_source(&toplevel.handle, &qh, Quiet),
                    Stream {
                        node: 0,
                        source_type: source.source_type(),
                        size: (0, 0),
                        position: None,
                    },
                )
            }
        };
        let id = StreamId(self.next);
        self.next += 1;
        let options = match cursor {
            CursorMode::Embedded => Options::PaintCursors,
            CursorMode::Hidden => Options::empty(),
        };
        let copy = self
            .copy
            .create_session(&capture_source, options, &qh, Tag(id));
        self.shares.push(Share {
            id,
            session,
            source,
            capture_source,
            copy,
            constraints: Constraints::default(),
            offer: None,
            state: State::Connecting,
            started: Some(started),
            shown,
            _running: running,
            buffers: Vec::new(),
            pacing: Pacing::Idle {
                next: Instant::now(),
                held: None,
            },
        });
    }

    fn share(&mut self, id: StreamId) -> Option<usize> {
        self.shares.iter().position(|s| s.id == id)
    }

    fn wire(&mut self, wire: &Wire) {
        match wire {
            Wire::NewToplevel(handle) => self.toplevels.push(Toplevel {
                handle: handle.clone(),
                identifier: None,
                title: String::new(),
                app_id: String::new(),
                done: false,
            }),
            Wire::Toplevel(handle, event) => self.toplevel(handle, event),
            Wire::Session(id, event) => {
                if let Some(i) = self.share(*id) {
                    self.session_event(i, event);
                }
            }
            Wire::Frame(id, event) => {
                if let Some(i) = self.share(*id) {
                    self.frame_event(i, event);
                }
            }
            Wire::StillSession(id, n, event) => self.still_session(*id, *n, event),
            Wire::StillFrame(id, n, event) => self.still_frame(*id, *n, event),
        }
    }

    fn toplevel(
        &mut self,
        handle: &ExtForeignToplevelHandleV1,
        event: &ext_foreign_toplevel_handle_v1::Event,
    ) {
        let Some(i) = self.toplevels.iter().position(|t| t.handle == *handle) else {
            return;
        };
        let t = &mut self.toplevels[i];
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                t.identifier = Some(identifier.clone())
            }
            ext_foreign_toplevel_handle_v1::Event::Title { title } => t.title = title.clone(),
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => t.app_id = app_id.clone(),
            ext_foreign_toplevel_handle_v1::Event::Done => t.done = true,
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                self.toplevels.remove(i).handle.destroy();
            }
            _ => {}
        }
    }

    fn session_event(&mut self, i: usize, event: &ext_image_copy_capture_session_v1::Event) {
        let share = &mut self.shares[i];
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                share.constraints.size = Some(Size {
                    width: *width,
                    height: *height,
                })
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat {
                format: WEnum::Value(format),
            } => share.constraints.formats.push(*format),
            ext_image_copy_capture_session_v1::Event::Done => {
                let constraints = std::mem::take(&mut share.constraints);
                self.constrained(i, constraints);
            }
            ext_image_copy_capture_session_v1::Event::Stopped => {
                let why = match self.shares[i].source {
                    Source::Display { .. } => "the display went away",
                    Source::Window { .. } => "the window closed",
                };
                self.end(i, End::Failed(why.to_owned()));
            }
            // dma-buf constraints are not used: the buffers are shm
            _ => {}
        }
    }

    /// The copy session announced the buffers it takes: open the stream with them, or offer the new size.
    fn constrained(&mut self, i: usize, constraints: Constraints) {
        let share = &mut self.shares[i];
        let pixels = pixels(&share.source, &constraints.formats);
        let Some(size) = constraints.size.filter(|_| !pixels.is_empty()) else {
            return self.end(
                i,
                End::Failed(
                    "the capture offers neither XRGB8888 nor ARGB8888 shm buffers".to_owned(),
                ),
            );
        };
        if !size.shareable() {
            return self.end(
                i,
                End::Failed(format!(
                    "a {}x{} source is larger than a frame the stream can carry",
                    size.width, size.height
                )),
            );
        }
        let Some(screencast) = &self.screencast else {
            return;
        };
        match &mut share.offer {
            None => {
                if let Source::Window { .. } = share.source {
                    share.shown.size = (size.width as i32, size.height as i32);
                }
                let offer = Offer { size, pixels };
                screencast.open(share.id, NODE_NAME, offer.clone());
                share.offer = Some(offer);
            }
            Some(offer) if offer.size != size => {
                offer.size = size;
                screencast.resize(share.id, size);
            }
            Some(_) => {}
        }
    }

    fn frame_event(&mut self, i: usize, event: &ext_image_copy_capture_frame_v1::Event) {
        let share = &mut self.shares[i];
        let Pacing::Copying {
            buffer,
            frame,
            started,
            damage,
            pts,
        } = &mut share.pacing
        else {
            return;
        };
        match event {
            ext_image_copy_capture_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => damage.push(Region {
                x: *x,
                y: *y,
                width: (*width).max(0) as u32,
                height: (*height).max(0) as u32,
            }),
            ext_image_copy_capture_frame_v1::Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let secs = (u64::from(*tv_sec_hi) << 32) | u64::from(*tv_sec_lo);
                *pts = Some(Duration::new(secs, *tv_nsec));
            }
            // A stream's frames are not turned upright: they go to the stream as the server copied them. Cupertino's
            // server always copies a share's frames upright and reports them as not turned, so there is nothing to
            // undo; a server that sent turned frames would have them streamed turned. Screenshots and colour picks do
            // apply this event (`still_frame`).
            ext_image_copy_capture_frame_v1::Event::Transform { .. } => {}
            ext_image_copy_capture_frame_v1::Event::Ready => {
                frame.destroy();
                let ready = Frame {
                    damage: std::mem::take(damage),
                    pts: pts.unwrap_or_else(monotonic_now),
                };
                let (buffer, started) = (*buffer, *started);
                let interval = share
                    .streaming()
                    .map_or(Duration::ZERO, |f| f.framerate.interval());
                share.pacing = Pacing::Idle {
                    next: started + interval,
                    held: None,
                };
                if let Some(screencast) = &self.screencast {
                    screencast.queue(share.id, buffer, ready);
                }
            }
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                frame.destroy();
                let buffer = *buffer;
                if let WEnum::Value(FailureReason::Stopped) = reason {
                    return self.end(i, End::Failed("the capture stopped".to_owned()));
                }
                // a buffer that no longer fits waits for the constraints of the new size; anything else is tried again
                share.pacing = Pacing::Idle {
                    next: Instant::now(),
                    held: Some(buffer),
                };
            }
            _ => {}
        }
    }

    fn notice(&mut self, notice: Notice) {
        match notice {
            Notice::Node { stream, node } => {
                if let Some(i) = self.share(stream) {
                    let share = &mut self.shares[i];
                    if let Some(started) = share.started.take() {
                        started.send(Ok(Stream {
                            node,
                            ..share.shown
                        }));
                    }
                }
            }
            Notice::State { stream, state } => {
                let Some(i) = self.share(stream) else {
                    return;
                };
                if let State::Closed(reason) = &state {
                    return self.end(i, End::Failed(reason.clone()));
                }
                self.shares[i].state = state;
            }
            Notice::Added { stream, buffer, fd } => {
                let Some(i) = self.share(stream) else {
                    return;
                };
                let share = &mut self.shares[i];
                let format = match share.state {
                    State::Negotiated(format) | State::Streaming(format) => format,
                    State::Connecting | State::Closed(_) => return,
                };
                let qh = &self.qh;
                let pool = self
                    .shm
                    .create_pool(fd.as_fd(), format.frame_len() as i32, qh, Quiet);
                let wl = pool.create_buffer(
                    0,
                    format.width as i32,
                    format.height as i32,
                    format.stride() as i32,
                    wl_format(format.pixel),
                    qh,
                    Quiet,
                );
                share.buffers.push(Buffer {
                    id: buffer,
                    pool,
                    wl,
                });
            }
            Notice::Removed { stream, buffer } => {
                let Some(i) = self.share(stream) else {
                    return;
                };
                let share = &mut self.shares[i];
                if let Some(b) = share.buffers.iter().position(|b| b.id == buffer) {
                    let b = share.buffers.remove(b);
                    b.wl.destroy();
                    b.pool.destroy();
                }
                if let Pacing::Idle { held, .. } = &mut share.pacing {
                    if *held == Some(buffer) {
                        *held = None;
                    }
                }
            }
            Notice::Dequeued { stream, buffer } => {
                let Some(i) = self.share(stream) else {
                    return;
                };
                let share = &mut self.shares[i];
                if !matches!(share.pacing, Pacing::Dequeuing) {
                    return;
                }
                let interval = share
                    .streaming()
                    .map_or(Duration::ZERO, |f| f.framerate.interval());
                share.pacing = Pacing::Idle {
                    next: match buffer {
                        Some(_) => Instant::now(),
                        None => Instant::now() + interval,
                    },
                    held: buffer,
                };
            }
        }
    }

    /// Ask for every frame that is due.
    fn pace(&mut self) {
        let now = Instant::now();
        let Some(screencast) = &self.screencast else {
            return;
        };
        for share in &mut self.shares {
            let Some(format) = share.streaming() else {
                continue;
            };
            let Pacing::Idle { next, held } = share.pacing else {
                continue;
            };
            if next > now {
                continue;
            }
            let Some(buffer) = held.and_then(|id| share.buffers.iter().find(|b| b.id == id)) else {
                screencast.dequeue(share.id);
                share.pacing = Pacing::Dequeuing;
                continue;
            };
            let frame = share.copy.create_frame(&self.qh, Tag(share.id));
            frame.attach_buffer(&buffer.wl);
            frame.damage_buffer(0, 0, format.width as i32, format.height as i32);
            frame.capture();
            share.pacing = Pacing::Copying {
                buffer: buffer.id,
                frame,
                started: now,
                damage: Vec::new(),
                pts: None,
            };
        }
    }

    /// Open a copy session on every display for each screenshot that is due. One with no display to copy fails.
    fn shoot(&mut self, conn: &Connection) {
        let now = Instant::now();
        let qh = conn.queue_handle();
        let outputs = conn.outputs();
        for still in self
            .stills
            .iter_mut()
            .filter(|s| s.copies.is_empty() && s.due <= now)
        {
            for info in &outputs {
                let Some(wl_output) = conn.wl_output(info.id) else {
                    continue;
                };
                let Placement { position, size } = placement(info);
                let source = self.output_sources.create_source(&wl_output, &qh, Quiet);
                let part = Part(still.id, still.copies.len());
                let session = self
                    .copy
                    .create_session(&source, Options::empty(), &qh, part);
                still.copies.push(OutputCopy {
                    position,
                    size,
                    turn: Transform::Normal,
                    source,
                    session,
                    constraints: Constraints::default(),
                    copying: None,
                    image: None,
                });
            }
        }
        while let Some(i) = self
            .stills
            .iter()
            .position(|s| s.copies.is_empty() && s.due <= now)
        {
            self.end_still(
                i,
                Some(Failure::Lasting(
                    "there is no display to capture".to_owned(),
                )),
            );
        }
    }

    fn still(&self, id: u64) -> Option<usize> {
        self.stills.iter().position(|s| s.id == id)
    }

    fn still_session(
        &mut self,
        id: u64,
        n: usize,
        event: &ext_image_copy_capture_session_v1::Event,
    ) {
        let Some(i) = self.still(id) else {
            return;
        };
        let Some(copy) = self.stills[i].copies.get_mut(n) else {
            return;
        };
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                copy.constraints.size = Some(Size {
                    width: *width,
                    height: *height,
                })
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat {
                format: WEnum::Value(format),
            } => copy.constraints.formats.push(*format),
            ext_image_copy_capture_session_v1::Event::Done => {
                let constraints = std::mem::take(&mut copy.constraints);
                if copy.copying.is_some() || copy.image.is_some() {
                    return;
                }
                if let Err(reason) = copy_frame(&self.shm, &self.qh, copy, constraints, Part(id, n))
                {
                    self.end_still(i, Some(Failure::Lasting(reason)));
                }
            }
            ext_image_copy_capture_session_v1::Event::Stopped => {
                self.end_still(i, Some(Failure::Passing("a display went away".to_owned())))
            }
            _ => {}
        }
    }

    fn still_frame(&mut self, id: u64, n: usize, event: &ext_image_copy_capture_frame_v1::Event) {
        let Some(i) = self.still(id) else {
            return;
        };
        let Some(copy) = self.stills[i].copies.get_mut(n) else {
            return;
        };
        match event {
            ext_image_copy_capture_frame_v1::Event::Transform { transform } => {
                copy.turn = match transform {
                    WEnum::Value(t) => Transform::from_wayland(u32::from(*t)).unwrap_or_default(),
                    WEnum::Unknown(v) => Transform::from_wayland(*v).unwrap_or_default(),
                }
            }
            ext_image_copy_capture_frame_v1::Event::Ready => {
                let Some(copying) = copy.copying.take() else {
                    return;
                };
                copying.frame.destroy();
                let Size { width, height } = copying.size;
                let stride = width as usize * 4;
                let mut data = vec![0; stride * height as usize];
                let image = copying
                    .file
                    .read_exact_at(&mut data, 0)
                    .ok()
                    .and_then(|()| {
                        screenshot::from_shm(width, height, stride, &data, copying.alpha)
                    })
                    // a buffer the server handed out turned is read upright, as the display shows it
                    .and_then(|image| geometry::upright(&image, copy.turn));
                copying.buffer.destroy();
                copying.pool.destroy();
                let Some(image) = image else {
                    return self.end_still(
                        i,
                        Some(Failure::Lasting(
                            "a display's copy could not be read".to_owned(),
                        )),
                    );
                };
                copy.image = Some(image);
                if self.stills[i].copies.iter().all(|c| c.image.is_some()) {
                    self.end_still(i, None);
                }
            }
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                let why = format!("copying a display failed: {reason:?}");
                self.end_still(
                    i,
                    Some(match reason {
                        WEnum::Value(
                            ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints
                            | ext_image_copy_capture_frame_v1::FailureReason::Stopped,
                        ) => Failure::Passing(why),
                        _ => Failure::Lasting(why),
                    }),
                )
            }
            _ => {}
        }
    }

    /// End screenshot `i` and answer it: with every display's image, or with `failed`.
    fn end_still(&mut self, i: usize, failed: Option<Failure>) {
        let still = self.stills.remove(i);
        let mut outputs = Vec::new();
        for copy in still.copies {
            if let Some(copying) = copy.copying {
                copying.frame.destroy();
                copying.buffer.destroy();
                copying.pool.destroy();
            }
            copy.session.destroy();
            copy.source.destroy();
            if let Some(image) = copy.image {
                outputs.push(Output {
                    position: copy.position,
                    size: copy.size,
                    image,
                });
            }
        }
        still.shot.send(match failed {
            Some(failure) => Err(failure),
            None => Ok(outputs),
        });
    }

    /// End share `i`: its stream and copy session go, and so does the session it belongs to.
    fn end(&mut self, i: usize, end: End) {
        let share = self.shares.remove(i);
        if let End::Failed(reason) = &end {
            eprintln!(
                "cupertino-portal: the share of {} ended: {reason}",
                share.session
            );
            // a share that had not started yet is recorded where its Start learns it
            if share.started.is_none() {
                screen_cast::screencast_failed(None, "running", reason);
            }
        }
        if let Some(started) = share.started {
            started.send(Err(match end {
                End::Stopped => "the session was closed".to_owned(),
                End::Failed(reason) => reason,
            }));
        }
        if let Pacing::Copying { frame, .. } = share.pacing {
            frame.destroy();
        }
        share.copy.destroy();
        share.capture_source.destroy();
        for b in share.buffers {
            b.wl.destroy();
            b.pool.destroy();
        }
        if let Some(screencast) = &self.screencast {
            screencast.close(share.id);
        }
    }
}

/// The pixel layouts a source is shared in, among the `wl_shm` formats its copy session takes: a window keeps its
/// transparent corners with alpha first, a display is opaque first.
fn pixels(source: &Source, formats: &[wl_shm::Format]) -> Vec<Pixel> {
    let order = match source {
        Source::Display { .. } => [Pixel::Bgrx, Pixel::Bgra],
        Source::Window { .. } => [Pixel::Bgra, Pixel::Bgrx],
    };
    order
        .into_iter()
        .filter(|p| formats.contains(&wl_format(*p)))
        .collect()
}

/// Ask `copy`'s session for one frame, in a memfd buffer that fits the constraints it announced; an opaque format first.
fn copy_frame(
    shm: &WlShm,
    qh: &QueueHandle<Wayland>,
    copy: &mut OutputCopy,
    constraints: Constraints,
    part: Part,
) -> Result<(), String> {
    let size = constraints
        .size
        .ok_or("the capture announced no buffer size")?;
    let (format, alpha) = if constraints.formats.contains(&wl_shm::Format::Xrgb8888) {
        (wl_shm::Format::Xrgb8888, false)
    } else if constraints.formats.contains(&wl_shm::Format::Argb8888) {
        (wl_shm::Format::Argb8888, true)
    } else {
        return Err("the capture offers neither XRGB8888 nor ARGB8888 shm buffers".to_owned());
    };
    let stride = size.width as usize * 4;
    let len = stride
        .checked_mul(size.height as usize)
        .filter(|len| *len > 0 && i32::try_from(*len).is_ok())
        .ok_or_else(|| format!("a {}x{} display cannot be copied", size.width, size.height))?;
    let fd = memfd_create("cupertino-portal-screenshot", MemfdFlags::CLOEXEC)
        .map_err(|e| e.to_string())?;
    let file = File::from(fd);
    file.set_len(len as u64).map_err(|e| e.to_string())?;
    let pool = shm.create_pool(file.as_fd(), len as i32, qh, Quiet);
    let buffer = pool.create_buffer(
        0,
        size.width as i32,
        size.height as i32,
        stride as i32,
        format,
        qh,
        Quiet,
    );
    let frame = copy.session.create_frame(qh, part);
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, size.width as i32, size.height as i32);
    frame.capture();
    copy.copying = Some(Copying {
        frame,
        file,
        pool,
        buffer,
        size,
        alpha,
    });
    Ok(())
}

/// The transform of a display, as `wl_output` reports it.
fn turn(transform: OutputTransform) -> Transform {
    match transform {
        OutputTransform::Normal => Transform::Normal,
        OutputTransform::Rotated90 => Transform::Rotated90,
        OutputTransform::Rotated180 => Transform::Rotated180,
        OutputTransform::Rotated270 => Transform::Rotated270,
        OutputTransform::Flipped => Transform::Flipped,
        OutputTransform::Flipped90 => Transform::Flipped90,
        OutputTransform::Flipped180 => Transform::Flipped180,
        OutputTransform::Flipped270 => Transform::Flipped270,
    }
}

/// Where a display sits in the compositor's space: its logical geometry, else its mode upright over its scale at the
/// origin, so a quarter-turned display is never taken as wide when it shows tall.
pub fn placement(info: &OutputInfo) -> Placement {
    match info.geometry {
        Some(g) => Placement {
            position: (g.x, g.y),
            size: (g.width as i32, g.height as i32),
        },
        None => Placement {
            position: (0, 0),
            size: geometry::logical_size(
                info.mode.unwrap_or_default(),
                turn(info.transform),
                info.scale.factor(),
            ),
        },
    }
}

fn wl_format(pixel: Pixel) -> wl_shm::Format {
    match pixel {
        Pixel::Bgrx => wl_shm::Format::Xrgb8888,
        Pixel::Bgra => wl_shm::Format::Argb8888,
    }
}

fn monotonic_now() -> Duration {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}

macro_rules! quiet {
    ($($interface:ty),* $(,)?) => {$(
        impl Dispatch<$interface, Quiet> for Wayland {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as Proxy>::Event,
                _: &Quiet,
                _: &wayland_client::Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

quiet!(
    WlShm,
    WlShmPool,
    WlBuffer,
    ExtOutputImageCaptureSourceManagerV1,
    ExtForeignToplevelImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1,
);

impl Dispatch<ExtForeignToplevelListV1, Listed> for Wayland {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &Listed,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.emit(Wire::NewToplevel(toplevel));
        }
    }

    event_created_child!(Wayland, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, Listed),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, Listed> for Wayland {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &Listed,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        state.emit(Wire::Toplevel(handle.clone(), event));
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, Tag> for Wayland {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        tag: &Tag,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        state.emit(Wire::Session(tag.0, event));
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Tag> for Wayland {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        tag: &Tag,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        state.emit(Wire::Frame(tag.0, event));
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, Part> for Wayland {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        part: &Part,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        state.emit(Wire::StillSession(part.0, part.1, event));
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Part> for Wayland {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        part: &Part,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        state.emit(Wire::StillFrame(part.0, part.1, event));
    }
}

#[cfg(test)]
mod tests {
    use cupertino_wayland_client::{OutputGeometry, Scale};

    use super::*;

    #[test]
    fn a_display_is_shared_opaque_first_and_a_window_with_alpha_first() {
        let display = Source::Display {
            output: 1,
            name: "DP-1".to_owned(),
            description: String::new(),
        };
        let window = Source::Window {
            identifier: "a".to_owned(),
            title: String::new(),
            app_id: String::new(),
        };
        let both = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888];
        assert_eq!(pixels(&display, &both), [Pixel::Bgrx, Pixel::Bgra]);
        assert_eq!(pixels(&window, &both), [Pixel::Bgra, Pixel::Bgrx]);
        assert_eq!(pixels(&window, &[wl_shm::Format::Xrgb8888]), [Pixel::Bgrx]);
        assert!(pixels(&display, &[wl_shm::Format::Rgb565]).is_empty());
    }

    fn info(transform: OutputTransform, geometry: Option<OutputGeometry>) -> OutputInfo {
        OutputInfo {
            id: OutputId::from_registry_name(1),
            name: Some("DP-1".to_owned()),
            description: None,
            make: None,
            model: None,
            physical_size_mm: None,
            transform,
            scale: Scale::from_integer(2).unwrap(),
            mode: Some((3840, 2160)),
            refresh_mhz: None,
            geometry,
        }
    }

    #[test]
    fn a_display_sits_by_its_logical_geometry_else_by_its_mode_turned_over_its_scale() {
        let at = OutputGeometry {
            x: 1920,
            y: 0,
            width: 1080,
            height: 1920,
        };
        assert_eq!(
            placement(&info(OutputTransform::Rotated90, Some(at))),
            Placement {
                position: (1920, 0),
                size: (1080, 1920)
            }
        );
        for (transform, size) in [
            (OutputTransform::Normal, (1920, 1080)),
            (OutputTransform::Rotated90, (1080, 1920)),
            (OutputTransform::Rotated180, (1920, 1080)),
            (OutputTransform::Flipped270, (1080, 1920)),
        ] {
            assert_eq!(
                placement(&info(transform, None)),
                Placement {
                    position: (0, 0),
                    size
                },
                "{transform:?}"
            );
        }
    }
}
