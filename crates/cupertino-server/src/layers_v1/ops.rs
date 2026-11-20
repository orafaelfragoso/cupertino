//! The layer requests queued in a parent's cached state ([`LayerOps`]) until its commit applies, and the count of the
//! animations among them ([`Queued`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cupertino_animation::LayerCurve;
use cupertino_protocols::server::layers::cupertino_layer_v1::CupertinoLayerV1;
use cupertino_theme::GlassStyle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::Cacheable;

use super::{BlendRole, GlassEdge};

/// One queued request of a layer, applied with the parent's commit.
pub(super) enum Op {
    Create {
        surface: WlSurface,
        resource: CupertinoLayerV1,
        whole: bool,
    },
    Destroy,
    Set(usize, f64),
    Animate {
        serial: u32,
        prop: usize,
        to: f64,
        curve: LayerCurve,
        velocity: f64,
        /// An animation through samples (`animate_samples`): the values at even steps of `curve`'s duration, the
        /// last of them `to`.
        samples: Option<Arc<[f32]>>,
        /// Counts it against its client's limit while it is queued.
        _queued: Queued,
    },
    Cancel(usize),
    Glass(Option<(GlassStyle, f64)>),
    Blend(BlendRole),
    Raised(bool),
    PointerLit(bool),
    BlurRamp([f32; 2]),
    Masks(bool),
    Placeholder([f32; 4]),
    Group(u32, f64),
    Edge(GlassEdge),
}

/// An animation queued in a parent's state and not yet applied, wherever that state is (pending, or cached by a
/// commit not yet applied): it counts against its client's [`MAX_ANIMATIONS`](super::MAX_ANIMATIONS) until it is applied or dropped with
/// the state. One that a later commit's request of the same layer and property makes moot before it applies
/// ([`LayerOps::follow`]) no longer counts, and is answered `replaced` with the next tick ([`Layers::superseded`](super::Layers::superseded)).
pub(super) struct Queued {
    count: Arc<AtomicUsize>,
    /// The layer object it animates, which hears `replaced` once it is superseded.
    layer: CupertinoLayerV1,
    superseded: Superseded,
}

/// Animations a later request superseded before they applied, to be answered `replaced`: the layer object, the
/// animation's serial and its property.
pub(super) type Superseded = Arc<Mutex<Vec<(CupertinoLayerV1, u32, usize)>>>;

impl Queued {
    pub(super) fn new(
        count: &Arc<AtomicUsize>,
        layer: &CupertinoLayerV1,
        superseded: &Superseded,
    ) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self {
            count: count.clone(),
            layer: layer.clone(),
            superseded: superseded.clone(),
        }
    }

    /// Answer the animation `serial` of property `prop` as replaced with the next tick, and count it no longer.
    fn supersede(self, serial: u32, prop: usize) {
        self.superseded
            .lock()
            .unwrap()
            .push((self.layer.clone(), serial, prop));
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The layer requests queued on a parent surface, by layer serial, in request order.
#[derive(Default)]
pub struct LayerOps {
    pub(super) ops: Vec<(u64, Op)>,
}

impl Op {
    /// The property a value, an animation or a cancel is of.
    fn property(&self) -> Option<usize> {
        match self {
            Op::Set(p, _) | Op::Cancel(p) | Op::Animate { prop: p, .. } => Some(*p),
            _ => None,
        }
    }
}

impl LayerOps {
    /// Queue `op` of the layer `serial`, dropping what it makes moot: an earlier value of the same property or an
    /// earlier look of the same kind, and a cancel that repeats one with no animation of the property between. So
    /// requests repeated without a commit do not pile up.
    pub(super) fn push(&mut self, serial: u64, op: Op) {
        use std::mem::discriminant;
        let of = Op::property;
        match &op {
            Op::Set(prop, _) => self
                .ops
                .retain(|(s, o)| *s != serial || !matches!(o, Op::Set(p, _) if p == prop)),
            Op::Cancel(prop) => {
                let last = self
                    .ops
                    .iter()
                    .rev()
                    .find(|(s, o)| {
                        *s == serial
                            && matches!(o, Op::Cancel(_) | Op::Animate { .. })
                            && of(o) == Some(*prop)
                    })
                    .map(|(_, o)| matches!(o, Op::Cancel(_)));
                if last == Some(true) {
                    return;
                }
            }
            Op::Create { .. } | Op::Destroy | Op::Animate { .. } => {}
            look => {
                let kind = discriminant(look);
                self.ops
                    .retain(|(s, o)| *s != serial || discriminant(o) != kind);
            }
        }
        self.ops.push((serial, op));
    }

    /// Queue `op` of the layer `serial`, a request of a later commit than those queued, dropping what it makes moot
    /// once the requests apply together: what a value replaces (an earlier value, animation or cancel of the
    /// property), an earlier animation of the property that an animation or a cancel replaces, a cancel that follows a
    /// value or a cancel of the property with no animation between, an earlier look of the same kind, and every
    /// request of a layer destroyed, its creation among them. A dropped animation is answered `replaced`
    /// ([`Queued::supersede`]). The requests then apply as they would one commit after another, except that an
    /// animation never shown leaves no motion for the next to take up, and what one parent keeps is bounded by its
    /// layers however many commits merge into it ([`Cacheable::merge_into`]).
    fn follow(&mut self, serial: u64, op: Op) {
        use std::mem::discriminant;
        let ours = |s: u64| s == serial;
        match &op {
            Op::Create { .. } => {}
            Op::Destroy => {
                let created = self
                    .ops
                    .iter()
                    .any(|(s, o)| ours(*s) && matches!(o, Op::Create { .. }));
                self.drop_where(|s, _| ours(s));
                if created {
                    return;
                }
            }
            Op::Set(prop, _) => {
                let prop = Some(*prop);
                self.drop_where(|s, o| ours(s) && o.property() == prop);
            }
            Op::Animate { prop, .. } => {
                let prop = *prop;
                self.drop_where(|s, o| {
                    ours(s) && matches!(o, Op::Animate { prop: p, .. } if *p == prop)
                });
            }
            Op::Cancel(prop) => {
                let prop = *prop;
                self.drop_where(|s, o| {
                    ours(s) && matches!(o, Op::Animate { prop: p, .. } if *p == prop)
                });
                // what is left of the property is a value or a cancel, which this one would repeat
                if self
                    .ops
                    .iter()
                    .any(|(s, o)| ours(*s) && o.property() == Some(prop))
                {
                    return;
                }
            }
            look => {
                let kind = discriminant(look);
                self.drop_where(|s, o| ours(s) && discriminant(o) == kind);
            }
        }
        self.ops.push((serial, op));
    }

    /// Drop the requests `moot` picks, answering an animation among them `replaced`.
    fn drop_where(&mut self, moot: impl Fn(u64, &Op) -> bool) {
        let mut kept = Vec::with_capacity(self.ops.len());
        for (s, op) in std::mem::take(&mut self.ops) {
            if !moot(s, &op) {
                kept.push((s, op));
            } else if let Op::Animate {
                serial,
                prop,
                _queued: queued,
                ..
            } = op
            {
                queued.supersede(serial, prop);
            }
        }
        self.ops = kept;
    }
}

impl Cacheable for LayerOps {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        std::mem::take(self)
    }

    /// A later commit's requests follow these ([`LayerOps::follow`]), as they do when a commit merges into one a
    /// transaction keeps (Smithay's coalescing blocker) or a cached state applies.
    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        for (serial, op) in self.ops {
            into.follow(serial, op);
        }
    }
}
