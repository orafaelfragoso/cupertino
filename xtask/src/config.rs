//! `.config/validation.toml`: path rules, feature configurations and profiles.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Where the configuration lives, relative to the repository root.
pub const PATH: &str = ".config/validation.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub rules: Vec<Rule>,
    pub features: BTreeMap<String, FeatureConfig>,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// How many runs may build at once on the machine.
    #[serde(default = "default_slots")]
    pub slots: usize,
    /// The size in GiB past which a run empties the target directory's `debug` builds first.
    #[serde(default = "default_target_gib")]
    pub target_gib: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            slots: default_slots(),
            target_gib: default_target_gib(),
        }
    }
}

fn default_slots() -> usize {
    2
}

fn default_target_gib() -> u64 {
    150
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub patterns: Vec<String>,
    pub action: Action,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// The whole workspace.
    All,
    /// No Rust package.
    None,
    /// The packages the rule lists.
    Packages,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureConfig {
    /// Feature arguments given to cargo.
    pub args: Vec<String>,
    /// Build each package on its own, so feature unification across packages hides nothing.
    pub per_package: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Changed,
    Affected,
    All,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "changed" => Some(Self::Changed),
            "affected" => Some(Self::Affected),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Affected => "affected",
            Self::All => "all",
        }
    }
}

/// Which packages a formatting check covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fmt {
    Off,
    Changed,
    All,
}

/// Which packages a build check covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Coverage {
    Off,
    Scope,
    Workspace,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub scope: Scope,
    pub features: Vec<String>,
    pub fmt: Fmt,
    pub clippy: Coverage,
    pub nextest: Coverage,
    pub doctests: Coverage,
    pub layers: bool,
    pub gate: bool,
}

impl Config {
    /// Reads and validates the configuration under `root`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join(PATH);
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{PATH}: {error}"))?;
        let config: Self = toml::from_str(&text).map_err(|error| format!("{PATH}: {error}"))?;
        for rule in &config.rules {
            if rule.patterns.is_empty() {
                return Err(format!("{PATH}: rule `{}` has no patterns", rule.name));
            }
            if (rule.action == Action::Packages) == rule.packages.is_empty() {
                return Err(format!(
                    "{PATH}: rule `{}`: `packages` is given exactly when the action is \"packages\"",
                    rule.name
                ));
            }
        }
        for (name, profile) in &config.profiles {
            for feature in &profile.features {
                if !config.features.contains_key(feature) {
                    return Err(format!("{PATH}: profile `{name}` names an unknown feature configuration `{feature}`"));
                }
            }
        }
        if config.limits.slots == 0 {
            return Err(format!("{PATH}: [limits] slots must be at least 1"));
        }
        if config.limits.target_gib == 0 {
            return Err(format!("{PATH}: [limits] target_gib must be at least 1"));
        }
        Ok(config)
    }
}
