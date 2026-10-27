//! Force Quit: `cupertino_dock_v1.force_quit_app`, sent by the dock's Force Quit dialog after the person confirmed. An
//! application whose client does not answer its pings is killed at once; any other is asked to close (`xdg_toplevel.close`
//! for each window) and sent SIGTERM, and SIGKILL follows [`KILL_GRACE`] seconds later for what is still connected then.

use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Resource;

use crate::state::Cupertino;

/// Seconds between SIGTERM and the SIGKILL for a process that is still there.
pub const KILL_GRACE: f64 = 3.0;

const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

/// `kill(2)`, logged when it fails.
pub fn kill_process(pid: u32, signal: i32) {
    // Safety: kill(2) with a pid read from the client's socket credentials.
    if unsafe { libc::kill(pid as libc::pid_t, signal) } != 0 {
        tracing::warn!(pid, signal, error = %std::io::Error::last_os_error(), "force quit: the signal was not delivered");
    }
}

struct Promise {
    at: f64,
    clients: Vec<Client>,
}

/// The SIGKILLs promised to applications that were asked to quit.
#[derive(Default)]
pub struct ForceQuits {
    promised: Vec<Promise>,
}

impl ForceQuits {
    pub fn pending(&self) -> usize {
        self.promised.len()
    }
}

impl Cupertino {
    /// The clients behind the toplevels of `identity` and their processes, each once.
    fn app_processes(&self, identity: &str) -> Vec<(Client, u32)> {
        let Some(app) = self.wm.app_for_identity(identity) else {
            return Vec::new();
        };
        let mut found: Vec<(Client, u32)> = Vec::new();
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
            match client.get_credentials(&self.display_handle) {
                Ok(credentials) => found.push((client, credentials.pid as u32)),
                Err(e) => {
                    tracing::warn!(app = identity, error = %e, "force quit: a window of the application has no known process");
                }
            }
        }
        found
    }

    /// `force_quit_app`: end the application the person confirmed.
    pub fn force_quit_app(&mut self, identity: &str) {
        let Some(app) = self.wm.app_for_identity(identity) else {
            return;
        };
        let processes = self.app_processes(identity);
        if processes.is_empty() {
            return;
        }
        let hung = self.unresponsive_apps().contains(identity);
        if hung {
            tracing::info!(
                app = identity,
                "force quit: the application does not answer, killing it"
            );
            for (_, pid) in &processes {
                (self.send_signal)(*pid, SIGKILL);
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
        for (_, pid) in &processes {
            (self.send_signal)(*pid, SIGTERM);
        }
        let at = self.now() + KILL_GRACE;
        self.force_quits.promised.push(Promise {
            at,
            clients: processes.into_iter().map(|(client, _)| client).collect(),
        });
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
            for client in promise.clients {
                // a client that is gone has gone: its process id may be someone else's by now
                if let Ok(credentials) = client.get_credentials(&self.display_handle) {
                    tracing::info!(
                        pid = credentials.pid,
                        "force quit: the process is still there, killing it"
                    );
                    (self.send_signal)(credentials.pid as u32, SIGKILL);
                }
            }
        }
    }
}
