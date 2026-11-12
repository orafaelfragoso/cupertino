//! The app a run comes from ([`cupertino_control_model::identity`]): the process table is `/proc`, read a bounded line
//! per process, and the display clients are this server's connections, named by [`Cupertino::process_of`]. It runs once
//! per run, when the controller names its peer, never while input is routed.

use super::Requester;
use crate::state::{ClientState, Cupertino};
use cupertino_control_model::identity::{self, ClientKind, ProcessInfo, Responsible, Unidentified};
use cupertino_defaults::desktop_control::App;
use smithay::reexports::wayland_server::{Client, Resource};
use std::path::PathBuf;

/// What a link to a deleted executable ends with; the path still names the program the process started as.
const DELETED: &str = " (deleted)";

impl Cupertino {
    /// The app of the requester `pid` that started at `start`.
    pub(super) fn control_identify(&self, pid: u32, start: u64) -> Result<Requester, Unidentified> {
        let clients = self.control_client_kinds();
        let client = |pid: u32, start: u64| {
            let mut found = clients
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
            crate::process::parent_and_start(pid)
                .map(|(parent, start)| ProcessInfo { parent, start })
        };
        match identity::responsible(pid, start, process, client)? {
            Responsible::Desktop(id) => {
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
            Responsible::Executable { pid, start } => {
                let path = executable(pid, start).ok_or(Unidentified::NoParent)?;
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

    /// Every connected client this server can name a process for, with what it is.
    fn control_client_kinds(&self) -> Vec<((u32, u64), ClientKind)> {
        let backend = self.display_handle.backend_handle();
        let mut ids = Vec::new();
        backend.with_all_clients(|id| ids.push(id));
        let satellite = self
            .xwayland
            .as_ref()
            .and_then(crate::xwayland::XWayland::pid);
        let mut kinds = Vec::new();
        for id in ids {
            let mut object = None;
            let _ = backend.with_all_objects_for(id.clone(), |o| {
                object.get_or_insert(o);
            });
            let Some(client) = object.and_then(|o| self.display_handle.get_client(o).ok()) else {
                continue;
            };
            let Some(process) = (self.process_of)(&self.display_handle, &client) else {
                continue;
            };
            let kind = match client.get_data::<ClientState>() {
                Some(data) if data.internal || !data.capabilities.is_empty() => ClientKind::Shell,
                None => ClientKind::Shell,
                Some(_) if satellite == Some(process.0) => ClientKind::Xwayland,
                Some(_) => ClientKind::App(self.control_client_entries(&client)),
            };
            kinds.push((process, kind));
        }
        kinds
    }

    /// The desktop entry ids the app ids of `client`'s windows name.
    fn control_client_entries(&self, client: &Client) -> Vec<String> {
        self.windows
            .iter()
            .filter(|e| e.surface().client().as_ref() == Some(client))
            .filter_map(|e| {
                let window = self.wm.window(e.id)?;
                let app = self.wm.app(window.app)?;
                Some(self.desktop.find(&app.identity)?.id.clone())
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_executable_of_a_live_process_is_its_program() {
        use std::io::{Read, Write};
        use std::process::Stdio;
        let mut child = Guard(
            std::process::Command::new("cat")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        // `spawn` returns once the exec cannot fail any more, which can be before `/proc` names the new program; a
        // byte echoed back comes from the program itself, so it runs by then
        let mut echo = [0; 1];
        child.0.stdin.as_mut().unwrap().write_all(b"x").unwrap();
        child
            .0
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut echo)
            .unwrap();
        let pid = child.0.id();
        let (_, start) = crate::process::parent_and_start(pid).unwrap();
        let exe = executable(pid, start).expect("a running program");
        assert!(exe.is_absolute());
        assert_eq!(exe.file_name().unwrap(), "cat");
        assert_eq!(
            executable(pid, start + 1),
            None,
            "another start is another process"
        );
        let _ = child.0.kill();
        let _ = child.0.wait();
        assert_eq!(executable(pid, start), None, "a process that is gone");
    }

    /// A child that is ended when the test ends, also when it fails.
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
