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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_pixels_become_rgba_and_padding_is_skipped() {
        // two pixels a row, stride 12: blue then translucent red, then 4 bytes of padding
        let data = [
            255, 0, 0, 7, 0, 0, 128, 128, 9, 9, 9, 9, 0, 255, 0, 255, 0, 0, 0, 0, 9, 9, 9, 9,
        ];
        let xrgb = from_shm(2, 2, 12, &data, Format::Xrgb8888).expect("a 2 x 2 image");
        assert_eq!(&xrgb.data()[..8], &[0, 0, 255, 255, 128, 0, 0, 255]);
        let argb = from_shm(2, 2, 12, &data, Format::Argb8888).expect("a 2 x 2 image");
        // the blue channel above the alpha of 7 is limited to it
        assert_eq!(&argb.data()[..8], &[0, 0, 7, 7, 128, 0, 0, 128]);
        assert_eq!(&argb.data()[8..12], &[0, 255, 0, 255]);
        assert!(
            from_shm(2, 2, 12, &data[..19], Format::Argb8888).is_none(),
            "a short buffer is refused"
        );
    }

    /// A `w` x `h` image whose pixel `(x, y)` has red `x` and green `y`.
    fn ramp(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                p.data_mut()[i..i + 4].copy_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        p
    }

    fn red_green(p: &Pixmap, x: u32, y: u32) -> (u8, u8) {
        let i = ((y * p.width() + x) * 4) as usize;
        (p.data()[i], p.data()[i + 1])
    }

    #[test]
    fn a_selection_maps_to_the_pixels_of_its_output_at_the_buffer_scale() {
        let output = Rect::new(1280.0, 0.0, 144.0, 90.0);
        let sel = Rect::new(1290.0, 5.0, 20.0, 15.0);
        let one = area(&ramp(144, 90), &sel, &output, OutputTransform::Normal).unwrap();
        assert_eq!((one.width(), one.height()), (20, 15));
        assert_eq!(red_green(&one, 0, 0), (10, 5));
        // the same selection on a second output at twice the scale: twice the pixels
        let two = area(&ramp(288, 180), &sel, &output, OutputTransform::Normal).unwrap();
        assert_eq!((two.width(), two.height()), (40, 30));
        assert_eq!(red_green(&two, 0, 0), (20, 10));
        assert!(area(
            &ramp(144, 90),
            &Rect::new(0.0, 0.0, 9.0, 9.0),
            &output,
            OutputTransform::Normal
        )
        .is_none());
    }

    #[test]
    fn a_selection_of_a_turned_buffer_comes_out_upright_for_every_transform() {
        let output = Rect::new(0.0, 0.0, 40.0, 30.0);
        let sel = Rect::new(3.0, 4.0, 10.0, 6.0);
        let picture = ramp(40, 30);
        let expected = area(&picture, &sel, &output, OutputTransform::Normal).unwrap();
        for t in OutputTransform::ALL {
            // the buffer the server would send turned by `t`: the picture turned by the inverse of turning it upright
            let buffer = upright(&picture, t.inverse());
            assert_eq!(
                (buffer.width(), buffer.height()),
                t.turn_pixels((40, 30)),
                "{t:?}"
            );
            assert_eq!(upright(&buffer, t).data(), picture.data(), "{t:?}");
            let cut = area(&buffer, &sel, &output, t).unwrap();
            assert_eq!(cut.data(), expected.data(), "{t:?}");
        }
    }

    #[test]
    fn downscaling_averages_the_covered_pixels() {
        let mut image = Pixmap::new(4, 2).expect("a pixmap");
        image.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        image.data_mut()[..8].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
        let small = downscale(&image, 2, 1).expect("a pixmap");
        assert_eq!(&small.data()[..4], &[128, 128, 128, 255]);
        assert_eq!(&small.data()[4..8], &[0, 0, 0, 255]);
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cupertino-screenshot-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_file_holds_its_bytes_and_leaves_nothing_else_behind() {
        let dir = temp_dir("write-new");
        let path = dir.join("a.png");
        write_new(&path, b"bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"bytes");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("a.png")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_already_there_is_not_replaced() {
        let dir = temp_dir("write-new-taken");
        let path = dir.join("a.png");
        std::fs::write(&path, b"first").unwrap();
        let error = write_new(&path, b"second").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_reader_that_finds_the_file_finds_all_of_it() {
        let dir = temp_dir("write-new-reader");
        let path = dir.join("a.png");
        let bytes = vec![7u8; 64 << 20];
        let reader = {
            let (path, len) = (path.clone(), bytes.len());
            std::thread::spawn(move || loop {
                if let Ok(found) = std::fs::metadata(&path) {
                    return found.len() == len as u64;
                }
            })
        };
        write_new(&path, &bytes).unwrap();
        assert!(reader.join().unwrap(), "the reader saw a partial file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use std::cell::RefCell;

    thread_local! {
        /// The calls the stand-in file system saw, in order.
        static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn calls() -> Vec<&'static str> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }

    fn failing(errno: rustix::io::Errno) -> io::Error {
        errno.into()
    }

    /// A file system whose `link` fails with `link` and whose `rename_noreplace` does what `rename` says.
    fn publish(
        link: fn(&Path, &Path) -> io::Result<()>,
        rename_noreplace: fn(&Path, &Path) -> io::Result<()>,
    ) -> Publish {
        Publish {
            link,
            rename_noreplace,
        }
    }

    fn no_links(_: &Path, _: &Path) -> io::Result<()> {
        CALLS.with(|c| c.borrow_mut().push("link"));
        Err(failing(rustix::io::Errno::PERM))
    }

    fn real_noreplace(part: &Path, path: &Path) -> io::Result<()> {
        CALLS.with(|c| c.borrow_mut().push("rename"));
        (Publish::SYSTEM.rename_noreplace)(part, path)
    }

    fn no_flags(_: &Path, _: &Path) -> io::Result<()> {
        CALLS.with(|c| c.borrow_mut().push("rename"));
        Err(failing(rustix::io::Errno::INVAL))
    }

    #[test]
    fn without_hard_links_the_hidden_file_is_renamed_without_replacing() {
        let dir = temp_dir("write-new-rename");
        let path = dir.join("a.png");
        calls();
        write_new_with(&path, b"bytes", &publish(no_links, real_noreplace)).unwrap();
        assert_eq!(calls(), ["link", "rename"]);
        assert_eq!(std::fs::read(&path).unwrap(), b"bytes");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::write(dir.join("b.png"), b"first").unwrap();
        let error = write_new_with(
            &dir.join("b.png"),
            b"second",
            &publish(no_links, real_noreplace),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(dir.join("b.png")).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_atomic_rename_decides_not_a_look_at_the_name_beforehand() {
        let dir = temp_dir("write-new-decides");
        let path = dir.join("a.png");
        std::fs::write(&path, b"first").unwrap();
        // a rename that finds the name free (the file went away meanwhile) is the answer, not the earlier look
        fn takes_it(part: &Path, path: &Path) -> io::Result<()> {
            std::fs::remove_file(path)?;
            std::fs::rename(part, path)
        }
        write_new_with(&path, b"second", &publish(no_links, takes_it)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_rename_flags_the_bytes_are_written_directly_and_never_replace() {
        let dir = temp_dir("write-new-direct");
        let path = dir.join("a.png");
        calls();
        write_new_with(&path, b"bytes", &publish(no_links, no_flags)).unwrap();
        assert_eq!(calls(), ["link", "rename"]);
        assert_eq!(std::fs::read(&path).unwrap(), b"bytes");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let error = write_new_with(&path, b"second", &publish(no_links, no_flags)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn any_other_link_error_is_returned_without_a_fallback() {
        fn full(_: &Path, _: &Path) -> io::Result<()> {
            CALLS.with(|c| c.borrow_mut().push("link"));
            Err(failing(rustix::io::Errno::NOSPC))
        }
        let dir = temp_dir("write-new-other");
        let path = dir.join("a.png");
        calls();
        let error = write_new_with(&path, b"bytes", &publish(full, real_noreplace)).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::NOSPC.raw_os_error())
        );
        assert_eq!(calls(), ["link"]);
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hidden_file_that_is_not_ours_is_left_alone() {
        let dir = temp_dir("write-new-stale");
        let path = dir.join("a.png");
        let stale = dir.join(format!(".a.png.{}.part", std::process::id()));
        std::fs::write(&stale, b"left by a crashed run").unwrap();
        let error = write_new(&path, b"bytes").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&stale).unwrap(), b"left by a crashed run");
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_direct_write_that_fails_leaves_no_partial_file() {
        let dir = temp_dir("write-new-partial");
        let path = dir.join("a.png");
        let error = create_and_write(&path, |file| {
            file.write_all(b"half")?;
            Err(io::Error::from(io::ErrorKind::StorageFull))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert!(!path.exists());
        // a file that was there is not this call's to remove
        std::fs::write(&path, b"first").unwrap();
        let error = create_and_write(&path, |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
