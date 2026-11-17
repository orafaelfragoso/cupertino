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

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn entries_are_read_by_path_or_pin_and_read_back_as_their_text() {
    let (c, problems) = DataControlConf::parse(
        "# mine\nallow = /usr/bin/clipd\nallow = 2049:1311:52016:1730281234000000000 /home/me/clipd\n",
    );
    assert!(problems.is_empty(), "{problems:?}");
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
        c.allowed.iter().map(Entry::text).collect::<Vec<_>>(),
        [
            "/usr/bin/clipd",
            "2049:1311:52016:1730281234000000000 /home/me/clipd"
        ]
    );
}

#[test]
fn a_bad_line_allows_nothing_and_is_reported_by_number() {
    for (line, expected) in [
        ("allow = relative/clipd", "not an absolute path"),
        ("allow = 1:2:3 /home/me/x", "is not a pin"),
        ("allow = /usr/bin/python3.12", RUNS_SCRIPTS),
        ("bare", "allow = <program>"),
        ("deny = /x", "unknown setting"),
    ] {
        let (c, problems) = DataControlConf::parse(&format!("# mine\n{line}\n"));
        assert!(c.allowed.is_empty(), "{line}");
        assert_eq!(problems.len(), 1, "{line}");
        assert_eq!(problems[0].line, 2, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
    }
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
fn a_program_only_root_can_change_is_allowed_by_its_path_alone() {
    let sh = Path::new("/bin/true");
    let real = std::fs::canonicalize(sh).unwrap();
    if !cupertino_defaults::system::only_root_can_change(&real) {
        return;
    }
    let meta = std::fs::metadata(sh).unwrap();
    assert!(Entry {
        program: real,
        pin: None
    }
    .allows_file(&meta));
    assert!(
        !Entry {
            program: "/nonexistent/true".into(),
            pin: None
        }
        .allows_file(&meta),
        "another path to the same file"
    );
}

#[test]
fn for_program_refuses_what_cannot_be_allowed() {
    let dir = tempfile::tempdir().unwrap();
    for (row, path, expected) in [
        ("missing", dir.path().join("absent"), ""),
        (
            "a directory",
            dir.path().to_path_buf(),
            "not a program file",
        ),
        ("an interpreter", program(dir.path(), "bash"), RUNS_SCRIPTS),
        ("relative", PathBuf::from("relative"), "cannot be written"),
        (
            "a comment sign",
            program(dir.path(), "a#b"),
            "cannot be written",
        ),
    ] {
        let err = Entry::for_program(&path).unwrap_err();
        assert!(err.contains(expected), "{row}: {err}");
    }
}

#[test]
fn interpreters_loaders_and_proxies_are_refused_and_their_lookalikes_are_not() {
    let scripts = [
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
    ];
    let proxies = [
        "Xwayland",
        "xwayland",
        "waypipe",
        "xwayland-satellite",
        "cupertino-server",
    ];
    let lookalikes = [
        "pythonista",
        "nodemon",
        "shell-helper",
        "clipd",
        "perlbrew",
        "lua-lsp",
    ];
    for name in scripts {
        assert_eq!(
            refusal(Path::new(&format!("/usr/bin/{name}"))),
            Some(RUNS_SCRIPTS),
            "{name}"
        );
    }
    for name in proxies {
        let why =
            refusal(Path::new(&format!("/usr/bin/{name}"))).unwrap_or_else(|| panic!("{name}"));
        assert!(why.contains("Wayland proxy"), "{name}");
    }
    for name in lookalikes {
        assert_eq!(
            refusal(Path::new(&format!("/usr/bin/{name}"))),
            None,
            "{name}"
        );
    }
}

#[test]
fn a_store_is_private_and_reads_back_and_an_entry_a_line_cannot_hold_writes_nothing() {
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
        assert_eq!(
            conf.store(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "{bad:?}"
        );
        assert!(!path.exists(), "{bad:?}");
    }
    let conf = DataControlConf {
        allowed: vec![Entry::for_program(&program(dir.path(), "clipd")).unwrap()],
    };
    conf.store(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(DataControlConf::load(&path).unwrap(), (conf, vec![]));
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
    set_mode(&path, 0o600);
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
    for (row, text) in [
        ("a line that allows", "allow = /usr/bin/clipd"),
        ("two lines", "a\nb"),
    ] {
        let kept = Unusable {
            text: text.into(),
            message: String::new(),
        };
        assert!(conf.store_keeping(&path, &[kept]).is_err(), "{row}");
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("relative"),
            "{row}: nothing was written"
        );
    }
}

#[test]
fn a_file_that_is_not_the_persons_alone_allows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data-control.conf");
    std::fs::write(&path, "allow = /usr/bin/clipd\n").unwrap();
    set_mode(&path, 0o600);
    assert!(
        DataControlConf::load(&path).is_ok(),
        "the person's own file"
    );

    set_mode(&path, 0o666);
    assert_eq!(
        DataControlConf::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    set_mode(&path, 0o600);

    set_mode(dir.path(), 0o777);
    assert!(
        DataControlConf::load(&path).is_err(),
        "a directory others can write"
    );
    set_mode(dir.path(), 0o700);

    let link = dir.path().join("link.conf");
    symlink(&path, &link).unwrap();
    assert!(DataControlConf::load(&link)
        .unwrap_err()
        .to_string()
        .contains("link"));

    let fifo = dir.path().join("fifo.conf");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // Safety: `c` is a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(
        DataControlConf::load(&fifo).is_err(),
        "a fifo is refused without blocking"
    );
}

#[test]
fn a_missing_file_allows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        DataControlConf::load(&dir.path().join("data-control.conf")).unwrap(),
        (DataControlConf::default(), vec![])
    );
}
