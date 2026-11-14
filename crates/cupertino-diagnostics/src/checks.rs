//! One function per [`Area`], each returning its checks.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use cupertino_defaults::compositor::CompositorSettings;
use cupertino_devices::preflight;

use cupertino_diagnostic_events::Severity;

use crate::{dev_builds, events, Area, Check, Scope, Sources, Status, Unit};

/// The longest log line a check quotes.
const QUOTE_LIMIT: usize = 300;

/// What the session command writes when the compositor ended on its own or was stopped by the session.
const NORMAL_ENDS: [&str; 2] = [
    "the compositor ended normally",
    "the compositor was stopped by the session",
];

/// Lines of the server's log that report a crash.
const CRASH_MARKS: [&str; 2] = ["panic: ", "fatal signal "];

/// What the server writes to `session.log` after its time when the GPU was reset (`cupertino_server::gpu_reset`).
const GPU_RESET_MARK: &str = " the GPU was reset: gpu=";

/// What the server writes to `session.log` after its time when it renders without a GPU
/// (`cupertino_server::software_render`).
const SOFTWARE_RENDER_MARK: &str = " only a software renderer is available: gpu=";

pub fn gpu(sources: &Sources) -> Vec<Check> {
    vec![
        driver(sources),
        renderer(sources),
        client_renderers(sources),
    ]
}

/// The reasons a process gives for drawing on the CPU when it was meant to draw on the GPU
/// (`cupertino_ui_wayland::renderer::Reason`).
const GPU_FAILURES: [&str; 6] = [
    "no GPU device",
    "no render node",
    "the context was lost",
    "a software rasterizer",
    "no dmabuf feedback",
    "no dmabuf allocator",
];

/// Which renderer every running toolkit process draws with, from the record each writes under
/// `$XDG_RUNTIME_DIR/cupertino/renderers/<pid>`; a process that fell back from the GPU warns.
fn client_renderers(sources: &Sources) -> Check {
    let skip = |why: &str| Check::new("gpu.client_renderers", Area::Gpu, Status::Skip, why);
    let Some(runtime) = sources.var("XDG_RUNTIME_DIR") else {
        return skip("no XDG_RUNTIME_DIR");
    };
    let Ok(entries) = fs::read_dir(Path::new(runtime).join("cupertino/renderers")) else {
        return skip("no process has drawn a window yet");
    };
    let mut processes: Vec<(u32, String, String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if !sources.installed(&format!("proc/{pid}")).exists() {
                return None;
            }
            let text = fs::read_to_string(entry.path()).ok()?;
            let field = |key: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                    .unwrap_or("")
                    .to_owned()
            };
            Some((pid, field("program"), field("renderer"), field("reason")))
        })
        .collect();
    if processes.is_empty() {
        return skip("no running process has drawn a window");
    }
    processes.sort();
    let gpu = processes.iter().filter(|p| p.2 == "gpu").count();
    let failed: Vec<&(u32, String, String, String)> = processes
        .iter()
        .filter(|p| p.2 != "gpu" && GPU_FAILURES.iter().any(|f| p.3.starts_with(f)))
        .collect();
    let mut check = match failed.first() {
        Some((pid, program, _, reason)) => Check::new(
            "gpu.client_renderers",
            Area::Gpu,
            Status::Warn,
            format!(
                "{} of {} processes draw on the CPU after the GPU failed; {program} ({pid}): {reason}",
                failed.len(),
                processes.len()
            ),
        )
        .fix("check gpu.driver; set CUPERTINO_RENDERER=gpu on one process to see the GPU path's own error"),
        None => Check::new(
            "gpu.client_renderers",
            Area::Gpu,
            Status::Ok,
            format!(
                "{gpu} of {} processes draw on the GPU, the rest on the CPU",
                processes.len()
            ),
        ),
    };
    for (pid, program, renderer, reason) in &processes {
        check = check.fact(
            "process",
            format!("{program} ({pid}): {renderer}, {reason}"),
        );
    }
    check
}

/// The primary GPU's driver and what it needs (`cupertino_devices::preflight`).
fn driver(sources: &Sources) -> Check {
    let report = match preflight::run(&sources.sysfs) {
        Ok(Some(report)) => report,
        Ok(None) => {
            return Check::new(
                "gpu.driver",
                Area::Gpu,
                Status::Fail,
                "no GPU found: /sys/class/drm lists no card",
            )
            .fix("check `lspci -k` for a display controller and its kernel driver")
        }
        Err(e) => {
            return Check::new(
                "gpu.driver",
                Area::Gpu,
                Status::Fail,
                format!("listing GPUs: {e}"),
            )
        }
    };
    let status = if !report.problems.is_empty() {
        Status::Fail
    } else if !report.warnings.is_empty() {
        Status::Warn
    } else {
        Status::Ok
    };
    let summary = match (
        &report.driver,
        report.problems.first(),
        report.warnings.first(),
    ) {
        (_, Some(problem), _) => problem.what.clone(),
        (_, None, Some(warning)) => warning.what.clone(),
        (Some(driver), None, None) => format!("the primary GPU uses the {driver} driver"),
        (None, None, None) => "the primary GPU has no driver bound".to_owned(),
    };
    let mut check = Check::new("gpu.driver", Area::Gpu, status, summary);
    if let Some(driver) = &report.driver {
        check = check.fact("driver", driver.clone());
    }
    for note in &report.notes {
        check = check.fact("note", note.clone());
    }
    for finding in report.problems.iter().chain(&report.warnings).skip(1) {
        check = check.fact("also", finding.what.clone());
    }
    let fixes: Vec<&str> = report
        .problems
        .iter()
        .chain(&report.warnings)
        .map(|f| f.fix.as_str())
        .collect();
    if !fixes.is_empty() {
        check = check.fix(fixes.join("; "));
    }
    check
}

/// Whether the last session, or the one running, rendered with a software renderer, as `session.log` says: no session
/// started after its line.
fn renderer(sources: &Sources) -> Check {
    let Some(text) = sources
        .state_dir()
        .and_then(|state| fs::read_to_string(state.join("session.log")).ok())
    else {
        return Check::new(
            "gpu.renderer",
            Area::Gpu,
            Status::Skip,
            "no session has run yet (no session.log)",
        );
    };
    let lines: Vec<&str> = text.lines().collect();
    let software = lines
        .iter()
        .rposition(|line| line.contains(SOFTWARE_RENDER_MARK))
        .filter(|&at| !lines[at + 1..].iter().any(|line| session_start(line)))
        .and_then(|at| lines[at].split_once(SOFTWARE_RENDER_MARK));
    let Some((time, rest)) = software else {
        return Check::new(
            "gpu.renderer",
            Area::Gpu,
            Status::Ok,
            "the last session rendered on a GPU",
        );
    };
    let gpu = rest.split_once(';').map_or(rest, |(gpu, _)| gpu);
    Check::new(
        "gpu.renderer",
        Area::Gpu,
        Status::Warn,
        format!("the last session had only a software renderer on {gpu}: the CPU draws the desktop"),
    )
    .fact("time", time.trim())
    .fact("gpu", gpu)
    .fix(
        "install the GPU's Mesa driver, or give a virtual machine a GPU with 3D (virtio-vga-gl or virtio-gpu-gl)",
    )
}

/// A line `cupertino-session` writes before it starts the compositor: `<date> <time> starting`, with a limit or not.
fn session_start(line: &str) -> bool {
    line.splitn(3, ' ')
        .nth(2)
        .is_some_and(|rest| rest == "starting" || rest.starts_with("starting with a limit of "))
}

pub fn session(sources: &Sources) -> Vec<Check> {
    let Some(id) = sources.var("XDG_SESSION_ID") else {
        return vec![Check::new(
            "session.logind",
            Area::Session,
            Status::Skip,
            "not run from a logind session",
        )];
    };
    let mut checks = vec![logind_session(sources, id)];
    checks.push(match sources.var("XDG_CURRENT_DESKTOP") {
        Some(desktop) if desktop.split(':').any(|d| d == "Cupertino") => Check::new(
            "session.desktop",
            Area::Session,
            Status::Ok,
            format!("XDG_CURRENT_DESKTOP is {desktop}"),
        ),
        Some(desktop) => Check::new(
            "session.desktop",
            Area::Session,
            Status::Skip,
            format!("not a Cupertino session (XDG_CURRENT_DESKTOP is {desktop})"),
        ),
        None => Check::new(
            "session.desktop",
            Area::Session,
            Status::Skip,
            "XDG_CURRENT_DESKTOP is not set: not a graphical session",
        ),
    });
    checks.push(wayland_display(sources));
    checks
}

fn logind_session(sources: &Sources, id: &str) -> Check {
    let path = sources.installed("/run/systemd/sessions").join(id);
    let Ok(text) = fs::read_to_string(&path) else {
        return Check::new(
            "session.logind",
            Area::Session,
            Status::Warn,
            format!("logind has no record of session {id}"),
        )
        .fact("path", path.display().to_string());
    };
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::to_owned)
    };
    let (kind, active) = (field("TYPE"), field("ACTIVE"));
    let status = match (kind.as_deref(), active.as_deref()) {
        (Some("wayland"), Some("1")) => Status::Ok,
        _ => Status::Warn,
    };
    let mut check = Check::new(
        "session.logind",
        Area::Session,
        status,
        format!(
            "session {id}: type {}, {}",
            kind.as_deref().unwrap_or("unknown"),
            if active.as_deref() == Some("1") {
                "active"
            } else {
                "not active"
            }
        ),
    );
    for key in ["CLASS", "SERVICE", "VTNR", "SEAT", "DESKTOP"] {
        if let Some(value) = field(key) {
            check = check.fact(key_name(key), value);
        }
    }
    if status == Status::Warn {
        check = check
            .fix("log in from the login screen, which opens a wayland session on its own console");
    }
    check
}

fn key_name(key: &str) -> &'static str {
    match key {
        "CLASS" => "class",
        "SERVICE" => "pam_service",
        "VTNR" => "vt",
        "SEAT" => "seat",
        _ => "desktop",
    }
}

fn wayland_display(sources: &Sources) -> Check {
    let Some(display) = sources.var("WAYLAND_DISPLAY") else {
        return Check::new(
            "session.wayland",
            Area::Session,
            Status::Skip,
            "WAYLAND_DISPLAY is not set",
        );
    };
    let socket = match (
        Path::new(display).is_absolute(),
        sources.var("XDG_RUNTIME_DIR"),
    ) {
        (true, _) => PathBuf::from(display),
        (false, Some(runtime)) => Path::new(runtime).join(display),
        (false, None) => {
            return Check::new(
                "session.wayland",
                Area::Session,
                Status::Fail,
                "XDG_RUNTIME_DIR is not set, so the display cannot be found",
            )
        }
    };
    let check = match UnixStream::connect(&socket) {
        Ok(_) => Check::new(
            "session.wayland",
            Area::Session,
            Status::Ok,
            format!("the compositor answers on {display}"),
        ),
        Err(e) => Check::new(
            "session.wayland",
            Area::Session,
            Status::Fail,
            format!("nothing answers on {display}: {e}"),
        )
        .fix("the compositor is gone: see `crash.last` and the server's log"),
    };
    check.fact("socket", socket.display().to_string())
}

pub fn portals(sources: &Sources) -> Vec<Check> {
    let file = sources.installed("/usr/share/xdg-desktop-portal/portals/cupertino.portal");
    let backend = match fs::read_to_string(&file) {
        Ok(text) => {
            let interfaces: Vec<&str> = text
                .lines()
                .find_map(|line| line.strip_prefix("Interfaces="))
                .map(|list| {
                    list.split(';')
                        .filter(|i| !i.is_empty())
                        .map(|i| i.trim_start_matches("org.freedesktop.impl.portal."))
                        .collect()
                })
                .unwrap_or_default();
            if interfaces.contains(&"ScreenCast") {
                Check::new(
                    "portals.backend",
                    Area::Portals,
                    Status::Ok,
                    format!("the Cupertino backend serves {}", interfaces.join(", ")),
                )
            } else {
                Check::new(
                    "portals.backend",
                    Area::Portals,
                    Status::Warn,
                    "the Cupertino backend does not list ScreenCast: screen sharing does not work",
                )
                .fix("reinstall the cupertino package")
            }
        }
        Err(_) => Check::new(
            "portals.backend",
            Area::Portals,
            Status::Warn,
            "the Cupertino portal backend is not installed: screen sharing does not work",
        )
        .fix("install a cupertino package that ships cupertino-portal"),
    }
    .fact("file", file.display().to_string());
    let conf = portals_conf(sources);
    let mut checks = vec![backend, conf];
    if sources.var("XDG_SESSION_ID").is_some() {
        checks.push(unit(
            sources,
            "portals.service",
            Area::Portals,
            Scope::User,
            "xdg-desktop-portal.service",
            "applications cannot ask for files, settings or screen sharing",
            None,
        ));
    }
    checks
}

/// The first `cupertino-portals.conf` xdg-desktop-portal reads, user configuration first.
fn portals_conf(sources: &Sources) -> Check {
    let user = sources
        .var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| sources.var("HOME").map(|h| Path::new(h).join(".config")))
        .map(|config| config.join("xdg-desktop-portal/cupertino-portals.conf"));
    let candidates = user.into_iter().chain([
        sources.installed("/etc/xdg-desktop-portal/cupertino-portals.conf"),
        sources.installed("/usr/share/xdg-desktop-portal/cupertino-portals.conf"),
    ]);
    for path in candidates {
        if let Ok(text) = fs::read_to_string(&path) {
            let screencast = text.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("org.freedesktop.impl.portal.ScreenCast=")
            });
            let check = match screencast {
                Some(backends) if backends.split(';').next() == Some("cupertino") => Check::new(
                    "portals.conf",
                    Area::Portals,
                    Status::Ok,
                    "screen sharing goes to the Cupertino backend",
                ),
                _ => Check::new(
                    "portals.conf",
                    Area::Portals,
                    Status::Warn,
                    "screen sharing does not go to the Cupertino backend first",
                )
                .fix(format!(
                    "set `org.freedesktop.impl.portal.ScreenCast=cupertino` in {}",
                    path.display()
                )),
            };
            return check.fact("file", path.display().to_string());
        }
    }
    Check::new(
        "portals.conf",
        Area::Portals,
        Status::Warn,
        "no cupertino-portals.conf: xdg-desktop-portal picks the backends on its own",
    )
    .fix("reinstall the cupertino package")
}

pub fn services(sources: &Sources) -> Vec<Check> {
    let mut checks = vec![
        unit(
            sources,
            "services.login",
            Area::Services,
            Scope::System,
            "cupertino-login.service",
            "no login screen at boot",
            None,
        ),
        unit(
            sources,
            "services.network",
            Area::Services,
            Scope::System,
            "cupertino-netd.service",
            "the network menu and settings cannot change anything",
            Some(("NetworkManager.service", "NetworkManager")),
        ),
        unit(
            sources,
            "services.power",
            Area::Services,
            Scope::System,
            "cupertino-powerd.service",
            "no battery or power profile information",
            None,
        ),
        unit(
            sources,
            "services.bluetooth",
            Area::Services,
            Scope::System,
            "cupertino-btd.service",
            "no Bluetooth menu",
            Some(("bluetooth.service", "BlueZ")),
        ),
    ];
    if sources.var("XDG_SESSION_ID").is_some() {
        checks.push(unit(
            sources,
            "services.session_target",
            Area::Services,
            Scope::User,
            "cupertino-session.target",
            "user services bound to the session did not start",
            None,
        ));
        checks.push(unit(
            sources,
            "services.pipewire",
            Area::Services,
            Scope::User,
            "pipewire.service",
            "no sound and no screen sharing",
            None,
        ));
    }
    checks
}

fn unit(
    sources: &Sources,
    id: &'static str,
    area: Area,
    scope: Scope,
    name: &str,
    without: &str,
    replaced_by: Option<(&str, &str)>,
) -> Check {
    let manager = match scope {
        Scope::System => "systemctl",
        Scope::User => "systemctl --user",
    };
    let Some(unit) = (sources.unit_state)(scope, name) else {
        return Check::new(
            id,
            area,
            Status::Skip,
            format!("{manager} cannot be asked about {name}"),
        );
    };
    // an installation may leave a Cupertino daemon off and let the distribution's service do its job, whatever the
    // daemon's unit file state; that service may be enabled yet stopped by a unit condition, as BlueZ is on a machine
    // without a Bluetooth adapter
    if let Some((service, by)) = replaced_by {
        let serves = |r: Unit| {
            matches!(r.active.as_str(), "active" | "activating" | "reloading")
                || r.active == "inactive"
                    && matches!(r.file.as_str(), "enabled" | "enabled-runtime")
        };
        if unit.active != "active" && (sources.unit_state)(scope, service).is_some_and(serves) {
            let state = match (unit.active.as_str(), unit.file.as_str()) {
                (crate::NOT_FOUND, _) => "not installed".to_owned(),
                (active, "") => active.to_owned(),
                (active, file) => format!("{active} ({file})"),
            };
            return Check::new(
                id,
                area,
                Status::Ok,
                format!("{name} is {state}: managed by {by}"),
            );
        }
    }
    match unit.active.as_str() {
        "active" => Check::new(id, area, Status::Ok, format!("{name} is active")),
        crate::NOT_FOUND => Check::new(
            id,
            area,
            Status::Warn,
            format!("{name} is not installed: {without}"),
        )
        .fix("install the cupertino package, which ships the unit"),
        state @ ("failed" | "inactive") => {
            let status = if state == "failed" {
                Status::Fail
            } else {
                Status::Warn
            };
            Check::new(id, area, status, format!("{name} is {state}: {without}")).fix(format!(
                "`{manager} status {name}` and `journalctl {} -u {name}` say why",
                if scope == Scope::User { "--user" } else { "-b" }
            ))
        }
        state => Check::new(
            id,
            area,
            Status::Warn,
            format!("{name} is {state}: {without}"),
        ),
    }
}

/// The system configuration, the one file that may declare the development folder.
const SYSTEM_CONFIG: &str = "/etc/cupertino/compositor.json";

pub fn shell(sources: &Sources) -> Vec<Check> {
    vec![dev_folder(sources)]
}

/// Whether a build in the development folder is older than the installed program it starts instead of, for each
/// process of the system file's shell list that the folder applies to, as the server decides it: named in the folder's
/// components, with only roles that may start from the folder.
fn dev_folder(sources: &Sources) -> Check {
    const ID: &str = "shell.dev_folder";
    let Ok(text) = fs::read_to_string(sources.installed(SYSTEM_CONFIG)) else {
        return Check::new(
            ID,
            Area::Shell,
            Status::Skip,
            format!("no system configuration ({SYSTEM_CONFIG})"),
        );
    };
    let settings = CompositorSettings::parse(&text, None).0;
    let Some(folder) = settings.dev_folder else {
        return Check::new(
            ID,
            Area::Shell,
            Status::Skip,
            "no development folder is declared: every shell process starts its installed program",
        );
    };
    let entries = settings.shell.unwrap_or_default();
    let mut builds = Vec::new();
    let mut older = Vec::new();
    for name in &folder.components {
        let Some(installed) = entries
            .iter()
            .find(|e| &e.name == name && e.may_start_from_dev_folder())
            .map(|e| e.command[0].as_str())
        else {
            continue;
        };
        let Some(file_name) = Path::new(installed).file_name() else {
            continue;
        };
        let build = Path::new(&folder.path).join(file_name);
        let found = sources.installed(&folder.path).join(file_name);
        if fs::symlink_metadata(&found).is_err() {
            continue;
        }
        // a program the system file names by a relative path is found on the server's PATH: not compared
        let is_older = Path::new(installed).is_absolute()
            && dev_builds::older_than_installed(&found, &sources.installed(installed));
        if is_older {
            older.push((name.as_str(), build, installed));
        } else {
            builds.push((name.as_str(), build));
        }
    }
    let mut check = if older.is_empty() {
        Check::new(
            ID,
            Area::Shell,
            Status::Ok,
            if builds.is_empty() {
                "the development folder holds no build".to_owned()
            } else {
                "no build in the development folder is older than its installed program".to_owned()
            },
        )
    } else {
        let names: Vec<&str> = older.iter().map(|(name, ..)| *name).collect();
        let commands: Vec<String> = names
            .iter()
            .map(|name| format!("`cupertino shell use-installed {name}`"))
            .collect();
        Check::new(
            ID,
            Area::Shell,
            Status::Warn,
            format!(
                "the development folder's build of {} is older than the installed program and starts instead of it",
                names.join(", ")
            ),
        )
        .fix(format!(
            "{} removes the older build and restarts the process on its installed program",
            commands.join(", ")
        ))
    };
    check = check.fact("folder", folder.path.clone());
    for (name, build, installed) in &older {
        check = check.fact(
            "older",
            format!("{name}: {} is older than {installed}", build.display()),
        );
    }
    for (name, build) in &builds {
        check = check.fact("build", format!("{name}: {}", build.display()));
    }
    check
}

pub fn crash(sources: &Sources) -> Vec<Check> {
    let Some(state) = sources.state_dir() else {
        return vec![Check::new(
            "crash.last",
            Area::Crash,
            Status::Skip,
            "neither XDG_STATE_HOME nor HOME is set",
        )];
    };
    let log = state.join("session.log");
    let Ok(text) = fs::read_to_string(&log) else {
        return vec![Check::new(
            "crash.last",
            Area::Crash,
            Status::Skip,
            "no session has ended yet (no session.log)",
        )
        .fact("file", log.display().to_string())];
    };
    let ends: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(" the compositor "))
        .collect();
    let abnormal: Vec<&&str> = ends
        .iter()
        .filter(|line| !NORMAL_ENDS.iter().any(|normal| line.contains(normal)))
        .collect();
    let mut check = match ends.last() {
        None => Check::new(
            "crash.last",
            Area::Crash,
            Status::Ok,
            "no session has ended yet",
        ),
        Some(last) if abnormal.last() == Some(&last) => Check::new(
            "crash.last",
            Area::Crash,
            Status::Warn,
            format!("the last session ended badly: {}", quote(last)),
        )
        .fix("the server's log of that run has the panic or the signal and its backtrace"),
        Some(last) => Check::new(
            "crash.last",
            Area::Crash,
            Status::Ok,
            format!("the last session ended well: {}", quote(last)),
        ),
    }
    .fact(
        "ended_badly",
        format!("{} of {} sessions", abnormal.len(), ends.len()),
    );
    if let Some(line) = abnormal.last() {
        check = check.fact("last_bad_end", quote(line));
    }
    for name in ["server.log", "server.log.1"] {
        if let Some(line) = first_crash_line(&state.join(name)) {
            check = check.fact(
                if name == "server.log" {
                    "server_log"
                } else {
                    "previous_server_log"
                },
                line,
            );
        }
    }
    vec![check, gpu_resets(&text)]
}

/// The GPU resets `session.log` records, the last one in parts.
fn gpu_resets(log: &str) -> Check {
    let resets: Vec<GpuReset> = log.lines().filter_map(GpuReset::parse).collect();
    let Some(last) = resets.last() else {
        return Check::new(
            "crash.gpu_reset",
            Area::Crash,
            Status::Ok,
            "the GPU has not been reset",
        );
    };
    let remade = resets.iter().filter(|r| r.remade()).count();
    Check::new(
        "crash.gpu_reset",
        Area::Crash,
        Status::Warn,
        format!(
            "the GPU was reset {} times, the last at {}: {}",
            resets.len(),
            last.time,
            last.outcome
        ),
    )
    .fact("resets", resets.len().to_string())
    .fact("remade", format!("{remade} of {}", resets.len()))
    .fact("last_time", last.time)
    .fact("last_gpu", last.gpu)
    .fact("last_reason", quote(last.reason))
    .fact("last_outcome", quote(last.outcome))
    .fix(
        "a reset is the GPU driver's or the hardware's; to investigate it, give the output of `cupertino-diagnose \
         --json` and the server.log lines around that time to an AI assistant, or to the driver's bug tracker",
    )
}

/// A `session.log` line of a GPU reset: `<time> the GPU was reset: gpu=<gpu> reason=<reason>; <outcome>`.
struct GpuReset<'a> {
    time: &'a str,
    gpu: &'a str,
    reason: &'a str,
    outcome: &'a str,
}

impl<'a> GpuReset<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let (time, rest) = line.split_once(GPU_RESET_MARK)?;
        let (gpu, rest) = rest.split_once(" reason=")?;
        let at = rest
            .find("; renderer remade")
            .or_else(|| rest.find("; renderer not remade"))?;
        Some(GpuReset {
            time: time.trim(),
            gpu,
            reason: &rest[..at],
            outcome: &rest[at + 2..],
        })
    }

    fn remade(&self) -> bool {
        self.outcome.starts_with("renderer remade")
    }
}

fn first_crash_line(path: &Path) -> Option<String> {
    let text = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&text);
    text.lines()
        .find(|line| CRASH_MARKS.iter().any(|mark| line.contains(mark)))
        .map(quote)
}

fn quote(line: &str) -> String {
    let mut line = line.trim().to_owned();
    if line.len() > QUOTE_LIMIT {
        let mut end = QUOTE_LIMIT;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

pub fn logs(sources: &Sources) -> Vec<Check> {
    let Some(state) = sources.state_dir() else {
        return vec![Check::new(
            "logs.paths",
            Area::Logs,
            Status::Skip,
            "neither XDG_STATE_HOME nor HOME is set",
        )];
    };
    let mut check = Check::new(
        "logs.paths",
        Area::Logs,
        Status::Ok,
        format!("the session's logs are in {}", state.display()),
    );
    for (key, name) in [
        ("session_log", "session.log"),
        ("server_log", "server.log"),
        ("previous_server_log", "server.log.1"),
    ] {
        let path = state.join(name);
        if let Ok(meta) = fs::metadata(&path) {
            check = check.fact(key, format!("{} ({} bytes)", path.display(), meta.len()));
        }
    }
    vec![check.fact(
        "journal",
        "`journalctl -b -u cupertino-login.service` has the login screen's and the sessions' output",
    )]
}

/// How far back `events.recent` looks.
const RECENT: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// The `error` and `fatal` diagnostic events of the last 24 hours, from the store, `session.log` and the journal.
pub fn events(sources: &Sources) -> Vec<Check> {
    let since = std::time::SystemTime::now().checked_sub(RECENT);
    let filter = events::Filter {
        since,
        ..events::Filter::default()
    };
    let report = events::read(sources, &filter);
    let readable = report.sources.iter().any(|s| s.status.readable());
    let with_sources = |mut check: Check| {
        for source in &report.sources {
            check = check.fact(
                "source",
                format!("{}: {}", source.name, source.status_text()),
            );
        }
        check
    };
    if !readable {
        return vec![with_sources(Check::new(
            "events.recent",
            Area::Events,
            Status::Skip,
            "no diagnostic events can be read",
        ))];
    }
    let counts = report.counts();
    let bad: Vec<&events::Event> = report
        .events
        .iter()
        .filter(|e| e.record.severity >= Severity::Error)
        .collect();
    if bad.is_empty() {
        let check = Check::new(
            "events.recent",
            Area::Events,
            Status::Ok,
            format!(
                "no error or fatal event in the last 24 h ({} events)",
                counts.events
            ),
        )
        .fact("events", counts.events.to_string());
        return vec![with_sources(check)];
    }
    let mut check = Check::new(
        "events.recent",
        Area::Events,
        Status::Warn,
        format!(
            "{} error and {} fatal events in the last 24 h",
            counts.error, counts.fatal
        ),
    )
    .fact("error", counts.error.to_string())
    .fact("fatal", counts.fatal.to_string());
    for (id, severity, n, last) in events::by_id(bad) {
        check = check.fact("event", format!("{id}: {n} {severity}, the last at {last}"));
    }
    vec![with_sources(check).fix(
        "`cupertino diagnose events --since 24h` lists them with their fields; `cupertino diagnose export --output \
         FILE` writes them with this report into one file for a bug report",
    )]
}
