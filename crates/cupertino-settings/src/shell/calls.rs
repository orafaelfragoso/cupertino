//! What the window asks of the services, off its thread: the calls behind the controls, made by tasks of the I/O
//! pool in the order they were asked ([`super::desk`]); a call that waits for a device, a pairing or sessiond runs as a
//! task of its own, so the readings go on meanwhile. Each result comes back to the window: a refusal shows under its
//! row (or on its device, or on its sheet), and the readings are read again, so a control shows the service's state,
//! not the one asked for.

use super::desk::{DEVICE_DEADLINE, PAIRING_DEADLINE, SERVICES_DEADLINE};
use super::state::{device_call, State};
use crate::live::{DeviceAction, LoginScreen, Reading};
use crate::panes::{self, Call, ControlId, Session};
use crate::services::Job;
use crate::settings::system_accessibility;
use crate::sheet::Kind;
use cupertino_defaults::accessibility::AccessibilitySettings;
use cupertino_defaults::accessibility::SYSTEM_ACCESSIBILITY;

/// How a call was asked for: by a control's row, or by a sheet that waits for its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asker {
    Row(Option<ControlId>),
    Sheet,
}

/// What the services did with a call: made it, or handed back the [`Job`] that waits for it.
enum Made {
    Done(Result<(), String>),
    Started(Result<Job, String>),
}

impl State {
    /// Make `call` on its service; its end shows on the row, the device or the sheet that asked ([`State::called`]).
    pub(crate) fn service_call(&mut self, asker: Asker, call: &Call) {
        // a new action on a device: the last one's error goes
        if let (Some(_), Reading::Ready(b)) = (device_call(call), &mut self.live.bluetooth) {
            b.clear_failures();
        }
        let asked = call.clone();
        if let Call::Pair(device) = call {
            let device = device.clone();
            self.desk.serve(
                move |services| services.pair(&device),
                move |state, job| state.started(asker, asked, job.map(Some), PAIRING_DEADLINE),
            );
            return;
        }
        self.desk.serve(
            move |services| match services.long_call(&asked) {
                Some(job) => (asked, Made::Started(job)),
                None => {
                    let result = services.call(&asked);
                    (asked, Made::Done(result))
                }
            },
            move |state, (call, made)| match made {
                Made::Started(job) => state.started(asker, call, job.map(Some), DEVICE_DEADLINE),
                Made::Done(result) => state.called(asker, &call, result),
            },
        );
    }

    /// A call that waits for its service has its [`Job`]: it runs as a task of its own, which should take `deadline`;
    /// a Join or a connection shows as asked meanwhile.
    fn started(
        &mut self,
        asker: Asker,
        call: Call,
        job: Result<Option<Job>, String>,
        deadline: std::time::Duration,
    ) {
        let job = match job {
            Ok(Some(job)) => job,
            Ok(None) => return self.called(asker, &call, Ok(())),
            Err(e) => return self.called(asker, &call, Err(e)),
        };
        match &call {
            Call::Join { ssid, .. } => {
                if let Reading::Ready(w) = &mut self.live.wifi {
                    w.joining = Some(ssid.clone());
                    w.failure = None;
                }
            }
            Call::Connect(device) => {
                if let Reading::Ready(b) = &mut self.live.bluetooth {
                    b.connect_asked(device);
                }
            }
            _ => {}
        }
        self.desk.off(deadline, job, move |state, result| {
            state.called(asker, &call, result)
        });
    }

    /// `call` ended with `result`: a refusal shows where it was asked (a Join on its sheet or its network, a device's
    /// action on the device, any other on its row), the passphrase sheet closes once its network joins, and the
    /// readings are read again.
    fn called(&mut self, asker: Asker, call: &Call, result: Result<(), String>) {
        if let (Call::Connect(device), Reading::Ready(b)) = (call, &mut self.live.bluetooth) {
            b.connect_returned(device);
        }
        // only the passphrase sheet waits for its call; the others close once they asked
        let sheet_open = asker == Asker::Sheet
            && matches!(&self.sheet, Some(sheet) if matches!(sheet.kind, Kind::Passphrase { .. }));
        match result {
            Ok(()) => {
                if let (Call::Join { ssid, .. }, Reading::Ready(w)) = (call, &mut self.live.wifi) {
                    if w.joining.is_none() {
                        w.joining = Some(ssid.clone());
                        w.failure = None;
                    }
                }
                if sheet_open {
                    self.sheet = None;
                }
            }
            Err(e) => {
                eprintln!("cupertino-settings: {e}");
                match (call, device_call(call), asker) {
                    // the passphrase sheet stays with the error (§7.2)
                    (Call::Join { .. }, _, Asker::Sheet) if sheet_open => {
                        if let Some(sheet) = &mut self.sheet {
                            sheet.failed(&e);
                        }
                    }
                    (Call::Join { .. }, _, _) => {
                        if let Reading::Ready(w) = &mut self.live.wifi {
                            w.joining = None;
                            w.failure = Some(e);
                        }
                    }
                    (_, Some((device, action)), _) => {
                        let device = device.to_string();
                        self.device_failed(&device, action, e);
                    }
                    (_, None, Asker::Sheet) => {
                        if let Reading::Ready(b) = &mut self.live.bluetooth {
                            b.failure = Some(e);
                        }
                    }
                    (_, None, Asker::Row(id)) => self.show_error(id, e),
                }
            }
        }
        let now = self.now();
        if let Err(e) = self.refresh(now) {
            eprintln!("cupertino-settings: {e}");
        }
    }

    /// Show `error` under the row of `id`, in place of the one it showed.
    pub(crate) fn show_error(&mut self, id: Option<ControlId>, error: String) {
        if let Some(id) = id {
            self.errors.retain(|(row, _)| *row != id);
            self.errors.push((id, error));
        }
    }

    /// Make a level slider's `call` (the brightness): a refusal shows under its row.
    pub(crate) fn level_call(&mut self, id: ControlId, call: Call) {
        self.desk.serve(
            move |services| services.call(&call),
            move |state, result| {
                if let Err(e) = result {
                    eprintln!("cupertino-settings: {e}");
                    state.show_error(Some(id), e);
                }
            },
        );
    }

    /// Ask the server for the appearance (`org.cupertino.Session1.SetAppearance`); the control shows it at once, a
    /// refusal shows under row `id` and the session's appearance is read again.
    pub(crate) fn set_appearance(&mut self, id: ControlId, dark: bool) {
        self.dark = dark;
        self.desk.serve(
            move |services| services.set_appearance(dark),
            move |state, result| {
                if let Err(e) = result {
                    eprintln!("cupertino-settings: {e}");
                    state.show_error(Some(id), e);
                    state.read_appearance();
                }
            },
        );
    }

    /// Ask the server to set one key of `accessibility.conf` (`org.cupertino.Session1.SetAccessibility`), then show
    /// what the server holds; a refusal shows under row `id`.
    pub(crate) fn set_accessibility(&mut self, id: ControlId, key: &str, value: &str) {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.desk.serve(
            move |services| services.set_accessibility(&key, &value),
            move |state, result| {
                if let Err(e) = result {
                    eprintln!("cupertino-settings: {e}");
                    state.show_error(Some(id), e);
                }
                state.sync_accessibility();
            },
        );
    }

    /// Ask sessiond whether the Login Screen section shows at this opening of the Accessibility pane: it stays hidden
    /// until the answer, and the refusal shown under it at the last opening goes.
    pub(crate) fn ask_login_screen(&mut self) {
        self.live.login_screen = None;
        self.errors.retain(|(id, _)| *id != ControlId::LoginScreen);
        self.login_opening += 1;
        let opening = self.login_opening;
        // a failure is logged and leaves the section hidden
        self.desk.serve(
            |services| services.can_set_system_accessibility(),
            move |state, ask| {
                let Ok(ask) = ask else {
                    return;
                };
                state.desk.off(
                    SERVICES_DEADLINE,
                    move || ask().then(login_screen_now),
                    move |state, current: Option<Option<AccessibilitySettings>>| {
                        if opening == state.login_opening {
                            state.live.login_screen = current.map(|current| LoginScreen {
                                current,
                                saving: state.login_saving,
                            });
                        }
                    },
                );
            },
        );
    }

    /// Use Mine: send the person's allow-listed options to sessiond; the row shows "Saving…" until the reply, and a
    /// refusal under it.
    pub(crate) fn save_login_screen(&mut self) {
        let session = Session {
            dark: self.dark,
            display: self.display,
        };
        let options = panes::login_screen_options(session, &self.settings);
        self.login_saving = true;
        if let Some(login) = &mut self.live.login_screen {
            login.saving = true;
        }
        self.desk.serve(
            move |services| services.set_system_accessibility(options),
            |state, job| match job {
                Ok(job) => state.desk.off(
                    SERVICES_DEADLINE,
                    move || (job(), login_screen_now()),
                    |state, (result, current)| state.login_screen_saved(result, Some(current)),
                ),
                Err(e) => state.login_screen_saved(Err(e), None),
            },
        );
    }

    /// sessiond answered Use Mine; `current` is what the login screen uses now, when it was read again.
    fn login_screen_saved(
        &mut self,
        result: Result<(), String>,
        current: Option<Option<AccessibilitySettings>>,
    ) {
        self.login_saving = false;
        if let Some(login) = &mut self.live.login_screen {
            login.saving = false;
            if let Some(current) = current {
                login.current = current;
            }
        }
        if let Err(e) = result {
            self.errors.push((ControlId::LoginScreen, e));
        }
    }

    /// The action on device `id` failed with `reason`: its row says so until the next action.
    pub(crate) fn device_failed(&mut self, id: &str, action: DeviceAction, reason: String) {
        if let Reading::Ready(b) = &mut self.live.bluetooth {
            b.device_failure = Some(crate::live::DeviceFailure {
                id: id.to_string(),
                action,
                reason,
            });
        }
    }
}

/// What the login screen uses now, as the readers of `accessibility.conf` read it; on the I/O pool.
fn login_screen_now() -> Option<AccessibilitySettings> {
    system_accessibility(std::path::Path::new(SYSTEM_ACCESSIBILITY))
}
