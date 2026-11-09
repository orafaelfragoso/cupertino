// Where the build script finds the source commit. `build.rs` and `tests/commit-source.rs` both include this file, so
// the order is tested without running Cargo.

use std::ffi::OsStr;
use std::path::Path;

/// The variable a packager sets to the commit the sources come from.
pub const VARIABLE: &str = "CUPERTINO_BUILD_COMMIT";

/// The tracked file `git archive` fills with the archived commit (`export-subst` in `.gitattributes`).
pub const ARCHIVE_FILE: &str = "data/build-commit";

/// The text of [`ARCHIVE_FILE`] in a checkout, before `git archive` substitutes it.
pub const UNSUBSTITUTED: &str = "$Format:%H$";

/// Where the commit came from; the build script passes its [`Source::name`] to the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Environment,
    Archive,
    Checkout,
    Unknown,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Archive => "archive",
            Self::Checkout => "checkout",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    /// The full hash in lower case, or `None` when no source knows it.
    pub commit: Option<String>,
    pub source: Source,
    /// Lines for Cargo's output (`cargo:warning=…`) about sources that were skipped.
    pub warnings: Vec<String>,
}

/// A full commit hash: 40 hexadecimal digits, or 64 for a repository with sha256 object names. Upper case is accepted and
/// lowered.
pub fn valid(text: &str) -> Option<String> {
    let text = text.trim();
    ((text.len() == 40 || text.len() == 64) && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// The commit of the sources under `root`, the workspace directory, from the first source that knows it:
/// 1. `variable`, the value of [`VARIABLE`]: a value that is not a full hash is an error, as a packager set it on purpose;
/// 2. [`ARCHIVE_FILE`] under `root`, once `git archive` substituted it;
/// 3. `checkout(root)`, the `HEAD` of a git checkout, called only when `root/.git` exists (a directory, or a file in a
///    linked worktree);
/// 4. otherwise unknown.
pub fn resolve(
    variable: Option<&OsStr>,
    root: &Path,
    checkout: impl Fn(&Path) -> Option<String>,
) -> Result<Found, String> {
    let mut warnings = Vec::new();
    if let Some(value) = variable {
        let text = value.to_string_lossy();
        return match valid(&text) {
            Some(commit) => Ok(Found {
                commit: Some(commit),
                source: Source::Environment,
                warnings,
            }),
            None => Err(format!(
                "{VARIABLE} is `{text}`, not a full commit hash (40 or 64 hexadecimal digits)"
            )),
        };
    }
    match std::fs::read_to_string(root.join(ARCHIVE_FILE)) {
        Ok(text) if text.trim() == UNSUBSTITUTED => {}
        Ok(text) => match valid(&text) {
            Some(commit) => {
                return Ok(Found {
                    commit: Some(commit),
                    source: Source::Archive,
                    warnings,
                })
            }
            None => warnings.push(format!(
                "{ARCHIVE_FILE} holds `{}`, neither a commit hash nor `{UNSUBSTITUTED}`; it is ignored",
                text.trim()
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warnings.push(format!("{ARCHIVE_FILE} cannot be read ({e}); it is ignored")),
    }
    if root.join(".git").exists() {
        match checkout(root).as_deref().and_then(valid) {
            Some(commit) => {
                return Ok(Found {
                    commit: Some(commit),
                    source: Source::Checkout,
                    warnings,
                })
            }
            None => warnings
                .push("the checkout's HEAD cannot be read with git; the commit is unknown".into()),
        }
    }
    Ok(Found {
        commit: None,
        source: Source::Unknown,
        warnings,
    })
}

/// `git rev-parse HEAD` in `root`; `None` when git is missing or fails.
pub fn git_head(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
