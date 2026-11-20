use super::*;

impl Manager {
    pub(super) fn adopt_sessions(&mut self) -> Result<()> {
        for found in self.logind.seat_sessions()? {
            let role = match (found.class.as_str(), found.service.as_str()) {
                ("greeter", GREETER_SERVICE) => Role::Greeter,
                ("user", SESSION_SERVICE | AUTO_LOGIN_SERVICE) => Role::Session,
                _ => continue,
            };
            if found.state == "closing" {
                continue;
            }
            if role == Role::Greeter
                && (found.uid != self.greeter.uid || self.greeter_session.is_some())
            {
                eprintln!("cupertino-sessiond: ignoring the login screen session {} (not the greeter account's, or a second one)", found.id);
                continue;
            }
            self.adopt(role, found)?;
        }
        Ok(())
    }

    /// Adopt one session: watch its leader and, for a user, note it in the table and hold its terminal.
    pub(super) fn adopt(&mut self, role: Role, found: SeatSession) -> Result<()> {
        let leader = i32::try_from(found.leader)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| format!("session {} has no leader process", found.id))?;
        let watch = match pidfd_open(leader, PidfdFlags::empty()) {
            Ok(watch) => watch,
            Err(rustix::io::Errno::SRCH) => {
                // the leader is gone: the session is ending and nothing is left to supervise
                eprintln!(
                    "cupertino-sessiond: session {} has no leader any more; terminating it",
                    found.id
                );
                if let Err(e) = self.logind.terminate_session(&found.id) {
                    eprintln!("cupertino-sessiond: {e}");
                }
                return Ok(());
            }
            Err(e) => return Err(format!("pidfd_open for session {}: {e}", found.id).into()),
        };
        let key = self.next_worker;
        self.next_worker += 1;
        eprintln!(
            "cupertino-sessiond: adopted the {role:?} session {} (process {}, VT {})",
            found.id, found.leader, found.vt
        );
        match role {
            Role::Greeter => {
                self.greeter_session = Some(found.id.clone());
                self.greeter_adopted = true;
            }
            Role::Session => {
                let terminal = match vt::hold(found.vt) {
                    Ok(terminal) => Some(terminal),
                    Err(e) => {
                        eprintln!(
                            "cupertino-sessiond: holding {} for session {}: {e}",
                            vt::device(found.vt),
                            found.id
                        );
                        None
                    }
                };
                // an adopted session was running before this run: its agent announces itself again, so it counts as ready
                self.sessions.push(Session {
                    id: found.id.clone(),
                    uid: found.uid,
                    worker: None,
                    ready: true,
                    ready_deadline: None,
                    prepare_deadline: None,
                    clear_when_shown: false,
                    ending: None,
                    agent: None,
                    _terminal: terminal,
                });
            }
        }
        self.adopted.push(Adopted {
            key,
            id: found.id,
            leader: watch,
        });
        Ok(())
    }

    /// The leader of an adopted session exited: its worker closes the PAM session, and what the leader left in the session is
    /// terminated so that logind can remove it (as for a session this run started, see `worker_exited`).
    pub(super) fn adopted_exited(&mut self, key: u64) {
        let Some(index) = self.adopted.iter().position(|adopted| adopted.key == key) else {
            return;
        };
        let adopted = self.adopted.remove(index);
        eprintln!(
            "cupertino-sessiond: the leader of the adopted session {} exited",
            adopted.id
        );
        self.leader_exited(&adopted.id);
    }

    /// The leader of session `id` exited: logind ends what it left behind, and the login screen takes the console now
    /// when the session had it. logind removes the session only once its last process is gone, which an app that ignores
    /// the end can put off for the stop timeout, and the console must not wait on a session whose compositor has ended.
    pub(super) fn leader_exited(&mut self, id: &str) {
        if let Err(e) = self.logind.terminate_session(id) {
            eprintln!("cupertino-sessiond: {e}");
        }
        // a session that ended before it was shown is never brought to the console; the login screen stops waiting for it
        let unshown = self
            .sessions
            .iter_mut()
            .find(|session| session.id == id)
            .and_then(|session| session.prepare_deadline.take())
            .is_some();
        if unshown {
            eprintln!(
                "cupertino-sessiond: session {id} ended before it was shown; clearing the login screen"
            );
            session_start_failed("console", "the session ended before it was shown");
            self.show_login_screen();
            return;
        }
        match self.logind.active_session() {
            Ok(Some(foreground)) if foreground == id => {
                eprintln!("cupertino-sessiond: session {id} ended; returning to the login screen");
                self.return_to_login_screen();
            }
            Ok(_) => {}
            Err(e) => eprintln!("cupertino-sessiond: {e}"),
        }
    }

    /// Log the configured account in once per boot, without a password, as the PAM service `cupertino-autologin`. It runs when
    /// the login screen's session is up. The claim is taken first, so a crash of the manager or a restart cannot repeat it, and
    /// it is skipped when the account has a session already (a person got there first).
    pub(super) fn try_auto_login(&mut self) {
        let Some(login) = self.auto_login.take() else {
            return;
        };
        if let Err(reason) = self.auto_login_start(&login) {
            eprintln!("cupertino-sessiond: auto login: {reason}");
        }
    }

    pub(super) fn auto_login_start(&mut self, login: &str) -> std::result::Result<(), String> {
        let account = users::login_accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|account| account.login == login)
            .ok_or_else(|| format!("{login:?} is not an ordinary account with a login shell"))?;
        if !auto_login::claim(Path::new(auto_login::DONE_FLAG))
            .map_err(|e| format!("{}: {e}", auto_login::DONE_FLAG))?
        {
            return Err("already done since boot".to_owned());
        }
        if self
            .logind
            .user_sessions()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|session| session.uid == account.uid)
        {
            return Err(format!("{login:?} has a session already"));
        }
        let (vt, terminal) = vt::allocate(self.config.vt)
            .map_err(|e| format!("allocating a virtual terminal: {e}"))?
            .ok_or("no virtual terminal is free for a session")?;
        let begin = ToWorker::Begin {
            service: AUTO_LOGIN_SERVICE.to_owned(),
            login: account.login,
            tty: vt::device(vt),
            vt,
            class: SessionClass::User,
            mode: Mode::Open,
            command: self.config.session_command.clone(),
            environment: Vec::new(),
            path: self.session_path.clone(),
            screen_reader: false,
            failures: 0,
        };
        let worker = self
            .spawn_worker(Role::Session, &begin, Some(terminal))
            .map_err(|e| e.to_string())?;
        self.auto_login_worker = Some(worker);
        eprintln!("cupertino-sessiond: auto login for uid {}", account.uid);
        Ok(())
    }

    /// Start the login screen's compositor as a session of class `greeter` on the login terminal.
    pub(super) fn start_greeter(&mut self) -> Result<()> {
        let begin = ToWorker::Begin {
            service: GREETER_SERVICE.to_owned(),
            login: self.greeter.login.clone(),
            tty: vt::device(self.config.vt),
            vt: self.config.vt,
            class: SessionClass::Greeter,
            mode: Mode::Open,
            command: self.config.greeter_command.clone(),
            // the login screen's server reads its own settings file
            environment: vec![
                format!("{SOCKET_VARIABLE}={SOCKET_PATH}"),
                format!("{COMPOSITOR_CONF_ENV}={GREETER_CONF_PATH}"),
            ],
            path: self.session_path.clone(),
            screen_reader: screen_reader_at_login(Path::new(system_accessibility::MACHINE_DIR)),
            failures: 0,
        };
        self.spawn_worker(Role::Greeter, &begin, None).map(drop)
    }

    /// Tell the login screen's worker whether the login screen is on the console: its screen reader runs only then.
    pub(super) fn tell_greeter_shown(&mut self, shown: bool) {
        let Some(channel) = self
            .workers
            .iter_mut()
            .find(|worker| worker.role == Role::Greeter)
            .and_then(|worker| worker.channel.as_mut())
        else {
            return;
        };
        let sent = encode(&ToWorker::Shown { shown }, MAX_FRAME)
            .map_err(|e| e.to_string())
            .and_then(|frame| channel.send_frame(&frame).map_err(|e| e.to_string()));
        if let Err(e) = sent {
            eprintln!("cupertino-sessiond: cannot tell the login screen's worker whether it is shown: {e}");
        }
    }

    /// The seat's active session changed, by the manager's own switch or by any other (a key that changes the virtual
    /// terminal): the login screen's reader runs only while its session has the console.
    pub(super) fn active_session_changed(&mut self) {
        let active = self.logind.active_session();
        if let Err(e) = &active {
            eprintln!("cupertino-sessiond: {e}; the login screen counts as hidden");
        }
        let shown = greeter_shown(
            active.ok().flatten().as_deref(),
            self.greeter_session.as_deref(),
        );
        self.tell_greeter_shown(shown);
    }

    /// Start `cupertino-sessiond worker` with one end of a socketpair as its standard input and send it `begin`.
    pub(super) fn spawn_worker(
        &mut self,
        role: Role,
        begin: &ToWorker,
        terminal: Option<File>,
    ) -> Result<u64> {
        let (ours, theirs) = UnixStream::pair()?;
        let child = worker_command(|k| std::env::var_os(k))
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .spawn()
            .map_err(|e| format!("starting a worker: {e}"))?;
        let exited = pidfd_open(
            rustix::process::Pid::from_raw(child.id() as i32)
                .ok_or("a worker has no process id")?,
            PidfdFlags::empty(),
        )
        .map_err(|e| format!("pidfd_open: {e}"))?;
        let mut channel = Channel::new(ours, MAX_FRAME);
        channel.send_frame(&encode(begin, MAX_FRAME)?)?;
        let id = self.next_worker;
        self.next_worker += 1;
        eprintln!("cupertino-sessiond: worker {id} started for the {role:?}");
        self.workers.push(Worker {
            id,
            role,
            child,
            exited,
            channel: Some(channel),
            terminal,
        });
        Ok(id)
    }

    /// A worker wrote.
    pub(super) fn worker_message(&mut self, id: u64) {
        let Some(worker) = self.workers.iter_mut().find(|worker| worker.id == id) else {
            return;
        };
        let role = worker.role;
        let Some(channel) = &mut worker.channel else {
            return;
        };
        let message = match channel.read_message::<FromWorker>() {
            Ok(None) => return,
            Ok(Some(message)) => message,
            Err(e) => {
                // closed, or broken: either way nothing more comes, and the exit is handled through the process descriptor
                if !matches!(e, ChannelError::Closed) {
                    eprintln!("cupertino-sessiond: worker {id}: {e}");
                }
                worker.channel = None;
                return;
            }
        };
        let conversation = self.next_conversation;
        let current = self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.worker == Some(id));
        match message {
            FromWorker::Prompt { echo, text } if role == Role::Session && current => {
                if let Some(attempt) = &mut self.attempt {
                    attempt.prompt = Some(conversation);
                    self.next_conversation += 1;
                    attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
                }
                self.send(&Event::Prompt {
                    conversation,
                    echo,
                    text,
                });
            }
            // the login screen's own session asks nothing, and an attempt that was dropped has no one to ask
            FromWorker::Prompt { .. } => {
                self.send_to_worker(id, encode(&ToWorker::Cancel {}, MAX_FRAME))
            }
            FromWorker::Notice { error, text } if current => {
                self.send(&Event::Notice { error, text })
            }
            FromWorker::Notice { .. } => {}
            FromWorker::Decided { outcome } if current => self.decided(outcome),
            FromWorker::Decided { outcome } => {
                if self.auto_login_worker == Some(id) && outcome != Outcome::Accepted {
                    eprintln!("cupertino-sessiond: auto login was not accepted ({outcome:?})");
                }
            }
            FromWorker::Started {
                session_id,
                leader,
                uid,
            } => self.started(id, role, &session_id, leader, uid),
            FromWorker::Failed { reason } => {
                eprintln!("cupertino-sessiond: worker {id} failed: {reason}");
                if current {
                    self.attempt_failed();
                }
            }
        }
    }

    /// The worker decided the open attempt.
    pub(super) fn decided(&mut self, outcome: Outcome) {
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        attempt.decision = Some(outcome);
        if outcome != Outcome::Accepted {
            self.refusals = self.refusals.saturating_add(1);
            self.attempt = None;
            self.send(&Event::Result { outcome });
            return;
        }
        self.refusals = 0;
        if let Some(action) = attempt.power {
            // the administrator proved her password: the worker ends, and the action is the manager's to carry out
            self.attempt = None;
            self.send(&Event::Result { outcome });
            eprintln!("cupertino-sessiond: an administrator authorised {action:?}");
            if let Err(reason) = self.power(action, &Requester::Administrator) {
                eprintln!("cupertino-sessiond: power request {action:?} refused: {reason:?}");
                self.send(&Event::PowerRefused { reason });
                self.send(&Event::Reset {});
            }
            return;
        }
        // the worker opens the session next, or, for a user who has one, ends
        attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
        let (uid, existing) = (attempt.uid, attempt.existing.clone());
        self.send(&Event::Result { outcome });
        let Some(uid) = uid else {
            eprintln!("cupertino-sessiond: the accepted account has no password database entry");
            return;
        };
        match existing {
            Some(session) => {
                eprintln!("cupertino-sessiond: returning to session {session} of uid {uid}");
                self.send(&Event::SessionStarting { uid });
                self.attempt = None;
                self.switch_to_session(&session);
            }
            None => {
                eprintln!("cupertino-sessiond: session starting for uid {uid}");
                self.send(&Event::SessionStarting { uid });
            }
        }
    }

    /// Bring the existing session `id` to the foreground: its agent unlocks it without a second password (the attempt just
    /// proved it; unlock result 4 of the design), then the console switches and the login screen is
    /// shown again for the next time.
    pub(super) fn switch_to_session(&mut self, id: &str) {
        let agent = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .and_then(|session| session.agent.clone());
        match agent {
            Some(agent) => {
                if let Err(e) = service::unlock(&self._bus, &agent) {
                    eprintln!(
                        "cupertino-sessiond: telling the agent of session {id} to unlock: {e}"
                    );
                }
            }
            None => eprintln!("cupertino-sessiond: session {id} has no agent to unlock it"),
        }
        if let Err(e) = self.activate(id) {
            eprintln!("cupertino-sessiond: {e}");
        }
        self.show_login_screen();
    }

    /// Activate the logind session `id` and note the switch as under way.
    pub(super) fn activate(&mut self, id: &str) -> std::result::Result<(), logind::Error> {
        // the screen reader of the login screen stops before another session takes the console
        let greeter = self.greeter_session.as_deref() == Some(id);
        if !greeter {
            self.tell_greeter_shown(false);
        }
        self.logind.activate_session(id)?;
        if greeter {
            self.tell_greeter_shown(true);
        }
        let now = Instant::now();
        self.switch = Some(Switch {
            target: id.to_owned(),
            next_poll: now + SWITCH_POLL,
            give_up_at: now + SWITCH_GIVE_UP,
        });
        Ok(())
    }

    /// `SwitchToLoginWindow` of a session's agent (which locked its screen first): the login screen takes the console.
    pub(super) fn switch_to_login_window(
        &mut self,
        caller: &CallerSession,
    ) -> std::result::Result<(), service::SwitchRefusal> {
        if self.switch.is_some() {
            eprintln!("cupertino-sessiond: session {} asked for the login window while a switch is under way; ignored", caller.id);
            return Ok(());
        }
        let greeter = self
            .greeter_session
            .clone()
            .ok_or(service::SwitchRefusal::NoLoginScreen)?;
        eprintln!(
            "cupertino-sessiond: session {} switches to the login window",
            caller.id
        );
        self.activate(&greeter).map_err(|e| {
            eprintln!("cupertino-sessiond: {e}");
            service::SwitchRefusal::Failed
        })?;
        self.show_login_screen();
        Ok(())
    }

    /// A worker opened its session and runs its leader.
    pub(super) fn started(&mut self, id: u64, role: Role, session_id: &str, leader: u32, uid: u32) {
        eprintln!("cupertino-sessiond: worker {id}: the {role:?} session {session_id} runs as process {leader}");
        if let Some(worker) = self.workers.iter_mut().find(|worker| worker.id == id) {
            // a session's worker supervises it alone from here; the login screen's keeps its channel
            if role == Role::Session {
                worker.channel = None;
            }
        }
        if role == Role::Greeter {
            self.greeter_session = Some(session_id.to_owned());
            self.greeter_leader = cupertino_peer::ProcessHandle::open(leader).ok();
            self.try_auto_login();
            return;
        }
        let terminal = self
            .workers
            .iter_mut()
            .find(|worker| worker.id == id)
            .and_then(|worker| worker.terminal.take());
        self.sessions.push(Session {
            id: session_id.to_owned(),
            uid,
            worker: Some(id),
            ready: false,
            ready_deadline: None,
            prepare_deadline: None,
            clear_when_shown: false,
            ending: None,
            agent: None,
            _terminal: terminal,
        });
        if self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.worker == Some(id))
        {
            self.attempt = None;
        }
        // the login screen stays on the console, with its spinner, while the session's compositor gets ready in the
        // background, at most [`PREPARE_TIMEOUT`]
        match self.keep_in_background(session_id) {
            Ok(()) => {
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    session.prepare_deadline = Some(Instant::now() + PREPARE_TIMEOUT);
                }
            }
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("console", &e.to_string());
                self.send(&Event::Reset {});
            }
        }
    }

    /// A session stays on its terminal in the background while its compositor takes the display's devices, draws its first
    /// frame and says so (`SessionPrepared`): the console switches only then, from the login screen straight to the session's
    /// picture, never to a terminal that nothing draws on yet. If logind has activated the session by itself, the login
    /// screen's session takes the console back first.
    pub(super) fn keep_in_background(&mut self, session_id: &str) -> Result<()> {
        if self.logind.is_active(session_id)? {
            eprintln!("cupertino-sessiond: logind activated session {session_id} by itself; activating the login screen's session again");
            let greeter = self
                .greeter_session
                .as_deref()
                .ok_or("the login screen has no session to activate again")?;
            self.logind.activate_session(greeter)?;
        } else {
            eprintln!("cupertino-sessiond: session {session_id} starts in the background until its first frame is ready");
        }
        Ok(())
    }

    /// Bring a session that got ready in the background to the console. The login screen keeps its spinner until the session's
    /// agent says it is up, at most [`SESSION_READY_TIMEOUT`]; for an agent that said so already it is cleared once the switch
    /// is over.
    pub(super) fn bring_to_console(&mut self, session_id: &str) {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };
        session.prepare_deadline = None;
        let ready = session.ready;
        match self.activate(session_id) {
            Ok(()) => {
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    if ready {
                        session.clear_when_shown = true;
                    } else {
                        session.ready_deadline = Some(Instant::now() + SESSION_READY_TIMEOUT);
                    }
                }
            }
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("console", &e.to_string());
                self.send(&Event::Reset {});
            }
        }
    }

    /// `SessionPrepared` of a session's compositor: its first frame is ready, so the session is brought to the console. A
    /// session on the console already (the wait for it was over) is left as it is.
    pub(super) fn session_prepared(
        &mut self,
        caller: &CallerSession,
    ) -> std::result::Result<(), ReadyRefusal> {
        let id = caller.id.as_str();
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == id && session.uid == caller.uid)
            .ok_or(ReadyRefusal::UnknownSession)?;
        if session.prepare_deadline.is_none() {
            eprintln!("cupertino-sessiond: session {id} has its first frame ready; it is on the console already");
            return Ok(());
        }
        eprintln!(
            "cupertino-sessiond: session {id} has its first frame ready; bringing it to the console"
        );
        self.bring_to_console(id);
        Ok(())
    }

    /// `LogOut` of a session's agent, once its applications have quit: the login screen takes the console first, and the
    /// session ends [`LOG_OUT_GRACE`] after the switch is over, so the person goes from the session's picture straight to the
    /// login screen and never sees the session's compositor end. A session in the background, or one with no login screen to
    /// switch to, ends at once.
    pub(super) fn log_out(
        &mut self,
        caller: &CallerSession,
    ) -> std::result::Result<(), ReadyRefusal> {
        let id = caller.id.clone();
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.id == id && session.uid == caller.uid)
            .ok_or(ReadyRefusal::UnknownSession)?;
        if session.ending.is_some() {
            return Ok(());
        }
        let foreground = match self.logind.is_active(&id) {
            Ok(active) => active,
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                false
            }
        };
        let greeter = self
            .greeter_session
            .clone()
            .filter(|_| foreground && self.switch.is_none());
        if let Some(greeter) = greeter {
            eprintln!("cupertino-sessiond: session {id} logs out; the login screen takes the console first");
            match self.activate(&greeter) {
                Ok(()) => {
                    if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id)
                    {
                        session.ending = Some(Ending::AfterSwitch);
                    }
                    self.show_login_screen();
                    return Ok(());
                }
                Err(e) => eprintln!("cupertino-sessiond: {e}"),
            }
        }
        eprintln!("cupertino-sessiond: session {id} logs out");
        self.end_session(&id);
        Ok(())
    }

    /// Have logind end session `id`, which logged out.
    pub(super) fn end_session(&mut self, id: &str) {
        if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) {
            session.ending = Some(Ending::Asked);
        }
        if let Err(e) = self.logind.terminate_session(id) {
            eprintln!("cupertino-sessiond: {e}");
        }
    }

    /// The open attempt cannot go on (its worker failed or died): `Internal` before the decision, a `Reset` after it.
    pub(super) fn attempt_failed(&mut self) {
        let Some(attempt) = self.attempt.take() else {
            return;
        };
        match attempt.decision {
            None => self.send(&Event::Result {
                outcome: Outcome::Internal,
            }),
            Some(_) => self.send(&Event::Reset {}),
        }
    }

    /// A deadline passed.
    pub(super) fn timer(&mut self) -> Result<()> {
        let now = Instant::now();
        if self.denial_at.is_some_and(|at| at <= now) {
            self.denial_at = None;
            if self
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.worker.is_none())
            {
                self.attempt = None;
                self.refusals = self.refusals.saturating_add(1);
                self.send(&Event::Result {
                    outcome: Outcome::Denied,
                });
            }
        }
        if self
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.deadline)
            .is_some_and(|at| at <= now)
        {
            self.abort_attempt();
        }
        if self.greeter_restart_at.is_some_and(|at| at <= now) {
            self.greeter_restart_at = None;
            self.start_greeter()?;
        }
        if self
            .switch
            .as_ref()
            .is_some_and(|switch| switch.next_poll <= now)
        {
            self.follow_switch(now);
        }
        let mut gave_up = false;
        for session in self
            .sessions
            .iter_mut()
            .filter(|session| session.ready_deadline.is_some_and(|at| at <= now))
        {
            session.ready_deadline = None;
            gave_up = true;
            eprintln!("cupertino-sessiond: session {} sent no ready signal within {} s; showing the login screen again", session.id, SESSION_READY_TIMEOUT.as_secs());
        }
        if gave_up {
            self.show_login_screen();
        }
        let unprepared: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| session.prepare_deadline.is_some_and(|at| at <= now))
            .map(|session| session.id.clone())
            .collect();
        for id in unprepared {
            eprintln!("cupertino-sessiond: session {id} did not say within {} s that its first frame is ready; bringing it to the console all the same", PREPARE_TIMEOUT.as_secs());
            self.bring_to_console(&id);
        }
        let ended: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| matches!(session.ending, Some(Ending::At(at)) if at <= now))
            .map(|session| session.id.clone())
            .collect();
        for id in ended {
            eprintln!("cupertino-sessiond: the login screen has the console; ending session {id}, which logged out");
            self.end_session(&id);
        }
        if self.upgrade_check_at <= now {
            self.upgrade_check_at = now + upgrade::CHECK_INTERVAL;
            self.hand_over_if_due(now);
        }
        Ok(())
    }

    /// Whether a user session runs, or something that leads to one: an attempt, a session's worker, a switch of the console.
    pub(super) fn user_session_runs(&self) -> bool {
        !self.sessions.is_empty()
            || self.attempt.is_some()
            || self.switch.is_some()
            || self.auto_login_worker.is_some()
            || self
                .workers
                .iter()
                .any(|worker| worker.role == Role::Session)
    }

    /// Hand the console over to a new version of the manager when its file was replaced and no user session runs
    /// ([`upgrade::decide`]): stop the login screen's session and end, so that systemd starts the new version, which starts a
    /// new login screen.
    pub(super) fn hand_over_if_due(&mut self, now: Instant) {
        let user_session = self.user_session_runs();
        let Some(watch) = &mut self.upgrade else {
            return;
        };
        let (installed, settled) = watch.look(now);
        if upgrade::decide(installed, settled, user_session) != Decision::HandOver {
            return;
        }
        eprintln!(
            "cupertino-sessiond: {} was replaced by a new version and no user session runs; stopping the login screen and ending, so that the new version starts",
            watch.path().display()
        );
        self.screen = None;
        self.stop_greeter();
        self.handed_over = true;
    }

    /// End the login screen's session as its worker ends it (the worker passes `SIGTERM` to the compositor, waits for it and
    /// closes the PAM session), then have logind end what is left in it, so the next manager finds no login screen to adopt
    /// and starts its own on a terminal nothing else draws on. A login screen of an earlier run is stopped through its
    /// session's leader, which is its worker too.
    pub(super) fn stop_greeter(&mut self) {
        self.greeter_restart_at = None;
        let session = self.greeter_session.take();
        self.greeter_leader = None;
        if let Some(index) = self
            .workers
            .iter()
            .position(|worker| worker.role == Role::Greeter)
        {
            let mut worker = self.workers.remove(index);
            match upgrade::stop(&worker.exited, GREETER_STOP_TIMEOUT) {
                Ok(true) => {}
                Ok(false) => {
                    eprintln!(
                        "cupertino-sessiond: the login screen's worker did not end within {} s; killing it",
                        GREETER_STOP_TIMEOUT.as_secs()
                    );
                    if let Err(e) = worker.child.kill() {
                        eprintln!("cupertino-sessiond: stopping worker {}: {e}", worker.id);
                    }
                }
                Err(e) => eprintln!("cupertino-sessiond: stopping worker {}: {e}", worker.id),
            }
            if let Err(e) = worker.child.wait() {
                eprintln!("cupertino-sessiond: worker {}: {e}", worker.id);
            }
        } else if let Some(index) = self.adopted.iter().position(|adopted| {
            self.greeter_adopted && session.as_deref() == Some(adopted.id.as_str())
        }) {
            let adopted = self.adopted.remove(index);
            match upgrade::stop(&adopted.leader, GREETER_STOP_TIMEOUT) {
                Ok(true) => {}
                Ok(false) => eprintln!(
                    "cupertino-sessiond: the login screen's session {} did not end within {} s",
                    adopted.id,
                    GREETER_STOP_TIMEOUT.as_secs()
                ),
                Err(e) => eprintln!(
                    "cupertino-sessiond: stopping the login screen's session {}: {e}",
                    adopted.id
                ),
            }
        }
        self.greeter_adopted = false;
        if let Some(id) = session {
            if let Err(e) = self.logind.terminate_session(&id) {
                eprintln!("cupertino-sessiond: {e}");
            }
        }
    }

    /// Ask logind whether the session switched to is in the foreground; the switch is over when it is, or after
    /// [`SWITCH_GIVE_UP`].
    pub(super) fn follow_switch(&mut self, now: Instant) {
        let Some(switch) = &mut self.switch else {
            return;
        };
        match self.logind.is_active(&switch.target) {
            Ok(true) => {}
            Ok(false) if now >= switch.give_up_at => {
                eprintln!("cupertino-sessiond: session {} is not in the foreground {} s after the switch; the switch is over", switch.target, SWITCH_GIVE_UP.as_secs());
            }
            Ok(false) => {
                switch.next_poll = now + SWITCH_POLL;
                return;
            }
            Err(e) => {
                // the target is gone (its session ended) or logind failed: either way nothing is under way
                eprintln!("cupertino-sessiond: {e}");
            }
        }
        if let Some(switch) = self.switch.take() {
            self.switch_over(&switch.target, now);
        }
    }

    /// The switch of the console to `target` is over: a session that logs out ends [`LOG_OUT_GRACE`] from `now`, and the login
    /// screen is cleared once the session its agent announced before the switch is shown.
    pub(super) fn switch_over(&mut self, target: &str, now: Instant) {
        for session in &mut self.sessions {
            if session.ending == Some(Ending::AfterSwitch) {
                session.ending = Some(Ending::At(now + LOG_OUT_GRACE));
            }
        }
        let cleared = self
            .sessions
            .iter_mut()
            .find(|session| session.id == target && session.clear_when_shown)
            .map(|session| session.clear_when_shown = false)
            .is_some();
        if cleared {
            self.show_login_screen();
        }
    }

    /// A worker exited: reap it.
    pub(super) fn worker_exited(&mut self, id: u64) -> Result<()> {
        let Some(index) = self.workers.iter().position(|worker| worker.id == id) else {
            return Ok(());
        };
        let mut worker = self.workers.remove(index);
        let status: ExitStatus = worker.child.wait()?;
        eprintln!(
            "cupertino-sessiond: worker {id} ({:?}) exited: {status}",
            worker.role
        );
        if self.auto_login_worker == Some(id) {
            self.auto_login_worker = None;
        }
        match worker.role {
            Role::Greeter => self.greeter_ended(Some(status)),
            Role::Session => {
                if self
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.worker == Some(id))
                {
                    self.attempt_failed();
                }
                // the leader is gone and its PAM session closed (the design's final hooks end the session's processes the
                // same way)
                let Some(ended) = self
                    .sessions
                    .iter()
                    .find(|session| session.worker == Some(id))
                    .map(|session| session.id.clone())
                else {
                    return Ok(());
                };
                self.leader_exited(&ended);
                Ok(())
            }
        }
    }

    /// The login screen's session ended, with its worker's `status` (none for a login screen of an earlier run, whose end
    /// logind alone reports): start it again after a second, unless it keeps failing.
    pub(super) fn greeter_ended(&mut self, status: Option<ExitStatus>) -> Result<()> {
        self.greeter_session = None;
        self.greeter_leader = None;
        let now = Instant::now();
        let giving_up = greeter_gives_up(&mut self.greeter_failures, now);
        greeter_exited(status, self.greeter_failures.len(), giving_up);
        if giving_up {
            eprintln!("cupertino-sessiond: the login screen's session ended {GREETER_FAILURES} times within {} s; not starting it again", GREETER_FAILURE_WINDOW.as_secs());
            self.try_auto_login();
        } else {
            self.greeter_restart_at = Some(now + GREETER_RESTART_DELAY);
        }
        Ok(())
    }

    /// A thread that reads the system bus wrote.
    pub(super) fn incoming_ready(&mut self) -> Result<()> {
        let mut counter = [0u8; 8];
        // the counter only wakes `poll`; the messages are in the channel
        match rustix::io::read(&*self.wake, &mut counter) {
            Ok(_) | Err(rustix::io::Errno::AGAIN) => {}
            Err(e) => return Err(format!("reading the wake-up: {e}").into()),
        }
        while let Ok(item) = self.incoming.try_recv() {
            match item {
                Incoming::Logind(Signal::SessionRemoved(id)) => self.session_removed(&id)?,
                Incoming::Logind(Signal::ActiveSessionChanged) => self.active_session_changed(),
                Incoming::Logind(Signal::Ended(reason)) => {
                    return Err(format!("logind's signals ended ({reason})").into())
                }
                Incoming::Call(call) => self.call(call),
            }
        }
        Ok(())
    }

    /// A method of `org.cupertino.LoginManager1` that needs the manager's state; the caller is authorised already.
    pub(super) fn call(&mut self, call: Call) {
        match call {
            Call::Power {
                session,
                action,
                reply,
            } => {
                eprintln!(
                    "cupertino-sessiond: power request {action:?} from session {}",
                    session.id
                );
                let result = self.power(action, &Requester::Agent(&session));
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of RequestPower stopped waiting");
                }
            }
            Call::SwitchToLoginWindow { session, reply } => {
                let result = self.switch_to_login_window(&session);
                if reply.send(result).is_err() {
                    eprintln!(
                        "cupertino-sessiond: the caller of SwitchToLoginWindow stopped waiting"
                    );
                }
            }
            Call::Ready {
                session,
                agent,
                reply,
            } => {
                let result = self.session_ready(&session, agent);
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of SessionReady stopped waiting");
                }
            }
            Call::Prepared { session, reply } => {
                let result = self.session_prepared(&session);
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of SessionPrepared stopped waiting");
                }
            }
            Call::LogOut { session, reply } => {
                let result = self.log_out(&session);
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of LogOut stopped waiting");
                }
            }
        }
    }

    /// The agent of a session this manager started is up: it is registered, and the login screen, which kept its spinner
    /// while the session started, is shown again.
    pub(super) fn session_ready(
        &mut self,
        caller: &CallerSession,
        agent: String,
    ) -> std::result::Result<(), ReadyRefusal> {
        let id = caller.id.as_str();
        // the caller is the user the session was started for, as well as in the session
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.id == id && session.uid == caller.uid)
            .ok_or(ReadyRefusal::UnknownSession)?;
        session.agent = Some(agent);
        // an agent that restarts, or that finds the manager restarted, announces the session again
        if session.ready {
            eprintln!("cupertino-sessiond: the agent of session {id} announced itself again");
            return Ok(());
        }
        session.ready = true;
        session.ready_deadline = None;
        eprintln!("cupertino-sessiond: session {id} is ready");
        // a session still in the background has its shell up before the switch: the login screen, still shown, keeps its
        // spinner until the switch is over
        let shown = session.prepare_deadline.is_none()
            && self
                .switch
                .as_ref()
                .is_none_or(|switch| switch.target != id);
        if shown {
            self.show_login_screen();
        } else {
            session.clear_when_shown = true;
        }
        Ok(())
    }

    /// `Power` from the login screen: refused with `PowerRefused`, else carried out.
    pub(super) fn screen_power(&mut self, action: PowerAction) {
        eprintln!("cupertino-sessiond: power request {action:?} from the login screen");
        if let Err(reason) = self.power(action, &Requester::Screen) {
            self.send(&Event::PowerRefused { reason });
        }
    }

    /// Restart, shut down or sleep. A restart or shut down ends other users' sessions: the login screen may ask only when none
    /// is open, and an agent only when none is open that belongs to another user or when it belongs to an administrator (the
    /// reference asks for an administrator's authentication, the dialog is a later unit's). A block inhibitor refuses
    /// the call although the manager is root.
    pub(super) fn power(
        &mut self,
        action: PowerAction,
        requester: &Requester<'_>,
    ) -> std::result::Result<(), PowerRefusal> {
        if matches!(requester, Requester::Screen | Requester::Administrator)
            && !self.power_shown(action)
        {
            return Err(PowerRefusal::Disabled);
        }
        if action != PowerAction::Sleep && !self.may_end_other_sessions(requester)? {
            return Err(PowerRefusal::OtherUsersLoggedIn);
        }
        self.logind.power(action).map_err(|e| {
            if e.blocked_by_inhibitor() {
                return PowerRefusal::Inhibited;
            }
            eprintln!("cupertino-sessiond: {e}");
            power_failed(action, &e.to_string());
            PowerRefusal::Failed
        })
    }

    /// Whether the settings show the power menu item for `action`.
    pub(super) fn power_shown(&self, action: PowerAction) -> bool {
        let menu = roster::power_menu(&self.config);
        match action {
            PowerAction::Restart => menu.restart,
            PowerAction::ShutDown => menu.shut_down,
            PowerAction::Sleep => menu.sleep,
        }
    }

    /// Whether a user session is open: Restart and Shut Down then ask the login screen for an administrator
    /// (the screen's rule of [`Manager::may_end_other_sessions`]).
    pub(super) fn other_users_logged_in(&self) -> std::result::Result<bool, logind::Error> {
        Ok(!self.logind.user_sessions()?.is_empty())
    }

    /// Whether `requester` may end the sessions of users it is not.
    pub(super) fn may_end_other_sessions(
        &self,
        requester: &Requester<'_>,
    ) -> std::result::Result<bool, PowerRefusal> {
        let failed = |e: &dyn std::fmt::Display| {
            eprintln!("cupertino-sessiond: {e}");
            PowerRefusal::Failed
        };
        let uids: Vec<u32> = self
            .logind
            .user_sessions()
            .map_err(|e| failed(&e))?
            .iter()
            .map(|session| session.uid)
            .collect();
        let asker = match requester {
            Requester::Screen => Asker::Screen,
            Requester::Administrator => Asker::Administrator,
            Requester::Agent(caller) => Asker::Agent { uid: caller.uid },
        };
        may_end_other_sessions(asker, &uids, |uid| {
            let administrators = users::admin_logins(&self.admin_groups).map_err(|e| failed(&e))?;
            let account = users::by_uid(uid).map_err(|e| failed(&e))?;
            Ok(account.is_some_and(|account| administrators.contains(&account.login)))
        })
    }

    /// A session of logind is gone. When it is one this manager started, its terminal is free again. The console goes back to
    /// the login screen only when the seat is left with no session in the foreground, that is when it was the one in the
    /// foreground: a session that ends in the background (its user is on the login screen or in another session) changes
    /// nothing on screen, and the login screen only learns that the list changed.
    pub(super) fn session_removed(&mut self, id: &str) -> Result<()> {
        self.adopted.retain(|adopted| adopted.id != id);
        // the end of an adopted login screen is told by logind alone (a worker of this run reports the end of its own)
        if self.greeter_adopted && self.greeter_session.as_deref() == Some(id) {
            self.greeter_adopted = false;
            return self.greeter_ended(None);
        }
        let Some(index) = self.sessions.iter().position(|session| session.id == id) else {
            return Ok(());
        };
        // dropping the record releases the terminal
        let unshown = self.sessions.remove(index).prepare_deadline.is_some();
        if unshown {
            eprintln!(
                "cupertino-sessiond: session {id} ended before it was shown; clearing the login screen"
            );
            self.show_login_screen();
            self.hand_over_if_due(Instant::now());
            return Ok(());
        }
        let foreground = match self.logind.active_session() {
            Ok(foreground) => foreground,
            Err(e) => {
                // the console is better on the login screen than on a terminal nothing draws on
                eprintln!("cupertino-sessiond: {e}");
                None
            }
        };
        if foreground.is_some() {
            eprintln!("cupertino-sessiond: session {id} ended in the background");
            self.refresh_users();
        } else {
            eprintln!("cupertino-sessiond: session {id} ended; returning to the login screen");
            self.return_to_login_screen();
        }
        // the last session may have been what kept a new version waiting
        self.hand_over_if_due(Instant::now());
        Ok(())
    }

    /// Bring the login screen to the console, cleared: the session in the foreground ended.
    pub(super) fn return_to_login_screen(&mut self) {
        match self.greeter_session.clone() {
            Some(greeter) => {
                if let Err(e) = self.activate(&greeter) {
                    eprintln!("cupertino-sessiond: {e}");
                }
            }
            None => eprintln!("cupertino-sessiond: the login screen has no session to return to"),
        }
        self.show_login_screen();
    }

    /// Send the screen the list again: a session started or ended. What is typed on the screen stays.
    pub(super) fn refresh_users(&mut self) {
        if self.screen.is_none() {
            return;
        }
        match self
            .build_roster()
            .and_then(|roster| Ok((roster, self.other_users_logged_in()?)))
        {
            Ok((roster, will_prompt)) => self.send(&Event::Users {
                users: roster.users,
                will_prompt,
            }),
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                self.drop_screen();
            }
        }
    }

    /// Clear the login screen and refresh its list: a session started or ended. An attempt that is not accepted yet ends, since
    /// the screen starts again from the picker.
    pub(super) fn show_login_screen(&mut self) {
        if self.screen.is_none() {
            return;
        }
        self.cancel();
        self.send(&Event::Reset {});
        self.refresh_users();
    }
}
