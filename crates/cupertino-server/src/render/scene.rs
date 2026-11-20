//! Per-output scene construction: wallpaper → layer surfaces → (overview backdrop) → workspace
//! windows with shadows → shell UI (menu bar, extensions) → layer overlays →
//! cursor, ordered by [`SceneLayer`].

use std::rc::Rc;

use cupertino_decorations::TitlebarStyle;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_effects::{DockEdge, Mesh, MinimizeEffect, Vertex};
use cupertino_scene::{
    BorderFalloff, Clip, ImageRole, OutputPoint, Placement, RoundedRect, SceneItem, SceneKind,
    Shadow, Source, SurfaceRole,
};
use cupertino_theme::glass::adapts_foreground;
use cupertino_theme::WindowState;
use cupertino_wm::{OutputId, Presentation, Rect, WindowId, Wm};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{Anchor, Layer};

use super::compose::SnapshotKeep;
use super::compose::{
    corner_style, from_rect, image_piece, item, logical_rect, output_rect, transform, Composed,
    Composition, Material, MeshPiece, Picture, Piece, Tree, TreeSlice,
};
pub use super::core::{
    finalize_materials, fullscreen_corner_cap, plan_window, rrect_item, wallpaper_tint,
    windows_on_output, CursorPlan, LockView, Panel, RenderResources, SessionOverlay, WindowPlan,
};
use super::gles::{Gles, GlesCompletion, GlesResources, GlesTarget};
use super::material::glass::{GlassContainer, ToneProbe};
use super::material::{GpuRecipe, MaterialMask, MaterialStage};
use super::motion::{skips_frame_animation, skips_sheet_slide, WorkspaceTransition};
use super::popup_shown::Shown;
use super::shader_rect::{to_physical_rect, ElementCache, Sig};
use super::SceneLayer;
use crate::app_effects_v1::{
    raised_parts, subsurface_backdrops, surface_backdrops, BackdropArea, TreePart,
};
use crate::config::Config;
use crate::decor::{Decor, DecorInput};
use crate::fx::{Effects, IntoGeom};
use crate::shell::Shell;
use crate::state::{Shrink, WindowEntry};
use crate::window_manager::Look;
use cupertino_glass::glass::{Container, GlassShape};
use cupertino_scene::renderer::Renderer;

/// Material over a window while a sheet hangs from it (material 30).
pub(super) const SHEET_DIMMING_MATERIAL: &str = "SheetUnderlay";

/// Flatten `stack` into the [`Composed`] frame of the output described by `p`.
fn finish_composition(stack: Composition, p: &SceneParams<'_>) -> Composed {
    let output = cupertino_scene::Output {
        id: cupertino_scene::OutputId(p.output_id.0),
        rect: cupertino_scene::GlobalRect(from_rect(p.rect)),
        physical_size: cupertino_scene::PhysicalSize {
            width: p.output_px.0,
            height: p.output_px.1,
        },
        scale: p.scale,
        // a frame drawn above the display's scale is drawn upright; the final pass turns it
        transform: match p.panel {
            Some(_) => cupertino_scene::Transform::Normal,
            None => transform(p.output.current_transform()),
        },
    };
    stack.finish_with(output, std::time::Duration::from_secs_f64(p.now.max(0.0)))
}

/// Plan of a window whose whole frame (decorations included) is scaled into `rect` with visual
/// corner radius `corner`, for a stage placement: the live content, as the overview draws it.
pub fn placed_plan(
    entry: &WindowEntry,
    rect: Rect,
    opacity: f64,
    shadow: f64,
    corner: f64,
) -> WindowPlan {
    let geo = entry.window.geometry();
    let ins = entry.insets();
    WindowPlan {
        visual: rect,
        scale_x: rect.w / (f64::from(geo.size.w) + ins.horizontal()).max(1.0),
        scale_y: rect.h / (f64::from(geo.size.h) + ins.vertical()).max(1.0),
        opacity,
        shadow,
        exact: false,
        corner: Some(corner),
        corner_cap: None,
    }
}

/// Plan of window `id` on its output at `now` (None when the window cannot be drawn). With Reduce
/// Motion in `options` a full-screen transition draws the window at its new frame, and an opening sheet fades in at
/// its resting frame (`render::motion`).
pub fn plan_for(
    wm: &Wm,
    entry: &WindowEntry,
    slide: f64,
    now: f64,
    fx: &Effects,
    options: DisplayOptions,
) -> Option<(WindowPlan, Presentation)> {
    let mut pres = wm.presentation(entry.id, now)?;
    if !pres.visible {
        return None;
    }
    let w = wm.window(entry.id)?;
    if skips_frame_animation(w.state(), options)
        || skips_sheet_slide(w.lifecycle.kind(), w.state(), options)
    {
        pres.frame = w.lifecycle.target_frame();
    }
    if crate::render::motion::skips_open_scale(w.state(), options)
        && w.lifecycle.kind() != cupertino_effects::WindowKind::Sheet
        && w.lifecycle.kind() != cupertino_effects::WindowKind::Alert
    {
        pres.scale = 1.0;
    }
    let animating = w.lifecycle.is_animating(now) || entry.shrink != Shrink::None;
    let mut plan = plan_window(&pres, entry.committed_size(), animating, (slide, 0.0));
    // a full-screen window meets the edges of its output: no corner mask (its shadow is faded out by the model); while
    // it fills the output or leaves it, its corners follow the room left between its frame and the output's edges
    match w.state() {
        cupertino_wm::LifecycleState::EnterFullScreen
        | cupertino_wm::LifecycleState::ExitFullScreen => {
            if let Some(out) = wm.output(w.output) {
                plan.corner_cap = Some(fullscreen_corner_cap(plan.visual, out.rect));
            }
        }
        _ if w.lifecycle.toplevel_states().frame_shape() == cupertino_wm::FrameShape::Square => {
            plan.corner = Some(0.0);
        }
        _ => {}
    }
    let desk = fx.show_desktop_progress(now);
    if desk > 0.0 && !w.sticky {
        if let Some(out) = wm.output(w.output) {
            plan.visual = fx.show_desktop_rect(plan.visual, out.rect, desk);
        }
    }
    Some((plan, pres))
}

/// Everything the scene builder needs for one output and frame.
pub struct SceneParams<'a> {
    pub output: &'a Output,
    pub output_id: OutputId,
    /// Output rectangle in the global logical space.
    pub rect: Rect,
    pub scale: f64,
    pub output_px: (i32, i32),
    pub now: f64,
    pub wm: &'a Wm,
    pub windows: &'a [WindowEntry],
    pub config: &'a Config,
    /// The accessibility display options in effect.
    pub display: DisplayOptions,
    /// Colour filter, invert and contrast pass over the whole output (`Accessibility::display_filter`).
    pub display_filter: Option<Rc<GpuRecipe>>,
    pub shell: &'a mut Shell,
    pub decor: &'a mut Decor,
    pub menu_bars: &'a crate::menu_bar::MenuBars,
    /// Shell host holds rendered while a component restarts (D19).
    pub hosts: &'a crate::host::HostManager,
    /// Active transactions and window/workspace holes (W5.3).
    pub transactions: &'a crate::transaction::TransactionManager,
    /// Stage placements: strip thumbnails and hidden windows.
    pub window_managers: &'a crate::window_manager::WindowManagers,
    pub overlays: &'a mut crate::overlays::Overlays,
    pub fx: &'a Effects,
    pub res: &'a RenderResources,
    pub cache: &'a mut ElementCache,
    pub cursor: CursorPlan,
    /// Pointer position in the global logical space.
    pub pointer: Point<f64, Logical>,
    pub drag_icon: Option<(WlSurface, Point<i32, Logical>)>,
    /// The surface that has the keyboard: a session panel there casts an active alert's shadow.
    pub keyboard_focus: Option<WlSurface>,
    pub lock: LockView,
    /// Black overlays over everything, front to back: the power dimming.
    pub session_overlays: [Option<SessionOverlay>; 1],
    /// The frame leaves the compositor (screen capture, screenshot files, recordings): the surfaces of the
    /// authentication prompt are left out.
    pub for_capture: bool,
    /// The display's own scale and pixels when the output is drawn at a whole scale above it (`scale` and
    /// `output_px` are then the render scale and the size of the frame drawn at it, upright).
    pub panel: Option<Panel>,
}

/// Put the session overlays on top of `stack`, whatever else the frame shows.
fn session_overlays(p: &SceneParams<'_>, stack: &mut Composition) {
    push_session_overlays((p.rect.w, p.rect.h), &p.session_overlays, stack);
}

fn push_session_overlays(
    size: (f64, f64),
    overlays: &[Option<SessionOverlay>],
    stack: &mut Composition,
) {
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), size.into());
    for overlay in overlays.iter().flatten() {
        stack.push(
            SceneLayer::SessionOverlay,
            Piece::item(item(
                overlay.kind,
                0,
                Source::Wallpaper {
                    rect: output_rect(area),
                    colours: [BLACK; 4],
                },
                overlay.alpha,
            )),
        );
    }
}

/// How far inside a rounded corner of radius 1 its rectangle is clear of the curve along the diagonal, a little more
/// than 1 − 1/√2 so a continuous corner, which reaches further in, clears too.
const CORNER_CLEARANCE: f64 = 0.33;

/// `r` with each side moved `by` towards its centre, never below an empty rectangle.
fn inset_rect(
    r: smithay::utils::Rectangle<f64, Logical>,
    by: f64,
) -> smithay::utils::Rectangle<f64, Logical> {
    let (w, h) = (
        (r.size.w - 2.0 * by).max(0.0),
        (r.size.h - 2.0 * by).max(0.0),
    );
    smithay::utils::Rectangle::new((r.loc.x + by, r.loc.y + by).into(), (w, h).into())
}

fn local(r: Rect, origin: &Rect) -> smithay::utils::Rectangle<f64, Logical> {
    smithay::utils::Rectangle::new((r.x - origin.x, r.y - origin.y).into(), (r.w, r.h).into())
}

/// The outline of an edge ([`cupertino_decorations::Edge`]) just outside `rect`, whose corners are rounded by
/// `radius`: a ring of whole device pixels whose inner side is the shape's outline, at `alpha`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn outline_item(
    kind: SceneKind,
    index: u64,
    rect: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    edge: &cupertino_decorations::Edge,
    scale: f64,
    alpha: f32,
) -> Option<Piece> {
    let line = EdgeLine {
        line: edge.outline,
        falloff: edge.falloff,
        outside: true,
    };
    edge_line_item(kind, index, rect, radius, continuous, line, scale, alpha)
}

/// The inner rim of an edge just inside `rect`, whose corners are rounded by `radius`: a ring of whole device pixels
/// whose outer side is the shape's outline, at `alpha`. `None` for an edge without one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn inner_rim_item(
    kind: SceneKind,
    index: u64,
    rect: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    edge: &cupertino_decorations::Edge,
    scale: f64,
    alpha: f32,
) -> Option<Piece> {
    let line = EdgeLine {
        line: edge.inner?,
        falloff: edge.inner_falloff,
        outside: false,
    };
    edge_line_item(kind, index, rect, radius, continuous, line, scale, alpha)
}

/// One line of an edge and the side of the shape's outline it lies on.
struct EdgeLine {
    line: cupertino_decorations::Hairline,
    falloff: cupertino_decorations::OutlineFalloff,
    /// Just outside the outline, else just inside it.
    outside: bool,
}

#[allow(clippy::too_many_arguments)]
fn edge_line_item(
    kind: SceneKind,
    index: u64,
    rect: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    line: EdgeLine,
    scale: f64,
    alpha: f32,
) -> Option<Piece> {
    let (px, color) = line.line.device(scale);
    if color[3] <= 0.0 || alpha <= 0.0 || rect.size.w <= 0.0 || rect.size.h <= 0.0 {
        return None;
    }
    let w = f64::from(px) / scale;
    // the shader draws a border inside its rectangle: outside the shape, grow the rectangle by the line's width
    let grow = if line.outside { w } else { 0.0 };
    let ring = smithay::utils::Rectangle::new(
        (rect.loc.x - grow, rect.loc.y - grow).into(),
        (rect.size.w + 2.0 * grow, rect.size.h + 2.0 * grow).into(),
    );
    let (sides, ends, floor) = line.falloff.shader();
    Some(Piece::item(item(
        kind,
        index,
        Source::RoundedRect(RoundedRect {
            rect: output_rect(ring),
            radius: radius + grow,
            style: corner_style(continuous),
            fill: [0.0; 4],
            border: Some((w, color)),
            border_falloff: BorderFalloff { sides, ends, floor },
        }),
        f64::from(alpha),
    )))
}

/// A surface tree drawn at `origin` as it is, without a window's rescale or mask.
fn standalone(
    surface: &WlSurface,
    kind: SceneKind,
    origin: Point<i32, Physical>,
    opacity: f32,
) -> Piece {
    Piece::Tree(standalone_tree(surface, kind, origin, opacity))
}

/// The tree [`standalone`] draws.
fn standalone_tree(
    surface: &WlSurface,
    kind: SceneKind,
    origin: Point<i32, Physical>,
    opacity: f32,
) -> Tree {
    Tree {
        surface: surface.clone(),
        kind,
        role: SurfaceRole::Standalone,
        origin,
        pivot: origin,
        scale: (1.0, 1.0),
        opacity,
        clip: Clip::None,
        only: None,
    }
}

/// The pieces of `tree`, front to back, with the backdrops its subsurfaces draw ([`raised_parts`]) between its slices:
/// each directly below the slice that holds the subsurface's subtree. `origin` is where the tree's origin lies
/// (output-local points, before a window's rescale); `backdrops` makes the pieces of one subsurface's backdrops, placed
/// from it, given the subsurface's rectangle there, and the glass among them reads and draws only inside that
/// rectangle as it is drawn ([`GlassContainer::bounded`]). A tree whose subsurfaces draw no backdrops is one piece.
fn tree_pieces(
    p: &mut SceneParams<'_>,
    tree: Tree,
    origin: Point<f64, Logical>,
    mut backdrops: impl FnMut(
        &mut SceneParams<'_>,
        Vec<BackdropArea>,
        smithay::utils::Rectangle<f64, Logical>,
    ) -> Vec<Piece>,
) -> Vec<Piece> {
    let Some(parts) = raised_parts(&tree.surface) else {
        return vec![Piece::Tree(tree)];
    };
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            TreePart::Surfaces(surfaces) => {
                use smithay::reexports::wayland_server::Resource;
                let index = surfaces[0].id().protocol_id() as u64;
                let ids = surfaces
                    .iter()
                    .map(smithay::backend::renderer::element::Id::from_wayland_resource)
                    .collect();
                out.push(Piece::Tree(Tree {
                    surface: tree.surface.clone(),
                    only: Some(TreeSlice { ids, index }),
                    ..tree
                }));
            }
            TreePart::Backdrops { surface, rect } => {
                let at = origin + rect.loc;
                let areas = subsurface_backdrops(&surface, at);
                if areas.is_empty() {
                    continue;
                }
                out.extend(backdrops(
                    p,
                    areas,
                    smithay::utils::Rectangle::new(at, rect.size),
                ));
            }
        }
    }
    out
}

/// `pieces` with their glass reading and drawing only inside `bound` (output-local points).
fn bounded_glass(
    mut pieces: Vec<Piece>,
    bound: smithay::utils::Rectangle<f64, Logical>,
) -> Vec<Piece> {
    for piece in &mut pieces {
        if let Piece::Glass(g) = piece {
            g.glass = Rc::new((*g.glass).clone().bounded(bound));
        }
    }
    pieces
}

/// The wallpaper of the output: the image of the user or of the asset pack, else the procedural one.
fn wallpaper_element(p: &mut SceneParams<'_>) -> Piece {
    let dark = p.config.is_dark();
    // the device rows the menu bar covers, measured for its backdrop when a new image is put up
    let strip_rows = p
        .menu_bars
        .set
        .bar_height(p.output_id)
        .map_or(0, |h| (h * p.scale).ceil() as u32);
    // the bar draws nothing of its own over the wallpaper (only a solid strip under Reduce Transparency, when its text
    // keeps the appearance's), so what is under its text is the strip as it is, with no colour operations
    let appearance = p
        .config
        .appearance
        .with_high_contrast(p.display.increase_contrast);
    p.overlays
        .wallpaper
        .set_bar_material((appearance, p.display.reduce_transparency), Vec::new);
    // the image is laid in the logical space, which a quarter-turned output has on its side (a frame drawn above the
    // display's scale is upright already)
    let turn = match p.panel {
        Some(_) => smithay::utils::Transform::Normal,
        None => p.output.current_transform(),
    };
    let px = turn.transform_size(smithay::utils::Size::<i32, smithay::utils::Physical>::from(
        p.output_px,
    ));
    if let Some(colour) = p.config.wallpaper_color {
        let rgb = colour.map(|c| f32::from(c) / 255.0);
        p.overlays.wallpaper.solid(p.output_id.0, rgb, p.now);
        return Piece::item(full_output_wallpaper(
            p,
            SceneKind::Wallpaper,
            [[rgb[0], rgb[1], rgb[2], 1.0]; 4],
        ));
    }
    let image = p.overlays.wallpaper.shown(
        p.output_id.0,
        &p.config.wallpaper,
        dark,
        (px.w, px.h),
        strip_rows,
        p.now,
    );
    match image {
        Some((buffer, px)) => image_piece(
            &buffer,
            SceneKind::WallpaperImage,
            px,
            0.0,
            0.0,
            p.scale,
            1.0,
        ),
        None => Piece::item(wallpaper(p)),
    }
}

fn wallpaper(p: &SceneParams<'_>) -> SceneItem {
    full_output_wallpaper(
        p,
        SceneKind::Wallpaper,
        wallpaper_colours(p.config.is_dark()),
    )
}

/// The corner colours of the drawn wallpaper, dark or light.
fn wallpaper_colours(dark: bool) -> [[f32; 4]; 4] {
    if dark {
        [
            [0.07, 0.09, 0.20, 1.0],
            [0.20, 0.09, 0.27, 1.0],
            [0.10, 0.30, 0.58, 1.0],
            [0.50, 0.16, 0.42, 1.0],
        ]
    } else {
        [
            [0.72, 0.82, 0.97, 1.0],
            [0.96, 0.78, 0.86, 1.0],
            [0.45, 0.68, 0.99, 1.0],
            [0.99, 0.66, 0.50, 1.0],
        ]
    }
}

/// Solid black over the whole output: all that is visible of the session besides the lock surface.
fn lock_backdrop(p: &SceneParams<'_>) -> SceneItem {
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    full_output_wallpaper(p, SceneKind::LockBackdrop, [BLACK; 4])
}

fn full_output_wallpaper(
    p: &SceneParams<'_>,
    kind: SceneKind,
    colours: [[f32; 4]; 4],
) -> SceneItem {
    let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), (p.rect.w, p.rect.h).into());
    item(
        kind,
        0,
        Source::Wallpaper {
            rect: output_rect(area),
            colours,
        },
        1.0,
    )
}

/// The colour windows' materials take a slight tint of: the wallpaper's average, or its grey of the same lightness
/// when windows are not to take the wallpaper's colour.
fn window_tint(p: &SceneParams<'_>, dark: bool) -> [f32; 3] {
    let tint = p
        .overlays
        .wallpaper
        .tint
        .unwrap_or_else(|| wallpaper_tint(dark));
    if p.config.tint_windows {
        tint
    } else {
        let grey = 0.2126 * tint[0] + 0.7152 * tint[1] + 0.0722 * tint[2];
        [grey; 3]
    }
}

/// Shadow of window `id`, with the plan's corner radius ([`WindowPlan::corner`]).
fn shadow_piece(
    p: &mut SceneParams<'_>,
    id: WindowId,
    plan: &WindowPlan,
    active: bool,
    alpha: f64,
) -> Option<Piece> {
    let params = p.decor.shadow_params(active);
    let radius = plan.capped(
        plan.corner
            .map_or(p.config.window.corner_radius, |r| r / shadow_scale(plan)),
    );
    shadow_item(p, SceneKind::Shadow, id.0, &params, plan, radius, alpha)
}

/// Factor the shadow's blur, offset and corner follow a scaled window by (at least 1/4).
fn shadow_scale(plan: &WindowPlan) -> f64 {
    plan.scale_x.min(plan.scale_y).max(0.0).max(0.25)
}

/// Shadow behind `plan.visual` from the verified parameter table; the shader receives the
/// device-pixel values of `ShadowParams::gpu` converted back to points.
fn shadow_data(
    p: &SceneParams<'_>,
    params: &cupertino_decorations::ShadowParams,
    plan: &WindowPlan,
    corner: f64,
    alpha: f64,
) -> Option<Shadow> {
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
    Some(Shadow {
        area: output_rect(area),
        caster: output_rect(smithay::utils::Rectangle::new(
            (v.x - origin.x, v.y - origin.y).into(),
            (v.w, v.h).into(),
        )),
        radius: corner * s,
        sigma,
        density,
        offset_y: off,
        style: corner_style(style.continuous_corners),
        cutout: style.shadow.cutout,
    })
}

pub(crate) fn shadow_item(
    p: &SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    params: &cupertino_decorations::ShadowParams,
    plan: &WindowPlan,
    corner: f64,
    alpha: f64,
) -> Option<Piece> {
    let shadow = shadow_data(p, params, plan, corner, alpha)?;
    Some(Piece::item(item(
        kind,
        index,
        Source::Shadow(shadow),
        alpha,
    )))
}

/// The physical origin of a window's surface tree in output-local device pixels: its visual origin, inset by the
/// decorations, less the window geometry's offset into the surface.
fn window_surface_origin(
    entry: &WindowEntry,
    plan: &WindowPlan,
    p: &SceneParams<'_>,
) -> Point<i32, Physical> {
    let scale = Scale::from(p.scale);
    let geo = entry.window.geometry();
    let ins = entry.insets();
    let v_phys = to_physical_rect(local(plan.visual, &p.rect), scale);
    let geo_phys: Point<i32, Physical> = geo.loc.to_f64().to_physical(scale).to_i32_round();
    let inset_phys: Point<i32, Physical> = Point::<f64, Logical>::from((ins.left, ins.top))
        .to_physical(scale)
        .to_i32_round();
    v_phys.loc + inset_phys - geo_phys
}

/// The output-local point of a surface tree drawn at `origin` (device pixels): its backdrops and layers are placed from
/// it, so an edge the client lays on a whole pixel of its buffer is where the compositor puts its edge too. Placed from
/// the window's visual origin instead, which lies between two pixels when it is a half pixel off, a backdrop's edge
/// rounds the other way to the content's and a column of one shows beside a line the client draws along it.
fn surface_points(origin: Point<i32, Physical>, scale: f64) -> Point<f64, Logical> {
    Point::from((f64::from(origin.x) / scale, f64::from(origin.y) / scale))
}

/// Elements of one window's popups, front to back, each with its own backdrop directly below it: moved and scaled
/// with the window's plan. On the desktop the key window's go to [`SceneLayer::Popups`], above every window and the bars
/// ([`raised_popups_window`]).
fn window_popup_elements(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let scale = Scale::from(p.scale);
    let geo = entry.window.geometry();
    let visual_local = local(plan.visual, &p.rect);
    let v_phys = to_physical_rect(visual_local, scale);
    let origin = window_surface_origin(entry, plan, p);
    let Some(surface) = entry.window.wl_surface().map(|s| s.into_owned()) else {
        return out;
    };
    let alpha = plan.opacity as f32;
    // a dismissed popup its client still fades out is drawn as it lay, under the live ones
    for (popup, popup_offset) in crate::popup_linger::shown(&surface, p.now) {
        let offset: Point<i32, Physical> =
            (geo.loc + popup_offset - popup.geometry().loc).to_physical_precise_round(scale);
        let popup_origin = surface_points(origin + offset, p.scale);
        // the popup's own presentation (cupertino_layers_v1 version 3) moves, scales and fades it, then the window's
        // rescale applies
        let own = popup_presentation(p, &popup, popup_origin);
        let alpha = alpha * own.map_or(1.0, |(_, opacity)| opacity);
        let own = own.map(|(shown, _)| shown);
        if alpha <= 0.0 || own.is_some_and(|s| s.k.0 <= 0.0) {
            continue;
        }
        let mut tree = Tree {
            surface: popup.wl_surface().clone(),
            kind: SceneKind::PopupContent,
            role: SurfaceRole::Popup,
            origin: origin + offset,
            pivot: v_phys.loc,
            scale: (plan.scale_x, plan.scale_y),
            opacity: alpha,
            clip: Clip::None,
            only: None,
        };
        if let Some(own) = own {
            let window = Shown::about(visual_local.loc, (plan.scale_x, plan.scale_y));
            tree = own.then(window).tree(tree, p.scale);
        }
        let shown = |areas: Vec<BackdropArea>| match own {
            Some(s) => s.areas(areas),
            None => areas,
        };
        // the backdrops of the popup's subsurfaces lie between its slices, moved and scaled with the window
        out.extend(tree_pieces(p, tree, popup_origin, |p, areas, bound| {
            let areas = rescaled_backdrops(shown(areas), visual_local.loc, plan);
            let bound = rescaled_rect(own.map_or(bound, |s| s.rect(bound)), visual_local.loc, plan);
            bounded_glass(
                backdrop_elements(p, &areas, alpha, WindowState::Active),
                bound,
            )
        }));
        // the popup's own backdrop (a menu's material) lies directly below it, above the window
        let backdrops = rescaled_backdrops(
            shown(surface_backdrops(popup.wl_surface(), popup_origin)),
            visual_local.loc,
            plan,
        );
        out.extend(popup_backdrop_elements(p, &backdrops, alpha));
    }
    out
}

/// Elements of one window, front to back: the inner rim of a styled window its client decorates, decorations, content
/// (masked to the plan's corner radius, rescaled), its backdrops, outline and shadow (with `shadow`). Its popups are apart
/// ([`window_popup_elements`]).
fn window_elements(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
    shadow: bool,
    decorations: Vec<Piece>,
) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let scale = Scale::from(p.scale);
    let style = p.config.window;
    let visual_local = local(plan.visual, &p.rect);
    let v_phys = to_physical_rect(visual_local, scale);
    let origin = window_surface_origin(entry, plan, p);
    let Some(surface) = entry.window.wl_surface().map(|s| s.into_owned()) else {
        return out;
    };
    let alpha = plan.opacity as f32;

    out.extend(decorations);
    let styled = entry.styled(style.style_client_decorated);
    let radius = plan.capped(
        plan.corner
            .unwrap_or(style.corner_radius * plan.scale_x.min(plan.scale_y)),
    );
    let clip = if styled && radius > 0.0 {
        Clip::Rounded {
            rect: output_rect(visual_local),
            radius,
            style: corner_style(style.continuous_corners),
            exact: plan.exact,
        }
    } else if plan.exact {
        Clip::None
    } else {
        Clip::Rect(output_rect(visual_local))
    };
    // backdrops follow the window's rescale about its visual origin, and lie where the content lands
    let origin_local = surface_points(origin, p.scale);
    let state = if active {
        WindowState::Active
    } else {
        WindowState::Inactive
    };
    // glass in a window reads the window: inside its corners, where nothing of what is around it shows
    let corner = if styled { radius } else { 0.0 };
    let within = inset_rect(visual_local, corner * CORNER_CLEARANCE + 1.0);
    // the window's outline its layers are always drawn inside, glass included: its rounded clip, or its rectangle when
    // its content is not clipped by its corners (a window drawing its own decorations at rest), as a mask of no radius,
    // since a square clip leaves a surface tree as it is
    let layer_clip = match clip {
        Clip::Rounded { .. } => clip,
        Clip::None | Clip::Rect(_) => Clip::Rounded {
            rect: output_rect(visual_local),
            radius: 0.0,
            style: corner_style(false),
            exact: plan.exact,
        },
    };
    let backdrops_in = |p: &mut SceneParams<'_>, areas: Vec<BackdropArea>, clip: Clip| {
        let areas = rescaled_backdrops(areas, visual_local.loc, plan);
        let areas = if plan.exact {
            areas
        } else {
            cropped_backdrops(areas, visual_local)
        };
        let mut pieces = backdrop_elements(p, &areas, alpha, state);
        // a square backdrop follows the window's rounded corners where it reaches them, as the content does
        let outline = match clip {
            Clip::Rounded {
                rect,
                radius,
                style,
                ..
            } => Some(MaterialMask {
                rect: logical_rect(rect),
                radius_pt: radius,
                continuous: !style.is_circular(),
            }),
            Clip::Rect(rect) => Some(MaterialMask {
                rect: logical_rect(rect),
                radius_pt: 0.0,
                continuous: true,
            }),
            Clip::None => None,
        };
        for piece in &mut pieces {
            match piece {
                Piece::Glass(g) => {
                    let glass = (*g.glass).clone().within(within);
                    g.glass = Rc::new(match outline {
                        Some(mask) => glass.masked(mask),
                        None => glass,
                    });
                }
                Piece::Material(m) if outline.is_some() && m.radius <= 0.0 => m.clip = clip,
                _ => {}
            }
        }
        pieces
    };
    // the window's own backdrops keep the clip of its content
    let window_backdrops = |p: &mut SceneParams<'_>, areas: Vec<BackdropArea>| match clip {
        Clip::Rect(_) => backdrops_in(p, areas, Clip::None),
        c => backdrops_in(p, areas, c),
    };
    let tree = Tree {
        surface: surface.clone(),
        kind: SceneKind::WindowContent,
        role: SurfaceRole::WindowContent,
        origin,
        pivot: v_phys.loc,
        scale: (plan.scale_x, plan.scale_y),
        opacity: alpha,
        clip,
        only: None,
    };
    // its layers over it (cupertino_layers_v1), inside the window wherever they are placed, their glass too
    let place = super::layers::LayerPlace {
        origin: origin_local,
        pivot: visual_local.loc,
        scale: (plan.scale_x, plan.scale_y),
        opacity: alpha,
        clip: layer_clip,
        kind: SceneKind::WindowContent,
    };
    out.extend(super::layers::layer_pieces(
        p,
        &surface,
        &place,
        |p, areas| backdrops_in(p, areas, layer_clip),
    ));
    // the backdrops of its subsurfaces lie between the slices of the content, each reading the window below it
    out.extend(tree_pieces(p, tree, origin_local, |p, areas, bound| {
        let bound = rescaled_rect(bound, visual_local.loc, plan);
        bounded_glass(window_backdrops(p, areas), bound)
    }));
    let backdrops = window_backdrops(p, surface_backdrops(&surface, origin_local));
    out.extend(backdrops);
    // around the device pixels the content and its mask cover, so the window's edge meets them at every scale and
    // position
    let lines = WindowEdgeLines {
        index: entry.id.0,
        frame: smithay::utils::Rectangle::<f64, Logical>::new(
            (
                f64::from(v_phys.loc.x) / p.scale,
                f64::from(v_phys.loc.y) / p.scale,
            )
                .into(),
            (
                f64::from(v_phys.size.w) / p.scale,
                f64::from(v_phys.size.h) / p.scale,
            )
                .into(),
        ),
        radius,
        continuous: style.continuous_corners,
        scale: p.scale,
    };
    // a styled window its client decorates gets the light rim just inside its frame, over everything of the window, as
    // a decorated one has it from its rim buffer: whenever it is not full screen, at the window's opacity, and that of
    // an active window whatever the window's state
    let (rim, outline) = edge_lines(styled, entry.server_side, entry.fullscreen, shadow);
    if rim {
        lines.rim(&mut out, &p.decor.edge(true), alpha);
    }
    if outline {
        // the fine outline just outside the frame, over the shadow, fading with it (none on a full-screen window)
        lines.outline(&mut out, &p.decor.edge(active), alpha * plan.shadow as f32);
        out.extend(shadow_piece(p, entry.id, plan, active, plan.opacity));
    }
    out
}

/// Which lines of its edge the scene draws for a window: the inner rim of one `styled` that its client decorates (not
/// `server_side`), unless `fullscreen`, as the rim of the compositor's decorations; the outline of one `styled`, drawn
/// with its `shadow`.
fn edge_lines(styled: bool, server_side: bool, fullscreen: bool, shadow: bool) -> (bool, bool) {
    (styled && !server_side && !fullscreen, styled && shadow)
}

/// Where the lines of a styled window's edge lie: around its `frame` (output points) with corners of `radius`, at
/// `scale`.
struct WindowEdgeLines {
    index: u64,
    frame: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    scale: f64,
}

impl WindowEdgeLines {
    /// Put the inner rim of `edge` at `alpha` in front of `out` (the window's pieces, front to back).
    fn rim(&self, out: &mut Vec<Piece>, edge: &cupertino_decorations::Edge, alpha: f32) {
        if let Some(rim) = inner_rim_item(
            SceneKind::ClientRim,
            self.index,
            self.frame,
            self.radius,
            self.continuous,
            edge,
            self.scale,
            alpha,
        ) {
            out.insert(0, rim);
        }
    }

    /// Put the outline of `edge` at `alpha` behind `out` (the window's pieces, front to back).
    fn outline(&self, out: &mut Vec<Piece>, edge: &cupertino_decorations::Edge, alpha: f32) {
        out.extend(outline_item(
            SceneKind::Outline,
            self.index,
            self.frame,
            self.radius,
            self.continuous,
            edge,
            self.scale,
            alpha,
        ));
    }
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

/// `rect` moved and scaled with the window's plan about its visual origin, as [`rescaled_backdrops`] moves backdrops.
fn rescaled_rect(
    rect: smithay::utils::Rectangle<f64, Logical>,
    visual_origin: Point<f64, Logical>,
    plan: &WindowPlan,
) -> smithay::utils::Rectangle<f64, Logical> {
    let at = visual_origin
        + Point::from((
            (rect.loc.x - visual_origin.x) * plan.scale_x,
            (rect.loc.y - visual_origin.y) * plan.scale_y,
        ));
    smithay::utils::Rectangle::new(
        at,
        (rect.size.w * plan.scale_x, rect.size.h * plan.scale_y).into(),
    )
}

/// Backdrops of a window cropped to `visual` with the window's content: their right and bottom edges stop at its edges.
fn cropped_backdrops(
    backdrops: Vec<BackdropArea>,
    visual: smithay::utils::Rectangle<f64, Logical>,
) -> Vec<BackdropArea> {
    let (right, bottom) = (visual.loc.x + visual.size.w, visual.loc.y + visual.size.h);
    backdrops
        .into_iter()
        .filter_map(|mut b| {
            b.area.size.w = b.area.size.w.min(right - b.area.loc.x);
            b.area.size.h = b.area.size.h.min(bottom - b.area.loc.y);
            (b.area.size.w > 0.0 && b.area.size.h > 0.0).then_some(b)
        })
        .collect()
}

/// Title bar and rim of a server-side decorated window (front to back), rescaled with the window
/// and, for a stage placement ([`WindowPlan::corner`]), masked to its animated corners.
fn builtin_decorations(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<Piece> {
    let geo = entry.window.geometry();
    let ins = entry.insets();
    let (fw, fh) = (
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    if fw < 1.0 || fh < 1.0 || (ins.top <= 0.0 && entry.titlebar == TitlebarStyle::Standard) {
        return Vec::new();
    }
    // a frame animation crops the window to its frame
    let (fw, fh) = if (plan.scale_x, plan.scale_y) == (1.0, 1.0) {
        (fw.min(plan.visual.w), fh.min(plan.visual.h))
    } else {
        (fw, fh)
    };
    let (title, resizable, can_minimize, can_zoom) =
        p.wm.window(entry.id)
            .map(|w| {
                (
                    w.title.clone(),
                    w.min_size.zip(w.max_size).is_none_or(|(a, b)| a != b),
                    w.can_minimize(),
                    w.can_zoom(),
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
        can_minimize,
        can_zoom,
        flush: false,
    });
    let scale = Scale::from(p.scale);
    let v_phys = to_physical_rect(local(plan.visual, &p.rect), scale);
    let alpha = plan.opacity as f32;
    let loc = v_phys.loc.to_f64();
    let clip = plan.corner.map_or(Clip::None, |radius| Clip::Rounded {
        rect: output_rect(local(plan.visual, &p.rect)),
        radius: plan.capped(radius),
        style: corner_style(p.config.window.continuous_corners),
        exact: plan.exact,
    });
    let grid = |point: Point<f64, Physical>| {
        OutputPoint(cupertino_geometry::Point::new(
            point.x / p.scale,
            point.y / p.scale,
        ))
    };
    let pivot = grid(v_phys.loc.to_f64());
    let placement = |at: Point<f64, Physical>| Placement {
        origin: grid(at),
        pivot,
        scale_x: plan.scale_x,
        scale_y: plan.scale_y,
    };
    let picture = |buffer: &std::rc::Rc<super::chrome::ChromeImage>,
                   kind: SceneKind,
                   buffer_px: (i32, i32),
                   src: smithay::utils::Rectangle<f64, Logical>,
                   size: smithay::utils::Size<i32, Logical>,
                   at: Point<f64, Physical>| {
        Piece::Image(Picture {
            buffer: super::compose::ImageBuffer::Chrome(buffer.clone()),
            kind,
            index: entry.id.0,
            role: ImageRole::Decoration,
            buffer_px,
            src,
            size: (size.w, size.h),
            placement: placement(at),
            opacity: alpha,
            clip,
        })
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
    out.push(picture(
        &bufs.titlebar,
        SceneKind::TitleBar,
        bufs.titlebar_px,
        tb_src,
        size,
        loc,
    ));
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
    out.insert(
        0,
        picture(
            &bufs.rim,
            SceneKind::Rim,
            bufs.rim_px,
            src,
            rim_size,
            rim_loc,
        ),
    );
    out
}

/// The title bar of a full-screen window while the menu bar is revealed over it, across the output below the bar.
fn revealed_titlebar(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Option<Piece> {
    let strip = entry
        .revealed_titlebar(
            p.menu_bars,
            p.decor,
            p.config.window.decorations,
            p.output_id,
            p.now,
        )
        .filter(|_| plan.exact)?;
    let title =
        p.wm.window(entry.id)
            .map(|w| w.title.clone())
            .unwrap_or_default();
    // a full-screen window does not minimize; the zoom button is its way out
    let bufs = p.decor.buffers(&DecorInput {
        id: entry.id,
        style: WindowEntry::REVEALED_TITLEBAR,
        frame: (strip.w, strip.h),
        scale: p.scale,
        active,
        title: &title,
        resizable: false,
        can_minimize: false,
        can_zoom: true,
        flush: true,
    });
    let origin = OutputPoint(cupertino_geometry::Point::new(
        strip.x - p.rect.x,
        strip.y - p.rect.y,
    ));
    Some(Piece::Image(Picture {
        buffer: super::compose::ImageBuffer::Chrome(bufs.titlebar.clone()),
        kind: SceneKind::TitleBar,
        index: entry.id.0,
        role: ImageRole::Decoration,
        buffer_px: bufs.titlebar_px,
        src: smithay::utils::Rectangle::new(
            (0.0, 0.0).into(),
            (f64::from(bufs.titlebar_px.0), f64::from(bufs.titlebar_px.1)).into(),
        ),
        size: (strip.w.round() as i32, bufs.titlebar_pt.round() as i32),
        placement: Placement {
            origin,
            pivot: origin,
            scale_x: 1.0,
            scale_y: 1.0,
        },
        opacity: plan.opacity as f32,
        clip: Clip::None,
    }))
}

/// Per-window pieces of the decorations, front to back.
fn window_decorations(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<Piece> {
    let mut out = Vec::new();
    if entry.server_side && !entry.fullscreen && p.config.window.decorations {
        out.extend(builtin_decorations(p, entry, plan, active));
    }
    out
}

mod compose;
mod held_slices;
mod holes;
mod layers;
use compose::backdrop_elements;
use layers::{popup_backdrop_elements, popup_presentation};

pub use compose::compose_scene;
pub(crate) use compose::{glass_layer, material_layer};
use holes::held_hole_pieces;
pub(crate) use holes::hole_pieces;
pub use layers::{
    capture_held_surface, capture_surface, capture_window, gives_way, raised_popups_window,
    top_layer_covered, HeldBackdrops, SnapshotBackdrop,
};
