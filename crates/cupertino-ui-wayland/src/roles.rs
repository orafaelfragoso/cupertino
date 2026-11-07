//! Windows whose role a protocol outside the window system gives them, such as a desktop shell's layer surfaces.

use cupertino_ui_platform::WindowSpec;
use cupertino_wayland_client::{Connection, Error, ExtensionEvent, SurfaceId};

/// Gives the windows an application opens roles from protocols the window system does not speak. The application that
/// speaks them picks a role by the window's `app_id`, creates its surface with [`Connection::external_surface`], and
/// reads each configure and close of its surfaces from the transport's extension events. Such a window draws, takes
/// input and has popups like a toplevel, without the toplevel's requests.
pub trait SurfaceRoles {
    /// The surface of a new window opened with `spec`, with its role; `None` opens a toplevel.
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<Result<SurfaceId, Error>>;

    /// What `event` says about a surface given its role here; `None` for any other event.
    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent>;

    /// `surface` was destroyed with its window.
    fn destroyed(&mut self, surface: SurfaceId) {
        let _ = surface;
    }

    /// `surface`, one of the application's windows, took (`focused`) or lost the keyboard focus; heard before the
    /// window's own focus event.
    fn keyboard_focus(&mut self, surface: SurfaceId, focused: bool) {
        let _ = (surface, focused);
    }

    /// The compositor added, removed or described anew one of its outputs ([`Connection::outputs`]).
    fn outputs_changed(&mut self, conn: &Connection) {
        let _ = conn;
    }

    /// Called at the start of every dispatch, before the window system waits: the roles send the requests their
    /// application asked for since (a desktop shell's session lock and unlock) and read the transport's state, such as
    /// the keyboard's modifiers. The window system flushes what they send with its own requests.
    fn prepare(&mut self, conn: &mut Connection) {
        let _ = conn;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleEvent {
    /// Draw `surface` at `size`, in logical points; the role has resolved any extent the compositor left to the
    /// client, and the transport acknowledges the configure with the next present.
    Configure {
        surface: SurfaceId,
        size: (u32, u32),
    },
    /// The compositor closed `surface`.
    Closed(SurfaceId),
}
