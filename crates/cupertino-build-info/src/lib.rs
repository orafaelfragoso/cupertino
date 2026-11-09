//! What a Cupertino build is and what it runs on: the commit the sources come from ([`COMMIT`]), a component's name
//! and version ([`component!`]), the OS image ([`os_image`]) and the kernel ([`kernel`]).
//!
//! The crate has no dependencies, so every process, the toolkit and the C library can link it. Nothing here touches
//! the network or runs a program at run time; the commit is found once, by the build script.

use std::fmt;
use std::io;
use std::path::Path;

/// The full hash of the commit the sources come from, in lower case, or `None` when the build could not know it.
///
/// The build script takes the first of: the `CUPERTINO_BUILD_COMMIT` variable (a full hash, or the build fails), the
/// `data/build-commit` file that `git archive` fills, the `HEAD` of a git checkout, or nothing. [`COMMIT_SOURCE`] says
/// which one it was.
pub const COMMIT: Option<&str> = match env!("CUPERTINO_BUILD_INFO_COMMIT").as_bytes() {
    [] => None,
    _ => Some(env!("CUPERTINO_BUILD_INFO_COMMIT")),
};

/// Where [`COMMIT`] came from.
pub const COMMIT_SOURCE: CommitSource =
    CommitSource::from_name(env!("CUPERTINO_BUILD_INFO_COMMIT_SOURCE"));

/// How many digits of the commit [`short_commit`] keeps.
pub const SHORT_COMMIT_DIGITS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitSource {
    /// `CUPERTINO_BUILD_COMMIT`, set by the package build.
    Environment,
    /// `data/build-commit` in a source archive made with `git archive`.
    Archive,
    /// `git rev-parse HEAD` in a checkout, when this crate was last built. A later commit does not rebuild it, so in
    /// a development build the value can be older than the checkout's `HEAD`.
    Checkout,
    /// None of them: a source tree without `.git` and without a filled `data/build-commit`.
    Unknown,
}

impl CommitSource {
    const fn from_name(name: &str) -> Self {
        match name.as_bytes() {
            b"environment" => Self::Environment,
            b"archive" => Self::Archive,
            b"checkout" => Self::Checkout,
            _ => Self::Unknown,
        }
    }

    /// `environment`, `archive`, `checkout` or `unknown`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Archive => "archive",
            Self::Checkout => "checkout",
            Self::Unknown => "unknown",
        }
    }
}

/// The first [`SHORT_COMMIT_DIGITS`] digits of [`COMMIT`], as people quote a commit.
pub fn short_commit() -> Option<&'static str> {
    COMMIT.map(|c| &c[..SHORT_COMMIT_DIGITS])
}

/// A program or library of the desktop: its name and its package version. Every component of one build shares
/// [`COMMIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    pub name: &'static str,
    pub version: &'static str,
}

impl Component {
    /// The commit of the build, [`COMMIT`].
    pub fn commit(&self) -> Option<&'static str> {
        COMMIT
    }
}

/// `name version (commit)` with the [`short_commit`], or `name version` when the commit is unknown:
/// `cupertino 0.1.0 (0123456789ab)`.
impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.version)?;
        match short_commit() {
            Some(commit) => write!(f, " ({commit})"),
            None => Ok(()),
        }
    }
}

/// The [`Component`] of the crate that expands it: `component!()` is named after the package (`CARGO_PKG_NAME`),
/// `component!("cupertino")` after a binary with another name. The version is always the package's
/// (`CARGO_PKG_VERSION`).
#[macro_export]
macro_rules! component {
    () => {
        $crate::Component {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        }
    };
    ($name:expr) => {
        $crate::Component {
            name: $name,
            version: env!("CARGO_PKG_VERSION"),
        }
    };
}

/// The fields of `os-release` that identify an installed image. Each is `None` when the file does not set it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsImage {
    /// `NAME`
    pub name: Option<String>,
    /// `ID`
    pub id: Option<String>,
    /// `VERSION_ID`
    pub version_id: Option<String>,
    /// `BUILD_ID`
    pub build_id: Option<String>,
    /// `IMAGE_ID`
    pub image_id: Option<String>,
    /// `IMAGE_VERSION`
    pub image_version: Option<String>,
}

/// Where `os-release` is looked for under a root, in order: the second only when the first does not exist.
pub const OS_RELEASE_FILES: [&str; 2] = ["etc/os-release", "usr/lib/os-release"];

impl OsImage {
    /// Reads the `KEY=value` lines of an `os-release` text. Blank lines and lines starting with `#` are skipped, as are
    /// lines without `=`. A value may be in double quotes (where `\"`, `\\`, `\$` and `` \` `` stand for the character
    /// after the backslash) or single quotes (taken literally); an unquoted value is taken as it is, up to the end of
    /// the line. A later line for the same key wins, and an empty value counts as not set.
    pub fn parse(text: &str) -> Self {
        let mut image = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let field = match key.trim() {
                "NAME" => &mut image.name,
                "ID" => &mut image.id,
                "VERSION_ID" => &mut image.version_id,
                "BUILD_ID" => &mut image.build_id,
                "IMAGE_ID" => &mut image.image_id,
                "IMAGE_VERSION" => &mut image.image_version,
                _ => continue,
            };
            let value = unquote(value.trim());
            *field = (!value.is_empty()).then_some(value);
        }
        image
    }

    /// Whether the file set none of the fields.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    match bytes.first() {
        Some(&q @ (b'"' | b'\'')) if bytes.len() >= 2 && bytes[bytes.len() - 1] == q => {
            let inner = &value[1..value.len() - 1];
            if q == b'\'' {
                return inner.to_string();
            }
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some(e @ ('"' | '\\' | '$' | '`')) => out.push(e),
                        Some(other) => {
                            out.push('\\');
                            out.push(other);
                        }
                        None => out.push('\\'),
                    },
                    c => out.push(c),
                }
            }
            out
        }
        _ => value.to_string(),
    }
}

/// The OS image of the running system; every field is `None` when no `os-release` can be read.
pub fn os_image() -> OsImage {
    read_os_image(Path::new("/")).unwrap_or_default()
}

/// The OS image from [`OS_RELEASE_FILES`] under `root`. A missing first file falls back to the second; the error is
/// `NotFound` when both are missing, or the read error of the file that exists.
pub fn read_os_image(root: &Path) -> io::Result<OsImage> {
    let [first, second] = OS_RELEASE_FILES;
    let text = match std::fs::read(root.join(first)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::read(root.join(second))?,
        other => other?,
    };
    Ok(OsImage::parse(&String::from_utf8_lossy(&text)))
}

/// Where the running kernel's release is read, under a root.
pub const KERNEL_RELEASE_FILE: &str = "proc/sys/kernel/osrelease";

/// The release of the running kernel (`uname -r`), or `None` when it cannot be read.
pub fn kernel() -> Option<String> {
    kernel_in(Path::new("/"))
}

/// The kernel release from [`KERNEL_RELEASE_FILE`] under `root`, without its line end; `None` when the file cannot
/// be read or is empty.
pub fn kernel_in(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(KERNEL_RELEASE_FILE)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests;
