//! What the greeter says to the session manager and hears from it, and what the person does on its screens.

use super::{Message, Model, Stage};
use crate::model::{AuthOutcome, Phase};
use crate::picker::Picker;
use crate::power::{Action, Menu};
use crate::screen::Result;
use crate::user::User;
use crate::view::{self, AccessOption, Input};
use cupertino_kit::core::{Command, Key};
use cupertino_login_ipc::{
    Account, Event as ManagerEvent, Outcome, PowerAction, PowerMenu, PowerRefusal, Request, Secret,
    UserRow, VERSION,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// The users' wallpapers by user id in `rows`.
fn wallpapers(rows: &[UserRow]) -> HashMap<u32, PathBuf> {
    rows.iter()
        .filter_map(|row| Some((row.uid, PathBuf::from(row.wallpaper.as_ref()?))))
        .collect()
}

/// The journal line that says who the picker shows.
fn log_picker(users: &[UserRow], other: bool) {
    eprintln!(
        "cupertino-login: picker: {} users, other tile: {}, {} logged in",
        users.len(),
        if other { "yes" } else { "no" },
        users.iter().filter(|row| row.logged_in).count()
    );
}

/// The power item a special name in the name field stands for: `>restart`, `>sleep`, and `>power` or `>shutdown`
/// for shutting down.
fn special_name(name: &str) -> Option<Action> {
    match name {
        ">restart" => Some(Action::Restart),
        ">sleep" => Some(Action::Sleep),
        ">power" | ">shutdown" => Some(Action::ShutDown),
        _ => None,
    }
}

/// The user of a row. A picture that cannot be decoded is left out and logged: the screen shows initials.
fn user_of(row: &UserRow) -> User {
    User::from_row(row).unwrap_or_else(|e| {
        eprintln!("cupertino-login: {e}");
        User {
            login: row.login.clone(),
            display: row.display_name.clone(),
            picture: None,
            uid: Some(row.uid),
            logged_in: row.logged_in,
        }
    })
}

/// The power items the manager says to show, in menu order.
fn power_actions(menu: &PowerMenu) -> Vec<Action> {
    [
        (Action::Restart, menu.restart),
        (Action::ShutDown, menu.shut_down),
        (Action::Sleep, menu.sleep),
    ]
    .into_iter()
    .filter(|&(_, shown)| shown)
    .map(|(action, _)| action)
    .collect()
}

/// The journal line that says which titles the power menu shows (the ellipsis marks an item that asks for an administrator).
fn log_power_items(menu: Option<&Menu>) {
    let items = menu.map_or_else(String::new, |menu| {
        menu.items()
            .iter()
            .map(|action| action.title(menu.will_prompt()))
            .collect::<Vec<_>>()
            .join(", ")
    });
    eprintln!("cupertino-login: power items: {items}");
}

/// What the name and password screens say while an administrator is asked to authorise `action`. approximate, ours.
fn authorisation_notice(action: PowerAction) -> &'static str {
    match action {
        PowerAction::Restart => {
            "Other users are logged in. To restart, enter an administrator's name and password."
        }
        PowerAction::ShutDown => {
            "Other users are logged in. To shut down, enter an administrator's name and password."
        }
        PowerAction::Sleep => unreachable!("sleep asks for no administrator"),
    }
}

/// The notice for a power action the manager did not carry out. The words are approximate, ours.
fn power_refusal_text(reason: PowerRefusal) -> &'static str {
    match reason {
        PowerRefusal::OtherUsersLoggedIn => "Other users are logged in.",
        PowerRefusal::Inhibited => "A program is preventing this.",
        PowerRefusal::Disabled => "This is turned off in the login settings.",
        PowerRefusal::Failed => "This did not work. See the system journal.",
    }
}

/// The notice for an attempt that ended without a wrong password. A module's own text (`pam_faillock` says how long the
/// lock-out lasts) is better than ours when PAM sent one. The words are approximate, ours: the design's are not in the
/// strings read.
fn refusal_text(outcome: Outcome, pam_notice: Option<&str>) -> String {
    let ours = match outcome {
        Outcome::Locked => "This account is locked.",
        Outcome::Expired => "The password for this account has expired.",
        Outcome::Refused => "This account is not allowed to log in.",
        Outcome::Busy => "Too many attempts. Try again in a moment.",
        Outcome::Internal => "Signing in failed. See the system journal.",
        Outcome::Accepted | Outcome::Denied => unreachable!("the callers handle these"),
    };
    match outcome {
        Outcome::Locked | Outcome::Expired | Outcome::Refused => {
            pam_notice.unwrap_or(ours).to_owned()
        }
        _ => ours.to_owned(),
    }
}

impl Model {
    /// Something the session manager said: first its `Hello` and `Welcome`, the two messages that answer ours, then
    /// everything else.
    pub(super) fn manager_event(&mut self, now: f64, event: &ManagerEvent) -> Result<()> {
        if self.welcomed.is_none() {
            return self.handshake(now, event);
        }
        match event {
            ManagerEvent::Reset {} => {
                eprintln!("cupertino-login: reset");
                self.pending = None;
                self.authorising = None;
                let home = self.home(now);
                self.switch(now, home);
                return Ok(());
            }
            ManagerEvent::Users { users, will_prompt } => {
                // the list changed (a session started or ended): the tiles are built again; a screen being typed on stays
                self.picker = Picker::new(users.iter().map(user_of).collect(), self.show_other)?;
                self.wallpapers = wallpapers(users);
                // the wallpaper files are looked at again
                self.wanted = None;
                if let Some(menu) = &mut self.power {
                    menu.set_will_prompt(*will_prompt);
                }
                log_power_items(self.power.as_ref());
                log_picker(users, self.show_other);
                return Ok(());
            }
            ManagerEvent::Refused { reason } => {
                return Err(
                    format!("the session manager refused the connection: {reason:?}").into(),
                )
            }
            &ManagerEvent::PowerRefused { reason } => {
                let text = power_refusal_text(reason);
                eprintln!("cupertino-login: power action refused: {text}");
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.notify(text.to_owned());
                }
                return Ok(());
            }
            _ => {}
        }
        if !matches!(self.stage, Stage::Password { .. }) {
            return Err("the session manager answered while no password was asked".into());
        }
        self.attempt_event(now, event)
    }

    /// The manager's `Hello` (its version must be ours) and `Welcome` (the users, the power items and the hint's
    /// threshold): the screens may show from then on.
    fn handshake(&mut self, now: f64, event: &ManagerEvent) -> Result<()> {
        match event {
            &ManagerEvent::Hello { version } if version == VERSION && !self.greeted => {
                self.greeted = true;
            }
            &ManagerEvent::Hello { version } => {
                return Err(format!(
                    "the session manager speaks version {version}, this screen {VERSION}"
                )
                .into())
            }
            ManagerEvent::Welcome {
                users,
                show_other,
                power,
                will_prompt,
                retries_until_hint,
            } if self.greeted => {
                self.picker = Picker::new(users.iter().map(user_of).collect(), *show_other)?;
                log_picker(users, *show_other);
                self.show_other = *show_other;
                self.power = Menu::new(power_actions(power));
                if let Some(menu) = &mut self.power {
                    menu.set_will_prompt(*will_prompt);
                }
                log_power_items(self.power.as_ref());
                self.retries_until_hint = *retries_until_hint;
                self.wallpapers = wallpapers(users);
                self.welcomed = Some(now);
            }
            ManagerEvent::Refused { reason } => {
                return Err(
                    format!("the session manager refused the connection: {reason:?}").into(),
                )
            }
            _ => return Err("the session manager sent a message out of order".into()),
        }
        Ok(())
    }

    /// What the manager said of the attempt on the password screen.
    fn attempt_event(&mut self, now: f64, event: &ManagerEvent) -> Result<()> {
        match event {
            ManagerEvent::Prompt {
                conversation,
                echo,
                text,
            } => {
                if *echo {
                    if let Stage::Password { screen, .. } = &mut self.stage {
                        screen.notify(text.clone());
                    }
                }
                // the password is spent: a second question is more than this screen can answer
                if !self.answer(*conversation)? {
                    self.manager()?.send(&Request::Cancel {})?;
                    if let Stage::Password { screen, .. } = &mut self.stage {
                        screen.finish_auth(
                            now,
                            AuthOutcome::Error("authentication needs more than a password".into()),
                        )?;
                    }
                }
            }
            ManagerEvent::Notice { text, .. } => {
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.notify(text.clone());
                }
            }
            ManagerEvent::Result {
                outcome: Outcome::Accepted,
            } if self.authorising.is_some() => {
                // the machine goes down, or the manager refuses and shows the screen again (`PowerRefused`, `Reset`)
                eprintln!("cupertino-login: power action authorised");
            }
            ManagerEvent::Result {
                outcome: Outcome::Accepted,
            } => {
                // the spinner goes on until the manager shows the screen again (`Reset`)
                eprintln!("cupertino-login: session starting");
            }
            &ManagerEvent::Result { outcome } => {
                let Stage::Password { user, screen, .. } = &mut self.stage else {
                    unreachable!("the caller checked the stage");
                };
                if outcome == Outcome::Denied {
                    self.failed.record(&user.login);
                    eprintln!("cupertino-login: authentication failed");
                    screen.finish_auth(now, AuthOutcome::Denied)?;
                } else {
                    if outcome == Outcome::Locked {
                        self.locked = Some(user.login.clone());
                    }
                    let reason = refusal_text(outcome, screen.notice());
                    eprintln!("cupertino-login: authentication error: {reason}");
                    screen.finish_auth(now, AuthOutcome::Error(reason))?;
                }
            }
            ManagerEvent::SessionStarting { .. } => {}
            ManagerEvent::Hello { .. } | ManagerEvent::Welcome { .. } => {
                return Err("the session manager sent a message out of order".into())
            }
            ManagerEvent::Reset {}
            | ManagerEvent::Users { .. }
            | ManagerEvent::Refused { .. }
            | ManagerEvent::PowerRefused { .. } => unreachable!("the caller handles these"),
        }
        Ok(())
    }

    /// The authentication hand-off: answer the manager's prompt with the password of the open attempt. The password is
    /// moved into the request's secret, which is zeroed once the request is written.
    fn answer(&mut self, conversation: u64) -> Result<bool> {
        match self.pending.take() {
            Some(password) => {
                self.manager()?.send(&Request::Answer {
                    conversation,
                    secret: Secret::new(password).map_err(|e| e.to_string())?,
                })?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The items of the menus, which every screen has.
    fn menu_input(&mut self, now: f64, input: Input) -> Result<Option<Input>> {
        match input {
            Input::TogglePower => {
                if let Some(menu) = &mut self.power {
                    menu.toggle();
                }
            }
            Input::ToggleAccess => self.access_open = !self.access_open,
            Input::CloseAccess => self.access_open = false,
            Input::AccessOption(option) => {
                match option {
                    AccessOption::IncreaseContrast => {
                        self.access.increase_contrast = !self.access.increase_contrast
                    }
                    AccessOption::ReduceTransparency => {
                        self.access.reduce_transparency = !self.access.reduce_transparency
                    }
                    AccessOption::ReduceMotion => self.reduce_motion = !self.reduce_motion,
                }
                self.access_open = false;
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.set_reduce_motion(self.reduce_motion);
                }
            }
            Input::ClosePower => {
                if let Some(menu) = &mut self.power {
                    menu.close();
                }
            }
            Input::Power(row) => {
                if let Some(action) = self.power.as_mut().and_then(|menu| menu.activate(row)) {
                    self.perform(now, action)?;
                }
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    /// What the person did on the screens.
    pub(super) fn input(
        &mut self,
        now: f64,
        input: Input,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        let Some(input) = self.menu_input(now, input)? else {
            return Ok(());
        };
        match (&mut self.stage, input) {
            (Stage::Picking, Input::Select(index)) => self.picker.select(index),
            (Stage::Picking, Input::Focused(index)) => self.picker.focus(index),
            (Stage::Picking, Input::Choose(index)) => {
                self.picker.select(index);
                self.choose(now);
            }
            (Stage::Picking, Input::Step(direction)) => {
                let max_per_page = self.max_per_page();
                self.picker.step(direction, max_per_page);
                commands.push(Command::Focus(view::tile_key(self.picker.selected_index())));
            }
            (Stage::Picking, Input::Jump(jump)) => {
                let max_per_page = self.max_per_page();
                self.picker.jump(jump, max_per_page);
                commands.push(Command::Focus(view::tile_key(self.picker.selected_index())));
            }
            // Option+Return: the name and password screen in place of the picker, and back
            (Stage::Picking, Input::ToggleNames) => {
                let to = self.credentials(now);
                self.switch(now, to);
            }
            (Stage::Password { name: Some(_), .. }, Input::ToggleNames) => {
                self.authorising = None;
                self.switch(now, Stage::Picking);
            }
            (
                Stage::Password {
                    name: Some(entry), ..
                },
                Input::Name(change),
            ) => entry.set(&change.text),
            (
                Stage::Password {
                    name: Some(entry),
                    screen,
                    ..
                },
                Input::SubmitName,
            ) => {
                if let Some(action) = special_name(entry.text()) {
                    return self.special(now, action);
                }
                if !entry.text().is_empty() {
                    commands.push(Command::Focus(Key::from(format!(
                        "password-{}",
                        screen.field()
                    ))));
                }
            }
            (Stage::Password { screen, .. }, Input::Escape) => {
                // Escape goes back to the tiles unless an attempt is on its way to a decision
                let deciding = matches!(
                    screen.phase(),
                    Phase::Authenticating { .. } | Phase::Unlocking | Phase::Unlocked
                );
                if deciding {
                    screen.escape(now);
                } else {
                    self.authorising = None;
                    let home = self.home(now);
                    self.switch(now, home);
                }
            }
            (Stage::Password { screen, .. }, Input::Activity) => screen.activity(now),
            (Stage::Password { screen, .. }, Input::Edited(typed)) => screen.edit_to(now, typed),
            (Stage::Password { .. }, Input::Help) => self.help_open = !self.help_open,
            (Stage::Password { .. }, Input::Submit(typed)) => {
                return self.submit(now, typed, commands)
            }
            _ => {}
        }
        Ok(())
    }

    /// Return in the password field: with a name field, a name first, and the special names do what they name; then the
    /// attempt starts, and the password waits for the manager's prompt.
    fn submit(
        &mut self,
        now: f64,
        typed: view::Typed,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        let Stage::Password { user, screen, name } = &mut self.stage else {
            return Ok(());
        };
        if let Some(entry) = name {
            if let Some(action) = special_name(entry.text()) {
                return self.special(now, action);
            }
            if entry.text().is_empty() {
                commands.push(Command::Focus(Key::from("name")));
                return Ok(());
            }
            *user = User::typed(entry.text().to_owned());
        }
        let Some(password) = screen.submit(now, typed.secret()) else {
            return Ok(());
        };
        let request = match self.authorising {
            Some(action) => Request::AuthenticatePower {
                name: user.login.clone(),
                action,
            },
            None => Request::Authenticate {
                account: match user.uid {
                    Some(uid) => Account::Listed { uid },
                    None => Account::Typed {
                        name: user.login.clone(),
                    },
                },
            },
        };
        // the manager asks for the password with a prompt; until then it waits here
        self.pending = Some(password);
        self.manager()?.send(&request)?;
        Ok(())
    }

    /// Carry out a menu item. Restart and Shut Down while a user session is open would end it: they ask for an
    /// administrator's name and password first (the ellipsis of the title), on the name and password screens; the
    /// manager checks the password and then does the action. Any other item goes to the manager at once; a refusal
    /// comes back as `PowerRefused` and the greeter stays up.
    fn perform(&mut self, now: f64, action: Action) -> Result<()> {
        eprintln!("cupertino-login: power action: {}", action.log_name());
        let action = match action {
            Action::Restart => PowerAction::Restart,
            Action::ShutDown => PowerAction::ShutDown,
            Action::Sleep => PowerAction::Sleep,
        };
        if action != PowerAction::Sleep && self.power.as_ref().is_some_and(Menu::will_prompt) {
            // an attempt that is open (a password being checked) ends: the dialog starts its own
            self.manager()?.send(&Request::Cancel {})?;
            self.pending = None;
            self.authorising = Some(action);
            self.stage = self.credentials(now);
            if let Stage::Password { screen, .. } = &mut self.stage {
                screen.notify(authorisation_notice(action).to_owned());
            }
            return Ok(());
        }
        self.manager()?.send(&Request::Power { action })?;
        Ok(())
    }

    /// A special name typed in the name field: the power item it names, when the menu offers it.
    fn special(&mut self, now: f64, action: Action) -> Result<()> {
        if self
            .power
            .as_ref()
            .is_some_and(|menu| menu.items().contains(&action))
        {
            return self.perform(now, action);
        }
        eprintln!(
            "cupertino-login: power action not offered: {}",
            action.log_name()
        );
        Ok(())
    }
}
