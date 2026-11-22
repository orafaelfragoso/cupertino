//! What opening an application asks for: the command, and the application its activation token names.

use cupertino_launcher::engine::{Action, Activation, ItemKind, ResultItem};
use cupertino_launcher::{DesktopEntry, Frecency, Launcher, LauncherConfig};

fn entry(id: &str, exec: &str, terminal: bool) -> DesktopEntry {
    DesktopEntry {
        id: id.to_owned(),
        name: id.to_owned(),
        generic_name: String::new(),
        comment: String::new(),
        keywords: Vec::new(),
        categories: Vec::new(),
        icon: None,
        exec: exec.to_owned(),
        try_exec: None,
        terminal,
        path: None,
        startup_wm_class: None,
        actions: Vec::new(),
        source: None,
    }
}

fn item(desktop_id: &str) -> ResultItem {
    ResultItem {
        id: desktop_id.to_owned(),
        kind: ItemKind::Application,
        title: desktop_id.to_owned(),
        subtitle: String::new(),
        icon: None,
        score: 1.0,
        highlights: Vec::new(),
        action: Action::LaunchApp {
            desktop_id: desktop_id.to_owned(),
            action: None,
        },
    }
}

fn opened(launcher: &mut Launcher, desktop_id: &str) -> cupertino_launcher::CommandSpec {
    match launcher.activate(&item(desktop_id), "", 0.0) {
        Activation::OpenApp { spec, .. } => spec,
        other => panic!("{desktop_id} opens as an application, not {other:?}"),
    }
}

#[test]
fn a_terminal_entrys_token_names_the_terminal_whose_window_comes() {
    let config = LauncherConfig {
        terminal_command: vec!["/usr/bin/foot".to_owned(), "-e".to_owned()],
        ..LauncherConfig::default()
    };
    let mut launcher = Launcher::new(
        config,
        vec![
            entry("htop", "htop", true),
            entry("org.example.Mail", "mail", false),
        ],
        Frecency::in_memory(),
    );
    let htop = opened(&mut launcher, "htop");
    assert_eq!(htop.program, "/usr/bin/foot");
    assert_eq!(htop.args, ["-e", "htop"]);
    assert_eq!(htop.desktop_id.as_deref(), Some("foot"));
    let mail = opened(&mut launcher, "org.example.Mail");
    assert_eq!(mail.desktop_id.as_deref(), Some("org.example.Mail"));
}
