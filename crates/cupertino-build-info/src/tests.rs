use super::*;

#[test]
fn quoted_and_unquoted_values_are_read_and_comments_skipped() {
    let image = OsImage::parse(
        "# the image\n\
         NAME=\"Cupertino \\\"Desk\\\" \\$HOME \\\\ \\`x\\`\"\n\
         \n\
         ID=cupertino\n\
         VERSION_ID='0.1 $raw \\n'\n\
         #BUILD_ID=commented\n\
         IMAGE_ID = cupertino-desktop \n\
         IMAGE_VERSION=\"2026.11.09\"\n\
         PRETTY_NAME=\"not read\"\n\
         no equals sign\n",
    );
    assert_eq!(
        image,
        OsImage {
            name: Some("Cupertino \"Desk\" $HOME \\ `x`".into()),
            id: Some("cupertino".into()),
            version_id: Some("0.1 $raw \\n".into()),
            build_id: None,
            image_id: Some("cupertino-desktop".into()),
            image_version: Some("2026.11.09".into()),
        }
    );
}

#[test]
fn an_unknown_escape_keeps_its_backslash_and_a_lone_quote_is_part_of_the_value() {
    let image =
        OsImage::parse("NAME=\"a\\qb\\\"\nID=\"open\nBUILD_ID=\"\"\nVERSION_ID=x\nVERSION_ID=y\n");
    assert_eq!(image.name.as_deref(), Some("a\\qb\\"));
    assert_eq!(image.id.as_deref(), Some("\"open"));
    assert_eq!(image.build_id, None, "an empty value is not set");
    assert_eq!(
        image.version_id.as_deref(),
        Some("y"),
        "the later line wins"
    );
}

#[test]
fn an_unquoted_value_takes_the_character_after_a_backslash() {
    let image = OsImage::parse("NAME=Cupertino\\ Desktop\nID=a\\$b\\\\c\nVERSION_ID=trailing\\\n");
    assert_eq!(image.name.as_deref(), Some("Cupertino Desktop"));
    assert_eq!(image.id.as_deref(), Some("a$b\\c"));
    assert_eq!(image.version_id.as_deref(), Some("trailing"));
}

#[test]
fn a_missing_file_is_not_found_and_the_usr_lib_file_stands_in_for_etc() {
    let root = tempfile::tempdir().unwrap();
    let err = read_os_image(root.path()).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert!(read_os_image(root.path()).is_err());

    std::fs::create_dir_all(root.path().join("usr/lib")).unwrap();
    std::fs::write(root.path().join("usr/lib/os-release"), "ID=vendor\n").unwrap();
    assert_eq!(
        read_os_image(root.path()).unwrap().id.as_deref(),
        Some("vendor")
    );

    std::fs::create_dir_all(root.path().join("etc")).unwrap();
    std::fs::write(root.path().join("etc/os-release"), "ID=admin\n").unwrap();
    assert_eq!(
        read_os_image(root.path()).unwrap().id.as_deref(),
        Some("admin")
    );
}

#[test]
fn an_unreadable_etc_file_is_its_own_error_not_the_fallback() {
    let root = tempfile::tempdir().unwrap();
    // a directory where the file belongs cannot be read as a file
    std::fs::create_dir_all(root.path().join("etc/os-release")).unwrap();
    std::fs::create_dir_all(root.path().join("usr/lib")).unwrap();
    std::fs::write(root.path().join("usr/lib/os-release"), "ID=vendor\n").unwrap();
    let err = read_os_image(root.path()).unwrap_err();
    assert_ne!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_file_without_the_fields_is_an_empty_image() {
    assert!(OsImage::parse("# nothing\nPRETTY_NAME=x\n").is_empty());
    assert!(!OsImage::parse("ID=x\n").is_empty());
}

#[test]
fn the_kernel_release_loses_its_line_end_and_is_none_when_missing_or_empty() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(kernel_in(root.path()), None);
    let file = root.path().join(KERNEL_RELEASE_FILE);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "\n").unwrap();
    assert_eq!(kernel_in(root.path()), None);
    std::fs::write(&file, "6.17.1-arch1-1\n").unwrap();
    assert_eq!(kernel_in(root.path()).as_deref(), Some("6.17.1-arch1-1"));
}

#[test]
fn the_component_macro_takes_the_package_or_a_given_name_and_the_package_version() {
    let own = component!();
    assert_eq!(
        own,
        Component {
            name: "cupertino-build-info",
            version: env!("CARGO_PKG_VERSION"),
        }
    );
    let named = component!("cupertino");
    assert_eq!((named.name, named.version), ("cupertino", own.version));
    assert_eq!(named.commit(), COMMIT);
}

#[test]
fn a_component_prints_its_short_commit_when_the_build_knows_it() {
    let text = component!("cupertino").to_string();
    match COMMIT {
        Some(commit) => {
            assert_eq!(
                text,
                format!(
                    "cupertino {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    &commit[..12]
                )
            );
            assert_eq!(short_commit(), Some(&commit[..12]));
        }
        None => assert_eq!(text, format!("cupertino {}", env!("CARGO_PKG_VERSION"))),
    }
}

#[test]
fn the_commit_is_a_full_lower_case_hash_with_its_source() {
    match COMMIT {
        Some(commit) => {
            assert!(matches!(commit.len(), 40 | 64), "{commit}");
            assert!(commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert_ne!(COMMIT_SOURCE, CommitSource::Unknown);
        }
        None => assert_eq!(COMMIT_SOURCE, CommitSource::Unknown),
    }
    for source in [
        CommitSource::Environment,
        CommitSource::Archive,
        CommitSource::Checkout,
        CommitSource::Unknown,
    ] {
        assert_eq!(CommitSource::from_name(source.name()), source);
    }
}
