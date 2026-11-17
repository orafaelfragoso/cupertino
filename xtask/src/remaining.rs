//! The remaining-work list of the layers check only shrinks: a line of `crates/cupertino-layers/remaining.txt` in the
//! working tree that the file at the merge base does not hold is a new problem, and fails.
//!
//! An added line passes when the same change removes a line with the same `<layer> <rule> <crate>` (one removed line
//! for each added one), as when a dependency or a file the line names is renamed, or when its rule is one the layers
//! check at the merge base does not have yet, so that a new rule starts with its own list.

use std::path::Path;
use std::process::Command;

/// The remaining-work file, relative to the repository root.
const FILE: &str = "crates/cupertino-layers/remaining.txt";
/// The sources of the layers check, which name every rule.
const RULE_SOURCES: &str = "crates/cupertino-layers/src";

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    crate::plan::output(root, "git", args)
}

fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// `<layer> <rule> <crate>`, the first three words of a line.
fn prefix(line: &str) -> Vec<&str> {
    line.split_whitespace().take(3).collect()
}

/// Whether the layers check at `base` names `rule` as a problem kind.
fn rule_known(root: &Path, base: &str, rule: &str) -> Result<bool, String> {
    let pattern = format!("\"{rule}\"");
    let args = ["grep", "-q", "-F", "-e", &pattern, base, "--", RULE_SOURCES];
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    // git grep exits with 1 when nothing matches, and above 1 on an error
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git {}: {status}", args.join(" "))),
    }
}

/// The problems of the working tree's remaining-work file against the one at `base`, a commit; empty when the list
/// did not grow, or when `base` has no such file.
pub fn problems(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let at_base = format!("{base}:{FILE}");
    if git(root, &["cat-file", "-e", &at_base]).is_err() {
        return Ok(Vec::new());
    }
    let before = git(root, &["show", &at_base])?;
    let now = match std::fs::read_to_string(root.join(FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{FILE}: {error}")),
    };
    let before = lines(&before);
    let now = lines(&now);
    let mut removed: Vec<Vec<&str>> = before
        .iter()
        .filter(|line| !now.contains(line))
        .map(|line| prefix(line))
        .collect();
    let short = base.get(..12).unwrap_or(base);
    let mut problems = Vec::new();
    for line in now.iter().filter(|line| !before.contains(line)) {
        let own = prefix(line);
        if let Some(index) = removed.iter().position(|p| *p == own) {
            removed.swap_remove(index);
            continue;
        }
        let rule = own.get(1).copied().unwrap_or_default();
        if !rule.is_empty() && !rule_known(root, base, rule)? {
            continue;
        }
        problems.push(format!(
            "{FILE}: `{line}` is not listed at {short}, and the list only shrinks: fix the problem instead of listing \
             it; a line that names a renamed dependency or file passes when the same change removes the old line of \
             the same `{}`",
            own.join(" ")
        ));
    }
    Ok(problems)
}

/// The summary line of a run.
pub fn summary(problems: &[String]) -> String {
    format!(
        "remaining-work list: {} problems against the merge base",
        problems.len()
    )
}
