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

use crate::authorization::{ClientCapabilitySet, Holder, ShellRole};
use crate::force_quit::ProcessHandle;
use crate::state::ClientState;

/// The client's end of a connection and the state the server keeps for it.
pub type BoundConnection = (UnixStream, Arc<ClientState>);

/// A new client with `state`: the server end is inserted, the other end is returned.
pub fn connect_client(display: &DisplayHandle, state: ClientState) -> std::io::Result<UnixStream> {
    connect_bound_client(display, state).map(|(stream, _)| stream)
}

/// Like [`connect_client`], and the state the server keeps for the connection, to bind the process to ([`bind_holder`]).
pub fn connect_bound_client(
    display: &DisplayHandle,
    state: ClientState,
) -> std::io::Result<BoundConnection> {
    let (server, client) = UnixStream::pair()?;
    let state = Arc::new(state);
    display.clone().insert_client(server, state.clone())?;
    Ok((client, state))
}

/// The state of a connection for a shell process the server supervises, holding `roles` and no other.
pub fn shell_process_state(roles: &[ShellRole]) -> ClientState {
    ClientState {
        capabilities: ClientCapabilitySet::of(roles.iter().copied()),
        ..ClientState::default()
    }
}

/// A connection for a client that runs in the server's own process (tests), holding `roles` and no other for as long
/// as it is open. A shell process the server starts is bound to its process instead ([`bind_holder`]).
pub fn shell_process_connection(
    display: &DisplayHandle,
    roles: &[ShellRole],
) -> std::io::Result<UnixStream> {
    connect_client(
        display,
        ClientState::detached(ClientCapabilitySet::of(roles.iter().copied())),
    )
}

/// Bind the connection to the process started for it: its capabilities end when the process exits, so a connection
/// kept open by anything else (a descriptor passed on, a leftover child) holds nothing. The child has not been waited
/// for yet, so its pid cannot belong to another process. A connection that is not bound holds nothing; when binding
/// fails the caller ends the process.
pub fn bind_holder(state: &ClientState, pid: u32) -> std::io::Result<()> {
    let handle = ProcessHandle::open(pid)?;
    state
        .holder
        .set(Holder::Process(handle))
        .map_err(|_| std::io::Error::other("the connection already has a holder"))
}

/// Pass `stream` to the process `command` starts as `WAYLAND_SOCKET`.
pub fn hand_over(command: &mut Command, stream: &UnixStream) {
    let fd = stream.as_raw_fd();
    command.env("WAYLAND_SOCKET", fd.to_string());
    crate::file_limit::restore_in_child(command);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_connection_names_the_process_started_for_it() {
        let state = shell_process_state(&[ShellRole::MenuBar]);
        assert!(
            state.process().is_none(),
            "no process before one is started"
        );
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        bind_holder(&state, child.id()).unwrap();
        let pid = state.process().map(|process| process.pid);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(pid, Some(child.id()));
    }
}
