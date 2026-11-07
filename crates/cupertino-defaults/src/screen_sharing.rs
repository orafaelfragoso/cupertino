//! The screen shares the person let an application restore without asking again (`screen-sharing.conf`,
//! `$CUPERTINO_SCREEN_SHARING_CONF` overrides the path). The portal backend writes a grant when the person ticks
//! "remember" in the screen-sharing window and reads the file at each restore; the settings application lists the grants
//! in its Privacy pane and removes the ones the person forgets. One `grant = <id> <app id> display <connector>` per line,
//! `#` starts a comment:
//!
//! ```text
//! grant = 6f1c0d2e9a8b4c3d5e6f708192a3b4c5 org.example.Meeting display DP-1
//! ```
//!
//! The id is 32 lowercase hexadecimal digits, made at random for the grant: xdg-desktop-portal keeps it in its permission
//! store and hands it back with the app id it knows the application by, and a grant restores only for that same app id.
//! Only a display is remembered, by its connector name; a window has no name that lasts beyond the session.
//!
//! The file is the person's: it is read only when it is a regular file, not a link, owned by the reader's user and
//! writable by no group or other user, in a directory no group or other user can write, and it is written with mode
//! `0600`. A file that is refused restores nothing.
//!
//! The portal and the settings application both change the file. Each change is [`ScreenSharingConf::update`]: the
//! file is read, changed and written while [`FileLock`] is held, so a grant one process adds is never dropped by a
//! store the other made from an older read, and a grant forgotten stays forgotten.

use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::conf::{ConfText, Line, Problem};
use crate::lock::{FileLock, LOCK_WAIT};

/// Environment variable that redirects `screen-sharing.conf`.
pub const SCREEN_SHARING_ENV: &str = "CUPERTINO_SCREEN_SHARING_CONF";
/// File name under the configuration directory.
pub const SCREEN_SHARING_FILE: &str = "screen-sharing.conf";
/// Hexadecimal digits of a grant id.
pub const GRANT_ID_DIGITS: usize = 32;
/// Bytes of an app id or a connector name a grant holds.
pub const MAX_NAME: usize = 255;

/// One remembered screen share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// [`GRANT_ID_DIGITS`] lowercase hexadecimal digits.
    pub id: String,
    /// The app id xdg-desktop-portal named the application by.
    pub app_id: String,
    /// The connector name of the shared display, such as `DP-1`.
    pub display: String,
}

impl Grant {
    /// The grant of a line's value, `<id> <app id> display <connector>`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let words: Vec<&str> = value.split_whitespace().collect();
        let [id, app_id, "display", display] = words[..] else {
            return Err(format!(
                "expected `grant = <id> <app id> display <connector>`, got {value:?}"
            ));
        };
        let grant = Grant {
            id: id.to_owned(),
            app_id: app_id.to_owned(),
            display: display.to_owned(),
        };
        grant.check()?;
        Ok(grant)
    }

    /// Refuse a grant a line cannot hold as itself.
    pub fn check(&self) -> Result<(), String> {
        if !is_id(&self.id) {
            return Err(format!(
                "a grant id is {GRANT_ID_DIGITS} lowercase hexadecimal digits, not {:?}",
                self.id
            ));
        }
        for (what, name) in [("app id", &self.app_id), ("connector", &self.display)] {
            if !is_name(name) {
                return Err(format!("{what} {name:?} cannot be written in a grant"));
            }
        }
        Ok(())
    }

    /// The value of the grant's line.
    pub fn text(&self) -> String {
        format!("{} {} display {}", self.id, self.app_id, self.display)
    }
}

/// Whether `id` can be a grant id.
pub fn is_id(id: &str) -> bool {
    id.len() == GRANT_ID_DIGITS
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A word of a line: not empty, at most [`MAX_NAME`] bytes, without white space, `#` or a control character.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '#')
}

/// The grants of the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenSharingConf {
    pub grants: Vec<Grant>,
}

impl ScreenSharingConf {
    /// Where the file is: `$CUPERTINO_SCREEN_SHARING_CONF`, else `screen-sharing.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(SCREEN_SHARING_ENV, SCREEN_SHARING_FILE)
    }

    /// The grants of `text`; a line that is not a grant restores nothing and is a problem. A later line with the id
    /// of an earlier one is a problem too.
    pub fn parse(text: &str) -> (Self, Vec<Problem>) {
        let mut conf = Self::default();
        let mut problems = Vec::new();
        for (line, kind) in ConfText::parse(text).lines() {
            let grant = match kind {
                Line::Blank => continue,
                Line::Entry {
                    key: "grant",
                    value,
                } => Grant::parse(value),
                Line::Entry { key, .. } => Err(format!("unknown setting {key:?}")),
                Line::Bare(word) => Err(format!("expected `grant = …`, got {word:?}")),
            };
            match grant {
                Ok(grant) if conf.grants.iter().any(|g| g.id == grant.id) => {
                    problems.push(Problem {
                        line,
                        message: format!("grant {} is already given", grant.id),
                    })
                }
                Ok(grant) => conf.grants.push(grant),
                Err(message) => problems.push(Problem { line, message }),
            }
        }
        (conf, problems)
    }

    /// [`Self::parse`] of the file at `path`, which must be the reader's own; a missing file holds no grant. `Err`
    /// when the file is refused or cannot be read: it then restores nothing either.
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

    /// Write the grants into `path` atomically with mode `0600`, replacing the file. A grant a line cannot hold is an
    /// error and nothing is written.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        for grant in &self.grants {
            grant
                .check()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            text.push_str(&format!("grant = {}\n", grant.text()));
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

    /// The grant `id` when it was given to `app_id`.
    pub fn find(&self, id: &str, app_id: &str) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.id == id && g.app_id == app_id)
    }

    /// Forget grant `id`; whether there was one.
    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.grants.len();
        self.grants.retain(|g| g.id != id);
        self.grants.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const ID: &str = "6f1c0d2e9a8b4c3d5e6f708192a3b4c5";

    fn grant(id: &str, app_id: &str) -> Grant {
        Grant {
            id: id.to_owned(),
            app_id: app_id.to_owned(),
            display: "DP-1".to_owned(),
        }
    }

    #[test]
    fn a_line_holds_one_grant_and_anything_else_is_a_problem() {
        let other = "0123456789abcdef0123456789abcdef";
        let text = format!(
            "# remembered\ngrant = {ID} org.example.Meeting display DP-1\n\
             grant = {ID} org.example.Other display DP-2\n\
             grant = {} org.example.Meeting display DP-1\n\
             grant = {other} org.example.Meeting window DP-1\n\
             share = {other} org.example.Meeting display DP-1\n\
             grant = {other} org.example.Meeting display HDMI-A-1\n",
            ID.to_uppercase()
        );
        let (conf, problems) = ScreenSharingConf::parse(&text);
        assert_eq!(
            conf.grants,
            [
                grant(ID, "org.example.Meeting"),
                Grant {
                    display: "HDMI-A-1".to_owned(),
                    ..grant(other, "org.example.Meeting")
                }
            ]
        );
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [3, 4, 5, 6], "{problems:?}");
    }

    #[test]
    fn a_grant_restores_only_for_its_own_app_and_is_gone_once_revoked() {
        let mut conf = ScreenSharingConf {
            grants: vec![grant(ID, "org.example.Meeting")],
        };
        assert!(conf.find(ID, "org.example.Meeting").is_some());
        assert!(conf.find(ID, "org.example.Other").is_none());
        assert!(conf.revoke(ID));
        assert!(!conf.revoke(ID));
        assert!(conf.find(ID, "org.example.Meeting").is_none());
    }

    #[test]
    fn the_file_is_written_private_read_back_and_refused_when_others_can_write_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCREEN_SHARING_FILE);
        assert_eq!(
            ScreenSharingConf::load(&path).unwrap().0,
            Default::default()
        );
        let conf = ScreenSharingConf {
            grants: vec![grant(ID, "org.example.Meeting")],
        };
        conf.store(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (read, problems) = ScreenSharingConf::load(&path).unwrap();
        assert_eq!(read, conf);
        assert!(problems.is_empty());

        let bad = ScreenSharingConf {
            grants: vec![grant(ID, "org.example Meeting")],
        };
        assert!(bad.store(&path).is_err(), "an app id with a space");
        assert_eq!(
            ScreenSharingConf::load(&path).unwrap().0,
            conf,
            "nothing written"
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(ScreenSharingConf::load(&path).is_err());
        assert!(
            ScreenSharingConf::update(&path, |conf| {
                conf.grants.clear();
                Ok(())
            })
            .is_err(),
            "a refused file is not changed"
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains(ID));
        let link = dir.path().join("link.conf");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(ScreenSharingConf::load(&link).is_err());
    }

    #[test]
    fn an_update_while_another_writer_keeps_the_lock_fails_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCREEN_SHARING_FILE);
        ScreenSharingConf {
            grants: vec![grant(ID, "org.example.Meeting")],
        }
        .store(&path)
        .unwrap();
        let held = FileLock::acquire(&path).unwrap();
        let refused = ScreenSharingConf::update_within(&path, Duration::from_millis(50), |conf| {
            conf.grants.clear();
            Ok(())
        })
        .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::TimedOut);
        assert!(std::fs::read_to_string(&path).unwrap().contains(ID));
        drop(held);
        assert!(ScreenSharingConf::update(&path, |conf| Ok(conf.revoke(ID))).unwrap());
    }

    /// The grant id of number `n`.
    fn numbered(n: usize) -> String {
        format!("{n:032x}")
    }

    #[test]
    fn concurrent_updates_keep_every_grant_added_and_never_bring_back_one_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCREEN_SHARING_FILE);
        ScreenSharingConf {
            grants: vec![grant(ID, "org.example.Meeting")],
        }
        .store(&path)
        .unwrap();
        const ADDERS: usize = 8;
        const EACH: usize = 12;
        /// How long each writer of the test waits for the lock.
        const CONTENDED: Duration = Duration::from_secs(60);
        let start = std::sync::Arc::new(std::sync::Barrier::new(ADDERS + 1));
        let mut threads = Vec::new();
        for adder in 0..ADDERS {
            let (path, start) = (path.clone(), std::sync::Arc::clone(&start));
            threads.push(std::thread::spawn(move || {
                start.wait();
                for n in 0..EACH {
                    // many writers on one file wait longer for each other than two processes ever do
                    ScreenSharingConf::update_within(&path, CONTENDED, |conf| {
                        conf.grants
                            .push(grant(&numbered(adder * EACH + n + 1), "org.example.Add"));
                        // a slow change: without the lock another writer reads the file meanwhile
                        std::thread::yield_now();
                        Ok(())
                    })
                    .unwrap();
                }
            }));
        }
        let forget = {
            let (path, start) = (path.clone(), std::sync::Arc::clone(&start));
            std::thread::spawn(move || {
                start.wait();
                ScreenSharingConf::update_within(&path, CONTENDED, |conf| Ok(conf.revoke(ID)))
                    .unwrap()
            })
        };
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(forget.join().unwrap(), "the grant was there to forget");
        let (conf, problems) = ScreenSharingConf::load(&path).unwrap();
        assert!(problems.is_empty());
        assert!(
            conf.find(ID, "org.example.Meeting").is_none(),
            "a forgotten grant stays forgotten"
        );
        for n in 1..=ADDERS * EACH {
            assert!(
                conf.find(&numbered(n), "org.example.Add").is_some(),
                "grant {n} was kept"
            );
        }
        assert_eq!(conf.grants.len(), ADDERS * EACH);
    }
}
