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
//!
//! The portal asks for the same program as its service (`org.cupertino.Session1.StartScreenshot`): the command with
//! [`SERVE_ARG`], on a connection of the same roles, started unless one started so still runs. The server holds a pidfd
//! of every screenshot process it started, so a bus caller is known as the screenshot program ([`PROGRAM`]) by its
//! process while it runs ([`Cupertino::program_of`]).
//!
//! A service that goes away drops its bus name ([`SERVICE_NAME`]) before its pidfd is readable: the name ends with its
//! bus connection, as its descriptors close, and the process is reaped later. The portal sees the name go and asks for
//! the service again in between. So a service process the server saw own the name and no longer owning it, as the bus
//! told it ([`Cupertino::screenshot_service_owner`]) or as it stands when the call comes, does not count as running:
//! it is on its way out, and a new one is started. One that has not owned the name yet still starts, and counts.

use std::os::unix::net::UnixStream;
use std::process::Command;

use cupertino_input::ScreenshotRequest;
use cupertino_peer::ProcessHandle;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;

use crate::authorization::{ClientCapabilitySet, Holder, ShellRole};
use crate::shell_connection::{bind_holder, connect_bound_client, connect_client, hand_over};
use crate::state::{ClientState, Cupertino};

/// The layer-shell namespace of the screenshot process's overlay and toolbar (its floating thumbnail uses another).
pub const OVERLAY_NAMESPACE: &str = "cupertino-screenshot";

/// The server's name for the screenshot program, as `Session1.ProgramOf` answers it.
pub const PROGRAM: &str = "screenshot";

/// The argument that starts the screenshot program as the portal's service ([`SERVICE_NAME`]).
pub const SERVE_ARG: &str = "--serve";

/// The bus name the screenshot program owns while it serves the portal (`cupertino-screenshot`'s `control::BUS_NAME`).
pub const SERVICE_NAME: &str = "org.cupertino.Screenshot1";

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

fn capture_capabilities() -> ClientCapabilitySet {
    ClientCapabilitySet::of([ShellRole::Capture, ShellRole::Session])
}

/// What a screenshot process was started for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// One capture, for a shortcut.
    Shortcut,
    /// The portal's service ([`SERVE_ARG`]).
    Serve,
}

/// What the server knows of a service process's hold on [`SERVICE_NAME`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Name {
    /// Not seen owning it yet: the process is still starting.
    Unseen,
    /// Seen owning it.
    Owned,
    /// Seen owning it and then not: the service is going away (see the module documentation).
    Dropped,
}

/// A screenshot process the server started.
#[derive(Debug)]
struct Started {
    process: ProcessHandle,
    mode: Mode,
    name: Name,
}

/// The screenshot processes the server started and has not seen exit, each held by a pidfd opened before it could be
/// reaped. One that exited is dropped at the next start.
#[derive(Debug, Default)]
pub struct Launched {
    running: Vec<Started>,
}

impl Launched {
    /// Whether `process` is a screenshot process the server started that still runs.
    pub(crate) fn holds(&self, process: &ProcessHandle) -> bool {
        self.running.iter().any(|s| s.process.is(process))
    }

    /// Whether a screenshot process started as the portal's service still runs and has not dropped its name.
    fn serving(&self) -> bool {
        self.running
            .iter()
            .any(|s| s.mode == Mode::Serve && s.name != Name::Dropped && !s.process.has_exited())
    }

    /// [`SERVICE_NAME`] is owned by `owner`, or by no process the server can name.
    fn owner_is(&mut self, owner: Option<&ProcessHandle>) {
        for started in self.running.iter_mut().filter(|s| s.mode == Mode::Serve) {
            let owns = owner.is_some_and(|owner| started.process.is(owner));
            started.name = match (started.name, owns) {
                (_, true) => Name::Owned,
                (Name::Owned | Name::Dropped, false) => Name::Dropped,
                (Name::Unseen, false) => Name::Unseen,
            };
        }
    }

    fn add(&mut self, process: ProcessHandle, mode: Mode) {
        self.running.retain(|s| !s.process.has_exited());
        self.running.push(Started {
            process,
            mode,
            name: Name::Unseen,
        });
    }
}

impl Cupertino {
    /// A new privileged client: the server inserts one end of a socket pair and returns the other, for a process the
    /// server starts (`WAYLAND_SOCKET`) or, in tests, for a client in the same process.
    pub fn privileged_client(&mut self) -> std::io::Result<UnixStream> {
        connect_client(
            &self.display_handle,
            ClientState::detached(capture_capabilities()),
        )
    }

    /// Whether `surface` is drawn by a screenshot process the server started and still holds: on the connection the
    /// server made for it (its holder) or on one the process opened itself (the connection's peer). Known by the
    /// process the server holds the pidfd of, never by a title, an app id or a namespace, so every surface of the
    /// program is one, whatever it draws (`crate::control` keeps desktop control away from them).
    pub fn is_screenshot_surface(&self, surface: &WlSurface) -> bool {
        let Some(client) = surface.client() else {
            return false;
        };
        let Some(data) = client.get_data::<ClientState>() else {
            return false;
        };
        let holder = match data.holder.get() {
            Some(Holder::Process(process)) => Some(process),
            _ => None,
        };
        holder
            .into_iter()
            .chain(data.peer.as_ref())
            .any(|process| self.capture.launched.holds(process))
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
        if self.login_screen {
            tracing::warn!(?request, "the login screen starts no programs");
            return;
        }
        if self.screenshot_overlay_shown() {
            tracing::info!(?request, "a screenshot overlay is already open");
            return;
        }
        match self.spawn_screenshot(mode_args(request), Mode::Shortcut) {
            Ok(pid) => tracing::info!(pid, ?request, "screenshot process started"),
            Err(e) => tracing::warn!(?request, error = %e, "the screenshot process did not start"),
        }
    }

    /// The bus says [`SERVICE_NAME`] is now owned by `owner`, or by no process the server can name (no owner): a service
    /// process seen owning it that no longer does is going away.
    pub fn screenshot_service_owner(&mut self, owner: Option<&ProcessHandle>) {
        self.capture.launched.owner_is(owner);
    }

    /// `StartScreenshot`: start the screenshot program as the portal's service ([`SERVE_ARG`]) unless one started so
    /// still runs and has not dropped [`SERVICE_NAME`]; `owner` is the name's owner when the call came, as for
    /// [`Cupertino::screenshot_service_owner`]. `Ok` once it runs; the error says why it did not start.
    pub fn start_serving_screenshot(
        &mut self,
        owner: Option<&ProcessHandle>,
    ) -> Result<(), String> {
        if self.login_screen {
            return Err("the login screen starts no programs".into());
        }
        self.capture.launched.owner_is(owner);
        if self.capture.launched.serving() {
            return Ok(());
        }
        let pid = self.spawn_screenshot(&[SERVE_ARG], Mode::Serve)?;
        tracing::info!(pid, "screenshot service started");
        Ok(())
    }

    /// Spawn `config.screenshot_command` with `args` on a connection of its own holding the capture roles, bound to
    /// its process, which the server holds a pidfd of and reaps on a thread of its own. Its pid, or why it did not start.
    fn spawn_screenshot(&mut self, args: &[&str], mode: Mode) -> Result<u32, String> {
        let command_line = self.config.screenshot_command.clone();
        let (program, base) = command_line
            .split_first()
            .expect("the screenshot command is validated non-empty at load");
        let mut command = Command::new(program);
        command.args(base).args(args);
        let (stream, state) = connect_bound_client(
            &self.display_handle,
            ClientState {
                capabilities: capture_capabilities(),
                ..ClientState::default()
            },
        )
        .map_err(|e| format!("creating the screenshot process's connection failed: {e}"))?;
        // the process reaches the server only through its own connection, never through the public socket
        hand_over(&mut command, &stream);
        command.env_remove("WAYLAND_DISPLAY");
        // on a failed spawn the server's end of the pair sees the hang-up and the client is dropped
        let mut child = command
            .spawn()
            .map_err(|e| format!("starting {program} failed: {e}"))?;
        drop(stream);
        let pid = child.id();
        // the child is not reaped yet, so its pid names it alone
        let held = ProcessHandle::open(pid).and_then(|process| {
            bind_holder(&state, pid)?;
            Ok(process)
        });
        let process = match held {
            Ok(process) => process,
            Err(e) => {
                crate::shell_connection::end_unbound(child);
                return Err(format!(
                    "binding the screenshot process {pid} to its connection failed: {e}"
                ));
            }
        };
        self.capture.launched.add(process, mode);
        std::thread::spawn(move || match child.wait() {
            Ok(status) => tracing::info!(pid, %status, "screenshot process exited"),
            Err(e) => {
                tracing::warn!(pid, error = %e, "waiting for the screenshot process failed")
            }
        });
        Ok(pid)
    }
}
