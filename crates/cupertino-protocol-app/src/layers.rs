//! `cupertino_layers_v1`: surfaces the server places and animates on its own clock, and from version 2 the
//! presentation of a layer shell surface as a whole, so a panel opens and closes without drawing a frame.

use crate::{AppEvent, Error, Quiet};
use cupertino_animation::LayerCurve;
use cupertino_app_protocols::client::layers::cupertino_layer_v1::{
    self, CupertinoLayerV1, Property as WireProperty,
};
use cupertino_app_protocols::client::layers::cupertino_layers_v1::CupertinoLayersV1;
use cupertino_wayland_client::{Connection, State, SurfaceId};
use std::sync::atomic::{AtomicU64, Ordering};
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle};

/// The version that adds [`Layers::presentation`].
const PRESENTATION_VERSION: u32 = 2;

/// Names a [`Layer`] of this process in [`AppEvent::LayerFinished`], [`AppEvent::LayerReplaced`] and
/// [`AppEvent::LayerSettled`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(pub u64);

static NEXT_LAYER: AtomicU64 = AtomicU64::new(1);

/// User data of a layer: the id its events name.
pub(crate) struct LayerData(LayerId);

/// A value as the wire carries it, in 256ths of a unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fixed(pub i32);

impl Fixed {
    pub fn from_f64(v: f64) -> Fixed {
        Fixed((v * 256.0).round() as i32)
    }

    pub fn to_f64(self) -> f64 {
        f64::from(self.0) / 256.0
    }
}

/// An animatable property of a layer or a presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerProperty {
    /// The origin's x, in the parent's points; a presentation's horizontal offset.
    X,
    /// The origin's y; a presentation's vertical offset.
    Y,
    /// The bounds' width (not of a presentation).
    Width,
    /// The bounds' height (not of a presentation).
    Height,
    /// From 0 to 1.
    Opacity,
    /// About the bounds' centre, or the presented surface's.
    Scale,
    /// The bounds' corner radius (not of a presentation).
    CornerRadius,
}

impl LayerProperty {
    fn wire(self) -> WireProperty {
        match self {
            LayerProperty::X => WireProperty::X,
            LayerProperty::Y => WireProperty::Y,
            LayerProperty::Width => WireProperty::Width,
            LayerProperty::Height => WireProperty::Height,
            LayerProperty::Opacity => WireProperty::Opacity,
            LayerProperty::Scale => WireProperty::Scale,
            LayerProperty::CornerRadius => WireProperty::CornerRadius,
        }
    }

    /// Whether a presentation has the property: offset, opacity and scale.
    pub fn presented(self) -> bool {
        matches!(
            self,
            LayerProperty::X | LayerProperty::Y | LayerProperty::Opacity | LayerProperty::Scale
        )
    }

    /// Whether the value lies in the property's range, as the server checks it.
    pub fn accepts(self, value: f64) -> bool {
        match self {
            LayerProperty::X | LayerProperty::Y => value.is_finite(),
            LayerProperty::Opacity => (0.0..=1.0).contains(&value),
            _ => value.is_finite() && value >= 0.0,
        }
    }
}

/// The layers global (`cupertino_layers_v1`).
pub struct Layers(CupertinoLayersV1);

impl Layers {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<Layers> {
        conn.bind_extension(1..=PRESENTATION_VERSION, Quiet)
            .map(Layers)
    }

    /// Whether the server presents layer shell surfaces as a whole ([`Self::presentation`], version 2).
    pub fn presents_surfaces(&self) -> bool {
        self.0.version() >= PRESENTATION_VERSION
    }

    /// Make `surface` a layer of `parent`: it shows with the parent's next commit. `None` when either surface is
    /// gone.
    pub fn layer(&self, conn: &Connection, surface: SurfaceId, parent: SurfaceId) -> Option<Layer> {
        let surface = conn.wl_surface(surface).ok()?;
        let parent = conn.wl_surface(parent).ok()?;
        let data = LayerData(LayerId(NEXT_LAYER.fetch_add(1, Ordering::Relaxed)));
        Some(Layer(self.0.get_layer(
            &surface,
            &parent,
            &conn.queue_handle(),
            data,
        )))
    }

    /// The presentation of `surface`, a layer shell surface's main surface, which offsets, scales and fades all it
    /// draws; `None` below version 2. Its values apply with the surface's own commit.
    pub fn presentation(&self, conn: &Connection, surface: SurfaceId) -> Option<Layer> {
        if !self.presents_surfaces() {
            return None;
        }
        let surface = conn.wl_surface(surface).ok()?;
        let data = LayerData(LayerId(NEXT_LAYER.fetch_add(1, Ordering::Relaxed)));
        Some(Layer(self.0.get_presentation(
            &surface,
            &conn.queue_handle(),
            data,
        )))
    }
}

impl Drop for Layers {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// A layer or a presentation (`cupertino_layer_v1`). What it asks applies with its parent's next commit; dropping it
/// removes the layer (puts a presentation back at rest) with that commit.
pub struct Layer(CupertinoLayerV1);

impl Layer {
    /// The id its events name.
    pub fn id(&self) -> LayerId {
        self.0.data::<LayerData>().map_or(LayerId(0), |d| d.0)
    }

    /// Set `property` to `value` at once; a running animation of it is replaced.
    pub fn set(&self, property: LayerProperty, value: f64) -> Result<(), Error> {
        if !property.accepts(value) {
            return Err(Error::InvalidLayerValue);
        }
        self.0.set_value(property.wire(), value);
        Ok(())
    }

    /// Animate `property` to `value` along `curve` from what is shown when the commit is first drawn; `velocity` (units
    /// per second) starts a spring from rest. `serial` names the animation in the events.
    pub fn animate(
        &self,
        serial: u32,
        property: LayerProperty,
        value: f64,
        curve: LayerCurve,
        velocity: f64,
    ) -> Result<(), Error> {
        if !property.accepts(value) {
            return Err(Error::InvalidLayerValue);
        }
        match curve {
            LayerCurve::Timed { duration, bezier } => {
                self.0.animate_timed(
                    serial,
                    property.wire(),
                    value,
                    (duration.max(0.0) * 1000.0).round() as u32,
                    bezier.x1,
                    bezier.y1,
                    bezier.x2,
                    bezier.y2,
                );
            }
            LayerCurve::Spring { response, damping } => {
                self.0
                    .animate_spring(serial, property.wire(), value, response, damping, velocity)
            }
        }
        Ok(())
    }

    /// Stop `property` where it is shown when the commit applies.
    pub fn cancel(&self, property: LayerProperty) {
        self.0.cancel(property.wire());
    }

    /// Clip the content to the layer's rounded bounds.
    pub fn set_masks_to_bounds(&self, masks: bool) {
        self.0.set_masks_to_bounds(u32::from(masks));
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

impl Dispatch<CupertinoLayersV1, Quiet> for State {
    fn event(
        _: &mut State,
        _: &CupertinoLayersV1,
        _: <CupertinoLayersV1 as Proxy>::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<CupertinoLayerV1, LayerData> for State {
    fn event(
        state: &mut State,
        _: &CupertinoLayerV1,
        event: cupertino_layer_v1::Event,
        data: &LayerData,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        let layer = data.0;
        match event {
            cupertino_layer_v1::Event::Finished { serial } => {
                state.emit(AppEvent::LayerFinished { layer, serial })
            }
            cupertino_layer_v1::Event::Replaced {
                serial,
                value,
                velocity,
            } => state.emit(AppEvent::LayerReplaced {
                layer,
                serial,
                value: Fixed::from_f64(value),
                velocity: Fixed::from_f64(velocity),
            }),
            cupertino_layer_v1::Event::Settled => state.emit(AppEvent::LayerSettled { layer }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_the_protocol_and_a_presentation_has_four_properties() {
        assert!(LayerProperty::X.accepts(-20.5) && !LayerProperty::X.accepts(f64::NAN));
        assert!(LayerProperty::Opacity.accepts(1.0) && !LayerProperty::Opacity.accepts(1.01));
        assert!(!LayerProperty::Scale.accepts(-0.1) && LayerProperty::Scale.accepts(1.1));
        let presented: Vec<_> = [
            LayerProperty::X,
            LayerProperty::Y,
            LayerProperty::Width,
            LayerProperty::Height,
            LayerProperty::Opacity,
            LayerProperty::Scale,
            LayerProperty::CornerRadius,
        ]
        .into_iter()
        .filter(|p| p.presented())
        .collect();
        assert_eq!(
            presented,
            [
                LayerProperty::X,
                LayerProperty::Y,
                LayerProperty::Opacity,
                LayerProperty::Scale
            ]
        );
        assert_eq!(Fixed::from_f64(0.5).to_f64(), 0.5);
        assert_eq!(Fixed::from_f64(-1.25), Fixed(-320));
    }
}
