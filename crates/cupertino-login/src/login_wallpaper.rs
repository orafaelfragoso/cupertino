//! The copy of the session's wallpaper that the login screen shows. The login screen's account cannot read homes, so the
//! session agent leaves a JPEG of the wallpaper the session shows at `~/` + [`LOGIN_WALLPAPER_FILE`]: scaled down to at
//! most [`MAX_WALLPAPER_SIDE`] pixels a side, at most [`MAX_WALLPAPER_BYTES`], readable by the user alone. The session
//! manager reads it as the user and keeps a copy the login screen may read. It is written when the session starts and
//! again whenever the compositor's settings file changes ([`write_current`]); without a wallpaper of the user's own (the default picture) it
//! is removed, and the login screen shows its own. Everything here reads or writes files: it runs on Kit's I/O pool
//! (`cupertino_kit::task::io`), never on the UI thread.

use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
use cupertino_kit::image::{self as picture, ImageError};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_login_ipc::message::{LOGIN_WALLPAPER_FILE, MAX_WALLPAPER_BYTES, MAX_WALLPAPER_SIDE};

/// Larger pictures are not decoded, per side: the compositor shows none larger either.
const MAX_SOURCE_SIDE: u32 = 8_192;
/// The largest picture file read: the wallpaper's own limit.
const MAX_SOURCE_BYTES: u64 = crate::wallpaper::MAX_FILE_BYTES;
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
/// to fit [`MAX_WALLPAPER_SIDE`], keeping its proportions; a smaller one keeps its size. A picture is read from its file:
/// run it on the I/O pool.
pub fn render(choice: &Choice) -> Result<Option<Vec<u8>>, String> {
    let image = match choice {
        Choice::Default => return Ok(None),
        Choice::Color([r, g, b]) => ImageResource {
            revision: 1,
            width: COLOR_SIDE,
            height: COLOR_SIDE,
            stride: COLOR_SIDE * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: [*r, *g, *b, 255]
                .repeat((COLOR_SIDE * COLOR_SIDE) as usize)
                .into(),
        },
        Choice::Picture(path) => {
            fit(decode(path)?).map_err(|e| format!("scaling the login screen's wallpaper: {e}"))?
        }
    };
    for quality in QUALITIES {
        let bytes = picture::encode_jpeg(&image, quality)
            .map_err(|e| format!("encoding the login screen's wallpaper: {e}"))?;
        if bytes.len() <= MAX_WALLPAPER_BYTES {
            return Ok(Some(bytes));
        }
    }
    Err("the login screen's wallpaper does not fit its size limit".into())
}

/// The picture at `path`, opened without blocking on a special file.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn decode(path: &Path) -> Result<ImageResource, String> {
    let file = std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let mut data = Vec::with_capacity(meta.len().min(MAX_SOURCE_BYTES) as usize);
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!("{}: the picture is too large", path.display()));
    }
    picture::decode(&data, MAX_SOURCE_SIDE).map_err(|e| match e {
        ImageError::Unsupported => {
            format!("{}: only PNG and JPEG pictures are taken", path.display())
        }
        e => format!("{}: {e}", path.display()),
    })
}

/// `image` scaled down so its longer side is at most [`MAX_WALLPAPER_SIDE`].
fn fit(image: ImageResource) -> Result<ImageResource, ImageError> {
    let (w, h) = (image.width, image.height);
    let longer = w.max(h);
    if longer <= MAX_WALLPAPER_SIDE {
        return Ok(image);
    }
    let scale = |side: u32| {
        ((u64::from(side) * u64::from(MAX_WALLPAPER_SIDE) + u64::from(longer) / 2)
            / u64::from(longer))
        .max(1) as u32
    };
    picture::resize(&image, scale(w), scale(h))
}

/// Put `bytes` at `path` whole (a new file renamed over the old one, the user's alone), or remove the file for `None`.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
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

/// Write the login screen's copy once, for the compositor's settings as they are now, to the home of `HOME`: the
/// program runs it at start and after each change of the settings file it follows. Without a home or a settings path
/// nothing is written. It reads and writes files: run it on Kit's I/O pool (`cupertino_kit::task::io`), never on the UI
/// thread.
pub fn write_current() {
    let Some(config) = CompositorSettings::path() else {
        return;
    };
    let Some(home) = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
    else {
        return;
    };
    update(&config, &home);
}
