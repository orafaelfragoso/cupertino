//! Starting the shell's hosts (`docs/architecture/shell-host.md`, *Processes and connections* and *The component list*):
//! [`Cupertino::apply_shell_plan`] works out where every component runs ([`Config::shell_plan`]) and makes the
//! supervisor and the hosts' lists follow it; [`Cupertino::supervise_shell`] runs a pass of the supervisor with the host
//! connections this module makes. The host's connection is one end of a socket pair the server passes to the child as
//! `WAYLAND_SOCKET`, holding the `host` role alone and bound to the child's process once it runs.
//!
//! The plan changes when the configuration's `shell` section changes, when a host answers `unavailable`, when a
//! component is lost (it runs alone for the rest of the session), when the shared host crashed often enough to isolate
//! every component (D18), and when the host program is refused at a start: then no component runs until
//! `cupertino shell restart host` or the next change of the section.

use std::path::PathBuf;
use std::process::ExitStatus;

use super::*;
use crate::config::{dev_folder_program, BuiltIn, HostFacts, ShellProcess};
use crate::shell_connection::BoundConnection;
use crate::supervisor::{HostProgram, Launcher, RestartError, RestartReply};

/// The host program of this server: none in the login session; with `--dev-shell` `cupertino-shell` found on `PATH`,
/// unchecked; otherwise the installed one ([`BuiltIn::installed`]), checked before every start. A host program that is
/// not there is logged once: no component runs then.
pub fn find_host_program(dev_shell: bool, login_screen: bool) -> Option<HostProgram> {
    if login_screen {
        return None;
    }
    let found = if dev_shell {
        crate::state::which(BuiltIn::Host.program_name()).map(|path| HostProgram {
            path,
            args: Vec::new(),
            checked: false,
        })
    } else {
        BuiltIn::Host.installed().map(|path| HostProgram {
            path,
            args: Vec::new(),
            checked: true,
        })
    };
    if found.is_none() {
        tracing::info!(
            program = BuiltIn::Host.program_name(),
            "the shell host is not installed: no shell component runs"
        );
    }
    found
}

impl Cupertino {
    /// Work out where every component runs and make the supervisor follow: the hosts that should run are started at the
    /// next supervision pass, the others end. A running host whose list changed gets it
    /// in whole; a separate host that is no longer needed gets an empty list and exits by itself, the shared host is
    /// ended. The pass is scheduled at once.
    pub fn apply_shell_plan(&mut self) {
        self.hosts.plan_dirty = false;
        let now = Instant::now();
        let facts = self.host_facts();
        let plan = self.config.shell_plan(self.login_screen, &facts);
        self.hosts.planned_shared = plan.shared.clone().unwrap_or_default();
        self.hosts.planned_separate = plan.separate.iter().map(|(name, _)| *name).collect();
        let folder = self.config.dev_folder.as_ref().map(|f| f.path.clone());
        let mut targets: Vec<(HostKind, Option<PathBuf>)> = Vec::new();
        if plan.shared.is_some() {
            targets.push((HostKind::Shared, None));
        }
        for &(name, dev) in &plan.separate {
            targets.push((HostKind::Separate(name), folder.clone().filter(|_| dev)));
        }
        let ended = self.supervisor.reconcile(&targets, now);
        for (kind, host_id) in ended {
            let Some(id) = host_id else { continue };
            match kind {
                // it stops its component and exits successfully; it is killed after the grace
                HostKind::Separate(_) => self.set_host_components(id, Vec::new()),
                HostKind::Shared => self.expect_host_exit(id),
            }
        }
        let lists: Vec<(u64, Vec<&'static str>)> = self
            .hosts
            .hosts
            .values()
            .filter(|h| h.spawned)
            .filter_map(|h| {
                let list = match h.kind {
                    HostKind::Shared => self.hosts.planned_shared.clone(),
                    HostKind::Separate(name) if self.hosts.planned_separate.contains(&name) => {
                        vec![name]
                    }
                    // a separate host that ends keeps its empty list
                    HostKind::Separate(_) => return None,
                };
                (h.components != list && h.expected_exit_kill_at.is_none()).then_some((h.id, list))
            })
            .collect();
        for (id, list) in lists {
            self.set_host_components(id, list);
        }
        self.release_unhosted_holds();
        crate::cli::schedule_supervision(self);
    }

    /// Whether a host runs `component`: the plan puts it in one, or a host the supervisor did not start
    /// ([`Cupertino::attach_host_connection`]) and that is not ending has it in its list. A component no host runs does
    /// not run: nothing of it is held.
    pub(super) fn component_hosted(&self, component: &str) -> bool {
        self.hosts.planned_host(component).is_some()
            || self.hosts.hosts.values().any(|h| {
                !h.spawned && h.expected_exit_kill_at.is_none() && h.components.contains(&component)
            })
    }

    /// End what is held for the components no host runs any more (turned off, or with no host to run them):
    /// their held pictures, reserved space and holes, and the window manager's held placements on every output.
    fn release_unhosted_holds(&mut self) {
        let mut held: Vec<&'static str> = self.hosts.holds.iter().map(|h| h.component).collect();
        if !self.hosts.held_wm_outputs.is_empty() {
            held.push("windowmanager");
        }
        held.sort_unstable();
        held.dedup();
        for component in held {
            if self.component_hosted(component) {
                continue;
            }
            self.release_component_holds(component);
            if component == "windowmanager" {
                // no instance takes anything over, so placements awaiting a first commit end too
                let outputs: Vec<OutputId> = self.hosts.held_wm_outputs.keys().copied().collect();
                for out in outputs {
                    self.end_held_wm_placements(out);
                }
            }
        }
    }

    /// What the plan needs to know about the hosts.
    fn host_facts(&self) -> HostFacts {
        let mut not_from_dev: Vec<&'static str> =
            self.hosts.dev_given_back.iter().copied().collect();
        let program = self
            .supervisor
            .host_program()
            .filter(|_| self.supervisor.hosts_available());
        if let (Some(folder), Some(program)) = (&self.config.dev_folder, program) {
            // the folder's file is looked at now; a component whose file is absent or refused runs in the installed host
            let installed = program.path.display().to_string();
            let user = rustix::process::getuid().as_raw();
            let path = folder.path.clone();
            let usable = crate::process::Lookups::shared()
                .run(None, crate::process::CHECK_WAIT, move || {
                    dev_folder_program(&path, &installed, user)
                })
                .unwrap_or_else(|| Err("the development folder was not checked in time".into()));
            if let Err(reason) = &usable {
                tracing::warn!(%reason, "the development folder's host is refused: its components run in the installed host");
            }
            if !matches!(usable, Ok(Some(_))) {
                not_from_dev.extend(
                    folder
                        .components
                        .iter()
                        .filter_map(|name| component_spec(name).map(|s| s.name)),
                );
            }
        }
        HostFacts {
            available: self.supervisor.hosts_available(),
            isolated: COMPONENTS
                .iter()
                .map(|s| s.name)
                .filter(|name| self.hosts.component_separate_for_session(name))
                .collect(),
            not_from_dev,
        }
    }

    /// The server expects the host `host_id` to exit: its exit is not counted, and it is killed after the grace.
    pub fn expect_host_exit(&mut self, host_id: u64) {
        if let Some(host) = self.hosts.hosts.get_mut(&host_id) {
            host.expected_exit_kill_at
                .get_or_insert(Instant::now() + EXPECTED_EXIT_KILL_GRACE);
        }
    }

    /// One supervision pass at `now`: start due processes and hosts, reap exited ones ([`Supervisor::poll`]). A host
    /// that was refused, or what the hosts' exits changed, works the plan out again, and the pass runs once more for it.
    pub fn supervise_shell(&mut self, now: Instant) {
        for _ in 0..2 {
            let mut supervisor = std::mem::take(&mut self.supervisor);
            let xwayland = self.xwayland.take();
            let socket = self.socket_name.clone();
            let mut launcher = ServerLauncher {
                state: self,
                refused: None,
            };
            supervisor.poll(
                now,
                socket.as_deref(),
                xwayland.as_ref().map(|x| x.display()),
                &mut launcher,
            );
            let refused = launcher.refused.take();
            self.xwayland = xwayland;
            self.supervisor = supervisor;
            if let Some(reason) = refused {
                tracing::warn!(%reason, "the shell host cannot start: no shell component runs until `cupertino shell restart host`");
                self.supervisor.set_host_refused(Some(reason));
                self.hosts.plan_dirty = true;
            }
            if !self.hosts.plan_dirty {
                return;
            }
            self.apply_shell_plan();
        }
    }

    /// Restart the component `component` where it runs (`cupertino shell restart NAME`): a component of a host is
    /// stopped and started by its host, a component of the development folder or of `alone` by starting its separate
    /// host again from the file that is there then, and a lost one goes back to the shared host. What a host said
    /// against it (a folder build given back) is forgotten. `reply` gets the host's process under
    /// the component's name once the component says it is ready, or an error after [`HOLD_READY_TIMEOUT`]. Returns
    /// `reply` back when no host runs the component.
    pub fn restart_hosted_component(
        &mut self,
        component: &'static str,
        reply: RestartReply,
    ) -> Option<RestartReply> {
        let Some(was) = self.hosts.planned_host(component) else {
            return Some(reply);
        };
        self.hosts.dev_given_back.remove(component);
        let answer = std::sync::Arc::new(std::sync::Mutex::new(PendingAnswer {
            reply: Some(reply),
            info: self.supervisor.host_info(was),
        }));
        let on_ready = answer.clone();
        let waiter: RestartWaiter = Box::new(move |result| {
            let mut pending = on_ready.lock().unwrap_or_else(|e| e.into_inner());
            let info = pending.info.clone();
            if let Some(reply) = pending.reply.take() {
                reply(match (result, info) {
                    (Ok(()), Some(mut info)) => {
                        info.name = component.to_string();
                        Ok(info)
                    }
                    (Ok(()), None) => Err(RestartError::NotStarted(format!(
                        "{component}: its host is not running"
                    ))),
                    (Err(reason), _) => Err(RestartError::NotReady(reason)),
                });
            }
        });
        // a lost component's restart puts it back in the shared host, unless the configuration runs it alone
        if let Some(rec) = self.hosts.components.get_mut(component) {
            rec.separate_for_session = false;
        }
        self.apply_shell_plan();
        match (was, self.hosts.planned_host(component)) {
            (HostKind::Shared, Some(HostKind::Shared)) => {
                // its host stops it and asks for its connection again
                self.request_component_restart(component, Some(waiter));
            }
            (HostKind::Separate(_), Some(kind @ HostKind::Separate(_))) => {
                self.forgive_component(component, waiter);
                // ended and started again from the file that is there now
                let name = crate::supervisor::host_name(kind);
                if let Some(id) = self.supervisor.host_id_of(&name) {
                    self.expect_host_exit(id);
                }
                let on_start = answer;
                self.supervisor.request_restart(
                    &name,
                    Instant::now(),
                    Box::new(move |started| {
                        let mut pending = on_start.lock().unwrap_or_else(|e| e.into_inner());
                        match started {
                            Ok(info) => pending.info = Some(info),
                            Err(e) => {
                                if let Some(reply) = pending.reply.take() {
                                    reply(Err(e));
                                }
                            }
                        }
                    }),
                );
            }
            // it moves to another host, which starts it afresh
            _ => self.forgive_component(component, waiter),
        }
        None
    }

    /// Start counting `component`'s failures afresh, as for a restart the person asked for, and answer `waiter` when it
    /// is ready; the host it runs in starts it.
    fn forgive_component(&mut self, component: &'static str, waiter: RestartWaiter) {
        if let Some(rec) = self.hosts.components.get_mut(component) {
            rec.gave_up = false;
            rec.failures.clear();
            rec.consecutive_failures = 0;
            rec.backoff_until = None;
            rec.restart_waiters
                .push((Instant::now() + HOLD_READY_TIMEOUT, rec.instance, waiter));
        }
    }

    /// A host connection for a host the supervisor is about to start, with `components` as its list: the client end
    /// and the host's id. It holds the `host` role, and nothing until it is bound to the process ([`Launcher::host_started`]).
    fn attach_spawned_host(
        &mut self,
        kind: HostKind,
        components: Vec<&'static str>,
    ) -> std::io::Result<(UnixStream, u64)> {
        let (server_end, client_end) = UnixStream::pair()?;
        let state = ClientState {
            capabilities: ClientCapabilitySet::of([ShellRole::Host]),
            ..ClientState::default()
        };
        let _ = state.gone.set(self.gone_clients.clone());
        let client = crate::state::insert_client_handle(&self.display_handle, server_end, state)?;
        let client_id = client.id();
        let host_id = self.hosts.next_host_id;
        self.hosts.next_host_id += 1;
        self.hosts.host_by_client.insert(client_id.clone(), host_id);
        self.hosts.hosts.insert(
            host_id,
            HostEntry {
                id: host_id,
                kind,
                client: client_id,
                from_dev_folder: false,
                spawned: true,
                components,
                resource: None,
                component_objects: HashMap::new(),
                connect_pending: HashSet::new(),
                restart_requested_at: None,
                expected_exit_kill_at: None,
                kill_sent: false,
                process: None,
                start_ticks: None,
                last_bridge_at: None,
                bridge_retry_at: None,
            },
        );
        Ok((client_end, host_id))
    }

    /// Bind the connection of host `host_id` to its process `pid`, which was just started.
    fn bind_host_process(
        &mut self,
        host_id: u64,
        pid: u32,
        from_dev_folder: bool,
    ) -> std::io::Result<()> {
        let host = self
            .hosts
            .hosts
            .get_mut(&host_id)
            .ok_or_else(|| std::io::Error::other("the host connection is gone"))?;
        let data = self
            .display_handle
            .backend_handle()
            .get_client_data(host.client.clone())
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let state = (*data)
            .downcast_ref::<ClientState>()
            .ok_or_else(|| std::io::Error::other("the host connection has no state"))?;
        crate::shell_connection::bind_holder(state, pid)?;
        let process = ProcessHandle::open(pid)?;
        host.start_ticks = cupertino_peer::component::process_start_ticks(&process);
        host.process = Some(process);
        host.from_dev_folder = from_dev_folder;
        Ok(())
    }

    /// The process of host `host_id` exited: its connection ends now if it has not yet (a crash of the shared host is
    /// counted there), and whether the exit was expected comes back. A host of the development folder that exited with
    /// [`HOST_VERSION_EXIT_STATUS`] gives its component back to the installed host.
    fn host_process_ended(
        &mut self,
        host_id: u64,
        status: ExitStatus,
        from_dev_folder: bool,
    ) -> bool {
        use std::os::unix::process::ExitStatusExt;
        let entry = self
            .hosts
            .hosts
            .get(&host_id)
            .map(|h| (h.client.clone(), h.kind));
        let kind = entry.as_ref().map(|(_, kind)| *kind);
        if let Some((client, _)) = entry {
            self.display_handle.backend_handle().kill_client(
                client.clone(),
                smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
            );
            self.host_client_disconnected(&client);
        }
        let expected = self.hosts.ended.remove(&host_id).unwrap_or(false);
        if status.code() == Some(HOST_VERSION_EXIT_STATUS) && status.signal().is_none() {
            tracing::warn!(
                host_version = HOST_PROTOCOL_VERSION,
                "the shell host was built with another version of cupertino_host_v1"
            );
            if let (true, Some(HostKind::Separate(component))) = (from_dev_folder, kind) {
                self.hosts.dev_given_back.insert(component);
                self.hosts.plan_dirty = true;
            }
        }
        expected
    }
}

/// A restart's answer, given once: by the component's `ready` (or its timeout), or by a failed start of its host.
struct PendingAnswer {
    reply: Option<RestartReply>,
    /// The host's process, once it runs.
    info: Option<crate::supervisor::ProcessInfo>,
}

/// The server as the supervisor's [`Launcher`] during a pass, while the supervisor is out of the state.
struct ServerLauncher<'a> {
    state: &'a mut Cupertino,
    /// Why a host could not start, if one could not.
    refused: Option<String>,
}

impl Launcher for ServerLauncher<'_> {
    fn connect(&mut self, spec: &ShellProcess) -> std::io::Result<BoundConnection> {
        crate::shell_connection::connect_bound_client(
            &self.state.display_handle,
            crate::shell_connection::shell_process_state(&spec.roles),
        )
    }

    fn connect_host(&mut self, kind: HostKind) -> std::io::Result<(UnixStream, u64)> {
        let list = match kind {
            HostKind::Shared => self.state.hosts.planned_shared.clone(),
            HostKind::Separate(name) => vec![name],
        };
        self.state.attach_spawned_host(kind, list)
    }

    fn host_started(
        &mut self,
        host_id: u64,
        pid: u32,
        from_dev_folder: bool,
    ) -> std::io::Result<()> {
        self.state.bind_host_process(host_id, pid, from_dev_folder)
    }

    fn host_ended(&mut self, host_id: u64, status: ExitStatus, from_dev_folder: bool) -> bool {
        self.state
            .host_process_ended(host_id, status, from_dev_folder)
    }

    fn host_unbound(&mut self, host_id: u64, from_dev_folder: bool) {
        // the host never ran: its connection's end is no crash
        self.state.expect_host_exit(host_id);
        self.state
            .host_process_ended(host_id, ExitStatus::default(), from_dev_folder);
    }

    fn host_refused(&mut self, kind: HostKind, host_id: Option<u64>, reason: &str) {
        if let Some(id) = host_id {
            // the host never ran: its connection's end is no crash
            self.state.expect_host_exit(id);
            self.state
                .host_process_ended(id, ExitStatus::default(), false);
        }
        tracing::warn!(host = ?kind, %reason, "the shell host is refused");
        self.refused.get_or_insert_with(|| reason.to_string());
    }
}
