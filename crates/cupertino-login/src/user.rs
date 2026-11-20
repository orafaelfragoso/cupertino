//! The user the screen is locked for or the login screen lists, and the clock text.

use cupertino_kit::account::{self, Account};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::next_resource_id;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::time;
use cupertino_login_ipc::UserRow;
use std::ffi::CStr;
use std::fmt;
use std::io;
use std::path::PathBuf;
use tiny_skia::Pixmap;

/// File in the home directory that holds the user's picture (the freedesktop `.face` convention).
const FACE_FILE: &str = ".face";

#[derive(Debug)]
pub enum UserError {
    /// The password database has no entry for the process's user.
    Unknown { uid: u32 },
    /// Reading the password database failed.
    Database(io::Error),
    /// The picture exists but cannot be read as a PNG.
    Picture {
        path: PathBuf,
        source: Box<dyn std::error::Error>,
    },
    /// A picture the session manager sent cannot be decoded as a PNG.
    SentPicture {
        login: String,
        source: Box<dyn std::error::Error>,
    },
    /// The local time cannot be read or formatted (`kit::time::format`).
    Clock(io::Error),
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { uid } => write!(f, "no user with id {uid} in the password database"),
            Self::Database(e) => write!(f, "reading the password database: {e}"),
            Self::Picture { path, source } => {
                write!(f, "reading the picture {}: {source}", path.display())
            }
            Self::SentPicture { login, source } => write!(f, "the picture of {login:?}: {source}"),
            Self::Clock(e) => write!(f, "formatting the time: {e}"),
        }
    }
}

impl std::error::Error for UserError {}

/// Who is locked in.
#[derive(Clone, Debug)]
pub struct User {
    /// The account name PAM authenticates.
    pub login: String,
    /// The full name from the account's comment field, else the account name.
    pub display: String,
    /// The picture, when the home directory has one, decoded once and named by one image id for good.
    pub picture: Option<ImageSource>,
    /// The user id, for a row of the session manager's list: an attempt names such an account by it. `None` for a name typed at
    /// "Other…" and for the user of this process.
    pub uid: Option<u32>,
    /// The account has a session: the picker marks the tile, and choosing it returns to that session.
    pub logged_in: bool,
}

impl User {
    /// The user of this process: the screen is locked for the session's own user.
    pub fn current() -> Result<User, UserError> {
        let uid = account::uid();
        let entry = account::by_uid(uid)
            .map_err(UserError::Database)?
            .ok_or(UserError::Unknown { uid })?;
        let picture = load_picture(entry.home.join(FACE_FILE))?;
        Ok(user_of(entry, picture))
    }

    /// A user typed at the picker's "Other…" tile: the name is whatever was typed and nothing is looked up, so the
    /// screen does not tell whether the account exists; the login manager decides at the attempt.
    pub fn typed(login: String) -> User {
        User {
            display: login.clone(),
            login,
            picture: None,
            uid: None,
            logged_in: false,
        }
    }

    /// A row of the session manager's user list. The manager reads the picture file; it is decoded here.
    pub fn from_row(row: &UserRow) -> Result<User, UserError> {
        let picture = match &row.picture {
            Some(picture) => Some(source_of(&Pixmap::decode_png(picture.bytes()).map_err(
                |e| UserError::SentPicture {
                    login: row.login.clone(),
                    source: Box::new(e),
                },
            )?)),
            None => None,
        };
        Ok(User {
            login: row.login.clone(),
            display: row.display_name.clone(),
            picture,
            uid: Some(row.uid),
            logged_in: row.logged_in,
        })
    }
}

/// The user of the password database entry `entry`, shown by the full name of its comment field, else by its name.
fn user_of(entry: Account, picture: Option<ImageSource>) -> User {
    let full_name = entry.full_name().trim();
    let display = if full_name.is_empty() {
        entry.login.clone()
    } else {
        full_name.to_owned()
    };
    User {
        login: entry.login,
        display,
        picture,
        uid: None,
        logged_in: false,
    }
}

/// A decoded picture as the image the screens draw: tiny-skia keeps premultiplied RGBA rows without padding.
fn source_of(pixmap: &Pixmap) -> ImageSource {
    ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width: pixmap.width(),
            height: pixmap.height(),
            stride: pixmap.width() * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: pixmap.data().into(),
        },
    }
}

/// The picture at `path`; `None` when there is no such file.
fn load_picture(path: PathBuf) -> Result<Option<ImageSource>, UserError> {
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(UserError::Picture {
            path,
            source: Box::new(e),
        }),
        Ok(_) => match Pixmap::load_png(&path) {
            Ok(picture) => Ok(Some(source_of(&picture))),
            Err(e) => Err(UserError::Picture {
                path,
                source: Box::new(e),
            }),
        },
    }
}

/// The letters shown in place of a missing picture: the first letters of the first and the last word of the name,
/// or of its only word.
pub fn initials(display: &str) -> String {
    let mut words = display.split_whitespace();
    let first = words.next();
    let last = words.next_back();
    first
        .into_iter()
        .chain(last)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// The hour cycle of the locale's time format (`LC_TIME`): 12-hour when it names a 12-hour hour (`%I`, `%l` or `%r`),
/// else 24-hour.
pub fn locale_hour_cycle() -> cupertino_defaults::menubar::HourCycle {
    hour_cycle_of(&time::locale_time_format())
}

/// The hour cycle of the `strftime` time format `format`.
pub fn hour_cycle_of(format: &str) -> cupertino_defaults::menubar::HourCycle {
    use cupertino_defaults::menubar::HourCycle;
    if ["%I", "%l", "%r"].iter().any(|h| format.contains(h)) {
        HourCycle::H12
    } else {
        HourCycle::H24
    }
}

/// Whether the screen's language reads right to left, by the language of `LC_ALL`, `LC_MESSAGES` or `LANG`.
pub fn locale_is_rtl() -> bool {
    let language = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    language_is_rtl(&language)
}

/// Whether the locale name `locale` (`ar_EG.UTF-8`) names a language written right to left.
pub fn language_is_rtl(locale: &str) -> bool {
    const RTL: [&str; 8] = ["ar", "he", "fa", "ur", "yi", "ps", "sd", "ug"];
    let language = locale.split(['_', '.', '@']).next().unwrap_or("");
    RTL.contains(&language)
}

/// Local time `unix_seconds` as text by the `strftime` pattern `format`.
pub fn format_time(unix_seconds: i64, format: &CStr) -> Result<String, UserError> {
    time::format(time::from_unix(unix_seconds), format).map_err(UserError::Clock)
}
