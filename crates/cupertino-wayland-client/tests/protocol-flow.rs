//! Runs the transport against a minimal in-process compositor that offers the core globals and, per test, some optional
//! ones, and checks what the compositor receives and what the client reports back.

use cupertino_geometry::Rect;
use cupertino_wayland_client::{
    Anchor, BufferFormat, Button, Capability, Connection, ConstraintAdjustment, Decorations, Error,
    Event, Extension, Gravity, KeyboardEvent, PointerEvent, PopupPlacement, Scale, SurfaceId,
    Toplevel, ToplevelConfig, WindowConfigure,
};
use std::collections::VecDeque;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::server::wp_viewporter::{self, WpViewporter};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::{
    self, ZxdgDecorationManagerV1,
};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::{
    self, Mode, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::server::xdg_popup::XdgPopup;
use wayland_protocols::xdg::shell::server::xdg_positioner::{self, XdgPositioner};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_data_device::WlDataDevice;
use wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::WlDataSource;
use wayland_server::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_seat::{self, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client as ServerClient, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};
use xkbcommon::xkb;

/// Evdev codes (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const KEY_A: u32 = 30;

/// The optional globals a test's compositor offers next to the core ones.
#[derive(Clone, Copy, Default)]
struct Globals {
    xdg_wm_base: bool,
    seat: bool,
    clipboard: bool,
    /// The mode the compositor answers a decoration request with.
    decoration: Option<Mode>,
    fractional_scale: bool,
    output_scale: Option<i32>,
    /// Never release a buffer.
    hold_buffers: bool,
    layer_shell: bool,
}

impl Globals {
    fn core() -> Globals {
        Globals {
            xdg_wm_base: true,
            ..Globals::default()
        }
    }
}

/// What the test tells the compositor to do.
enum Control {
    Configure {
        width: i32,
        height: i32,
        states: Vec<xdg_toplevel::State>,
    },
    EnterOutput,
    PreferredBufferScale(i32),
    FractionalScale(u32),
    DismissPopup,
    Click {
        x: f64,
        y: f64,
    },
    TypeKey(u32),
    Offer(Vec<&'static str>),
    /// Configure the layer surface this many times in a row, each a pixel wider.
    LayerConfigures(u32),
}

/// What the compositor saw.
#[derive(Debug, PartialEq)]
enum Seen {
    Ack(u32),
    Commit {
        size: (i32, i32),
        scale: i32,
        format: wl_shm::Format,
    },
    Viewport(i32, i32),
    DecorationMode(Mode),
    Positioner {
        size: (i32, i32),
        anchor_rect: [i32; 4],
    },
    Receive(String),
    LayerAck(u32),
    LayerPopup,
    LayerDestroyed,
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    scale: i32,
    frames: Vec<WlCallback>,
}

type SurfaceData = Mutex<SurfaceState>;

struct BufferData {
    width: i32,
    height: i32,
    format: wl_shm::Format,
}

#[derive(Default, Clone, Copy)]
struct PositionerState {
    size: (i32, i32),
    anchor_rect: [i32; 4],
}

struct Mock {
    globals: Globals,
    seen: Sender<Seen>,
    next_serial: u32,
    surfaces: Vec<WlSurface>,
    toplevel: Option<(
        WlSurface,
        XdgSurface,
        XdgToplevel,
        Option<ZxdgToplevelDecorationV1>,
    )>,
    configured: bool,
    popup: Option<(XdgSurface, XdgPopup, PositionerState, bool)>,
    output: Option<WlOutput>,
    fractional: Option<WpFractionalScaleV1>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    data_device: Option<WlDataDevice>,
    shown: Option<WlBuffer>,
    frames: Vec<WlCallback>,
    /// The layer surface with the size it asked for, and whether it was configured.
    layer: Option<(WlSurface, ZwlrLayerSurfaceV1, (u32, u32), bool)>,
}

impl Mock {
    fn serial(&mut self) -> u32 {
        self.next_serial += 1;
        self.next_serial
    }

    fn surface(&self) -> &WlSurface {
        &self.toplevel.as_ref().expect("a toplevel").0
    }
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

plain_global!(
    WlCompositor,
    WlShm,
    XdgWmBase,
    ZxdgDecorationManagerV1,
    WpFractionalScaleManagerV1,
    WpViewporter,
    WlDataDeviceManager,
    ZwlrLayerShellV1
);

/// Requests the compositor ignores.
macro_rules! ignore_requests {
    ($($interface:ty => $data:ty),*) => {$(
        impl Dispatch<$interface, $data> for Mock {
            fn request(_: &mut Self, _: &ServerClient, _: &$interface, _: <$interface as Resource>::Request, _: &$data, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
    )*};
}

ignore_requests!(WlBuffer => BufferData, WlCallback => (), WlPointer => (), WlKeyboard => (), WlOutput => (), WpFractionalScaleV1 => (), WlDataSource => ());

impl Dispatch<WlCompositor, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            let surface = init.init(id, SurfaceData::default());
            state.surfaces.push(surface);
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
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::Frame { callback } => s.frames.push(init.init(callback, ())),
            wl_surface::Request::Commit => {
                let initial = s.buffer.is_none();
                if initial
                    && !state.configured
                    && state.toplevel.as_ref().is_some_and(|t| t.0 == *surface)
                {
                    state.configured = true;
                    let serial = state.serial();
                    let (_, xdg, toplevel, decoration) = state.toplevel.as_ref().unwrap();
                    if let (Some(d), Some(mode)) = (decoration, state.globals.decoration) {
                        d.configure(mode);
                    }
                    toplevel.configure(0, 0, Vec::new());
                    xdg.configure(serial);
                }
                if initial
                    && state
                        .layer
                        .as_ref()
                        .is_some_and(|l| l.0 == *surface && !l.3)
                {
                    let serial = state.serial();
                    let (_, layer, (width, height), configured) = state.layer.as_mut().unwrap();
                    *configured = true;
                    layer.configure(serial, *width, *height);
                }
                if initial && state.popup.as_ref().is_some_and(|p| !p.3) {
                    let serial = state.serial();
                    let (xdg, popup, p, mapped) = state.popup.as_mut().unwrap();
                    *mapped = true;
                    // below the anchor rectangle, at its left edge
                    popup.configure(
                        p.anchor_rect[0],
                        p.anchor_rect[1] + p.anchor_rect[3],
                        p.size.0,
                        p.size.1,
                    );
                    xdg.configure(serial);
                }
                if let Some(buffer) = s.buffer.take() {
                    let b = buffer.data::<BufferData>().unwrap();
                    state
                        .seen
                        .send(Seen::Commit {
                            size: (b.width, b.height),
                            scale: s.scale.max(1),
                            format: b.format,
                        })
                        .unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    let previous = state.shown.replace(buffer);
                    if let Some(previous) = previous.filter(|_| !state.globals.hold_buffers) {
                        previous.release();
                    }
                    state.frames.append(&mut s.frames);
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
        if let wl_shm::Request::CreatePool { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WlShmPool, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            width,
            height,
            format,
            ..
        } = request
        {
            let format = format.into_result().unwrap();
            init.init(
                id,
                BufferData {
                    width,
                    height,
                    format,
                },
            );
        }
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
        match request {
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                init.init(id, surface);
            }
            xdg_wm_base::Request::CreatePositioner { id } => {
                init.init(id, Mutex::new(PositionerState::default()));
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgPositioner, Mutex<PositionerState>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgPositioner,
        request: xdg_positioner::Request,
        data: &Mutex<PositionerState>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut p = data.lock().unwrap();
        match request {
            xdg_positioner::Request::SetSize { width, height } => p.size = (width, height),
            xdg_positioner::Request::SetAnchorRect {
                x,
                y,
                width,
                height,
            } => p.anchor_rect = [x, y, width, height],
            _ => {}
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
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                let toplevel = init.init(id, ());
                state.toplevel = Some((surface.clone(), xdg.clone(), toplevel, None));
            }
            xdg_surface::Request::GetPopup { id, positioner, .. } => {
                let p = *positioner
                    .data::<Mutex<PositionerState>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                state
                    .seen
                    .send(Seen::Positioner {
                        size: p.size,
                        anchor_rect: p.anchor_rect,
                    })
                    .unwrap();
                let popup = init.init(id, ());
                state.popup = Some((xdg.clone(), popup, p, false));
            }
            xdg_surface::Request::AckConfigure { serial } => {
                state.seen.send(Seen::Ack(serial)).unwrap()
            }
            _ => {}
        }
    }
}

ignore_requests!(XdgToplevel => (), XdgPopup => ());

impl Dispatch<ZxdgDecorationManagerV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, .. } = request {
            let decoration = init.init(id, ());
            state.toplevel.as_mut().unwrap().3 = Some(decoration);
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Request::SetMode {
            mode: wayland_server::WEnum::Value(mode),
        } = request
        {
            state.seen.send(Seen::DecorationMode(mode)).unwrap();
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, .. } = request {
            state.fractional = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<WpViewporter, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewporter::Request::GetViewport { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WpViewport, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WpViewport,
        request: wp_viewport::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewport::Request::SetDestination { width, height } = request {
            state.seen.send(Seen::Viewport(width, height)).unwrap();
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
        output.scale(state.globals.output_scale.unwrap_or(1));
        output.name("TEST-1".into());
        output.done();
        state.output = Some(output);
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
        let seat = init.init(r, ());
        seat.capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard);
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
        match request {
            wl_seat::Request::GetPointer { id } => state.pointer = Some(init.init(id, ())),
            wl_seat::Request::GetKeyboard { id } => {
                let keyboard = init.init(id, ());
                let (fd, size) = us_keymap();
                keyboard.keymap(wl_keyboard::KeymapFormat::XkbV1, fd.as_fd(), size);
                keyboard.repeat_info(0, 0);
                state.keyboard = Some(keyboard);
            }
            _ => {}
        }
    }
}

/// The US layout as the keymap file a compositor sends: its text and a NUL.
fn us_keymap() -> (std::fs::File, u32) {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the us layout compiles");
    let mut text = keymap
        .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
        .into_bytes();
    text.push(0);
    let mut file = std::fs::File::from(
        rustix::fs::memfd_create("keymap", rustix::fs::MemfdFlags::CLOEXEC).unwrap(),
    );
    file.write_all(&text).unwrap();
    (file, text.len() as u32)
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
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                state.data_device = Some(init.init(id, ()))
            }
            wl_data_device_manager::Request::CreateDataSource { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

ignore_requests!(WlDataDevice => ());

impl Dispatch<WlDataOffer, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataOffer,
        request: wl_data_offer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_offer::Request::Receive { mime_type, fd } = request {
            let mut file = std::fs::File::from(fd);
            file.write_all(format!("as {mime_type}").as_bytes())
                .unwrap();
            state.seen.send(Seen::Receive(mime_type)).unwrap();
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface { id, surface, .. } = request {
            let layer = init.init(id, ());
            state.layer = Some((surface, layer, (0, 0), false));
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                if let Some(layer) = state.layer.as_mut() {
                    layer.2 = (width, height);
                }
                return;
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => Seen::LayerAck(serial),
            zwlr_layer_surface_v1::Request::GetPopup { .. } => Seen::LayerPopup,
            zwlr_layer_surface_v1::Request::Destroy => Seen::LayerDestroyed,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

/// Run a compositor with `globals` on its own thread for the client at the other end of `stream`.
fn spawn_compositor(stream: UnixStream, globals: Globals) -> (Sender<Control>, Receiver<Seen>) {
    let (control_tx, control_rx) = channel::<Control>();
    let (seen_tx, seen_rx) = channel();
    std::thread::spawn(move || {
        let mut display = Display::<Mock>::new().unwrap();
        let dh = display.handle();
        dh.create_global::<Mock, WlCompositor, _>(6, ());
        dh.create_global::<Mock, WlShm, _>(1, ());
        if globals.xdg_wm_base {
            dh.create_global::<Mock, XdgWmBase, _>(6, ());
        }
        if globals.seat {
            dh.create_global::<Mock, WlSeat, _>(7, ());
        }
        if globals.clipboard {
            dh.create_global::<Mock, WlDataDeviceManager, _>(3, ());
        }
        if globals.decoration.is_some() {
            dh.create_global::<Mock, ZxdgDecorationManagerV1, _>(1, ());
        }
        if globals.fractional_scale {
            dh.create_global::<Mock, WpFractionalScaleManagerV1, _>(1, ());
            dh.create_global::<Mock, WpViewporter, _>(1, ());
        }
        if globals.output_scale.is_some() {
            dh.create_global::<Mock, WlOutput, _>(4, ());
        }
        if globals.layer_shell {
            dh.create_global::<Mock, ZwlrLayerShellV1, _>(4, ());
        }
        let mut mock = Mock {
            globals,
            seen: seen_tx,
            next_serial: 100,
            surfaces: Vec::new(),
            toplevel: None,
            configured: false,
            popup: None,
            output: None,
            fractional: None,
            pointer: None,
            keyboard: None,
            data_device: None,
            shown: None,
            frames: Vec::new(),
            layer: None,
        };
        let mut handle = display.handle();
        handle.insert_client(stream, Arc::new(Data)).unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(control) => apply(&mut mock, &handle, control),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            mock.frames.drain(..).for_each(|cb| cb.done(0));
            if display.dispatch_clients(&mut mock).is_err() {
                return;
            }
            let _ = display.flush_clients();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (control_tx, seen_rx)
}

fn apply(mock: &mut Mock, handle: &DisplayHandle, control: Control) {
    match control {
        Control::Configure {
            width,
            height,
            states,
        } => {
            let serial = mock.serial();
            let (_, xdg, toplevel, _) = mock.toplevel.as_ref().unwrap();
            toplevel.configure(
                width,
                height,
                states
                    .iter()
                    .flat_map(|s| (*s as u32).to_ne_bytes())
                    .collect(),
            );
            xdg.configure(serial);
        }
        Control::EnterOutput => mock.surface().enter(mock.output.as_ref().unwrap()),
        Control::PreferredBufferScale(factor) => mock.surface().preferred_buffer_scale(factor),
        Control::FractionalScale(value) => mock.fractional.as_ref().unwrap().preferred_scale(value),
        Control::DismissPopup => mock.popup.as_ref().unwrap().1.popup_done(),
        Control::Click { x, y } => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.enter(serial, &surface, x, y);
            pointer.button(serial + 1, 0, BTN_LEFT, wl_pointer::ButtonState::Pressed);
            pointer.frame();
        }
        Control::TypeKey(code) => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let keyboard = mock.keyboard.as_ref().unwrap();
            keyboard.enter(serial, &surface, Vec::new());
            keyboard.key(serial + 1, 0, code, wl_keyboard::KeyState::Pressed);
        }
        Control::Offer(types) => {
            let device = mock.data_device.as_ref().unwrap();
            let offer = device
                .client()
                .unwrap()
                .create_resource::<WlDataOffer, (), Mock>(handle, device.version(), ())
                .unwrap();
            device.data_offer(&offer);
            for t in types {
                offer.offer(t.into());
            }
            device.selection(Some(&offer));
        }
        Control::LayerConfigures(count) => {
            for i in 0..count {
                let serial = mock.serial();
                let (_, layer, (width, height), _) = mock.layer.as_ref().unwrap();
                layer.configure(serial, width + i + 1, *height);
            }
        }
    }
}

/// A connection to a fresh compositor with `globals`.
fn connect(globals: Globals) -> (Connection, Sender<Control>, Receiver<Seen>) {
    let (client, server) = UnixStream::pair().unwrap();
    let (control, seen) = spawn_compositor(server, globals);
    (Connection::from_stream(client).unwrap(), control, seen)
}

/// Events the client reported but the test has not looked at yet.
type Stash = VecDeque<Event>;

/// Take events in order, waiting for more when the stash runs dry, until `pick` accepts one.
fn wait_for<T>(
    conn: &mut Connection,
    stash: &mut Stash,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(event) = stash.pop_front() {
            if let Some(found) = pick(&event) {
                return found;
            }
        }
        stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
    }
    panic!("timed out waiting for an event");
}

fn next_seen(seen: &Receiver<Seen>) -> Seen {
    seen.recv_timeout(Duration::from_secs(5))
        .expect("the compositor saw a request")
}

fn window(conn: &mut Connection, decorations: Decorations) -> Toplevel {
    conn.toplevel(&ToplevelConfig {
        app_id: "test".into(),
        title: "Test".into(),
        min_size: None,
        max_size: None,
        decorations,
    })
    .unwrap()
}

fn configured(conn: &mut Connection, stash: &mut Stash, window: Toplevel) -> WindowConfigure {
    wait_for(conn, stash, |e| match e {
        Event::Configure {
            window: w,
            configure,
        } if *w == window => Some(*configure),
        _ => None,
    })
}

fn present(conn: &mut Connection, surface: SurfaceId, preferred: (u32, u32)) {
    conn.lease(surface, preferred, BufferFormat::Xrgb8888)
        .unwrap()
        .present()
        .unwrap();
}

#[test]
fn the_core_globals_alone_map_a_window_the_client_decorates() {
    let (mut conn, _control, seen) = connect(Globals::core());
    assert_eq!(conn.capabilities().iter().count(), 0);
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Server);
    let configure = configured(&mut conn, &mut stash, w);
    assert_eq!(
        (configure.size, configure.decorations),
        (None, Decorations::Client)
    );
    present(&mut conn, w.surface(), (320, 240));
    assert_eq!(next_seen(&seen), Seen::Ack(configure.serial));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (320, 240),
            scale: 1,
            format: wl_shm::Format::Xrgb8888
        }
    );
    // the same configure is acknowledged once
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::Frame(_)).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(next_seen(&seen), Seen::Commit { .. }));
}

#[test]
fn a_compositor_without_xdg_wm_base_is_refused_by_name() {
    let (client, server) = UnixStream::pair().unwrap();
    let _compositor = spawn_compositor(server, Globals::default());
    assert!(matches!(
        Connection::from_stream(client),
        Err(Error::MissingRequiredProtocol {
            interface: "xdg_wm_base",
            ..
        })
    ));
}

#[test]
fn each_resize_draws_at_the_configured_size_and_acknowledges_its_serial() {
    let (mut conn, control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (320, 240));
    let mut last = match next_seen(&seen) {
        Seen::Ack(serial) => serial,
        other => panic!("{other:?}"),
    };
    next_seen(&seen);
    for (width, height, states) in [
        (3840, 2160, vec![xdg_toplevel::State::Maximized]),
        (1280, 800, vec![xdg_toplevel::State::Resizing]),
        (320, 240, vec![]),
    ] {
        control
            .send(Control::Configure {
                width,
                height,
                states,
            })
            .unwrap();
        let c = configured(&mut conn, &mut stash, w);
        assert_eq!(c.size, Some((width as u32, height as u32)));
        assert!(c.serial > last, "configure serials increase");
        last = c.serial;
        present(&mut conn, w.surface(), (100, 100));
        assert_eq!(next_seen(&seen), Seen::Ack(c.serial));
        assert_eq!(
            next_seen(&seen),
            Seen::Commit {
                size: (width, height),
                scale: 1,
                format: wl_shm::Format::Xrgb8888
            }
        );
    }
}

#[test]
fn a_lease_waits_while_the_compositor_holds_three_buffers() {
    let (mut conn, _control, seen) = connect(Globals {
        hold_buffers: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    for _ in 0..3 {
        present(&mut conn, w.surface(), (64, 64));
    }
    assert!(matches!(
        conn.lease(w.surface(), (64, 64), BufferFormat::Xrgb8888),
        Err(Error::NoFreeBuffer)
    ));
    let commits = std::iter::from_fn(|| seen.recv_timeout(Duration::from_millis(200)).ok())
        .filter(|s| matches!(s, Seen::Commit { .. }))
        .count();
    assert_eq!(commits, 3);
}

#[test]
fn the_scale_follows_the_output_then_the_preferred_buffer_scale() {
    let (mut conn, control, seen) = connect(Globals {
        output_scale: Some(2),
        ..Globals::core()
    });
    assert_eq!(conn.outputs()[0].name.as_deref(), Some("TEST-1"));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::EnterOutput).unwrap();
    let scale = wait_for(&mut conn, &mut stash, |e| match e {
        Event::ScaleChanged { scale, .. } => Some(*scale),
        _ => None,
    });
    assert_eq!(scale, Scale::from_integer(2).unwrap());
    present(&mut conn, w.surface(), (320, 240));
    next_seen(&seen);
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (640, 480),
            scale: 2,
            format: wl_shm::Format::Xrgb8888
        }
    );
    control.send(Control::PreferredBufferScale(3)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { .. }).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(
        next_seen(&seen),
        Seen::Commit {
            size: (960, 720),
            scale: 3,
            ..
        }
    ));
}

#[test]
fn a_fractional_scale_draws_more_pixels_into_the_same_logical_size() {
    let (mut conn, control, seen) = connect(Globals {
        fractional_scale: true,
        ..Globals::core()
    });
    assert!(conn.capabilities().contains(Capability::FractionalScale));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::FractionalScale(180)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { .. }).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(next_seen(&seen), Seen::Ack(_)));
    assert_eq!(next_seen(&seen), Seen::Viewport(320, 240));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (480, 360),
            scale: 1,
            format: wl_shm::Format::Xrgb8888
        }
    );
}

#[test]
fn decorations_are_the_compositors_when_granted_and_the_clients_when_refused() {
    for (granted, expected) in [
        (Mode::ServerSide, Decorations::Server),
        (Mode::ClientSide, Decorations::Client),
    ] {
        let (mut conn, _control, seen) = connect(Globals {
            decoration: Some(granted),
            ..Globals::core()
        });
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Server);
        conn.flush().unwrap();
        assert_eq!(next_seen(&seen), Seen::DecorationMode(Mode::ServerSide));
        assert_eq!(configured(&mut conn, &mut stash, w).decorations, expected);
    }
}

#[test]
fn a_popup_is_drawn_where_and_as_large_as_the_compositor_placed_it() {
    let (mut conn, control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (320, 240));
    next_seen(&seen);
    next_seen(&seen);
    let popup = conn
        .popup(
            w.surface(),
            &PopupPlacement {
                anchor_rect: Rect::new(10.0, 20.0, 30.5, 8.0),
                size: (120, 90),
                anchor: Anchor::BottomLeft,
                gravity: Gravity::BottomRight,
                constraint_adjustment: ConstraintAdjustment::FlipY,
                grab: false,
            },
        )
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::Positioner {
            size: (120, 90),
            anchor_rect: [10, 20, 31, 8]
        }
    );
    let placed = wait_for(&mut conn, &mut stash, |e| match e {
        Event::PopupConfigure {
            popup: p,
            configure,
        } if *p == popup => Some(*configure),
        _ => None,
    });
    assert_eq!(
        (placed.x, placed.y, placed.width, placed.height),
        (10, 28, 120, 90),
        "{placed:?}"
    );
    present(&mut conn, popup.surface(), (1, 1));
    assert_eq!(next_seen(&seen), Seen::Ack(placed.serial));
    assert!(matches!(
        next_seen(&seen),
        Seen::Commit {
            size: (120, 90),
            ..
        }
    ));
    control.send(Control::DismissPopup).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        (*e == Event::PopupDone(popup)).then_some(())
    });
    conn.destroy(w.surface()).unwrap();
    assert!(matches!(
        conn.scale(popup.surface()),
        Err(Error::UnknownSurface)
    ));
}

#[test]
fn pointer_and_key_input_arrive_typed_on_the_focused_window() {
    let (mut conn, control, _seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::Click { x: 12.5, y: 7.25 }).unwrap();
    let button = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Pointer {
            surface,
            event:
                PointerEvent::Button {
                    button,
                    pressed,
                    position,
                },
        } if *surface == w.surface() => Some((*button, *pressed, position.x, position.y)),
        _ => None,
    });
    assert_eq!(button, (Button::Left, true, 12.5, 7.25));
    control.send(Control::TypeKey(KEY_A)).unwrap();
    let key = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Keyboard {
            event:
                KeyboardEvent::Key {
                    code,
                    character,
                    repeat,
                    ..
                },
            ..
        } => Some((*code, *character, *repeat)),
        _ => None,
    });
    assert_eq!(key, (KEY_A, Some('a'), false));
}

#[test]
fn the_selection_is_read_in_an_offered_type_only_and_a_replaced_offer_is_cancelled() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    control
        .send(Control::Offer(vec!["text/plain", "image/png"]))
        .unwrap();
    let offer = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(offer)) => Some(offer.clone()),
        _ => None,
    });
    assert_eq!(offer.mime_types(), ["text/plain", "image/png"]);
    assert_eq!(conn.receive(&offer, "image/png").unwrap(), b"as image/png");
    assert_eq!(next_seen(&seen), Seen::Receive("image/png".into()));
    assert!(matches!(
        conn.receive(&offer, "text/html"),
        Err(Error::UnsupportedFormat(mime)) if mime == "text/html"
    ));
    control.send(Control::Offer(vec!["text/plain"])).unwrap();
    wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(o)) if *o != offer => Some(()),
        _ => None,
    });
    assert!(matches!(
        conn.receive(&offer, "text/plain"),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        conn.set_selection(vec![("text/plain".into(), b"x".to_vec())]),
        Err(Error::PermissionDenied(_))
    ));
}

#[test]
fn a_compositor_that_goes_away_ends_the_wait_with_disconnected() {
    let (mut conn, control, _seen) = connect(Globals::core());
    drop(control);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match conn.wait(Some(Duration::from_millis(50))) {
            Err(Error::Disconnected(_)) => break,
            Ok(_) if Instant::now() < deadline => {}
            other => panic!("{other:?}"),
        }
    }
}

/// An extension as a crate above the transport writes one, through the public API only: layer surfaces, a protocol the
/// transport does not know.
mod layer {
    use cupertino_wayland_client::{Connection, Error, ExternalRole, State, SurfaceId};
    use std::sync::{Arc, Mutex};
    use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};
    use wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup;
    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
        Layer, ZwlrLayerShellV1,
    };
    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
        self, ZwlrLayerSurfaceV1,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Configured {
        pub surface: SurfaceId,
        pub serial: u32,
        pub size: (u32, u32),
    }

    type Inbox = Arc<Mutex<Vec<Configured>>>;

    /// User data of the extension's own type: `Dispatch` for the transport's `State` is only allowed with it.
    pub struct ShellData;

    pub struct SurfaceData {
        surface: SurfaceId,
        inbox: Inbox,
    }

    pub struct Layers {
        shell: ZwlrLayerShellV1,
        inbox: Inbox,
    }

    struct LayerRole(ZwlrLayerSurfaceV1);

    impl ExternalRole for LayerRole {
        fn ack_configure(&self, serial: u32) {
            self.0.ack_configure(serial);
        }

        fn destroy(&self) {
            self.0.destroy();
        }

        fn adopt_popup(&self, popup: &XdgPopup) -> Result<(), Error> {
            self.0.get_popup(popup);
            Ok(())
        }
    }

    impl Layers {
        pub fn bind(conn: &mut Connection) -> Layers {
            let shell = conn.bind_extension(1..=4, ShellData).unwrap();
            Layers {
                shell,
                inbox: Inbox::default(),
            }
        }

        pub fn surface(&self, conn: &mut Connection, size: (u32, u32)) -> SurfaceId {
            conn.external_surface(|wl, qh, surface| {
                let data = SurfaceData {
                    surface,
                    inbox: self.inbox.clone(),
                };
                let layer =
                    self.shell
                        .get_layer_surface(wl, None, Layer::Top, "test".into(), qh, data);
                layer.set_size(size.0, size.1);
                LayerRole(layer)
            })
        }

        /// What the extension received since the last call.
        pub fn take(&self) -> Vec<Configured> {
            std::mem::take(&mut self.inbox.lock().unwrap())
        }
    }

    impl Dispatch<ZwlrLayerShellV1, ShellData> for State {
        fn event(
            _: &mut State,
            _: &ZwlrLayerShellV1,
            _: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
            _: &ShellData,
            _: &WlConnection,
            _: &QueueHandle<State>,
        ) {
        }
    }

    impl Dispatch<ZwlrLayerSurfaceV1, SurfaceData> for State {
        fn event(
            state: &mut State,
            _: &ZwlrLayerSurfaceV1,
            event: zwlr_layer_surface_v1::Event,
            data: &SurfaceData,
            _: &WlConnection,
            _: &QueueHandle<State>,
        ) {
            if let zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } = event
            {
                state.configure_external(data.surface, serial, (width, height));
                data.inbox.lock().unwrap().push(Configured {
                    surface: data.surface,
                    serial,
                    size: (width, height),
                });
            }
        }
    }
}

/// Wait until the extension received `count` configures, at most 5 s; them and how many waits that took.
fn layer_configures(
    conn: &mut Connection,
    layers: &layer::Layers,
    count: usize,
) -> (Vec<layer::Configured>, u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut got, mut waits) = (Vec::new(), 0);
    while got.len() < count {
        assert!(
            Instant::now() < deadline,
            "{} of {count} configures",
            got.len()
        );
        conn.wait(Some(Duration::from_secs(5))).unwrap();
        waits += 1;
        got.extend(layers.take());
    }
    (got, waits)
}

#[test]
fn an_extension_role_draws_through_the_lease_and_acknowledges_through_its_own_protocol() {
    let (mut conn, _control, seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (200, 100));
    assert_eq!(
        conn.capabilities().extensions(),
        [Extension {
            interface: "zwlr_layer_shell_v1",
            version: 4
        }]
    );
    let (configures, _) = layer_configures(&mut conn, &layers, 1);
    assert_eq!(configures[0].size, (200, 100));
    {
        let lease = conn.lease(surface, (1, 1), BufferFormat::Argb8888).unwrap();
        assert_eq!(lease.logical_size(), (200, 100));
        lease.present().unwrap();
    }
    assert_eq!(next_seen(&seen), Seen::LayerAck(configures[0].serial));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (200, 100),
            scale: 1,
            format: wl_shm::Format::Argb8888
        }
    );
    let placement = PopupPlacement {
        anchor_rect: Rect::new(10.0, 0.0, 20.0, 20.0),
        size: (80, 40),
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        grab: false,
    };
    let popup = conn.popup(surface, &placement).unwrap();
    conn.flush().unwrap();
    assert!(matches!(next_seen(&seen), Seen::Positioner { .. }));
    assert_eq!(next_seen(&seen), Seen::LayerPopup);
    let mut stash = Stash::new();
    let configure = wait_for(&mut conn, &mut stash, |e| match e {
        Event::PopupConfigure {
            popup: p,
            configure,
        } if *p == popup => Some(*configure),
        _ => None,
    });
    assert_eq!((configure.width, configure.height), (80, 40));
    conn.destroy(surface).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_seen(&seen), Seen::LayerDestroyed);
    assert!(matches!(
        conn.lease(popup.surface(), (1, 1), BufferFormat::Argb8888),
        Err(Error::UnknownSurface)
    ));
}

#[test]
fn a_zero_dimension_of_an_extension_configure_is_left_to_the_client() {
    let (mut conn, _control, _seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (0, 30));
    layer_configures(&mut conn, &layers, 1);
    let lease = conn
        .lease(surface, (640, 1), BufferFormat::Argb8888)
        .unwrap();
    assert_eq!(lease.logical_size(), (640, 30));
}

#[test]
fn an_extension_the_compositor_does_not_offer_binds_nothing_and_is_not_listed() {
    let (mut conn, _control, _seen) = connect(Globals::core());
    let shell: Option<
        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1,
    > = conn.bind_extension(1..=4, layer::ShellData);
    assert!(shell.is_none());
    assert_eq!(conn.capabilities().extensions(), []);
}

#[test]
fn a_burst_of_extension_events_ends_one_wait_not_one_per_event() {
    let (mut conn, control, _seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (200, 100));
    layer_configures(&mut conn, &layers, 1);
    control.send(Control::LayerConfigures(100)).unwrap();
    let (configures, waits) = layer_configures(&mut conn, &layers, 100);
    // the burst may straddle a socket read; it never takes a wait per event
    assert!(waits <= 3, "{waits} waits for 100 configures");
    assert_eq!(
        configures.iter().map(|c| c.size.0).collect::<Vec<_>>(),
        (201..=300).collect::<Vec<_>>()
    );
    let latest = configures.last().unwrap();
    assert_eq!(latest.surface, surface);
    let lease = conn.lease(surface, (1, 1), BufferFormat::Argb8888).unwrap();
    assert_eq!(
        (lease.serial(), lease.logical_size()),
        (latest.serial, (300, 100))
    );
}
