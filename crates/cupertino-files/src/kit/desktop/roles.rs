//! The desktop's surfaces: one layer surface on every output, on the layer under the windows, over the whole output but
//! the room the bars keep, taking the keyboard only once a press lands on it.

use cupertino_kit::platform::WindowSpec;
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};

use super::layer::{DesktopShell, LayerEvent};
use super::{Message, Screen, APP_ID};

/// What the roles tell the desktop.
#[derive(Clone, Debug)]
pub enum RoleNote {
    /// The outputs as the compositor now describes them.
    Outputs(Vec<Screen>),
    /// The surface of output `key` is `width` by `height` points.
    Sized { key: u32, width: f64, height: f64 },
}

pub(super) fn output_key(id: OutputId) -> u32 {
    id.registry_name()
}

/// The compositor's outputs that have a logical size, in its order: the first is the main display.
pub(super) fn screens(conn: &Connection) -> Vec<Screen> {
    conn.outputs()
        .into_iter()
        .filter_map(|o| {
            let g = o.geometry?;
            Some(Screen {
                key: output_key(o.id),
                width: f64::from(g.width),
                height: f64::from(g.height),
            })
        })
        .collect()
}

/// The window `app_id` names the output it is for: `cupertino-desktop@KEY`.
fn output_of(app_id: &str) -> Option<u32> {
    let (prefix, key) = app_id.rsplit_once('@')?;
    (prefix == APP_ID).then(|| key.parse().ok())?
}

pub(super) struct DesktopRoles {
    pub shell: DesktopShell,
    /// The surfaces made, with the output each is on.
    pub surfaces: Vec<(u32, SurfaceId)>,
    pub notes: async_channel::Sender<Message>,
}

impl DesktopRoles {
    fn send(&self, note: RoleNote) {
        // the receiver is gone only while the process exits
        let _ = self.notes.try_send(Message::Role(note));
    }
}

impl SurfaceRoles for DesktopRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<Result<SurfaceId, WaylandError>> {
        let key = output_of(&spec.app_id)?;
        let Some(output) = conn
            .outputs()
            .into_iter()
            .map(|o| o.id)
            .find(|&id| output_key(id) == key)
        else {
            return Some(Err(WaylandError::Protocol(
                "the desktop's output went away",
            )));
        };
        let surface = match self.shell.surface(conn, output) {
            Ok(surface) => surface,
            Err(e) => return Some(Err(e)),
        };
        self.surfaces.push((key, surface));
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        match *event.downcast_ref::<LayerEvent>()? {
            LayerEvent::Configure { surface, size } => {
                let key = self.surfaces.iter().find(|(_, s)| *s == surface)?.0;
                // the server sizes the surface to its output: a size it leaves to the client closes it
                if size.0 == 0 || size.1 == 0 {
                    return Some(RoleEvent::Closed(surface));
                }
                self.send(RoleNote::Sized {
                    key,
                    width: f64::from(size.0),
                    height: f64::from(size.1),
                });
                Some(RoleEvent::Configure { surface, size })
            }
            LayerEvent::Closed(surface) => {
                self.surfaces.iter().find(|(_, s)| *s == surface)?;
                Some(RoleEvent::Closed(surface))
            }
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        self.surfaces.retain(|(_, s)| *s != surface);
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        self.send(RoleNote::Outputs(screens(conn)));
    }
}
