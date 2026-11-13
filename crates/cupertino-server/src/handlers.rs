//! Wayland protocol handlers: compositor, xdg-shell, xdg-decoration, xdg-dialog, layer-shell,
//! seat, data device, dmabuf, fractional scale, activation, foreign toplevel list and friends.

use std::os::unix::io::OwnedFd;

use cupertino_wm::{Command, Point as MPoint};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::backend::renderer::ImportDma;
use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface as DesktopLayerSurface, PopupKeyboardGrab,
    PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType,
};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::{xdg_toplevel, xdg_wm_base};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::backend::protocol::ProtocolError;
use smithay::reexports::wayland_server::backend::DisconnectReason;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::{Logical, Point, Rectangle, Serial, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states, BufferAssignment, CompositorClientState, CompositorHandler,
    Blocker, CompositorState, SurfaceAttributes,
};
use smithay::wayland::dmabuf::{get_dmabuf, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelListHandler, ForeignToplevelListState};
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::backend::input::TabletToolDescriptor;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::dialog::XdgDialogHandler;
use smithay::wayland::shell::xdg::{
    Configure, PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconHandler;
use smithay::{
    delegate_compositor, delegate_cursor_shape, delegate_data_device, delegate_dmabuf, delegate_drm_syncobj, delegate_foreign_toplevel_list,
    delegate_fractional_scale, delegate_layer_shell, delegate_output, delegate_pointer_gestures, delegate_presentation,
    delegate_primary_selection, delegate_relative_pointer, delegate_seat, delegate_shm, delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_activation,
    delegate_xdg_decoration, delegate_xdg_dialog, delegate_xdg_shell, delegate_xdg_toplevel_icon,
};

use crate::acquire_waits::{self, Admission, Kind};
use crate::clipboard::Snapshot;
use crate::control::ActionGrab;
use crate::menu_bar::cut_text;
use crate::policy;
use crate::state::{ClientState, Cupertino, PointerMode, WindowEntry};

impl BufferHandler for Cupertino {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for Cupertino {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client data of an unknown type")
            .compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            let dmabuf = with_states(surface, |data| {
                data.cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|b| match b {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            let unmapping = with_states(surface, |data| {
                matches!(
                    data.cached_state
                        .get::<SurfaceAttributes>()
                        .pending()
                        .buffer,
                    Some(BufferAssignment::Removed)
                )
            });
            if unmapping {
                state.snapshot_closing_surface(surface);
            }
            // A client that uses explicit sync says when its buffer is ready; any other dmabuf is waited for through its
            // implicit fences.
            let acquire = with_states(surface, |data| {
                data.cached_state
                    .get::<DrmSyncobjCachedState>()
                    .pending()
                    .acquire_point
                    .clone()
            });
            if let Some(point) = acquire {
                match point.generate_blocker() {
                    Ok((blocker, source)) => {
                        hold_commit(state, surface, Kind::AcquirePoint, blocker, source)
                    }
                    Err(e) => {
                        tracing::warn!(client = ?surface.client().map(|c| c.id()), error = %e, "no wait for an acquire point: the commit goes through unblocked")
                    }
                }
            } else if let Some(dmabuf) = dmabuf {
                // the only error is that every fence has signalled: the commit then needs no wait
                let blocker: Result<_, smithay::backend::allocator::dmabuf::AlreadyReady> =
                    dmabuf.generate_blocker(smithay::reexports::calloop::Interest::READ);
                if let Ok((blocker, source)) = blocker {
                    hold_commit(state, surface, Kind::ImplicitFence, blocker, source)
                }
            }
        });
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.pending_activations.remove(&surface.id());
        self.menu_surface_destroyed(surface);
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(e) = self.windows.iter().find(|w| w.has_surface(&root)) {
                e.window.on_commit();
            }
        }
        self.popups.commit(surface);
        if self.popups.find_popup(surface).is_some() {
            self.refocus_pointer();
        }
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
        }

        if matches!(&self.cursor_status, CursorImageStatus::Surface(s) if s == surface) {
            with_states(surface, |states| {
                if let Some(attrs) = states.data_map.get::<CursorImageSurfaceData>() {
                    let delta = states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer_delta
                        .take();
                    if let Some(delta) = delta {
                        attrs.lock().unwrap().hotspot -= delta;
                    }
                }
            });
        }
        if let Some((s, offset)) = self.drag_icon.as_mut() {
            if s == surface {
                let delta = with_states(s, |states| {
                    states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer_delta
                        .take()
                });
                if let Some(d) = delta {
                    *offset += d;
                }
            }
        }

        self.lock_surface_committed(surface);
        self.toplevel_committed(surface);
        self.layer_committed(surface);
        self.surface_output_committed(surface);
        self.request_redraw_for(surface);
    }
}
impl smithay::wayland::security_context::SecurityContextHandler for Cupertino {
    fn context_created(
        &mut self,
        source: smithay::wayland::security_context::SecurityContextListenerSource,
        _context: smithay::wayland::security_context::SecurityContext,
    ) {
        let inserted = self.loop_handle.insert_source(source, |stream, _, state| {
            let data = std::sync::Arc::new(crate::state::ClientState {
                sandboxed: true,
                peer: crate::state::client_process(&stream),
                ..crate::state::ClientState::default()
            });
            if let Err(e) = state.display_handle.insert_client(stream, data) {
                tracing::warn!(error = %e, "adding a sandboxed wayland client failed");
            }
        });
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "listening for a security context failed");
        }
    }
}
smithay::delegate_security_context!(Cupertino);

delegate_compositor!(Cupertino);

impl ShmHandler for Cupertino {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(Cupertino);

/// The object id of the `wl_display`, which every connection has.
pub(crate) const WL_DISPLAY_ID: u32 = 1;
/// `wl_display.error` `implementation`: a problem in the compositor or in what the client asks of it.
pub(crate) const WL_DISPLAY_ERROR_IMPLEMENTATION: u32 = 3;

/// Keeps the commit of `surface` back until `blocker` clears, which `source` reports. How many such commits a client may
/// have depends on what they wait for ([`acquire_waits`]): past the limit of explicit-sync waits the client is
/// disconnected, and past the bound of implicit-fence waits the commit goes through without waiting. A commit that cannot
/// be held back goes through at once, and is logged.
fn hold_commit<B, S>(state: &mut Cupertino, surface: &WlSurface, kind: Kind, blocker: B, source: S)
where
    B: Blocker + Send + 'static,
    S: smithay::reexports::calloop::EventSource<Event = (), Ret = Result<(), std::io::Error>>
        + 'static,
{
    let Some(client) = surface.client() else {
        return;
    };
    let Some(data) = client.get_data::<ClientState>() else {
        return;
    };
    let waits = data.acquire_waits.clone();
    let id = match waits.admit(kind) {
        Admission::Granted(id) => id,
        Admission::Closed => return,
        Admission::PassUnblocked { first } => {
            if first {
                tracing::warn!(client = ?client.id(), bound = acquire_waits::IMPLICIT_LIMIT, "a client has more commits waiting for dmabuf fences than the bound: its further commits go through without waiting");
            }
            return;
        }
        Admission::Disconnect => {
            tracing::warn!(client = ?client.id(), limit = acquire_waits::ACQUIRE_LIMIT, "a client has too many commits waiting for acquire points: disconnected");
            state.display_handle.backend_handle().kill_client(
                client.id(),
                DisconnectReason::ProtocolError(ProtocolError {
                    code: WL_DISPLAY_ERROR_IMPLEMENTATION,
                    object_id: WL_DISPLAY_ID,
                    object_interface: "wl_display".into(),
                    message: format!(
                        "more than {} commits waiting for acquire points",
                        acquire_waits::ACQUIRE_LIMIT
                    ),
                }),
            );
            return;
        }
    };
    let on_fire = waits.clone();
    let inserted = state.loop_handle.insert_source(source, move |_, _, data| {
        on_fire.cleared(id);
        let dh = data.display_handle.clone();
        data.client_compositor_state(&client)
            .blocker_cleared(data, &dh);
        Ok(())
    });
    match inserted {
        Ok(token) => {
            waits.registered(id, token);
            if waits.first_listing() {
                state.acquire_registry.push(waits);
            }
            add_blocker(surface, blocker);
        }
        Err(e) => {
            waits.cleared(id);
            tracing::warn!(client = ?surface.client().map(|c| c.id()), error = %e, "a commit could not be held back for its buffer: it goes through unblocked");
        }
    }
}

impl DrmSyncobjHandler for Cupertino {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}

delegate_drm_syncobj!(Cupertino);

impl DmabufHandler for Cupertino {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let ok = self
            .backend
            .as_mut()
            .map(|b| b.renderer().import_dmabuf(&dmabuf, None).is_ok())
            .unwrap_or(false);
        if ok {
            let _ = notifier.successful::<Cupertino>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Cupertino);

impl OutputHandler for Cupertino {}
delegate_output!(Cupertino);

impl SeatHandler for Cupertino {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Cupertino> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
        {
            use smithay::wayland::text_input::TextInputSeat;
            let ti = seat.text_input();
            ti.leave();
            ti.set_focus(focused.cloned());
            ti.enter();
        }
        self.text_cursor = None;
        self.menu_bar_edit_focus(focused);
        self.keep_activated_under_menu_bar(focused);
        self.shortcut_inhibit_focus(focused);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
        self.request_redraw();
    }
}
delegate_seat!(Cupertino);

impl TabletSeatHandler for Cupertino {
    fn tablet_tool_image(&mut self, _tool: &TabletToolDescriptor, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}
delegate_cursor_shape!(Cupertino);
delegate_pointer_gestures!(Cupertino);
delegate_relative_pointer!(Cupertino);
delegate_single_pixel_buffer!(Cupertino);
delegate_viewporter!(Cupertino);
delegate_presentation!(Cupertino);
smithay::delegate_content_type!(Cupertino);

impl SelectionHandler for Cupertino {
    /// The server's copy of a clipboard whose client has gone (`clipboard.rs`).
    type SelectionUserData = std::sync::Arc<Snapshot>;

    // a clear is not asked for, so it goes through while locked: a password tool's timed clear must not leave the
    // password behind
    fn allow_selection(
        &mut self,
        _ty: smithay::wayland::selection::SelectionTarget,
        source: &smithay::wayland::selection::SelectionSource,
        _seat: &Seat<Self>,
    ) -> bool {
        crate::clipboard::source_client(source)
            .is_some_and(|client| self.session_lock.allows_selection(&client))
    }

    fn allow_receive(
        &mut self,
        client: &smithay::reexports::wayland_server::backend::ClientId,
        _seat: &Seat<Self>,
    ) -> bool {
        self.session_lock.allows_selection(client)
    }

    fn new_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        seat: Seat<Self>,
    ) {
        self.clipboard_selection_changed(ty, source.as_ref(), &seat);
    }

    fn send_selection(
        &mut self,
        _ty: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        snapshot: &std::sync::Arc<Snapshot>,
    ) {
        let snapshot = snapshot.clone();
        self.clipboard_serve_snapshot(&snapshot, &mime_type, fd);
    }
}

impl DataDeviceHandler for Cupertino {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Cupertino {
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        let offset = if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            with_states(surface, |states| {
                let hotspot = states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .hotspot;
                Point::from((-hotspot.x, -hotspot.y))
            })
        } else {
            (0, 0).into()
        };
        self.drag_icon = icon.map(|s| (s, offset));
        self.client_drag = self.drag_start();
    }

    fn dropped(&mut self, _target: Option<WlSurface>, validated: bool, _seat: Seat<Self>) {
        // the pointer is locked while its grab ends: the icon is captured once it is free (`Cupertino::drag_ended`)
        let start = self.client_drag.take();
        if let (Some((icon, offset)), false) = (self.drag_icon.take(), validated) {
            self.drag_ended = Some(crate::drag_return::DragEnd {
                icon,
                offset,
                start,
            });
        }
        self.request_redraw();
    }
}

impl ServerDndGrabHandler for Cupertino {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}
delegate_data_device!(Cupertino);

impl PrimarySelectionHandler for Cupertino {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(Cupertino);

impl FractionalScaleHandler for Cupertino {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.surface_scale(&surface);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
        });
    }
}
delegate_fractional_scale!(Cupertino);

impl ForeignToplevelListHandler for Cupertino {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevel_state
    }
}
delegate_foreign_toplevel_list!(Cupertino);

impl XdgToplevelIconHandler for Cupertino {
    fn set_icon(
        &mut self,
        _: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
        wl_surface: WlSurface,
    ) {
        crate::toplevel_icon::icon_set(&wl_surface);
    }
}
delegate_xdg_toplevel_icon!(Cupertino);

/// What the server decided about an activation token when it was made: its creation time on the
/// model clock and whether it may activate at all (made by the server, or by a client for an input
/// event it got while it had the keyboard focus), and the application it launches when its maker named one.
#[derive(Clone, Debug)]
pub struct TokenGrant {
    pub created: f64,
    pub valid: bool,
    pub app_id: Option<String>,
}

impl Cupertino {
    /// Whether `serial` is that of a pointer press or touch that is still held, and went to a surface of the window whose
    /// toplevel is `toplevel`: all a client may start a move or resize with. A serial of a released button, of a key or of
    /// another window gets nothing, or a request made without a held button would glue the window to the pointer.
    fn held_press_on(&self, toplevel: &WlSurface, seat: &WlSeat, serial: Serial) -> bool {
        let Some(seat) = Seat::<Cupertino>::from_resource(seat) else {
            return false;
        };
        // an action's grab serial stands for no press
        if self.is_action_grab_serial(serial) {
            return false;
        }
        let on_window = |focus: Option<WlSurface>| {
            focus.is_some_and(|mut surface| {
                while let Some(parent) = get_parent(&surface) {
                    surface = parent;
                }
                &surface == toplevel
            })
        };
        if let Some(pointer) = seat.get_pointer() {
            if pointer.has_grab(serial) {
                return on_window(
                    pointer
                        .grab_start_data()
                        .and_then(|data| data.focus)
                        .map(|(surface, _)| surface),
                );
            }
        }
        if let Some(touch) = seat.get_touch() {
            if touch.has_grab(serial) {
                return on_window(
                    touch
                        .grab_start_data()
                        .and_then(|data| data.focus)
                        .map(|(surface, _)| surface),
                );
            }
        }
        false
    }

    /// A token for a program the server starts: valid, made now.
    pub fn server_activation_token(&mut self) -> String {
        let data = XdgActivationTokenData::default();
        data.user_data.insert_if_missing(|| TokenGrant {
            created: self.now(),
            valid: true,
            app_id: None,
        });
        self.xdg_activation_state
            .create_external_token(data)
            .0
            .as_str()
            .to_string()
    }
}

impl XdgActivationHandler for Cupertino {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Every token is kept, so a client that activates with one it may not use is still heard (its
    /// tile bounces); only a token made for an input event of the keyboard-focused client is valid.
    /// A token that names an application starts that application's launch bounce.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let valid = match &data.serial {
            // an action's grab serial is no input event
            Some((serial, _)) if self.is_action_grab_serial(*serial) => false,
            Some((serial, seat)) => {
                let keyboard = self.seat.get_keyboard().unwrap();
                Seat::from_resource(seat) == Some(self.seat.clone())
                    && keyboard
                        .last_enter()
                        .map(|enter| serial.is_no_older_than(&enter))
                        .unwrap_or(false)
            }
            None => false,
        };
        data.user_data.insert_if_missing(|| TokenGrant {
            created: self.now(),
            valid,
            app_id: data.app_id.clone().filter(|id| !id.is_empty()),
        });
        // a token that names an application is a launch (launchers, file managers and GTK's
        // launch context set the id): the docks bounce its tile before its first window maps
        if let Some(app_id) = data.app_id.as_deref().filter(|id| !id.is_empty()) {
            self.docks.launching(app_id);
        }
        true
    }

    /// A token has no expiry; it is judged once, at use: a valid token newer
    /// than the application's last activation, or not followed by a user event within 100 ms,
    /// brings the window forward; any other request asks for attention. Either way the token is used up.
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.xdg_activation_state.remove_token(&token);
        let grant = data
            .user_data
            .get::<TokenGrant>()
            .expect("every token gets its grant when it is made")
            .clone();
        self.activate_surface(&surface, grant);
    }
}

impl Cupertino {
    /// `gtk_surface1.present` or `request_focus` for `surface`, whose `startup_id` is an activation token. A window of
    /// the client that has the keyboard focus already is brought forward; any other request is judged as an
    /// `xdg_activation` request is, by the token it names (`present` names none), so a client that has no input behind
    /// it cannot take the keyboard from another: its window asks for attention instead.
    pub fn gtk_focus_request(&mut self, surface: &WlSurface, startup_id: Option<&str>) {
        let Some(id) = self.window_id_for_surface(surface) else {
            return;
        };
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|focus| focus.client())
            .zip(surface.client())
            .is_some_and(|(focused, asking)| focused.id() == asking.id());
        if focused {
            self.command(Command::FocusWindow { window: id });
            return;
        }
        let token = startup_id.map(|id| XdgActivationToken::from(id.to_string()));
        let grant = token
            .as_ref()
            .and_then(|t| self.xdg_activation_state.data_for_token(t))
            .and_then(|d| d.user_data.get::<TokenGrant>().cloned());
        if let Some(token) = &token {
            self.xdg_activation_state.remove_token(token);
        }
        let grant = grant.unwrap_or(TokenGrant {
            created: self.now(),
            valid: false,
            app_id: None,
        });
        self.activate_surface(surface, grant);
    }

    /// A request to bring the window of `surface` forward with `grant`: a valid grant newer than the application's last
    /// activation, and not followed by a user event within 100 ms, brings the window forward; any other asks for
    /// attention. A grant for a window that is not mapped yet is kept until it maps.
    fn activate_surface(&mut self, surface: &WlSurface, grant: TokenGrant) {
        let mapped = self
            .window_id_for_surface(surface)
            .filter(|id| self.entry(*id).is_some_and(|e| e.mapped));
        let Some(id) = mapped else {
            // a program activates its first surface as it maps, usually before the window is mapped: the window picks
            // the grant up when it maps
            if grant.valid {
                let now = self.now();
                self.pending_activations
                    .retain(|_, g| now - g.created <= cupertino_wm::GRANT_LIFETIME);
                self.pending_activations.insert(surface.id(), grant);
            }
            return;
        };
        if grant.valid
            && self
                .wm
                .activation_allowed(id, grant.created, self.last_user_event, self.now())
        {
            self.command(Command::FocusWindow { window: id });
        } else {
            tracing::info!(window = id.0, valid = grant.valid, "activation refused");
            self.command(Command::DemandAttention { window: id });
        }
    }
}
delegate_xdg_activation!(Cupertino);

/// A parent set through an imported handle reaches the model through `parent_changed`, as `xdg_toplevel.set_parent`
/// does; a handle names no window, and a parent that is not a window is no parent.
impl smithay::wayland::xdg_foreign::XdgForeignHandler for Cupertino {
    fn xdg_foreign_state(&mut self) -> &mut smithay::wayland::xdg_foreign::XdgForeignState {
        &mut self.xdg_foreign_state
    }
}
smithay::delegate_xdg_foreign!(Cupertino);

impl XdgDecorationHandler for Cupertino {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.set_decoration_mode(&toplevel, DecorationMode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        self.set_decoration_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.set_decoration_mode(&toplevel, DecorationMode::ServerSide);
    }
}
delegate_xdg_decoration!(Cupertino);

impl XdgDialogHandler for Cupertino {
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let Some(e) = self.windows.iter_mut().find(|w| w.toplevel == toplevel) else {
            return;
        };
        e.modal = is_modal;
        let id = e.id;
        if e.mapped {
            self.command(Command::SetModal {
                window: id,
                modal: is_modal,
            });
        }
    }
}
delegate_xdg_dialog!(Cupertino);

impl XdgShellHandler for Cupertino {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn client_pong(&mut self, client: ShellClient) {
        if self.hung.pong(&client).0 {
            tracing::info!("an application answers its pings again");
            self.announce_responsiveness();
            self.request_redraw();
        }
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // the prompt is layer surfaces only: a window of it would reach the window list, the capture of a window or of
        // the screen, and the stacking of applications
        if crate::authorization::is_prompt(surface.wl_surface()) {
            tracing::warn!(
                "the authentication prompt asked for a toplevel: its connection is ended"
            );
            surface.xdg_toplevel().post_error(
                xdg_wm_base::Error::Role,
                "the authentication prompt draws on layer surfaces only",
            );
            return;
        }
        // without the built-in decorations an extension draws a frame around every window
        let legacy = !self.config.window.decorations;
        let id = cupertino_wm::WindowId(self.next_window);
        self.next_window += 1;
        self.windows.push(WindowEntry {
            id,
            window: Window::new_wayland_window(surface.clone()),
            toplevel: surface,
            mapped: false,
            server_side: legacy,
            titlebar: cupertino_decorations::TitlebarStyle::Standard,
            foreign: None,
            modal: false,
            decor_insets: if legacy {
                self.config.window.ssd_insets
            } else {
                Default::default()
            },
            fullscreen: false,
            shrink: Default::default(),
        });
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            tracing::warn!(error = %e, "tracking popup failed");
        }
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popup_dismissed_by_client();
        // a dismissed popup still drawn while it faded is gone now
        self.request_redraw();
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        if self.session_lock.is_active() {
            return;
        }
        let kind = PopupKind::Xdg(surface.clone());
        let Some(root) = find_popup_root_surface(&kind).ok() else {
            return;
        };
        // the keyboard stays with the authentication prompt while it is shown
        if self.prompt_shown() && !crate::authorization::is_prompt(&root) {
            return;
        }
        // A popup that grows the grabbed chain grabs as the chain does. Otherwise an action's grab serial opens a new
        // chain once, for the client and window the action went to: the seat then holds that chain under a serial of
        // its own, which no client is given, so neither the action's serial nor that one stands for a press.
        let (serial, private, enter) = match self.growing_popup_grab(&kind) {
            Some((held, true)) => (held, true, SERIAL_COUNTER.next_serial()),
            Some((_, false)) => (serial, false, serial),
            None => match root
                .client()
                .map(|c| self.take_action_grab(&c.id(), &root, serial))
            {
                Some(ActionGrab::Granted) => {
                    // as a press elsewhere would, the action first ends the grab another chain holds
                    if self.popup_grab_root().is_some() {
                        let time = std::time::Duration::from(self.clock.now()).as_millis() as u32;
                        self.end_popup_grab(SERIAL_COUNTER.next_serial(), time);
                    }
                    let held = SERIAL_COUNTER.next_serial();
                    (held, true, SERIAL_COUNTER.next_serial())
                }
                Some(ActionGrab::Refused) => {
                    tracing::debug!(
                        "popup grab refused: a spent, expired or foreign action serial"
                    );
                    surface.send_popup_done();
                    return;
                }
                Some(ActionGrab::NotAction) | None => (serial, false, serial),
            },
        };
        if let Ok(mut grab) = self.popups.grab_popup(root.clone(), kind, &seat, serial) {
            let keyboard = seat.get_keyboard();
            let pointer = seat.get_pointer();
            let keyboard_held = keyboard.as_ref().is_some_and(|keyboard| {
                keyboard.is_grabbed()
                    && !(keyboard.has_grab(serial)
                        || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            });
            let pointer_held = pointer.as_ref().is_some_and(|pointer| {
                pointer.is_grabbed()
                    && !(pointer.has_grab(serial)
                        || pointer
                            .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            });
            if keyboard_held || pointer_held {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            // The pointer grab of the menu this one grows from gives the keyboard back to the chain's root as it is
            // replaced, while the keyboard is still grabbed under the chain's serial. Let go of that keyboard grab
            // first, so the keyboard goes straight from the menu to the new popup and the window that opened the chain
            // never hears of it in between.
            if let Some(keyboard) = &keyboard {
                if keyboard.is_grabbed() {
                    keyboard.unset_grab(self);
                }
            }
            if let Some(pointer) = &pointer {
                pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
            }
            if let Some(keyboard) = &keyboard {
                keyboard.set_focus(self, grab.current_grab(), enter);
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            }
            self.popup_grab = Some(crate::popup_grab::ActivePopupGrab::new(grab, root, private));
            self.refocus_pointer();
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        if !self.held_press_on(surface.wl_surface(), &seat, serial) {
            tracing::debug!(
                "move request ignored: the serial is not that of a press still held on the window"
            );
            return;
        }
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
        let p = self.pointer.current_location();
        self.begin_model_interaction(Command::BeginMove {
            window: id,
            pointer: MPoint::new(p.x, p.y),
        });
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        if !self.held_press_on(surface.wl_surface(), &seat, serial) {
            tracing::debug!("resize request ignored: the serial is not that of a press still held on the window");
            return;
        }
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
        let p = self.pointer.current_location();
        self.begin_model_interaction(Command::BeginResize {
            window: id,
            edges: policy::resize_edges(edges),
            pointer: MPoint::new(p.x, p.y),
        });
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_surface(surface.wl_surface()) {
            let zoomed = self
                .wm
                .window(id)
                .is_some_and(|w| w.state() == cupertino_wm::LifecycleState::Zoomed);
            if !zoomed {
                self.command(Command::ToggleZoom { window: id });
            }
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_surface(surface.wl_surface()) {
            let zoomed = self
                .wm
                .window(id)
                .is_some_and(|w| w.state() == cupertino_wm::LifecycleState::Zoomed);
            if zoomed {
                self.command(Command::ToggleZoom { window: id });
            }
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        let target = output
            .as_ref()
            .and_then(Output::from_resource)
            .and_then(|o| self.output_by_wl(&o).map(|e| e.id));
        if let Some(id) = self.window_id_for_surface(surface.wl_surface()) {
            self.command(Command::Fullscreen {
                requested_by: cupertino_wm::Requester::Client,
                window: id,
                output: target,
            });
        } else if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_surface(surface.wl_surface()) {
            self.command(Command::ExitFullscreen { window: id });
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
        let target = self.minimize_target(id);
        self.command(Command::Minimize { window: id, target });
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: Configure) {}

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let title = with_states(surface.wl_surface(), |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .title
                .clone()
        });
        if let Some(e) = self.windows.iter().find(|w| w.toplevel == surface) {
            if let (Some(h), Some(t)) = (&e.foreign, &title) {
                h.send_title(cut_text(t));
                h.send_done();
            }
            if e.mapped {
                let id = e.id;
                self.command(Command::SetTitle {
                    window: id,
                    title: title.unwrap_or_default(),
                });
                self.refresh_menu_bars();
                self.refresh_docks();
            }
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let app_id = with_states(surface.wl_surface(), |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .app_id
                .clone()
        });
        if let (Some(e), Some(a)) = (self.windows.iter().find(|w| w.toplevel == surface), app_id) {
            if let Some(h) = &e.foreign {
                h.send_app_id(cut_text(&a));
                h.send_done();
            }
        }
    }

    /// `set_parent` after the window mapped (before that, the parent is read when it maps); the
    /// model rejects cycles.
    fn parent_changed(&mut self, surface: ToplevelSurface) {
        let Some(id) = self
            .windows
            .iter()
            .find(|w| w.mapped && w.toplevel == surface)
            .map(|w| w.id)
        else {
            return;
        };
        let parent = surface
            .parent()
            .and_then(|p| self.window_id_for_surface(&p));
        self.command(Command::SetParent { window: id, parent });
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(idx) = self.windows.iter().position(|w| w.toplevel == surface) else {
            return;
        };
        if self.windows[idx].mapped {
            let id = self.windows[idx].id;
            self.snapshot_closing(id);
        }
        let entry = self.windows.remove(idx);
        self.decor.forget(entry.id);
        self.fx.forget_snapshot(entry.id);
        let root = entry.toplevel.wl_surface();
        self.menu_surface_destroyed(root);
        self.menu_bars.forget_window(entry.id.0, root);
        if entry.mapped {
            if let Some(h) = &entry.foreign {
                h.send_closed();
            }
            let now = self.now();
            let fx = self.wm.handle(Command::Destroy { window: entry.id }, now);
            self.apply_effects(fx);
        }
        if self.pointer_mode == PointerMode::Model && !self.wm.is_interacting() {
            self.pointer_mode = PointerMode::Normal;
        }
        self.request_redraw();
    }
}
delegate_xdg_shell!(Cupertino);

impl WlrLayerShellHandler for Cupertino {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // a surface without an output goes to the active output (`displays::placement`)
        let output = match output {
            Some(wl) => Output::from_resource(&wl),
            None => self
                .active_output()
                .and_then(|id| self.output_entry(id))
                .map(|o| o.output.clone()),
        };
        let Some(output) = output else {
            surface.send_close();
            return;
        };
        let mut map = layer_map_for_output(&output);
        match map.map_layer(&DesktopLayerSurface::new(surface, namespace.clone())) {
            Ok(()) => tracing::info!(%namespace, "layer surface mapped"),
            Err(e) => tracing::warn!(error = %e, "mapping layer surface failed"),
        }
    }

    /// A popup is created without a parent and given its layer surface here, before its first commit: only now is it
    /// known whose output it must stay on.
    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let found = self.outputs.iter().map(|o| o.output.clone()).find_map(|o| {
            let map = layer_map_for_output(&o);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            layer.map(|l| (o.clone(), l))
        });
        if let Some((output, layer)) = found {
            layer_map_for_output(&output).unmap_layer(&layer);
            self.layer_keyboard_exclusivity(surface.wl_surface(), false);
            self.menu_bar_surface_gone(surface.wl_surface());
            self.dock_surface_gone(surface.wl_surface());
            if let Some(id) = self.output_by_wl(&output).map(|o| o.id) {
                self.refresh_usable_area(id);
            }
            // the client of a prompt that is going may be gone already, so its role cannot be asked
            self.refresh_holds();
        }
    }
}
delegate_layer_shell!(Cupertino);

impl Cupertino {
    /// Negotiate the decoration mode of a toplevel: clients that ask for client-side decorations
    /// keep drawing their own; everything else is decorated by the compositor.
    fn set_decoration_mode(&mut self, toplevel: &ToplevelSurface, mode: DecorationMode) {
        let server = mode != DecorationMode::ClientSide;
        toplevel.with_pending_state(|st| {
            st.decoration_mode = Some(if server {
                DecorationMode::ServerSide
            } else {
                DecorationMode::ClientSide
            })
        });
        if let Some(i) = self.windows.iter().position(|w| &w.toplevel == toplevel) {
            let insets = if server {
                self.decoration_insets(self.windows[i].titlebar)
            } else {
                Default::default()
            };
            let e = &mut self.windows[i];
            e.server_side = server;
            if !e.mapped {
                e.decor_insets = insets;
            }
        }
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    /// Arrange layer surfaces after a commit and send their initial configure.
    pub fn layer_committed(&mut self, surface: &WlSurface) {
        let Some(output) = self.outputs.iter().map(|o| o.output.clone()).find(|o| {
            layer_map_for_output(o)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_some()
        }) else {
            return;
        };
        let initial_sent = with_states(surface, |s| {
            s.data_map
                .get::<LayerSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(true)
        });
        {
            let mut map = layer_map_for_output(&output);
            map.arrange();
            if !initial_sent {
                if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL) {
                    layer.layer_surface().send_configure();
                }
            }
        }
        let exclusive = layer_map_for_output(&output)
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some_and(|l| {
                l.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && matches!(l.layer(), Layer::Top | Layer::Overlay)
            });
        self.layer_keyboard_exclusivity(surface, exclusive);
        if let Some(id) = self.output_by_wl(&output).map(|o| o.id) {
            self.refresh_usable_area(id);
        }
        if crate::authorization::is_prompt(surface) {
            self.refresh_holds();
        }
    }

    /// Start a model-driven move or resize (xdg move/resize request or compositor binding).
    pub fn begin_model_interaction(&mut self, cmd: Command) {
        self.command(cmd);
        if self.wm.is_interacting() {
            self.pointer_mode = PointerMode::Model;
            let serial = self.next_serial();
            let pointer = self.pointer.clone();
            pointer.motion(
                self,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: pointer.current_location(),
                    serial,
                    time: 0,
                },
            );
            pointer.frame(self);
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Some(id) = self.window_id_for_surface(&root) else {
            self.unconstrain_layer_popup(popup, &kind, &root);
            return;
        };
        let Some(entry) = self.entry(id) else { return };
        let now = self.now();
        let Some(pres) = self.wm.presentation(id, now) else {
            return;
        };
        let out = self
            .wm
            .window(id)
            .and_then(|w| self.wm.output(w.output))
            .map(|o| o.rect)
            .unwrap_or(pres.frame);
        let geo = entry.window.geometry();
        let mut target: Rectangle<i32, Logical> = Self::rect_to_logical(out);
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= Point::from((pres.frame.x.round() as i32, pres.frame.y.round() as i32));
        target.loc += geo.loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    /// A popup of a layer surface is kept inside the output the surface is on, flipped or slid as its positioner
    /// allows: a dock's menu at the bottom edge opens above its tile. A root that is no layer surface is left alone.
    fn unconstrain_layer_popup(&self, popup: &PopupSurface, kind: &PopupKind, root: &WlSurface) {
        for out in &self.outputs {
            let map = layer_map_for_output(&out.output);
            let Some(geo) = map
                .layer_for_surface(root, WindowSurfaceType::TOPLEVEL)
                .and_then(|layer| map.layer_geometry(layer))
            else {
                continue;
            };
            // the output in the coordinates of the layer surface, then of the popup's parent
            let mut target = Self::rect_to_logical(out.rect);
            target.loc = Point::from((0, 0)) - geo.loc;
            target.loc -= get_popup_toplevel_coords(kind);
            popup.with_pending_state(|state| {
                state.geometry = state.positioner.get_unconstrained_geometry(target);
            });
            return;
        }
    }
}
