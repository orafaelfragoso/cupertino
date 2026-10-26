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
            ignored_dir_names: ["node_modules", "target", "__pycache__", "venv", "site-packages", "vendor", "build", "dist", "Trash", "snap", "Library"].map(String::from).to_vec(),
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
                    go(r, t) || t.iter().enumerate().any(|(k, &c)| c == '/' && go(r, &t[k + 1..]))
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
                let Some(end) = p.iter().skip(2).position(|&c| c == ']').map(|e| e + 2) else { return t.first() == Some(&'[') && go(&p[1..], &t[1..]) };
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
            patterns.push(Pattern { glob: line.chars().collect(), negated, dir_only, anchored });
        }
        IgnoreRules { base: base.to_path_buf(), patterns }
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
            let hit = if p.anchored { glob_match(&p.glob, &rel_s) } else { glob_match(&p.glob, &name) };
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
    meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
}

impl FileIndex {
    pub fn empty(config: FileSearchConfig) -> FileIndex {
        FileIndex { config, entries: Vec::new(), truncated: false, dirs_read: 0 }
    }

    pub fn build(config: FileSearchConfig) -> FileIndex {
        let started = Instant::now();
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut dirs_read = 0;
        let mut queue: VecDeque<(PathBuf, usize, Vec<Arc<IgnoreRules>>)> = VecDeque::new();
        let mut root_rules = Vec::new();
        if !config.extra_patterns.is_empty() {
            root_rules.push(Arc::new(IgnoreRules::parse(&config.root, &config.extra_patterns.join("\n"))));
        }
        queue.push_back((config.root.clone(), 0, root_rules));
        'walk: while let Some((dir, depth, mut rules)) = queue.pop_front() {
            if dirs_read >= config.max_dirs || config.time_budget.is_some_and(|b| started.elapsed() > b) {
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
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
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
                let ignored = rules.iter().fold(None, |acc, r| r.decision(&path, is_dir).or(acc)).unwrap_or(false);
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
                let parent_lower = fold_lower(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                entries.push(FileEntry { lower: fold_lower(&name), parent_lower, name, path, is_dir, depth: depth.min(255) as u8, modified });
                if entries.len() >= config.max_entries {
                    truncated = true;
                    break 'walk;
                }
            }
        }
        if !queue.is_empty() {
            truncated = true;
        }
        FileIndex { config, entries, truncated, dirs_read }
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
            if !q.tokens.iter().all(|t| subsequence(&e.lower, t) || subsequence(&e.parent_lower, t)) {
                continue;
            }
            let parent = e.path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let cand = Candidate { fields: vec![CandidateField::new(&e.name, 1.0, true), CandidateField::new(&parent, 0.4, false)], required_field: None };
            let Some(s) = matcher::score(q, &cand, w) else { continue };
            if s.score < self.config.min_score {
                continue;
            }
            let age_days = ((now - e.modified) / 86400.0).max(0.0);
            let recency = if e.modified > 0.0 { 15.0 * 0.5f64.powf(age_days / 14.0) } else { 0.0 };
            let score = s.score - 4.0 * (e.depth as f64).min(6.0) + recency;
            out.push(FileMatch { index: i, score, positions: s.positions });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| self.entries[a.index].depth.cmp(&self.entries[b.index].depth)).then_with(|| self.entries[a.index].name.cmp(&self.entries[b.index].name)));
        out.truncate(limit);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    fn names(idx: &FileIndex) -> Vec<String> {
        let root = &idx.config.root;
        let mut v: Vec<String> = idx.entries().iter().map(|e| e.path.strip_prefix(root).unwrap().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn g(p: &str, t: &str) -> bool {
        glob_match(&p.chars().collect::<Vec<_>>(), &t.chars().collect::<Vec<_>>())
    }

    #[test]
    fn glob_basics() {
        assert!(g("*.rs", "main.rs"));
        assert!(!g("*.rs", "main.rc"));
        assert!(g("a?c", "abc"));
        assert!(!g("a?c", "a/c"));
        assert!(!g("*.rs", "src/main.rs"));
        assert!(g("src/*.rs", "src/main.rs"));
        assert!(g("**/test", "a/b/test"));
        assert!(g("**/test", "test"));
        assert!(g("a/**/z", "a/b/c/z"));
        assert!(g("a/**/z", "a/z"));
        assert!(g("docs/**", "docs/a/b.md"));
        assert!(g("[abc]x", "bx"));
        assert!(!g("[!abc]x", "bx"));
        assert!(g("[a-c]x", "cx"));
        assert!(g("\\*star", "*star"));
        assert!(g("*", ""));
    }

    #[test]
    fn walker_skips_hidden_heavy_and_ignored_entries() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(r, "Documents/report.txt");
        touch(r, "Documents/.secret");
        touch(r, ".config/app.conf");
        touch(r, "proj/node_modules/pkg/index.js");
        touch(r, "proj/src/main.rs");
        touch(r, "proj/target/debug/out");
        touch(r, "proj/.gitignore");
        fs::write(r.join("proj/.gitignore"), "*.log\n/generated/\nsecrets.txt\n!keep.log\n").unwrap();
        touch(r, "proj/debug.log");
        touch(r, "proj/keep.log");
        touch(r, "proj/generated/code.rs");
        touch(r, "proj/src/secrets.txt");
        touch(r, "proj/src/generated/ok.rs");
        let idx = FileIndex::build(FileSearchConfig::new(r));
        let n = names(&idx);
        assert!(n.contains(&"Documents/report.txt".to_string()));
        assert!(n.contains(&"proj/src/main.rs".to_string()));
        assert!(n.contains(&"proj/keep.log".to_string()), "negation re-includes: {n:?}");
        assert!(n.contains(&"proj/src/generated/ok.rs".to_string()), "anchored pattern only matches at the base");
        for hidden in [".config", "Documents/.secret"] {
            assert!(!n.iter().any(|x| x.starts_with(hidden)), "{hidden}");
        }
        for ignored in ["proj/node_modules", "proj/target", "proj/debug.log", "proj/generated", "proj/src/secrets.txt"] {
            assert!(!n.iter().any(|x| x.starts_with(ignored)), "{ignored} in {n:?}");
        }
        assert!(!idx.truncated);
    }

    #[test]
    fn hidden_and_ignore_files_can_be_enabled() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".hidden/file.txt");
        touch(d.path(), "a.log");
        fs::write(d.path().join(".gitignore"), "*.log\n").unwrap();
        let mut c = FileSearchConfig::new(d.path());
        c.include_hidden = true;
        c.respect_ignore_files = false;
        let n = names(&FileIndex::build(c));
        assert!(n.contains(&".hidden/file.txt".to_string()) && n.contains(&"a.log".to_string()));
    }

    #[test]
    fn extra_patterns_and_dir_exclusion() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a.tmp");
        touch(d.path(), "b.txt");
        touch(d.path(), "sub/c.txt");
        let mut c = FileSearchConfig::new(d.path());
        c.extra_patterns = vec!["*.tmp".into()];
        c.include_dirs = false;
        let n = names(&FileIndex::build(c));
        assert_eq!(n, vec!["b.txt", "sub/c.txt"]);
    }

    #[test]
    fn bounds_are_respected() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..20 {
            touch(d.path(), &format!("f{i:02}.txt"));
        }
        touch(d.path(), "a/b/c/d/deep.txt");
        let mut c = FileSearchConfig::new(d.path());
        c.max_entries = 5;
        let idx = FileIndex::build(c);
        assert_eq!(idx.len(), 5);
        assert!(idx.truncated);

        let mut c = FileSearchConfig::new(d.path());
        c.max_depth = 2;
        let n = names(&FileIndex::build(c));
        assert!(n.contains(&"a/b".to_string()), "dirs at depth 2 are listed");
        assert!(!n.iter().any(|x| x.contains("deep.txt")), "depth bound: {n:?}");

        let mut c = FileSearchConfig::new(d.path());
        c.max_dirs = 1;
        let idx = FileIndex::build(c);
        assert!(idx.truncated && idx.dirs_read == 1);

        let mut c = FileSearchConfig::new(d.path());
        c.time_budget = Some(Duration::ZERO);
        std::thread::sleep(Duration::from_millis(2));
        let idx = FileIndex::build(c);
        assert!(idx.truncated || idx.dirs_read <= 1);
    }

    #[test]
    fn breadth_first_keeps_shallow_files_under_a_budget() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "top.txt");
        touch(d.path(), "x/y/z/deep.txt");
        let mut c = FileSearchConfig::new(d.path());
        c.max_entries = 2;
        let n = names(&FileIndex::build(c));
        assert!(n.contains(&"top.txt".to_string()));
        assert!(!n.iter().any(|x| x.contains("deep")));
    }

    #[test]
    fn symlinked_directories_are_not_followed() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "real/inside.txt");
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("link")).unwrap();
        let n = names(&FileIndex::build(FileSearchConfig::new(d.path())));
        assert!(n.contains(&"real/inside.txt".to_string()));
        assert!(!n.contains(&"link/inside.txt".to_string()));
    }

    #[test]
    fn search_ranks_names_depth_and_recency() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "Documents/budget-2024.xlsx");
        touch(d.path(), "a/b/c/budget.txt");
        touch(d.path(), "budget.txt");
        touch(d.path(), "unrelated.md");
        touch(d.path(), "Photos/Beach/holiday.jpg");
        let idx = FileIndex::build(FileSearchConfig::new(d.path()));
        let w = Weights::default();
        let r = idx.search(&Query::new("budget"), 10, crate::frecency::unix_now(), &w);
        let found: Vec<String> = r.iter().map(|m| idx.entry(m.index).path.strip_prefix(d.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0], "budget.txt", "exact-prefix shallow first");
        assert!(found.contains(&"Documents/budget-2024.xlsx".to_string()));
        assert!(r[0].score > r[2].score);
        assert_eq!(idx.search(&Query::new("budget"), 1, 0.0, &w).len(), 1);
        assert!(idx.search(&Query::new("zzz"), 10, 0.0, &w).is_empty());
        assert!(idx.search(&Query::new(""), 10, 0.0, &w).is_empty());
        let by_dir = idx.search(&Query::new("beach holiday"), 10, 0.0, &w);
        assert_eq!(by_dir.len(), 1, "parent folder name is a secondary field");
        let bud = idx.search(&Query::new("BUDG"), 10, 0.0, &w);
        assert_eq!(bud.len(), 3, "case-insensitive");
        assert_eq!(bud[0].positions, vec![0, 1, 2, 3]);
    }

    #[test]
    fn fuzzy_only_matches_are_filtered_out() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "quarterly-report.txt");
        let idx = FileIndex::build(FileSearchConfig::new(d.path()));
        let w = Weights::default();
        assert!(idx.search(&Query::new("qly"), 10, 0.0, &w).is_empty());
        assert_eq!(idx.search(&Query::new("qrt"), 10, 0.0, &w).len(), 1, "initials are a better tier than fuzzy");
        assert_eq!(idx.search(&Query::new("rep"), 10, 0.0, &w).len(), 1);
    }

    #[test]
    fn missing_root_gives_empty_index() {
        let idx = FileIndex::build(FileSearchConfig::new("/nonexistent/root/for/test"));
        assert!(idx.is_empty());
    }

    #[test]
    fn background_build() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a.txt");
        let h = FileIndex::spawn_build(FileSearchConfig::new(d.path()));
        assert_eq!(h.join().unwrap().len(), 1);
    }
}
