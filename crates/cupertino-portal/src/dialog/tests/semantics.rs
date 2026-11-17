//! What a screen reader announces of each portal dialog: the role, name, value and states of its nodes, as the
//! window opens and after a row is picked.

use cupertino_kit::core::AccessibilityUpdate;
use cupertino_ui_test::{semantic_gaps, semantic_outline, whole_tree};

use super::*;

fn tree(driver: &TestDriver<Dialogs>, window: WindowId) -> AccessibilityUpdate {
    let snapshot = driver.runtime().snapshot(window).unwrap();
    whole_tree(window, snapshot.accessibility, snapshot.focus)
}

/// The window's outline without the rows of its Places list, which are the session's folders and differ by machine.
fn outline(driver: &TestDriver<Dialogs>, window: WindowId) -> Vec<String> {
    let mut lines = Vec::new();
    let mut places: Option<usize> = None;
    for line in semantic_outline(&tree(driver, window)) {
        let depth = line.len() - line.trim_start().len();
        match places {
            Some(d) if depth > d => continue,
            _ => places = None,
        }
        if line.trim_start().starts_with("List \"Places\"") {
            places = Some(depth);
        }
        lines.push(line);
    }
    lines
}

/// What the window reads that a screen reader cannot announce, besides what Kit leaves unnamed in every window: the
/// root, and a list row, which has no name of its own and is read by its text (the outlines below show that text).
fn gaps(driver: &TestDriver<Dialogs>, window: WindowId) -> Vec<String> {
    let mut gaps = semantic_gaps(&tree(driver, window));
    gaps.retain(|gap| {
        gap != "Window #0: no name"
            && !(gap.contains(" > ListItem #") && gap.ends_with(": no name"))
    });
    gaps
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

/// The node assistive technology is told holds the focus is the Cancel button, so a screen reader announces it first.
fn announces_cancel(driver: &TestDriver<Dialogs>, window: WindowId) {
    let runtime = driver.runtime();
    let snapshot = runtime.snapshot(window).unwrap();
    let cancel = runtime.find(window, &Key::from("cancel"));
    assert!(cancel.is_some());
    assert_eq!(snapshot.accessibility_focus, cancel);
    let name = snapshot
        .accessibility
        .iter()
        .find(|(id, _)| Some(*id) == cancel)
        .and_then(|(_, node)| node.name.clone());
    assert_eq!(name.as_deref(), Some("Cancel"));
}

fn reads(driver: &TestDriver<Dialogs>, window: WindowId, expected: &[&str]) {
    assert_eq!(outline(driver, window), lines(expected));
    assert_eq!(gaps(driver, window), Vec::<String>::new());
}
