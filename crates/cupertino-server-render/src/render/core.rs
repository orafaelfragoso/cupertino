//! Core scene planning, GPU resource cache, and material backdrop finalisation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cupertino_effects::WorkspaceSlide;
use cupertino_scene::{BorderFalloff, RoundedRect, SceneKind, Source};
use cupertino_theme::{Appearance, MaterialOptions, Theme, WindowState};
use cupertino_wm::{OutputId, Presentation, Rect, WindowId, Wm};
use smithay::backend::renderer::element::Element;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};

use super::compose::{corner_style, item, output_rect, Piece};
use super::lower::id_hash;
use super::material::glass::{backdrop_change, BackdropRecord, Below};
use super::material::{GpuRecipe, MaterialStage, SharedMaterial};
use super::mesh::{MeshGl, SharedMesh};
use super::motion::{cross_fade_opacity, WorkspaceTransition};
use super::programs::Programs;
use super::shader_rect::ElementCache;
use super::snapshots::{SnapshotLease, SnapshotOwner, Snapshots};
use super::CupertinoElement;
use crate::cursor::CursorFrame;

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

/// Programs, material pipeline and recipes of one renderer.
pub struct RenderResources {
    pub programs: Programs,
    pub material: Option<SharedMaterial>,
    pub material_error: Option<String>,
    pub mesh: Option<SharedMesh>,
    /// The retained window snapshots (closing ghosts).
    pub snapshots: Snapshots,
    /// The snapshot each minimizing or restoring window is drawn from, by window and output: captured on the
    /// animation's first frame and drawn through the mesh until a frame of that output no longer shows it.
    minimizing: RefCell<HashMap<(WindowId, OutputId), MinimizeCapture>>,
    /// How many window snapshots minimize and restore animations have captured for the output's frames.
    minimize_captures: std::cell::Cell<u64>,
    theme: Option<Theme>,
    recipes: RefCell<RecipeCache>,
    /// How much denser every material is made, in percent (0 to 100).
    density: std::cell::Cell<u8>,
    /// Windows' own backgrounds and sidebars take a slight tint of the wallpaper's colour.
    tint_windows: std::cell::Cell<bool>,
}

/// The snapshot a minimizing or restoring window is drawn from, at the size in pixels it was captured at.
struct MinimizeCapture {
    owner: SnapshotOwner,
    size_px: (i32, i32),
    used: bool,
}

impl RenderResources {
    /// How many window snapshots minimize and restore animations have captured for the outputs' frames (a
    /// screenshot's own are not counted): one an animation.
    pub fn minimize_captures(&self) -> u64 {
        self.minimize_captures.get()
    }

    /// Pin an existing minimize snapshot of `key` if it was captured at `size_px`, marking it used this frame.
    pub fn pin_minimize_capture(
        &self,
        key: (WindowId, OutputId),
        size_px: (i32, i32),
    ) -> Option<SnapshotLease> {
        let mut minimizing = self.minimizing.borrow_mut();
        let c = minimizing.get_mut(&key).filter(|c| c.size_px == size_px)?;
        c.used = true;
        self.snapshots.pin(c.owner.id()).ok()
    }

    /// Store a freshly captured minimize snapshot for `key` at `size_px` and return a frame lease for it.
    pub fn store_minimize_capture(
        &self,
        key: (WindowId, OutputId),
        owner: SnapshotOwner,
        size_px: (i32, i32),
    ) -> Option<SnapshotLease> {
        self.minimize_captures.set(self.minimize_captures.get() + 1);
        let lease = self.snapshots.pin(owner.id()).ok()?;
        self.minimizing.borrow_mut().insert(
            key,
            MinimizeCapture {
                owner,
                size_px,
                used: true,
            },
        );
        Some(lease)
    }

    /// Forget the minimize snapshots of `output` that its last frame did not draw: their animation ended.
    pub fn end_minimize_frame(&self, output: OutputId) {
        self.minimizing
            .borrow_mut()
            .retain(|(_, o), c| *o != output || std::mem::replace(&mut c.used, false));
    }

    pub fn new(renderer: &mut GlesRenderer) -> anyhow::Result<Self> {
        let programs =
            Programs::compile(renderer).map_err(|e| anyhow::anyhow!("compiling shaders: {e}"))?;
        let (material, material_error) = match super::MaterialGl::new(renderer) {
            Ok(m) => (Some(m), None),
            Err(e) => {
                // a program the driver refused names itself and carries the driver's log in `e`
                tracing::error!(error = %e, "a material or glass program failed: the material pass is off, translucent fills stand in");
                (None, Some(e))
            }
        };
        let mesh = match MeshGl::new(renderer) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!(error = %e, "the mesh program failed: minimize falls back to a plain rescale");
                None
            }
        };
        let theme = match cupertino_assets::theme::load() {
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
            snapshots: Snapshots::default(),
            minimizing: RefCell::new(HashMap::new()),
            minimize_captures: std::cell::Cell::new(0),
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
        self.programs.passes.destroy(renderer);
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

/// A display drawn at a whole scale above its own: its own scale and its mode's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub scale: f64,
    pub px: (i32, i32),
}

/// A black overlay of the session over the whole output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionOverlay {
    pub kind: SceneKind,
    pub alpha: f64,
}

/// Rounded rectangle as a scene item.
#[allow(clippy::too_many_arguments)]
pub fn rrect_item(
    kind: SceneKind,
    index: u64,
    rect: Rectangle<f64, Logical>,
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

/// Approximate average colour of the procedural wallpaper (tint layer input, approximate).
pub fn wallpaper_tint(dark: bool) -> [f32; 3] {
    if dark {
        [0.22, 0.17, 0.37]
    } else {
        [0.78, 0.76, 0.84]
    }
}

/// Assign each material and glass element the signature of its backdrop, bottom to top: of the elements below it that
/// overlap the area it samples, so a change elsewhere on the output, or a commit that leaves
/// that area as it was, keeps its blurred levels. A region that blurs what is under it samples its rectangle with the
/// reach of its blur around it, a backdrop captured below the windows the whole output, a glass element its own area;
/// a composite takes the signature of its capture element, which sits lower in the scene. Each element is finalized
/// before the elements above it read its commit, so a backdrop that changes this frame shows above it this frame.
pub fn finalize_materials(elements: &mut [CupertinoElement], scale: f64, cache: &mut ElementCache) {
    let scale = Scale::from(scale);
    let mut captured: Vec<(u64, u64)> = Vec::new();
    // the elements finalized so far, bottom to top, each with its index into `elements`
    let mut below: Vec<(Below, usize)> = Vec::new();
    // what the backdrops finalized so far read of the framebuffer as they draw
    let mut read: Vec<Rectangle<i32, Physical>> = Vec::new();
    for i in (0..elements.len()).rev() {
        let (upper, lower) = elements.split_at_mut(i + 1);
        let e = &mut upper[i];
        // a window's opaque regions leave undrawn what lies under them, so they leave out what a backdrop below reads
        if let CupertinoElement::Window(w) = e {
            let geometry = w.geometry(scale);
            let under = read
                .iter()
                .filter(|r| r.overlaps(geometry))
                .copied()
                .collect();
            w.element_mut().keep_read_below(under);
        }
        let sampled = match &*e {
            CupertinoElement::Glass(g) => Some((g.key(), g.geometry(scale))),
            CupertinoElement::Material(m)
                if !matches!(m.stage(), MaterialStage::Composite { .. }) =>
            {
                Some((m.key(), m.sampled_area(scale)))
            }
            _ => None,
        };
        read.extend(sampled.map(|(_, area)| area));
        let change = sampled.map(|(key, area)| {
            let (overlapping, at): (Vec<Below>, Vec<usize>) = below
                .iter()
                .filter(|(b, _)| b.geometry.overlaps(area))
                .map(|(b, j)| (*b, *j))
                .unzip();
            let change = backdrop_change(area, &overlapping, cache.backdrop(key), |k, since| {
                // `lower` starts at index `i + 1` of `elements`
                let e = &lower[at[k] - i - 1];
                let origin = e.geometry(scale).loc;
                e.damage_since(scale, Some(CommitCounter::from(since as usize)))
                    .iter()
                    .map(|d| Rectangle::new(d.loc + origin, d.size))
                    .collect()
            });
            cache.set_backdrop(
                key,
                BackdropRecord {
                    generation: change.generation,
                    below: overlapping,
                },
            );
            change
        });
        let generation = change.as_ref().map(|c| c.generation);
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
                if let Some(change) = change {
                    m.changed_since(change.since);
                }
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
                hole: match &*e {
                    CupertinoElement::Shader(s) => s.hole(scale),
                    _ => None,
                },
            },
            i,
        ));
    }
}
