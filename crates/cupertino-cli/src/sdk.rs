//! `cupertino sdk`: the C SDK of CupertinoKit as the `cupertino-kit` package installs it. `info` reports its files,
//! `check` builds and runs a probe against it with the system's compiler, and `new` writes a starter project.

use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Sink, Stability,
    ValueHint,
};
use serde::Serialize;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const GROUP: Group = Group {
    name: "sdk",
    summary: "The C SDK of CupertinoKit: what is installed, whether it builds, and new projects",
    description: "The C SDK is the cupertino-kit package: the shared library libcupertinokit.so.1, its header \
                  /usr/include/cupertinokit-1/cupertinokit.h and the pkg-config file cupertinokit.pc. A C program \
                  builds against it with `pkg-config cupertinokit` alone.\n\n\
                  These commands read the installed SDK, check that the system's compiler builds and runs a program \
                  against it, and start a project in C or Rust.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[
    Command {
        path: &["sdk", "info"],
        summary: "Reports the installed C SDK: its header, library, links, pkg-config file and package",
        description: "Reads the files of the installed C SDK and reports each one: the header and the ABI version it \
                      defines, the library behind the soname link and the soname it carries, the link-time name \
                      libcupertinokit.so, the pkg-config file and its Version, and the build record of the \
                      cupertino-kit package. A file that is missing is not_installed and one that cannot be read or \
                      parsed is unreadable; files that disagree (a header of another version than the pkg-config \
                      file, a library of another soname) are listed as problems.\n\n\
                      It exits 0 whatever it finds. With --require it fails when the SDK is not wholly installed \
                      (unavailable, sdk_not_installed, exit 3) or when its files are unreadable or disagree \
                      (validation, sdk_invalid, exit 5). --root reads an SDK installed under another directory, such \
                      as a package's staging tree.",
        examples: &[
            ("cupertino sdk info", "Show the installed SDK."),
            (
                "cupertino --format json sdk info --require",
                "Report the SDK as JSON, and fail unless it is installed and consistent.",
            ),
            (
                "cupertino sdk info --root ./pkg",
                "Report the SDK installed under ./pkg instead of /.",
            ),
        ],
        stability: Stability::Stable,
        args: &[
            Arg::flag(
                "require",
                "fail unless every file is installed, readable and consistent",
            ),
            Arg::option(
                "root",
                "the directory the SDK is installed under, instead of /",
            )
            .hint(ValueHint::Dir),
        ],
        run: Run::Once(info),
    },
    Command {
        path: &["sdk", "check"],
        summary: "Builds and runs a small C program against the installed SDK with the system's compiler",
        description: "Compiles a probe program built into this command against the installed SDK with /usr/bin/cc, \
                      taking its flags from /usr/bin/pkg-config, then runs it: it prints the ABI version of the \
                      header and of the library and starts and releases a headless runtime. Each step is reported as \
                      it finishes. Only /usr/bin is searched for the tools, never PATH.\n\n\
                      Without a compiler it fails with unavailable, no_compiler (exit 3); without pkg-config with \
                      no_pkg_config, and when pkg-config finds no cupertinokit with sdk_not_installed (both exit 3). \
                      A compile error is validation, compile_failed (exit 5), with the compiler's messages in the \
                      data; a header and library of different ABI versions are abi_mismatch, and a probe that fails \
                      is probe_failed (both exit 5). --root checks an SDK installed under another directory: \
                      pkg-config reads it as a sysroot and the probe loads the library from there.",
        examples: &[
            (
                "cupertino sdk check",
                "Check that C programs build and run against the installed SDK.",
            ),
            (
                "cupertino --format jsonl sdk check",
                "Report each step as a JSON line.",
            ),
        ],
        stability: Stability::Stable,
        args: &[Arg::option(
            "root",
            "the directory the SDK is installed under, instead of /",
        )
        .hint(ValueHint::Dir)],
        run: Run::Stream(check),
    },
    Command {
        path: &["sdk", "new"],
        summary: "Writes a starter project of a CupertinoKit application in C or Rust",
        description: "Writes a new project into the directory <name> (or --dir): for --lang c a main.c, a Makefile \
                      that takes its flags from pkg-config cupertinokit, the desktop entry <name>.desktop and a \
                      README; for --lang rust a Cargo.toml depending on the cupertino-kit crate of this release (the \
                      git tag v<version> of the Cupertino repository, or the checkout --source names), a src/main.rs \
                      with a window that counts, the desktop entry and a README. Files are written with mode 0644 and \
                      directories with 0755, and no file that exists is replaced.\n\n\
                      <name> is the application's id, the name of its desktop entry: letters, digits, `-` and `_`, in \
                      elements separated by dots, none starting with a digit, and not starting with `-`. Another name \
                      is invalid_usage, invalid_name (exit 2). --source that is not a Cupertino checkout, or is given \
                      for C, is invalid_usage, invalid_value. A directory that exists and is not empty is validation, \
                      directory_not_empty (exit 5); one that cannot be written is denied, not_writable (exit 4).",
        examples: &[
            (
                "cupertino sdk new --lang c demo && make -C demo",
                "Start a C project in ./demo and build it.",
            ),
            (
                "cupertino sdk new --lang rust org.example.Counter --dir counter",
                "Start a Rust project for the app id org.example.Counter in ./counter.",
            ),
            (
                "cupertino sdk new --lang rust demo --source ~/src/cupertino",
                "Start a Rust project on the cupertino-kit of a checkout: offline, or before its release is tagged.",
            ),
        ],
        stability: Stability::Preview,
        args: &[
            Arg::option("lang", "the language of the project").values(&["c", "rust"]),
            Arg::positional("name", "the application's id, the name of its desktop entry"),
            Arg::option(
                "dir",
                "the directory to write the project into, instead of ./<name>",
            )
            .hint(ValueHint::Dir),
            Arg::option(
                "source",
                "a Cupertino checkout to take cupertino-kit from (--lang rust), instead of the release's git tag",
            )
            .hint(ValueHint::Dir),
        ],
        run: Run::Once(new),
    },
];

/// Where the installed SDK's files are, as absolute paths of the system.
const HEADER: &str = "/usr/include/cupertinokit-1/cupertinokit.h";
const LIBRARY: &str = "/usr/lib/libcupertinokit.so.1";
const LINK: &str = "/usr/lib/libcupertinokit.so";
const PC_FILE: &str = "/usr/lib/pkgconfig/cupertinokit.pc";
const BUILD_INFO: &str = "/usr/share/cupertino-kit/build-info.json";
/// The tools `check` runs; PATH is never searched.
const CC: &str = "/usr/bin/cc";
const PKG_CONFIG: &str = "/usr/bin/pkg-config";
/// How long the compiler and the probe may take.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a running tool is checked for an exit or a stop request.
const WAIT_STEP: Duration = Duration::from_millis(20);

const PROBE: &str = include_str!("../templates/check/probe.c");

/// The directory the SDK is read under: `--root` (relative to the working directory), else the context's root.
fn sdk_root(args: &Args, cx: &Context) -> Result<PathBuf, Failure> {
    let Some(root) = args.value("root") else {
        return Ok(cx.root().to_path_buf());
    };
    let root = Path::new(root);
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        let cwd = cx.current_dir().map_err(|e| {
            Failure::new(
                FailureKind::Unavailable,
                "no_working_directory",
                format!("cannot read the working directory: {e}"),
            )
        })?;
        cwd.join(root)
    };
    if !root.is_dir() {
        return Err(Failure::usage(
            "invalid_value",
            format!("--root {}: not a directory", root.display()),
        ));
    }
    Ok(root)
}

/// `absolute` under `root`.
fn under(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    NotInstalled,
    Unreadable,
}

impl Status {
    fn text(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NotInstalled => "not installed",
            Self::Unreadable => "unreadable",
        }
    }
}

/// One file of the SDK as `info` found it.
#[derive(Serialize)]
struct Part {
    name: &'static str,
    path: &'static str,
    status: Status,
    /// What a link points to, as written in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// The ABI version of the header (`1.0`), the pkg-config Version (`1.0.0`) or the package version.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    soname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    /// Why the file is unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl Part {
    fn new(name: &'static str, path: &'static str, status: Status) -> Self {
        Self {
            name,
            path,
            status,
            target: None,
            version: None,
            soname: None,
            commit: None,
            detail: None,
        }
    }

    fn unreadable(name: &'static str, path: &'static str, detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
            ..Self::new(name, path, Status::Unreadable)
        }
    }

    /// The part for an error reading its file: missing is not installed, anything else unreadable.
    fn from_error(name: &'static str, path: &'static str, e: &io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            Self::new(name, path, Status::NotInstalled)
        } else {
            Self::unreadable(name, path, e.to_string())
        }
    }
}

#[derive(Serialize)]
struct Info {
    /// Every file is there and readable, and they agree.
    installed: bool,
    /// The directory the SDK was read under.
    root: String,
    /// The ABI version the header defines, `1.0`.
    abi: Option<String>,
    soname: Option<String>,
    pkg_config_version: Option<String>,
    package_version: Option<String>,
    parts: Vec<Part>,
    /// Files that disagree with each other.
    problems: Vec<String>,
}

/// `#define CUPERTINOKIT_ABI_MAJOR 1` and `..._MINOR 0` of a header.
fn header_version(text: &str) -> Option<(u32, u32)> {
    let define = |name: &str| {
        text.lines().find_map(|line| {
            let rest = line.strip_prefix("#define ")?.strip_prefix(name)?;
            rest.strip_prefix(' ')?.trim().parse::<u32>().ok()
        })
    };
    Some((
        define("CUPERTINOKIT_ABI_MAJOR")?,
        define("CUPERTINOKIT_ABI_MINOR")?,
    ))
}

fn read_header(root: &Path) -> (Part, Option<(u32, u32)>) {
    match fs::read_to_string(under(root, HEADER)) {
        Err(e) => (Part::from_error("header", HEADER, &e), None),
        Ok(text) => match header_version(&text) {
            None => (
                Part::unreadable(
                    "header",
                    HEADER,
                    "it defines no CUPERTINOKIT_ABI_MAJOR and CUPERTINOKIT_ABI_MINOR",
                ),
                None,
            ),
            Some((major, minor)) => (
                Part {
                    version: Some(format!("{major}.{minor}")),
                    ..Part::new("header", HEADER, Status::Ok)
                },
                Some((major, minor)),
            ),
        },
    }
}

/// The soname of a little-endian ELF64 shared library: the DT_SONAME entry of its dynamic section, read through the
/// section headers.
fn elf_soname(path: &Path) -> Result<Option<String>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let read = |offset: u64, len: usize| -> Result<Vec<u8>, String> {
        let mut buf = vec![0; len];
        file.read_exact_at(&mut buf, offset)
            .map_err(|_| "it is cut short".to_string())?;
        Ok(buf)
    };
    let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let u64_at = |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let header = read(0, 64)?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err("it is not a little-endian 64-bit ELF file".into());
    }
    let (shoff, shentsize, shnum) = (
        u64_at(&header, 0x28),
        u64::from(u16_at(&header, 0x3a)),
        u64::from(u16_at(&header, 0x3c)),
    );
    if shentsize < 64 || shnum > 4096 {
        return Err("its section headers are not valid".into());
    }
    // every offset comes from the file, so each sum is checked: a corrupt file is an error, never an overflow
    let sum = |base: u64, offset: u64| {
        base.checked_add(offset)
            .ok_or_else(|| "its offsets are not valid".to_string())
    };
    let section = |index: u64| read(sum(shoff, index * shentsize)?, 64);
    for index in 0..shnum {
        let s = section(index)?;
        const SHT_DYNAMIC: u32 = 6;
        if u32_at(&s, 4) != SHT_DYNAMIC {
            continue;
        }
        let (offset, size, link) = (u64_at(&s, 0x18), u64_at(&s, 0x20), u32_at(&s, 0x28));
        if size > 1 << 20 {
            return Err("its dynamic section is too large".into());
        }
        let strings = section(u64::from(link))?;
        let (str_offset, str_size) = (u64_at(&strings, 0x18), u64_at(&strings, 0x20));
        let dynamic = read(offset, size as usize)?;
        const DT_SONAME: u64 = 14;
        for entry in dynamic.as_chunks::<16>().0 {
            match u64_at(entry, 0) {
                0 => break,
                DT_SONAME => {
                    let at = u64_at(entry, 8);
                    if at >= str_size {
                        return Err("its soname is outside the string table".into());
                    }
                    let bytes = read(sum(str_offset, at)?, (str_size - at).min(4096) as usize)?;
                    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    return String::from_utf8(bytes[..end].to_vec())
                        .map(Some)
                        .map_err(|_| "its soname is not UTF-8".into());
                }
                _ => {}
            }
        }
        return Ok(None);
    }
    Ok(None)
}

fn read_library(root: &Path) -> Part {
    let path = under(root, LIBRARY);
    let target = match fs::read_link(&path) {
        Ok(target) => Some(target.to_string_lossy().into_owned()),
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => None,
        Err(e) => return Part::from_error("library", LIBRARY, &e),
    };
    // a file named libcupertinokit.so.1.<minor>.0 holds the library's own version in its name
    let version = target
        .as_deref()
        .and_then(|t| t.strip_prefix("libcupertinokit.so."))
        .map(str::to_owned);
    match elf_soname(&path) {
        Ok(soname) => Part {
            target,
            version,
            soname,
            ..Part::new("library", LIBRARY, Status::Ok)
        },
        Err(detail) => {
            let missing = !path.exists();
            Part {
                target,
                ..if missing {
                    Part::new("library", LIBRARY, Status::NotInstalled)
                } else {
                    Part::unreadable("library", LIBRARY, detail)
                }
            }
        }
    }
}

fn read_link(root: &Path) -> Part {
    let path = under(root, LINK);
    match fs::read_link(&path) {
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            Part::unreadable("link", LINK, "it is a file, not a link to the soname")
        }
        Err(e) => Part::from_error("link", LINK, &e),
        Ok(target) => Part {
            target: Some(target.to_string_lossy().into_owned()),
            ..if path.exists() {
                Part::new("link", LINK, Status::Ok)
            } else {
                Part::unreadable("link", LINK, "it points to nothing")
            }
        },
    }
}

/// The `Version:` of a pkg-config file.
fn pc_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix("Version:")?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn read_pc(root: &Path) -> Part {
    match fs::read_to_string(under(root, PC_FILE)) {
        Err(e) => Part::from_error("pkg_config", PC_FILE, &e),
        Ok(text) => match pc_version(&text) {
            None => Part::unreadable("pkg_config", PC_FILE, "it has no Version"),
            Some(version) => Part {
                version: Some(version),
                ..Part::new("pkg_config", PC_FILE, Status::Ok)
            },
        },
    }
}

/// The build record `release.py` writes into the package: `{package, version, commit, source_date_epoch}`.
#[derive(serde::Deserialize)]
struct BuildRecord {
    package: String,
    version: String,
    commit: Option<String>,
}

fn read_package(root: &Path) -> Part {
    match fs::read(under(root, BUILD_INFO)) {
        Err(e) => Part::from_error("package", BUILD_INFO, &e),
        Ok(bytes) => match serde_json::from_slice::<BuildRecord>(&bytes) {
            Ok(record) if record.package == "cupertino-kit" => Part {
                version: Some(record.version),
                commit: record.commit,
                ..Part::new("package", BUILD_INFO, Status::Ok)
            },
            Ok(record) => Part::unreadable(
                "package",
                BUILD_INFO,
                format!("it is the record of {}, not cupertino-kit", record.package),
            ),
            Err(e) => Part::unreadable(
                "package",
                BUILD_INFO,
                format!("it is not a build record: {e}"),
            ),
        },
    }
}

/// What `info` reports about the SDK under `root`.
fn inspect(root: &Path) -> Info {
    let (header, abi) = read_header(root);
    let library = read_library(root);
    let link = read_link(root);
    let pc = read_pc(root);
    let package = read_package(root);
    let mut problems = Vec::new();
    if let Some((major, minor)) = abi {
        let soname = format!("libcupertinokit.so.{major}");
        if library.status == Status::Ok && library.soname.as_deref() != Some(soname.as_str()) {
            problems.push(format!(
                "the library's soname is {}, the header's ABI {major}.{minor} needs {soname}",
                library.soname.as_deref().unwrap_or("missing")
            ));
        }
        let file = format!("{major}.{minor}.0");
        if library.status == Status::Ok && library.version.as_deref() != Some(file.as_str()) {
            problems.push(format!(
                "{LIBRARY} points to {}, the header's ABI {major}.{minor} is libcupertinokit.so.{file}",
                library.target.as_deref().unwrap_or("no versioned file")
            ));
        }
        if pc.status == Status::Ok && pc.version.as_deref() != Some(file.as_str()) {
            problems.push(format!(
                "the pkg-config Version is {}, the header's ABI {major}.{minor} is {file}",
                pc.version.as_deref().unwrap_or("")
            ));
        }
    }
    if library.status == Status::Ok
        && link.status == Status::Ok
        && fs::canonicalize(under(root, LINK)).ok() != fs::canonicalize(under(root, LIBRARY)).ok()
    {
        problems.push(format!("{LINK} and {LIBRARY} are not the same library"));
    }
    let parts = vec![header, library, link, pc, package];
    Info {
        installed: parts.iter().all(|p| p.status == Status::Ok) && problems.is_empty(),
        root: root.display().to_string(),
        abi: abi.map(|(major, minor)| format!("{major}.{minor}")),
        soname: parts[1].soname.clone(),
        pkg_config_version: parts[3].version.clone(),
        package_version: parts[4].version.clone(),
        parts,
        problems,
    }
}

fn info_text(info: &Info) -> String {
    let state = if info.installed {
        "installed".to_string()
    } else if info.parts.iter().all(|p| p.status == Status::NotInstalled) {
        "not installed (install the cupertino-kit package)".to_string()
    } else if info.parts.iter().any(|p| p.status == Status::NotInstalled) {
        "partly installed".to_string()
    } else {
        "installed with problems".to_string()
    };
    let mut text = format!("CupertinoKit C SDK: {state}");
    if let Some(abi) = &info.abi {
        let _ = write!(text, ", ABI {abi}");
    }
    text.push('\n');
    for part in &info.parts {
        let label = match part.name {
            "pkg_config" => "pkg-config",
            name => name,
        };
        let _ = write!(text, "  {label:<11} {}", part.path);
        if let Some(target) = &part.target {
            let _ = write!(text, " -> {target}");
        }
        let _ = match part.status {
            Status::Ok => match part.name {
                "header" => write!(text, ": ABI {}", part.version.as_deref().unwrap_or("")),
                "library" => write!(
                    text,
                    ": soname {}",
                    part.soname.as_deref().unwrap_or("none")
                ),
                "pkg_config" => write!(text, ": version {}", part.version.as_deref().unwrap_or("")),
                "package" => {
                    let commit = part.commit.as_deref().map_or(String::new(), |c| {
                        format!(" ({})", c.get(..12).unwrap_or(c))
                    });
                    write!(
                        text,
                        ": cupertino-kit {}{commit}",
                        part.version.as_deref().unwrap_or("")
                    )
                }
                _ => Ok(()),
            },
            Status::NotInstalled | Status::Unreadable => {
                let detail = part
                    .detail
                    .as_deref()
                    .map_or(String::new(), |d| format!(" ({d})"));
                write!(text, ": {}{detail}", part.status.text())
            }
        };
        text.push('\n');
    }
    for problem in &info.problems {
        let _ = writeln!(text, "  problem: {problem}");
    }
    text
}

fn info(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let root = sdk_root(args, cx)?;
    let info = inspect(&root);
    if args.flag("require") && !info.installed {
        let missing: Vec<&str> = info
            .parts
            .iter()
            .filter(|p| p.status == Status::NotInstalled)
            .map(|p| p.path)
            .collect();
        let failure = if !missing.is_empty() {
            Failure::new(
                FailureKind::Unavailable,
                "sdk_not_installed",
                format!(
                    "the C SDK is not installed: {} missing; install the cupertino-kit package",
                    missing.join(", ")
                ),
            )
        } else {
            let mut problems: Vec<String> = info
                .parts
                .iter()
                .filter(|p| p.status == Status::Unreadable)
                .map(|p| format!("{} is unreadable", p.path))
                .collect();
            problems.extend(info.problems.iter().cloned());
            Failure::new(
                FailureKind::Validation,
                "sdk_invalid",
                format!(
                    "the installed C SDK is not consistent: {}",
                    problems.join("; ")
                ),
            )
        };
        return Err(failure.with_data(&info));
    }
    Ok(Reply::new(&info, info_text(&info)))
}

/// One step of `check` as it is reported.
#[derive(Serialize)]
struct Step {
    step: &'static str,
    ok: bool,
    #[serde(flatten)]
    detail: serde_json::Value,
}

#[derive(Serialize)]
struct Checked {
    ok: bool,
    root: String,
    abi: String,
    steps: Vec<Step>,
}

/// A tool `check` ran and what it printed.
struct Ran {
    success: bool,
    status: String,
    stdout: String,
    stderr: String,
}

/// A temporary directory removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(cx: &Context) -> io::Result<Self> {
        let base = cx
            .var("TMPDIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = base.join(format!(
            "cupertino-sdk-check-{}-{nanos:x}",
            std::process::id()
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The environment of the tools: nothing of the caller's but a fixed PATH, and the SDK's pkg-config directory (as a
/// sysroot when it is not under /).
fn tool(program: &Path, sdk: &Path, scratch: &Path) -> Process {
    let mut command = Process::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin")
        .env("LC_ALL", "C")
        .env("TMPDIR", scratch)
        .env("PKG_CONFIG_LIBDIR", under(sdk, "/usr/lib/pkgconfig"))
        .current_dir(scratch)
        .stdin(Stdio::null());
    if sdk != Path::new("/") {
        command.env("PKG_CONFIG_SYSROOT_DIR", sdk);
    }
    command
}

/// Runs a tool to its end, or until the timeout or a stop request kills it.
fn run_tool(mut command: Process, sink: &Sink, timeout: Duration) -> Result<Ran, Failure> {
    let name = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            Failure::new(
                FailureKind::Unavailable,
                "tool_failed",
                format!("cannot run {name}: {e}"),
            )
        })?;
    // the pipes are read on threads so that a tool that writes much never blocks
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let stdout = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| {
            Failure::new(
                FailureKind::Temporary,
                "tool_failed",
                format!("{name}: {e}"),
            )
        })? {
            break status;
        }
        if sink.stop_requested() || started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if sink.stop_requested() {
                Failure::interrupted()
            } else {
                Failure::new(
                    FailureKind::Temporary,
                    "timed_out",
                    format!("{name} did not finish in {} s", timeout.as_secs()),
                )
            });
        }
        std::thread::sleep(WAIT_STEP);
    };
    Ok(Ran {
        success: status.success(),
        status: status.to_string(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// A program of `/usr/bin` under the context's root that can be run.
fn executable(cx: &Context, absolute: &str) -> Option<PathBuf> {
    let path = cx.path(absolute);
    let meta = fs::metadata(&path).ok()?;
    (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(path)
}

/// `header 1.0` and the like: the version after `key`.
fn probe_version(output: &str, key: &str) -> Option<(u32, u32)> {
    output.lines().find_map(|line| {
        let (major, minor) = line.strip_prefix(key)?.strip_prefix(' ')?.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    })
}

/// The steps `check` finished, reported as they finish.
struct Steps(Vec<Step>);

impl Steps {
    fn report(&mut self, sink: &mut Sink, step: Step, line: &str) -> Result<(), Failure> {
        sink.event("step", &step, line)?;
        self.0.push(step);
        Ok(())
    }
}

fn check(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.json_result_only();
    let sdk = sdk_root(args, cx)?;
    let mut steps = Steps(Vec::new());

    let Some(cc) = executable(cx, CC) else {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "no_compiler",
            format!("there is no C compiler at {CC}: install gcc"),
        ));
    };
    steps.report(
        sink,
        Step {
            step: "compiler",
            ok: true,
            detail: serde_json::json!({ "path": CC }),
        },
        &format!("compiler: {CC}"),
    )?;
    let Some(pkg_config) = executable(cx, PKG_CONFIG) else {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "no_pkg_config",
            format!("there is no pkg-config at {PKG_CONFIG}: install pkgconf"),
        )
        .with_data(&serde_json::json!({ "steps": steps.0 })));
    };
    let scratch = Scratch::new(cx).map_err(|e| {
        Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot make a temporary directory: {e}"),
        )
    })?;

    let query = |sink: &Sink, flag: &str| -> Result<Ran, Failure> {
        let mut command = tool(&pkg_config, &sdk, &scratch.0);
        command.args([flag, "cupertinokit"]);
        run_tool(command, sink, PROBE_TIMEOUT)
    };
    let version = query(sink, "--modversion")?;
    if !version.success {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "sdk_not_installed",
            "pkg-config finds no cupertinokit: install the cupertino-kit package",
        )
        .with_data(&serde_json::json!({ "steps": steps.0, "stderr": version.stderr })));
    }
    let (cflags, libs) = (query(sink, "--cflags")?, query(sink, "--libs")?);
    if !cflags.success || !libs.success {
        return Err(Failure::new(
            FailureKind::Validation,
            "sdk_invalid",
            "pkg-config cannot give the flags of cupertinokit",
        )
        .with_data(&serde_json::json!({
            "steps": steps.0,
            "stderr": format!("{}{}", cflags.stderr, libs.stderr),
        })));
    }
    let pc_version = version.stdout.trim().to_owned();
    let (cflags, libs) = (
        cflags.stdout.trim().to_owned(),
        libs.stdout.trim().to_owned(),
    );
    steps.report(
        sink,
        Step {
            step: "pkg_config",
            ok: true,
            detail: serde_json::json!({
                "path": PKG_CONFIG,
                "version": pc_version,
                "cflags": cflags,
                "libs": libs,
            }),
        },
        &format!("pkg-config: cupertinokit {pc_version}: {cflags} {libs}"),
    )?;

    fs::write(scratch.0.join("probe.c"), PROBE).map_err(|e| {
        Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot write the probe: {e}"),
        )
    })?;
    let mut line = vec!["-std=c11".to_owned(), "-Wall".into(), "-Wextra".into()];
    line.extend(cflags.split_whitespace().map(str::to_owned));
    line.extend(["probe.c".into(), "-o".into(), "probe".into()]);
    line.extend(libs.split_whitespace().map(str::to_owned));
    let mut compile = tool(&cc, &sdk, &scratch.0);
    compile.args(&line);
    let command = format!("{CC} {}", line.join(" "));
    let compiled = run_tool(compile, sink, COMPILE_TIMEOUT)?;
    if !compiled.success {
        return Err(Failure::new(
            FailureKind::Validation,
            "compile_failed",
            format!(
                "the probe does not compile against the installed SDK ({}): {}",
                compiled.status,
                compiled.stderr.trim()
            ),
        )
        .with_data(&serde_json::json!({
            "steps": steps.0,
            "command": command,
            "status": compiled.status,
            "stderr": compiled.stderr,
        })));
    }
    steps.report(
        sink,
        Step {
            step: "compile",
            ok: true,
            detail: serde_json::json!({ "command": command, "warnings": compiled.stderr }),
        },
        &format!("compile: {command}"),
    )?;

    let mut probe = tool(&scratch.0.join("probe"), &sdk, &scratch.0);
    if sdk != Path::new("/") {
        probe.env("LD_LIBRARY_PATH", under(&sdk, "/usr/lib"));
    }
    let ran = run_tool(probe, sink, PROBE_TIMEOUT)?;
    let header = probe_version(&ran.stdout, "header");
    let library = probe_version(&ran.stdout, "library");
    let output = serde_json::json!({
        "steps": steps.0,
        "status": ran.status,
        "stdout": ran.stdout,
        "stderr": ran.stderr,
    });
    let (Some(header), Some(library)) = (header, library) else {
        return Err(Failure::new(
            FailureKind::Validation,
            "probe_failed",
            format!(
                "the probe did not run against the installed library ({}): {}",
                ran.status,
                ran.stderr.trim()
            ),
        )
        .with_data(&output));
    };
    let abi = format!("{}.{}", header.0, header.1);
    if header != library || pc_version != format!("{abi}.0") {
        return Err(Failure::new(
            FailureKind::Validation,
            "abi_mismatch",
            format!(
                "the header is ABI {abi}, the library {}.{} and the pkg-config file {pc_version}: they come from \
                 different SDKs",
                library.0, library.1
            ),
        )
        .with_data(&output));
    }
    if !ran.success || !ran.stdout.lines().any(|l| l == "runtime 0") {
        return Err(Failure::new(
            FailureKind::Validation,
            "probe_failed",
            format!(
                "the probe could not start a headless runtime ({}): {}",
                ran.status,
                ran.stderr.trim()
            ),
        )
        .with_data(&output));
    }
    steps.report(
        sink,
        Step {
            step: "run",
            ok: true,
            detail: serde_json::json!({ "header": abi, "library": abi, "stdout": ran.stdout }),
        },
        &format!("run: header and library are ABI {abi}; a headless runtime started and stopped"),
    )?;
    let human = format!("The C SDK builds and runs: ABI {abi}\n");
    let data = Checked {
        ok: true,
        root: sdk.display().to_string(),
        abi,
        steps: steps.0,
    };
    Ok(Reply::new(&data, human))
}

/// The starter project of a language: (path in the project, template text, whether it is named after the project).
const C_FILES: &[(&str, &str)] = &[
    ("main.c", include_str!("../templates/c/main.c")),
    ("Makefile", include_str!("../templates/c/Makefile")),
    ("README.md", include_str!("../templates/c/README.md")),
    (
        "{{name}}.desktop",
        include_str!("../templates/c/app.desktop"),
    ),
];
const RUST_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../templates/rust/Cargo.toml.in"),
    ),
    ("src/main.rs", include_str!("../templates/rust/src/main.rs")),
    ("README.md", include_str!("../templates/rust/README.md")),
    (".gitignore", include_str!("../templates/rust/gitignore")),
    (
        "{{name}}.desktop",
        include_str!("../templates/rust/app.desktop"),
    ),
];

/// Names Cargo refuses for a package: Rust's keywords and its own crates.
const CARGO_RESERVED: &[&str] = &[
    "alloc",
    "core",
    "proc_macro",
    "proc-macro",
    "std",
    "test",
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "gen",
    "macro",
    "override",
    "priv",
    "try",
    "typeof",
    "unsized",
    "virtual",
    "yield",
];

/// Why `name` is not a desktop-entry-safe application id: elements of letters, digits, `-` and `_` separated by
/// dots, none empty or starting with a digit, 255 bytes at most.
fn invalid_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.len() > 255 {
        return Some("it must have 1 to 255 characters");
    }
    // it names the program, a make target and a Cargo package, none of which may look like an option
    if name.starts_with('-') {
        return Some("it may not start with `-`");
    }
    for element in name.split('.') {
        let Some(first) = element.chars().next() else {
            return Some("its dot-separated elements may not be empty");
        };
        if first.is_ascii_digit() {
            return Some("no dot-separated element may start with a digit");
        }
        if !element
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Some("it may hold only letters, digits, `-`, `_` and dots");
        }
    }
    None
}

#[derive(Serialize)]
struct Created {
    lang: &'static str,
    name: String,
    dir: String,
    files: Vec<CreatedFile>,
    /// Where a Rust project takes cupertino-kit from: `{git, tag}` or `{path}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    kit: Option<serde_json::Value>,
}

/// The repository whose release tags a Rust project depends on.
const REPOSITORY: &str = "https://github.com/orafaelfragoso/cupertino";

/// The checkout `--source` names, made absolute, if it holds the crates a Rust project depends on.
fn kit_source(source: &str, cx: &Context) -> Result<String, Failure> {
    let path = Path::new(source);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cx.current_dir()
            .map_err(|e| {
                Failure::new(
                    FailureKind::Unavailable,
                    "no_working_directory",
                    format!("cannot read the working directory: {e}"),
                )
            })?
            .join(path)
    };
    let not_checkout = || {
        Failure::usage(
            "invalid_value",
            format!(
                "--source {source}: not a Cupertino checkout (it needs crates/cupertino-kit and \
                 crates/cupertino-xkbcommon)"
            ),
        )
    };
    let path = fs::canonicalize(&path).map_err(|_| not_checkout())?;
    if !["crates/cupertino-kit", "crates/cupertino-xkbcommon"]
        .iter()
        .all(|c| path.join(c).join("Cargo.toml").is_file())
    {
        return Err(not_checkout());
    }
    path.to_str().map(str::to_owned).ok_or_else(|| {
        Failure::usage(
            "invalid_value",
            format!("--source {source}: the path is not UTF-8, which Cargo.toml cannot hold"),
        )
    })
}

#[derive(Serialize)]
struct CreatedFile {
    path: String,
    mode: String,
}

const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;

fn write_failure(path: &Path, e: &io::Error) -> Failure {
    match e.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Failure::new(
            FailureKind::Denied,
            "not_writable",
            format!("cannot write {}: {e}", path.display()),
        ),
        _ => Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot write {}: {e}", path.display()),
        ),
    }
}

/// Creates `dir` and its missing parents with mode 0755, returning the directories it made, outermost first.
fn make_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut at = dir;
    while !at.exists() {
        missing.push(at.to_path_buf());
        match at.parent() {
            Some(parent) if parent != Path::new("") => at = parent,
            _ => break,
        }
    }
    missing.reverse();
    let mut made = Vec::new();
    for d in missing {
        match fs::DirBuilder::new().mode(DIR_MODE).create(&d) {
            Ok(()) => {
                fs::set_permissions(&d, fs::Permissions::from_mode(DIR_MODE))?;
                made.push(d);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && d.is_dir() => {}
            Err(e) => {
                undo(&[], &made);
                return Err(e);
            }
        }
    }
    Ok(made)
}

/// Removes what a failed `new` wrote: its files, then its directories, innermost first.
fn undo(files: &[PathBuf], dirs: &[PathBuf]) {
    for f in files {
        let _ = fs::remove_file(f);
    }
    for d in dirs.iter().rev() {
        let _ = fs::remove_dir(d);
    }
}

fn new(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let lang = args
        .value("lang")
        .ok_or_else(|| Failure::usage("missing_option", "sdk new needs --lang c or --lang rust"))?;
    let name = args.value("name").expect("<name> is a positional argument");
    if let Some(why) = invalid_name(name) {
        return Err(Failure::usage(
            "invalid_name",
            format!("{name:?} is not an application id: {why}"),
        ));
    }
    let crate_name = name.replace('.', "-").to_ascii_lowercase();
    if lang == "rust" && CARGO_RESERVED.contains(&crate_name.as_str()) {
        return Err(Failure::usage(
            "invalid_name",
            format!("{name:?} is a name Cargo keeps for Rust itself"),
        ));
    }
    let source = match args.value("source") {
        Some(_) if lang != "rust" => {
            return Err(Failure::usage(
                "invalid_value",
                "--source names the checkout of a Rust project's cupertino-kit; a C project uses the installed SDK",
            ))
        }
        Some(source) => Some(kit_source(source, cx)?),
        None => None,
    };
    let dir = args.value("dir").unwrap_or(name);
    let dir = Path::new(dir);
    let shown = dir.display().to_string();
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        cx.current_dir()
            .map_err(|e| {
                Failure::new(
                    FailureKind::Unavailable,
                    "no_working_directory",
                    format!("cannot read the working directory: {e}"),
                )
            })?
            .join(dir)
    };
    match fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.is_dir() => {
            return Err(Failure::new(
                FailureKind::Validation,
                "not_a_directory",
                format!("{shown} exists and is not a directory"),
            ))
        }
        Ok(_) => {
            let mut entries = fs::read_dir(&dir).map_err(|e| write_failure(&dir, &e))?;
            if entries.next().is_some() {
                return Err(Failure::new(
                    FailureKind::Validation,
                    "directory_not_empty",
                    format!("{shown} is not empty; give an empty or new directory with --dir"),
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(write_failure(&dir, &e)),
    }

    let (files, lang): (&[(&str, &str)], &'static str) = match lang {
        "c" => (C_FILES, "c"),
        _ => (RUST_FILES, "rust"),
    };
    let version = env!("CARGO_PKG_VERSION");
    // a Rust project's two dependencies on the repository: the release's tag, or the checkout's crates by path
    let (kit, kit_dependency, xkbcommon_dependency, kit_origin) = match &source {
        None => {
            let git = format!("{{ git = \"{REPOSITORY}\", tag = \"v{version}\" }}");
            (
                serde_json::json!({ "git": REPOSITORY, "tag": format!("v{version}") }),
                git.clone(),
                git,
                format!("the git tag v{version} of {REPOSITORY}"),
            )
        }
        Some(checkout) => {
            // a JSON string is a TOML basic string: quotes, backslashes and controls are escaped the same way
            let path = |krate: &str| {
                let at = format!("{checkout}/crates/{krate}");
                format!(
                    "{{ path = {} }}",
                    serde_json::to_string(&at).expect("a string serializes")
                )
            };
            (
                serde_json::json!({ "path": format!("{checkout}/crates/cupertino-kit") }),
                path("cupertino-kit"),
                path("cupertino-xkbcommon"),
                format!("the checkout {checkout}"),
            )
        }
    };
    let fill = |text: &str| {
        text.replace("{{kit_dependency}}", &kit_dependency)
            .replace("{{xkbcommon_dependency}}", &xkbcommon_dependency)
            .replace("{{kit_origin}}", &kit_origin)
            .replace("{{name}}", name)
            .replace("{{app_id}}", name)
            .replace("{{crate_name}}", &crate_name)
            .replace("{{version}}", version)
    };
    let mut made_dirs = make_dirs(&dir).map_err(|e| write_failure(&dir, &e))?;
    let mut written = Vec::new();
    let mut created = Vec::new();
    for (path, template) in files {
        let rel = fill(path);
        let out = dir.join(&rel);
        let result = (|| -> io::Result<()> {
            if let Some(parent) = out.parent() {
                made_dirs.extend(make_dirs(parent)?);
            }
            // never replaces a file: one that appeared since the directory was found empty stops the project
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(FILE_MODE)
                .open(&out)?;
            written.push(out.clone());
            file.write_all(fill(template).as_bytes())?;
            fs::set_permissions(&out, fs::Permissions::from_mode(FILE_MODE))
        })();
        if let Err(e) = result {
            undo(&written, &made_dirs);
            if e.kind() == io::ErrorKind::AlreadyExists {
                return Err(Failure::new(
                    FailureKind::Validation,
                    "directory_not_empty",
                    format!(
                        "{} appeared while the project was written; nothing was replaced",
                        out.display()
                    ),
                ));
            }
            return Err(write_failure(&out, &e));
        }
        created.push(CreatedFile {
            path: rel,
            mode: format!("{FILE_MODE:04o}"),
        });
    }
    // the program's path from the working directory: ./ only before a relative directory
    let program = if Path::new(&shown).is_absolute() {
        format!("{shown}/{name}")
    } else {
        format!("./{shown}/{name}")
    };
    let next = match lang {
        "c" => format!("make -C {shown} && {program}"),
        _ => format!("cargo run --manifest-path {shown}/Cargo.toml"),
    };
    let mut human = format!(
        "Wrote a {} project for {name} in {shown}:\n",
        if lang == "c" { "C" } else { "Rust" }
    );
    for f in &created {
        let _ = writeln!(human, "  {}", f.path);
    }
    if lang == "rust" {
        let _ = writeln!(human, "It takes cupertino-kit from {kit_origin}.");
        if source.is_none() {
            let _ = writeln!(
                human,
                "Offline, or before that tag is published, give --source with a Cupertino checkout instead."
            );
        }
    }
    let _ = writeln!(human, "Build and run it: {next}");
    let data = Created {
        lang,
        name: name.to_owned(),
        dir: shown,
        files: created,
        kit: (lang == "rust").then_some(kit),
    };
    Ok(Reply::new(&data, human))
}
