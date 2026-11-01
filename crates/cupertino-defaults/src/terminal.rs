//! The terminal emulator of the session. A list of entries is tried in order and the first installed one is used: the
//! default-terminal launcher of the XDG terminal specification (`xdg-terminal-exec`, which reads the user's choice),
//! the program `$TERMINAL` names, then known emulators. An entry `$NAME` stands for the program the environment
//! variable `NAME` holds; an unset or empty variable skips it.

use std::path::Path;

/// The default-terminal launcher of the XDG terminal specification: it starts the user's terminal and takes the command
/// to run as plain arguments.
const XDG_TERMINAL_EXEC: &str = "xdg-terminal-exec";

/// The entries tried when the settings name no terminals.
pub const DEFAULT_TERMINALS: [&str; 7] = [
    XDG_TERMINAL_EXEC,
    "$TERMINAL",
    "foot",
    "alacritty",
    "kitty",
    "wezterm",
    "xterm",
];

/// The program of the first entry that names an installed program. `variable` reads an environment variable and
/// `installed` tells whether a program exists.
pub fn resolve(
    entries: &[String],
    variable: impl Fn(&str) -> Option<String>,
    installed: impl Fn(&str) -> bool,
) -> Option<String> {
    entries.iter().find_map(|entry| {
        let program = match entry.strip_prefix('$') {
            Some(name) => variable(name).filter(|value| !value.is_empty())?,
            None => entry.clone(),
        };
        installed(&program).then_some(program)
    })
}

/// [`resolve`] with the process environment and `PATH`.
pub fn resolve_installed(entries: &[String]) -> Option<String> {
    resolve(entries, |name| std::env::var(name).ok(), on_path)
}

/// The argument vector that starts `program` as a terminal running a command: the command follows it directly for
/// `xdg-terminal-exec` and after `-e` for every other emulator.
pub fn command_prefix(program: &str) -> Vec<String> {
    let mut prefix = vec![program.to_owned()];
    if Path::new(program).file_name().and_then(|n| n.to_str()) != Some(XDG_TERMINAL_EXEC) {
        prefix.push("-e".to_owned());
    }
    prefix
}

fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|e| (*e).to_owned()).collect()
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn present<'a>(programs: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |p| programs.contains(&p)
    }

    #[test]
    fn the_default_terminal_launcher_wins_over_the_variable_and_the_known_emulators() {
        let list = entries(&DEFAULT_TERMINALS);
        let got = resolve(
            &list,
            env(&[("TERMINAL", "kitty")]),
            present(&["xdg-terminal-exec", "kitty", "xterm"]),
        );
        assert_eq!(got.as_deref(), Some("xdg-terminal-exec"));
    }

    #[test]
    fn the_variable_is_used_when_there_is_no_launcher() {
        let list = entries(&DEFAULT_TERMINALS);
        let got = resolve(
            &list,
            env(&[("TERMINAL", "kitty")]),
            present(&["kitty", "xterm"]),
        );
        assert_eq!(got.as_deref(), Some("kitty"));
    }

    #[test]
    fn an_unset_empty_or_missing_variable_falls_through_to_the_known_emulators() {
        let list = entries(&DEFAULT_TERMINALS);
        for vars in [
            &[][..],
            &[("TERMINAL", "")][..],
            &[("TERMINAL", "gone")][..],
        ] {
            let got = resolve(&list, env(vars), present(&["alacritty", "xterm"]));
            assert_eq!(got.as_deref(), Some("alacritty"));
        }
    }

    #[test]
    fn xterm_is_the_last_resort_and_nothing_installed_gives_none() {
        let list = entries(&DEFAULT_TERMINALS);
        assert_eq!(
            resolve(&list, env(&[]), present(&["xterm"])).as_deref(),
            Some("xterm")
        );
        assert_eq!(resolve(&list, env(&[]), present(&[])), None);
    }

    #[test]
    fn configured_entries_decide_alone() {
        let got = resolve(
            &entries(&["wezterm"]),
            env(&[("TERMINAL", "kitty")]),
            present(&["xdg-terminal-exec", "kitty", "wezterm"]),
        );
        assert_eq!(got.as_deref(), Some("wezterm"));
        assert_eq!(
            resolve(&entries(&["wezterm"]), env(&[]), present(&["kitty"])),
            None
        );
    }

    #[test]
    fn the_launcher_takes_the_command_directly_and_other_terminals_after_dash_e() {
        assert_eq!(command_prefix("xdg-terminal-exec"), ["xdg-terminal-exec"]);
        assert_eq!(
            command_prefix("/usr/bin/xdg-terminal-exec"),
            ["/usr/bin/xdg-terminal-exec"]
        );
        assert_eq!(command_prefix("kitty"), ["kitty", "-e"]);
    }
}
