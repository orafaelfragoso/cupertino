//! The session agent: the shell host's `agent` component (`cupertino-shell-agent`), which asks whether to log out,
//! restart or shut down, shows the power panel, times the power key and reads what the machine allows. The server
//! asks it over `org.cupertino.SessionAgent1` on the session bus, from the connection that owns
//! `org.cupertino.Session1` (the only one the component answers, D20), at the unique name the server accepted for the
//! component: a process that took a name first is never asked.
//!
//! | Method | When the server calls it |
//! |---|---|
//! | `RequestLogOut` | the log out shortcut or the menu bar's Log Out, when no session agent follows the session's `log_out_requested` |
//! | `RequestPowerDialog` | the power panel's shortcut |
//! | `RequestRestart`, `RequestShutDown` | the menu bar's Restart… and Shut Down… |
//! | `PowerKey(b pressed)` | the power key went down or up; the agent times the hold |
//! | `RefreshCapabilities` | the menu bar opened its system menu: what the machine allows is read again |
//!
//! The calls go out one after the other, so a key's press and release reach the agent in the order they happened. A
//! log out asked while the agent's connection is not accepted yet (it starts, or restarts) is dropped with a notice;
//! with no agent component at all (no host program, or `agent` turned off) the log out ends the server, as the
//! shortcut did before there was a panel.

use crate::state::Cupertino;

/// The component's name in the component list (`docs/architecture/shell-host.md`).
pub const COMPONENT: &str = "agent";
/// The object path and interface the component serves on its own connection.
pub const PATH: &str = "/org/cupertino/SessionAgent1";
pub const INTERFACE: &str = "org.cupertino.SessionAgent1";

/// What the server asks of the session agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRequest {
    LogOut,
    PowerDialog,
    Restart,
    ShutDown,
    PowerKey { pressed: bool },
    RefreshCapabilities,
}

impl AgentRequest {
    /// The method of `org.cupertino.SessionAgent1` that carries it.
    pub fn method(self) -> &'static str {
        match self {
            AgentRequest::LogOut => "RequestLogOut",
            AgentRequest::PowerDialog => "RequestPowerDialog",
            AgentRequest::Restart => "RequestRestart",
            AgentRequest::ShutDown => "RequestShutDown",
            AgentRequest::PowerKey { .. } => "PowerKey",
            AgentRequest::RefreshCapabilities => "RefreshCapabilities",
        }
    }
}

/// The queue of calls on the session agent, served in order by one task of the session connection's executor.
pub(crate) struct AgentCalls {
    /// The unique name of the session connection the task runs on.
    connection: Option<String>,
    queue: async_channel::Sender<(String, AgentRequest)>,
}

/// Why a log out request reached nobody and the session goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogOutDropped {
    /// The session runs an agent component, but its connection is not accepted yet (it is starting or restarting), or
    /// went away before it answered.
    AgentNotReady,
}

impl LogOutDropped {
    /// The summary and the body of the notice that tells the person.
    pub fn text(self) -> (&'static str, &'static str) {
        match self {
            LogOutDropped::AgentNotReady => (
                "Log out did not start",
                "The session agent is not ready yet. Try again in a moment.",
            ),
        }
    }
}

/// Tell the person a log out request was dropped: a banner from the notification service. Nothing else shows it, and a
/// shortcut that does nothing looks like a broken one. A session without a notification service only has the log.
pub async fn post_notice(conn: &zbus::Connection, dropped: LogOutDropped) {
    let (summary, body) = dropped.text();
    // urgency 1 is normal; the banner is not kept in the centre
    let hints = std::collections::HashMap::from([
        ("urgency", zbus::zvariant::Value::U8(1)),
        ("transient", zbus::zvariant::Value::Bool(true)),
    ]);
    let actions: Vec<&str> = Vec::new();
    // sent without waiting for a reply: whoever owns the name answers it, and a squatter that never does must not hold
    // anything up
    let sent = async {
        let message =
            zbus::message::Message::method_call("/org/freedesktop/Notifications", "Notify")?
                .destination("org.freedesktop.Notifications")?
                .interface("org.freedesktop.Notifications")?
                .with_flags(zbus::message::Flags::NoReplyExpected)?
                .build(&("Cupertino", 0u32, "", summary, body, actions, hints, -1i32))?;
        conn.send(&message).await
    }
    .await;
    if let Err(e) = sent {
        tracing::warn!(error = %e, "cannot show that the log out request was dropped");
    }
}

/// Whether a call failed because nobody owns the name it went to.
fn is_nobody_there(e: &zbus::Error) -> bool {
    matches!(
        e,
        zbus::Error::MethodError(name, ..)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
            )
    )
}

/// Make the call of `request` on the agent's connection `agent`, and log a failure.
async fn call(conn: &zbus::Connection, agent: &str, request: AgentRequest) {
    let sent = match request {
        AgentRequest::PowerKey { pressed } => {
            conn.call_method(
                Some(agent),
                PATH,
                Some(INTERFACE),
                request.method(),
                &pressed,
            )
            .await
        }
        _ => {
            conn.call_method(Some(agent), PATH, Some(INTERFACE), request.method(), &())
                .await
        }
    };
    match sent {
        Ok(_) => {}
        Err(e) if request == AgentRequest::LogOut && is_nobody_there(&e) => {
            tracing::warn!(
                agent,
                "the session agent's connection went away: the log out request is dropped"
            );
            post_notice(conn, LogOutDropped::AgentNotReady).await;
        }
        Err(e) => tracing::warn!(?request, agent, error = %e, "cannot reach the session agent"),
    }
}

impl Cupertino {
    /// Ask the session agent for `request`: at its accepted connection when it has one, else as the module says.
    pub fn ask_session_agent(&mut self, request: AgentRequest) {
        let accepted = self.hosts.component_bus_name(COMPONENT).map(str::to_owned);
        let conn = self.quit.session_connection();
        match (accepted, conn) {
            (Some(agent), Some(conn)) => {
                let queue = self.agent_queue(&conn);
                if queue.try_send((agent, request)).is_err() {
                    tracing::warn!(
                        ?request,
                        "the session agent's queue is gone: the request is dropped"
                    );
                }
            }
            (Some(_), None) => tracing::warn!(
                ?request,
                "the session interface is not served: the session agent cannot be asked"
            ),
            (None, conn) if self.hosts.planned_host(COMPONENT).is_some() => {
                tracing::warn!(
                    ?request,
                    "the session agent is not ready: the request is dropped"
                );
                if let (AgentRequest::LogOut, Some(conn)) = (request, conn) {
                    conn.executor()
                        .clone()
                        .spawn(
                            {
                                let conn = conn.clone();
                                async move { post_notice(&conn, LogOutDropped::AgentNotReady).await }
                            },
                            "cupertino-log-out-dropped",
                        )
                        .detach();
                }
            }
            (None, _) if request == AgentRequest::LogOut => {
                tracing::warn!("the session runs no session agent: the log out ends the server");
                self.quit_compositor();
            }
            (None, _) => {
                tracing::warn!(
                    ?request,
                    "the session runs no session agent: the request is dropped"
                )
            }
        }
    }

    /// The queue of calls on the agent served on `conn`; a new one when the session connection changed, and the task
    /// of the old one ends with its queue.
    fn agent_queue(
        &mut self,
        conn: &zbus::Connection,
    ) -> async_channel::Sender<(String, AgentRequest)> {
        let connection = conn.unique_name().map(ToString::to_string);
        if let Some(calls) = &self.quit.agent_calls {
            if calls.connection == connection && !calls.queue.is_closed() {
                return calls.queue.clone();
            }
        }
        let (queue, requests) = async_channel::unbounded::<(String, AgentRequest)>();
        let task_conn = conn.clone();
        conn.executor()
            .clone()
            .spawn(
                async move {
                    while let Ok((agent, request)) = requests.recv().await {
                        call(&task_conn, &agent, request).await;
                    }
                },
                "cupertino-session-agent",
            )
            .detach();
        self.quit.agent_calls = Some(AgentCalls {
            connection,
            queue: queue.clone(),
        });
        queue
    }

    /// Lock the session, then hand the console to the login screen (fast user switching): `cupertino-sessiond` takes
    /// the console once the lock screen covers the session, so what the next person sees of it is its lock screen.
    pub fn switch_to_login_window(&mut self) {
        if self.session_lock.covers() {
            self.quit.switch_pending = false;
            switch_to_login_window();
            return;
        }
        self.quit.switch_pending = true;
        self.lock_session();
    }

    /// The lock screen covers the session: a switch to the login window waiting for it goes on.
    pub(crate) fn session_locked_for_switch(&mut self) {
        if std::mem::take(&mut self.quit.switch_pending) {
            switch_to_login_window();
        }
    }
}

/// `LoginManager1.SwitchToLoginWindow` of `cupertino-sessiond` on the system bus, on a thread of its own, since the
/// call waits for the manager; a failure is only logged (no login screen runs, or the switch failed).
fn switch_to_login_window() {
    const MANAGER: &str = "org.cupertino.LoginManager1";
    const MANAGER_PATH: &str = "/org/cupertino/LoginManager1";
    let spawned = std::thread::Builder::new()
        .name("cupertino-switch-user".to_owned())
        .spawn(|| {
            let answer = zbus::blocking::Connection::system().and_then(|bus| {
                bus.call_method(
                    Some(MANAGER),
                    MANAGER_PATH,
                    Some(MANAGER),
                    "SwitchToLoginWindow",
                    &(),
                )
                .map(drop)
            });
            match answer {
                Ok(()) => tracing::info!("switching to the login window"),
                Err(e) => tracing::warn!(error = %e, "the login window did not take the console"),
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot ask for the login window");
    }
}
