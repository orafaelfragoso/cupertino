//! The app a run comes from ([`cupertino_control_model::identity`]): the process table is `/proc`, read a bounded line
//! per process, and the display clients are this server's connections, named by the process each was read to have
//! ([`crate::state::ClientState::process_facts`]). It runs once per run, when the controller names its peer, never while
//! input is routed. The walk through the process table runs on a thread the loop waits for until
//! [`crate::process::CHECK_WAIT`] has passed; one that does not answer by then names no app, and neither does a walk
//! that reaches a connection whose process was not read in time.

use super::Requester;
use crate::process::{Lookups, CHECK_WAIT, IDENTITY_WAIT};
use crate::state::{ClientState, Cupertino};
use cupertino_control_model::identity::{self, ClientKind, ProcessInfo, Responsible, Unidentified};
use cupertino_defaults::desktop_control::App;
use smithay::reexports::wayland_server::{Client, Resource};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

/// What a link to a deleted executable ends with; the path still names the program the process started as.
const DELETED: &str = " (deleted)";

/// The display clients as the walk sees them.
struct Clients {
    /// Each client whose process is named, with what it is.
    named: Vec<((u32, u64), ClientKind)>,
    /// The process ids of the clients whose process was not read in time: a walk that reaches one names nothing.
    unnamed: Vec<u32>,
}

/// What the walk found.
enum Found {
    Desktop(String),
    Executable(PathBuf),
}

impl Cupertino {
    /// The app of the requester `pid` that started at `start`.
    pub(super) fn control_identify(&self, pid: u32, start: u64) -> Result<Requester, Unidentified> {
        let clients = self.control_client_kinds();
        let walked = Lookups::shared().run(None, CHECK_WAIT, move || walk(pid, start, &clients));
        let Some(found) = walked else {
            tracing::warn!(
                pid,
                "desktop control: the process table was not read in time, the run names no app"
            );
            return Err(Unidentified::NoParent);
        };
        match found? {
            Found::Desktop(id) => {
                let name = self
                    .desktop
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .map_or_else(|| id.clone(), |e| e.name.clone());
                Ok(Requester {
                    app: App::Desktop(id),
                    name,
                })
            }
            Found::Executable(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                Ok(Requester {
                    app: App::Executable(path),
                    name,
                })
            }
        }
    }

    /// Every connected client with what it is: named by the process it was read to have, waiting for the readings under
    /// way until one deadline together, else by its process id alone.
    fn control_client_kinds(&self) -> Clients {
        let backend = self.display_handle.backend_handle();
        let mut ids = Vec::new();
        backend.with_all_clients(|id| ids.push(id));
        let satellite = self
            .xwayland
            .as_ref()
            .and_then(crate::xwayland::XWayland::pid);
        let deadline = Instant::now() + IDENTITY_WAIT;
        let mut clients = Clients {
            named: Vec::new(),
            unnamed: Vec::new(),
        };
        for id in ids {
            let mut object = None;
            let _ = backend.with_all_objects_for(id.clone(), |o| {
                object.get_or_insert(o);
            });
            let Some(client) = object.and_then(|o| self.display_handle.get_client(o).ok()) else {
                continue;
            };
            let Some(data) = client.get_data::<ClientState>() else {
                // not a connection of this server's making: its process is the socket's, never read
                if let Ok(credentials) = client.get_credentials(&self.display_handle) {
                    clients.unnamed.extend(u32::try_from(credentials.pid).ok());
                }
                continue;
            };
            if data.internal {
                // the server's own connection: the server is a shell process of the walk ([`walk`])
                continue;
            }
            let Some(facts) = data.process_facts_until(deadline) else {
                clients
                    .unnamed
                    .extend(data.process().map(|process| process.pid));
                continue;
            };
            let process = facts.process;
            let kind = if !data.capabilities.is_empty() {
                ClientKind::Shell
            } else if satellite == Some(process.0) {
                ClientKind::Xwayland
            } else {
                ClientKind::App(self.control_client_entries(&client))
            };
            clients.named.push((process, kind));
        }
        clients
    }

    /// The desktop entry ids the app ids of `client`'s windows name: the entry each names itself, hidden or not
    /// ([`crate::desktop::DesktopIndex::named`]), which is the key the person's decisions are stored under.
    fn control_client_entries(&self, client: &Client) -> Vec<String> {
        self.windows
            .iter()
            .filter(|e| e.surface().client().as_ref() == Some(client))
            .filter_map(|e| {
                let window = self.wm.window(e.id)?;
                let app = self.wm.app(window.app)?;
                Some(self.desktop.named(&app.identity)?.id.clone())
            })
            .collect()
    }
}

/// The app of the requester `pid` that started at `start` among `clients`, reading `/proc`: off the loop. The server's own
/// process is a shell process.
fn walk(pid: u32, start: u64, clients: &Clients) -> Result<Found, Unidentified> {
    let own = std::process::id();
    let own_start = crate::process::parent_and_start(own).map(|(_, start)| start);
    let unnamed = Cell::new(false);
    let client = |pid: u32, start: u64| {
        if clients.unnamed.contains(&pid) {
            unnamed.set(true);
            return Some(ClientKind::Shell);
        }
        if pid == own && own_start == Some(start) {
            return Some(ClientKind::Shell);
        }
        let mut found = clients
            .named
            .iter()
            .filter(|(p, _)| *p == (pid, start))
            .peekable();
        found.peek()?;
        let mut entries = Vec::new();
        for (_, kind) in found {
            match kind {
                ClientKind::App(e) => entries.extend(e.iter().cloned()),
                // one connection of the process that is not an app's makes the process none
                other => return Some(other.clone()),
            }
        }
        Some(ClientKind::App(entries))
    };
    let process = |pid: u32| {
        crate::process::parent_and_start(pid).map(|(parent, start)| ProcessInfo { parent, start })
    };
    let responsible = identity::responsible(pid, start, process, client)?;
    if unnamed.get() {
        tracing::warn!(
            pid,
            "desktop control: the run's ancestry reaches a client whose process was not read in time, it names no app"
        );
        return Err(Unidentified::NoParent);
    }
    match responsible {
        Responsible::Desktop(id) => Ok(Found::Desktop(id)),
        Responsible::Executable { pid, start } => executable(pid, start)
            .map(Found::Executable)
            .ok_or(Unidentified::NoParent),
    }
}

/// The executable process `pid` runs, while it is still the process that started at `start`.
fn executable(pid: u32, start: u64) -> Option<PathBuf> {
    let link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    // the link names the process's program only while the id is still that process's
    if crate::process::parent_and_start(pid)?.1 != start {
        return None;
    }
    let text = link.to_str()?;
    let path = PathBuf::from(text.strip_suffix(DELETED).unwrap_or(text));
    path.is_absolute().then_some(path)
}
