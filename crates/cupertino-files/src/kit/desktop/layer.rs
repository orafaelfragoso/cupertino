//! The standard layer shell (`zwlr_layer_shell_v1`) on the transport, as far as the desktop needs it: a surface on the
//! layer under the windows, over a whole output, whose configures come back as [`LayerEvent`]s.

use cupertino_wayland_client::{Connection, Error, ExternalRole, OutputId, State, SurfaceId};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

/// The surfaces' role for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-desktop";

/// What a desktop surface heard from the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayerEvent {
    /// Draw at this size, in points; the transport has acknowledged it.
    Configure {
        surface: SurfaceId,
        size: (u32, u32),
    },
    /// The server closed the surface (its output went away).
    Closed(SurfaceId),
}

/// User data of the shell global, whose events carry nothing.
struct Quiet;

/// User data of a desktop surface: the transport's id of it.
struct OfDesktop(SurfaceId);

/// The layer shell global.
pub(super) struct DesktopShell(ZwlrLayerShellV1);

impl DesktopShell {
    /// Bind the global; `None` when the server does not offer version 4, which on-demand keyboard interactivity needs.
    pub fn bind(conn: &mut Connection) -> Option<DesktopShell> {
        conn.bind_extension(4..=4, Quiet).map(DesktopShell)
    }

    /// A surface on the bottom layer of `output`, anchored to its four edges so the server sizes it to the output (but
    /// what the bars keep for themselves), taking the keyboard once a press lands on it.
    pub fn surface(&self, conn: &mut Connection, output: OutputId) -> Result<SurfaceId, Error> {
        let wl_output = conn
            .wl_output(output)
            .ok_or(Error::Protocol("the desktop's output went away"))?;
        Ok(conn.external_surface(|wl, qh, id| {
            let layer = self.0.get_layer_surface(
                wl,
                Some(&wl_output),
                Layer::Bottom,
                NAMESPACE.to_string(),
                qh,
                OfDesktop(id),
            );
            layer.set_size(0, 0);
            layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
            // 0: keep clear of what the bars reserve
            layer.set_exclusive_zone(0);
            layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
            DesktopRole(layer)
        }))
    }
}

/// The role of a desktop surface.
struct DesktopRole(ZwlrLayerSurfaceV1);

impl ExternalRole for DesktopRole {
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

impl Dispatch<ZwlrLayerShellV1, Quiet> for State {
    fn event(
        _: &mut State,
        _: &ZwlrLayerShellV1,
        _: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, OfDesktop> for State {
    fn event(
        state: &mut State,
        _: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &OfDesktop,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                let surface = surface.0;
                state.configure_external(surface, serial, (width, height));
                state.emit(LayerEvent::Configure {
                    surface,
                    size: (width, height),
                });
            }
            zwlr_layer_surface_v1::Event::Closed => state.emit(LayerEvent::Closed(surface.0)),
            _ => {}
        }
    }
}
