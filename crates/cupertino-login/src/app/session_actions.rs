//! What the session agent does with a confirmed action: asking `org.cupertino.Session1` to carry it out, saving the
//! applications open for the next login around it, sleeping, restarting and shutting down through the session manager
//! or logind, and the answers to the server's quit signals and the interrupt panel.

use super::{Interrupted, Quitting, SessionApp};
use crate::screen::Result;
use cupertino_login::interrupt::{Choice, Reason, Stage};
use cupertino_login::login_manager::Power;
use cupertino_login::panel::Action;
use cupertino_login::restore::{self, Applications, SessionMark};
use cupertino_login::session;

/// The action an interrupt this agent did not start is worded as (the interrupt panel's "The log out stopped...") when
/// `PendingInterrupt` does not say which it belongs to (the call failed, or the interrupt is gone already).
const FALLBACK_ACTION: Action = Action::LogOut;

/// The action `PendingInterrupt` names (an interrupted sequence is a `LogOut`, `Restart` or `ShutDown`).
fn action_of(name: &str) -> Option<Action> {
    match name {
        "LogOut" => Some(Action::LogOut),
        "Restart" => Some(Action::Restart),
        "ShutDown" => Some(Action::ShutDown),
        _ => None,
    }
}

impl SessionApp {
    /// Store the "Open my windows again" choice, with no list yet: the list follows just before the action is asked for.
    pub(super) fn remember_choice(&self, reopen: bool) {
        let Some(path) = &self.restore_path else {
            return;
        };
        match restore::save(path, reopen, Vec::new(), None) {
            Ok(()) => eprintln!("cupertino-login: reopen windows at the next login: {reopen}"),
            Err(e) => eprintln!("cupertino-login: the reopen choice is not saved: {e}"),
        }
    }

    /// Save the applications open now (`open`, in quit order), for the next login, just before the action that ends the
    /// session is asked for ([`restore`]).
    pub(super) fn save_open_applications(&self, reopen: bool, open: &[String]) {
        let Some(path) = &self.restore_path else {
            return eprintln!(
                "cupertino-login: applications not saved: neither XDG_STATE_HOME nor HOME is set"
            );
        };
        let (apps, skipped) = if reopen {
            restore::saved_list(open.iter().map(String::as_str), &Applications::from_env())
        } else {
            (Vec::new(), Vec::new())
        };
        for (identity, skip) in &skipped {
            eprintln!(
                "cupertino-login: application not saved for the next login: {identity}: {skip:?}"
            );
        }
        let mark = SessionMark::current(self.logind.session_id());
        if reopen && mark.is_none() {
            eprintln!(
                "cupertino-login: applications not saved: this boot and session cannot be named"
            );
        }
        match restore::save(path, reopen, apps.clone(), mark) {
            Ok(()) if reopen => eprintln!(
                "cupertino-login: saved for the next login: {}",
                if apps.is_empty() {
                    "no application".to_string()
                } else {
                    apps.join(" ")
                }
            ),
            Ok(()) => {}
            Err(e) => eprintln!("cupertino-login: applications not saved for the next login: {e}"),
        }
    }

    /// The session goes on after all (a refused restart or shut down): a list saved for the next login is dropped, so it
    /// is not started after some later end of the session.
    pub(super) fn discard_saved_applications(&self) {
        if let Some(path) = &self.restore_path {
            if let Err(e) = restore::discard(path) {
                eprintln!("cupertino-login: the saved applications are not dropped: {e}");
            }
        }
    }

    /// Ask the session manager to restart, shut down or sleep. A refusal (another user's session is open, an inhibitor) is
    /// reported and the session goes on; any other failure ends the process, which the server restarts.
    pub(super) fn request_power(&mut self, action: Power) -> Result<()> {
        match self.manager.request_power(action) {
            Ok(()) => {}
            Err(e) if e.refusal().is_some() => {
                eprintln!("cupertino-login: {action:?} refused: {e}");
                self.logging_out = false;
                self.discard_saved_applications();
            }
            // a session another display manager started: logind does it
            Err(e) if e.manager_absent() => self.power_through_logind(action)?,
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Restart, shut down or sleep through logind, for a session that has no session manager. A refusal (another user's
    /// session is open, a block inhibitor, the person may not) is reported and the session goes on.
    pub(super) fn power_through_logind(&mut self, action: Power) -> Result<()> {
        eprintln!("cupertino-login: no session manager: asking logind to {action:?}");
        match self.logind.power(action) {
            Ok(()) => {}
            Err(e) if e.is_refusal() => {
                eprintln!("cupertino-login: {action:?} refused: {e}");
                self.logging_out = false;
                self.discard_saved_applications();
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Sleep through `org.cupertino.Session1`, which quits no application. When it does not take the request (it is not on
    /// the bus, or it refuses while the session is locked), the session manager or logind is asked directly, as Session1
    /// itself would: a sleep has nothing to quit first.
    pub(super) fn sleep(&mut self) -> Result<()> {
        match session::sleep(&self.session_bus) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!(
                    "cupertino-login: Session1 did not take the sleep: {e}; asking for it directly"
                );
                self.request_power(Power::Sleep)
            }
        }
    }

    /// Carry out a confirmed action through `org.cupertino.Session1` (D13), which quits the applications first (sleep quits
    /// none). Before a log out, restart or shut down the applications open now are saved for the next login, once, with the
    /// "Open my windows again" choice `reopen`. Without Session1 nothing is quit and nothing happens: the person is told and
    /// the session goes on.
    pub(super) fn carry_out(&mut self, action: Action, reopen: bool) -> Result<()> {
        if action == Action::Sleep {
            return self.sleep();
        }
        let open = match session::open_applications(&self.session_bus) {
            Ok(open) => open,
            Err(e) => {
                self.not_carried_out(action, &e);
                return Ok(());
            }
        };
        self.save_open_applications(reopen, &open);
        let forwarded = match action {
            Action::LogOut => session::log_out(&self.session_bus),
            Action::Restart => session::restart(&self.session_bus),
            Action::ShutDown => session::shut_down(&self.session_bus),
            Action::Sleep => unreachable!("answered above"),
        };
        if let Err(e) = forwarded {
            self.discard_saved_applications();
            self.not_carried_out(action, &e);
            return Ok(());
        }
        eprintln!(
            "cupertino-login: Session1 is carrying out the {}",
            action.log_name()
        );
        self.logging_out = true;
        self.quitting = Some(Quitting {
            then: action,
            interrupt: None,
        });
        Ok(())
    }

    /// `org.cupertino.Session1` did not take `action`: log it and tell the person; the session goes on.
    pub(super) fn not_carried_out(&mut self, action: Action, error: &zbus::Error) {
        eprintln!(
            "cupertino-login: the {} did not start: Session1 did not take it: {error}",
            action.log_name()
        );
        self.logging_out = false;
        self.quitting = None;
        let summary = match action {
            Action::LogOut | Action::Sleep => "Log out did not start",
            Action::Restart => "Restart did not start",
            Action::ShutDown => "Shut down did not start",
        };
        let body = "The window server did not take the request, so no application was quit. The session goes on.";
        if let Err(e) = session::report_failure(&self.session_bus, summary, body) {
            eprintln!(
                "cupertino-login: cannot show that the {} did not start: {e}",
                action.log_name()
            );
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
    pub(super) fn quit_signal(&mut self, interrupt: session::Interrupt) -> Result<()> {
        if let session::Interrupt::ActionFailed { action, reason } = interrupt {
            self.action_failed(&action, &reason);
            return Ok(());
        }
        if self.quitting.is_none() {
            if interrupt == session::Interrupt::Closed {
                eprintln!("cupertino-login: quit signal ignored: no action is under way");
                return Ok(());
            }
            let then = match session::pending_interrupt(&self.session_bus) {
                Ok(Some(pending)) => action_of(&pending.action).unwrap_or_else(|| {
                    eprintln!(
                        "cupertino-login: PendingInterrupt names an unknown action: {}",
                        pending.action
                    );
                    FALLBACK_ACTION
                }),
                Ok(None) => {
                    // the sequence ended (another caller cancelled it) after the signal was sent: nothing to answer
                    eprintln!(
                        "cupertino-login: quit signal ignored: Session1 has no interrupt pending"
                    );
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("cupertino-login: PendingInterrupt did not answer: {e}");
                    FALLBACK_ACTION
                }
            };
            eprintln!(
                "cupertino-login: adopting an interrupted {} this agent did not start",
                then.log_name()
            );
            self.logging_out = true;
            self.quitting = Some(Quitting {
                then,
                interrupt: None,
            });
        }
        let quitting = self.quitting.get_or_insert(Quitting {
            then: FALLBACK_ACTION,
            interrupt: None,
        });
        match interrupt {
            session::Interrupt::Closed => {
                eprintln!("cupertino-login: the application quit after all");
                quitting.interrupt = None;
            }
            session::Interrupt::ActionFailed { .. } => unreachable!("answered above"),
            session::Interrupt::Interrupted { app, reason } => {
                let reason = Reason::of_status(&reason);
                eprintln!("cupertino-login: application did not quit: {app} ({reason:?})");
                quitting.interrupt = Some(Interrupted {
                    app,
                    reason,
                    stage: Stage::Ask,
                });
                if self.lock.is_some() {
                    self.choose(Choice::Cancel)?;
                }
            }
        }
        Ok(())
    }

    /// The server's final step of an action was refused or failed (`SessionActionFailed`: another user is logged in, an
    /// inhibitor, polkit denied it): the session goes on, so the log-out state is left, the saved list is dropped and the
    /// person is told why.
    pub(super) fn action_failed(&mut self, action: &str, reason: &str) {
        eprintln!("cupertino-login: Session1 could not finish the {action}: {reason}");
        self.logging_out = false;
        self.quitting = None;
        self.discard_saved_applications();
        let summary = match action {
            "LogOut" => "Log out did not start",
            "Restart" => "Restart did not start",
            "ShutDown" => "Shut down did not start",
            "Sleep" => "Sleep did not start",
            other => {
                eprintln!("cupertino-login: SessionActionFailed names an unknown action: {other}");
                "The action did not start"
            }
        };
        let body = format!("{reason}. The session goes on.");
        if let Err(e) = session::report_failure(&self.session_bus, summary, &body) {
            eprintln!("cupertino-login: cannot show that the {action} did not start: {e}");
        }
    }

    pub(super) fn interrupt_shown(&self) -> bool {
        self.quitting
            .as_ref()
            .is_some_and(|q| q.interrupt.is_some())
    }

    /// The answer on the interrupt panel: Try Again asks the server to quit the applications again, Cancel ends the
    /// action and the session goes on, Force Quit asks whether to force the application, and the answer to that either
    /// goes back to the first question or kills the application, which the server then gives another full timeout.
    pub(super) fn choose(&mut self, choice: Choice) -> Result<()> {
        let quitting = self
            .quitting
            .as_mut()
            .expect("a choice is only made while the applications are quit");
        let Some(shown) = &mut quitting.interrupt else {
            return Ok(());
        };
        match choice {
            Choice::ForceQuit | Choice::Back => {
                shown.stage = if choice == Choice::ForceQuit && shown.reason.offers_force_quit() {
                    Stage::ConfirmForceQuit
                } else {
                    Stage::Ask
                };
                return Ok(());
            }
            Choice::TryAgain | Choice::Cancel | Choice::ConfirmForceQuit => {}
        }
        let shown = quitting.interrupt.take().expect("checked above");
        let then = quitting.then;
        match choice {
            Choice::TryAgain => {
                eprintln!("cupertino-login: trying the {} again", then.log_name());
                if let Err(e) = session::retry_session_action(&self.session_bus) {
                    self.discard_saved_applications();
                    self.not_carried_out(then, &e);
                }
            }
            Choice::Cancel => {
                self.quitting = None;
                self.logging_out = false;
                // the session goes on: the list saved for the next login is not started after some later end of it
                self.discard_saved_applications();
                eprintln!(
                    "cupertino-login: {} cancelled while quitting the applications",
                    then.log_name()
                );
                if let Err(e) = session::cancel_session_action(&self.session_bus) {
                    eprintln!("cupertino-login: Session1 did not take the cancel: {e}");
                }
            }
            Choice::ConfirmForceQuit => {
                // a failure leaves the server waiting: the panel comes back when the timeout runs out again
                match session::force_quit_application(&self.session_bus, &shown.app) {
                    Ok(processes) => eprintln!(
                        "cupertino-login: force quit: {} ({processes} processes)",
                        shown.app
                    ),
                    Err(e) => eprintln!("cupertino-login: force quit of {} failed: {e}", shown.app),
                }
            }
            Choice::ForceQuit | Choice::Back => unreachable!("answered above"),
        }
        Ok(())
    }
}
