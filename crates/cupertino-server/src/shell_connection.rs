//! Connections the server hands to the processes it starts: one end of a socket pair is inserted as a client with the
//! given [`ClientState`], the other goes to the process as `WAYLAND_SOCKET`. Such a client cannot be told apart from an
//! application on the public socket by anything but this state ([`crate::authorization`]), which is why the shell
//! roles and the screen capture are offered to it alone.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;

use smithay::reexports::wayland_server::DisplayHandle;

use crate::authorization::{ClientCapabilitySet, ShellRole};
use crate::state::ClientState;

/// A new client with `state`: the server end is inserted, the other end is returned.
pub fn connect_client(display: &DisplayHandle, state: ClientState) -> std::io::Result<UnixStream> {
    let (server, client) = UnixStream::pair()?;
    display.clone().insert_client(server, Arc::new(state))?;
    Ok(client)
}

/// A connection for a shell process the server supervises, holding `roles` and no other.
pub fn shell_process_connection(
    display: &DisplayHandle,
    roles: &[ShellRole],
) -> std::io::Result<UnixStream> {
    connect_client(
        display,
        ClientState {
            capabilities: ClientCapabilitySet::of(roles.iter().copied()),
            ..ClientState::default()
        },
    )
}

/// Pass `stream` to the process `command` starts as `WAYLAND_SOCKET`.
pub fn hand_over(command: &mut Command, stream: &UnixStream) {
    let fd = stream.as_raw_fd();
    command.env("WAYLAND_SOCKET", fd.to_string());
    // Safety: the closure runs in the child between fork and exec and calls only fcntl, which is async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            // `UnixStream::pair` sets close-on-exec; the child keeps its end across exec
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
