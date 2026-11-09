//! A `cupertino` program with a registry of probe streams, run through the same [`cupertino_cli::main_with`] as the
//! real command: `tests/stream-process.rs` stops it with signals and closes its output.

use cupertino_cli::{
    Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Sink, Stability,
};
use std::time::Duration;

const COMMANDS: &[Command] = &[
    Command {
        path: &["probe", "follow"],
        summary: "Reports `started`, then waits until it is stopped",
        description: "A follow stream.",
        examples: &[("cupertino probe follow", "Follow until stopped.")],
        stability: Stability::Preview,
        args: &[],
        run: Run::Stream(follow),
    },
    Command {
        path: &["probe", "finite"],
        summary: "Reports `started`, then waits; a stop interrupts it",
        description: "A finite stream.",
        examples: &[("cupertino probe finite", "Wait to be interrupted.")],
        stability: Stability::Preview,
        args: &[],
        run: Run::Stream(finite),
    },
    Command {
        path: &["probe", "flood"],
        summary: "Reports events until the output closes",
        description: "A stream that never ends by itself.",
        examples: &[("cupertino probe flood | head -1", "Read one event.")],
        stability: Stability::Preview,
        args: &[],
        run: Run::Stream(flood),
    },
    Command {
        path: &["probe", "lines"],
        summary:
            "Reports, for every line on stdin, whether a stop was asked; ends at the end of stdin",
        description: "A follow stream that does not stop by itself when asked.",
        examples: &[("cupertino probe lines", "Report the lines.")],
        stability: Stability::Preview,
        args: &[],
        run: Run::Stream(lines),
    },
    Command {
        path: &["probe", "child"],
        summary:
            "Starts `cat /proc/self/status` and reports the signals the child starts with ignored",
        description: "A stream that starts a program, as a command that runs others does.",
        examples: &[(
            "cupertino probe child",
            "Report the child's ignored signals.",
        )],
        stability: Stability::Preview,
        args: &[],
        run: Run::Stream(child),
    },
];

const REGISTRY: &[Group] = &[Group {
    name: "probe",
    summary: "Probe streams",
    description: "Streams for the process tests.",
    commands: &[COMMANDS],
}];

/// How often the probes look at the stop flag.
const STEP: Duration = Duration::from_millis(5);

fn follow(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.follow();
    sink.event("started", &serde_json::json!({}), "started")?;
    while !sink.stop_requested() {
        std::thread::sleep(STEP);
    }
    Ok(Reply::new(
        &serde_json::json!({"stopped": true}),
        "stopped\n",
    ))
}

fn finite(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.event("started", &serde_json::json!({}), "started")?;
    while !sink.stop_requested() {
        std::thread::sleep(STEP);
    }
    Err(Failure::interrupted())
}

fn flood(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    for n in 0u64.. {
        sink.event("item", &serde_json::json!({ "n": n }), &format!("item {n}"))?;
    }
    unreachable!("the loop ends when the output closes")
}

fn lines(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.follow();
    sink.event("started", &serde_json::json!({}), "started")?;
    for line in std::io::stdin().lines() {
        if line.is_err() {
            break;
        }
        let stop_requested = sink.stop_requested();
        sink.event(
            "line",
            &serde_json::json!({ "stop_requested": stop_requested }),
            "line",
        )?;
    }
    Ok(Reply::new(&serde_json::json!({}), ""))
}

/// The child's `SigIgn` mask, read by a program that is not built with Rust's start-up code, so it shows what it
/// inherited.
fn child(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    let out = std::process::Command::new("cat")
        .arg("/proc/self/status")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Failure::new(FailureKind::Unavailable, "no_cat", e.to_string()))?;
    let status = String::from_utf8_lossy(&out.stdout);
    let ignored = status
        .lines()
        .find_map(|l| l.strip_prefix("SigIgn:"))
        .map(str::trim)
        .unwrap_or_default();
    sink.event("child", &serde_json::json!({ "sig_ign": ignored }), ignored)?;
    Ok(Reply::new(&serde_json::json!({}), ""))
}

fn main() -> std::process::ExitCode {
    cupertino_cli::main_with(REGISTRY)
}
