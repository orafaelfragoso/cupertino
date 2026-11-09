//! Typed diagnostics of a Cupertino installation and the session it runs in: the GPU and its driver, the session, the
//! desktop portal, the services, how the last sessions ended, where the logs are and the recent diagnostic events. Every
//! check reads files, asks systemd for unit states and reads the journal; nothing is changed and no device is opened.
//! [`run`] takes its inputs as [`Sources`], so the tests run it against a fixture tree. [`events`] reads the diagnostic
//! events back.

mod checks;
pub mod events;
pub mod report;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use report::{Area, Check, Fact, Report, Status, SCHEMA};

/// Whose service manager a unit belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    System,
    User,
}

/// A unit's state, or `None` when the service manager cannot be asked.
pub type UnitState = Box<dyn Fn(Scope, &str) -> Option<Unit>>;

/// A unit's states as systemd names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// `active`, `inactive`, `failed`, `activating`, or [`NOT_FOUND`] when no such unit is installed.
    pub active: String,
    /// `enabled`, `disabled`, `static`, `masked`, or empty when systemd gives none.
    pub file: String,
}

/// The state of a unit that is not installed.
pub const NOT_FOUND: &str = "not-found";

/// What the checks read.
pub struct Sources {
    /// `/sys`.
    pub sysfs: PathBuf,
    /// `/`: installed files and `/run` are read below it.
    pub root: PathBuf,
    /// The environment of the session asking.
    pub env: HashMap<String, String>,
    pub unit_state: UnitState,
    /// Reads the system journal's diagnostic events ([`events::journalctl`] on this machine).
    pub journal: JournalFn,
    /// The command that follows the system journal from a point on, printing `--output=json` lines
    /// ([`events::journalctl_follow`] on this machine).
    pub journal_follow: JournalFollowFn,
}

/// Reads the system journal's diagnostic events for a filter.
pub type JournalFn = Box<dyn Fn(&events::Filter) -> events::JournalResult>;

/// Makes the command that follows the system journal.
pub type JournalFollowFn = Box<dyn Fn(&events::FollowFrom) -> Command>;

impl Sources {
    /// This machine and the calling process's environment.
    pub fn system() -> Self {
        Sources {
            sysfs: PathBuf::from("/sys"),
            root: PathBuf::from("/"),
            env: std::env::vars().collect(),
            unit_state: Box::new(systemctl_state),
            journal: Box::new(events::journalctl),
            journal_follow: Box::new(events::journalctl_follow),
        }
    }

    fn var(&self, key: &str) -> Option<&str> {
        self.env
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// A path of the installed system, below [`Sources::root`].
    fn installed(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// The directory of the session's logs, as `cupertino-session` and the server name it.
    pub fn state_dir(&self) -> Option<PathBuf> {
        let state = self
            .var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| self.var("HOME").map(|h| Path::new(h).join(".local/state")))?;
        Some(state.join("cupertino"))
    }

    /// The user's store of diagnostic events, as the emitter names it (`store::user_dir`).
    pub fn store_dir(&self) -> Option<PathBuf> {
        cupertino_diagnostic_events::store::user_dir(|k| self.var(k).map(Into::into))
    }
}

fn systemctl_state(scope: Scope, unit: &str) -> Option<Unit> {
    let mut command = Command::new("systemctl");
    if scope == Scope::User {
        command.arg("--user");
    }
    let output = command
        .args([
            OsStr::new("show"),
            OsStr::new("--property=LoadState,ActiveState,UnitFileState"),
            OsStr::new(unit),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // with no manager to ask, systemctl prints nothing
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    let active = match (field("LoadState"), field("ActiveState")) {
        (Some("not-found"), _) => NOT_FOUND,
        (_, Some(active)) => active,
        _ => return None,
    };
    Some(Unit {
        active: active.to_owned(),
        file: field("UnitFileState").unwrap_or_default().to_owned(),
    })
}

/// The checks of `areas`, in the order of [`Area::ALL`]; every area when `areas` is empty.
pub fn run(sources: &Sources, areas: &[Area]) -> Report {
    let wanted = |area| areas.is_empty() || areas.contains(&area);
    let mut checks = Vec::new();
    for area in Area::ALL.into_iter().filter(|a| wanted(*a)) {
        checks.extend(match area {
            Area::Gpu => checks::gpu(sources),
            Area::Session => checks::session(sources),
            Area::Portals => checks::portals(sources),
            Area::Services => checks::services(sources),
            Area::Crash => checks::crash(sources),
            Area::Logs => checks::logs(sources),
            Area::Events => checks::events(sources),
        });
    }
    Report {
        schema: SCHEMA,
        checks,
    }
}
