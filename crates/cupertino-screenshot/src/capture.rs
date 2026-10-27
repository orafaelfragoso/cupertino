//! The capture side of the connection: a second event queue on the process's connection, next to the toolkit's, with
//! the globals the toolkit does not bind. It copies outputs and windows into shm buffers
//! (`ext-image-copy-capture-v1` over `ext-image-capture-source-v1`), follows the windows on screen for the window pick
//! (`cupertino_screenshot_v1`), names windows for the toplevel capture (`ext-foreign-toplevel-list-v1`) and offers a
//! PNG on the clipboard (`wl_data_device`). The server offers the capture globals only on the connection it started
//! the process with.
//!
//! A capture is synchronous: the queue is dispatched until the frame is ready. Requests of both queues go out in the
//! order they were made, so a surface the toolkit destroyed before a capture is not in it.

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};

use cupertino_geometry::Rect;
use cupertino_protocols::client::screenshot::cupertino_screenshot_v1::{self, CupertinoScreenshotV1};
use cupertino_screenshot::image::{self, Format};
use cupertino_screenshot::pick::PickWindow;
use cupertino_wayland_client::OutputId;
use memmap2::Mmap;
use rustix::fs::{memfd_create, MemfdFlags};
use tiny_skia::Pixmap;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};
use wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// MIME type of the clipboard offer.
const PNG_MIME: &str = "image/png";

/// What a copy session announced before its first frame.
#[derive(Default)]
struct Constraints {
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    done: bool,
    stopped: bool,
}

/// How the frame in flight ended.
enum FrameEnd {
    Ready,
    Failed(String),
}

#[derive(Default)]
struct Toplevel {
    identifier: Option<String>,
    closed: bool,
}

struct State {
    shm: WlShm,
    output_sources: ExtOutputImageCaptureSourceManagerV1,
    toplevel_sources: ExtForeignToplevelImageCaptureSourceManagerV1,
    copy: ExtImageCopyCaptureManagerV1,
    _toplevel_list: ExtForeignToplevelListV1,
    toplevels: Vec<(ExtForeignToplevelHandleV1, Toplevel)>,
    _feed: CupertinoScreenshotV1,
    /// The group of window events being received, and the last complete one.
    feed_pending: Vec<PickWindow>,
    windows: Vec<PickWindow>,
    windows_changed: bool,
    seat: WlSeat,
    data_devices: WlDataDeviceManager,
    constraints: Constraints,
    frame: Option<FrameEnd>,
    /// The PNG offered on the clipboard, and whether another client's selection replaced it.
    clipboard: Option<Vec<u8>>,
    clipboard_cancelled: bool,
    /// A write to a pasting client that failed; the paste is lost, the offer stays.
    clipboard_error: Option<String>,
}

/// The capture queue and its state.
pub struct Capturer {
    globals: GlobalList,
    queue: EventQueue<State>,
    state: State,
}

/// Bind a global the capture needs; the server offers them all to the screenshot process.
fn bind<I: Proxy + 'static>(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    version: u32,
) -> Result<I>
where
    State: Dispatch<I, ()>,
{
    globals
        .bind::<I, State, ()>(qh, version..=version, ())
        .map_err(|e| {
            format!(
                "binding {}: {e} (was the process started by the server?)",
                I::interface().name
            )
            .into()
        })
}

impl Capturer {
    /// The capture queue on `conn`, with the window feed and the toplevel list read once.
    pub fn new(conn: &Connection) -> Result<Self> {
        let (globals, queue) = registry_queue_init::<State>(conn)?;
        let qh = queue.handle();
        let state = State {
            shm: bind(&globals, &qh, 1)?,
            output_sources: bind(&globals, &qh, 1)?,
            toplevel_sources: bind(&globals, &qh, 1)?,
            copy: bind(&globals, &qh, 1)?,
            _toplevel_list: bind(&globals, &qh, 1)?,
            toplevels: Vec::new(),
            _feed: bind(&globals, &qh, 1)?,
            feed_pending: Vec::new(),
            windows: Vec::new(),
            windows_changed: false,
            seat: globals.bind::<WlSeat, State, ()>(&qh, 1..=7, ())?,
            data_devices: globals.bind::<WlDataDeviceManager, State, ()>(&qh, 1..=3, ())?,
            constraints: Constraints::default(),
            frame: None,
            clipboard: None,
            clipboard_cancelled: false,
            clipboard_error: None,
        };
        let mut capturer = Self {
            globals,
            queue,
            state,
        };
        capturer.queue.roundtrip(&mut capturer.state)?;
        Ok(capturer)
    }

    /// Handle what arrived for this queue while the toolkit's queue was read.
    pub fn dispatch(&mut self) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }

    /// The windows on screen, front to back, when they changed since the last call.
    pub fn take_windows(&mut self) -> Option<Vec<PickWindow>> {
        std::mem::take(&mut self.state.windows_changed).then(|| self.state.windows.clone())
    }

    /// Copy the output `output` (its mode size in pixels).
    pub fn output(&mut self, output: OutputId, pointer: bool) -> Result<Pixmap> {
        let name = output.registry_name();
        let version = self
            .globals
            .contents()
            .with_list(|list| {
                list.iter()
                    .find(|g| g.name == name && g.interface == "wl_output")
                    .map(|g| g.version)
            })
            .ok_or("the output went away")?;
        let qh = self.queue.handle();
        let wl_output: WlOutput = self.globals.registry().bind(name, version.min(4), &qh, ());
        let source = self.state.output_sources.create_source(&wl_output, &qh, ());
        let image = self.copy_source(&source, pointer);
        source.destroy();
        if wl_output.version() >= 3 {
            wl_output.release();
        }
        image
    }

    /// Copy the window `identifier` (a foreign-toplevel identifier from the window feed), with its decorations.
    pub fn window(&mut self, identifier: &str, pointer: bool) -> Result<Pixmap> {
        let handle = self
            .state
            .toplevels
            .iter()
            .find(|(_, t)| !t.closed && t.identifier.as_deref() == Some(identifier))
            .map(|(h, _)| h.clone())
            .ok_or_else(|| format!("no window {identifier:?} is open"))?;
        let qh = self.queue.handle();
        let source = self.state.toplevel_sources.create_source(&handle, &qh, ());
        let image = self.copy_source(&source, pointer);
        source.destroy();
        image
    }

    /// One frame of `source` through a copy session, into a shm buffer read back as an image.
    fn copy_source(&mut self, source: &ExtImageCaptureSourceV1, pointer: bool) -> Result<Pixmap> {
        let qh = self.queue.handle();
        self.state.constraints = Constraints::default();
        self.state.frame = None;
        let options = if pointer {
            Options::PaintCursors
        } else {
            Options::empty()
        };
        let session = self.state.copy.create_session(source, options, &qh, ());
        while !self.state.constraints.done && !self.state.constraints.stopped {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        if self.state.constraints.stopped {
            session.destroy();
            return Err("the server stopped the capture (the output or window went away)".into());
        }
        let (width, height) = self
            .state
            .constraints
            .size
            .ok_or("the capture announced no buffer size")?;
        // a toplevel keeps its transparent corners with alpha; an output is opaque either way
        let (format, wl_format) = if self
            .state
            .constraints
            .formats
            .contains(&wl_shm::Format::Argb8888)
        {
            (Format::Argb8888, wl_shm::Format::Argb8888)
        } else if self
            .state
            .constraints
            .formats
            .contains(&wl_shm::Format::Xrgb8888)
        {
            (Format::Xrgb8888, wl_shm::Format::Xrgb8888)
        } else {
            session.destroy();
            return Err("the capture offers neither ARGB8888 nor XRGB8888 shm buffers".into());
        };
        let stride = width as usize * 4;
        let len = stride * height as usize;
        let fd: OwnedFd = memfd_create("cupertino-screenshot", MemfdFlags::CLOEXEC)?;
        let file = File::from(fd);
        file.set_len(len as u64)?;
        let pool = self
            .state
            .shm
            .create_pool(file.as_fd(), len as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_format,
            &qh,
            (),
        );
        let frame = session.create_frame(&qh, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, width as i32, height as i32);
        frame.capture();
        let end = loop {
            if let Some(end) = self.state.frame.take() {
                break end;
            }
            self.queue.blocking_dispatch(&mut self.state)?;
        };
        frame.destroy();
        session.destroy();
        buffer.destroy();
        pool.destroy();
        if let FrameEnd::Failed(reason) = end {
            return Err(format!("the capture failed: {reason}").into());
        }
        // SAFETY: the memfd is private to this process and the server, which wrote the frame before `ready`.
        let map = unsafe { Mmap::map(&file) }?;
        image::from_shm(width, height, stride, &map, format)
            .ok_or_else(|| format!("the {width} x {height} buffer does not hold its image").into())
    }

    /// Offer `png` as the clipboard selection. The server accepts it only while a surface of this connection has the
    /// keyboard focus, which the caller arranges; `serial` is that of the latest input event.
    pub fn offer_clipboard(&mut self, png: Vec<u8>, serial: u32) -> Result<()> {
        let qh = self.queue.handle();
        let source = self.state.data_devices.create_data_source(&qh, ());
        source.offer(PNG_MIME.into());
        let device = self
            .state
            .data_devices
            .get_data_device(&self.state.seat, &qh, ());
        device.set_selection(Some(&source), serial);
        self.state.clipboard = Some(png);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    /// Serve pastes until another selection replaces the offer, after which nobody can paste it. A paste that could
    /// not be written is reported and the offer stays.
    pub fn serve_clipboard(&mut self) -> Result<()> {
        while !self.state.clipboard_cancelled {
            self.queue.blocking_dispatch(&mut self.state)?;
            if let Some(e) = self.state.clipboard_error.take() {
                eprintln!("cupertino-screenshot: {e}");
            }
        }
        Ok(())
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // outputs are looked up in the global list when they are captured
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlOutput);
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: WlDataDeviceManager);
delegate_noop!(State: ignore WlDataOffer);
delegate_noop!(State: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(State: ExtImageCaptureSourceV1);
delegate_noop!(State: ExtImageCopyCaptureManagerV1);

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let c = &mut state.constraints;
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                c.size = Some((width, height))
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat {
                format: WEnum::Value(format),
            } => c.formats.push(format),
            ext_image_copy_capture_session_v1::Event::Done => c.done = true,
            ext_image_copy_capture_session_v1::Event::Stopped => c.stopped = true,
            // unknown shm formats and dma-buf constraints are not used
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => state.frame = Some(FrameEnd::Ready),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.frame = Some(FrameEnd::Failed(format!("{reason:?}")))
            }
            // the whole buffer is read, whatever the damage; transform and timing are not used
            _ => {}
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push((toplevel, Toplevel::default()));
        }
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(index) = state.toplevels.iter().position(|(h, _)| h == handle) else {
            return;
        };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                state.toplevels[index].1.identifier = Some(identifier)
            }
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                state.toplevels.remove(index).0.destroy();
            }
            // titles and app ids are not used
            _ => {}
        }
    }
}

impl Dispatch<CupertinoScreenshotV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoScreenshotV1,
        event: cupertino_screenshot_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_screenshot_v1::Event::Window {
                identifier,
                x,
                y,
                width,
                height,
            } => {
                state.feed_pending.push(PickWindow {
                    identifier,
                    frame: Rect::new(
                        f64::from(x),
                        f64::from(y),
                        f64::from(width),
                        f64::from(height),
                    ),
                });
            }
            cupertino_screenshot_v1::Event::Done => {
                state.windows = std::mem::take(&mut state.feed_pending);
                state.windows_changed = true;
            }
            _ => unreachable!("cupertino_screenshot_v1 version 1 has no other event"),
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        _: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // offers of other clients are not read; their objects are destroyed with the connection
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let Some(png) = state.clipboard.as_ref().filter(|_| mime_type == PNG_MIME) else {
                    return;
                };
                // the pasting client reads until the descriptor closes, which dropping the file does
                let mut file = File::from(fd);
                if let Err(e) = file.write_all(png) {
                    state.clipboard_error = Some(format!(
                        "writing the screenshot to a pasting client failed: {e}"
                    ));
                }
            }
            wl_data_source::Event::Cancelled => {
                source.destroy();
                state.clipboard_cancelled = true;
            }
            // drag-and-drop events do not apply to a selection source
            _ => {}
        }
    }
}
