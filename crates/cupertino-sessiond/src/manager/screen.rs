use super::*;

impl Manager {
    /// A connection on the screen's socket: only the greeter account, and only one at a time.
    pub(super) fn accept(&mut self) -> Result<()> {
        let (stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            // the peer went away between `poll` and `accept`
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(e) => return Err(format!("accept: {e}").into()),
        };
        let peer = rustix::net::sockopt::socket_peercred(&stream)
            .map_err(|e| format!("SO_PEERCRED: {e}"))?;
        if peer.uid.as_raw() != self.greeter.uid {
            eprintln!(
                "cupertino-sessiond: refused a connection from uid {}",
                peer.uid.as_raw()
            );
            return Ok(());
        }
        if self.screen.is_some() {
            eprintln!("cupertino-sessiond: refused a second connection of the login screen");
            return Ok(());
        }
        if let Some(leader) = &self.greeter_leader {
            if !started_by(&stream, leader, Path::new("/proc")) {
                eprintln!("cupertino-sessiond: refused a connection of the greeter's user from outside the login screen");
                return Ok(());
            }
        }
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        self.screen = Some(Screen {
            channel: Channel::new(stream, MAX_REQUEST_FRAME),
            state: State::New,
        });
        eprintln!("cupertino-sessiond: the login screen connected");
        Ok(())
    }

    /// The screen sent bytes, or hung up.
    pub(super) fn screen_readable(&mut self) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        let request = match screen.channel.read_message::<Request>() {
            Ok(None) => return,
            Ok(Some(request)) => request,
            Err(ChannelError::Closed) => {
                eprintln!("cupertino-sessiond: the login screen disconnected");
                self.drop_screen();
                return;
            }
            Err(e) => {
                // `ChannelError` names the class of the problem, never the message
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
                return;
            }
        };
        // an answer is for one open prompt of the open attempt; any other is dropped and its secret zeroed with it
        if let Request::Answer { conversation, .. } = &request {
            if !self
                .attempt
                .as_mut()
                .is_some_and(|attempt| attempt.answered(*conversation))
            {
                eprintln!("cupertino-sessiond: dropped an answer to a prompt that is not open");
                return;
            }
        }
        match screen.state.next(&request) {
            Ok(state) => screen.state = state,
            Err(refusal) => {
                eprintln!("cupertino-sessiond: closed the login screen: refused ({refusal:?})");
                self.refuse(refusal);
                return;
            }
        }
        match request {
            Request::Hello { .. } => self.greet(),
            Request::Authenticate { account } => self.authenticate(account),
            Request::AuthenticatePower { name, action } => self.authenticate_power(&name, action),
            Request::Answer { secret, .. } => self.answer(secret),
            Request::Cancel {} => self.cancel(),
            Request::Power { action } => self.screen_power(action),
        }
    }

    /// Tell the screen the version and everything it draws.
    pub(super) fn greet(&mut self) {
        self.send(&Event::Hello { version: VERSION });
        match self.welcome() {
            Ok(event) => self.send(&event),
            Err(e) => {
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
            }
        }
    }

    /// The roster of the picker. Its accounts become the ones an attempt may name.
    pub(super) fn build_roster(&mut self) -> Result<roster::Roster> {
        let accounts = users::login_accounts()?;
        let admins = if self.config.hide_admin_users {
            users::admin_logins(&self.admin_groups)?
        } else {
            Vec::new()
        };
        let reported = &mut self.reported_pictures;
        let wallpapers = &mut self.wallpapers;
        let logged_in: Vec<u32> = self.sessions.iter().map(|session| session.uid).collect();
        let roster = roster::build(
            &self.config,
            accounts,
            &admins,
            &logged_in,
            |account| match picture::read(account.uid, Path::new(&account.home)) {
                Ok(bytes) => bytes,
                Err(problem) => {
                    if reported.insert(account.uid) {
                        eprintln!(
                            "cupertino-sessiond: no picture for uid {}: {problem}",
                            account.uid
                        );
                    }
                    None
                }
            },
            |account| wallpapers.wallpaper(account.uid, Path::new(&account.home)),
        );
        self.listed = roster
            .users
            .iter()
            .map(|row| (row.uid, row.login.clone()))
            .collect();
        Ok(roster)
    }

    /// The `Welcome` event: the roster, the power items and the hint setting.
    pub(super) fn welcome(&mut self) -> Result<Event> {
        let roster = self.build_roster()?;
        let will_prompt = self.other_users_logged_in()?;
        Ok(Event::Welcome {
            users: roster.users,
            show_other: roster.show_other,
            power: roster::power_menu(&self.config),
            will_prompt,
            retries_until_hint: self.config.retries_until_hint,
        })
    }

    /// Send `event` to the screen and follow its state; a screen that cannot take it is dropped.
    pub(super) fn send(&mut self, event: &Event) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        let sent = encode(event, MAX_EVENT_FRAME)
            .map_err(|e| e.to_string())
            .and_then(|frame| screen.channel.send_frame(&frame).map_err(|e| e.to_string()));
        match sent {
            Ok(()) => screen.state = screen.state.after(event),
            Err(e) => {
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
            }
        }
    }

    /// Tell the screen why it is refused, and close the connection.
    pub(super) fn refuse(&mut self, reason: ProtocolRefusal) {
        self.send(&Event::Refused { reason });
        self.drop_screen();
    }

    /// The screen is gone: its open attempt ends with it.
    pub(super) fn drop_screen(&mut self) {
        self.screen = None;
        self.cancel();
    }

    /// Whether an attempt may start now; when not, the screen has been answered `Busy`.
    pub(super) fn may_start_attempt(&mut self) -> bool {
        if self.switch.is_some() {
            eprintln!(
                "cupertino-sessiond: a switch of the console is under way; refusing this attempt"
            );
            self.send(&Event::Result {
                outcome: Outcome::Busy,
            });
            return false;
        }
        if !self.attempts.allow(Instant::now()) {
            eprintln!("cupertino-sessiond: more than {ATTEMPTS_PER_WINDOW} attempts within {} s; refusing this one", ATTEMPT_WINDOW.as_secs());
            self.send(&Event::Result {
                outcome: Outcome::Busy,
            });
            return false;
        }
        true
    }

    /// `Authenticate`: check the account, then run the attempt in a worker.
    pub(super) fn authenticate(&mut self, account: Account) {
        if !self.may_start_attempt() {
            return;
        }
        // a listed account is named by uid in the journal, a typed one not at all: people type passwords there
        let login = match account {
            Account::Listed { uid } => {
                eprintln!("cupertino-sessiond: attempt for uid {uid}");
                self.listed
                    .iter()
                    .find(|(listed, _)| *listed == uid)
                    .map(|(_, login)| login.clone())
            }
            Account::Typed { name } => {
                eprintln!("cupertino-sessiond: attempt for a typed name");
                is_login_name(&name).then_some(name)
            }
        };
        let Some(login) = login else {
            // not an account the screen could name: answered like a wrong password, after the same time
            self.deny_unknown_account();
            return;
        };
        if let Err(outcome) = self.start_attempt(login, None) {
            self.send(&Event::Result { outcome });
        }
    }

    /// `AuthenticatePower`: an administrator proves her password for a restart or shut down that other users' sessions would
    /// otherwise forbid. Only the password is checked; the action runs when the attempt is accepted. A name that is
    /// no administrator is answered like a wrong password after the same time, so the screen learns nothing about the accounts.
    pub(super) fn authenticate_power(&mut self, name: &str, action: PowerAction) {
        if !self.may_start_attempt() {
            return;
        }
        // a typed name is not logged: people type passwords there
        eprintln!("cupertino-sessiond: attempt to authorise {action:?}");
        if action == PowerAction::Sleep || !self.power_shown(action) {
            eprintln!(
                "cupertino-sessiond: {action:?} needs no authorisation or is disabled; refusing"
            );
            self.send(&Event::Result {
                outcome: Outcome::Refused,
            });
            return;
        }
        let administrator = match users::admin_logins(&self.admin_groups) {
            Ok(administrators) => {
                is_login_name(name) && administrators.iter().any(|login| login == name)
            }
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                self.send(&Event::Result {
                    outcome: Outcome::Internal,
                });
                return;
            }
        };
        if !administrator {
            self.deny_unknown_account();
            return;
        }
        if let Err(outcome) = self.start_attempt(name.to_owned(), Some(action)) {
            self.send(&Event::Result { outcome });
        }
    }

    /// Answer the open request like a wrong password, after the time one takes.
    pub(super) fn deny_unknown_account(&mut self) {
        self.attempt = Some(Attempt {
            worker: None,
            uid: None,
            prompt: None,
            deadline: None,
            decision: None,
            existing: None,
            power: None,
        });
        self.denial_at = Some(Instant::now() + INVALID_ACCOUNT_DELAY);
    }

    /// Start the worker; the outcome to report when that cannot be done. An account with a session already is only checked
    /// (the worker opens nothing and the attempt ends in a switch to that session); any other gets a terminal of its own, held
    /// until its session is gone.
    pub(super) fn start_attempt(
        &mut self,
        login: String,
        power: Option<PowerAction>,
    ) -> std::result::Result<(), Outcome> {
        let uid = users::by_name(&login)
            .map_err(|e| {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("account", &e.to_string());
                Outcome::Internal
            })?
            .map(|account| account.uid);
        let existing = uid
            .filter(|_| power.is_none())
            .and_then(|uid| {
                self.sessions
                    .iter()
                    .find(|session| session.uid == uid && session.ending.is_none())
            })
            .map(|session| session.id.clone());
        let (mode, vt, terminal) = if existing.is_some() || power.is_some() {
            (Mode::Verify, self.config.vt, None)
        } else {
            match vt::allocate(self.config.vt) {
                Ok(Some((vt, terminal))) => (Mode::Open, vt, Some(terminal)),
                Ok(None) => {
                    eprintln!("cupertino-sessiond: no virtual terminal is free for a session");
                    session_start_failed("terminal", "no virtual terminal is free");
                    return Err(Outcome::Busy);
                }
                Err(e) => {
                    eprintln!("cupertino-sessiond: allocating a virtual terminal: {e}");
                    session_start_failed("terminal", &e.to_string());
                    return Err(Outcome::Internal);
                }
            }
        };
        let begin = ToWorker::Begin {
            service: SESSION_SERVICE.to_owned(),
            login: login.clone(),
            tty: vt::device(vt),
            vt,
            class: SessionClass::User,
            mode,
            command: self.config.session_command.clone(),
            environment: Vec::new(),
            path: self.session_path.clone(),
            screen_reader: false,
            failures: self.refusals,
        };
        let worker = self
            .spawn_worker(Role::Session, &begin, terminal)
            .map_err(|e| {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("worker", &e.to_string());
                Outcome::Internal
            })?;
        self.attempt = Some(Attempt {
            worker: Some(worker),
            uid,
            prompt: None,
            deadline: Some(Instant::now() + ATTEMPT_TIMEOUT),
            decision: None,
            existing,
            power,
        });
        Ok(())
    }

    /// `Answer`: pass the screen's answer to the worker that asked.
    pub(super) fn answer(&mut self, secret: Secret) {
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
        let Some(id) = attempt.worker else { return };
        let sent = encode_secret(&ToWorker::Answer { secret }, MAX_FRAME);
        self.send_to_worker(id, sent);
    }

    /// `Cancel` (or the screen's end): stop an attempt that has not been accepted. Once the session starts it runs on.
    pub(super) fn cancel(&mut self) {
        if cancel_attempt(&mut self.attempt, &mut self.workers) {
            self.denial_at = None;
        }
    }

    /// The attempt waited too long for the screen or its worker: end the worker, whatever the attempt had reached.
    pub(super) fn abort_attempt(&mut self) {
        let Some(attempt) = &self.attempt else { return };
        let (waited_for, worker) = (
            if attempt.prompt.is_some() {
                "the screen's answer to a prompt"
            } else {
                "its worker"
            },
            attempt.worker,
        );
        eprintln!(
            "cupertino-sessiond: an attempt waited {} s for {waited_for}; cancelling it",
            ATTEMPT_TIMEOUT.as_secs()
        );
        if let Some(worker) =
            worker.and_then(|id| self.workers.iter_mut().find(|worker| worker.id == id))
        {
            if let Err(e) = worker.child.kill() {
                eprintln!("cupertino-sessiond: stopping worker {}: {e}", worker.id);
            }
        }
        self.attempt_failed();
    }

    /// Write a frame to worker `id`; a worker that cannot be written to is on its way out and the exit handles it.
    pub(super) fn send_to_worker(
        &mut self,
        id: u64,
        frame: std::result::Result<zeroize::Zeroizing<Vec<u8>>, cupertino_login_ipc::FrameError>,
    ) {
        let Some(channel) = self
            .workers
            .iter_mut()
            .find(|worker| worker.id == id)
            .and_then(|worker| worker.channel.as_mut())
        else {
            return;
        };
        if let Err(e) = frame
            .map_err(|e| e.to_string())
            .and_then(|frame| channel.send_frame(&frame).map_err(|e| e.to_string()))
        {
            eprintln!("cupertino-sessiond: worker {id}: {e}");
        }
    }
}
