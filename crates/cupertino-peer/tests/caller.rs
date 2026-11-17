//! Classifying the process behind a connection from the daemon's credentials, over a made-up `/proc`.

use cupertino_peer::{
    own_parent, process_fd_notice, same_instance, Caller, CallerError, Denial, Identified,
    ProcessHandle, Relation, Sandbox, NO_PROCESS_FD,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;
use zbus::fdo::ConnectionCredentials;
use zbus::DBusError;

fn own_pid() -> u32 {
    std::process::id()
}

/// A made-up `/proc` with a folder for each process given; each has an empty `root`.
fn proc_with(pids: &[u32]) -> TempDir {
    let proc = tempfile::tempdir().unwrap();
    for pid in pids {
        std::fs::create_dir_all(proc.path().join(pid.to_string()).join("root")).unwrap();
    }
    proc
}

fn flatpak_info(proc: &Path, pid: u32, text: &str) {
    std::fs::write(proc.join(pid.to_string()).join("root/.flatpak-info"), text).unwrap();
}

fn flatpak_text(app: &str, instance: &str) -> String {
    format!("[Application]\nname={app}\n\n[Instance]\ninstance-id={instance}\n")
}

fn by_pid(pid: u32) -> ConnectionCredentials {
    ConnectionCredentials::default().set_process_id(pid)
}

fn by_fd(pid: u32) -> ConnectionCredentials {
    let fd = ProcessHandle::open(pid)
        .unwrap()
        .fd()
        .try_clone_to_owned()
        .unwrap();
    ConnectionCredentials::default().set_process_fd(fd.into())
}

fn identified(pid: u32, sandbox: Option<Sandbox>) -> Identified {
    Identified {
        process: ProcessHandle::open(pid).unwrap(),
        sandbox,
        exe: None,
    }
}

fn process_of(caller: &Caller) -> &Identified {
    match caller {
        Caller::Process(p) => p,
        other => panic!("{other:?}"),
    }
}

/// The pid of a process that has exited and been reaped.
fn gone_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn credentials_name_the_process_only_when_they_identify_it_for_sure() {
    let gone = gone_pid();
    let proc = proc_with(&[own_pid(), own_parent(), gone]);
    let not_a_pidfd = || {
        let file = std::fs::File::open("/dev/null").unwrap();
        ConnectionCredentials::default().set_process_fd(std::os::fd::OwnedFd::from(file).into())
    };
    for (row, credentials, expected) in [
        ("nothing", ConnectionCredentials::default(), None),
        ("a running pid", by_pid(own_pid()), Some(own_pid())),
        ("a pid that is gone", by_pid(gone), None),
        ("a pidfd", by_fd(own_pid()), Some(own_pid())),
        (
            "a pid that agrees with the pidfd",
            by_fd(own_pid()).set_process_id(own_pid()),
            Some(own_pid()),
        ),
        (
            "a pid that disagrees with the pidfd",
            by_fd(own_pid()).set_process_id(own_parent()),
            None,
        ),
        ("a descriptor that is no pidfd", not_a_pidfd(), None),
    ] {
        let caller = Caller::from_credentials(&credentials, proc.path());
        match expected {
            Some(pid) => {
                assert_eq!(process_of(&caller).process.pid, pid, "{row}");
                assert_eq!(caller.pid(), Some(pid), "{row}");
            }
            None => {
                assert!(matches!(caller, Caller::Anonymous), "{row}: {caller:?}");
                assert_eq!(caller.pid(), None, "{row}");
            }
        }
    }
    // a caller that must have a descriptor is not named by its pid alone
    assert!(matches!(
        Caller::from_process_fd(&by_pid(own_pid()), proc.path()),
        Caller::Anonymous
    ));
    assert!(matches!(
        Caller::from_process_fd(&by_fd(own_pid()), proc.path()),
        Caller::Process(_)
    ));
}

#[test]
fn the_sandbox_comes_from_the_security_label_then_the_flatpak_info() {
    for (label, flatpak, expected) in [
        (None, false, None),
        (None, true, Some(Sandbox::Flatpak)),
        (
            Some(&b"snap.firefox.firefox (enforce)"[..]),
            false,
            Some(Sandbox::Snap),
        ),
        (Some(b"unconfined"), false, None),
        (Some(b""), false, None),
        (Some(b"notsnap.x"), false, None),
        (Some(b"snap.x.y"), true, Some(Sandbox::Snap)),
    ] {
        let proc = proc_with(&[own_pid()]);
        if flatpak {
            flatpak_info(
                proc.path(),
                own_pid(),
                "[Application]\nname=org.example.App\n",
            );
        }
        let mut credentials = by_pid(own_pid());
        if let Some(label) = label {
            credentials = credentials.set_linux_security_label(label.to_vec());
        }
        let caller = Caller::from_credentials(&credentials, proc.path());
        assert_eq!(
            process_of(&caller).sandbox,
            expected,
            "{label:?}, flatpak info {flatpak}"
        );
    }
}

#[test]
fn the_program_is_what_the_exe_link_names() {
    let proc = proc_with(&[own_pid()]);
    let caller = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    assert_eq!(process_of(&caller).exe, None, "no link");
    std::os::unix::fs::symlink(
        "/usr/bin/example",
        proc.path().join(own_pid().to_string()).join("exe"),
    )
    .unwrap();
    let caller = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    assert_eq!(
        process_of(&caller).exe.as_deref(),
        Some(Path::new("/usr/bin/example"))
    );
}

#[test]
fn a_root_the_kernel_refuses_leaves_the_caller_unclassified_and_untrusted() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let proc = proc_with(&[own_pid()]);
    let root = proc.path().join(own_pid().to_string()).join("root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let caller = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(caller, Caller::Unclassified(_)), "{caller:?}");
    assert_eq!(caller.pid(), Some(own_pid()));
    assert_eq!(caller.unsandboxed().unwrap_err(), Denial::Untrusted);
    assert_eq!(
        caller
            .require_relation(own_pid(), 0, &[Relation::Itself])
            .unwrap_err(),
        Denial::Untrusted
    );
}

#[test]
fn a_root_that_cannot_be_checked_for_another_reason_is_anonymous() {
    let proc = tempfile::tempdir().unwrap();
    let folder = proc.path().join(own_pid().to_string());
    std::fs::create_dir_all(&folder).unwrap();
    // a file where the root folder should be: the check fails with an error that is not "refused"
    std::fs::write(folder.join("root"), "").unwrap();
    assert!(matches!(
        Caller::from_credentials(&by_pid(own_pid()), proc.path()),
        Caller::Anonymous
    ));
}

#[test]
fn a_process_held_otherwise_is_classified_by_its_own_label_and_root() {
    for (file, label, expected) in [
        ("attr/apparmor/current", Some("unconfined"), Some(None)),
        (
            "attr/apparmor/current",
            Some("snap.app.cmd"),
            Some(Some(Sandbox::Snap)),
        ),
        // the first security module's label when AppArmor has no entry of its own
        (
            "attr/current",
            Some("snap.app.cmd"),
            Some(Some(Sandbox::Snap)),
        ),
        ("attr/current", None, None),
    ] {
        let proc = proc_with(&[own_pid()]);
        let path = proc.path().join(own_pid().to_string()).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(label) = label {
            std::fs::write(&path, label).unwrap();
        }
        let caller = Caller::of_process(ProcessHandle::open(own_pid()).unwrap(), proc.path());
        match expected {
            Some(sandbox) => assert_eq!(process_of(&caller).sandbox, sandbox, "{file} {label:?}"),
            None => assert!(matches!(caller, Caller::Anonymous), "{file}: {caller:?}"),
        }
    }
}

#[test]
fn only_an_unsandboxed_caller_is_accepted_and_the_rest_are_refused_by_kind() {
    let plain = Caller::Process(identified(own_pid(), None));
    assert_eq!(plain.unsandboxed().unwrap().process.pid, own_pid());
    for (caller, denial) in [
        (
            Caller::Process(identified(own_pid(), Some(Sandbox::Flatpak))),
            Denial::Sandboxed(Sandbox::Flatpak),
        ),
        (
            Caller::Process(identified(own_pid(), Some(Sandbox::Snap))),
            Denial::Sandboxed(Sandbox::Snap),
        ),
        (Caller::Anonymous, Denial::Anonymous),
        (
            Caller::Unclassified(ProcessHandle::open(own_pid()).unwrap()),
            Denial::Untrusted,
        ),
    ] {
        assert_eq!(caller.unsandboxed().unwrap_err(), denial, "{caller:?}");
    }
}

#[test]
fn a_process_is_its_own_relation_or_its_parents_or_neither() {
    let own = identified(own_pid(), None);
    for (own_pid_, parent, expected) in [
        (own_pid(), own_parent(), Relation::Itself),
        (1, own_pid(), Relation::Parent),
        (1, 2, Relation::Other),
        // the process itself wins when it is also named as its own parent
        (own_pid(), own_pid(), Relation::Itself),
    ] {
        assert_eq!(
            own.relation(own_pid_, parent),
            expected,
            "{own_pid_} {parent}"
        );
    }
}

#[test]
fn a_relation_is_required_of_an_unsandboxed_caller_only() {
    let plain = || Caller::Process(identified(own_pid(), None));
    let all = &[Relation::Itself, Relation::Parent, Relation::Other][..];
    for (row, caller, pid, parent, allowed, expected) in [
        (
            "itself",
            plain(),
            own_pid(),
            1,
            &[Relation::Itself][..],
            Ok(()),
        ),
        (
            "parent",
            plain(),
            1,
            own_pid(),
            &[Relation::Parent][..],
            Ok(()),
        ),
        (
            "either",
            plain(),
            1,
            own_pid(),
            &[Relation::Itself, Relation::Parent][..],
            Ok(()),
        ),
        (
            "other allowed",
            plain(),
            1,
            2,
            &[Relation::Other][..],
            Ok(()),
        ),
        (
            "neither",
            plain(),
            1,
            2,
            &[Relation::Itself, Relation::Parent][..],
            Err(Denial::Untrusted),
        ),
        (
            "itself not allowed",
            plain(),
            own_pid(),
            1,
            &[Relation::Parent][..],
            Err(Denial::Untrusted),
        ),
        (
            "nothing allowed",
            plain(),
            own_pid(),
            1,
            &[][..],
            Err(Denial::Untrusted),
        ),
        (
            "flatpak",
            Caller::Process(identified(own_pid(), Some(Sandbox::Flatpak))),
            own_pid(),
            1,
            all,
            Err(Denial::Sandboxed(Sandbox::Flatpak)),
        ),
        (
            "anonymous",
            Caller::Anonymous,
            own_pid(),
            1,
            all,
            Err(Denial::Anonymous),
        ),
    ] {
        assert_eq!(
            caller.require_relation(pid, parent, allowed),
            expected,
            "{row}"
        );
    }
}

#[test]
fn two_processes_are_one_instance_only_as_one_flatpak_instance() {
    let flatpak = Some(Sandbox::Flatpak);
    let app = flatpak_text("org.example.App", "42");
    for (row, second, sandboxes, infos, expected) in [
        (
            "the same process, no sandbox",
            own_pid(),
            (None, None),
            None,
            true,
        ),
        (
            "the same process, flatpak",
            own_pid(),
            (flatpak, flatpak),
            None,
            true,
        ),
        (
            "the same process, snap",
            own_pid(),
            (Some(Sandbox::Snap), Some(Sandbox::Snap)),
            None,
            true,
        ),
        ("sandboxed and not", own_pid(), (None, flatpak), None, false),
        (
            "flatpak and snap",
            own_pid(),
            (flatpak, Some(Sandbox::Snap)),
            None,
            false,
        ),
        (
            "two plain processes",
            own_parent(),
            (None, None),
            None,
            false,
        ),
        (
            "two snaps",
            own_parent(),
            (Some(Sandbox::Snap), Some(Sandbox::Snap)),
            None,
            false,
        ),
        (
            "an app and its proxy",
            own_parent(),
            (flatpak, flatpak),
            Some((app.clone(), app.clone())),
            true,
        ),
        (
            "another instance",
            own_parent(),
            (flatpak, flatpak),
            Some((app.clone(), flatpak_text("org.example.App", "43"))),
            false,
        ),
        (
            "another application",
            own_parent(),
            (flatpak, flatpak),
            Some((app.clone(), flatpak_text("org.example.Other", "42"))),
            false,
        ),
        (
            "ids trimmed",
            own_parent(),
            (flatpak, flatpak),
            Some((
                "[Application]\n name = org.example.App \n[Instance]\n instance-id = 7 \n".into(),
                flatpak_text("org.example.App", "7"),
            )),
            true,
        ),
        (
            "no info for the second",
            own_parent(),
            (flatpak, flatpak),
            Some((app.clone(), String::new())),
            false,
        ),
    ] {
        let proc = proc_with(&[own_pid(), own_parent()]);
        if let Some((a, b)) = &infos {
            flatpak_info(proc.path(), own_pid(), a);
            if !b.is_empty() {
                flatpak_info(proc.path(), own_parent(), b);
            }
        }
        let a = identified(own_pid(), sandboxes.0);
        let b = identified(second, sandboxes.1);
        assert_eq!(same_instance(&a, &b, proc.path()), expected, "{row}");
        assert_eq!(
            same_instance(&b, &a, proc.path()),
            expected,
            "{row}, reversed"
        );
    }
}

#[test]
fn flatpak_info_without_both_ids_is_no_match() {
    for text in [
        "[Application]\nname=org.example.App\n",
        "[Instance]\ninstance-id=42\n",
        "[Application]\nname=\n[Instance]\ninstance-id=\n",
        "[Other]\nname=org.example.App\ninstance-id=42\n",
    ] {
        let proc = proc_with(&[own_pid(), own_parent()]);
        flatpak_info(proc.path(), own_pid(), text);
        flatpak_info(proc.path(), own_parent(), text);
        let app = identified(own_pid(), Some(Sandbox::Flatpak));
        let other = identified(own_parent(), Some(Sandbox::Flatpak));
        assert!(!same_instance(&app, &other, proc.path()), "{text:?}");
    }
}

#[test]
fn an_exited_process_is_no_instance_of_anything() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let gone = ProcessHandle::open(child.id()).unwrap();
    let pid = gone.pid;
    child.wait().unwrap();
    let proc = proc_with(&[own_pid(), pid]);
    flatpak_info(
        proc.path(),
        own_pid(),
        &flatpak_text("org.example.App", "42"),
    );
    flatpak_info(proc.path(), pid, &flatpak_text("org.example.App", "42"));
    let live = identified(own_pid(), Some(Sandbox::Flatpak));
    let dead = Identified {
        process: gone,
        sandbox: Some(Sandbox::Flatpak),
        exe: None,
    };
    assert!(!same_instance(&live, &dead, proc.path()));
    let twin = Identified {
        process: dead.process.try_clone().unwrap(),
        sandbox: Some(Sandbox::Flatpak),
        exe: None,
    };
    assert!(!same_instance(&dead, &twin, proc.path()));
}

#[test]
fn the_notice_says_so_when_the_bus_gives_no_process_descriptor() {
    for (credentials, expected) in [
        (by_pid(own_pid()), Some(NO_PROCESS_FD)),
        (ConnectionCredentials::default(), Some(NO_PROCESS_FD)),
        (by_fd(own_pid()), None),
    ] {
        assert_eq!(process_fd_notice(&credentials), expected, "{credentials:?}");
    }
}

#[test]
fn each_denial_is_a_bus_error_of_its_own_that_says_who_is_refused() {
    for (denial, name, says) in [
        (Denial::Anonymous, "org.cupertino.Error.AnonymousCaller", ""),
        (
            Denial::Sandboxed(Sandbox::Flatpak),
            "org.cupertino.Error.FlatpakCaller",
            "Flatpak",
        ),
        (
            Denial::Sandboxed(Sandbox::Snap),
            "org.cupertino.Error.SnapCaller",
            "Snap",
        ),
        (Denial::Untrusted, "org.cupertino.Error.UntrustedCaller", ""),
    ] {
        let error = CallerError::from(denial);
        assert_eq!(error.name().as_str(), name);
        assert!(error.description().unwrap().contains(says), "{name}");
    }
}
