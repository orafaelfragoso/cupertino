//! The connection: globals, surfaces with their roles and buffers, seat input and the event loop.

#[path = "commit.rs"]
pub(crate) mod commit;
#[path = "gpu.rs"]
pub(crate) mod gpu;

use std::any::Any;
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::ops::RangeInclusive;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
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
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{
    delegate_noop, Connection as WlConnection, Dispatch, DispatchError, EventQueue, Proxy,
    QueueHandle, WEnum,
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
use wayland_protocols::xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1;
use wayland_protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1::XdgWmDialogV1;
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

use crate::buffer::{pick_slot, Slot, SlotChoice, SlotTag, Spare, MAX_SLOTS};
use crate::capability::REQUIRED;
use crate::clipboard::Clipboard;
use crate::configure::{wire_size, Serials};
use crate::cursor::{CursorTheme, ThemedCursor};
use crate::input::{
    InputClock, Modifiers, PointerTracker, Repeat, TouchEvent, TouchTracker, Translator,
};
use crate::output::{identification, physical_size, OutputBuilder, OutputId, OutputTransform};
use crate::scale::ScaleSources;
use crate::text_input::{Preedit, TextInput, TextInputDone};
use commit::Commit;
use wayland_client::protocol::wl_data_device_manager::DndAction as DragActions;

use crate::{
    BufferFormat, BufferLayout, Capability, CapabilitySet, ClipboardOffer, Decorations, Disconnect,
    DragEvent, DragIcon, DragOffer, DragSource, DragSourceEvent, Error, Extension, KeyboardEvent,
    OutputInfo, PixelRect, PointerEvent, Presented, ReadId, Scale, TextInputEvent, TextInputState,
    WindowConfigure, WindowStates, WmCapabilities,
};

/// The longest pause between a surface's frames for it to count as drawing frame after frame: a few refreshes.
const ANIMATION_GAP: Duration = Duration::from_millis(100);

/// What becomes of a surface's spare buffer memory as it leases a buffer: `Some(true)` to start making one,
/// `Some(false)` to let it go, `None` to keep what there is. A surface drawing frame after frame (`animating`) whose
/// buffers of the layout (`of_layout` with the one leased) are all in use, none `free` besides it and the compositor
/// holding the others, will want one more soon, so its memory is made meanwhile, unless a `fitting` spare is there
/// already. The first buffer of a layout (`created` alone) makes none, so a resize, whose every frame is of a new size,
/// makes no memory it does not use; a surface that draws again after a pause lets an unused spare go, unless it had to
/// make a buffer beside one the compositor still holds, as the first frame of an animation does, so its second frame
/// finds its buffer made.
fn spare_wanted(
    animating: bool,
    created: bool,
    of_layout: usize,
    free: bool,
    fitting: bool,
) -> Option<bool> {
    if !animating && !(created && of_layout > 1) {
        return Some(false);
    }
    let wanted = of_layout < MAX_SLOTS && !(created && of_layout == 1) && !free && !fitting;
    wanted.then_some(true)
}

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

/// Another client's toplevel that a window opens as a child of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignParent {
    /// The handle the other client exported its toplevel under (xdg-foreign-unstable-v2).
    pub handle: String,
    /// The window is a modal dialog of that parent (xdg-dialog-v1). A compositor without the protocol shows it as any
    /// child.
    pub modal: bool,
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
    /// The toplevel another client exported, which the window opens as a child of, as a dialog of that client's
    /// window. A compositor without the protocol, or one that does not know the handle, leaves the window without a
    /// parent.
    pub foreign_parent: Option<ForeignParent>,
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
    /// The selection this client set with [`Connection::set_selection`] is no longer its own: another client's
    /// replaced it, or the compositor dropped it (`wl_data_source.cancelled`). A selection this client replaces itself
    /// gives none.
    SelectionLost,
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
        /// The window's dialog role, which says it is modal; few windows have one, so it is kept out of line.
        dialog: Option<Box<XdgDialogV1>>,
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
        role: Arc<dyn ExternalRole>,
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
pub trait ExternalRole: Send + Sync {
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

    /// The surface this one is drawn at the scale of, for a role the compositor never configures (a layer of that
    /// surface): it may present at once, its frames sized by that surface's scale. `None` (the default): the
    /// compositor configures it and it is drawn at its own scale.
    fn drawn_with(&self) -> Option<SurfaceId> {
        None
    }
}

struct Surface {
    id: SurfaceId,
    wl: WlSurface,
    role: Role,
    viewport: Option<WpViewport>,
    /// The one-pixel buffer of the colour the surface last showed all over ([`Connection::present_solid`]), with that
    /// colour as the protocol takes it (premultiplied, each component scaled to the whole `u32` range).
    solid: Option<(WlBuffer, [u32; 4])>,
    fractional: Option<WpFractionalScaleV1>,
    fractional_scale: Option<Scale>,
    preferred_scale: Option<Scale>,
    entered: Vec<OutputId>,
    /// The scale last reported to the application, which buffers are drawn at.
    scale: Scale,
    /// The configure serials, shared with the commits made for the surface.
    shared: commit::SharedState,
    slots: Vec<Slot>,
    next_slot: u64,
    /// The memory of the next buffer, made while the surface draws into the ones it has.
    spare: Option<Spare>,
    /// When the surface last presented a buffer.
    presented_at: Option<Instant>,
    /// The surface's `wp_linux_drm_syncobj_surface_v1` while it shows dmabufs with explicit sync.
    syncobj: Option<wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>,
}

impl Surface {
    fn serials(&self) -> Serials {
        commit::lock(&self.shared).serials
    }

    /// Who acknowledges the surface's configures, and its latest configure, which a frame drawn now answers; `None`
    /// for a subsurface, which answers none.
    fn answers(&self) -> Result<Option<(commit::Ack, u32)>, Error> {
        let ack = match &self.role {
            Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => commit::Ack::Xdg(xdg.clone()),
            Role::External { role, .. } => commit::Ack::External(Arc::clone(role)),
            Role::Subsurface { .. } => return Ok(None),
        };
        let serial = self.serials().latest().ok_or(Error::NotConfigured)?;
        Ok(Some((ack, serial)))
    }

    /// How a buffer is scaled onto the surface to show `logical` points at `scale`: through the viewport where it has
    /// one, showing the `source` rectangle of the buffer (pixels) when given and all of it otherwise; else by a whole
    /// buffer scale, which shows all of it.
    fn scaling(
        &self,
        logical: (u32, u32),
        scale: Scale,
        source: Option<(f64, f64, f64, f64)>,
    ) -> Result<commit::Scaling, Error> {
        match (&self.viewport, scale.integer()) {
            (Some(viewport), _) => Ok(commit::Scaling::Viewport {
                viewport: viewport.clone(),
                crop: source,
                destination: logical,
            }),
            (None, _) if source.is_some() => Err(Error::Protocol(
                "a cropped frame on a surface without a viewport",
            )),
            (None, Some(n)) => Ok(commit::Scaling::Buffer(n)),
            (None, None) => Err(Error::InvalidScale),
        }
    }
}

/// Dispatch state of the connection. Extensions implement `Dispatch` for it with user data of their own type, so their
/// objects share the connection's queue; see the crate README.
pub struct State {
    capabilities: CapabilitySet,
    compositor: WlCompositor,
    shm: WlShm,
    /// The formats `wl_shm` lists.
    shm_formats: Vec<wl_shm::Format>,
    wm_base: XdgWmBase,
    subcompositor: Option<WlSubcompositor>,
    seat: Option<WlSeat>,
    decoration_manager: Option<ZxdgDecorationManagerV1>,
    importer: Option<ZxdgImporterV2>,
    dialogs: Option<XdgWmDialogV1>,
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
    /// A key whose press only took a pending dead key away; its release is not sent either.
    swallowed: Option<u32>,
    repeat: Repeat,
    last_serial: Option<u32>,
    input_clock: InputClock,
    pub(crate) events: Vec<Event>,
    fault: Option<Error>,
    /// Dmabuf buffers, timelines and the dmabuf and syncobj globals, bound on first use.
    gpu: gpu::GpuState,
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

mod buffer;
mod dispatch;
use dispatch::{Synced, TokenUse};
mod surface;

use buffer::{clamp_i32, decoration_mode, logical_size, positioner, single_pixel};
pub(crate) use buffer::{round_in, round_out};
pub use buffer::{BufferLease, DetachedBuffer};
