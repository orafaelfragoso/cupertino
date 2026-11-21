//! `cupertino diagnose` runs the checks of a machine: here the `shell.dev_folder` check, where a build of the shell host in
//! the development folder that is older than the installed host starts instead of it for the components the folder
//! names, and the session agent is never one of them.

use cupertino_cli::{Context, Runner, REGISTRY};
use serde_json::Value;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A machine under a directory of its own: the system file names the components, and its folder `/dev` holds a build.
fn machine(name: &str, build_age: Duration) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&root);
    let folder = root.join("dev");
    fs::create_dir_all(&folder).unwrap();
    fs::create_dir_all(root.join("etc/cupertino")).unwrap();
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(
        root.join("etc/cupertino/compositor.json"),
        r#"{"shell": {"components": ["dock", "menubar", "agent"], "off": ["menubar"]},
            "dev_folder": {"path": "/dev", "components": ["dock", "menubar", "agent"]}}"#,
    )
    .unwrap();
    let installed = root.join("usr/bin/cupertino-shell");
    fs::write(&installed, b"installed").unwrap();
    let build = folder.join("cupertino-shell");
    fs::write(&build, b"build").unwrap();
    set_modified(&build, SystemTime::now() - build_age);
    root
}

fn set_modified(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// The `shell.dev_folder` check of `cupertino --format json diagnose --area shell` on the machine under `root`.
fn dev_folder(root: &Path) -> Value {
    let args: Vec<String> = ["--format", "json", "diagnose", "--area", "shell"]
        .into_iter()
        .map(String::from)
        .collect();
    let output = Runner::new(REGISTRY)
        .context(Context::isolated(root))
        .run(&args, "diagnose-test");
    assert_eq!(output.code, 0, "the checks ran: {output:?}");
    let envelope: Value = serde_json::from_str(&output.stdout)
        .unwrap_or_else(|e| panic!("JSON on stdout ({e}): {output:?}"));
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["data"]["schema"], 1, "{envelope}");
    envelope["data"]["checks"]
        .as_array()
        .expect("the report's checks")
        .iter()
        .find(|c| c["id"] == "shell.dev_folder")
        .cloned()
        .unwrap_or_else(|| panic!("the check runs: {envelope}"))
}

#[test]
fn the_dev_folder_check_warns_of_an_older_host_for_each_component_that_runs_from_it() {
    // (machine, age of the build, status, whether the summary and fix name the dock)
    let cases = [
        ("dev-folder-older", Duration::from_secs(3600), "warn", true),
        // the installed host was written first in the setup, so the build is not older
        ("dev-folder-new", Duration::ZERO, "ok", false),
    ];
    for (name, age, status, names_dock) in cases {
        let check = dev_folder(&machine(name, age));
        assert_eq!(check["status"], status, "{name}: {check}");
        let summary = check["summary"].as_str().unwrap();
        assert_eq!(summary.contains("dock"), names_dock, "{name}: {summary}");
        assert!(
            !summary.contains("menubar") && !summary.contains("agent"),
            "{name}: a component turned off and the agent run in the installed host: {summary}"
        );
        let fix = check["fix"].as_str().unwrap_or_default();
        assert_eq!(
            fix.contains("use-installed dock"),
            names_dock,
            "{name}: {fix}"
        );
        if names_dock {
            assert!(
                check["facts"].as_array().unwrap().iter().any(|f| f["value"]
                    .as_str()
                    .unwrap()
                    .contains("/dev/cupertino-shell")),
                "{name}: {check}"
            );
        }
    }
}
