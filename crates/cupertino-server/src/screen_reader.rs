//! The screen reader at the login screen. The session manager's worker runs it and gives the login screen's compositor,
//! and no other process, a socket to turn it on and off (`CUPERTINO_READER_FD`). Super+Alt+S, pressed on a keyboard, is the
//! only thing that toggles it: the chord is matched on the keys from the input devices, before any client or overlay sees
//! them, so no client, assistive technology or bus call can start or stop the reader.
//!
//! The socket closes on `exec`, and its variable is removed before any thread or child exists. The login screen's
//! compositor is not dumpable, so another process of the greeter's user cannot borrow the socket through `ptrace`.
//!
//! In a person's session the compositor runs the reader itself ([`SessionReader`]), as one of its own children, while
//! `accessibility.conf` says `screen_reader = on`. Settings sets that key through `Session1.SetAccessibility`, and
//! Super+Alt+S on a keyboard turns the reader on or off and stores the key the same way.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_peer::ProcessHandle;
use smithay::input::keyboard::keysyms;

use crate::input::Mods;
use crate::state::Cupertino;

/// The variable that names the socket, and the only descriptor it may name.
pub const READER_FD_ENV: &str = "CUPERTINO_READER_FD";
const READER_FD: libc::c_int = 3;

/// The toggle socket, taken at the login screen only.
#[derive(Debug)]
pub struct ScreenReader {
    socket: OwnedFd,
    /// The key of a chord that was pressed: its release is the compositor's too.
    down: Option<u32>,
}

/// Take the socket the worker gave the login screen, before any thread or child exists. Anywhere but at the login screen
/// the variable is removed and nothing is taken. `None` too when it names anything but a socket on descriptor 3.
pub fn take(login_screen: bool) -> Option<OwnedFd> {
    let named = std::env::var_os(READER_FD_ENV);
    std::env::remove_var(READER_FD_ENV);
    if !login_screen || named.as_deref() != Some(std::ffi::OsStr::new("3")) {
        return None;
    }
    // Safety: `fstat` writes into the struct it is given; descriptor 3 is not owned by anything in this process yet.
    let socket = unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(READER_FD, &mut stat) != 0 || stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return None;
        }
        OwnedFd::from_raw_fd(READER_FD)
    };
    rustix::io::fcntl_setfd(&socket, rustix::io::FdFlags::CLOEXEC).ok()?;
    Some(socket)
}

/// Make this process undumpable: no core file, and no `ptrace` by another process of the same user. False when it stays
/// dumpable, and then it must not hold the socket.
pub fn harden() -> bool {
    // Safety: `prctl(PR_SET_DUMPABLE)` takes integers only.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        tracing::error!(error = %std::io::Error::last_os_error(), "the login screen stays dumpable: no screen reader");
        return false;
    }
    true
}

/// Super+Alt+S, with no other modifier, on the physical modifier keys (a level-3 Alt is Alt).
pub fn is_chord(sym: u32, mods: &Mods) -> bool {
    sym == keysyms::KEY_s && mods.logo && mods.alt && !mods.ctrl && !mods.shift
}

impl ScreenReader {
    pub fn new(socket: OwnedFd) -> Self {
        ScreenReader { socket, down: None }
    }

    fn send(&self, request: &str) {
        if let Err(e) = rustix::net::send(
            &self.socket,
            request.as_bytes(),
            rustix::net::SendFlags::DONTWAIT,
        ) {
            tracing::warn!(error = %e, "the screen reader's worker did not take a request");
        }
    }
}

impl Cupertino {
    /// Hand the screen reader's socket to the server once its Wayland socket is known: the worker learns the name, and
    /// starts the reader if the machine's settings ask for it.
    pub fn start_screen_reader(&mut self, socket: OwnedFd) {
        let Some(name) = self.socket_name.clone() else {
            tracing::warn!("the login screen has no Wayland socket: no screen reader");
            return;
        };
        let reader = ScreenReader::new(socket);
        reader.send(&format!("ready {name}"));
        let Ok(watched) = reader.socket.try_clone() else {
            return;
        };
        use smithay::reexports::calloop::generic::Generic;
        use smithay::reexports::calloop::{Interest, Mode, PostAction};
        let inserted = self.loop_handle.insert_source(
            Generic::new(watched, Interest::READ, Mode::Level),
            |_, socket, _| {
                let mut reply = [0u8; 32];
                // Safety: the descriptor stays owned by the source; it is only read here.
                let socket = unsafe { std::os::fd::BorrowedFd::borrow_raw(socket.as_raw_fd()) };
                match rustix::net::recv(socket, &mut reply, rustix::net::RecvFlags::DONTWAIT) {
                    Ok((0, _)) => {
                        tracing::warn!("the screen reader's worker closed its socket");
                        Ok(PostAction::Remove)
                    }
                    Ok((count, _)) => {
                        let state =
                            String::from_utf8_lossy(&reply[..count.min(reply.len())]).into_owned();
                        tracing::info!(state, "screen reader");
                        Ok(PostAction::Continue)
                    }
                    Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                        Ok(PostAction::Continue)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "reading the screen reader's socket");
                        Ok(PostAction::Remove)
                    }
                }
            },
        );
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "cannot watch the screen reader's socket");
        }
        self.screen_reader = Some(reader);
    }

    /// A key from an input device: whether it is Super+Alt+S (or the release of its key), which the compositor takes. At
    /// the login screen a press asks the worker to toggle the reader; in a person's session it toggles the session's.
    ///
    /// The chord acts while the session is locked as well, on purpose: it is an accessibility shortcut a person may need
    /// before they can unlock, and it starts or stops the reader only. Every other global shortcut is off while the
    /// session is locked (`KeyRouter::key`).
    pub fn screen_reader_key(&mut self, code: u32, sym: u32, mods: &Mods, pressed: bool) -> bool {
        let Some(reader) = self.screen_reader.as_mut() else {
            return self.session_reader_key(code, sym, mods, pressed);
        };
        if !pressed {
            return reader.down.take_if(|down| *down == code).is_some();
        }
        if !is_chord(sym, mods) {
            return false;
        }
        reader.down = Some(code);
        if let Some(name) = self.socket_name.clone() {
            reader.send(&format!("toggle {name}"));
        }
        true
    }
}

/// How long a session reader has to end after `SIGTERM` before it is killed.
const SESSION_STOP_GRACE: Duration = Duration::from_secs(2);
/// How often a stopping reader is checked while it has its grace.
const SESSION_STOP_POLL: Duration = Duration::from_millis(50);

/// The screen reader of a person's session: a child of the compositor, started and stopped to match the `screen_reader`
/// key of `accessibility.conf`.
#[derive(Debug, Default)]
pub struct SessionReader {
    /// The reader to start: the installed one in a session on the hardware, `None` at the login screen, in nested and
    /// offscreen runs and when none is installed. Tests name another program.
    pub program: Option<PathBuf>,
    running: Option<ProcessHandle>,
    /// The key of a chord that was pressed: its release is the compositor's too.
    down: Option<u32>,
}

impl SessionReader {
    /// The reader of a session that starts `program`.
    pub fn new(program: Option<PathBuf>) -> Self {
        SessionReader {
            program,
            running: None,
            down: None,
        }
    }

    /// Whether the reader runs now. A reader that ended by itself is forgotten.
    pub fn is_running(&mut self) -> bool {
        if self.running.as_ref().is_some_and(ProcessHandle::has_exited) {
            tracing::warn!("the screen reader ended by itself");
            self.running = None;
        }
        self.running.is_some()
    }

    /// The process id of the running reader.
    pub fn pid(&mut self) -> Option<u32> {
        self.is_running()
            .then(|| self.running.as_ref().map(|r| r.pid))
            .flatten()
    }

    /// Start the reader as a client of the Wayland socket `socket`: its own process group, standard input from
    /// `/dev/null`, the compositor's environment with the session's display. A thread reaps it when it ends.
    fn start(&mut self, socket: Option<&str>) {
        let Some(program) = &self.program else {
            tracing::warn!("no screen reader is installed");
            return;
        };
        let mut command = std::process::Command::new(program);
        command
            .stdin(std::process::Stdio::null())
            .env("XDG_SESSION_TYPE", "wayland")
            .env("XDG_CURRENT_DESKTOP", crate::state::CURRENT_DESKTOP)
            .env_remove("WAYLAND_SOCKET");
        if let Some(socket) = socket {
            command.env("WAYLAND_DISPLAY", socket);
        }
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        crate::file_limit::restore_in_child(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                tracing::warn!(program = %program.display(), error = %e, "the screen reader cannot start");
                return;
            }
        };
        // the child is not reaped before the thread below waits for it, so its pid names it here
        let handle = ProcessHandle::open(child.id());
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        match handle {
            Ok(handle) => {
                tracing::info!(program = %program.display(), pid = handle.pid, "the screen reader started");
                self.running = Some(handle);
            }
            Err(e) => tracing::warn!(error = %e, "the screen reader started but cannot be held"),
        }
    }

    /// Ask the reader to end; a thread kills it when it is still there after [`SESSION_STOP_GRACE`].
    fn stop(&mut self) {
        let Some(process) = self.running.take() else {
            return;
        };
        if process.send(libc::SIGTERM).is_err() {
            return;
        }
        tracing::info!(pid = process.pid, "the screen reader stops");
        std::thread::spawn(move || {
            let end = Instant::now() + SESSION_STOP_GRACE;
            while !process.has_exited() && Instant::now() < end {
                std::thread::sleep(SESSION_STOP_POLL);
            }
            if !process.has_exited() {
                let _ = process.send(libc::SIGKILL);
            }
        });
    }
}

impl Drop for SessionReader {
    fn drop(&mut self) {
        if let Some(process) = self.running.take() {
            let _ = process.send(libc::SIGTERM);
        }
    }
}

impl Cupertino {
    /// Start or stop the session's reader to match `screen_reader` of `accessibility.conf`. The login screen's compositor
    /// starts no program: its reader is the session manager's.
    pub fn sync_screen_reader(&mut self) {
        if self.login_screen {
            return;
        }
        let wanted = self.accessibility.settings.screen_reader;
        match (wanted, self.session_reader.is_running()) {
            // without a program (nested and offscreen runs, the start before the lookup) there is nothing to start
            (true, false) if self.session_reader.program.is_some() => {
                let socket = self.socket_name.clone();
                self.session_reader.start(socket.as_deref());
            }
            (false, true) => self.session_reader.stop(),
            _ => {}
        }
    }

    /// Super+Alt+S in a person's session, when a reader is installed: a press turns the reader off when it runs and on
    /// when it does not, and stores the choice in `accessibility.conf`. Whether the compositor took the key.
    fn session_reader_key(&mut self, code: u32, sym: u32, mods: &Mods, pressed: bool) -> bool {
        if self.login_screen || self.session_reader.program.is_none() {
            return false;
        }
        if !pressed {
            return self
                .session_reader
                .down
                .take_if(|down| *down == code)
                .is_some();
        }
        if !is_chord(sym, mods) {
            return false;
        }
        self.session_reader.down = Some(code);
        let on = !self.session_reader.is_running();
        tracing::info!(on, "screen reader shortcut");
        let mut settings = self.accessibility.settings;
        settings.screen_reader = on;
        self.set_accessibility(settings);
        // the stored key may have said `on` already, for a reader that ended by itself
        self.sync_screen_reader();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(logo: bool, alt: bool, ctrl: bool, shift: bool) -> Mods {
        Mods {
            logo,
            alt,
            ctrl,
            shift,
        }
    }

    #[test]
    fn only_super_alt_s_is_the_chord() {
        assert!(is_chord(keysyms::KEY_s, &mods(true, true, false, false)));
        assert!(!is_chord(keysyms::KEY_s, &mods(true, false, false, false)));
        assert!(!is_chord(keysyms::KEY_s, &mods(true, true, true, false)));
        assert!(!is_chord(keysyms::KEY_S, &mods(true, true, false, true)));
        assert!(!is_chord(keysyms::KEY_a, &mods(true, true, false, false)));
    }
}
