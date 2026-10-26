//! Starting the screenshot process on a privileged connection.
//!
//! The design binds its five screenshot hot keys in the system UI process, and each press spawns the capture tool
//! with the flags of that key; the tool lives for one capture. The server does the same: a
//! screenshot shortcut starts `config.screenshot_command` with the mode appended (`--mode screen|selection|toolbar`,
//! `--clipboard` for the clipboard keys). The process gets its connection as `WAYLAND_SOCKET`, one end of a socket pair
//! whose other end the server inserted as a privileged client ([`ClientState::privileged`]): that is what lets it see
//! the capture globals, and why it is not supervised: it exits when its capture is done.
//!
//! While a screenshot overlay is on screen (a layer surface of namespace [`OVERLAY_NAMESPACE`]) a further shortcut
//! starts nothing: the overlay is already in front and has the keyboard.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;

use cupertino_input::ScreenshotRequest;
use smithay::desktop::layer_map_for_output;

use crate::state::{ClientState, Cupertino};

/// The layer-shell namespace of the screenshot process's overlay and toolbar (its floating thumbnail uses another).
pub const OVERLAY_NAMESPACE: &str = "cupertino-screenshot";

/// The arguments that select `request`'s mode in the screenshot process.
pub fn mode_args(request: ScreenshotRequest) -> &'static [&'static str] {
    match request {
        ScreenshotRequest::Screen => &["--mode", "screen"],
        ScreenshotRequest::ScreenToClipboard => &["--mode", "screen", "--clipboard"],
        ScreenshotRequest::Selection => &["--mode", "selection"],
        ScreenshotRequest::SelectionToClipboard => &["--mode", "selection", "--clipboard"],
        ScreenshotRequest::Toolbar => &["--mode", "toolbar"],
    }
}

impl Cupertino {
    /// A new privileged client: the server inserts one end of a socket pair and returns the other, for a process the
    /// server starts (`WAYLAND_SOCKET`) or, in tests, for a client in the same process.
    pub fn privileged_client(&mut self) -> std::io::Result<UnixStream> {
        let (server, client) = UnixStream::pair()?;
        self.display_handle.insert_client(
            server,
            Arc::new(ClientState {
                privileged: true,
                ..ClientState::default()
            }),
        )?;
        Ok(client)
    }

    /// Whether a screenshot overlay or toolbar is on an output.
    pub fn screenshot_overlay_shown(&self) -> bool {
        self.outputs.iter().any(|o| {
            layer_map_for_output(&o.output)
                .layers()
                .any(|l| l.namespace() == OVERLAY_NAMESPACE)
        })
    }

    /// Start the screenshot process for a shortcut (see the module documentation).
    pub fn start_screenshot(&mut self, request: ScreenshotRequest) {
        if self.screenshot_overlay_shown() {
            tracing::info!(?request, "a screenshot overlay is already open");
            return;
        }
        let command_line = self.config.screenshot_command.clone();
        let (program, args) = command_line
            .split_first()
            .expect("the screenshot command is validated non-empty at load");
        let mut command = Command::new(program);
        command.args(args).args(mode_args(request));
        let stream = match self.privileged_client() {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(error = %e, "creating the screenshot process's connection failed");
                return;
            }
        };
        let fd = stream.as_raw_fd();
        // the process reaches the server only through its own connection, never through the public socket
        command
            .env("WAYLAND_SOCKET", fd.to_string())
            .env_remove("WAYLAND_DISPLAY");
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
        match command.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                tracing::info!(pid, ?request, "screenshot process started");
                std::thread::spawn(move || match child.wait() {
                    Ok(status) => tracing::info!(pid, %status, "screenshot process exited"),
                    Err(e) => {
                        tracing::warn!(pid, error = %e, "waiting for the screenshot process failed")
                    }
                });
            }
            // the server's end of the pair sees the hang-up and the client is dropped
            Err(e) => tracing::warn!(program, error = %e, "starting the screenshot process failed"),
        }
        drop(stream);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shortcut_selects_its_mode_and_destination() {
        assert_eq!(mode_args(ScreenshotRequest::Screen), ["--mode", "screen"]);
        assert_eq!(
            mode_args(ScreenshotRequest::ScreenToClipboard),
            ["--mode", "screen", "--clipboard"]
        );
        assert_eq!(
            mode_args(ScreenshotRequest::Selection),
            ["--mode", "selection"]
        );
        assert_eq!(
            mode_args(ScreenshotRequest::SelectionToClipboard),
            ["--mode", "selection", "--clipboard"]
        );
        assert_eq!(mode_args(ScreenshotRequest::Toolbar), ["--mode", "toolbar"]);
    }
}
