//! Where a capture goes on disk: PNG files named after the local time, in the directory of the settings. The
//! thumbnail and the clipboard are the controller's.

use std::error::Error;
use std::path::{Path, PathBuf};

use cupertino_screenshot::filename::{self, LocalTime};
use cupertino_screenshot::image;
use cupertino_screenshot::settings::{parse_user_dirs, Settings};
use tiny_skia::Pixmap;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The local wall-clock time now (`localtime_r`, POSIX).
fn local_now() -> Result<LocalTime> {
    // SAFETY: `time` with a null pointer only returns the time; `localtime_r` writes the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    })
}

/// The directory `settings` sends files to, created when missing.
pub fn directory(settings: &Settings) -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let config =
        cupertino_defaults::config_home().ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    let user_dirs = match std::fs::read_to_string(config.join("user-dirs.dirs")) {
        Ok(text) => parse_user_dirs(&text, &home),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let dir = settings
        .location
        .directory(&home, &user_dirs)
        .expect("the clipboard is not a directory");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Write every image as a PNG under one time-stamped name, numbered when taken.
pub fn write_files(dir: &Path, images: &[Pixmap]) -> Result<()> {
    let base = filename::base_name(&local_now()?);
    for shot in images {
        let path = filename::free_path(dir, &base, |p| p.exists());
        let png = image::png(shot)?;
        // a file that appeared since the name was chosen is not overwritten
        image::write_new(&path, &png)?;
        println!("{}", path.display());
    }
    Ok(())
}
