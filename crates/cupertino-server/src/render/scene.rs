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
    BorderFalloff, Clip, ImageRole, OutputPoint, Placement, RoundedRect, SceneItem, SceneKind,
    Shadow, Source, SurfaceRole,
};
use cupertino_theme::glass::adapts_foreground;
use cupertino_theme::{Appearance, MaterialOptions, Theme, WindowState};
use cupertino_wm::{OutputId, Presentation, Rect, WindowId, Wm};
use smithay::backend::renderer::element::Element;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{Anchor, Layer};

use super::compose::SnapshotKeep;
use super::compose::{
    corner_style, image_piece, item, logical_rect, output_rect, Composed, Composition, Material,
    MeshPiece, Picture, Piece, Tree, TreeSlice,
};
use super::gles::{Gles, GlesCompletion, GlesResources, GlesTarget};
use super::lower::id_hash;
use super::material::glass::{
    backdrop_generation, BackdropRecord, Below, GlassContainer, ToneProbe,
};
use super::material::{GpuRecipe, MaterialMask, MaterialStage, SharedMaterial};
use super::mesh::{MeshGl, SharedMesh};
use super::motion::{
    cross_fade_opacity, skips_frame_animation, skips_sheet_slide, WorkspaceTransition,
};
use super::programs::Programs;
use super::shader_rect::{to_physical_rect, ElementCache, Sig};
use super::{CupertinoElement, SceneLayer};
use crate::app_effects_v1::{
    raised_parts, subsurface_backdrops, surface_backdrops, BackdropArea, TreePart,
};
use crate::config::Config;
use crate::cursor::CursorFrame;
use crate::decor::{Decor, DecorInput};
use crate::fx::{Effects, IntoGeom};
use crate::shell::Shell;
use crate::state::{Shrink, WindowEntry};
use crate::window_manager::Look;
use cupertino_glass::glass::{Container, GlassShape};
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
    /// Visual corner radius of a stage placement, in points, or 0 for a full-screen window
    /// ([`cupertino_wm::FrameShape::Square`]); `None` scales the window style's radius with the content.
    pub corner: Option<f64>,
    /// The most the corner radius may be, in points, whatever `corner` says: while a window fills its output or leaves
    /// it, the room between its frame and the output's edges ([`fullscreen_corner_cap`]), so the corners round off as
    /// the frame meets the edges instead of squaring at the start of the transition. `None` is no limit.
    pub corner_cap: Option<f64>,
}

/// The corner limit of a frame `frame` filling or leaving `output`: the largest gap between one of the frame's edges
/// and the output's edge beside it. It is zero once the frame covers the output and grows as the frame moves away.
pub fn fullscreen_corner_cap(frame: Rect, output: Rect) -> f64 {
    [
        (frame.x - output.x).abs(),
        (frame.y - output.y).abs(),
        (frame.right() - output.right()).abs(),
        (frame.bottom() - output.bottom()).abs(),
    ]
    .into_iter()
    .fold(0.0, f64::max)
}

impl WindowPlan {
    /// The radius `radius` within [`WindowPlan::corner_cap`].
    pub fn capped(&self, radius: f64) -> f64 {
        self.corner_cap
            .map_or(radius, |cap| radius.min(cap.max(0.0)))
    }
}

/// How a window is drawn at `now`: at rest the client's committed size wins (no stretch); while
/// the model animates only its frame, the content keeps its size from the frame's top-left corner,
/// cropped to the frame (a buffer that does not reach the frame yet holds the window at its size);
/// while it minimizes or takes a presentation scale, the content is scaled to the animated
/// rectangle (frame, then the minimize collapse, then the scale about the centre).
pub fn plan_window(
    pres: &Presentation,
    geometry: (f64, f64),
    animating: bool,
    offset: (f64, f64),
) -> WindowPlan {
    let (gw, gh) = (geometry.0.max(1.0), geometry.1.max(1.0));
    let f = pres.frame.translate(offset.0, offset.1);
    let unscaled = (pres.scale - 1.0).abs() < 1e-9 && pres.minimize_progress.abs() < 1e-9;
    if unscaled && animating && geometry.0 > 0.0 && geometry.1 > 0.0 {
        return WindowPlan {
            visual: Rect::new(f.x, f.y, f.w.min(gw), f.h.min(gh)),
            scale_x: 1.0,
            scale_y: 1.0,
            opacity: pres.opacity.clamp(0.0, 1.0),
            shadow: pres.shadow.clamp(0.0, 1.0),
            exact: false,
            corner: None,
            corner_cap: None,
        };
    }
    if unscaled && !animating {
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
            corner_cap: None,
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
        corner_cap: None,
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

/// Recipes by material name, appearance and Reduce Transparency.
/// The variant of a material a backdrop draws: the opaque one with Reduce Transparency, and the inactive one for a
/// window that is not active.
fn material_options(reduce_transparency: bool, window: WindowState) -> MaterialOptions {
    MaterialOptions {
        reduce_transparency,
        window,
        ..MaterialOptions::default()
    }
}

/// The materials of a window's own background and sidebars, which take a slight tint of the wallpaper's colour.
const TINTED_MATERIALS: [&str; 3] = ["WindowBackground", "TextBackground", "Sidebar"];
/// How strongly they take it.
const WINDOW_TINT_OPACITY: f32 = 0.03;

/// The share of what is still see-through that the densest setting fills.
const DENSITY_FILL_SHARE: f32 = 0.3;

/// `recipe` made denser by `amount` (0 to 1): its ordinary fills grow more opaque, and a recipe that is only a blur
/// gets a faint veil of the appearance's own grey.
fn densify(recipe: &mut cupertino_theme::recipe::Recipe, dark: bool, amount: f32) {
    use cupertino_theme::recipe::{Blend, Fill};
    let mut grew = false;
    for fill in &mut recipe.fills {
        if fill.blend == Blend::Normal
            && !fill.tint_layer
            && fill.rgba[3] > 0.0
            && fill.rgba[3] < 1.0
        {
            fill.rgba[3] += (1.0 - fill.rgba[3]) * DENSITY_FILL_SHARE * amount;
            grew = true;
        }
    }
    if !grew && recipe.backdrop.is_some() {
        let grey = if dark { 0.12 } else { 0.95 };
        recipe.fills.push(Fill {
            rgba: [grey, grey, grey, DENSITY_FILL_SHARE * amount * 0.5],
            blend: Blend::Normal,
            tint_layer: false,
        });
    }
}

/// Recipes by material, appearance, Reduce Transparency and whether the window is inactive.
type RecipeCache = HashMap<(String, Appearance, bool, bool, u8, bool), Option<Rc<GpuRecipe>>>;

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
    /// How much denser every material is made, in percent ([`Config::material_density`](crate::config::Config)).
    density: std::cell::Cell<u8>,
    /// Windows' own backgrounds and sidebars take a slight tint of the wallpaper's colour.
    tint_windows: std::cell::Cell<bool>,
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
            density: std::cell::Cell::new(0),
            tint_windows: std::cell::Cell::new(true),
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
        self.window_recipe(
            material,
            appearance,
            reduce_transparency,
            WindowState::Active,
        )
    }

    /// [`Self::recipe`] for a backdrop of a window in `window` state: an inactive window draws the material's
    /// inactive variant.
    pub fn window_recipe(
        &self,
        material: &str,
        appearance: Appearance,
        reduce_transparency: bool,
        window: WindowState,
    ) -> Option<Rc<GpuRecipe>> {
        let density = self.density.get();
        let tint = self.tint_windows.get() && TINTED_MATERIALS.contains(&material);
        let key = (
            material.to_string(),
            appearance,
            reduce_transparency,
            window == WindowState::Inactive,
            density,
            tint,
        );
        self.recipes
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let theme = self.theme.as_ref()?;
                match theme.recipe(
                    appearance,
                    material,
                    &material_options(reduce_transparency, window),
                ) {
                    Ok(mut r) => {
                        if tint && !r.uses_tint_layer() {
                            r.fills.push(cupertino_theme::recipe::Fill {
                                rgba: [0.0, 0.0, 0.0, WINDOW_TINT_OPACITY],
                                blend: cupertino_theme::recipe::Blend::Normal,
                                tint_layer: true,
                            });
                        }
                        if density > 0 && !r.opaque {
                            densify(&mut r, appearance.is_dark(), f32::from(density) / 100.0);
                        }
                        Some(Rc::new(GpuRecipe::new(&r)))
                    }
                    Err(e) => {
                        tracing::warn!(material, error = %e, "missing material recipe");
                        None
                    }
                }
            })
            .clone()
    }

    /// Whether windows take a slight tint of the wallpaper's colour from now on.
    pub fn set_tint_windows(&self, on: bool) {
        self.tint_windows.set(on);
    }

    /// Make every material denser by `percent` (0 to 100) from now on.
    pub fn set_density(&self, percent: u8) {
        self.density.set(percent.min(100));
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
            border_falloff: BorderFalloff::default(),
        }),
        f64::from(alpha),
    ))
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
    Piece::Tree(Box::new(standalone_tree(surface, kind, origin, opacity)))
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
        return vec![Piece::Tree(Box::new(tree))];
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
                out.push(Piece::Tree(Box::new(Tree {
                    surface: tree.surface.clone(),
                    only: Some(TreeSlice { ids, index }),
                    ..tree
                })));
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
    // the image is laid in the logical space, which a quarter-turned output has on its side
    let px = p
        .output
        .current_transform()
        .transform_size(smithay::utils::Size::<i32, smithay::utils::Physical>::from(
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

/// Approximate average colour of the procedural wallpaper (tint layer input, approximate).
pub fn wallpaper_tint(dark: bool) -> [f32; 3] {
    if dark {
        [0.22, 0.17, 0.37]
    } else {
        [0.78, 0.76, 0.84]
    }
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
    let ins = entry.insets();
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
        let popup_origin = Point::<f64, Logical>::from((
            visual_local.loc.x + ins.left + f64::from(popup_offset.x - popup.geometry().loc.x),
            visual_local.loc.y + ins.top + f64::from(popup_offset.y - popup.geometry().loc.y),
        ));
        let tree = Tree {
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
        // the backdrops of the popup's subsurfaces lie between its slices, moved and scaled with the window
        out.extend(tree_pieces(p, tree, popup_origin, |p, areas, bound| {
            let areas = rescaled_backdrops(areas, visual_local.loc, plan);
            let bound = rescaled_rect(bound, visual_local.loc, plan);
            bounded_glass(
                backdrop_elements(p, &areas, alpha, WindowState::Active),
                bound,
            )
        }));
        // the popup's own backdrop (a menu's material) lies directly below it, above the window
        let backdrops = rescaled_backdrops(
            surface_backdrops(popup.wl_surface(), popup_origin),
            visual_local.loc,
            plan,
        );
        out.extend(popup_backdrop_elements(p, &backdrops, alpha));
    }
    out
}

/// Elements of one window, front to back: decorations, content (masked to the plan's corner radius, rescaled), its
/// backdrops, outline and shadow. Its popups are apart ([`window_popup_elements`]).
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
    let ins = entry.insets();
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
    // backdrops follow the window's rescale about its visual origin
    let origin_local = Point::<f64, Logical>::from((
        visual_local.loc.x + ins.left - f64::from(geo.loc.x),
        visual_local.loc.y + ins.top - f64::from(geo.loc.y),
    ));
    let state = if active {
        WindowState::Active
    } else {
        WindowState::Inactive
    };
    // glass in a window reads the window: inside its corners, where nothing of what is around it shows
    let corner = if styled { radius } else { 0.0 };
    let within = inset_rect(visual_local, corner * CORNER_CLEARANCE + 1.0);
    let window_backdrops = |p: &mut SceneParams<'_>, areas: Vec<BackdropArea>| {
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
            _ => None,
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
    // the backdrops of its subsurfaces lie between the slices of the content, each reading the window below it
    out.extend(tree_pieces(p, tree, origin_local, |p, areas, bound| {
        let bound = rescaled_rect(bound, visual_local.loc, plan);
        bounded_glass(window_backdrops(p, areas), bound)
    }));
    let backdrops = window_backdrops(p, surface_backdrops(&surface, origin_local));
    out.extend(backdrops);
    if shadow && styled {
        // the fine outline just outside the frame, over the shadow, fading with it (none on a full-screen window);
        // around the device pixels the content and its mask cover, so it meets them at every scale and position
        let edge = p.decor.edge(active);
        let frame = smithay::utils::Rectangle::<f64, Logical>::new(
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
        );
        out.extend(outline_item(
            SceneKind::Outline,
            entry.id.0,
            frame,
            radius,
            style.continuous_corners,
            &edge,
            p.scale,
            alpha * plan.shadow as f32,
        ));
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
    Some(Piece::Image(Box::new(Picture {
        buffer: bufs.titlebar.clone(),
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
    })))
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
    let composed = stack.finish(p);
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

/// The icon of a client's drag that ended without a drop, flying back or fading out (`crate::drag_return`).
fn drag_return_element(p: &mut SceneParams<'_>, stack: &mut Composition) {
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
        Piece::Mesh(Box::new(MeshPiece {
            kind: SceneKind::DragIcon,
            index: 0,
            snapshot: lease.id(),
            keep: SnapshotKeep::Pinned(lease),
            mesh: Mesh::quad(local.into_geom()),
            opacity: alpha as f32,
            revision: Sig::new().f(r.progress(p.now)).finish(),
        })),
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
/// such as the outside-click catcher of the launcher or Control Center, which must still take the click that closes them.
pub fn gives_way(surface: &LayerSurface, menu_bars: &crate::menu_bar::MenuBars) -> bool {
    !menu_bars.has_surface(surface.wl_surface()) && surface.cached_state().anchor != Anchor::all()
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
    let kind = super::motion::minimize_kind(p.fx.cfg.minimize, p.display)?;
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
        opacity: (pres.opacity * super::motion::minimize_opacity(pres.minimize_progress, p.display))
            as f32,
        revision: Sig::new()
            .f(pres.minimize_progress)
            .u(kind as u64)
            .u(u64::from(p.display.reduce_motion))
            .finish(),
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
/// appended to `captures`: they belong below the windows (see [`bar_backdrop_elements`]). The popups of a layer below
/// the overlay layer (a menu bar's menus, a dock's menus) are appended to `popups`, for [`SceneLayer::Popups`] above
/// every window and every bar; an overlay surface keeps its popups directly above it.
fn layer_elements(
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
        let overview_bar = p
            .shell
            .overview
            .as_ref()
            .filter(|o| o.output == p.output_id)
            .map_or(1.0, |o| o.menu_bar_alpha(p.now) as f32);
        let (dy, opacity) = look.map_or((0.0, 1.0), |l| (l.dy, l.alpha * overview_bar));
        if opacity <= 0.0 {
            continue;
        }
        let origin: Point<f64, Logical> = geo.loc.to_f64() + Point::from((0.0, dy));
        let loc: Point<i32, Physical> = origin.to_physical_precise_round(scale);
        // popups above their parent, each with its own backdrop directly below it
        let mut own = Vec::new();
        for (popup, popup_offset) in crate::popup_linger::shown(surface.wl_surface(), p.now) {
            let popup_origin = origin + (popup_offset - popup.geometry().loc).to_f64();
            let tree = standalone_tree(
                popup.wl_surface(),
                SceneKind::LayerContent,
                popup_origin.to_physical_precise_round(scale),
                opacity,
            );
            own.extend(tree_pieces(p, tree, popup_origin, |p, areas, bound| {
                bounded_glass(
                    backdrop_elements(p, &areas, opacity, WindowState::Active),
                    bound,
                )
            }));
            let backdrops = surface_backdrops(popup.wl_surface(), popup_origin);
            own.extend(popup_backdrop_elements(p, &backdrops, opacity));
        }
        if layer == Layer::Overlay {
            out.append(&mut own);
        } else {
            popups.append(&mut own);
        }
        let tree = standalone_tree(surface.wl_surface(), SceneKind::LayerContent, loc, opacity);
        out.extend(tree_pieces(p, tree, origin, |p, areas, bound| {
            bounded_glass(
                backdrop_elements(p, &areas, opacity, WindowState::Active),
                bound,
            )
        }));
        if layer == Layer::Overlay && surface.namespace() == crate::dock::OVERVIEW_NAMESPACE {
            // the workspaces' live pictures lie on the chrome's band, under what it draws
            out.extend(overview_pictures(p, opacity));
        }
        let backdrops = surface_backdrops(surface.wl_surface(), origin);
        if look.is_some() {
            out.extend(bar_backdrop_elements(p, &backdrops, opacity, captures));
        } else {
            out.extend(backdrop_elements(
                p,
                &backdrops,
                opacity,
                WindowState::Active,
            ));
        }
    }
    out
}

/// Elements of popups that carry a material, to go directly below the popup's surface (front to back:
/// material or glass, outline, shadow). The outline is the menu edge (`Decor::menu_edge`) just outside the shape, the
/// shadow the panel row of the table (it is cut out under the popup, so the material stays translucent). `backdrops`
/// are in output-local logical coordinates.
fn popup_backdrop_elements(
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
const SMALL_CALLOUT_FACE_DARK: [f32; 4] = [1.0, 1.0, 1.0, 0.05];

/// The opacity and the width in points of the black outline along the outside of a popover.
const POPOVER_OUTLINE_DARK: (f32, f32) = (0.45, 1.0);
const POPOVER_OUTLINE_LIGHT: (f32, f32) = (0.33, 1.0);

/// The soft band of light under the top and bottom edges of a light large panel, in the same terms: wide, so it also
/// lifts the face where the blur of the backdrop outside the panel darkens it near the edge.
const LARGE_PANEL_GLOW_LIGHT: (f32, f32, f32) = (0.075, 7.5, 0.0);

/// The shortest side a shape of a callout has to be for the callout to cast the panel shadow under it: the label's
/// capsule and a pointer cast none.
const CALLOUT_SHADOW_MIN_SIDE: f64 = 40.0;

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

/// Material pieces of committed backdrops, to go directly below the owning surface's pieces
/// (the list is front to back, so later backdrops come first), in the variant of a window in `window` state.
fn backdrop_elements(
    p: &mut SceneParams<'_>,
    backdrops: &[BackdropArea],
    opacity: f32,
    window: WindowState,
) -> Vec<Piece> {
    note_not_glass(p, backdrops);
    let mut out = Vec::new();
    let mut glass = GlassGroups::default();
    for b in backdrops.iter().rev() {
        if b.glass.is_some() {
            glass.add(b, out.len());
            continue;
        }
        out.extend(window_material_layer(
            p,
            SceneKind::Backdrop,
            b.key,
            &b.material,
            b.area,
            b.radius,
            b.alpha as f32 * opacity,
            window,
        ));
    }
    glass.finish(p, out, opacity)
}

/// Note the backdrops drawn as a material other than glass: one whose face was measured while it was glass goes back
/// to the appearance's tone ([`crate::app_effects_v1::BackdropTones`]).
fn note_not_glass(p: &SceneParams<'_>, backdrops: &[BackdropArea]) {
    if let Some(m) = &p.res.material {
        let plain = backdrops
            .iter()
            .filter(|b| b.glass.is_none())
            .map(|b| b.key);
        m.borrow_mut().note_unadapted(plain);
    }
}

/// Glass backdrops of one surface joined per container while its backdrops are laid out front to back: a container
/// is drawn where its frontmost backdrop is.
#[derive(Default)]
struct GlassGroups<'a> {
    /// The position in the pieces, the container and the backdrops of each group.
    groups: Vec<(usize, Option<u32>, Vec<&'a BackdropArea>)>,
}

impl<'a> GlassGroups<'a> {
    /// Add glass backdrop `b`; a new container keeps the place of the next piece of the list, `at`.
    fn add(&mut self, b: &'a BackdropArea, at: usize) {
        let Some(spec) = b.glass else {
            return;
        };
        let container = (spec.container != 0).then_some(spec.container);
        if let Some(group) = container.and_then(|c| {
            self.groups
                .iter_mut()
                .find(|(_, other, _)| *other == Some(c))
        }) {
            group.2.push(b);
            return;
        }
        self.groups.push((at, container, vec![b]));
    }

    /// `out` with each container's piece at its place.
    fn finish(self, p: &SceneParams<'_>, out: Vec<Piece>, opacity: f32) -> Vec<Piece> {
        self.place(out, |members| glass_pieces(p, members, opacity, false))
    }

    /// `out` with each container's piece at its place, for the glass of a popup (a menu, a popover): its rim is lit
    /// apart.
    fn finish_popup(self, p: &SceneParams<'_>, out: Vec<Piece>, opacity: f32) -> Vec<Piece> {
        self.place(out, |members| glass_pieces(p, members, opacity, true))
    }

    /// `out` with the pieces `make` gives each container (its backdrops front to back) at the container's place.
    fn place(
        self,
        out: Vec<Piece>,
        mut make: impl FnMut(&[&BackdropArea]) -> Vec<Piece>,
    ) -> Vec<Piece> {
        if self.groups.is_empty() {
            return out;
        }
        let mut at = self.groups.into_iter().peekable();
        let mut all = Vec::with_capacity(out.len() + 1);
        for (i, piece) in out.into_iter().enumerate() {
            while let Some((_, _, members)) = at.next_if(|g| g.0 == i) {
                all.extend(make(&members));
            }
            all.push(piece);
        }
        for (_, _, members) in at {
            all.extend(make(&members));
        }
        all
    }
}

/// The glass piece of the container of `members` (glass backdrops of one surface, front to back), or translucent fills
/// standing in for it while the material pass is unavailable. A `popup`'s glass takes the popup face, and its rim is
/// drawn apart ([`inner_rim_item`]).
fn glass_pieces(
    p: &SceneParams<'_>,
    members: &[&BackdropArea],
    opacity: f32,
    popup: bool,
) -> Vec<Piece> {
    let Some(spec) = members.first().and_then(|b| b.glass) else {
        return Vec::new();
    };
    let mode = cupertino_theme::GlassMode {
        density: (p.config.material_density * 100.0).round() as u8,
        ..crate::accessibility::glass_mode(p.config.is_dark(), spec.style, p.display)
    };
    // the shapes back to front, as the surface created them
    let shapes = members
        .iter()
        .rev()
        .map(|b| {
            GlassShape::new(
                [
                    b.area.loc.x as f32,
                    b.area.loc.y as f32,
                    b.area.size.w as f32,
                    b.area.size.h as f32,
                ],
                b.radius as f32,
            )
        })
        .collect();
    let mut container = Container::new(shapes, spec.spacing as f32, mode);
    // a callout (a panel and its pointer, a label and its pointer) is one piece of glass that lights its own rim, and a
    // small one keeps the glass's face
    let callout = members.len() > 1 && spec.container != 0;
    if !callout {
        use cupertino_theme::GlassStyle;
        match spec.style {
            // the dark panel's recipe holds its bevel and the soft band under it
            GlassStyle::LargePanel if !mode.dark => {
                let h = &mut container.params.highlight;
                (h.glow_amount, h.glow_height, h.glow_offset) = LARGE_PANEL_GLOW_LIGHT;
            }
            // with Reduce Transparency or Increase Contrast the popup's rim is drawn apart (`inner_rim_item`), lit all
            // round, in place of the glass's own
            _ if popup && (mode.reduce_transparency || mode.increase_contrast) => {
                container.params.highlight.amount = 0.0
            }
            _ => {}
        }
    }
    // a popover's outline, which a callout's one piece of glass draws itself
    if popup
        && callout
        && matches!(
            spec.style,
            cupertino_theme::GlassStyle::Dense | cupertino_theme::GlassStyle::Menu
        )
    {
        let (opacity, width) = if mode.dark {
            POPOVER_OUTLINE_DARK
        } else {
            POPOVER_OUTLINE_LIGHT
        };
        container.params.shadow.outline_opacity = opacity;
        container.params.shadow.outline_width = width;
    }
    if popup {
        let small = container
            .bounds()
            .is_some_and(|b| b[2].min(b[3]) < CALLOUT_SHADOW_MIN_SIDE as f32);
        let own_face = spec.style == cupertino_theme::GlassStyle::Popover;
        if callout
            && small
            && !own_face
            && !mode.reduce_transparency
            && !mode.increase_contrast
            && mode.dark
        {
            // a small dark callout (a label) is a veil a little lighter than the plain glass; popover glass keeps its
            // own face
            container.params.face.fill = SMALL_CALLOUT_FACE_DARK;
        }
    }
    let alpha = members.iter().map(|b| b.alpha).fold(0.0, f64::max) as f32 * opacity;
    let index = members[0].key;
    let ready = p
        .res
        .material
        .as_ref()
        .is_some_and(|m| m.borrow().is_ready());
    if !ready {
        return glass_fallback(p, SceneKind::Backdrop, index, &container, alpha);
    }
    let probes = tone_probes(members, mode.dark);
    if probes.is_empty() {
        if let Some(m) = &p.res.material {
            m.borrow_mut().note_unadapted(members.iter().map(|b| b.key));
        }
    }
    vec![Piece::Glass(Box::new(super::compose::Glass {
        glass: Rc::new(GlassContainer::new(container).with_probes(probes)),
        kind: SceneKind::Backdrop,
        index,
        opacity: alpha,
    }))]
}

/// The glass backdrops of a container (`members`) whose text follows what lies behind them: every one when the
/// container's bounds are small enough ([`adapts_foreground`]), none otherwise, so the shapes of a larger panel keep
/// the appearance together. A dark card (`dark`) never follows: its face stays a mid-dark pane over any backdrop, so
/// its text keeps the light tone and every card of a column reads alike. Nor does a large panel's glass, even where a
/// piece of it is small (the launcher's search field): its strong tone compression keeps the face in its appearance's
/// range, so its text keeps the appearance's tone over a bright scene and the field reads with the panel it opens.
fn tone_probes(members: &[&BackdropArea], dark: bool) -> Vec<ToneProbe> {
    use cupertino_theme::GlassStyle;
    let fixed = members.iter().any(|b| {
        b.glass.is_some_and(|g| {
            g.style == GlassStyle::LargePanel || (dark && g.style == GlassStyle::Card)
        })
    });
    let Some(bounds) = members
        .iter()
        .map(|b| b.area)
        .reduce(|a, b| a.merge(b))
        .filter(|_| !fixed)
    else {
        return Vec::new();
    };
    if !adapts_foreground(bounds.size.w.min(bounds.size.h) as f32) {
        return Vec::new();
    }
    members
        .iter()
        .map(|b| ToneProbe {
            serial: b.key,
            rect: [
                b.area.loc.x as f32,
                b.area.loc.y as f32,
                b.area.size.w as f32,
                b.area.size.h as f32,
            ],
        })
        .collect()
}

/// Translucent fills standing in for a glass container, one per shape.
fn glass_fallback(
    p: &SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    container: &Container,
    alpha: f32,
) -> Vec<Piece> {
    container
        .shapes
        .iter()
        .rev()
        .enumerate()
        .map(|(i, s)| {
            let rect = smithay::utils::Rectangle::<f64, Logical>::new(
                (f64::from(s.rect[0]), f64::from(s.rect[1])).into(),
                (f64::from(s.rect[2]), f64::from(s.rect[3])).into(),
            );
            let index = index.wrapping_add((i as u64) << 48);
            fallback_fill(p, kind, index, rect, f64::from(s.fitted_radius()), alpha)
        })
        .collect()
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
    note_not_glass(p, backdrops);
    let mut out = Vec::new();
    let output = smithay::utils::Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (p.rect.w, p.rect.h).into(),
    );
    let mut glass = GlassGroups::default();
    for b in backdrops.iter().rev() {
        let alpha = b.alpha as f32 * opacity;
        let owner = b.key;
        if b.glass.is_some() {
            glass.add(b, out.len());
            continue;
        }
        let composite = material_piece(
            p,
            SceneKind::Backdrop,
            b.key,
            &b.material,
            b.area,
            b.radius,
            alpha,
            MaterialStage::Composite { owner },
            WindowState::Active,
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
            WindowState::Active,
        ));
        out.push(composite);
    }
    glass.finish(p, out, opacity)
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
    window_material_layer(
        p,
        kind,
        index,
        material,
        area,
        radius,
        alpha,
        WindowState::Active,
    )
}

/// [`material_layer`] for a backdrop of a window in `window` state.
#[allow(clippy::too_many_arguments)]
fn window_material_layer(
    p: &mut SceneParams<'_>,
    kind: SceneKind,
    index: u64,
    material: &str,
    area: smithay::utils::Rectangle<f64, Logical>,
    radius: f64,
    alpha: f32,
    window: WindowState,
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
        window,
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
    window: WindowState,
) -> Option<Piece> {
    let dark = p.config.is_dark();
    let appearance = p
        .config
        .appearance
        .with_high_contrast(p.display.increase_contrast);
    let (shared, recipe) = (
        p.res.material.clone()?,
        p.res
            .window_recipe(material, appearance, p.display.reduce_transparency, window)?,
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
        tint: window_tint(p, dark),
        opacity: alpha,
        clip: Clip::None,
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
            let top_left = pos - frame.hotspot_at(size);
            let (w, h) = frame.pixels;
            let logical = frame.size_at(size);
            let drawn = (logical.0.round() as i32, logical.1.round() as i32);
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
                    only: None,
                })),
            );
        }
    }
}

/// The live pictures of the workspaces the dock's overview asked for (`overview_workspace_frame`), front to back: each
/// an opaque black backing, the wallpaper and, for a workspace other than the current one (whose windows the overview
/// itself shows), its windows, all scaled from the output into the picture's frame, at `alpha`.
fn overview_pictures(p: &mut SceneParams<'_>, alpha: f32) -> Vec<Piece> {
    let Some(session) = p
        .shell
        .overview
        .as_ref()
        .filter(|s| s.output == p.output_id && !s.closing)
    else {
        return Vec::new();
    };
    let pictures = session.pictures.clone();
    let current = p.wm.current_workspace(p.output_id);
    let out_rect = p.rect;
    let mut out = Vec::new();
    for (workspace, frame) in pictures {
        let k = frame.w / out_rect.w.max(1.0);
        let local_frame = local(frame, &out_rect);
        // windows of the workspace, front first
        if Some(workspace) != current {
            let order: Vec<WindowId> = p.wm.stacking_order().to_vec();
            for id in order.iter().rev() {
                let Some(w) = p.wm.window(*id) else { continue };
                if w.output != p.output_id
                    || w.workspace != workspace
                    || w.sticky
                    || w.state().is_minimized()
                    || w.state().is_dead()
                {
                    continue;
                }
                let Some(entry) = p.windows.iter().find(|e| e.id == *id) else {
                    continue;
                };
                if p.for_capture && crate::control::protected_surface(entry.surface()) {
                    continue;
                }
                let g = w.geometry();
                let visual = cupertino_wm::Rect::new(
                    frame.x + (g.x - out_rect.x) * k,
                    frame.y + (g.y - out_rect.y) * k,
                    g.w * k,
                    g.h * k,
                );
                let geo = entry.window.geometry();
                let ins = entry.insets();
                let plan = WindowPlan {
                    visual,
                    scale_x: visual.w / (f64::from(geo.size.w) + ins.horizontal()).max(1.0),
                    scale_y: visual.h / (f64::from(geo.size.h) + ins.vertical()).max(1.0),
                    opacity: f64::from(alpha),
                    shadow: 0.0,
                    exact: false,
                    corner: None,
                    corner_cap: None,
                };
                let deco = window_decorations(p, entry, &plan, false);
                out.extend(window_elements(p, entry, &plan, false, false, deco));
            }
        }
        // the wallpaper, scaled into the frame
        let index = 1 + workspace.0;
        match wallpaper_element(p) {
            Piece::Image(mut picture) => {
                picture.index = index;
                picture.size = (
                    local_frame.size.w.round() as i32,
                    local_frame.size.h.round() as i32,
                );
                picture.placement.origin = OutputPoint(cupertino_geometry::Point::new(
                    local_frame.loc.x,
                    local_frame.loc.y,
                ));
                picture.opacity = alpha;
                out.push(Piece::Image(picture));
            }
            _ => {
                let colours = match p.config.wallpaper_color {
                    Some(c) => {
                        let rgb = c.map(|c| f32::from(c) / 255.0);
                        [[rgb[0], rgb[1], rgb[2], 1.0]; 4]
                    }
                    None => wallpaper_colours(p.config.is_dark()),
                };
                out.push(Piece::item(item(
                    SceneKind::Wallpaper,
                    index,
                    Source::Wallpaper {
                        rect: output_rect(local_frame),
                        colours,
                    },
                    f64::from(alpha),
                )));
            }
        }
        // the opaque black backing
        out.push(rrect_item(
            SceneKind::OverviewDim,
            index,
            local_frame,
            0.0,
            false,
            [0.0, 0.0, 0.0, 1.0],
            None,
            alpha,
        ));
    }
    out
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
    // under Reduce Motion the windows do not move: they fade out of their desktop places and into their overview
    // places (`crossfade`), each drawn at its target
    let reduced = session.reduce_motion;
    let crossfade = session.crossfade(now);
    let progress = if reduced {
        crossfade
    } else {
        session.transition.progress_at(now).clamp(0.0, 1.0)
    };
    let placed = if reduced {
        session.targets()
    } else {
        session.frames(now)
    };
    let origins: Vec<(WindowId, cupertino_wm::Rect)> =
        session.windows.iter().map(|w| (w.id, w.from)).collect();
    let dragged = session.dragged;
    let dim = p.config.shell.overview_dim;

    if dim > 0.0 {
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
    }

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
        if p.for_capture && crate::control::protected_surface(entry.surface()) {
            continue;
        }
        let geo = entry.window.geometry();
        let ins = entry.insets();
        // under Reduce Motion a window that has not faded in yet is seen at its desktop place, fading out
        let (frame, opacity) = if reduced && crossfade < 0.5 {
            (
                origins
                    .iter()
                    .find(|o| o.0 == *id)
                    .map_or(pw.frame, |o| o.1),
                1.0 - 2.0 * crossfade,
            )
        } else if reduced {
            (pw.frame, 2.0 * crossfade - 1.0)
        } else {
            (pw.frame, 1.0)
        };
        let plan = WindowPlan {
            visual: frame,
            scale_x: frame.w / (f64::from(geo.size.w) + ins.horizontal()).max(1.0),
            scale_y: frame.h / (f64::from(geo.size.h) + ins.vertical()).max(1.0),
            opacity: opacity.clamp(0.0, 1.0),
            shadow: 1.0,
            exact: false,
            corner: None,
            corner_cap: None,
        };
        let active = key == Some(*id);
        let deco = window_decorations(p, entry, &plan, active);
        let mut els = window_popup_elements(p, entry, &plan);
        els.extend(window_elements(p, entry, &plan, active, true, deco));
        if dragged == Some(*id) {
            // a dragged window goes over the chrome, so it can be taken onto the desktops band
            stack.extend(SceneLayer::DragIcon, els);
        } else {
            // a window keeps its popups over it while it is a preview
            windows_front_to_back.extend(els);
        }
    }
    stack.extend(SceneLayer::Windows, windows_front_to_back);
    true
}

/// The neutral frame of one output, with the elements the renderer still builds itself.
pub fn compose_scene(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>) -> Composed {
    p.res
        .set_density((p.config.material_density * 100.0).round() as u8);
    p.res.set_tint_windows(p.config.tint_windows);
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
    // front to back: the popups of the layer surfaces, the higher layer's first, then those of the windows
    let mut popups: Vec<Piece> = Vec::new();
    for (layer, scene_layer) in [
        (Layer::Overlay, SceneLayer::LayerOverlay),
        (Layer::Top, SceneLayer::LayerTop),
        (Layer::Bottom, SceneLayer::LayerBottom),
        (Layer::Background, SceneLayer::LayerBackground),
    ] {
        let els = layer_elements(p, layer, &mut captures, &mut popups);
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
        let raised = raised_popups_window(p.wm, p.output_id);
        let mut front_to_back: Vec<Piece> = Vec::new();
        // strip thumbnails, drawn below every window on the stage
        let mut strip: Vec<Piece> = Vec::new();
        for (id, slide, workspace_opacity) in list.into_iter().rev() {
            let Some(entry) = p.windows.iter().find(|w| w.id == id) else {
                continue;
            };
            if p.for_capture && crate::control::protected_surface(entry.surface()) {
                continue;
            }
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
                strip.extend(window_popup_elements(p, entry, &plan));
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
            let mut deco = window_decorations(p, entry, &plan, active);
            deco.extend(revealed_titlebar(p, entry, &plan, active));
            // the key window's popups go above every window and the bars; another window's stay over it
            let own_popups = window_popup_elements(p, entry, &plan);
            if raised == Some(id) {
                popups.extend(own_popups);
            } else {
                front_to_back.extend(own_popups);
            }
            front_to_back.extend(window_elements(p, entry, &plan, active, true, deco));
        }
        let mut ghosts: Vec<Piece> = closing_elements(p);
        ghosts.append(&mut front_to_back);
        ghosts.append(&mut strip);
        stack.extend(SceneLayer::Windows, ghosts);
    }
    stack.extend(SceneLayer::Popups, popups);

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
    drag_return_element(p, stack);
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
            clip: Clip::None,
        })),
    );
}

/// Assign each material and glass element the signature of its backdrop, bottom to top: of the elements below it that
/// overlap the area it samples ([`backdrop_generation`]), so a change elsewhere on the output, or a commit that leaves
/// that area as it was, keeps its blurred levels. A region that blurs what is under it samples its rectangle with the
/// reach of its blur around it, a backdrop captured below the windows the whole output, a glass element its own area;
/// a composite takes the signature of its capture element, which sits lower in the scene. Each element is finalized
/// before the elements above it read its commit, so a backdrop that changes this frame shows above it this frame.
pub fn finalize_materials(elements: &mut [CupertinoElement], scale: f64, cache: &mut ElementCache) {
    let scale = Scale::from(scale);
    let mut captured: Vec<(u64, u64)> = Vec::new();
    // the elements finalized so far, bottom to top, each with its index into `elements`
    let mut below: Vec<(Below, usize)> = Vec::new();
    for i in (0..elements.len()).rev() {
        let (upper, lower) = elements.split_at_mut(i + 1);
        let e = &mut upper[i];
        let sampled = match &*e {
            CupertinoElement::Glass(g) => Some((g.key(), g.geometry(scale))),
            CupertinoElement::Material(m)
                if !matches!(m.stage(), MaterialStage::Composite { .. }) =>
            {
                Some((m.key(), m.sampled_area(scale)))
            }
            _ => None,
        };
        let generation = sampled.map(|(key, area)| {
            let (overlapping, at): (Vec<Below>, Vec<usize>) = below
                .iter()
                .filter(|(b, _)| b.geometry.overlaps(area))
                .map(|(b, j)| (*b, *j))
                .unzip();
            let generation =
                backdrop_generation(area, &overlapping, cache.backdrop(key), |k, since| {
                    // `lower` starts at index `i + 1` of `elements`
                    let e = &lower[at[k] - i - 1];
                    let origin = e.geometry(scale).loc;
                    e.damage_since(scale, Some(CommitCounter::from(since as usize)))
                        .iter()
                        .map(|d| smithay::utils::Rectangle::new(d.loc + origin, d.size))
                        .collect()
                });
            cache.set_backdrop(
                key,
                BackdropRecord {
                    generation,
                    below: overlapping,
                },
            );
            generation
        });
        match e {
            CupertinoElement::Glass(g) => {
                g.finalize(generation.expect("glass samples an area"), cache);
            }
            CupertinoElement::Material(m) => {
                let generation = match m.stage() {
                    MaterialStage::Full => generation.expect("a region samples an area"),
                    MaterialStage::Capture { owner } => {
                        let generation = generation.expect("a capture samples the output");
                        captured.push((owner, generation));
                        generation
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
            _ => {}
        }
        let c = e
            .current_commit()
            .distance(Some(CommitCounter::default()))
            .unwrap_or(0) as u64;
        below.push((
            Below {
                id: id_hash(e.id()),
                commit: c,
                alpha: e.alpha(),
                geometry: e.geometry(scale),
            },
            i,
        ));
    }
}

#[cfg(test)]
mod tests {
    /// An inactive window's sidebar is the data's inactive variant, an opaque grey laid with the desktop tint and no
    /// backdrop, so it keeps the wallpaper's tint whatever lies behind it; with Reduce Transparency it is the active
    /// window's opaque sidebar, which is no darker than the window.
    #[test]
    fn an_inactive_window_tints_its_sidebar_and_keeps_its_opaque_one() {
        let theme = Theme::load().unwrap();
        for appearance in Appearance::ALL {
            let sidebar = |reduce_transparency, window| {
                theme
                    .recipe(
                        appearance,
                        "Sidebar",
                        &super::material_options(reduce_transparency, window),
                    )
                    .unwrap()
            };
            assert_eq!(
                sidebar(true, WindowState::Active),
                sidebar(true, WindowState::Inactive),
                "{appearance:?}: the opaque sidebar stays"
            );
            let (active, inactive) = (
                sidebar(false, WindowState::Active),
                sidebar(false, WindowState::Inactive),
            );
            if active.opaque {
                // the high-contrast appearances draw the opaque sidebar alone
                assert_eq!(active, inactive, "{appearance:?}");
                continue;
            }
            assert!(
                active.has_backdrop(),
                "{appearance:?}: the active sidebar blurs what is behind"
            );
            assert!(
                !inactive.has_backdrop() && inactive.uses_tint_layer(),
                "{appearance:?}: the inactive one is an opaque grey tinted by the wallpaper"
            );
            assert_eq!(inactive.rendition_key, "inactive.normal");
        }
    }

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
    fn a_frame_animation_crops_the_content_at_its_size() {
        let grown = plan_window(
            &pres(Rect::new(10.0, 20.0, 800.0, 600.0)),
            (400.0, 300.0),
            true,
            (0.0, 0.0),
        );
        assert!(!grown.exact);
        assert_eq!((grown.scale_x, grown.scale_y), (1.0, 1.0));
        assert_eq!(grown.visual, Rect::new(10.0, 20.0, 400.0, 300.0));
        let shrunk = plan_window(
            &pres(Rect::new(10.0, 20.0, 500.0, 350.0)),
            (800.0, 600.0),
            true,
            (0.0, 0.0),
        );
        assert_eq!((shrunk.scale_x, shrunk.scale_y), (1.0, 1.0));
        assert_eq!(shrunk.visual, Rect::new(10.0, 20.0, 500.0, 350.0));
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
        material_on(cache, key, stage, area, radius, opacity, (1280, 800))
    }

    /// A material region on an output of `output_px` pixels at scale 1.
    fn material_on(
        cache: &mut ElementCache,
        key: ElementKey,
        stage: MaterialStage,
        area: Rect,
        radius: f64,
        opacity: f32,
        output_px: (i32, i32),
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
            output_px,
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
    fn glass_backdrops_of_a_surface_join_per_container_where_the_frontmost_is() {
        use crate::app_effects_v1::GlassSpec;
        use cupertino_theme::GlassStyle;
        let area = |key: u64, container: Option<u32>| BackdropArea {
            key,
            material: String::new(),
            area: smithay::utils::Rectangle::new((0.0, 0.0).into(), (10.0, 10.0).into()),
            radius: 0.0,
            alpha: 1.0,
            glass: container.map(|container| GlassSpec {
                style: GlassStyle::Regular,
                container,
                spacing: 4.0,
            }),
        };
        // front to back: glass in container 1, a material, glass on its own, glass in container 1 again
        let backdrops = [
            area(1, Some(1)),
            area(2, None),
            area(3, Some(0)),
            area(4, Some(1)),
        ];
        let mut out = Vec::new();
        let mut groups = GlassGroups::default();
        for b in &backdrops {
            if b.glass.is_some() {
                groups.add(b, out.len());
            } else {
                out.push(marker(SceneKind::Backdrop));
            }
        }
        let mut made = Vec::new();
        let pieces = groups.place(out, |members| {
            made.push(members.iter().map(|b| b.key).collect::<Vec<_>>());
            vec![marker(SceneKind::BackdropCapture)]
        });
        assert_eq!(made, vec![vec![1, 4], vec![3]], "one piece per container");
        let kinds: Vec<SceneKind> = pieces
            .iter()
            .map(|p| match p {
                Piece::Item(i) => i.id.kind,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                SceneKind::BackdropCapture,
                SceneKind::Backdrop,
                SceneKind::BackdropCapture
            ],
            "container 1 in front of the material, where its frontmost backdrop is; the lone glass behind it"
        );
    }

    #[test]
    fn large_panel_glass_and_dark_cards_keep_their_foreground_tone_and_small_glass_adapts() {
        use crate::app_effects_v1::GlassSpec;
        use cupertino_theme::GlassStyle;
        let area = |style: GlassStyle| BackdropArea {
            key: 7,
            material: String::new(),
            area: smithay::utils::Rectangle::new((0.0, 0.0).into(), (300.0, 40.0).into()),
            radius: 20.0,
            alpha: 1.0,
            glass: Some(GlassSpec {
                style,
                container: 0,
                spacing: 0.0,
            }),
        };
        let probes = |style: GlassStyle, dark: bool| tone_probes(&[&area(style)], dark).len();
        for dark in [false, true] {
            assert_eq!(probes(GlassStyle::Regular, dark), 1, "a small field adapts");
            assert_eq!(
                probes(GlassStyle::LargePanel, dark),
                0,
                "a field of the large panel glass never adapts"
            );
        }
        assert_eq!(probes(GlassStyle::Card, false), 1, "a light card adapts");
        assert_eq!(
            probes(GlassStyle::Card, true),
            0,
            "a dark card keeps light text"
        );
    }

    #[test]
    fn glass_reblurs_only_when_what_lies_below_its_sampled_area_changes() {
        use crate::render::GlassElement;
        use cupertino_theme::GlassMode;
        let mut cache = ElementCache::default();
        let container = Rc::new(GlassContainer::new(Container::new(
            vec![GlassShape::new([400.0, 300.0, 200.0, 60.0], 16.0)],
            0.0,
            GlassMode::default(),
        )));
        let sampled = container.sample_area().unwrap();
        // front to back: the glass, then a material standing in for a window
        let mut scene = |window: Rect| {
            let glass = GlassElement::new(
                ElementKey::new("backdrop", 1),
                &mut cache,
                sampled,
                container.clone(),
                MaterialGl::detached(),
                1.0,
                1.0,
            );
            let mut els = vec![
                CupertinoElement::Glass(glass),
                material(
                    &mut cache,
                    ElementKey::new("window", 0),
                    MaterialStage::Full,
                    window,
                ),
            ];
            finalize_materials(&mut els, 1.0, &mut cache);
            match &els[0] {
                CupertinoElement::Glass(g) => g.generation(),
                _ => unreachable!(),
            }
        };
        let far = scene(Rect::new(0.0, 700.0, 100.0, 50.0));
        assert_eq!(
            far,
            scene(Rect::new(10.0, 700.0, 100.0, 50.0)),
            "a window moving far from the glass keeps its blurred levels"
        );
        assert_ne!(
            far,
            scene(Rect::new(450.0, 320.0, 100.0, 50.0)),
            "a window moving under the glass makes them again"
        );
    }

    #[test]
    fn a_material_reblurs_only_when_what_lies_below_its_sampled_area_changes() {
        let mut cache = ElementCache::default();
        let output = (3840, 2160);
        let region = Rect::new(400.0, 300.0, 200.0, 60.0);
        // front to back: a region blurring what is under it, a material standing in for a window, and a full-output
        // one standing in for the wallpaper
        let mut scene = |window: Rect| {
            let mut els = vec![
                material_on(
                    &mut cache,
                    ElementKey::new("sidebar", 0),
                    MaterialStage::Full,
                    region,
                    0.0,
                    1.0,
                    output,
                ),
                material_on(
                    &mut cache,
                    ElementKey::new("window", 0),
                    MaterialStage::Full,
                    window,
                    0.0,
                    1.0,
                    output,
                ),
                material_on(
                    &mut cache,
                    ElementKey::new("wallpaper", 0),
                    MaterialStage::Full,
                    Rect::new(0.0, 0.0, 3840.0, 2160.0),
                    0.0,
                    1.0,
                    output,
                ),
            ];
            finalize_materials(&mut els, 1.0, &mut cache);
            let CupertinoElement::Material(m) = &els[0] else {
                unreachable!()
            };
            (m.generation(), m.sampled_area(Scale::from(1.0)))
        };
        let (far, sampled) = scene(Rect::new(3000.0, 1800.0, 300.0, 200.0));
        assert!(
            sampled.size.w > 200 && sampled.size.h > 60,
            "the region samples the reach of its blur around it"
        );
        assert_eq!(
            far,
            scene(Rect::new(3010.0, 1800.0, 300.0, 200.0)).0,
            "a window moving elsewhere on the output keeps the region's blurred levels"
        );
        assert_ne!(
            far,
            scene(Rect::new(450.0, 320.0, 100.0, 50.0)).0,
            "a window moving under the region makes them again"
        );
        // just past the region's bottom edge, inside the reach of its blur
        let edge = f64::from(sampled.loc.y + sampled.size.h) - 20.0;
        let near = scene(Rect::new(400.0, edge, 100.0, 10.0)).0;
        assert_ne!(
            near,
            scene(Rect::new(400.0, edge + 1.0, 100.0, 10.0)).0,
            "a window moving inside the reach of its blur makes them again"
        );
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

#[cfg(test)]
mod fullscreen_corner_tests {
    use super::*;

    fn plan(cap: Option<f64>) -> WindowPlan {
        WindowPlan {
            visual: Rect::new(0.0, 0.0, 100.0, 100.0),
            scale_x: 1.0,
            scale_y: 1.0,
            opacity: 1.0,
            shadow: 1.0,
            exact: false,
            corner: None,
            corner_cap: cap,
        }
    }

    #[test]
    fn the_corners_round_off_as_the_frame_meets_the_output_edges() {
        let output = Rect::new(0.0, 0.0, 1280.0, 800.0);
        // far from the edges the window keeps its radius
        let far = fullscreen_corner_cap(Rect::new(200.0, 100.0, 800.0, 500.0), output);
        assert!(far > 16.0);
        assert_eq!(plan(Some(far)).capped(16.0), 16.0);
        // close to every edge the radius is the room left, and nothing once the frame covers the output
        let near = fullscreen_corner_cap(Rect::new(4.0, 3.0, 1272.0, 794.0), output);
        assert_eq!(near, 4.0);
        assert_eq!(plan(Some(near)).capped(16.0), 4.0);
        assert_eq!(fullscreen_corner_cap(output, output), 0.0);
        assert_eq!(plan(Some(0.0)).capped(16.0), 0.0);
        // the radius shrinks steadily as the frame grows towards the output
        let mut last = f64::INFINITY;
        for i in 0..=20 {
            let t = f64::from(i) / 20.0;
            let f = Rect::new(
                200.0 * (1.0 - t),
                100.0 * (1.0 - t),
                800.0 + 480.0 * t,
                500.0 + 300.0 * t,
            );
            let r = plan(Some(fullscreen_corner_cap(f, output))).capped(16.0);
            assert!(r <= last, "{r} after {last} at {t}");
            last = r;
        }
        assert_eq!(last, 0.0);
        assert_eq!(plan(None).capped(16.0), 16.0, "no transition, no limit");
    }
}
