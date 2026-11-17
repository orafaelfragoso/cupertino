//! `build_info`: the commit, the component, the OS image and the kernel.

use std::path::Path;

use cupertino_diagnostic_events::build_info::{self, CommitSource, OsImage, KERNEL_RELEASE_FILE};
use cupertino_diagnostic_events::component;

fn write(root: &Path, file: &str, text: &[u8]) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn the_commit_is_a_full_lower_case_hash_with_its_short_form_or_unknown() {
    match build_info::COMMIT {
        Some(commit) => {
            assert!(commit.len() == 40 || commit.len() == 64, "{commit}");
            assert!(commit
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
            assert_ne!(build_info::COMMIT_SOURCE, CommitSource::Unknown);
            let short = build_info::short_commit().unwrap();
            assert_eq!(short.len(), build_info::SHORT_COMMIT_DIGITS);
            assert!(commit.starts_with(short));
        }
        None => {
            assert_eq!(build_info::COMMIT_SOURCE, CommitSource::Unknown);
            assert_eq!(build_info::short_commit(), None);
        }
    }
}

#[test]
fn a_component_is_its_package_or_the_name_given_and_shows_its_version_and_short_commit() {
    let version = env!("CARGO_PKG_VERSION");
    for (component, name) in [
        (component!(), "cupertino-diagnostic-events"),
        (component!("cupertino"), "cupertino"),
    ] {
        assert_eq!((component.name, component.version), (name, version));
        assert_eq!(component.commit(), build_info::COMMIT);
        let shown = match build_info::short_commit() {
            Some(short) => format!("{name} {version} ({short})"),
            None => format!("{name} {version}"),
        };
        assert_eq!(component.to_string(), shown);
    }
}

#[test]
fn os_release_text_gives_the_image_fields_as_the_format_quotes_them() {
    let full = OsImage::parse(
        "NAME=\"Arch Linux\"\nID=arch\nVERSION_ID=2026.11\nBUILD_ID=rolling\nIMAGE_ID=cupertino\nIMAGE_VERSION=7\nHOME_URL=x\n",
    );
    let fields = |i: &OsImage| {
        [
            i.name.clone(),
            i.id.clone(),
            i.version_id.clone(),
            i.build_id.clone(),
            i.image_id.clone(),
            i.image_version.clone(),
        ]
    };
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(
        fields(&full),
        [
            some("Arch Linux"),
            some("arch"),
            some("2026.11"),
            some("rolling"),
            some("cupertino"),
            some("7")
        ]
    );
    assert!(!full.is_empty());
    // escapes in double quotes, single quotes literal, an escaped space unquoted, an empty value unset, the later
    // line winning, comments and lines without `=` skipped
    let quoted = OsImage::parse(
        "# a comment\n\nNAME=\"A \\\"quoted\\\" \\$name \\\\ \\x\"\nID='single \\n literal'\nVERSION_ID=un\\ quoted\n\
         BUILD_ID=\nIMAGE_ID=first\nIMAGE_ID=second\nno equals sign\n",
    );
    assert_eq!(
        fields(&quoted),
        [
            some("A \"quoted\" $name \\ \\x"),
            some("single \\n literal"),
            some("un quoted"),
            None,
            some("second"),
            None
        ]
    );
    for empty in ["", "# only a comment\nOTHER=1\n"] {
        assert!(OsImage::parse(empty).is_empty(), "{empty:?}");
    }
}

#[test]
fn the_image_is_read_from_etc_else_usr_lib_and_a_read_error_is_no_fallback() {
    let root = tempfile::tempdir().unwrap();
    let err = build_info::read_os_image(root.path()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "neither file");
    write(root.path(), "usr/lib/os-release", b"ID=lib\n");
    assert_eq!(
        build_info::read_os_image(root.path())
            .unwrap()
            .id
            .as_deref(),
        Some("lib")
    );
    // not UTF-8: read lossily
    write(root.path(), "etc/os-release", b"ID=etc\nNAME=\"Caf\xe9\"\n");
    let image = build_info::read_os_image(root.path()).unwrap();
    assert_eq!(image.id.as_deref(), Some("etc"));
    assert!(image.name.unwrap().starts_with("Caf"));
    // /etc/os-release there but unreadable: its error, not /usr/lib's file
    let unreadable = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(unreadable.path().join("etc/os-release")).unwrap();
    write(unreadable.path(), "usr/lib/os-release", b"ID=lib\n");
    let err = build_info::read_os_image(unreadable.path()).unwrap_err();
    assert_ne!(err.kind(), std::io::ErrorKind::NotFound);
    // the running system's never fails
    let _ = build_info::os_image();
}

#[test]
fn the_kernel_release_is_read_without_its_line_end() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(build_info::kernel_in(root.path()), None, "no file");
    for (text, release) in [("7.3.4-test\n", Some("7.3.4-test")), ("\n", None)] {
        write(root.path(), KERNEL_RELEASE_FILE, text.as_bytes());
        assert_eq!(
            build_info::kernel_in(root.path()).as_deref(),
            release,
            "{text:?}"
        );
    }
    assert!(build_info::kernel().is_some_and(|k| !k.is_empty()));
}
