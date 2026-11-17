//! What the session menu may offer on this machine: which of Sleep, Restart, Shut Down, Log Out, Switch User and Lock work,
//! from logind's answers to `CanSuspend`, `CanReboot` and `CanPowerOff` and from whether the session manager is there. Pure:
//! no bus.

use std::collections::HashMap;

/// logind's answer to a `Can…` question: `"yes"`, `"challenge"` (allowed after authentication), `"no"` or `"na"` (the
/// machine cannot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    Challenge,
    No,
    NotAvailable,
}

impl Answer {
    /// The answer logind wrote; anything it does not say is "not available".
    pub fn from_logind(text: &str) -> Self {
        match text {
            "yes" => Self::Yes,
            "challenge" => Self::Challenge,
            "no" => Self::No,
            _ => Self::NotAvailable,
        }
    }

    /// The action may be tried: a "challenge" asks for authentication when it is.
    pub fn allows(self) -> bool {
        matches!(self, Self::Yes | Self::Challenge)
    }
}

/// What the decision rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub can_suspend: Answer,
    pub can_reboot: Answer,
    pub can_power_off: Answer,
    /// `cupertino-sessiond` is on the system bus: it alone can hand the console to the login screen.
    pub session_manager: bool,
}

/// The keys of the `Capabilities` property, in the order of [`Capabilities::entries`].
pub const KEYS: [&str; 6] = [
    "sleep",
    "restart",
    "shut-down",
    "log-out",
    "switch-user",
    "lock",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub sleep: bool,
    pub restart: bool,
    pub shut_down: bool,
    pub log_out: bool,
    pub switch_user: bool,
    pub lock: bool,
}

impl Capabilities {
    /// Log out and lock need nothing but this process (it runs only with a logind session); the power actions need logind's
    /// consent, which holds with or without the session manager (without it the process asks logind itself); the switch to
    /// the login window needs the session manager.
    pub fn new(inputs: &Inputs) -> Self {
        Self {
            sleep: inputs.can_suspend.allows(),
            restart: inputs.can_reboot.allows(),
            shut_down: inputs.can_power_off.allows(),
            log_out: true,
            switch_user: inputs.session_manager,
            lock: true,
        }
    }

    /// Why each action that is off is off, for the log.
    pub fn reasons(inputs: &Inputs) -> Vec<String> {
        let mut reasons = Vec::new();
        for (name, answer) in [
            ("sleep", inputs.can_suspend),
            ("restart", inputs.can_reboot),
            ("shut-down", inputs.can_power_off),
        ] {
            if !answer.allows() {
                reasons.push(format!("{name}: logind answers {answer:?}"));
            }
        }
        if !inputs.session_manager {
            reasons.push("switch-user: no session manager on the system bus".into());
        }
        reasons
    }

    pub fn entries(&self) -> [(&'static str, bool); 6] {
        [
            (KEYS[0], self.sleep),
            (KEYS[1], self.restart),
            (KEYS[2], self.shut_down),
            (KEYS[3], self.log_out),
            (KEYS[4], self.switch_user),
            (KEYS[5], self.lock),
        ]
    }

    /// The value of the `Capabilities` property.
    pub fn to_map(&self) -> HashMap<String, bool> {
        self.entries()
            .into_iter()
            .map(|(key, on)| (key.to_string(), on))
            .collect()
    }
}
