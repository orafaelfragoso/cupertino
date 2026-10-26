//! The shared MIME database (freedesktop shared-mime-info specification) without a C library: file
//! names to types from `globs2`, aliases, the subclass tree and the icon names of a type.
//!
//! Only the text files `update-mime-database` writes are read (`globs2`, `aliases`, `subclasses`,
//! `icons`, `generic-icons`); `mime.cache` and `magic` are not. Types come from file names alone: no
//! content sniffing, so a file without a matching name has no type here and the caller decides
//! (`application/octet-stream` for a regular file, per the specification).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

/// The type every streamable type is a subclass of (shared-mime-info "Subclassing").
pub const OCTET_STREAM: &str = "application/octet-stream";
/// The type all `text/* types are subclasses of (same section).
pub const TEXT_PLAIN: &str = "text/plain";
/// The type of a directory (shared-mime-info "Non-regular files").
pub const DIRECTORY: &str = "inode/directory";

/// The pattern that, in `globs2` of a more important directory, discards the globs of a type defined in
/// less important ones.
const NO_GLOBS: &str = "__NOGLOBS__";

/// How a glob is matched, in the specification's recommended checking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GlobKind {
    /// No wildcard: the whole name (`Makefile`).
    Literal,
    /// `*` followed by a wildcard-free suffix (`*.tar.gz`).
    Suffix,
    /// Anything else (`README*`, `*.[ch]`).
    Pattern,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Glob {
    weight: u32,
    mime: String,
    /// Lower-cased unless `case_sensitive`.
    pattern: String,
    case_sensitive: bool,
    kind: GlobKind,
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

impl Glob {
    fn new(weight: u32, mime: &str, pattern: &str, case_sensitive: bool) -> Glob {
        let kind = match pattern.strip_prefix('*') {
            _ if !has_wildcard(pattern) => GlobKind::Literal,
            Some(rest) if !has_wildcard(rest) => GlobKind::Suffix,
            _ => GlobKind::Pattern,
        };
        let pattern = if case_sensitive { pattern.to_string() } else { pattern.to_lowercase() };
        Glob { weight, mime: mime.to_string(), pattern, case_sensitive, kind }
    }

    /// Whether `name` (and its lower-cased form `lower`) matches.
    fn matches(&self, name: &str, lower: &str) -> bool {
        let subject = if self.case_sensitive { name } else { lower };
        match self.kind {
            GlobKind::Literal => subject == self.pattern,
            GlobKind::Suffix => subject.ends_with(&self.pattern[1..]),
            GlobKind::Pattern => fnmatch(self.pattern.as_bytes(), subject.as_bytes()),
        }
    }
}

/// Shell glob matching of `*`, `?` and bracket expressions (`[abc]`, `[a-z]`, `[!x]`) over bytes, as the
/// patterns of `globs2` use them. A `[` without its `]` matches itself.
fn fnmatch(pattern: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n) = (0, 0);
    // the position after the last `*` and the name position it is retried from
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    star = Some((p + 1, n));
                    p += 1;
                    continue;
                }
                b'?' => {
                    p += 1;
                    n += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next)) = bracket(&pattern[p..], name[n]) {
                        if matched {
                            p += next;
                            n += 1;
                            continue;
                        }
                    } else if name[n] == b'[' {
                        p += 1;
                        n += 1;
                        continue;
                    }
                }
                c if c == name[n] => {
                    p += 1;
                    n += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, sn)) => {
                p = sp;
                n = sn + 1;
                star = Some((sp, sn + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Match `c` against the bracket expression at the start of `pattern`: whether it matched and the length
/// of the expression, or `None` when the bracket is not closed.
fn bracket(pattern: &[u8], c: u8) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        let lo = pattern[i];
        if lo == b']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if pattern.get(i + 1) == Some(&b'-') && pattern.get(i + 2).is_some_and(|&hi| hi != b']') {
            let hi = pattern[i + 2];
            matched |= lo <= c && c <= hi;
            i += 3;
        } else {
            matched |= lo == c;
            i += 1;
        }
    }
    None
}

/// A parse error of a database file: the file, the 1-based line and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub path: PathBuf,
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.path.display(), self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Why the database could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, io::Error),
    Parse(ParseError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            LoadError::Parse(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {}

/// Read a database file; a missing file is `None` (a data directory need not have every file).
fn read_optional(path: &Path) -> Result<Option<String>, LoadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LoadError::Io(path.to_path_buf(), e)),
    }
}

/// The non-comment lines of a database file with their 1-based numbers.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().map(|(i, l)| (i + 1, l.trim_end())).filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

/// `a b` lines (`aliases`, `subclasses`) or `a:b` lines (`icons`, `generic-icons`).
fn pairs<'t>(text: &'t str, path: &Path, separator: char) -> Result<Vec<(&'t str, &'t str)>, LoadError> {
    lines(text)
        .map(|(n, l)| {
            l.split_once(separator)
                .map(|(a, b)| (a.trim(), b.trim()))
                .filter(|(a, b)| !a.is_empty() && !b.is_empty())
                .ok_or_else(|| LoadError::Parse(ParseError { path: path.to_path_buf(), line: n, message: format!("expected two fields separated by {separator:?}") }))
        })
        .collect()
}

/// The shared MIME database of a set of data directories.
#[derive(Clone, Debug, Default)]
pub struct MimeDatabase {
    globs: Vec<Glob>,
    aliases: HashMap<String, String>,
    parents: HashMap<String, Vec<String>>,
    icons: HashMap<String, String>,
    generic_icons: HashMap<String, String>,
}

impl MimeDatabase {
    /// Load `<dir>/mime/*` of every XDG data directory in `data_dirs`, most important first. For aliases and
    /// icons the most important directory wins; subclass lists are merged; globs are merged unless a more
    /// important directory lists `__NOGLOBS__` for the type.
    pub fn load(data_dirs: &[PathBuf]) -> Result<MimeDatabase, LoadError> {
        let mut db = MimeDatabase::default();
        let mut no_globs: HashSet<String> = HashSet::new();
        for dir in data_dirs.iter().map(|d| d.join("mime")) {
            let mut cleared_here: Vec<String> = Vec::new();
            let globs_path = dir.join("globs2");
            if let Some(text) = read_optional(&globs_path)? {
                for (n, line) in lines(&text) {
                    let mut fields = line.splitn(4, ':');
                    let (Some(weight), Some(mime), Some(pattern)) = (fields.next(), fields.next(), fields.next()) else {
                        return Err(LoadError::Parse(ParseError { path: globs_path, line: n, message: "expected weight:type:pattern".into() }));
                    };
                    let weight: u32 = weight.parse().map_err(|_| LoadError::Parse(ParseError { path: globs_path.clone(), line: n, message: format!("weight {weight:?} is not a number") }))?;
                    if no_globs.contains(mime) {
                        continue;
                    }
                    // the less important directories come later, so their globs are skipped as they load
                    if pattern == NO_GLOBS {
                        cleared_here.push(mime.to_string());
                        continue;
                    }
                    let case_sensitive = fields.next().is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
                    db.globs.push(Glob::new(weight, mime, pattern, case_sensitive));
                }
            }
            no_globs.extend(cleared_here);
            let path = dir.join("aliases");
            if let Some(text) = read_optional(&path)? {
                for (alias, canonical) in pairs(&text, &path, ' ')? {
                    db.aliases.entry(alias.to_string()).or_insert_with(|| canonical.to_string());
                }
            }
            let path = dir.join("subclasses");
            if let Some(text) = read_optional(&path)? {
                for (child, parent) in pairs(&text, &path, ' ')? {
                    let list = db.parents.entry(child.to_string()).or_default();
                    if !list.iter().any(|p| p == parent) {
                        list.push(parent.to_string());
                    }
                }
            }
            for (name, map) in [("icons", &mut db.icons), ("generic-icons", &mut db.generic_icons)] {
                let path = dir.join(name);
                if let Some(text) = read_optional(&path)? {
                    for (mime, icon) in pairs(&text, &path, ':')? {
                        map.entry(mime.to_string()).or_insert_with(|| icon.to_string());
                    }
                }
            }
        }
        Ok(db)
    }

    /// The canonical name of `mime` (an alias resolves to its type; any other name is itself).
    pub fn canonical<'a>(&'a self, mime: &'a str) -> &'a str {
        self.aliases.get(mime).map_or(mime, String::as_str)
    }

    /// The type of a file name from its globs, canonical; `None` when no glob matches. Checking order of the
    /// specification: literal names first, then the suffix and wildcard patterns; among the matches of a
    /// class the highest weight wins, then the longest pattern. Names that tie on both are ambiguous, and
    /// the specification resolves them by content, which is not read here: the first in database order
    /// (the order `update-mime-database` sorted the file in) is taken.
    pub fn type_for_name(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        let best = |literal: bool| {
            self.globs
                .iter()
                .filter(|g| (g.kind == GlobKind::Literal) == literal && g.matches(name, &lower))
                .fold(None::<&Glob>, |best, g| match best {
                    Some(b) if (b.weight, b.pattern.len()) >= (g.weight, g.pattern.len()) => Some(b),
                    _ => Some(g),
                })
        };
        best(true).or_else(|| best(false)).map(|g| self.canonical(&g.mime))
    }

    /// Direct parents of `mime` (canonical): the `subclasses` entries in file order, then the implicit ones,
    /// `text/plain` for a `text/*` type and `application/octet-stream` for every streamable type (all but
    /// `inode/*` and the `x-content/* volume types; shared-mime-info "Subclassing").
    pub fn parents(&self, mime: &str) -> Vec<String> {
        let mime = self.canonical(mime);
        let mut out: Vec<String> = self.parents.get(mime).map(|ps| ps.iter().map(|p| self.canonical(p).to_string()).collect()).unwrap_or_default();
        let mut implicit = |p: &str| {
            if mime != p && !out.iter().any(|o| o == p) {
                out.push(p.to_string());
            }
        };
        if mime.starts_with("text/") {
            implicit(TEXT_PLAIN);
        }
        if is_streamable(mime) {
            implicit(OCTET_STREAM);
        }
        out
    }

    /// `mime` and all its ancestors, nearest first (breadth first over [`MimeDatabase::parents`]), with
    /// `application/octet-stream`, the root of every streamable type, always last.
    pub fn ancestors(&self, mime: &str) -> Vec<String> {
        let mut out = vec![self.canonical(mime).to_string()];
        let mut i = 0;
        while i < out.len() {
            for p in self.parents(&out[i]) {
                if p != OCTET_STREAM && !out.contains(&p) {
                    out.push(p);
                }
            }
            i += 1;
        }
        if is_streamable(&out[0]) && out[0] != OCTET_STREAM {
            out.push(OCTET_STREAM.to_string());
        }
        out
    }

    /// Whether `mime` is `ancestor` or one of its subclasses.
    pub fn is_a(&self, mime: &str, ancestor: &str) -> bool {
        let ancestor = self.canonical(ancestor);
        self.ancestors(mime).iter().any(|a| a == ancestor)
    }

    /// Icon name of a type: its `icons` entry, else the type with `/` replaced by `-
    /// (shared-mime-info "Icons").
    pub fn icon_name(&self, mime: &str) -> String {
        let mime = self.canonical(mime);
        self.icons.get(mime).cloned().unwrap_or_else(|| mime.replace('/', "-"))
    }

    /// The `generic-icons` entry of a type, if it has one.
    pub fn generic_icon(&self, mime: &str) -> Option<&str> {
        self.generic_icons.get(self.canonical(mime)).map(String::as_str)
    }

    /// Generic icon name of a type: its `generic-icons` entry, else `<media>-x-generic` (same
    /// section).
    pub fn generic_icon_name(&self, mime: &str) -> String {
        match self.generic_icon(mime) {
            Some(icon) => icon.to_string(),
            None => media_generic_icon(self.canonical(mime)),
        }
    }
}

/// `<media>-x-generic`, the generic icon of a type without a `generic-icons` entry.
pub fn media_generic_icon(mime: &str) -> String {
    format!("{}-x-generic", mime.split_once('/').map_or(mime, |(media, _)| media))
}

/// Every type but the non-regular `inode/*` types and the `x-content/*` volume types has a byte stream.
fn is_streamable(mime: &str) -> bool {
    !mime.starts_with("inode/") && !mime.starts_with("x-content/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(files: &[(&str, &str)]) -> (tempfile::TempDir, MimeDatabase) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mime")).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join("mime").join(name), text).unwrap();
        }
        let db = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap();
        (dir, db)
    }

    const GLOBS: &str = "# comment\n\
        80:text/x-makefile:Makefile:cs\n\
        50:application/x-compressed-tar:*.tar.gz\n\
        50:application/gzip:*.gz\n\
        50:text/x-csrc:*.c:cs\n\
        50:text/x-c++src:*.C:cs\n\
        50:image/png:*.png\n\
        10:text/x-readme:readme*\n\
        50:text/x-chdr:*.[hH]\n\
        50:application/x-legacy:*.leg\n";

    #[test]
    fn literal_names_win_then_the_longest_suffix() {
        let (_d, db) = db(&[("globs2", GLOBS)]);
        assert_eq!(db.type_for_name("Makefile"), Some("text/x-makefile"));
        assert_eq!(db.type_for_name("makefile"), None, "a case-sensitive literal");
        assert_eq!(db.type_for_name("a.tar.gz"), Some("application/x-compressed-tar"));
        assert_eq!(db.type_for_name("a.gz"), Some("application/gzip"));
        assert_eq!(db.type_for_name("PHOTO.PNG"), Some("image/png"), "globs without cs ignore case");
        assert_eq!(db.type_for_name("x.c"), Some("text/x-csrc"));
        assert_eq!(db.type_for_name("x.C"), Some("text/x-c++src"));
        assert_eq!(db.type_for_name("README.md"), Some("text/x-readme"));
        assert_eq!(db.type_for_name("x.H"), Some("text/x-chdr"));
        assert_eq!(db.type_for_name("x.unknown"), None);
    }

    #[test]
    fn a_higher_weight_beats_a_longer_pattern() {
        let (_d, db) = db(&[("globs2", "60:application/x-a:*.b\n50:application/x-ab:*.a.b\n")]);
        assert_eq!(db.type_for_name("x.a.b"), Some("application/x-a"));
    }

    #[test]
    fn aliases_resolve_to_the_canonical_type() {
        let (_d, db) = db(&[("globs2", GLOBS), ("aliases", "application/x-legacy application/x-modern\n")]);
        assert_eq!(db.type_for_name("f.leg"), Some("application/x-modern"));
        assert_eq!(db.canonical("application/x-legacy"), "application/x-modern");
        assert_eq!(db.canonical("image/png"), "image/png");
    }

    #[test]
    fn ancestry_is_explicit_then_implicit_with_octet_stream_last() {
        let (_d, db) = db(&[("subclasses", "application/x-shellscript application/x-executable\napplication/x-shellscript text/plain\ntext/x-csrc text/plain\napplication/xml text/plain\nimage/svg+xml application/xml\n")]);
        assert_eq!(db.ancestors("application/x-shellscript"), ["application/x-shellscript", "application/x-executable", "text/plain", "application/octet-stream"]);
        assert_eq!(db.ancestors("text/x-rust"), ["text/x-rust", "text/plain", "application/octet-stream"], "text/* is implicitly text/plain");
        assert_eq!(db.ancestors("image/svg+xml"), ["image/svg+xml", "application/xml", "text/plain", "application/octet-stream"]);
        assert_eq!(db.ancestors(DIRECTORY), [DIRECTORY], "a directory is not a byte stream");
        assert_eq!(db.ancestors(OCTET_STREAM), [OCTET_STREAM]);
        assert!(db.is_a("text/x-csrc", "text/plain"));
        assert!(!db.is_a("image/png", "text/plain"));
    }

    #[test]
    fn icon_names_follow_the_icons_files_then_the_type_name() {
        let (_d, db) = db(&[("icons", "application/x-special:special-icon\n"), ("generic-icons", "application/zip:package-x-generic\ninode/directory:folder\n")]);
        assert_eq!(db.icon_name("application/x-special"), "special-icon");
        assert_eq!(db.icon_name("image/png"), "image-png");
        assert_eq!(db.generic_icon_name("application/zip"), "package-x-generic");
        assert_eq!(db.generic_icon_name("image/png"), "image-x-generic");
        assert_eq!(db.generic_icon_name(DIRECTORY), "folder");
        assert_eq!(db.generic_icon("image/png"), None);
    }

    #[test]
    fn noglobs_in_a_more_important_directory_drops_lower_globs() {
        let user = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        for d in [&user, &system] {
            std::fs::create_dir_all(d.path().join("mime")).unwrap();
        }
        std::fs::write(user.path().join("mime/globs2"), "50:text/x-a:__NOGLOBS__\n50:text/x-a:*.aa\n").unwrap();
        std::fs::write(system.path().join("mime/globs2"), "50:text/x-a:*.a\n50:text/x-b:*.b\n").unwrap();
        let db = MimeDatabase::load(&[user.path().to_path_buf(), system.path().to_path_buf()]).unwrap();
        assert_eq!(db.type_for_name("x.a"), None);
        assert_eq!(db.type_for_name("x.aa"), Some("text/x-a"));
        assert_eq!(db.type_for_name("x.b"), Some("text/x-b"));
    }

    #[test]
    fn malformed_lines_are_errors_with_their_position() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mime")).unwrap();
        std::fs::write(dir.path().join("mime/globs2"), "50:text/plain:*.txt\nnonsense\n").unwrap();
        match MimeDatabase::load(&[dir.path().to_path_buf()]) {
            Err(LoadError::Parse(e)) => assert_eq!(e.line, 2),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn glob_matching() {
        assert!(fnmatch(b"*.[ch]", b"x.c"));
        assert!(!fnmatch(b"*.[!ch]", b"x.c"));
        assert!(fnmatch(b"[a-c]?z*", b"bqzzz"));
        assert!(fnmatch(b"a*b*c", b"aXbYbc"));
        assert!(!fnmatch(b"a*b", b"ab_"));
        assert!(fnmatch(b"x[", b"x["));
    }
}
