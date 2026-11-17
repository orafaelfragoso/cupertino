//! The user the screen is locked for or the login screen lists, and the clock text.

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::next_resource_id;
use cupertino_kit::render::{ImageResource, PixelFormat};
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
    /// `localtime_r` or `strftime` failed.
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
        // Safety: `getuid` has no preconditions.
        let uid = unsafe { libc::getuid() };
        let entry = read_entry(|passwd, buffer, found| unsafe {
            libc::getpwuid_r(uid, passwd, buffer.as_mut_ptr().cast(), buffer.len(), found)
        })?;
        let entry = entry.ok_or(UserError::Unknown { uid })?;
        let picture = load_picture(PathBuf::from(&entry.home).join(FACE_FILE))?;
        Ok(entry.into_user(picture))
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

/// The fields of a password database entry that the screen needs.
struct Entry {
    login: String,
    gecos: String,
    home: String,
}

impl Entry {
    fn into_user(self, picture: Option<ImageSource>) -> User {
        let full_name = self.gecos.split(',').next().unwrap_or("").trim();
        let display = if full_name.is_empty() {
            self.login.clone()
        } else {
            full_name.to_owned()
        };
        User {
            login: self.login,
            display,
            picture,
            uid: None,
            logged_in: false,
        }
    }
}

/// Run a `getpw*_r` call with a buffer that grows until the entry fits; `None` when there is no such entry. `call` gets
/// the `passwd` to fill, the buffer and the result pointer.
fn read_entry(
    call: impl Fn(&mut libc::passwd, &mut [u8], &mut *mut libc::passwd) -> libc::c_int,
) -> Result<Option<Entry>, UserError> {
    let mut buffer = vec![0u8; 1024];
    // Safety: all-zero is a valid `passwd` (null pointers and zero ids), and `call` overwrites it.
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    loop {
        match call(&mut passwd, &mut buffer, &mut found) {
            0 => break,
            libc::ERANGE => buffer.resize(buffer.len() * 2, 0),
            errno => return Err(UserError::Database(io::Error::from_raw_os_error(errno))),
        }
    }
    if found.is_null() {
        return Ok(None);
    }
    // Safety: on success the fields point at nul-terminated strings inside `buffer`.
    Ok(Some(unsafe {
        Entry {
            login: field(passwd.pw_name),
            gecos: field(passwd.pw_gecos),
            home: field(passwd.pw_dir),
        }
    }))
}

/// # Safety
/// `ptr` must point at a nul-terminated string.
unsafe fn field(ptr: *const libc::c_char) -> String {
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
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
    // Safety: `nl_langinfo` returns a pointer to a nul-terminated string that stays valid until the locale changes.
    let format = unsafe { CStr::from_ptr(libc::nl_langinfo(libc::T_FMT)) };
    hour_cycle_of(&format.to_string_lossy())
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
    let t = unix_seconds as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // Safety: both pointers are valid for the duration of the call.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return Err(UserError::Clock(io::Error::last_os_error()));
    }
    let mut buf = [0u8; 128];
    // Safety: the buffer length is passed and `format` is nul terminated.
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm) };
    if n == 0 {
        return Err(UserError::Clock(io::Error::new(
            io::ErrorKind::InvalidData,
            "strftime produced no text",
        )));
    }
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hour_cycle_follows_the_locales_time_format() {
        use cupertino_defaults::menubar::HourCycle;
        assert_eq!(hour_cycle_of("%H:%M:%S"), HourCycle::H24);
        assert_eq!(hour_cycle_of("%I:%M:%S %p"), HourCycle::H12);
        assert_eq!(hour_cycle_of("%r"), HourCycle::H12);
        assert_eq!(hour_cycle_of("%l:%M"), HourCycle::H12);
        // 9:05 in the evening in each cycle, as the large time shows it
        let tm_text = |format: &CStr| {
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            tm.tm_hour = 21;
            tm.tm_min = 5;
            let mut buf = [0u8; 32];
            // Safety: the buffer length is passed and `format` is nul terminated.
            let n =
                unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm) };
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };
        assert_eq!(tm_text(c"%H:%M"), "21:05");
        assert_eq!(tm_text(c"%-I:%M"), "9:05");
    }

    #[test]
    fn right_to_left_languages_are_known_by_their_code() {
        assert!(language_is_rtl("ar_EG.UTF-8"));
        assert!(language_is_rtl("he_IL"));
        assert!(language_is_rtl("fa"));
        assert!(!language_is_rtl("en_US.UTF-8"));
        assert!(!language_is_rtl("C"));
        assert!(!language_is_rtl(""));
    }

    #[test]
    fn initials_take_the_first_and_last_word() {
        assert_eq!(initials("Rafael Fragoso Moura"), "RM");
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("rafael"), "R");
        assert_eq!(initials("  élan  vital "), "ÉV");
    }

    #[test]
    fn a_typed_user_is_the_text_as_typed() {
        let user = User::typed("carol".into());
        assert_eq!(
            (
                user.login.as_str(),
                user.display.as_str(),
                user.picture.is_none()
            ),
            ("carol", "carol", true)
        );
    }

    #[test]
    fn the_time_follows_the_pattern() {
        // 1_700_000_000 is 22:13:20 UTC; no zone offset changes the seconds
        let seconds = format_time(1_700_000_000, c"%S").unwrap();
        assert_eq!(seconds, "20");
    }

    #[test]
    fn a_row_without_a_picture_has_initials_only_and_one_that_is_not_a_png_is_an_error() {
        use cupertino_login_ipc::Picture;
        let mut row = UserRow {
            uid: 1000,
            login: "alice".into(),
            display_name: "Alice Liddell".into(),
            picture: None,
            logged_in: true,
            wallpaper: None,
        };
        let user = User::from_row(&row).unwrap();
        assert_eq!(
            (
                user.login.as_str(),
                user.display.as_str(),
                user.picture.is_none(),
                user.logged_in
            ),
            ("alice", "Alice Liddell", true, true)
        );
        row.picture = Some(Picture::new(b"not a png".to_vec()).unwrap());
        assert!(
            matches!(User::from_row(&row), Err(UserError::SentPicture { login, .. }) if login == "alice")
        );
    }
}
