//! Windows' surface layers ([`Submission::surface_layers`]): each a `wl_surface` made a `cupertino_layer_v1` of the
//! window's surface, its frame drawn into buffers of its own with the window's, its rect and look sent as the layer's
//! values, and the application's changes ([`WindowRequest::Layer`]) sent as the layer's requests, so they apply with
//! the window's next commit. Layers need version 4 of the global (content blur): below it a window has none and the
//! runtime paints marked subtrees in place. A layer's look carries its glass's lens and blur ramp at every version, and
//! from version 5 the edge along its glass.
//!
//! [`Submission::surface_layers`]: cupertino_ui_core::platform::Submission::surface_layers
//! [`WindowRequest::Layer`]: cupertino_ui_core::platform::WindowRequest

use std::collections::{HashMap, HashSet};

use cupertino_geometry::Rect;
use cupertino_ui_core::platform::{
    surface_layer_size, BlurRamp, LayerChange, LayerProperty, PlatformError, SurfaceLayerLook,
    WindowId, MAX_LAYER_LENS,
};
use cupertino_wayland_client::app::{AppEvent, Layer, LayerId, LayerProperty as Wire, Layers};
use cupertino_wayland_client::{Connection, ExternalRole, SurfaceId};

// a change through samples the toolkit takes is one the wire carries
const _: () = assert!(
    cupertino_wayland_client::app::MAX_SAMPLES == cupertino_ui_core::platform::MAX_LAYER_SAMPLES
);

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

/// A look's lens as the protocol takes it: within its limits, 1 for one that is not a number.
fn on_wire_lens(lens: f64) -> f64 {
    if lens.is_nan() {
        return 1.0;
    }
    lens.clamp(1.0, MAX_LAYER_LENS)
}

/// A blur ramp's weight as the protocol takes it: from 0 to 1, 1 for one that is not a number.
fn on_wire_weight(weight: f64) -> f64 {
    if weight.is_nan() {
        return 1.0;
    }
    weight.clamp(0.0, 1.0)
}

/// A tint channel as the protocol takes it: from 0 to 1, 0 for one that is not a number.
fn on_wire_tint(channel: f64) -> f64 {
    if channel.is_nan() {
        return 0.0;
    }
    channel.clamp(0.0, 1.0)
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
    /// The pixel size of the frame shown last.
    pub(crate) shown: Option<(u32, u32)>,
}

impl Shown {
    /// The layer's size in whole points, as its buffers are drawn at.
    pub(crate) fn size(&self) -> (u32, u32) {
        surface_layer_size(self.rect)
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
    /// Makes `window`'s layers those of `wanted` (each key with its rect and look), in their order: a new key makes a
    /// surface and a layer of `parent`,
    /// a key no longer wanted goes (its surface returned, for the caller to let go of its buffers before destroying
    /// it), and a rect or look that changed is sent.
    pub(crate) fn sync(
        &mut self,
        conn: &mut Connection,
        layers: &Layers,
        window: WindowId,
        parent: SurfaceId,
        wanted: &[(u64, Rect, SurfaceLayerLook)],
    ) -> Result<Vec<SurfaceId>, PlatformError> {
        let was = self.settled(window);
        let shown = self.windows.entry(window).or_default();
        let mut gone = Vec::new();
        shown.retain(|s| {
            let keep = wanted.iter().any(|w| w.0 == s.key);
            if !keep {
                gone.push(s.surface);
                // what it ran goes with it, so it keeps its window from coming to rest no longer
                self.running.remove(&s.layer.id());
                self.moving.remove(&s.layer.id());
                self.owners.remove(&s.layer.id());
            }
            keep
        });
        for &(key, rect, look) in wanted {
            let i = match shown.iter().position(|s| s.key == key) {
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
                        key,
                        surface,
                        layer,
                        rect: Rect::ZERO,
                        told: [None; 4],
                        look: SurfaceLayerLook {
                            radius: -1.0,
                            ..SurfaceLayerLook::default()
                        },
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
                rect.x(),
                rect.y(),
                rect.width().max(0.0),
                rect.height().max(0.0),
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
            s.rect = rect;
            if s.look != look {
                if look.radius != s.look.radius {
                    s.layer
                        .set(Wire::CornerRadius, look.radius.max(0.0))
                        .map_err(protocol_error)?;
                }
                if look.tint != s.look.tint {
                    s.layer
                        .set_glass_tint(look.tint.map(|c| on_wire_tint(f64::from(c))))
                        .map_err(protocol_error)?;
                }
                if look.glass != s.look.glass
                    || look.lens != s.look.lens
                    || look.variant != s.look.variant
                {
                    // a compositor without variants draws the style
                    let variant = look
                        .variant
                        .filter(|_| look.glass.is_some() && layers.takes_glass_variants());
                    match variant {
                        Some(v) => s.layer.set_glass_variant(
                            Some(crate::glass_wire::protocol_variant(v)),
                            on_wire_lens(look.lens),
                        ),
                        None => s.layer.set_glass(
                            look.glass.map(crate::window_system::protocol_style),
                            on_wire_lens(look.lens),
                        ),
                    }
                    .map_err(protocol_error)?;
                }
                if look.blur_ramp != s.look.blur_ramp {
                    let BlurRamp { top, bottom } = look.blur_ramp;
                    s.layer
                        .set_blur_ramp(on_wire_weight(top), on_wire_weight(bottom))
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
            if let Some(changes) = self.pending.remove(&(window, key)) {
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
        match (change.curve.map(|c| c.layer_curve()), &change.samples) {
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
                    (
                        cupertino_ui_core::platform::LayerCurve::Timed { duration, .. },
                        Some(values),
                    ) => layer.animate_samples(*next_serial, property, duration, values),
                    _ => layer.animate(*next_serial, property, change.to, curve, change.velocity),
                }
            }
        }
        .map_err(protocol_error)?;
    }
    Ok(())
}
