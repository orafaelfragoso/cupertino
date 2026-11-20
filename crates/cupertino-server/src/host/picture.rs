//! D19 held pictures: the last picture of a persistent surface, taken once as its hold starts, whatever kind of
//! buffer it showed.

use smithay::desktop::WindowSurfaceType;
use smithay::wayland::compositor::add_destruction_hook;

use super::*;

impl Cupertino {
    /// `CompositorHandler::new_surface`: watch the end of every surface of a component connection. The hook comes
    /// ahead of the renderer's own, which lets the surface's buffer and textures go, so a persistent surface whose
    /// `wl_surface` ends before its layer role (a dropped connection ends objects in the order they were made) can
    /// still be drawn.
    pub fn host_surface_created(&mut self, surface: &WlSurface) {
        let of_component = surface
            .client()
            .and_then(|c| c.get_data::<ClientState>().and_then(|s| s.component))
            .is_some();
        if of_component {
            add_destruction_hook::<Self, _>(surface, |state, surface| {
                state.host_surface_destroyed(surface);
            });
        }
    }

    /// A component's `wl_surface` ends: a persisted one keeps its last picture, and where it lay, for the hold the end
    /// of its layer role starts.
    fn host_surface_destroyed(&mut self, surface: &WlSurface) {
        let id = surface.id();
        if !self.hosts.persisted.contains_key(&id) {
            return;
        }
        let picture = self.capture_held_picture(surface);
        let geometry = self.outputs.iter().find_map(|o| {
            let map = layer_map_for_output(&o.output);
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)?;
            map.layer_geometry(layer)
        });
        if let Some(persisted) = self.hosts.persisted.get_mut(&id) {
            persisted.last_picture = picture;
            persisted.last_geometry = geometry;
        }
    }

    /// The picture of the layer surface `surface` as its output shows it: its surface tree and its layers' content drawn
    /// by the renderer into textures (shm, dmabuf or any buffer the renderer imports), cut at each glass they draw, with
    /// the backdrops they draw kept to be drawn live between and behind them; without a renderer a copy of its shm pixels, with its own backdrops.
    pub(super) fn capture_held_picture(&mut self, surface: &WlSurface) -> Option<HeldPicture> {
        let (idx, area) = self.outputs.iter().enumerate().find_map(|(i, o)| {
            let map = layer_map_for_output(&o.output);
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)?;
            let g = map.layer_geometry(layer)?;
            let area = Rect::new(
                o.rect.x + f64::from(g.loc.x),
                o.rect.y + f64::from(g.loc.y),
                f64::from(g.size.w),
                f64::from(g.size.h),
            );
            Some((i, area))
        })?;
        let menu_bar = self.menu_bars.has_surface(surface);
        if let Some((slices, backdrops)) = self.capture_layer_texture(surface, idx, area) {
            return Some(HeldPicture {
                content: HeldContent::Texture {
                    slices: slices.into(),
                    size: (area.w as i32, area.h as i32),
                },
                backdrops,
                menu_bar,
            });
        }
        Some(HeldPicture {
            content: HostManager::capture_shm_picture(surface)?,
            backdrops: crate::render::scene::HeldBackdrops::of_surface(surface),
            menu_bar,
        })
    }

    /// Draw the surface tree of `surface` and its layers' content, its top-left corner at the corner of `area` (global
    /// logical points), at the scale of output `idx` into snapshot textures, one per slice between the glass they draw
    /// (front to back, `None` for an empty slice), each over the bounds of what it draws, with the backdrops they draw;
    /// `None` without a renderer.
    fn capture_layer_texture(
        &mut self,
        surface: &WlSurface,
        idx: usize,
        area: Rect,
    ) -> Option<(Vec<Option<HeldSlice>>, crate::render::scene::HeldBackdrops)> {
        let mut backend = self.backend.take()?;
        let captured = self
            .with_scene(backend.renderer(), idx, |renderer, mut p| {
                let (textures, backdrops) =
                    crate::render::scene::capture_held_surface(renderer, &mut p, surface, area)?;
                let slices = textures
                    .into_iter()
                    .map(|slice| {
                        slice.map(|s| HeldSlice {
                            pixels: p.res.snapshots.insert(s.pixels),
                            rect: s.rect,
                            opacity: s.opacity,
                        })
                    })
                    .collect();
                Some((slices, backdrops))
            })
            .flatten();
        self.backend = Some(backend);
        captured
    }
}
