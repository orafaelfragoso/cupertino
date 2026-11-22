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

/// Entries whose `Exec` starts with a program that runs another one (`env`, `flatpak run`, a shell, an interpreter),
/// and the shown entry each hidden one stands for, by its ID, or `None` when it stays itself.
fn wrapped() -> (DesktopIndex, Vec<(&'static str, Option<&'static str>)>) {
    let entries = vec![
        // the person's file manager, started through env
        entry("org.kde.dolphin", "env LANGUAGE=en_GB:en dolphin %u", false),
        entry(
            "org.kde.dolphin.handler",
            "env -u QT_SCALE_FACTOR dolphin --select %u",
            true,
        ),
        entry(
            "org.example.Helper",
            "env GDK_BACKEND=x11 /usr/lib/example-helper",
            true,
        ),
        // Flatpak applications
        entry(
            "org.mozilla.firefox",
            "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@",
            false,
        ),
        entry(
            "org.mozilla.firefox.handler",
            "/usr/bin/flatpak run --command=firefox org.mozilla.firefox --new-window %u",
            true,
        ),
        entry(
            "org.example.Sandboxed",
            "/usr/bin/flatpak run org.example.Sandboxed",
            true,
        ),
        // shells and interpreters
        entry("org.example.Shelled", "sh -c \"cd /opt/a && ./run\"", false),
        entry("org.example.OtherShelled", "sh -c \"cd /opt/b && ./run\"", true),
        entry("org.example.Script", "python3 /opt/a/main.py", false),
        entry(
            "org.example.OtherScript",
            "python3.12 -X dev /opt/b/tool.py",
            true,
        ),
        entry(
            "org.example.ScriptHandler",
            "python3 /opt/a/main.py --open %u",
            true,
        ),
        // a program two shown entries run
        entry("org.example.SuiteWriter", "/opt/suite/launch --writer", false),
        entry("org.example.SuiteSheets", "/opt/suite/launch --sheets", false),
        entry("org.example.SuiteHandler", "/opt/suite/launch %U", true),
    ];
    let expected = vec![
        ("org.kde.dolphin.handler", Some("org.kde.dolphin")),
        ("org.example.Helper", None),
        ("org.mozilla.firefox.handler", Some("org.mozilla.firefox")),
        ("org.example.Sandboxed", None),
        ("org.example.OtherShelled", None),
        ("org.example.OtherScript", None),
        ("org.example.ScriptHandler", Some("org.example.Script")),
        ("org.example.SuiteHandler", None),
    ];
    (DesktopIndex { entries }, expected)
}

#[test]
fn a_hidden_entry_stands_for_the_one_shown_entry_its_wrapped_program_is() {
    let (index, expected) = wrapped();
    for (hidden, shown) in expected {
        let found = index.find(hidden).expect("the entry");
        assert_eq!(found.id, shown.unwrap_or(hidden), "{hidden}");
        // what Desktop Control names an application by is the entry itself
        assert_eq!(index.named(hidden).map(|e| e.id.as_str()), Some(hidden));
    }
}
