//! The `shell.dev_folder` check: a build of the shell host in the development folder that is older than the installed
//! host starts instead of it for the components the folder names, and the session agent is never one of them.

use cupertino_diagnostics::{run, Area, Sources, Status};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A machine under a directory of its own: the system file names the components, and its folder `/dev` holds a build.
fn machine(name: &str, build_age: Duration) -> Sources {
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
    Sources {
        root,
        ..Sources::system()
    }
}

fn set_modified(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn dev_folder(sources: &Sources) -> cupertino_diagnostics::Check {
    run(sources, &[Area::Shell])
        .checks
        .into_iter()
        .find(|c| c.id == "shell.dev_folder")
        .expect("the check runs")
}

#[test]
fn an_older_host_in_the_folder_warns_with_the_command_for_each_component_that_runs_from_it() {
    let sources = machine("dev-folder-older", Duration::from_secs(3600));
    let check = dev_folder(&sources);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("dock"), "{}", check.summary);
    assert!(
        !check.summary.contains("menubar") && !check.summary.contains("agent"),
        "a component turned off and the agent run in the installed host: {}",
        check.summary
    );
    assert_eq!(
        check
            .fix
            .as_deref()
            .map(|f| f.contains("use-installed dock")),
        Some(true)
    );
    assert!(check
        .facts
        .iter()
        .any(|f| f.value.contains("/dev/cupertino-shell")));
}

#[test]
fn a_build_as_new_as_the_installed_host_is_no_problem() {
    let sources = machine("dev-folder-new", Duration::ZERO);
    // the installed host was written first in the setup, so the build is not older
    let check = dev_folder(&sources);
    assert_eq!(check.status, Status::Ok, "{check:?}");
}
