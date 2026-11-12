//! Compares two screenshots of the same size for scripts/preview.
//!
//!     compare-shots BEFORE.png AFTER.png DIFF.png
//!
//! Prints `CHANGED X Y WIDTH HEIGHT`: how many pixels differ by more than [`TOLERANCE`] in any channel, and the
//! rectangle that holds them (zeros when none does). DIFF.png is AFTER faded to grey with the changed pixels in magenta.
//! Shots of different sizes count every pixel as changed, and DIFF.png is AFTER as it is.

use tiny_skia::Pixmap;

/// The largest difference in one channel, out of 255, that still counts as the same pixel.
const TOLERANCE: u8 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [before, after, diff] = args.as_slice() else {
        eprintln!("usage: compare-shots BEFORE.png AFTER.png DIFF.png");
        std::process::exit(2);
    };
    let after = Pixmap::load_png(after).expect("AFTER is a PNG");
    let Ok(before) = Pixmap::load_png(before) else {
        // nothing to compare against: every pixel is new
        after.save_png(diff).expect("write DIFF");
        println!(
            "CHANGED {} 0 0 {} {}",
            after.width() * after.height(),
            after.width(),
            after.height()
        );
        return;
    };
    if (before.width(), before.height()) != (after.width(), after.height()) {
        after.save_png(diff).expect("write DIFF");
        println!(
            "CHANGED {} 0 0 {} {}",
            after.width() * after.height(),
            after.width(),
            after.height()
        );
        return;
    }
    let width = after.width() as usize;
    let mut out = after.clone();
    let (mut count, mut x0, mut y0, mut x1, mut y1) = (0u64, usize::MAX, usize::MAX, 0, 0);
    let pixels = before
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.data().as_chunks::<4>().0);
    for (i, ((b, a), o)) in pixels
        .zip(out.data_mut().as_chunks_mut::<4>().0)
        .enumerate()
    {
        let changed = b.iter().zip(a).any(|(b, a)| b.abs_diff(*a) > TOLERANCE);
        if changed {
            let (x, y) = (i % width, i / width);
            count += 1;
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            o.copy_from_slice(&[255, 0, 255, 255]);
        } else {
            let grey = ((a[0] as u32 * 3 + a[1] as u32 * 6 + a[2] as u32) / 10 / 3 + 150) as u8;
            o.copy_from_slice(&[grey, grey, grey, 255]);
        }
    }
    out.save_png(diff).expect("write DIFF");
    if count == 0 {
        println!("CHANGED 0 0 0 0 0");
    } else {
        println!("CHANGED {count} {x0} {y0} {} {}", x1 - x0 + 1, y1 - y0 + 1);
    }
}
