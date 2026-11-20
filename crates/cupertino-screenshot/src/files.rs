//! The tool's file work: the settings file read and written, and the captures written as PNG files in the directory
//! the settings name. Each function is work for the I/O pool (`kit::task::io`), never the UI thread.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cupertino_kit::render::ImageResource;

use crate::filename::{self, LocalTime};
use crate::settings::{Location, Problem, Settings};

/// How long reading or writing the settings file should take: a few lines on a local disk.
pub const SETTINGS_DEADLINE: Duration = Duration::from_secs(2);
/// How long writing the captures should take: a PNG of every output of a large screen encoded and written.
pub const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// The settings at `path`: the defaults when there is no file, and the lines that could not be used.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn read_settings(path: &Path) -> io::Result<(Settings, Vec<Problem>)> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Settings::parse(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Settings::default(), Vec::new())),
        Err(e) => Err(e),
    }
}

/// Write `settings` to `path` whole (`cupertino_defaults::atomic`), creating its directory.
pub fn store_settings(path: &Path, settings: &Settings) -> io::Result<()> {
    cupertino_defaults::atomic::write_atomic(path, settings.to_text().as_bytes())
}

/// The local wall-clock time now (`kit::time`).
fn local_now() -> io::Result<LocalTime> {
    let now = cupertino_kit::time::now().ok_or_else(io::Error::last_os_error)?;
    Ok(LocalTime {
        year: now.year,
        month: u32::from(now.month),
        day: u32::from(now.day),
        hour: u32::from(now.hour),
        minute: u32::from(now.minute),
        second: u32::from(now.second),
    })
}

/// The directory `location` sends files to, from `$HOME` and the session's `user-dirs.dirs`
/// (`cupertino_defaults::user_dirs`), created when missing.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn directory(location: &Location) -> Result<PathBuf, String> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let user_dirs = cupertino_defaults::user_dirs::read(&home)
        .map_err(|e| format!("reading the user directories: {e}"))?;
    let dir = location
        .directory(&home, &user_dirs)
        .ok_or("the clipboard is not a directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Write every image as a PNG in the directory of `location`, under one time-stamped name, numbered when taken; the
/// paths written, in order.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn write_captures(
    location: &Location,
    images: &[ImageResource],
) -> Result<Vec<PathBuf>, String> {
    let dir = directory(location)?;
    let base = filename::base_name(&local_now().map_err(|e| format!("the local time: {e}"))?);
    let mut written = Vec::new();
    for image in images {
        let path = filename::free_path(&dir, &base, |p| p.exists());
        let png = cupertino_kit::image::encode_png(image).map_err(|e| e.to_string())?;
        // a file that appeared since the name was chosen is not overwritten
        write_new(&path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
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
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn create_and_write(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = write(&mut file);
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

/// How the hidden file reaches its name.
struct Publish {
    link: fn(&Path, &Path) -> io::Result<()>,
    rename_noreplace: fn(&Path, &Path) -> io::Result<()>,
}

impl Publish {
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
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

#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
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
