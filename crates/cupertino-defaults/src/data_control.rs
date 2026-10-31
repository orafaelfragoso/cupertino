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

    fn text(&self) -> String {
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

    fn load_as(path: &Path, user: u32) -> io::Result<(Self, Vec<Problem>)> {
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            // without O_NONBLOCK a FIFO put at the path would block the open before the check refuses it
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Default::default()),
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(refused("the file is a link"))
            }
            Err(e) => return Err(e),
        };
        let dir = std::fs::metadata(path.parent().unwrap_or(Path::new(".")))?;
        check_owned(&file.metadata()?, &dir, user)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(Self::parse(&text))
    }

    /// Write the entries into `path` atomically with mode `0600`, replacing the file. An entry whose program could not be
    /// read back as itself, or can never be allowed, is an error and nothing is written.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        for entry in &self.allowed {
            writable(&entry.program).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        }
        let text: String = self
            .allowed
            .iter()
            .map(|e| format!("allow = {}\n", e.text()))
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
fn check_owned(file: &std::fs::Metadata, dir: &std::fs::Metadata, user: u32) -> io::Result<()> {
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

fn refused(why: &str) -> io::Error {
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
        Some("an interpreter runs any script: allowing it would allow every one")
    } else if PROXIES.iter().any(|p| name.eq_ignore_ascii_case(p)) {
        Some("a Wayland proxy carries other clients: allowing it would allow every one")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn entries_are_read_by_path_or_pin_and_bad_lines_allow_nothing() {
        let (conf, problems) = DataControlConf::parse(
            "# managers\n\
             allow = /usr/bin/clipboard-history\n\
             allow = 2049:1311:52016:1730281234000000005 /home/me/my tools/clipd  # pinned\n\
             allow = clipd\n\
             allow = 1:2:3 /home/me/x\n\
             allow = /usr/bin/python3.12\n\
             deny = /usr/bin/y\n\
             /usr/bin/z\n",
        );
        assert_eq!(
            conf.allowed,
            [
                Entry {
                    program: "/usr/bin/clipboard-history".into(),
                    pin: None
                },
                Entry {
                    program: "/home/me/my tools/clipd".into(),
                    pin: Some(Pin {
                        dev: 2049,
                        ino: 1311,
                        size: 52016,
                        mtime: 1_730_281_234_000_000_005
                    })
                },
            ]
        );
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            [4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn a_stored_file_reads_back_the_same_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_CONTROL_FILE);
        let program = dir.path().join("my tools").join("clipd");
        std::fs::create_dir(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        let conf = DataControlConf {
            allowed: vec![
                Entry::for_program(Path::new("/usr/bin/ls")).unwrap(),
                Entry::for_program(&program).unwrap(),
            ],
        };
        assert_eq!(
            conf.allowed[0].pin, None,
            "a program of the system goes by its path"
        );
        assert!(
            conf.allowed[1].pin.is_some(),
            "a program of the user is pinned"
        );
        conf.store(&path).unwrap();
        assert_eq!(DataControlConf::load(&path).unwrap(), (conf, vec![]));
    }

    #[test]
    fn interpreters_loaders_and_proxies_are_refused_and_their_lookalikes_are_not() {
        for refused in [
            "/usr/bin/python3",
            "/usr/bin/python3.12",
            "/usr/bin/perl5.38.2",
            "/usr/bin/nodejs",
            "/usr/bin/bash",
            "/usr/bin/env",
            "/usr/lib/ld-linux-x86-64.so.2",
            "/lib/ld-musl-x86_64.so.1",
            "/usr/bin/Xwayland",
            "/usr/bin/waypipe",
        ] {
            assert!(refusal(Path::new(refused)).is_some(), "{refused}");
            assert!(Entry::for_program(Path::new(refused)).is_err(), "{refused}");
        }
        for allowed in [
            "/usr/bin/pythonista",
            "/usr/bin/shotwell",
            "/usr/bin/clipd",
            "/usr/bin/node-clip",
        ] {
            assert_eq!(refusal(Path::new(allowed)), None, "{allowed}");
        }
    }

    #[test]
    fn a_file_that_is_not_the_persons_alone_allows_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join(DATA_CONTROL_FILE);
        std::fs::write(&path, "allow = /usr/bin/clipboard-history\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // Safety: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        assert_eq!(
            DataControlConf::load_as(&path, user)
                .unwrap()
                .0
                .allowed
                .len(),
            1
        );

        let error = |path: &Path, user| DataControlConf::load_as(path, user).unwrap_err().kind();
        // in a directory of root, so only the file's owner is wrong
        let root_dir = std::fs::metadata("/").unwrap();
        let file = std::fs::metadata(&path).unwrap();
        assert!(check_owned(&file, &root_dir, user).is_ok());
        assert_eq!(
            check_owned(&file, &root_dir, user + 1).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "another user's"
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(
            error(&path, user),
            io::ErrorKind::PermissionDenied,
            "writable by all"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            error(&path, user),
            io::ErrorKind::PermissionDenied,
            "in a shared directory"
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        let link = dir.path().join("link.conf");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            error(&link, user),
            io::ErrorKind::PermissionDenied,
            "a link"
        );

        assert_eq!(
            DataControlConf::load_as(&dir.path().join("missing.conf"), user).unwrap(),
            Default::default()
        );

        let fifo = dir.path().join("fifo.conf");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // Safety: `name` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            error(&fifo, user),
            io::ErrorKind::PermissionDenied,
            "a FIFO, refused without waiting for a writer"
        );
    }

    #[test]
    fn an_entry_a_line_cannot_hold_as_itself_is_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_CONTROL_FILE);
        store_ok(&path);
        for program in [
            "/home/me/clipd\nallow = /usr/bin/ls",
            "/usr/bin/ls\r",
            "/usr/bin/ls ",
            "/home/me/#clipd",
            "clipd",
            "/usr/bin/python3",
        ] {
            let conf = DataControlConf {
                allowed: vec![Entry {
                    program: program.into(),
                    pin: None,
                }],
            };
            assert_eq!(
                conf.store(&path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{program:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "allow = /usr/bin/ls\n",
                "{program:?}: the file is unchanged"
            );
        }
    }

    fn store_ok(path: &Path) {
        DataControlConf {
            allowed: vec![Entry::for_program(Path::new("/usr/bin/ls")).unwrap()],
        }
        .store(path)
        .unwrap();
    }
}
