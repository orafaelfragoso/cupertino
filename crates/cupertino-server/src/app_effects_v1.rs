//! `cupertino_app_effects_v1` (any client → server): the global, the material backdrops of `get_backdrop`, the title bar
//! style of `set_titlebar_style` and the appearance observer of `get_appearance` ([`crate::session_v1`]). The global is
//! public, so every request is about the client's own surfaces: a surface of another client is a protocol error.
//! Backdrop state lives in the surface's cached state, so it applies atomically with the surface's `wl_surface.commit`;
//! the scene reads the committed (`current`) side.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_decorations::TitlebarStyle;
use cupertino_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1, Error as EffectsError,
};
use cupertino_protocols::server::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, Error,
};
use smithay::desktop::{layer_map_for_output, LayerMap, LayerSurface};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::{with_states, Cacheable};

use crate::state::{ClientState, Cupertino};

/// Registers the `cupertino_app_effects_v1` global.
pub struct AppEffectsV1Global {
    _global: GlobalId,
}

impl AppEffectsV1Global {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoAppEffectsV1, ()>(1, ()),
        }
    }
}

/// Most backdrops one surface may hold: the dock and the menus use a few per surface.
const MAX_BACKDROPS_PER_SURFACE: usize = 8;
/// Most backdrops one client may hold, over all its surfaces.
const MAX_BACKDROPS_PER_CLIENT: usize = 128;

/// Source of backdrop serials: identity of a backdrop and its creation order across surfaces.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Region of a backdrop in surface-local logical coordinates.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: f64,
}

#[derive(Clone, Debug)]
struct BackdropEntry {
    serial: u64,
    /// `None` until `set_material`: the backdrop draws nothing.
    material: Option<String>,
    shape: Shape,
    alpha: f64,
}

/// The backdrops of one surface in creation order, double-buffered with the surface.
#[derive(Clone, Debug, Default)]
pub struct BackdropState {
    entries: Vec<BackdropEntry>,
}

impl Cacheable for BackdropState {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// A committed backdrop, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct BackdropArea {
    /// Identity for the element cache.
    pub key: u64,
    pub material: String,
    pub area: Rectangle<f64, Logical>,
    pub radius: f64,
    pub alpha: f64,
}

/// Committed, visible backdrops of `surface` (set material, non-empty shape) in creation order,
/// placed relative to `origin`, the logical position of the surface's local origin.
pub fn surface_backdrops(surface: &WlSurface, origin: Point<f64, Logical>) -> Vec<BackdropArea> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<BackdropState>()
            .current()
            .entries
            .iter()
            .filter_map(|e| {
                let material = e.material.clone()?;
                let s = e.shape;
                (s.width > 0 && s.height > 0).then(|| BackdropArea {
                    key: e.serial,
                    material,
                    area: Rectangle::new(
                        origin + Point::from((f64::from(s.x), f64::from(s.y))),
                        (f64::from(s.width), f64::from(s.height)).into(),
                    ),
                    radius: s.radius,
                    alpha: e.alpha,
                })
            })
            .collect()
    })
}

/// Backdrops of a layer surface, in output-local logical coordinates.
pub fn layer_surface_backdrops(map: &LayerMap, layer: &LayerSurface) -> Vec<BackdropArea> {
    let Some(geo) = map.layer_geometry(layer) else {
        return Vec::new();
    };
    surface_backdrops(layer.wl_surface(), geo.loc.to_f64())
}

/// Backdrops of every layer surface of `output`, in output-local logical coordinates.
pub fn layer_backdrops(output: &Output) -> Vec<BackdropArea> {
    let map = layer_map_for_output(output);
    map.layers()
        .flat_map(|l| layer_surface_backdrops(&map, l))
        .collect()
}

/// Per-object data of a backdrop: the surface it belongs to and its identity there. The backdrop of a surface that
/// belongs to another client has no surface and does nothing.
pub struct BackdropData {
    surface: Option<WlSurface>,
    serial: u64,
    /// The client's count of backdrops, given back when this one is destroyed; `None` for an inert backdrop.
    held: Option<Arc<AtomicUsize>>,
}

impl BackdropData {
    /// Edit the backdrop in the surface's pending state; inert once the surface is gone.
    fn edit(&self, f: impl FnOnce(&mut BackdropEntry)) {
        let Some(surface) = self.surface.as_ref().filter(|s| s.is_alive()) else {
            return;
        };
        with_states(surface, |states| {
            let mut cache = states.cached_state.get::<BackdropState>();
            let entry = cache
                .pending()
                .entries
                .iter_mut()
                .find(|e| e.serial == self.serial);
            f(entry.expect(
                "a backdrop stays in its surface's pending state while its resource is alive",
            ));
        });
    }
}

impl GlobalDispatch<CupertinoAppEffectsV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoAppEffectsV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

/// Whether `surface` was created by `client`.
fn owned_by(surface: &WlSurface, client: &Client) -> bool {
    surface.client().is_some_and(|c| c.id() == client.id())
}

impl Dispatch<CupertinoAppEffectsV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        effects: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_app_effects_v1::Request::GetBackdrop { id, surface } => {
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                if !owned_by(&surface, client) {
                    init.init(
                        id,
                        BackdropData {
                            surface: None,
                            serial,
                            held: None,
                        },
                    );
                    effects.post_error(
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    );
                    return;
                }
                let held = client
                    .get_data::<ClientState>()
                    .map(|data| data.backdrops.clone())
                    .unwrap_or_default();
                let on_surface = with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<BackdropState>()
                        .pending()
                        .entries
                        .len()
                });
                // the count is taken before the check, so a refused request cannot leave it raised
                if held.fetch_add(1, Ordering::SeqCst) >= MAX_BACKDROPS_PER_CLIENT
                    || on_surface >= MAX_BACKDROPS_PER_SURFACE
                {
                    held.fetch_sub(1, Ordering::SeqCst);
                    init.init(
                        id,
                        BackdropData {
                            surface: None,
                            serial,
                            held: None,
                        },
                    );
                    effects.post_error(
                        EffectsError::TooManyBackdrops,
                        format!(
                            "at most {MAX_BACKDROPS_PER_SURFACE} backdrops per surface and {MAX_BACKDROPS_PER_CLIENT} per client"
                        ),
                    );
                    return;
                }
                with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<BackdropState>()
                        .pending()
                        .entries
                        .push(BackdropEntry {
                            serial,
                            material: None,
                            shape: Shape::default(),
                            alpha: 1.0,
                        });
                });
                init.init(
                    id,
                    BackdropData {
                        surface: Some(surface),
                        serial,
                        held: Some(held),
                    },
                );
            }
            cupertino_app_effects_v1::Request::SetTitlebarStyle { surface, style } => {
                if owned_by(&surface, client) {
                    state.set_titlebar_style(effects, &surface, style)
                } else {
                    effects.post_error(
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    );
                }
            }
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                state.create_appearance_observer(id, init)
            }
            // `Destroy` needs nothing: objects created through the global stay valid
            _ => {}
        }
    }
}

impl Cupertino {
    /// `set_titlebar_style`: record the style of a window that has not mapped and size its insets for it.
    /// A window still on the client-side decoration mode keeps no insets; its style applies if it switches.
    fn set_titlebar_style(
        &mut self,
        effects: &CupertinoAppEffectsV1,
        surface: &WlSurface,
        style: WEnum<cupertino_app_effects_v1::TitlebarStyle>,
    ) {
        let style = match style {
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::Standard) => {
                TitlebarStyle::Standard
            }
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::Unified) => {
                TitlebarStyle::Unified
            }
            other => {
                effects.post_error(
                    EffectsError::Window,
                    format!("{other:?} is not a title bar style"),
                );
                return;
            }
        };
        let Some(i) = self
            .windows
            .iter()
            .position(|w| w.has_surface(surface))
            .filter(|i| !self.windows[*i].mapped)
        else {
            effects.post_error(
                EffectsError::Window,
                "the surface is not an application window that is still unmapped",
            );
            return;
        };
        let insets = self.decoration_insets(style);
        let e = &mut self.windows[i];
        e.titlebar = style;
        if e.server_side {
            e.insets = insets;
        }
    }
}

impl Dispatch<CupertinoBackdropV1, BackdropData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoBackdropV1,
        request: cupertino_backdrop_v1::Request,
        data: &BackdropData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_backdrop_v1::Request::SetMaterial { name } => {
                let (appearance, reduce_transparency) = (
                    state.appearance(),
                    state.display_options().reduce_transparency,
                );
                if !state
                    .render
                    .as_ref()
                    .is_some_and(|r| r.recipe(&name, appearance, reduce_transparency).is_some())
                {
                    resource.post_error(
                        Error::UnknownMaterial,
                        format!("{name:?} is not a material recipe"),
                    );
                    return;
                }
                data.edit(|e| e.material = Some(name));
            }
            cupertino_backdrop_v1::Request::SetShape {
                x,
                y,
                width,
                height,
                radius,
            } => {
                if width < 0 || height < 0 || radius < 0.0 {
                    resource.post_error(
                        Error::InvalidShape,
                        format!("negative size or radius: {width}x{height}, radius {radius}"),
                    );
                    return;
                }
                data.edit(|e| {
                    e.shape = Shape {
                        x,
                        y,
                        width,
                        height,
                        radius,
                    }
                });
            }
            cupertino_backdrop_v1::Request::SetAlpha { alpha } => {
                if !(0.0..=1.0).contains(&alpha) {
                    resource.post_error(
                        Error::InvalidAlpha,
                        format!("alpha {alpha} is outside 0 to 1"),
                    );
                    return;
                }
                data.edit(|e| e.alpha = alpha);
            }
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The backdrop leaves its surface's pending state, so it disappears with the next commit.
    fn destroyed(_: &mut Self, _: ClientId, _: &CupertinoBackdropV1, data: &BackdropData) {
        if let Some(held) = &data.held {
            held.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(surface) = data.surface.as_ref().filter(|s| s.is_alive()) {
            with_states(surface, |states| {
                states
                    .cached_state
                    .get::<BackdropState>()
                    .pending()
                    .entries
                    .retain(|e| e.serial != data.serial)
            });
        }
    }
}
