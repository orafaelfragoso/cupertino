//! Captured pixels: from the shm buffer the server filled to an image, turned upright when the server sent it turned,
//! the selection cut out of an output's image, the thumbnail's smaller copy, and the PNG written to disk or offered on
//! the clipboard.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use cupertino_geometry::{area_in_buffer, OutputTransform, Rect};
use tiny_skia::{IntRect, Pixmap};

/// Pixel format of a captured shm buffer (`wl_shm` formats, 32-bit little-endian words, so the bytes of a
/// pixel are blue, green, red, alpha).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Argb8888,
    /// The alpha byte is undefined and the image opaque.
    Xrgb8888,
}

/// The image in a shm buffer of `width` x `height` pixels with rows `stride` bytes apart. The server's pixels are
/// premultiplied; a colour channel above its alpha (which a renderer's rounding can produce) is limited to it.
pub fn from_shm(
    width: u32,
    height: u32,
    stride: usize,
    data: &[u8],
    format: Format,
) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let row = width as usize * 4;
    if stride < row || data.len() < stride * (height as usize - 1) + row {
        return None;
    }
    let out = pixmap.data_mut();
    for y in 0..height as usize {
        let src = &data[y * stride..y * stride + row];
        let dst = &mut out[y * row..(y + 1) * row];
        for (s, d) in src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<4>().0.iter_mut())
        {
            let a = match format {
                Format::Argb8888 => s[3],
                Format::Xrgb8888 => 255,
            };
            *d = [s[2].min(a), s[1].min(a), s[0].min(a), a];
        }
    }
    Some(pixmap)
}

/// The upright picture of a buffer that holds it turned by `transform` (the `transform` of a copied frame).
pub fn upright(image: &Pixmap, transform: OutputTransform) -> Pixmap {
    if transform == OutputTransform::Normal {
        return image.clone();
    }
    let (bw, bh) = (image.width(), image.height());
    let (uw, uh) = transform.turn_pixels((bw, bh));
    let mut out = Pixmap::new(uw, uh).expect("the image has a size");
    let src = image.data();
    let dst = out.data_mut();
    for y in 0..bh {
        for x in 0..bw {
            let (ux, uy) = transform.upright_pixel((x, y), (bw, bh));
            let i = ((y * bw + x) * 4) as usize;
            let o = ((uy * uw + ux) * 4) as usize;
            dst[o..o + 4].copy_from_slice(&src[i..i + 4]);
        }
    }
    out
}

/// The upright picture of `area` (global points) of the output at `output` (global points), cut from the buffer
/// `image` that holds the output's picture turned by `transform`, at the buffer's own scale: the pixels the area covers
/// ([`area_in_buffer`]), turned upright. `None` when the area covers none of the output.
pub fn area(
    image: &Pixmap,
    area: &Rect,
    output: &Rect,
    transform: OutputTransform,
) -> Option<Pixmap> {
    let a = area_in_buffer(area, output, (image.width(), image.height()), transform)?;
    let rect = IntRect::from_xywh(a.x as i32, a.y as i32, a.width, a.height)?;
    Some(upright(&image.clone_rect(rect)?, transform))
}

/// `image` reduced to `width` x `height` pixels by averaging the source pixels each target pixel covers, so a capture
/// a tenth of its size stays smooth.
pub fn downscale(image: &Pixmap, width: u32, height: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let src = image.data();
    let dst = out.data_mut();
    for y in 0..height as usize {
        let (y0, y1) = (
            y * sh / height as usize,
            ((y + 1) * sh).div_ceil(height as usize).min(sh),
        );
        for x in 0..width as usize {
            let (x0, x1) = (
                x * sw / width as usize,
                ((x + 1) * sw).div_ceil(width as usize).min(sw),
            );
            let mut sum = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * sw + xx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(src[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u32;
            let o = (y * width as usize + x) * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    Some(out)
}

/// The PNG file of `image`.
pub fn png(image: &Pixmap) -> Result<Vec<u8>, String> {
    image
        .encode_png()
        .map_err(|e| format!("encoding the PNG failed: {e}"))
}

/// Create `path` holding `bytes`. A reader that finds the file finds all of it, and a file that is already there is
/// never replaced (`AlreadyExists`): the bytes go to a hidden file in the same directory, which is linked to `path`
/// and removed. Where the file system has no hard links the hidden file is renamed with `RENAME_NOREPLACE`, and where
/// that is unsupported too the bytes are written to `path` directly (`create_new`: not atomic, never replacing).
/// Every other error is returned.
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_new_with(path, bytes, &Publish::SYSTEM)
}

/// Create `path` (never replacing a file) and fill it with `write`. A file this call created and could not fill is
/// removed, so no partial file stays under the name.
fn create_and_write(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = write(&mut file);
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

/// How the hidden file reaches its name, apart so that a test can stand in for a file system that lacks one.
struct Publish {
    link: fn(&Path, &Path) -> io::Result<()>,
    rename_noreplace: fn(&Path, &Path) -> io::Result<()>,
}

impl Publish {
    const SYSTEM: Publish = Publish {
        link: |part, path| std::fs::hard_link(part, path),
        rename_noreplace: |part, path| {
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                part,
                rustix::fs::CWD,
                path,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(io::Error::from)
        },
    };
}

/// `link` failed for a file system that has no hard links (FAT, exFAT, some FUSE).
fn no_hard_links(error: &io::Error) -> bool {
    matches!(
        rustix::io::Errno::from_io_error(error),
        Some(rustix::io::Errno::PERM | rustix::io::Errno::OPNOTSUPP)
    )
}

/// `RENAME_NOREPLACE` is not there: an old kernel, or a file system that does not take the flag.
fn no_rename_flags(error: &io::Error) -> bool {
    matches!(
        rustix::io::Errno::from_io_error(error),
        Some(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP)
    )
}

fn write_new_with(path: &Path, bytes: &[u8], publish: &Publish) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let mut hidden = std::ffi::OsString::from(".");
    hidden.push(name);
    hidden.push(format!(".{}.part", std::process::id()));
    let part = dir.map_or_else(|| hidden.clone().into(), |dir| dir.join(&hidden));
    // a file that is there already is not ours to remove, so it is only cleaned up once this call has created it
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| match (publish.link)(&part, path) {
            Err(e) if no_hard_links(&e) => match (publish.rename_noreplace)(&part, path) {
                Err(e) if no_rename_flags(&e) => {
                    let _ = std::fs::remove_file(&part);
                    create_and_write(path, |file| file.write_all(bytes))
                }
                other => other,
            },
            other => other,
        });
    drop(file);
    let _ = std::fs::remove_file(&part);
    result
}
