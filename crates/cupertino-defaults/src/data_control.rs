//! The applications the person allows to use data control (`data-control.conf`, `$CUPERTINO_DATA_CONTROL_CONF` overrides
//! the path): clipboard managers, which read every selection as it is set and replace it without focus. The settings
//! application writes the file and the compositor reads it. One `allow = <program>` per line, `#` starts a comment:
//!
//! ```text
//! allow = /usr/bin/clipboard-history
//! allow = 2049:1311:52016:1730281234000000000 /home/me/.local/bin/clipd
//! ```
//!
//! The program is the absolute path of the executable, as the kernel names it for a running process. A program only root
//! can change ([`crate::system::only_root_can_change`]) is allowed by its path alone, so an upgrade keeps it allowed. Any
//! other program is pinned: device, inode, size and modification time in nanoseconds before its path, and a file that
//! no longer matches is not allowed until the person allows it again. A path that holds `#` cannot be written.
//!
//! The file is the person's: it is read only when it is a regular file, not a link, owned by the reader's user and
//! writable by no group or other user, in a directory no group or other user can write ([`DataControlConf::load`]).
//! Interpreters and Wayland proxies are never allowed ([`refusal`]): allowing one would allow every script it runs or
//! every client behind it.

use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::conf::{ConfText, Line, Problem};

/// Environment variable that redirects `data-control.conf`.
pub const DATA_CONTROL_ENV: &str = "CUPERTINO_DATA_CONTROL_CONF";
/// File name under the configuration directory.
pub const DATA_CONTROL_FILE: &str = "data-control.conf";

/// The file a pinned entry was allowed as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// Modification time in nanoseconds since the epoch.
    pub mtime: i64,
}

impl Pin {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split(':');
        let pin = Self {
            dev: fields.next()?.parse().ok()?,
            ino: fields.next()?.parse().ok()?,
            size: fields.next()?.parse().ok()?,
            mtime: fields.next()?.parse().ok()?,
        };
        fields.next().is_none().then_some(pin)
    }
}

/// One allowed program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub program: PathBuf,
    /// `None` for a program only root can change.
    pub pin: Option<Pin>,
}

impl Entry {
    /// The entry that allows the program at `program` as it is now: by its path when only root can change it, else
    /// pinned. `Err` when it can never be allowed or cannot be looked at.
    pub fn for_program(program: &Path) -> Result<Self, String> {
        writable(program)?;
        let meta = std::fs::symlink_metadata(program)
            .map_err(|e| format!("{}: {e}", program.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a program file", program.display()));
        }
        let pin = (!crate::system::only_root_can_change(program)).then(|| Pin::of(&meta));
        Ok(Self {
            program: program.to_path_buf(),
            pin,
        })
    }

    /// Whether the entry allows the program file `file` (its metadata, links followed): a pinned entry while `file` is
    /// still the file it was pinned as, an unpinned one while only root can change its program.
    pub fn allows_file(&self, file: &std::fs::Metadata) -> bool {
        match self.pin {
            Some(pin) => pin == Pin::of(file),
            None => crate::system::only_root_can_change(&self.program),
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        let (pin, program) = match value.split_once(' ') {
            Some((pin, program)) if !pin.starts_with('/') => {
                let pin = Pin::parse(pin).ok_or_else(|| format!("{pin:?} is not a pin"))?;
                (Some(pin), program.trim_start())
            }
            _ => (None, value),
        };
        let program = PathBuf::from(program);
        if !program.is_absolute() {
            return Err(format!("{program:?} is not an absolute path"));
        }
        if let Some(reason) = refusal(&program) {
            return Err(reason.to_owned());
        }
        Ok(Self { program, pin })
    }

    pub fn text(&self) -> String {
        match self.pin {
            Some(p) => format!(
                "{}:{}:{}:{} {}",
                p.dev,
                p.ino,
                p.size,
                p.mtime,
                self.program.display()
            ),
            None => self.program.display().to_string(),
        }
    }
}

/// A line of `data-control.conf` that allows nothing: its text as written, trimmed, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub text: String,
    pub message: String,
}

/// The programs `data-control.conf` allows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataControlConf {
    pub allowed: Vec<Entry>,
}

impl DataControlConf {
    /// Where the file is: `$CUPERTINO_DATA_CONTROL_CONF`, else `data-control.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(DATA_CONTROL_ENV, DATA_CONTROL_FILE)
    }

    /// The entries of `text`; a line that is not an entry allows nothing and is a problem.
    pub fn parse(text: &str) -> (Self, Vec<Problem>) {
        let mut conf = Self::default();
        let mut problems = Vec::new();
        for (line, kind) in ConfText::parse(text).lines() {
            let entry = match kind {
                Line::Blank => continue,
                Line::Entry {
                    key: "allow",
                    value,
                } => Entry::parse(value),
                Line::Entry { key, .. } => Err(format!("unknown setting {key:?}")),
                Line::Bare(word) => Err(format!("expected `allow = <program>`, got {word:?}")),
            };
            match entry {
                Ok(entry) => conf.allowed.push(entry),
                Err(message) => problems.push(Problem { line, message }),
            }
        }
        (conf, problems)
    }

    /// [`Self::parse`] of the file at `path`, which must be the reader's own; a missing file allows
    /// nothing. `Err` when the file is refused or cannot be read: it then allows nothing either.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<Problem>)> {
        // Safety: geteuid has no preconditions.
        Self::load_as(path, unsafe { libc::geteuid() })
    }

    /// [`Self::load`] with the lines that allow nothing as written, for a person to see and remove.
    pub fn load_unusable(path: &Path) -> io::Result<(Self, Vec<Unusable>)> {
        // Safety: geteuid has no preconditions.
        let text = Self::read_as(path, unsafe { libc::geteuid() })?;
        let (conf, problems) = Self::parse(&text);
        let lines: Vec<&str> = text.lines().collect();
        let unusable = problems
            .into_iter()
            .map(|p| Unusable {
                text: lines.get(p.line - 1).map_or("", |l| l.trim()).to_owned(),
                message: p.message,
            })
            .collect();
        Ok((conf, unusable))
    }

    fn load_as(path: &Path, user: u32) -> io::Result<(Self, Vec<Problem>)> {
        Ok(Self::parse(&Self::read_as(path, user)?))
    }

    /// The text of the file at `path`, empty when there is none.
    fn read_as(path: &Path, user: u32) -> io::Result<String> {
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            // without O_NONBLOCK a FIFO put at the path would block the open before the check refuses it
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(refused("the file is a link"))
            }
            Err(e) => return Err(e),
        };
        let dir = std::fs::metadata(path.parent().unwrap_or(Path::new(".")))?;
        check_owned(&file.metadata()?, &dir, user)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(text)
    }

    /// Write the entries into `path` atomically with mode `0600`, replacing the file. An entry whose program could not be
    /// read back as itself, or can never be allowed, is an error and nothing is written.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        self.store_keeping(path, &[])
    }

    /// [`Self::store`], with the lines of `unusable` written after the entries as they are. A line that would allow a
    /// program or is not a single line is an error and nothing is written.
    pub fn store_keeping(&self, path: &Path, unusable: &[Unusable]) -> io::Result<()> {
        for line in unusable {
            if line.text.contains('\n') || !Self::parse(&line.text).0.allowed.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{:?} is not a line that allows nothing", line.text),
                ));
            }
        }
        for entry in &self.allowed {
            writable(&entry.program).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        }
        let text: String = self
            .allowed
            .iter()
            .map(|e| format!("allow = {}\n", e.text()))
            .chain(unusable.iter().map(|l| format!("{}\n", l.text)))
            .collect();
        crate::atomic::write_atomic_private(path, text.as_bytes())
    }
}

/// Refuse a program that a line of the file cannot hold as itself (a relative path, `#`, a line break, or white space at
/// either end, which a read trims) or that can never be allowed ([`refusal`]).
fn writable(program: &Path) -> Result<(), String> {
    let text = program.as_os_str().to_string_lossy();
    if !program.is_absolute() || text.contains(['#', '\n']) || text.trim() != text {
        return Err(format!(
            "{} cannot be written as an entry",
            program.display()
        ));
    }
    match refusal(program) {
        Some(reason) => Err(reason.to_owned()),
        None => Ok(()),
    }
}

/// Refuse a file that is not a regular file of `user` that only that user can write, or whose directory a group or other
/// user can write.
pub(crate) fn check_owned(
    file: &std::fs::Metadata,
    dir: &std::fs::Metadata,
    user: u32,
) -> io::Result<()> {
    if !file.is_file() || file.uid() != user {
        return Err(refused("the file is not a regular file of this user"));
    }
    if file.mode() & 0o022 != 0 {
        return Err(refused("a group or other users can write the file"));
    }
    if dir.mode() & 0o022 != 0 || (dir.uid() != user && dir.uid() != 0) {
        return Err(refused(
            "the directory of the file is not this user's alone",
        ));
    }
    Ok(())
}

pub(crate) fn refused(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, why)
}

/// Interpreters, by file name: a name with a version after it (`python3.12`) is the same interpreter.
const INTERPRETERS: &[&str] = &[
    "python", "pypy", "perl", "ruby", "node", "nodejs", "lua", "php", "tclsh", "wish",
];
/// Interpreters, shells and loaders that have no version in their name.
const EXACT: &[&str] = &[
    "java", "sh", "bash", "dash", "zsh", "fish", "ksh", "mksh", "csh", "tcsh", "busybox", "env",
    "ld.so",
];
const LOADERS: &[&str] = &["ld-linux", "ld-musl"];
/// Wayland proxies and servers whose clients would act through them.
const PROXIES: &[&str] = &[
    "Xwayland",
    "xwayland-satellite",
    "waypipe",
    "cupertino-server",
];

/// What [`refusal`] says of an interpreter or a loader.
pub const RUNS_SCRIPTS: &str = "an interpreter runs any script: allowing it would allow every one";

/// Why `program` can never be allowed, by its file name: an interpreter or a dynamic loader runs whatever it is given,
/// and a Wayland proxy or server carries other clients, so allowing one would allow all of them.
pub fn refusal(program: &Path) -> Option<&'static str> {
    let name = program.file_name()?.to_str()?;
    let versioned = |base: &str| {
        name.strip_prefix(base)
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
    };
    if INTERPRETERS.iter().any(|b| versioned(b))
        || EXACT.contains(&name)
        || LOADERS.iter().any(|l| name.starts_with(l))
    {
        Some(RUNS_SCRIPTS)
    } else if PROXIES.iter().any(|p| name.eq_ignore_ascii_case(p)) {
        Some("a Wayland proxy carries other clients: allowing it would allow every one")
    } else {
        None
    }
}
