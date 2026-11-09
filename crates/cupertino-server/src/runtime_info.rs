//! What a running server tells other programs about itself.
//!
//! - `--ready-fd N`: once the Wayland socket is bound, the server writes `WAYLAND_DISPLAY=<name>` and a line end to
//!   descriptor `N` and closes it, so the program that started it (`cupertino desktop start`) knows when clients can
//!   connect and to which socket.
//! - The runtime file `$XDG_RUNTIME_DIR/cupertino/server-<name>.json` (mode 0600, its directory 0700) says which
//!   server runs on the socket `<name>`: `{schema, version, commit, pid, backend, renderer, wayland_display, started}`,
//!   `started` in seconds since 1970. `cupertino version` reads it for the compositor of `WAYLAND_DISPLAY`. The server
//!   removes it when it ends normally or by a panic, and, once [`RuntimeFile::remove_on_ending_signals`] ran, when a
//!   signal ends it (SIGTERM, SIGINT, SIGHUP, SIGQUIT, an abort, a fault) before the signal does what it did before.
//!   Only SIGKILL leaves it, and its `pid` then names no process: readers check that the process is there.

use std::ffi::CString;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Once;

use serde::Serialize;

/// The version of the runtime file's fields; it changes when one is removed or changes meaning.
pub const SCHEMA: u32 = 1;

/// The directory of the runtime files under `XDG_RUNTIME_DIR`.
pub const DIRECTORY: &str = "cupertino";

/// What the server's renderer is: every backend draws with the GLES renderer.
pub const RENDERER: &str = "gles";

/// The runtime file of the server on the socket `display`, relative to `XDG_RUNTIME_DIR`.
pub fn file_name(display: &str) -> PathBuf {
    Path::new(DIRECTORY).join(format!("server-{display}.json"))
}

/// The fields of the runtime file.
#[derive(Debug, Serialize)]
pub struct RuntimeInfo<'a> {
    pub schema: u32,
    pub version: &'a str,
    pub commit: Option<&'a str>,
    pub pid: u32,
    /// `udev`, `winit` or `headless`.
    pub backend: &'a str,
    pub renderer: &'a str,
    pub wayland_display: &'a str,
    /// When the server started, in seconds since 1970.
    pub started: u64,
}

impl<'a> RuntimeInfo<'a> {
    /// This process, started now, on `display` with `backend`.
    pub fn now(display: &'a str, backend: &'a str) -> Self {
        Self {
            schema: SCHEMA,
            version: env!("CARGO_PKG_VERSION"),
            commit: cupertino_build_info::COMMIT,
            pid: std::process::id(),
            backend,
            renderer: RENDERER,
            wayland_display: display,
            started: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }
}

/// The runtime file while the server runs; dropping it removes the file.
#[derive(Debug)]
pub struct RuntimeFile {
    path: PathBuf,
}

impl RuntimeFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the file also when a signal ends the process: SIGTERM, SIGINT, SIGHUP, SIGQUIT, SIGABRT (an abort) and
    /// the faults SIGSEGV, SIGBUS, SIGILL and SIGFPE. The handler removes the file, then hands the signal to the
    /// handler that was there before, or gives it back its default action, so the process ends as it did before. A
    /// signal the process ignores stays ignored. Once for the process; the latest file is the one removed.
    pub fn remove_on_ending_signals(&self) {
        let Ok(path) = CString::new(self.path.as_os_str().as_bytes()) else {
            return;
        };
        let old = REMOVE_ON_SIGNAL.swap(path.into_raw(), Ordering::SeqCst);
        if !old.is_null() {
            // SAFETY: the pointer came from `CString::into_raw` and was swapped out, so nothing else owns it.
            drop(unsafe { CString::from_raw(old) });
        }
        static INSTALL: Once = Once::new();
        INSTALL.call_once(install_ending_signal_handlers);
    }
}

impl Drop for RuntimeFile {
    fn drop(&mut self) {
        // a signal from here on finds nothing to remove; a handler that already took the path owns it
        let path = REMOVE_ON_SIGNAL.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !path.is_null() {
            // SAFETY: the pointer came from `CString::into_raw` and was swapped out, so nothing else owns it.
            drop(unsafe { CString::from_raw(path) });
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The signals that end the process by default and that a handler can catch.
const ENDING_SIGNALS: [libc::c_int; 9] = [
    libc::SIGTERM,
    libc::SIGINT,
    libc::SIGHUP,
    libc::SIGQUIT,
    libc::SIGABRT,
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
];

/// The runtime file a signal handler removes: a C string owned by whoever swaps it out, null when there is none.
static REMOVE_ON_SIGNAL: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());

/// The handler and flags each of [`ENDING_SIGNALS`] had before, for the handler to pass the signal on.
static PREVIOUS_HANDLERS: [AtomicUsize; ENDING_SIGNALS.len()] =
    [const { AtomicUsize::new(libc::SIG_DFL) }; ENDING_SIGNALS.len()];
static PREVIOUS_FLAGS: [AtomicI32; ENDING_SIGNALS.len()] =
    [const { AtomicI32::new(0) }; ENDING_SIGNALS.len()];

fn install_ending_signal_handlers() {
    for (i, &signal) in ENDING_SIGNALS.iter().enumerate() {
        // SAFETY: sigaction reads and writes the actions of a valid signal number into zeroed structures it owns.
        unsafe {
            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut previous) != 0
                || previous.sa_sigaction == libc::SIG_IGN
            {
                continue;
            }
            PREVIOUS_HANDLERS[i].store(previous.sa_sigaction, Ordering::SeqCst);
            PREVIOUS_FLAGS[i].store(previous.sa_flags, Ordering::SeqCst);
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_ending_signal
                as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)
                as libc::sighandler_t;
            // on the alternate stack where a thread has one, so a stack overflow still reaches it
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

/// Removes the runtime file, then passes the signal on as it was handled before. Only async-signal-safe calls: atomics,
/// unlink(2), sigaction(2), raise(3) and the previous handler.
extern "C" fn on_ending_signal(
    signal: libc::c_int,
    info: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    let path = REMOVE_ON_SIGNAL.swap(std::ptr::null_mut(), Ordering::SeqCst);
    if !path.is_null() {
        // SAFETY: a C string that stays allocated: the swap made this handler its owner, and it is never freed here.
        unsafe { libc::unlink(path) };
    }
    let Some(i) = ENDING_SIGNALS.iter().position(|&s| s == signal) else {
        return;
    };
    let handler = PREVIOUS_HANDLERS[i].load(Ordering::SeqCst);
    let flags = PREVIOUS_FLAGS[i].load(Ordering::SeqCst);
    // SAFETY: `handler` is the previous action of this signal, called as its flags say it takes its arguments; the
    // default action is restored with a zeroed sigaction (SIG_DFL, empty mask) and the signal raised again, which ends
    // the process once this handler returns (the signal is blocked while it runs).
    unsafe {
        if handler == libc::SIG_DFL {
            let default: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, &default, std::ptr::null_mut());
            libc::raise(signal);
        } else if flags & libc::SA_SIGINFO != 0 {
            let previous: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(handler);
            previous(signal, info, context);
        } else {
            let previous: extern "C" fn(libc::c_int) = std::mem::transmute(handler);
            previous(signal);
        }
    }
}

/// Keeps the descriptor `fd` from the programs the server starts: close-on-exec, set before anything starts one. The
/// standard streams (0 to 2) and a descriptor that is not open are left alone; [`announce`] reports the latter.
pub fn keep_from_children(fd: RawFd) {
    if fd <= 2 {
        return;
    }
    // SAFETY: F_GETFD and F_SETFD read and set the descriptor flags of a number; on one that is not open they fail.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags != -1 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Writes `info` as the runtime file under `runtime_dir`: the directory is made with mode 0700 when it does not exist,
/// and the file with mode 0600, whole or not at all (written beside it, then renamed).
pub fn write(runtime_dir: &Path, info: &RuntimeInfo) -> io::Result<RuntimeFile> {
    let path = runtime_dir.join(file_name(info.wayland_display));
    let dir = path.parent().expect("the file is in a directory");
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let partial = dir.join(format!(".server-{}.{}", info.wayland_display, info.pid));
    let text = serde_json::to_vec(info).map_err(io::Error::other)?;
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .and_then(|mut file| {
            // a file left by an earlier run of the same pid keeps its old mode: set it again
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(&text)?;
            file.write_all(b"\n")
        })
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    Ok(RuntimeFile { path })
}

/// Writes `WAYLAND_DISPLAY=<display>` and a line end to the descriptor `fd`, then closes it (standard input, output
/// and error stay open). A descriptor that is not open is an error.
pub fn announce(fd: RawFd, display: &str) -> io::Result<()> {
    // SAFETY: F_GETFD only reads the flags of the descriptor number; it fails when the number is not open.
    if fd < 0 || unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("--ready-fd {fd} is not an open file descriptor"),
        ));
    }
    let line = format!("WAYLAND_DISPLAY={display}\n");
    // SAFETY: the descriptor is open and given to this process for the announcement alone; the file owns it from here
    // and closes it when dropped. Standard streams are borrowed instead.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let written = file.write_all(line.as_bytes());
    if fd <= 2 {
        std::mem::forget(file);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::IntoRawFd;

    #[test]
    fn the_runtime_file_is_private_whole_and_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let info = RuntimeInfo::now("wayland-7", "headless");
        let file = write(dir.path(), &info).unwrap();
        assert_eq!(
            file.path(),
            dir.path().join("cupertino/server-wayland-7.json")
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(file.path()), 0o600);
        assert_eq!(mode(&dir.path().join("cupertino")), 0o700);
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["pid"], std::process::id());
        assert_eq!(v["backend"], "headless");
        assert_eq!(v["renderer"], "gles");
        assert_eq!(v["wayland_display"], "wayland-7");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert!(v["started"].as_u64().unwrap() > 1_600_000_000);
        let names: Vec<_> = std::fs::read_dir(dir.path().join("cupertino"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["server-wayland-7.json"], "no partial file is left");
        let path = file.path().to_path_buf();
        drop(file);
        assert!(!path.exists());
    }

    #[test]
    fn a_ready_descriptor_is_kept_from_children_and_the_standard_streams_are_not() {
        let (_read, write) = std::os::unix::net::UnixStream::pair().unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&write);
        // SAFETY: F_GETFD reads the flags of a descriptor number.
        let cloexec = |fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC != 0;
        // SAFETY: clearing the flags of a descriptor this test owns.
        unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
        assert!(!cloexec(fd));
        keep_from_children(fd);
        assert!(cloexec(fd));
        let stderr = cloexec(2);
        keep_from_children(2);
        assert_eq!(cloexec(2), stderr, "standard error is left alone");
        keep_from_children(100_000);
    }

    #[test]
    fn the_announcement_is_one_line_and_closes_the_descriptor() {
        let (mut read, write) = std::os::unix::net::UnixStream::pair().unwrap();
        let fd = write.into_raw_fd();
        announce(fd, "wayland-3").unwrap();
        let mut text = String::new();
        read.read_to_string(&mut text).unwrap();
        assert_eq!(
            text, "WAYLAND_DISPLAY=wayland-3\n",
            "read to the end: closed"
        );
        assert!(
            announce(fd, "wayland-3").is_err(),
            "closed, so no longer open"
        );
        assert!(announce(-1, "wayland-3").is_err());
    }
}
