//! `cupertino dev …`: commands for people working on the desktop's sources. Every one is a preview.

use crate::{
    manual, Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Sink, Stability,
    ValueHint,
};
use cupertino_layers::{Findings, LayersError, Policy, Problem, POLICY};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const GROUP: Group = Group {
    name: "dev",
    summary: "Commands for working on the desktop's sources: the layer check and the manual pages",
    description: "Commands for people who build the desktop from a checkout of its sources. They are previews: their \
                  options and output may change in any release.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[
    Command {
        path: &["dev", "layers"],
        summary: "Checks that the crates of a checkout depend on each other as the layer rules allow, and that the \
                  licences of what the MPL-2.0 crates link are accepted",
        description: "Reads `cargo metadata` of the checkout and checks every dependency between its crates against \
                      the layer rules, the dependency paths and crates that are denied, the workspace licences and \
                      the licence closure of the MPL-2.0 crates, as the `cupertino-layers` program does. Cargo is \
                      `$CARGO`, else `cargo` on `PATH`; it runs offline with the lock file as it is.\n\n\
                      The checkout is the one given with --root, else the nearest directory above the working \
                      directory whose Cargo.toml has a [workspace] and which holds crates/cupertino-layers. Outside a \
                      checkout the command fails with exit code 3 (no_checkout), and without Cargo with exit code 3 \
                      (no_cargo). Problems fail it with exit code 5 (layer_violations); the JSON forms then list them \
                      in data.problems, each with kind, from, to and detail. A clean checkout gives the number of \
                      crates and dependencies and the crates of each layer, and --graph the DOT graph of the layers \
                      (data.dot in the JSON forms).",
        examples: &[
            ("cupertino dev layers", "Check the checkout the working directory is in."),
            (
                "cupertino dev layers --graph > layers.dot",
                "Write the DOT graph of the layers.",
            ),
            (
                "cupertino --format json dev layers --root ~/src/cupertino",
                "Check another checkout and report as one JSON envelope.",
            ),
        ],
        stability: Stability::Preview,
        args: &[
            Arg::option("root", "the checkout to check, instead of the one the working directory is in")
                .hint(ValueHint::Dir),
            Arg::flag("graph", "print the DOT graph of the layers instead of the summary"),
        ],
        run: Run::Stream(layers),
    },
    Command {
        path: &["dev", "manual"],
        summary: "Writes the manual pages of the command, cupertino.1 and one cupertino-<group>.1 per group, into \
                  <dir>",
        description: "Writes the manual pages generated from the command's registry into <dir>, which is created \
                      when it does not exist: cupertino.1 for the command as a whole (its global options, output \
                      forms, exit status, environment and files) and cupertino-<group>.1 for each command group, with \
                      the synopsis, description, options and examples of each command.\n\n\
                      The pages' date is SOURCE_DATE_EPOCH when it is set, else a fixed date, never the current \
                      time, so two runs write the same bytes. The packages write the pages this way at build time.",
        examples: &[(
            "cupertino dev manual ./man",
            "Write the pages into ./man; `man -l ./man/cupertino.1` reads one.",
        )],
        stability: Stability::Preview,
        args: &[Arg::positional("dir", "the directory to write the pages into").hint(ValueHint::Dir)],
        run: Run::Once(write_manual),
    },
];

#[derive(Serialize)]
struct LayersData<'a> {
    root: &'a Path,
    crates: usize,
    edges: usize,
    exceptions: usize,
    by_layer: BTreeMap<&'static str, Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dot: Option<String>,
    problems: Vec<ProblemData<'a>>,
}

#[derive(Serialize)]
struct ProblemData<'a> {
    kind: &'static str,
    from: &'a str,
    to: Option<&'a str>,
    detail: String,
}

fn layers(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.json_result_only();
    let root = checkout(args.value("root"), cx)?;
    let cargo = cargo(cx)?;
    let policy = cx.provided::<Policy>().copied().unwrap_or(POLICY);
    let findings = cupertino_layers::check_workspace_with(&root, &|| cx.command(&cargo), &policy)
        .map_err(|e| match e {
        LayersError::Cargo(e) => Failure::new(
            FailureKind::Unavailable,
            "no_cargo",
            format!("cannot run {}: {e}", cargo.display()),
        ),
        LayersError::Metadata(_) | LayersError::Manifest(_) => {
            Failure::new(FailureKind::Validation, "metadata_failed", e.to_string())
        }
    })?;
    let graph = args.flag("graph");
    // printing the graph is how its committed snapshot is made again, so an out-of-date one does not fail that run
    let problems: Vec<&Problem> = if graph {
        findings.problems_printing_the_graph().collect()
    } else {
        findings.all_problems().collect()
    };
    let data = layers_data(&root, &findings, graph, &problems);
    for p in &problems {
        sink.event("problem", &problem_data(p), &p.to_string())?;
    }
    if !problems.is_empty() {
        let count = problems.len();
        return Err(Failure::new(
            FailureKind::Validation,
            "layer_violations",
            format!(
                "{count} layer and licence problem{} in {}",
                if count == 1 { "" } else { "s" },
                root.display()
            ),
        )
        .with_data(&data));
    }
    let human = match &data.dot {
        Some(dot) => dot.clone(),
        None => format!("{}\n", findings.summary()),
    };
    Ok(Reply::new(&data, human))
}

fn layers_data<'a>(
    root: &'a Path,
    findings: &'a Findings,
    graph: bool,
    problems: &[&'a Problem],
) -> LayersData<'a> {
    LayersData {
        root,
        crates: findings.layers.len(),
        edges: findings.edges.len(),
        exceptions: findings.policy.exceptions.len(),
        by_layer: findings
            .by_layer()
            .into_iter()
            .map(|(layer, crates)| (layer.name(), crates))
            .collect(),
        dot: graph.then(|| findings.dot()),
        problems: problems.iter().copied().map(problem_data).collect(),
    }
}

fn problem_data(p: &Problem) -> ProblemData<'_> {
    ProblemData {
        kind: p.kind(),
        from: p.from(),
        to: p.to(),
        detail: p.to_string(),
    }
}

/// The checkout to check: `--root`, else the nearest directory above the working directory that is one.
fn checkout(given: Option<&str>, cx: &Context) -> Result<PathBuf, Failure> {
    let here = cx.current_dir().map_err(|e| {
        Failure::new(
            FailureKind::Unavailable,
            "no_checkout",
            format!("cannot read the working directory: {e}"),
        )
    })?;
    if let Some(given) = given {
        let root = here.join(given);
        return if is_checkout(&root) {
            Ok(root)
        } else {
            Err(Failure::new(
                FailureKind::Unavailable,
                "no_checkout",
                format!(
                    "{} is not a checkout of the desktop's sources (a Cargo.toml with a [workspace] and \
                     crates/cupertino-layers)",
                    root.display()
                ),
            ))
        };
    }
    here.ancestors()
        .find(|dir| is_checkout(dir))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Failure::new(
                FailureKind::Unavailable,
                "no_checkout",
                format!(
                    "{} is not inside a checkout of the desktop's sources; give one with --root",
                    here.display()
                ),
            )
        })
}

fn is_checkout(dir: &Path) -> bool {
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    manifest.lines().any(|l| l.trim() == "[workspace]")
        && dir.join("crates/cupertino-layers").is_dir()
}

/// `$CARGO`, else `cargo` in a directory of `PATH`: this developer command alone looks programs up on `PATH`.
fn cargo(cx: &Context) -> Result<PathBuf, Failure> {
    if let Some(cargo) = cx.var("CARGO").filter(|c| !c.is_empty()) {
        return Ok(PathBuf::from(cargo));
    }
    let path = cx.var("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("cargo"))
        .find(|candidate| is_executable(candidate))
        .ok_or_else(|| {
            Failure::new(
                FailureKind::Unavailable,
                "no_cargo",
                "cargo is not installed: neither $CARGO nor a cargo on PATH",
            )
        })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[derive(Serialize)]
struct Written {
    dir: PathBuf,
    pages: Vec<String>,
}

fn write_manual(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let dir = cx
        .current_dir()
        .map(|here| here.join(args.value("dir").expect("<dir> is a positional argument")))
        .map_err(|e| {
            Failure::new(
                FailureKind::Temporary,
                "write_failed",
                format!("cannot read the working directory: {e}"),
            )
        })?;
    let date = manual::page_date(cx.var("SOURCE_DATE_EPOCH").as_deref());
    let pages = manual::pages(cx.registry(), &date);
    let failed = |e: std::io::Error, path: &Path| {
        Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot write {}: {e}", path.display()),
        )
    };
    std::fs::create_dir_all(&dir).map_err(|e| failed(e, &dir))?;
    let mut human = String::new();
    for (name, text) in &pages {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| failed(e, &path))?;
        human.push_str(&format!("{}\n", path.display()));
    }
    let data = Written {
        dir,
        pages: pages.into_iter().map(|(name, _)| name).collect(),
    };
    Ok(Reply::new(&data, human))
}
