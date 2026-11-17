//! Vendored trees kept as an upstream release plus a patch series.
//!
//! Each tree holds `PATCHES.md`, which lists the series in order, `patches/` with one unified diff per change, and
//! `UPSTREAM.sha256`, the sha256 of every file of the upstream release in `sha256sum` format. The check reads the tree
//! as git would track it into memory, reverses the patches from the last to the first, and compares the result with
//! the recorded hashes. It fetches nothing and writes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The vendored trees, relative to the repository root.
pub const TREES: [&str; 1] = ["third-party/smithay"];

/// The series' own files in a tree, which are not part of the upstream release.
const LIST: &str = "PATCHES.md";
const HASHES: &str = "UPSTREAM.sha256";
const PATCH_DIRECTORY: &str = "patches/";

/// Whether a change to `path` (relative to the repository root) needs the check.
pub fn touches(path: &str) -> bool {
    TREES.iter().any(|tree| {
        path.strip_prefix(tree)
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The problems of every vendored tree under `root`; empty when each one reverses to its upstream release.
pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let visible = crate::gate::visible_paths(root)?;
    let mut problems = Vec::new();
    for tree in TREES {
        problems.extend(
            tree_problems(root, tree, &visible)
                .unwrap_or_else(|error| vec![format!("{tree}: {error}")]),
        );
    }
    Ok(problems)
}

/// The summary line of a run.
pub fn summary(problems: &[String]) -> String {
    format!(
        "vendored patches: {} problems in {} trees",
        problems.len(),
        TREES.len()
    )
}

fn tree_problems(
    root: &Path,
    tree: &str,
    visible: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let prefix = format!("{tree}/");
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut patch_names = Vec::new();
    for path in visible {
        let Some(rel) = path.strip_prefix(&prefix) else {
            continue;
        };
        let full = root.join(path);
        // A tracked file deleted in the working tree is not part of it.
        let Ok(metadata) = full.symlink_metadata() else {
            continue;
        };
        if !metadata.is_file() {
            return Err(format!("{rel}: not a regular file"));
        }
        if rel == LIST || rel == HASHES {
            continue;
        }
        if let Some(name) = rel.strip_prefix(PATCH_DIRECTORY) {
            if name.contains('/') || !name.ends_with(".patch") {
                return Err(format!(
                    "{rel}: {PATCH_DIRECTORY} holds only `*.patch` files"
                ));
            }
            patch_names.push(name.to_owned());
            continue;
        }
        let data = std::fs::read(&full).map_err(|error| format!("{rel}: {error}"))?;
        files.insert(rel.to_owned(), data);
    }
    patch_names.sort();

    let mut problems = Vec::new();
    let list = std::fs::read_to_string(root.join(tree).join(LIST))
        .map_err(|error| format!("{LIST}: {error}"))?;
    let listed = listed_patches(&list);
    if listed != patch_names {
        problems.push(format!(
            "{tree}/{LIST}: lists the patches {listed:?} in this order, but {PATCH_DIRECTORY} holds {patch_names:?}"
        ));
    }

    for name in patch_names.iter().rev() {
        let path = format!("{PATCH_DIRECTORY}{name}");
        let text = std::fs::read(root.join(tree).join(&path))
            .map_err(|error| format!("{path}: {error}"))?;
        let diffs = parse(&text).map_err(|error| format!("{path}: {error}"))?;
        for diff in diffs.iter().rev() {
            reverse(&mut files, diff).map_err(|error| format!("{path}: {error}"))?;
        }
    }

    let hashes = std::fs::read_to_string(root.join(tree).join(HASHES))
        .map_err(|error| format!("{HASHES}: {error}"))?;
    let expected = parse_hashes(&hashes).map_err(|error| format!("{HASHES}: {error}"))?;
    for (rel, hash) in &expected {
        match files.get(rel) {
            None => problems.push(format!(
                "{tree}/{rel}: in the upstream release but missing after reversing the patches"
            )),
            Some(data) if crate::gate::digest(data) != *hash => problems.push(format!(
                "{tree}/{rel}: differs from the upstream release after reversing the patches"
            )),
            Some(_) => {}
        }
    }
    for rel in files.keys() {
        if !expected.contains_key(rel) {
            problems.push(format!(
                "{tree}/{rel}: not in the upstream release and added by no patch"
            ));
        }
    }
    Ok(problems)
}

/// The patch file names `PATCHES.md` names, in the order it first names them.
fn listed_patches(text: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for word in
        text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '(' | ')' | '[' | ']' | '|'))
    {
        let word = word.strip_prefix(PATCH_DIRECTORY).unwrap_or(word);
        if word.ends_with(".patch") && !word.contains('/') && !seen.iter().any(|s| s == word) {
            seen.push(word.to_owned());
        }
    }
    seen
}

/// `<sha256>  <path>` lines, sorted by path, each path once.
fn parse_hashes(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut hashes = BTreeMap::new();
    let mut last: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let Some((hash, path)) = line.split_once("  ") else {
            return Err(format!("line {number}: not `<sha256>  <path>`"));
        };
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("line {number}: not a lowercase sha256"));
        }
        if last.is_some_and(|last| last >= path) {
            return Err(format!(
                "line {number}: not sorted by path, or a path twice"
            ));
        }
        last = Some(path);
        hashes.insert(path.to_owned(), hash.to_owned());
    }
    Ok(hashes)
}

/// One file's part of a patch.
#[derive(Debug)]
struct FileDiff {
    /// The path before the change; `None` when the patch creates the file.
    old: Option<String>,
    /// The path after the change; `None` when the patch deletes the file.
    new: Option<String>,
    hunks: Vec<Hunk>,
}

#[derive(Debug)]
struct Hunk {
    new_start: usize,
    /// The lines before and after the change, each with its line ending.
    old: Vec<Vec<u8>>,
    new: Vec<Vec<u8>>,
}

/// The unified diffs of a patch file. Text before the first `--- ` line and between files is ignored.
fn parse(text: &[u8]) -> Result<Vec<FileDiff>, String> {
    let lines: Vec<&[u8]> = text.split_inclusive(|&b| b == b'\n').collect();
    let mut diffs: Vec<FileDiff> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let number = index + 1;
        for unsupported in [
            &b"GIT binary patch"[..],
            b"Binary files ",
            b"rename from ",
            b"copy from ",
        ] {
            if line.starts_with(unsupported) {
                return Err(format!(
                    "line {number}: binary, rename and copy diffs are not supported"
                ));
            }
        }
        if line.starts_with(b"--- ") {
            let next = lines.get(index + 1).copied().unwrap_or_default();
            if !next.starts_with(b"+++ ") {
                return Err(format!("line {number}: `--- ` without `+++ ` after it"));
            }
            let old =
                header_path(line, b"a/").map_err(|error| format!("line {number}: {error}"))?;
            let new = header_path(next, b"b/")
                .map_err(|error| format!("line {}: {error}", number + 1))?;
            if old.is_none() && new.is_none() {
                return Err(format!("line {number}: both sides are /dev/null"));
            }
            if let (Some(old), Some(new)) = (&old, &new) {
                if old != new {
                    return Err(format!("line {number}: renames are not supported"));
                }
            }
            diffs.push(FileDiff {
                old,
                new,
                hunks: Vec::new(),
            });
            index += 2;
            continue;
        }
        if line.starts_with(b"@@ ") {
            let Some(diff) = diffs.last_mut() else {
                return Err(format!("line {number}: hunk before any file header"));
            };
            let (old_count, new_start, new_count) =
                hunk_header(line).ok_or_else(|| format!("line {number}: malformed hunk header"))?;
            let mut hunk = Hunk {
                new_start,
                old: Vec::new(),
                new: Vec::new(),
            };
            index += 1;
            // Which sides the last body line went to, for a following "\ No newline at end of file".
            let mut last = (false, false);
            while hunk.old.len() < old_count
                || hunk.new.len() < new_count
                || lines.get(index).is_some_and(|line| line.starts_with(b"\\"))
            {
                let Some(body) = lines.get(index) else {
                    return Err(format!("line {number}: hunk ends early"));
                };
                let (marker, content) = body.split_first().ok_or("empty hunk line")?;
                match marker {
                    b' ' => {
                        hunk.old.push(content.to_vec());
                        hunk.new.push(content.to_vec());
                        last = (true, true);
                    }
                    b'-' => {
                        hunk.old.push(content.to_vec());
                        last = (true, false);
                    }
                    b'+' => {
                        hunk.new.push(content.to_vec());
                        last = (false, true);
                    }
                    b'\\' => {
                        if last.0 {
                            strip_newline(hunk.old.last_mut());
                        }
                        if last.1 {
                            strip_newline(hunk.new.last_mut());
                        }
                    }
                    _ => return Err(format!("line {}: not a hunk line", index + 1)),
                }
                index += 1;
            }
            if hunk.old.len() != old_count || hunk.new.len() != new_count {
                return Err(format!(
                    "line {number}: hunk line counts do not match its header"
                ));
            }
            diff.hunks.push(hunk);
            continue;
        }
        index += 1;
    }
    Ok(diffs)
}

fn strip_newline(line: Option<&mut Vec<u8>>) {
    if let Some(line) = line {
        if line.last() == Some(&b'\n') {
            line.pop();
        }
    }
}

/// The path of a `--- ` or `+++ ` line without its `a/` or `b/` prefix; `None` for `/dev/null`.
fn header_path(line: &[u8], side: &[u8]) -> Result<Option<String>, String> {
    let text = std::str::from_utf8(&line[4..]).map_err(|_| "path is not UTF-8".to_owned())?;
    let text = text.trim_end_matches(['\n', '\r']);
    // A timestamp may follow the path after a tab.
    let path = text.split('\t').next().unwrap_or(text);
    if path == "/dev/null" {
        return Ok(None);
    }
    let path = path.as_bytes().strip_prefix(side).ok_or_else(|| {
        format!(
            "path `{path}` does not start with `{}`",
            String::from_utf8_lossy(side)
        )
    })?;
    let path = std::str::from_utf8(path).map_err(|_| "path is not UTF-8".to_owned())?;
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("path `{path}` is not a plain relative path"));
    }
    Ok(Some(path.to_owned()))
}

/// `@@ -a[,b] +c[,d] @@`: the old line count, the new start and the new line count.
fn hunk_header(line: &[u8]) -> Option<(usize, usize, usize)> {
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |range: &str| -> Option<(usize, usize)> {
        match range.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (_, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some((old_count, new_start, new_count))
}

/// Undoes `diff` on `files`: the lines each hunk added must be where it put them, and are replaced by the lines it
/// removed. No fuzz and no offset.
fn reverse(files: &mut BTreeMap<String, Vec<u8>>, diff: &FileDiff) -> Result<(), String> {
    let name = diff.new.as_ref().or(diff.old.as_ref()).ok_or("no path")?;
    let mut lines: Vec<Vec<u8>> = match &diff.new {
        Some(path) => files
            .remove(path)
            .ok_or_else(|| format!("{path}: the patch changes it, but it is not in the tree"))?
            .split_inclusive(|&b| b == b'\n')
            .map(<[u8]>::to_vec)
            .collect(),
        None => {
            if files.contains_key(name) {
                return Err(format!(
                    "{name}: the patch deletes it, but it is in the tree"
                ));
            }
            Vec::new()
        }
    };
    // Hunk positions are in the patched file's numbering; `shift` follows the hunks already undone above.
    let mut shift: isize = 0;
    for hunk in &diff.hunks {
        let start = if hunk.new.is_empty() {
            hunk.new_start
        } else {
            hunk.new_start.saturating_sub(1)
        };
        let at = start
            .checked_add_signed(shift)
            .ok_or_else(|| format!("{name}: hunk at line {} is out of order", hunk.new_start))?;
        let end = at + hunk.new.len();
        if end > lines.len() || lines[at..end] != hunk.new[..] {
            return Err(format!(
                "{name}: the hunk at line {} does not match the tree; the file changed without its patch",
                hunk.new_start
            ));
        }
        lines.splice(at..end, hunk.old.iter().cloned());
        shift += hunk.old.len() as isize - hunk.new.len() as isize;
    }
    match &diff.old {
        Some(path) => {
            files.insert(path.clone(), lines.concat());
        }
        None => {
            if !lines.is_empty() {
                return Err(format!(
                    "{name}: the patch creates it, but the tree holds more than the patch adds"
                ));
            }
        }
    }
    Ok(())
}
