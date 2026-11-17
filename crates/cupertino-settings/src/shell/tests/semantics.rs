//! What a screen reader announces of each pane and each sheet: the role, name, value and states of their nodes, read
//! from the accessibility tree the window builds headless. The panes that show only with a network or Bluetooth
//! service, or with a touchpad, open against fake daemons on a private bus and a fake sysfs.

use cupertino_filepicker::Mode;
use cupertino_kit::core::AccessibleValue;
use cupertino_ui_test::{semantic_outline, unannounced, whole_tree};

use super::*;
use crate::clipboard::App;
use crate::hardware::Sources;
use crate::sheet::Prompt;

// the fake daemons live with the integration tests, outside `src`
mod fakes {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fakes/semantics.rs"
    ));
}
use fakes::{FakeBluetooth, FakeNetwork};

fn tree(driver: &TestDriver<SettingsApp>, window: WindowId) -> AccessibilityUpdate {
    let snapshot = driver.runtime().snapshot(window).unwrap();
    whole_tree(window, snapshot.accessibility, snapshot.focus)
}

/// The nodes a screen reader would read a secret of: a password field, or a text field named as one, whose value is
/// text.
fn secrets_read(tree: &AccessibilityUpdate) -> Vec<String> {
    tree.nodes
        .iter()
        .filter(|(_, n)| {
            let named = n.name.as_deref().is_some_and(|name| {
                let name = name.to_lowercase();
                name.contains("password") || name.contains("passphrase")
            });
            (n.role == Role::PasswordInput || (n.role == Role::TextInput && named))
                && matches!(n.value, Some(AccessibleValue::Text { .. }))
        })
        .map(|(_, n)| cupertino_ui_test::describe(n))
        .collect()
}

/// Every line of `expected` is read by a node of the window, no node lacks a name and no secret is read out.
fn announces(driver: &TestDriver<SettingsApp>, window: WindowId, what: &str, expected: &[&str]) {
    let tree = tree(driver, window);
    let outline = semantic_outline(&tree).join("\n");
    assert_eq!(
        unannounced(&tree, expected),
        Vec::<&str>::new(),
        "{what} reads:\n{outline}"
    );
    assert_eq!(
        kit_gaps(&tree),
        Vec::<String>::new(),
        "{what} reads:\n{outline}"
    );
    assert_eq!(
        secrets_read(&tree),
        Vec::<String>::new(),
        "{what} reads:\n{outline}"
    );
}

/// What every pane reads besides its content: its sidebar row selected, the history buttons with no history, the
/// search field and the sidebar's splitter.
fn chrome(title: &str) -> [String; 5] {
    [
        format!("ListItem {title:?} focusable selected"),
        "Button \"Back\" disabled".to_owned(),
        "Button \"Forward\" disabled".to_owned(),
        "TextInput \"Search\" = \"\" focusable".to_owned(),
        "Splitter \"Sidebar\" = 232 of 0..400 focusable".to_owned(),
    ]
}

fn pane_announces(pane: PaneId, title: &str, content: &[&str], services: Option<Services>) {
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = match services {
        Some(services) => start_on(dir.path(), pane, None, services),
        None => start(dir.path(), pane),
    };
    settle(&mut driver, window);
    let chrome = chrome(title);
    let mut expected: Vec<&str> = chrome.iter().map(String::as_str).collect();
    expected.extend(content);
    announces(&driver, window, &format!("{pane:?}"), &expected);
}

/// What a sub-page reads besides its content: its pane's sidebar row still selected, its own title in the toolbar,
/// and a history to go back through.
fn sub_page_chrome(pane: &str, title: &str) -> [String; 6] {
    [
        format!("ListItem {pane:?} focusable selected"),
        format!("StaticText {title:?}"),
        "Button \"Back\" focusable".to_owned(),
        "Button \"Forward\" disabled".to_owned(),
        "TextInput \"Search\" = \"\" focusable".to_owned(),
        "Splitter \"Sidebar\" = 232 of 0..400 focusable".to_owned(),
    ]
}

// ---- the panes of a service or a device ----------------------------------------------------------------------

// ---- sheets ---------------------------------------------------------------------------------------------------

/// The whole tree of the window with `sheet` up: the sheet alone, a modal dialog over a content no reader reaches.
fn sheet_reads(dir: &Path, sheet: Sheet, what: &str) -> Vec<String> {
    let (driver, window) = start_with(dir, PaneId::Wifi, Some(sheet));
    let tree = tree(&driver, window);
    assert_eq!(kit_gaps(&tree), Vec::<String>::new(), "{what}");
    assert_eq!(secrets_read(&tree), Vec::<String>::new(), "{what}");
    semantic_outline(&tree)
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}
