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

#[test]
fn credentials_without_a_process_are_anonymous() {
    let proc = proc_with(&[]);
    let caller = Caller::from_credentials(&ConnectionCredentials::default(), proc.path());
    assert!(matches!(caller, Caller::Anonymous));
    assert_eq!(caller.pid(), None);
}

#[test]
fn a_process_id_alone_names_the_process_when_it_is_running() {
    let proc = proc_with(&[own_pid()]);
    let caller = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    let found = process_of(&caller);
    assert_eq!(found.process.pid, own_pid());
    assert_eq!(found.sandbox, None);
    assert_eq!(caller.pid(), Some(own_pid()));
}

#[test]
fn a_process_id_of_a_process_that_is_gone_is_anonymous() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let proc = proc_with(&[pid]);
    assert!(matches!(
        Caller::from_credentials(&by_pid(pid), proc.path()),
        Caller::Anonymous
    ));
}

#[test]
fn a_process_descriptor_names_the_process() {
    let proc = proc_with(&[own_pid()]);
    let caller = Caller::from_credentials(&by_fd(own_pid()), proc.path());
    assert_eq!(process_of(&caller).process.pid, own_pid());
}

#[test]
fn a_process_id_that_disagrees_with_the_descriptor_is_anonymous() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    let credentials = by_fd(own_pid()).set_process_id(own_parent());
    assert!(matches!(
        Caller::from_credentials(&credentials, proc.path()),
        Caller::Anonymous
    ));
    let agreeing = by_fd(own_pid()).set_process_id(own_pid());
    assert!(matches!(
        Caller::from_credentials(&agreeing, proc.path()),
        Caller::Process(_)
    ));
}

#[test]
fn a_descriptor_that_is_not_a_pidfd_is_anonymous() {
    let proc = proc_with(&[own_pid()]);
    let file = std::fs::File::open("/dev/null").unwrap();
    let credentials =
        ConnectionCredentials::default().set_process_fd(std::os::fd::OwnedFd::from(file).into());
    assert!(matches!(
        Caller::from_credentials(&credentials, proc.path()),
        Caller::Anonymous
    ));
}

#[test]
fn only_a_descriptor_names_the_process_when_the_caller_must_have_one() {
    let proc = proc_with(&[own_pid()]);
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
fn a_flatpak_info_in_the_root_makes_a_flatpak() {
    let proc = proc_with(&[own_pid()]);
    flatpak_info(
        proc.path(),
        own_pid(),
        "[Application]\nname=org.example.App\n",
    );
    let caller = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    assert_eq!(process_of(&caller).sandbox, Some(Sandbox::Flatpak));
}

#[test]
fn a_security_label_that_starts_with_snap_makes_a_snap() {
    let proc = proc_with(&[own_pid()]);
    let credentials =
        by_pid(own_pid()).set_linux_security_label(b"snap.firefox.firefox (enforce)".to_vec());
    let caller = Caller::from_credentials(&credentials, proc.path());
    assert_eq!(process_of(&caller).sandbox, Some(Sandbox::Snap));
}

#[test]
fn other_security_labels_are_no_sandbox() {
    let proc = proc_with(&[own_pid()]);
    for label in [&b"unconfined"[..], b"", b"notsnap.x"] {
        let credentials = by_pid(own_pid()).set_linux_security_label(label.to_vec());
        let caller = Caller::from_credentials(&credentials, proc.path());
        assert_eq!(process_of(&caller).sandbox, None, "{label:?}");
    }
}

#[test]
fn a_snap_label_wins_over_a_flatpak_info() {
    let proc = proc_with(&[own_pid()]);
    flatpak_info(proc.path(), own_pid(), "");
    let credentials = by_pid(own_pid()).set_linux_security_label(b"snap.x.y".to_vec());
    let caller = Caller::from_credentials(&credentials, proc.path());
    assert_eq!(process_of(&caller).sandbox, Some(Sandbox::Snap));
}

#[test]
fn the_program_is_what_the_exe_link_names() {
    let proc = proc_with(&[own_pid()]);
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
    let without = proc_with(&[own_pid()]);
    let caller = Caller::from_credentials(&by_pid(own_pid()), without.path());
    assert_eq!(process_of(&caller).exe, None);
}

#[test]
fn a_root_the_kernel_refuses_leaves_the_caller_unclassified() {
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
    let proc = proc_with(&[own_pid()]);
    let attr = proc.path().join(own_pid().to_string()).join("attr");
    std::fs::create_dir_all(attr.join("apparmor")).unwrap();
    std::fs::write(attr.join("apparmor/current"), "unconfined").unwrap();
    let caller = Caller::of_process(ProcessHandle::open(own_pid()).unwrap(), proc.path());
    assert_eq!(process_of(&caller).sandbox, None);
    std::fs::write(attr.join("apparmor/current"), "snap.app.cmd").unwrap();
    let caller = Caller::of_process(ProcessHandle::open(own_pid()).unwrap(), proc.path());
    assert_eq!(process_of(&caller).sandbox, Some(Sandbox::Snap));
}

#[test]
fn the_label_of_the_first_security_module_is_used_without_apparmors_entry() {
    let proc = proc_with(&[own_pid()]);
    let attr = proc.path().join(own_pid().to_string()).join("attr");
    std::fs::create_dir_all(&attr).unwrap();
    std::fs::write(attr.join("current"), "snap.app.cmd").unwrap();
    let caller = Caller::of_process(ProcessHandle::open(own_pid()).unwrap(), proc.path());
    assert_eq!(process_of(&caller).sandbox, Some(Sandbox::Snap));
}

#[test]
fn a_process_whose_label_cannot_be_read_is_anonymous() {
    let proc = proc_with(&[own_pid()]);
    assert!(matches!(
        Caller::of_process(ProcessHandle::open(own_pid()).unwrap(), proc.path()),
        Caller::Anonymous
    ));
}

#[test]
fn the_pid_of_each_kind_of_caller() {
    let proc = proc_with(&[own_pid()]);
    let process = Caller::from_credentials(&by_pid(own_pid()), proc.path());
    assert_eq!(process.pid(), Some(own_pid()));
    assert_eq!(Caller::Anonymous.pid(), None);
    assert_eq!(
        Caller::Unclassified(ProcessHandle::open(own_pid()).unwrap()).pid(),
        Some(own_pid())
    );
}

#[test]
fn an_unsandboxed_caller_is_accepted_and_the_rest_are_refused_by_kind() {
    let plain = Caller::Process(identified(own_pid(), None));
    assert_eq!(plain.unsandboxed().unwrap().process.pid, own_pid());
    let flatpak = Caller::Process(identified(own_pid(), Some(Sandbox::Flatpak)));
    assert_eq!(
        flatpak.unsandboxed().unwrap_err(),
        Denial::Sandboxed(Sandbox::Flatpak)
    );
    let snap = Caller::Process(identified(own_pid(), Some(Sandbox::Snap)));
    assert_eq!(
        snap.unsandboxed().unwrap_err(),
        Denial::Sandboxed(Sandbox::Snap)
    );
    assert_eq!(
        Caller::Anonymous.unsandboxed().unwrap_err(),
        Denial::Anonymous
    );
    let unclassified = Caller::Unclassified(ProcessHandle::open(own_pid()).unwrap());
    assert_eq!(unclassified.unsandboxed().unwrap_err(), Denial::Untrusted);
}

#[test]
fn a_process_is_its_own_relation_or_its_parents_or_neither() {
    let own = identified(own_pid(), None);
    assert_eq!(own.relation(own_pid(), own_parent()), Relation::Itself);
    assert_eq!(own.relation(1, own_pid()), Relation::Parent);
    assert_eq!(own.relation(1, 2), Relation::Other);
    // the process itself wins when it is also named as its own parent
    assert_eq!(own.relation(own_pid(), own_pid()), Relation::Itself);
}

#[test]
fn a_relation_is_required_of_an_unsandboxed_caller() {
    let caller = Caller::Process(identified(own_pid(), None));
    assert_eq!(
        caller.require_relation(own_pid(), 1, &[Relation::Itself]),
        Ok(())
    );
    assert_eq!(
        caller.require_relation(1, own_pid(), &[Relation::Parent]),
        Ok(())
    );
    assert_eq!(
        caller.require_relation(1, own_pid(), &[Relation::Itself, Relation::Parent]),
        Ok(())
    );
}

#[test]
fn a_caller_in_no_allowed_relation_is_untrusted() {
    let caller = Caller::Process(identified(own_pid(), None));
    assert_eq!(
        caller.require_relation(1, 2, &[Relation::Itself, Relation::Parent]),
        Err(Denial::Untrusted)
    );
    assert_eq!(
        caller.require_relation(own_pid(), 1, &[Relation::Parent]),
        Err(Denial::Untrusted)
    );
    assert_eq!(
        caller.require_relation(own_pid(), 1, &[]),
        Err(Denial::Untrusted)
    );
    assert_eq!(caller.require_relation(1, 2, &[Relation::Other]), Ok(()));
}

#[test]
fn a_sandboxed_or_anonymous_caller_is_refused_whatever_its_relation() {
    let flatpak = Caller::Process(identified(own_pid(), Some(Sandbox::Flatpak)));
    assert_eq!(
        flatpak.require_relation(
            own_pid(),
            1,
            &[Relation::Itself, Relation::Parent, Relation::Other]
        ),
        Err(Denial::Sandboxed(Sandbox::Flatpak))
    );
    assert_eq!(
        Caller::Anonymous.require_relation(own_pid(), 1, &[Relation::Other]),
        Err(Denial::Anonymous)
    );
}

#[test]
fn a_flatpak_process_and_its_proxy_are_one_instance() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    flatpak_info(
        proc.path(),
        own_pid(),
        &flatpak_text("org.example.App", "42"),
    );
    flatpak_info(
        proc.path(),
        own_parent(),
        &flatpak_text("org.example.App", "42"),
    );
    let app = identified(own_pid(), Some(Sandbox::Flatpak));
    let proxy = identified(own_parent(), Some(Sandbox::Flatpak));
    assert!(same_instance(&app, &proxy, proc.path()));
    assert!(same_instance(&proxy, &app, proc.path()));
}

#[test]
fn flatpak_processes_of_another_instance_or_application_are_not() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    let app = identified(own_pid(), Some(Sandbox::Flatpak));
    let other = identified(own_parent(), Some(Sandbox::Flatpak));
    flatpak_info(
        proc.path(),
        own_pid(),
        &flatpak_text("org.example.App", "42"),
    );
    flatpak_info(
        proc.path(),
        own_parent(),
        &flatpak_text("org.example.App", "43"),
    );
    assert!(!same_instance(&app, &other, proc.path()));
    flatpak_info(
        proc.path(),
        own_parent(),
        &flatpak_text("org.example.Other", "42"),
    );
    assert!(!same_instance(&app, &other, proc.path()));
}

#[test]
fn flatpak_info_that_cannot_be_read_or_lacks_an_id_is_no_match() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    let app = identified(own_pid(), Some(Sandbox::Flatpak));
    let other = identified(own_parent(), Some(Sandbox::Flatpak));
    flatpak_info(
        proc.path(),
        own_pid(),
        &flatpak_text("org.example.App", "42"),
    );
    assert!(
        !same_instance(&app, &other, proc.path()),
        "no file for one of them"
    );
    for text in [
        "[Application]\nname=org.example.App\n",
        "[Instance]\ninstance-id=42\n",
        "[Application]\nname=\n[Instance]\ninstance-id=\n",
        "[Other]\nname=org.example.App\ninstance-id=42\n",
    ] {
        flatpak_info(proc.path(), own_pid(), text);
        flatpak_info(proc.path(), own_parent(), text);
        assert!(!same_instance(&app, &other, proc.path()), "{text:?}");
    }
}

#[test]
fn the_ids_of_a_flatpak_info_are_trimmed() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    flatpak_info(
        proc.path(),
        own_pid(),
        "[Application]\n name = org.example.App \n[Instance]\n instance-id = 7 \n",
    );
    flatpak_info(
        proc.path(),
        own_parent(),
        &flatpak_text("org.example.App", "7"),
    );
    let app = identified(own_pid(), Some(Sandbox::Flatpak));
    let other = identified(own_parent(), Some(Sandbox::Flatpak));
    assert!(same_instance(&app, &other, proc.path()));
}

#[test]
fn the_same_process_is_one_instance_without_reading_anything() {
    let proc = proc_with(&[]);
    for sandbox in [None, Some(Sandbox::Flatpak), Some(Sandbox::Snap)] {
        let a = identified(own_pid(), sandbox);
        let b = identified(own_pid(), sandbox);
        assert!(same_instance(&a, &b, proc.path()), "{sandbox:?}");
    }
}

#[test]
fn two_processes_without_a_sandbox_or_in_snaps_are_never_one_instance() {
    let proc = proc_with(&[own_pid(), own_parent()]);
    for sandbox in [None, Some(Sandbox::Snap)] {
        let a = identified(own_pid(), sandbox);
        let b = identified(own_parent(), sandbox);
        assert!(!same_instance(&a, &b, proc.path()), "{sandbox:?}");
    }
}

#[test]
fn a_sandboxed_and_an_unsandboxed_process_never_match() {
    let proc = proc_with(&[own_pid()]);
    let plain = identified(own_pid(), None);
    let flatpak = identified(own_pid(), Some(Sandbox::Flatpak));
    let snap = identified(own_pid(), Some(Sandbox::Snap));
    assert!(!same_instance(&plain, &flatpak, proc.path()));
    assert!(!same_instance(&flatpak, &snap, proc.path()));
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
    assert_eq!(process_fd_notice(&by_pid(own_pid())), Some(NO_PROCESS_FD));
    assert_eq!(
        process_fd_notice(&ConnectionCredentials::default()),
        Some(NO_PROCESS_FD)
    );
    assert_eq!(process_fd_notice(&by_fd(own_pid())), None);
}

#[test]
fn each_denial_has_an_error_name_of_its_own() {
    let name = |denial| CallerError::from(denial).name().to_string();
    assert_eq!(
        name(Denial::Anonymous),
        "org.cupertino.Error.AnonymousCaller"
    );
    assert_eq!(
        name(Denial::Sandboxed(Sandbox::Flatpak)),
        "org.cupertino.Error.FlatpakCaller"
    );
    assert_eq!(
        name(Denial::Sandboxed(Sandbox::Snap)),
        "org.cupertino.Error.SnapCaller"
    );
    assert_eq!(
        name(Denial::Untrusted),
        "org.cupertino.Error.UntrustedCaller"
    );
}

#[test]
fn a_denial_has_a_message_that_says_who_is_refused() {
    let description = |denial| CallerError::from(denial).description().map(str::to_owned);
    assert!(description(Denial::Sandboxed(Sandbox::Flatpak))
        .unwrap()
        .contains("Flatpak"));
    assert!(description(Denial::Sandboxed(Sandbox::Snap))
        .unwrap()
        .contains("Snap"));
    assert!(description(Denial::Anonymous).is_some());
    assert!(description(Denial::Untrusted).is_some());
}
