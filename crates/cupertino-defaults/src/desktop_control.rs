//! What the person decided about each app that asked to operate or see the desktop (`desktop-control.conf`,
//! `$CUPERTINO_DESKTOP_CONTROL_CONF` overrides the path). An app is asked once per service; its answer is written here
//! and every later request from the same app follows it without asking again. The settings application lists the
//! apps of each service in its Privacy pane, where the person turns an app on or off or removes it, which makes the app
//! ask again at its next request. One line per app and service, `#` starts a comment:
//!
//! ```text
//! allow = control app:foot
//! allow = capture app:foot
//! deny = control exe:/home/me/bin/agent
//! ```
//!
//! [`Service::Control`] covers observing windows and their controls, semantic actions, pointer and keyboard input;
//! [`Service::Capture`] covers images of the screen. An app is named by the desktop entry id of the application window
//! it runs under ([`App::Desktop`]), or, when it runs under none, by the absolute path of its executable
//! ([`App::Executable`]). A path holding white space or `#` cannot be written; such an app is asked every time.
//!
//! The file is the person's: it is read only when it is a regular file, not a link, owned by the reader's user and
//! writable by no group or other user, in a directory no group or other user can write, and it is written with mode
//! `0600`. A file that is refused allows nothing, and nothing is written over it.
//!
//! The compositor and the settings application both change the file. Each change is [`DesktopControlConf::update`]:
//! the file is read, changed and written while [`FileLock`] is held, so an answer one process records is never dropped
//! by a store the other made from an older read, and an app removed stays removed.

use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::conf::{ConfText, Line, Problem};
use crate::lock::{FileLock, LOCK_WAIT};

/// Environment variable that redirects `desktop-control.conf`.
pub const DESKTOP_CONTROL_ENV: &str = "CUPERTINO_DESKTOP_CONTROL_CONF";
/// File name under the configuration directory.
pub const DESKTOP_CONTROL_FILE: &str = "desktop-control.conf";
/// Bytes of an app's name a line holds, prefix excluded.
pub const MAX_APP: usize = 1024;

/// What an app may be allowed to do, each asked and remembered on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Service {
    /// Observe windows and their controls, perform semantic actions, and send pointer and keyboard input.
    Control,
    /// Take images of the screen.
    Capture,
}

impl Service {
    /// Every service, in the order the settings application lists them.
    pub const ALL: [Service; 2] = [Service::Control, Service::Capture];

    /// The word of a line.
    pub fn word(self) -> &'static str {
        match self {
            Service::Control => "control",
            Service::Capture => "capture",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.word() == word)
    }
}

/// The app a decision belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum App {
    /// The desktop entry id, without `.desktop`, of the application window the requester runs under.
    Desktop(String),
    /// The absolute path of the requester's executable, when it runs under no application window.
    Executable(PathBuf),
}

impl App {
    const DESKTOP: &'static str = "app:";
    const EXECUTABLE: &'static str = "exe:";

    /// The app of a line's word, `app:<id>` or `exe:<absolute path>`.
    pub fn parse(word: &str) -> Result<Self, String> {
        let app = if let Some(id) = word.strip_prefix(Self::DESKTOP) {
            App::Desktop(id.to_owned())
        } else if let Some(path) = word.strip_prefix(Self::EXECUTABLE) {
            App::Executable(PathBuf::from(path))
        } else {
            return Err(format!("expected `app:<id>` or `exe:<path>`, got {word:?}"));
        };
        app.check()?;
        Ok(app)
    }

    /// Refuse an app a line cannot hold as itself.
    pub fn check(&self) -> Result<(), String> {
        let name = match self {
            App::Desktop(id) => id.as_str(),
            App::Executable(path) => {
                if !path.is_absolute() {
                    return Err(format!("executable {path:?} is not an absolute path"));
                }
                path.to_str()
                    .ok_or_else(|| format!("executable {path:?} is not UTF-8"))?
            }
        };
        if name.is_empty()
            || name.len() > MAX_APP
            || name
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '#')
        {
            return Err(format!("app {name:?} cannot be written in a line"));
        }
        Ok(())
    }

    /// Whether a line can hold the app, so that its answer can be remembered.
    pub fn is_rememberable(&self) -> bool {
        self.check().is_ok()
    }

    /// The word of the app's line.
    pub fn text(&self) -> String {
        match self {
            App::Desktop(id) => format!("{}{id}", Self::DESKTOP),
            App::Executable(path) => format!("{}{}", Self::EXECUTABLE, path.display()),
        }
    }
}

/// The person's answer for one app and service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    fn key(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
        }
    }
}

/// One remembered answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub service: Service,
    pub app: App,
    pub decision: Decision,
}

/// The remembered answers of the file, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopControlConf {
    pub entries: Vec<Entry>,
}

impl DesktopControlConf {
    /// Where the file is: `$CUPERTINO_DESKTOP_CONTROL_CONF`, else `desktop-control.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(DESKTOP_CONTROL_ENV, DESKTOP_CONTROL_FILE)
    }

    /// The answers of `text`; a line that is not one decides nothing and is a problem. A later line for the service and
    /// app of an earlier one is a problem too: the first answer stands.
    pub fn parse(text: &str) -> (Self, Vec<Problem>) {
        let mut conf = Self::default();
        let mut problems = Vec::new();
        for (line, kind) in ConfText::parse(text).lines() {
            let entry = match kind {
                Line::Blank => continue,
                Line::Entry { key, value } => match key {
                    "allow" => parse_entry(Decision::Allow, value),
                    "deny" => parse_entry(Decision::Deny, value),
                    _ => Err(format!("unknown setting {key:?}")),
                },
                Line::Bare(word) => {
                    Err(format!("expected `allow = …` or `deny = …`, got {word:?}"))
                }
            };
            match entry {
                Ok(entry) if conf.decision(entry.service, &entry.app).is_some() => {
                    problems.push(Problem {
                        line,
                        message: format!(
                            "{} for {} is already decided",
                            entry.service.word(),
                            entry.app.text()
                        ),
                    })
                }
                Ok(entry) => conf.entries.push(entry),
                Err(message) => problems.push(Problem { line, message }),
            }
        }
        (conf, problems)
    }

    /// [`Self::parse`] of the file at `path`, which must be the reader's own; a missing file decides nothing. `Err`
    /// when the file is refused or cannot be read: it then allows nothing either.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<Problem>)> {
        // Safety: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            // without O_NONBLOCK a FIFO put at the path would block the open before the check refuses it
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Default::default()),
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(crate::data_control::refused("the file is a link"))
            }
            Err(e) => return Err(e),
        };
        let dir = std::fs::metadata(path.parent().unwrap_or(Path::new(".")))?;
        crate::data_control::check_owned(&file.metadata()?, &dir, user)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(Self::parse(&text))
    }

    /// Write the answers into `path` atomically with mode `0600`, replacing the file. An app a line cannot hold is an
    /// error and nothing is written.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        for entry in &self.entries {
            entry
                .app
                .check()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            text.push_str(&format!(
                "{} = {} {}\n",
                entry.decision.key(),
                entry.service.word(),
                entry.app.text()
            ));
        }
        crate::atomic::write_atomic_private(path, text.as_bytes())
    }

    /// Change the file at `path` with `change` while holding its lock: it is read again ([`Self::load`]), changed,
    /// and written back ([`Self::store`]) before another writer can read it. Nothing is written when the file is
    /// refused or cannot be read, when another writer holds the lock longer than [`LOCK_WAIT`], or when `change`
    /// fails; what `change` returns otherwise.
    pub fn update<R>(
        path: &Path,
        change: impl FnOnce(&mut Self) -> io::Result<R>,
    ) -> io::Result<R> {
        Self::update_within(path, LOCK_WAIT, change)
    }

    /// [`Self::update`], waiting at most `wait` for the lock.
    fn update_within<R>(
        path: &Path,
        wait: Duration,
        change: impl FnOnce(&mut Self) -> io::Result<R>,
    ) -> io::Result<R> {
        let _lock = FileLock::acquire_within(path, wait)?;
        let (mut conf, _) = Self::load(path)?;
        let changed = change(&mut conf)?;
        conf.store(path)?;
        Ok(changed)
    }

    /// The remembered answer of `app` for `service`; `None` when it was never asked or was removed.
    pub fn decision(&self, service: Service, app: &App) -> Option<Decision> {
        self.entries
            .iter()
            .find(|e| e.service == service && &e.app == app)
            .map(|e| e.decision)
    }

    /// Remember `decision` for `app` and `service`, replacing an earlier answer in its place. `Err` when a line cannot
    /// hold the app.
    pub fn set(&mut self, service: Service, app: &App, decision: Decision) -> Result<(), String> {
        app.check()?;
        match self
            .entries
            .iter_mut()
            .find(|e| e.service == service && &e.app == app)
        {
            Some(entry) => entry.decision = decision,
            None => self.entries.push(Entry {
                service,
                app: app.clone(),
                decision,
            }),
        }
        Ok(())
    }

    /// Forget the answer of `app` for `service`, so that it is asked again; whether there was one.
    pub fn forget(&mut self, service: Service, app: &App) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.service == service && &e.app == app));
        self.entries.len() != before
    }

    /// The answers for `service`, in file order.
    pub fn service(&self, service: Service) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.service == service)
    }
}

/// The entry of a line's value, `<service> <app>`.
fn parse_entry(decision: Decision, value: &str) -> Result<Entry, String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    let [service, app] = words[..] else {
        return Err(format!("expected `<service> <app>`, got {value:?}"));
    };
    let service = Service::parse(service)
        .ok_or_else(|| format!("unknown service {service:?}; expected `control` or `capture`"))?;
    Ok(Entry {
        service,
        app: App::parse(app)?,
        decision,
    })
}
