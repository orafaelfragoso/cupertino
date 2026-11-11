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
/// could write or copy into place names nothing, and a process id reused while it was read names nothing either.
pub fn trusted_executable(display: &DisplayHandle, client: &Client) -> Option<PathBuf> {
    let process = peer_process(display, client)?;
    let exe = std::fs::read_link(format!("/proc/{}/exe", process.0)).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.0)).ok()?;
    (start_time(&stat)? == process.1 && system_owned(&exe)).then_some(exe)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_time_is_field_22_even_when_the_command_name_has_parentheses() {
        let stat = "4242 (a (b) c) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
        assert_eq!(start_time(stat), Some(987654));
    }

    #[test]
    fn a_line_cut_short_has_no_start_time() {
        assert_eq!(start_time("4242 (x) S 1 2"), None);
        assert_eq!(parent_and_start_of("4242 (x) S 1 2"), None);
    }

    #[test]
    fn the_parent_is_field_4_and_this_process_has_its_own() {
        let stat = "4242 (a (b) 3 c) S 17 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(parent_and_start_of(stat), Some((17, 987654)));
        let (parent, start) = parent_and_start(std::process::id()).unwrap();
        assert_eq!(parent, std::os::unix::process::parent_id());
        assert!(start > 0);
        assert_eq!(parent_and_start(0), None);
    }

    #[test]
    fn the_token_is_the_whole_value_of_its_own_variable() {
        let environ = b"XDG_ACTIVATION_TOKEN_X=no\0PATH=/bin\0XDG_ACTIVATION_TOKEN=tok-1\0";
        assert_eq!(
            environ_value(environ, "XDG_ACTIVATION_TOKEN").as_deref(),
            Some("tok-1")
        );
        assert_eq!(
            environ_value(b"XDG_ACTIVATION_TOKEN=\0", "XDG_ACTIVATION_TOKEN"),
            None
        );
        assert_eq!(environ_value(b"PATH=/bin", "XDG_ACTIVATION_TOKEN"), None);
    }

    #[test]
    fn only_a_program_the_system_installed_is_trusted() {
        assert!(system_owned(&std::fs::canonicalize("/bin/sh").unwrap()));
        let mine = std::env::temp_dir().join(format!("cupertino-exe-{}", std::process::id()));
        std::fs::write(&mine, b"").unwrap();
        assert!(!system_owned(&mine), "a file of the user's");
        std::fs::remove_file(&mine).unwrap();
        assert!(!system_owned(Path::new("relative/sh")));
    }
}
