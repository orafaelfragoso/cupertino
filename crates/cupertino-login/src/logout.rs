//! The log out after it was confirmed: quit the applications one at a time, then end the session:
//! each application gets 45 s to quit before the interrupt panel shows ([`crate::interrupt`]); the next one is asked
//! 0.1 s after one quits. The server asks the application's windows to close and reports when they are gone
//! (`org.cupertino.Session1`); this is the timing around it. Pure: time is an absolute `f64` in seconds.

/// Time an application gets to quit before the interrupt panel (the 45.0 s termination timer).
pub const QUIT_TIMEOUT_S: f64 = 45.0;
/// Pause between one application quitting and asking the next.
pub const NEXT_APPLICATION_S: f64 = 0.1;

/// What the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    /// Call `QuitNextApplication` and pass its answer to [`LogOut::answered`].
    AskNext,
    /// No application is left: end the session.
    Terminate,
    /// The application did not quit in time: show the interrupt panel for it.
    Interrupt(String),
    /// The application quit while its interrupt panel was up: close the panel.
    CloseInterrupt,
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    /// `QuitNextApplication` is on its way.
    Asking,
    Waiting {
        app: String,
        since: f64,
    },
    Interrupted {
        app: String,
    },
    /// Ask the next application at this time.
    Next {
        at: f64,
    },
    Done,
}

/// The running log out.
#[derive(Clone, Debug, PartialEq)]
pub struct LogOut {
    step: Step,
}

impl LogOut {
    /// Start; the caller then asks for the first application.
    pub fn start() -> (LogOut, Do) {
        (LogOut { step: Step::Asking }, Do::AskNext)
    }

    /// `QuitNextApplication` answered `app` with `windows` windows asked to close; an empty `app` means none is left.
    pub fn answered(&mut self, now: f64, app: &str, windows: u32) -> Option<Do> {
        if self.step != Step::Asking {
            return None;
        }
        if app.is_empty() || windows == 0 {
            self.step = Step::Done;
            return Some(Do::Terminate);
        }
        self.step = Step::Waiting {
            app: app.to_string(),
            since: now,
        };
        None
    }

    /// `ApplicationQuit(app)`.
    pub fn quit(&mut self, now: f64, app: &str) -> Option<Do> {
        let close = match &self.step {
            Step::Waiting { app: waiting, .. } if waiting == app => None,
            Step::Interrupted { app: waiting } if waiting == app => Some(Do::CloseInterrupt),
            _ => return None,
        };
        self.step = Step::Next {
            at: now + NEXT_APPLICATION_S,
        };
        close
    }

    /// The user chose Force Quit on the interrupt panel: the caller kills the application, which gets another full
    /// timeout to go away. Returns the application to kill.
    pub fn force_quit(&mut self, now: f64) -> Option<String> {
        let Step::Interrupted { app } = &self.step else {
            return None;
        };
        let app = app.clone();
        self.step = Step::Waiting {
            app: app.clone(),
            since: now,
        };
        Some(app)
    }

    pub fn tick(&mut self, now: f64) -> Option<Do> {
        match &self.step {
            Step::Next { at } if now >= *at => {
                self.step = Step::Asking;
                Some(Do::AskNext)
            }
            Step::Waiting { app, since } if now - since >= QUIT_TIMEOUT_S => {
                let app = app.clone();
                self.step = Step::Interrupted { app: app.clone() };
                Some(Do::Interrupt(app))
            }
            _ => None,
        }
    }

    /// Seconds until [`LogOut::tick`] has something to do.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        match &self.step {
            Step::Next { at } => Some((at - now).max(0.0)),
            Step::Waiting { since, .. } => Some((since + QUIT_TIMEOUT_S - now).max(0.0)),
            Step::Asking | Step::Interrupted { .. } | Step::Done => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applications_quit_one_after_another_then_the_session_ends() {
        let (mut l, first) = LogOut::start();
        assert_eq!(first, Do::AskNext);
        assert_eq!(l.answered(0.0, "org.example.a", 2), None);
        assert_eq!(
            l.quit(3.0, "org.example.other"),
            None,
            "another application's report is not the one waited for"
        );
        assert_eq!(l.quit(3.0, "org.example.a"), None);
        assert_eq!(l.tick(3.05), None);
        assert!((l.next_wakeup(3.05).unwrap() - 0.05).abs() < 1e-9);
        assert_eq!(l.tick(3.1), Some(Do::AskNext), "the next one 0.1 s later");
        assert_eq!(l.answered(3.2, "", 0), Some(Do::Terminate));
        assert_eq!(l.tick(100.0), None);
    }

    #[test]
    fn an_application_that_does_not_quit_in_45_s_brings_the_interrupt_panel() {
        let (mut l, _) = LogOut::start();
        l.answered(10.0, "org.example.editor", 1);
        assert_eq!(l.tick(54.9), None);
        assert_eq!(
            l.tick(55.0),
            Some(Do::Interrupt("org.example.editor".into()))
        );
        assert_eq!(l.next_wakeup(56.0), None, "the panel waits for the user");
        assert_eq!(l.force_quit(60.0), Some("org.example.editor".into()));
        assert_eq!(l.force_quit(60.0), None, "only from the panel");
        assert_eq!(l.quit(60.5, "org.example.editor"), None);
        assert_eq!(l.tick(60.6), Some(Do::AskNext));
    }

    #[test]
    fn an_application_that_quits_while_its_panel_is_up_closes_the_panel() {
        let (mut l, _) = LogOut::start();
        l.answered(0.0, "org.example.slow", 1);
        l.tick(45.0);
        assert_eq!(l.quit(50.0, "org.example.slow"), Some(Do::CloseInterrupt));
        assert_eq!(l.tick(50.1), Some(Do::AskNext));
    }
}
