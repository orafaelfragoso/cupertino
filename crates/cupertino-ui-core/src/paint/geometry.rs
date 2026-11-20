use super::*;

/// How far a blur reaches beyond the content it blurs, per unit of radius.
pub(super) const BLUR_REACH: f64 = 3.0;

/// How far past its clip, in points, a node's content may still change a pixel: the clip's anti-aliased edge covers
/// part of the pixel it falls in, and the renderer scales the content's own coverage of that pixel by it.
pub(super) const CLIP_EDGE: f64 = 1.0;

/// Everything `ops` may touch, in their own coordinates: strokes reach half their width outside the shape, effects
/// reach as far as their blur and offset, and a glyph is taken to fill its em square around the pen with room for
/// ascenders, descenders and overhang.
/// `ops` of a node at `origin` in the window, with those at `pixels` on the window's device pixels at `scale`: a fill
/// at the nearest pixel edges, a clip widened to the pixels it touches. Each stays on the pixels its rect touches but a
/// fill narrower than a pixel, which grows to one ([`pixel_extent`]).
pub(super) fn on_pixels(
    ops: &[PaintOp],
    pixels: &[usize],
    origin: Point,
    scale: f64,
) -> Vec<PaintOp> {
    let mut ops = ops.to_vec();
    let snap = |rect: Rect, to: fn(Rect, f64) -> Rect| {
        to(rect.translate(origin.x, origin.y), scale).translate(-origin.x, -origin.y)
    };
    for &i in pixels {
        match ops.get_mut(i) {
            Some(PaintOp::Fill {
                shape: Shape::Rect(rect),
                ..
            }) => *rect = snap(*rect, crate::cx::pixel_fill),
            Some(PaintOp::PushClip(Shape::Rect(rect))) => {
                *rect = snap(*rect, crate::cx::pixel_clip)
            }
            _ => {}
        }
    }
    ops
}

/// The extent of `ops`, those at `pixels` to be put on device pixels at `scale` ([`on_pixels`]): a fill there reaches
/// at least one device pixel each way, which is as far as it can grow.
pub(super) fn pixel_extent(ops: &[PaintOp], pixels: &[usize], scale: f64) -> Option<Rect> {
    if pixels.is_empty() {
        return extent(ops);
    }
    let pixel = 1.0 / crate::cx::usable_scale(scale);
    let mut ops = ops.to_vec();
    for &i in pixels {
        if let Some(PaintOp::Fill {
            shape: Shape::Rect(rect),
            ..
        }) = ops.get_mut(i)
        {
            *rect = Rect::new(
                rect.min_x(),
                rect.min_y(),
                rect.width().max(pixel),
                rect.height().max(pixel),
            );
        }
    }
    extent(&ops)
}

pub(crate) fn extent(ops: &[PaintOp]) -> Option<Rect> {
    // Each open layer collects its content's extent; its effects widen it when it closes.
    let mut layers: Vec<(Vec<Effect>, Option<Rect>)> = vec![(Vec::new(), None)];
    let add = |layers: &mut Vec<(Vec<Effect>, Option<Rect>)>, r: Rect| {
        let top = &mut layers.last_mut().expect("the base layer is never popped").1;
        *top = Some(top.map_or(r, |t| t.union(&r)));
    };
    for op in ops {
        match op {
            PaintOp::Fill { shape, .. } => {
                if let Some(r) = shape_bounds(shape) {
                    add(&mut layers, r);
                }
            }
            PaintOp::Stroke { shape, width, .. } => {
                if let Some(r) = shape_bounds(shape) {
                    add(&mut layers, r.outset(width / 2.0));
                }
            }
            PaintOp::Glyphs { origin, run, .. } => {
                let size = f64::from(run.size);
                for g in &run.glyphs {
                    let pen = Point::new(origin.x + f64::from(g.x), origin.y + f64::from(g.y));
                    add(
                        &mut layers,
                        Rect::from_ltrb(
                            pen.x - size,
                            pen.y - 1.5 * size,
                            pen.x + 2.0 * size,
                            pen.y + size,
                        ),
                    );
                }
            }
            PaintOp::Image { dst, .. } => add(&mut layers, *dst),
            PaintOp::PushLayer { effects, .. } => layers.push((effects.clone(), None)),
            PaintOp::PopLayer if layers.len() > 1 => {
                let (effects, content) = layers.pop().expect("checked above");
                if let Some(content) = content {
                    add(&mut layers, with_effects(content, &effects));
                }
            }
            _ => {}
        }
    }
    while layers.len() > 1 {
        let (effects, content) = layers.pop().expect("checked above");
        if let Some(content) = content {
            add(&mut layers, with_effects(content, &effects));
        }
    }
    layers.pop().and_then(|(_, r)| r)
}

pub(super) fn with_effects(content: Rect, effects: &[Effect]) -> Rect {
    effects.iter().fold(content, |r, effect| match *effect {
        Effect::Blur { radius } => r.outset(radius.abs() * BLUR_REACH),
        Effect::DropShadow { offset, blur, .. } => r.union(
            &content
                .translate(offset.x, offset.y)
                .outset(blur.abs() * BLUR_REACH),
        ),
        Effect::InnerShadow { .. } => r,
    })
}

pub(super) fn shape_bounds(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Rect(r) | Shape::RoundedRect { rect: r, .. } => Some(*r),
        Shape::Superellipse { rect, outset, .. } => Some(rect.outset(outset.max(0.0))),
        Shape::Path { verbs, .. } => {
            let mut acc: Option<Rect> = None;
            let mut include = |p: Point| {
                acc = Some(match acc {
                    None => Rect::new(p.x, p.y, 0.0, 0.0),
                    Some(a) => Rect::from_ltrb(
                        a.min_x().min(p.x),
                        a.min_y().min(p.y),
                        a.max_x().max(p.x),
                        a.max_y().max(p.y),
                    ),
                });
            };
            for verb in verbs {
                match *verb {
                    PathVerb::MoveTo(p) | PathVerb::LineTo(p) => include(p),
                    PathVerb::QuadTo(a, b) => {
                        include(a);
                        include(b);
                    }
                    PathVerb::CubicTo(a, b, c) => {
                        include(a);
                        include(b);
                        include(c);
                    }
                    PathVerb::Close => {}
                }
            }
            acc
        }
    }
}

/// How far the raised nodes' rects (`raised`, window coordinates) cover `rect` from its top: a raised rect that lies
/// across it and reaches its top edge covers it down to the raised rect's bottom. The raised layer runs from the
/// window's top edge, so nothing raised covers a node from another edge.
pub(crate) fn obscured_insets(rect: Rect, raised: &[(NodeId, Rect)]) -> Insets {
    const SLACK: f64 = 0.5;
    let top = raised
        .iter()
        .filter(|(_, r)| {
            r.min_y() <= rect.min_y() + SLACK
                && r.max_y() > rect.min_y()
                && r.min_x().max(rect.min_x()) < r.max_x().min(rect.max_x())
        })
        .map(|(_, r)| r.max_y().min(rect.max_y()) - rect.min_y())
        .fold(0.0_f64, f64::max);
    Insets {
        top,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    }
}

/// The raised nodes ([`Widget::raised`](crate::Widget::raised)) under `root` with their rects in window coordinates,
/// in paint order; a raised node's subtree is part of it, so none under it is listed.
pub(crate) fn raised_nodes<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
) -> Vec<(NodeId, Rect)> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        if node.widget.raised(&*node.state) {
            if let Some(placement) = geometry.get(&id) {
                out.push((id, placement.rect));
            }
            continue;
        }
        stack.extend(node.children.iter().rev());
    }
    // floating ones paint after the rest
    out.sort_by_key(|(id, _)| tree.get(*id).is_some_and(|n| !n.widget.obscures(&*n.state)));
    out
}

/// `rect` less the top of it raised rects lie across, whole width and from its top edge: what of it shows below them.
pub(crate) fn unobscured(rect: Rect, raised: &[(NodeId, Rect)]) -> Rect {
    const SLACK: f64 = 0.5;
    let top = raised
        .iter()
        .filter(|(_, r)| {
            r.min_y() <= rect.min_y() + SLACK
                && r.max_y() > rect.min_y()
                && r.min_x() <= rect.min_x() + SLACK
                && r.max_x() >= rect.max_x() - SLACK
        })
        .map(|(_, r)| r.max_y())
        .fold(rect.min_y(), f64::max)
        .min(rect.max_y());
    Rect::from_ltrb(rect.min_x(), top, rect.max_x(), rect.max_y())
}

/// The nodes under `root` in the order they paint: the tree's preorder, and where raised nodes (`raised`) draw in place
/// (no `layer`) their subtrees after all the rest, so what they draw and the materials they ask for lie over the rest
/// of the window's, wherever they are in the tree. Raised nodes that float over the rest and obscure nothing
/// ([`Widget::obscures`](crate::Widget::obscures)), such as an alert, draw their subtrees after every other raised
/// one, with or without a layer, so a bar's controls never lie over them. A raised subtree's root still comes after
/// its parent.
pub(super) fn paint_order<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    raised: &IdSet<NodeId>,
    layer: bool,
) -> Vec<NodeId> {
    let all = tree.preorder(root);
    if raised.is_empty() {
        return all;
    }
    // where each lifted node goes: over the rest (`false`) or over every other raised node (`true`)
    let mut lifted: IdMap<NodeId, bool> = IdMap::default();
    let (mut rest, mut over, mut floating) =
        (Vec::with_capacity(all.len()), Vec::new(), Vec::new());
    for id in all {
        let Some(node) = tree.get(id) else { continue };
        let from_parent = node.parent.and_then(|p| lifted.get(&p).copied());
        let floats = match from_parent {
            Some(floats) => Some(floats),
            None if raised.contains(&id) => Some(!node.widget.obscures(&*node.state)),
            None => None,
        };
        match floats {
            Some(true) => floating.push(id),
            Some(false) if !layer => over.push(id),
            _ => rest.push(id),
        }
        if let Some(floats) = floats {
            lifted.insert(id, floats);
        }
    }
    rest.append(&mut over);
    rest.append(&mut floating);
    rest
}

/// Whether `ancestor` is `id` or holds it.
pub(super) fn within<M: 'static>(tree: &Tree<M>, ancestor: NodeId, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(node) = at {
        if node == ancestor {
            return true;
        }
        at = tree.get(node).and_then(|n| n.parent);
    }
    false
}

/// The list's boxes made whole: each node's bounds widened by its children and its group's effects, and what each
/// node damages when it changes. `clips` are the clips of each node's ancestors, in the list's order.
pub(super) fn finish(
    mut list: DisplayList,
    clips: &[Option<Rect>],
    blank: Vec<bool>,
    bump: &bumpalo::Bump,
) -> (DisplayList, Vec<Rect>, Vec<bool>) {
    let mut own: Vec<Rect> = list.nodes.iter().map(|n| n.bounds).collect();
    // Children come after their parent, so walking backwards widens each parent by its finished children.
    let mut index: IdMap<PaintNodeId, usize> =
        IdMap::with_capacity_and_hasher(list.nodes.len(), Default::default());
    for (i, n) in list.nodes.iter().enumerate() {
        index.insert(n.id, i);
    }
    for i in (0..list.nodes.len()).rev() {
        if let Some(group) = &list.nodes[i].group {
            let widened = with_effects(list.nodes[i].bounds, &group.effects);
            list.nodes[i].bounds = match clips[i] {
                Some(clip) => widened
                    .intersection(&clip.outset(CLIP_EDGE))
                    .unwrap_or(list.nodes[i].bounds),
                None => widened,
            };
        }
        let (bounds, parent) = (list.nodes[i].bounds, list.nodes[i].parent);
        if let Some(&p) = parent.and_then(|p| index.get(&p)) {
            list.nodes[p].bounds = list.nodes[p].bounds.union(&bounds);
        }
    }
    // A group's effects spread each pixel across their reach, so a group with effects damages all of it, and under
    // one any node damages the outermost such group's whole reach. A group of opacity alone changes only the pixels its
    // subtree draws: its box is what it draws itself, and the damage tracker adds each descendant's box when it changes.
    let mut spread: bumpalo::collections::Vec<'_, Option<usize>> =
        bumpalo::collections::Vec::with_capacity_in(list.nodes.len(), bump);
    for (i, (node, own)) in list.nodes.iter().zip(&mut own).enumerate() {
        let above = node
            .parent
            .and_then(|p| index.get(&p))
            .and_then(|&p| spread[p]);
        match &node.group {
            Some(group) if !group.effects.is_empty() => *own = node.bounds,
            Some(_) if node.ops.is_empty() => *own = Rect::new(own.min_x(), own.min_y(), 0.0, 0.0),
            _ => {}
        }
        if let Some(group) = above {
            *own = list.nodes[group].bounds;
        }
        let effects = node.group.as_ref().is_some_and(|g| !g.effects.is_empty());
        spread.push(above.or(effects.then_some(i)));
    }
    (list, own, blank)
}

/// The node's layout rect together with what its ops touch, in window coordinates, cut by its clip.
pub(super) fn bounds(rect: Rect, extent: Option<Rect>, clip: Option<Rect>) -> Rect {
    let reach = extent.map_or(rect, |e| {
        rect.union(&e.translate(rect.origin.x, rect.origin.y))
    });
    match clip {
        Some(clip) => reach
            .intersection(&clip.outset(CLIP_EDGE))
            .unwrap_or(Rect::new(rect.origin.x, rect.origin.y, 0.0, 0.0)),
        None => reach,
    }
}
