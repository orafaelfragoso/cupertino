//! Who may use data control (`ext-data-control-v1`, `zwlr-data-control-v1`): a clipboard manager reads every selection as
//! it is set and replaces it without focus, so the globals are offered only to a connection holding
//! [`ShellRole::DataControl`] and to an unsandboxed application the person allowed in `data-control.conf`
//! ([`cupertino_defaults::data_control`]). Everything else is refused, a client of a security context whatever it runs.
//!
//! The application is the process the kernel named as it connected (`SO_PEERPIDFD`): its executable, read through
//! `/proc` while the pidfd shows the process still running, must be the path of an entry, and either the entry's pin
//! must match the running file, or only root can change the file at that path and it is the running file. A client
//! without a pidfd (Linux before 6.5) is refused. The rule runs where the globals are offered and bound, so a refused
//! client neither sees nor binds them.
//!
//! The file is read at start and on every change. A client seen as allowed that no longer is after a change is
//! disconnected: the protocol has no way to end its data control objects.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cupertino_defaults::data_control::{refusal, DataControlConf, Entry};
use cupertino_peer::{Caller, ProcessHandle};
use smithay::reexports::wayland_server::backend::protocol::ProtocolError;
use smithay::reexports::wayland_server::backend::{ClientId, DisconnectReason};
use smithay::reexports::wayland_server::Client;

use crate::authorization::ShellRole;
use crate::state::{ClientState, Cupertino};

/// The allowed programs, shared with the filter of the globals.
#[derive(Debug, Default)]
pub struct DataControlAccess {
    allowed: Mutex<Vec<Entry>>,
    /// The clients the globals were offered to since the last change of the file.
    offered: Mutex<HashSet<ClientId>>,
}

impl DataControlAccess {
    /// The filter of the globals: whether `client` may see and bind them.
    pub fn may_view(&self, client: &Client) -> bool {
        let allowed = client
            .get_data::<ClientState>()
            .is_some_and(|data| may_control(data, &lock(&self.allowed)));
        if allowed {
            lock(&self.offered).insert(client.id());
        }
        allowed
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the client of `data` may use data control while `allowed` are the allowed programs.
fn may_control(data: &ClientState, allowed: &[Entry]) -> bool {
    if data.granted().contains(ShellRole::DataControl) {
        return true;
    }
    if data.sandboxed || allowed.is_empty() {
        return false;
    }
    let Some(process) = data.peer.as_ref().and_then(|p| p.try_clone().ok()) else {
        return false;
    };
    match running_program(process, Path::new("/proc")) {
        Some(running) => {
            let allows = allows(allowed, &running);
            tracing::debug!(program = %running.exe.display(), allows, "data control: an application asks");
            allows
        }
        None => false,
    }
}

/// The program a process runs: the path `/proc` names and the file it runs from.
struct Running {
    exe: PathBuf,
    image: std::fs::Metadata,
}

/// The program `process` runs; `None` when it is sandboxed, cannot be looked at, or exited while it was looked at (its
/// process id may then name another process).
fn running_program(process: ProcessHandle, proc_root: &Path) -> Option<Running> {
    let Caller::Process(caller) = Caller::of_process(process, proc_root) else {
        return None;
    };
    if caller.sandbox.is_some() {
        return None;
    }
    let exe = caller.exe?;
    let image =
        std::fs::metadata(proc_root.join(caller.process.pid.to_string()).join("exe")).ok()?;
    (!caller.process.has_exited()).then_some(Running { exe, image })
}

/// Whether an entry of `allowed` allows `running`. A program whose file was replaced since it started runs from a
/// deleted file, which `/proc` names with ` (deleted)` after its path: it matches no entry.
fn allows(allowed: &[Entry], running: &Running) -> bool {
    use std::os::unix::fs::MetadataExt;
    let is_running = |path: &Path| {
        std::fs::metadata(path)
            .is_ok_and(|m| (m.dev(), m.ino()) == (running.image.dev(), running.image.ino()))
    };
    refusal(&running.exe).is_none()
        && allowed.iter().any(|entry| {
            entry.program == running.exe
                && entry.allows_file(&running.image)
                && (entry.pin.is_some() || is_running(&entry.program))
        })
}

impl Cupertino {
    /// Read `data-control.conf` at `path` and apply it: a file that is refused or cannot be read allows nothing.
    pub fn reload_data_control(&mut self, path: &Path) {
        let allowed = match DataControlConf::load(path) {
            Ok((conf, problems)) => {
                for problem in problems {
                    tracing::warn!(path = %path.display(), line = problem.line, problem = problem.message, "data control: ignoring a line");
                }
                conf.allowed
            }
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "data control: the file is refused, no application is allowed");
                Vec::new()
            }
        };
        self.set_data_control_allowed(allowed);
    }

    /// Allow `allowed`, and disconnect each client the globals were offered to that may not use them any more.
    pub fn set_data_control_allowed(&mut self, allowed: Vec<Entry>) {
        let access = &self.data_control_access;
        *lock(&access.allowed) = allowed;
        let offered = std::mem::take(&mut *lock(&access.offered));
        let backend = self.display_handle.backend_handle();
        for id in offered {
            let Ok(data) = backend.get_client_data(id.clone()) else {
                continue;
            };
            let Some(state) = (*data).downcast_ref::<ClientState>() else {
                continue;
            };
            if may_control(state, &lock(&access.allowed)) {
                lock(&access.offered).insert(id);
                continue;
            }
            tracing::info!(client = ?id, "data control: no longer allowed, disconnecting the client");
            backend.kill_client(
                id,
                DisconnectReason::ProtocolError(ProtocolError {
                    code: crate::handlers::WL_DISPLAY_ERROR_IMPLEMENTATION,
                    object_id: crate::handlers::WL_DISPLAY_ID,
                    object_interface: "wl_display".into(),
                    message: "data control is no longer allowed".into(),
                }),
            );
        }
    }
}
