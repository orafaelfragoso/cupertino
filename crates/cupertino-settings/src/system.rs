//! What the General and Account panes read of the machine and the account: the signed-in person, the mounted
//! disks, the time zone and language, the programs that start at login and the default web browser. Reads are plain
//! files and the C library; writes are the user's own files (`~/.config/autostart`, `~/.config/mimeapps.list`).
//! Nothing here touches a window toolkit.

use std::io;
use std::path::{Path, PathBuf};

/// The most bytes read of one small text file.
const MAX_TEXT: u64 = 1024 * 1024;

fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_TEXT)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
}

/// The signed-in person.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    /// The full name, else the user name.
    pub full_name: String,
    /// Up to two capital letters that stand for the person when there is no picture.
    pub initials: String,
    pub picture: Option<PathBuf>,
    pub home: PathBuf,
    /// Whether the person may administer the machine (a member of the `wheel` or `sudo` group).
    pub administrator: bool,
}

/// The initials of `name`: the first letters of its first and last word, upper-case.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let letter = |w: &str| w.chars().find(|c| c.is_alphanumeric());
    let mut out = String::new();
    if let Some(first) = words.first().and_then(|w| letter(w)) {
        out.extend(first.to_uppercase());
    }
    if words.len() > 1 {
        if let Some(last) = words.last().and_then(|w| letter(w)) {
            out.extend(last.to_uppercase());
        }
    }
    out
}

/// The fields of the `/etc/passwd` line of `uid`: user name, comment, home.
fn passwd_entry(text: &str, uid: u32) -> Option<(String, String, PathBuf)> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 6 && f[2].parse::<u32>() == Ok(uid)).then(|| {
            let comment = f[4].split(',').next().unwrap_or("").trim().to_owned();
            (f[0].to_owned(), comment, PathBuf::from(f[5]))
        })
    })
}

/// Whether the group file lists `user` in one of the administrator groups.
fn in_admin_group(groups: &str, user: &str) -> bool {
    groups.lines().any(|line| {
        let f: Vec<&str> = line.split(':').collect();
        f.len() >= 4 && matches!(f[0], "wheel" | "sudo") && f[3].split(',').any(|m| m == user)
    })
}

impl Account {
    /// The account of this process's user.
    pub fn read() -> Account {
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        let passwd = read_small(Path::new("/etc/passwd")).unwrap_or_default();
        let (username, comment, home_dir) = passwd_entry(&passwd, uid).unwrap_or_else(|| {
            (
                std::env::var("USER").unwrap_or_default(),
                String::new(),
                home().unwrap_or_default(),
            )
        });
        let full_name = if comment.is_empty() {
            username.clone()
        } else {
            comment
        };
        let picture = [
            home_dir.join(".face"),
            home_dir.join(".face.icon"),
            PathBuf::from("/var/lib/AccountsService/icons").join(&username),
        ]
        .into_iter()
        .find(|p| p.is_file());
        let administrator = read_small(Path::new("/etc/group"))
            .is_some_and(|g| in_admin_group(&g, &username))
            || uid == 0;
        Account {
            initials: initials(&full_name),
            username,
            full_name,
            picture,
            home: home_dir,
            administrator,
        }
    }
}

/// A mounted disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub target: PathBuf,
    pub device: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
}

/// The mounted disks that have a device of their own, each once, the one that holds `/` first.
pub fn mounts() -> Vec<Mount> {
    let Some(text) = read_small(Path::new("/proc/self/mounts")) else {
        return Vec::new();
    };
    let mut out: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 || !f[0].starts_with("/dev/") || f[0].starts_with("/dev/loop") {
            continue;
        }
        if out.iter().any(|m| m.device == f[0]) {
            continue;
        }
        let target = PathBuf::from(unescape_mount(f[1]));
        let Some((total, free)) = space(&target) else {
            continue;
        };
        out.push(Mount {
            target,
            device: f[0].to_owned(),
            fs: f[2].to_owned(),
            total,
            free,
        });
    }
    out.sort_by_key(|m| m.target != Path::new("/"));
    out
}

/// A mount point as `/proc` writes it: a space, tab, newline or backslash is an octal escape.
fn unescape_mount(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let digits: String = chars.clone().take(3).collect();
            if digits.len() == 3 && digits.chars().all(|d| ('0'..='7').contains(&d)) {
                if let Some(v) = u32::from_str_radix(&digits, 8)
                    .ok()
                    .and_then(char::from_u32)
                {
                    out.push(v);
                    chars.nth(2);
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// The size and the free space (for a normal person) of the filesystem at `path`, in bytes.
fn space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid C string and `st` is a valid out-pointer for one statvfs.
    let rc = unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs succeeded, so it filled the structure.
    let st = unsafe { st.assume_init() };
    let unit = st.f_frsize as u64;
    Some((st.f_blocks as u64 * unit, st.f_bavail as u64 * unit))
}

/// A size as people read it: decimal units, one decimal below ten.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// The local time of day, in minutes after midnight.
pub fn local_minutes() -> u32 {
    // SAFETY: `time` accepts a null pointer; `localtime_r` fills the structure we pass it.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
        if libc::localtime_r(&now, tm.as_mut_ptr()).is_null() {
            return 0;
        }
        let tm = tm.assume_init();
        (tm.tm_hour.clamp(0, 23) * 60 + tm.tm_min.clamp(0, 59)) as u32
    }
}

/// The computer's host name.
pub fn hostname() -> Option<String> {
    read_small(Path::new("/etc/hostname"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The time zone the system uses, such as `Europe/Lisbon`: the target of `/etc/localtime`, else `/etc/timezone`.
pub fn timezone() -> Option<String> {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let text = target.to_string_lossy();
        if let Some(i) = text.find("zoneinfo/") {
            return Some(text[i + "zoneinfo/".len()..].to_owned());
        }
    }
    read_small(Path::new("/etc/timezone"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The language and region of this session: `LC_ALL`, `LC_MESSAGES`, `LANG`, else `/etc/locale.conf`'s `LANG`.
pub fn locale() -> Option<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(v) = std::env::var(var).ok().filter(|v| !v.is_empty()) {
            return Some(v);
        }
    }
    read_small(Path::new("/etc/locale.conf")).and_then(|text| {
        text.lines()
            .find_map(|l| l.strip_prefix("LANG="))
            .map(|v| v.trim().trim_matches('"').to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// A program that starts when the person signs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginItem {
    /// The file name of its `.desktop` file.
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

fn autostart_dirs() -> (Option<PathBuf>, Vec<PathBuf>) {
    let user = config_home().map(|c| c.join("autostart"));
    let system = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/etc/xdg".to_owned())
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("autostart"))
        .collect();
    (user, system)
}

/// The value of `key` in the `[Desktop Entry]` group of `text`.
fn entry_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
        } else if inside {
            if let Some(rest) = line.strip_prefix(key) {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    return Some(value.trim());
                }
            }
        }
    }
    None
}

/// `text` with `key` set to `value` in its `[Desktop Entry]` group: the line changed in place, or added at the end of
/// the group.
fn with_entry_value(text: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    let mut done = false;
    let mut insert_at: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if inside && !done {
                insert_at = Some(out.len());
            }
            inside = trimmed == "[Desktop Entry]";
        }
        if inside && !done {
            if let Some(rest) = trimmed.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    out.push(format!("{key}={value}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_owned());
    }
    if !done {
        let at = insert_at.unwrap_or(out.len());
        out.insert(at, format!("{key}={value}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn is_enabled(text: &str) -> bool {
    let off = |key| entry_value(text, key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    !off("Hidden")
        && entry_value(text, "X-GNOME-Autostart-enabled")
            .map_or(true, |v| !v.eq_ignore_ascii_case("false"))
}

fn desktop_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".desktop").then(|| (name, e.path()))
        })
        .collect();
    files.sort();
    files
}

/// The programs that start at login, by name: those of the user's `autostart` folder and the system's, a user file
/// of the same name taking the place of the system's.
pub fn login_items() -> Vec<LoginItem> {
    let (user, system) = autostart_dirs();
    let mut seen: Vec<String> = Vec::new();
    let mut items = Vec::new();
    for dir in user.iter().chain(system.iter()) {
        for (id, path) in desktop_files(dir) {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let Some(text) = read_small(&path) else {
                continue;
            };
            if entry_value(&text, "Type").is_some_and(|t| t != "Application") {
                continue;
            }
            let name = entry_value(&text, "Name")
                .map(str::to_owned)
                .unwrap_or_else(|| id.trim_end_matches(".desktop").to_owned());
            items.push(LoginItem {
                enabled: is_enabled(&text),
                id,
                name,
            });
        }
    }
    items.sort_by(|a, b| crate::sidebar::standard_compare(&a.name, &b.name));
    items
}

/// Turn the login item `id` on or off: its file in the user's `autostart` folder is changed or, for a system one, a
/// copy of it with `Hidden=true` is put there; turning a copy on again removes it.
pub fn set_login_item(id: &str, enable: bool) -> io::Result<()> {
    if id.contains('/') || !id.ends_with(".desktop") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a desktop file",
        ));
    }
    let (user, system) = autostart_dirs();
    let user = user.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config folder"))?;
    let own = user.join(id);
    let system_file = system.iter().map(|d| d.join(id)).find(|p| p.is_file());
    let text = match (read_small(&own), &system_file) {
        (Some(t), _) => t,
        (None, Some(path)) => read_small(path)
            .ok_or_else(|| io::Error::other(format!("{}: unreadable", path.display())))?,
        (None, None) => return Err(io::Error::new(io::ErrorKind::NotFound, id.to_owned())),
    };
    if enable && system_file.is_some() && own.is_file() {
        // the copy only hid the system's entry
        let restored = with_entry_value(
            &with_entry_value(&text, "Hidden", "false"),
            "X-GNOME-Autostart-enabled",
            "true",
        );
        let same_as_system = system_file
            .as_ref()
            .and_then(|p| read_small(p))
            .is_some_and(|s| is_enabled(&s));
        if same_as_system {
            return std::fs::remove_file(&own);
        }
        return write_atomic(&own, &restored);
    }
    let value = if enable { "false" } else { "true" };
    let mut changed = with_entry_value(&text, "Hidden", value);
    if enable {
        changed = with_entry_value(&changed, "X-GNOME-Autostart-enabled", "true");
    }
    std::fs::create_dir_all(&user)?;
    write_atomic(&own, &changed)
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// An installed web browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    /// Its desktop file id.
    pub id: String,
    pub name: String,
}

/// The web types a browser is the default for.
const BROWSER_TYPES: [&str; 4] = [
    "x-scheme-handler/http",
    "x-scheme-handler/https",
    "text/html",
    "application/xhtml+xml",
];

/// The installed browsers (the applications that handle web links) and the default one's index.
pub fn browsers() -> (Vec<Browser>, Option<usize>) {
    use cupertino_apps::entry::{Locale, Visibility};
    use cupertino_apps::mime::MimeDatabase;
    use cupertino_apps::mimeapps::{MimeApps, XdgDirs};
    let apps = XdgDirs::from_env().ok().and_then(|dirs| {
        let db = MimeDatabase::load(&dirs.all_data_dirs()).ok()?;
        MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env()).ok()
    });
    let Some(apps) = apps else {
        return (Vec::new(), None);
    };
    let mut list: Vec<Browser> = Vec::new();
    let mut default = None;
    for item in apps.open_with("x-scheme-handler/https") {
        if item.is_default {
            default = Some(list.len());
        }
        list.push(Browser {
            id: item.entry.id.clone(),
            name: item.entry.name.clone(),
        });
    }
    (list, default)
}

/// The text of a `mimeapps.list` with the default for each of the web types set to `id`.
fn with_default_browser(text: &str, id: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let section = lines
        .iter()
        .position(|l| l.trim() == "[Default Applications]");
    let start = match section {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Default Applications]".to_owned());
            lines.len() - 1
        }
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with('['))
        .map_or(lines.len(), |p| start + 1 + p);
    let mut keep: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let inside = i > start && i < end;
        let is_web = inside
            && BROWSER_TYPES.iter().any(|t| {
                line.trim()
                    .strip_prefix(t)
                    .is_some_and(|r| r.trim_start().starts_with('='))
            });
        if !is_web {
            keep.push(line.clone());
        }
        if i == start {
            for t in BROWSER_TYPES {
                keep.push(format!("{t}={id}"));
            }
        }
    }
    let mut out = keep.join("\n");
    out.push('\n');
    out
}

/// Make the browser with desktop file `id` the default for web links and pages, in the user's `mimeapps.list`.
pub fn set_default_browser(id: &str) -> io::Result<()> {
    if id.contains(['/', '\n', ';']) || !id.ends_with(".desktop") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a desktop file id",
        ));
    }
    let path = config_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config folder"))?
        .join("mimeapps.list");
    let text = read_small(&path).unwrap_or_default();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("list.tmp");
    std::fs::write(&tmp, with_default_browser(&text, id))?;
    std::fs::rename(&tmp, &path)
}

/// What the General pane shows of the installed system, read once when it opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub account: Account,
    pub about: Option<crate::about::About>,
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub mounts: Vec<Mount>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub os_build: Option<String>,
    pub desktop_version: String,
    pub commit: Option<String>,
    /// The sign-in options of `login.conf`, as the machine has them.
    pub login: Option<cupertino_defaults::login::LoginConfig>,
}

impl SystemInfo {
    pub fn read() -> SystemInfo {
        let image = ["etc/os-release", "usr/lib/os-release"]
            .iter()
            .find_map(|p| read_small(&Path::new("/").join(p)))
            .map(|t| cupertino_build_info::OsImage::parse(&t))
            .unwrap_or_default();
        SystemInfo {
            account: Account::read(),
            about: Some(crate::about::About::read(Path::new("/"))),
            hostname: hostname(),
            timezone: timezone(),
            locale: locale(),
            mounts: mounts(),
            os_name: image.name,
            os_version: image.version_id,
            os_build: image.build_id.or(image.image_version),
            desktop_version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: cupertino_build_info::short_commit().map(str::to_owned),
            login: cupertino_defaults::login::LoginConfig::load()
                .ok()
                .map(|loaded| loaded.config),
        }
    }
}

/// A person with an account on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub username: String,
    pub initials: String,
    pub picture: Option<PathBuf>,
    pub administrator: bool,
    /// The account this program runs as.
    pub current: bool,
}

/// The lowest user id of a person's account.
const FIRST_USER_ID: u32 = 1000;
/// The user id `nobody` has, which is not a person.
const NOBODY_ID: u32 = 65534;

/// The accounts of people in the text of `/etc/passwd` and `/etc/group`: user ids from 1000 up, not `nobody`, by user
/// id. `current` is the user id this program runs as.
pub fn users_in(passwd: &str, groups: &str, current: u32) -> Vec<User> {
    let mut found: Vec<(u32, User)> = passwd
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            let uid = f.get(2)?.parse::<u32>().ok()?;
            if f.len() < 6 || !(FIRST_USER_ID..NOBODY_ID).contains(&uid) {
                return None;
            }
            // an account whose shell refuses logins is not a person who signs in
            if f.get(6)
                .is_some_and(|shell| shell.ends_with("nologin") || shell.ends_with("/false"))
            {
                return None;
            }
            let username = f[0].to_owned();
            let comment = f[4].split(',').next().unwrap_or("").trim().to_owned();
            let name = if comment.is_empty() {
                username.clone()
            } else {
                comment
            };
            let home = PathBuf::from(f[5]);
            let picture = [
                home.join(".face"),
                home.join(".face.icon"),
                PathBuf::from("/var/lib/AccountsService/icons").join(&username),
            ]
            .into_iter()
            .find(|p| p.is_file());
            Some((
                uid,
                User {
                    initials: initials(&name),
                    administrator: in_admin_group(groups, &username),
                    picture,
                    current: uid == current,
                    name,
                    username,
                },
            ))
        })
        .collect();
    found.sort_by_key(|(uid, _)| *uid);
    found.into_iter().map(|(_, user)| user).collect()
}

/// The accounts of people on this machine.
pub fn users() -> Vec<User> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    users_in(
        &read_small(Path::new("/etc/passwd")).unwrap_or_default(),
        &read_small(Path::new("/etc/group")).unwrap_or_default(),
        uid,
    )
}
