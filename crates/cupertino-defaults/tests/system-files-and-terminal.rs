//! `system`: trust in files only root can change; `terminal`: choosing the session's terminal.

use std::os::unix::fs::symlink;
use std::path::Path;

use cupertino_defaults::system::*;
use cupertino_defaults::terminal::*;

fn running_as_root() -> bool {
    // Safety: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn a_regular_file_of_root_nobody_else_can_write_is_root_only() {
    let reg = libc::S_IFREG;
    for (uid, mode, expected, row) in [
        (0, reg | 0o644, true, "root's, read by all"),
        (0, reg | 0o600, true, "root's, private"),
        (1000, reg | 0o644, false, "not root's"),
        (0, reg | 0o664, false, "group can write"),
        (0, reg | 0o646, false, "others can write"),
        (0, libc::S_IFDIR | 0o755, false, "a directory"),
        (0, libc::S_IFLNK | 0o755, false, "a link"),
    ] {
        assert_eq!(is_root_only(uid, mode), expected, "{row}");
    }
}

#[test]
fn only_a_system_file_reached_without_a_users_link_is_one_only_root_can_change() {
    let sh = Path::new("/bin/sh");
    if sh.exists() {
        let target = root_only_target(sh).expect("the system shell is root's");
        assert!(target.is_absolute());
        assert!(only_root_can_change(&target));
    }
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join("f");
    std::fs::write(&own, "x").unwrap();
    let link = dir.path().join("sh");
    symlink("/bin/sh", &link).unwrap();
    let mut rows = vec![
        (Path::new("etc/passwd"), "relative"),
        (Path::new("/nonexistent/for/this/test"), "missing"),
    ];
    if !running_as_root() {
        rows.push((own.as_path(), "the user's file"));
        rows.push((link.as_path(), "the user's link"));
    }
    for (path, row) in rows {
        assert_eq!(root_only_target(path), None, "{row}");
        if row != "the user's link" {
            assert!(!only_root_can_change(path), "{row}");
        }
    }
}

#[test]
fn a_root_file_reads_only_when_root_owns_it_and_no_link_leads_to_it() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        read_root_file(&dir.path().join("absent.conf")).unwrap(),
        None,
        "missing"
    );
    let link = dir.path().join("passwd");
    symlink("/etc/passwd", &link).unwrap();
    assert!(read_root_file(&link).is_err(), "a link is not followed");
    if !running_as_root() {
        let own = dir.path().join("f.conf");
        std::fs::write(&own, "x").unwrap();
        assert_eq!(
            read_root_file(&own).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
            "the user's file"
        );
    }
    let text = read_root_file(Path::new("/etc/passwd")).unwrap();
    assert!(text.is_some_and(|t| t.contains("root")), "a system file");
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
fn the_terminal_is_the_launcher_else_the_variable_else_a_known_emulator() {
    let defaults: Vec<&str> = DEFAULT_TERMINALS.to_vec();
    for (row, list, vars, installed, expected) in [
        (
            "the launcher wins",
            defaults.clone(),
            &[("TERMINAL", "kitty")][..],
            &["xdg-terminal-exec", "kitty", "foot"][..],
            Some("xdg-terminal-exec"),
        ),
        (
            "the variable without a launcher",
            defaults.clone(),
            &[("TERMINAL", "kitty")],
            &["kitty", "foot"],
            Some("kitty"),
        ),
        (
            "unset variable",
            defaults.clone(),
            &[],
            &["alacritty"],
            Some("alacritty"),
        ),
        (
            "empty variable",
            defaults.clone(),
            &[("TERMINAL", "")],
            &["alacritty"],
            Some("alacritty"),
        ),
        (
            "variable not installed",
            defaults.clone(),
            &[("TERMINAL", "not-installed")],
            &["alacritty"],
            Some("alacritty"),
        ),
        (
            "xterm last",
            defaults.clone(),
            &[],
            &["xterm"],
            Some("xterm"),
        ),
        ("nothing installed", defaults.clone(), &[], &[], None),
        (
            "configured entries decide alone",
            vec!["$MYTERM", "wezterm"],
            &[("MYTERM", "ghostty")],
            &["ghostty", "wezterm"],
            Some("ghostty"),
        ),
        (
            "configured entries ignore the known emulators",
            vec!["$MYTERM", "wezterm"],
            &[],
            &["foot"],
            None,
        ),
    ] {
        let list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        let found = resolve(&list, env_of(vars), |p| installed.contains(&p));
        assert_eq!(found.as_deref(), expected, "{row}");
    }
}

#[test]
fn the_launcher_takes_the_command_directly_and_other_terminals_after_dash_e() {
    for (terminal, expected) in [
        ("xdg-terminal-exec", &["xdg-terminal-exec"][..]),
        (
            "/usr/bin/xdg-terminal-exec",
            &["/usr/bin/xdg-terminal-exec"],
        ),
        ("foot", &["foot", "-e"]),
    ] {
        assert_eq!(command_prefix(terminal), expected, "{terminal}");
    }
}

#[test]
fn an_entry_naming_a_path_is_installed_when_the_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let term = dir.path().join("myterm");
    std::fs::write(&term, "").unwrap();
    let list = vec![
        dir.path().join("absent").to_str().unwrap().to_string(),
        term.to_str().unwrap().to_string(),
    ];
    assert_eq!(
        resolve_installed(&list),
        Some(term.to_str().unwrap().to_string())
    );
}
