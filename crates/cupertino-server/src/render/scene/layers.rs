use super::compose::*;
use super::*;

/// What a window snapshot shows where the window's materials are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotBackdrop {
    /// A translucent fill per outline stands in for its materials: nothing behind the window is kept, so the picture
    /// can move and scale on its own.
    Fills,
    /// The materials themselves, each over an opaque ground of the wallpaper's average colour laid inside the window's
    /// outline: every material keeps its own colour and its edges stay where they are drawn, so the picture matches the
    /// window as it shows and can fade over the window's own redrawn picture.
    Ground,
}

/// Render a window (decorations, content, optionally its shadow) at its natural size into a new
/// texture covering `natural` grown by `margin` points. Returns the texture and the captured
/// rectangle (global logical points).
#[allow(clippy::too_many_arguments)]
pub fn capture_window(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    natural: Rect,
    active: bool,
    margin: f64,
    shadow: bool,
    backdrop: SnapshotBackdrop,
) -> Option<(smithay::backend::renderer::gles::GlesTexture, Rect)> {
    if p.for_capture && crate::control::protected_surface(entry.surface()) {
        return None;
    }
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
    let plan = WindowPlan {
        visual: natural,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
        corner_cap: None,
    };
    let deco = window_decorations(p, entry, &plan, active);
    // the snapshot holds the window as it shows, its popups over it
    let mut pieces = window_popup_elements(p, entry, &plan);
    pieces.extend(window_elements(p, entry, &plan, active, shadow, deco));
    // a material samples what lies below it and keeps the target's alpha, so on the clear snapshot it would leave
    // its area transparent: either the fallback fill stands in, or the window's ground is laid under it
    let pieces = match backdrop {
        SnapshotBackdrop::Fills => snapshot_fills(p, pieces),
        SnapshotBackdrop::Ground => {
            let mut pieces = pieces;
            pieces.push(snapshot_ground(p, entry, natural));
            pieces
        }
    };
    let mut stack = Composition::new(p.scale);
    stack.extend(SceneLayer::Windows, pieces);
    let composed = finish_composition(stack, p);
    p.rect = saved.0;
    p.output_px = saved.1;
    // the window is drawn alone, with caches of its own
    let mut cache = ElementCache::default();
    let mut resources = GlesResources::new(p.res, &mut cache, composed.resources);
    match Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Snapshot { renderer },
    ) {
        Ok(GlesCompletion::Snapshot(t)) => Some((t, area)),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "window snapshot failed");
            None
        }
    }
}

/// The opaque ground a [`SnapshotBackdrop::Ground`] snapshot lays under the window, back-most: the wallpaper's
/// average colour inside the window's outline at `natural` (global logical points), so the window's materials read
/// a backdrop as they do on screen and nothing outside the outline is covered.
fn snapshot_ground(p: &SceneParams<'_>, entry: &WindowEntry, natural: Rect) -> Piece {
    let style = p.config.window;
    let radius = if entry.styled(style.style_client_decorated) {
        style.corner_radius
    } else {
        0.0
    };
    let [r, g, b] = ground_colour(p.overlays.wallpaper.tint, p.config.is_dark());
    rrect_item(
        SceneKind::Backdrop,
        entry.id.0,
        local(natural, &p.rect),
        radius,
        style.continuous_corners,
        [r, g, b, 1.0],
        None,
        1.0,
    )
}

/// The colour of a snapshot's ground: the wallpaper's average (`measured`, when an image's was measured), else the
/// procedural wallpaper's.
fn ground_colour(measured: Option<[f32; 3]>, dark: bool) -> [f32; 3] {
    measured.unwrap_or_else(|| wallpaper_tint(dark))
}

/// The fills that stand in for the materials of a window snapshot, in `pieces`' order, each at its first material's
/// place. The materials cut to one rounded outline share one fill over the part of their bounds inside it, with the
/// outline's corners when that part spans it: a fill cannot be cut by the outline, and the square corners of a sidebar
/// and a content area would show past it.
fn snapshot_fills(p: &SceneParams<'_>, pieces: Vec<Piece>) -> Vec<Piece> {
    let key = |clip: &Clip| match clip {
        Clip::Rounded { rect, .. } => {
            Some([rect.0.x(), rect.0.y(), rect.0.width(), rect.0.height()].map(f64::to_bits))
        }
        _ => None,
    };
    // the bounds and opacity of the materials of each outline
    let mut groups: Vec<([u64; 4], Rect, f64, f32)> = Vec::new();
    for piece in &pieces {
        let Piece::Material(m) = piece else { continue };
        let (Some(k), Clip::Rounded { radius, .. }) = (key(&m.clip), m.clip) else {
            continue;
        };
        let area = Rect::new(m.rect.loc.x, m.rect.loc.y, m.rect.size.w, m.rect.size.h);
        match groups.iter_mut().find(|g| g.0 == k) {
            Some(g) => {
                let (l, t) = (g.1.x.min(area.x), g.1.y.min(area.y));
                let (r, b) = (
                    g.1.right().max(area.right()),
                    g.1.bottom().max(area.bottom()),
                );
                g.1 = Rect::new(l, t, r - l, b - t);
                g.3 = g.3.max(m.opacity);
            }
            None => groups.push((k, area, radius, m.opacity)),
        }
    }
    let mut drawn: Vec<[u64; 4]> = Vec::new();
    let mut out = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Material(m) => {
                let Some(k) = key(&m.clip) else {
                    out.push(fallback_fill(
                        p, m.kind, m.index, m.rect, m.radius, m.opacity,
                    ));
                    continue;
                };
                if drawn.contains(&k) {
                    continue;
                }
                drawn.push(k);
                let Some(&(_, bounds, radius, opacity)) = groups.iter().find(|g| g.0 == k) else {
                    continue;
                };
                let [x, y, w, h] = k.map(f64::from_bits);
                let clip = Rect::new(x, y, w, h);
                let (left, top) = (bounds.x.max(clip.x), bounds.y.max(clip.y));
                let (right, bottom) = (
                    bounds.right().min(clip.right()),
                    bounds.bottom().min(clip.bottom()),
                );
                if right <= left || bottom <= top {
                    continue;
                }
                let near = |a: f64, b: f64| (a - b).abs() < 0.5;
                let spans = near(left, clip.x)
                    && near(right, clip.right())
                    && near(top, clip.y)
                    && near(bottom, clip.bottom());
                let inside = smithay::utils::Rectangle::<f64, Logical>::new(
                    (left, top).into(),
                    (right - left, bottom - top).into(),
                );
                let radius = if spans { radius } else { m.radius };
                out.push(fallback_fill(p, m.kind, m.index, inside, radius, opacity));
            }
            Piece::Glass(g) => out.extend(glass_fallback(
                p,
                g.kind,
                g.index,
                &g.glass.container,
                g.opacity,
            )),
            piece => out.push(piece),
        }
    }
    out
}

/// Render the surface tree of `surface`, its surface's top-left corner at `origin` (global logical points), into a new
/// texture covering `area` (global logical points), as the output of `p` draws it: the icon of a drag captured as the
/// drag ends, before its client lets the surface go.
pub fn capture_surface(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    surface: &WlSurface,
    origin: Point<f64, Logical>,
    area: Rect,
) -> Option<smithay::backend::renderer::gles::GlesTexture> {
    let size_px = (
        ((area.w * p.scale).round() as i32).max(1),
        ((area.h * p.scale).round() as i32).max(1),
    );
    let saved = (p.rect, p.output_px);
    p.rect = area;
    p.output_px = size_px;
    let local: Point<f64, Logical> = (origin.x - area.x, origin.y - area.y).into();
    let loc: Point<i32, Physical> = local.to_physical(Scale::from(p.scale)).to_i32_round();
    let mut stack = Composition::new(p.scale);
    stack.push(
        SceneLayer::DragIcon,
        standalone(surface, SceneKind::DragIcon, loc, 1.0),
    );
    let composed = finish_composition(stack, p);
    p.rect = saved.0;
    p.output_px = saved.1;
    let mut cache = ElementCache::default();
    let mut resources = GlesResources::new(p.res, &mut cache, composed.resources);
    match Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Snapshot { renderer },
    ) {
        Ok(GlesCompletion::Snapshot(t)) => Some(t),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "drag icon snapshot failed");
            None
        }
    }
}

/// What a held persistent surface (D19) drew besides its pixels, in points from its top-left corner: the backdrops of
/// the surface, of its subsurfaces and of its layers (`cupertino_layers_v1`). They sample what lies behind the
/// surface, so the server goes on drawing them live under the held picture, with the live surface's piece builders.
#[derive(Clone, Default)]
pub struct HeldBackdrops {
    /// The surface's own backdrops (`cupertino_app_effects_v1`), front to back.
    surface: Vec<BackdropArea>,
    /// The glass of each of its subsurfaces and the rectangle it is drawn within.
    subsurfaces: Vec<(Vec<BackdropArea>, Rectangle<f64, Logical>)>,
    /// The glass of each of its layers, front to back, with the container and opacity the layer made of it (its lens,
    /// blur ramp, edge, shadow and light), when the glass pass drew it.
    layers: Vec<(Vec<BackdropArea>, Option<(Rc<GlassContainer>, f32)>)>,
}

impl HeldBackdrops {
    /// The backdrops of `surface` itself, for a picture taken without a renderer to draw its layers with.
    pub fn of_surface(surface: &WlSurface) -> Self {
        Self {
            surface: surface_backdrops(surface, Point::from((0.0, 0.0))),
            ..Self::default()
        }
    }
}

/// The kind of scene item `piece` makes.
fn piece_kind(piece: &Piece) -> SceneKind {
    match piece {
        Piece::Item(item) => item.id.kind,
        Piece::Tree(t) => t.kind,
        Piece::Image(i) => i.kind,
        Piece::Material(m) => m.kind,
        Piece::Glass(g) => g.kind,
        Piece::Mesh(m) => m.kind,
    }
}

/// Render the layer shell surface `surface` as its output draws it, its top-left corner at the corner of `area`
/// (global logical points), into a new texture covering `area`: its surface tree and its layers' content, for the
/// picture of its hold (D19). The backdrops they draw are left out of the texture and returned, to be drawn live under
/// it ([`HeldBackdrops`]); so are its holes, whose sources the server draws live behind the held picture.
pub fn capture_held_surface(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    surface: &WlSurface,
    area: Rect,
) -> Option<(smithay::backend::renderer::gles::GlesTexture, HeldBackdrops)> {
    let size_px = (
        ((area.w * p.scale).round() as i32).max(1),
        ((area.h * p.scale).round() as i32).max(1),
    );
    let saved = (p.rect, p.output_px, p.for_capture);
    p.rect = area;
    p.output_px = size_px;
    // a capture notes no hole's bounds, and a hole inside it is as deep as holes go, so it draws nothing
    p.for_capture = true;
    let nested = p.transactions.enter_hole();
    let origin = Point::<f64, Logical>::from((0.0, 0.0));
    let place = super::super::layers::LayerPlace {
        origin,
        pivot: origin,
        scale: (1.0, 1.0),
        opacity: 1.0,
        clip: Clip::None,
        kind: SceneKind::LayerContent,
    };
    let mut layer_glass: Vec<Vec<BackdropArea>> = Vec::new();
    let layers = super::super::layers::layer_pieces(p, surface, &place, |p, areas| {
        let pieces = backdrop_elements(p, &areas, 1.0, WindowState::Active);
        layer_glass.push(areas);
        pieces
    });
    let mut subsurfaces = Vec::new();
    let tree = standalone_tree(surface, SceneKind::LayerContent, (0, 0).into(), 1.0);
    let tree = tree_pieces(p, tree, origin, |_, areas, bound| {
        subsurfaces.push((areas, bound));
        Vec::new()
    });
    if nested {
        p.transactions.leave_hole();
    }
    let layers_made = layer_glass
        .into_iter()
        .map(|areas| {
            let key = areas.first().map(|a| a.key);
            let made = layers.iter().find_map(|piece| match piece {
                Piece::Glass(g) if Some(g.index) == key => Some((g.glass.clone(), g.opacity)),
                _ => None,
            });
            (areas, made)
        })
        .collect();
    let held = HeldBackdrops {
        surface: surface_backdrops(surface, origin),
        subsurfaces,
        layers: layers_made,
    };
    let mut stack = Composition::new(p.scale);
    stack.extend(
        SceneLayer::LayerTop,
        layers
            .into_iter()
            .chain(tree)
            .filter(|piece| piece_kind(piece) != SceneKind::Backdrop),
    );
    let composed = finish_composition(stack, p);
    (p.rect, p.output_px, p.for_capture) = saved;
    let mut cache = ElementCache::default();
    let mut resources = GlesResources::new(p.res, &mut cache, composed.resources);
    match Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Snapshot { renderer },
    ) {
        Ok(GlesCompletion::Snapshot(t)) => Some((t, held)),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "held surface snapshot failed");
            None
        }
    }
}

/// The icon of a client's drag that ended without a drop, flying back or fading out (`crate::drag_return`).
pub(super) fn drag_return_element(p: &mut SceneParams<'_>, stack: &mut Composition) {
    if p.res.mesh.is_none() {
        return;
    }
    let Some(r) = p.fx.drag_return.as_ref() else {
        return;
    };
    let (rect, alpha) = r.placement(p.now);
    if alpha <= 0.0 {
        return;
    }
    let local = Rect::new(rect.x - p.rect.x, rect.y - p.rect.y, rect.w, rect.h);
    let lease = match p.res.snapshots.pin(r.snapshot.id()) {
        Ok(lease) => lease,
        Err(error) => {
            tracing::error!(%error, "returning drag icon without its snapshot");
            return;
        }
    };
    stack.push(
        SceneLayer::DragIcon,
        Piece::Mesh(MeshPiece {
            kind: SceneKind::DragIcon,
            index: 0,
            snapshot: lease.id(),
            keep: SnapshotKeep::Pinned(lease),
            mesh: Mesh::quad(local.into_geom()),
            opacity: alpha as f32,
            revision: Sig::new().f(r.progress(p.now)).finish(),
        }),
    );
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

/// The layer-shell `top` surfaces of `output` that [`give_way`] do so to a full-screen window in front of it: the dock
/// and panels are neither drawn nor hit while it covers the output, except in the overview.
pub fn top_layer_covered(wm: &Wm, shell: &crate::shell::Shell, output: OutputId) -> bool {
    !shell.overview_open() && wm.fullscreen_front(output).is_some()
}

/// The window of `output` whose popups lie above every window and the bars ([`SceneLayer::Popups`]): the key window,
/// unless another window covers the output in full screen. The popups of every other window stay with their window, in
/// its place in the stack, so a popup left open in a window behind (a tooltip, a popup without a grab) never covers the
/// window in front of it nor takes its pointer.
pub fn raised_popups_window(wm: &Wm, output: OutputId) -> Option<WindowId> {
    let key = wm.key_window()?;
    match wm.fullscreen_front(output) {
        Some(front) if front != key => None,
        _ => Some(key),
    }
}

/// Whether the `top` surface gives way to a full-screen window: all but a menu bar and a surface over the whole output,
/// such as the outside-click catcher of the launcher or the control panel, which must still take the click that closes them.
pub fn gives_way(surface: &LayerSurface, menu_bars: &crate::menu_bar::MenuBars) -> bool {
    !menu_bars.has_surface(surface.wl_surface()) && surface.cached_state().anchor != Anchor::all()
}

/// Minimize / restore through the effect mesh: the window is captured at its natural size and
/// drawn through the warp, funnel or scale mesh towards the dock tile.
pub(super) fn minimize_elements(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    pres: &Presentation,
    slide: f64,
    active: bool,
) -> Option<Vec<Piece>> {
    let kind = super::super::motion::minimize_kind(p.fx.cfg.minimize, p.display)?;
    p.res.mesh.as_ref()?;
    let geo = entry.window.geometry();
    let ins = entry.insets();
    let natural = Rect::new(
        pres.frame.x + slide,
        pres.frame.y,
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    if natural.w < 1.0 || natural.h < 1.0 {
        return None;
    }
    let size_px = (
        ((natural.w * p.scale).round() as i32).max(1),
        ((natural.h * p.scale).round() as i32).max(1),
    );
    // a capture for a screenshot takes its own snapshot, so a protected window is left out of it
    let key = (!p.for_capture).then_some((entry.id, p.output_id));
    let kept = key.and_then(|key| p.res.pin_minimize_capture(key, size_px));
    let keep = match kept {
        Some(lease) => SnapshotKeep::Pinned(lease),
        None => {
            let (texture, _) = capture_window(
                renderer,
                p,
                entry,
                natural,
                active,
                0.0,
                false,
                SnapshotBackdrop::Fills,
            )?;
            let owner = p.res.snapshots.insert(texture);
            match key {
                Some(key) => {
                    let lease = p.res.store_minimize_capture(key, owner, size_px)?;
                    SnapshotKeep::Pinned(lease)
                }
                None => SnapshotKeep::Owned(owner),
            }
        }
    };
    let snapshot = match &keep {
        SnapshotKeep::Pinned(lease) => lease.id(),
        SnapshotKeep::Owned(owner) => owner.id(),
    };
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
    Some(vec![Piece::Mesh(MeshPiece {
        kind: SceneKind::MinimizeMesh,
        index: entry.id.0,
        snapshot,
        keep,
        mesh: local_mesh,
        opacity: (pres.opacity
            * super::super::motion::minimize_opacity(pres.minimize_progress, p.display))
            as f32,
        revision: Sig::new()
            .f(pres.minimize_progress)
            .u(kind as u64)
            .u(u64::from(p.display.reduce_motion))
            .finish(),
    })])
}

/// The ghosts of this output that lie over every window, front to back: the last frame of a window fading out. The
/// ghosts in `placed` are left out: each was drawn with its own window ([`ghost_piece`]).
pub(super) fn closing_elements(p: &SceneParams<'_>, placed: &[WindowId]) -> Vec<Piece> {
    p.fx.closing
        .iter()
        .rev()
        .filter(|c| c.output == p.output_id && !placed.contains(&c.id))
        .filter_map(|c| ghost_piece(p, c))
        .collect()
}

/// The picture of the ghost `c` where and as far as it shows now, `None` once it has faded out.
pub(super) fn ghost_piece(p: &SceneParams<'_>, c: &crate::fx::ClosingWindow) -> Option<Piece> {
    p.res.mesh.as_ref()?;
    let (rect, alpha) = c.placement(p.now);
    let alpha = alpha as f32;
    if alpha <= 0.0 {
        return None;
    }
    let r = Rect::new(rect.x - p.rect.x, rect.y - p.rect.y, rect.w, rect.h);
    let lease = match p.res.snapshots.pin(c.snapshot.id()) {
        Ok(lease) => lease,
        Err(error) => {
            tracing::error!(%error, window = c.id.0, "closing ghost without its snapshot");
            return None;
        }
    };
    Some(Piece::Mesh(MeshPiece {
        kind: SceneKind::ClosingGhost,
        index: c.id.0,
        snapshot: lease.id(),
        keep: SnapshotKeep::Pinned(lease),
        mesh: Mesh::quad(r.into_geom()),
        opacity: alpha,
        revision: Sig::new().f(c.progress(p.now)).finish(),
    }))
}

/// Elements of the layer-shell `layer`, front to back. The backdrop captures of menu bars are
/// appended to `captures`: they belong below the windows (see [`bar_backdrop_elements`]). The popups of a layer below
/// the overlay layer (a menu bar's menus, a dock's menus) are appended to `popups`, for [`SceneLayer::Popups`] above
/// every window and every bar; an overlay surface keeps its popups directly above it.
pub(super) fn layer_elements(
    p: &mut SceneParams<'_>,
    layer: Layer,
    captures: &mut Vec<Piece>,
    popups: &mut Vec<Piece>,
) -> Vec<Piece> {
    let map = layer_map_for_output(p.output);
    let scale = Scale::from(p.scale);
    let mut out: Vec<Piece> = Vec::new();
    let covered = layer == Layer::Top && top_layer_covered(p.wm, p.shell, p.output_id);
    // later surfaces are drawn on top of earlier ones
    for surface in crate::authorization::layers_stacked(&map, layer).rev() {
        if p.for_capture
            && (crate::authorization::is_prompt(surface.wl_surface())
                || crate::control::protected_surface(surface.wl_surface()))
        {
            continue;
        }
        if covered && gives_way(surface, p.menu_bars) {
            continue;
        }
        let Some(geo) = map.layer_geometry(surface) else {
            continue;
        };
        // a menu bar is offset and faded by its presentation (hide, reveal, inactive output), and gives way to an
        // overview of its output
        let look = p.menu_bars.look(surface.wl_surface(), p.rect.y, p.now);
        let (dy, opacity) = bar_shown(p, look);
        // the surface's own presentation (cupertino_layers_v1 version 2) moves, scales and fades all it draws
        let shown =
            crate::layers_v1::presentation(surface.wl_surface(), p.now, p.display.reduce_motion);
        let (dx, dy, opacity, zoom) = shown.map_or((0.0, dy, opacity, 1.0), |s| {
            (s.x, dy + s.y, opacity * s.opacity as f32, s.scale)
        });
        if opacity <= 0.0 || zoom <= 0.0 {
            continue;
        }
        let origin: Point<f64, Logical> = geo.loc.to_f64() + Point::from((dx, dy));
        let loc: Point<i32, Physical> = origin.to_physical_precise_round(scale);
        let zoomed = Zoom {
            centre: origin
                + Point::from((f64::from(geo.size.w) / 2.0, f64::from(geo.size.h) / 2.0)),
            scale: zoom,
        };
        // popups above their parent, each with its own backdrop directly below it
        let mut own = Vec::new();
        for (popup, popup_offset) in crate::popup_linger::shown(surface.wl_surface(), p.now) {
            let popup_origin = origin + (popup_offset - popup.geometry().loc).to_f64();
            // a popup with a presentation of its own is moved, scaled and faded by it before the surface's
            if let Some((shown, popup_opacity)) = popup_presentation(p, &popup, popup_origin) {
                let opacity = opacity * popup_opacity;
                if opacity <= 0.0 || shown.k.0 <= 0.0 {
                    continue;
                }
                let both = shown.then(Shown::about(zoomed.centre, (zoomed.scale, zoomed.scale)));
                let mut tree = standalone_tree(
                    popup.wl_surface(),
                    SceneKind::LayerContent,
                    popup_origin.to_physical_precise_round(scale),
                    opacity,
                );
                tree.role = SurfaceRole::Popup;
                let tree = both.tree(tree, p.scale);
                own.extend(tree_pieces(p, tree, popup_origin, |p, areas, bound| {
                    bounded_glass(
                        backdrop_elements(p, &both.areas(areas), opacity, WindowState::Active),
                        both.rect(bound),
                    )
                }));
                let backdrops = both.areas(surface_backdrops(popup.wl_surface(), popup_origin));
                own.extend(popup_backdrop_elements(p, &backdrops, opacity));
                continue;
            }
            let tree = zoomed.tree(
                standalone_tree(
                    popup.wl_surface(),
                    SceneKind::LayerContent,
                    popup_origin.to_physical_precise_round(scale),
                    opacity,
                ),
                scale,
            );
            own.extend(tree_pieces(p, tree, popup_origin, |p, areas, bound| {
                bounded_glass(
                    backdrop_elements(p, &zoomed.areas(areas), opacity, WindowState::Active),
                    zoomed.rect(bound),
                )
            }));
            let backdrops = zoomed.areas(surface_backdrops(popup.wl_surface(), popup_origin));
            own.extend(popup_backdrop_elements(p, &backdrops, opacity));
        }
        if layer == Layer::Overlay {
            out.append(&mut own);
        } else {
            popups.append(&mut own);
        }
        // its layers over it (cupertino_layers_v1)
        let place = super::super::layers::LayerPlace {
            origin,
            pivot: if zoomed.scale == 1.0 {
                origin
            } else {
                zoomed.centre
            },
            scale: (zoomed.scale, zoomed.scale),
            opacity,
            clip: Clip::None,
            kind: SceneKind::LayerContent,
        };
        out.extend(super::super::layers::layer_pieces(
            p,
            surface.wl_surface(),
            &place,
            |p, areas| backdrop_elements(p, &zoomed.areas(areas), opacity, WindowState::Active),
        ));
        let tree = zoomed.tree(
            standalone_tree(surface.wl_surface(), SceneKind::LayerContent, loc, opacity),
            scale,
        );
        out.extend(tree_pieces(p, tree, origin, |p, areas, bound| {
            bounded_glass(
                backdrop_elements(p, &zoomed.areas(areas), opacity, WindowState::Active),
                zoomed.rect(bound),
            )
        }));
        if layer == Layer::Overlay && surface.namespace() == crate::dock::OVERVIEW_NAMESPACE {
            // the workspaces' live pictures lie on the chrome's band, under what it draws
            out.extend(overview_pictures(p, opacity));
        }
        let backdrops = zoomed.areas(surface_backdrops(surface.wl_surface(), origin));
        if look.is_some() {
            out.extend(bar_backdrop_elements(p, &backdrops, opacity, captures));
        } else {
            let mut pieces = backdrop_elements(p, &backdrops, opacity, WindowState::Active);
            if surface.namespace() == crate::state::SESSION_PANEL_NAMESPACE {
                // a session panel is an alert: its glass casts an alert's shadow, deep while it has the keyboard
                let active = p.keyboard_focus.as_ref() == Some(surface.wl_surface());
                lift_as_alert(&mut pieces, active, p.config.is_dark());
            }
            out.extend(pieces);
        }
    }
    out.extend(held_elements(p, layer, covered, captures));
    out
}

/// How a menu bar of the output shows, given its `look`: its offset and opacity, faded with an overview of its
/// output; `(0, 1)` for any other surface.
fn bar_shown(p: &SceneParams<'_>, look: Option<crate::menu_bar::BarLook>) -> (f64, f32) {
    let overview_bar = p
        .shell
        .overview
        .as_ref()
        .filter(|o| o.output == p.output_id)
        .map_or(1.0, |o| o.menu_bar_alpha(p.now) as f32);
    look.map_or((0.0, 1.0), |l| (l.dy, l.alpha * overview_bar))
}

/// The held persistent surfaces of `layer` whose component connection ended (D19), front to back, as the live
/// surfaces drew them: each one's last picture over its backdrops, drawn live, a held menu bar offset and faded as the
/// output's menu bar shows, and nothing of a surface that gives way while a full-screen window covers the output
/// (`covered`); then their holes, behind the pictures. The backdrop captures of a held menu bar go to `captures`.
fn held_elements(
    p: &mut SceneParams<'_>,
    layer: Layer,
    covered: bool,
    captures: &mut Vec<Piece>,
) -> Vec<Piece> {
    let hosts = p.hosts;
    let mut out = Vec::new();
    let mut shown = Vec::new();
    for hold in hosts.held_surfaces_on(p.output_id, layer) {
        let menu_bar = hold
            .picture
            .as_ref()
            .is_some_and(|picture| picture.menu_bar);
        if covered && !menu_bar && hold.anchor != Anchor::all() {
            continue;
        }
        let look = menu_bar
            .then(|| p.menu_bars.look_on(hold.output, p.rect.y, p.now))
            .flatten();
        let (dy, opacity) = bar_shown(p, look);
        if opacity <= 0.0 {
            continue;
        }
        shown.push(HeldShown {
            component: hold.component,
            key: &hold.key,
            dy,
            opacity,
        });
        let Some(picture) = &hold.picture else {
            continue;
        };
        let g = hold.geometry;
        let at = Rect::new(
            f64::from(g.loc.x),
            f64::from(g.loc.y) + dy,
            f64::from(g.size.w),
            f64::from(g.size.h),
        );
        match &picture.content {
            crate::host::HeldContent::Texture { snapshot, .. } => {
                out.extend(held_texture_piece(p, snapshot, at, opacity));
            }
            crate::host::HeldContent::Memory { buffer, size } => {
                out.push(held_memory_piece(buffer, *size, at, opacity));
            }
        }
        out.extend(held_backdrop_pieces(
            p,
            &picture.backdrops,
            (at.x, at.y),
            opacity,
            menu_bar,
            captures,
        ));
    }
    out.extend(held_hole_pieces(p, &shown));
    out
}

/// A held surface as it shows on the output: which hold, and its offset and opacity ([`held_elements`]).
pub(super) struct HeldShown<'a> {
    pub component: &'static str,
    pub key: &'a str,
    pub dy: f64,
    pub opacity: f32,
}

/// The backdrops a held surface drew ([`HeldBackdrops`]), its top-left corner at `at` (output-local points), front to
/// back as the live surface draws them: its layers' glass, its subsurfaces' glass, its own backdrops (a menu bar's
/// through [`bar_backdrop_elements`], its captures going to `captures`).
fn held_backdrop_pieces(
    p: &mut SceneParams<'_>,
    held: &HeldBackdrops,
    at: (f64, f64),
    opacity: f32,
    menu_bar: bool,
    captures: &mut Vec<Piece>,
) -> Vec<Piece> {
    let moved = |areas: &[BackdropArea]| -> Vec<BackdropArea> {
        areas
            .iter()
            .map(|a| BackdropArea {
                area: Rectangle::new(a.area.loc + Point::from(at), a.area.size),
                ..a.clone()
            })
            .collect()
    };
    let mut out = Vec::new();
    for (areas, made) in &held.layers {
        for mut piece in backdrop_elements(p, &moved(areas), opacity, WindowState::Active) {
            if let (Piece::Glass(g), Some((made, made_opacity))) = (&mut piece, made) {
                // the glass as the layer made it, at the held surface's place
                let mut gc = (*g.glass).clone();
                let m = &made.container;
                gc.container.lens = m.lens;
                gc.container = gc.container.with_blur_ramp(m.blur_ramp);
                gc.container.edge = m.edge;
                gc.container.params.shadow = m.params.shadow;
                gc.container.params.highlight.angle = m.params.highlight.angle;
                gc.key = Sig::new().u(gc.key).u(made.key).finish();
                g.glass = Rc::new(gc);
                g.opacity = made_opacity * opacity;
            }
            out.push(piece);
        }
    }
    for (areas, bound) in &held.subsurfaces {
        let bound = Rectangle::new(bound.loc + Point::from(at), bound.size);
        out.extend(bounded_glass(
            backdrop_elements(p, &moved(areas), opacity, WindowState::Active),
            bound,
        ));
    }
    let own = moved(&held.surface);
    if menu_bar {
        out.extend(bar_backdrop_elements(p, &own, opacity, captures));
    } else {
        out.extend(backdrop_elements(p, &own, opacity, WindowState::Active));
    }
    out
}

/// A held surface's picture copied from its shm buffer (D19), over `at` (output-local points) at `opacity`.
fn held_memory_piece(
    buffer: &smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    size: (i32, i32),
    at: Rect,
    opacity: f32,
) -> Piece {
    let src = smithay::utils::Rectangle::<f64, Logical>::from_size(
        (f64::from(size.0), f64::from(size.1)).into(),
    );
    Piece::Image(Picture {
        buffer: buffer.clone().into(),
        kind: SceneKind::LayerContent,
        index: 0,
        role: ImageRole::Standalone,
        buffer_px: size,
        src,
        size: (at.w as i32, at.h as i32),
        placement: Placement {
            origin: OutputPoint(cupertino_geometry::Point::new(at.x, at.y)),
            ..Placement::IDENTITY
        },
        opacity,
        clip: Clip::None,
    })
}

/// A held surface's picture drawn by the renderer (D19), over `at` (output-local points) at `opacity`: the snapshot's
/// quad, as a returning drag icon draws its own. Nothing without the mesh program or once it is retired.
fn held_texture_piece(
    p: &SceneParams<'_>,
    snapshot: &crate::render::snapshots::SnapshotOwner,
    local: Rect,
    opacity: f32,
) -> Option<Piece> {
    p.res.mesh.as_ref()?;
    let lease = match p.res.snapshots.pin(snapshot.id()) {
        Ok(lease) => lease,
        Err(error) => {
            tracing::error!(%error, "held surface without its snapshot");
            return None;
        }
    };
    Some(Piece::Mesh(MeshPiece {
        kind: SceneKind::LayerContent,
        index: 0,
        snapshot: lease.id(),
        keep: SnapshotKeep::Pinned(lease),
        mesh: Mesh::quad(local.into_geom()),
        opacity,
        // a held picture never changes
        revision: 0,
    }))
}

/// The glass among `pieces` with an alert's drop shadow shown ([`cupertino_theme::glass::GlassShadow::alert`]): the deep
/// one while the alert has the keyboard (`active`), the shallow one while another surface has it.
fn lift_as_alert(pieces: &mut [Piece], active: bool, dark: bool) {
    for piece in pieces {
        if let Piece::Glass(g) = piece {
            let mut gc = (*g.glass).clone();
            gc.container.params.shadow = gc.container.params.shadow.alert(active, dark);
            gc.key = Sig::new().u(gc.key).u(1 + u64::from(active)).finish();
            g.glass = Rc::new(gc);
        }
    }
}

/// The presentation of `popup` (its surface's origin at `origin`, output-local points) as shown now, and its opacity;
/// `None` for a popup without one.
pub(super) fn popup_presentation(
    p: &SceneParams<'_>,
    popup: &smithay::desktop::PopupKind,
    origin: Point<f64, Logical>,
) -> Option<(Shown, f32)> {
    let shown = crate::layers_v1::presentation(popup.wl_surface(), p.now, p.display.reduce_motion)?;
    // its window geometry, or without one what its surfaces cover
    let g = Some(popup.geometry())
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| {
            smithay::desktop::utils::bbox_from_surface_tree(popup.wl_surface(), (0, 0))
        });
    let geometry = Rectangle::new(origin + g.loc.to_f64(), g.size.to_f64());
    Some((Shown::presented(shown, geometry), shown.opacity as f32))
}

/// A layer shell surface's presentation scale about its centre (output-local points), applied to its trees, its
/// backdrops and the bounds of its subsurfaces' glass; at 1 everything is left as it is.
#[derive(Clone, Copy)]
struct Zoom {
    centre: Point<f64, Logical>,
    scale: f64,
}

impl Zoom {
    fn point(&self, q: Point<f64, Logical>) -> Point<f64, Logical> {
        Point::from((
            self.centre.x + (q.x - self.centre.x) * self.scale,
            self.centre.y + (q.y - self.centre.y) * self.scale,
        ))
    }

    fn rect(&self, r: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        if self.scale == 1.0 {
            return r;
        }
        Rectangle::new(
            self.point(r.loc),
            (r.size.w * self.scale, r.size.h * self.scale).into(),
        )
    }

    fn areas(&self, mut areas: Vec<BackdropArea>) -> Vec<BackdropArea> {
        if self.scale != 1.0 {
            for a in &mut areas {
                a.area = self.rect(a.area);
                a.radius *= self.scale;
            }
        }
        areas
    }

    fn tree(&self, mut tree: Tree, scale: Scale<f64>) -> Tree {
        if self.scale != 1.0 {
            tree.pivot = self.centre.to_physical_precise_round(scale);
            tree.scale = (self.scale, self.scale);
            // drawn rescaled about the pivot, as a popup is, unmasked
            tree.role = SurfaceRole::Popup;
        }
        tree
    }
}

/// Elements of popups that carry a material, to go directly below the popup's surface (front to back:
/// material or glass, outline, shadow). The outline is the menu edge (`Decor::menu_edge`) just outside the shape, the
/// shadow the panel row of the table (it is cut out under the popup, so the material stays translucent). `backdrops`
/// are in output-local logical coordinates.
pub(super) fn popup_backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
) -> Vec<Piece> {
    note_not_glass(p, backdrops);
    let mut out: Vec<Piece> = Vec::new();
    let params = p.decor.popup_shadow_params();
    let edge = p.decor.menu_edge();
    let mut glass = GlassGroups::default();
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        let joined = in_callout(backdrops, b);
        if joined {
            // a callout is one piece of glass: the glass lights its own rim, and no shape of it gets an outline
            glass.add(b, out.len());
            let (w, h) = (b.area.size.w, b.area.size.h);
            if w.min(h) >= CALLOUT_SHADOW_MIN_SIDE {
                out.extend(popup_shadow(p, b, &params, alpha));
            }
            continue;
        }
        if b.glass.is_some() {
            // the glass draws its own rim at rest; with Reduce Transparency or Increase Contrast the light rim inside the
            // glass, in front of it, marks the edge instead
            if p.display.reduce_transparency || p.display.increase_contrast {
                out.extend(inner_rim_item(
                    SceneKind::PopupRim,
                    b.key,
                    b.area,
                    b.radius,
                    true,
                    &edge,
                    p.scale,
                    alpha,
                ));
            }
            glass.add(b, out.len());
        } else {
            out.extend(material_layer(
                p,
                SceneKind::Backdrop,
                b.key,
                &b.material,
                b.area,
                b.radius,
                alpha,
            ));
        }
        out.extend(outline_item(
            SceneKind::PopupOutline,
            b.key,
            b.area,
            b.radius,
            true,
            &edge,
            p.scale,
            alpha,
        ));
        out.extend(popup_shadow(p, b, &params, alpha));
    }
    glass.finish_popup(p, out, opacity)
}

/// The white lifted over the face of a small dark callout.
pub(super) const SMALL_CALLOUT_FACE_DARK: [f32; 4] = [1.0, 1.0, 1.0, 0.05];

/// The opacity and the width in points of the black outline along the outside of a popover.
pub(super) const POPOVER_OUTLINE_DARK: (f32, f32) = (0.45, 1.0);
pub(super) const POPOVER_OUTLINE_LIGHT: (f32, f32) = (0.33, 1.0);

/// The soft band of light under the top and bottom edges of a light large panel, in the same terms: wide, so it also
/// lifts the face where the blur of the backdrop outside the panel darkens it near the edge.
pub(super) const LARGE_PANEL_GLOW_LIGHT: (f32, f32, f32) = (0.075, 7.5, 0.0);

/// The shortest side a shape of a callout has to be for the callout to cast the panel shadow under it: the label's
/// capsule and a pointer cast none.
pub(super) const CALLOUT_SHADOW_MIN_SIDE: f64 = 40.0;

/// Whether glass backdrop `b` joins at least one other glass backdrop of its surface in a container: the shapes of one
/// callout, a panel and its pointer.
fn in_callout(backdrops: &[BackdropArea], b: &BackdropArea) -> bool {
    let Some(spec) = b.glass.filter(|g| g.container != 0) else {
        return false;
    };
    backdrops
        .iter()
        .filter(|o| o.glass.is_some_and(|g| g.container == spec.container))
        .count()
        > 1
}

/// The panel shadow below popup backdrop `b`.
fn popup_shadow(
    p: &mut SceneParams<'_>,
    b: &BackdropArea,
    params: &cupertino_decorations::ShadowParams,
    alpha: f32,
) -> Vec<Piece> {
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
        corner_cap: None,
    };
    shadow_item(
        p,
        SceneKind::PopupShadow,
        b.key,
        params,
        &plan,
        b.radius,
        f64::from(alpha),
    )
    .into_iter()
    .collect()
}
