//! File search provider: a bounded breadth-first walk of a root directory (normally `$HOME`)
//! builds an in-memory name index that queries are matched against.
//!
//! Bounds (all configurable, defaults approximate): maximum depth, maximum number of indexed
//! entries, maximum number of directories read and an optional time budget. The walk is
//! breadth-first, so shallow files are indexed before the budget runs out. Ignore rules: hidden
//! entries, a list of heavy directory names (build outputs, package caches), `.gitignore` and
//! `.ignore` files found on the way (patterns, negation, directory-only, anchored and `** globs)
//! and extra patterns from the configuration. Symbolic links are never followed.
//!
//! Building is blocking; run it on a thread ([`FileIndex::spawn_build`]) and hand the finished
//! index to the launcher.

use crate::matcher::{self, fold_char, Candidate, CandidateField, Query, Weights};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug)]
pub struct FileSearchConfig {
    pub root: PathBuf,
    pub max_depth: usize,
    pub max_entries: usize,
    pub max_dirs: usize,
    pub time_budget: Option<Duration>,
    pub include_hidden: bool,
    pub include_dirs: bool,
    pub respect_ignore_files: bool,
    /// Directory names never entered.
    pub ignored_dir_names: Vec<String>,
    /// Extra gitignore-style patterns applied relative to `root`.
    pub extra_patterns: Vec<String>,
    /// Entries scoring below this are not returned (300 = substring matches and better).
    pub min_score: f64,
}

impl FileSearchConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_depth: 6,
            max_entries: 200_000,
            max_dirs: 40_000,
            time_budget: Some(Duration::from_secs(5)),
            include_hidden: false,
            include_dirs: true,
            respect_ignore_files: true,
            ignored_dir_names: [
                "node_modules",
                "target",
                "__pycache__",
                "venv",
                "site-packages",
                "vendor",
                "build",
                "dist",
                "Trash",
                "snap",
                "Library",
            ]
            .map(String::from)
            .to_vec(),
            extra_patterns: Vec::new(),
            min_score: 300.0,
        }
    }

    pub fn home() -> Option<Self> {
        std::env::var_os("HOME").map(|h| Self::new(PathBuf::from(h)))
    }
}

#[derive(Clone, Debug)]
struct Pattern {
    glob: Vec<char>,
    negated: bool,
    dir_only: bool,
    anchored: bool,
}

#[derive(Clone, Debug, Default)]
struct IgnoreRules {
    base: PathBuf,
    patterns: Vec<Pattern>,
}

/// Glob matching with `*`, `?`, `[set]`, `**` and `\` escapes. `*` and `?` do not cross `/`.
pub fn glob_match(pattern: &[char], text: &[char]) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') if p.get(1) == Some(&'*') => {
                let rest = &p[2..];
                if rest.is_empty() {
                    return true;
                }
                if rest[0] == '/' {
                    let r = &rest[1..];
                    go(r, t)
                        || t.iter()
                            .enumerate()
                            .any(|(k, &c)| c == '/' && go(r, &t[k + 1..]))
                } else {
                    (0..=t.len()).any(|i| go(rest, &t[i..]))
                }
            }
            Some('*') => {
                let mut i = 0;
                loop {
                    if go(&p[1..], &t[i..]) {
                        return true;
                    }
                    if i >= t.len() || t[i] == '/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some('?') => t.first().is_some_and(|&c| c != '/') && go(&p[1..], &t[1..]),
            Some('[') => {
                let Some(end) = p.iter().skip(2).position(|&c| c == ']').map(|e| e + 2) else {
                    return t.first() == Some(&'[') && go(&p[1..], &t[1..]);
                };
                let Some(&c) = t.first() else { return false };
                let (neg, set) = match p.get(1) {
                    Some('!' | '^') => (true, &p[2..end]),
                    _ => (false, &p[1..end]),
                };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= set[i] <= c && c <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == c;
                        i += 1;
                    }
                }
                hit != neg && c != '/' && go(&p[end + 1..], &t[1..])
            }
            Some('\\') if p.len() > 1 => t.first() == Some(&p[1]) && go(&p[2..], &t[1..]),
            Some(&c) => t.first() == Some(&c) && go(&p[1..], &t[1..]),
        }
    }
    go(pattern, text)
}

impl IgnoreRules {
    fn parse(base: &Path, text: &str) -> IgnoreRules {
        let mut patterns = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches(['\r', ' ']);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, line) = match line.strip_prefix('!') {
                Some(r) => (true, r),
                None => (false, line),
            };
            let (dir_only, line) = match line.strip_suffix('/') {
                Some(r) => (true, r),
                None => (false, line),
            };
            if line.is_empty() {
                continue;
            }
            let anchored = line.contains('/');
            let line = line.trim_start_matches('/');
            patterns.push(Pattern {
                glob: line.chars().collect(),
                negated,
                dir_only,
                anchored,
            });
        }
        IgnoreRules {
            base: base.to_path_buf(),
            patterns,
        }
    }

    /// `Some(true)` ignored, `Some(false)` explicitly re-included, `None` no pattern applies.
    fn decision(&self, path: &Path, is_dir: bool) -> Option<bool> {
        let rel = path.strip_prefix(&self.base).ok()?;
        let rel_s: Vec<char> = rel.to_string_lossy().chars().collect();
        let name: Vec<char> = path.file_name()?.to_string_lossy().chars().collect();
        let mut result = None;
        for p in &self.patterns {
            if p.dir_only && !is_dir {
                continue;
            }
            let hit = if p.anchored {
                glob_match(&p.glob, &rel_s)
            } else {
                glob_match(&p.glob, &name)
            };
            if hit {
                result = Some(!p.negated);
            }
        }
        result
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: u8,
    /// Seconds since the Unix epoch.
    pub modified: f64,
    lower: String,
    parent_lower: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileMatch {
    pub index: usize,
    pub score: f64,
    pub positions: Vec<usize>,
}

#[derive(Debug)]
pub struct FileIndex {
    pub config: FileSearchConfig,
    entries: Vec<FileEntry>,
    /// The walk stopped because a bound was reached.
    pub truncated: bool,
    pub dirs_read: usize,
}

fn fold_lower(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

fn mtime(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

impl FileIndex {
    pub fn empty(config: FileSearchConfig) -> FileIndex {
        FileIndex {
            config,
            entries: Vec::new(),
            truncated: false,
            dirs_read: 0,
        }
    }

    pub fn build(config: FileSearchConfig) -> FileIndex {
        let started = Instant::now();
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut dirs_read = 0;
        let mut queue: VecDeque<(PathBuf, usize, Vec<Arc<IgnoreRules>>)> = VecDeque::new();
        let mut root_rules = Vec::new();
        if !config.extra_patterns.is_empty() {
            root_rules.push(Arc::new(IgnoreRules::parse(
                &config.root,
                &config.extra_patterns.join("\n"),
            )));
        }
        queue.push_back((config.root.clone(), 0, root_rules));
        'walk: while let Some((dir, depth, mut rules)) = queue.pop_front() {
            if dirs_read >= config.max_dirs
                || config.time_budget.is_some_and(|b| started.elapsed() > b)
            {
                truncated = true;
                break;
            }
            dirs_read += 1;
            if config.respect_ignore_files {
                for f in [".gitignore", ".ignore"] {
                    if let Ok(t) = std::fs::read_to_string(dir.join(f)) {
                        rules.push(Arc::new(IgnoreRules::parse(&dir, &t)));
                    }
                }
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<_> = rd.flatten().collect();
            children.sort_by_key(|e| e.file_name());
            for e in children {
                let name = e.file_name().to_string_lossy().into_owned();
                if !config.include_hidden && name.starts_with('.') {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                let is_dir = ft.is_dir();
                if is_dir && config.ignored_dir_names.contains(&name) {
                    continue;
                }
                let path = e.path();
                let ignored = rules
                    .iter()
                    .fold(None, |acc, r| r.decision(&path, is_dir).or(acc))
                    .unwrap_or(false);
                if ignored {
                    continue;
                }
                if !is_dir && !ft.is_file() && !ft.is_symlink() {
                    continue;
                }
                if is_dir && depth + 1 < config.max_depth {
                    queue.push_back((path.clone(), depth + 1, rules.clone()));
                }
                if is_dir && !config.include_dirs {
                    continue;
                }
                let modified = e.metadata().map(|m| mtime(&m)).unwrap_or(0.0);
                let parent_lower = fold_lower(
                    &dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                entries.push(FileEntry {
                    lower: fold_lower(&name),
                    parent_lower,
                    name,
                    path,
                    is_dir,
                    depth: depth.min(255) as u8,
                    modified,
                });
                if entries.len() >= config.max_entries {
                    truncated = true;
                    break 'walk;
                }
            }
        }
        if !queue.is_empty() {
            truncated = true;
        }
        FileIndex {
            config,
            entries,
            truncated,
            dirs_read,
        }
    }

    pub fn spawn_build(config: FileSearchConfig) -> std::thread::JoinHandle<FileIndex> {
        std::thread::spawn(move || FileIndex::build(config))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, i: usize) -> &FileEntry {
        &self.entries[i]
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Best `limit` matches of `q`. `now`: Unix seconds, for the small recency bonus.
    pub fn search(&self, q: &Query, limit: usize, now: f64, w: &Weights) -> Vec<FileMatch> {
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut out: Vec<FileMatch> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let subsequence = |hay: &str, t: &[char]| {
                let mut it = hay.chars();
                t.iter().all(|c| it.any(|h| h == *c))
            };
            if !q
                .tokens
                .iter()
                .all(|t| subsequence(&e.lower, t) || subsequence(&e.parent_lower, t))
            {
                continue;
            }
            let parent = e
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let cand = Candidate {
                fields: vec![
                    CandidateField::new(&e.name, 1.0, true),
                    CandidateField::new(&parent, 0.4, false),
                ],
                required_field: None,
            };
            let Some(s) = matcher::score(q, &cand, w) else {
                continue;
            };
            if s.score < self.config.min_score {
                continue;
            }
            let age_days = ((now - e.modified) / 86400.0).max(0.0);
            let recency = if e.modified > 0.0 {
                15.0 * 0.5f64.powf(age_days / 14.0)
            } else {
                0.0
            };
            let score = s.score - 4.0 * (e.depth as f64).min(6.0) + recency;
            out.push(FileMatch {
                index: i,
                score,
                positions: s.positions,
            });
        }
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| {
                    self.entries[a.index]
                        .depth
                        .cmp(&self.entries[b.index].depth)
                })
                .then_with(|| self.entries[a.index].name.cmp(&self.entries[b.index].name))
        });
        out.truncate(limit);
        out
    }
}
