//! The `key = value` format of the `*.conf` settings files, read and written by one table per file.
//!
//! A line is blank, a comment (`#` starts a comment anywhere on a line), `key = value`, or a bare word (the dock's
//! pins; an error in the other files). [`ConfText`] keeps the file as its lines, so a write changes only the lines of
//! the keys whose value changed and keeps comments, unknown keys, blank lines and the order of everything else.
//!
//! A file's settings are described by a table of [`Key`]s: how a value is read, how it is written and how a key's
//! value is carried over from the last good settings. [`read`] applies the lines in order (a later line of a key
//! wins); a line that is not understood is reported as a [`Problem`] with its line number and, when the last good
//! settings are given, that key keeps its last good value rather than falling back to the default. [`write`] touches
//! the lines of the keys whose written value differs.

use std::io;
use std::path::Path;

/// A line that was not applied: its 1-based number and what is wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

/// One line of a settings file, without its comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    /// Empty or a comment only.
    Blank,
    Entry {
        key: &'a str,
        value: &'a str,
    },
    /// Text without `=`.
    Bare(&'a str),
}

/// A settings file as its lines, to read entries from and to change values in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfText {
    lines: Vec<String>,
}

/// The text of a line before its comment, and the comment with the spaces before it (`  # note`).
fn split_comment(raw: &str) -> (&str, &str) {
    match raw.find('#') {
        Some(i) => {
            let body = &raw[..i];
            let start = body.trim_end().len();
            (&raw[..start], &raw[start..])
        }
        None => (raw, ""),
    }
}

fn classify(raw: &str) -> Line<'_> {
    let body = split_comment(raw).0.trim();
    if body.is_empty() {
        return Line::Blank;
    }
    match body.split_once('=') {
        Some((key, value)) => Line::Entry {
            key: key.trim(),
            value: value.trim(),
        },
        None => Line::Bare(body),
    }
}

impl ConfText {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.lines().map(str::to_string).collect(),
        }
    }

    /// The text of `path`; a file that does not exist is empty.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// The lines with their 1-based numbers.
    pub fn lines(&self) -> impl Iterator<Item = (usize, Line<'_>)> {
        self.lines
            .iter()
            .enumerate()
            .map(|(i, raw)| (i + 1, classify(raw)))
    }

    /// Give `key` the value `value`: the last line of the key is rewritten (its indentation and comment kept; the
    /// earlier ones are overridden by it when read), else a line is appended.
    pub fn set(&mut self, key: &str, value: &str) {
        let last = self
            .lines
            .iter()
            .rposition(|raw| matches!(classify(raw), Line::Entry { key: k, .. } if k == key));
        match last {
            Some(i) => {
                let raw = &self.lines[i];
                let indent = &raw[..raw.len() - raw.trim_start().len()];
                let comment = split_comment(raw).1;
                self.lines[i] = format!("{indent}{key} = {value}{comment}");
            }
            None => self.lines.push(format!("{key} = {value}")),
        }
    }

    /// Remove every line of `key`.
    pub fn remove(&mut self, key: &str) {
        self.lines
            .retain(|raw| !matches!(classify(raw), Line::Entry { key: k, .. } if k == key));
    }

    /// The file text: every line followed by a newline.
    pub fn to_text(&self) -> String {
        self.lines.iter().flat_map(|l| [l.as_str(), "\n"]).collect()
    }
}

/// How one key of a settings file is read and written.
pub struct Key<S> {
    pub name: &'static str,
    /// Apply the value of a line; `Err` is the problem, the settings unchanged.
    pub read: fn(&mut S, &str) -> Result<(), String>,
    /// The value as the file writes it; `None` when the setting is unset, which removes the key.
    pub write: fn(&S) -> Option<String>,
    /// Take this key's value from `from`, the last good settings.
    pub keep: fn(&mut S, &S),
}

/// `unset` with the entries of `text` applied in order. Unknown keys and values that do not parse are problems; with
/// `last_good`, such a key keeps its value from it. `bare` applies a line without `=` (`None`: a problem).
pub fn read<S: Clone>(
    text: &ConfText,
    keys: &[Key<S>],
    unset: &S,
    last_good: Option<&S>,
    bare: Option<fn(&mut S, &str)>,
) -> (S, Vec<Problem>) {
    let mut settings = unset.clone();
    let mut problems = Vec::new();
    for (line, kind) in text.lines() {
        match kind {
            Line::Blank => {}
            Line::Bare(word) => match bare {
                Some(apply) => apply(&mut settings, word),
                None => problems.push(Problem {
                    line,
                    message: format!("expected `key = value`, got {word:?}"),
                }),
            },
            Line::Entry { key, value } => match keys.iter().find(|k| k.name == key) {
                None => problems.push(Problem {
                    line,
                    message: format!("unknown setting {key:?}"),
                }),
                Some(k) => {
                    if let Err(e) = (k.read)(&mut settings, value) {
                        problems.push(Problem {
                            line,
                            message: format!("{key}: {e}"),
                        });
                        if let Some(good) = last_good {
                            (k.keep)(&mut settings, good);
                        }
                    }
                }
            },
        }
    }
    (settings, problems)
}

/// Change `text` from the settings it holds, `before`, to `after`: only the keys whose written value differs are
/// set or removed.
pub fn write<S>(text: &mut ConfText, keys: &[Key<S>], before: &S, after: &S) {
    for k in keys {
        let new = (k.write)(after);
        if (k.write)(before) == new {
            continue;
        }
        match new {
            Some(value) => text.set(k.name, &value),
            None => text.remove(k.name),
        }
    }
}

/// The settings of the file at `path` (a missing file: `unset`) and the problems of its lines, as [`read`].
pub fn load<S: Clone>(
    path: &Path,
    keys: &[Key<S>],
    unset: &S,
    last_good: Option<&S>,
    bare: Option<fn(&mut S, &str)>,
) -> io::Result<(S, Vec<Problem>)> {
    Ok(read(&ConfText::load(path)?, keys, unset, last_good, bare))
}

/// Store `settings` in the file at `path` atomically: the file is read, the changed keys rewritten and the result
/// written with [`crate::atomic::write_atomic`]. Returns the problems of the lines that were there before (kept as
/// they are).
pub fn store<S: Clone>(
    path: &Path,
    keys: &[Key<S>],
    unset: &S,
    bare: Option<fn(&mut S, &str)>,
    settings: &S,
) -> io::Result<Vec<Problem>> {
    let mut text = ConfText::load(path)?;
    let (before, problems) = read(&text, keys, unset, None, bare);
    write(&mut text, keys, &before, settings);
    crate::atomic::write_atomic(path, text.to_text().as_bytes())?;
    Ok(problems)
}

/// `on`/`off` and their spellings `true`/`false`, `yes`/`no`, `1`/`0`, any case.
pub fn parse_switch(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(format!("expected on or off, got {value:?}")),
    }
}

/// How a switch is written.
pub fn switch_text(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// A comma-separated list; empty items are dropped.
pub fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Demo {
        size: Option<u32>,
        shown: Option<bool>,
        words: Vec<String>,
    }

    const KEYS: &[Key<Demo>] = &[
        Key {
            name: "size",
            read: |s, v| {
                v.parse()
                    .map(|n| s.size = Some(n))
                    .map_err(|_| format!("expected a number, got {v:?}"))
            },
            write: |s| s.size.map(|n| n.to_string()),
            keep: |s, g| s.size = g.size,
        },
        Key {
            name: "shown",
            read: |s, v| parse_switch(v).map(|b| s.shown = Some(b)),
            write: |s| s.shown.map(switch_text),
            keep: |s, g| s.shown = g.shown,
        },
    ];

    #[test]
    fn lines_are_classified_without_their_comments() {
        let text = ConfText::parse("# heading\n\n  size = 48 # pt\nfoot\nshown=on\n");
        let lines: Vec<(usize, Line)> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                (1, Line::Blank),
                (2, Line::Blank),
                (
                    3,
                    Line::Entry {
                        key: "size",
                        value: "48"
                    }
                ),
                (4, Line::Bare("foot")),
                (
                    5,
                    Line::Entry {
                        key: "shown",
                        value: "on"
                    }
                )
            ]
        );
    }

    #[test]
    fn a_later_line_wins_and_problems_carry_line_numbers() {
        let text = ConfText::parse("size = 10\nsize = 20\nzoom = 3\nshown = maybe\nstray\n");
        let (s, problems) = read(&text, KEYS, &Demo::default(), None, None);
        assert_eq!(
            s,
            Demo {
                size: Some(20),
                shown: None,
                words: vec![]
            }
        );
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, vec![3, 4, 5]);
        assert_eq!(
            problems[1].message,
            "shown: expected on or off, got \"maybe\""
        );
    }

    #[test]
    fn a_bad_value_keeps_the_last_good_value_of_its_key_only() {
        let good = Demo {
            size: Some(48),
            shown: Some(true),
            words: vec![],
        };
        let text = ConfText::parse("size = big\n");
        let (s, problems) = read(&text, KEYS, &Demo::default(), Some(&good), None);
        assert_eq!(s.size, Some(48), "kept, not defaulted");
        assert_eq!(s.shown, None, "a key the file no longer sets is unset");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn bare_lines_go_to_their_handler() {
        let (s, problems) = read(
            &ConfText::parse("foot\nsize = 3\nfirefox\n"),
            KEYS,
            &Demo::default(),
            None,
            Some(|s: &mut Demo, w: &str| s.words.push(w.to_string())),
        );
        assert_eq!(s.words, ["foot", "firefox"]);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_write_changes_only_the_lines_of_changed_keys() {
        let original = "# my dock\nfoot\n  size = 48   # small\nshown = yes\nunknown_key = kept\n";
        let mut text = ConfText::parse(original);
        let (before, _) = read(
            &text,
            KEYS,
            &Demo::default(),
            None,
            Some(|_: &mut Demo, _: &str| {}),
        );
        let mut after = before.clone();
        after.size = Some(64);
        write(&mut text, KEYS, &before, &after);
        assert_eq!(
            text.to_text(),
            "# my dock\nfoot\n  size = 64   # small\nshown = yes\nunknown_key = kept\n",
            "`yes` stays: its value did not change"
        );
        let off = Demo {
            shown: Some(false),
            ..after.clone()
        };
        write(&mut text, KEYS, &after, &off);
        assert_eq!(
            text.to_text(),
            "# my dock\nfoot\n  size = 64   # small\nshown = off\nunknown_key = kept\n"
        );
        write(
            &mut text,
            KEYS,
            &off,
            &Demo {
                shown: None,
                ..off.clone()
            },
        );
        assert_eq!(
            text.to_text(),
            "# my dock\nfoot\n  size = 64   # small\nunknown_key = kept\n",
            "unset removes the key"
        );
    }

    #[test]
    fn set_rewrites_the_last_line_of_a_key_and_appends_a_new_key() {
        let mut text = ConfText::parse("size = 1\nsize = 2 # second\n");
        text.set("size", "3");
        text.set("shown", "on");
        assert_eq!(text.to_text(), "size = 1\nsize = 3 # second\nshown = on\n");
    }

    #[test]
    fn store_writes_the_changes_into_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.conf");
        std::fs::write(&path, "# keep me\nsize = 1\nbogus = 2\n").unwrap();
        let problems = store(
            &path,
            KEYS,
            &Demo::default(),
            None,
            &Demo {
                size: Some(5),
                shown: Some(true),
                words: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep me\nsize = 5\nbogus = 2\nshown = on\n"
        );
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![3],
            "the unknown line is reported and kept"
        );
    }
}
