//! Running a plan: the machine-wide build slots, the checks, and the report.

use std::fs::File;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::Config;
use crate::plan::{self, Check, Kind, Plan};
use crate::{gate, remaining, vendored, Options};

/// What a run did with each check.
struct Outcome {
    status: &'static str,
    duration: Duration,
}

/// The report file, below the target directory.
/// Where a run's report goes: a dry run's plan never replaces the last run's results.
fn report_path(target: &Path, dry_run: bool) -> PathBuf {
    let name = if dry_run {
        "ci-plan.json"
    } else {
        "ci-latest.json"
    };
    target.join("xtask").join(name)
}

/// The target directory before `cargo metadata` names it: `CARGO_TARGET_DIR`, else `target` under `root`.
fn early_target(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| root.join("target"), PathBuf::from)
}

fn first_line(root: &Path, program: &str, args: &[&str]) -> Option<String> {
    plan::output(root, program, args)
        .ok()
        .and_then(|text| text.lines().next().map(str::to_owned))
}

fn tools(root: &Path) -> Value {
    json!({
        "rustc": first_line(root, "rustc", &["--version"]),
        "cargo": first_line(root, "cargo", &["--version"]),
        "nextest": first_line(root, "cargo", &["nextest", "--version"]),
        "sccache": first_line(root, "sccache", &["--version"]),
    })
}

fn target_triple(root: &Path) -> Option<String> {
    let text = plan::output(root, "rustc", &["-vV"]).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
}

/// sccache's hits and misses so far, summed over languages.
fn sccache_counts(root: &Path) -> Option<(u64, u64)> {
    let text = plan::output(root, "sccache", &["--show-stats", "--stats-format=json"]).ok()?;
    let stats: Value = serde_json::from_str(&text).ok()?;
    let sum = |key: &str| -> Option<u64> {
        Some(
            stats["stats"][key]["counts"]
                .as_object()?
                .values()
                .filter_map(Value::as_u64)
                .sum(),
        )
    };
    Some((sum("cache_hits")?, sum("cache_misses")?))
}

/// The number of build slots: XTASK_SLOTS, else the configuration's.
fn slots(config: Option<&Config>) -> Result<usize, String> {
    match std::env::var("XTASK_SLOTS") {
        Ok(text) => match text.parse::<usize>() {
            Ok(slots) if slots > 0 => Ok(slots),
            _ => Err(format!(
                "XTASK_SLOTS must be a positive number, not `{text}`"
            )),
        },
        Err(_) => Ok(config.map_or(2, |config| config.limits.slots)),
    }
}

/// Takes one of the machine's build slots and holds it until the returned file is dropped.
fn take_slot(slots: usize) -> Result<(File, Duration), String> {
    let base =
        std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let directory = base.join("cupertino-xtask");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let open = |index: usize| {
        let path = directory.join(format!("slot-{index}.lock"));
        File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let start = Instant::now();
    for index in 0..slots {
        let file = open(index)?;
        if file.try_lock().is_ok() {
            return Ok((file, start.elapsed()));
        }
    }
    let index = std::process::id() as usize % slots;
    eprintln!("xtask: all {slots} build slots are in use by other runs; waiting for slot {index}");
    let file = open(index)?;
    file.lock()
        .map_err(|error| format!("build slot {index}: {error}"))?;
    Ok((file, start.elapsed()))
}

fn run_cargo(
    root: &Path,
    check: &Check,
    jobs: Option<usize>,
    in_ci: bool,
    json: bool,
) -> Result<bool, String> {
    let (program, args) = check
        .command
        .split_first()
        .expect("a command has a program");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(root)
        .env("CARGO_BUILD_WARNINGS", "deny");
    if let Some(jobs) = jobs {
        command.env("CARGO_BUILD_JOBS", jobs.to_string());
    }
    if in_ci {
        command.env("CARGO_INCREMENTAL", "0");
    }
    if json {
        // The report is the only thing on standard output.
        let stderr = std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| error.to_string())?;
        command.stdout(Stdio::from(stderr));
    }
    let status = command
        .status()
        .map_err(|error| format!("{}: {error}", check.command.join(" ")))?;
    Ok(status.success())
}

fn run_gate(root: &Path) -> bool {
    match gate::run(root) {
        Ok(outcome) => {
            for problem in &outcome.problems {
                eprintln!("{problem}");
            }
            eprintln!("{}", gate::summary(&outcome));
            outcome.problems.is_empty()
        }
        Err(error) => {
            eprintln!("gate: {error}");
            false
        }
    }
}

/// Runs the vendored-tree check and prints its problems and summary; whether it passed.
pub fn run_vendored(root: &Path) -> bool {
    match vendored::problems(root) {
        Ok(problems) => {
            for problem in &problems {
                eprintln!("{problem}");
            }
            eprintln!("{}", vendored::summary(&problems));
            problems.is_empty()
        }
        Err(error) => {
            eprintln!("vendored patches: {error}");
            false
        }
    }
}

/// Runs the check that the remaining-work list only shrinks since `base` and prints its problems and summary; whether
/// it passed, always while `remaining::GROWTH_FAILS` is false.
pub fn run_remaining(root: &Path, base: &str) -> bool {
    match remaining::problems(root, base) {
        Ok(problems) => {
            for problem in &problems {
                eprintln!("{problem}");
            }
            eprintln!("{}", remaining::summary(&problems));
            problems.is_empty() || !remaining::GROWTH_FAILS
        }
        Err(error) => {
            eprintln!("remaining-work list: {error}");
            false
        }
    }
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn print_plan(plan: &Plan, report: &Value) {
    let git = &plan.git;
    println!(
        "xtask ci: profile {}, scope {}, base {} (merge base {}), HEAD {}{}",
        report["profile"].as_str().unwrap_or_default(),
        report["scope"].as_str().unwrap_or_default(),
        git.base_ref,
        short(&git.merge_base),
        short(&git.head),
        if git.dirty { ", dirty" } else { "" }
    );
    println!(
        "changed files: {}; changed packages: {}; affected: {}; selected {} of {}{}",
        git.changed_files.len(),
        if plan.changed_packages.is_empty() {
            "none".to_owned()
        } else {
            plan.changed_packages.join(", ")
        },
        plan.affected_packages.len(),
        plan.selected.len(),
        plan.total_count,
        if plan.scope_empty {
            " (scope empty: no Rust package to check)"
        } else {
            ""
        }
    );
    for note in &plan.notes {
        println!("  {note}");
    }
    for reason in &plan.fallback_reasons {
        println!("  fallback: {reason}");
    }
    println!("plan (features: {}):", plan.feature_configs.join(", "));
    for check in &plan.checks {
        match &check.skipped {
            Some(reason) => println!("  skip  {:<24} ({reason})", check.name),
            None => println!("  run   {:<24} {}", check.name, check.command.join(" ")),
        }
    }
}

/// Writes the report and prints it in the chosen format.
fn finish(target: &Path, report: &Value, plan: Option<&Plan>, options: &Options, code: u8) -> u8 {
    let path = report_path(target, options.dry_run);
    let text = serde_json::to_string_pretty(report).expect("the report serializes");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, format!("{text}\n")));
    if let Err(error) = written {
        eprintln!("xtask: {}: {error}", path.display());
    }
    if options.json {
        println!("{text}");
    } else if let Some(error) = report["error"].as_str() {
        eprintln!("xtask ci: planning failed: {error}");
    } else if let Some(plan) = plan {
        print_plan(plan, report);
        if !options.dry_run {
            println!("results:");
            for check in report["checks"].as_array().into_iter().flatten() {
                println!(
                    "  {:<7} {:<24} {:>8} ms",
                    check["status"].as_str().unwrap_or_default(),
                    check["name"].as_str().unwrap_or_default(),
                    check["duration_ms"].as_u64().unwrap_or_default()
                );
            }
            println!(
                "xtask ci: {} in {} ms; report: {}",
                report["status"].as_str().unwrap_or_default(),
                report["duration_ms"].as_u64().unwrap_or_default(),
                path.display()
            );
        }
    }
    code
}

fn planning_failure(
    target: &Path,
    options: &Options,
    scope: Option<&str>,
    error: String,
    start: Instant,
) -> u8 {
    let report = json!({
        "status": "planning failed",
        "error": error,
        "scope": scope,
        "profile": options.profile,
        "base_ref": options.base,
        "dry_run": options.dry_run,
        "duration_ms": milliseconds(start.elapsed()),
    });
    finish(target, &report, None, options, 2)
}

/// `cargo xtask ci`: returns the exit status.
pub fn ci(options: &Options) -> u8 {
    let start = Instant::now();
    let in_ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
    let fallback_target = early_target(Path::new("."));
    let root = match plan::repository_root() {
        Ok(root) => root,
        Err(error) => return planning_failure(&fallback_target, options, None, error, start),
    };
    let fallback_target = early_target(&root);
    let config = match Config::load(&root) {
        Ok(config) => config,
        Err(error) => return planning_failure(&fallback_target, options, None, error, start),
    };
    let Some(profile) = config.profiles.get(&options.profile) else {
        let error = format!("{}: no profile `{}`", crate::config::PATH, options.profile);
        return planning_failure(&fallback_target, options, None, error, start);
    };
    let scope = options.scope.unwrap_or(profile.scope);
    let mut plan = Plan::default();
    plan.git = match plan::git_state(&root, &options.base) {
        Ok(git) => git,
        Err(error) => {
            return planning_failure(&fallback_target, options, Some(scope.name()), error, start)
        }
    };
    let workspace = match plan::workspace(&root) {
        Ok(workspace) => workspace,
        Err(error) => {
            return planning_failure(&fallback_target, options, Some(scope.name()), error, start)
        }
    };
    if let Err(error) = plan::select(&mut plan, &workspace, &config, scope) {
        return planning_failure(
            &workspace.target_directory,
            options,
            Some(scope.name()),
            error,
            start,
        );
    }
    plan::checks(&mut plan, &workspace, &config, profile, in_ci);
    let slots = match slots(Some(&config)) {
        Ok(slots) => slots,
        Err(error) => {
            return planning_failure(
                &workspace.target_directory,
                options,
                Some(scope.name()),
                error,
                start,
            )
        }
    };

    let mut outcomes: Vec<Option<Outcome>> = plan.checks.iter().map(|_| None).collect();
    let mut lock_wait = Duration::ZERO;
    let mut sccache = Value::Null;
    let mut code = 0;
    if !options.dry_run {
        let builds = plan
            .checks
            .iter()
            .any(|check| check.skipped.is_none() && check.kind == Kind::Cargo);
        let mut jobs = None;
        let mut _slot = None;
        if builds {
            match take_slot(slots) {
                Ok((file, waited)) => {
                    _slot = Some(file);
                    lock_wait = waited;
                }
                Err(error) => {
                    return planning_failure(
                        &workspace.target_directory,
                        options,
                        Some(scope.name()),
                        error,
                        start,
                    )
                }
            }
            if std::env::var_os("CARGO_BUILD_JOBS").is_none() {
                let cores = std::thread::available_parallelism().map_or(1, usize::from);
                jobs = Some((cores / slots).max(1));
            }
        }
        let before = sccache_counts(&root);
        for (check, outcome) in plan.checks.iter().zip(outcomes.iter_mut()) {
            if check.skipped.is_some() {
                continue;
            }
            eprintln!("xtask ci: {}: {}", check.name, check.command.join(" "));
            let started = Instant::now();
            let passed = match check.kind {
                Kind::Gate => run_gate(&root),
                Kind::Vendored => run_vendored(&root),
                Kind::Remaining => run_remaining(&root, &plan.git.merge_base),
                Kind::Cargo => {
                    run_cargo(&root, check, jobs, in_ci, options.json).unwrap_or_else(|error| {
                        eprintln!("xtask ci: {error}");
                        false
                    })
                }
            };
            if !passed {
                code = 1;
            }
            *outcome = Some(Outcome {
                status: if passed { "passed" } else { "failed" },
                duration: started.elapsed(),
            });
        }
        if let (Some(before), Some(after)) = (before, sccache_counts(&root)) {
            sccache = json!({
                "hits": after.0.saturating_sub(before.0),
                "misses": after.1.saturating_sub(before.1),
            });
        }
    }

    let checks: Vec<Value> = plan
        .checks
        .iter()
        .zip(&outcomes)
        .map(|(check, outcome)| {
            let status = match (outcome, &check.skipped, options.dry_run) {
                (Some(outcome), _, _) => outcome.status,
                (None, Some(_), _) => "skipped",
                (None, None, true) => "planned",
                (None, None, false) => "skipped",
            };
            json!({
                "name": check.name,
                "status": status,
                "command": check.command.join(" "),
                "skip_reason": check.skipped,
                "duration_ms": outcome.as_ref().map_or(0, |outcome| milliseconds(outcome.duration)),
            })
        })
        .collect();
    let git = &plan.git;
    // An empty scope is reported as such, never as a pass of checks that ran.
    let status = if options.dry_run {
        "planned"
    } else if code != 0 {
        "failed"
    } else if checks.iter().all(|check| check["status"] == "skipped") {
        "nothing to check"
    } else {
        "passed"
    };
    let report = json!({
        "status": status,
        "scope": scope.name(),
        "profile": options.profile,
        "dry_run": options.dry_run,
        "base_ref": git.base_ref,
        "merge_base": git.merge_base,
        "head": git.head,
        "tree": git.tree,
        "dirty": git.dirty,
        "changed_files": git.changed_files,
        "changed_packages": plan.changed_packages,
        "affected_packages": plan.affected_packages,
        "selected_packages": plan.selected,
        "selected_count": plan.selected.len(),
        "total_count": plan.total_count,
        "whole_workspace": plan.whole_workspace,
        "fallback": plan.fallback,
        "fallback_reasons": plan.fallback_reasons,
        "notes": plan.notes,
        "scope_empty": plan.scope_empty,
        "feature_configs": plan.feature_configs,
        "slots": slots,
        "checks": checks,
        "lock_wait_ms": milliseconds(lock_wait),
        "duration_ms": milliseconds(start.elapsed()),
        "tools": tools(&root),
        "target": target_triple(&root),
        "sccache": sccache,
    });
    finish(
        &workspace.target_directory,
        &report,
        Some(&plan),
        options,
        code,
    )
}
