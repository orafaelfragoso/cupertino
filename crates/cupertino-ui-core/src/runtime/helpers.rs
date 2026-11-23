use super::*;

/// Whether the anchor's node is in the parent's tree but has no place yet.
pub(super) fn anchor_declared<M: 'static>(
    tree: &Tree<M>,
    parent: &WindowState<M>,
    anchor: &PopupAnchor,
) -> bool {
    let (PopupAnchor::Below { key, .. }
    | PopupAnchor::Beside { key, .. }
    | PopupAnchor::Over { key, .. }
    | PopupAnchor::Above { key, .. }
    | PopupAnchor::CentredAbove { key, .. }
    | PopupAnchor::Panel { over: key, .. }) = anchor
    else {
        return false;
    };
    let Some(root) = parent.root else {
        return false;
    };
    tree.preorder(root)
        .into_iter()
        .find(|&n| tree.get(n).is_some_and(|n| n.key.as_ref() == Some(key)))
        .is_some_and(|node| !parent.geometry.contains_key(&node))
}

/// The rectangle a popup of `parent` anchored at `anchor` and placed at `placed` grows out of and shrinks back into,
/// in the parent's coordinates: the node a [`PopupAnchor::Panel`] grows from, the part of it its clips leave in view,
/// where it is laid out and in view; else the anchor's rectangle.
pub(super) fn grows_out_of<M: 'static>(
    tree: &Tree<M>,
    parent: &WindowState<M>,
    anchor: &PopupAnchor,
    placed: &PopupSpec,
) -> Rect {
    let PopupAnchor::Panel { from, .. } = anchor else {
        return placed.anchor_rect;
    };
    let shown = parent.root.and_then(|root| {
        let node = tree
            .preorder(root)
            .into_iter()
            .find(|&n| tree.get(n).is_some_and(|n| n.key.as_ref() == Some(from)))?;
        let at = parent.geometry.get(&node)?;
        match at.clip {
            Some(clip) => at.rect.intersection(&clip),
            None => Some(at.rect),
        }
    });
    shown.unwrap_or(placed.anchor_rect)
}

/// Where a popup of `parent` opens, in the parent's coordinates; `None` while its anchor is not laid out.
pub(super) fn place<M: 'static>(
    tree: &Tree<M>,
    parent: &WindowState<M>,
    anchor: &PopupAnchor,
    grab: bool,
    size: Size,
) -> Option<PopupSpec> {
    let rect_of = |key: &Key| {
        let node = tree
            .preorder(parent.root?)
            .into_iter()
            .find(|&n| tree.get(n).is_some_and(|n| n.key.as_ref() == Some(key)))?;
        Some(parent.geometry.get(&node)?.rect)
    };
    let rtl = parent.env.direction == LayoutDirection::Rtl;
    let (anchor_rect, edge, gravity, offset) = match anchor {
        PopupAnchor::Below { key, gap } => (
            rect_of(key)?,
            PopupEdge::Bottom,
            PopupEdge::Bottom,
            Point::new(0.0, *gap),
        ),
        PopupAnchor::At(point) => {
            let edge = if rtl {
                PopupEdge::BottomLeft
            } else {
                PopupEdge::BottomRight
            };
            (
                Rect::new(point.x, point.y, 1.0, 1.0),
                edge,
                edge,
                Point::ZERO,
            )
        }
        PopupAnchor::Beside { key, offset } if rtl => (
            rect_of(key)?,
            PopupEdge::TopLeft,
            PopupEdge::BottomLeft,
            Point::new(-offset.x, offset.y),
        ),
        PopupAnchor::Beside { key, offset } => (
            rect_of(key)?,
            PopupEdge::TopRight,
            PopupEdge::BottomRight,
            *offset,
        ),
        PopupAnchor::CentredAbove { key, gap } => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::Top,
            Point::new(0.0, -gap),
        ),
        PopupAnchor::Above { key, gap, dx } if rtl => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::TopLeft,
            Point::new(-dx, -gap),
        ),
        PopupAnchor::Above { key, gap, dx } => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::TopRight,
            Point::new(*dx, -gap),
        ),
        PopupAnchor::Over { key, offset } if rtl => (
            rect_of(key)?,
            PopupEdge::Right,
            PopupEdge::BottomLeft,
            Point::new(-offset.x, offset.y),
        ),
        PopupAnchor::Over { key, offset } => (
            rect_of(key)?,
            PopupEdge::Left,
            PopupEdge::BottomRight,
            *offset,
        ),
        PopupAnchor::Panel { over, .. } => {
            let rect = rect_of(over)?;
            // centred on the node, or down to the parent's top edge where it is taller than the room above the middle
            let top = (rect.height() - size.height) / 2.0;
            (
                rect,
                PopupEdge::Top,
                PopupEdge::Bottom,
                Point::new(0.0, top.max(-rect.min_y())),
            )
        }
    };
    Some(PopupSpec {
        anchor_rect,
        anchor: edge,
        gravity,
        offset,
        size,
        constraint: ConstraintAdjust {
            slide: true,
            flip: !matches!(anchor, PopupAnchor::Over { .. } | PopupAnchor::Panel { .. }),
            // a panel taller or wider than the room shrinks to fit it
            resize: matches!(anchor, PopupAnchor::Panel { .. }),
        },
        reactive: true,
        grab,
    })
}

/// The buffer size of a logical length, rounded to the nearest pixel with halves up, as the window system sizes the
/// buffer it accepts for a configure at a fractional scale.
pub(super) fn physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The image of a drag from a window with `env`: its content in a tree of its own, laid out at whole points and painted
/// over nothing at whole pixels per point, so each side of the buffer is a whole number of points.
pub(super) fn drag_image<M: 'static>(
    options: &mut RuntimeOptions,
    env: &Environment,
    preview: DragPreview<M>,
    now: Timestamp,
) -> Result<DragImage, UiError> {
    let DragPreview {
        content,
        size,
        hotspot,
    } = preview;
    let scale = if env.scale.is_finite() {
        env.scale.ceil().clamp(1.0, f64::from(MAX_DRAG_SCALE))
    } else {
        1.0
    };
    let whole = |v: f64| {
        if v.is_finite() {
            v.ceil().max(1.0)
        } else {
            1.0
        }
    };
    let size = Size::new(whole(size.width), whole(size.height));
    let env = Environment {
        scale,
        ..env.clone()
    };
    let mut tree = Tree::new(options.node_limit);
    let root = tree.reconcile(None, content)?;
    let result = layout(
        &TreeView::new(&tree, &env),
        root,
        Constraints::tight(size).map_err(UiError::Layout)?,
        &LayoutOptions {
            direction: env.direction,
            node_limit: options.node_limit,
        },
    )
    .map_err(UiError::Layout)?;
    let geometry = placements(&tree, root, &result, &env, &[]);
    let painting = Painter::default().paint(
        &tree,
        root,
        &geometry,
        &PaintInput {
            env: &env,
            focus: None,
            focus_visible: false,
            window_active: true,
            background: WindowBackground::None,
            backdrop: false,
            // nothing draws a compositor material under a drag icon, so material nodes paint their own fill
            materials: false,
            now,
            raised: &[],
            obscuring: &[],
            layer: false,
            surface_layers: false,
            glass: crate::paint::GlassSupport::default(),
            pinned: &[None, None],
            activation: 1.0,
        },
    );
    let mut renderer = (options.renderers)();
    let (width, height) = (physical(size.width, scale), physical(size.height, scale));
    let id = renderer
        .render(&RenderRequest {
            list: &painting.list,
            resources: &painting.resources,
            scale,
            width,
            height,
            damage: &DamageRegion::Full,
        })
        .map_err(UiError::Render)?;
    // a frame drawn on a GPU is read back: the image goes to the compositor as pixels
    let frame = renderer
        .frame(id)
        .and_then(CpuFrame::in_memory)
        .map_err(UiError::Render)?;
    let row = width as usize * 4;
    if (frame.width, frame.height) != (width, height)
        || (frame.stride as usize) < row
        || frame.pixels.len() < frame.stride as usize * height as usize
    {
        return Err(UiError::Render(RenderError::InvalidSize));
    }
    let pixels = frame
        .pixels
        .chunks(frame.stride as usize)
        .take(height as usize)
        .flat_map(|line| &line[..row])
        .copied()
        .collect();
    Ok(DragImage {
        width,
        height,
        scale: scale as u32,
        pixels,
        hotspot,
    })
}

/// The most pixels per point a drag image is drawn at.
const MAX_DRAG_SCALE: u32 = 4;

/// A roving focus move waiting for the next frame.
pub(super) enum FocusRequest {
    Node(NodeId),
    /// The item with this key in this group, found once the view is rebuilt.
    Key(NodeId, Key),
}

/// The nearest roving group above `node`.
pub(super) fn group_of<M: 'static>(
    tree: &Tree<M>,
    groups: &[NodeId],
    node: NodeId,
) -> Option<NodeId> {
    tree.path(node)
        .into_iter()
        .rev()
        .skip(1)
        .find(|a| groups.contains(a))
}

pub(super) fn resolve<M: 'static>(
    tree: &Tree<M>,
    groups: &[NodeId],
    items: &[NodeId],
    request: FocusRequest,
) -> Option<NodeId> {
    match request {
        FocusRequest::Node(node) => Some(node),
        FocusRequest::Key(group, key) => items.iter().copied().find(|&item| {
            tree.get(item).is_some_and(|n| n.key.as_ref() == Some(&key))
                && group_of(tree, groups, item) == Some(group)
        }),
    }
}

/// The item `target` names among the items of `node`'s roving group (the group itself, or the nearest one above it),
/// counted from the focused item, else from `node`.
pub(super) fn roving_target<M: 'static>(
    tree: &Tree<M>,
    w: &WindowState<M>,
    node: NodeId,
    target: FocusTarget,
) -> Option<FocusRequest> {
    let group = if w.roving_groups.contains(&node) {
        node
    } else {
        group_of(tree, &w.roving_groups, node)?
    };
    if let FocusTarget::Key(key) = target {
        return Some(FocusRequest::Key(group, key));
    }
    let items: Vec<NodeId> = w
        .roving_items
        .iter()
        .copied()
        .filter(|&item| group_of(tree, &w.roving_groups, item) == Some(group))
        .collect();
    let n = items.len();
    if n == 0 {
        return None;
    }
    let at = |id: NodeId| items.iter().position(|&item| item == id);
    let current = w.focus.node().and_then(at).or_else(|| at(node));
    let index = match target {
        FocusTarget::Next => current.map_or(0, |c| (c + 1) % n),
        FocusTarget::Previous => current.map_or(n - 1, |c| (c + n - 1) % n),
        FocusTarget::First => 0,
        FocusTarget::Last => n - 1,
        FocusTarget::Index(k) => (k < n).then_some(k)?,
        FocusTarget::Key(_) => unreachable!("resolved after the rebuild, above"),
    };
    Some(FocusRequest::Node(items[index]))
}

pub(super) fn route<'a, M: 'static>(
    tree: &'a mut Tree<M>,
    w: &'a WindowState<M>,
    now: Timestamp,
    effects: &'a mut Effects<M>,
) -> Route<'a, M> {
    Route {
        tree,
        geometry: &w.geometry,
        env: &w.env,
        status: Status {
            focus: w.focus.node(),
            focus_visible: w.focus.visible(),
            capture: w.pointer.capture(),
            window_active: w.active,
            now,
            opening_press: w.opening_press.or(w.opening_touch.map(|(_, start)| start)),
            clipboard_text: w.clipboard_text,
            owned_popup: w.owned_popup,
        },
        popups: &w.popups,
        effects,
    }
}

/// Gives `w` the background `background`. A window that leaves a solid colour shown without a frame draws its next
/// frame whole, and one that turns solid shows its colour with its next frame.
pub(super) fn set_background<M>(w: &mut WindowState<M>, background: WindowBackground) {
    let solid = |b: WindowBackground| matches!(b, WindowBackground::Solid(_));
    if w.background != background
        && (solid(w.background) || solid(background))
        && w.solid_shown.take().is_some()
    {
        w.damage.invalidate();
    }
    w.background = background;
}

/// Asks the window system for the material `w`'s background wants, or for none, when that changed. A platform that
/// refuses leaves the window on its opaque fill.
pub(super) fn sync_backdrop<M>(system: &mut impl WindowSystem, w: &mut WindowState<M>) {
    let shaped = match w.background {
        WindowBackground::Surface {
            surface,
            radius,
            alpha,
            glass,
        } => Some((surface, radius, alpha, glass)),
        _ => None,
    };
    let wanted = match shaped {
        Some((surface, radius, alpha, glass)) if system.capabilities().app_effects => w
            .env
            .theme
            .surface(surface)
            .material
            .map(|material| Backdrop {
                material: material.to_owned(),
                radius,
                alpha: crate::surface_alpha(alpha),
                glass: glass.map(|g| g.window_region()),
            }),
        _ => None,
    };
    if wanted == w.backdrop {
        return;
    }
    let shown = system
        .request(w.id, WindowRequest::Backdrop(wanted.clone()))
        .is_ok();
    // another material is a new backdrop, whose face nothing measured yet
    if wanted.as_ref().map(|b| &b.material) != w.backdrop.as_ref().map(|b| &b.material) {
        w.painter.tones.window = GlassTone::Appearance;
    }
    w.backdrop = wanted.filter(|_| shown);
    // the material is shaped with the next present, which a change that paints nothing new (new corners on a material
    // the compositor draws) must still bring
    w.invalidate();
    w.invalidate_damage();
}

/// Asks the window system to publish `menus`, or to withdraw them, when they changed.
pub(super) fn sync_menus<M>(
    system: &mut impl WindowSystem,
    w: &mut WindowState<M>,
    menus: Option<Menus<M>>,
) {
    let (wanted, on_menu) = menus
        .map(|m| {
            let description = MenuDescription {
                bar: m.bar,
                disabled: m.disabled,
            };
            (description, m.on_activate)
        })
        .unzip();
    w.on_menu = on_menu;
    if wanted != w.menus {
        let _ = system.request(w.id, WindowRequest::Menus(wanted.clone()));
        w.menus = wanted;
    }
}

/// The kind of a platform event as its span's label (a window's event by the window event's kind): the variant its
/// `Debug` output begins with. Formatted only when a timeline records the span, and only that far.
pub(super) struct EventName<'a>(pub(super) &'a PlatformEvent);

impl std::fmt::Display for EventName<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write as _;
        /// Keeps what comes before the first `(`, `{` or space, and stops the formatting there.
        struct Name(String);
        impl std::fmt::Write for Name {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                match s.find(['(', '{', ' ']) {
                    Some(end) => {
                        self.0.push_str(&s[..end]);
                        Err(std::fmt::Error)
                    }
                    None => {
                        self.0.push_str(s);
                        Ok(())
                    }
                }
            }
        }
        let mut name = Name(String::new());
        let _ = match self.0 {
            PlatformEvent::Window { event, .. } => write!(name, "{event:?}"),
            event => write!(name, "{event:?}"),
        };
        f.write_str(&name.0)
    }
}

/// The themes of `theme`'s session options in the light and the dark appearance, from `theme_for`; none without it.
/// The session's own appearance is asked for last, as the theme maker may remember the last one it made.
pub(super) fn pinned_themes(
    theme_for: &mut Option<ThemeForAppearance>,
    theme: &ThemeTokens,
) -> [Option<Arc<ThemeTokens>>; 2] {
    let Some(theme_for) = theme_for else {
        return [None, None];
    };
    let session = appearance_of(theme);
    let pinned = [false, true].map(|dark| Some(theme_for(&SystemAppearance { dark, ..session })));
    theme_for(&session);
    pinned
}

/// The session's appearance and display options as `theme` was made for them.
pub(super) fn appearance_of(theme: &ThemeTokens) -> SystemAppearance {
    let preferences = &theme.preferences;
    SystemAppearance {
        dark: preferences.appearance.is_dark(),
        increase_contrast: preferences.appearance.is_high_contrast(),
        reduce_transparency: preferences.reduce_transparency,
        reduce_motion: preferences.reduce_motion,
        steady_caret: theme.motion.caret_phase_ms == 0,
    }
}

/// The window's renderer, reached only while no frame of it is in flight (a renderer on the GPU waits for the render
/// thread, which may be drawing that frame with the renderer).
pub(super) fn held(
    renderer: &SharedRenderer,
) -> MutexGuard<'_, Box<dyn UiRenderer<Frame = CpuFrame> + Send>> {
    renderer.lock().unwrap_or_else(PoisonError::into_inner)
}
