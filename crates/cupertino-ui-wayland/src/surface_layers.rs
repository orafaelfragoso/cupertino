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

/// Which part of a layer's place a property is: x, y, width or height, in that order; `None` for the others.
fn place_index(p: LayerProperty) -> Option<usize> {
    match p {
        LayerProperty::X => Some(0),
        LayerProperty::Y => Some(1),
        LayerProperty::Width => Some(2),
        LayerProperty::Height => Some(3),
        _ => None,
    }
}

/// A value as the wire carries it: in 256ths of a point, the nearest.
fn on_wire(v: f64) -> i64 {
    (v * 256.0).round() as i64
}

/// The parts of a layer's place to send now, for a layer declared at `place` (x, y, width, height): each part the
/// compositor was not told at that value (`told`, as the wire carries it; `None` once an animation moved it), unless
/// an animation the application sent still moves it (`moving`). A value sent while an animation runs replaces the
/// animation, so the part an animation moves is left to it until it ends; it is sent with the first frame after.
/// `told` then holds what the compositor has.
fn place_to_send(
    told: &mut [Option<i64>; 4],
    moving: [bool; 4],
    place: [f64; 4],
) -> [Option<f64>; 4] {
    let mut send = [None; 4];
    for i in 0..4 {
        if moving[i] {
            told[i] = None;
        } else if told[i] != Some(on_wire(place[i])) {
            told[i] = Some(on_wire(place[i]));
            send[i] = Some(place[i]);
        }
    }
    send
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
    /// Its x, y, width and height as the compositor was last told them ([`place_to_send`]).
    told: [Option<i64>; 4],
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
    /// The running animations that move a part of their layer's place (x, y, width or height, by index), by serial.
    moving: HashMap<LayerId, HashMap<u32, usize>>,
    /// The window of each layer that ran an animation.
    owners: HashMap<LayerId, WindowId>,
    next_serial: u32,
    /// The windows whose layers came to rest, not yet taken ([`SurfaceLayers::take_settled`]).
    came_to_rest: Vec<WindowId>,
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
        let was = self.settled(window);
        let shown = self.windows.entry(window).or_default();
        let mut gone = Vec::new();
        shown.retain(|s| {
            let keep = wanted.iter().any(|w| w.key == s.key);
            if !keep {
                gone.push(s.surface);
                // what it ran goes with it, so it keeps its window from coming to rest no longer
                self.running.remove(&s.layer.id());
                self.moving.remove(&s.layer.id());
                self.owners.remove(&s.layer.id());
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
                        told: [None; 4],
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
            // an animation the application sent keeps the part of the place it moves: a value sent now would
            // replace it
            let moving = moving_parts(&self.moving, s.layer.id());
            let place = [
                w.rect.x(),
                w.rect.y(),
                w.rect.width().max(0.0),
                w.rect.height().max(0.0),
            ];
            let parts = place_to_send(&mut s.told, moving, place);
            for (p, v) in [Wire::X, Wire::Y, Wire::Width, Wire::Height]
                .into_iter()
                .zip(parts)
            {
                if let Some(v) = v {
                    s.layer.set(p, v).map_err(protocol_error)?;
                }
            }
            s.rect = w.rect;
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
                self.owners.insert(s.layer.id(), window);
                send(
                    &s.layer,
                    &changes,
                    &mut self.next_serial,
                    &mut self.running,
                    &mut self.moving,
                )?;
            }
        }
        // the last layer still running went
        if !was && self.settled(window) {
            self.came_to_rest.push(window);
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
            .get_mut(&window)
            .and_then(|v| v.iter_mut().find(|s| s.key == key))
        else {
            self.pending
                .entry((window, key))
                .or_default()
                .extend(changes);
            return Ok(false);
        };
        self.owners.insert(s.layer.id(), window);
        // a part of the place set here is what the compositor has now
        for change in changes.iter().filter(|c| c.curve.is_none()) {
            if let Some(i) = place_index(change.property) {
                s.told[i] = Some(on_wire(change.to));
            }
        }
        send(
            &s.layer,
            &changes,
            &mut self.next_serial,
            &mut self.running,
            &mut self.moving,
        )?;
        Ok(true)
    }

    /// Whether every layer of `window` shows its values: none of their animations runs.
    pub(crate) fn settled(&self, window: WindowId) -> bool {
        self.running
            .iter()
            .all(|(layer, serials)| serials.is_empty() || self.owners.get(layer) != Some(&window))
    }

    /// Follows a layer event of one of the layers; false for any other event. With it the window of the layer may come
    /// to rest: it is then given by [`SurfaceLayers::take_settled`].
    pub(crate) fn heard(&mut self, event: &AppEvent) -> bool {
        let (layer, serial) = match *event {
            AppEvent::LayerFinished { layer, serial } => (layer, Some(serial)),
            AppEvent::LayerReplaced { layer, serial, .. } => (layer, Some(serial)),
            AppEvent::LayerSettled { layer } => (layer, None),
            _ => return false,
        };
        let window = self.owners.get(&layer).copied();
        let was = window.is_none_or(|w| self.settled(w));
        let Some(running) = self.running.get_mut(&layer) else {
            return false;
        };
        match serial {
            Some(serial) => {
                running.remove(&serial);
                if let Some(moving) = self.moving.get_mut(&layer) {
                    moving.remove(&serial);
                }
            }
            None => {
                running.clear();
                self.moving.remove(&layer);
            }
        }
        if let Some(window) = window.filter(|&w| !was && self.settled(w)) {
            self.came_to_rest.push(window);
        }
        true
    }

    /// The windows whose layers came to rest since the last call: their last running animation ended.
    pub(crate) fn take_settled(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.came_to_rest)
    }

    /// The window goes: its layers' surfaces, for the caller to let go of their buffers and destroy after the layers.
    pub(crate) fn forget(&mut self, window: WindowId) -> Vec<SurfaceId> {
        self.pending.retain(|(w, _), _| *w != window);
        let gone = self.windows.remove(&window).unwrap_or_default();
        for s in &gone {
            self.running.remove(&s.layer.id());
            self.moving.remove(&s.layer.id());
        }
        self.owners.retain(|_, w| *w != window);
        self.came_to_rest.retain(|w| *w != window);
        gone.into_iter().map(|s| s.surface).collect()
    }
}

/// Which parts of `layer`'s place (x, y, width, height) an animation still moves.
fn moving_parts(moving: &HashMap<LayerId, HashMap<u32, usize>>, layer: LayerId) -> [bool; 4] {
    let mut parts = [false; 4];
    for &i in moving.get(&layer).into_iter().flat_map(HashMap::values) {
        parts[i] = true;
    }
    parts
}

fn send(
    layer: &Layer,
    changes: &[LayerChange],
    next_serial: &mut u32,
    running: &mut HashMap<LayerId, HashSet<u32>>,
    moving: &mut HashMap<LayerId, HashMap<u32, usize>>,
) -> Result<(), PlatformError> {
    for change in changes {
        let property = wire(change.property);
        match (change.curve, &change.samples) {
            (None, _) => layer.set(property, change.to),
            (Some(curve), samples) => {
                *next_serial = next_serial.wrapping_add(1);
                running.entry(layer.id()).or_default().insert(*next_serial);
                if let Some(i) = place_index(change.property) {
                    moving
                        .entry(layer.id())
                        .or_default()
                        .insert(*next_serial, i);
                }
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

    /// A window comes to rest once, when the last animation of its last running layer ends, finished or settled;
    /// another window's layers, an event of a layer it never ran and a window gone tell nothing.
    #[test]
    fn a_window_comes_to_rest_once_with_its_last_running_layer() {
        let mut layers = SurfaceLayers::default();
        let (window, other) = (WindowId(1), WindowId(2));
        let (glass, content, elsewhere) = (LayerId(10), LayerId(11), LayerId(12));
        for (layer, w, serials) in [
            (glass, window, vec![1, 2]),
            (content, window, vec![3]),
            (elsewhere, other, vec![4]),
        ] {
            layers.owners.insert(layer, w);
            layers.running.insert(layer, serials.into_iter().collect());
        }
        let finished = |layer, serial| AppEvent::LayerFinished { layer, serial };
        assert!(layers.heard(&finished(glass, 1)));
        assert!(layers.heard(&AppEvent::LayerSettled { layer: glass }));
        assert!(layers.take_settled().is_empty(), "the content still runs");
        assert!(!layers.settled(window));
        assert!(layers.heard(&finished(content, 3)));
        assert_eq!(layers.take_settled(), vec![window]);
        assert!(layers.settled(window) && !layers.settled(other));
        assert!(layers.heard(&AppEvent::LayerSettled { layer: content }));
        assert!(layers.take_settled().is_empty(), "told once");
        assert!(
            !layers.heard(&finished(LayerId(99), 5)),
            "a layer that never ran"
        );
        assert!(layers.heard(&finished(elsewhere, 4)));
        layers.forget(other);
        assert!(layers.take_settled().is_empty(), "a window gone");
    }

    /// A layer declared again while an animation moves its place keeps the animation: the parts it moves are not
    /// sent, as a value sent would replace it, the others are, and a part declared where the compositor has it (to
    /// the wire's 256th of a point) is not sent again. Once the animation ended, or was replaced, the first frame
    /// after sends what the layer is declared at.
    #[test]
    fn a_layer_declared_again_keeps_the_animation_that_moves_its_place() {
        let mut layers = SurfaceLayers::default();
        let glass = LayerId(10);
        let mut told = [None; 4];
        // made: every part is sent
        let still = moving_parts(&layers.moving, glass);
        assert_eq!(
            place_to_send(&mut told, still, [164.0, 44.0, 512.0, 57.0]),
            [Some(164.0), Some(44.0), Some(512.0), Some(57.0)]
        );
        // the same place, a rounding error apart: nothing
        assert_eq!(
            place_to_send(&mut told, still, [163.99999999, 44.0, 512.0, 57.0]),
            [None; 4]
        );
        // the application animates x, width and height; the frame that follows declares the layer where the
        // animation starts
        layers
            .moving
            .insert(glass, HashMap::from([(1, 0), (2, 2), (3, 3)]));
        let moving = moving_parts(&layers.moving, glass);
        assert_eq!(moving, [true, false, true, true]);
        assert_eq!(
            place_to_send(&mut told, moving, [163.99999999, 48.0, 640.0, 57.0]),
            [None, Some(48.0), None, None],
            "only the part no animation moves"
        );
        // one ends: its part is sent with the next frame, though it is declared where it was before the animation
        assert!(layers
            .running
            .insert(glass, HashSet::from([1, 2, 3]))
            .is_none());
        assert!(layers.heard(&AppEvent::LayerFinished {
            layer: glass,
            serial: 1
        }));
        let moving = moving_parts(&layers.moving, glass);
        assert_eq!(moving, [false, false, true, true]);
        assert_eq!(
            place_to_send(&mut told, moving, [164.0, 48.0, 640.0, 57.0]),
            [Some(164.0), None, None, None]
        );
        // the others are replaced or come to rest: the place the layer is declared at now is sent
        assert!(layers.heard(&AppEvent::LayerSettled { layer: glass }));
        let moving = moving_parts(&layers.moving, glass);
        assert_eq!(moving, [false; 4]);
        assert_eq!(
            place_to_send(&mut told, moving, [62.0, 48.0, 844.0, 577.0]),
            [Some(62.0), None, Some(844.0), Some(577.0)]
        );
        assert_eq!(
            place_to_send(&mut told, moving, [62.0, 48.0, 844.0, 577.0]),
            [None; 4],
            "sent once"
        );
    }

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
