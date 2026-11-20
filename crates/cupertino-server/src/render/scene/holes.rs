//! What the server draws in a hole (`cupertino_hole_v1`): the hole's source fitted and centred in its layer's bounds
//! as the layer shows, so it moves, scales and fades with the layer, and no client receives its pixels. A window is
//! drawn as it shows, with its decorations, shadow and popups, live; a hole whose claim is suspended under a later one
//! draws nothing. A thumbnail is the picture of a minimized window as it last showed, a workspace its wallpaper and
//! windows, live. After its window closed, a hole fades the window's last picture out where it lay, along the server's
//! closing animation, then draws nothing. A capture leaves out of a hole what it leaves out at the source's own place.

use super::compose::*;
use super::*;
use crate::transaction::{ClosedSource, HoleSource};

/// Bit set in the indices of the pictures holes draw (a thumbnail, a closing window, a workspace's wallpaper), apart from
/// the pieces that draw the same picture elsewhere.
const HOLE_PICTURE_KEY: u64 = 1 << 61;

/// The pieces of the source of the hole of `layer`, if the layer is a hole, front to back: fitted and centred in
/// `bounds` (output-local points, the layer's bounds as shown) at `opacity`. The hole of a layer shell surface's layer
/// (`on_shell_surface`) notes where it showed, which a hold of the surface keeps ([`crate::transaction::HeldHole`]).
pub(crate) fn hole_pieces(
    p: &mut SceneParams<'_>,
    layer: &WlSurface,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
    on_shell_surface: bool,
) -> Vec<Piece> {
    use smithay::reexports::wayland_server::Resource;
    let Some(hole) = p.transactions.hole_for_layer(&layer.id()) else {
        return Vec::new();
    };
    if on_shell_surface && !p.for_capture {
        hole.shown.set(Some((p.output_id, bounds)));
    }
    source_pieces(p, hole, bounds, opacity)
}

/// The holes of the persistent surfaces held on the output after their component's connection ended (D19) that show
/// (`shown`) and whose pictures do not draw them among their slices (`HeldShown::placed`), front to back: each draws
/// its source live where it last showed, behind the surface's last picture, moved and faded as its surface shows.
pub(crate) fn held_hole_pieces(
    p: &mut SceneParams<'_>,
    shown: &[layers::HeldShown<'_>],
) -> Vec<Piece> {
    let transactions = p.transactions;
    let mut out = Vec::new();
    for hole in transactions.held_holes() {
        let Some(on) = shown.iter().find(|on| holds(on, hole)) else {
            continue;
        };
        if !on.placed.contains(&hole.layer_surface_id) {
            out.extend(held_source_pieces(p, on, hole));
        }
    }
    out
}

/// The hole of the layer `layer` (its `wl_surface`) of the held surface shown as `on`, drawn live where it last
/// showed, moved and faded as its surface shows: what the surface's picture draws at the layer's place among its slices,
/// behind the layer's content and in front of its glass, as the live layer drew it.
pub(super) fn held_hole_in_slot(
    p: &mut SceneParams<'_>,
    on: &layers::HeldShown<'_>,
    layer: &smithay::reexports::wayland_server::backend::ObjectId,
) -> Vec<Piece> {
    let transactions = p.transactions;
    let Some(hole) = transactions
        .held_holes()
        .find(|hole| hole.layer_surface_id == *layer && holds(on, hole))
    else {
        return Vec::new();
    };
    held_source_pieces(p, on, hole)
}

/// Whether `hole` lies on the held surface shown as `on`.
fn holds(on: &layers::HeldShown<'_>, hole: &crate::transaction::HoleRecord) -> bool {
    hole.held
        .as_ref()
        .is_some_and(|held| on.component == hole.component && on.key == held.key)
}

/// The source of the held hole `hole` where it last showed on its held surface, shown as `on`; nothing on another
/// output.
fn held_source_pieces(
    p: &mut SceneParams<'_>,
    on: &layers::HeldShown<'_>,
    hole: &crate::transaction::HoleRecord,
) -> Vec<Piece> {
    let Some(held) = hole.held.as_ref().filter(|held| held.output == p.output_id) else {
        return Vec::new();
    };
    let b = held.bounds;
    let bounds = Rectangle::new((b.loc.x, b.loc.y + on.dy).into(), b.size);
    source_pieces(p, hole, bounds, on.opacity)
}

/// The pieces of `hole`'s source fitted and centred in `bounds` (output-local points) at `opacity`.
fn source_pieces(
    p: &mut SceneParams<'_>,
    hole: &crate::transaction::HoleRecord,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
) -> Vec<Piece> {
    let transactions = p.transactions;
    if opacity <= 0.0 || bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
        return Vec::new();
    }
    if !transactions.enter_hole() {
        return Vec::new();
    }
    let key = HOLE_PICTURE_KEY | hole.id;
    let out = match hole.committed_source {
        HoleSource::Window(w)
            if !hole.committed_claim || transactions.claim_in_force(w, hole.id) =>
        {
            window_in_hole(p, w, bounds, opacity)
        }
        HoleSource::Window(_) => Vec::new(),
        HoleSource::Thumbnail(w) => thumbnail_in_hole(p, key, w, bounds, opacity)
            .into_iter()
            .collect(),
        HoleSource::Workspace(ws) => workspace_in_hole(p, key, ws, bounds, opacity),
        HoleSource::None => hole
            .closing
            .and_then(|closed| closing_in_hole(p, key, closed, bounds, opacity))
            .into_iter()
            .collect(),
    };
    transactions.leave_hole();
    out
}

/// `size` fitted in `bounds` with its shape kept, and centred; `None` for an empty size.
fn fitted(size: (f64, f64), bounds: Rectangle<f64, Logical>) -> Option<Rectangle<f64, Logical>> {
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return None;
    }
    let k = (bounds.size.w / size.0).min(bounds.size.h / size.1);
    let (w, h) = (size.0 * k, size.1 * k);
    Some(Rectangle::new(
        (
            bounds.loc.x + (bounds.size.w - w) / 2.0,
            bounds.loc.y + (bounds.size.h - h) / 2.0,
        )
            .into(),
        (w, h).into(),
    ))
}

/// Window `id` as it shows, live, its frame (decorations included) fitted in `bounds`: its popups, decorations, content,
/// layers and shadow, as the stage strip draws a placed window.
fn window_in_hole(
    p: &mut SceneParams<'_>,
    id: WindowId,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
) -> Vec<Piece> {
    let windows = p.windows;
    let Some(entry) = windows.iter().find(|w| w.id == id && w.mapped) else {
        return Vec::new();
    };
    if p.for_capture && crate::control::protected_surface(entry.surface()) {
        return Vec::new();
    }
    let geo = entry.window.geometry();
    let ins = entry.insets();
    let natural = (
        f64::from(geo.size.w) + ins.horizontal(),
        f64::from(geo.size.h) + ins.vertical(),
    );
    let Some(fit) = fitted(natural, bounds) else {
        return Vec::new();
    };
    let k = fit.size.w / natural.0;
    let rect = Rect::new(
        p.rect.x + fit.loc.x,
        p.rect.y + fit.loc.y,
        fit.size.w,
        fit.size.h,
    );
    let corner = p.config.window.corner_radius * k;
    let plan = placed_plan(entry, rect, f64::from(opacity), 1.0, corner);
    let active = p.wm.key_window() == Some(id);
    let deco = window_decorations(p, entry, &plan, active);
    let mut out = window_popup_elements(p, entry, &plan);
    out.extend(window_elements(p, entry, &plan, active, true, deco));
    out
}

/// The picture of minimized window `id` as it last showed (its stored snapshot, shadow included), its frame fitted in
/// `bounds`.
fn thumbnail_in_hole(
    p: &SceneParams<'_>,
    key: u64,
    id: WindowId,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
) -> Option<Piece> {
    let protected = p
        .windows
        .iter()
        .find(|w| w.id == id)
        .is_some_and(|e| p.for_capture && crate::control::protected_surface(e.surface()));
    if protected {
        return None;
    }
    let (owner, area, origin) = p.fx.snapshot_of(id)?;
    // the snapshot covers the frame and the shadow's reach around it on every side
    let margin = ((origin.0 - area.x).max(0.0), (origin.1 - area.y).max(0.0));
    let frame = (area.w - 2.0 * margin.0, area.h - 2.0 * margin.1);
    let fit = fitted(frame, bounds)?;
    let k = fit.size.w / frame.0;
    let quad = Rect::new(
        fit.loc.x - margin.0 * k,
        fit.loc.y - margin.1 * k,
        area.w * k,
        area.h * k,
    );
    picture_piece(p, owner.id(), key, quad, opacity, 0)
}

/// The last picture of a hole's window that closed, where and as far as it shows now along the closing animation,
/// fitted as the window's frame was; `None` once it faded out.
fn closing_in_hole(
    p: &SceneParams<'_>,
    key: u64,
    closed: ClosedSource,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
) -> Option<Piece> {
    let ghost = p.fx.closing.iter().find(|c| c.id == closed.window)?;
    let fit = fitted(closed.size, bounds)?;
    let k = fit.size.w / closed.size.0;
    let (shown, alpha) = ghost.placement(p.now);
    let alpha = alpha as f32 * opacity;
    if alpha <= 0.0 {
        return None;
    }
    // the frame lies at the centre of its captured area; the picture moves and scales about it as the animation says
    let (cx, cy) = (
        fit.loc.x + fit.size.w / 2.0 + (shown.cx() - ghost.rect.cx()) * k,
        fit.loc.y + fit.size.h / 2.0 + (shown.cy() - ghost.rect.cy()) * k,
    );
    let (w, h) = (shown.w * k, shown.h * k);
    let quad = Rect::new(cx - w / 2.0, cy - h / 2.0, w, h);
    let revision = Sig::new().f(ghost.progress(p.now)).finish();
    picture_piece(p, ghost.snapshot.id(), key, quad, alpha, revision)
}

/// Workspace `ws` as it is, live: its output fitted in `bounds`, the wallpaper and the workspace's windows scaled into
/// it.
fn workspace_in_hole(
    p: &mut SceneParams<'_>,
    key: u64,
    ws: cupertino_wm::WorkspaceId,
    bounds: Rectangle<f64, Logical>,
    opacity: f32,
) -> Vec<Piece> {
    let wm = p.wm;
    let Some(output) = wm
        .outputs()
        .iter()
        .find(|o| wm.workspaces(o.id).iter().any(|w| w.id == ws))
    else {
        return Vec::new();
    };
    let Some(fit) = fitted((output.rect.w, output.rect.h), bounds) else {
        return Vec::new();
    };
    let frame = Rect::new(
        p.rect.x + fit.loc.x,
        p.rect.y + fit.loc.y,
        fit.size.w,
        fit.size.h,
    );
    workspace_picture(p, (ws, output.id, output.rect), frame, opacity, true, key)
}

/// A snapshot drawn as a quad over `quad` (output-local points) at `opacity`; `None` without the mesh pass or the
/// snapshot.
fn picture_piece(
    p: &SceneParams<'_>,
    snapshot: cupertino_scene::SnapshotId,
    index: u64,
    quad: Rect,
    opacity: f32,
    revision: u64,
) -> Option<Piece> {
    p.res.mesh.as_ref()?;
    let lease = p.res.snapshots.pin(snapshot).ok()?;
    Some(Piece::Mesh(MeshPiece {
        kind: SceneKind::LayerContent,
        index,
        snapshot: lease.id(),
        keep: SnapshotKeep::Pinned(lease),
        mesh: Mesh::quad(quad.into_geom()),
        opacity,
        revision,
    }))
}
