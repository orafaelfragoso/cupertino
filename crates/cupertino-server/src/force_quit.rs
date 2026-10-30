//! Force Quit: `cupertino_dock_v1.force_quit_app`, sent by the dock's Force Quit dialog after the person confirmed. An
//! application whose client does not answer its pings is killed at once; any other is asked to close (`xdg_toplevel.close`
//! for each window) and sent SIGTERM, and SIGKILL follows [`KILL_GRACE`] seconds later for what is still connected then.
//!
//! The process is the one the kernel named as the client connected (`SO_PEERPIDFD`, Linux 6.5), held as a pidfd from
//! the moment the client connected, so a process id that is reused meanwhile is never signalled. A client the kernel
//! named no process for (an older kernel) is not signalled: the socket credentials are a snapshot from connect time and
//! cannot tell that their process id has been reused. Only a client that is
//! an application of its own is signalled: not the server, init, a shell process, or a client that serves windows of more
//! than one application (a proxy).

use cupertino_wm::AppId;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Resource;

use crate::state::{ClientState, Cupertino};

/// Seconds between SIGTERM and the SIGKILL for a process that is still there.
pub const KILL_GRACE: f64 = 3.0;

const SIGTERM: i32 = 15;
pub(crate) const SIGKILL: i32 = 9;

pub use cupertino_peer::ProcessHandle;

/// Delivers a signal to a held process; replaced in tests.
pub type SendSignal = Box<dyn FnMut(&ProcessHandle, i32)>;

/// The default `Cupertino.send_signal`: deliver through the pidfd, logging a failure.
pub fn signal_process(process: &ProcessHandle, signal: i32) {
    if let Err(e) = process.send(signal) {
        tracing::warn!(pid = process.pid, signal, error = %e, "force quit: the signal was not delivered");
    }
}

/// The process behind an application's client, held by a pidfd: the one the kernel named when the client connected
/// (`SO_PEERPIDFD`, kept in [`ClientState::peer`]). No process id is looked up, so none can have been reused. A client
/// without one is refused, as is one whose process has exited.
fn client_process(state: &ClientState) -> std::io::Result<ProcessHandle> {
    let gone = |what: &str| std::io::Error::other(what.to_string());
    let peer = state.peer.as_ref().ok_or_else(|| {
        gone("the kernel named no process for this client (SO_PEERPIDFD needs Linux 6.5)")
    })?;
    let process = peer.try_clone()?;
    if process.has_exited() {
        return Err(gone("the process that connected has exited"));
    }
    Ok(process)
}

/// Whether a process may be signalled: never init, the pids `kill(2)` gives a meaning to, or the server itself.
fn signalable(pid: u32, own: u32) -> bool {
    pid > 1 && pid != own
}

/// Whether every window of a client is a window of `app`: a client that serves several applications is a proxy, and
/// ending its process would end them all.
fn serves_only(app: AppId, apps_of_its_windows: impl IntoIterator<Item = AppId>) -> bool {
    apps_of_its_windows.into_iter().all(|other| other == app)
}

struct Promise {
    at: f64,
    processes: Vec<(Client, ProcessHandle)>,
}

/// The SIGKILLs promised to applications that were asked to quit.
pub struct ForceQuits {
    promised: Vec<Promise>,
    /// The server's own process id; replaced in tests, whose clients are threads of the server's process.
    pub own_pid: u32,
}

impl Default for ForceQuits {
    fn default() -> Self {
        Self {
            promised: Vec::new(),
            own_pid: std::process::id(),
        }
    }
}

impl ForceQuits {
    pub fn pending(&self) -> usize {
        self.promised.len()
    }

    /// When the next promised SIGKILL is due.
    pub fn next_deadline(&self) -> Option<f64> {
        self.promised.iter().map(|p| p.at).reduce(f64::min)
    }
}

impl Cupertino {
    /// The clients behind the toplevels of `app` and their processes, each once, left out when they may not be signalled.
    pub(crate) fn app_processes(&self, app: AppId, identity: &str) -> Vec<(Client, ProcessHandle)> {
        let shell_pids = self.supervisor.pids();
        let mut found: Vec<(Client, ProcessHandle)> = Vec::new();
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            let Some(client) = entry.surface().client() else {
                continue;
            };
            if found.iter().any(|(known, _)| known.id() == client.id()) {
                continue;
            }
            // the server's own connections and its shell processes are not applications
            if client
                .get_data::<ClientState>()
                .is_none_or(|c| c.internal || !c.capabilities.is_empty())
            {
                continue;
            }
            let apps_of_its_windows = self
                .windows
                .iter()
                .filter(|e| e.surface().client().is_some_and(|c| c.id() == client.id()))
                .filter_map(|e| self.wm.window(e.id).map(|w| w.app));
            if !serves_only(app, apps_of_its_windows) {
                tracing::warn!(
                    app = identity,
                    "force quit: the client serves more than one application, not signalling it"
                );
                continue;
            }
            let Some(state) = client.get_data::<ClientState>() else {
                continue;
            };
            let process = match client_process(state) {
                Ok(process) => process,
                Err(e) => {
                    tracing::warn!(app = identity, error = %e, "force quit: the application has no process to signal");
                    continue;
                }
            };
            let pid = process.pid;
            if !signalable(pid, self.force_quits.own_pid) || shell_pids.contains(&pid) {
                tracing::warn!(
                    app = identity,
                    pid,
                    "force quit: refusing to signal this process"
                );
                continue;
            }
            found.push((client, process));
        }
        found
    }

    /// `force_quit_app`: end the application the person confirmed.
    pub fn force_quit_app(&mut self, identity: &str) {
        let Some(app) = self.wm.app_for_identity(identity) else {
            return;
        };
        let processes = self.app_processes(app, identity);
        if processes.is_empty() {
            return;
        }
        let hung = self.unresponsive_apps().contains(identity);
        if hung {
            tracing::info!(
                app = identity,
                "force quit: the application does not answer, killing it"
            );
            for (_, process) in &processes {
                (self.send_signal)(process, SIGKILL);
            }
            return;
        }
        tracing::info!(
            app = identity,
            "force quit: asking the application to close"
        );
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            entry.close();
        }
        for (_, process) in &processes {
            (self.send_signal)(process, SIGTERM);
        }
        let at = self.now() + KILL_GRACE;
        self.force_quits.promised.push(Promise { at, processes });
    }

    /// SIGKILL what is still connected when the grace of an application asked to quit has run out.
    pub fn tick_force_quit(&mut self, now: f64) {
        if self.force_quits.promised.is_empty() {
            return;
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.force_quits.promised)
            .into_iter()
            .partition(|p| p.at <= now);
        self.force_quits.promised = later;
        for promise in due {
            for (client, process) in &promise.processes {
                // a client that is gone has quit
                if client.get_credentials(&self.display_handle).is_ok() {
                    tracing::info!(
                        pid = process.pid,
                        "force quit: the process is still there, killing it"
                    );
                    (self.send_signal)(process, SIGKILL);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn init_the_server_and_pid_zero_are_never_signalled() {
        assert!(!signalable(0, 500));
        assert!(!signalable(1, 500));
        assert!(!signalable(500, 500));
        assert!(signalable(501, 500));
    }

    #[test]
    fn a_client_with_windows_of_two_applications_is_not_signalled() {
        assert!(serves_only(AppId(1), [AppId(1), AppId(1)]));
        assert!(!serves_only(AppId(1), [AppId(1), AppId(2)]));
    }

    #[test]
    fn a_signal_goes_to_the_process_held_and_never_to_a_reused_pid() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let held = ProcessHandle::open(child.id()).unwrap();
        held.send(SIGTERM).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(SIGTERM)
        );
        assert!(
            held.send(SIGKILL).is_err(),
            "the process has been reaped: the pidfd names nothing now, whoever takes the pid"
        );
    }

    fn state_with_peer(peer: Option<ProcessHandle>) -> ClientState {
        ClientState {
            peer,
            ..ClientState::default()
        }
    }

    #[test]
    fn the_process_the_kernel_named_at_connect_is_held_and_refused_once_it_exited() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let peer = ProcessHandle::open(child.id()).unwrap();
        let state = state_with_peer(Some(peer));
        assert_eq!(client_process(&state).unwrap().pid, child.id());
        let _ = child.kill();
        let _ = child.wait();
        let error = client_process(&state).unwrap_err();
        assert!(error.to_string().contains("exited"), "{error}");
    }

    #[test]
    fn a_client_the_kernel_named_no_process_for_is_never_signalled() {
        let error = client_process(&state_with_peer(None)).unwrap_err();
        assert!(error.to_string().contains("SO_PEERPIDFD"), "{error}");
    }
}
