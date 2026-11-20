//! The display side of the stand-in: the globals a component's Kit loop binds on its connection (compositor, shared
//! memory, the window shell, a seat, one output, the layer shell, `cupertino_shell_v1`, the transaction manager and the
//! app effects' appearance observer), so a component runs on it as on a server. A layer surface is configured at its first commit, every commit answers the
//! surface's frame callbacks, and what the stand-in saw goes to [`Seen`].

use std::collections::HashMap;

use cupertino_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_protocols::server::app_effects::cupertino_appearance_v1::{
    CupertinoAppearanceV1, DisplayOption,
};
use cupertino_protocols::server::cupertino_dock_v1::CupertinoDockV1;
use cupertino_protocols::server::cupertino_menu_bar_v1::CupertinoMenuBarV1;
use cupertino_protocols::server::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::server::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::server::cupertino_window_manager_v1::CupertinoWindowManagerV1;
use cupertino_protocols::server::transaction::cupertino_hole_v1::CupertinoHoleV1;
use cupertino_protocols::server::transaction::cupertino_transaction_manager_v1::{
    self as transaction_manager, CupertinoTransactionManagerV1,
};
use cupertino_protocols::server::transaction::cupertino_transaction_v1::CupertinoTransactionV1;
use wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase;
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use wayland_server::backend::ObjectId;
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_region::WlRegion;
use wayland_server::protocol::wl_seat::{self, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::{Seen, State};

/// The stand-in output's size in pixels, at scale 1, which a layer surface stretched between edges is configured to.
pub const OUTPUT_SIZE: (u32, u32) = (1920, 1080);

/// The versions of the display globals the stand-in offers.
#[derive(Clone, Copy, Debug)]
pub struct Offer {
    /// `cupertino_shell_v1`.
    pub shell: u32,
    /// `zwlr_layer_shell_v1`.
    pub layer_shell: u32,
    /// `cupertino_app_effects_v1`: the appearance observer's appearance and display options, and nothing else the
    /// stand-in answers.
    pub app_effects: u32,
}

impl Offer {
    /// The versions a current server offers.
    pub const CURRENT: Offer = Offer {
        shell: 26,
        layer_shell: 4,
        app_effects: 1,
    };
}

/// What the stand-in keeps of the clients' surfaces.
#[derive(Default)]
pub(super) struct Surfaces {
    surfaces: HashMap<ObjectId, Surface>,
    layers: HashMap<ObjectId, Layer>,
    serial: u32,
}

#[derive(Default)]
struct Surface {
    attached: Option<Option<WlBuffer>>,
    shown: Option<WlBuffer>,
    callbacks: Vec<WlCallback>,
}

struct Layer {
    namespace: String,
    size: (u32, u32),
    configured: bool,
}

/// Creates the display globals `offer` names.
pub(super) fn create_globals(handle: &mut DisplayHandle, offer: Offer) {
    handle.create_global::<State, WlCompositor, ()>(6, ());
    handle.create_global::<State, WlShm, ()>(1, ());
    handle.create_global::<State, XdgWmBase, ()>(6, ());
    handle.create_global::<State, WlSeat, ()>(7, ());
    handle.create_global::<State, WlOutput, ()>(4, ());
    handle.create_global::<State, ZwlrLayerShellV1, ()>(offer.layer_shell, ());
    handle.create_global::<State, CupertinoShellV1, ()>(offer.shell, ());
    handle.create_global::<State, CupertinoTransactionManagerV1, ()>(1, ());
    handle.create_global::<State, CupertinoAppEffectsV1, ()>(offer.app_effects, ());
}

/// The session's display options as the stand-in tells them, and the appearance observers it tells them to.
pub(super) struct Appearance {
    options: DisplayOption,
    observers: Vec<CupertinoAppearanceV1>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            options: DisplayOption::empty(),
            observers: Vec::new(),
        }
    }
}

impl Appearance {
    /// The session's display options are `options` from now on: every live observer hears them.
    pub(super) fn set_display_options(&mut self, options: DisplayOption) {
        self.options = options;
        self.observers.retain(|observer| observer.is_alive());
        for observer in &self.observers {
            observer.display_options(options);
        }
    }
}

impl State {
    /// The component `client`'s connection belongs to; empty for the host's.
    pub(super) fn component_of(&self, client: &Client) -> String {
        self.component_clients
            .get(&client.id())
            .cloned()
            .unwrap_or_default()
    }

    fn commit(&mut self, client: &Client, surface: &WlSurface) {
        let component = self.component_of(client);
        let id = surface.id();
        if let Some(layer) = self.display.layers.get_mut(&id) {
            if !layer.configured {
                layer.configured = true;
                self.display.serial += 1;
                let (width, height) = layer.size;
                let width = if width == 0 { OUTPUT_SIZE.0 } else { width };
                let height = if height == 0 { OUTPUT_SIZE.1 } else { height };
                if let Some(resource) = self.layer_resources.get(&id) {
                    resource.configure(self.display.serial, width, height);
                }
            }
        }
        let state = self.display.surfaces.entry(id.clone()).or_default();
        if let Some(attached) = state.attached.take() {
            if let Some(old) = std::mem::replace(&mut state.shown, attached.clone()) {
                if Some(&old) != attached.as_ref() {
                    old.release();
                }
            }
            if attached.is_some() {
                let namespace = self
                    .display
                    .layers
                    .get(&id)
                    .map(|layer| layer.namespace.clone())
                    .unwrap_or_default();
                let _ = self.seen.send(Seen::Drawn {
                    component,
                    namespace,
                });
            }
        }
        let state = self.display.surfaces.entry(id).or_default();
        for callback in state.callbacks.drain(..) {
            callback.done(0);
        }
    }
}

macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for State {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(resource, ());
            }
        }
    )*};
}

plain_global!(
    WlCompositor,
    XdgWmBase,
    ZwlrLayerShellV1,
    CupertinoShellV1,
    CupertinoTransactionManagerV1,
    CupertinoAppEffectsV1
);

macro_rules! ignore_requests {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$interface,
                _: <$interface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    )*};
}

ignore_requests!(
    XdgWmBase,
    WlRegion,
    WlSeat,
    WlOutput,
    WlCallback,
    WlBuffer,
    CupertinoWindowManagerV1,
    CupertinoMenuBarV1,
    CupertinoDockV1,
    CupertinoTransactionV1,
    CupertinoHoleV1,
    CupertinoAppearanceV1
);

impl GlobalDispatch<WlShm, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlShm>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let shm = init.init(resource, ());
        shm.format(wl_shm::Format::Argb8888);
        shm.format(wl_shm::Format::Xrgb8888);
    }
}

impl GlobalDispatch<WlSeat, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(wl_seat::Capability::empty());
    }
}

impl GlobalDispatch<WlOutput, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlOutput>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(resource, ());
        output.geometry(
            0,
            0,
            600,
            340,
            wl_output::Subpixel::Unknown,
            "Test".into(),
            "Output".into(),
            wl_output::Transform::Normal,
        );
        output.mode(
            wl_output::Mode::Current,
            OUTPUT_SIZE.0 as i32,
            OUTPUT_SIZE.1 as i32,
            60_000,
        );
        output.scale(1);
        output.name("TEST-1".into());
        output.done();
    }
}

impl Dispatch<WlCompositor, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                init.init(id, ());
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        surface: &WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                state
                    .display
                    .surfaces
                    .entry(surface.id())
                    .or_default()
                    .attached = Some(buffer);
            }
            wl_surface::Request::Frame { callback } => {
                let callback = init.init(callback, ());
                state
                    .display
                    .surfaces
                    .entry(surface.id())
                    .or_default()
                    .callbacks
                    .push(callback);
            }
            wl_surface::Request::Commit => state.commit(client, surface),
            wl_surface::Request::Destroy => {
                state.display.surfaces.remove(&surface.id());
                state.display.layers.remove(&surface.id());
                state.layer_resources.remove(&surface.id());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
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

impl Dispatch<WlShmPool, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            surface,
            namespace,
            ..
        } = request
        {
            let resource = init.init(id, surface.id());
            state.display.layers.insert(
                surface.id(),
                Layer {
                    namespace,
                    size: (0, 0),
                    configured: false,
                },
            );
            state.layer_resources.insert(surface.id(), resource);
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ObjectId> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        surface: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_surface_v1::Request::SetSize { width, height } = request {
            if let Some(layer) = state.display.layers.get_mut(surface) {
                layer.size = (width, height);
            }
        }
    }
}

impl Dispatch<CupertinoShellV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &CupertinoShellV1,
        request: cupertino_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_shell_v1::Request::GetMenuBar { id, .. } => {
                init.init(id, ());
            }
            cupertino_shell_v1::Request::GetDock { id, .. } => {
                init.init(id, ());
            }
            cupertino_shell_v1::Request::GetSession { id } => {
                init.init(id, ());
            }
            cupertino_shell_v1::Request::GetWindowManager { id, .. } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoSessionV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &CupertinoSessionV1,
        request: cupertino_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let cupertino_session_v1::Request::ActivateApp { serial, desktop_id } = request {
            let _ = state.seen.send(Seen::ActivateApp {
                component: state.component_of(client),
                serial,
                desktop_id,
            });
        }
    }
}

impl Dispatch<CupertinoTransactionManagerV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &CupertinoTransactionManagerV1,
        request: transaction_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            transaction_manager::Request::Create { id, .. }
            | transaction_manager::Request::Join { id, .. } => {
                init.init(id, ());
            }
            transaction_manager::Request::GetHole { id, .. } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoAppEffectsV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        // the light appearance, then the display options, as a server answers a new observer
        if let cupertino_app_effects_v1::Request::GetAppearance { id } = request {
            let observer = init.init(id, ());
            observer.appearance(0);
            observer.display_options(state.appearance.options);
            state.appearance.observers.push(observer);
        }
    }
}
