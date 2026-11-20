//! Where a capture goes on disk: PNG files named after the local time, in the directory of the settings. The
//! thumbnail and the clipboard are the controller's.

use std::error::Error;
use std::path::{Path, PathBuf};

use cupertino_screenshot::filename::{self, LocalTime};
use cupertino_screenshot::image;
use cupertino_screenshot::settings::{parse_user_dirs, Settings};
use tiny_skia::Pixmap;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The local wall-clock time now (`kit::time`).
fn local_now() -> Result<LocalTime> {
    let now = cupertino_kit::time::now().ok_or_else(std::io::Error::last_os_error)?;
    Ok(LocalTime {
        year: now.year,
        month: u32::from(now.month),
        day: u32::from(now.day),
        hour: u32::from(now.hour),
        minute: u32::from(now.minute),
        second: u32::from(now.second),
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
