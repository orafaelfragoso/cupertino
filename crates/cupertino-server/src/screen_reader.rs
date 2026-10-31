//! The screen reader at the login screen. The session manager's worker runs it and gives the login screen's compositor,
//! and no other process, a socket to turn it on and off (`CUPERTINO_READER_FD`). Super+Alt+S, pressed on a keyboard, is the
//! only thing that toggles it: the chord is matched on the keys from the input devices, before any client or overlay sees
//! them, so no client, assistive technology or bus call can start or stop the reader.
//!
//! The socket closes on `exec`, and its variable is removed before any thread or child exists. The login screen's
//! compositor is not dumpable, so another process of the greeter's user cannot borrow the socket through `ptrace`.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use smithay::input::keyboard::{keysyms, ModifiersState};

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

/// Make this process undumpable: no core file, and no `ptrace` by another process of the same user.
pub fn harden() {
    // Safety: `prctl(PR_SET_DUMPABLE)` takes integers only.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        tracing::error!(error = %std::io::Error::last_os_error(), "the login screen stays dumpable");
    }
}

/// Super+Alt+S, with no other modifier.
pub fn is_chord(sym: u32, mods: &ModifiersState) -> bool {
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

    /// A key from an input device: whether it is Super+Alt+S at the login screen (or the release of its key), which the
    /// compositor takes. A press asks the worker to toggle the reader.
    pub fn screen_reader_key(
        &mut self,
        code: u32,
        sym: u32,
        mods: &ModifiersState,
        pressed: bool,
    ) -> bool {
        let Some(reader) = self.screen_reader.as_mut() else {
            return false;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(logo: bool, alt: bool, ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            logo,
            alt,
            ctrl,
            shift,
            ..ModifiersState::default()
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
