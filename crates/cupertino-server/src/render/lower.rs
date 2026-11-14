//! Lowering of a composed [`SceneFrame`] to the elements of the GLES damage tracker: one element per item,
//! front to back, with the cache slots and commit counters the renderer has always used.

use cupertino_scene::{
    Clip, ImageRole, OutputPoint, RoundedRect, SceneFrame, SceneItem, Shadow, Source, SurfaceRole,
};
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};

use super::clip::{ClipElement, ClipShape};
use super::compose::{logical_rect, FrameResources, TreeSlice};
use super::material::glass::GlassElement;
use super::material::{MaterialElement, MaterialMask, SharedMaterial};
use super::mesh::{MeshElement, SharedMesh};
use super::passes::RectPass;
use super::programs::{corner_uniforms, Programs};
use super::reduce::{self, ReducedElement};
use super::shader_rect::{to_physical_rect, ElementCache, ElementKey, ShaderRect, Sig};
use super::snapshots::Snapshots;
use super::ui::{rounded_rect, RoundedRectSpec};
use super::CupertinoElement;

/// What lowering draws with: the renderer, shader programs, the element cache of the output and its scale.
pub struct Lowering<'a> {
    pub renderer: &'a mut GlesRenderer,
    pub programs: &'a Programs,
    pub material: Option<SharedMaterial>,
    pub mesh: Option<SharedMesh>,
    pub snapshots: Snapshots,
    pub cache: &'a mut ElementCache,
    pub scale: f64,
}

/// The elements of a composed frame front to back.
pub fn lower(
    frame: &SceneFrame,
    resources: FrameResources,
    ctx: &mut Lowering<'_>,
) -> Vec<CupertinoElement> {
    assert_eq!(resources.frame, frame.id, "resources of another frame");
    let FrameResources {
        surfaces,
        slices,
        images,
        recipes,
        glass,
        mut snapshots,
        ..
    } = resources;
    let viewport = (
        frame.output.physical_size.width,
        frame.output.physical_size.height,
    );
    let mut elements = Vec::with_capacity(frame.items().len());
    for (_, item) in frame.items() {
        let key = ElementKey::new(item.id.kind.key(), item.id.index);
        match &item.source {
            Source::SurfaceTree { id, role } => {
                let surface = surfaces
                    .get(id.0.slot as usize)
                    .filter(|_| id.0.generation == frame.id.0);
                match surface {
                    Some(surface) => {
                        let only = slices.get(id.0.slot as usize).cloned().flatten();
                        surface_tree(ctx, surface, only, *role, item, viewport, &mut elements)
                    }
                    None => {
                        debug_assert!(false, "surface handle {id:?} does not belong to the frame");
                        tracing::error!(?id, "scene surface handle unknown, item skipped");
                    }
                }
            }
            Source::Image {
                id,
                mapping,
                role,
                size,
            } => {
                let buffer = images
                    .get(id.0.slot as usize)
                    .filter(|_| id.0.generation == frame.id.0);
                match (buffer, mapping.crop) {
                    (Some(buffer), Some(crop)) => {
                        let src = Rectangle::<f64, Logical>::new(
                            (crop.0.x(), crop.0.y()).into(),
                            (crop.0.width(), crop.0.height()).into(),
                        );
                        picture(
                            ctx,
                            buffer,
                            *role,
                            item,
                            src,
                            *size,
                            viewport,
                            &mut elements,
                        );
                    }
                    _ => {
                        debug_assert!(false, "image {id:?} is unknown to the frame or has no crop");
                        tracing::error!(?id, "scene image unknown, item skipped");
                    }
                }
            }
            Source::Material(pass) => {
                let recipe = recipes
                    .get(pass.recipe.0.slot as usize)
                    .filter(|_| pass.recipe.0.generation == frame.id.0);
                match (recipe, ctx.material.clone()) {
                    (Some(recipe), Some(shared)) => {
                        let element = MaterialElement::new(
                            key,
                            pass.stage,
                            ctx.cache,
                            logical_rect(pass.rect),
                            recipe.clone(),
                            shared,
                            pass.radius,
                            !pass.style.is_circular(),
                            item.opacity as f32,
                            pass.tint,
                            viewport,
                            ctx.scale,
                        );
                        // a region cannot be drawn apart and clipped (it reads what is drawn below it): a rounded
                        // clip becomes its mask
                        let element = match item.clip {
                            Clip::Rounded {
                                rect,
                                radius,
                                style,
                                ..
                            } => element.masked(MaterialMask {
                                rect: logical_rect(rect),
                                radius_pt: radius,
                                continuous: !style.is_circular(),
                            }),
                            _ => element,
                        };
                        elements.push(element.into());
                    }
                    _ => {
                        debug_assert!(
                            false,
                            "material recipe unknown or material pass unavailable"
                        );
                        tracing::error!(recipe = ?pass.recipe, "scene material unavailable, item skipped");
                    }
                }
            }
            Source::Glass(pass) => {
                let container = glass
                    .get(pass.glass.0.slot as usize)
                    .filter(|_| pass.glass.0.generation == frame.id.0);
                match (container, ctx.material.clone()) {
                    (Some(container), Some(shared)) => elements.push(
                        GlassElement::new(
                            key,
                            ctx.cache,
                            logical_rect(pass.rect),
                            container.clone(),
                            shared,
                            item.opacity as f32,
                            ctx.scale,
                        )
                        .into(),
                    ),
                    _ => {
                        debug_assert!(
                            false,
                            "glass container unknown or material pass unavailable"
                        );
                        tracing::error!(glass = ?pass.glass, "scene glass unavailable, item skipped");
                    }
                }
            }
            Source::TexturedMesh { snapshot, mesh } => {
                let taken = snapshots
                    .get_mut(snapshot.0.slot as usize)
                    .filter(|_| snapshot.0.generation == frame.id.0)
                    .and_then(Option::take);
                let leased = taken.map(|t| ctx.snapshots.lease(t.id));
                match (leased, ctx.mesh.clone()) {
                    (Some(Ok((texture, lease))), Some(shared)) => {
                        let (id, commit) = ctx
                            .cache
                            .slot(key, mesh_signature(mesh, item.opacity, item.revision));
                        elements.push(
                            MeshElement::new(
                                id,
                                commit,
                                texture,
                                shared,
                                mesh,
                                ctx.scale,
                                item.opacity as f32,
                                Some(lease),
                            )
                            .into(),
                        );
                    }
                    (Some(Err(error)), _) => {
                        tracing::error!(%error, ?snapshot, "scene snapshot is retired, item skipped");
                    }
                    _ => {
                        debug_assert!(false, "snapshot {snapshot:?} is unknown, drawn twice or the mesh pass is unavailable");
                        tracing::error!(?snapshot, "scene snapshot unavailable, item skipped");
                    }
                }
            }
            Source::RoundedRect(rect) => {
                elements.push(rounded(ctx, key, rect, item.opacity as f32).into());
            }
            Source::Shadow(shadow) => {
                elements.push(
                    shadow_rect(
                        ctx.programs,
                        ctx.cache,
                        ctx.scale,
                        key,
                        shadow,
                        item.opacity,
                    )
                    .into(),
                );
            }
            Source::Wallpaper { rect, colours } => {
                elements
                    .push(wallpaper(ctx, key, logical_rect(*rect), colours, item.opacity).into());
            }
            other => {
                debug_assert!(false, "composition does not emit {other:?}");
                tracing::error!(source = ?other, "scene source not lowerable, item skipped");
            }
        }
    }
    ctx.programs.reduced.end_lowering();
    elements
}

fn rounded(ctx: &mut Lowering<'_>, key: ElementKey, rect: &RoundedRect, alpha: f32) -> ShaderRect {
    let spec = RoundedRectSpec {
        rect: logical_rect(rect.rect),
        radius: rect.radius,
        continuous: !rect.style.is_circular(),
        fill: rect.fill,
        border: rect.border,
        border_falloff: rect.border_falloff,
        alpha,
    };
    let shape = rounded_rect(ctx.programs, ctx.cache, ctx.scale, key, &spec);
    match rect.border {
        // a ring: nothing inside its border, the corners' curves but half the radius in
        Some((width, _)) if rect.fill[3] <= 0.0 => {
            let r = logical_rect(rect.rect);
            let inset = width + rect.radius * rect.style.expansion() * 0.5;
            shape.with_hole(smithay::utils::Rectangle::new(
                (r.loc.x + inset, r.loc.y + inset).into(),
                (
                    (r.size.w - 2.0 * inset).max(0.0),
                    (r.size.h - 2.0 * inset).max(0.0),
                )
                    .into(),
            ))
        }
        _ => shape,
    }
}

pub fn shadow_rect(
    programs: &Programs,
    cache: &mut ElementCache,
    scale: f64,
    key: ElementKey,
    shadow: &Shadow,
    alpha: f64,
) -> ShaderRect {
    let area = logical_rect(shadow.area);
    let caster = [
        (shadow.caster.0.x() - shadow.area.0.x()) as f32,
        (shadow.caster.0.y() - shadow.area.0.y()) as f32,
        shadow.caster.0.width() as f32,
        shadow.caster.0.height() as f32,
    ];
    let extent = shadow.radius * shadow.style.expansion();
    // the block of the shadow pass: the caster, then radius, sigma, density and offset, then the cut-out, the
    // corner exponent, pixels per point and the corner extent
    let params = [
        caster,
        [
            shadow.radius as f32,
            shadow.sigma as f32,
            shadow.density as f32,
            shadow.offset_y as f32,
        ],
        [
            shadow.cutout as u8 as f32,
            shadow.style.exponent() as f32,
            scale as f32,
            extent as f32,
        ],
    ];
    let sig = Sig::new()
        .rect(area)
        .f(shadow.radius)
        .f(shadow.sigma)
        .f(shadow.density)
        .f(shadow.offset_y)
        .f(alpha)
        .f(scale)
        .u(shadow.cutout as u64)
        .finish();
    let (id, commit) = cache.slot(key, sig);
    let rect = ShaderRect::pass(
        &programs.passes,
        RectPass::Shadow,
        id,
        commit,
        area,
        alpha as f32,
        &params,
        false,
    );
    if !shadow.cutout {
        return rect;
    }
    // cut out under the window's shape: nothing drawn inside it, the corners' curves but half their extent in
    let window = logical_rect(shadow.caster);
    let inset = extent * 0.5;
    rect.with_hole(smithay::utils::Rectangle::new(
        (window.loc.x + inset, window.loc.y + inset).into(),
        (
            (window.size.w - 2.0 * inset).max(0.0),
            (window.size.h - 2.0 * inset).max(0.0),
        )
            .into(),
    ))
}

fn wallpaper(
    ctx: &mut Lowering<'_>,
    key: ElementKey,
    area: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
    colours: &[[f32; 4]; 4],
    opacity: f64,
) -> ShaderRect {
    let mut sig = Sig::new().rect(area).f(opacity);
    for v in colours.iter().flatten() {
        sig = sig.f(f64::from(*v));
    }
    let (id, commit) = ctx.cache.slot(key, sig.finish());
    ShaderRect::pass(
        &ctx.programs.passes,
        RectPass::Wallpaper,
        id,
        commit,
        area,
        opacity as f32,
        colours,
        opacity >= 1.0,
    )
}

fn physical(point: OutputPoint, scale: f64) -> Point<i32, Physical> {
    Point::<f64, Logical>::from((point.0.x, point.0.y)).to_physical_precise_round(scale)
}

/// The elements of the tree of `surface`, front to back; with `only`, those of the slice's surfaces alone.
fn surface_elements(
    ctx: &mut Lowering<'_>,
    surface: &WlSurface,
    only: Option<&TreeSlice>,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
    kind: Kind,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let mut els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(ctx.renderer, surface, origin, scale, alpha, kind);
    if let Some(only) = only {
        els.retain(|e| only.ids.contains(e.id()));
    }
    els
}

fn surface_tree(
    ctx: &mut Lowering<'_>,
    surface: &WlSurface,
    only: Option<TreeSlice>,
    role: SurfaceRole,
    item: &SceneItem,
    viewport: (i32, i32),
    out: &mut Vec<CupertinoElement>,
) {
    let place = item.placement;
    let origin = physical(place.origin, ctx.scale);
    let alpha = item.opacity as f32;
    let scale = Scale::from(ctx.scale);
    match role {
        SurfaceRole::Standalone => {
            let els = surface_elements(
                ctx,
                surface,
                only.as_ref(),
                origin,
                scale,
                alpha,
                Kind::Unspecified,
            );
            out.extend(els.into_iter().map(CupertinoElement::from));
        }
        SurfaceRole::Cursor => {
            let els = surface_elements(
                ctx,
                surface,
                only.as_ref(),
                origin,
                scale * place.scale_x,
                alpha,
                Kind::Cursor,
            );
            out.extend(els.into_iter().map(CupertinoElement::from));
        }
        SurfaceRole::WindowContent | SurfaceRole::Popup => {
            let els = surface_elements(
                ctx,
                surface,
                only.as_ref(),
                origin,
                scale,
                alpha,
                Kind::Unspecified,
            );
            let pivot = physical(place.pivot, ctx.scale);
            let rescale = Scale::from((place.scale_x, place.scale_y));
            let shape = match role {
                SurfaceRole::WindowContent => mask(ctx, item.clip, viewport),
                _ => None,
            };
            // a surface drawn at half its size or less is drawn from a reduced copy, made at full opacity
            let halvings = reduce::halvings(place.scale_x.min(place.scale_y));
            let mut opaque: Option<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> = None;
            for e in els {
                let reduced = if halvings == 0 {
                    None
                } else {
                    let g = e.geometry(scale);
                    let size = reduce::copy_size((g.size.w, g.size.h), halvings);
                    let reductions = ctx.programs.reduced.clone();
                    reductions.kept(&e, size).or_else(|| {
                        if e.alpha() >= 1.0 {
                            return reductions.make(
                                ctx.renderer,
                                &ctx.programs.clip_minify,
                                &e,
                                size,
                            );
                        }
                        let all = opaque.get_or_insert_with(|| {
                            surface_elements(
                                ctx,
                                surface,
                                only.as_ref(),
                                origin,
                                scale,
                                1.0,
                                Kind::Unspecified,
                            )
                        });
                        let full = all.iter().find(|f| f.id() == e.id())?;
                        reductions.make(ctx.renderer, &ctx.programs.clip_minify, full, size)
                    })
                };
                let e = match reduced {
                    Some((copy, of)) => ReducedElement::reduced(e, copy, of),
                    None => ReducedElement::direct(e),
                };
                match shape {
                    Some(shape) => {
                        let clip =
                            clipped(ctx, e, shape).minified(ctx.programs.clip_minify.clone());
                        out.push(RescaleRenderElement::from_element(clip, pivot, rescale).into());
                    }
                    None => {
                        let e = e.minified(ctx.programs.clip_minify.clone());
                        out.push(RescaleRenderElement::from_element(e, pivot, rescale).into())
                    }
                }
            }
        }
        SurfaceRole::Vibrant(blend) => {
            let els = surface_elements(
                ctx,
                surface,
                only.as_ref(),
                origin,
                scale,
                alpha,
                Kind::Unspecified,
            );
            let pivot = physical(place.pivot, ctx.scale);
            let rescale = Scale::from((place.scale_x, place.scale_y));
            // unclipped, the mask is the whole framebuffer and more
            let shape = mask(ctx, item.clip, viewport).unwrap_or(ClipShape {
                rect: Rectangle::new((-(1 << 20), -(1 << 20)).into(), (1 << 21, 1 << 21).into()),
                extent: 0.0,
                exponent: 2.0,
                viewport,
                exact: false,
            });
            let shape = ClipShape {
                exact: false,
                ..shape
            };
            for e in els {
                let e = ReducedElement::direct(e);
                let clip = clipped(ctx, e, shape).vibrant(ctx.programs.vibrant.clone(), blend);
                out.push(RescaleRenderElement::from_element(clip, pivot, rescale).into());
            }
        }
    }
}

/// `e` masked by `shape`. The shape's generation comes from the element cache, so a change of the
/// mask alone (an animated corner radius) damages the element.
pub fn clipped<E: Element>(ctx: &mut Lowering<'_>, e: E, shape: ClipShape) -> ClipElement<E> {
    let r = shape.rect;
    let sig = Sig::new()
        .f(f64::from(shape.extent))
        .f(f64::from(shape.exponent))
        .f(f64::from(r.loc.x))
        .f(f64::from(r.loc.y))
        .f(f64::from(r.size.w))
        .f(f64::from(r.size.h))
        .finish();
    let (_, generation) = ctx
        .cache
        .slot(ElementKey::new("clip", id_hash(e.id())), sig);
    ClipElement::new(e, ctx.programs.clip.clone(), shape, generation)
}

pub(super) fn id_hash(id: &smithay::backend::renderer::element::Id) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

#[allow(clippy::too_many_arguments)]
fn picture(
    ctx: &mut Lowering<'_>,
    buffer: &MemoryRenderBuffer,
    role: ImageRole,
    item: &SceneItem,
    src: Rectangle<f64, Logical>,
    size: (i32, i32),
    viewport: (i32, i32),
    out: &mut Vec<CupertinoElement>,
) {
    let place = item.placement;
    let loc =
        Point::<f64, Physical>::from((place.origin.0.x * ctx.scale, place.origin.0.y * ctx.scale));
    let kind = match role {
        ImageRole::Cursor => Kind::Cursor,
        ImageRole::Standalone | ImageRole::Decoration => Kind::Unspecified,
    };
    // the renderer counts what it uploads; an image it already holds costs nothing
    let before = ctx.renderer.memory_uploads();
    let element = MemoryRenderBufferRenderElement::from_buffer(
        ctx.renderer,
        loc,
        buffer,
        Some(item.opacity as f32),
        Some(src),
        Some(Size::from(size)),
        kind,
    );
    let after = ctx.renderer.memory_uploads();
    if after != before {
        super::uploads::note(
            item.id.kind,
            after.uploads - before.uploads,
            after.bytes - before.bytes,
        );
    }
    let Ok(element) = element else {
        return;
    };
    match role {
        ImageRole::Standalone | ImageRole::Cursor => out.push(element.into()),
        ImageRole::Decoration => {
            let pivot = physical(place.pivot, ctx.scale);
            let rescale = Scale::from((place.scale_x, place.scale_y));
            match mask(ctx, item.clip, viewport) {
                Some(shape) => {
                    let clip = clipped(ctx, element, shape);
                    out.push(RescaleRenderElement::from_element(clip, pivot, rescale).into());
                }
                None => {
                    out.push(RescaleRenderElement::from_element(element, pivot, rescale).into())
                }
            }
        }
    }
}

/// The device-pixel mask of a rounded clip.
fn mask(ctx: &Lowering<'_>, clip: Clip, viewport: (i32, i32)) -> Option<ClipShape> {
    let Clip::Rounded {
        rect,
        radius,
        style,
        exact,
    } = clip
    else {
        return None;
    };
    let (extent, exponent) = corner_uniforms(radius, !style.is_circular(), ctx.scale);
    Some(ClipShape {
        rect: to_physical_rect(logical_rect(rect), Scale::from(ctx.scale)),
        extent,
        exponent,
        viewport,
        exact,
    })
}

/// A signature of the mesh, its opacity and revision: the commit advances whenever any changes.
fn mesh_signature(mesh: &cupertino_effects::Mesh, opacity: f64, revision: u64) -> u64 {
    let mut sig = Sig::new()
        .f(opacity)
        .u(revision)
        .u(mesh.vertices.len() as u64);
    for v in &mesh.vertices {
        for c in v.position.iter().chain(v.uv.iter()) {
            sig = sig.f(f64::from(*c));
        }
    }
    sig.finish()
}
