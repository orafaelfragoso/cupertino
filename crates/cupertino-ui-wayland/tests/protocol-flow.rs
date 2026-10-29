//! Runs the presenter and the text clipboard on the transport against a minimal in-process server (shm, an output at
//! scale 2, a seat with a pointer, one window, the clipboard) and checks what the server receives.

use cupertino_geometry::Rect;
use cupertino_ui_wayland::clipboard::Clipboard;
use cupertino_ui_wayland::{rgba_to_argb8888, Ready, ShmPresenter};
use cupertino_wayland_client::{
    Connection, Decorations, Event, PointerEvent, Scale, Toplevel, ToplevelConfig,
};
use memmap2::Mmap;
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_data_device::{self, WlDataDevice};
use wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::{self, WlDataSource};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client as ServerClient, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};

/// The size the window asks for; the server leaves the size to it.
const SIZE: (u32, u32) = (100, 50);

/// What the server saw in one surface commit that carried a buffer.
#[derive(Debug, PartialEq)]
struct Committed {
    buffer_size: (i32, i32),
    stride: i32,
    /// The protocol id of the committed `wl_buffer`.
    buffer: u32,
    buffer_scale: i32,
    first_pixel: [u8; 4],
    pixels: Pixels,
    /// The `damage_buffer` rectangles of the commit (x, y, width, height).
    damage: Vec<[i32; 4]>,
}

/// The bytes of a committed buffer.
#[derive(Clone, PartialEq)]
struct Pixels(Vec<u8>);

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({} bytes)", self.0.len())
    }
}

enum Control {
    /// Answer the frame callbacks of the committed surface.
    FrameDone,
    /// Enter the surface with the pointer, move, and press the left button.
    PointerPress,
    /// Another client's text becomes the selection: offer it to the client's data device.
    OfferSelection(String),
    /// Keep every buffer the next commits replace.
    HoldBuffers,
    /// Release the held buffers and release replaced ones again.
    ReleaseBuffers,
}

/// A request the server saw that the commit record does not cover.
#[derive(Debug, PartialEq)]
enum Seen {
    /// The client set the selection with this serial, offering these types.
    Selection { serial: u32, types: Vec<String> },
    /// What the client wrote when the server asked its selection for the first type (a paste elsewhere).
    Served(String),
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    damage: Vec<[i32; 4]>,
    scale: i32,
    frames: Vec<WlCallback>,
}

type SurfaceData = Arc<Mutex<SurfaceState>>;

struct BufferData {
    map: Arc<Mmap>,
    offset: i32,
    width: i32,
    height: i32,
    stride: i32,
}

/// The window of the test client.
struct ToplevelState {
    surface: WlSurface,
    xdg: XdgSurface,
    toplevel: XdgToplevel,
    /// The initial configure has been sent.
    configured: bool,
}

struct Mock {
    committed: Sender<Committed>,
    seen: Sender<Seen>,
    next_serial: u32,
    outputs: Vec<WlOutput>,
    pointer: Option<WlPointer>,
    pending_frames: Vec<WlCallback>,
    shown: Option<WlBuffer>,
    /// Replaced buffers kept unreleased, while holding.
    held: Option<Vec<WlBuffer>>,
    toplevel: Option<ToplevelState>,
    data_device: Option<WlDataDevice>,
    /// The read end of a paste from the client's selection, and what arrived so far.
    pasting: Option<(std::io::PipeReader, Vec<u8>)>,
}

struct Data;
impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

/// A global whose bind only creates the resource.
macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for Mock {
            fn bind(_: &mut Self, _: &DisplayHandle, _: &ServerClient, r: New<$interface>, _: &(), init: &mut DataInit<'_, Self>) {
                init.init(r, ());
            }
        }
    )*};
}
plain_global!(WlCompositor, WlShm, WlDataDeviceManager);

/// Objects whose requests the server ignores.
macro_rules! ignore_requests {
    ($($interface:ty: $data:ty),*) => {$(
        impl Dispatch<$interface, $data> for Mock {
            fn request(_: &mut Self, _: &ServerClient, _: &$interface, _: <$interface as Resource>::Request, _: &$data, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
    )*};
}
ignore_requests!(
    WlCallback: (),
    WlBuffer: BufferData,
    WlOutput: (),
    WlPointer: (),
    XdgToplevel: ()
);

impl Dispatch<WlCompositor, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            init.init(id, SurfaceData::default());
        }
    }
}

impl Dispatch<WlSurface, SurfaceData> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        surface: &WlSurface,
        request: wl_surface::Request,
        data: &SurfaceData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let mut s = data.lock().unwrap();
        match request {
            wl_surface::Request::Attach { buffer, .. } => s.buffer = buffer,
            wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => s.damage.push([x, y, width, height]),
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::Frame { callback } => s.frames.push(init.init(callback, ())),
            wl_surface::Request::Commit => {
                let unbuffered = s.buffer.is_none();
                if let Some(t) = state
                    .toplevel
                    .as_mut()
                    .filter(|t| !t.configured && unbuffered && t.surface == *surface)
                {
                    // initial commit of the window: place it on the output and leave the size to the client
                    t.configured = true;
                    surface.enter(&state.outputs[0]);
                    state.next_serial += 1;
                    t.toplevel.configure(0, 0, Vec::new());
                    t.xdg.configure(state.next_serial);
                }
                if let Some(buffer) = s.buffer.take() {
                    let b = buffer.data::<BufferData>().expect("buffer data");
                    let start = b.offset as usize;
                    let first_pixel = b.map[start..start + 4].try_into().unwrap();
                    state
                        .committed
                        .send(Committed {
                            buffer_size: (b.width, b.height),
                            stride: b.stride,
                            buffer: buffer.id().protocol_id(),
                            buffer_scale: s.scale,
                            first_pixel,
                            pixels: Pixels(
                                b.map[start..start + (b.stride * b.height) as usize].to_vec(),
                            ),
                            damage: std::mem::take(&mut s.damage),
                        })
                        .unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    if let Some(previous) = state.shown.replace(buffer) {
                        match &mut state.held {
                            Some(held) => held.push(previous),
                            None => previous.release(),
                        }
                    }
                    state.pending_frames.append(&mut s.frames);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlShm, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlShm,
        request: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, .. } = request {
            // SAFETY: the client sized the memfd before sending it and keeps it that size.
            let map = unsafe { Mmap::map(&std::fs::File::from(fd)) }.unwrap();
            init.init(id, Arc::new(map));
        }
    }
}
impl Dispatch<WlShmPool, Arc<Mmap>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        map: &Arc<Mmap>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            offset,
            width,
            height,
            stride,
            format,
        } = request
        {
            assert_eq!(format.into_result().ok(), Some(wl_shm::Format::Argb8888));
            init.init(
                id,
                BufferData {
                    map: map.clone(),
                    offset,
                    width,
                    height,
                    stride,
                },
            );
        }
    }
}

impl GlobalDispatch<WlOutput, ()> for Mock {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlOutput>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(r, ());
        output.scale(2);
        output.done();
        state.outputs.push(output);
    }
}

impl GlobalDispatch<WlSeat, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ()).capabilities(Capability::Pointer);
    }
}
impl Dispatch<WlSeat, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            state.pointer = Some(init.init(id, ()));
        }
    }
}

impl GlobalDispatch<XdgWmBase, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<XdgWmBase>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
    }
}
impl Dispatch<XdgWmBase, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_wm_base::Request::GetXdgSurface { id, surface } = request {
            init.init(id, surface);
        }
    }
}
impl Dispatch<XdgSurface, WlSurface> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        xdg: &XdgSurface,
        request: xdg_surface::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            let toplevel = init.init(id, ());
            state.toplevel = Some(ToplevelState {
                surface: surface.clone(),
                xdg: xdg.clone(),
                toplevel,
                configured: false,
            });
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                init.init(id, Mutex::new(Vec::new()));
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                state.data_device = Some(init.init(id, ()));
            }
            _ => {}
        }
    }
}
impl Dispatch<WlDataSource, Mutex<Vec<String>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlDataSource,
        request: wl_data_source::Request,
        types: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_source::Request::Offer { mime_type } = request {
            types.lock().unwrap().push(mime_type);
        }
    }
}
impl Dispatch<WlDataDevice, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataDevice,
        request: wl_data_device::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_device::Request::SetSelection {
            source: Some(source),
            serial,
        } = request
        {
            let types = source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .clone();
            // paste it at once, as another client would
            let (reader, writer) = std::io::pipe().unwrap();
            source.send(types[0].clone(), writer.as_fd());
            state.pasting = Some((reader, Vec::new()));
            state.seen.send(Seen::Selection { serial, types }).unwrap();
        }
    }
}
impl Dispatch<WlDataOffer, String> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlDataOffer,
        request: wl_data_offer::Request,
        text: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_offer::Request::Receive { fd, .. } = request {
            std::fs::File::from(fd).write_all(text.as_bytes()).unwrap();
        }
    }
}

/// Run the server on a thread until the test drops the control channel.
fn spawn_server(stream: UnixStream) -> (Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    let (control_tx, control_rx) = channel::<Control>();
    let (committed_tx, committed_rx) = channel();
    let (seen_tx, seen_rx) = channel();
    std::thread::spawn(move || {
        let mut display = Display::<Mock>::new().unwrap();
        let dh = display.handle();
        dh.create_global::<Mock, WlCompositor, _>(6, ());
        dh.create_global::<Mock, WlShm, _>(1, ());
        dh.create_global::<Mock, WlOutput, _>(4, ());
        dh.create_global::<Mock, WlSeat, _>(7, ());
        dh.create_global::<Mock, XdgWmBase, _>(6, ());
        dh.create_global::<Mock, WlDataDeviceManager, _>(3, ());
        let mut mock = Mock {
            committed: committed_tx,
            seen: seen_tx,
            next_serial: 1,
            outputs: Vec::new(),
            pointer: None,
            pending_frames: Vec::new(),
            shown: None,
            held: None,
            toplevel: None,
            data_device: None,
            pasting: None,
        };
        let mut display_handle = display.handle();
        display_handle
            .insert_client(stream, Arc::new(Data))
            .unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(Control::FrameDone) => mock.pending_frames.drain(..).for_each(|cb| cb.done(0)),
                Ok(Control::HoldBuffers) => mock.held = Some(Vec::new()),
                Ok(Control::ReleaseBuffers) => mock
                    .held
                    .take()
                    .into_iter()
                    .flatten()
                    .for_each(|b| b.release()),
                Ok(Control::PointerPress) => {
                    let (pointer, t) = (
                        mock.pointer.as_ref().unwrap(),
                        mock.toplevel.as_ref().unwrap(),
                    );
                    pointer.enter(5, &t.surface, 1.5, 2.5);
                    pointer.motion(0, 10.25, 20.75);
                    pointer.button(6, 0, 0x110, wl_pointer::ButtonState::Pressed);
                }
                Ok(Control::OfferSelection(text)) => {
                    let device = mock.data_device.as_ref().unwrap();
                    let offer = device
                        .client()
                        .unwrap()
                        .create_resource::<WlDataOffer, String, Mock>(
                            &display_handle,
                            device.version(),
                            text,
                        )
                        .unwrap();
                    device.data_offer(&offer);
                    offer.offer("text/plain;charset=utf-8".into());
                    device.selection(Some(&offer));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            display.dispatch_clients(&mut mock).unwrap();
            display.flush_clients().unwrap();
            if let Some((reader, bytes)) = mock.pasting.as_mut() {
                let mut fds = [PollFd::new(&*reader, PollFlags::IN)];
                if poll(&mut fds, Some(&Timespec::default())).unwrap() > 0 {
                    let mut chunk = [0u8; 256];
                    match reader.read(&mut chunk).unwrap() {
                        0 => {
                            let text = String::from_utf8(std::mem::take(bytes)).unwrap();
                            mock.seen.send(Seen::Served(text)).unwrap();
                            mock.pasting = None;
                        }
                        n => bytes.extend_from_slice(&chunk[..n]),
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (control_tx, committed_rx, seen_rx)
}

/// What the client loop reports: a transport event, or what the presenter asks for after the events of a wait.
#[derive(Debug)]
enum Report {
    Event(Event),
    Ready(Ready),
}

/// The application side: one window drawn by a presenter, and the clipboard.
struct App {
    conn: Connection,
    window: Toplevel,
    presenter: ShmPresenter,
    clipboard: Clipboard,
    /// Reports the test has not looked at yet.
    stash: VecDeque<Report>,
}

impl App {
    /// Connect and create the window.
    fn connect(stream: UnixStream) -> App {
        let mut conn = Connection::from_stream(stream).unwrap();
        let window = conn
            .toplevel(&ToplevelConfig {
                app_id: "org.example.Test".into(),
                title: "Test".into(),
                min_size: None,
                max_size: None,
                decorations: Decorations::Client,
            })
            .unwrap();
        App {
            presenter: ShmPresenter::new(window.surface()),
            conn,
            window,
            clipboard: Clipboard::default(),
            stash: VecDeque::new(),
        }
    }

    /// One wait of the transport: its events, with the presenter and the clipboard told, then the presenter's polls.
    fn wait(&mut self, timeout: Duration) -> Vec<Report> {
        let mut reports = Vec::new();
        for event in self.conn.wait(Some(timeout)).unwrap() {
            match &event {
                Event::Configure { window, configure } if *window == self.window => {
                    self.presenter.configured(configure.size.unwrap_or(SIZE))
                }
                Event::Frame(callback) if callback.surface == self.window.surface() => {
                    self.presenter.frame_done()
                }
                Event::Selection(offer) => self.clipboard.selection_changed(offer.clone()),
                _ => {}
            }
            reports.push(Report::Event(event));
        }
        loop {
            match self.presenter.poll(&self.conn).unwrap() {
                Ready::Idle => break,
                ready => reports.push(Report::Ready(ready)),
            }
        }
        reports
    }

    /// Send what was drawn: the wait flushes the requests.
    fn flush(&mut self) {
        let reports = self.wait(Duration::from_millis(50));
        self.stash.extend(reports);
    }

    /// Take reports in order, waiting for more when the stash runs dry, until `pick` accepts one.
    fn wait_for<T>(&mut self, mut pick: impl FnMut(&Report) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            while let Some(report) = self.stash.pop_front() {
                if let Some(found) = pick(&report) {
                    return found;
                }
            }
            let reports = self.wait(Duration::from_millis(50));
            self.stash.extend(reports);
        }
        panic!("timed out waiting for a report");
    }

    fn wait_for_draw(&mut self) {
        self.wait_for(|r| matches!(r, Report::Ready(Ready::Draw)).then_some(()));
    }
}

/// Connect, map the window once and give the client an input serial (6).
fn connected_window() -> (App, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);
    app.wait_for_draw();
    control.send(Control::PointerPress).unwrap();
    app.wait_for(|r| {
        matches!(
            r,
            Report::Event(Event::Pointer {
                event: PointerEvent::Button { .. },
                ..
            })
        )
        .then_some(())
    });
    (app, control, committed, seen)
}

#[test]
fn a_frame_is_drawn_at_the_output_scale_in_wl_shm_byte_order_and_the_next_waits_for_the_frame_callback(
) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, _seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);

    // the window asked for 100x50; the output is at scale 2, so the first draw is 200x100
    app.wait_for(|r| {
        matches!(r, Report::Ready(Ready::ScaleChanged(scale)) if *scale == Scale::from_integer(2).unwrap())
            .then_some(())
    });
    app.wait_for_draw();
    app.presenter
        .draw(&mut app.conn, None, |pixmap, scale| {
            assert_eq!(
                (pixmap.width(), pixmap.height(), scale),
                (200, 100, Scale::from_integer(2).unwrap())
            );
            // premultiplied RGBA (10, 20, 30, 40)
            pixmap.pixels_mut()[0] =
                tiny_skia::PremultipliedColorU8::from_rgba(10, 20, 30, 40).unwrap();
        })
        .unwrap();

    // the frame arrives whole, in wl_shm byte order (B, G, R, A)
    app.flush();
    let first = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        first,
        Committed {
            buffer_size: (200, 100),
            stride: 800,
            buffer: first.buffer,
            buffer_scale: 2,
            first_pixel: [30, 20, 10, 40],
            pixels: first.pixels.clone(),
            damage: vec![[0, 0, i32::MAX, i32::MAX]],
        }
    );

    // no new frame is offered before the server answered the frame callback
    app.presenter.request_redraw();
    let early = app.wait(Duration::from_millis(100));
    assert!(
        !early
            .iter()
            .any(|r| matches!(r, Report::Ready(Ready::Draw))),
        "{early:?}"
    );
    control.send(Control::FrameDone).unwrap();
    app.wait_for_draw();

    // the second frame uses another buffer while the server holds the first
    app.presenter.draw(&mut app.conn, None, |_, _| {}).unwrap();
    app.flush();
    let second = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(second.buffer, first.buffer);
    assert_eq!(second.first_pixel, [0, 0, 0, 0]);
}

#[test]
fn a_paste_reads_another_clients_text_selection() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, _committed, _seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);
    assert_eq!(app.clipboard.text(&mut app.conn).unwrap(), None);
    control
        .send(Control::OfferSelection("from elsewhere".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        assert!(Instant::now() < deadline, "the selection never arrived");
        app.wait(Duration::from_millis(20));
        if let Some(text) = app.clipboard.text(&mut app.conn).unwrap() {
            break text;
        }
    };
    assert_eq!(text, "from elsewhere");
}

#[test]
fn copied_text_is_offered_with_the_input_serial_and_served_to_a_paste() {
    let (mut app, _control, _committed, seen) = connected_window();
    app.clipboard
        .set_text(&mut app.conn, "copied text".into())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while !events.iter().any(|e| matches!(e, Seen::Served(_))) {
        assert!(
            Instant::now() < deadline,
            "the paste was never served: {events:?}"
        );
        app.wait(Duration::from_millis(20));
        events.extend(seen.try_iter());
    }
    assert_eq!(
        events,
        vec![
            Seen::Selection {
                serial: 6,
                types: vec![
                    "text/plain;charset=utf-8".into(),
                    "text/plain".into(),
                    "UTF8_STRING".into()
                ],
            },
            Seen::Served("copied text".into()),
        ]
    );
    assert_eq!(
        app.clipboard.text(&mut app.conn).unwrap().as_deref(),
        Some("copied text"),
        "a paste in the copying client reads its own text"
    );
}

#[test]
fn a_damaged_frame_keeps_the_last_one_repaints_its_rectangles_and_reports_only_them() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let red = tiny_skia::Color::from_rgba8(255, 0, 0, 255);
    let blue = tiny_skia::Color::from_rgba8(0, 0, 255, 255);
    app.presenter
        .draw(&mut app.conn, None, |pixmap, _| pixmap.fill(red))
        .unwrap();
    app.flush();
    let mut frames = vec![committed.recv_timeout(Duration::from_secs(5)).unwrap()];
    // a rectangle in points; the output is at scale 2
    let rect = Rect::new(10.0, 5.25, 4.0, 3.0);
    for damage in [vec![rect], vec![]] {
        app.presenter.request_redraw();
        control.send(Control::FrameDone).unwrap();
        app.wait_for_draw();
        app.presenter
            .draw(&mut app.conn, Some(&damage), |pixmap, _| {
                let at = |x, y| pixmap.pixel(x, y).unwrap().demultiply();
                assert_eq!(
                    at(0, 0),
                    red.to_color_u8(),
                    "outside the damage: the last frame"
                );
                if !damage.is_empty() {
                    assert_eq!(at(20, 10).alpha(), 0, "inside: cleared");
                    assert_eq!(at(27, 16).alpha(), 0, "inside: cleared");
                    assert_eq!(at(28, 16), red.to_color_u8(), "right of it: kept");
                    let r = tiny_skia::Rect::from_xywh(20.0, 10.0, 8.0, 7.0).unwrap();
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color(blue);
                    pixmap.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
                }
            })
            .unwrap();
        app.flush();
        frames.push(committed.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    let damage: Vec<_> = frames.iter().map(|c| c.damage.clone()).collect();
    assert_eq!(
        damage,
        vec![
            vec![[0, 0, i32::MAX, i32::MAX]],
            vec![[20, 10, 8, 7]],
            vec![]
        ],
        "a whole frame, then the rectangle rounded out to device pixels, then nothing"
    );
    // the second buffer was never written: it got the whole frame, red at its corner
    assert_eq!(frames[1].first_pixel, [0, 0, 255, 255]);
    assert_ne!(frames[0].buffer, frames[1].buffer, "the other buffer");
}

/// A configure can change what a frame shows without changing its size (activation, maximized), so the frame after one
/// is whole even when the caller's damage is empty or partial.
#[test]
fn a_configure_makes_the_next_frame_whole_whatever_its_damage() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let red = tiny_skia::Color::from_rgba8(255, 0, 0, 255);
    app.presenter
        .draw(&mut app.conn, None, |pixmap, _| pixmap.fill(red))
        .unwrap();
    app.flush();
    committed.recv_timeout(Duration::from_secs(5)).unwrap();
    for damage in [vec![], vec![Rect::new(10.0, 5.25, 4.0, 3.0)]] {
        let size = app.presenter.size().unwrap();
        app.presenter.configured(size);
        control.send(Control::FrameDone).unwrap();
        app.wait_for_draw();
        let mut cleared = None;
        app.presenter
            .draw_with(&mut app.conn, Some(&damage), |pixmap, _, rects| {
                cleared = Some(rects.map(<[_]>::len));
                assert_eq!(
                    pixmap.pixel(0, 0).unwrap().alpha(),
                    0,
                    "the frame is cleared"
                );
            })
            .unwrap();
        assert_eq!(
            cleared,
            Some(None),
            "a whole frame after a configure: {damage:?}"
        );
        app.flush();
        let frame = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(frame.damage, vec![[0, 0, i32::MAX, i32::MAX]]);
    }
    // the configure is answered once: the next damaged frame is damaged again
    app.presenter.request_redraw();
    control.send(Control::FrameDone).unwrap();
    app.wait_for_draw();
    let mut cleared = None;
    app.presenter
        .draw_with(&mut app.conn, Some(&[]), |_, _, rects| {
            cleared = Some(rects.map(<[_]>::len));
        })
        .unwrap();
    assert_eq!(cleared, Some(Some(0)));
}

/// The server holds the buffer on screen, so frames take turns between two buffers. A damaged frame converts only its
/// rectangles and what its buffer missed of the frame before, and every committed buffer equals the whole frame.
#[test]
fn buffers_taking_turns_hold_every_damaged_frame_exactly() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let shade = |i: u8| tiny_skia::Color::from_rgba8(i.wrapping_mul(40), 200 - i, i, 255);
    let r = Rect::new;
    // `None` draws a whole frame
    let frames = [
        None,
        Some(vec![r(1.0, 1.0, 4.0, 3.0)]),
        Some(vec![r(50.0, 20.0, 30.0, 10.0), r(0.0, 0.0, 2.0, 2.0)]),
        Some(vec![]),
        Some(vec![r(90.0, 40.0, 20.0, 20.0)]),
        None,
        Some(vec![r(10.5, 7.25, 3.0, 12.0)]),
        Some(vec![r(1.0, 1.0, 4.0, 3.0)]),
        Some(vec![r(30.0, 60.0, 8.0, 8.0)]),
    ];
    let mut shown: Vec<Committed> = Vec::new();
    for (i, damage) in frames.iter().enumerate() {
        if i > 0 {
            app.presenter.request_redraw();
            control.send(Control::FrameDone).unwrap();
            app.wait_for_draw();
        }
        let mut frame = Vec::new();
        let paint = |pixmap: &mut tiny_skia::Pixmap, scale: Scale| {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(shade(i as u8));
            let to_device =
                tiny_skia::Transform::from_scale(scale.factor() as f32, scale.factor() as f32);
            match damage {
                None => pixmap.fill(shade(i as u8)),
                Some(rects) => {
                    for d in rects {
                        let d = tiny_skia::Rect::from_xywh(
                            d.x() as f32,
                            d.y() as f32,
                            d.width() as f32,
                            d.height() as f32,
                        )
                        .unwrap();
                        pixmap.fill_rect(d, &paint, to_device, None);
                    }
                }
            }
            frame = pixmap.data().to_vec();
        };
        app.presenter
            .draw(&mut app.conn, damage.as_deref(), paint)
            .unwrap();
        app.flush();
        let c = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut whole = vec![0; frame.len()];
        rgba_to_argb8888(&frame, &mut whole);
        assert!(
            c.pixels.0 == whole,
            "frame {i}: the buffer differs from the frame"
        );
        if let Some(last) = shown.last() {
            assert_ne!(c.buffer, last.buffer, "frame {i}: the buffer on screen");
        }
        shown.push(c);
    }
    assert_eq!(
        shown
            .iter()
            .map(|c| c.buffer)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2,
        "two buffers take turns"
    );
}

mod window_system {
    use super::*;
    use cupertino_geometry::{Point, Size};
    use cupertino_modifiers::Profile;
    use cupertino_ui_platform::{
        ClipboardKind, DecorationMode, MouseButton, PlatformEvent, PointerEvent, PresentOutcome,
        WindowEvent, WindowId, WindowSpec, WindowSystem,
    };
    use cupertino_ui_render::{CpuFrame, PixelFormat, PixelRect, RenderFrameId};
    use cupertino_ui_wayland::WaylandWindowSystem;

    struct Kit {
        ws: WaylandWindowSystem,
        window: WindowId,
        stash: VecDeque<PlatformEvent>,
    }

    impl Kit {
        fn wait_for<T>(&mut self, mut pick: impl FnMut(&PlatformEvent) -> Option<T>) -> T {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                while let Some(event) = self.stash.pop_front() {
                    if let Some(found) = pick(&event) {
                        return found;
                    }
                }
                let mut events = Vec::new();
                self.ws
                    .dispatch(Some(Duration::from_millis(50)), &mut events)
                    .unwrap();
                self.stash.extend(events);
            }
            panic!("timed out waiting for an event");
        }

        fn window_event<T>(&mut self, mut pick: impl FnMut(&WindowEvent) -> Option<T>) -> T {
            let window = self.window;
            self.wait_for(|e| match e {
                PlatformEvent::Window { window: w, event } if *w == window => pick(event),
                _ => None,
            })
        }

        /// Dispatch once without waiting, so the requests go out and what arrived is read.
        fn pump(&mut self) {
            let mut events = Vec::new();
            self.ws
                .dispatch(Some(Duration::from_millis(20)), &mut events)
                .unwrap();
            self.stash.extend(events);
        }

        /// Whether the stashed events hold a frame for the window; empties the stash.
        fn took_frame(&mut self) -> bool {
            std::mem::take(&mut self.stash).iter().any(|e| {
                matches!(
                    e,
                    PlatformEvent::Window {
                        event: WindowEvent::Frame(_),
                        ..
                    }
                )
            })
        }
    }

    /// A window mapped at the output's scale 2, so its frames are 200 × 100.
    fn mapped() -> (Kit, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
        let (mut kit, control, committed, seen) = created();
        let configure = kit.window_event(|e| match e {
            WindowEvent::Configure(c) => Some(*c),
            _ => None,
        });
        assert_eq!(
            (configure.size, configure.server_decorations),
            (None, false)
        );
        assert_eq!(
            kit.window_event(|e| match e {
                WindowEvent::Scale(s) => Some(*s),
                _ => None,
            }),
            2.0
        );
        (kit, control, committed, seen)
    }

    /// A window created but not yet configured.
    fn created() -> (Kit, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, seen) = spawn_server(server_stream);
        let mut ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        );
        let window = ws
            .create_window(&WindowSpec {
                title: "Test".into(),
                app_id: "org.example.Test".into(),
                size: Size::new(100.0, 50.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Client,
            })
            .unwrap();
        let kit = Kit {
            ws,
            window,
            stash: VecDeque::new(),
        };
        (kit, control, committed, seen)
    }

    fn frame(id: u64, first: [u8; 4], damage: &[PixelRect]) -> CpuFrame {
        frame_sized(id, (200, 100), first, damage)
    }

    fn frame_sized(
        id: u64,
        (width, height): (u32, u32),
        first: [u8; 4],
        damage: &[PixelRect],
    ) -> CpuFrame {
        let mut pixels = vec![0; (width * height * 4) as usize];
        pixels[..4].copy_from_slice(&first);
        CpuFrame {
            id: RenderFrameId(id),
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels,
            damage: damage.to_vec(),
        }
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn frames_are_paced_by_the_compositor_and_damage_a_frame_could_not_show_is_carried() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;

        // nothing paces a window that never presented: the frame is due at once
        kit.ws.request_frame(w);
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        let first = kit
            .ws
            .present(w, &frame(1, [10, 20, 30, 40], &[]), None)
            .unwrap();
        assert_eq!(first, PresentOutcome::Presented);
        kit.pump();
        let shown = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (shown.buffer_size, shown.first_pixel, shown.damage),
            (
                (200, 100),
                [30, 20, 10, 40],
                vec![[0, 0, i32::MAX, i32::MAX]]
            )
        );

        // the next frame waits for the frame callback
        kit.ws.request_frame(w);
        kit.pump();
        assert!(!kit.stash.iter().any(|e| matches!(
            e,
            PlatformEvent::Window {
                event: WindowEvent::Frame(_),
                ..
            }
        )));
        control.send(Control::FrameDone).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));

        // a second buffer while the server holds the first; showing it releases the first
        kit.ws
            .present(w, &frame(2, [1, 1, 1, 1], &[rect(0, 0, 1, 1)]), None)
            .unwrap();
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
        kit.pump();

        // a frame callback nobody asked for wakes nothing
        control.send(Control::FrameDone).unwrap();
        kit.pump();
        kit.pump();
        assert!(!kit.stash.iter().any(|e| matches!(
            e,
            PlatformEvent::Window {
                event: WindowEvent::Frame(_),
                ..
            }
        )));

        // a frame drawn at a stale size is not shown, and its damage is carried
        let stale = frame_sized(3, (100, 50), [0; 4], &[rect(40, 30, 5, 5)]);
        assert_eq!(
            kit.ws.present(w, &stale, None).unwrap(),
            PresentOutcome::NotReady
        );
        assert!(committed.recv_timeout(Duration::from_millis(100)).is_err());
        // the first buffer again: the compositor hears of the carried damage and this frame's; the buffer also gets
        // what it missed (frame 2's pixel), which the compositor already showed
        // the first buffer again: this frame's damage, the carried one, and what it missed (frame 2's pixel)
        kit.ws
            .present(w, &frame(4, [1, 1, 1, 1], &[rect(10, 10, 2, 2)]), None)
            .unwrap();
        kit.pump();
        let reused = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(reused.buffer, shown.buffer);
        assert_eq!(reused.damage, vec![[40, 30, 5, 5], [10, 10, 2, 2]]);
        assert_eq!(reused.first_pixel, [1, 1, 1, 1]);
    }

    #[test]
    fn a_frame_presented_before_the_first_configure_is_due_again_with_the_configure() {
        let (mut kit, _control, committed, _seen) = created();
        let w = kit.window;
        assert_eq!(
            kit.ws.present(w, &frame(1, [0; 4], &[]), None).unwrap(),
            PresentOutcome::NotReady
        );
        kit.ws.request_frame(w);
        let configured_first = kit.window_event(|e| match e {
            WindowEvent::Configure(_) => Some(true),
            WindowEvent::Frame(_) => Some(false),
            _ => None,
        });
        assert!(configured_first);
        let (mut framed, mut scaled) = (false, false);
        while !(framed && scaled) {
            let is_frame = kit.window_event(|e| match e {
                WindowEvent::Frame(_) => Some(true),
                WindowEvent::Scale(_) => Some(false),
                _ => None,
            });
            framed |= is_frame;
            scaled |= !is_frame;
        }
        assert_eq!(
            kit.ws.present(w, &frame(2, [0; 4], &[]), None).unwrap(),
            PresentOutcome::Presented
        );
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_frame_with_no_free_buffer_is_due_again_once_the_compositor_releases_one() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;
        control.send(Control::HoldBuffers).unwrap();
        let mut presented = 0;
        while kit
            .ws
            .present(w, &frame(presented, [0; 4], &[]), None)
            .unwrap()
            == PresentOutcome::Presented
        {
            presented += 1;
            assert!(presented < 8, "the transport never ran out of buffers");
        }
        // the frame callbacks are done, so only the held buffers keep the next frame back
        kit.pump();
        for _ in 0..presented {
            committed.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        control.send(Control::FrameDone).unwrap();
        kit.pump();
        kit.pump();
        assert_eq!(
            kit.ws.present(w, &frame(8, [0; 4], &[]), None).unwrap(),
            PresentOutcome::NotReady
        );
        kit.stash.clear();

        kit.ws.request_frame(w);
        kit.pump();
        kit.pump();
        assert!(!kit.took_frame());

        control.send(Control::ReleaseBuffers).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        assert_eq!(
            kit.ws.present(w, &frame(9, [0; 4], &[]), None).unwrap(),
            PresentOutcome::Presented
        );
    }

    #[test]
    fn a_click_arrives_in_logical_points_with_its_serial_and_a_time_on_the_systems_clock() {
        let (mut kit, control, _committed, _seen) = mapped();
        control.send(Control::PointerPress).unwrap();
        let (button, time) = kit.window_event(|e| match e {
            WindowEvent::Pointer(p @ PointerEvent::Button { .. }, time) => Some((*p, *time)),
            _ => None,
        });
        assert_eq!(
            button,
            PointerEvent::Button {
                position: Point::new(10.25, 20.75),
                button: MouseButton::Left,
                pressed: true,
                serial: 6,
            }
        );
        assert!(time.0 > 0);
    }

    #[test]
    fn the_clipboard_reports_its_offer_reads_it_and_copies_with_the_latest_serial() {
        let (mut kit, control, _committed, seen) = mapped();
        control
            .send(Control::OfferSelection("hello".into()))
            .unwrap();
        let mimes = kit.wait_for(|e| match e {
            PlatformEvent::ClipboardOffered { kind, mimes } => Some((*kind, mimes.clone())),
            _ => None,
        });
        assert_eq!(
            mimes,
            (
                ClipboardKind::Clipboard,
                Some(vec!["text/plain;charset=utf-8".into()])
            )
        );
        let token = kit
            .ws
            .read_clipboard(ClipboardKind::Clipboard, "text/plain;charset=utf-8");
        let data = kit.wait_for(|e| match e {
            PlatformEvent::Clipboard { token: t, data } if *t == token => Some(data.clone()),
            _ => None,
        });
        assert_eq!(data.map(|d| d.bytes), Some(b"hello".to_vec()));
        // the primary selection is not offered here, whatever the clipboard holds
        let primary = kit
            .ws
            .read_clipboard(ClipboardKind::Primary, "text/plain;charset=utf-8");
        let nothing = kit.wait_for(|e| match e {
            PlatformEvent::Clipboard { token: t, data } if *t == primary => Some(data.clone()),
            _ => None,
        });
        assert_eq!(nothing, None);

        control.send(Control::PointerPress).unwrap();
        kit.window_event(|e| {
            matches!(e, WindowEvent::Pointer(PointerEvent::Button { .. }, _)).then_some(())
        });
        kit.ws.set_clipboard(
            ClipboardKind::Clipboard,
            cupertino_ui_platform::ClipboardData {
                mime: "text/plain".into(),
                bytes: b"copied".to_vec(),
            },
        );
        kit.pump();
        assert_eq!(
            seen.recv_timeout(Duration::from_secs(5)).unwrap(),
            Seen::Selection {
                serial: 6,
                types: vec!["text/plain".into()]
            }
        );
    }

    #[test]
    fn a_waker_from_another_thread_ends_a_dispatch_with_wake() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let waker = kit.ws.waker();
        std::thread::spawn(move || waker.wake());
        kit.wait_for(|e| matches!(e, PlatformEvent::Wake).then_some(()));
    }

    #[test]
    fn requests_on_an_unknown_window_or_the_wrong_kind_are_refused() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let gone = WindowId(99);
        assert_eq!(
            kit.ws
                .request(gone, cupertino_ui_platform::WindowRequest::Minimize),
            Err(cupertino_ui_platform::PlatformError::UnknownWindow(gone))
        );
        let w = kit.window;
        assert_eq!(
            kit.ws.request(
                w,
                cupertino_ui_platform::WindowRequest::Reposition(
                    cupertino_ui_platform::PopupSpec {
                        anchor_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                        anchor: Default::default(),
                        gravity: Default::default(),
                        offset: Point::ZERO,
                        size: Size::new(10.0, 10.0),
                        constraint: Default::default(),
                        reactive: false,
                        grab: false,
                    }
                )
            ),
            Err(cupertino_ui_platform::PlatformError::Unsupported)
        );
        kit.ws.destroy(w);
        assert_eq!(
            kit.ws.present(w, &frame(1, [0; 4], &[]), None),
            Err(cupertino_ui_platform::PlatformError::UnknownWindow(w))
        );
    }
}
