// Exact fixture artifacts supplied by `ci/build-test-fixtures.py`, never built by individual tests.
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

const PREFIX: &str = "CUPERTINO_TEST_FIXTURE_";
const PREPARE: &str = "run through python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features (pass --release to both commands for release tests)";

pub fn artifact(package: &str, kind: &str, name: &str) -> PathBuf {
    let session = std::env::var_os(format!("{PREFIX}SESSION")).expect(PREPARE);
    assert!(
        std::path::Path::new(&session).is_dir(),
        "expired fixture session: {PREPARE}"
    );
    let key = format!("{PREFIX}{package}_{kind}_{name}")
        .to_uppercase()
        .replace('-', "_");
    let path = PathBuf::from(
        std::env::var_os(&key).unwrap_or_else(|| panic!("missing fixture {key}: {PREPARE}")),
    );
    let expected = std::env::var(format!("{key}_STAMP")).expect("fixture artifact stamp");
    let metadata = std::fs::metadata(&path).expect("prepared fixture artifact");
    let modified = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    assert_eq!(
        format!("{}:{modified}", metadata.len()),
        expected,
        "fixture changed after preparation: {}",
        path.display()
    );
    // Test and fixture must come from the same target/profile, including a cross-compilation target directory.
    let exe = std::env::current_exe().unwrap();
    let profile = exe.parent().unwrap().parent().unwrap();
    let parent = path.parent().unwrap();
    let fixture_profile = if matches!(
        parent.file_name().unwrap().to_str(),
        Some("examples" | "deps")
    ) {
        parent.parent().unwrap()
    } else {
        parent
    };
    assert_eq!(
        fixture_profile, profile,
        "fixture and test profiles differ: {PREPARE}"
    );
    path
}

/// Keep fixture provenance when a test starts another copy of itself with a cleared environment.
pub fn forward(command: &mut std::process::Command) {
    for (key, value) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with(PREFIX)) {
            command.env(key, value);
        }
    }
}
