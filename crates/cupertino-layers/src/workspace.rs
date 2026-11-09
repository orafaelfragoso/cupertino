//! Every check on a checkout: `cargo metadata` read into layers, dependencies and licences, then the layer rules, the
//! denied paths and crates, the workspace licences and the external licence closure of the MPL-2.0 crates.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{
    check, closure, denied_crate_paths, dot, edges, licence_problems, Dep, Edge, Exception, Kind,
    Layer, Problem, Rules, DENIED_CRATES, DENIED_PATHS, EXCEPTIONS, RULES,
};

/// What a workspace is checked against.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub rules: Rules,
    pub exceptions: &'static [Exception],
    pub denied_paths: &'static [(Layer, &'static [Layer])],
    pub denied_crates: &'static [(&'static str, &'static [&'static str])],
    /// Crates that must be in the workspace under MPL-2.0.
    pub mpl_roots: &'static [&'static str],
}

/// The rules of this repository.
pub const POLICY: Policy = Policy {
    rules: RULES,
    exceptions: EXCEPTIONS,
    denied_paths: DENIED_PATHS,
    denied_crates: DENIED_CRATES,
    mpl_roots: closure::REQUIRED_MPL_ROOTS,
};

/// Why a workspace could not be checked.
#[derive(Debug)]
pub enum LayersError {
    /// Cargo could not be started.
    Cargo(std::io::Error),
    /// `cargo metadata` failed: what it printed on stderr.
    Metadata(String),
    /// The metadata is not what Cargo writes, or a crate declares no layer or one without a row in the rules.
    Manifest(String),
}

impl fmt::Display for LayersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayersError::Cargo(e) => write!(f, "cannot run cargo metadata: {e}"),
            LayersError::Metadata(stderr) => {
                write!(f, "cargo metadata failed: {}", stderr.trim_end())
            }
            LayersError::Manifest(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LayersError {}

/// What the checks found in a workspace.
#[derive(Debug)]
pub struct Findings {
    pub policy: Policy,
    /// Each workspace crate's declared layer.
    pub layers: BTreeMap<String, Layer>,
    /// The dependencies between workspace crates, one per kind.
    pub edges: Vec<Edge>,
    /// The layer, path and workspace licence problems, in the order the binary prints them.
    pub problems: Vec<Problem>,
    /// The licence closure problems of the MPL-2.0 crates, sorted.
    pub closure: Vec<Problem>,
}

impl Findings {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty() && self.closure.is_empty()
    }

    /// Every problem: [`Findings::problems`], then [`Findings::closure`].
    pub fn all_problems(&self) -> impl Iterator<Item = &Problem> {
        self.problems.iter().chain(&self.closure)
    }

    /// `80 crates, 378 internal dependencies, 5 exceptions, 0 problems`
    pub fn summary(&self) -> String {
        format!(
            "{} crates, {} internal dependencies, {} exceptions, {} problems",
            self.layers.len(),
            self.edges.len(),
            self.policy.exceptions.len(),
            self.problems.len() + self.closure.len()
        )
    }

    /// The DOT graph of the layers, as `cupertino-layers --graph` prints it.
    pub fn dot(&self) -> String {
        dot(
            "layers",
            self.policy.rules,
            &self.layers,
            &self.edges,
            self.policy.exceptions,
            self.policy.denied_paths,
        )
    }

    /// The crates of each layer, in layer order and by name.
    pub fn by_layer(&self) -> BTreeMap<Layer, Vec<&str>> {
        let mut by_layer: BTreeMap<Layer, Vec<&str>> = BTreeMap::new();
        for (name, layer) in &self.layers {
            by_layer.entry(*layer).or_default().push(name);
        }
        by_layer
    }
}

/// Checks the workspace that `root` is in against [`POLICY`] with `$CARGO`, else `cargo` from `PATH`.
pub fn check_workspace(root: &Path) -> Result<Findings, LayersError> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    check_workspace_with(root, &|| Command::new(&cargo), &POLICY)
}

/// Checks the workspace that `root` is in against `policy`. `cargo` makes the Cargo command to run (its environment
/// is the caller's choice); it runs `cargo metadata` twice in `root`, offline and with the lock file as it is.
pub fn check_workspace_with(
    root: &Path,
    cargo: &dyn Fn() -> Command,
    policy: &Policy,
) -> Result<Findings, LayersError> {
    let members = metadata(root, cargo, &["--no-deps"])?;
    let (layers, deps, licences) = read_workspace(&members, policy.rules)?;
    let edges = edges(&layers, &deps);
    let mut problems = check(policy.rules, &edges, policy.exceptions, policy.denied_paths);
    problems.extend(
        denied_crate_paths(&deps, policy.denied_crates)
            .into_iter()
            .map(Problem::DeniedPath),
    );
    problems.extend(licence_problems(&licences.licences, &deps, |name| {
        licences.has_text.get(name).copied().unwrap_or(false)
    }));
    let resolved = metadata(root, cargo, &["--all-features", "--locked", "--offline"])?;
    let closure =
        closure::check_roots(&resolved, policy.mpl_roots).map_err(LayersError::Manifest)?;
    Ok(Findings {
        policy: *policy,
        layers,
        edges,
        problems,
        closure,
    })
}

fn metadata(root: &Path, cargo: &dyn Fn() -> Command, args: &[&str]) -> Result<Value, LayersError> {
    let out = cargo()
        .args(["metadata", "--format-version", "1"])
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(LayersError::Cargo)?;
    if !out.status.success() {
        return Err(LayersError::Metadata(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| LayersError::Manifest(format!("cargo metadata did not print JSON: {e}")))
}

/// The licence of each workspace crate, and whether a `LICENSE` file with the MPL-2.0 text sits next to its manifest.
struct Licences {
    licences: BTreeMap<String, String>,
    has_text: BTreeMap<String, bool>,
}

type Workspace = (BTreeMap<String, Layer>, BTreeSet<Dep>, Licences);

/// Each workspace crate's declared layer, every dependency (normal, build, dev) of a workspace crate, and its licence.
fn read_workspace(meta: &Value, rules: Rules) -> Result<Workspace, LayersError> {
    let bad = |what: &str| LayersError::Manifest(format!("cargo metadata has no {what}"));
    let mut layers = BTreeMap::new();
    let mut deps = BTreeSet::new();
    let mut licences = Licences {
        licences: BTreeMap::new(),
        has_text: BTreeMap::new(),
    };
    for p in meta["packages"].as_array().ok_or_else(|| bad("packages"))? {
        let name = p["name"].as_str().ok_or_else(|| bad("package name"))?;
        let layer = p["metadata"]["cupertino"]["layer"]
            .as_str()
            .ok_or_else(|| {
                LayersError::Manifest(format!(
                    "{name}: no `[package.metadata.cupertino] layer` in its manifest"
                ))
            })?;
        let layer = Layer::from_name(layer)
            .filter(|l| rules.iter().any(|(r, _)| r == l))
            .ok_or_else(|| {
                LayersError::Manifest(format!("{name}: `{layer}` is not a layer in RULES"))
            })?;
        layers.insert(name.to_owned(), layer);
        licences.licences.insert(
            name.to_owned(),
            p["license"].as_str().unwrap_or_default().to_owned(),
        );
        let manifest = Path::new(
            p["manifest_path"]
                .as_str()
                .ok_or_else(|| bad("manifest path"))?,
        );
        let text = std::fs::read_to_string(manifest.with_file_name("LICENSE")).unwrap_or_default();
        licences.has_text.insert(
            name.to_owned(),
            text.trim_start()
                .starts_with("Mozilla Public License Version 2.0"),
        );
        for d in p["dependencies"]
            .as_array()
            .ok_or_else(|| bad("dependencies"))?
        {
            let to = d["name"].as_str().ok_or_else(|| bad("dependency name"))?;
            let kind = Kind::from_metadata(d["kind"].as_str()).ok_or_else(|| {
                LayersError::Manifest(format!(
                    "{name} -> {to}: unknown dependency kind {}",
                    d["kind"]
                ))
            })?;
            deps.insert((name.to_owned(), to.to_owned(), kind));
        }
    }
    Ok((layers, deps, licences))
}
