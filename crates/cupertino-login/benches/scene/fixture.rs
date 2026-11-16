//! A wallpaper made for the benches and captures, so neither reads a picture from the machine.

use cupertino_login::wallpaper::Picture;

/// A picture `width` by `height` pixels: soft fields of colour over a gradient, lighter for the light appearance and
/// deeper for the dark one, with fine grain, so the blur, the dimming and the scaling all have something to work on.
pub fn fixture_picture(width: u32, height: u32, dark: bool) -> Picture {
    let (w, h) = (f64::from(width), f64::from(height));
    // centre (share of the width and height), radius (share of the width), colour
    let blobs: [((f64, f64), f64, [f64; 3]); 4] = if dark {
        [
            ((0.22, 0.30), 0.42, [0.55, 0.20, 0.30]),
            ((0.78, 0.25), 0.38, [0.12, 0.22, 0.60]),
            ((0.55, 0.85), 0.45, [0.38, 0.14, 0.52]),
            ((0.10, 0.90), 0.30, [0.06, 0.40, 0.42]),
        ]
    } else {
        [
            ((0.22, 0.30), 0.42, [0.98, 0.62, 0.42]),
            ((0.78, 0.25), 0.38, [0.45, 0.65, 0.98]),
            ((0.55, 0.85), 0.45, [0.82, 0.52, 0.92]),
            ((0.10, 0.90), 0.30, [0.40, 0.88, 0.80]),
        ]
    };
    let base = if dark {
        [0.04, 0.04, 0.10]
    } else {
        [0.62, 0.70, 0.86]
    };
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    for (y, row) in rgb.chunks_exact_mut(width as usize * 3).enumerate() {
        let v = y as f64 / h;
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let u = x as f64 / w;
            let mut c = [
                base[0] + 0.10 * v,
                base[1] + 0.06 * u,
                base[2] + 0.12 * (1.0 - v),
            ];
            for &((bx, by), r, colour) in &blobs {
                let (dx, dy) = ((u - bx) * w, (v - by) * h);
                let d2 = (dx * dx + dy * dy) / (r * w * r * w);
                let a = (-2.5 * d2).exp();
                for (c, colour) in c.iter_mut().zip(colour) {
                    *c += (colour - *c) * a;
                }
            }
            // grain, from the pixel's place
            let grain = ((x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)) % 17) as f64 / 17.0 - 0.5;
            for (out, c) in px.iter_mut().zip(c) {
                *out = ((c + grain * 0.02).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    Picture::from_rgb(width, height, rgb).expect("a picture of allowed sides")
}
