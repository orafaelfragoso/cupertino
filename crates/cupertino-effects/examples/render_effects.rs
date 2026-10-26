//! Renders sample images of every effect into `output/`.
//!
//! `cargo run -p cupertino-effects --release --example render_effects`

use cupertino_effects::raster::{contact_sheet, draw_mesh, draw_wireframe, window_test_pattern};
use cupertino_effects::*;
use cupertino_geometry::Rect;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

const OUT_W: f32 = 1280.0;
const OUT_H: f32 = 800.0;
const SCALE: f32 = 0.3;

fn background() -> Pixmap {
    let w = (OUT_W * SCALE) as u32;
    let h = (OUT_H * SCALE) as u32;
    let mut pm = Pixmap::new(w, h).unwrap();
    for y in 0..h {
        let t = y as f32 / h as f32;
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8((30.0 + 40.0 * t) as u8, (50.0 + 50.0 * t) as u8, (90.0 + 60.0 * t) as u8, 255));
        pm.fill_rect(tiny_skia::Rect::from_xywh(0.0, y as f32, w as f32, 1.0).unwrap(), &paint, Transform::identity(), None);
    }
    pm
}

fn dock(pm: &mut Pixmap, slot: Rect) {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(255, 255, 255, 70));
    let bar = tiny_skia::Rect::from_xywh(
        (slot.min_x() as f32 - 200.0) * SCALE,
        (OUT_H - 90.0) * SCALE,
        (slot.width() as f32 + 400.0) * SCALE,
        80.0 * SCALE,
    )
    .unwrap();
    pm.fill_rect(bar, &paint, Transform::identity(), None);
    paint.set_color(Color::from_rgba8(255, 214, 10, 200));
    let mut pb = PathBuilder::new();
    pb.push_rect(tiny_skia::Rect::from_xywh(slot.min_x() as f32 * SCALE, slot.min_y() as f32 * SCALE, slot.width() as f32 * SCALE, slot.height() as f32 * SCALE).unwrap());
    pm.fill_path(&pb.finish().unwrap(), &paint, FillRule::Winding, Transform::identity(), None);
}

fn frame(effect: &MinimizeEffect, tex: &Pixmap, p: f64, wire: bool) -> Pixmap {
    let mut pm = background();
    dock(&mut pm, effect.slot);
    let mesh = effect.mesh(p).scaled(SCALE);
    draw_mesh(&mut pm, tex, &mesh, (0.0, 0.0), 1.0);
    if wire {
        draw_wireframe(&mut pm, &mesh, (0.0, 0.0), Color::from_rgba8(255, 255, 255, 200));
    }
    pm
}

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("output");
    std::fs::create_dir_all(&out).unwrap();
    let window = Rect::new(220.0, 50.0, 560.0, 380.0);
    let slot = Rect::new(840.0, 720.0, 64.0, 64.0);
    let tex = window_test_pattern(560, 380);
    let bg = Color::from_rgba8(18, 18, 22, 255);

    let steps = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
    let mut frames = Vec::new();
    for kind in MinimizeKind::ALL {
        let e = MinimizeEffect::new(kind, window, slot, DockEdge::Bottom);
        for &p in &steps {
            frames.push(frame(&e, &tex, p, false));
        }
    }
    contact_sheet(&frames, steps.len(), 4, bg).save_png(out.join("minimize_contact_sheet.png")).unwrap();

    let e = MinimizeEffect::new(MinimizeKind::Warp, window, slot, DockEdge::Bottom);
    let wire: Vec<Pixmap> = [0.0, 0.15, 0.3, 0.45, 0.6, 0.75, 0.9, 1.0].iter().map(|&p| frame(&e, &tex, p, true)).collect();
    contact_sheet(&wire, 4, 4, bg).save_png(out.join("warp_wireframe.png")).unwrap();

    let mut dirs = Vec::new();
    for (edge, win, sl) in [
        (DockEdge::Bottom, window, slot),
        (DockEdge::Bottom, Rect::new(220.0, 560.0, 560.0, 300.0), Rect::new(840.0, 100.0, 64.0, 64.0)),
        (DockEdge::Right, window, Rect::new(1180.0, 380.0, 64.0, 64.0)),
        (DockEdge::Left, Rect::new(420.0, 50.0, 560.0, 380.0), Rect::new(20.0, 380.0, 64.0, 64.0)),
    ] {
        let e = MinimizeEffect::new(MinimizeKind::Warp, win, sl, edge);
        for &p in &[0.0, 0.25, 0.5, 0.75] {
            let mut pm = background();
            let mesh = e.mesh(p).scaled(SCALE);
            draw_mesh(&mut pm, &tex, &mesh, (0.0, 0.0), 1.0);
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(255, 214, 10, 200));
            let mut pb = PathBuilder::new();
            pb.push_rect(tiny_skia::Rect::from_xywh(sl.min_x() as f32 * SCALE, sl.min_y() as f32 * SCALE, sl.width() as f32 * SCALE, sl.height() as f32 * SCALE).unwrap());
            pm.fill_path(&pb.finish().unwrap(), &paint, FillRule::Winding, Transform::identity(), None);
            dirs.push(pm);
        }
    }
    contact_sheet(&dirs, 4, 4, bg).save_png(out.join("warp_directions.png")).unwrap();

    let mut seq = Vec::new();
    let mut anim = MinimizeAnimation::new(MinimizeEffect::new(MinimizeKind::Warp, window, slot, DockEdge::Bottom), false);
    anim.minimize(0.0);
    for k in 0..16 {
        let t = k as f64 * 0.03;
        if k == 9 {
            anim.reverse(t);
        }
        let mut pm = background();
        dock(&mut pm, slot);
        draw_mesh(&mut pm, &tex, &anim.mesh(t).scaled(SCALE), (0.0, 0.0), 1.0);
        seq.push(pm);
    }
    contact_sheet(&seq, 8, 4, bg).save_png(out.join("warp_interrupted_at_270ms.png")).unwrap();

    let screen = Rect::new(0.0, 0.0, OUT_W as f64, OUT_H as f64);
    let win = Rect::new(380.0, 200.0, 520.0, 360.0);
    let mut t_frames = Vec::new();
    let draw_placement = |pm: &mut Pixmap, p: Placement, tex: &Pixmap| draw_mesh(pm, tex, &p.mesh().scaled(SCALE), (0.0, 0.0), p.opacity as f32);
    for &p in &[0.0, 0.3, 0.6, 1.0] {
        let mut pm = background();
        draw_placement(&mut pm, TransformKind::PopIn.placement(win, p), &tex);
        t_frames.push(pm);
    }
    let ws = WorkspaceSlide::new(OUT_W as f64);
    for &pos in &[0.0, 0.25, 0.5, 0.75] {
        let mut pm = background();
        for idx in 0..2 {
            if ws.is_visible(idx, pos) {
                draw_placement(&mut pm, ws.placement(win, idx, pos), &tex);
            }
        }
        t_frames.push(pm);
    }
    let ov = OverviewZoom::default();
    let slot_ov = Rect::new(120.0, 120.0, 340.0, 230.0);
    for &p in &[0.0, 0.33, 0.66, 1.0] {
        let mut pm = background();
        draw_placement(&mut pm, ov.placement(win, slot_ov, p), &tex);
        t_frames.push(pm);
    }
    let sd = ShowDesktop::default();
    for &p in &[0.0, 0.33, 0.66, 1.0] {
        let mut pm = background();
        draw_placement(&mut pm, sd.placement(win, screen, p), &tex);
        t_frames.push(pm);
    }
    contact_sheet(&t_frames, 4, 4, bg).save_png(out.join("transitions_contact_sheet.png")).unwrap();
    println!("wrote output/*.png");
}
