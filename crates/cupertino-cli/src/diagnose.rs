//! `cupertino diagnose`: the checks of `cupertino-diagnostics` on this machine and session.
//!
//! The group's command lists are joined in [`GROUP`]; another module adds its commands under `diagnose` by adding its
//! list there.

use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Format, Group, Reply, Run, Sink, Stability,
};
use cupertino_diagnostics::{Area, Report, Sources, Status};
use serde::Serialize;

pub(crate) const GROUP: Group = Group {
    name: "diagnose",
    summary: "Checks this machine and session for what the desktop needs, and reads what its processes recorded",
    description: "Commands that look at the machine and the session the desktop runs in and report what they find. \
                  The checks read files and ask the service manager for unit states; they change nothing and open \
                  no device.\n\n\
                  The event commands read what the desktop's processes recorded when something failed. Every \
                  process records diagnostic events (schema cupertino.diag.v1): the user's own processes in a \
                  bounded store in ~/.local/state/cupertino/events, the system services in the system journal. The \
                  starts and ends of sessions come from session.log. Nothing is uploaded; the events stay on this \
                  machine until `cupertino diagnose events clear` deletes them or they expire after 14 days.",
    commands: &[COMMANDS, crate::diagnose_events::COMMANDS],
};

/// The area names, in the order the checks run.
const AREAS: &[&str] = &{
    let mut names = [""; Area::ALL.len()];
    let mut i = 0;
    while i < Area::ALL.len() {
        names[i] = area_name(Area::ALL[i]);
        i += 1;
    }
    names
};

/// [`Area::name`], usable in a constant.
const fn area_name(area: Area) -> &'static str {
    match area {
        Area::Gpu => "gpu",
        Area::Session => "session",
        Area::Portals => "portals",
        Area::Services => "services",
        Area::Crash => "crash",
        Area::Logs => "logs",
        Area::Events => "events",
    }
}

pub(crate) const COMMANDS: &[Command] = &[Command {
    path: &["diagnose"],
    summary: "Checks the GPU and its driver, the session, the portals, the services, the last crash, the logs and \
              the errors recorded in the last day, and reports each check's status and what to do about it",
    description: "Runs the checks of the areas given with --area, or of every area, in the order gpu, session, \
                  portals, services, crash, logs, events. Each check has a stable dotted id (gpu.driver), a status \
                  (skip, ok, warn or fail), a summary, the facts it found and, for warn and fail, a fix. Checks that need a \
                  session are skipped outside one.\n\n\
                  The command exits 0 once the checks ran, whatever they found; with --strict a check that fails \
                  makes it exit 5 (checks_failed). In the JSON envelope data is the report itself (schema 1, the \
                  same object `cupertino-diagnose --json` prints). In JSON lines each check is a `check` event, in \
                  order, and the result counts the checks of each status.\n\n\
                  It replaces `cupertino-diagnose`, which still works and prints a notice.",
    examples: &[
        ("cupertino diagnose", "Run every check and print the report."),
        (
            "cupertino diagnose --area gpu --area crash",
            "Check the GPU and how the last sessions ended.",
        ),
        (
            "cupertino --format json diagnose --strict",
            "Print the report as one JSON envelope and exit 5 when a check fails.",
        ),
        (
            "cupertino --format jsonl diagnose",
            "Print each check as a JSON line as the report is made.",
        ),
    ],
    stability: Stability::Stable,
    args: &[
        Arg::option("area", "an area to check; every area without one").values(AREAS).repeat(),
        Arg::flag("strict", "exit 5 when a check fails"),
    ],
    run: Run::Stream(diagnose),
}];

/// How many checks came out each way: the result of the JSON lines form.
#[derive(Debug, Default, Serialize)]
struct Counts {
    skip: usize,
    ok: usize,
    warn: usize,
    fail: usize,
}

fn diagnose(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.json_result_only();
    let wanted: Vec<Area> = args
        .values("area")
        .iter()
        .map(|name| Area::from_name(name).expect("--area takes the area names"))
        .collect();
    let system;
    let sources = match cx.provided::<Sources>() {
        Some(sources) => sources,
        None => {
            system = sources_of(cx);
            &system
        }
    };
    // area by area, so a stop is seen between them
    let mut report = Report {
        schema: cupertino_diagnostics::SCHEMA,
        checks: Vec::new(),
    };
    for area in Area::ALL
        .into_iter()
        .filter(|a| wanted.is_empty() || wanted.contains(a))
    {
        if sink.stop_requested() {
            return Err(Failure::interrupted());
        }
        report
            .checks
            .extend(cupertino_diagnostics::run(sources, &[area]).checks);
    }
    // the report's text, cut into the lines of each check, so the human form prints as `Display` does
    let text = report.to_string();
    let mut lines = per_check(&text);
    let mut counts = Counts::default();
    for check in &report.checks {
        *match check.status {
            Status::Skip => &mut counts.skip,
            Status::Ok => &mut counts.ok,
            Status::Warn => &mut counts.warn,
            Status::Fail => &mut counts.fail,
        } += 1;
        sink.event("check", check, lines.next().unwrap_or_default())?;
    }
    if args.flag("strict") && counts.fail > 0 {
        return Err(Failure::new(
            FailureKind::Validation,
            "checks_failed",
            format!(
                "{} check{} failed",
                counts.fail,
                if counts.fail == 1 { "" } else { "s" }
            ),
        ));
    }
    Ok(match sink.format() {
        Format::Jsonl => Reply::new(&counts, ""),
        _ => Reply::new(&report, ""),
    })
}

/// The lines of each check in a report's text: a check's first line starts with its status, the lines below it with a
/// space.
fn per_check(text: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = text
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .filter(|&i| i < text.len() && !text[i..].starts_with(' '))
        .collect();
    if !text.is_empty() {
        starts.insert(0, 0);
    }
    starts.push(text.len());
    let starts: Vec<(usize, usize)> = starts.windows(2).map(|w| (w[0], w[1])).collect();
    starts.into_iter().map(move |(from, to)| &text[from..to])
}

/// The machine of the context: its root and sysfs, its variables and the service manager.
fn sources_of(cx: &Context) -> Sources {
    let mut sources = Sources::system();
    sources.root = cx.root().to_path_buf();
    sources.sysfs = cx.path("/sys");
    sources.env = cx
        .vars()
        .into_iter()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_values_are_the_area_names_in_order() {
        let names: Vec<&str> = Area::ALL.iter().map(|a| a.name()).collect();
        assert_eq!(AREAS, names.as_slice());
    }

    #[test]
    fn a_report_text_is_cut_at_each_check() {
        let text = "ok    a  one\n      fact\nwarn  b  two\nFAIL  c  three\n      fix: x\n";
        assert_eq!(
            per_check(text).collect::<Vec<_>>(),
            [
                "ok    a  one\n      fact\n",
                "warn  b  two\n",
                "FAIL  c  three\n      fix: x\n"
            ]
        );
        assert_eq!(per_check("").count(), 0);
    }
}
