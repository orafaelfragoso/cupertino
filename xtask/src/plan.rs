//! Planning: which files changed, which packages they select, and which checks run on them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::config::{Action, Config, Coverage, Fmt, Profile, Scope};
use crate::glob;

/// A workspace member, as `cargo metadata` describes it.
#[derive(Debug)]
pub struct Package {
    pub name: String,
    /// Its directory relative to the repository root, without a trailing `/`.
    pub directory: String,
    /// The workspace members it depends on, in any dependency kind.
    pub dependencies: BTreeSet<String>,
    /// Whether it has a library target (and so can have doctests).
    pub library: bool,
}

/// The workspace as `cargo metadata --no-deps` reports it.
#[derive(Debug)]
pub struct Workspace {
    pub target_directory: PathBuf,
    pub packages: BTreeMap<String, Package>,
}

/// What the repository's git state is.
#[derive(Debug, Default, Serialize)]
pub struct GitState {
    pub base_ref: String,
    pub merge_base: String,
    pub head: String,
    pub tree: String,
    pub dirty: bool,
    pub changed_files: Vec<String>,
}

/// One planned check.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    /// The command line as it is run; the gate runs inside xtask.
    pub command: Vec<String>,
    #[serde(skip)]
    pub kind: Kind,
    /// Why the check does not run, when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Gate,
    /// The vendored trees reverse to their upstream releases (`crate::vendored`).
    Vendored,
}

/// The whole plan of a run.
#[derive(Debug, Default)]
pub struct Plan {
    pub git: GitState,
    pub changed_packages: Vec<String>,
    pub affected_packages: Vec<String>,
    pub selected: Vec<String>,
    pub total_count: usize,
    pub whole_workspace: bool,
    pub fallback: bool,
    pub fallback_reasons: Vec<String>,
    pub notes: Vec<String>,
    pub scope_empty: bool,
    pub feature_configs: Vec<String>,
    pub checks: Vec<Check>,
}

/// Runs a command and returns its standard output, or a message naming the command and its error.
pub fn output(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let shown = || format!("{program} {}", args.join(" "));
    let result = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("{}: {error}", shown()))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!("{}: {}: {}", shown(), result.status, stderr.trim()));
    }
    String::from_utf8(result.stdout).map_err(|_| format!("{}: output is not UTF-8", shown()))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    output(root, "git", args)
}

fn null_separated(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

/// Whether `root` is a linked worktree (`git worktree add`) rather than the repository's main checkout: its git
/// directory differs from the common one. A failed query counts as the main checkout.
pub fn linked_worktree(root: &Path) -> bool {
    git(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )
    .is_ok_and(|text| {
        let mut lines = text.lines();
        matches!((lines.next(), lines.next()), (Some(dir), Some(common)) if dir != common)
    })
}

/// The repository root, from git.
pub fn repository_root() -> Result<PathBuf, String> {
    let top = git(Path::new("."), &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(top.trim()))
}

/// The files changed since `base` forked from HEAD, committed or not.
pub fn git_state(root: &Path, base: &str) -> Result<GitState, String> {
    let base_commit = format!("{base}^{{commit}}");
    git(root, &["rev-parse", "--verify", "--quiet", &base_commit])
        .map_err(|_| format!("base `{base}` does not name a commit"))?;
    let merge_base = git(root, &["merge-base", base, "HEAD"])?.trim().to_owned();
    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_owned();
    let committed = git(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &merge_base,
            "HEAD",
            "--",
        ],
    )?;
    let staged = git(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            "--cached",
            "--",
        ],
    )?;
    let unstaged = git(root, &["diff", "--name-only", "--no-renames", "-z", "--"])?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let dirty = !(staged.is_empty() && unstaged.is_empty() && untracked.is_empty());
    let tree = if dirty {
        // A commit of the working tree's tracked changes, stored nowhere; untracked files are not in it.
        let stash = git(root, &["stash", "create"])?.trim().to_owned();
        if stash.is_empty() {
            git(root, &["rev-parse", "HEAD^{tree}"])?.trim().to_owned()
        } else {
            stash
        }
    } else {
        git(root, &["rev-parse", "HEAD^{tree}"])?.trim().to_owned()
    };
    let changed_files: BTreeSet<String> = [&committed, &staged, &unstaged, &untracked]
        .into_iter()
        .flat_map(|text| null_separated(text))
        .collect();
    Ok(GitState {
        base_ref: base.to_owned(),
        merge_base,
        head,
        tree,
        dirty,
        changed_files: changed_files.into_iter().collect(),
    })
}

/// Reads the workspace members and the dependencies between them.
pub fn workspace(root: &Path) -> Result<Workspace, String> {
    let text = output(
        root,
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps", "--locked"],
    )?;
    let metadata: Value =
        serde_json::from_str(&text).map_err(|error| format!("cargo metadata: {error}"))?;
    let invalid = || {
        "cargo metadata: missing workspace_root, target_directory, packages or workspace_members"
            .to_owned()
    };
    let workspace_root = PathBuf::from(metadata["workspace_root"].as_str().ok_or_else(invalid)?);
    let target_directory =
        PathBuf::from(metadata["target_directory"].as_str().ok_or_else(invalid)?);
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let packages: Vec<&Value> = metadata["packages"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter(|package| {
            package["id"]
                .as_str()
                .is_some_and(|id| members.contains(id))
        })
        .collect();
    // Members by manifest directory, so a path dependency resolves to the member it points at.
    let mut by_directory = BTreeMap::new();
    for package in &packages {
        let name = package["name"].as_str().ok_or_else(invalid)?;
        let manifest = Path::new(package["manifest_path"].as_str().ok_or_else(invalid)?);
        let directory = manifest.parent().ok_or_else(invalid)?.to_path_buf();
        by_directory.insert(directory, name.to_owned());
    }
    let names: BTreeSet<&String> = by_directory.values().collect();
    let mut result = BTreeMap::new();
    for package in &packages {
        let name = package["name"].as_str().ok_or_else(invalid)?.to_owned();
        let manifest = Path::new(package["manifest_path"].as_str().ok_or_else(invalid)?);
        let directory = manifest.parent().ok_or_else(invalid)?;
        let relative = directory
            .strip_prefix(&workspace_root)
            .map_err(|_| format!("cargo metadata: {name} lies outside the workspace root"))?;
        let mut dependencies = BTreeSet::new();
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            let by_path = dependency["path"]
                .as_str()
                .and_then(|path| by_directory.get(Path::new(path)));
            let by_name = dependency["name"]
                .as_str()
                .map(str::to_owned)
                .filter(|name| names.contains(name));
            if let Some(member) = by_path.cloned().or(by_name) {
                if member != name {
                    dependencies.insert(member);
                }
            }
        }
        let library = package["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|target| {
                target["kind"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|kind| matches!(kind.as_str(), Some("lib" | "rlib" | "proc-macro")))
            });
        let directory = relative
            .to_str()
            .ok_or_else(|| format!("cargo metadata: {name}'s path is not UTF-8"))?;
        result.insert(
            name.clone(),
            Package {
                name,
                directory: directory.to_owned(),
                dependencies,
                library,
            },
        );
    }
    Ok(Workspace {
        target_directory,
        packages: result,
    })
}

/// The package whose directory holds `path`, the deepest one when packages nest.
fn owner<'a>(workspace: &'a Workspace, path: &str) -> Option<&'a Package> {
    workspace
        .packages
        .values()
        .filter(|package| {
            package.directory.is_empty()
                || path
                    .strip_prefix(package.directory.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|package| package.directory.len())
}

/// Every package that depends on one in `seeds`, directly or not, and the seeds themselves.
fn with_dependents(workspace: &Workspace, seeds: &BTreeSet<String>) -> BTreeSet<String> {
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for package in workspace.packages.values() {
        for dependency in &package.dependencies {
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(&package.name);
        }
    }
    let mut selected = seeds.clone();
    let mut pending: Vec<String> = seeds.iter().cloned().collect();
    while let Some(name) = pending.pop() {
        for dependent in dependents.get(name.as_str()).into_iter().flatten() {
            if selected.insert((*dependent).to_owned()) {
                pending.push((*dependent).to_owned());
            }
        }
    }
    selected
}

/// Maps the changed files to packages and decides the scope.
pub fn select(
    plan: &mut Plan,
    workspace: &Workspace,
    config: &Config,
    scope: Scope,
) -> Result<(), String> {
    let mut changed = BTreeSet::new();
    for path in &plan.git.changed_files {
        let build_script = path.rsplit('/').next() == Some("build.rs");
        if !build_script {
            if let Some(package) = owner(workspace, path) {
                changed.insert(package.name.clone());
                continue;
            }
        }
        match config.rules.iter().find(|rule| {
            rule.patterns
                .iter()
                .any(|pattern| glob::matches(pattern, path))
        }) {
            Some(rule) => match rule.action {
                Action::All => {
                    plan.whole_workspace = true;
                    plan.notes.push(format!(
                        "{path}: rule `{}` checks the whole workspace",
                        rule.name
                    ));
                }
                Action::None => {
                    plan.notes.push(format!(
                        "{path}: rule `{}` selects no Rust package",
                        rule.name
                    ));
                }
                Action::Packages => {
                    for name in &rule.packages {
                        if !workspace.packages.contains_key(name) {
                            return Err(format!(
                                "rule `{}` names `{name}`, which is not a workspace member",
                                rule.name
                            ));
                        }
                        changed.insert(name.clone());
                    }
                    plan.notes.push(format!(
                        "{path}: rule `{}` selects {}",
                        rule.name,
                        rule.packages.join(", ")
                    ));
                }
            },
            None => {
                plan.whole_workspace = true;
                plan.fallback = true;
                plan.fallback_reasons.push(format!(
                    "{path}: in no package and matched by no rule; the whole workspace is checked"
                ));
            }
        }
    }
    let all: BTreeSet<String> = workspace.packages.keys().cloned().collect();
    let affected = if plan.whole_workspace {
        all.clone()
    } else {
        with_dependents(workspace, &changed)
    };
    let selected = if plan.whole_workspace {
        all.clone()
    } else {
        match scope {
            Scope::Changed => changed.clone(),
            Scope::Affected => affected.clone(),
            Scope::All => all.clone(),
        }
    };
    plan.changed_packages = changed.into_iter().collect();
    plan.affected_packages = affected.into_iter().collect();
    plan.selected = selected.into_iter().collect();
    plan.total_count = workspace.packages.len();
    plan.scope_empty = plan.selected.is_empty();
    Ok(())
}

fn cargo(args: &[&str]) -> Vec<String> {
    std::iter::once("cargo")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn with_packages(mut command: Vec<String>, packages: &[String]) -> Vec<String> {
    for package in packages {
        command.push("-p".to_owned());
        command.push(package.clone());
    }
    command
}

/// The checks of `profile` over the selection.
pub fn checks(
    plan: &mut Plan,
    workspace: &Workspace,
    config: &Config,
    profile: &Profile,
    in_ci: bool,
) {
    let all: Vec<String> = workspace.packages.keys().cloned().collect();
    let rust_changed = plan
        .git
        .changed_files
        .iter()
        .any(|path| path.ends_with(".rs"));
    let mut list = Vec::new();
    let skip = |name: String, command: Vec<String>, kind: Kind, reason: &str| Check {
        name,
        command,
        kind,
        skipped: Some(reason.to_owned()),
    };
    let run = |name: String, command: Vec<String>, kind: Kind| Check {
        name,
        command,
        kind,
        skipped: None,
    };

    if profile.gate {
        list.push(run(
            "gate".to_owned(),
            vec!["xtask".to_owned(), "gate".to_owned()],
            Kind::Gate,
        ));
    }

    let vendored_changed = plan
        .git
        .changed_files
        .iter()
        .any(|path| crate::vendored::touches(path));
    let command = vec!["xtask".to_owned(), "vendored".to_owned()];
    if profile.gate || vendored_changed {
        list.push(run("vendored".to_owned(), command, Kind::Vendored));
    } else {
        list.push(skip(
            "vendored".to_owned(),
            command,
            Kind::Vendored,
            "no vendored tree changed",
        ));
    }

    match profile.fmt {
        Fmt::Off => {}
        Fmt::Changed => {
            let packages = if plan.whole_workspace {
                &all
            } else {
                &plan.changed_packages
            };
            let command = if plan.whole_workspace {
                cargo(&["fmt", "--all", "--", "--check"])
            } else {
                let mut command = with_packages(cargo(&["fmt"]), packages);
                command.extend(["--".to_owned(), "--check".to_owned()]);
                command
            };
            if packages.is_empty() {
                list.push(skip(
                    "fmt".to_owned(),
                    command,
                    Kind::Cargo,
                    "no package changed",
                ));
            } else {
                list.push(run("fmt".to_owned(), command, Kind::Cargo));
            }
        }
        Fmt::All => {
            let command = cargo(&["fmt", "--all", "--", "--check"]);
            if plan.scope_empty && !rust_changed {
                list.push(skip(
                    "fmt".to_owned(),
                    command,
                    Kind::Cargo,
                    "no Rust file changed",
                ));
            } else {
                list.push(run("fmt".to_owned(), command, Kind::Cargo));
            }
        }
    }

    let coverage = |coverage: Coverage, packages: &[String]| -> Option<Vec<String>> {
        match coverage {
            Coverage::Off => None,
            Coverage::Scope => Some(packages.to_vec()),
            Coverage::Workspace => Some(all.clone()),
        }
    };
    let libraries = |packages: Vec<String>| -> Vec<String> {
        packages
            .into_iter()
            .filter(|name| workspace.packages[name].library)
            .collect()
    };
    type Build = (&'static str, Coverage, Vec<&'static str>);
    let nextest_profile: Vec<&str> = if in_ci {
        vec!["--profile", "ci"]
    } else {
        Vec::new()
    };
    let builds: [Build; 4] = [
        (
            "check",
            profile.clippy,
            vec!["check", "--locked", "--all-targets"],
        ),
        (
            "clippy",
            profile.clippy,
            vec!["clippy", "--locked", "--all-targets"],
        ),
        (
            "nextest",
            profile.nextest,
            [
                vec!["nextest", "run", "--locked", "--no-tests=pass"],
                nextest_profile,
            ]
            .concat(),
        ),
        (
            "doctests",
            profile.doctests,
            vec!["test", "--locked", "--doc"],
        ),
    ];
    for (check, mode, base) in builds {
        let Some(mut packages) = coverage(mode, &plan.selected) else {
            continue;
        };
        if check == "doctests" {
            packages = libraries(packages);
        }
        for feature in &profile.features {
            let configuration = &config.features[feature];
            // A check-only configuration gets the check and nothing else; the others get everything else.
            if configuration.check_only != (check == "check") {
                continue;
            }
            let mut command = cargo(&base);
            command.extend(configuration.args.iter().cloned());
            if packages.is_empty() {
                let reason = if plan.scope_empty {
                    "scope empty"
                } else {
                    "no package with a library in scope"
                };
                list.push(skip(
                    format!("{check} [{feature}]"),
                    command,
                    Kind::Cargo,
                    reason,
                ));
            } else if configuration.per_package {
                for package in &packages {
                    let command = with_packages(command.clone(), std::slice::from_ref(package));
                    list.push(run(
                        format!("{check} [{feature}] {package}"),
                        command,
                        Kind::Cargo,
                    ));
                }
            } else {
                list.push(run(
                    format!("{check} [{feature}]"),
                    with_packages(command, &packages),
                    Kind::Cargo,
                ));
            }
        }
    }

    if profile.layers {
        let layers = cargo(&["run", "--locked", "-q", "-p", "cupertino-layers"]);
        if plan.scope_empty {
            list.push(skip(
                "layers".to_owned(),
                layers,
                Kind::Cargo,
                "scope empty",
            ));
        } else {
            list.push(run("layers".to_owned(), layers, Kind::Cargo));
        }
    }
    plan.feature_configs = profile.features.clone();
    plan.checks = list;
}
