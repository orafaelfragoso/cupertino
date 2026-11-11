//! The typed result: a report of checks, each with an area, a status, the facts it found and a fix.

use std::fmt;

use serde::Serialize;

/// The version of the JSON form; it changes when a field is removed or changes meaning.
pub const SCHEMA: u32 = 1;

/// What a check looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    /// The primary GPU, its driver and the settings the driver needs.
    Gpu,
    /// The graphical session this runs in: its type, its display and its logind record.
    Session,
    /// The desktop portal backend and its configuration.
    Portals,
    /// The system and user services of the desktop.
    Services,
    /// The programs of the shell processes: builds in the development folder that start instead of a newer installed
    /// program.
    Shell,
    /// How the last sessions ended, and the panic or fatal signal the server logged.
    Crash,
    /// Where the logs are.
    Logs,
    /// The diagnostic events of the last day: the errors and fatal ends the desktop's processes recorded.
    Events,
}

impl Area {
    pub const ALL: [Area; 8] = [
        Area::Gpu,
        Area::Session,
        Area::Portals,
        Area::Services,
        Area::Shell,
        Area::Crash,
        Area::Logs,
        Area::Events,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Area::Gpu => "gpu",
            Area::Session => "session",
            Area::Portals => "portals",
            Area::Services => "services",
            Area::Shell => "shell",
            Area::Crash => "crash",
            Area::Logs => "logs",
            Area::Events => "events",
        }
    }

    pub fn from_name(name: &str) -> Option<Area> {
        Area::ALL.into_iter().find(|area| area.name() == name)
    }
}

/// How a check came out, from best to worst after `Skip`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Nothing to check here: not in a session, or the tool it needs is missing.
    Skip,
    Ok,
    /// It works, but this is likely to cause trouble.
    Warn,
    /// This is broken.
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Skip => "skip",
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        }
    }
}

/// One named value a check found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fact {
    pub key: &'static str,
    pub value: String,
}

/// One check: `id` is stable and dotted (`gpu.driver`), `summary` is one line for a person, and `fix` says what to do
/// when the status is `warn` or `fail`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub area: Area,
    pub status: Status,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Fact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    pub fn new(id: &'static str, area: Area, status: Status, summary: impl Into<String>) -> Self {
        Check {
            id,
            area,
            status,
            summary: summary.into(),
            facts: Vec::new(),
            fix: None,
        }
    }

    pub fn fact(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.facts.push(Fact {
            key,
            value: value.into(),
        });
        self
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub checks: Vec<Check>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let width = self.checks.iter().map(|c| c.id.len()).max().unwrap_or(0);
        for check in &self.checks {
            writeln!(
                f,
                "{:<4}  {:<width$}  {}",
                check.status.label(),
                check.id,
                check.summary
            )?;
            for fact in &check.facts {
                writeln!(f, "      {:<width$}  {}: {}", "", fact.key, fact.value)?;
            }
            if let Some(fix) = &check.fix {
                writeln!(f, "      {:<width$}  fix: {fix}", "")?;
            }
        }
        Ok(())
    }
}
