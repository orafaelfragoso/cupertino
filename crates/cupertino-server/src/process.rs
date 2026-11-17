//! The process behind a client connection, named so that a reused process id is another process.

use std::path::{Path, PathBuf};

use smithay::reexports::wayland_server::{Client, DisplayHandle};

/// A process id and the clock tick the process started at (`/proc/<pid>/stat`, field 22).
pub type ProcessIdentity = (u32, u64);

/// Names the process behind a client; [`peer_process`] in the server.
pub type ProcessOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<ProcessIdentity>>;

/// The process on the other end of `client`'s socket, from its credentials; `None` when they or `/proc` do not say.
pub fn peer_process(display: &DisplayHandle, client: &Client) -> Option<ProcessIdentity> {
    let pid = u32::try_from(client.get_credentials(display).ok()?.pid).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    Some((pid, start_time(&stat)?))
}

/// Names the program of the process behind a client; [`trusted_executable`] in the server.
pub type ExecutableOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<PathBuf>>;

/// The program the process on the other end of `client`'s socket runs (`/proc/<pid>/exe`), when only the system can have
/// put it there: the file and every folder above it belong to root and nobody else may write to them. A program the user
/// could write or copy into place names nothing, and a process id reused while it was read names nothing either. A
/// program replaced since the process started it, as a package upgrade does, is named by the path it was started from.
/// The file the process runs must be the one at that path here, not a file of the same path in another mount namespace
/// (a folder or file mounted over it there).
pub fn trusted_executable(display: &DisplayHandle, client: &Client) -> Option<PathBuf> {
    let process = peer_process(display, client)?;
    let link = format!("/proc/{}/exe", process.0);
    let exe = started_from(std::fs::read_link(&link).ok()?);
    // through the link: the file the process runs, wherever it lies
    let running = std::fs::metadata(&link).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.0)).ok()?;
    (start_time(&stat)? == process.1 && trusted_program(&running, &exe)).then_some(exe)
}

/// Whether a process that runs the file `running` (its metadata through `/proc/<pid>/exe`) started from `exe` runs a
/// program only the system can have installed: `exe` is [`system_owned`] and `running` is the file at `exe` in this
/// server's mount namespace, the same device and inode, or, for a program removed or replaced since it started, a
/// removed file on that device that belonged to root and nobody else could write to.
fn trusted_program(running: &std::fs::Metadata, exe: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(installed) = std::fs::metadata(exe) else {
        return false;
    };
    let same_file = running.dev() == installed.dev() && running.ino() == installed.ino();
    let replaced = running.dev() == installed.dev()
        && running.nlink() == 0
        && running.uid() == 0
        && running.mode() & 0o022 == 0;
    system_owned(exe) && (same_file || replaced)
}

/// Names the activation token a client's process was started with; [`launch_token`] in the server.
pub type LaunchTokenOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<String>>;

/// The `XDG_ACTIVATION_TOKEN` the process on the other end of `client`'s socket was started with, for a program that
/// never presents it. Holding a token is the right to use it, so reading it from the process is no more than the process
/// could present itself; a process id reused while it was read names nothing.
pub fn launch_token(display: &DisplayHandle, client: &Client) -> Option<String> {
    let process = peer_process(display, client)?;
    let environ = std::fs::read(format!("/proc/{}/environ", process.0)).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.0)).ok()?;
    if start_time(&stat)? != process.1 {
        return None;
    }
    environ_value(&environ, "XDG_ACTIVATION_TOKEN")
}

/// The value of `name` in a NUL-separated environment block.
fn environ_value(environ: &[u8], name: &str) -> Option<String> {
    environ.split(|b| *b == 0).find_map(|entry| {
        let value = entry.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
        std::str::from_utf8(value)
            .ok()
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    })
}

/// What the kernel marks a program that was removed or replaced while it runs with, after its path in `/proc/<pid>/exe`.
const REMOVED_PROGRAM_MARK: &str = " (deleted)";

/// The path a process started its program from, out of the link `/proc/<pid>/exe` holds: without the mark of a program
/// removed since.
fn started_from(link: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let bytes = link.as_os_str().as_bytes();
    match bytes.strip_suffix(REMOVED_PROGRAM_MARK.as_bytes()) {
        Some(path) => PathBuf::from(std::ffi::OsStr::from_bytes(path)),
        None => link,
    }
}

/// Whether `path` and each folder above it belong to root and are writable by nobody else.
fn system_owned(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.is_absolute()
        && path.ancestors().all(|p| {
            std::fs::symlink_metadata(p)
                .is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0 && !m.file_type().is_symlink())
        })
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name in field 2 may hold spaces and parentheses, so the fields are
/// counted from the last `)`.
fn start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// Bytes of `/proc/<pid>/stat` read at most. The line is a few hundred bytes (its command name is at most 64), so this
/// bounds the read of a process the caller did not choose.
const STAT_LIMIT: u64 = 4096;

/// The parent process id (field 4) and the start time (field 22) of process `pid`, from a bounded read of
/// `/proc/<pid>/stat`; `None` when the process is gone or the line is not one.
pub fn parent_and_start(pid: u32) -> Option<(u32, u64)> {
    use std::io::Read;
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{pid}/stat"))
        .ok()?
        .take(STAT_LIMIT)
        .read_to_string(&mut stat)
        .ok()?;
    parent_and_start_of(&stat)
}

/// Fields 4 and 22 of a `/proc/<pid>/stat` line, counted from the last `)` like [`start_time`].
fn parent_and_start_of(stat: &str) -> Option<(u32, u64)> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    let parent = after_name.split_whitespace().nth(1)?.parse().ok()?;
    Some((parent, start_time(stat)?))
}
