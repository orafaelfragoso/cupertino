//! The server's desktop entries: which application an identity is, as the dock resolves it.

use cupertino_server::desktop::{DesktopEntry, DesktopIndex};

const T3_SHOWN: &str = "T3-Code-x86_64";
const T3_HIDDEN: &str = "com.t3tools.T3Code";
const T3_EXEC: &str = "\"/home/user/.local/bin/T3-Code-x86_64.AppImage\" %U";

fn entry(id: &str, exec: &str, no_display: bool) -> DesktopEntry {
    DesktopEntry {
        id: id.to_owned(),
        name: id.to_owned(),
        exec: exec.to_owned(),
        no_display,
        ..DesktopEntry::default()
    }
}

/// An application with the entry people start it by and a hidden URL handler named by its windows' id, and a portal's
/// hidden entry alone.
fn index() -> DesktopIndex {
    DesktopIndex {
        entries: vec![
            entry(T3_SHOWN, T3_EXEC, false),
            entry(T3_HIDDEN, T3_EXEC, true),
            entry(
                "org.example.Portal",
                "/usr/lib/example-portal --replace",
                true,
            ),
        ],
    }
}

#[test]
fn a_hidden_entry_stands_for_the_shown_entry_that_runs_its_program() {
    let index = index();
    assert_eq!(index.find(T3_HIDDEN).map(|e| e.id.as_str()), Some(T3_SHOWN));
    assert_eq!(index.find(T3_SHOWN).map(|e| e.id.as_str()), Some(T3_SHOWN));
    assert_eq!(
        cupertino_server::desktop::display_name(&index, T3_HIDDEN),
        T3_SHOWN
    );
    let portal = index
        .find("org.example.Portal")
        .expect("the portal's entry");
    assert!(
        portal.no_display,
        "a helper without a shown sibling stays hidden"
    );
}
