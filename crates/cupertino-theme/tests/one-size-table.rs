//! Control sizes come from one table, `control_sizes`: no crate of the workspace reads the per-control height table
//! that `metrics` once held beside it, by any path.

use std::fs;
use std::path::{Path, PathBuf};

/// The items that table was read through.
const REMOVED: [&str; 5] = [
    "alignment_height",
    "control_metric",
    "ControlMetric",
    "CONTROL_METRICS",
    "CONTROL_METRICS_EVIDENCE",
];

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The identifiers of `source`, in order.
fn words(source: &str) -> Vec<&str> {
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `source` names the table's control enum: `Control` in a `use` of the theme crate, or `metrics::Control`.
fn names_control(source: &str) -> bool {
    let spaced: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let uses_theme = spaced
        .split(';')
        .filter(|s| s.contains("cupertino_theme::"))
        .any(|s| words(s).contains(&"Control"));
    let qualified = spaced.match_indices("metrics::Control").any(|(i, m)| {
        !spaced[i + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    });
    uses_theme || qualified
}

#[test]
fn no_crate_reads_the_removed_control_height_table() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in fs::read_dir(&crates)
        .expect("the workspace's crates")
        .flatten()
    {
        for part in ["src", "tests", "examples", "benches", "build.rs"] {
            let path = dir.path().join(part);
            if path.is_file() {
                files.push(path);
            } else {
                rust_files(&path, &mut files);
            }
        }
    }
    let this = Path::new(file!()).file_name().unwrap();
    files.retain(|f| f.file_name() != Some(this));
    assert!(
        files.len() > 100,
        "the workspace's sources: {}",
        files.len()
    );
    let mut readers = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).unwrap_or_default();
        let named = words(&source);
        for removed in REMOVED {
            if named.contains(&removed) {
                readers.push(format!("{}: {removed}", file.display()));
            }
        }
        if names_control(&source) {
            readers.push(format!("{}: metrics::Control", file.display()));
        }
    }
    assert!(readers.is_empty(), "{readers:#?}");
}

#[test]
fn the_check_finds_each_way_of_naming_the_table() {
    assert!(names_control(
        "use cupertino_theme::metrics::{alignment_height, Control};"
    ));
    assert!(names_control(
        "use cupertino_theme::{\n    Color,\n    Control,\n};"
    ));
    assert!(names_control(
        "let c = cupertino_theme::metrics::Control::Switch;"
    ));
    assert!(!names_control("use cupertino_theme::metrics::ControlSize;"));
    assert!(!names_control(
        "use cupertino_ui_theme::{Control, ControlMetrics};"
    ));
    assert!(words("x.alignment_height(s)").contains(&"alignment_height"));
    assert!(!words("ControlMetrics::regular()").contains(&"ControlMetric"));
}
