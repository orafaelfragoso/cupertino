//! The connection: globals, surfaces with their roles and buffers, seat input and the event loop.

use std::any::Any;
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::ops::RangeInclusive;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_geometry::{Point, Rect};
use rustix::event::{poll, PollFd, PollFlags};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_keyboard::{self, KeyState, WlKeyboard};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::{self, Axis, ButtonState, WlPointer};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{
    delegate_noop, Connection as WlConnection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    self, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_imported_v2::{self, ZxdgImportedV2};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_importer_v2::ZxdgImporterV2;
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity, XdgPositioner,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, ResizeEdge, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

use crate::buffer::{pick_slot, Slot, SlotChoice, SlotTag, MAX_SLOTS};
use crate::capability::REQUIRED;
use crate::clipboard::Clipboard;
use crate::configure::{wire_size, Serials};
use crate::cursor::{CursorTheme, ThemedCursor};
use crate::input::{
    InputClock, Modifiers, PointerTracker, Repeat, TouchEvent, TouchTracker, Translator,
};
use crate::output::{identification, physical_size, OutputBuilder, OutputId, OutputTransform};
use crate::scale::ScaleSources;
use crate::text_input::TextInput;
use wayland_client::protocol::wl_data_device_manager::DndAction as DragActions;

use crate::{
    BufferFormat, BufferLayout, Capability, CapabilitySet, ClipboardOffer, Decorations, Disconnect,
    DragEvent, DragIcon, DragOffer, DragSource, DragSourceEvent, Error, Extension, KeyboardEvent,
    OutputInfo, PixelRect, PointerEvent, Presented, ReadId, Scale, TextInputEvent, TextInputState,
    WindowConfigure, WindowStates, WmCapabilities,
};

/// A surface of this connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(pub(crate) u32);

/// An application window (`xdg_toplevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Toplevel(SurfaceId);

impl Toplevel {
    pub fn surface(self) -> SurfaceId {
        self.0
    }
}

/// A popup (`xdg_popup`) of a window or of another popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Popup(SurfaceId);

impl Popup {
    pub fn surface(self) -> SurfaceId {
        self.0
    }
}

/// How a new application window starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToplevelConfig {
    pub app_id: String,
    pub title: String,
    /// The smallest and largest size the window can take, in logical points; `None` for no limit.
    pub min_size: Option<(u32, u32)>,
    pub max_size: Option<(u32, u32)>,
    /// Who should draw the frame. A compositor without the decoration protocol, or one that answers otherwise,
    /// decides; the outcome is in [`WindowConfigure::decorations`].
    pub decorations: Decorations,
    /// The handle another client exported its toplevel under (xdg-foreign-unstable-v2), which the window opens as a
    /// child of, as a dialog of that client's window. A compositor without the protocol, or one that does not know the
    /// handle, leaves the window without a parent.
    pub foreign_parent: Option<String>,
}

/// Where a popup goes, relative to its parent's surface (`xdg_positioner`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupPlacement {
    /// The rectangle of the parent the popup is placed against, in logical points.
    pub anchor_rect: Rect,
    /// The popup's size in logical points.
    pub size: (u32, u32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    /// How the compositor may move the popup when it would not fit.
    pub constraint_adjustment: ConstraintAdjustment,
    /// Moves the placed popup by this many logical points, before the compositor's constraint adjustment.
    pub offset: (i32, i32),
    /// Place the popup again when its parent moves or resizes, answered by an [`Event::PopupConfigure`]. Needs
    /// `xdg_wm_base` version 3; an older compositor leaves the popup where it is.
    pub reactive: bool,
    /// The parent's size in logical points that the anchor rectangle is relative to, when it is changing: the size the
    /// parent draws at in answer to its pending configure. Needs `xdg_wm_base` version 3, like `reactive`.
    pub parent_size: Option<(u32, u32)>,
    /// Take an explicit grab with the latest input event, so a click outside dismisses the popup.
    pub grab: bool,
}

/// Where a popup was placed: relative to its parent, in logical points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupConfigure {
    pub serial: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The compositor said a surface may draw its next frame (`wl_surface.frame`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCallback {
    pub surface: SurfaceId,
    /// Milliseconds on the compositor's clock.
    pub time: u32,
}

/// Something the application reacts to; returned by [`Connection::wait`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The compositor configured a window; also the first configure that maps it. Draw for it and present.
    Configure {
        window: Toplevel,
        configure: WindowConfigure,
    },
    /// The compositor placed a popup. Draw at its size and present.
    PopupConfigure {
        popup: Popup,
        configure: PopupConfigure,
    },
    /// The request to close the window: the application decides and destroys it.
    Close(Toplevel),
    /// The compositor dismissed the popup; the application destroys it.
    PopupDone(Popup),
    /// The compositor took the placement of [`Connection::reposition_popup`] that returned `token`; its
    /// [`Event::PopupConfigure`] follows.
    PopupRepositioned {
        popup: Popup,
        token: u32,
    },
    Frame(FrameCallback),
    /// A presented frame reached the screen (with [`Capability::Presentation`]).
    Presented(Presented),
    /// A presented frame of the surface was never shown, such as one replaced before the screen's next refresh (with
    /// [`Capability::Presentation`]).
    Discarded(SurfaceId),
    /// The surface's buffer scale changed: draw again.
    ScaleChanged {
        surface: SurfaceId,
        scale: Scale,
    },
    /// `time` is the compositor's input timestamp in milliseconds (an arbitrary base, wrapping): the event's own, or
    /// for one that carries none (enter, leave, a repeated key) the latest one advanced by the time since.
    Pointer {
        surface: SurfaceId,
        time: u32,
        event: PointerEvent,
    },
    Touch {
        surface: SurfaceId,
        time: u32,
        event: TouchEvent,
    },
    Keyboard {
        surface: SurfaceId,
        time: u32,
        event: KeyboardEvent,
    },
    /// The input method on a surface; see [`Connection::set_text_input`].
    TextInput {
        surface: SurfaceId,
        event: TextInputEvent,
    },
    /// Another selection was set, or it was cleared.
    Selection(Option<ClipboardOffer>),
    /// Another client's drag over a surface of this client.
    Drag {
        surface: SurfaceId,
        event: DragEvent,
    },
    /// How a drag this client started goes.
    DragSource {
        source: DragSource,
        event: DragSourceEvent,
    },
    /// What a read of [`Connection::receive_async`] or [`Connection::receive_drop`] gave; `None` when it was cancelled
    /// or failed.
    Received {
        id: ReadId,
        data: Option<Vec<u8>>,
    },
    /// An output appeared, went away or changed: read [`Connection::outputs`] again.
    OutputsChanged,
    /// The token asked for with [`Connection::request_activation_token`].
    ActivationToken {
        request: TokenRequest,
        token: String,
    },
    /// What an extension's `Dispatch` impl queued with [`State::emit`], in order with the events around it.
    Extension(ExtensionEvent),
}

/// A value an extension queued with [`State::emit`]; the extension reads it back with [`ExtensionEvent::downcast`].
/// Equal only to clones of itself.
#[derive(Clone)]
pub struct ExtensionEvent(Arc<dyn Any + Send + Sync>);

impl ExtensionEvent {
    /// The value, when it is a `T`; else the event itself, for another extension.
    pub fn downcast<T: Any + Send + Sync>(self) -> Result<Arc<T>, ExtensionEvent> {
        self.0.downcast().map_err(ExtensionEvent)
    }

    /// The value, when it is a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for ExtensionEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtensionEvent(..)")
    }
}

impl PartialEq for ExtensionEvent {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A request for an activation token, matched to its [`Event::ActivationToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TokenRequest(u32);

/// The input device a [`HeldPress`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PressDevice {
    /// A pointer button.
    Pointer,
    /// A finger on a touch screen.
    Touch,
}

/// A press the seat still holds: the serial of the button press or touch down that started its implicit grab (the
/// first of the buttons or fingers still down), which a window move, resize or window menu answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HeldPress {
    pub serial: u32,
    pub device: PressDevice,
}

/// Interrupts [`Connection::wait`] from other threads.
#[derive(Clone, Debug)]
pub struct Waker(Arc<UnixStream>);

impl Waker {
    /// Make the next or current [`Connection::wait`] return.
    pub fn wake(&self) {
        match (&*self.0).write(&[1]) {
            // a full channel already holds a pending wake-up
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            // the connection is gone, so nothing waits to be woken: a task dropped after it may still wake
            Err(e) if e.kind() == ErrorKind::BrokenPipe => {}
            Err(e) => panic!("wake channel broken: {e}"),
        }
    }
}

struct Output {
    registry_name: u32,
    wl: WlOutput,
    xdg: Option<ZxdgOutputV1>,
    info: OutputBuilder,
}

/// The toplevel configure being assembled until its `xdg_surface.configure`.
#[derive(Clone, Copy, Debug, Default)]
struct PendingToplevel {
    size: Option<(u32, u32)>,
    bounds: Option<(u32, u32)>,
    states: WindowStates,
    capabilities: Option<WmCapabilities>,
}

enum Role {
    Toplevel {
        xdg: XdgSurface,
        toplevel: XdgToplevel,
        decoration: Option<ZxdgToplevelDecorationV1>,
        decorations: Decorations,
        /// The other client's toplevel this one is a child of, until the compositor ends the import.
        imported: Option<ZxdgImportedV2>,
        pending: PendingToplevel,
        current: Option<WindowConfigure>,
    },
    Popup {
        xdg: XdgSurface,
        popup: XdgPopup,
        parent: SurfaceId,
        pending: Option<(i32, i32, u32, u32)>,
        current: Option<PopupConfigure>,
    },
    External {
        role: Box<dyn ExternalRole>,
        /// The size of the latest [`State::configure_external`]; a 0 dimension is left to the client.
        size: (u32, u32),
    },
    /// Part of `parent`, drawn at its scale; its state applies with the parent's commit.
    Subsurface {
        subsurface: WlSubsurface,
        parent: SurfaceId,
        /// The logical size its buffers are drawn at, from [`Connection::place_subsurface`].
        size: (u32, u32),
        /// Its position in the parent's coordinates, as last sent.
        position: (i32, i32),
    },
}

/// The role an extension gives a surface of [`Connection::external_surface`] through a protocol of its own, such as
/// a layer surface. The connection calls it where the role's requests belong in the surface's lifecycle.
pub trait ExternalRole {
    /// Acknowledge the configure `serial`, which the frame being presented answers. Called before the commit.
    fn ack_configure(&self, serial: u32);

    /// Destroy the role's objects. Called before the surface is destroyed.
    fn destroy(&self);

    /// Make `popup`, created without a parent, a popup of this surface. Called before the popup's first commit. Roles
    /// without popups keep the default, which refuses.
    fn adopt_popup(&self, popup: &XdgPopup) -> Result<(), Error> {
        let _ = popup;
        Err(Error::Protocol("this surface role takes no popups"))
    }

    /// Whether the surface commits once without a buffer, after the role is assigned, to ask for its first configure.
    /// Roles the compositor configures unasked, and that forbid a commit before the first acknowledgement, return
    /// `false`.
    fn initial_commit(&self) -> bool {
        true
    }
}

struct Surface {
    id: SurfaceId,
    wl: WlSurface,
    role: Role,
    viewport: Option<WpViewport>,
    /// The viewport shows a crop of the buffer: the last frame was magnified ([`BufferLease::set_zoom`]).
    zoomed: bool,
    /// The one-pixel buffer of the colour the surface last showed all over ([`Connection::present_solid`]), with that
    /// colour as the protocol takes it (premultiplied, each component scaled to the whole `u32` range).
    solid: Option<(WlBuffer, [u32; 4])>,
    fractional: Option<WpFractionalScaleV1>,
    fractional_scale: Option<Scale>,
    preferred_scale: Option<Scale>,
    entered: Vec<OutputId>,
    /// The scale last reported to the application, which buffers are drawn at.
    scale: Scale,
    serials: Serials,
    slots: Vec<Slot>,
    next_slot: u64,
}

/// Dispatch state of the connection. Extensions implement `Dispatch` for it with user data of their own type, so their
/// objects share the connection's queue; see the crate README.
pub struct State {
    capabilities: CapabilitySet,
    compositor: WlCompositor,
    shm: WlShm,
    wm_base: XdgWmBase,
    subcompositor: Option<WlSubcompositor>,
    seat: Option<WlSeat>,
    decoration_manager: Option<ZxdgDecorationManagerV1>,
    importer: Option<ZxdgImporterV2>,
    scaling: Option<(WpFractionalScaleManagerV1, WpViewporter)>,
    /// The viewporter on its own, which also stretches a solid colour's pixel where the compositor has no fractional
    /// scaling.
    viewporter: Option<WpViewporter>,
    single_pixel: Option<WpSinglePixelBufferManagerV1>,
    activation: Option<XdgActivationV1>,
    cursor_shape: Option<WpCursorShapeManagerV1>,
    themed_cursor: ThemedCursor,
    presentation: Option<WpPresentation>,
    pub(crate) presentation_clock: Option<u32>,
    output_manager: Option<ZxdgOutputManagerV1>,
    data_device_manager: Option<WlDataDeviceManager>,
    pub(crate) clipboard: Clipboard,
    pub(crate) text_input: TextInput,
    pointer: Option<WlPointer>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    pointer_enter_serial: Option<u32>,
    /// The buttons held down, and the serial of the press that started the implicit grab they hold: the first of them
    /// pressed while none was down, which a drag, move, resize or window menu answers.
    held_buttons: Vec<u32>,
    press_serial: Option<u32>,
    touch: Option<WlTouch>,
    /// The touch points down, and the serial of the touch down that started their implicit grab.
    held_touches: Vec<i32>,
    touch_press_serial: Option<u32>,
    /// The device whose implicit grab started last, which a move answers when both are held.
    latest_press: PressDevice,
    touch_tracker: TouchTracker,
    keyboard: Option<WlKeyboard>,
    outputs: Vec<Output>,
    surfaces: Vec<Surface>,
    next_surface: u32,
    /// The token of the latest `xdg_popup.reposition`.
    next_reposition: u32,
    next_token: u32,
    pointer_tracker: PointerTracker,
    key_focus: Option<SurfaceId>,
    translator: Option<Translator>,
    repeat: Repeat,
    last_serial: Option<u32>,
    input_clock: InputClock,
    pub(crate) events: Vec<Event>,
    fault: Option<Error>,
}

/// A connection to a Wayland compositor using standard protocols only. One loop calls [`Connection::wait`] and
/// reacts to the returned [`Event`]s.
pub struct Connection {
    conn: WlConnection,
    globals: GlobalList,
    queue: EventQueue<State>,
    state: State,
    wake: UnixStream,
    waker: Waker,
}

fn bind<I>(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    versions: std::ops::RangeInclusive<u32>,
) -> Option<I>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    globals.bind(qh, versions, ()).ok()
}

/// A global [`CapabilitySet::from_globals`] already found in a usable version.
fn required<I>(global: Option<I>, interface: &'static str) -> Result<I, Error> {
    global.ok_or_else(|| Error::MissingRequiredProtocol {
        interface,
        minimum_version: REQUIRED
            .iter()
            .find(|r| r.interface == interface)
            .map_or(1, |r| r.minimum_version),
    })
}

impl Connection {
    /// Connect to the compositor named by `WAYLAND_DISPLAY` (or the socket in `WAYLAND_SOCKET`).
    pub fn connect() -> Result<Connection, Error> {
        Self::from_wayland(
            WlConnection::connect_to_env()
                .map_err(|e| Error::Disconnected(Disconnect::Connect(e)))?,
        )
    }

    /// Connect over an established socket, such as one end of a socket pair.
    pub fn from_stream(stream: UnixStream) -> Result<Connection, Error> {
        Self::from_wayland(
            WlConnection::from_socket(stream)
                .map_err(|e| Error::Disconnected(Disconnect::Connect(e)))?,
        )
    }

    /// Take over an established connection, for a process that runs event queues of its own on it next to this one.
    pub fn from_wayland(conn: WlConnection) -> Result<Connection, Error> {
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| Error::Disconnected(Disconnect::Registry(e)))?;
        let qh = queue.handle();
        let capabilities = CapabilitySet::from_globals(
            globals
                .contents()
                .clone_list()
                .iter()
                .map(|g| (g.interface.as_str(), g.version))
                .collect::<Vec<_>>(),
        )?;
        // version 8 adds `axis_value120`, version 9 `axis_relative_direction`
        let seat: Option<WlSeat> = bind(&globals, &qh, 1..=9);
        let data_device_manager = bind(&globals, &qh, 1..=3);
        let clipboard = Clipboard::new(data_device_manager.as_ref(), seat.as_ref(), &qh);
        let text_input_manager: Option<ZwpTextInputManagerV3> = bind(&globals, &qh, 1..=1);
        let text_input = TextInput::new(text_input_manager.as_ref(), seat.as_ref(), &qh);
        // version 6 adds `preferred_buffer_scale`
        let compositor: WlCompositor = required(bind(&globals, &qh, 4..=6), "wl_compositor")?;
        let shm: WlShm = required(bind(&globals, &qh, 1..=1), "wl_shm")?;
        let fractional: Option<WpFractionalScaleManagerV1> = bind(&globals, &qh, 1..=1);
        let viewporter: Option<WpViewporter> = bind(&globals, &qh, 1..=1);
        let mut state = State {
            capabilities,
            themed_cursor: ThemedCursor::new(compositor.clone(), shm.clone()),
            compositor,
            shm,
            // version 4 adds `configure_bounds`, version 6 the suspended state
            wm_base: required(bind(&globals, &qh, 1..=6), "xdg_wm_base")?,
            subcompositor: bind(&globals, &qh, 1..=1),
            seat,
            decoration_manager: bind(&globals, &qh, 1..=1),
            importer: bind(&globals, &qh, 1..=1),
            scaling: fractional.zip(viewporter.clone()),
            viewporter,
            single_pixel: bind(&globals, &qh, 1..=1),
            activation: bind(&globals, &qh, 1..=1),
            cursor_shape: bind(&globals, &qh, 1..=1),
            // version 2 allows a refresh rate picked for a variable one
            presentation: bind(&globals, &qh, 1..=2),
            presentation_clock: None,
            // version 3 sends the logical geometry before `wl_output.done` instead of a `done` of its own
            output_manager: bind(&globals, &qh, 1..=3),
            data_device_manager,
            clipboard,
            text_input,
            pointer: None,
            cursor_device: None,
            pointer_enter_serial: None,
            held_buttons: Vec::new(),
            press_serial: None,
            touch: None,
            held_touches: Vec::new(),
            touch_press_serial: None,
            latest_press: PressDevice::Pointer,
            keyboard: None,
            outputs: Vec::new(),
            surfaces: Vec::new(),
            next_surface: 0,
            next_reposition: 0,
            next_token: 0,
            pointer_tracker: PointerTracker::default(),
            touch_tracker: TouchTracker::default(),
            key_focus: None,
            translator: None,
            repeat: Repeat::default(),
            last_serial: None,
            input_clock: InputClock::default(),
            events: Vec::new(),
            fault: None,
        };
        let registry = globals.registry();
        for g in globals
            .contents()
            .clone_list()
            .iter()
            .filter(|g| g.interface == "wl_output")
        {
            state.bind_output(registry, &qh, g.name, g.version);
        }
        // outputs and seat capabilities report before the first surface exists
        queue.roundtrip(&mut state)?;
        state.events.clear();
        let (wake, writer) = UnixStream::pair().map_err(Error::Io)?;
        wake.set_nonblocking(true).map_err(Error::Io)?;
        writer.set_nonblocking(true).map_err(Error::Io)?;
        Ok(Connection {
            conn,
            globals,
            queue,
            state,
            wake,
            waker: Waker(Arc::new(writer)),
        })
    }

    /// The optional protocols the compositor offers.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.state.capabilities
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// The clock of [`Presented::time_ns`]: a `clockid_t` such as `CLOCK_MONOTONIC`, to read with `clock_gettime`.
    /// `None` without [`Capability::Presentation`].
    pub fn presentation_clock(&self) -> Option<u32> {
        self.state.presentation_clock
    }

    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state
            .outputs
            .iter()
            .map(|o| o.info.current.clone())
            .collect()
    }

    /// The buffer scale of a surface: what [`Connection::lease`] draws at.
    pub fn scale(&self, surface: SurfaceId) -> Result<Scale, Error> {
        Ok(self.state.surface(surface)?.scale)
    }

    /// Create an application window. It maps with its first presented buffer, after its first
    /// [`Event::Configure`].
    pub fn toplevel(&mut self, config: &ToplevelConfig) -> Result<Toplevel, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let xdg = s.wm_base.get_xdg_surface(&wl, &qh, id);
        let toplevel = xdg.get_toplevel(&qh, id);
        toplevel.set_app_id(config.app_id.clone());
        toplevel.set_title(config.title.clone());
        if let Some((w, h)) = config.min_size {
            toplevel.set_min_size(clamp_i32(w), clamp_i32(h));
        }
        if let Some((w, h)) = config.max_size {
            toplevel.set_max_size(clamp_i32(w), clamp_i32(h));
        }
        let decoration = s.decoration_manager.as_ref().map(|m| {
            let d = m.get_toplevel_decoration(&toplevel, &qh, id);
            d.set_mode(decoration_mode(config.decorations));
            d
        });
        let imported =
            config
                .foreign_parent
                .as_ref()
                .zip(s.importer.as_ref())
                .map(|(handle, importer)| {
                    let imported = importer.import_toplevel(handle.clone(), &qh, id);
                    imported.set_parent_of(&wl);
                    imported
                });
        s.add_surface(
            id,
            wl,
            Role::Toplevel {
                xdg,
                toplevel,
                decoration,
                decorations: Decorations::Client,
                imported,
                pending: PendingToplevel::default(),
                current: None,
            },
            &qh,
        );
        Ok(Toplevel(id))
    }

    /// Create a popup of `parent` (a window or popup). It maps with its first presented buffer, after its first
    /// [`Event::PopupConfigure`].
    pub fn popup(&mut self, parent: SurfaceId, placement: &PopupPlacement) -> Result<Popup, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let parent_xdg = match &s.surface(parent)?.role {
            Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => Some(xdg.clone()),
            Role::External { .. } => None,
            Role::Subsurface { .. } => {
                return Err(Error::Protocol("a subsurface takes no popups"));
            }
        };
        let positioner = positioner(&s.wm_base, &qh, placement, s.surface(parent)?.serials)?;
        let grab_serial = if placement.grab {
            Some(
                s.last_serial
                    .ok_or(Error::PermissionDenied("a popup grab"))?,
            )
        } else {
            None
        };
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let xdg = s.wm_base.get_xdg_surface(&wl, &qh, id);
        let popup = xdg.get_popup(parent_xdg.as_ref(), &positioner, &qh, id);
        positioner.destroy();
        if let Role::External { role, .. } = &s.surface(parent)?.role {
            if let Err(e) = role.adopt_popup(&popup) {
                popup.destroy();
                xdg.destroy();
                wl.destroy();
                return Err(e);
            }
        }
        if let (Some(serial), Some(seat)) = (grab_serial, s.seat.as_ref()) {
            popup.grab(seat, serial);
        }
        s.add_surface(
            id,
            wl,
            Role::Popup {
                xdg,
                popup,
                parent,
                pending: None,
                current: None,
            },
            &qh,
        );
        Ok(Popup(id))
    }

    /// Move or resize `popup` to `placement` (its grab and `placement.grab` are left as they are); the compositor answers
    /// with [`Event::PopupRepositioned`] carrying the returned token, then a new [`Event::PopupConfigure`]. Needs
    /// `xdg_wm_base` version 3.
    pub fn reposition_popup(
        &mut self,
        popup: Popup,
        placement: &PopupPlacement,
    ) -> Result<u32, Error> {
        const REPOSITION_VERSION: u32 = 3;
        let qh = self.queue.handle();
        let s = &mut self.state;
        if s.wm_base.version() < REPOSITION_VERSION {
            return Err(Error::MissingRequiredProtocol {
                interface: "xdg_wm_base",
                minimum_version: REPOSITION_VERSION,
            });
        }
        let Role::Popup { popup, parent, .. } = &s.surface(popup.0)?.role else {
            unreachable!("a Popup handle names a popup");
        };
        let (popup, parent) = (popup.clone(), *parent);
        let positioner = positioner(&s.wm_base, &qh, placement, s.surface(parent)?.serials)?;
        s.next_reposition += 1;
        popup.reposition(&positioner, s.next_reposition);
        positioner.destroy();
        Ok(s.next_reposition)
    }

    /// Destroy a surface, with its popups first.
    pub fn destroy(&mut self, surface: SurfaceId) -> Result<(), Error> {
        self.state.surface(surface)?;
        let children: Vec<SurfaceId> = self
            .state
            .surfaces
            .iter()
            .filter_map(|s| match s.role {
                Role::Popup { parent, .. } | Role::Subsurface { parent, .. }
                    if parent == surface =>
                {
                    Some(s.id)
                }
                _ => None,
            })
            .collect();
        for child in children {
            self.destroy(child)?;
        }
        let i = self
            .state
            .surfaces
            .iter()
            .position(|s| s.id == surface)
            .ok_or(Error::UnknownSurface)?;
        let s = self.state.surfaces.remove(i);
        self.state.clipboard.forget_surface(surface);
        match &s.role {
            Role::Toplevel {
                xdg,
                toplevel,
                decoration,
                imported,
                ..
            } => {
                if let Some(d) = decoration {
                    d.destroy();
                }
                if let Some(i) = imported {
                    i.destroy();
                }
                toplevel.destroy();
                xdg.destroy();
            }
            Role::Popup { xdg, popup, .. } => {
                popup.destroy();
                xdg.destroy();
            }
            Role::External { role, .. } => role.destroy(),
            Role::Subsurface { subsurface, .. } => subsurface.destroy(),
        }
        if let Some(v) = &s.viewport {
            v.destroy();
        }
        if let Some(f) = &s.fractional {
            f.destroy();
        }
        s.wl.destroy();
        if let Some((buffer, _)) = &s.solid {
            buffer.destroy();
        }
        self.state.pointer_tracker.forget(surface);
        self.state.touch_tracker.forget(surface);
        self.state.text_input.forget(surface);
        if self.state.key_focus == Some(surface) {
            self.state.key_focus = None;
            self.state.repeat.clear();
        }
        Ok(())
    }

    pub fn set_title(&mut self, window: Toplevel, title: &str) -> Result<(), Error> {
        self.state.toplevel(window)?.set_title(title.to_string());
        Ok(())
    }

    /// The smallest and largest size, in logical points; `None` for no limit. Applies with the next present.
    pub fn set_size_limits(
        &mut self,
        window: Toplevel,
        min: Option<(u32, u32)>,
        max: Option<(u32, u32)>,
    ) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        let (w, h) = min.unwrap_or((0, 0));
        t.set_min_size(clamp_i32(w), clamp_i32(h));
        let (w, h) = max.unwrap_or((0, 0));
        t.set_max_size(clamp_i32(w), clamp_i32(h));
        Ok(())
    }

    pub fn set_maximized(&mut self, window: Toplevel, maximized: bool) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        if maximized {
            t.set_maximized();
        } else {
            t.unset_maximized();
        }
        Ok(())
    }

    pub fn set_fullscreen(&mut self, window: Toplevel, fullscreen: bool) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        if fullscreen {
            t.set_fullscreen(None);
        } else {
            t.unset_fullscreen();
        }
        Ok(())
    }

    pub fn minimize(&mut self, window: Toplevel) -> Result<(), Error> {
        self.state.toplevel(window)?.set_minimized();
        Ok(())
    }

    /// Start moving the window with the pointer or finger, in answer to the press still held ([`Connection::held_press`]),
    /// whatever pointer motion or keys came since (a client-drawn title bar). With no press held nothing is sent and the
    /// move is [`Error::PermissionDenied`].
    pub fn start_move(&mut self, window: Toplevel) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "moving a window without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?._move(&seat, serial);
        Ok(())
    }

    /// Show the compositor's window menu at `position` (window-local logical points), in answer to the press still held
    /// ([`Connection::held_press`]: a right click on a client-drawn title bar). With no press held nothing is sent and
    /// the menu is [`Error::PermissionDenied`].
    pub fn show_window_menu(&mut self, window: Toplevel, position: Point) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "a window menu without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?.show_window_menu(
            &seat,
            serial,
            position.x.round() as i32,
            position.y.round() as i32,
        );
        Ok(())
    }

    /// Ask again who draws the frame; the answer arrives with the next [`Event::Configure`]. Without the decoration
    /// protocol the client always does, so asking the compositor is [`Error::Unsupported`].
    pub fn set_decorations(
        &mut self,
        window: Toplevel,
        decorations: Decorations,
    ) -> Result<(), Error> {
        let Role::Toplevel { decoration, .. } = &self.state.surface(window.0)?.role else {
            unreachable!("a Toplevel handle names a toplevel");
        };
        match (decoration, decorations) {
            (Some(d), _) => d.set_mode(decoration_mode(decorations)),
            (None, Decorations::Client) => {}
            (None, Decorations::Server) => {
                return Err(Error::Unsupported(Capability::ServerDecoration))
            }
        }
        Ok(())
    }

    /// Start resizing the window from `edge`, in answer to the press still held ([`Connection::held_press`]: a
    /// client-drawn border). With no press held nothing is sent and the resize is [`Error::PermissionDenied`].
    pub fn start_resize(&mut self, window: Toplevel, edge: ResizeEdge) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "resizing a window without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?.resize(&seat, serial, edge);
        Ok(())
    }

    /// Ask the compositor to focus the window (`xdg_activation_v1`), with the latest input event as the reason.
    pub fn activate(&mut self, window: Toplevel) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let qh = self.queue.handle();
        let s = &self.state;
        let surface = s.surface(window.0)?;
        let activation = s.activation.as_ref().expect("bound with the capability");
        let token = activation.get_activation_token(&qh, TokenUse::Activate(window.0));
        if let (Some(serial), Some(seat)) = (s.last_serial, s.seat.as_ref()) {
            token.set_serial(serial, seat);
        }
        token.set_surface(&surface.wl);
        token.commit();
        Ok(())
    }

    /// Activate an existing window with a token forwarded by its launcher or a second instance.
    /// The compositor validates and consumes the token; this does not mint an input grant.
    pub fn activate_with_token(&mut self, window: Toplevel, token: &str) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let surface = self.state.surface(window.0)?;
        self.state
            .activation
            .as_ref()
            .expect("bound with the capability")
            .activate(token.to_owned(), &surface.wl);
        Ok(())
    }

    /// Take pointer input only inside `rects` (surface-local logical points, rounded outwards), or over the whole surface
    /// with `None`; input elsewhere reaches the surfaces below, and an empty slice lets all of it through. Applies with
    /// the next present.
    pub fn set_input_region(
        &mut self,
        surface: SurfaceId,
        rects: Option<&[Rect]>,
    ) -> Result<(), Error> {
        let wl = &self.state.surface(surface)?.wl;
        let Some(rects) = rects else {
            wl.set_input_region(None);
            return Ok(());
        };
        let region = self
            .state
            .compositor
            .create_region(&self.queue.handle(), ());
        for r in rects {
            let (x, y, width, height) = round_out(r);
            region.add(x, y, width, height);
        }
        wl.set_input_region(Some(&region));
        region.destroy();
        Ok(())
    }

    /// Ask for an activation token to hand to another client, such as an application this one launches (in
    /// `XDG_ACTIVATION_TOKEN`), with the latest input event on `surface` as the reason; `app_id` names that application
    /// when known. The token arrives as [`Event::ActivationToken`].
    pub fn request_activation_token(
        &mut self,
        surface: SurfaceId,
        app_id: Option<&str>,
    ) -> Result<TokenRequest, Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let qh = self.queue.handle();
        let s = &mut self.state;
        let wl = s.surface(surface)?.wl.clone();
        s.next_token += 1;
        let request = TokenRequest(s.next_token);
        let activation = s.activation.as_ref().expect("bound with the capability");
        let token = activation.get_activation_token(&qh, TokenUse::Hand(request));
        if let (Some(serial), Some(seat)) = (s.last_serial, s.seat.as_ref()) {
            token.set_serial(serial, seat);
        }
        if let Some(app_id) = app_id {
            token.set_app_id(app_id.to_owned());
        }
        token.set_surface(&wl);
        token.commit();
        Ok(request)
    }

    /// Hide the cursor over the surface the pointer is on, for an application that draws its own.
    pub fn hide_cursor(&mut self) -> Result<(), Error> {
        let (Some(pointer), Some(serial)) = (&self.state.pointer, self.state.pointer_enter_serial)
        else {
            return Err(Error::PermissionDenied(
                "a cursor before the pointer entered",
            ));
        };
        pointer.set_cursor(serial, None, 0, 0);
        self.state.themed_cursor.stop();
        Ok(())
    }

    /// The modifiers held now, as of the last `wl_keyboard.modifiers`; none before the keymap arrives.
    pub fn modifiers(&self) -> Modifiers {
        self.state
            .translator
            .as_ref()
            .map_or(Modifiers::default(), Translator::modifiers)
    }

    /// The serial of the latest input event, for an extension request that answers one; `None` before any input.
    pub fn input_serial(&self) -> Option<u32> {
        self.state.last_serial
    }

    /// The press the seat still holds, for an extension request that must answer it: of the pointer buttons and the
    /// fingers down, whichever started its implicit grab last. `None` once all of them are up, or after the pointer
    /// left or the compositor cancelled the touches.
    pub fn held_press(&self) -> Option<HeldPress> {
        self.state.held_press()
    }

    /// The outputs `surface` is on, in the order it entered them.
    pub fn surface_outputs(&self, surface: SurfaceId) -> Result<Vec<OutputId>, Error> {
        Ok(self.state.surface(surface)?.entered.clone())
    }

    /// Draw the cursors of [`Connection::set_cursor`] from `theme` when the compositor has no cursor shapes, instead of
    /// the user's ([`CursorTheme::from_env`], read when first needed).
    pub fn set_cursor_theme(&mut self, theme: CursorTheme) {
        self.state.themed_cursor.set_theme(theme);
    }

    /// Show the cursor `shape` over the surface the pointer is on: the compositor's with [`Capability::CursorShape`],
    /// else drawn from the cursor theme at the scale of that surface.
    pub fn set_cursor(&mut self, shape: wp_cursor_shape_device_v1::Shape) -> Result<(), Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let (Some(pointer), Some(serial)) = (&s.pointer, s.pointer_enter_serial) else {
            return Err(Error::PermissionDenied(
                "a cursor before the pointer entered",
            ));
        };
        if let Some(device) = &s.cursor_device {
            device.set_shape(serial, shape);
            s.themed_cursor.stop();
            return Ok(());
        }
        let scale = s.cursor_scale();
        s.themed_cursor.show(&qh, pointer, serial, shape, scale)?;
        self.flush()
    }

    /// Tell the input method about the text field focused on `surface`, or with `None` that no field there takes text.
    /// It is sent while the surface has the text-input focus and again after each [`TextInputEvent::Enter`]; an
    /// unchanged state sends nothing. A password or PIN purpose sends no surrounding text and adds the sensitive-data
    /// hint. Answers arrive as [`Event::TextInput`].
    pub fn set_text_input(
        &mut self,
        surface: SurfaceId,
        state: Option<&TextInputState>,
    ) -> Result<(), Error> {
        if state.is_some() {
            self.state.capabilities.require(Capability::TextInput)?;
        }
        self.state.surface(surface)?;
        self.state.text_input.set(surface, state);
        Ok(())
    }

    /// Set the selection to `contents`, one byte string per MIME type, in answer to the latest input event.
    pub fn set_selection(&mut self, contents: Vec<(String, Vec<u8>)>) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        let serial = self
            .state
            .last_serial
            .ok_or(Error::PermissionDenied("setting the selection"))?;
        let qh = self.queue.handle();
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("bound with the capability");
        self.state.clipboard.set(&manager, contents, serial, &qh);
        Ok(())
    }

    /// Read the selection `offer` in `mime`, one of its [`ClipboardOffer::mime_types`]. Blocks for up to one second;
    /// [`Connection::receive_async`] doesn't.
    pub fn receive(&mut self, offer: &ClipboardOffer, mime: &str) -> Result<Vec<u8>, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        self.state.clipboard.receive(&self.conn, offer, mime)
    }

    /// Start reading the selection `offer` in `mime` while [`Connection::wait`] goes on; the data arrives as
    /// [`Event::Received`] with the returned id, at most 16 MiB, or `None` when the offer was replaced, its client sent
    /// nothing for a second, or the read took longer than ten seconds. At most eight reads run at once.
    pub fn receive_async(&mut self, offer: &ClipboardOffer, mime: &str) -> Result<ReadId, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        self.state.clipboard.receive_async(&self.conn, offer, mime)
    }

    /// Start dragging `contents`, offered in each of their MIME types, from `origin` while a pointer button is held,
    /// the target allowed the `actions`; [`Event::DragSource`] tells how it goes. The drag answers that button's
    /// press, whatever keys were pressed since; with no button held it is refused. A drag started before replaces the
    /// last.
    pub fn start_drag(
        &mut self,
        origin: SurfaceId,
        contents: Vec<(String, Vec<u8>)>,
        actions: DragActions,
    ) -> Result<DragSource, Error> {
        self.start_drag_with_icon(origin, contents, actions, None)
    }

    /// [`Connection::start_drag`] with `icon` following the pointer, its hotspot under it. The icon's surface and
    /// buffer live until the drag drops, is cancelled or is replaced by another; a drag the compositor ignores (one
    /// that came after the button went up) ends as [`DragSourceEvent::Cancelled`] once the release arrives, and frees
    /// them too.
    pub fn start_drag_with_icon(
        &mut self,
        origin: SurfaceId,
        contents: Vec<(String, Vec<u8>)>,
        actions: DragActions,
        icon: Option<&DragIcon>,
    ) -> Result<DragSource, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        let serial = self
            .state
            .press_serial
            .ok_or(Error::PermissionDenied("a drag without a button held"))?;
        let qh = self.queue.handle();
        let origin = self.state.surface(origin)?.wl.clone();
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("bound with the capability");
        let source = self.state.clipboard.start_drag(
            &manager,
            (&self.state.compositor, &self.state.shm),
            &origin,
            contents,
            actions,
            icon,
            serial,
            &qh,
        )?;
        self.flush()?;
        Ok(source)
    }

    /// Accept the drag `offer` in `mime`, one of its [`DragOffer::mime_types`], or refuse it with `None`; it may change
    /// until the drop, and only an accepted drop can be read.
    pub fn accept_drag(&mut self, offer: &DragOffer, mime: Option<&str>) -> Result<(), Error> {
        self.state.clipboard.accept_drag(offer, mime)
    }

    /// The `actions` this client takes for the drag `offer`, and the one it prefers; the compositor's choice arrives as
    /// [`DragEvent::Action`].
    pub fn set_drag_actions(
        &mut self,
        offer: &DragOffer,
        actions: DragActions,
        preferred: DragActions,
    ) -> Result<(), Error> {
        self.state
            .clipboard
            .set_drag_actions(offer, actions, preferred)
    }

    /// Start reading the drop of `offer` in `mime`, the type it was accepted in; the data arrives as
    /// [`Event::Received`], with the limits of [`Connection::receive_async`], except that a drop past the size limit
    /// gives `None` rather than its start. Before the drop, or in another type, it is refused.
    pub fn receive_drop(&mut self, offer: &DragOffer, mime: &str) -> Result<ReadId, Error> {
        self.state.clipboard.receive_drop(&self.conn, offer, mime)
    }

    /// Tell the dragging client the drop of `offer` was taken, after reading it.
    pub fn finish_drag(&mut self, offer: &DragOffer) -> Result<(), Error> {
        self.state.clipboard.finish_drag(offer)?;
        self.flush()
    }

    /// Let the drop of `offer` go without taking it, as when its data could not be read or used: the dragging client
    /// hears a cancel (from `wl_data_offer` version 3) and keeps its data. Before the drop it is refused.
    pub fn refuse_drop(&mut self, offer: &DragOffer) -> Result<(), Error> {
        self.state.clipboard.refuse_drop(offer)?;
        self.flush()
    }

    /// The queue extensions create their objects on.
    pub fn queue_handle(&self) -> QueueHandle<State> {
        self.queue.handle()
    }

    /// Bind the global of an extension's protocol at the highest version in `versions` the compositor offers, with
    /// `data` as user data, and list it in [`Connection::capabilities`]; `None` when it offers none in the range, so the
    /// application falls back. Binding through here is what makes an extension visible in the capabilities.
    pub fn bind_extension<I, U>(&mut self, versions: RangeInclusive<u32>, data: U) -> Option<I>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        State: Dispatch<I, U>,
    {
        let proxy: I = self
            .globals
            .bind(&self.queue.handle(), versions, data)
            .ok()?;
        self.state.capabilities.add_extension(Extension {
            interface: I::interface().name,
            version: proxy.version(),
        });
        Some(proxy)
    }

    /// The `wl_surface` of `surface`, for an extension request that names it.
    pub fn wl_surface(&self, surface: SurfaceId) -> Result<WlSurface, Error> {
        Ok(self.state.surface(surface)?.wl.clone())
    }

    /// The seat, when the compositor offers one.
    pub fn seat(&self) -> Option<WlSeat> {
        self.state.seat.clone()
    }

    /// The seat's `wl_pointer` once it has one, for an extension that follows the pointer.
    pub fn pointer(&self) -> Option<WlPointer> {
        self.state.pointer.clone()
    }

    /// The `wl_output` of `output`, for an extension request that names it.
    pub fn wl_output(&self, output: OutputId) -> Option<WlOutput> {
        self.state
            .outputs
            .iter()
            .find(|o| o.info.current.id == output)
            .map(|o| o.wl.clone())
    }

    /// Create a surface whose role an extension gives it: `assign` creates the role's objects for the new `wl_surface`
    /// (with the [`SurfaceId`] as user data where the protocol reports per surface) before its first commit. The
    /// extension reports each configure with [`State::configure_external`]; the surface then draws through
    /// [`Connection::lease`] like any other, with scale, frame callbacks and input.
    pub fn external_surface<R: ExternalRole + 'static>(
        &mut self,
        assign: impl FnOnce(&WlSurface, &QueueHandle<State>, SurfaceId) -> R,
    ) -> SurfaceId {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let role = Box::new(assign(&wl, &qh, id));
        s.add_surface(id, wl, Role::External { role, size: (0, 0) }, &qh);
        id
    }

    /// Create a surface drawn as part of `parent` (a window, popup or external surface), above it, at its top left
    /// corner until [`Connection::place_subsurface`] moves it. It is synchronized: what it presents shows with the
    /// parent's next commit, so the two always change together. It takes no input (its input region is empty), so
    /// the pointer and touches over it reach the parent below, and it is drawn at the parent's scale; it gets no
    /// frame callbacks of its own. It shows nothing until it has a size and presents a buffer, and
    /// [`Connection::unmap_subsurface`] hides it again with the parent's next commit. Needs
    /// [`Capability::Subsurface`].
    pub fn subsurface(&mut self, parent: SurfaceId) -> Result<SurfaceId, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let subcompositor = s
            .subcompositor
            .clone()
            .ok_or(Error::Unsupported(Capability::Subsurface))?;
        let parent_wl = s.surface(parent)?.wl.clone();
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let subsurface = subcompositor.get_subsurface(&wl, &parent_wl, &qh, ());
        subsurface.set_sync();
        let region = s.compositor.create_region(&qh, ());
        wl.set_input_region(Some(&region));
        region.destroy();
        s.add_surface(
            id,
            wl,
            Role::Subsurface {
                subsurface,
                parent,
                size: (0, 0),
                position: (0, 0),
            },
            &qh,
        );
        Ok(id)
    }

    /// Move `surface`, a subsurface, to `position` in its parent's logical coordinates (with the parent's next commit)
    /// and draw its next buffers `size` logical points large.
    pub fn place_subsurface(
        &mut self,
        surface: SurfaceId,
        position: (i32, i32),
        size: (u32, u32),
    ) -> Result<(), Error> {
        let surf = self.state.surface_mut(surface)?;
        let Role::Subsurface {
            subsurface,
            size: current,
            position: at,
            ..
        } = &mut surf.role
        else {
            return Err(Error::Protocol(
                "place_subsurface on a surface that is not a subsurface",
            ));
        };
        if *at != position {
            subsurface.set_position(position.0, position.1);
            *at = position;
        }
        *current = size;
        Ok(())
    }

    /// Hide `surface`, a subsurface, with its parent's next commit; presenting a buffer shows it again.
    pub fn unmap_subsurface(&mut self, surface: SurfaceId) -> Result<(), Error> {
        let surf = self.state.surface_mut(surface)?;
        if !matches!(surf.role, Role::Subsurface { .. }) {
            return Err(Error::Protocol(
                "unmap_subsurface on a surface that is not a subsurface",
            ));
        }
        surf.wl.attach(None, 0, 0);
        surf.wl.commit();
        // the buffers it showed are the compositor's until it releases them; what they hold is still known
        self.flush()
    }

    /// Send the queued requests and dispatch every event the compositor sent in answer, keeping the events for the
    /// next [`Connection::wait`]; for extensions that read state at setup.
    pub fn roundtrip(&mut self) -> Result<(), Error> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    /// Whether [`Connection::lease`] has a buffer for `surface`: `false` while the compositor holds all of them, until
    /// it releases one.
    pub fn buffer_free(&self, surface: SurfaceId) -> Result<bool, Error> {
        let busy = self
            .state
            .surface(surface)?
            .slots
            .iter()
            .filter(|s| s.busy)
            .count();
        Ok(busy < MAX_SLOTS)
    }

    /// A buffer to draw the next frame of `surface` into, at the surface's scale. Its logical size is the configured
    /// one; `preferred` (logical points) applies when the compositor leaves the size to the client. The pixels are
    /// those of an earlier frame or zero; [`BufferLease::missed`] tells which differ from the frame shown last. Dropping
    /// the lease without presenting gives it back.
    pub fn lease(
        &mut self,
        surface: SurfaceId,
        preferred: (u32, u32),
        format: BufferFormat,
    ) -> Result<BufferLease<'_>, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let shm = s.shm.clone();
        // a subsurface is drawn at its parent's scale and answers no configure
        let parent_scale = match &s.surface(surface)?.role {
            Role::Subsurface { parent, .. } => Some(s.surface(*parent)?.scale),
            _ => None,
        };
        let surf = s.surface_mut(surface)?;
        let serial = match surf.role {
            Role::Subsurface { .. } => 0,
            _ => surf.serials.latest().ok_or(Error::NotConfigured)?,
        };
        let logical = logical_size(&surf.role, preferred)?;
        let scale = parent_scale.unwrap_or(surf.scale);
        let layout = scale
            .to_physical(logical.0)
            .zip(scale.to_physical(logical.1))
            .and_then(|(w, h)| BufferLayout::new(w, h, format))
            .ok_or(Error::InvalidScale)?;
        let slot = match pick_slot(surf.slots.iter().map(|s| (s.busy, s.layout)), layout) {
            SlotChoice::Reuse(i) => i,
            SlotChoice::AllBusy => return Err(Error::NoFreeBuffer),
            SlotChoice::Create => {
                surf.slots.retain(|s| s.busy);
                surf.next_slot += 1;
                let tag = SlotTag {
                    surface: surface.0,
                    slot: surf.next_slot,
                };
                surf.slots.push(Slot::new(&shm, &qh, tag, layout)?);
                surf.slots.len() - 1
            }
        };
        Ok(BufferLease {
            connection: self,
            surface,
            slot,
            serial,
            logical,
            scale,
            zoom: 1.0,
        })
    }

    /// Show `surface` as one colour all over without drawing a buffer of its size: one pixel of `rgba` (straight, not
    /// premultiplied, sRGB components from 0 to 1; a transparent one for a surface that only takes input), which the
    /// compositor stretches over the surface's logical size: the configured one, or `preferred` (points) where the
    /// compositor leaves it to the client. Like a present it acknowledges the latest configure and asks for a frame
    /// callback; a pixel of the colour shown last is attached again rather than made anew. The surface's input region,
    /// outputs and scale are untouched, and a new scale needs no new present, the pixel being stretched in points. A
    /// buffer leased afterwards is drawn whole ([`BufferLease::missed`] is `None`). [`Error::Unsupported`] when the
    /// compositor lacks [`Capability::SolidColor`]: draw the colour into a leased buffer instead.
    pub fn present_solid(
        &mut self,
        surface: SurfaceId,
        preferred: (u32, u32),
        rgba: [f32; 4],
    ) -> Result<(), Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let (Some(pixels), Some(viewporter)) = (s.single_pixel.clone(), s.viewporter.clone())
        else {
            return Err(Error::Unsupported(Capability::SolidColor));
        };
        let surf = s.surface_mut(surface)?;
        // a subsurface is drawn into buffers at its parent's scale, with the parent's commit; it has no one-colour path
        if matches!(surf.role, Role::Subsurface { .. }) {
            return Err(Error::Protocol("present_solid on a subsurface"));
        }
        surf.serials.latest().ok_or(Error::NotConfigured)?;
        let logical = logical_size(&surf.role, preferred)?;
        if logical.0 == 0 || logical.1 == 0 {
            return Err(Error::InvalidScale);
        }
        if let Some(serial) = surf.serials.present()? {
            match &surf.role {
                Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => xdg.ack_configure(serial),
                Role::External { role, .. } => role.ack_configure(serial),
                Role::Subsurface { .. } => unreachable!("refused above"),
            }
        }
        let color = single_pixel(rgba);
        let replaced = match &surf.solid {
            Some((_, shown)) if *shown == color => None,
            _ => {
                let [r, g, b, a] = color;
                let buffer = pixels.create_u32_rgba_buffer(r, g, b, a, &qh, ());
                surf.solid.replace((buffer, color)).map(|(old, _)| old)
            }
        };
        let (buffer, _) = surf.solid.as_ref().expect("made above");
        surf.wl.attach(Some(buffer), 0, 0);
        let wl = &surf.wl;
        let viewport = surf
            .viewport
            .get_or_insert_with(|| viewporter.get_viewport(wl, &qh, ()));
        if surf.zoomed {
            viewport.set_source(-1.0, -1.0, -1.0, -1.0);
            surf.zoomed = false;
        }
        viewport.set_destination(clamp_i32(logical.0), clamp_i32(logical.1));
        surf.wl.set_buffer_scale(1);
        surf.wl.damage_buffer(0, 0, 1, 1);
        // the slots no longer hold what is shown
        for slot in &mut surf.slots {
            slot.missed.forget();
        }
        surf.wl.frame(&qh, surface);
        surf.wl.commit();
        // the pixel it replaced is no longer attached
        if let Some(old) = replaced {
            old.destroy();
        }
        self.flush()
    }

    /// Wait for events, at most `timeout` (`None` waits until one arrives or a [`Waker`] wakes the loop). It returns
    /// after each read from the compositor, maybe with no event, once everything read is dispatched, so extensions
    /// find what their objects received.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, Error> {
        // read what arrived while the caller was busy first: a key released meanwhile ends its repeat before the
        // repeats due since are counted
        let woken = self.block(Some(Duration::ZERO))?;
        self.queue.dispatch_pending(&mut self.state)?;
        self.state.repeat_keys(Instant::now());
        self.animate_cursor()?;
        self.state.pump_reads();
        self.update_scales()?;
        if self.state.events.is_empty() && self.state.fault.is_none() && !woken {
            // a key repeat, the next image of an animated cursor or a clipboard read giving up earlier than the
            // caller's timeout ends the wait first
            let now = Instant::now();
            let timers = [
                self.state.repeat.deadline(),
                self.state.themed_cursor.deadline(),
                self.state.clipboard.read_deadline(),
            ];
            let until_timer = timers
                .into_iter()
                .flatten()
                .map(|at| at.saturating_duration_since(now));
            self.block(timeout.into_iter().chain(until_timer).min())?;
            self.queue.dispatch_pending(&mut self.state)?;
            self.state.repeat_keys(Instant::now());
            self.animate_cursor()?;
            self.state.pump_reads();
            self.update_scales()?;
        }
        match self.state.fault.take() {
            Some(e) => Err(e),
            None => Ok(std::mem::take(&mut self.state.events)),
        }
    }

    /// Update the scales of the surfaces; a drawn cursor follows the scale of the surface it is over.
    fn update_scales(&mut self) -> Result<(), Error> {
        self.state.update_scales();
        let qh = self.queue.handle();
        let scale = self.state.cursor_scale();
        if self.state.themed_cursor.rescale(&qh, scale)? {
            self.flush()?;
        }
        Ok(())
    }

    fn animate_cursor(&mut self) -> Result<(), Error> {
        if self.state.themed_cursor.advance(Instant::now()) {
            self.flush()?;
        }
        Ok(())
    }

    /// Send queued requests now; [`Connection::wait`] also does.
    pub fn flush(&self) -> Result<(), Error> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            // the socket is full; the next call flushes again
            Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Poll the connection and the wake channel once and read what is ready; true when a [`Waker`] woke the loop.
    fn block(&mut self, timeout: Option<Duration>) -> Result<bool, Error> {
        let guard = loop {
            match self.queue.prepare_read() {
                Some(guard) => break guard,
                None => {
                    self.queue.dispatch_pending(&mut self.state)?;
                }
            }
        };
        self.flush()?;
        let timespec = timeout.map(|d| rustix::event::Timespec {
            tv_sec: d.as_secs() as i64,
            tv_nsec: i64::from(d.subsec_nanos()),
        });
        let (connection_ready, wake_ready) = {
            let connection_fd = guard.connection_fd();
            // a clipboard read's pipe ends the wait too, to be read with the next pump
            let reads: Vec<_> = self.state.clipboard.read_fds().collect();
            let mut fds: Vec<_> = [
                PollFd::new(&connection_fd, PollFlags::IN),
                PollFd::new(&self.wake, PollFlags::IN),
            ]
            .into_iter()
            .chain(reads.iter().map(|fd| PollFd::new(fd, PollFlags::IN)))
            .collect();
            match poll(&mut fds, timespec.as_ref()) {
                Ok(_) => {}
                // a signal interrupted the wait: report an empty wake-up
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(Error::Io(e.into())),
            }
            (
                fds[0]
                    .revents()
                    .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR),
                fds[1].revents().contains(PollFlags::IN),
            )
        };
        if connection_ready {
            match guard.read() {
                Ok(_) => {}
                // another reader consumed the data
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            drop(guard);
        }
        if wake_ready {
            let mut buf = [0u8; 64];
            loop {
                match (&self.wake).read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => return Err(Error::Io(e)),
                }
            }
        }
        Ok(wake_ready)
    }
}

/// A buffer of a surface leased for drawing one frame; [`BufferLease::present`] commits it.
pub struct BufferLease<'a> {
    connection: &'a mut Connection,
    surface: SurfaceId,
    slot: usize,
    serial: u32,
    logical: (u32, u32),
    scale: Scale,
    zoom: f64,
}

impl BufferLease<'_> {
    /// Show the frame magnified `zoom` times (at least 1) about its centre, scaled by the compositor: the viewport shows
    /// the middle `1 / zoom` of the buffer over the whole surface. `false` when the surface has no viewport (the
    /// compositor lacks `wp_viewporter` or fractional scaling), and the frame shows unmagnified.
    pub fn set_zoom(&mut self, zoom: f64) -> bool {
        self.zoom = zoom.max(1.0);
        self.connection
            .state
            .surface(self.surface)
            .is_ok_and(|s| s.viewport.is_some())
    }

    pub fn layout(&self) -> BufferLayout {
        self.slot_ref().layout
    }

    /// The size of the frame in logical points.
    pub fn logical_size(&self) -> (u32, u32) {
        self.logical
    }

    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// The configure this frame answers.
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// The rectangles in which this buffer differs from the frame shown last: what the frames shown in the other
    /// buffers since it last held one changed. Drawing the next frame needs only them and its own damage. `None` when
    /// its pixels are unknown, such as a new buffer, one of another size or one written but not presented: draw all of
    /// it.
    pub fn missed(&self) -> Option<&[PixelRect]> {
        self.slot_ref().missed.rects()
    }

    /// The pixels to draw the frame into. What [`BufferLease::missed`] says holds until the lease is presented.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        let (surface, slot) = (self.surface, self.slot);
        let slot = &mut self
            .connection
            .state
            .surface_mut(surface)
            .expect("a leased surface lives while the lease does")
            .slots[slot];
        slot.missed.forget();
        slot.pixels_mut()
    }

    fn slot_ref(&self) -> &Slot {
        &self
            .connection
            .state
            .surface(self.surface)
            .expect("a leased surface lives while the lease does")
            .slots[self.slot]
    }

    /// Show the whole buffer.
    pub fn present(self) -> Result<(), Error> {
        self.present_damage(None)
    }

    /// Show the buffer, telling the compositor only `damage` changed since the frame before; `None` for all of it. The
    /// lease holds the connection, so no configure arrives between the lease and the present: the buffer answers the
    /// latest one, which is acknowledged with it.
    pub fn present_damage(self, damage: Option<&[PixelRect]>) -> Result<(), Error> {
        let qh = self.connection.queue.handle();
        let presentation = self.connection.state.presentation.clone();
        let surf = self.connection.state.surface_mut(self.surface)?;
        let subsurface = matches!(surf.role, Role::Subsurface { .. });
        if !subsurface {
            if let Some(serial) = surf.serials.present()? {
                match &surf.role {
                    Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => {
                        xdg.ack_configure(serial)
                    }
                    Role::External { role, .. } => role.ack_configure(serial),
                    Role::Subsurface { .. } => {}
                }
            }
        }
        let layout = surf.slots[self.slot].layout;
        let damage: Option<Vec<PixelRect>> =
            damage.map(|rects| rects.iter().filter_map(|r| r.clamp(layout)).collect());
        for (i, slot) in surf.slots.iter_mut().enumerate() {
            if i == self.slot {
                slot.missed.shown();
            } else if slot.layout == layout {
                slot.missed.passed(damage.as_deref());
            } else {
                slot.missed.forget();
            }
        }
        let slot = &mut surf.slots[self.slot];
        slot.busy = true;
        surf.wl.attach(Some(&slot.buffer), 0, 0);
        match (&surf.viewport, self.scale.integer()) {
            (Some(viewport), _) => {
                surf.wl.set_buffer_scale(1);
                if self.zoom > 1.0 {
                    let (w, h) = (f64::from(layout.width()), f64::from(layout.height()));
                    let (cw, ch) = (w / self.zoom, h / self.zoom);
                    viewport.set_source((w - cw) / 2.0, (h - ch) / 2.0, cw, ch);
                    surf.zoomed = true;
                } else if surf.zoomed {
                    viewport.set_source(-1.0, -1.0, -1.0, -1.0);
                    surf.zoomed = false;
                }
                viewport.set_destination(clamp_i32(self.logical.0), clamp_i32(self.logical.1));
            }
            (None, Some(n)) => surf.wl.set_buffer_scale(n),
            (None, None) => unreachable!("a surface without a viewport is drawn at a whole scale"),
        }
        match damage {
            None => surf.wl.damage_buffer(0, 0, i32::MAX, i32::MAX),
            Some(rects) => {
                for r in rects {
                    surf.wl
                        .damage_buffer(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                }
            }
        }
        // a subsurface shows with its parent's commit, whose frame callback and feedback stand for both
        if !subsurface {
            surf.wl.frame(&qh, self.surface);
            if let Some(presentation) = presentation {
                presentation.feedback(&surf.wl, &qh, self.surface);
            }
        }
        surf.wl.commit();
        self.connection.flush()
    }
}

/// A positioner for `placement`; its anchor rectangle is rounded outwards to whole points. A reactive or resizing
/// placement names the parent's configure not yet acknowledged, from `parent`, which it answers.
fn positioner(
    wm_base: &XdgWmBase,
    qh: &QueueHandle<State>,
    placement: &PopupPlacement,
    parent: Serials,
) -> Result<XdgPositioner, Error> {
    const REACTIVE_VERSION: u32 = 3;
    let r = placement.anchor_rect;
    if !(r.width() > 0.0 && r.height() > 0.0 && placement.size.0 > 0 && placement.size.1 > 0) {
        return Err(Error::Protocol(
            "a popup needs a non-empty anchor rectangle and size",
        ));
    }
    let positioner = wm_base.create_positioner(qh, ());
    positioner.set_size(clamp_i32(placement.size.0), clamp_i32(placement.size.1));
    let (x, y) = (r.x().floor() as i32, r.y().floor() as i32);
    positioner.set_anchor_rect(
        x,
        y,
        ((r.x() + r.width()).ceil() as i32 - x).max(1),
        ((r.y() + r.height()).ceil() as i32 - y).max(1),
    );
    positioner.set_anchor(placement.anchor);
    positioner.set_gravity(placement.gravity);
    positioner.set_constraint_adjustment(placement.constraint_adjustment);
    if placement.offset != (0, 0) {
        positioner.set_offset(placement.offset.0, placement.offset.1);
    }
    if wm_base.version() >= REACTIVE_VERSION {
        if placement.reactive {
            positioner.set_reactive();
        }
        if let Some((w, h)) = placement.parent_size {
            positioner.set_parent_size(clamp_i32(w), clamp_i32(h));
        }
        if placement.reactive || placement.parent_size.is_some() {
            if let Some(serial) = parent.pending() {
                positioner.set_parent_configure(serial);
            }
        }
    }
    Ok(positioner)
}

fn decoration_mode(decorations: Decorations) -> zxdg_toplevel_decoration_v1::Mode {
    match decorations {
        Decorations::Server => zxdg_toplevel_decoration_v1::Mode::ServerSide,
        Decorations::Client => zxdg_toplevel_decoration_v1::Mode::ClientSide,
    }
}

/// The logical size a frame of a surface with `role` takes: the configured one, or `preferred` (points) where the
/// compositor leaves it to the client.
fn logical_size(role: &Role, preferred: (u32, u32)) -> Result<(u32, u32), Error> {
    Ok(match role {
        Role::Toplevel { current, .. } => current.and_then(|c| c.size).unwrap_or(preferred),
        Role::Popup { current, .. } => current
            .map(|c| (c.width, c.height))
            .ok_or(Error::NotConfigured)?,
        Role::External { size: (w, h), .. } => (
            if *w == 0 { preferred.0 } else { *w },
            if *h == 0 { preferred.1 } else { *h },
        ),
        Role::Subsurface { size, .. } if size.0 > 0 && size.1 > 0 => *size,
        Role::Subsurface { .. } => return Err(Error::NotConfigured),
    })
}

/// `rgba` (straight sRGB components from 0 to 1) as a one-pixel buffer takes it: premultiplied by its alpha, each
/// component scaled to the whole `u32` range.
fn single_pixel(rgba: [f32; 4]) -> [u32; 4] {
    let alpha = f64::from(rgba[3]).clamp(0.0, 1.0);
    let wide = |c: f64| (c.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32;
    [
        wide(f64::from(rgba[0]) * alpha),
        wide(f64::from(rgba[1]) * alpha),
        wide(f64::from(rgba[2]) * alpha),
        wide(alpha),
    ]
}

fn clamp_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// `r` as whole points `(x, y, width, height)`, rounded outwards and saturated.
pub(crate) fn round_out(r: &Rect) -> (i32, i32, i32, i32) {
    let (left, top) = (r.min_x().floor() as i32, r.min_y().floor() as i32);
    let (right, bottom) = (r.max_x().ceil() as i32, r.max_y().ceil() as i32);
    (
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

impl State {
    /// Record a configure of an [`Connection::external_surface`]: its `serial`, acknowledged with the next present, and
    /// its logical `size`, where a 0 width or height leaves that dimension to the client. A serial that does not
    /// increase, or a surface that is not external, ends the next [`Connection::wait`] with the error.
    pub fn configure_external(&mut self, surface: SurfaceId, serial: u32, size: (u32, u32)) {
        let surf = match self.surface_mut(surface) {
            Ok(surf) => surf,
            // the application destroyed the surface while the configure was on its way
            Err(_) => return,
        };
        let Role::External { size: current, .. } = &mut surf.role else {
            return self.fail(Error::Protocol(
                "configure_external on a surface without an external role",
            ));
        };
        *current = size;
        if let Err(e) = surf.serials.receive(serial) {
            self.fail(e);
        }
    }

    /// The output an extension event names, as the application knows it; `None` for one this connection has not bound.
    pub fn output_id(&self, output: &WlOutput) -> Option<OutputId> {
        self.outputs
            .iter()
            .find(|o| o.wl == *output)
            .map(|o| o.info.current.id)
    }

    /// Queue `event` for the application as an [`Event::Extension`], after the events dispatched before it.
    pub fn emit(&mut self, event: impl Any + Send + Sync) {
        self.events
            .push(Event::Extension(ExtensionEvent(Arc::new(event))));
    }

    /// End the next [`Connection::wait`] with `error`, such as [`Error::Protocol`] for an event the extension cannot
    /// accept; the first error wins.
    pub fn fail(&mut self, error: Error) {
        self.fault.get_or_insert(error);
    }

    fn next_id(&mut self) -> SurfaceId {
        self.next_surface += 1;
        SurfaceId(self.next_surface)
    }

    fn add_surface(&mut self, id: SurfaceId, wl: WlSurface, role: Role, qh: &QueueHandle<State>) {
        let (viewport, fractional) = match &self.scaling {
            Some((manager, viewporter)) => (
                Some(viewporter.get_viewport(&wl, qh, ())),
                Some(manager.get_fractional_scale(&wl, qh, id)),
            ),
            None => (None, None),
        };
        let configured = match &role {
            Role::External { role, .. } => role.initial_commit(),
            Role::Subsurface { .. } => false,
            Role::Toplevel { .. } | Role::Popup { .. } => true,
        };
        if configured {
            // the initial commit without a buffer asks for the first configure
            wl.commit();
        }
        self.surfaces.push(Surface {
            id,
            wl,
            role,
            viewport,
            zoomed: false,
            solid: None,
            fractional,
            fractional_scale: None,
            preferred_scale: None,
            entered: Vec::new(),
            scale: Scale::ONE,
            serials: Serials::default(),
            slots: Vec::new(),
            next_slot: 0,
        });
    }

    pub(crate) fn has_surface(&self, id: SurfaceId) -> bool {
        self.surface(id).is_ok()
    }

    fn surface(&self, id: SurfaceId) -> Result<&Surface, Error> {
        self.surfaces
            .iter()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownSurface)
    }

    fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface, Error> {
        self.surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownSurface)
    }

    fn toplevel(&self, window: Toplevel) -> Result<&XdgToplevel, Error> {
        match &self.surface(window.0)?.role {
            Role::Toplevel { toplevel, .. } => Ok(toplevel),
            Role::Popup { .. } | Role::External { .. } | Role::Subsurface { .. } => {
                unreachable!("a Toplevel handle names a toplevel")
            }
        }
    }

    fn bind_output(
        &mut self,
        registry: &WlRegistry,
        qh: &QueueHandle<State>,
        name: u32,
        version: u32,
    ) {
        let id = OutputId(name);
        // version 4 adds the name and description
        let wl = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, id);
        let xdg = self
            .output_manager
            .as_ref()
            .map(|m| m.get_xdg_output(&wl, qh, id));
        self.outputs.push(Output {
            registry_name: name,
            wl,
            xdg,
            info: OutputBuilder::new(id),
        });
    }

    /// The whole scale a drawn cursor is made for: the next whole scale of the surface under the pointer, else 1.
    fn cursor_scale(&self) -> u32 {
        self.pointer_tracker
            .focus()
            .and_then(|id| self.surfaces.iter().find(|surf| surf.id == id))
            .and_then(|surf| surf.scale.ceil().integer())
            .map_or(1, i32::unsigned_abs)
    }

    /// Report surfaces whose scale changed since they were last drawn.
    fn update_scales(&mut self) {
        let fractional = self.scaling.is_some();
        for surf in &mut self.surfaces {
            let sources = ScaleSources {
                fractional: surf.fractional_scale,
                preferred: surf.preferred_scale,
                entered: surf
                    .entered
                    .iter()
                    .filter_map(|id| self.outputs.iter().find(|o| o.info.current.id == *id))
                    .map(|o| o.info.current.scale)
                    .collect(),
            };
            let scale = sources.resolve(fractional);
            // a subsurface is drawn at its parent's scale: the parent's change is the one reported
            if matches!(surf.role, Role::Subsurface { .. }) {
                surf.scale = scale;
                continue;
            }
            if scale != surf.scale {
                surf.scale = scale;
                self.events.push(Event::ScaleChanged {
                    surface: surf.id,
                    scale,
                });
            }
        }
    }

    fn pump_reads(&mut self) {
        self.clipboard.pump(Instant::now(), &mut self.events);
    }

    fn repeat_keys(&mut self, now: Instant) {
        let (Some(surface), Some(t)) = (self.key_focus, &self.translator) else {
            return;
        };
        if let Some((code, count)) = self.repeat.due(now) {
            let k = t.translate(code);
            let time = self.input_time(now);
            for _ in 0..count {
                self.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Key {
                        code,
                        pressed: true,
                        repeat: true,
                        keysym: k.keysym,
                        character: k.character,
                        mods: k.mods,
                    },
                });
            }
        }
    }

    fn note_time(&mut self, time: u32) {
        self.input_clock.note(time, Instant::now());
    }

    fn input_time(&self, now: Instant) -> u32 {
        self.input_clock.at(now)
    }

    pub(crate) fn surface_of(&self, wl: &WlSurface) -> Option<SurfaceId> {
        wl.data::<SurfaceId>().copied()
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: XdgPositioner);
delegate_noop!(State: WpFractionalScaleManagerV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: WpSinglePixelBufferManagerV1);
delegate_noop!(State: ZxdgDecorationManagerV1);
delegate_noop!(State: ZxdgImporterV2);
delegate_noop!(State: XdgActivationV1);
delegate_noop!(State: WpCursorShapeManagerV1);
delegate_noop!(State: WpCursorShapeDeviceV1);
// the themed cursor's surface and buffers
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ZxdgOutputManagerV1);

impl Dispatch<ZxdgOutputV1, OutputId> for State {
    fn event(
        state: &mut Self,
        xdg: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        id: &OutputId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.info.current.id == *id) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.info.set_position(x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.info.set_size(width, height)
            }
            zxdg_output_v1::Event::Done if xdg.version() < 3 && output.info.done() => {
                state.events.push(Event::OutputsChanged)
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                state.bind_output(registry, qh, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.registry_name == name) {
                    let output = state.outputs.remove(i);
                    if let Some(xdg) = &output.xdg {
                        xdg.destroy();
                    }
                    if output.wl.version() >= 3 {
                        output.wl.release();
                    }
                    for s in &mut state.surfaces {
                        s.entered.retain(|id| *id != OutputId(name));
                    }
                    state.events.push(Event::OutputsChanged);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, OutputId> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        id: &OutputId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.info.current.id == *id) else {
            return;
        };
        let pending = output.info.pending();
        match event {
            wl_output::Event::Scale { factor } => match Scale::from_integer(factor) {
                Some(scale) => pending.scale = scale,
                None => state.fail(Error::InvalidScale),
            },
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                refresh,
            } if flags.contains(wl_output::Mode::Current) => {
                pending.mode = u32::try_from(width).ok().zip(u32::try_from(height).ok());
                pending.refresh_mhz = u32::try_from(refresh).ok().filter(|r| *r > 0);
            }
            wl_output::Event::Geometry {
                physical_width,
                physical_height,
                make,
                model,
                transform,
                ..
            } => {
                pending.make = identification(make);
                pending.model = identification(model);
                pending.physical_size_mm = physical_size(physical_width, physical_height);
                pending.transform = match transform {
                    WEnum::Value(wl_output::Transform::_90) => OutputTransform::Rotated90,
                    WEnum::Value(wl_output::Transform::_180) => OutputTransform::Rotated180,
                    WEnum::Value(wl_output::Transform::_270) => OutputTransform::Rotated270,
                    WEnum::Value(wl_output::Transform::Flipped) => OutputTransform::Flipped,
                    WEnum::Value(wl_output::Transform::Flipped90) => OutputTransform::Flipped90,
                    WEnum::Value(wl_output::Transform::Flipped180) => OutputTransform::Flipped180,
                    WEnum::Value(wl_output::Transform::Flipped270) => OutputTransform::Flipped270,
                    _ => OutputTransform::Normal,
                };
            }
            wl_output::Event::Name { name } => pending.name = Some(name),
            wl_output::Event::Description { description } => {
                pending.description = Some(description)
            }
            wl_output::Event::Done if output.info.done() => {
                state.events.push(Event::OutputsChanged)
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let output_id = |o: &WlOutput| o.data::<OutputId>().copied();
        let Ok(surf) = state.surface_mut(*id) else {
            return;
        };
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some(o) = output_id(&output).filter(|o| !surf.entered.contains(o)) {
                    surf.entered.push(o);
                }
            }
            wl_surface::Event::Leave { output } => {
                if let Some(o) = output_id(&output) {
                    surf.entered.retain(|e| *e != o);
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => match Scale::from_integer(factor)
            {
                Some(scale) => surf.preferred_scale = Some(scale),
                None => state.fail(Error::InvalidScale),
            },
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        match (Scale::from_120ths(scale), state.surface_mut(*id)) {
            (Some(scale), Ok(surf)) => surf.fractional_scale = Some(scale),
            (None, _) => state.fail(Error::InvalidScale),
            (_, Err(_)) => {}
        }
    }
}

impl Dispatch<WlCallback, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wl_callback::Event::Done { callback_data } = event else {
            return;
        };
        if state.surface(*id).is_ok() {
            state.events.push(Event::Frame(FrameCallback {
                surface: *id,
                time: callback_data,
            }));
        }
    }
}

impl Dispatch<WlBuffer, SlotTag> for State {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        tag: &SlotTag,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wl_buffer::Event::Release = event else {
            return;
        };
        if let Some(slot) = state
            .surface_mut(SurfaceId(tag.surface))
            .ok()
            .and_then(|surf| surf.slots.iter_mut().find(|s| s.tag == *tag))
        {
            slot.busy = false;
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        let Ok(surf) = state.surface_mut(*id) else {
            return;
        };
        if let Err(e) = surf.serials.receive(serial) {
            return state.fail(e);
        }
        let event = match &mut surf.role {
            Role::Toplevel {
                pending,
                decorations,
                current,
                ..
            } => {
                let configure = WindowConfigure {
                    serial,
                    size: pending.size,
                    bounds: pending.bounds,
                    states: pending.states,
                    decorations: *decorations,
                    capabilities: pending.capabilities,
                };
                *current = Some(configure);
                Event::Configure {
                    window: Toplevel(*id),
                    configure,
                }
            }
            Role::Popup {
                pending, current, ..
            } => {
                let Some((x, y, width, height)) = *pending else {
                    return state.fail(Error::Protocol("popup configured without a placement"));
                };
                let configure = PopupConfigure {
                    serial,
                    x,
                    y,
                    width,
                    height,
                };
                *current = Some(configure);
                Event::PopupConfigure {
                    popup: Popup(*id),
                    configure,
                }
            }
            Role::External { .. } | Role::Subsurface { .. } => {
                return state.fail(Error::Protocol(
                    "an xdg_surface configure for a surface with an external role",
                ))
            }
        };
        state.events.push(event);
    }
}

impl Dispatch<XdgToplevel, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Ok(Surface {
            role: Role::Toplevel { pending, .. },
            ..
        }) = state.surface_mut(*id)
        else {
            return;
        };
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => match wire_size(width, height) {
                Ok(size) => {
                    pending.size = size;
                    pending.states = WindowStates::from_wire(&states);
                }
                Err(e) => state.fail(e),
            },
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                match wire_size(width, height) {
                    Ok(bounds) => pending.bounds = bounds,
                    Err(e) => state.fail(e),
                }
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                pending.capabilities = Some(WmCapabilities::from_wire(&capabilities));
            }
            xdg_toplevel::Event::Close => state.events.push(Event::Close(Toplevel(*id))),
            _ => {}
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event else {
            return;
        };
        if let Ok(Surface {
            role: Role::Toplevel { decorations, .. },
            ..
        }) = state.surface_mut(*id)
        {
            *decorations = match mode {
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => Decorations::Server,
                _ => Decorations::Client,
            };
        }
    }
}

/// The compositor ended an import: the handle is unknown or no longer exported, or the exported window is gone. The
/// window stays, without the parent.
impl Dispatch<ZxdgImportedV2, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_imported_v2::Event::Destroyed = event else {
            return;
        };
        if let Ok(Surface {
            role: Role::Toplevel { imported, .. },
            ..
        }) = state.surface_mut(*id)
        {
            if let Some(i) = imported.take() {
                i.destroy();
            }
        }
    }
}

impl Dispatch<XdgPopup, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgPopup,
        event: xdg_popup::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                let size = match wire_size(width, height) {
                    Ok(Some(size)) => size,
                    Ok(None) => {
                        return state.fail(Error::Protocol("popup configured with an empty size"))
                    }
                    Err(e) => return state.fail(e),
                };
                if let Ok(Surface {
                    role: Role::Popup { pending, .. },
                    ..
                }) = state.surface_mut(*id)
                {
                    *pending = Some((x, y, size.0, size.1));
                }
            }
            xdg_popup::Event::PopupDone => state.events.push(Event::PopupDone(Popup(*id))),
            xdg_popup::Event::Repositioned { token } => {
                state.events.push(Event::PopupRepositioned {
                    popup: Popup(*id),
                    token,
                })
            }
            _ => {}
        }
    }
}

/// What an activation token is for.
enum TokenUse {
    /// Activate this surface with it.
    Activate(SurfaceId),
    /// Hand it to the application.
    Hand(TokenRequest),
}

impl Dispatch<XdgActivationTokenV1, TokenUse> for State {
    fn event(
        state: &mut Self,
        token: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        used: &TokenUse,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_activation_token_v1::Event::Done { token: value } = event else {
            return;
        };
        match used {
            TokenUse::Activate(id) => {
                if let (Some(activation), Ok(surf)) = (&state.activation, state.surface(*id)) {
                    activation.activate(value, &surf.wl);
                }
            }
            TokenUse::Hand(request) => state.events.push(Event::ActivationToken {
                request: *request,
                token: value,
            }),
        }
        token.destroy();
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        let has_pointer = caps.contains(wl_seat::Capability::Pointer);
        if has_pointer && state.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            state.cursor_device = state
                .cursor_shape
                .as_ref()
                .map(|m| m.get_pointer(&pointer, qh, ()));
            state.pointer = Some(pointer);
        } else if let Some(pointer) = state.pointer.take_if(|_| !has_pointer) {
            state.themed_cursor.stop();
            state.held_buttons.clear();
            state.press_serial = None;
            if let Some(device) = state.cursor_device.take() {
                device.destroy();
            }
            if pointer.version() >= 3 {
                pointer.release();
            }
            if let Some((surface, event)) = state.pointer_tracker.leave() {
                let time = state.input_time(Instant::now());
                state.events.push(Event::Pointer {
                    surface,
                    time,
                    event,
                });
            }
        }
        let has_touch = caps.contains(wl_seat::Capability::Touch);
        if has_touch && state.touch.is_none() {
            state.touch = Some(seat.get_touch(qh, ()));
        } else if let Some(touch) = state.touch.take_if(|_| !has_touch) {
            if touch.version() >= 3 {
                touch.release();
            }
            state.cancel_touches();
        }
        let has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
        if has_keyboard && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        } else if let Some(keyboard) = state.keyboard.take_if(|_| !has_keyboard) {
            if keyboard.version() >= 3 {
                keyboard.release();
            }
            state.repeat.clear();
            if let Some(surface) = state.key_focus.take() {
                let time = state.input_time(Instant::now());
                state.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Leave,
                });
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        // a frame's scroll takes the time of its last axis event
        let own_time = match &event {
            wl_pointer::Event::Motion { time, .. }
            | wl_pointer::Event::Button { time, .. }
            | wl_pointer::Event::Axis { time, .. } => Some(*time),
            wl_pointer::Event::Frame => state.input_clock.latest(),
            _ => None,
        };
        let mut released_all = false;
        let found = match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.pointer_enter_serial = Some(serial);
                state.surface_of(&surface).map(|s| {
                    state
                        .pointer_tracker
                        .enter(s, Point::new(surface_x, surface_y))
                })
            }
            wl_pointer::Event::Leave { .. } => {
                state.themed_cursor.stop();
                state.held_buttons.clear();
                state.press_serial = None;
                state.clipboard.pointer_left();
                state.pointer_tracker.leave()
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => {
                state.note_time(time);
                state
                    .pointer_tracker
                    .motion(Point::new(surface_x, surface_y))
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(button_state),
            } => {
                state.note_time(time);
                state.last_serial = Some(serial);
                let pressed = button_state == ButtonState::Pressed;
                if pressed {
                    if state.held_buttons.is_empty() {
                        state.press_serial = Some(serial);
                        state.latest_press = PressDevice::Pointer;
                    }
                    state.held_buttons.push(button);
                } else {
                    state.held_buttons.retain(|b| *b != button);
                    if state.held_buttons.is_empty() {
                        state.press_serial = None;
                        released_all = true;
                    }
                }
                state.pointer_tracker.button(button, pressed)
            }
            wl_pointer::Event::Axis {
                time,
                axis: WEnum::Value(axis),
                value,
            } => {
                state.note_time(time);
                state
                    .pointer_tracker
                    .axis(axis == Axis::VerticalScroll, value);
                // before version 5 there is no `frame`, so each axis is a scroll of its own
                if pointer.version() < 5 {
                    state.pointer_tracker.frame()
                } else {
                    None
                }
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => {
                state.pointer_tracker.axis_source(source);
                None
            }
            wl_pointer::Event::AxisStop {
                time,
                axis: WEnum::Value(_),
            } => {
                state.note_time(time);
                state.pointer_tracker.axis_stop();
                None
            }
            // before version 8 a wheel reports whole notches
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                state
                    .pointer_tracker
                    .axis_value120(axis == Axis::VerticalScroll, discrete.saturating_mul(120));
                None
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                state
                    .pointer_tracker
                    .axis_value120(axis == Axis::VerticalScroll, value120);
                None
            }
            wl_pointer::Event::AxisRelativeDirection {
                axis: WEnum::Value(axis),
                direction: WEnum::Value(direction),
            } => {
                state.pointer_tracker.axis_relative_direction(
                    axis == Axis::VerticalScroll,
                    direction == wl_pointer::AxisRelativeDirection::Inverted,
                );
                None
            }
            wl_pointer::Event::Frame => state.pointer_tracker.frame(),
            _ => None,
        };
        if let Some((surface, event)) = found {
            let time = own_time.unwrap_or_else(|| state.input_time(Instant::now()));
            state.events.push(Event::Pointer {
                surface,
                time,
                event,
            });
        }
        if released_all {
            state.clipboard.buttons_released(&mut state.events);
        }
    }
}

impl State {
    fn held_press(&self) -> Option<HeldPress> {
        let pointer = self.press_serial.map(|serial| HeldPress {
            serial,
            device: PressDevice::Pointer,
        });
        let touch = self.touch_press_serial.map(|serial| HeldPress {
            serial,
            device: PressDevice::Touch,
        });
        match self.latest_press {
            PressDevice::Pointer => pointer.or(touch),
            PressDevice::Touch => touch.or(pointer),
        }
    }

    fn cancel_touches(&mut self) {
        self.held_touches.clear();
        self.touch_press_serial = None;
        let time = self.input_time(Instant::now());
        for (surface, event) in self.touch_tracker.cancel() {
            self.events.push(Event::Touch {
                surface,
                time,
                event,
            });
        }
    }
}

impl Dispatch<WlTouch, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let (found, time) = match event {
            wl_touch::Event::Down {
                serial,
                time,
                surface,
                id,
                x,
                y,
            } => {
                state.last_serial = Some(serial);
                if state.held_touches.is_empty() {
                    state.touch_press_serial = Some(serial);
                    state.latest_press = PressDevice::Touch;
                }
                state.held_touches.push(id);
                let found = state
                    .surface_of(&surface)
                    .map(|s| (s, state.touch_tracker.down(id, s, Point::new(x, y))));
                (found, time)
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                (state.touch_tracker.motion(id, Point::new(x, y)), time)
            }
            wl_touch::Event::Up { serial, time, id } => {
                state.last_serial = Some(serial);
                state.held_touches.retain(|t| *t != id);
                if state.held_touches.is_empty() {
                    state.touch_press_serial = None;
                }
                (state.touch_tracker.up(id), time)
            }
            wl_touch::Event::Cancel => {
                state.cancel_touches();
                return;
            }
            _ => return,
        };
        state.note_time(time);
        if let Some((surface, event)) = found {
            state.events.push(Event::Touch {
                surface,
                time,
                event,
            });
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => match Translator::from_fd(fd, size as usize) {
                Ok(t) => state.translator = Some(t),
                Err(e) => state.fail(e),
            },
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                state.last_serial = Some(serial);
                state.key_focus = state.surface_of(&surface);
                if let Some(surface) = state.key_focus {
                    let time = state.input_time(Instant::now());
                    state.events.push(Event::Keyboard {
                        surface,
                        time,
                        event: KeyboardEvent::Enter,
                    });
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                state.repeat.clear();
                if let Some(t) = state.translator.as_mut() {
                    t.reset_compose();
                }
                if let Some(surface) = state.key_focus.take() {
                    let time = state.input_time(Instant::now());
                    state.events.push(Event::Keyboard {
                        surface,
                        time,
                        event: KeyboardEvent::Leave,
                    });
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: WEnum::Value(key_state),
            } => {
                state.note_time(time);
                state.last_serial = Some(serial);
                let pressed = key_state == KeyState::Pressed;
                let (Some(surface), Some(t)) = (state.key_focus, state.translator.as_mut()) else {
                    return;
                };
                let k = if pressed {
                    t.press(key, &mut state.repeat, Instant::now())
                } else {
                    state.repeat.release(key);
                    t.translate(key)
                };
                state.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Key {
                        code: key,
                        pressed,
                        repeat: false,
                        keysym: k.keysym,
                        character: k.character,
                        mods: k.mods,
                    },
                });
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(t) = state.translator.as_mut() {
                    t.update_modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => state.repeat.set_info(rate, delay),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_after_the_connection_is_gone_does_nothing() {
        let (wake, writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let waker = Waker(Arc::new(writer));
        waker.wake();
        drop(wake);
        waker.wake();
    }

    #[test]
    fn a_solid_colour_is_sent_premultiplied_over_the_whole_u32_range() {
        assert_eq!(single_pixel([0.0; 4]), [0; 4], "transparent");
        assert_eq!(
            single_pixel([1.0, 0.0, 0.0, 1.0]),
            [u32::MAX, 0, 0, u32::MAX]
        );
        let half = single_pixel([1.0, 0.5, 0.0, 0.5]);
        assert_eq!(half[3], u32::MAX / 2 + 1, "alpha rounds to the nearest");
        assert_eq!(half[0], half[3], "red is premultiplied by the alpha");
        assert_eq!(half[1], u32::MAX / 4 + 1);
        assert_eq!(
            single_pixel([2.0, -1.0, 0.0, 3.0]),
            [u32::MAX, 0, 0, u32::MAX],
            "components outside 0 to 1 are clamped"
        );
    }
}
