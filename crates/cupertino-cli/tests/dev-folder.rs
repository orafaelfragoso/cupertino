//! `cupertino dev-folder enable|disable` on a temporary root: the system file keeps every other key and its text, the
//! folder is created for the user who ran sudo, only eligible processes are accepted, and only root may run them. The
//! real `/etc` is never read or written.

use crate::support;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cupertino_cli::{Context, EffectiveUser, Output, Runner, REGISTRY};
use serde_json::Value;
use support::{args, envelope};

/// The packaged system file, with an unknown key and a setting that must keep their text.
const SYSTEM: &str = r#"{
  "appearance": "dark",
  "shell": [
    {"name": "dock", "command": ["/usr/bin/cupertino-dock"], "roles": ["session", "dock"]},
    {"name": "menubar", "command": ["/usr/bin/cupertino-menubar"], "roles": ["session", "menu_bar"]},
    {"name": "launcher", "command": ["/usr/bin/cupertino-launcher"]},
    {"name": "windowmanager", "command": ["/usr/bin/cupertino-windowmanager"], "roles": ["window_manager"]},
    {"name": "clipboard", "command": ["/usr/bin/clipboard"], "roles": ["session", "clipboard"]},
    {"name": "login", "command": ["/usr/bin/cupertino-login"], "roles": ["session", "session_lock"]}
  ],
  "future_key": {"kept":   true}
}
"#;

struct Machine {
    root: tempfile::TempDir,
    uid: u32,
    gid: u32,
}

impl Machine {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let etc = root.path().join("etc/cupertino");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::write(etc.join("compositor.json"), SYSTEM).unwrap();
        std::fs::set_permissions(
            etc.join("compositor.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let meta = std::fs::metadata(root.path()).unwrap();
        let (uid, gid) = (meta.uid(), meta.gid());
        let home = root.path().join("home/dev");
        std::fs::write(
            root.path().join("etc/passwd"),
            format!(
                "root:x:0:0::/root:/bin/sh\ndev:x:{uid}:{gid}::{}:/bin/sh\n",
                home.display()
            ),
        )
        .unwrap();
        Self { root, uid, gid }
    }

    fn system_file(&self) -> PathBuf {
        self.root.path().join("etc/cupertino/compositor.json")
    }

    fn system_text(&self) -> String {
        std::fs::read_to_string(self.system_file()).unwrap()
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home/dev")
    }

    /// Runs as `euid`, through sudo from the test's own account.
    fn run_as(&self, euid: u32, words: &[&str]) -> Output {
        let effective = EffectiveUser(euid);
        let cx = Context::isolated(self.root.path())
            .with_var("SUDO_UID", self.uid.to_string())
            .with_var("SUDO_GID", self.gid.to_string())
            .provide(&effective);
        Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
    }

    fn run(&self, words: &[&str]) -> Output {
        self.run_as(0, words)
    }
}

fn dev_folder(text: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap()["dev_folder"].clone()
}

#[test]
fn enable_declares_every_eligible_process_and_creates_the_folder_for_the_sudo_user() {
    let m = Machine::new();
    let v = envelope(&m.run(&["--format", "json", "dev-folder", "enable"]));
    assert_eq!(v["ok"], true, "{v}");
    let folder = m.home().join(".local/lib/cupertino/dev");
    assert_eq!(v["data"]["folder"], folder.display().to_string());
    assert_eq!(v["data"]["created"], true);
    assert_eq!(
        v["data"]["components"],
        serde_json::json!(["dock", "menubar", "launcher", "windowmanager"])
    );

    let text = m.system_text();
    let expected = SYSTEM.replace(
        "  \"future_key\": {\"kept\":   true}\n}",
        &format!(
            "  \"future_key\": {{\"kept\":   true}},\n  \"dev_folder\": {{\"path\": \"{}\", \"components\": [\"dock\", \"menubar\", \"launcher\", \"windowmanager\"]}}\n}}",
            folder.display()
        ),
    );
    assert_eq!(text, expected, "every other key keeps its text");
    let mode = std::fs::metadata(m.system_file()).unwrap().mode();
    assert_eq!(mode & 0o777, 0o644, "the file keeps its mode");

    let meta = std::fs::symlink_metadata(&folder).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.mode() & 0o777, 0o700);
    assert_eq!((meta.uid(), meta.gid()), (m.uid, m.gid));

    // again: the key is replaced, the folder kept
    let v = envelope(&m.run(&[
        "--format",
        "json",
        "dev-folder",
        "enable",
        "--components",
        "dock",
    ]));
    assert_eq!(v["data"]["created"], false);
    let text = m.system_text();
    assert_eq!(text.matches("dev_folder").count(), 1);
    assert_eq!(dev_folder(&text)["components"], serde_json::json!(["dock"]));
}

#[test]
fn enable_takes_a_folder_and_a_list_of_eligible_processes() {
    let m = Machine::new();
    let folder = m.root.path().join("elsewhere/dev");
    let out = m.run(&[
        "dev-folder",
        "enable",
        "--folder",
        folder.to_str().unwrap(),
        "--components",
        "menubar, dock,menubar",
    ]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("menubar, dock (created)"),
        "{}",
        out.stdout
    );
    let declared = dev_folder(&m.system_text());
    assert_eq!(declared["path"], folder.display().to_string());
    assert_eq!(
        declared["components"],
        serde_json::json!(["menubar", "dock"])
    );
}

#[test]
fn enable_refuses_what_could_never_start_from_the_folder() {
    let m = Machine::new();
    for (words, code) in [
        (&["--components", "login"][..], "ineligible_component"),
        (&["--components", "clipboard"][..], "ineligible_component"),
        (&["--components", "portal"][..], "unknown_component"),
        (&["--components", "dock,nope"][..], "unknown_component"),
        (&["--folder", "relative/dev"][..], "invalid_value"),
        (&["--folder", "/home/dev/../root"][..], "invalid_value"),
    ] {
        let mut line = vec!["--format", "json", "dev-folder", "enable"];
        line.extend_from_slice(words);
        let v = envelope(&m.run(&line));
        assert_eq!(v["error"]["code"], code, "{words:?}: {v}");
    }
    let shared = m.root.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    let v = envelope(&m.run(&[
        "--format",
        "json",
        "dev-folder",
        "enable",
        "--folder",
        shared.to_str().unwrap(),
    ]));
    assert_eq!(v["error"]["code"], "unsafe_folder");
    assert_eq!(m.system_text(), SYSTEM, "nothing was written");
}

#[test]
fn only_root_through_sudo_runs_them() {
    let m = Machine::new();
    for command in ["enable", "disable"] {
        let out = m.run_as(1000, &["dev-folder", command]);
        assert_eq!(out.code, 4, "{command}");
        assert!(
            out.stderr
                .contains(&format!("sudo cupertino dev-folder {command}")),
            "{}",
            out.stderr
        );
    }
    let effective = EffectiveUser(0);
    let out = Runner::new(REGISTRY)
        .context(Context::isolated(m.root.path()).provide(&effective))
        .run(&args(&["--format", "json", "dev-folder", "enable"]), "r1");
    assert_eq!(envelope(&out)["error"]["code"], "no_invoking_user");
    assert_eq!(m.system_text(), SYSTEM);
}

#[test]
fn disable_removes_the_key_alone_and_keeps_the_folder() {
    let m = Machine::new();
    assert_eq!(m.run(&["dev-folder", "enable"]).code, 0);
    let folder = m.home().join(".local/lib/cupertino/dev");
    std::fs::write(folder.join("cupertino-dock"), "build").unwrap();
    let v = envelope(&m.run(&["--format", "json", "dev-folder", "disable"]));
    assert_eq!(v["data"]["folder"], folder.display().to_string());
    assert_eq!(
        m.system_text(),
        SYSTEM,
        "the file is as it was before enable"
    );
    assert!(
        folder.join("cupertino-dock").exists(),
        "the folder's files stay"
    );

    let out = m.run(&["dev-folder", "disable"]);
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("declares no development folder"),
        "{}",
        out.stdout
    );
}

#[test]
fn a_system_file_that_is_missing_or_not_an_object_is_left_alone() {
    let m = Machine::new();
    std::fs::write(m.system_file(), "[1, 2]\n").unwrap();
    let v = envelope(&m.run(&["--format", "json", "dev-folder", "enable"]));
    assert_eq!(v["error"]["code"], "invalid_system_config");
    assert_eq!(m.system_text(), "[1, 2]\n");
    std::fs::write(m.system_file(), "{\"appearance\": \"dark\"}\n").unwrap();
    let v = envelope(&m.run(&["--format", "json", "dev-folder", "enable"]));
    assert_eq!(v["error"]["code"], "no_shell_list");
    std::fs::remove_file(m.system_file()).unwrap();
    let out = m.run(&["--format", "json", "dev-folder", "disable"]);
    assert_eq!(out.code, 3);
    assert_eq!(envelope(&out)["error"]["code"], "no_system_config");
    assert!(!Path::new(&m.system_file()).exists());
}
