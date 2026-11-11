//! `cupertino dev-folder enable|disable`: the one change as root that lets a developer try builds of the shell
//! processes in the running session without root afterwards. It declares (or removes) the development folder in the
//! system configuration, `/etc/cupertino/compositor.json`, editing that one key in place so every other key keeps its
//! text, and creates the folder for the user who ran `sudo`.

use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Stability, ValueHint,
};
use cupertino_defaults::compositor::CompositorSettings;
use serde::Serialize;
use serde_json::Value;
use std::ops::Range;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};

pub(crate) const GROUP: Group = Group {
    name: "dev-folder",
    summary: "Declares the development folder shell processes may start from, once, as root",
    description: "A development folder lets the person who develops the desktop try a build of a shell process (the \
                  dock, the menu bar, the launcher, the notifications, the control panel, the overlay, the window \
                  manager) in the running session without root: they copy the build into the folder and restart the \
                  process with `cupertino shell restart`. Only the system configuration can declare the folder, so \
                  these commands run as root, once.\n\n\
                  Processes holding the lock screen's, capture's, the authentication prompt's, data control's or \
                  desktop control's role never start from the folder, and neither does the server itself. The server \
                  starts the folder's file only when it is a regular file owned by the session's user or root that no \
                  one else can write, in a folder of the same kind.",
    commands: &[COMMANDS],
};

const COMMANDS: &[Command] = &[
    Command {
        path: &["dev-folder", "enable"],
        summary: "Declares the development folder in /etc/cupertino/compositor.json and creates it for the user who \
                  ran sudo",
        description: "Sets the dev_folder key of /etc/cupertino/compositor.json to the folder and the processes \
                      allowed to start from it, keeping the file's other keys and their text, and replaces the file \
                      atomically. It must run as root (not_root, exit code 4), through sudo from the developer's \
                      account: the folder is created for SUDO_UID and SUDO_GID with mode 0700 when it is missing \
                      (no_invoking_user, exit code 2, without them).\n\n\
                      The folder is --folder, an absolute path, else ~/.local/lib/cupertino/dev of that user. The \
                      processes are --components, names separated by commas, else every process of the file's shell \
                      list that may start from a development folder: one whose roles are all among session, menu_bar, \
                      dock and window_manager. A name that is not such a process fails with exit code 5 \
                      (unknown_component or ineligible_component), and so does an existing folder that another user \
                      owns or that its group or others can write (unsafe_folder). A file without a shell list, or \
                      that is not a JSON object, fails with exit code 5 too; a missing file with exit code 3 \
                      (no_system_config).\n\n\
                      A running server of this release reads the key again at once; it applies from each process's \
                      next start.",
        examples: &[
            (
                "cupertino dev-folder enable",
                "Through sudo: let every eligible shell process start from ~/.local/lib/cupertino/dev.",
            ),
            (
                "cupertino dev-folder enable --components dock,menubar",
                "Through sudo: let only the dock and the menu bar start from the folder.",
            ),
            (
                "cupertino dev-folder enable --folder /home/me/cupertino-dev",
                "Through sudo: use another folder.",
            ),
        ],
        stability: Stability::Preview,
        args: &[
            Arg::option("folder", "the absolute path of the folder, instead of ~/.local/lib/cupertino/dev")
                .hint(ValueHint::Dir),
            Arg::option(
                "components",
                "the names of the shell processes that may start from the folder, separated by commas",
            ),
        ],
        run: Run::Once(enable),
    },
    Command {
        path: &["dev-folder", "disable"],
        summary: "Removes the development folder from /etc/cupertino/compositor.json",
        description: "Removes the dev_folder key of /etc/cupertino/compositor.json, keeping the file's other keys \
                      and their text, so every shell process starts its installed program from its next start. The \
                      folder and its files stay. It must run as root (not_root, exit code 4). Without the key it \
                      changes nothing.",
        examples: &[(
            "cupertino dev-folder disable",
            "Through sudo: stop starting shell processes from the development folder.",
        )],
        stability: Stability::Preview,
        args: &[],
        run: Run::Once(disable),
    },
];

/// The system configuration file.
const SYSTEM_CONFIG: &str = "/etc/cupertino/compositor.json";

/// The key the commands change.
const KEY: &str = "dev_folder";

/// The default folder, under the developer's home.
const DEFAULT_FOLDER: &str = ".local/lib/cupertino/dev";

/// The mode of the folder the command creates: its owner's alone.
const FOLDER_MODE: &str = "0700";

/// The effective user id the commands take as theirs. A test hands one in with [`Context::provide`]; the real command
/// asks the kernel.
#[derive(Debug, Clone, Copy)]
pub struct EffectiveUser(pub u32);

fn require_root(cx: &Context, command: &str) -> Result<(), Failure> {
    // Safety: geteuid has no preconditions.
    let euid = cx
        .provided::<EffectiveUser>()
        .map_or_else(|| unsafe { libc::geteuid() }, |u| u.0);
    if euid == 0 {
        return Ok(());
    }
    Err(Failure::new(
        FailureKind::Denied,
        "not_root",
        format!(
            "`dev-folder {command}` changes {SYSTEM_CONFIG}, which only root may write: run `sudo cupertino \
             dev-folder {command}` from the account that develops"
        ),
    ))
}

/// The user who ran sudo: `SUDO_UID` and `SUDO_GID`.
fn invoking_user(cx: &Context) -> Result<(u32, u32), Failure> {
    let id = |name: &str| {
        cx.var(name)
            .and_then(|v| v.to_str().and_then(|v| v.parse::<u32>().ok()))
    };
    match (id("SUDO_UID"), id("SUDO_GID")) {
        (Some(0), _) => Err(Failure::usage(
            "no_invoking_user",
            "the development folder belongs to a person's account, not root's: run the command with sudo from that \
             account",
        )),
        (Some(uid), Some(gid)) => Ok((uid, gid)),
        _ => Err(Failure::usage(
            "no_invoking_user",
            "SUDO_UID and SUDO_GID do not name the account that develops: run the command with sudo from that account",
        )),
    }
}

/// The home directory of `uid`: its line in the machine's `/etc/passwd`, else the user database.
fn home_of(cx: &Context, uid: u32) -> Option<PathBuf> {
    let passwd = std::fs::read_to_string(cx.path("/etc/passwd")).unwrap_or_default();
    let listed = passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 7 && fields[2].parse() == Ok(uid)).then(|| PathBuf::from(fields[5]))
    });
    listed.or_else(|| {
        (cx.root() == Path::new("/"))
            .then(|| user_database_home(uid))
            .flatten()
    })
}

/// The home directory the user database (NSS) gives for `uid`.
fn user_database_home(uid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    /// Room for the strings of one entry.
    const ENTRY_BUFFER: usize = 16 * 1024;
    let mut buffer = vec![0u8; ENTRY_BUFFER];
    // Safety: an all-zero passwd is a valid value of the plain C struct; getpwuid_r fills it.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // Safety: the entry and the buffer outlive the call, and the buffer's length is passed with it.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // Safety: on success pw_dir points to a NUL-terminated string in `buffer`, which is still alive.
    let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

/// The text of the system file, and its settings when it is a JSON object.
fn read_system(cx: &Context) -> Result<(PathBuf, String), Failure> {
    let path = cx.path(SYSTEM_CONFIG);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Failure::new(
            FailureKind::Unavailable,
            "no_system_config",
            format!("cannot read {SYSTEM_CONFIG}: {e}"),
        )
    })?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(_)) => Ok((path, text)),
        Ok(_) | Err(_) => Err(Failure::new(
            FailureKind::Validation,
            "invalid_system_config",
            format!("{SYSTEM_CONFIG} is not a JSON object; it is left as it is"),
        )),
    }
}

/// Write `text` into the system file atomically, after checking that it changes nothing but [`KEY`].
fn write_system(path: &Path, before: &str, text: &str) -> Result<(), Failure> {
    let rest = |text: &str| {
        let mut value: Value = serde_json::from_str(text).ok()?;
        value.as_object_mut()?.remove(KEY);
        Some(value)
    };
    if rest(text).is_none() || rest(before) != rest(text) {
        return Err(invalid(format!(
            "an edit would change more than the {KEY} key"
        )));
    }
    cupertino_defaults::atomic::write_atomic(path, text.as_bytes()).map_err(|e| {
        Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot write {SYSTEM_CONFIG}: {e}"),
        )
    })
}

#[derive(Serialize)]
struct Enabled {
    file: &'static str,
    folder: PathBuf,
    components: Vec<String>,
    /// The user the folder belongs to.
    uid: u32,
    /// The folder was created now.
    created: bool,
}

fn enable(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    require_root(cx, "enable")?;
    let (uid, gid) = invoking_user(cx)?;
    let folder = match args.value("folder") {
        Some(folder) => {
            let folder = PathBuf::from(folder);
            if !folder.is_absolute() || folder.components().any(|c| c == Component::ParentDir) {
                return Err(Failure::usage(
                    "invalid_value",
                    format!("--folder takes an absolute path without `..`, not `{}`", folder.display()),
                ));
            }
            folder
        }
        None => home_of(cx, uid)
            .ok_or_else(|| {
                Failure::usage(
                    "no_home",
                    format!("user {uid} has no home directory to put the folder in: give one with --folder"),
                )
            })?
            .join(DEFAULT_FOLDER),
    };
    let (path, text) = read_system(cx)?;
    let settings = CompositorSettings::parse(&text, None).0;
    let entries = settings.shell.ok_or_else(|| {
        Failure::new(
            FailureKind::Validation,
            "no_shell_list",
            format!("{SYSTEM_CONFIG} has no valid shell list, so no process could start from the folder"),
        )
    })?;
    let eligible: Vec<&str> = entries
        .iter()
        .filter(|e| e.may_start_from_dev_folder())
        .map(|e| e.name.as_str())
        .collect();
    let components: Vec<String> = match args.value("components") {
        Some(list) => {
            let mut names: Vec<String> = Vec::new();
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let Some(entry) = entries.iter().find(|e| e.name == name) else {
                    return Err(Failure::new(
                        FailureKind::Validation,
                        "unknown_component",
                        format!(
                            "{SYSTEM_CONFIG} starts no shell process named {name:?}; these may start from the \
                             folder: {}",
                            eligible.join(", ")
                        ),
                    ));
                };
                if !entry.may_start_from_dev_folder() {
                    return Err(Failure::new(
                        FailureKind::Validation,
                        "ineligible_component",
                        format!(
                            "{name} holds a role that always starts its installed program; these may start from \
                             the folder: {}",
                            eligible.join(", ")
                        ),
                    ));
                }
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
            names
        }
        None => eligible.iter().map(|n| (*n).to_owned()).collect(),
    };
    if components.is_empty() {
        return Err(Failure::new(
            FailureKind::Validation,
            "no_eligible_component",
            format!("no shell process of {SYSTEM_CONFIG} may start from a development folder"),
        ));
    }
    let created = ensure_folder(&folder, uid, gid)?;
    let edited = with_key(&text, KEY, &folder_value(&folder, &components)).map_err(invalid)?;
    write_system(&path, &text, &edited)?;
    let human = format!(
        "{SYSTEM_CONFIG}: the development folder is {} for {}{}\n\
         Copy a build of one of them into the folder under its program's name, then run `cupertino shell restart \
         NAME`; `cupertino shell use-installed NAME` goes back.\n",
        folder.display(),
        components.join(", "),
        if created { " (created)" } else { "" },
    );
    Ok(Reply::new(
        &Enabled {
            file: SYSTEM_CONFIG,
            folder,
            components,
            uid,
            created,
        },
        human,
    ))
}

#[derive(Serialize)]
struct Disabled {
    file: &'static str,
    /// The folder the key named; `None` when the file had no key.
    folder: Option<String>,
}

fn disable(_: &Args, cx: &Context) -> Result<Reply, Failure> {
    require_root(cx, "disable")?;
    let (path, text) = read_system(cx)?;
    let declared: Option<Value> = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get(KEY).cloned());
    let Some(declared) = declared else {
        return Ok(Reply::new(
            &Disabled {
                file: SYSTEM_CONFIG,
                folder: None,
            },
            format!("{SYSTEM_CONFIG} declares no development folder; nothing changed\n"),
        ));
    };
    let edited = without_key(&text, KEY).map_err(invalid)?;
    write_system(&path, &text, &edited)?;
    let folder = declared
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let human = format!(
        "{SYSTEM_CONFIG}: the development folder{} is no longer used; its files stay. Every shell process starts \
         its installed program from its next start (`cupertino shell restart NAME` restarts one now).\n",
        folder.as_ref().map(|f| format!(" {f}")).unwrap_or_default()
    );
    Ok(Reply::new(
        &Disabled {
            file: SYSTEM_CONFIG,
            folder,
        },
        human,
    ))
}

fn invalid(message: String) -> Failure {
    Failure::new(
        FailureKind::Validation,
        "invalid_system_config",
        format!("{SYSTEM_CONFIG}: {message}; it is left as it is"),
    )
}

/// `{"path": "/x", "components": ["dock", "menubar"]}`: the key's value on one line, spaced as the packaged file writes
/// its entries.
fn folder_value(folder: &Path, components: &[String]) -> String {
    let names: Vec<String> = components
        .iter()
        .map(|n| Value::String(n.clone()).to_string())
        .collect();
    format!(
        "{{\"path\": {}, \"components\": [{}]}}",
        Value::String(folder.display().to_string()),
        names.join(", ")
    )
}

/// Make sure the folder is there for `uid`: create it with mode 0700 as that user when it is missing (so nothing in
/// the user's home is followed with root's rights), and check that it is a directory of the user or root that no one
/// else can write. Whether it was created now.
fn ensure_folder(folder: &Path, uid: u32, gid: u32) -> Result<bool, Failure> {
    let created = match std::fs::symlink_metadata(folder) {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mkdir = ["/usr/bin/mkdir", "/bin/mkdir"]
                .into_iter()
                .find(|p| Path::new(p).exists())
                .unwrap_or("mkdir");
            let out = std::process::Command::new(mkdir)
                .args(["-p", "-m", FOLDER_MODE, "--"])
                .arg(folder)
                .uid(uid)
                .gid(gid)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .current_dir("/")
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| {
                    Failure::new(
                        FailureKind::Denied,
                        "cannot_create_folder",
                        format!("cannot run {mkdir}: {e}"),
                    )
                })?;
            if !out.status.success() {
                return Err(Failure::new(
                    FailureKind::Denied,
                    "cannot_create_folder",
                    format!(
                        "user {uid} cannot create {}: {}",
                        folder.display(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                ));
            }
            true
        }
        Err(e) => {
            return Err(Failure::new(
                FailureKind::Denied,
                "cannot_create_folder",
                format!("{}: {e}", folder.display()),
            ))
        }
    };
    let meta = std::fs::symlink_metadata(folder).map_err(|e| {
        Failure::new(
            FailureKind::Denied,
            "cannot_create_folder",
            format!("{}: {e}", folder.display()),
        )
    })?;
    if !meta.is_dir() || !(meta.uid() == uid || meta.uid() == 0) || meta.mode() & 0o022 != 0 {
        return Err(Failure::new(
            FailureKind::Validation,
            "unsafe_folder",
            format!(
                "{} must be a directory (not a link) of user {uid} or root that its group and others cannot write; \
                 the server would never start a program from it",
                folder.display()
            ),
        ));
    }
    Ok(created)
}

/// A member of the top-level object: where its key's opening quote is and the span of its value.
#[derive(Debug, PartialEq, Eq)]
struct Member {
    key: String,
    start: usize,
    value: Range<usize>,
}

/// The positions of the top-level object's braces and its members, in order. `text` must be a JSON object.
fn members(text: &str) -> Result<(usize, usize, Vec<Member>), String> {
    let bytes = text.as_bytes();
    let mut at = skip_space(bytes, 0);
    if bytes.get(at) != Some(&b'{') {
        return Err("it is not a JSON object".into());
    }
    let open = at;
    at += 1;
    let mut found = Vec::new();
    loop {
        at = skip_space(bytes, at);
        match bytes.get(at) {
            Some(b'}') => return Ok((open, at, found)),
            Some(b',') if !found.is_empty() => at = skip_space(bytes, at + 1),
            Some(b'"') if found.is_empty() => {}
            _ => return Err(format!("unexpected text at byte {at}")),
        }
        let start = at;
        let key_end = skip_string(bytes, at)?;
        let key: String = serde_json::from_str(&text[start..key_end]).map_err(|e| e.to_string())?;
        at = skip_space(bytes, key_end);
        if bytes.get(at) != Some(&b':') {
            return Err(format!("no `:` after the key {key:?}"));
        }
        let value_start = skip_space(bytes, at + 1);
        let value_end = skip_value(bytes, value_start)?;
        found.push(Member {
            key,
            start,
            value: value_start..value_end,
        });
        at = value_end;
    }
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The end of the string whose opening quote is at `at`.
fn skip_string(bytes: &[u8], mut at: usize) -> Result<usize, String> {
    at += 1;
    while let Some(&b) = bytes.get(at) {
        match b {
            b'\\' => at += 2,
            b'"' => return Ok(at + 1),
            _ => at += 1,
        }
    }
    Err("a string does not end".into())
}

/// The end of the value that starts at `at`.
fn skip_value(bytes: &[u8], at: usize) -> Result<usize, String> {
    match bytes.get(at) {
        Some(b'"') => skip_string(bytes, at),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut i = at;
            while let Some(&b) = bytes.get(i) {
                match b {
                    b'"' => {
                        i = skip_string(bytes, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Err("a value does not end".into())
        }
        Some(_) => {
            let mut i = at;
            while bytes
                .get(i)
                .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
            {
                i += 1;
            }
            Ok(i)
        }
        None => Err("a value is missing".into()),
    }
}

/// `text` without the top-level member `key`, every other byte as it was.
fn without_key(text: &str, key: &str) -> Result<String, String> {
    let mut text = text.to_owned();
    loop {
        let (open, close, found) = members(&text)?;
        let Some(i) = found.iter().position(|m| m.key == key) else {
            return Ok(text);
        };
        let cut = if let Some(next) = found.get(i + 1) {
            found[i].start..next.start
        } else if i > 0 {
            found[i - 1].value.end..found[i].value.end
        } else {
            open + 1..close
        };
        text.replace_range(cut, "");
    }
}

/// `text` with the top-level member `key` set to the JSON `value`, last, indented as the member before it; every other
/// member keeps its text.
fn with_key(text: &str, key: &str, value: &str) -> Result<String, String> {
    let mut text = without_key(text, key)?;
    let (open, close, found) = members(&text)?;
    let member = format!("{}: {value}", Value::String(key.to_owned()));
    match found.last() {
        Some(last) => {
            let line_start = text[..last.start].rfind('\n').map_or(0, |n| n + 1);
            let indent = &text[line_start..last.start];
            let insert = if indent.chars().all(char::is_whitespace) && line_start > open {
                format!(",\n{indent}{member}")
            } else {
                format!(", {member}")
            };
            text.insert_str(last.value.end, &insert);
        }
        None => text.replace_range(open..close + 1, &format!("{{\n  {member}\n}}")),
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGED: &str = r#"{
  "shell": [
    {"name": "dock", "command": ["/usr/bin/cupertino-dock"], "roles": ["session", "dock"]},
    {"name": "login", "command": ["/usr/bin/cupertino-login"], "roles": ["session", "session_lock"]}
  ]
}
"#;

    #[test]
    fn a_key_is_added_last_with_the_indentation_of_the_others_and_nothing_else_changes() {
        let edited = with_key(
            PACKAGED,
            "dev_folder",
            r#"{"path": "/d", "components": ["dock"]}"#,
        )
        .unwrap();
        assert_eq!(
            edited,
            PACKAGED.replace(
                "  ]\n}",
                "  ],\n  \"dev_folder\": {\"path\": \"/d\", \"components\": [\"dock\"]}\n}"
            )
        );
        // set again: replaced, not added twice
        let again = with_key(
            &edited,
            "dev_folder",
            r#"{"path": "/e", "components": ["dock"]}"#,
        )
        .unwrap();
        assert_eq!(again, edited.replace("\"/d\"", "\"/e\""));
        assert_eq!(without_key(&again, "dev_folder").unwrap(), PACKAGED);
    }

    #[test]
    fn a_key_is_removed_wherever_it_is() {
        for (text, expected) in [
            (r#"{"dev_folder": {"a": "}"}, "b": 1}"#, r#"{"b": 1}"#),
            (r#"{"b": [1, "]"], "dev_folder": 2}"#, r#"{"b": [1, "]"]}"#),
            (
                r#"{"a": 1, "dev_folder": "x\"", "b": 2}"#,
                r#"{"a": 1, "b": 2}"#,
            ),
            (r#"{ "dev_folder" : true }"#, "{}"),
            (r#"{"dev_folder": 1, "dev_folder": 2}"#, "{}"),
            (r#"{"b": false}"#, r#"{"b": false}"#),
        ] {
            assert_eq!(without_key(text, "dev_folder").unwrap(), expected, "{text}");
        }
    }

    #[test]
    fn a_compact_or_empty_object_gets_the_key_inline_or_on_its_own_line() {
        assert_eq!(
            with_key(r#"{"a": 1}"#, "k", "2").unwrap(),
            r#"{"a": 1, "k": 2}"#
        );
        assert_eq!(with_key("{}\n", "k", "2").unwrap(), "{\n  \"k\": 2\n}\n");
        assert!(with_key("[1]", "k", "2").is_err());
    }

    #[test]
    fn a_value_is_written_on_one_line_as_the_packaged_file_writes_its_entries() {
        assert_eq!(
            folder_value(Path::new("/x \"y\""), &["dock".into(), "menubar".into()]),
            r#"{"path": "/x \"y\"", "components": ["dock", "menubar"]}"#
        );
    }
}
