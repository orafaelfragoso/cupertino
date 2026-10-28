//! Captured pixels: from the shm buffer the server filled to an image, the selection cut out of an output's image, and
//! the PNG written to disk or offered on the clipboard.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use cupertino_geometry::Rect;
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

/// The pixels of `area` (global points) in the image of an output at `output` (global points) that is `size` pixels:
/// the area relative to the output times the image's scale, rounded to whole pixels.
pub fn area_pixels(area: &Rect, output: &Rect, size: (u32, u32)) -> Option<IntRect> {
    let (sx, sy) = (
        f64::from(size.0) / output.width(),
        f64::from(size.1) / output.height(),
    );
    let x0 = ((area.min_x() - output.min_x()) * sx).round() as i32;
    let y0 = ((area.min_y() - output.min_y()) * sy).round() as i32;
    let x1 = ((area.max_x() - output.min_x()) * sx).round() as i32;
    let y1 = ((area.max_y() - output.min_y()) * sy).round() as i32;
    let r = IntRect::from_ltrb(
        x0.max(0),
        y0.max(0),
        x1.min(size.0 as i32),
        y1.min(size.1 as i32),
    )?;
    Some(r)
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

    #[test]
    fn a_selection_maps_to_the_pixels_of_its_output_at_the_buffer_scale() {
        let output = Rect::new(1280.0, 0.0, 1440.0, 900.0);
        let area = Rect::new(1380.0, 50.0, 200.0, 150.0);
        assert_eq!(
            area_pixels(&area, &output, (1440, 900)),
            IntRect::from_xywh(100, 50, 200, 150)
        );
        assert_eq!(
            area_pixels(&area, &output, (2880, 1800)),
            IntRect::from_xywh(200, 100, 400, 300)
        );
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
