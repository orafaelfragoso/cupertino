//! What the agent does with a confirmed action: asking `org.cupertino.Session1` to carry it out, saving the
//! applications open for the next login around it, sleeping through the session manager or logind when Session1 does
//! not take it, and the answers to the server's quit signals, the interrupt panel and the battery's level. Each bus call
//! is a task of the component's loop whose answer comes back as [`Done`]; each file is written on the I/O pool.

use cupertino_kit::core::{Command as Do, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit_shell::Keyboard;

use super::{
    interrupt_key, panel_key, Interrupted, Message, Model, PanelState, Quitting, FILE_DEADLINE,
};
use crate::interrupt::{Choice, Reason, Stage};
use crate::low_battery;
use crate::panel::{Action, Decision};
use crate::session::{self, Interrupt, PendingInterrupt};
use crate::startup;
use crate::system::{self, Battery, Power, PowerError};
use crate::view::{self, Input, InterruptView};

/// The action an interrupt this agent did not start is worded as (the interrupt panel's "The log out stopped...") when
/// `PendingInterrupt` does not say which it belongs to (the call failed, or the interrupt is gone already).
const FALLBACK_ACTION: Action = Action::LogOut;

/// What a call or a file task an action started came back with.
#[derive(Clone, Debug)]
pub enum Done {
    /// `OpenApplications` answered, before `action` with the choice `reopen`.
    OpenApplications {
        action: Action,
        reopen: bool,
        open: Result<Vec<String>, String>,
    },
    /// The applications open were saved for the next login: `action` may be asked for.
    Saved(Action),
    /// Session1 took `action`, or did not.
    Forwarded {
        action: Action,
        result: Result<(), String>,
    },
    /// Session1 did not take the sleep: why.
    SleepRefused(String),
    /// The session manager or logind answered a power request.
    Power {
        action: Power,
        result: Result<(), PowerError>,
    },
    /// `PendingInterrupt` answered about an interrupt this agent did not start.
    Pending {
        app: String,
        reason: String,
        pending: Result<Option<PendingInterrupt>, String>,
    },
    /// `RetrySessionAction` answered, for `then`.
    Retried {
        then: Action,
        result: Result<(), String>,
    },
    /// The notification service answered a low-battery post.
    NoticePosted(Result<u32, String>),
    /// Nothing to do with the answer: it was logged.
    Logged,
}

/// A task of the loop that logs a failure of `call` as `what` and ends in nothing.
fn logged<F>(what: &'static str, call: F) -> Do<Message>
where
    F: std::future::Future<Output = zbus::Result<()>> + Send + 'static,
{
    Do::Perform(Box::pin(async move {
        if let Err(e) = call.await {
            eprintln!("cupertino-shell-agent: {what}: {e}");
        }
        Message::Done(Done::Logged)
    }))
}

/// A file task on the I/O pool that ends in nothing.
fn file_task(work: impl FnOnce() + Send + 'static) -> Do<Message> {
    cupertino_kit::task::io(FILE_DEADLINE, work, |()| Message::Done(Done::Logged))
}

/// What a failed action is called in its notice.
fn did_not_start(action: &str) -> &'static str {
    match action {
        "LogOut" => "Log out did not start",
        "Restart" => "Restart did not start",
        "ShutDown" => "Shut down did not start",
        "Sleep" => "Sleep did not start",
        other => {
            eprintln!(
                "cupertino-shell-agent: SessionActionFailed names an unknown action: {other}"
            );
            "The action did not start"
        }
    }
}

/// The name Session1 gives `action`.
fn session_name(action: Action) -> &'static str {
    match action {
        Action::LogOut => "LogOut",
        Action::Restart => "Restart",
        Action::ShutDown => "ShutDown",
        Action::Sleep => "Sleep",
    }
}

/// The action `PendingInterrupt` names (an interrupted sequence is a `LogOut`, `Restart` or `ShutDown`).
fn action_of(name: &str) -> Option<Action> {
    match name {
        "LogOut" => Some(Action::LogOut),
        "Restart" => Some(Action::Restart),
        "ShutDown" => Some(Action::ShutDown),
        _ => None,
    }
}

impl Model {
    pub(super) fn interrupt_shown(&self) -> bool {
        self.quitting
            .as_ref()
            .is_some_and(|q| q.interrupt.is_some())
    }

    /// The interrupt panel on screen, with its height.
    pub(super) fn interrupt(&self) -> Option<(&Interrupted, f64)> {
        let quitting = self.quitting.as_ref()?;
        let shown = quitting.interrupt.as_ref()?;
        let frames = view::interrupt_frames(&InterruptView {
            app: &shown.app,
            reason: shown.reason,
            stage: shown.stage,
            action: quitting.then,
            text: &self.text,
        });
        Some((shown, frames.height))
    }

    fn files(&self) -> Option<std::path::PathBuf> {
        self.restore_path.clone()
    }

    fn discard_saved_applications(&self) -> Do<Message> {
        let path = self.files();
        file_task(move || startup::discard_saved_applications(path.as_deref()))
    }

    /// Tell the person that `summary`, for `body`; the session goes on.
    fn report_failure(&self, summary: &'static str, body: String) -> Vec<Do<Message>> {
        let Some(session) = self.session.clone() else {
            return Vec::new();
        };
        vec![logged(
            "cannot show that an action did not start",
            async move { session::report_failure(&session, summary, &body).await },
        )]
    }

    /// What the person did on the confirmation panel.
    pub(super) fn panel_input(&mut self, input: Input) -> Vec<Do<Message>> {
        let Some(panel) = &mut self.panel else {
            return Vec::new();
        };
        match input {
            Input::Decide(decision) => return self.decide(decision),
            // the panel's checkbox; the power-button panel has none and keeps the saved choice
            Input::Reopen(on) => {
                if panel.panel.reopen() != on {
                    panel.panel.toggle_reopen();
                }
            }
            Input::MovePanel(by) => panel.placement.moved((by.x, by.y)),
            Input::LetGo => panel.keyboard = Keyboard::OnDemand,
            Input::Interrupt(_) => {}
        }
        Vec::new()
    }

    /// What the person did on the interrupt panel.
    pub(super) fn interrupt_input(&mut self, input: Input) -> Vec<Do<Message>> {
        let Some(shown) = self.quitting.as_mut().and_then(|q| q.interrupt.as_mut()) else {
            return Vec::new();
        };
        match input {
            Input::Interrupt(choice) => return self.choose(choice),
            Input::MovePanel(by) => shown.placement.moved((by.x, by.y)),
            Input::LetGo => shown.keyboard = Keyboard::OnDemand,
            Input::Decide(_) | Input::Reopen(_) => {}
        }
        Vec::new()
    }

    /// The server closed a panel's window: as if its close were asked for.
    pub(super) fn closed(&mut self, key: &WindowKey) -> Vec<Do<Message>> {
        if self
            .panel
            .as_ref()
            .is_some_and(|p| &panel_key(p.number) == key)
        {
            return self.decide(Decision::Cancel);
        }
        match self.interrupt() {
            Some((shown, _)) if &interrupt_key(shown.number) == key => {
                let choice = match shown.stage {
                    Stage::Ask => Choice::Cancel,
                    Stage::ConfirmForceQuit => Choice::Back,
                };
                self.choose(choice)
            }
            _ => Vec::new(),
        }
    }

    /// Close the panel with `decision`; an action is carried out through `org.cupertino.Session1`. Confirming a log out,
    /// restart or shut down remembers the panel's "Open my windows again" choice; cancelling forgets a change to it.
    pub(super) fn decide(&mut self, decision: Decision) -> Vec<Do<Message>> {
        let Some(PanelState { panel, .. }) = self.panel.take() else {
            return Vec::new();
        };
        match decision {
            Decision::Cancel => {
                eprintln!("cupertino-shell-agent: panel cancelled");
                Vec::new()
            }
            Decision::Do(action) => {
                eprintln!(
                    "cupertino-shell-agent: panel confirmed: {}",
                    action.log_name()
                );
                let mut commands = Vec::new();
                if action != Action::Sleep {
                    self.reopen = panel.reopen();
                    let (path, reopen) = (self.files(), panel.reopen());
                    commands.push(file_task(move || {
                        startup::remember_choice(path.as_deref(), reopen)
                    }));
                }
                commands.extend(self.carry_out(action, panel.reopen()));
                commands
            }
        }
    }

    /// Sleep through `org.cupertino.Session1`, which quits no application. When it does not take the request (it is not
    /// on the bus, or it refuses while the session is locked), the session manager or logind is asked directly, as
    /// Session1 itself would: a sleep has nothing to quit first.
    pub(super) fn sleep(&mut self) -> Vec<Do<Message>> {
        let Some(session) = self.session.clone() else {
            return self.request_power(Power::Sleep);
        };
        vec![Do::Perform(Box::pin(async move {
            match session::sleep(&session).await {
                Ok(()) => Message::Done(Done::Logged),
                Err(e) => Message::Done(Done::SleepRefused(e.to_string())),
            }
        }))]
    }

    fn request_power(&mut self, action: Power) -> Vec<Do<Message>> {
        let Some(system) = self.system.clone() else {
            eprintln!("cupertino-shell-agent: {action:?} not asked for: no system bus connection");
            return Vec::new();
        };
        vec![Do::Perform(Box::pin(async move {
            Message::Done(Done::Power {
                action,
                result: system::request_power(&system, action).await,
            })
        }))]
    }

    /// Carry out a confirmed action through `org.cupertino.Session1` (D13), which quits the applications first (sleep quits
    /// none). Before a log out, restart or shut down the applications open now are saved for the next login, once, with the
    /// "Open my windows again" choice `reopen`.
    fn carry_out(&mut self, action: Action, reopen: bool) -> Vec<Do<Message>> {
        if action == Action::Sleep {
            return self.sleep();
        }
        let Some(session) = self.session.clone() else {
            return self.not_carried_out(action, "no session bus connection");
        };
        // under way from here: no power-button panel shows over it
        self.logging_out = true;
        vec![Do::Perform(Box::pin(async move {
            Message::Done(Done::OpenApplications {
                action,
                reopen,
                open: session::open_applications(&session)
                    .await
                    .map_err(|e| e.to_string()),
            })
        }))]
    }

    /// `org.cupertino.Session1` did not take `action`: log it and tell the person; the session goes on.
    fn not_carried_out(&mut self, action: Action, error: &str) -> Vec<Do<Message>> {
        eprintln!(
            "cupertino-shell-agent: the {} did not start: Session1 did not take it: {error}",
            action.log_name()
        );
        self.logging_out = false;
        self.quitting = None;
        self.report_failure(
            did_not_start(session_name(action)),
            "The window server did not take the request, so no application was quit. The session goes on.".into(),
        )
    }

    /// The answer of a call an action started.
    pub(super) fn done(&mut self, done: Done) -> Vec<Do<Message>> {
        match done {
            Done::OpenApplications {
                action,
                open: Err(e),
                ..
            } => self.not_carried_out(action, &e),
            Done::OpenApplications {
                action,
                reopen,
                open: Ok(open),
            } => {
                let (path, id) = (self.files(), self.session_id.clone());
                vec![cupertino_kit::task::io(
                    FILE_DEADLINE,
                    move || {
                        startup::save_open_applications(
                            path.as_deref(),
                            reopen,
                            &open,
                            id.as_deref(),
                        )
                    },
                    move |()| Message::Done(Done::Saved(action)),
                )]
            }
            Done::Saved(action) => {
                let Some(session) = self.session.clone() else {
                    return self.not_carried_out(action, "no session bus connection");
                };
                vec![Do::Perform(Box::pin(async move {
                    let result = match action {
                        Action::LogOut => session::log_out(&session).await,
                        Action::Restart => session::restart(&session).await,
                        Action::ShutDown | Action::Sleep => session::shut_down(&session).await,
                    };
                    Message::Done(Done::Forwarded {
                        action,
                        result: result.map_err(|e| e.to_string()),
                    })
                }))]
            }
            Done::Forwarded {
                action,
                result: Ok(()),
            } => {
                eprintln!(
                    "cupertino-shell-agent: Session1 is carrying out the {}",
                    action.log_name()
                );
                self.logging_out = true;
                self.quitting = Some(Quitting {
                    then: action,
                    interrupt: None,
                });
                Vec::new()
            }
            Done::Forwarded {
                action,
                result: Err(e),
            } => {
                let mut commands = vec![self.discard_saved_applications()];
                commands.extend(self.not_carried_out(action, &e));
                commands
            }
            Done::SleepRefused(e) => {
                eprintln!("cupertino-shell-agent: Session1 did not take the sleep: {e}; asking for it directly");
                self.request_power(Power::Sleep)
            }
            Done::Power { result: Ok(()), .. } => Vec::new(),
            Done::Power {
                action,
                result: Err(PowerError::Refused(e)),
            } => {
                eprintln!("cupertino-shell-agent: {action:?} refused: {e}");
                self.logging_out = false;
                vec![self.discard_saved_applications()]
            }
            Done::Power {
                action,
                result: Err(PowerError::Failed(e)),
            } => {
                eprintln!("cupertino-shell-agent: {action:?} failed: {e}");
                Vec::new()
            }
            Done::Pending {
                app,
                reason,
                pending,
            } => self.adopt(app, reason, pending),
            Done::Retried { result: Ok(()), .. } | Done::Logged => Vec::new(),
            Done::Retried {
                then,
                result: Err(e),
            } => {
                let mut commands = vec![self.discard_saved_applications()];
                commands.extend(self.not_carried_out(then, &e));
                commands
            }
            Done::NoticePosted(Ok(id)) => {
                self.low_battery.posted(id);
                eprintln!("cupertino-shell-agent: low battery: notification {id}");
                Vec::new()
            }
            Done::NoticePosted(Err(e)) => {
                eprintln!("cupertino-shell-agent: posting the low-battery notification: {e}");
                Vec::new()
            }
        }
    }

    /// The server's word on the applications it is quitting: one did not quit in time (the interrupt panel shows, with
    /// why), or it quit after all (the panel closes). On a locked session nobody can answer the panel: the action is
    /// cancelled.
    ///
    /// The server sends an interrupt once and waits for its answer, so one this agent did not start (it started again
    /// in the middle of a sequence, or another caller Session1 lets end the session asked for it) is adopted: the panel
    /// shows and is answered as for its own, worded as the action `PendingInterrupt` names ([`FALLBACK_ACTION`] when it
    /// does not answer); one `PendingInterrupt` says is no longer pending is ignored.
    pub(super) fn quit_signal(&mut self, interrupt: Interrupt) -> Vec<Do<Message>> {
        match interrupt {
            Interrupt::ActionFailed { action, reason } => self.action_failed(&action, &reason),
            Interrupt::Closed => {
                match &mut self.quitting {
                    Some(quitting) => {
                        eprintln!("cupertino-shell-agent: the application quit after all");
                        quitting.interrupt = None;
                    }
                    None => eprintln!(
                        "cupertino-shell-agent: quit signal ignored: no action is under way"
                    ),
                }
                Vec::new()
            }
            Interrupt::Interrupted { app, reason } if self.quitting.is_some() => {
                self.interrupted(app, &reason)
            }
            Interrupt::Interrupted { app, reason } => {
                let Some(session) = self.session.clone() else {
                    return self.adopt(app, reason, Err("no session bus connection".into()));
                };
                vec![Do::Perform(Box::pin(async move {
                    let pending = session::pending_interrupt(&session)
                        .await
                        .map_err(|e| e.to_string());
                    Message::Done(Done::Pending {
                        app,
                        reason,
                        pending,
                    })
                }))]
            }
        }
    }

    /// An interrupt this agent did not start, with what `PendingInterrupt` said of it.
    fn adopt(
        &mut self,
        app: String,
        reason: String,
        pending: Result<Option<PendingInterrupt>, String>,
    ) -> Vec<Do<Message>> {
        if self.quitting.is_some() {
            return self.interrupted(app, &reason);
        }
        let then = match pending {
            Ok(Some(pending)) => action_of(&pending.action).unwrap_or_else(|| {
                eprintln!(
                    "cupertino-shell-agent: PendingInterrupt names an unknown action: {}",
                    pending.action
                );
                FALLBACK_ACTION
            }),
            Ok(None) => {
                // the sequence ended (another caller cancelled it) after the signal was sent: nothing to answer
                eprintln!(
                    "cupertino-shell-agent: quit signal ignored: Session1 has no interrupt pending"
                );
                return Vec::new();
            }
            Err(e) => {
                eprintln!("cupertino-shell-agent: PendingInterrupt did not answer: {e}");
                FALLBACK_ACTION
            }
        };
        eprintln!(
            "cupertino-shell-agent: adopting an interrupted {} this agent did not start",
            then.log_name()
        );
        self.logging_out = true;
        self.quitting = Some(Quitting {
            then,
            interrupt: None,
        });
        self.interrupted(app, &reason)
    }

    /// `app` did not quit in time, for `reason`: the interrupt panel shows, centred, unless the session is locked.
    fn interrupted(&mut self, app: String, reason: &str) -> Vec<Do<Message>> {
        let reason = Reason::of_status(reason);
        eprintln!("cupertino-shell-agent: application did not quit: {app} ({reason:?})");
        let number = self.next_window();
        let Some(quitting) = &self.quitting else {
            return Vec::new();
        };
        let height = view::interrupt_frames(&InterruptView {
            app: &app,
            reason,
            stage: Stage::Ask,
            action: quitting.then,
            text: &self.text,
        })
        .height;
        let placement = self.centred(Size::new(crate::interrupt::WIDTH, height));
        if let Some(quitting) = &mut self.quitting {
            quitting.interrupt = Some(Interrupted {
                app,
                reason,
                stage: Stage::Ask,
                placement,
                keyboard: Keyboard::Exclusive,
                number,
            });
        }
        if self.locked {
            return self.choose(Choice::Cancel);
        }
        Vec::new()
    }

    /// The server's final step of an action was refused or failed (`SessionActionFailed`: another user is logged in, an
    /// inhibitor, polkit denied it): the session goes on, so the log-out state is left, the saved list is dropped and the
    /// person is told why.
    fn action_failed(&mut self, action: &str, reason: &str) -> Vec<Do<Message>> {
        eprintln!("cupertino-shell-agent: Session1 could not finish the {action}: {reason}");
        self.logging_out = false;
        self.quitting = None;
        let mut commands = vec![self.discard_saved_applications()];
        commands.extend(self.report_failure(
            did_not_start(action),
            format!("{reason}. The session goes on."),
        ));
        commands
    }

    /// The answer on the interrupt panel: Try Again asks the server to quit the applications again, Cancel ends the
    /// action and the session goes on, Force Quit asks whether to force the application, and the answer to that either
    /// goes back to the first question or kills the application, which the server then gives another full timeout.
    pub(super) fn choose(&mut self, choice: Choice) -> Vec<Do<Message>> {
        let Some(quitting) = &mut self.quitting else {
            return Vec::new();
        };
        let Some(shown) = &mut quitting.interrupt else {
            return Vec::new();
        };
        if let Choice::ForceQuit | Choice::Back = choice {
            shown.stage = if choice == Choice::ForceQuit && shown.reason.offers_force_quit() {
                Stage::ConfirmForceQuit
            } else {
                Stage::Ask
            };
            return Vec::new();
        }
        let Some(shown) = quitting.interrupt.take() else {
            return Vec::new();
        };
        let then = quitting.then;
        let Some(session) = self.session.clone() else {
            return Vec::new();
        };
        match choice {
            Choice::TryAgain => {
                eprintln!(
                    "cupertino-shell-agent: trying the {} again",
                    then.log_name()
                );
                vec![Do::Perform(Box::pin(async move {
                    let result = session::retry_session_action(&session).await;
                    Message::Done(Done::Retried {
                        then,
                        result: result.map_err(|e| e.to_string()),
                    })
                }))]
            }
            Choice::Cancel => {
                self.quitting = None;
                self.logging_out = false;
                eprintln!(
                    "cupertino-shell-agent: {} cancelled while quitting the applications",
                    then.log_name()
                );
                // the session goes on: the list saved for the next login is not started after some later end of it
                vec![
                    self.discard_saved_applications(),
                    logged("Session1 did not take the cancel", async move {
                        session::cancel_session_action(&session).await
                    }),
                ]
            }
            Choice::ConfirmForceQuit => {
                // a failure leaves the server waiting: the panel comes back when the timeout runs out again
                vec![Do::Perform(Box::pin(async move {
                    match session::force_quit_application(&session, &shown.app).await {
                        Ok(processes) => eprintln!(
                            "cupertino-shell-agent: force quit: {} ({processes} processes)",
                            shown.app
                        ),
                        Err(e) => eprintln!(
                            "cupertino-shell-agent: force quit of {} failed: {e}",
                            shown.app
                        ),
                    }
                    Message::Done(Done::Logged)
                }))]
            }
            Choice::ForceQuit | Choice::Back => Vec::new(),
        }
    }

    /// Post, replace or withdraw the low-battery notification. The notification service may be missing or fail: that is
    /// reported and the warning is tried again at the next change.
    pub(super) fn battery(&mut self, report: Battery) -> Vec<Do<Message>> {
        let (level, percentage) = match report {
            Battery::Reading { level, percentage } => (level, percentage),
            Battery::NoDaemon(reason) => {
                eprintln!("cupertino-shell-agent: no battery warnings: {reason}");
                return Vec::new();
            }
        };
        let Some(session) = self.session.clone() else {
            return Vec::new();
        };
        match self.low_battery.report(level, percentage) {
            Some(low_battery::Action::Post { replaces, notice }) => {
                eprintln!("cupertino-shell-agent: low battery: {level:?} at {percentage:.0} %");
                vec![Do::Perform(Box::pin(async move {
                    let posted = system::post_notice(&session, replaces, &notice).await;
                    Message::Done(Done::NoticePosted(posted.map_err(|e| e.to_string())))
                }))]
            }
            Some(low_battery::Action::Close(id)) => {
                eprintln!(
                    "cupertino-shell-agent: low battery cleared, notification {id} withdrawn"
                );
                vec![logged(
                    "withdrawing the low-battery notification",
                    async move { system::close_notice(&session, id).await },
                )]
            }
            None => Vec::new(),
        }
    }
}
