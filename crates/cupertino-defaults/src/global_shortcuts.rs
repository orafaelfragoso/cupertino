//! What the person answered when an application asked to use keyboard shortcuts while another one has the keyboard
//! (`global-shortcuts.conf`, `$CUPERTINO_GLOBAL_SHORTCUTS_CONF` overrides the path). The portal backend asks once per
//! app; the answer is written here, and every later request of the same app follows it without asking again. Taking
//! a line out makes the app ask again. One line per app, by the app id xdg-desktop-portal names it by, `#` starts a
//! comment:
//!
//! ```text
//! allow = org.example.Meeting
//! deny = org.example.Chat
//! ```
//!
//! The file is the person's: it is read only when it is a regular file, not a link, owned by the reader's user and
//! writable by no group or other user, in a directory no group or other user can write, and it is written with mode
//! `0600`. A file that is refused allows nothing, and nothing is written over it. Each change is
//! [`GlobalShortcutsConf::update`], made under the file's lock.

use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::conf::{ConfText, Line, Problem};
use crate::desktop_control::Decision;
use crate::lock::{FileLock, LOCK_WAIT};

/// Environment variable that redirects `global-shortcuts.conf`.
pub const GLOBAL_SHORTCUTS_ENV: &str = "CUPERTINO_GLOBAL_SHORTCUTS_CONF";
/// File name under the configuration directory.
pub const GLOBAL_SHORTCUTS_FILE: &str = "global-shortcuts.conf";
/// Bytes of an app id a line holds.
pub const MAX_APP_ID: usize = 255;

/// Whether a line can hold `app_id`: not empty, at most [`MAX_APP_ID`] bytes, without white space, control characters
/// or `#`. An app whose id cannot be held is asked every time.
pub fn is_rememberable(app_id: &str) -> bool {
    !app_id.is_empty()
        && app_id.len() <= MAX_APP_ID
        && !app_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '#')
}

/// The remembered answers, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalShortcutsConf {
    pub answers: Vec<(String, Decision)>,
}

impl GlobalShortcutsConf {
    /// Where the file is: `$CUPERTINO_GLOBAL_SHORTCUTS_CONF`, else `global-shortcuts.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(GLOBAL_SHORTCUTS_ENV, GLOBAL_SHORTCUTS_FILE)
    }

    /// The answers of `text`; a line that is not one is a problem, and so is a second answer for an app: the first
    /// stands.
    pub fn parse(text: &str) -> (Self, Vec<Problem>) {
        let mut conf = Self::default();
        let mut problems = Vec::new();
        for (line, kind) in ConfText::parse(text).lines() {
            let answer = match kind {
                Line::Blank => continue,
                Line::Entry { key, value } => {
                    let decision = match key {
                        "allow" => Ok(Decision::Allow),
                        "deny" => Ok(Decision::Deny),
                        _ => Err(format!("unknown setting {key:?}")),
                    };
                    decision.and_then(|d| {
                        if is_rememberable(value) {
                            Ok((value.to_owned(), d))
                        } else {
                            Err(format!("{value:?} is not an app id"))
                        }
                    })
                }
                Line::Bare(word) => Err(format!(
                    "expected `allow = <app id>` or `deny = <app id>`, got {word:?}"
                )),
            };
            match answer {
                Ok((app, _)) if conf.decision(&app).is_some() => problems.push(Problem {
                    line,
                    message: format!("{app} is already answered"),
                }),
                Ok(answer) => conf.answers.push(answer),
                Err(message) => problems.push(Problem { line, message }),
            }
        }
        (conf, problems)
    }

    /// [`Self::parse`] of the file at `path`, which must be the reader's own; a missing file answers nothing. `Err`
    /// when the file is refused or cannot be read.
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

    /// Write the answers into `path` atomically with mode `0600`.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        for (app, decision) in &self.answers {
            if !is_rememberable(app) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{app:?} cannot be written in a line"),
                ));
            }
            let key = match decision {
                Decision::Allow => "allow",
                Decision::Deny => "deny",
            };
            text.push_str(&format!("{key} = {app}\n"));
        }
        crate::atomic::write_atomic_private(path, text.as_bytes())
    }

    /// Change the file at `path` with `change` while holding its lock: read again, changed and written back. Nothing
    /// is written when the file is refused or cannot be read, or another writer holds the lock longer than
    /// [`LOCK_WAIT`].
    pub fn update<R>(path: &Path, change: impl FnOnce(&mut Self) -> R) -> io::Result<R> {
        let _lock = FileLock::acquire_within(path, LOCK_WAIT)?;
        let (mut conf, _) = Self::load(path)?;
        let changed = change(&mut conf);
        conf.store(path)?;
        Ok(changed)
    }

    /// The remembered answer of `app_id`; `None` when it was never asked or the line was taken out.
    pub fn decision(&self, app_id: &str) -> Option<Decision> {
        self.answers
            .iter()
            .find(|(app, _)| app == app_id)
            .map(|(_, d)| *d)
    }

    /// Remember `decision` for `app_id`, in place of an earlier answer. An id a line cannot hold is not remembered.
    pub fn set(&mut self, app_id: &str, decision: Decision) {
        if !is_rememberable(app_id) {
            return;
        }
        match self.answers.iter_mut().find(|(app, _)| app == app_id) {
            Some((_, d)) => *d = decision,
            None => self.answers.push((app_id.to_owned(), decision)),
        }
    }

    /// Forget the answer of `app_id`, so that it is asked again; whether there was one.
    pub fn forget(&mut self, app_id: &str) -> bool {
        let before = self.answers.len();
        self.answers.retain(|(app, _)| app != app_id);
        self.answers.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn answers_parse_and_the_first_one_of_an_app_stands() {
        let (conf, problems) = GlobalShortcutsConf::parse(
            "allow = org.example.Meeting # push to talk\n\ndeny = org.example.Chat\nallow = org.example.Chat\nmaybe = x\nallow = two words\n",
        );
        assert_eq!(conf.decision("org.example.Meeting"), Some(Decision::Allow));
        assert_eq!(conf.decision("org.example.Chat"), Some(Decision::Deny));
        assert_eq!(conf.decision("org.example.Other"), None);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn an_answer_is_stored_changed_and_forgotten_under_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join(GLOBAL_SHORTCUTS_FILE);
        GlobalShortcutsConf::update(&path, |c| c.set("org.example.Meeting", Decision::Deny))
            .unwrap();
        GlobalShortcutsConf::update(&path, |c| {
            c.set("org.example.Meeting", Decision::Allow);
            c.set("", Decision::Allow);
            c.set("has space", Decision::Allow);
        })
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "allow = org.example.Meeting\n"
        );
        assert!(GlobalShortcutsConf::update(&path, |c| c.forget("org.example.Meeting")).unwrap());
        let (conf, _) = GlobalShortcutsConf::load(&path).unwrap();
        assert_eq!(conf, GlobalShortcutsConf::default());
    }

    #[test]
    fn a_file_others_can_write_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join(GLOBAL_SHORTCUTS_FILE);
        std::fs::write(&path, "allow = org.example.Meeting\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(GlobalShortcutsConf::load(&path).is_err());
        std::os::unix::fs::symlink(&path, dir.path().join("link")).unwrap();
        assert!(GlobalShortcutsConf::load(&dir.path().join("link")).is_err());
    }
}
