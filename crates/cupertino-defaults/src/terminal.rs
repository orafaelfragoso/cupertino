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
