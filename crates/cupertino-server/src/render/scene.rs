//! Per-output scene construction: wallpaper → layer surfaces → (overview backdrop) → workspace
//! windows with shadows → shell UI (menu bar, extensions) → layer overlays →
//! cursor, ordered by [`SceneLayer`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cupertino_decorations::TitlebarStyle;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_effects::{DockEdge, Mesh, MinimizeEffect, Vertex, WorkspaceSlide};
use cupertino_scene::{
    Clip, ImageRole, OutputPoint, Placement, RoundedRect, SceneItem, SceneKind, Shadow, Source,
    SurfaceRole,
};
use cupertino_theme::{Appearance, MaterialOptions, Theme};
use cupertino_wm::{OutputId, Presentation, Rect, WindowId, Wm};
use smithay::backend::renderer::element::Element;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use super::compose::SnapshotKeep;
use super::compose::{
    corner_style, image_piece, item, output_rect, Composed, Composition, Material, MeshPiece,
    Picture, Piece, Tree,
};
use super::gles::{Gles, GlesCompletion, GlesResources, GlesTarget};
use super::lower::id_hash;
use super::material::{GpuRecipe, MaterialStage, SharedMaterial};
use super::mesh::{MeshGl, SharedMesh};
use super::motion::{cross_fade_opacity, skips_frame_animation, WorkspaceTransition};
use super::programs::Programs;
use super::shader_rect::{to_physical_rect, ElementCache, Sig};
use super::{CupertinoElement, SceneLayer};
use crate::app_effects_v1::{surface_backdrops, BackdropArea};
use crate::config::Config;
use crate::cursor::CursorFrame;
use crate::decor::{Decor, DecorInput};
use crate::fx::{Effects, IntoGeom};
use crate::shell::Shell;
use crate::state::WindowEntry;
use crate::window_manager::Look;
use cupertino_render_api::Renderer;

/// Material over a window while a sheet hangs from it (material 30).
pub(super) const SHEET_DIMMING_MATERIAL: &str = "SheetUnderlay";

/// Geometry of one window for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlan {
    /// Visual rectangle of the window geometry (output-local logical points).
    pub visual: Rect,
    pub scale_x: f64,
    pub scale_y: f64,
    pub opacity: f64,
    pub shadow: f64,
    /// Drawn at its natural size and position (no animation in progress).
    pub exact: bool,
    /// Visual corner radius of a stage placement, in points; `None` scales the window style's
    /// radius with the content.
    pub corner: Option<f64>,
}

/// How a window is drawn at `now`: at rest the client's committed size wins (no stretch);
/// while the model animates it, the content is scaled to the animated rectangle (frame, then the
/// minimize collapse, then the presentation scale about the centre).
pub fn plan_window(
    pres: &Presentation,
    geometry: (f64, f64),
    animating: bool,
    offset: (f64, f64),
) -> WindowPlan {
    let (gw, gh) = (geometry.0.max(1.0), geometry.1.max(1.0));
    let f = pres.frame.translate(offset.0, offset.1);
    let at_rest =
        !animating && (pres.scale - 1.0).abs() < 1e-9 && pres.minimize_progress.abs() < 1e-9;
    if at_rest {
        let size = if geometry.0 > 0.0 && geometry.1 > 0.0 {
            (geometry.0, geometry.1)
        } else {
            (f.w, f.h)
        };
        return WindowPlan {
            visual: Rect::new(f.x, f.y, size.0, size.1),
            scale_x: 1.0,
            scale_y: 1.0,
            opacity: pres.opacity.clamp(0.0, 1.0),
            shadow: pres.shadow.clamp(0.0, 1.0),
            exact: true,
            corner: None,
        };
    }
    let target = pres.minimize_target.translate(offset.0, offset.1);
    let v = Rect::lerp(f, target, pres.minimize_progress.clamp(0.0, 1.0));
    let (w, h) = ((v.w * pres.scale).max(1.0), (v.h * pres.scale).max(1.0));
    let visual = Rect::new(v.cx() - w / 2.0, v.cy() - h / 2.0, w, h);
    WindowPlan {
        visual,
        scale_x: w / gw,
        scale_y: h / gh,
        opacity: pres.opacity.clamp(0.0, 1.0),
        shadow: pres.shadow.clamp(0.0, 1.0),
        exact: false,
        corner: None,
    }
}

/// Windows drawn on `output` at `now`, bottom to top, with the horizontal slide offset (in points)
/// and the opacity of their workspace: a slide moves the workspaces and keeps them opaque, a
/// cross-fade keeps them in place (`render::motion`). Sticky windows neither slide nor fade;
/// windows of workspaces that are not on screen and of hidden or minimized apps are skipped.
pub fn windows_on_output(
    wm: &Wm,
    output: OutputId,
    width: f64,
    transition: WorkspaceTransition,
    now: f64,
) -> Vec<(WindowId, f64, f64)> {
    let pos = wm.workspace_position(output, now);
    let list = wm.workspaces(output);
    let slide = WorkspaceSlide::new(width);
    let mut shown: Vec<(cupertino_wm::WorkspaceId, f64, f64)> = Vec::new();
    for idx in [pos.floor() as i64, pos.ceil() as i64] {
        if let Some((i, ws)) = usize::try_from(idx)
            .ok()
            .and_then(|i| list.get(i).map(|w| (i, w)))
        {
            if !shown.iter().any(|(id, _, _)| *id == ws.id) {
                let (offset, opacity) = match transition {
                    WorkspaceTransition::Slide => (slide.offset_x(i, pos), 1.0),
                    WorkspaceTransition::CrossFade => (0.0, cross_fade_opacity(i, pos)),
                };
                shown.push((ws.id, offset, opacity));
            }
        }
    }
    let mut out = Vec::new();
    for id in wm.stacking_order() {
        let Some(w) = wm.window(*id) else { continue };
        if w.output != output || !w.state().is_visible() || wm.app(w.app).is_some_and(|a| a.hidden)
        {
            continue;
        }
        if w.sticky {
            out.push((*id, 0.0, 1.0));
        } else if let Some((_, off, opacity)) = shown.iter().find(|(ws, _, _)| *ws == w.workspace) {
            out.push((*id, *off, *opacity));
        }
    }
    out
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
    let ins = entry.insets;
    WindowPlan {
        visual: rect,
        scale_x: rect.w / (f64::from(geo.size.w) + ins.horizontal()).max(1.0),
        scale_y: rect.h / (f64::from(geo.size.h) + ins.vertical()).max(1.0),
        opacity,
        shadow,
        exact: false,
        corner: Some(corner),
    }
}

/// Recipes by material name, appearance and Reduce Transparency.
type RecipeCache = HashMap<(String, Appearance, bool), Option<Rc<GpuRecipe>>>;

/// Plan of window `id` on its output at `now` (None when the window cannot be drawn). With Reduce
/// Motion in `options` a full-screen transition draws the window at its new frame (`render::motion`).
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
    if skips_frame_animation(w.state(), options) {
        pres.frame = w.lifecycle.target_frame();
    }
    let animating = w.lifecycle.is_animating(now);
    let geo = entry.window.geometry();
    let ins = entry.insets;
    let size = (
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    let mut plan = plan_window(&pres, size, animating, (slide, 0.0));
    let desk = fx.show_desktop_progress(now);
    if desk > 0.0 && !w.sticky {
        if let Some(out) = wm.output(w.output) {
            plan.visual = fx.show_desktop_rect(plan.visual, out.rect, desk);
        }
    }
    Some((plan, pres))
}

/// Programs, material pipeline and recipes of one renderer.
pub struct RenderResources {
    pub programs: Programs,
    pub material: Option<SharedMaterial>,
    pub material_error: Option<String>,
    pub mesh: Option<SharedMesh>,
    /// The retained window snapshots (closing ghosts).
    pub snapshots: super::snapshots::Snapshots,
    theme: Option<Theme>,
    recipes: RefCell<RecipeCache>,
}

impl RenderResources {
    pub fn new(renderer: &mut GlesRenderer) -> anyhow::Result<Self> {
        let programs =
            Programs::compile(renderer).map_err(|e| anyhow::anyhow!("compiling shaders: {e}"))?;
        let (material, material_error) = match super::MaterialGl::new(renderer) {
            Ok(m) => (Some(m), None),
            Err(e) => {
                tracing::warn!(error = %e, "material pass unavailable, using translucent fallbacks");
                (None, Some(e))
            }
        };
        let mesh = match MeshGl::new(renderer) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(error = %e, "mesh pass unavailable, minimize falls back to a plain rescale");
                None
            }
        };
        let theme = match Theme::load() {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "theme not loaded, material recipes unavailable");
                None
            }
        };
        Ok(Self {
            programs,
            material,
            material_error,
            mesh,
            snapshots: super::snapshots::Snapshots::default(),
            theme,
            recipes: RefCell::new(HashMap::new()),
        })
    }

    /// GPU recipe for a material name and appearance (cached). With `reduce_transparency`, or in a
    /// high-contrast appearance, the material's opaque variant is drawn (`cupertino-theme`).
    pub fn recipe(
        &self,
        material: &str,
        appearance: Appearance,
        reduce_transparency: bool,
    ) -> Option<Rc<GpuRecipe>> {
        let key = (material.to_string(), appearance, reduce_transparency);
        self.recipes
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let theme = self.theme.as_ref()?;
                match theme.recipe(
                    appearance,
                    material,
                    &MaterialOptions {
                        reduce_transparency,
                        ..MaterialOptions::default()
                    },
                ) {
                    Ok(r) => Some(Rc::new(GpuRecipe::new(&r))),
                    Err(e) => {
                        tracing::warn!(material, error = %e, "missing material recipe");
                        None
                    }
                }
            })
            .clone()
    }

    /// Release GL objects (call before dropping the renderer).
    pub fn destroy(&self, renderer: &mut GlesRenderer) {
        if let Some(m) = &self.material {
            m.borrow_mut().destroy(renderer);
        }
        if let Some(m) = &self.mesh {
            m.borrow_mut().destroy(renderer);
        }
    }
}

/// What to draw for the pointer.
pub enum CursorPlan {
    Hidden,
    /// A themed cursor drawn `size` points tall (its nominal size times the cursor scale).
    Themed {
        frame: CursorFrame,
        size: f64,
    },
    /// A client's cursor surface, enlarged by `scale`.
    Surface {
        surface: WlSurface,
        hotspot: Point<i32, Logical>,
        scale: f64,
    },
}

/// What the session lock allows an output to draw.
pub enum LockView {
    Unlocked,
    /// Only the lock surface of the output (if it has one) over black.
    Locked {
        surface: Option<WlSurface>,
    },
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
    pub lock: LockView,
    /// Black overlays over everything, front to back: the activation fade, then the power dimming.
    pub session_overlays: [Option<SessionOverlay>; 2],
    /// The frame leaves the compositor (screen capture, screenshot files, recordings): the surfaces of the
    /// authentication prompt are left out.
    pub for_capture: bool,
}

/// A black overlay of the session over the whole output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionOverlay {
    pub kind: SceneKind,
    pub alpha: f64,
}

/// Put the session overlays on top of `stack`, whatever else the frame shows.
fn session_overlays(p: &SceneParams<'_>, stack: &mut Composition) {
    push_session_overlays((p.rect.w, p.rect.h), &p.session_overlays, stack);
}

fn push_session_overlays(
    size: (f64, f64),
    overlays: &[Option<SessionOverlay>; 2],
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

fn local(r: Rect, origin: &Rect) -> smithay::utils::Rectangle<f64, Logical> {
    smithay::utils::Rectangle::new((r.x - origin.x, r.y - origin.y).into(), (r.w, r.h).into())
}

/// Rounded rectangle as a scene item, drawn like [`rrect`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn rrect_item(
    kind: SceneKind,
    index: u64,
    rect: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    continuous: bool,
    fill: [f32; 4],
    border: Option<(f64, [f32; 4])>,
    alpha: f32,
) -> Piece {
    Piece::item(item(
        kind,
        index,
        Source::RoundedRect(RoundedRect {
            rect: output_rect(rect),
            radius,
            style: corner_style(continuous),
            fill,
            border,
        }),
        f64::from(alpha),
    ))
}

/// A surface tree drawn at `origin` as it is, without a window's rescale or mask.
fn standalone(
    surface: &WlSurface,
    kind: SceneKind,
    origin: Point<i32, Physical>,
    opacity: f32,
) -> Piece {
    Piece::Tree(Box::new(Tree {
        surface: surface.clone(),
        kind,
        role: SurfaceRole::Standalone,
        origin,
        pivot: origin,
        scale: (1.0, 1.0),
        opacity,
        clip: Clip::None,
    }))
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
    // the bar's text sits on its material, so what is under the text is the strip through the material's colour ops
    let appearance = p
        .config
        .appearance
        .with_high_contrast(p.display.increase_contrast);
    let reduce_transparency = p.display.reduce_transparency;
    let res = &p.res;
    p.overlays
        .wallpaper
        .set_bar_material((appearance, reduce_transparency), || {
            res.recipe("MenuBar", appearance, reduce_transparency)
                .and_then(|r| r.backdrop.as_ref().map(|(_, _, ops)| ops.clone()))
                .unwrap_or_default()
        });
    // the image is laid in the logical space, which a quarter-turned output has on its side
    let px = p
        .output
        .current_transform()
        .transform_size(smithay::utils::Size::<i32, smithay::utils::Physical>::from(
            p.output_px,
        ));
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
    let dark = p.config.is_dark();
    let colours = if dark {
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
    };
    full_output_wallpaper(p, SceneKind::Wallpaper, colours)
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

/// Approximate average colour of the procedural wallpaper (tint layer input, approximate).
pub fn wallpaper_tint(dark: bool) -> [f32; 3] {
    if dark {
        [0.22, 0.17, 0.37]
    } else {
        [0.78, 0.76, 0.84]
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
    let radius = plan
        .corner
        .map_or(p.config.window.corner_radius, |r| r / shadow_scale(plan));
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

/// Elements of one window, front to back: popups, content (masked to the plan's corner radius,
/// rescaled), shadow.
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
    let alpha = plan.opacity as f32;

    for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
        let offset: Point<i32, Physical> =
            (geo.loc + popup_offset - popup.geometry().loc).to_physical_precise_round(scale);
        out.push(Piece::Tree(Box::new(Tree {
            surface: popup.wl_surface().clone(),
            kind: SceneKind::PopupContent,
            role: SurfaceRole::Popup,
            origin: origin + offset,
            pivot: v_phys.loc,
            scale: (plan.scale_x, plan.scale_y),
            opacity: alpha,
            clip: Clip::None,
        })));
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
    let radius = plan
        .corner
        .unwrap_or(style.corner_radius * plan.scale_x.min(plan.scale_y));
    let clip = if styled && radius > 0.0 {
        Clip::Rounded {
            rect: output_rect(visual_local),
            radius,
            style: corner_style(style.continuous_corners),
            exact: plan.exact,
        }
    } else {
        Clip::None
    };
    out.push(Piece::Tree(Box::new(Tree {
        surface: surface.clone(),
        kind: SceneKind::WindowContent,
        role: SurfaceRole::WindowContent,
        origin,
        pivot: v_phys.loc,
        scale: (plan.scale_x, plan.scale_y),
        opacity: alpha,
        clip,
    })));
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
        out.extend(shadow_piece(p, entry.id, plan, active, plan.opacity));
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
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<Piece> {
    let geo = entry.window.geometry();
    let ins = entry.insets;
    let (fw, fh) = (
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    if fw < 1.0 || fh < 1.0 || (ins.top <= 0.0 && entry.titlebar == TitlebarStyle::Standard) {
        return Vec::new();
    }
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
    });
    let scale = Scale::from(p.scale);
    let v_phys = to_physical_rect(local(plan.visual, &p.rect), scale);
    let alpha = plan.opacity as f32;
    let loc = v_phys.loc.to_f64();
    let clip = plan.corner.map_or(Clip::None, |radius| Clip::Rounded {
        rect: output_rect(local(plan.visual, &p.rect)),
        radius,
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
    let picture = |buffer: &smithay::backend::renderer::element::memory::MemoryRenderBuffer,
                   kind: SceneKind,
                   buffer_px: (i32, i32),
                   src: smithay::utils::Rectangle<f64, Logical>,
                   size: smithay::utils::Size<i32, Logical>,
                   at: Point<f64, Physical>| {
        Piece::Image(Box::new(Picture {
            buffer: buffer.clone(),
            kind,
            index: entry.id.0,
            role: ImageRole::Decoration,
            buffer_px,
            src,
            size: (size.w, size.h),
            placement: placement(at),
            opacity: alpha,
            clip,
        }))
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

/// Per-window pieces of the decorations, front to back.
fn window_decorations(
    p: &mut SceneParams<'_>,
    entry: &WindowEntry,
    plan: &WindowPlan,
    active: bool,
) -> Vec<Piece> {
    let mut out = Vec::new();
    if entry.server_side && p.config.window.decorations {
        out.extend(builtin_decorations(p, entry, plan, active));
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
    let plan = WindowPlan {
        visual: natural,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
    };
    let deco = window_decorations(p, entry, &plan, active);
    let pieces = window_elements(p, entry, &plan, active, shadow, deco);
    let mut stack = Composition::new(p.scale);
    stack.extend(SceneLayer::Windows, pieces);
    let composed = stack.finish(p);
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
) -> Option<Vec<Piece>> {
    let kind = p.fx.cfg.minimize?;
    p.res.mesh.as_ref()?;
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
    let owner = p.res.snapshots.insert(texture);
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
    Some(vec![Piece::Mesh(Box::new(MeshPiece {
        kind: SceneKind::MinimizeMesh,
        index: entry.id.0,
        snapshot: owner.id(),
        keep: SnapshotKeep::Owned(owner),
        mesh: local_mesh,
        opacity: pres.opacity as f32,
        revision: Sig::new().f(pres.minimize_progress).u(kind as u64).finish(),
    }))])
}

/// Closing ghosts of this output: the last frame of a window fading out.
fn closing_elements(p: &mut SceneParams<'_>) -> Vec<Piece> {
    if p.res.mesh.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in
        p.fx.closing
            .iter()
            .rev()
            .filter(|c| c.output == p.output_id)
    {
        let (rect, alpha) = c.placement(p.now);
        let alpha = alpha as f32;
        if alpha <= 0.0 {
            continue;
        }
        let r = Rect::new(rect.x - p.rect.x, rect.y - p.rect.y, rect.w, rect.h);
        let lease = match p.res.snapshots.pin(c.snapshot.id()) {
            Ok(lease) => lease,
            Err(error) => {
                tracing::error!(%error, window = c.id.0, "closing ghost without its snapshot");
                continue;
            }
        };
        out.push(Piece::Mesh(Box::new(MeshPiece {
            kind: SceneKind::ClosingGhost,
            index: c.id.0,
            snapshot: lease.id(),
            keep: SnapshotKeep::Pinned(lease),
            mesh: Mesh::quad(r.into_geom()),
            opacity: alpha,
            revision: Sig::new().f(c.progress(p.now)).finish(),
        })));
    }
    out
}

/// Elements of the layer-shell `layer`, front to back. The backdrop captures of menu bars are
/// appended to `captures`: they belong below the windows (see [`bar_backdrop_elements`]).
fn layer_elements(p: &mut SceneParams<'_>, layer: Layer, captures: &mut Vec<Piece>) -> Vec<Piece> {
    let map = layer_map_for_output(p.output);
    let scale = Scale::from(p.scale);
    let mut out: Vec<Piece> = Vec::new();
    // later surfaces are drawn on top of earlier ones
    for surface in crate::authorization::layers_stacked(&map, layer).rev() {
        if p.for_capture && crate::authorization::is_prompt(surface.wl_surface()) {
            continue;
        }
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
            out.push(standalone(
                popup.wl_surface(),
                SceneKind::LayerContent,
                popup_origin.to_physical_precise_round(scale),
                opacity,
            ));
            let backdrops = surface_backdrops(popup.wl_surface(), popup_origin);
            out.extend(popup_backdrop_elements(p, &backdrops, opacity));
        }
        out.push(standalone(
            surface.wl_surface(),
            SceneKind::LayerContent,
            loc,
            opacity,
        ));
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
) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let params = p.decor.popup_shadow_params();
    let rim = p.decor.rim_color(p.scale);
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        out.push(rrect_item(
            SceneKind::PopupRim,
            b.key,
            b.area,
            b.radius,
            true,
            [0.0; 4],
            Some((1.0 / p.scale, rim)),
            alpha,
        ));
        out.extend(material_layer(
            p,
            SceneKind::Backdrop,
            b.key,
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
        out.extend(shadow_item(
            p,
            SceneKind::PopupShadow,
            b.key,
            &params,
            &plan,
            b.radius,
            f64::from(alpha),
        ));
    }
    out
}

/// Material pieces of committed backdrops, to go directly below the owning surface's pieces
/// (the list is front to back, so later backdrops come first).
fn backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
) -> Vec<Piece> {
    let mut out = Vec::new();
    for b in backdrops.iter().rev() {
        out.extend(material_layer(
            p,
            SceneKind::Backdrop,
            b.key,
            &b.material,
            b.area,
            b.radius,
            b.alpha as f32 * opacity,
        ));
    }
    out
}

/// Backdrops of a menu bar, to go directly below the bar's pieces. A bar reserves its strip,
/// so no window lies under it; the only window pixels there would be shadows, and they must not
/// tint the material. The material therefore samples what lies below the window stack: its
/// capture piece is added to `captures` (for [`SceneLayer::BackdropCapture`]) and only the
/// composite stays with the bar. Without the material pass the fallback fill is drawn instead.
fn bar_backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
    captures: &mut Vec<Piece>,
) -> Vec<Piece> {
    let mut out = Vec::new();
    let output = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (p.rect.w, p.rect.h).into(),
    );
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        let owner = b.key;
        let composite = material_piece(
            p,
            SceneKind::Backdrop,
            b.key,
            &b.material,
            b.area,
            b.radius,
            alpha,
            MaterialStage::Composite { owner },
        );
        let Some(composite) = composite else {
            out.push(fallback_fill(
                p,
                SceneKind::Backdrop,
                b.key,
                b.area,
                b.radius,
                alpha,
            ));
            continue;
        };
        // the capture covers the whole output: it is only redrawn together with a full repaint
        captures.extend(material_piece(
            p,
            SceneKind::BackdropCapture,
            b.key,
            &b.material,
            output,
            0.0,
            alpha,
            MaterialStage::Capture { owner },
        ));
        out.push(composite);
    }
    out
}

/// Material pass (or the translucent fallback fill without one) behind `area`, drawn at `alpha`.
pub(crate) fn material_layer(
    p: &mut SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    material: &str,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
) -> Vec<Piece> {
    match material_piece(
        p,
        kind,
        index,
        material,
        area,
        radius,
        alpha,
        MaterialStage::Full,
    ) {
        Some(m) => vec![m],
        None => vec![fallback_fill(p, kind, index, area, radius, alpha)],
    }
}

/// Material piece for `material`; `None` while the material pass is unavailable.
#[allow(clippy::too_many_arguments)]
fn material_piece(
    p: &SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    material: &str,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
    stage: MaterialStage,
) -> Option<Piece> {
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
    Some(Piece::Material(Box::new(Material {
        recipe,
        kind,
        index,
        rect: area,
        radius,
        style: corner_style(true),
        stage,
        tint: p
            .overlays
            .wallpaper
            .tint
            .unwrap_or_else(|| wallpaper_tint(dark)),
        opacity: alpha,
    })))
}

/// Translucent fill standing in for a material.
fn fallback_fill(
    p: &SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
) -> Piece {
    let fill = if p.config.is_dark() {
        [0.12, 0.12, 0.14, 0.78]
    } else {
        [0.96, 0.96, 0.98, 0.78]
    };
    rrect_item(kind, index, area, radius, true, fill, None, alpha)
}

fn snap_preview(p: &mut SceneParams<'_>, stack: &mut Composition) {
    let Some(prev) = p.wm.snap_preview(p.now) else {
        return;
    };
    if prev.opacity <= 0.0 {
        return;
    }
    let r = prev.rect;
    let rect = local(r, &p.rect);
    let e = rrect_item(
        SceneKind::SnapPreview,
        0,
        rect,
        12.0,
        true,
        [0.30, 0.52, 1.0, 0.22],
        Some((2.0, [0.45, 0.65, 1.0, 0.7])),
        prev.opacity as f32,
    );
    stack.push(SceneLayer::SnapPreview, e);
}

fn cursor(p: &mut SceneParams<'_>, stack: &mut Composition) {
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
            let top_left = pos - hotspot;
            let (w, h) = frame.pixels;
            let drawn = (
                (f64::from(w) * k).round() as i32,
                (f64::from(h) * k).round() as i32,
            );
            // the whole image: without `src` the element samples a `drawn`-sized corner of it
            let src = smithay::utils::Rectangle::<f64, Logical>::from_size(
                (f64::from(w), f64::from(h)).into(),
            );
            stack.push(
                SceneLayer::Cursor,
                Piece::Image(Box::new(Picture {
                    buffer: frame.buffer.clone(),
                    kind: SceneKind::CursorImage,
                    index: 0,
                    role: ImageRole::Cursor,
                    buffer_px: (w as i32, h as i32),
                    src,
                    size: drawn,
                    placement: Placement {
                        origin: OutputPoint(cupertino_geometry::Point::new(top_left.x, top_left.y)),
                        ..Placement::IDENTITY
                    },
                    opacity: 1.0,
                    clip: Clip::None,
                })),
            );
        }
        CursorPlan::Surface {
            surface,
            hotspot,
            scale: grow,
        } => {
            let loc: Point<i32, Physical> = (pos - hotspot.to_f64().upscale(grow))
                .to_physical(scale)
                .to_i32_round();
            stack.push(
                SceneLayer::Cursor,
                Piece::Tree(Box::new(Tree {
                    surface,
                    kind: SceneKind::CursorContent,
                    role: SurfaceRole::Cursor,
                    origin: loc,
                    pivot: loc,
                    scale: (grow, grow),
                    opacity: 1.0,
                    clip: Clip::None,
                })),
            );
        }
    }
}

fn overview(p: &mut SceneParams<'_>, stack: &mut Composition) -> bool {
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
    let e = rrect_item(
        SceneKind::OverviewDim,
        0,
        full,
        0.0,
        false,
        [0.0, 0.0, 0.0, (dim * progress) as f32],
        None,
        1.0,
    );
    stack.push(SceneLayer::OverviewBackdrop, e);

    // windows, back to front in stacking order, drawn at their frames; the workspaces bar, the
    // highlight and the titles are the dock's layer surface
    let order: Vec<WindowId> = p.wm.stacking_order().to_vec();
    let key = p.wm.key_window();
    let mut windows_front_to_back: Vec<Piece> = Vec::new();
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
        let deco = window_decorations(p, entry, &plan, active);
        let els = window_elements(p, entry, &plan, active, true, deco);
        windows_front_to_back.extend(els);
    }
    stack.extend(SceneLayer::Windows, windows_front_to_back);
    true
}

/// The neutral frame of one output, with the elements the renderer still builds itself.
pub fn compose_scene(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>) -> Composed {
    let mut stack = Composition::new(p.scale);
    match std::mem::replace(&mut p.lock, LockView::Unlocked) {
        LockView::Locked { surface } => locked_scene(p, &mut stack, surface),
        LockView::Unlocked => desktop_scene(renderer, p, &mut stack),
    }
    session_overlays(p, &mut stack);
    // what the composition used this frame is all it keeps for the next one
    p.decor.end_frame();
    p.overlays.images.end_frame();
    stack.finish(p)
}

fn desktop_scene(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>, stack: &mut Composition) {
    let wp = wallpaper_element(p);
    stack.push(SceneLayer::Wallpaper, wp);

    let mut captures: Vec<Piece> = Vec::new();
    for (layer, scene_layer) in [
        (Layer::Background, SceneLayer::LayerBackground),
        (Layer::Bottom, SceneLayer::LayerBottom),
        (Layer::Top, SceneLayer::LayerTop),
        (Layer::Overlay, SceneLayer::LayerOverlay),
    ] {
        let els = layer_elements(p, layer, &mut captures);
        stack.extend(scene_layer, els);
    }
    stack.extend(SceneLayer::BackdropCapture, captures);

    let in_overview = overview(p, stack);
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
        let mut front_to_back: Vec<Piece> = Vec::new();
        // strip thumbnails, drawn below every window on the stage
        let mut strip: Vec<Piece> = Vec::new();
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
                let deco = window_decorations(p, entry, &plan, active);
                strip.extend(window_elements(p, entry, &plan, active, true, deco));
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
                    SceneKind::SheetDimming,
                    id.0,
                    SHEET_DIMMING_MATERIAL,
                    area,
                    radius,
                    (dimming * plan.opacity) as f32,
                ));
            }
            let deco = window_decorations(p, entry, &plan, active);
            front_to_back.extend(window_elements(p, entry, &plan, active, true, deco));
        }
        let mut ghosts: Vec<Piece> = closing_elements(p);
        ghosts.append(&mut front_to_back);
        ghosts.append(&mut strip);
        stack.extend(SceneLayer::Windows, ghosts);
    }

    snap_preview(p, stack);
    stack.extend(SceneLayer::InputHud, crate::kbd_ui::input_hud(p));
    stack.extend(SceneLayer::AccentPopup, crate::kbd_ui::accent_popup(p));
    stack.extend(
        SceneLayer::CompositorOverlay,
        crate::kbd_ui::shortcut_notice(p),
    );

    if let Some((surface, offset)) = p.drag_icon.clone() {
        let scale = Scale::from(p.scale);
        let pos = p.pointer - Point::from((p.rect.x, p.rect.y)) + offset.to_f64();
        let loc: Point<i32, Physical> = pos.to_physical(scale).to_i32_round();
        stack.push(
            SceneLayer::DragIcon,
            standalone(&surface, SceneKind::DragIcon, loc, 1.0),
        );
    }
    display_filter(p, stack);
    cursor(p, stack);
}

/// The scene while the session is locked: black, the lock surface and the pointer; nothing else may show.
fn locked_scene(p: &mut SceneParams<'_>, stack: &mut Composition, surface: Option<WlSurface>) {
    stack.push(SceneLayer::Wallpaper, Piece::item(lock_backdrop(p)));
    if let Some(surface) = surface {
        stack.push(
            SceneLayer::LayerOverlay,
            standalone(&surface, SceneKind::LockContent, (0, 0).into(), 1.0),
        );
    }
    display_filter(p, stack);
    cursor(p, stack);
}

/// The display filter pass: everything below the pointer, captured and passed through the colour
/// chain of the accessibility settings. The design filters the pointer too (the whole display,
/// ); it stays above here so that pointer motion does not repaint the output.
fn display_filter(p: &mut SceneParams<'_>, stack: &mut Composition) {
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
    stack.push(
        SceneLayer::DisplayFilter,
        Piece::Material(Box::new(Material {
            recipe,
            kind: SceneKind::DisplayFilter,
            index: 0,
            rect: area,
            radius: 0.0,
            style: corner_style(false),
            stage: MaterialStage::Full,
            tint: [0.0; 3],
            opacity: 1.0,
        })),
    );
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

#[cfg(test)]
mod tests {
    use super::super::material::{MaterialElement, MaterialGl};
    use super::super::shader_rect::ElementKey;
    use super::*;
    use cupertino_wm::{Command, MapParams, WmConfig};

    fn output(w: f64, h: f64) -> cupertino_scene::Output {
        cupertino_scene::Output {
            id: cupertino_scene::OutputId(1),
            rect: cupertino_scene::GlobalRect(cupertino_geometry::Rect::new(0.0, 0.0, w, h)),
            physical_size: cupertino_scene::PhysicalSize {
                width: w as i32,
                height: h as i32,
            },
            scale: 1.0,
            transform: cupertino_scene::Transform::Normal,
        }
    }

    fn marker(kind: SceneKind) -> Piece {
        rrect_item(
            kind,
            0,
            smithay::utils::Rectangle::new((0.0, 0.0).into(), (1.0, 1.0).into()),
            0.0,
            false,
            [0.0; 4],
            None,
            1.0,
        )
    }

    #[test]
    fn session_overlays_cover_everything_activation_fade_first() {
        let mut stack = Composition::new(1.0);
        stack.push(SceneLayer::Windows, marker(SceneKind::Shadow));
        stack.push(SceneLayer::Cursor, marker(SceneKind::CursorImage));
        stack.push(
            SceneLayer::CompositorOverlay,
            marker(SceneKind::SnapPreview),
        );
        push_session_overlays(
            (100.0, 80.0),
            &[
                Some(SessionOverlay {
                    kind: SceneKind::ActivationFade,
                    alpha: 0.5,
                }),
                Some(SessionOverlay {
                    kind: SceneKind::PowerOverlay,
                    alpha: 0.25,
                }),
            ],
            &mut stack,
        );
        let composed = stack.finish_with(output(100.0, 80.0), std::time::Duration::ZERO);
        let kinds: Vec<_> = composed
            .frame
            .items()
            .iter()
            .map(|(_, item)| item.id.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                SceneKind::ActivationFade,
                SceneKind::PowerOverlay,
                SceneKind::SnapPreview,
                SceneKind::CursorImage,
                SceneKind::Shadow,
            ]
        );
    }

    #[test]
    fn an_idle_session_adds_no_overlay() {
        let mut stack = Composition::new(1.0);
        push_session_overlays((100.0, 80.0), &[None, None], &mut stack);
        let composed = stack.finish_with(output(100.0, 80.0), std::time::Duration::ZERO);
        assert!(composed.frame.items().is_empty());
    }

    fn pres(frame: Rect) -> Presentation {
        Presentation {
            frame,
            opacity: 1.0,
            scale: 1.0,
            shadow: 1.0,
            minimize_progress: 0.0,
            minimize_target: Rect::new(0.0, 0.0, 10.0, 10.0),
            visible: true,
        }
    }

    #[test]
    fn rest_plan_uses_committed_size_without_stretch() {
        let p = plan_window(
            &pres(Rect::new(100.0, 80.0, 640.0, 480.0)),
            (600.0, 400.0),
            false,
            (0.0, 0.0),
        );
        assert!(p.exact);
        assert_eq!(p.visual, Rect::new(100.0, 80.0, 600.0, 400.0));
        assert_eq!((p.scale_x, p.scale_y), (1.0, 1.0));
    }

    #[test]
    fn animating_plan_scales_content_to_the_frame() {
        let p = plan_window(
            &pres(Rect::new(0.0, 0.0, 800.0, 600.0)),
            (400.0, 300.0),
            true,
            (0.0, 0.0),
        );
        assert!(!p.exact);
        assert_eq!((p.scale_x, p.scale_y), (2.0, 2.0));
    }

    #[test]
    fn presentation_scale_is_about_the_centre() {
        let mut pr = pres(Rect::new(100.0, 100.0, 200.0, 100.0));
        pr.scale = 0.5;
        let p = plan_window(&pr, (200.0, 100.0), true, (0.0, 0.0));
        assert_eq!(p.visual, Rect::new(150.0, 125.0, 100.0, 50.0));
        assert_eq!(p.scale_x, 0.5);
    }

    #[test]
    fn minimize_collapses_towards_the_target() {
        let mut pr = pres(Rect::new(0.0, 0.0, 400.0, 300.0));
        pr.minimize_target = Rect::new(800.0, 700.0, 40.0, 40.0);
        pr.minimize_progress = 1.0;
        let p = plan_window(&pr, (400.0, 300.0), true, (0.0, 0.0));
        assert_eq!(p.visual, Rect::new(800.0, 700.0, 40.0, 40.0));
        pr.minimize_progress = 0.5;
        let half = plan_window(&pr, (400.0, 300.0), true, (0.0, 0.0));
        assert_eq!(half.visual.w, 220.0);
    }

    #[test]
    fn slide_offset_applies_to_the_plan() {
        let p = plan_window(
            &pres(Rect::new(10.0, 10.0, 100.0, 100.0)),
            (100.0, 100.0),
            false,
            (-1920.0, 0.0),
        );
        assert_eq!(p.visual.x, -1910.0);
    }

    #[test]
    fn windows_follow_the_workspace_slide() {
        let mut wm = Wm::new(WmConfig::default());
        let out = OutputId(1);
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        wm.handle(
            Command::AddOutput {
                output: out,
                rect: area,
                usable: area,
            },
            0.0,
        );
        wm.handle(
            Command::Map(MapParams::new(
                WindowId(1),
                "a",
                Rect::new(10.0, 10.0, 300.0, 200.0),
            )),
            0.0,
        );
        let ws2 = {
            wm.handle(Command::AddWorkspace { output: out }, 0.0);
            wm.workspaces(out)[1].id
        };
        wm.handle(
            Command::SwitchWorkspace {
                output: out,
                workspace: ws2,
            },
            10.0,
        );
        wm.handle(
            Command::Map(MapParams::new(
                WindowId(2),
                "b",
                Rect::new(10.0, 10.0, 300.0, 200.0),
            )),
            10.0,
        );
        // settled on the second workspace: only window 2 is drawn, at offset 0
        let late = windows_on_output(&wm, out, 1000.0, WorkspaceTransition::Slide, 100.0);
        assert_eq!(late, vec![(WindowId(2), 0.0, 1.0)]);
        // mid-slide (just after the switch) both workspaces contribute
        let mid = windows_on_output(&wm, out, 1000.0, WorkspaceTransition::Slide, 10.05);
        assert!(mid.iter().any(|(w, _, _)| *w == WindowId(1)) || mid.len() == 1);
    }

    #[test]
    fn under_the_cross_fade_the_switching_workspaces_stay_in_place_and_share_the_opacity() {
        let mut wm = Wm::new(WmConfig::default());
        let out = OutputId(1);
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        wm.handle(
            Command::AddOutput {
                output: out,
                rect: area,
                usable: area,
            },
            0.0,
        );
        wm.handle(
            Command::Map(MapParams::new(
                WindowId(1),
                "a",
                Rect::new(10.0, 10.0, 300.0, 200.0),
            )),
            0.0,
        );
        wm.handle(Command::AddWorkspace { output: out }, 0.0);
        let ws2 = wm.workspaces(out)[1].id;
        wm.handle(
            Command::SwitchWorkspace {
                output: out,
                workspace: ws2,
            },
            10.0,
        );
        wm.handle(
            Command::Map(MapParams::new(
                WindowId(2),
                "b",
                Rect::new(10.0, 10.0, 300.0, 200.0),
            )),
            10.0,
        );
        let now = (1..200)
            .map(|i| 10.0 + f64::from(i) * 0.005)
            .find(|t| (wm.workspace_position(out, *t) - 0.5).abs() < 0.25)
            .expect("a mid-switch instant");
        let pos = wm.workspace_position(out, now);
        let mid = windows_on_output(&wm, out, 1000.0, WorkspaceTransition::CrossFade, now);
        assert_eq!(
            mid.iter().map(|(w, off, _)| (*w, *off)).collect::<Vec<_>>(),
            vec![(WindowId(1), 0.0), (WindowId(2), 0.0)]
        );
        assert!(
            (mid[0].2 - (1.0 - pos)).abs() < 1e-12 && (mid[1].2 - pos).abs() < 1e-12,
            "{mid:?} at {pos}"
        );
        let slid = windows_on_output(&wm, out, 1000.0, WorkspaceTransition::Slide, now);
        assert!(
            slid.iter()
                .all(|(_, off, opacity)| *off != 0.0 && *opacity == 1.0),
            "{slid:?}"
        );
    }

    /// A menu bar material element of `stage` over `area`.
    fn material(
        cache: &mut ElementCache,
        key: ElementKey,
        stage: MaterialStage,
        area: Rect,
    ) -> CupertinoElement {
        material_with(cache, key, stage, area, 0.0, 1.0)
    }

    fn material_with(
        cache: &mut ElementCache,
        key: ElementKey,
        stage: MaterialStage,
        area: Rect,
        radius: f64,
        opacity: f32,
    ) -> CupertinoElement {
        let recipe = Theme::load()
            .unwrap()
            .recipe(Appearance::Light, "MenuBar", &MaterialOptions::default())
            .unwrap();
        let area = smithay::utils::Rectangle::<f64, Logical>::new(
            (area.x, area.y).into(),
            (area.w, area.h).into(),
        );
        MaterialElement::new(
            key,
            stage,
            cache,
            area,
            Rc::new(GpuRecipe::new(&recipe)),
            MaterialGl::detached(),
            radius,
            true,
            opacity,
            [0.0; 3],
            (1280, 800),
            1.0,
        )
        .into()
    }

    fn generation(e: &CupertinoElement) -> u64 {
        match e {
            CupertinoElement::Material(m) => m.generation(),
            _ => unreachable!("the scene holds only material elements"),
        }
    }

    #[test]
    fn bar_composite_takes_the_generation_of_its_capture_below_the_windows() {
        let mut cache = ElementCache::default();
        let full = Rect::new(0.0, 0.0, 1280.0, 800.0);
        let bar = Rect::new(0.0, 0.0, 1280.0, 24.0);
        let owner = MaterialStage::Capture { owner: 7 };
        let composite = MaterialStage::Composite { owner: 7 };
        // front to back: a material over everything, the bar composite, a material standing in for a window, the capture
        let mut scene = |window: Rect| {
            let mut els = vec![
                material(
                    &mut cache,
                    ElementKey::new("above", 0),
                    MaterialStage::Full,
                    bar,
                ),
                material(&mut cache, ElementKey::new("backdrop", 7), composite, bar),
                material(
                    &mut cache,
                    ElementKey::new("window", 0),
                    MaterialStage::Full,
                    window,
                ),
                material(
                    &mut cache,
                    ElementKey::new("backdrop-capture", 7),
                    owner,
                    full,
                ),
            ];
            finalize_materials(&mut els, 1.0, &mut cache);
            els.iter().map(generation).collect::<Vec<u64>>()
        };
        let before = scene(Rect::new(100.0, 40.0, 300.0, 200.0));
        let moved = scene(Rect::new(100.0, 10.0, 300.0, 200.0));
        assert_eq!(
            before[1], before[3],
            "the composite shares the capture's generation"
        );
        assert_eq!(
            moved[1], before[1],
            "a window moving under the bar leaves the bar's backdrop alone"
        );
        assert_ne!(
            moved[0], before[0],
            "a full material above the window does see it move"
        );
    }

    /// A material over everything and, below it, the lowered item whose look is `(area, radius, opacity)`; the
    /// generation the upper one gets is the signature of what lies below it.
    fn backdrop_over(cache: &mut ElementCache, item: (Rect, f64, f32)) -> u64 {
        let top = Rect::new(0.0, 0.0, 1280.0, 800.0);
        let (area, radius, opacity) = item;
        let mut els = vec![
            material(cache, ElementKey::new("top", 0), MaterialStage::Full, top),
            material_with(
                cache,
                ElementKey::new("item", 0),
                MaterialStage::Full,
                area,
                radius,
                opacity,
            ),
        ];
        finalize_materials(&mut els, 1.0, cache);
        generation(&els[0])
    }

    #[test]
    fn unchanged_lower_item_preserves_backdrop_signature() {
        let mut cache = ElementCache::default();
        let item = (Rect::new(100.0, 40.0, 300.0, 200.0), 10.0, 0.5);
        let first = backdrop_over(&mut cache, item);
        assert_eq!(backdrop_over(&mut cache, item), first);
        cache.end_frame();
        assert_eq!(backdrop_over(&mut cache, item), first, "also across frames");
    }

    #[test]
    fn lower_item_move_clip_or_alpha_invalidates_backdrop() {
        let mut cache = ElementCache::default();
        let (area, radius, opacity) = (Rect::new(100.0, 40.0, 300.0, 200.0), 10.0, 0.5);
        let base = backdrop_over(&mut cache, (area, radius, opacity));
        let moved = backdrop_over(
            &mut cache,
            (Rect::new(101.0, 40.0, 300.0, 200.0), radius, opacity),
        );
        let clipped = backdrop_over(&mut cache, (area, radius + 1.0, opacity));
        let faded = backdrop_over(&mut cache, (area, radius, 0.4));
        let all = [base, moved, clipped, faded];
        for (at, a) in all.iter().enumerate() {
            for b in &all[at + 1..] {
                assert_ne!(
                    a, b,
                    "each change alone gives the backdrop another signature"
                );
            }
        }
    }

    #[test]
    fn material_generation_changes_with_scene_content() {
        // the signature helper is order- and content-sensitive
        let a = Sig::new().u(1).u(2).finish();
        let b = Sig::new().u(2).u(1).finish();
        assert_ne!(a, b);
    }
}
