//! Every check on a checkout: `cargo metadata` read into layers, dependencies, features and licences, the sources of
//! the pure layers, the `clippy.toml` files and the programs' alias read, then every rule checked and compared with the
//! remaining-work file and the graph snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{
    check, clippy_toml, crate_paths, dot, edges, external_problems, is_licence, matches, sources,
    without_counts, Dep, Edge, External, Facade, Kind, Layer, Problem, Rules, DISALLOWED_METHODS,
    ENFORCED, EXTERNAL_CRATES, FACADES, GRAPH_SNAPSHOT, IMPURE, PROGRAMS_ALIAS, PURE_LAYERS,
    REMAINING, RULES, TEST_FEATURE, THREADLESS_LAYERS, WINDOW_SYSTEM,
};

/// What a workspace is checked against.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub rules: Rules,
    /// The closed layers, whose problems the remaining-work file may not list.
    pub enforced: &'static [Layer],
    pub facades: &'static [Facade],
    pub window_system: &'static [(&'static str, &'static [&'static str])],
    pub external_crates: &'static [External],
    /// The layers whose sources must be pure, and the standard library modules they must not use.
    pub pure_layers: &'static [Layer],
    pub impure: &'static [&'static str],
    /// The layers whose crates start no threads in their `src/`.
    pub threadless_layers: &'static [Layer],
    /// What the `clippy.toml` of each `shell` and `apps` crate disallows.
    pub disallowed_methods: &'static [(&'static str, &'static str)],
    /// The feature, as package and feature, that only dev-dependencies turn on.
    pub test_feature: (&'static str, &'static str),
    /// The Cargo alias that names every feature but the test feature.
    pub programs_alias: &'static str,
}

/// The rules of this repository.
pub const POLICY: Policy = Policy {
    rules: RULES,
    enforced: ENFORCED,
    facades: FACADES,
    window_system: WINDOW_SYSTEM,
    external_crates: EXTERNAL_CRATES,
    pure_layers: PURE_LAYERS,
    impure: IMPURE,
    threadless_layers: THREADLESS_LAYERS,
    disallowed_methods: DISALLOWED_METHODS,
    test_feature: TEST_FEATURE,
    programs_alias: PROGRAMS_ALIAS,
};

/// The Cargo configuration of a workspace, relative to its root, which holds the programs' alias.
const CARGO_CONFIG: &str = ".cargo/config.toml";

/// The layers whose crates hold the generated `clippy.toml`.
const CLIPPY_LAYERS: &[Layer] = &[Layer::Shell, Layer::Apps];

const REMAINING_HEADER: &str = "\
# The target layer rules that crates still break, one problem per line: <layer> <rule> <crate> ...
# Written by `cargo run -q -p cupertino-layers -- --write-remaining`; never edit by hand. Lines only leave.
";

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
    /// The workspace root.
    pub root: PathBuf,
    /// Each workspace crate's declared layer.
    pub layers: BTreeMap<String, Layer>,
    /// The dependencies between workspace crates, one per kind.
    pub edges: Vec<Edge>,
    /// Every problem of the rules, sorted by its line, listed or not.
    pub problems: Vec<Problem>,
    /// The lines of the committed remaining-work file (none when the workspace has none).
    pub remaining: Vec<String>,
    /// [`Problem::StaleRemaining`] and [`Problem::EnforcedRemaining`] for lines of the file.
    pub listing: Vec<Problem>,
    /// [`Problem::StaleGraph`] when the workspace has a committed graph snapshot that no longer matches it.
    pub snapshot: Vec<Problem>,
    /// The `clippy.toml` of each `shell` and `apps` crate, by crate, as absolute paths.
    pub clippy_files: BTreeMap<String, PathBuf>,
}

impl Findings {
    pub fn is_clean(&self) -> bool {
        self.all_problems().next().is_none()
    }

    /// The problems of the rules that the remaining-work file does not excuse: those it does not list, and every one
    /// of an enforced layer.
    pub fn unlisted(&self) -> impl Iterator<Item = &Problem> {
        let listed: BTreeSet<&str> = self.remaining.iter().map(String::as_str).collect();
        self.problems.iter().filter(move |p| {
            p.layer().is_some_and(|l| self.policy.enforced.contains(&l))
                || p.line().is_none_or(|line| !listed.contains(line.as_str()))
        })
    }

    /// Every problem: [`Findings::unlisted`], [`Findings::listing`], then [`Findings::snapshot`].
    pub fn all_problems(&self) -> impl Iterator<Item = &Problem> {
        self.unlisted().chain(&self.listing).chain(&self.snapshot)
    }

    /// The problems of a run that prints the graph: every one but an out-of-date snapshot, which that run makes again.
    pub fn problems_printing_the_graph(&self) -> impl Iterator<Item = &Problem> {
        self.all_problems()
            .filter(|p| !matches!(p, Problem::StaleGraph(_)))
    }

    /// The problems of a run that writes the remaining-work file and so makes its lines again: those of enforced
    /// layers, which it cannot list.
    pub fn problems_writing_the_remaining_work(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.layer().is_some_and(|l| self.policy.enforced.contains(&l)))
    }

    /// `91 crates, 390 internal dependencies, 120 listed, 0 problems`
    pub fn summary(&self) -> String {
        format!(
            "{} crates, {} internal dependencies, {} listed, {} problems",
            self.layers.len(),
            self.edges.len(),
            self.remaining.len(),
            self.all_problems().count()
        )
    }

    /// The DOT graph of the layers, as `cupertino-layers --graph` prints it.
    pub fn dot(&self) -> String {
        dot("layers", self.policy.rules, &self.layers, &self.edges)
    }

    /// The remaining-work file for the problems found: every line of a layer that is not enforced, sorted.
    pub fn remaining_file(&self) -> String {
        let lines: BTreeSet<String> = self
            .problems
            .iter()
            .filter(|p| !p.layer().is_some_and(|l| self.policy.enforced.contains(&l)))
            .filter_map(Problem::line)
            .collect();
        let mut s = REMAINING_HEADER.to_owned();
        for line in lines {
            s.push_str(&line);
            s.push('\n');
        }
        s
    }

    /// Writes [`Findings::remaining_file`] to the workspace's remaining-work file.
    pub fn write_remaining(&self) -> std::io::Result<()> {
        std::fs::write(self.root.join(REMAINING), self.remaining_file())
    }

    /// Writes the `clippy.toml` of the `shell` and `apps` crates in `crates`, or of all of them when `crates` is
    /// empty, where the file differs from the list, and returns the files written. A name that is not a `shell` or
    /// `apps` crate is an [`std::io::ErrorKind::InvalidInput`] error, and nothing is written.
    pub fn write_clippy(&self, crates: &[&str]) -> std::io::Result<Vec<&Path>> {
        if let Some(unknown) = crates.iter().find(|c| !self.clippy_files.contains_key(**c)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{unknown} is not a shell or apps crate of the workspace"),
            ));
        }
        let content = clippy_toml(self.policy.disallowed_methods);
        let mut written = Vec::new();
        for (name, path) in &self.clippy_files {
            if !crates.is_empty() && !crates.contains(&name.as_str()) {
                continue;
            }
            if std::fs::read_to_string(path).ok().as_deref() != Some(content.as_str()) {
                std::fs::write(path, &content)?;
                written.push(path.as_path());
            }
        }
        Ok(written)
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
    let workspace_root =
        PathBuf::from(members["workspace_root"].as_str().ok_or_else(|| {
            LayersError::Manifest("cargo metadata has no workspace root".to_owned())
        })?);
    let crates = read_workspace(&members, policy.rules)?;
    let edges = edges(&crates.layers, &crates.deps);
    let mut problems = check(policy.rules, &edges, policy.facades);
    problems.extend(external_problems(
        &crates.layers,
        &crates.deps,
        policy.external_crates,
    ));
    let resolved = metadata(root, cargo, &["--all-features", "--locked", "--offline"])?;
    let normal = normal_dependencies(&resolved)?;
    for (start, layer) in &crates.layers {
        for (source, targets) in policy.window_system {
            if matches(source, start) {
                problems.extend(
                    crate_paths(&normal, start, targets)
                        .into_iter()
                        .map(|path| Problem::WindowSystem {
                            from_layer: *layer,
                            path,
                        }),
                );
            }
        }
    }
    for (name, dir) in &crates.dirs {
        let layer = crates.layers[name];
        if policy.pure_layers.contains(&layer) {
            for file in sources::rust_files(dir, dir) {
                let src = std::fs::read_to_string(dir.join(&file)).unwrap_or_default();
                for what in sources::impurities(&src, policy.impure) {
                    problems.push(Problem::Impure {
                        krate: name.clone(),
                        layer,
                        file: file.clone(),
                        what,
                    });
                }
            }
        }
        if policy.threadless_layers.contains(&layer) {
            for file in sources::rust_files(dir, &dir.join("src")) {
                let src = std::fs::read_to_string(dir.join(&file)).unwrap_or_default();
                for call in sources::thread_starts(&src) {
                    problems.push(Problem::StartsThread {
                        krate: name.clone(),
                        layer,
                        file: file.clone(),
                        call,
                    });
                }
            }
        }
        let licence = &crates.licences[name];
        if !licence.as_deref().is_some_and(is_licence) {
            problems.push(Problem::UndeclaredLicence {
                krate: name.clone(),
                layer,
                licence: licence.clone().unwrap_or_default(),
            });
        }
    }
    let clippy_files: BTreeMap<String, PathBuf> = crates
        .dirs
        .iter()
        .filter(|(name, _)| CLIPPY_LAYERS.contains(&crates.layers[*name]))
        .map(|(name, dir)| (name.clone(), dir.join("clippy.toml")))
        .collect();
    let content = clippy_toml(policy.disallowed_methods);
    for (name, path) in &clippy_files {
        if std::fs::read_to_string(path).ok().as_deref() != Some(content.as_str()) {
            problems.push(Problem::DisallowedMethods {
                krate: name.clone(),
                layer: crates.layers[name],
            });
        }
    }
    problems.extend(test_feature_problems(&members, policy.test_feature)?);
    // a workspace without a Cargo configuration (a test workspace) packages no programs
    if let Ok(config) = std::fs::read_to_string(workspace_root.join(CARGO_CONFIG)) {
        problems.extend(program_features_problem(&members, &config, policy)?);
    }
    let mut sorted: BTreeMap<String, Problem> = BTreeMap::new();
    for p in problems {
        sorted
            .entry(p.line().unwrap_or_else(|| p.to_string()))
            .or_insert(p);
    }
    let problems: Vec<Problem> = sorted.into_values().collect();

    let remaining: Vec<String> = std::fs::read_to_string(workspace_root.join(REMAINING))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    let lines: BTreeSet<String> = problems.iter().filter_map(Problem::line).collect();
    let listing = remaining
        .iter()
        .filter_map(|line| {
            let layer = line.split(' ').next().and_then(Layer::from_name);
            if layer.is_some_and(|l| policy.enforced.contains(&l)) {
                Some(Problem::EnforcedRemaining(line.clone()))
            } else if !lines.contains(line) {
                Some(Problem::StaleRemaining(line.clone()))
            } else {
                None
            }
        })
        .collect();

    let mut findings = Findings {
        policy: *policy,
        root: workspace_root,
        layers: crates.layers,
        edges,
        problems,
        remaining,
        listing,
        snapshot: Vec::new(),
        clippy_files,
    };
    // a workspace without the snapshot (another checkout's layout, a test workspace) has nothing to keep current
    let committed = std::fs::read_to_string(findings.root.join(GRAPH_SNAPSHOT)).ok();
    if committed.is_some_and(|c| without_counts(&c) != without_counts(&findings.dot())) {
        findings
            .snapshot
            .push(Problem::StaleGraph(GRAPH_SNAPSHOT.to_owned()));
    }
    Ok(findings)
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

/// The workspace crates as their manifests declare them.
struct Crates {
    layers: BTreeMap<String, Layer>,
    /// Every dependency (normal, build, dev) of a workspace crate, on workspace and external crates.
    deps: BTreeSet<Dep>,
    /// The directory of each crate's manifest.
    dirs: BTreeMap<String, PathBuf>,
    /// The `license` of each crate, or `license-file` when it names one instead.
    licences: BTreeMap<String, Option<String>>,
}

fn read_workspace(meta: &Value, rules: Rules) -> Result<Crates, LayersError> {
    let bad = |what: &str| LayersError::Manifest(format!("cargo metadata has no {what}"));
    let mut crates = Crates {
        layers: BTreeMap::new(),
        deps: BTreeSet::new(),
        dirs: BTreeMap::new(),
        licences: BTreeMap::new(),
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
        crates.layers.insert(name.to_owned(), layer);
        let licence = p["license"].as_str().map(str::to_owned).or_else(|| {
            p["license_file"]
                .as_str()
                .map(|_| "LicenseRef-file".to_owned())
        });
        crates.licences.insert(name.to_owned(), licence);
        let manifest = Path::new(
            p["manifest_path"]
                .as_str()
                .ok_or_else(|| bad("manifest path"))?,
        );
        let dir = manifest.parent().ok_or_else(|| bad("manifest directory"))?;
        crates.dirs.insert(name.to_owned(), dir.to_path_buf());
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
            crates.deps.insert((name.to_owned(), to.to_owned(), kind));
        }
    }
    Ok(crates)
}

/// A problem for each normal or build dependency, and each feature, of a workspace package in `meta` that turns on
/// `test` (package and feature).
fn test_feature_problems(meta: &Value, test: (&str, &str)) -> Result<Vec<Problem>, LayersError> {
    let bad = |what: &str| LayersError::Manifest(format!("cargo metadata has no {what}"));
    let (package, feature) = test;
    let named = format!("{package}/{feature}");
    let mut out = Vec::new();
    for p in meta["packages"].as_array().ok_or_else(|| bad("packages"))? {
        let name = p["name"].as_str().ok_or_else(|| bad("package name"))?;
        for d in p["dependencies"]
            .as_array()
            .ok_or_else(|| bad("dependencies"))?
        {
            let kind = Kind::from_metadata(d["kind"].as_str());
            let enables = d["name"].as_str() == Some(package)
                && d["features"]
                    .as_array()
                    .is_some_and(|f| f.iter().any(|f| f.as_str() == Some(feature)));
            if enables && matches!(kind, Some(Kind::Normal | Kind::Build)) {
                out.push(Problem::TestFeature {
                    krate: name.to_owned(),
                    feature: named.clone(),
                    how: format!("a {} dependency", kind.unwrap_or(Kind::Normal)),
                });
            }
        }
        let features = p["features"]
            .as_object()
            .ok_or_else(|| bad("package features"))?;
        for (own, enabled) in features {
            let enables = enabled.as_array().is_some_and(|values| {
                values.iter().filter_map(Value::as_str).any(|v| {
                    v == named
                        || v == format!("{package}?/{feature}")
                        || name == package && v == feature
                })
            });
            if enables {
                out.push(Problem::TestFeature {
                    krate: name.to_owned(),
                    feature: named.clone(),
                    how: format!("its feature `{own}`"),
                });
            }
        }
    }
    Ok(out)
}

/// The problem of the programs' alias in `config` (a Cargo configuration), when it is missing or does not name exactly
/// the features of the workspace packages in `meta`, `default` and the test feature aside.
fn program_features_problem(
    meta: &Value,
    config: &str,
    policy: &Policy,
) -> Result<Option<Problem>, LayersError> {
    let bad = |what: &str| LayersError::Manifest(format!("cargo metadata has no {what}"));
    let (package, feature) = policy.test_feature;
    let test = format!("{package}/{feature}");
    let mut expected = BTreeSet::new();
    for p in meta["packages"].as_array().ok_or_else(|| bad("packages"))? {
        let name = p["name"].as_str().ok_or_else(|| bad("package name"))?;
        let features = p["features"]
            .as_object()
            .ok_or_else(|| bad("package features"))?;
        expected.extend(
            features
                .keys()
                .filter(|f| *f != "default")
                .map(|f| format!("{name}/{f}"))
                .filter(|f| *f != test),
        );
    }
    let alias = config
        .parse::<toml::Table>()
        .map_err(|e| LayersError::Manifest(format!("{CARGO_CONFIG}: {e}")))?
        .get("alias")
        .and_then(|a| a.get(policy.programs_alias))
        .map(alias_words);
    let named = alias.as_deref().map(named_features).unwrap_or_default();
    let missing: Vec<String> = expected.difference(&named).cloned().collect();
    let extra: Vec<String> = named.difference(&expected).cloned().collect();
    Ok(
        (alias.is_none() || !missing.is_empty() || !extra.is_empty()).then(|| {
            Problem::ProgramFeatures {
                alias: policy.programs_alias.to_owned(),
                missing,
                extra,
            }
        }),
    )
}

/// The words of a Cargo alias: an array of them, or a string Cargo splits at whitespace.
fn alias_words(alias: &toml::Value) -> Vec<String> {
    match alias {
        toml::Value::String(s) => s.split_whitespace().map(str::to_owned).collect(),
        toml::Value::Array(words) => words
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The features `words` turn on, as `--features` and `-F` name them (`--all-features` stands for itself).
fn named_features(words: &[String]) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let list = match word.as_str() {
            "--features" | "-F" => words.next().map(String::as_str),
            "--all-features" => {
                named.insert(word.clone());
                None
            }
            w => w.strip_prefix("--features="),
        };
        named.extend(
            list.into_iter()
                .flat_map(|l| l.split([',', ' ']))
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        );
    }
    named
}

/// The normal dependencies of every resolved package, by name, from the full `cargo metadata`.
fn normal_dependencies(meta: &Value) -> Result<BTreeMap<String, BTreeSet<String>>, LayersError> {
    let bad = |what: &str| LayersError::Manifest(format!("resolved cargo metadata has no {what}"));
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    for p in meta["packages"].as_array().ok_or_else(|| bad("packages"))? {
        names.insert(
            p["id"].as_str().ok_or_else(|| bad("package id"))?,
            p["name"].as_str().ok_or_else(|| bad("package name"))?,
        );
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in meta["resolve"]["nodes"]
        .as_array()
        .ok_or_else(|| bad("resolved nodes"))?
    {
        let from = node["id"]
            .as_str()
            .and_then(|id| names.get(id))
            .ok_or_else(|| bad("known node id"))?;
        let deps = out.entry((*from).to_owned()).or_default();
        for dep in node["deps"]
            .as_array()
            .ok_or_else(|| bad("node dependencies"))?
        {
            let normal = dep["dep_kinds"]
                .as_array()
                .ok_or_else(|| bad("dependency kinds"))?
                .iter()
                .any(|k| k["kind"].is_null());
            if normal {
                let to = dep["pkg"]
                    .as_str()
                    .and_then(|id| names.get(id))
                    .ok_or_else(|| bad("known dependency id"))?;
                deps.insert((*to).to_owned());
            }
        }
    }
    Ok(out)
}
