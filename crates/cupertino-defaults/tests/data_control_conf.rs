//! `data-control.conf`: the programs the person allows to use data control.

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use cupertino_defaults::data_control::*;

fn program(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/true\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_missing_file_allows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (c, problems) = DataControlConf::load(&dir.path().join("data-control.conf")).unwrap();
    assert!(c.allowed.is_empty() && problems.is_empty());
}

#[test]
fn entries_are_read_by_path_or_pin_and_bad_lines_allow_nothing() {
    let (c, problems) = DataControlConf::parse(
        "# mine\nallow = /usr/bin/clipd\nallow = 2049:1311:52016:1730281234000000000 /home/me/clipd\n\
         allow = relative/clipd\nallow = 1:2:3 /home/me/x\nallow = /usr/bin/python3.12\nbare\ndeny = /x\n",
    );
    assert_eq!(c.allowed.len(), 2);
    assert_eq!(
        c.allowed[0],
        Entry {
            program: "/usr/bin/clipd".into(),
            pin: None
        }
    );
    let pin = c.allowed[1].pin.unwrap();
    assert_eq!(
        (pin.dev, pin.ino, pin.size, pin.mtime),
        (2049, 1311, 52016, 1_730_281_234_000_000_000)
    );
    assert_eq!(c.allowed[1].program, Path::new("/home/me/clipd"));
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [4, 5, 6, 7, 8]
    );
    assert!(problems[0].message.contains("not an absolute path"));
    assert!(problems[1].message.contains("is not a pin"));
    assert_eq!(problems[2].message, RUNS_SCRIPTS);
    assert!(problems[3].message.contains("allow = <program>"));
    assert!(problems[4].message.contains("unknown setting"));
}

#[test]
fn an_entry_reads_back_as_its_text() {
    for text in ["/usr/bin/clipd", "1:2:3:4 /home/me/clipd"] {
        let (c, p) = DataControlConf::parse(&format!("allow = {text}\n"));
        assert!(p.is_empty());
        assert_eq!(c.allowed[0].text(), text);
    }
}

#[test]
fn a_stored_file_reads_back_the_same_entries_with_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    let entry = Entry::for_program(&program(dir.path(), "clipd")).unwrap();
    let conf = DataControlConf {
        allowed: vec![entry],
    };
    conf.store(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let (back, problems) = DataControlConf::load(&path).unwrap();
    assert!(problems.is_empty());
    assert_eq!(back, conf);
}

#[test]
fn a_program_of_the_user_is_pinned_and_allowed_until_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = program(dir.path(), "clipd");
    let entry = Entry::for_program(&path).unwrap();
    assert!(entry.pin.is_some(), "the user can change it");
    assert!(entry.allows_file(&std::fs::metadata(&path).unwrap()));
    std::fs::write(&path, "#!/bin/true\n# changed so the size differs\n").unwrap();
    assert!(!entry.allows_file(&std::fs::metadata(&path).unwrap()));
}

#[test]
fn for_program_refuses_what_cannot_be_allowed() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Entry::for_program(&dir.path().join("absent")).is_err());
    assert!(Entry::for_program(dir.path())
        .unwrap_err()
        .contains("not a program file"));
    assert_eq!(
        Entry::for_program(&program(dir.path(), "bash")).unwrap_err(),
        RUNS_SCRIPTS
    );
    assert!(Entry::for_program(Path::new("relative"))
        .unwrap_err()
        .contains("cannot be written"));
    let hash = program(dir.path(), "a#b");
    assert!(Entry::for_program(&hash)
        .unwrap_err()
        .contains("cannot be written"));
}

#[test]
fn a_program_only_root_can_change_is_allowed_by_its_path_alone() {
    let sh = Path::new("/bin/true");
    if !cupertino_defaults::system::only_root_can_change(&std::fs::canonicalize(sh).unwrap()) {
        return;
    }
    let entry = Entry {
        program: std::fs::canonicalize(sh).unwrap(),
        pin: None,
    };
    assert!(entry.allows_file(&std::fs::metadata(sh).unwrap()));
    let user_owned = Entry {
        program: PathBuf::from("/nonexistent/true"),
        pin: None,
    };
    assert!(!user_owned.allows_file(&std::fs::metadata(sh).unwrap()));
}

#[test]
fn interpreters_loaders_and_proxies_are_refused_and_their_lookalikes_are_not() {
    for name in [
        "python",
        "python3",
        "python3.12",
        "perl",
        "ruby",
        "node",
        "nodejs",
        "lua5.4",
        "php8.3",
        "sh",
        "bash",
        "env",
        "java",
        "busybox",
        "ld.so",
        "ld-linux-x86-64.so.2",
        "ld-musl-x86_64.so.1",
    ] {
        assert_eq!(
            refusal(Path::new(&format!("/usr/bin/{name}"))),
            Some(RUNS_SCRIPTS),
            "{name}"
        );
    }
    for name in [
        "Xwayland",
        "xwayland",
        "waypipe",
        "xwayland-satellite",
        "cupertino-server",
    ] {
        let why =
            refusal(Path::new(&format!("/usr/bin/{name}"))).unwrap_or_else(|| panic!("{name}"));
        assert!(why.contains("Wayland proxy"), "{name}");
    }
    for name in [
        "pythonista",
        "nodemon",
        "shell-helper",
        "clipd",
        "perlbrew",
        "lua-lsp",
    ] {
        assert_eq!(
            refusal(Path::new(&format!("/usr/bin/{name}"))),
            None,
            "{name}"
        );
    }
}

#[test]
fn the_lines_that_allow_nothing_are_kept_as_written_and_never_one_that_allows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    std::fs::write(
        &path,
        "allow = /usr/bin/python3\nallow = relative\n# note\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (conf, unusable) = DataControlConf::load_unusable(&path).unwrap();
    assert!(conf.allowed.is_empty());
    assert_eq!(unusable.len(), 2);
    assert_eq!(unusable[0].text, "allow = /usr/bin/python3");
    assert_eq!(unusable[0].message, RUNS_SCRIPTS);
    conf.store_keeping(&path, &unusable).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "allow = /usr/bin/python3\nallow = relative\n"
    );
    let bad = Unusable {
        text: "allow = /usr/bin/clipd".into(),
        message: String::new(),
    };
    assert!(conf.store_keeping(&path, &[bad]).is_err());
    let multi = Unusable {
        text: "a\nb".into(),
        message: String::new(),
    };
    assert!(conf.store_keeping(&path, &[multi]).is_err());
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("relative"),
        "nothing was written"
    );
}

#[test]
fn an_entry_a_line_cannot_hold_is_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    for bad in [
        "relative/clipd",
        "/a#b",
        "/a\nb",
        " /a",
        "/a ",
        "/usr/bin/bash",
    ] {
        let conf = DataControlConf {
            allowed: vec![Entry {
                program: bad.into(),
                pin: None,
            }],
        };
        let err = conf.store(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
        assert!(!path.exists(), "{bad:?}");
    }
}

#[test]
fn a_file_that_is_not_the_persons_alone_allows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    std::fs::write(&path, "allow = /usr/bin/clipd\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        DataControlConf::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(DataControlConf::load(&path).is_ok());
    // a directory others can write
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(DataControlConf::load(&path).is_err());
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    // a link
    let link = dir.path().join("link.conf");
    symlink(&path, &link).unwrap();
    assert!(DataControlConf::load(&link)
        .unwrap_err()
        .to_string()
        .contains("link"));
}

#[test]
fn a_fifo_at_the_path_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // Safety: `c` is a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(DataControlConf::load(&path).is_err());
}

#[test]
fn the_variable_and_file_name_are_the_documented_ones() {
    assert_eq!(DATA_CONTROL_ENV, "CUPERTINO_DATA_CONTROL_CONF");
    assert_eq!(DATA_CONTROL_FILE, "data-control.conf");
    std::env::set_var(DATA_CONTROL_ENV, "/x/dc.conf");
    assert_eq!(DataControlConf::path(), Some("/x/dc.conf".into()));
}
