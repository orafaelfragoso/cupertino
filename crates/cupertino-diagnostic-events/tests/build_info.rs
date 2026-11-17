//! `build_info`: the commit, the component, the OS image and the kernel.

use std::path::Path;

use cupertino_diagnostic_events::build_info::{
    self, CommitSource, Component, OsImage, KERNEL_RELEASE_FILE, OS_RELEASE_FILES,
    SHORT_COMMIT_DIGITS,
};
use cupertino_diagnostic_events::component;

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn the_commit_is_a_full_lower_case_hash_or_unknown() {
    match build_info::COMMIT {
        Some(commit) => {
            assert!(commit.len() == 40 || commit.len() == 64, "{commit}");
            assert!(commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert_ne!(build_info::COMMIT_SOURCE, CommitSource::Unknown);
            let short = build_info::short_commit().unwrap();
            assert_eq!(short.len(), SHORT_COMMIT_DIGITS);
            assert!(commit.starts_with(short));
        }
        None => {
            assert_eq!(build_info::COMMIT_SOURCE, CommitSource::Unknown);
            assert_eq!(build_info::short_commit(), None);
        }
    }
}

#[test]
fn the_commit_sources_have_their_lower_case_names() {
    assert_eq!(CommitSource::Environment.name(), "environment");
    assert_eq!(CommitSource::Archive.name(), "archive");
    assert_eq!(CommitSource::Checkout.name(), "checkout");
    assert_eq!(CommitSource::Unknown.name(), "unknown");
}

#[test]
fn a_component_is_named_after_the_package_or_by_name_and_has_its_version() {
    let own = component!();
    assert_eq!(own.name, "cupertino-diagnostic-events");
    assert_eq!(own.version, env!("CARGO_PKG_VERSION"));
    let renamed = component!("cupertino");
    assert_eq!(renamed.name, "cupertino");
    assert_eq!(renamed.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(own.commit(), build_info::COMMIT);
}

#[test]
fn a_component_displays_its_name_version_and_short_commit() {
    let shown = component!("cupertino").to_string();
    match build_info::short_commit() {
        Some(short) => assert_eq!(
            shown,
            format!("cupertino {} ({short})", env!("CARGO_PKG_VERSION"))
        ),
        None => assert_eq!(shown, format!("cupertino {}", env!("CARGO_PKG_VERSION"))),
    }
    let by_hand = Component {
        name: "x",
        version: "1.2.3",
    };
    assert!(by_hand.to_string().starts_with("x 1.2.3"));
}

#[test]
fn os_release_lines_give_the_image_fields() {
    let image = OsImage::parse(
        "NAME=\"Arch Linux\"\nID=arch\nVERSION_ID=2026.11\nBUILD_ID=rolling\nIMAGE_ID=cupertino\nIMAGE_VERSION=7\nHOME_URL=x\n",
    );
    assert_eq!(image.name.as_deref(), Some("Arch Linux"));
    assert_eq!(image.id.as_deref(), Some("arch"));
    assert_eq!(image.version_id.as_deref(), Some("2026.11"));
    assert_eq!(image.build_id.as_deref(), Some("rolling"));
    assert_eq!(image.image_id.as_deref(), Some("cupertino"));
    assert_eq!(image.image_version.as_deref(), Some("7"));
    assert!(!image.is_empty());
}

#[test]
fn os_release_quoting_follows_the_format() {
    let image = OsImage::parse(
        "# a comment\n\nNAME=\"A \\\"quoted\\\" \\$name \\\\ \\x\"\nID='single \\n literal'\nVERSION_ID=un\\ quoted\n\
         BUILD_ID=\nIMAGE_ID=first\nIMAGE_ID=second\nno equals sign\n",
    );
    assert_eq!(image.name.as_deref(), Some("A \"quoted\" $name \\ \\x"));
    assert_eq!(image.id.as_deref(), Some("single \\n literal"));
    assert_eq!(image.version_id.as_deref(), Some("un quoted"));
    assert_eq!(image.build_id, None, "an empty value is not set");
    assert_eq!(
        image.image_id.as_deref(),
        Some("second"),
        "a later line wins"
    );
}

#[test]
fn an_empty_os_release_is_an_empty_image() {
    assert!(OsImage::parse("").is_empty());
    assert!(OsImage::parse("# only a comment\nOTHER=1\n").is_empty());
}

#[test]
fn the_image_is_read_from_etc_and_falls_back_to_usr_lib() {
    assert_eq!(OS_RELEASE_FILES, ["etc/os-release", "usr/lib/os-release"]);
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "usr/lib/os-release", "ID=lib\n");
    assert_eq!(
        build_info::read_os_image(root.path())
            .unwrap()
            .id
            .as_deref(),
        Some("lib")
    );
    write(root.path(), "etc/os-release", "ID=etc\n");
    assert_eq!(
        build_info::read_os_image(root.path())
            .unwrap()
            .id
            .as_deref(),
        Some("etc")
    );
}

#[test]
fn a_root_without_os_release_is_not_found() {
    let root = tempfile::tempdir().unwrap();
    let err = build_info::read_os_image(root.path()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn an_unreadable_os_release_is_the_read_error_and_not_a_fallback() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("etc/os-release")).unwrap();
    write(root.path(), "usr/lib/os-release", "ID=lib\n");
    let err = build_info::read_os_image(root.path()).unwrap_err();
    assert_ne!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn text_that_is_not_utf8_is_read_lossily() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("etc/os-release");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"ID=arch\nNAME=\"Caf\xe9\"\n").unwrap();
    let image = build_info::read_os_image(root.path()).unwrap();
    assert_eq!(image.id.as_deref(), Some("arch"));
    assert!(image.name.unwrap().starts_with("Caf"));
}

#[test]
fn the_kernel_release_is_read_without_its_line_end() {
    assert_eq!(KERNEL_RELEASE_FILE, "proc/sys/kernel/osrelease");
    let root = tempfile::tempdir().unwrap();
    assert_eq!(build_info::kernel_in(root.path()), None);
    write(root.path(), KERNEL_RELEASE_FILE, "7.3.4-test\n");
    assert_eq!(
        build_info::kernel_in(root.path()).as_deref(),
        Some("7.3.4-test")
    );
    write(root.path(), KERNEL_RELEASE_FILE, "\n");
    assert_eq!(build_info::kernel_in(root.path()), None, "empty is none");
}

#[test]
fn the_running_system_has_a_kernel_and_the_image_never_fails() {
    assert!(build_info::kernel().is_some_and(|k| !k.is_empty()));
    let _ = build_info::os_image();
}
