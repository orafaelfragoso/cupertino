//! `system`: trust in files only root can change; `terminal`: choosing the session's terminal.

use std::os::unix::fs::symlink;
use std::path::Path;

use cupertino_defaults::system::*;
use cupertino_defaults::terminal::*;

fn owned_by_root() -> bool {
    // Safety: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

// system

#[test]
fn a_regular_file_of_root_nobody_else_can_write_is_root_only() {
    let reg = libc::S_IFREG;
    assert!(is_root_only(0, reg | 0o644));
    assert!(is_root_only(0, reg | 0o600));
    assert!(!is_root_only(1000, reg | 0o644), "not root's");
    assert!(!is_root_only(0, reg | 0o664), "group can write");
    assert!(!is_root_only(0, reg | 0o646), "others can write");
    assert!(
        !is_root_only(0, libc::S_IFDIR | 0o755),
        "not a regular file"
    );
    assert!(!is_root_only(0, libc::S_IFLNK | 0o755));
}

#[test]
fn a_file_of_the_user_is_not_a_file_only_root_can_change() {
    if owned_by_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, "x").unwrap();
    assert!(!only_root_can_change(&path));
    assert_eq!(root_only_target(&path), None);
}

#[test]
fn a_relative_or_missing_path_is_not_a_file_only_root_can_change() {
    assert!(!only_root_can_change(Path::new("etc/passwd")));
    assert!(!only_root_can_change(Path::new(
        "/nonexistent/for/this/test"
    )));
    assert_eq!(root_only_target(Path::new("etc/passwd")), None);
    assert_eq!(
        root_only_target(Path::new("/nonexistent/for/this/test")),
        None
    );
}

#[test]
fn a_program_of_the_system_is_a_file_only_root_can_change() {
    let sh = Path::new("/bin/sh");
    if !sh.exists() {
        return;
    }
    let target = root_only_target(sh).expect("the system shell is root's");
    assert!(target.is_absolute());
    assert!(only_root_can_change(&target));
}

#[test]
fn a_link_of_the_user_leads_nowhere() {
    if owned_by_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("sh");
    symlink("/bin/sh", &link).unwrap();
    assert_eq!(root_only_target(&link), None);
}

#[test]
fn a_root_file_that_is_missing_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        read_root_file(&dir.path().join("absent.conf")).unwrap(),
        None
    );
}

#[test]
fn a_root_file_of_the_user_is_refused() {
    if owned_by_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.conf");
    std::fs::write(&path, "x").unwrap();
    let err = read_root_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn a_link_is_not_followed_when_reading_a_root_file() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("passwd");
    symlink("/etc/passwd", &link).unwrap();
    assert!(read_root_file(&link).is_err());
}

#[test]
fn a_root_file_of_the_system_reads() {
    let text = read_root_file(Path::new("/etc/passwd")).unwrap();
    assert!(text.is_some_and(|t| t.contains("root")));
}

// terminal

fn entries(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn the_default_terminal_launcher_wins_over_the_variable_and_the_known_emulators() {
    let list = entries(&DEFAULT_TERMINALS);
    let found = resolve(&list, env_of(&[("TERMINAL", "kitty")]), |p| {
        ["xdg-terminal-exec", "kitty", "foot"].contains(&p)
    });
    assert_eq!(found.as_deref(), Some("xdg-terminal-exec"));
}

#[test]
fn the_variable_is_used_when_there_is_no_launcher() {
    let list = entries(&DEFAULT_TERMINALS);
    let found = resolve(&list, env_of(&[("TERMINAL", "kitty")]), |p| {
        ["kitty", "foot"].contains(&p)
    });
    assert_eq!(found.as_deref(), Some("kitty"));
}

#[test]
fn an_unset_empty_or_missing_variable_falls_through_to_the_known_emulators() {
    let list = entries(&DEFAULT_TERMINALS);
    for vars in [
        &[][..],
        &[("TERMINAL", "")][..],
        &[("TERMINAL", "not-installed")][..],
    ] {
        let found = resolve(&list, env_of(vars), |p| p == "alacritty");
        assert_eq!(found.as_deref(), Some("alacritty"));
    }
}

#[test]
fn xterm_is_the_last_resort_and_nothing_installed_gives_none() {
    let list = entries(&DEFAULT_TERMINALS);
    assert_eq!(
        resolve(&list, env_of(&[]), |p| p == "xterm").as_deref(),
        Some("xterm")
    );
    assert_eq!(resolve(&list, env_of(&[]), |_| false), None);
}

#[test]
fn configured_entries_decide_alone() {
    let list = entries(&["$MYTERM", "wezterm"]);
    assert_eq!(
        resolve(&list, env_of(&[("MYTERM", "ghostty")]), |_| true).as_deref(),
        Some("ghostty")
    );
    assert_eq!(resolve(&list, env_of(&[]), |p| p == "foot"), None);
}

#[test]
fn the_launcher_takes_the_command_directly_and_other_terminals_after_dash_e() {
    assert_eq!(command_prefix("xdg-terminal-exec"), ["xdg-terminal-exec"]);
    assert_eq!(
        command_prefix("/usr/bin/xdg-terminal-exec"),
        ["/usr/bin/xdg-terminal-exec"]
    );
    assert_eq!(command_prefix("foot"), ["foot", "-e"]);
}

#[test]
fn an_entry_naming_a_path_is_installed_when_the_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let term = dir.path().join("myterm");
    std::fs::write(&term, "").unwrap();
    let list = entries(&[
        dir.path().join("absent").to_str().unwrap(),
        term.to_str().unwrap(),
    ]);
    assert_eq!(
        resolve_installed(&list),
        Some(term.to_str().unwrap().to_string())
    );
}
