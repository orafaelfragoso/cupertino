//! One function per [`Area`], each returning its checks.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use cupertino_devices::preflight;

use crate::{Area, Check, Scope, Sources, Status};

/// The longest log line a check quotes.
const QUOTE_LIMIT: usize = 300;

/// What the session command writes when the compositor ended on its own or was stopped by the session.
const NORMAL_ENDS: [&str; 2] = [
    "the compositor ended normally",
    "the compositor was stopped by the session",
];

/// Lines of the server's log that report a crash.
const CRASH_MARKS: [&str; 2] = ["panic: ", "fatal signal "];

pub fn gpu(sources: &Sources) -> Vec<Check> {
    let report = match preflight::run(&sources.sysfs) {
        Ok(Some(report)) => report,
        Ok(None) => {
            return vec![Check::new(
                "gpu.driver",
                Area::Gpu,
                Status::Fail,
                "no GPU found: /sys/class/drm lists no card",
            )
            .fix("check `lspci -k` for a display controller and its kernel driver")]
        }
        Err(e) => {
            return vec![Check::new(
                "gpu.driver",
                Area::Gpu,
                Status::Fail,
                format!("listing GPUs: {e}"),
            )]
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
    vec![check]
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
        ),
        unit(
            sources,
            "services.network",
            Area::Services,
            Scope::System,
            "cupertino-netd.service",
            "the network menu and settings cannot change anything",
        ),
        unit(
            sources,
            "services.power",
            Area::Services,
            Scope::System,
            "cupertino-powerd.service",
            "no battery or power profile information",
        ),
        unit(
            sources,
            "services.bluetooth",
            Area::Services,
            Scope::System,
            "cupertino-btd.service",
            "no Bluetooth menu",
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
        ));
        checks.push(unit(
            sources,
            "services.pipewire",
            Area::Services,
            Scope::User,
            "pipewire.service",
            "no sound and no screen sharing",
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
) -> Check {
    let manager = match scope {
        Scope::System => "systemctl",
        Scope::User => "systemctl --user",
    };
    match (sources.unit_state)(scope, name).as_deref() {
        None => Check::new(
            id,
            area,
            Status::Skip,
            format!("{manager} cannot be asked about {name}"),
        ),
        Some("active") => Check::new(id, area, Status::Ok, format!("{name} is active")),
        Some(crate::NOT_FOUND) => Check::new(
            id,
            area,
            Status::Warn,
            format!("{name} is not installed: {without}"),
        )
        .fix("install the cupertino package, which ships the unit"),
        Some(state @ ("failed" | "inactive")) => {
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
        Some(state) => Check::new(
            id,
            area,
            Status::Warn,
            format!("{name} is {state}: {without}"),
        ),
    }
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
    vec![check]
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
