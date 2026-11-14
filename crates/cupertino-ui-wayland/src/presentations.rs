//! Windows' presentations on the compositor's clock (`cupertino_layers_v1` version 2): the offset, opacity and scale
//! of all a window draws, animated by the compositor, so a panel opens and closes without a frame drawn.
//!
//! A window's presentation is made on its first [`WindowRequest::Present`](cupertino_ui_platform::WindowRequest) and
//! lives with the window. Each change is sent at once; the window system then commits the surface without a buffer,
//! or leaves it to the window's first frame. The animations each change starts are followed until they finish or
//! are replaced, so the window system knows when what is shown is again the window's layout ([`Presentations::settled`]).

use std::collections::HashMap;

use cupertino_protocol_app::{AppEvent, Layer, LayerId, LayerProperty, Layers};
use cupertino_ui_platform::{PlatformError, PresentChange, PresentProperty, WindowId};
use cupertino_wayland_client::{Connection, SurfaceId};

fn property(p: PresentProperty) -> LayerProperty {
    match p {
        PresentProperty::OffsetX => LayerProperty::X,
        PresentProperty::OffsetY => LayerProperty::Y,
        PresentProperty::Opacity => LayerProperty::Opacity,
        PresentProperty::Scale => LayerProperty::Scale,
    }
}

/// One window's presentation and its animations still running, by serial.
struct Presentation {
    layer: Layer,
    running: HashMap<u32, PresentProperty>,
}

/// The layers global, when the compositor presents surfaces as a whole, and the windows' presentations.
#[derive(Default)]
pub(crate) struct Presentations {
    layers: Option<Layers>,
    shown: HashMap<WindowId, Presentation>,
    next_serial: u32,
}

impl Presentations {
    /// Bind the global; without it, or below version 2, no window has a presentation and the application animates
    /// its windows itself.
    pub(crate) fn bind(conn: &mut Connection) -> Presentations {
        Presentations {
            layers: Layers::bind(conn).filter(Layers::presents_surfaces),
            ..Presentations::default()
        }
    }

    pub(crate) fn available(&self) -> bool {
        self.layers.is_some()
    }

    /// Send `changes` to the presentation of `window` (its surface `surface`), made on its first change.
    pub(crate) fn change(
        &mut self,
        conn: &Connection,
        window: WindowId,
        surface: SurfaceId,
        changes: &[PresentChange],
    ) -> Result<(), PlatformError> {
        let layers = self.layers.as_ref().ok_or(PlatformError::Unsupported)?;
        if !self.shown.contains_key(&window) {
            let layer = layers
                .presentation(conn, surface)
                .ok_or(PlatformError::Unsupported)?;
            self.shown.insert(
                window,
                Presentation {
                    layer,
                    running: HashMap::new(),
                },
            );
        }
        let p = self.shown.get_mut(&window).expect("made above");
        for change in changes {
            let prop = property(change.property);
            let sent = match change.curve {
                None => p.layer.set(prop, change.to),
                Some(curve) => {
                    self.next_serial = self.next_serial.wrapping_add(1);
                    let serial = self.next_serial;
                    p.running.insert(serial, change.property);
                    p.layer
                        .animate(serial, prop, change.to, curve, change.velocity)
                }
            };
            sent.map_err(|e| PlatformError::Protocol(e.to_string()))?;
        }
        Ok(())
    }

    /// Whether `window` shows its layout: it has no presentation, or none of its animations runs.
    pub(crate) fn settled(&self, window: WindowId) -> bool {
        self.shown.get(&window).is_none_or(|p| p.running.is_empty())
    }

    /// Follow a layer event; false for any other event.
    pub(crate) fn heard(&mut self, event: &AppEvent) -> bool {
        let (layer, serial): (LayerId, Option<u32>) = match *event {
            AppEvent::LayerFinished { layer, serial } => (layer, Some(serial)),
            AppEvent::LayerReplaced { layer, serial, .. } => (layer, Some(serial)),
            AppEvent::LayerSettled { layer } => (layer, None),
            _ => return false,
        };
        if let Some(p) = self.shown.values_mut().find(|p| p.layer.id() == layer) {
            match serial {
                Some(serial) => {
                    p.running.remove(&serial);
                }
                None => p.running.clear(),
            }
        }
        true
    }

    /// The window is gone, and its presentation with it.
    pub(crate) fn forget(&mut self, window: WindowId) {
        self.shown.remove(&window);
    }
}
