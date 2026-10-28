//! Test-only oracle: the scene builder as it was before the neutral scene frame, kept to compare
//! the migrated composition against. Removed in `wp04-seam-gates`.

use cupertino_decorations::TitlebarStyle;
use cupertino_effects::{DockEdge, Mesh, MinimizeEffect, Vertex};
use cupertino_geometry::CornerStyle;
use std::cell::RefCell;
use std::collections::HashMap;

use cupertino_wm::{OutputId, Presentation, Rect, WindowId};
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale, Size};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use super::clip::{ClipElement, ClipShape};
use super::material::{MaterialElement, MaterialStage};
use super::mesh::{capture, MeshElement};
use super::motion::WorkspaceTransition;
use super::programs::{corner_uniforms, u4};
use super::scene::{
    placed_plan, plan_for, windows_on_output, CursorPlan, LockView, SceneParams, WindowPlan,
    SHEET_DIMMING_MATERIAL,
};
use super::shader_rect::{to_physical_rect, ElementCache, ElementKey, ShaderRect, Sig};
use super::ui::{rounded_rect, RoundedRectSpec};
use super::{CupertinoElement, LayerStack, SceneLayer};
use crate::app_effects_v1::{surface_backdrops, BackdropArea};
use crate::decor::DecorInput;
use crate::fx::IntoGeom;
use crate::state::WindowEntry;
use crate::window_manager::Look;

fn local(r: Rect, origin: &Rect) -> smithay::utils::Rectangle<f64, Logical> {
    smithay::utils::Rectangle::new((r.x - origin.x, r.y - origin.y).into(), (r.w, r.h).into())
}

fn sig_rect(r: smithay::utils::Rectangle<f64, Logical>) -> Sig {
    Sig::new().rect(r)
}

/// Rounded rectangle element helper.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rrect(
    p: &mut SceneParams<'_>,
    key: ElementKey,
    rect: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    fill: [f32; 4],
    border: Option<(f64, [f32; 4])>,
    alpha: f32,
) -> ShaderRect {
    let spec = RoundedRectSpec {
        rect,
        radius,
        continuous,
        fill,
        border,
        alpha,
    };
    rounded_rect(&p.res.programs, p.cache, p.scale, key, &spec)
}

/// The wallpaper of the output: the image of the user or of the asset pack, else the procedural one.
fn wallpaper_element(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>) -> CupertinoElement {
    let dark = p.config.is_dark();
    let image = p.overlays.wallpaper.element(
        renderer,
        p.output_id.0,
        &p.config.wallpaper,
        dark,
        p.output_px,
        p.scale,
        p.now,
    );
    image.unwrap_or_else(|| wallpaper(p).into())
}

fn wallpaper(p: &mut SceneParams<'_>) -> ShaderRect {
    let dark = p.config.is_dark();
    let (c0, c1, c2, c3) = if dark {
        (
            [0.07, 0.09, 0.20, 1.0],
            [0.20, 0.09, 0.27, 1.0],
            [0.10, 0.30, 0.58, 1.0],
            [0.50, 0.16, 0.42, 1.0],
        )
    } else {
        (
            [0.72, 0.82, 0.97, 1.0],
            [0.96, 0.78, 0.86, 1.0],
            [0.45, 0.68, 0.99, 1.0],
            [0.99, 0.66, 0.50, 1.0],
        )
    };
    let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), (p.rect.w, p.rect.h).into());
    let (id, commit) = p.cache.slot(
        ElementKey::new("wallpaper", 0),
        sig_rect(area).u(dark as u64).finish(),
    );
    ShaderRect::new(
        p.res.programs.wallpaper.clone(),
        id,
        commit,
        area,
        1.0,
        vec![u4("c0", c0), u4("c1", c1), u4("c2", c2), u4("c3", c3)],
        true,
    )
}

/// Solid black over the whole output: all that is visible of the session besides the lock surface.
fn lock_backdrop(p: &mut SceneParams<'_>) -> ShaderRect {
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), (p.rect.w, p.rect.h).into());
    let (id, commit) = p
        .cache
        .slot(ElementKey::new("lock-backdrop", 0), sig_rect(area).finish());
    ShaderRect::new(
        p.res.programs.wallpaper.clone(),
        id,
        commit,
        area,
        1.0,
        vec![
            u4("c0", BLACK),
            u4("c1", BLACK),
            u4("c2", BLACK),
            u4("c3", BLACK),
        ],
        true,
    )
}

/// Approximate average colour of the procedural wallpaper (tint layer input, approximate).
pub fn wallpaper_tint(dark: bool) -> [f32; 3] {
    if dark {
        [0.22, 0.17, 0.37]
    } else {
        [0.78, 0.76, 0.84]
    }
}

/// Shadow of window `id`, with the plan's corner radius ([`WindowPlan::corner`]).
fn shadow_element(
    p: &mut SceneParams<'_>,
    id: WindowId,
    plan: &WindowPlan,
    active: bool,
    alpha: f64,
) -> Option<ShaderRect> {
    let params = p.decor.shadow_params(active);
    let radius = plan
        .corner
        .map_or(p.config.window.corner_radius, |r| r / shadow_scale(plan));
    shadow_with(
        p,
        ElementKey::new("shadow", id.0),
        &params,
        plan,
        radius,
        alpha,
    )
}

/// Factor the shadow's blur, offset and corner follow a scaled window by (at least 1/4).
fn shadow_scale(plan: &WindowPlan) -> f64 {
    plan.scale_x.min(plan.scale_y).max(0.0).max(0.25)
}

/// `e` masked by `shape`. The shape's generation comes from the element cache, so a change of the
/// mask alone (an animated corner radius) damages the element.
fn clipped<E: Element>(p: &mut SceneParams<'_>, e: E, shape: ClipShape) -> ClipElement<E> {
    let r = shape.rect;
    let sig = Sig::new()
        .f(f64::from(shape.extent))
        .f(f64::from(shape.exponent))
        .f(f64::from(r.loc.x))
        .f(f64::from(r.loc.y))
        .f(f64::from(r.size.w))
        .f(f64::from(r.size.h))
        .finish();
    let (_, generation) = p.cache.slot(ElementKey::new("clip", id_hash(e.id())), sig);
    ClipElement::new(e, p.res.programs.clip.clone(), shape, generation)
}

/// Rounded mask of `plan.visual` with a visual corner radius of `radius` points.
fn corner_clip(p: &SceneParams<'_>, plan: &WindowPlan, radius: f64) -> ClipShape {
    let rect = to_physical_rect(local(plan.visual, &p.rect), Scale::from(p.scale));
    let (extent, exponent) = corner_uniforms(radius, p.config.window.continuous_corners, p.scale);
    ClipShape {
        rect,
        extent,
        exponent,
        viewport: p.output_px,
        exact: plan.exact,
    }
}

/// Shadow shader rectangle behind `plan.visual` from the verified parameter table; the shader
/// receives the device-pixel values of `ShadowParams::gpu` converted back to points.
pub(crate) fn shadow_with(
    p: &mut SceneParams<'_>,
    key: ElementKey,
    params: &cupertino_decorations::ShadowParams,
    plan: &WindowPlan,
    corner: f64,
    alpha: f64,
) -> Option<ShaderRect> {
    let style = &p.config.window;
    let v = plan.visual;
    let s = shadow_scale(plan);
    let frame = cupertino_geometry::Size::new(v.w, v.h);
    let g = params.gpu(frame, corner, p.scale);
    let density = f64::from(g.density) * plan.shadow;
    if density <= 0.0 || alpha <= 0.0 {
        return None;
    }
    let sigma = f64::from(g.sigma_px) / p.scale * s;
    let off = f64::from(g.offset_px[1]) / p.scale * s;
    let reach = f64::from(g.radius_px) / p.scale * s;
    let origin = &p.rect;
    let area = smithay::utils::Rectangle::<f64, Logical>::new(
        (
            v.x - origin.x - reach,
            v.y - origin.y - reach + off.min(0.0),
        )
            .into(),
        (v.w + 2.0 * reach, v.h + 2.0 * reach + off.abs()).into(),
    );
    let caster = [
        (v.x - origin.x - area.loc.x) as f32,
        (v.y - origin.y - area.loc.y) as f32,
        v.w as f32,
        v.h as f32,
    ];
    let radius = corner * s;
    let cs = if style.continuous_corners {
        CornerStyle::continuous()
    } else {
        CornerStyle::Circular
    };
    let extent = radius * cs.expansion();
    let uniforms = vec![
        u4("caster", caster),
        u4(
            "params",
            [radius as f32, sigma as f32, density as f32, off as f32],
        ),
        u4(
            "flags",
            [
                style.shadow.cutout as u8 as f32,
                cs.exponent() as f32,
                p.scale as f32,
                extent as f32,
            ],
        ),
    ];
    let sig = sig_rect(area)
        .f(radius)
        .f(sigma)
        .f(density)
        .f(off)
        .f(alpha)
        .f(p.scale)
        .u(style.shadow.cutout as u64)
        .finish();
    let (eid, commit) = p.cache.slot(key, sig);
    Some(ShaderRect::new(
        p.res.programs.shadow.clone(),
        eid,
        commit,
        area,
        alpha as f32,
        uniforms,
        false,
    ))
}

/// Elements of one window, front to back: popups, content (masked to the plan's corner radius,
/// rescaled), shadow.
fn window_elements(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
    shadow: bool,
    decorations: Vec<CupertinoElement>,
) -> Vec<CupertinoElement> {
    let mut out: Vec<CupertinoElement> = Vec::new();
    let scale = Scale::from(p.scale);
    let geo = entry.window.geometry();
    let ins = entry.insets;
    let style = p.config.window;
    let visual_local = local(plan.visual, &p.rect);
    let v_phys = to_physical_rect(visual_local, scale);
    let geo_phys: Point<i32, Physical> = geo.loc.to_f64().to_physical(scale).to_i32_round();
    let inset_phys: Point<i32, Physical> = Point::<f64, Logical>::from((ins.left, ins.top))
        .to_physical(scale)
        .to_i32_round();
    let origin = v_phys.loc + inset_phys - geo_phys;
    let Some(surface) = entry.window.wl_surface().map(|s| s.into_owned()) else {
        return out;
    };
    let rescale = Scale::from((plan.scale_x, plan.scale_y));
    let alpha = plan.opacity as f32;

    for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
        let offset: Point<i32, Physical> =
            (geo.loc + popup_offset - popup.geometry().loc).to_physical_precise_round(scale);
        let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            origin + offset,
            scale,
            alpha,
            Kind::Unspecified,
        );
        out.extend(els.into_iter().map(|e| {
            CupertinoElement::from(RescaleRenderElement::from_element(e, v_phys.loc, rescale))
        }));
        // the popup's own backdrop (a menu's material) lies directly below it, above the window
        let popup_origin = Point::<f64, Logical>::from((
            visual_local.loc.x + ins.left + f64::from(popup_offset.x - popup.geometry().loc.x),
            visual_local.loc.y + ins.top + f64::from(popup_offset.y - popup.geometry().loc.y),
        ));
        let backdrops = rescaled_backdrops(
            surface_backdrops(popup.wl_surface(), popup_origin),
            visual_local.loc,
            plan,
        );
        out.extend(popup_backdrop_elements(p, &backdrops, alpha));
    }

    out.extend(decorations);
    let styled = style.style_client_decorated || entry.server_side;
    let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        origin,
        scale,
        alpha,
        Kind::Unspecified,
    );
    let radius = plan
        .corner
        .unwrap_or(style.corner_radius * plan.scale_x.min(plan.scale_y));
    let shape = corner_clip(p, plan, radius);
    for e in els {
        if styled && radius > 0.0 {
            let clip = clipped(p, e, shape);
            out.push(RescaleRenderElement::from_element(clip, v_phys.loc, rescale).into());
        } else {
            out.push(RescaleRenderElement::from_element(e, v_phys.loc, rescale).into());
        }
    }
    // backdrops follow the window's rescale about its visual origin
    let origin_local = Point::<f64, Logical>::from((
        visual_local.loc.x + ins.left - f64::from(geo.loc.x),
        visual_local.loc.y + ins.top - f64::from(geo.loc.y),
    ));
    let backdrops = rescaled_backdrops(
        surface_backdrops(&surface, origin_local),
        visual_local.loc,
        plan,
    );
    out.extend(backdrop_elements(p, &backdrops, alpha));
    if shadow && styled {
        if let Some(s) = shadow_element(p, entry.id, plan, active, plan.opacity) {
            out.push(s.into());
        }
    }
    out
}

/// Backdrops of a window's surface or popup moved and scaled with the window's plan about its visual origin.
fn rescaled_backdrops(
    backdrops: Vec<BackdropArea>,
    visual_origin: Point<f64, Logical>,
    plan: &WindowPlan,
) -> Vec<BackdropArea> {
    let s_min = plan.scale_x.min(plan.scale_y);
    backdrops
        .into_iter()
        .map(|mut b| {
            let at = visual_origin
                + Point::from((
                    (b.area.loc.x - visual_origin.x) * plan.scale_x,
                    (b.area.loc.y - visual_origin.y) * plan.scale_y,
                ));
            b.area = smithay::utils::Rectangle::new(
                at,
                (b.area.size.w * plan.scale_x, b.area.size.h * plan.scale_y).into(),
            );
            b.radius *= s_min;
            b
        })
        .collect()
}

/// Title bar and rim of a server-side decorated window (front to back), rescaled with the window
/// and, for a stage placement ([`WindowPlan::corner`]), masked to its animated corners.
fn builtin_decorations(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<CupertinoElement> {
    let geo = entry.window.geometry();
    let ins = entry.insets;
    let (fw, fh) = (
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    if fw < 1.0 || fh < 1.0 || (ins.top <= 0.0 && entry.titlebar == TitlebarStyle::Standard) {
        return Vec::new();
    }
    let (title, resizable) =
        p.wm.window(entry.id)
            .map(|w| {
                (
                    w.title.clone(),
                    w.min_size.zip(w.max_size).is_none_or(|(a, b)| a != b),
                )
            })
            .unwrap_or_default();
    let bufs = p.decor.buffers(&DecorInput {
        id: entry.id,
        style: entry.titlebar,
        frame: (fw, fh),
        scale: p.scale,
        active,
        title: &title,
        resizable,
    });
    let scale = Scale::from(p.scale);
    let v_phys = to_physical_rect(local(plan.visual, &p.rect), scale);
    let rescale = Scale::from((plan.scale_x, plan.scale_y));
    let alpha = Some(plan.opacity as f32);
    let loc = v_phys.loc.to_f64();
    let clip = plan.corner.map(|r| corner_clip(p, plan, r));
    let element = |p: &mut SceneParams<'_>,
                   e: MemoryRenderBufferRenderElement<GlesRenderer>|
     -> CupertinoElement {
        match clip {
            Some(shape) => {
                RescaleRenderElement::from_element(clipped(p, e, shape), v_phys.loc, rescale).into()
            }
            None => RescaleRenderElement::from_element(e, v_phys.loc, rescale).into(),
        }
    };
    let mut out = Vec::new();
    // the strip of a unified bar lies over the surface; the rim below a standard bar starts under it
    let reserved = ins.top.round() as i32;
    let size = smithay::utils::Size::<i32, Logical>::from((
        fw.round() as i32,
        bufs.titlebar_pt.round() as i32,
    ));
    let tb_src = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (f64::from(bufs.titlebar_px.0), f64::from(bufs.titlebar_px.1)).into(),
    );
    if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        loc,
        &bufs.titlebar,
        alpha,
        Some(tb_src),
        Some(size),
        Kind::Unspecified,
    ) {
        out.push(element(p, e));
    }
    let tb_px = if entry.titlebar == TitlebarStyle::Standard {
        bufs.titlebar_px.1
    } else {
        0
    };
    let src = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, f64::from(tb_px)).into(),
        (f64::from(bufs.rim_px.0), f64::from(bufs.rim_px.1 - tb_px)).into(),
    );
    let rim_size = smithay::utils::Size::<i32, Logical>::from((
        fw.round() as i32,
        fh.round() as i32 - reserved,
    ));
    let rim_loc = loc + Point::<f64, Physical>::from((0.0, f64::from(tb_px)));
    if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        rim_loc,
        &bufs.rim,
        alpha,
        Some(src),
        Some(rim_size),
        Kind::Unspecified,
    ) {
        out.insert(0, element(p, e));
    }
    out
}

/// Per-window elements of the decorations, front to back.
fn window_decorations(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<CupertinoElement> {
    let mut out = Vec::new();
    if entry.server_side && p.config.window.decorations {
        out.extend(builtin_decorations(renderer, p, entry, plan, active));
    }
    out
}

/// Render a window (decorations, content, optionally its shadow) at its natural size into a new
/// texture covering `natural` grown by `margin` points. Returns the texture and the captured
/// rectangle (global logical points).
pub fn capture_window(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    natural: Rect,
    active: bool,
    margin: f64,
    shadow: bool,
) -> Option<(smithay::backend::renderer::gles::GlesTexture, Rect)> {
    let area = Rect::new(
        natural.x - margin,
        natural.y - margin,
        natural.w + 2.0 * margin,
        natural.h + 2.0 * margin,
    );
    let size_px = (
        ((area.w * p.scale).round() as i32).max(1),
        ((area.h * p.scale).round() as i32).max(1),
    );
    let saved = (p.rect, p.output_px);
    p.rect = area;
    p.output_px = size_px;
    let mut tmp = ElementCache::default();
    std::mem::swap(p.cache, &mut tmp);
    let plan = WindowPlan {
        visual: natural,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
    };
    let deco = window_decorations(renderer, p, entry, &plan, active);
    let els = window_elements(renderer, p, entry, &plan, active, shadow, deco);
    std::mem::swap(p.cache, &mut tmp);
    p.rect = saved.0;
    p.output_px = saved.1;
    match capture(renderer, &els, size_px.into(), p.scale) {
        Ok(t) => Some((t, area)),
        Err(e) => {
            tracing::warn!(error = %e, "window snapshot failed");
            None
        }
    }
}

/// Screen edge the dock slot `target` sits at: the nearest of the bottom, left and right edges.
fn dock_edge(target: Rect, screen: &Rect) -> DockEdge {
    let (bottom, left, right) = (
        screen.bottom() - target.cy(),
        target.cx() - screen.x,
        screen.right() - target.cx(),
    );
    if left < bottom && left <= right {
        DockEdge::Left
    } else if right < bottom {
        DockEdge::Right
    } else {
        DockEdge::Bottom
    }
}

/// Minimize / restore through the effect mesh: the window is captured at its natural size and
/// drawn through the warp, funnel or scale mesh towards the dock tile.
fn minimize_elements(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    pres: &Presentation,
    slide: f64,
    active: bool,
) -> Option<Vec<CupertinoElement>> {
    let kind = p.fx.cfg.minimize?;
    let shared = p.res.mesh.clone()?;
    let geo = entry.window.geometry();
    let ins = entry.insets;
    let natural = Rect::new(
        pres.frame.x + slide,
        pres.frame.y,
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    if natural.w < 1.0 || natural.h < 1.0 {
        return None;
    }
    let (texture, _) = capture_window(renderer, p, entry, natural, active, 0.0, false)?;
    let effect = MinimizeEffect::new(
        kind,
        natural.into_geom(),
        pres.minimize_target.into_geom(),
        dock_edge(pres.minimize_target, &p.rect),
    );
    let mesh = effect.mesh(pres.minimize_progress);
    let (ox, oy) = (p.rect.x as f32, p.rect.y as f32);
    let local_mesh = Mesh::new(
        mesh.cols,
        mesh.rows,
        mesh.vertices
            .iter()
            .map(|v| Vertex {
                position: [v.position[0] - ox, v.position[1] - oy],
                uv: v.uv,
            })
            .collect(),
    );
    tracing::trace!(progress = pres.minimize_progress, triangles = mesh.cell_count() * 2, ?natural, target = ?pres.minimize_target, "minimize mesh");
    let sig = Sig::new()
        .f(pres.minimize_progress)
        .f(natural.x)
        .f(natural.y)
        .f(natural.w)
        .f(pres.opacity)
        .u(kind as u64)
        .finish();
    let (id, commit) = p
        .cache
        .slot(ElementKey::new("minimize-mesh", entry.id.0), sig);
    Some(vec![MeshElement::new(
        id,
        commit,
        texture,
        shared,
        &local_mesh,
        p.scale,
        pres.opacity as f32,
    )
    .into()])
}

/// Closing ghosts of this output: the last frame of a window fading out.
fn closing_elements(p: &mut SceneParams<'_>) -> Vec<CupertinoElement> {
    let Some(shared) = p.res.mesh.clone() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in
        p.fx.closing
            .iter()
            .rev()
            .filter(|c| c.output == p.output_id)
    {
        let progress = c.progress(p.now);
        let (rect, alpha) = c.placement(p.now);
        let alpha = alpha as f32;
        if alpha <= 0.0 {
            continue;
        }
        let r = Rect::new(rect.x - p.rect.x, rect.y - p.rect.y, rect.w, rect.h);
        let mesh = Mesh::quad(r.into_geom());
        let sig = Sig::new().f(progress).f(r.x).f(r.y).finish();
        let (id, commit) = p.cache.slot(ElementKey::new("closing-ghost", c.id.0), sig);
        out.push(
            MeshElement::new(
                id,
                commit,
                c.texture.clone(),
                shared.clone(),
                &mesh,
                p.scale,
                alpha,
            )
            .into(),
        );
    }
    out
}

/// Elements of the layer-shell `layer`, front to back. The backdrop captures of menu bars are
/// appended to `captures`: they belong below the windows (see [`bar_backdrop_elements`]).
fn layer_elements(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    layer: Layer,
    captures: &mut Vec<CupertinoElement>,
) -> Vec<CupertinoElement> {
    let map = layer_map_for_output(p.output);
    let scale = Scale::from(p.scale);
    let mut out = Vec::new();
    // later surfaces are drawn on top of earlier ones
    for surface in map.layers_on(layer).rev() {
        let Some(geo) = map.layer_geometry(surface) else {
            continue;
        };
        // a menu bar is offset and faded by its presentation (hide, reveal, inactive output)
        let look = p.menu_bars.look(surface.wl_surface(), p.rect.y, p.now);
        let (dy, opacity) = look.map_or((0.0, 1.0), |l| (l.dy, l.alpha));
        if opacity <= 0.0 {
            continue;
        }
        let origin: Point<f64, Logical> = geo.loc.to_f64() + Point::from((0.0, dy));
        let loc: Point<i32, Physical> = origin.to_physical_precise_round(scale);
        // popups above their parent, each with its own backdrop directly below it
        for (popup, popup_offset) in PopupManager::popups_for_surface(surface.wl_surface()) {
            let popup_origin = origin + (popup_offset - popup.geometry().loc).to_f64();
            let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    popup_origin.to_physical_precise_round(scale),
                    scale,
                    opacity,
                    Kind::Unspecified,
                );
            out.extend(els.into_iter().map(CupertinoElement::from));
            let backdrops = surface_backdrops(popup.wl_surface(), popup_origin);
            out.extend(popup_backdrop_elements(p, &backdrops, opacity));
        }
        let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
            renderer,
            surface.wl_surface(),
            loc,
            scale,
            opacity,
            Kind::Unspecified,
        );
        out.extend(els.into_iter().map(CupertinoElement::from));
        let backdrops = surface_backdrops(surface.wl_surface(), origin);
        if look.is_some() {
            out.extend(bar_backdrop_elements(p, &backdrops, opacity, captures));
        } else {
            out.extend(backdrop_elements(p, &backdrops, opacity));
        }
    }
    out
}

/// Elements of popups that carry a material, to go directly below the popup's surface (front to back:
/// rim, material, shadow). The rim is the window rim of 1 device pixel, the shadow
/// the panel row of the table (it is cut out under the popup, so the material stays
/// translucent). `backdrops` are in output-local logical coordinates.
fn popup_backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
) -> Vec<CupertinoElement> {
    let mut out = Vec::new();
    let params = p.decor.popup_shadow_params();
    let rim = p.decor.rim_color(p.scale);
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        out.push(
            rrect(
                p,
                ElementKey::new("popup-rim", b.key),
                b.area,
                b.radius,
                true,
                [0.0; 4],
                Some((1.0 / p.scale, rim)),
                alpha,
            )
            .into(),
        );
        out.extend(material_layer(
            p,
            ElementKey::new("backdrop", b.key),
            &b.material,
            b.area,
            b.radius,
            alpha,
        ));
        let visual = cupertino_wm::Rect::new(
            p.rect.x + b.area.loc.x,
            p.rect.y + b.area.loc.y,
            b.area.size.w,
            b.area.size.h,
        );
        let plan = WindowPlan {
            visual,
            scale_x: 1.0,
            scale_y: 1.0,
            opacity: 1.0,
            shadow: 1.0,
            exact: true,
            corner: None,
        };
        if let Some(s) = shadow_with(
            p,
            ElementKey::new("popup-shadow", b.key),
            &params,
            &plan,
            b.radius,
            f64::from(alpha),
        ) {
            out.push(s.into());
        }
    }
    out
}

/// Material elements of committed backdrops, to go directly below the owning surface's elements
/// (the list is front to back, so later backdrops come first).
fn backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
) -> Vec<CupertinoElement> {
    let mut out = Vec::new();
    for b in backdrops.iter().rev() {
        let key = ElementKey::new("backdrop", b.key);
        out.extend(material_layer(
            p,
            key,
            &b.material,
            b.area,
            b.radius,
            b.alpha as f32 * opacity,
        ));
    }
    out
}

/// Backdrops of a menu bar, to go directly below the bar's elements. A bar reserves its strip,
/// so no window lies under it; the only window pixels there would be shadows, and they must not
/// tint the material. The material therefore samples what lies below the window stack: its
/// capture element is added to `captures` (for [`SceneLayer::BackdropCapture`]) and only the
/// composite stays with the bar. Without the material pass the fallback fill is drawn instead.
fn bar_backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
    captures: &mut Vec<CupertinoElement>,
) -> Vec<CupertinoElement> {
    let mut out = Vec::new();
    let output = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (p.rect.w, p.rect.h).into(),
    );
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        let owner = b.key;
        let composite = material_element(
            p,
            ElementKey::new("backdrop", b.key),
            &b.material,
            b.area,
            b.radius,
            alpha,
            MaterialStage::Composite { owner },
        );
        let Some(composite) = composite else {
            out.push(
                fallback_fill(
                    p,
                    ElementKey::new("backdrop", b.key),
                    b.area,
                    b.radius,
                    alpha,
                )
                .into(),
            );
            continue;
        };
        // the capture covers the whole output: it is only redrawn together with a full repaint
        let capture = material_element(
            p,
            ElementKey::new("backdrop-capture", b.key),
            &b.material,
            output,
            0.0,
            alpha,
            MaterialStage::Capture { owner },
        );
        captures.extend(capture.map(CupertinoElement::from));
        out.push(composite.into());
    }
    out
}

/// Material pass (or the translucent fallback fill without one) behind `area`, drawn at `alpha`.
pub(crate) fn material_layer(
    p: &mut SceneParams<'_>,
    key: ElementKey,
    material: &str,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
) -> Vec<CupertinoElement> {
    match material_element(p, key, material, area, radius, alpha, MaterialStage::Full) {
        Some(m) => vec![m.into()],
        None => vec![fallback_fill(p, key, area, radius, alpha).into()],
    }
}

/// Material element for `material`; `None` while the material pass is unavailable.
fn material_element(
    p: &mut SceneParams<'_>,
    key: ElementKey,
    material: &str,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
    stage: MaterialStage,
) -> Option<MaterialElement> {
    let dark = p.config.is_dark();
    let appearance = p
        .config
        .appearance
        .with_high_contrast(p.display.increase_contrast);
    let (shared, recipe) = (
        p.res.material.clone()?,
        p.res
            .recipe(material, appearance, p.display.reduce_transparency)?,
    );
    if !shared.borrow().is_ready() {
        return None;
    }
    Some(MaterialElement::new(
        key,
        stage,
        p.cache,
        area,
        recipe,
        shared,
        radius,
        true,
        alpha,
        p.overlays
            .wallpaper
            .tint
            .unwrap_or_else(|| wallpaper_tint(dark)),
        p.output_px,
        p.scale,
    ))
}

/// Translucent fill standing in for a material.
fn fallback_fill(
    p: &mut SceneParams<'_>,
    key: ElementKey,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
) -> ShaderRect {
    let fill = if p.config.is_dark() {
        [0.12, 0.12, 0.14, 0.78]
    } else {
        [0.96, 0.96, 0.98, 0.78]
    };
    rrect(p, key, area, radius, true, fill, None, alpha)
}

fn snap_preview(p: &mut SceneParams<'_>, stack: &mut LayerStack<CupertinoElement>) {
    let Some(prev) = p.wm.snap_preview(p.now) else {
        return;
    };
    if prev.opacity <= 0.0 {
        return;
    }
    let r = prev.rect;
    let rect = local(r, &p.rect);
    let e = rrect(
        p,
        ElementKey::new("snap-preview", 0),
        rect,
        12.0,
        true,
        [0.30, 0.52, 1.0, 0.22],
        Some((2.0, [0.45, 0.65, 1.0, 0.7])),
        prev.opacity as f32,
    );
    stack.push(SceneLayer::SnapPreview, e.into());
}

fn cursor(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    stack: &mut LayerStack<CupertinoElement>,
) {
    let scale = Scale::from(p.scale);
    let pos = p.pointer - Point::from((p.rect.x, p.rect.y));
    match std::mem::replace(&mut p.cursor, CursorPlan::Hidden) {
        CursorPlan::Hidden => {}
        CursorPlan::Themed { frame, size } => {
            // logical points per image pixel
            let k = size / f64::from(frame.nominal);
            let hotspot: Point<f64, Logical> = Point::from((
                f64::from(frame.hotspot.x) * k,
                f64::from(frame.hotspot.y) * k,
            ));
            let loc = (pos - hotspot).to_physical(scale);
            let (w, h) = frame.pixels;
            let drawn = Size::<i32, Logical>::from((
                (f64::from(w) * k).round() as i32,
                (f64::from(h) * k).round() as i32,
            ));
            // the whole image: without `src` the element samples a `drawn`-sized corner of it
            let src = smithay::utils::Rectangle::<f64, Logical>::from_size(
                (f64::from(w), f64::from(h)).into(),
            );
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc,
                &frame.buffer,
                None,
                Some(src),
                Some(drawn),
                Kind::Cursor,
            ) {
                stack.push(SceneLayer::Cursor, e.into());
            }
        }
        CursorPlan::Surface {
            surface,
            hotspot,
            scale: grow,
        } => {
            let loc: Point<i32, Physical> = (pos - hotspot.to_f64().upscale(grow))
                .to_physical(scale)
                .to_i32_round();
            let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    &surface,
                    loc,
                    scale * grow,
                    1.0,
                    Kind::Cursor,
                );
            stack.extend(
                SceneLayer::Cursor,
                els.into_iter().map(CupertinoElement::from),
            );
        }
    }
}

fn overview(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    stack: &mut LayerStack<CupertinoElement>,
) -> bool {
    let Some(session) = p
        .shell
        .overview
        .as_ref()
        .filter(|s| s.output == p.output_id)
    else {
        return false;
    };
    let now = p.now;
    let progress = session.transition.progress_at(now).clamp(0.0, 1.0);
    let placed = session.frames(now);
    let dim = p.config.shell.overview_dim;

    let full = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (p.rect.w, p.rect.h).into(),
    );
    let e = rrect(
        p,
        ElementKey::new("overview-dim", 0),
        full,
        0.0,
        false,
        [0.0, 0.0, 0.0, (dim * progress) as f32],
        None,
        1.0,
    );
    stack.push(SceneLayer::OverviewBackdrop, e.into());

    // windows, back to front in stacking order, drawn at their frames; the workspaces bar, the
    // highlight and the titles are the dock's layer surface
    let order: Vec<WindowId> = p.wm.stacking_order().to_vec();
    let key = p.wm.key_window();
    let mut windows_front_to_back: Vec<CupertinoElement> = Vec::new();
    for id in order.iter().rev() {
        let Some(pw) = placed.iter().find(|w| w.id == *id) else {
            continue;
        };
        let Some(entry) = p.windows.iter().find(|w| w.id == *id) else {
            continue;
        };
        let geo = entry.window.geometry();
        let ins = entry.insets;
        let frame = pw.frame;
        let plan = WindowPlan {
            visual: frame,
            scale_x: frame.w / (f64::from(geo.size.w) + ins.horizontal()).max(1.0),
            scale_y: frame.h / (f64::from(geo.size.h) + ins.vertical()).max(1.0),
            opacity: 1.0,
            shadow: 1.0,
            exact: false,
            corner: None,
        };
        let active = key == Some(*id);
        let deco = window_decorations(renderer, p, entry, &plan, active);
        let els = window_elements(renderer, p, entry, &plan, active, true, deco);
        windows_front_to_back.extend(els);
    }
    stack.extend(SceneLayer::Windows, windows_front_to_back);
    true
}

thread_local! {
    static LAST: RefCell<Vec<CupertinoElement>> = const { RefCell::new(Vec::new()) };
    static CACHES: RefCell<HashMap<OutputId, ElementCache>> = RefCell::new(HashMap::new());
}

/// Forget the oracle's caches and recorded frame, when a new compositor starts on this thread.
pub fn reset() {
    CACHES.with(|c| c.borrow_mut().clear());
    LAST.with(|last| last.borrow_mut().clear());
}

/// Keep the oracle's list of the frame just built, for [`take_recorded`].
pub fn record(elements: Vec<CupertinoElement>) {
    LAST.with(|last| *last.borrow_mut() = elements);
}

/// The oracle's list of the last frame built on this thread.
pub fn take_recorded() -> Vec<CupertinoElement> {
    LAST.with(|last| std::mem::take(&mut *last.borrow_mut()))
}

/// Build with element caches of its own, so the oracle and the migrated builder never share ids
/// or commit counters.
pub fn build_scene(renderer: &mut GlesRenderer, mut p: SceneParams<'_>) -> Vec<CupertinoElement> {
    let id = p.output_id;
    let mut own = CACHES
        .with(|c| c.borrow_mut().remove(&id))
        .unwrap_or_default();
    std::mem::swap(p.cache, &mut own);
    let elements = build(renderer, &mut p);
    std::mem::swap(p.cache, &mut own);
    CACHES.with(|c| c.borrow_mut().insert(id, own));
    elements
}

/// Build the element list of one output, front to back.
fn build(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>) -> Vec<CupertinoElement> {
    let mut stack: LayerStack<CupertinoElement> = LayerStack::new();
    if let LockView::Locked { surface } = std::mem::replace(&mut p.lock, LockView::Unlocked) {
        return locked_scene(renderer, p, surface);
    }

    let wp = wallpaper_element(renderer, p);
    stack.push(SceneLayer::Wallpaper, wp);

    let mut captures: Vec<CupertinoElement> = Vec::new();
    for (layer, scene_layer) in [
        (Layer::Background, SceneLayer::LayerBackground),
        (Layer::Bottom, SceneLayer::LayerBottom),
        (Layer::Top, SceneLayer::LayerTop),
        (Layer::Overlay, SceneLayer::LayerOverlay),
    ] {
        let els = layer_elements(renderer, p, layer, &mut captures);
        stack.extend(scene_layer, els);
    }
    stack.extend(SceneLayer::BackdropCapture, captures);

    let in_overview = overview(renderer, p, &mut stack);
    if !in_overview {
        let width = p.rect.w;
        let list = windows_on_output(
            p.wm,
            p.output_id,
            width,
            WorkspaceTransition::for_options(p.display),
            p.now,
        );
        let key = p.wm.key_window();
        let mut front_to_back: Vec<CupertinoElement> = Vec::new();
        // strip thumbnails, drawn below every window on the stage
        let mut strip: Vec<CupertinoElement> = Vec::new();
        for (id, slide, workspace_opacity) in list.into_iter().rev() {
            let Some(entry) = p.windows.iter().find(|w| w.id == id) else {
                continue;
            };
            let Some((mut plan, pres)) = plan_for(p.wm, entry, slide, p.now, p.fx, p.display)
            else {
                continue;
            };
            plan.opacity *= workspace_opacity;
            // a stage placement draws the window at its animated frame, alpha and corner radius
            let in_strip = match p.window_managers.look(id, p.now) {
                Look::Natural => false,
                Look::Hidden => continue,
                Look::Placed {
                    rect,
                    strip,
                    alpha,
                    rounding,
                } => {
                    let corner = p
                        .window_managers
                        .corner_radius(p.config.window.corner_radius, rounding);
                    plan = placed_plan(entry, rect, plan.opacity * alpha, plan.shadow, corner);
                    strip
                }
            };
            if plan.opacity <= 0.0
                || plan.visual.right() < p.rect.x
                || plan.visual.x > p.rect.right()
            {
                continue;
            }
            let active = key == Some(id);
            if in_strip {
                let deco = window_decorations(renderer, p, entry, &plan, active);
                strip.extend(window_elements(
                    renderer, p, entry, &plan, active, true, deco,
                ));
                continue;
            }
            if pres.minimize_progress > 1e-6 {
                if let Some(els) = minimize_elements(renderer, p, entry, &pres, slide, active) {
                    front_to_back.extend(els);
                    continue;
                }
            }
            // a window with a sheet is covered by the sheet-underlay material, as far as the sheet is
            // shown; the material samples the window below it rather than the desktop
            // behind it (approximate: the design blends behind the window)
            let dimming = p.wm.sheet_dimming(id, p.now);
            if dimming > 0.0 {
                let area = local(plan.visual, &p.rect);
                let radius = p.config.window.corner_radius;
                front_to_back.extend(material_layer(
                    p,
                    ElementKey::new("sheet-dimming", id.0),
                    SHEET_DIMMING_MATERIAL,
                    area,
                    radius,
                    (dimming * plan.opacity) as f32,
                ));
            }
            let deco = window_decorations(renderer, p, entry, &plan, active);
            front_to_back.extend(window_elements(
                renderer, p, entry, &plan, active, true, deco,
            ));
        }
        let mut ghosts = closing_elements(p);
        ghosts.append(&mut front_to_back);
        ghosts.append(&mut strip);
        stack.extend(SceneLayer::Windows, ghosts);
    }

    snap_preview(p, &mut stack);
    stack.extend(SceneLayer::InputHud, crate::kbd_ui::input_hud(renderer, p));
    stack.extend(
        SceneLayer::AccentPopup,
        crate::kbd_ui::accent_popup(renderer, p),
    );

    if let Some((surface, offset)) = p.drag_icon.clone() {
        let scale = Scale::from(p.scale);
        let pos = p.pointer - Point::from((p.rect.x, p.rect.y)) + offset.to_f64();
        let loc: Point<i32, Physical> = pos.to_physical(scale).to_i32_round();
        let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
            renderer,
            &surface,
            loc,
            scale,
            1.0,
            Kind::Unspecified,
        );
        stack.extend(
            SceneLayer::DragIcon,
            els.into_iter().map(CupertinoElement::from),
        );
    }
    display_filter(p, &mut stack);
    cursor(renderer, p, &mut stack);

    let mut ordered = stack.flatten();
    finalize_materials(&mut ordered, p.scale, p.cache);
    p.cache.end_frame();
    p.decor.end_frame();
    p.overlays.images.end_frame();
    ordered
}

/// The scene while the session is locked: black, the lock surface and the pointer; nothing else may show.
fn locked_scene(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    surface: Option<WlSurface>,
) -> Vec<CupertinoElement> {
    let mut stack: LayerStack<CupertinoElement> = LayerStack::new();
    let backdrop = lock_backdrop(p);
    stack.push(SceneLayer::Wallpaper, backdrop.into());
    if let Some(surface) = surface {
        let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
            renderer,
            &surface,
            (0, 0),
            Scale::from(p.scale),
            1.0,
            Kind::Unspecified,
        );
        stack.extend(
            SceneLayer::LayerOverlay,
            els.into_iter().map(CupertinoElement::from),
        );
    }
    display_filter(p, &mut stack);
    cursor(renderer, p, &mut stack);
    let mut ordered = stack.flatten();
    finalize_materials(&mut ordered, p.scale, p.cache);
    p.cache.end_frame();
    p.decor.end_frame();
    p.overlays.images.end_frame();
    ordered
}

/// The display filter pass: everything below the pointer, captured and passed through the colour
/// chain of the accessibility settings. The design filters the pointer too (the whole display,
/// ); it stays above here so that pointer motion does not repaint the output.
fn display_filter(p: &mut SceneParams<'_>, stack: &mut LayerStack<CupertinoElement>) {
    let (Some(recipe), Some(shared)) = (p.display_filter.clone(), p.res.material.clone()) else {
        return;
    };
    if !shared.borrow().is_ready() {
        return;
    }
    let area = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (p.rect.w, p.rect.h).into(),
    );
    let e = MaterialElement::new(
        ElementKey::new("display-filter", 0),
        MaterialStage::Full,
        p.cache,
        area,
        recipe,
        shared,
        0.0,
        false,
        1.0,
        [0.0; 3],
        p.output_px,
        p.scale,
    );
    stack.push(SceneLayer::DisplayFilter, e.into());
}

/// Assign each material element the signature of everything below it (bottom to top). A
/// composite takes the signature of its capture element, which sits lower in the scene.
pub fn finalize_materials(elements: &mut [CupertinoElement], scale: f64, cache: &mut ElementCache) {
    let scale = Scale::from(scale);
    let mut running = Sig::new();
    let mut acc: u64 = 0;
    let mut captured: Vec<(u64, u64)> = Vec::new();
    for e in elements.iter_mut().rev() {
        if let CupertinoElement::Material(m) = e {
            let generation = match m.stage() {
                MaterialStage::Full => acc,
                MaterialStage::Capture { owner } => {
                    captured.push((owner, acc));
                    acc
                }
                MaterialStage::Composite { owner } => {
                    captured
                        .iter()
                        .find(|(o, _)| *o == owner)
                        .expect("a composite is built together with its capture")
                        .1
                }
            };
            m.finalize(generation, cache);
        }
        let g = e.geometry(scale);
        let c = e
            .current_commit()
            .distance(Some(CommitCounter::default()))
            .unwrap_or(0) as u64;
        // order-dependent combination of (id, commit, geometry)
        let mut h = Sig::new()
            .u(acc)
            .u(c)
            .u(g.loc.x as u64)
            .u(g.loc.y as u64)
            .u(g.size.w as u64)
            .u(g.size.h as u64);
        h = h.u(id_hash(e.id()));
        acc = h.finish();
        let _ = &mut running;
    }
}

fn id_hash(id: &smithay::backend::renderer::element::Id) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}
