//! Windows' surface layers ([`WindowSystem::present_surface_layers`]): each a `wl_surface` made a `cupertino_layer_v1`
//! of the window's surface, its frame copied into buffers of its own, its rect and look sent as the layer's values,
//! and the application's changes ([`WindowRequest::Layer`]) sent as the layer's requests, so they apply with the
//! window's next commit. Layers need version 4 of the global (content blur): below it a window has none and the
//! runtime paints marked subtrees in place. From version 5 a layer's look carries the edge along its glass.
//!
//! [`WindowSystem::present_surface_layers`]: cupertino_ui_platform::WindowSystem::present_surface_layers
//! [`WindowRequest::Layer`]: cupertino_ui_platform::WindowRequest

use std::collections::{HashMap, HashSet};

use cupertino_geometry::Rect;
use cupertino_protocol_app::{AppEvent, Layer, LayerId, LayerProperty as Wire, Layers};
use cupertino_ui_platform::{
    LayerChange, LayerProperty, PlatformError, SurfaceLayer, SurfaceLayerLook, WindowId,
};
use cupertino_wayland_client::{Connection, ExternalRole, PixelRect, SurfaceId};

// a change through samples the toolkit takes is one the wire carries
const _: () =
    assert!(cupertino_protocol_app::MAX_SAMPLES == cupertino_ui_platform::MAX_LAYER_SAMPLES);

fn wire(p: LayerProperty) -> Wire {
    match p {
        LayerProperty::X => Wire::X,
        LayerProperty::Y => Wire::Y,
        LayerProperty::Width => Wire::Width,
        LayerProperty::Height => Wire::Height,
        LayerProperty::Opacity => Wire::Opacity,
        LayerProperty::Scale => Wire::Scale,
        LayerProperty::CornerRadius => Wire::CornerRadius,
        LayerProperty::ContentBlur => Wire::ContentBlur,
    }
}

fn protocol_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Protocol(e.to_string())
}

/// The role of a layer's surface on the transport: the layer object gives it its role; it needs no configure, is drawn
/// at its parent's scale and is destroyed before the surface.
struct LayerRole(SurfaceId);

impl ExternalRole for LayerRole {
    fn ack_configure(&self, _: u32) {}

    fn destroy(&self) {}

    fn initial_commit(&self) -> bool {
        false
    }

    fn drawn_with(&self) -> Option<SurfaceId> {
        Some(self.0)
    }
}

/// One surface layer of a window.
pub(crate) struct Shown {
    pub(crate) key: u64,
    pub(crate) surface: SurfaceId,
    layer: Layer,
    rect: Rect,
    look: SurfaceLayerLook,
    /// Damage of frames that were not shown, carried into the next.
    pub(crate) carried: Vec<PixelRect>,
    /// The pixel size of the frame shown last.
    pub(crate) shown: Option<(u32, u32)>,
}

impl Shown {
    /// The layer's size in whole points, as its buffers are drawn at.
    pub(crate) fn size(&self) -> (u32, u32) {
        (
            self.rect.width().round().max(1.0) as u32,
            self.rect.height().round().max(1.0) as u32,
        )
    }
}

/// Every window's surface layers, the changes asked for layers not made yet, and the animations still running.
#[derive(Default)]
pub(crate) struct SurfaceLayers {
    windows: HashMap<WindowId, Vec<Shown>>,
    pending: HashMap<(WindowId, u64), Vec<LayerChange>>,
    running: HashMap<LayerId, HashSet<u32>>,
    next_serial: u32,
}

impl SurfaceLayers {
    /// Makes `window`'s layers those of `wanted`, in their order: a new key makes a surface and a layer of `parent`,
    /// a key no longer wanted goes (its surface returned, for the caller to let go of its buffers before destroying
    /// it), and a rect or look that changed is sent.
    pub(crate) fn sync(
        &mut self,
        conn: &mut Connection,
        layers: &Layers,
        window: WindowId,
        parent: SurfaceId,
        wanted: &[SurfaceLayer<'_>],
    ) -> Result<Vec<SurfaceId>, PlatformError> {
        let shown = self.windows.entry(window).or_default();
        let mut gone = Vec::new();
        shown.retain(|s| {
            let keep = wanted.iter().any(|w| w.key == s.key);
            if !keep {
                gone.push(s.surface);
            }
            keep
        });
        for w in wanted {
            let i = match shown.iter().position(|s| s.key == w.key) {
                Some(i) => i,
                None => {
                    let surface = conn.external_surface(|_, _, _| LayerRole(parent));
                    let Some(layer) = layers.layer(conn, surface, parent) else {
                        let _ = conn.destroy(surface);
                        return Err(PlatformError::Protocol(
                            "a layer of a surface that is gone".into(),
                        ));
                    };
                    shown.push(Shown {
                        key: w.key,
                        surface,
                        layer,
                        rect: Rect::ZERO,
                        look: SurfaceLayerLook {
                            radius: -1.0,
                            ..SurfaceLayerLook::default()
                        },
                        carried: Vec::new(),
                        shown: None,
                    });
                    shown.len() - 1
                }
            };
            let s = &mut shown[i];
            if s.rect != w.rect {
                for (p, v) in [
                    (Wire::X, w.rect.x()),
                    (Wire::Y, w.rect.y()),
                    (Wire::Width, w.rect.width().max(0.0)),
                    (Wire::Height, w.rect.height().max(0.0)),
                ] {
                    s.layer.set(p, v).map_err(protocol_error)?;
                }
                s.rect = w.rect;
            }
            if s.look != w.look {
                let look = w.look;
                if look.radius != s.look.radius {
                    s.layer
                        .set(Wire::CornerRadius, look.radius.max(0.0))
                        .map_err(protocol_error)?;
                }
                if look.glass != s.look.glass {
                    s.layer
                        .set_glass(look.glass.map(crate::window_system::protocol_style), 1.0)
                        .map_err(protocol_error)?;
                }
                if look.group != s.look.group {
                    s.layer
                        .set_glass_group(look.group.0, look.group.1)
                        .map_err(protocol_error)?;
                }
                if look.masks != s.look.masks {
                    s.layer.set_masks_to_bounds(look.masks);
                }
                if look.edge != s.look.edge && layers.draws_glass_edges() {
                    let e = look.edge;
                    s.layer
                        .set_glass_edge(e.width, e.color.map(f64::from))
                        .map_err(protocol_error)?;
                }
                s.look = look;
            }
            // the changes asked for before the layer was made go with its first values
            if let Some(changes) = self.pending.remove(&(window, w.key)) {
                send(&s.layer, &changes, &mut self.next_serial, &mut self.running)?;
            }
        }
        Ok(gone)
    }

    /// The layers `window` shows, in order.
    pub(crate) fn shown_mut(&mut self, window: WindowId) -> &mut [Shown] {
        self.windows
            .get_mut(&window)
            .map_or(&mut [], |v| v.as_mut_slice())
    }

    /// Sends `changes` to `window`'s layer `key`; false when it is not made yet, the changes then waiting for it.
    pub(crate) fn change(
        &mut self,
        window: WindowId,
        key: u64,
        changes: Vec<LayerChange>,
    ) -> Result<bool, PlatformError> {
        let Some(s) = self
            .windows
            .get(&window)
            .and_then(|v| v.iter().find(|s| s.key == key))
        else {
            self.pending
                .entry((window, key))
                .or_default()
                .extend(changes);
            return Ok(false);
        };
        send(&s.layer, &changes, &mut self.next_serial, &mut self.running)?;
        Ok(true)
    }

    /// Whether every layer of `window` shows its values: none of their animations runs.
    pub(crate) fn settled(&self, window: WindowId) -> bool {
        self.windows.get(&window).is_none_or(|v| {
            v.iter().all(|s| {
                self.running
                    .get(&s.layer.id())
                    .is_none_or(HashSet::is_empty)
            })
        })
    }

    /// Follows a layer event of one of the layers; false for any other event.
    pub(crate) fn heard(&mut self, event: &AppEvent) -> bool {
        let (layer, serial) = match *event {
            AppEvent::LayerFinished { layer, serial } => (layer, Some(serial)),
            AppEvent::LayerReplaced { layer, serial, .. } => (layer, Some(serial)),
            AppEvent::LayerSettled { layer } => (layer, None),
            _ => return false,
        };
        let Some(running) = self.running.get_mut(&layer) else {
            return false;
        };
        match serial {
            Some(serial) => {
                running.remove(&serial);
            }
            None => running.clear(),
        }
        true
    }

    /// The window goes: its layers' surfaces, for the caller to let go of their buffers and destroy after the layers.
    pub(crate) fn forget(&mut self, window: WindowId) -> Vec<SurfaceId> {
        self.pending.retain(|(w, _), _| *w != window);
        let gone = self.windows.remove(&window).unwrap_or_default();
        for s in &gone {
            self.running.remove(&s.layer.id());
        }
        gone.into_iter().map(|s| s.surface).collect()
    }
}

fn send(
    layer: &Layer,
    changes: &[LayerChange],
    next_serial: &mut u32,
    running: &mut HashMap<LayerId, HashSet<u32>>,
) -> Result<(), PlatformError> {
    for change in changes {
        let property = wire(change.property);
        match (change.curve, &change.samples) {
            (None, _) => layer.set(property, change.to),
            (Some(curve), samples) => {
                *next_serial = next_serial.wrapping_add(1);
                running.entry(layer.id()).or_default().insert(*next_serial);
                match (curve, samples) {
                    (cupertino_ui_platform::LayerCurve::Timed { duration, .. }, Some(values)) => {
                        layer.animate_samples(*next_serial, property, duration, values)
                    }
                    _ => layer.animate(*next_serial, property, change.to, curve, change.velocity),
                }
            }
        }
        .map_err(protocol_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_for_a_layer_not_made_yet_wait_for_it_and_go_with_its_window() {
        let mut layers = SurfaceLayers::default();
        let (window, other) = (WindowId(1), WindowId(2));
        let fade = LayerChange::new(LayerProperty::Opacity, 0.0, None);
        let blur = LayerChange::through(LayerProperty::ContentBlur, 0.1, [0.0, 4.0].into());
        assert_eq!(layers.change(window, 3, vec![fade.clone()]), Ok(false));
        assert_eq!(layers.change(window, 3, vec![blur.clone()]), Ok(false));
        assert_eq!(layers.change(other, 3, vec![fade.clone()]), Ok(false));
        assert_eq!(
            layers.pending.get(&(window, 3)),
            Some(&vec![fade, blur]),
            "in the order asked for"
        );
        assert!(
            layers.settled(window),
            "a window with no layers runs nothing"
        );
        assert!(layers.forget(window).is_empty());
        assert!(!layers.pending.contains_key(&(window, 3)));
        assert!(
            layers.pending.contains_key(&(other, 3)),
            "another window's stay"
        );
    }

    #[test]
    fn every_property_has_its_own_wire_property_and_content_blur_needs_version_4() {
        let all = [
            LayerProperty::X,
            LayerProperty::Y,
            LayerProperty::Width,
            LayerProperty::Height,
            LayerProperty::Opacity,
            LayerProperty::Scale,
            LayerProperty::CornerRadius,
            LayerProperty::ContentBlur,
        ];
        let wired: HashSet<Wire> = all.into_iter().map(wire).collect();
        assert_eq!(wired.len(), all.len());
        assert_eq!(wire(LayerProperty::ContentBlur).since(), 4);
        assert!(all[..7].iter().all(|&p| wire(p).since() < 4));
    }
}
