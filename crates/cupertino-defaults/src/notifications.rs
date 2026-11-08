//! `notifications.conf` (`$CUPERTINO_NOTIFICATIONS_CONF` overrides the path): what notifications show while the screen
//! is locked, and how much of them shows at all. A global default, and per-application overrides that each key may
//! leave unset (the application then follows the default).
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | `show_on_lock_screen` | `on`, `off` | `on` |
//! | `show_previews` | `always`, `when-unlocked`, `never` | `when-unlocked` |
//! | `app.<id>.show_on_lock_screen` | `on`, `off` | the default's |
//! | `app.<id>.show_previews` | `always`, `when-unlocked`, `never` | the default's |
//!
//! `<id>` is the application's desktop entry id without `.desktop` (`org.example.Mail`), or the name it gives itself
//! when it names no desktop entry; [`valid_app_id`] says which ids a file can hold. The notification daemon reads the
//! file and follows its changes; the settings application writes it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{parse_switch, switch_text, ConfText, Line, Problem};

/// Environment variable that redirects the settings file.
pub const NOTIFICATIONS_CONF_ENV: &str = "CUPERTINO_NOTIFICATIONS_CONF";

/// How much of a notification shows: its title and body, or only its application and the word "Notification".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Previews {
    /// Everywhere, the lock screen included.
    Always,
    /// While the session is unlocked; the lock screen shows the application only.
    WhenUnlocked,
    /// Nowhere: banners, the notification centre and the lock screen show the application only.
    Never,
}

impl Previews {
    /// Every value, in the order the settings offer them.
    pub const ALL: [Previews; 3] = [Previews::Always, Previews::WhenUnlocked, Previews::Never];

    /// The value as the file writes it.
    pub fn key(self) -> &'static str {
        match self {
            Previews::Always => "always",
            Previews::WhenUnlocked => "when-unlocked",
            Previews::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Result<Previews, String> {
        Previews::ALL
            .into_iter()
            .find(|p| p.key() == value)
            .ok_or_else(|| format!("must be always, when-unlocked or never, not {value:?}"))
    }

    /// Whether the title and body show, with the session locked or not.
    pub fn shows_content(self, locked: bool) -> bool {
        match self {
            Previews::Always => true,
            Previews::WhenUnlocked => !locked,
            Previews::Never => false,
        }
    }
}

/// One application's overrides; `None` follows the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppOverride {
    pub show_on_lock_screen: Option<bool>,
    pub show_previews: Option<Previews>,
}

impl AppOverride {
    fn is_empty(&self) -> bool {
        self.show_on_lock_screen.is_none() && self.show_previews.is_none()
    }
}

/// What an application's notifications get once the default and its override are combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub show_on_lock_screen: bool,
    pub show_previews: Previews,
}

/// Everything in `notifications.conf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationSettings {
    /// The default of every application without an override of its own.
    pub show_on_lock_screen: bool,
    pub show_previews: Previews,
    /// By application id, sorted; an application without an entry follows the default.
    pub apps: BTreeMap<String, AppOverride>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self::UNSET
    }
}

/// Longest application id a file holds, in bytes.
pub const MAX_APP_ID_BYTES: usize = 255;

/// Whether `id` can name an application in the file: not empty, at most [`MAX_APP_ID_BYTES`], no control character,
/// no `=` or `#` (which end a key), no space at either end, and not ending in `.` (which would run into the key's
/// name).
pub fn valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_APP_ID_BYTES
        && id.trim() == id
        && !id.ends_with('.')
        && !id.chars().any(|c| c.is_control() || c == '=' || c == '#')
}

const LOCK_KEY: &str = "show_on_lock_screen";
const PREVIEWS_KEY: &str = "show_previews";
const APP_PREFIX: &str = "app.";

fn app_key(id: &str, field: &str) -> String {
    format!("{APP_PREFIX}{id}.{field}")
}

/// What a line of the file sets.
enum Setting<'a> {
    Lock(bool),
    Previews(Previews),
    AppLock(&'a str, bool),
    AppPreviews(&'a str, Previews),
}

/// The key a setting belongs to, for keeping its last good value.
#[derive(PartialEq)]
enum Slot<'a> {
    Lock,
    Previews,
    AppLock(&'a str),
    AppPreviews(&'a str),
}

fn slot(key: &str) -> Result<Slot<'_>, String> {
    match key {
        LOCK_KEY => return Ok(Slot::Lock),
        PREVIEWS_KEY => return Ok(Slot::Previews),
        _ => {}
    }
    let Some((id, field)) = key
        .strip_prefix(APP_PREFIX)
        .and_then(|rest| rest.rsplit_once('.'))
    else {
        return Err(format!("unknown setting {key:?}"));
    };
    if !valid_app_id(id) {
        return Err(format!("{id:?} is not an application id"));
    }
    match field {
        LOCK_KEY => Ok(Slot::AppLock(id)),
        PREVIEWS_KEY => Ok(Slot::AppPreviews(id)),
        _ => Err(format!("unknown setting {key:?}")),
    }
}

fn setting<'a>(slot: &Slot<'a>, value: &str) -> Result<Setting<'a>, String> {
    Ok(match *slot {
        Slot::Lock => Setting::Lock(parse_switch(value)?),
        Slot::Previews => Setting::Previews(Previews::parse(value)?),
        Slot::AppLock(id) => Setting::AppLock(id, parse_switch(value)?),
        Slot::AppPreviews(id) => Setting::AppPreviews(id, Previews::parse(value)?),
    })
}

impl NotificationSettings {
    /// What a missing or empty file reads as: notifications on the lock screen, their content only while unlocked.
    pub const UNSET: Self = Self {
        show_on_lock_screen: true,
        show_previews: Previews::WhenUnlocked,
        apps: BTreeMap::new(),
    };

    /// Where the file is: `$CUPERTINO_NOTIFICATIONS_CONF`, else `notifications.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(NOTIFICATIONS_CONF_ENV, "notifications.conf")
    }

    /// The policy of the application `id` (see the module's table for what names it).
    pub fn policy(&self, id: &str) -> Policy {
        let own = self.apps.get(id).copied().unwrap_or_default();
        Policy {
            show_on_lock_screen: own.show_on_lock_screen.unwrap_or(self.show_on_lock_screen),
            show_previews: own.show_previews.unwrap_or(self.show_previews),
        }
    }

    /// Change one application's override; an override with both keys unset is dropped. `false` for an id the file
    /// cannot hold ([`valid_app_id`]), which changes nothing.
    pub fn set_app(&mut self, id: &str, value: AppOverride) -> bool {
        if !valid_app_id(id) {
            return false;
        }
        if value.is_empty() {
            self.apps.remove(id);
        } else {
            self.apps.insert(id.to_owned(), value);
        }
        true
    }

    fn apply(&mut self, setting: Setting<'_>) {
        match setting {
            Setting::Lock(on) => self.show_on_lock_screen = on,
            Setting::Previews(p) => self.show_previews = p,
            Setting::AppLock(id, on) => {
                self.apps.entry(id.to_owned()).or_default().show_on_lock_screen = Some(on)
            }
            Setting::AppPreviews(id, p) => {
                self.apps.entry(id.to_owned()).or_default().show_previews = Some(p)
            }
        }
    }

    fn keep(&mut self, slot: &Slot<'_>, good: &Self) {
        match *slot {
            Slot::Lock => self.show_on_lock_screen = good.show_on_lock_screen,
            Slot::Previews => self.show_previews = good.show_previews,
            Slot::AppLock(id) => {
                let kept = good.apps.get(id).and_then(|a| a.show_on_lock_screen);
                self.apps.entry(id.to_owned()).or_default().show_on_lock_screen = kept;
            }
            Slot::AppPreviews(id) => {
                let kept = good.apps.get(id).and_then(|a| a.show_previews);
                self.apps.entry(id.to_owned()).or_default().show_previews = kept;
            }
        }
    }

    fn read(text: &ConfText, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        let mut settings = Self::UNSET;
        let mut problems = Vec::new();
        for (line, kind) in text.lines() {
            let (key, value) = match kind {
                Line::Blank => continue,
                Line::Bare(word) => {
                    problems.push(Problem {
                        line,
                        message: format!("expected `key = value`, got {word:?}"),
                    });
                    continue;
                }
                Line::Entry { key, value } => (key, value),
            };
            let slot = match slot(key) {
                Ok(slot) => slot,
                Err(message) => {
                    problems.push(Problem { line, message });
                    continue;
                }
            };
            match setting(&slot, value) {
                Ok(s) => settings.apply(s),
                Err(e) => {
                    problems.push(Problem {
                        line,
                        message: format!("{key}: {e}"),
                    });
                    if let Some(good) = last_good {
                        settings.keep(&slot, good);
                    }
                }
            }
        }
        settings.apps.retain(|_, a| !a.is_empty());
        (settings, problems)
    }

    /// Every key with the value it is written with; unset keys are absent.
    fn entries(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert(LOCK_KEY.to_owned(), switch_text(self.show_on_lock_screen));
        out.insert(PREVIEWS_KEY.to_owned(), self.show_previews.key().to_owned());
        for (id, a) in &self.apps {
            if let Some(on) = a.show_on_lock_screen {
                out.insert(app_key(id, LOCK_KEY), switch_text(on));
            }
            if let Some(p) = a.show_previews {
                out.insert(app_key(id, PREVIEWS_KEY), p.key().to_owned());
            }
        }
        out
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        Self::read(&ConfText::parse(text), last_good)
    }

    /// [`Self::parse`] of the file at `path`; a missing file is every key unset.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        Ok(Self::read(&ConfText::load(path)?, last_good))
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay, and
    /// an override taken away removes its lines. Returns the problems of the lines that were there before.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        let mut text = ConfText::load(path)?;
        let (before, problems) = Self::read(&text, None);
        let (old, new) = (before.entries(), self.entries());
        for (key, value) in &new {
            if old.get(key) != Some(value) {
                text.set(key, value);
            }
        }
        for key in old.keys().filter(|k| !new.contains_key(*k)) {
            text.remove(key);
        }
        crate::atomic::write_atomic(path, text.to_text().as_bytes())?;
        Ok(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_shows_notifications_on_the_lock_screen_without_their_content() {
        let (s, problems) = NotificationSettings::parse("", None);
        assert!(problems.is_empty());
        assert_eq!(s, NotificationSettings::UNSET);
        assert_eq!(
            s.policy("org.example.Mail"),
            Policy {
                show_on_lock_screen: true,
                show_previews: Previews::WhenUnlocked
            }
        );
    }

    #[test]
    fn an_application_override_wins_key_by_key_and_others_follow_the_default() {
        let text = "show_on_lock_screen = off\nshow_previews = never\n\
                    app.org.example.Mail.show_previews = always\n\
                    app.Chat App.show_on_lock_screen = on\n";
        let (s, problems) = NotificationSettings::parse(text, None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            s.policy("org.example.Mail"),
            Policy {
                show_on_lock_screen: false,
                show_previews: Previews::Always
            }
        );
        assert_eq!(
            s.policy("Chat App"),
            Policy {
                show_on_lock_screen: true,
                show_previews: Previews::Never
            }
        );
        assert_eq!(
            s.policy("other"),
            Policy {
                show_on_lock_screen: false,
                show_previews: Previews::Never
            }
        );
    }

    #[test]
    fn bad_lines_are_reported_and_a_bad_value_keeps_its_last_good_one() {
        let good = NotificationSettings::parse(
            "show_previews = always\napp.mail.show_on_lock_screen = off\n",
            None,
        )
        .0;
        let text = "show_previews = sometimes\napp.mail.show_on_lock_screen = maybe\n\
                    app.mail.colour = red\nzoom = 2\napp..show_previews = never\nstray\n";
        let (s, problems) = NotificationSettings::parse(text, Some(&good));
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert_eq!(s.show_previews, Previews::Always);
        assert!(!s.policy("mail").show_on_lock_screen);
        let (fresh, _) = NotificationSettings::parse(text, None);
        assert_eq!(fresh, NotificationSettings::UNSET, "no last good value: unset");
    }

    #[test]
    fn ids_that_would_break_a_key_are_refused() {
        for bad in ["", " mail", "mail ", "a=b", "a#b", "mail.", "a\nb"] {
            assert!(!valid_app_id(bad), "{bad:?}");
        }
        assert!(valid_app_id("org.example.Mail"));
        assert!(valid_app_id("Chat App"));
        let mut s = NotificationSettings::UNSET;
        assert!(!s.set_app("a=b", AppOverride::default()));
    }

    #[test]
    fn a_store_round_trips_keeps_comments_and_removes_a_dropped_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notifications.conf");
        std::fs::write(
            &path,
            "# lock screen\nshow_previews = never # quiet\napp.mail.show_previews = always\n",
        )
        .unwrap();
        let mut s = NotificationSettings::load(&path, None).unwrap().0;
        s.show_previews = Previews::WhenUnlocked;
        s.set_app("mail", AppOverride::default());
        s.set_app(
            "org.example.Chat",
            AppOverride {
                show_on_lock_screen: Some(false),
                show_previews: None,
            },
        );
        assert!(s.store(&path).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# lock screen\nshow_previews = when-unlocked # quiet\n\
             app.org.example.Chat.show_on_lock_screen = off\n"
        );
        assert_eq!(NotificationSettings::load(&path, None).unwrap(), (s, vec![]));
    }
}
