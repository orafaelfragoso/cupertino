use super::layers::*;
use super::*;

/// Material pieces of committed backdrops, to go directly below the owning surface's pieces
/// (the list is front to back, so later backdrops come first), in the variant of a window in `window` state.
pub(super) fn backdrop_elements(
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
        out.extend(
            window_material_layer(
                p,
                SceneKind::Backdrop,
                b.key,
                &b.material,
                b.area,
                b.radius,
                b.alpha as f32 * opacity,
                window,
            )
            .into_iter()
            .map(|piece| ramped(piece, b.blur_ramp)),
        );
    }
    glass.finish(p, out, opacity)
}

/// `piece` with its material's filter fading by `ramp` from its top to its bottom (a backdrop's `set_blur_ramp`); the
/// fallback fill that stands in for a material has no filter to fade.
fn ramped(piece: Piece, ramp: [f32; 2]) -> Piece {
    match piece {
        Piece::Material(m) => Piece::Material(Material {
            blur_ramp: ramp,
            ..m
        }),
        other => other,
    }
}

/// Note the backdrops drawn as a material other than glass: one whose face was measured while it was glass goes back
/// to the appearance's tone ([`crate::app_effects_v1::BackdropTones`]).
pub(super) fn note_not_glass(p: &SceneParams<'_>, backdrops: &[BackdropArea]) {
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
pub(super) struct GlassGroups<'a> {
    /// The position in the pieces, the container and the backdrops of each group.
    groups: Vec<(usize, Option<u32>, Vec<&'a BackdropArea>)>,
}

impl<'a> GlassGroups<'a> {
    /// Add glass backdrop `b`; a new container keeps the place of the next piece of the list, `at`.
    pub(super) fn add(&mut self, b: &'a BackdropArea, at: usize) {
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
    pub(super) fn finish(self, p: &SceneParams<'_>, out: Vec<Piece>, opacity: f32) -> Vec<Piece> {
        self.place(out, |members| glass_pieces(p, members, opacity, false))
    }

    /// `out` with each container's piece at its place, for the glass of a popup (a menu, a popover): its rim is lit
    /// apart.
    pub(super) fn finish_popup(
        self,
        p: &SceneParams<'_>,
        out: Vec<Piece>,
        opacity: f32,
    ) -> Vec<Piece> {
        self.place(out, |members| glass_pieces(p, members, opacity, true))
    }

    /// `out` with the pieces `make` gives each container (its backdrops front to back) at the container's place.
    pub(super) fn place(
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
    // a member in other glass than the first (a selected segment in its group) keeps its own recipe on its shape
    let last = members.len() - 1;
    for (j, b) in members.iter().enumerate().skip(1) {
        if let Some(own) = b.glass.filter(|g| g.style != spec.style) {
            let own_mode = mode.with_style(own.style);
            container = container.with_shape_mode(last - j, own_mode);
        }
    }
    // the container's blur ramp runs along its first shape, the backdrop the surface created first
    let ramp = members[last].blur_ramp;
    if ramp != [1.0, 1.0] {
        container = container.with_blur_ramp(ramp);
    }
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
                container.for_each_recipe(|r| r.highlight.amount = 0.0)
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
            container.for_each_recipe(|r| r.face.fill = SMALL_CALLOUT_FACE_DARK);
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
    vec![Piece::Glass(super::super::compose::Glass {
        glass: Rc::new(GlassContainer::new(container).with_probes(probes)),
        kind: SceneKind::Backdrop,
        index,
        opacity: alpha,
    })]
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
pub(super) fn glass_fallback(
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
pub(super) fn bar_backdrop_elements(
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
        let Some(composite) = composite.map(|piece| ramped(piece, b.blur_ramp)) else {
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
    Some(Piece::Material(Material {
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
        blur_ramp: [1.0, 1.0],
    }))
}

/// Translucent fill standing in for a material.
pub(super) fn fallback_fill(
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
                Piece::Image(Picture {
                    buffer: frame.buffer.clone().into(),
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
                }),
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
                Piece::Tree(Tree {
                    surface,
                    kind: SceneKind::CursorContent,
                    role: SurfaceRole::Cursor,
                    origin: loc,
                    pivot: loc,
                    scale: (grow, grow),
                    opacity: 1.0,
                    clip: Clip::None,
                    only: None,
                }),
            );
        }
    }
}

/// The live picture of `workspace` on `output` (whose rectangle is `out_rect`), front to back: an opaque black backing,
/// the wallpaper and, with `windows`, the workspace's windows but the claimed ones (drawn in their holes alone), all
/// scaled from the output into `frame` (global logical points), at `alpha`. Its backing and wallpaper pieces take
/// `index`.
pub(super) fn workspace_picture(
    p: &mut SceneParams<'_>,
    (workspace, output, out_rect): (cupertino_wm::WorkspaceId, OutputId, Rect),
    frame: Rect,
    alpha: f32,
    windows: bool,
    index: u64,
) -> Vec<Piece> {
    let mut out = Vec::new();
    let k = frame.w / out_rect.w.max(1.0);
    let local_frame = local(frame, &p.rect);
    // windows of the workspace, front first
    if windows {
        let order: Vec<WindowId> = p.wm.stacking_order().to_vec();
        for id in order.iter().rev() {
            let Some(w) = p.wm.window(*id) else { continue };
            if w.output != output
                || w.workspace != workspace
                || w.sticky
                || w.state().is_minimized()
                || w.state().is_dead()
                || p.transactions.is_window_claimed(*id)
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
    out
}

/// The neutral frame of one output, with the elements the renderer still builds itself.
pub fn compose_scene(renderer: &mut GlesRenderer, p: &mut SceneParams<'_>) -> Composed {
    p.res
        .set_density((p.config.material_density * 100.0).round() as u8);
    p.res.set_tint_windows(p.config.tint_windows);
    let mut stack = Composition::new(p.scale);
    let locked = matches!(p.lock, LockView::Locked { .. });
    match std::mem::replace(&mut p.lock, LockView::Unlocked) {
        LockView::Locked { surface } => locked_scene(p, &mut stack, surface),
        LockView::Unlocked => desktop_scene(renderer, p, &mut stack),
    }
    // drawn at a whole scale above the display's: the pointer and what lies above it at the display's own, over the
    // shrunk frame
    let top = match p.panel {
        Some(panel) => Some(top_scene(p, panel, locked)),
        None => {
            session_overlays(p, &mut stack);
            None
        }
    };
    // what the composition used this frame is all it keeps for the next one
    p.decor.end_frame();
    p.overlays.images.end_frame();
    if !p.for_capture {
        p.res.end_minimize_frame(p.output_id);
    }
    let mut composed = finish_composition(stack, p);
    composed.resources.top = top.map(Box::new);
    composed
}

/// The pointer, the compositor's notices (but on a locked frame) and the session's overlays of an output drawn at a
/// whole scale above its display's, composed at the display's own scale and pixels.
fn top_scene(p: &mut SceneParams<'_>, panel: Panel, locked: bool) -> Composed {
    let main = (p.scale, p.output_px, p.panel.take());
    (p.scale, p.output_px) = (panel.scale, panel.px);
    let mut top = Composition::new(p.scale);
    if !locked {
        top.extend(
            SceneLayer::CompositorOverlay,
            crate::kbd_ui::shortcut_notice(p),
        );
    }
    cursor(p, &mut top);
    session_overlays(p, &mut top);
    let composed = finish_composition(top, p);
    (p.scale, p.output_px, p.panel) = main;
    composed
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
    // the windows whose ghost was drawn with them
    let mut with_their_window: Vec<WindowId> = Vec::new();
    for (id, slide, workspace_opacity) in list.into_iter().rev() {
        if p.transactions.is_window_claimed(id) {
            continue;
        }
        let Some(entry) = p.windows.iter().find(|w| w.id == id) else {
            continue;
        };
        if p.for_capture && crate::control::protected_surface(entry.surface()) {
            continue;
        }
        let Some((mut plan, pres)) = plan_for(p.wm, entry, slide, p.now, p.fx, p.display) else {
            continue;
        };
        plan.opacity *= workspace_opacity;
        // a window the window manager hides fades out where it was drawn, and back in at its own frame
        match p.window_managers.look(id, p.now) {
            Look::Natural => {}
            Look::Hidden => continue,
            Look::Placed { rect, alpha, .. } => {
                let corner = p.config.window.corner_radius;
                plan = placed_plan(entry, rect, plan.opacity * alpha, plan.shadow, corner);
            }
        }
        if plan.opacity <= 0.0 || plan.visual.right() < p.rect.x || plan.visual.x > p.rect.right() {
            continue;
        }
        let active = key == Some(id);
        if pres.minimize_progress > 1e-6 {
            if let Some(els) = minimize_elements(renderer, p, entry, &pres, slide, active) {
                front_to_back.extend(els);
                continue;
            }
        }
        // the picture of the window as it showed before an appearance change fades out over this window alone, under
        // the windows in front of it: over every window it would show a window behind through the ones in front.
        // The window's own shadow and outline come in as the picture's go, so the two never add up
        if let Some(ghost) = p.fx.fading_over(id, p.output_id) {
            plan.shadow *= 1.0 - ghost.placement(p.now).1.clamp(0.0, 1.0);
            front_to_back.extend(ghost_piece(p, ghost));
            with_their_window.push(id);
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
    // the picture of a claimed window that closed fades out in the hole that claimed it alone
    with_their_window.extend(p.transactions.closing_in_claims());
    let mut ghosts: Vec<Piece> = closing_elements(p, &with_their_window);
    ghosts.append(&mut front_to_back);
    stack.extend(SceneLayer::Windows, ghosts);
    stack.extend(SceneLayer::Popups, popups);

    snap_preview(p, stack);
    if p.panel.is_none() {
        stack.extend(
            SceneLayer::CompositorOverlay,
            crate::kbd_ui::shortcut_notice(p),
        );
    }

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
    if p.panel.is_none() {
        cursor(p, stack);
    }
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
    if p.panel.is_none() {
        cursor(p, stack);
    }
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
        Piece::Material(Material {
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
            blur_ramp: [1.0, 1.0],
        }),
    );
}
