//! Renders artwork through the same image loader used by the shell.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected SVG, output PNG and pixel size".into());
    }
    let source = PathBuf::from(&args[0]);
    let size: u32 = args[2].parse()?;
    let image = cupertino_icons::load_icon(&source, size).ok_or("artwork did not render")?;
    let dimensions = tiny_skia::IntSize::from_wh(image.width, image.height)
        .ok_or("artwork dimensions were empty")?;
    let pixmap = tiny_skia::Pixmap::from_vec(image.rgba, dimensions)
        .ok_or("artwork pixels did not match dimensions")?;
    pixmap.save_png(&args[1])?;
    Ok(())
}
