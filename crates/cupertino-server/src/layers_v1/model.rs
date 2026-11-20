//! A parent's committed layers ([`LayerModel`], in [`ParentLayers`]): their animated properties and their look.

use std::sync::{Arc, Mutex};

use cupertino_animation::LayerCurve;
use cupertino_animation::LayerProperty;
use cupertino_protocols::server::layers::cupertino_layer_v1::CupertinoLayerV1;
use cupertino_theme::GlassStyle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;

use super::ops::Op;
use super::{held, spatial, PROPERTIES};

/// How a layer's content mixes with what lies behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendRole {
    #[default]
    Normal,
    /// Coverage drawn in a colour taken from the glass behind, at a weight (primary, secondary, fill).
    Vibrant(VibrantWeight),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VibrantWeight {
    Primary,
    Secondary,
    Fill,
}

/// The line along the outside of a layer's glass (`set_glass_edge`): its width in logical points, constant whatever
/// the layer's scale, its straight colour and opacity, and whether it softens by the layer's content blur (a layer
/// object of [`SOFT_EDGE_VERSION`](super::SOFT_EDGE_VERSION) or later); a width of 0 draws none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlassEdge {
    pub width: f64,
    pub color: [f32; 4],
    pub soft: bool,
}

/// An animation asked for but not started: it starts when its commit is first shown.
#[derive(Clone, Debug)]
pub(super) struct Waiting {
    serial: u32,
    pub(super) prop: usize,
    pub(super) to: f64,
    curve: LayerCurve,
    velocity: f64,
    samples: Option<Arc<[f32]>>,
    pub(super) since: f64,
}

/// The value property `prop` of a new layer starts at: opacity and scale at 1, the rest at 0.
pub(super) fn initial(prop: usize) -> f64 {
    if prop == 4 || prop == 5 {
        1.0
    } else {
        0.0
    }
}

/// A committed layer of a parent.
pub struct LayerModel {
    pub serial: u64,
    pub surface: WlSurface,
    /// The parent's own presentation (`get_presentation`), not a layer drawn over it.
    pub whole: bool,
    pub(super) resource: CupertinoLayerV1,
    pub(super) props: [LayerProperty; PROPERTIES],
    /// The serial of each property's running animation.
    pub(super) running: [Option<u32>; PROPERTIES],
    pub(super) waiting: Vec<Waiting>,
    /// Width and height follow the content's size until they are set.
    sized: [bool; 2],
    /// Whether the layer had an animation running or waiting at the last tick, for `settled`.
    pub(super) busy: bool,
    pub glass: Option<(GlassStyle, f64)>,
    pub blend: BlendRole,
    pub raised: bool,
    pub pointer_lit: bool,
    pub blur_ramp: [f32; 2],
    pub masks: bool,
    pub placeholder: [f32; 4],
    /// The glass group (0 for none) and how far its shapes blend into each other, in points.
    pub group: u32,
    pub spacing: f64,
    /// The line along the outside of its glass.
    pub edge: GlassEdge,
}

impl LayerModel {
    pub(super) fn new(
        serial: u64,
        surface: WlSurface,
        resource: CupertinoLayerV1,
        whole: bool,
    ) -> Self {
        let props = std::array::from_fn(|i| LayerProperty::new(initial(i)));
        Self {
            serial,
            surface,
            whole,
            resource,
            props,
            running: [None; PROPERTIES],
            waiting: Vec::new(),
            sized: [false; 2],
            busy: false,
            glass: None,
            blend: BlendRole::Normal,
            raised: false,
            pointer_lit: false,
            blur_ramp: [1.0, 1.0],
            masks: false,
            placeholder: [0.0; 4],
            group: 0,
            spacing: 0.0,
            edge: GlassEdge::default(),
        }
    }

    /// The content's size in logical points, (0, 0) before its first buffer.
    pub fn content_size(&self) -> (f64, f64) {
        use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
        with_states(&self.surface, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|d| d.lock().unwrap().view())
                .map_or((0.0, 0.0), |v| (f64::from(v.dst.w), f64::from(v.dst.h)))
        })
    }

    /// Width and height take the content's size before the first value or animation of either.
    fn fix_size(&mut self, prop: usize) {
        if prop == 2 || prop == 3 {
            let i = prop - 2;
            if !self.sized[i] {
                let (w, h) = self.content_size();
                self.props[prop].set(if i == 0 { w } else { h });
                self.sized[i] = true;
            }
        }
    }

    /// End the running animation of `prop` at `now`: `replaced` with what was shown.
    fn replace(&mut self, prop: usize, now: f64) {
        if let Some(serial) = self.running[prop].take() {
            if self.props[prop].is_animating(now) {
                let p = &self.props[prop];
                self.resource
                    .replaced(serial, p.value(now), p.velocity(now));
            } else {
                self.resource.finished(serial);
            }
        }
    }

    /// End the animations of `prop` still waiting to start, from commits that applied before, at `now`: `replaced`
    /// with what is shown, where the motion that follows starts.
    fn replace_waiting(&mut self, prop: usize, now: f64) {
        if !self.waiting.iter().any(|w| w.prop == prop) {
            return;
        }
        let (value, velocity) = (self.value(prop, now), self.props[prop].velocity(now));
        for w in std::mem::take(&mut self.waiting) {
            if w.prop == prop {
                self.resource.replaced(w.serial, value, velocity);
            } else {
                self.waiting.push(w);
            }
        }
    }

    /// The value of `prop` shown at `now`, [`held`] in its range: an animation that overshoots does not take the layer
    /// past it.
    pub fn value(&self, prop: usize, now: f64) -> f64 {
        held(
            prop,
            match prop {
                2 | 3 if !self.sized[prop - 2] => {
                    let (w, h) = self.content_size();
                    if prop == 2 {
                        w
                    } else {
                        h
                    }
                }
                _ => self.props[prop].value(now),
            },
        )
    }

    /// The value `prop` rests at once its animation ends: what the client hit-tests against.
    pub fn target(&self, prop: usize) -> f64 {
        held(
            prop,
            self.waiting
                .iter()
                .rev()
                .find(|w| w.prop == prop)
                .map_or_else(|| self.props[prop].target(), |w| w.to),
        )
    }

    /// Whether an animation of the layer runs or waits at `now`.
    pub fn animating(&self, now: f64) -> bool {
        !self.waiting.is_empty() || self.props.iter().any(|p| p.is_animating(now))
    }

    /// Start the waiting animations at `now`; with `reduce_motion` the spatial ones end at their target at once.
    pub(super) fn start(&mut self, now: f64, reduce_motion: bool) {
        for w in std::mem::take(&mut self.waiting) {
            self.fix_size(w.prop);
            self.replace(w.prop, now);
            if reduce_motion && spatial(w.prop) {
                self.props[w.prop].set(w.to);
            } else if let (Some(samples), LayerCurve::Timed { duration, .. }) = (w.samples, w.curve)
            {
                self.props[w.prop].animate_samples(samples, duration, now);
            } else {
                self.props[w.prop].animate(w.to, w.curve, now, w.velocity);
            }
            self.running[w.prop] = Some(w.serial);
        }
    }

    pub(super) fn apply(&mut self, op: Op, now: f64) {
        match op {
            Op::Create { .. } | Op::Destroy => {}
            Op::Set(prop, v) => {
                self.fix_size(prop);
                self.replace(prop, now);
                self.replace_waiting(prop, now);
                self.props[prop].set(v);
            }
            Op::Animate {
                serial,
                prop,
                to,
                curve,
                velocity,
                samples,
                ..
            } => self.waiting.push(Waiting {
                serial,
                prop,
                to,
                curve,
                velocity,
                samples,
                since: now,
            }),
            Op::Cancel(prop) => {
                let shown = self.props[prop].value(now);
                self.replace(prop, now);
                self.replace_waiting(prop, now);
                self.props[prop].set(shown);
            }
            Op::Glass(g) => self.glass = g,
            Op::Blend(b) => self.blend = b,
            Op::Raised(r) => self.raised = r,
            Op::PointerLit(l) => self.pointer_lit = l,
            Op::BlurRamp(r) => self.blur_ramp = r,
            Op::Masks(m) => self.masks = m,
            Op::Placeholder(c) => self.placeholder = c,
            Op::Group(group, spacing) => (self.group, self.spacing) = (group, spacing),
            Op::Edge(edge) => self.edge = edge,
        }
    }
}

/// The committed layers of a parent surface in creation order, in its data map.
#[derive(Default)]
pub struct ParentLayers(pub Mutex<Vec<LayerModel>>);
