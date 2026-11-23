//! The requests of the `cupertino_layers_v1` global and of its layer objects.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use cupertino_animation::{CubicBezier, LayerCurve};
use cupertino_app_protocols::layers::{
    LENS, MAX_DURATION_MS, MAX_GLASS_EDGE, MAX_GROUP_SPACING, MAX_SAMPLES, MIN_SAMPLES,
    SPRING_DAMPING, SPRING_RESPONSE,
};
use cupertino_protocols::server::layers::cupertino_layer_v1::{
    self, Blend, CupertinoLayerV1, Elevation, Error, Highlight, Property,
};
use cupertino_protocols::server::layers::cupertino_layers_v1::{
    self, CupertinoLayersV1, Error as LayersError,
};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor::{get_role, give_role, with_states};

use super::ops::Op;
use super::{
    held, in_range, presented, property_index, with_parent, BlendRole, GlassEdge, LayerData,
    LayerOps, LayerSurfaceData, VibrantWeight, BEZIER_Y, LAYER_ROLE, MAX_ANIMATIONS, MAX_EXTENT,
    MAX_PER_CLIENT, MAX_PER_GROUP, MAX_PER_PARENT, NEXT_SERIAL, NO_GLASS, POPUP_VERSION,
    SOFT_EDGE_VERSION, VARIANT_VERSION,
};
use crate::glass_look::GlassLook;
use crate::state::Cupertino;

/// In a layer shell surface's data map: whether it holds a presentation that is not destroyed.
#[derive(Default)]
struct HasPresentation(Mutex<bool>);

/// In a layer surface's data map: whether its layer object is not destroyed, so it may not become a layer again yet.
#[derive(Default)]
struct HasLayer(Mutex<bool>);

impl GlobalDispatch<CupertinoLayersV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoLayersV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

fn owned_by(surface: &WlSurface, client: &Client) -> bool {
    surface.is_alive() && surface.client().is_some_and(|c| c.id() == client.id())
}

impl Dispatch<CupertinoLayersV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        global: &CupertinoLayersV1,
        request: cupertino_layers_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_layers_v1::Request::GetLayer {
                id,
                surface,
                parent,
            } => {
                let inert = LayerData {
                    parent: None,
                    surface: None,
                    serial: 0,
                    client: client.id(),
                    whole: false,
                };
                if !owned_by(&surface, client) || !owned_by(&parent, client) {
                    global.post_error(
                        LayersError::InvalidSurface,
                        "the surface or the parent belongs to another client or is gone",
                    );
                    init.init(id, inert);
                    return;
                }
                // the parent is a window's, a popup's or a layer shell surface's main surface: those are drawn
                // with their layers, and none of them is a layer, a subsurface or without a role
                let parent_role = get_role(&parent);
                let parent_drawn = [
                    smithay::wayland::shell::xdg::XDG_TOPLEVEL_ROLE,
                    smithay::wayland::shell::xdg::XDG_POPUP_ROLE,
                    smithay::wayland::shell::wlr_layer::LAYER_SURFACE_ROLE,
                ]
                .into_iter()
                .any(|r| parent_role == Some(r));
                let live = |s: &WlSurface| {
                    with_states(s, |states| {
                        states
                            .data_map
                            .get::<HasLayer>()
                            .is_some_and(|h| *h.0.lock().unwrap())
                    })
                };
                // a surface that already is a layer of this parent may become one again, once its layer object was
                // destroyed; it is never the layer of another parent
                let surface_free = match get_role(&surface) {
                    None => true,
                    Some(LAYER_ROLE) => {
                        Cupertino::layer_parent(&surface).as_ref() == Some(&parent)
                            && !live(&surface)
                    }
                    Some(_) => false,
                };
                if surface == parent
                    || !parent_drawn
                    || !surface_free
                    || give_role(&surface, LAYER_ROLE).is_err()
                {
                    global.post_error(
                        LayersError::Role,
                        "the surface has a role or a layer object, is the parent, or the parent is not a \
                         window's, a popup's or a layer shell surface's main surface",
                    );
                    init.init(id, inert);
                    return;
                }
                let on_parent = with_parent(&parent, |l| l.len())
                    + with_states(&parent, |s| {
                        s.cached_state
                            .get::<LayerOps>()
                            .pending()
                            .ops
                            .iter()
                            .filter(|(_, o)| matches!(o, Op::Create { .. }))
                            .count()
                    });
                let held = state.layers.held.entry(client.id()).or_default();
                if on_parent >= MAX_PER_PARENT || *held >= MAX_PER_CLIENT {
                    global.post_error(
                        LayersError::TooManyLayers,
                        format!(
                            "a parent holds at most {MAX_PER_PARENT} layers and a client {MAX_PER_CLIENT}"
                        ),
                    );
                    init.init(id, inert);
                    return;
                }
                *held += 1;
                with_states(&surface, |s| {
                    s.data_map
                        .insert_if_missing_threadsafe(|| LayerSurfaceData {
                            parent: parent.clone(),
                        });
                    *s.data_map
                        .get_or_insert_threadsafe(HasLayer::default)
                        .0
                        .lock()
                        .unwrap() = true;
                });
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                let data = LayerData {
                    parent: Some(parent.clone()),
                    surface: Some(surface.clone()),
                    serial,
                    client: client.id(),
                    whole: false,
                };
                let resource = init.init(id, data);
                with_states(&parent, |s| {
                    s.cached_state.get::<LayerOps>().pending().push(
                        serial,
                        Op::Create {
                            surface: surface.clone(),
                            resource,
                            whole: false,
                        },
                    )
                });
                if !state.layers.parents.contains(&parent) {
                    state.layers.parents.push(parent);
                }
            }
            cupertino_layers_v1::Request::GetPresentation { id, surface } => {
                let inert = LayerData {
                    parent: None,
                    surface: None,
                    serial: 0,
                    client: client.id(),
                    whole: true,
                };
                if !owned_by(&surface, client) {
                    global.post_error(
                        LayersError::InvalidSurface,
                        "the surface belongs to another client or is gone",
                    );
                    init.init(id, inert);
                    return;
                }
                let role = get_role(&surface);
                // a layer shell surface, and from version 3 a popup
                let layer_shell = role
                    == Some(smithay::wayland::shell::wlr_layer::LAYER_SURFACE_ROLE)
                    || (global.version() >= POPUP_VERSION
                        && role == Some(smithay::wayland::shell::xdg::XDG_POPUP_ROLE));
                let taken = with_states(&surface, |s| {
                    let held = s
                        .data_map
                        .get_or_insert_threadsafe(HasPresentation::default);
                    let mut held = held.0.lock().unwrap();
                    let taken = *held;
                    if layer_shell {
                        *held = true;
                    }
                    taken
                });
                if !layer_shell || taken {
                    global.post_error(
                        LayersError::Role,
                        "the surface is not a layer shell surface or a popup, or already has a presentation",
                    );
                    init.init(id, inert);
                    return;
                }
                *state.layers.held.entry(client.id()).or_default() += 1;
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                let data = LayerData {
                    parent: Some(surface.clone()),
                    surface: Some(surface.clone()),
                    serial,
                    client: client.id(),
                    whole: true,
                };
                let resource = init.init(id, data);
                with_states(&surface, |s| {
                    s.cached_state.get::<LayerOps>().pending().push(
                        serial,
                        Op::Create {
                            surface: surface.clone(),
                            resource,
                            whole: true,
                        },
                    )
                });
                if !state.layers.parents.contains(&surface) {
                    state.layers.parents.push(surface);
                }
            }
            cupertino_layers_v1::Request::Destroy => {}
            _ => unreachable!("cupertino_layers_v1 version 6 has no other request"),
        }
    }
}

/// The curve of `animate_timed`, or the reason it is refused; y is held in [`BEZIER_Y`].
fn timed(duration: u32, x1: f64, y1: f64, x2: f64, y2: f64) -> Option<LayerCurve> {
    let unit = 0.0..=1.0;
    let y = |v: f64| v.clamp(BEZIER_Y.0, BEZIER_Y.1);
    (duration <= MAX_DURATION_MS && unit.contains(&x1) && unit.contains(&x2)).then(|| {
        LayerCurve::Timed {
            duration: f64::from(duration) / 1000.0,
            bezier: CubicBezier::new(x1, y(y1), x2, y(y2)),
        }
    })
}

/// The samples of `animate_samples`, floats in the host's byte order, or `None` when the request is refused for its
/// duration or its array.
fn sampled(duration: u32, bytes: &[u8]) -> Option<Arc<[f32]>> {
    let (chunks, rest) = bytes.as_chunks::<4>();
    if duration == 0
        || duration > MAX_DURATION_MS
        || !rest.is_empty()
        || !(MIN_SAMPLES..=MAX_SAMPLES).contains(&chunks.len())
    {
        return None;
    }
    let values: Vec<f32> = chunks.iter().map(|&b| f32::from_ne_bytes(b)).collect();
    values.iter().all(|v| v.is_finite()).then(|| values.into())
}

/// The curve of `animate_spring`, or the reason it is refused.
fn spring(response: f64, damping: f64) -> Option<LayerCurve> {
    ((SPRING_RESPONSE.0..=SPRING_RESPONSE.1).contains(&response)
        && (SPRING_DAMPING.0..=SPRING_DAMPING.1).contains(&damping))
    .then_some(LayerCurve::Spring { response, damping })
}

impl Dispatch<CupertinoLayerV1, LayerData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        layer: &CupertinoLayerV1,
        request: cupertino_layer_v1::Request,
        data: &LayerData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use cupertino_layer_v1::Request as R;
        let bad_property = |what: &str| {
            layer.post_error(Error::InvalidProperty, format!("unknown {what}"));
        };
        let bad_value = |what: &str| layer.post_error(Error::InvalidValue, what.to_string());
        let animation = |state: &mut Cupertino,
                         serial: u32,
                         property: WEnum<Property>,
                         to: f64,
                         curve: Option<LayerCurve>,
                         velocity: f64| {
            let Some(prop) =
                property_index(property, layer.version()).filter(|&p| !data.whole || presented(p))
            else {
                return bad_property("property");
            };
            if !in_range(prop, to) {
                return bad_value("the target is outside the property's range");
            }
            let Some(curve) = curve else {
                return layer.post_error(
                    Error::InvalidCurve,
                    "a duration over 10 s, Bézier x outside 0 to 1, or a spring outside its ranges",
                );
            };
            if state.layers.animations(&data.client) >= MAX_ANIMATIONS {
                return layer.post_error(
                    Error::TooManyAnimations,
                    format!("a client has at most {MAX_ANIMATIONS} animations"),
                );
            }
            let queued = state.layers.queue_animation(&data.client, layer);
            data.queue(Op::Animate {
                serial,
                prop,
                to: held(prop, to),
                curve,
                velocity: velocity.clamp(-MAX_EXTENT, MAX_EXTENT),
                samples: None,
                _queued: queued,
            });
        };
        match request {
            R::Destroy => {}
            R::SetValue { property, value } => {
                match property_index(property, layer.version())
                    .filter(|&p| !data.whole || presented(p))
                {
                    None => bad_property("property"),
                    Some(prop) if !in_range(prop, value) => {
                        bad_value("the value is outside the property's range")
                    }
                    Some(prop) => data.queue(Op::Set(prop, held(prop, value))),
                }
            }
            R::AnimateTimed {
                serial,
                property,
                value,
                duration,
                x1,
                y1,
                x2,
                y2,
            } => animation(
                state,
                serial,
                property,
                value,
                timed(duration, x1, y1, x2, y2),
                0.0,
            ),
            R::AnimateSpring {
                serial,
                property,
                value,
                response,
                damping,
                velocity,
            } => animation(
                state,
                serial,
                property,
                value,
                spring(response, damping),
                velocity,
            ),
            R::AnimateSamples {
                serial,
                property,
                duration,
                samples,
            } => {
                let Some(prop) = property_index(property, layer.version())
                    .filter(|&p| !data.whole || presented(p))
                else {
                    return bad_property("property");
                };
                let Some(values) = sampled(duration, &samples) else {
                    return layer.post_error(
                        Error::InvalidCurve,
                        "a duration outside 1 to 10000 ms, or not 2 to 1000 samples of 4 bytes",
                    );
                };
                if !values.iter().all(|&v| in_range(prop, f64::from(v))) {
                    return bad_value("a sample is outside the property's range");
                }
                if state.layers.animations(&data.client) >= MAX_ANIMATIONS {
                    return layer.post_error(
                        Error::TooManyAnimations,
                        format!("a client has at most {MAX_ANIMATIONS} animations"),
                    );
                }
                let values: Arc<[f32]> = values
                    .iter()
                    .map(|&v| held(prop, f64::from(v)) as f32)
                    .collect();
                let to = f64::from(*values.last().expect("two samples at least"));
                let queued = state.layers.queue_animation(&data.client, layer);
                data.queue(Op::Animate {
                    serial,
                    prop,
                    to,
                    curve: LayerCurve::Timed {
                        duration: f64::from(duration) / 1000.0,
                        bezier: CubicBezier::new(0.0, 0.0, 1.0, 1.0),
                    },
                    velocity: 0.0,
                    samples: Some(values),
                    _queued: queued,
                });
            }
            R::Cancel { property } => {
                match property_index(property, layer.version())
                    .filter(|&p| !data.whole || presented(p))
                {
                    Some(prop) => data.queue(Op::Cancel(prop)),
                    None => bad_property("property"),
                }
            }
            _ if data.whole => bad_property("request on a presentation"),
            R::SetGlass { style, lens } => {
                if style == NO_GLASS {
                    data.queue(Op::Glass(None));
                    return;
                }
                let Some((_, style)) = crate::app_effects_v1::GLASS_MATERIALS.get(style as usize)
                else {
                    return bad_property("glass style");
                };
                if !(LENS.0..=LENS.1).contains(&lens) {
                    return bad_value("a lens below 1 or above 4");
                }
                data.queue(Op::Glass(Some((GlassLook::of_style(*style), lens))));
            }
            R::SetGlassVariant {
                variant,
                modifiers,
                size_class,
                lens,
            } if layer.version() >= VARIANT_VERSION => {
                let Some(look) = crate::glass_look::of_wire(variant, modifiers, size_class) else {
                    return bad_property("glass variant, modifier or size class");
                };
                if !(LENS.0..=LENS.1).contains(&lens) {
                    return bad_value("a lens below 1 or above 4");
                }
                data.queue(Op::Glass(Some((look, lens))));
            }
            R::SetGlassTint {
                red,
                green,
                blue,
                alpha,
            } if layer.version() >= VARIANT_VERSION => {
                let Some(tint) = crate::glass_look::tint([red, green, blue, alpha]) else {
                    return bad_value("a glass tint channel outside 0 to 1");
                };
                data.queue(Op::Tint(tint));
            }
            R::SetBlend { blend } => {
                let role = match blend {
                    WEnum::Value(Blend::Normal) => BlendRole::Normal,
                    WEnum::Value(Blend::Vibrant) => BlendRole::Vibrant(VibrantWeight::Primary),
                    WEnum::Value(Blend::VibrantSecondary) => {
                        BlendRole::Vibrant(VibrantWeight::Secondary)
                    }
                    WEnum::Value(Blend::VibrantFill) => BlendRole::Vibrant(VibrantWeight::Fill),
                    _ => return bad_property("blend role"),
                };
                data.queue(Op::Blend(role));
            }
            R::SetElevation { elevation } => match elevation {
                WEnum::Value(Elevation::Flat) => data.queue(Op::Raised(false)),
                WEnum::Value(Elevation::Raised) => data.queue(Op::Raised(true)),
                _ => bad_property("elevation"),
            },
            R::SetHighlight { highlight } => match highlight {
                WEnum::Value(Highlight::Fixed) => data.queue(Op::PointerLit(false)),
                WEnum::Value(Highlight::Pointer) => data.queue(Op::PointerLit(true)),
                _ => bad_property("highlight"),
            },
            R::SetBlurRamp { top, bottom } => {
                if !(0.0..=1.0).contains(&top) || !(0.0..=1.0).contains(&bottom) {
                    return bad_value("a blur weight outside 0 to 1");
                }
                data.queue(Op::BlurRamp([top as f32, bottom as f32]));
            }
            R::SetMasksToBounds { masks } => data.queue(Op::Masks(masks != 0)),
            R::SetPlaceholder {
                red,
                green,
                blue,
                alpha,
            } => {
                let c = [red, green, blue, alpha];
                if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return bad_value("a placeholder channel outside 0 to 1");
                }
                data.queue(Op::Placeholder(c.map(|v| v as f32)));
            }
            R::SetGlassGroup { group, spacing } => {
                if !(0.0..=MAX_GROUP_SPACING).contains(&spacing) {
                    return bad_value("a glass group spacing outside 0 to 64");
                }
                if group != 0 && data.in_group(group) >= MAX_PER_GROUP {
                    return layer.post_error(
                        Error::GroupFull,
                        format!("a glass group holds at most {MAX_PER_GROUP} layers of a parent"),
                    );
                }
                data.queue(Op::Group(group, spacing));
            }
            R::SetGlassEdge {
                width,
                red,
                green,
                blue,
                alpha,
            } => {
                if !(0.0..=MAX_GLASS_EDGE).contains(&width) {
                    return bad_value("a glass edge wider than 4 or below 0");
                }
                let c = [red, green, blue, alpha];
                if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return bad_value("a glass edge channel outside 0 to 1");
                }
                data.queue(Op::Edge(GlassEdge {
                    width,
                    color: c.map(|v| v as f32),
                    soft: layer.version() >= SOFT_EDGE_VERSION,
                }));
            }
            _ => unreachable!("cupertino_layer_v1 version 7 has no other request"),
        }
    }

    fn destroyed(state: &mut Self, client: ClientId, _: &CupertinoLayerV1, data: &LayerData) {
        if let Some(surface) = data
            .surface
            .as_ref()
            .filter(|s| !data.whole && s.is_alive())
        {
            with_states(surface, |s| {
                if let Some(live) = s.data_map.get::<HasLayer>() {
                    *live.0.lock().unwrap() = false;
                }
            });
        }
        if let Some(surface) = data.parent.as_ref().filter(|s| data.whole && s.is_alive()) {
            with_states(surface, |s| {
                if let Some(held) = s.data_map.get::<HasPresentation>() {
                    *held.0.lock().unwrap() = false;
                }
            });
        }
        if data.parent.is_some() {
            if let Some(n) = state.layers.held.get_mut(&client) {
                *n = n.saturating_sub(1);
            }
            data.queue(Op::Destroy);
        }
    }
}

/// Forget the layers of a destroyed layer surface at once, and a client's count with its last layer.
pub fn surface_destroyed(state: &mut Cupertino, surface: &WlSurface) {
    if let Some(parent) = Cupertino::layer_parent(surface).filter(|p| p.is_alive()) {
        with_parent(&parent, |layers| layers.retain(|l| l.surface != *surface));
    }
    state.layers.held.retain(|_, n| *n > 0);
    state.layers.queued.retain(|_, n| Arc::strong_count(n) > 1);
}
