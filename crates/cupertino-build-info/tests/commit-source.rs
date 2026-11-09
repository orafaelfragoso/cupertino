//! The order in which the build script looks for the commit, run on temporary source trees.

#[path = "../build/commit.rs"]
mod commit;

use commit::{resolve, Source, ARCHIVE_FILE, UNSUBSTITUTED};
use std::ffi::OsStr;
use std::path::Path;

const A: &str = "0123456789abcdef0123456789abcdef01234567";
const B: &str = "89abcdef0123456789abcdef0123456789abcdef";
const C: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// A source tree holding `data/build-commit` with `archive` (none when `None`), and a `.git` directory when `git`.
fn tree(archive: Option<&str>, git: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    if let Some(text) = archive {
        std::fs::create_dir_all(root.path().join("data")).unwrap();
        std::fs::write(root.path().join(ARCHIVE_FILE), text).unwrap();
    }
    if git {
        std::fs::create_dir(root.path().join(".git")).unwrap();
    }
    root
}

/// A checkout whose HEAD is `C`; it fails the test when the order should not have asked it.
fn head(_: &Path) -> Option<String> {
    Some(format!("{C}\n"))
}

fn never(_: &Path) -> Option<String> {
    panic!(
        "the checkout is asked only when the variable and the archive file do not know the commit"
    )
}

#[test]
fn the_variable_wins_over_the_archive_file_and_the_checkout() {
    let root = tree(Some(&format!("{B}\n")), true);
    let found = resolve(Some(OsStr::new(A)), root.path(), never).unwrap();
    assert_eq!(
        (found.commit.as_deref(), found.source),
        (Some(A), Source::Environment)
    );
    let upper = resolve(Some(OsStr::new(&A.to_uppercase())), root.path(), never).unwrap();
    assert_eq!(upper.commit.as_deref(), Some(A), "upper case is lowered");
}

#[test]
fn a_variable_that_is_not_a_full_hash_fails_the_build() {
    let root = tree(Some(B), true);
    for bad in [
        "",
        "abc123",
        &A[..39],
        &format!("{A}0"),
        &A.replace('0', "g"),
    ] {
        let err = resolve(Some(OsStr::new(bad)), root.path(), never).unwrap_err();
        assert!(err.contains("CUPERTINO_BUILD_COMMIT"), "{err}");
    }
    let sha256 = format!("{A}{}", &B[..24]);
    assert_eq!(
        resolve(Some(OsStr::new(&sha256)), root.path(), never)
            .unwrap()
            .commit,
        Some(sha256),
        "a hash of a repository with sha256 object names is full too"
    );
}

#[test]
fn a_substituted_archive_file_wins_over_the_checkout() {
    let root = tree(Some(&format!("{B}\n")), true);
    let found = resolve(None, root.path(), never).unwrap();
    assert_eq!(
        (found.commit.as_deref(), found.source, found.warnings.len()),
        (Some(B), Source::Archive, 0)
    );
}

#[test]
fn the_literal_placeholder_falls_through_to_the_checkout() {
    let root = tree(Some(&format!("{UNSUBSTITUTED}\n")), true);
    let found = resolve(None, root.path(), head).unwrap();
    assert_eq!(
        (found.commit.as_deref(), found.source, found.warnings.len()),
        (Some(C), Source::Checkout, 0)
    );
}

#[test]
fn a_git_file_of_a_linked_worktree_counts_as_a_checkout() {
    let root = tree(Some(UNSUBSTITUTED), false);
    std::fs::write(root.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
    let found = resolve(None, root.path(), head).unwrap();
    assert_eq!(found.source, Source::Checkout);
}

#[test]
fn without_git_the_placeholder_leaves_the_commit_unknown() {
    let root = tree(Some(UNSUBSTITUTED), false);
    let found = resolve(None, root.path(), never).unwrap();
    assert_eq!(
        (found.commit, found.source, found.warnings.len()),
        (None, Source::Unknown, 0)
    );
    let bare = tree(None, false);
    assert_eq!(
        resolve(None, bare.path(), never).unwrap().source,
        Source::Unknown
    );
}

#[test]
fn a_garbled_archive_file_is_skipped_with_a_warning() {
    let root = tree(Some("not a commit\n"), true);
    let found = resolve(None, root.path(), head).unwrap();
    assert_eq!(found.source, Source::Checkout);
    assert!(
        found.warnings[0].contains("not a commit"),
        "{:?}",
        found.warnings
    );
}

#[test]
fn a_checkout_git_cannot_read_leaves_the_commit_unknown_with_a_warning() {
    let root = tree(Some(UNSUBSTITUTED), true);
    let found = resolve(None, root.path(), |_| None).unwrap();
    assert_eq!((found.commit, found.source), (None, Source::Unknown));
    assert_eq!(found.warnings.len(), 1);
    let garbage = resolve(None, root.path(), |_| Some("ref: refs/heads/x".into())).unwrap();
    assert_eq!(garbage.source, Source::Unknown);
}

#[test]
fn git_reads_the_head_of_a_real_repository() {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join(ARCHIVE_FILE), format!("{UNSUBSTITUTED}\n")).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "source"]);
    let found = resolve(None, root.path(), commit::git_head).unwrap();
    assert_eq!(found.source, Source::Checkout);
    let commit = found.commit.unwrap();
    assert_eq!(commit::valid(&commit).as_deref(), Some(commit.as_str()));
}

#[test]
fn the_names_the_script_passes_are_the_ones_the_crate_reads() {
    use cupertino_build_info::CommitSource;
    for (script, crate_side) in [
        (Source::Environment, CommitSource::Environment),
        (Source::Archive, CommitSource::Archive),
        (Source::Checkout, CommitSource::Checkout),
        (Source::Unknown, CommitSource::Unknown),
    ] {
        assert_eq!(script.name(), crate_side.name());
    }
}
