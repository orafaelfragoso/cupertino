//! What the person answered when an application asked to use keyboard shortcuts while another one has the keyboard
//! (`global-shortcuts.conf`, `$CUPERTINO_GLOBAL_SHORTCUTS_CONF` overrides the path). The portal backend asks an app
//! once for the keys it wants; the answer is written here with the keys allowed, and a later request of the same app
//! for those keys follows it without asking again. A request for a key not allowed yet asks again, and a `deny` line
//! refuses every request of the app. Taking a line out makes the app ask again. One line per app, by the app id
//! xdg-desktop-portal names it by, then for `allow` the keys allowed; `#` starts a comment:
//!
//! ```text
//! allow = org.example.Meeting CTRL+ALT+t CTRL+ALT+m
//! deny = org.example.Chat
//! ```
//!
//! An `allow` line that names no key allows none: the app is asked again for every key it wants.
//!
//! The file is the person's: it is read only when it is a regular file, not a link, owned by the reader's user and
//! writable by no group or other user, in a directory no group or other user can write, and it is written with mode
//! `0600`. A file that is refused allows nothing, and nothing is written over it. Each change is
//! [`GlobalShortcutsConf::update`], made under the file's lock; it keeps the file's comments, blank lines and the lines
//! it does not understand.

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
/// Bytes of one key a line holds.
pub const MAX_TRIGGER: usize = 64;
/// Keys one app's line remembers; past it the oldest are forgotten, and asked for again when wanted.
pub const MAX_TRIGGERS: usize = 256;

/// Whether a line can hold `app_id`: not empty, at most [`MAX_APP_ID`] bytes, without white space, control characters
/// or `#`. An app whose id cannot be held is asked every time.
pub fn is_rememberable(app_id: &str) -> bool {
    !app_id.is_empty() && app_id.len() <= MAX_APP_ID && is_word(app_id)
}

/// Whether a line can hold `trigger` as one of an app's keys: not empty, at most [`MAX_TRIGGER`] bytes, without white
/// space, control characters or `#`.
pub fn is_rememberable_trigger(trigger: &str) -> bool {
    !trigger.is_empty() && trigger.len() <= MAX_TRIGGER && is_word(trigger)
}

fn is_word(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '#')
}

/// The answer remembered for one app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub app: String,
    pub decision: Decision,
    /// The keys allowed, as the portal writes them (`CTRL+ALT+t`); none for `deny`.
    pub triggers: Vec<String>,
}

/// The remembered answers, in file order, with the file's lines, so that a change keeps the rest of the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalShortcutsConf {
    pub answers: Vec<Answer>,
    /// The lines read, each with the app whose answer it holds (the first good line of the app).
    lines: Vec<(String, Option<String>)>,
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
        let parsed = ConfText::parse(text);
        for ((line, kind), raw) in parsed.lines().zip(text.lines()) {
            let answer = match kind {
                Line::Blank => Ok(None),
                Line::Entry { key, value } => parse_answer(key, value).map(Some),
                Line::Bare(word) => Err(format!(
                    "expected `allow = <app id> [<key>…]` or `deny = <app id>`, got {word:?}"
                )),
            };
            let owner = match answer {
                Ok(Some(answer)) if conf.decision(&answer.app).is_some() => {
                    problems.push(Problem {
                        line,
                        message: format!("{} is already answered", answer.app),
                    });
                    None
                }
                Ok(Some(answer)) => {
                    let app = answer.app.clone();
                    conf.answers.push(answer);
                    Some(app)
                }
                Ok(None) => None,
                Err(message) => {
                    problems.push(Problem { line, message });
                    None
                }
            };
            conf.lines.push((raw.to_owned(), owner));
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

    /// The file's text: the lines read, each answer's line rewritten with the answer now (its comment kept) or left
    /// out when the answer was forgotten, and every other line as it was; then a line for each new answer. `Err` when
    /// an app a line cannot hold is answered.
    pub fn text(&self) -> io::Result<String> {
        let mut text = String::new();
        let mut written: Vec<&str> = Vec::new();
        for (raw, owner) in &self.lines {
            match owner {
                None => text.push_str(raw),
                Some(app) => {
                    let Some(answer) = self.answer(app) else {
                        continue;
                    };
                    let comment = raw.find('#').map_or("", |i| {
                        let start = raw[..i].trim_end().len();
                        &raw[start..]
                    });
                    text.push_str(&answer_line(answer)?);
                    text.push_str(comment);
                    written.push(app);
                }
            }
            text.push('\n');
        }
        for answer in &self.answers {
            if !written.contains(&answer.app.as_str()) {
                text.push_str(&answer_line(answer)?);
                text.push('\n');
            }
        }
        Ok(text)
    }

    /// Write [`Self::text`] into `path` atomically with mode `0600`.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        crate::atomic::write_atomic_private(path, self.text()?.as_bytes())
    }

    /// Change the file at `path` with `change` while holding its lock: read again, changed and written back. Nothing
    /// is written when the file is refused or cannot be read, or another writer holds the lock longer than
    /// [`LOCK_WAIT`]; waiting that long blocks the caller, so a caller that serves other work runs it on a thread of
    /// its own.
    pub fn update<R>(path: &Path, change: impl FnOnce(&mut Self) -> R) -> io::Result<R> {
        let _lock = FileLock::acquire_within(path, LOCK_WAIT)?;
        let (mut conf, _) = Self::load(path)?;
        let changed = change(&mut conf);
        conf.store(path)?;
        Ok(changed)
    }

    /// The remembered answer of `app_id`; `None` when it was never asked or the line was taken out.
    pub fn answer(&self, app_id: &str) -> Option<&Answer> {
        self.answers.iter().find(|a| a.app == app_id)
    }

    /// The remembered decision of `app_id`.
    pub fn decision(&self, app_id: &str) -> Option<Decision> {
        self.answer(app_id).map(|a| a.decision)
    }

    /// Remember `decision` for `app_id`, in place of an earlier answer: Deny forgets the keys allowed, Allow keeps them.
    /// An id a line cannot hold is not remembered.
    pub fn set(&mut self, app_id: &str, decision: Decision) {
        if !is_rememberable(app_id) {
            return;
        }
        match self.answers.iter_mut().find(|a| a.app == app_id) {
            Some(answer) => {
                answer.decision = decision;
                if decision == Decision::Deny {
                    answer.triggers.clear();
                }
            }
            None => self.answers.push(Answer {
                app: app_id.to_owned(),
                decision,
                triggers: Vec::new(),
            }),
        }
    }

    /// Remember that `app_id` may use `triggers` as well as the keys it was allowed before (a denied app: these
    /// alone). A key a line cannot hold is left out, and past [`MAX_TRIGGERS`] the oldest are forgotten.
    pub fn allow(&mut self, app_id: &str, triggers: impl IntoIterator<Item = String>) {
        self.set(app_id, Decision::Allow);
        let Some(answer) = self.answers.iter_mut().find(|a| a.app == app_id) else {
            return;
        };
        for trigger in triggers {
            if is_rememberable_trigger(&trigger) && !answer.triggers.contains(&trigger) {
                answer.triggers.push(trigger);
            }
        }
        let over = answer.triggers.len().saturating_sub(MAX_TRIGGERS);
        answer.triggers.drain(..over);
    }

    /// Forget the answer of `app_id`, so that it is asked again; whether there was one.
    pub fn forget(&mut self, app_id: &str) -> bool {
        let before = self.answers.len();
        self.answers.retain(|a| a.app != app_id);
        self.answers.len() != before
    }
}

/// The answer of a `key = value` line.
fn parse_answer(key: &str, value: &str) -> Result<Answer, String> {
    let decision = match key {
        "allow" => Decision::Allow,
        "deny" => Decision::Deny,
        _ => return Err(format!("unknown setting {key:?}")),
    };
    let mut words = value.split_whitespace();
    let app = words.next().unwrap_or_default();
    if !is_rememberable(app) {
        return Err(format!("{app:?} is not an app id"));
    }
    let triggers: Vec<String> = words.map(str::to_owned).collect();
    if decision == Decision::Deny && !triggers.is_empty() {
        return Err(format!("`deny` takes the app id alone, got {value:?}"));
    }
    if let Some(bad) = triggers.iter().find(|t| !is_rememberable_trigger(t)) {
        return Err(format!("{bad:?} is not a key"));
    }
    if triggers.len() > MAX_TRIGGERS {
        return Err(format!("more than {MAX_TRIGGERS} keys"));
    }
    Ok(Answer {
        app: app.to_owned(),
        decision,
        triggers,
    })
}

/// The line of `answer`, without its newline.
fn answer_line(answer: &Answer) -> io::Result<String> {
    if !is_rememberable(&answer.app) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{:?} cannot be written in a line", answer.app),
        ));
    }
    Ok(match answer.decision {
        Decision::Allow => {
            let mut line = format!("allow = {}", answer.app);
            for trigger in answer
                .triggers
                .iter()
                .filter(|t| is_rememberable_trigger(t))
            {
                line.push(' ');
                line.push_str(trigger);
            }
            line
        }
        Decision::Deny => format!("deny = {}", answer.app),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn answers_parse_and_the_first_one_of_an_app_stands() {
        let (conf, problems) = GlobalShortcutsConf::parse(
            "allow = org.example.Meeting # push to talk\n\ndeny = org.example.Chat\nallow = org.example.Chat\nmaybe = x\nallow = two words#\n",
        );
        assert_eq!(conf.decision("org.example.Meeting"), Some(Decision::Allow));
        assert_eq!(conf.decision("org.example.Chat"), Some(Decision::Deny));
        assert_eq!(conf.decision("org.example.Other"), None);
        // `two` with the key `words`
        assert_eq!(conf.answer("two").unwrap().triggers, ["words"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn an_allow_line_remembers_its_keys_and_an_old_line_none() {
        let (conf, problems) = GlobalShortcutsConf::parse(
            "allow = org.example.Meeting CTRL+ALT+t  CTRL+ALT+m # talk, mute\nallow = org.example.Old\ndeny = org.example.Chat CTRL+ALT+c\n",
        );
        let meeting = conf.answer("org.example.Meeting").unwrap();
        assert_eq!(meeting.triggers, ["CTRL+ALT+t", "CTRL+ALT+m"]);
        let old = conf.answer("org.example.Old").unwrap();
        assert_eq!(old.decision, Decision::Allow);
        assert!(old.triggers.is_empty());
        // a deny line with keys is not understood: the app is asked
        assert_eq!(conf.decision("org.example.Chat"), None);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn allowing_more_keys_adds_them_and_deny_forgets_them() {
        let mut conf = GlobalShortcutsConf::default();
        conf.allow("org.example.Meeting", ["CTRL+ALT+t".to_owned()]);
        conf.allow(
            "org.example.Meeting",
            [
                "CTRL+ALT+t".to_owned(),
                "CTRL+ALT+m".to_owned(),
                "has space".to_owned(),
            ],
        );
        assert_eq!(
            conf.answer("org.example.Meeting").unwrap().triggers,
            ["CTRL+ALT+t", "CTRL+ALT+m"]
        );
        assert_eq!(
            conf.text().unwrap(),
            "allow = org.example.Meeting CTRL+ALT+t CTRL+ALT+m\n"
        );
        conf.set("org.example.Meeting", Decision::Deny);
        assert_eq!(conf.text().unwrap(), "deny = org.example.Meeting\n");
        conf.allow(
            "org.example.Meeting",
            (0..MAX_TRIGGERS + 2).map(|n| format!("F{n}")),
        );
        let triggers = &conf.answer("org.example.Meeting").unwrap().triggers;
        assert_eq!(triggers.len(), MAX_TRIGGERS);
        assert_eq!(triggers[0], "F2");
    }

    #[test]
    fn a_change_keeps_the_persons_comments_and_the_lines_not_understood() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join(GLOBAL_SHORTCUTS_FILE);
        let before = "# my shortcuts\n\nallow = org.example.Meeting CTRL+ALT+t # talk\nnot a line\nmaybe = x\ndeny = org.example.Chat\nallow = org.example.Chat\n";
        std::fs::write(&path, before).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        GlobalShortcutsConf::update(&path, |c| {
            c.allow("org.example.Meeting", ["CTRL+ALT+m".to_owned()]);
            c.forget("org.example.Chat");
            c.set("org.example.Mail", Decision::Deny);
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# my shortcuts\n\nallow = org.example.Meeting CTRL+ALT+t CTRL+ALT+m # talk\nnot a line\nmaybe = x\nallow = org.example.Chat\ndeny = org.example.Mail\n"
        );
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
        assert!(conf.answers.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
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
