//! `cupertino version`: the versions of the command, the installed packages, the running compositor, the C library,
//! the theme, the asset pack, the output schemas, the OS image and the kernel.

use crate::{Args, Command, Context, Failure, Group, Reply, Run, Stability, SCHEMA, STREAM_SCHEMA};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) const GROUP: Group = Group {
    name: "version",
    summary: "The versions of the desktop's parts",
    description: "Reports what is installed and running, for a bug report or a script that needs a minimum version.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[Command {
    path: &["version"],
    summary: "Reports the version of each part of the desktop: this command, the packages, the running compositor, \
              the C library, the theme, the asset pack, the output schemas, the OS image and the kernel",
    description: "Lists one entry per part with its name, version, source (the file it was read from), status and \
                  a detail. The status is ok, not_installed (the file is not there), unreadable (it is there but \
                  cannot be read or parsed) or not_running (the compositor of WAYLAND_DISPLAY is not a running \
                  Cupertino compositor); an entry that is not ok never fails the command, which exits 0.\n\n\
                  The parts are: cli (this command and its commit), runtime-package and data-package (the build \
                  records the packages install, /usr/share/cupertino/build-info.json and \
                  /usr/share/cupertino/assets/build-info.json), compositor-running (the runtime file the compositor \
                  of WAYLAND_DISPLAY writes in XDG_RUNTIME_DIR/cupertino), c-abi (cupertinokit.pc and the ABI \
                  version of its header), theme (the tokens' version, else the start of their sha256), asset-pack \
                  (the default pack's manifest), the schemas of the output forms, os-image (os-release) and kernel. \
                  `cupertino --version` prints this command's version alone.",
    examples: &[
        ("cupertino version", "Show the version of every part."),
        (
            "cupertino --format json version",
            "Report the versions as one JSON envelope, for a bug report.",
        ),
    ],
    stability: Stability::Stable,
    args: &[],
    run: Run::Once(version),
}];

/// The output schemas this command speaks, as `(entry name, version)`.
const SCHEMAS: &[(&str, &str)] = &[
    ("schema-cli", SCHEMA),
    ("schema-cli-stream", STREAM_SCHEMA),
    ("schema-diagnose", DIAGNOSE_SCHEMA),
];

/// The schema of `cupertino-diagnose --json` and of the `diagnose` report.
const DIAGNOSE_SCHEMA: &str = {
    assert!(
        cupertino_diagnostics::SCHEMA == 1,
        "update the diagnose schema entry"
    );
    "1"
};

/// The build record of the runtime package.
const RUNTIME_RECORD: &str = "/usr/share/cupertino/build-info.json";
/// The build record of the data package.
const DATA_RECORD: &str = "/usr/share/cupertino/assets/build-info.json";
const PKG_CONFIG: &str = "/usr/lib/pkgconfig/cupertinokit.pc";
const HEADER: &str = "/usr/include/cupertinokit-1/cupertinokit.h";
const TOKENS: &str = "/usr/share/cupertino/theme/tokens.json";
const PACK_MANIFEST: &str = "/usr/share/cupertino/assets/manifest.json";
/// How many hexadecimal digits of the tokens' sha256 stand for their version when they have none.
const DIGEST_DIGITS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    NotInstalled,
    Unreadable,
    NotRunning,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::NotInstalled => "not_installed",
            Status::Unreadable => "unreadable",
            Status::NotRunning => "not_running",
        }
    }
}

#[derive(Debug, Serialize)]
struct Entry {
    name: &'static str,
    version: Option<String>,
    /// Where it was read: an absolute path of the running system, or `built-in`.
    source: String,
    status: Status,
    detail: String,
    /// The commit it was built from, when it says.
    commit: Option<String>,
}

impl Entry {
    fn new(
        name: &'static str,
        source: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name,
            version: None,
            source: source.into(),
            status,
            detail: detail.into(),
            commit: None,
        }
    }

    fn ok(name: &'static str, source: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            version: Some(version.into()),
            ..Self::new(name, source, Status::Ok, "")
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    fn commit(mut self, commit: Option<&str>) -> Self {
        self.commit = commit.map(str::to_owned);
        self
    }
}

#[derive(Serialize)]
struct Versions {
    components: Vec<Entry>,
}

fn version(_: &Args, cx: &Context) -> Result<Reply, Failure> {
    let mut components = vec![
        Entry::ok("cli", "built-in", env!("CARGO_PKG_VERSION"))
            .commit(cupertino_build_info::COMMIT)
            .detail(format!(
                "commit from {}",
                cupertino_build_info::COMMIT_SOURCE.name()
            )),
        record(cx, "runtime-package", RUNTIME_RECORD),
        record(cx, "data-package", DATA_RECORD),
        compositor(cx),
        c_abi(cx),
        theme(cx),
        asset_pack(cx),
    ];
    components.extend(
        SCHEMAS
            .iter()
            .map(|(name, version)| Entry::ok(name, "built-in", *version)),
    );
    components.push(os_image(cx));
    components.push(kernel(cx));
    let mut human = String::new();
    for c in &components {
        let line = format!(
            "{:<20} {:<26} {:<14} {}",
            c.name,
            c.version.as_deref().unwrap_or("-"),
            c.status.name(),
            [
                c.commit.as_deref().map(short).unwrap_or_default(),
                c.detail.as_str()
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
        );
        let _ = writeln!(human, "{}", line.trim_end());
    }
    Ok(Reply::new(&Versions { components }, human))
}

/// The first digits of a commit as people quote it; a record's commit is read from a file, so it is cut at a character,
/// whatever it holds.
fn short(commit: &str) -> &str {
    commit
        .char_indices()
        .nth(cupertino_build_info::SHORT_COMMIT_DIGITS)
        .map_or(commit, |(end, _)| &commit[..end])
}

/// A file of the running system, by absolute path: its text, or the entry saying why there is none.
fn read(cx: &Context, name: &'static str, absolute: &str) -> Result<String, Entry> {
    match std::fs::read_to_string(cx.path(absolute)) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Entry::new(
            name,
            absolute,
            Status::NotInstalled,
            format!("{absolute} does not exist"),
        )),
        Err(e) => Err(Entry::new(
            name,
            absolute,
            Status::Unreadable,
            format!("cannot read {absolute}: {e}"),
        )),
    }
}

fn json(name: &'static str, absolute: &str, text: &str) -> Result<Value, Entry> {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| {
            Entry::new(
                name,
                absolute,
                Status::Unreadable,
                format!("{absolute} is not a JSON object"),
            )
        })
}

/// A string or a number of a JSON object, as text.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A package's build record, `{package, version, commit, source_date_epoch}`, written when the package was made.
fn record(cx: &Context, name: &'static str, absolute: &str) -> Entry {
    let parsed = read(cx, name, absolute).and_then(|t| json(name, absolute, &t));
    let v = match parsed {
        Ok(v) => v,
        Err(entry) => return entry,
    };
    let Some(version) = text(&v["version"]) else {
        return Entry::new(
            name,
            absolute,
            Status::Unreadable,
            format!("{absolute} has no version"),
        );
    };
    let package = v["package"].as_str().unwrap_or_default();
    Entry::ok(name, absolute, version)
        .commit(v["commit"].as_str())
        .detail(format!("package {package}"))
}

/// The compositor of `WAYLAND_DISPLAY`, from the runtime file it writes while it runs.
fn compositor(cx: &Context) -> Entry {
    const NAME: &str = "compositor-running";
    let set = |name: &str| cx.var(name).filter(|v| !v.is_empty());
    let Some(display) = set("WAYLAND_DISPLAY") else {
        return Entry::new(
            NAME,
            "WAYLAND_DISPLAY",
            Status::NotRunning,
            "WAYLAND_DISPLAY is not set",
        );
    };
    let Some(runtime) = set("XDG_RUNTIME_DIR") else {
        return Entry::new(
            NAME,
            "XDG_RUNTIME_DIR",
            Status::NotRunning,
            "XDG_RUNTIME_DIR is not set",
        );
    };
    // a display given as a path names its socket's file
    let display = PathBuf::from(display);
    let socket = display
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = Path::new(&runtime)
        .join("cupertino")
        .join(format!("server-{socket}.json"));
    let source = file.display().to_string();
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Entry::new(
                NAME,
                source,
                Status::NotRunning,
                format!("no Cupertino compositor runs on {socket}"),
            )
        }
        Err(e) => {
            return Entry::new(
                NAME,
                source.clone(),
                Status::Unreadable,
                format!("cannot read {source}: {e}"),
            )
        }
    };
    let v = match json(NAME, "", &text) {
        Ok(v) => v,
        Err(_) => {
            return Entry::new(
                NAME,
                source.clone(),
                Status::Unreadable,
                format!("{source} is not a JSON object"),
            )
        }
    };
    let (Some(version), Some(pid)) = (text_of(&v, "version"), v["pid"].as_u64()) else {
        return Entry::new(
            NAME,
            source.clone(),
            Status::Unreadable,
            format!("{source} has no version or pid"),
        );
    };
    if !cx.path(format!("/proc/{pid}")).exists() {
        return Entry::new(
            NAME,
            source,
            Status::NotRunning,
            format!("the compositor that wrote it, process {pid}, is gone"),
        );
    }
    let field = |key: &str| v[key].as_str().unwrap_or("unknown").to_owned();
    Entry::ok(NAME, source, version)
        .commit(v["commit"].as_str())
        .detail(format!(
            "backend {}, renderer {}, process {pid}",
            field("backend"),
            field("renderer")
        ))
}

fn text_of(v: &Value, key: &str) -> Option<String> {
    text(&v[key])
}

/// The installed C library: the version of its pkg-config file and the ABI version of its header.
fn c_abi(cx: &Context) -> Entry {
    const NAME: &str = "c-abi";
    let pc = match read(cx, NAME, PKG_CONFIG) {
        Ok(t) => t,
        Err(entry) => return entry,
    };
    let header = match read(cx, NAME, HEADER) {
        Ok(t) => t,
        Err(entry) => return entry,
    };
    let version = pc
        .lines()
        .find_map(|l| l.strip_prefix("Version:"))
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let define = |name: &str| {
        header.lines().find_map(|l| {
            let mut words = l.split_whitespace();
            (words.next() == Some("#define") && words.next() == Some(name))
                .then(|| words.next()?.parse::<u32>().ok())
                .flatten()
        })
    };
    match (
        version,
        define("CUPERTINOKIT_ABI_MAJOR"),
        define("CUPERTINOKIT_ABI_MINOR"),
    ) {
        (Some(version), Some(major), Some(minor)) => {
            Entry::ok(NAME, PKG_CONFIG, version).detail(format!("header ABI {major}.{minor}"))
        }
        (None, ..) => Entry::new(
            NAME,
            PKG_CONFIG,
            Status::Unreadable,
            format!("{PKG_CONFIG} has no Version"),
        ),
        _ => Entry::new(
            NAME,
            HEADER,
            Status::Unreadable,
            format!("{HEADER} has no ABI version"),
        ),
    }
}

/// The installed design tokens: their `version`, else the start of their sha256.
fn theme(cx: &Context) -> Entry {
    const NAME: &str = "theme";
    let tokens = match read(cx, NAME, TOKENS) {
        Ok(t) => t,
        Err(entry) => return entry,
    };
    let v = match json(NAME, TOKENS, &tokens) {
        Ok(v) => v,
        Err(entry) => return entry,
    };
    match text(&v["version"]) {
        Some(version) => Entry::ok(NAME, TOKENS, version),
        None => {
            let digest = format!("{:x}", Sha256::digest(tokens.as_bytes()));
            Entry::ok(NAME, TOKENS, format!("sha256:{}", &digest[..DIGEST_DIGITS]))
                .detail("the tokens have no version: the start of their sha256")
        }
    }
}

/// The default asset pack's manifest: its version and name.
fn asset_pack(cx: &Context) -> Entry {
    const NAME: &str = "asset-pack";
    let parsed = read(cx, NAME, PACK_MANIFEST).and_then(|t| json(NAME, PACK_MANIFEST, &t));
    let v = match parsed {
        Ok(v) => v,
        Err(entry) => return entry,
    };
    match text(&v["version"]) {
        Some(version) => Entry::ok(NAME, PACK_MANIFEST, version)
            .detail(v["name"].as_str().unwrap_or_default().to_owned()),
        None => Entry::new(
            NAME,
            PACK_MANIFEST,
            Status::Unreadable,
            format!("{PACK_MANIFEST} has no version"),
        ),
    }
}

fn os_image(cx: &Context) -> Entry {
    const NAME: &str = "os-image";
    const SOURCE: &str = "/etc/os-release";
    match cupertino_build_info::read_os_image(cx.root()) {
        Ok(image) if !image.is_empty() => {
            let version = [&image.image_version, &image.version_id, &image.build_id]
                .into_iter()
                .find_map(|v| v.clone());
            let detail = [&image.name, &image.image_id, &image.id]
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            Entry {
                version,
                ..Entry::new(NAME, SOURCE, Status::Ok, detail)
            }
        }
        Ok(_) => Entry::new(NAME, SOURCE, Status::Unreadable, "os-release names nothing"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Entry::new(
            NAME,
            SOURCE,
            Status::NotInstalled,
            "neither /etc/os-release nor /usr/lib/os-release exists",
        ),
        Err(e) => Entry::new(
            NAME,
            SOURCE,
            Status::Unreadable,
            format!("cannot read os-release: {e}"),
        ),
    }
}

fn kernel(cx: &Context) -> Entry {
    const NAME: &str = "kernel";
    let source = format!("/{}", cupertino_build_info::KERNEL_RELEASE_FILE);
    match cupertino_build_info::kernel_in(cx.root()) {
        Some(release) => Entry::ok(NAME, source, release),
        None => Entry::new(
            NAME,
            source.clone(),
            Status::Unreadable,
            format!("cannot read {source}"),
        ),
    }
}
