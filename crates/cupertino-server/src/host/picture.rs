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

    /// A component's `wl_surface` ends: a persisted one keeps its last picture for the hold the end of its layer role
    /// starts.
    fn host_surface_destroyed(&mut self, surface: &WlSurface) {
        let id = surface.id();
        if !self.hosts.persisted.contains_key(&id) {
            return;
        }
        let picture = self.capture_held_picture(surface);
        if let Some(persisted) = self.hosts.persisted.get_mut(&id) {
            persisted.last_picture = picture;
        }
    }

    /// The picture of the layer surface `surface` as its output shows it: its surface tree drawn by the renderer into
    /// a texture (shm, dmabuf or any buffer the renderer imports), or without a renderer a copy of its shm pixels.
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
        if let Some(snapshot) = self.capture_layer_texture(surface, idx, area) {
            return Some(HeldPicture::Texture {
                snapshot: Rc::new(snapshot),
                size: (area.w as i32, area.h as i32),
            });
        }
        HostManager::capture_shm_picture(surface)
    }

    /// Draw the surface tree of `surface`, its top-left corner at the corner of `area` (global logical points), at the
    /// scale of output `idx` into a snapshot texture; `None` without a renderer.
    fn capture_layer_texture(
        &mut self,
        surface: &WlSurface,
        idx: usize,
        area: Rect,
    ) -> Option<crate::render::snapshots::SnapshotOwner> {
        let mut backend = self.backend.take()?;
        let origin = smithay::utils::Point::<f64, Logical>::from((area.x, area.y));
        let captured = self
            .with_scene(backend.renderer(), idx, |renderer, mut p| {
                let texture =
                    crate::render::scene::capture_surface(renderer, &mut p, surface, origin, area)?;
                Some(p.res.snapshots.insert(texture))
            })
            .flatten();
        self.backend = Some(backend);
        captured
    }
}
