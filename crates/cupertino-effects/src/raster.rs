//! A small CPU renderer for textured meshes, used for tests and sample images.
//!
//! The mesh is rasterised into a 2× supersampled buffer (later triangles overwrite earlier ones, so
//! shared edges leave no seams), box-filtered and composited source-over onto the destination.
//! Textures are sampled bilinearly with clamped coordinates. All pixmaps hold premultiplied RGBA.

use crate::mesh::Mesh;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Stroke, Transform};

const SS: usize = 2;

fn texel(tex: &Pixmap, x: i64, y: i64) -> [f32; 4] {
    let (w, h) = (tex.width() as i64, tex.height() as i64);
    let (x, y) = (x.clamp(0, w - 1) as usize, y.clamp(0, h - 1) as usize);
    let d = &tex.data()[(y * w as usize + x) * 4..][..4];
    [d[0] as f32, d[1] as f32, d[2] as f32, d[3] as f32]
}

fn sample(tex: &Pixmap, u: f32, v: f32) -> [f32; 4] {
    let x = u * tex.width() as f32 - 0.5;
    let y = v * tex.height() as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (texel(tex, x0, y0), texel(tex, x0 + 1, y0), texel(tex, x0, y0 + 1), texel(tex, x0 + 1, y0 + 1));
    std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

/// Draw `mesh` (positions in output coordinates, shifted by `offset`) textured with `texture` onto
/// `dst`, scaled by `opacity`.
pub fn draw_mesh(dst: &mut Pixmap, texture: &Pixmap, mesh: &Mesh, offset: (f32, f32), opacity: f32) {
    let (w, h) = (dst.width() as usize, dst.height() as usize);
    let (sw, sh) = (w * SS, h * SS);
    let mut buf = vec![[0.0f32; 4]; sw * sh];
    let idx = mesh.indices();
    let vert = |i: u32| {
        let v = mesh.vertices[i as usize];
        ((v.position[0] + offset.0) * SS as f32, (v.position[1] + offset.1) * SS as f32, v.uv[0], v.uv[1])
    };
    for &[i0, i1, i2] in idx.as_chunks::<3>().0 {
        let (a, b, c) = (vert(i0), vert(i1), vert(i2));
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-9 {
            continue;
        }
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let max_x = (a.0.max(b.0).max(c.0).ceil().max(0.0) as usize).min(sw);
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let max_y = (a.1.max(b.1).max(c.1).ceil().max(0.0) as usize).min(sh);
        for y in min_y..max_y {
            for x in min_x..max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6 {
                    continue;
                }
                let u = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let v = w0 * a.3 + w1 * b.3 + w2 * c.3;
                buf[y * sw + x] = sample(texture, u, v);
            }
        }
    }
    let data = dst.data_mut();
    let n = (SS * SS) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let s = buf[(y * SS + sy) * sw + x * SS + sx];
                    for i in 0..4 {
                        acc[i] += s[i];
                    }
                }
            }
            let src: [f32; 4] = std::array::from_fn(|i| acc[i] / n * opacity);
            if src[3] <= 0.0 {
                continue;
            }
            let inv = 1.0 - src[3] / 255.0;
            let px = &mut data[(y * w + x) * 4..][..4];
            for i in 0..4 {
                px[i] = (src[i] + px[i] as f32 * inv).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Draw the grid lines of the mesh (rows, columns and cell diagonals) in `color`.
pub fn draw_wireframe(dst: &mut Pixmap, mesh: &Mesh, offset: (f32, f32), color: Color) {
    let mut pb = PathBuilder::new();
    let p = |c: usize, r: usize| {
        let v = mesh.vertex(c, r);
        (v.position[0] + offset.0, v.position[1] + offset.1)
    };
    for r in 0..mesh.rows {
        for c in 0..mesh.cols {
            let (x, y) = p(c, r);
            if c + 1 < mesh.cols {
                let (x2, y2) = p(c + 1, r);
                pb.move_to(x, y);
                pb.line_to(x2, y2);
            }
            if r + 1 < mesh.rows {
                let (x2, y2) = p(c, r + 1);
                pb.move_to(x, y);
                pb.line_to(x2, y2);
            }
        }
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        dst.stroke_path(&path, &paint, &Stroke { width: 1.0, ..Stroke::default() }, Transform::identity(), None);
    }
}

/// A window-like pattern: title bar with three round buttons, a checker body with a gradient
/// stripe and a coloured frame, so distortion and orientation are easy to see.
pub fn window_test_pattern(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).expect("non-zero size");
    let (wf, hf) = (w as f32, h as f32);
    let fill = |pm: &mut Pixmap, x: f32, y: f32, ww: f32, hh: f32, c: Color| {
        if let Some(r) = SkRect::from_xywh(x, y, ww, hh) {
            let mut paint = Paint::default();
            paint.set_color(c);
            pm.fill_rect(r, &paint, Transform::identity(), None);
        }
    };
    fill(&mut pm, 0.0, 0.0, wf, hf, Color::from_rgba8(238, 238, 240, 255));
    let cell = (wf / 16.0).max(8.0);
    let top = (hf * 0.08).max(24.0);
    let mut y = top;
    let mut row = 0;
    while y < hf {
        let mut x = 0.0;
        let mut col = 0;
        while x < wf {
            if (row + col) % 2 == 0 {
                fill(&mut pm, x, y, cell.min(wf - x), cell.min(hf - y), Color::from_rgba8(214, 218, 226, 255));
            }
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
    fill(&mut pm, 0.0, 0.0, wf, top, Color::from_rgba8(60, 64, 74, 255));
    for (i, c) in [(255, 95, 86), (255, 189, 46), (39, 201, 63)].iter().enumerate() {
        let cx = top * 0.5 + i as f32 * top * 0.6;
        let mut pb = PathBuilder::new();
        pb.push_circle(cx, top * 0.5, top * 0.2);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(c.0, c.1, c.2, 255));
            paint.anti_alias = true;
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    let stripe_y = top + (hf - top) * 0.45;
    let stripe_h = (hf - top) * 0.12;
    for i in 0..(w as usize) {
        let t = i as f32 / wf;
        fill(&mut pm, i as f32, stripe_y, 1.0, stripe_h, Color::from_rgba8((40.0 + 200.0 * t) as u8, (110.0 + 60.0 * (1.0 - t)) as u8, (230.0 - 150.0 * t) as u8, 255));
    }
    let b = (wf.min(hf) * 0.012).max(2.0);
    for (x, y, ww, hh) in [(0.0, 0.0, wf, b), (0.0, hf - b, wf, b), (0.0, 0.0, b, hf), (wf - b, 0.0, b, hf)] {
        fill(&mut pm, x, y, ww, hh, Color::from_rgba8(230, 60, 60, 255));
    }
    pm
}

/// Lay `frames` out in a grid with `cols` columns and `pad` pixels between them.
pub fn contact_sheet(frames: &[Pixmap], cols: usize, pad: u32, background: Color) -> Pixmap {
    let cols = cols.max(1);
    let rows = frames.len().div_ceil(cols).max(1);
    let (cw, ch) = frames.iter().fold((1, 1), |a, f| (a.0.max(f.width()), a.1.max(f.height())));
    let (w, h) = (cols as u32 * (cw + pad) + pad, rows as u32 * (ch + pad) + pad);
    let mut sheet = Pixmap::new(w, h).expect("non-zero size");
    sheet.fill(background);
    for (i, f) in frames.iter().enumerate() {
        let (c, r) = ((i % cols) as u32, (i / cols) as u32);
        sheet.draw_pixmap((pad + c * (cw + pad)) as i32, (pad + r * (ch + pad)) as i32, f.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Rect;

    fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let d = &p.data()[((y * p.width() + x) * 4) as usize..][..4];
        [d[0], d[1], d[2], d[3]]
    }

    #[test]
    fn identity_mesh_reproduces_the_texture() {
        let tex = window_test_pattern(64, 48);
        let mut dst = Pixmap::new(64, 48).unwrap();
        draw_mesh(&mut dst, &tex, &Mesh::quad(Rect::new(0.0, 0.0, 64.0, 48.0)), (0.0, 0.0), 1.0);
        let mut sum = 0i64;
        for y in 1..47 {
            for x in 1..63 {
                let (a, b) = (px(&tex, x, y), px(&dst, x, y));
                for i in 0..4 {
                    sum += (a[i] as i64 - b[i] as i64).abs();
                }
            }
        }
        let mean = sum as f64 / (46.0 * 62.0 * 4.0);
        assert!(mean < 5.0, "mean channel difference {mean}");
        assert_eq!(px(&dst, 32, 24)[3], 255);
    }

    #[test]
    fn mesh_outside_the_destination_is_clipped() {
        let tex = window_test_pattern(32, 32);
        let mut dst = Pixmap::new(16, 16).unwrap();
        draw_mesh(&mut dst, &tex, &Mesh::quad(Rect::new(-100.0, -100.0, 50.0, 50.0)), (0.0, 0.0), 1.0);
        assert!(dst.data().iter().all(|&b| b == 0));
    }

    #[test]
    fn opacity_scales_alpha() {
        let tex = window_test_pattern(32, 32);
        let mut dst = Pixmap::new(32, 32).unwrap();
        draw_mesh(&mut dst, &tex, &Mesh::quad(Rect::new(0.0, 0.0, 32.0, 32.0)), (0.0, 0.0), 0.5);
        let a = px(&dst, 16, 16)[3];
        assert!((a as i32 - 128).abs() <= 2, "{a}");
    }

    #[test]
    fn contact_sheet_dimensions() {
        let f = Pixmap::new(10, 20).unwrap();
        let s = contact_sheet(&[f.clone(), f.clone(), f.clone()], 2, 4, Color::BLACK);
        assert_eq!((s.width(), s.height()), (2 * 14 + 4, 2 * 24 + 4));
    }
}
