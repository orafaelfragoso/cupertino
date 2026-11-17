//! The copy of the session's wallpaper that the login screen shows. The login screen's account cannot read homes, so the
//! session agent leaves a JPEG of the wallpaper the session shows at `~/` + [`LOGIN_WALLPAPER_FILE`]: scaled down to at
//! most [`MAX_WALLPAPER_SIDE`] pixels a side, at most [`MAX_WALLPAPER_BYTES`], readable by the user alone. The session
//! manager reads it as the user and keeps a copy the login screen may read. It is written when the session starts and
//! again whenever the compositor's settings file changes; without a wallpaper of the user's own (the default picture) it
//! is removed, and the login screen shows its own.

use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_login_ipc::message::{LOGIN_WALLPAPER_FILE, MAX_WALLPAPER_BYTES, MAX_WALLPAPER_SIDE};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits, RgbImage};

/// Larger pictures are not decoded, per side and in memory: the compositor shows none larger either.
const MAX_SOURCE_SIDE: u32 = 8_192;
const MAX_SOURCE_ALLOC: u64 = 512 << 20;
/// The JPEG qualities tried in turn until the file fits [`MAX_WALLPAPER_BYTES`] (approximate, ours: 90 shows no
/// artefacts on a photograph at 4K).
const QUALITIES: [u8; 3] = [90, 75, 60];
/// The side of the picture written for a solid colour: the login screen scales it to the screen.
const COLOR_SIDE: u32 = 16;

/// What the session shows as its wallpaper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The user's picture file.
    Picture(PathBuf),
    /// A solid colour in place of any picture.
    Color([u8; 3]),
    /// The default picture: the login screen shows its own.
    Default,
}

/// The wallpaper `settings` make the session show: the colour when one is set, else the picture of the appearance (the
/// dark one in a dark appearance when it is set), else the default.
pub fn choose(settings: &CompositorSettings) -> Choice {
    if let Some([r, g, b, _]) = settings.wallpaper_color {
        return Choice::Color([r, g, b]);
    }
    let dark = matches!(
        settings.appearance,
        Some(AppearanceName::Dark | AppearanceName::DarkHighContrast)
    );
    let picture = if dark {
        settings
            .wallpaper_dark
            .as_ref()
            .or(settings.wallpaper.as_ref())
    } else {
        settings.wallpaper.as_ref()
    };
    match picture {
        Some(path) => Choice::Picture(PathBuf::from(path)),
        None => Choice::Default,
    }
}

/// The JPEG of `choice` for the login screen, `None` for the default: a picture is decoded (PNG or JPEG) and scaled down
/// to fit [`MAX_WALLPAPER_SIDE`], keeping its proportions; a smaller one keeps its size.
pub fn render(choice: &Choice) -> Result<Option<Vec<u8>>, String> {
    let image = match choice {
        Choice::Default => return Ok(None),
        Choice::Color(rgb) => RgbImage::from_pixel(COLOR_SIDE, COLOR_SIDE, image::Rgb(*rgb)),
        Choice::Picture(path) => fit(decode(path)?),
    };
    for quality in QUALITIES {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&image)
            .map_err(|e| format!("encoding the login screen's wallpaper: {e}"))?;
        if bytes.len() <= MAX_WALLPAPER_BYTES {
            return Ok(Some(bytes));
        }
    }
    Err("the login screen's wallpaper does not fit its size limit".into())
}

/// The picture at `path`, opened without blocking on a special file.
fn decode(path: &Path) -> Result<RgbImage, String> {
    let file = std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let mut reader = ImageReader::new(io::BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(format!(
            "{}: only PNG and JPEG pictures are taken",
            path.display()
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_SOURCE_ALLOC);
    reader.limits(limits);
    Ok(reader
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .into_rgb8())
}

/// `image` scaled down so its longer side is at most [`MAX_WALLPAPER_SIDE`].
fn fit(image: RgbImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let longer = w.max(h);
    if longer <= MAX_WALLPAPER_SIDE {
        return image;
    }
    let scale = |side: u32| {
        ((u64::from(side) * u64::from(MAX_WALLPAPER_SIDE) + u64::from(longer) / 2)
            / u64::from(longer))
        .max(1) as u32
    };
    image::imageops::resize(&image, scale(w), scale(h), FilterType::CatmullRom)
}

/// Put `bytes` at `path` whole (a new file renamed over the old one, the user's alone), or remove the file for `None`.
pub fn store(path: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    let Some(bytes) = bytes else {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    };
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let mut new = path.as_os_str().to_owned();
    new.push(".new");
    let new = PathBuf::from(new);
    let _ = std::fs::remove_file(&new);
    let written = (|| {
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&new)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&new, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    written
}

/// Write the login screen's copy for the settings in `config` to `home`.
fn update(config: &Path, home: &Path) {
    let settings = match CompositorSettings::load(config, None) {
        Ok((settings, _)) => settings,
        Err(e) => {
            eprintln!(
                "cupertino-login: the login screen's wallpaper is left as it was: {}: {e}",
                config.display()
            );
            return;
        }
    };
    let choice = choose(&settings);
    let bytes = match render(&choice) {
        Ok(bytes) => bytes,
        Err(e) => {
            // a picture the session cannot show either: the login screen shows its own
            eprintln!("cupertino-login: the login screen shows its own wallpaper: {e}");
            None
        }
    };
    if let Err(e) = store(&home.join(LOGIN_WALLPAPER_FILE), bytes.as_deref()) {
        eprintln!("cupertino-login: writing the login screen's wallpaper: {e}");
    }
}

/// Write the login screen's copy now and whenever the compositor's settings file changes, on a thread of its own for the
/// life of the process. Without a home or a settings path nothing is written.
pub fn keep_current() {
    let Some(config) = CompositorSettings::path() else {
        return;
    };
    let Some(home) = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
    else {
        return;
    };
    let (changed, changes) = std::sync::mpsc::channel::<()>();
    // the first write
    let _ = changed.send(());
    let watcher = ConfigWatcher::new(vec![config.clone()], move |event| match event {
        WatchEvent::Changed(_) => {
            let _ = changed.send(());
        }
        WatchEvent::Error(e) => {
            eprintln!(
                "cupertino-login: the login screen's wallpaper no longer follows the settings: {e}"
            )
        }
    });
    let watcher = match watcher {
        Ok(watcher) => Some(watcher),
        Err(e) => {
            eprintln!("cupertino-login: the login screen's wallpaper is written once: {e}");
            None
        }
    };
    let started = std::thread::Builder::new()
        .name("login-wallpaper".into())
        .spawn(move || {
            // the watch lives as long as this thread, which is as long as the process
            let _watcher = watcher;
            while changes.recv().is_ok() {
                // a burst of changes is one write
                while changes.try_recv().is_ok() {}
                update(&config, &home);
            }
        });
    if let Err(e) = started {
        eprintln!("cupertino-login: the login screen's wallpaper is not written: {e}");
    }
}
