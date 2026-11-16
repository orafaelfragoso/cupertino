//! Programs built on Chromium: the browser and Electron applications. On Linux they show scroll bars over the content
//! only with Chromium's overlay scroll bar feature on; with it on, they follow GTK's `gtk-overlay-scrolling` setting,
//! changes included, while they run. A launch from a desktop entry turns the feature on ([`with_overlay_scroll_bars`]),
//! so the desktop's scroll bar setting reaches them through GTK.
//!
//! A program counts as built on Chromium when the file every Chromium build ships beside its executable is in the
//! program's folder or the folder above, or in the folder of a program its launcher names (a script or a small
//! program that starts the real one, read up to [`READ`] bytes, [`DEPTH`] launchers deep).

use std::io::Read;
use std::path::{Path, PathBuf};

/// The resource file every Chromium build ships beside its executable.
const MARKER: &str = "chrome_100_percent.pak";
/// The switch that turns Chromium features on, a comma-separated list after it.
const ENABLE: &str = "--enable-features=";
/// The switch that turns Chromium features off.
const DISABLE: &str = "--disable-features=";
/// Chromium's overlay scroll bar feature.
pub const OVERLAY_SCROLL_BARS: &str = "OverlayScrollbar";
/// How much of a launcher is read for the programs it names.
const READ: u64 = 64 * 1024;
/// How many launchers deep the real program is looked for.
const DEPTH: usize = 3;
/// How many absolute paths a launcher may name before the rest are ignored.
const PATHS: usize = 64;

/// Whether `program` (a path, or a name looked for in `path_dirs`) is built on Chromium.
pub fn is_chromium(program: &str, path_dirs: &[PathBuf]) -> bool {
    resolve(program, path_dirs).is_some_and(|path| built_on_chromium(&path, path_dirs, DEPTH))
}

/// `args` (a Chromium program's arguments) with the overlay scroll bar feature on: added to a list of enabled features
/// already there, else as the first argument, so a later switch of the person's own still wins. A list that turns the
/// feature off, or already turns it on, is left as it is.
pub fn with_overlay_scroll_bars(args: &mut Vec<String>) {
    let named = |list: &str| {
        list.split(',')
            .any(|f| f == OVERLAY_SCROLL_BARS || f.starts_with("OverlayScrollbar:"))
    };
    if args
        .iter()
        .any(|a| a.strip_prefix(DISABLE).is_some_and(named))
    {
        return;
    }
    match args.iter_mut().find(|a| a.starts_with(ENABLE)) {
        Some(list) if named(&list[ENABLE.len()..]) => {}
        Some(list) => {
            if list.len() > ENABLE.len() {
                list.push(',');
            }
            list.push_str(OVERLAY_SCROLL_BARS);
        }
        None => args.insert(0, format!("{ENABLE}{OVERLAY_SCROLL_BARS}")),
    }
}

fn resolve(program: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    path_dirs
        .iter()
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn has_marker(dir: Option<&Path>) -> bool {
    dir.is_some_and(|d| d.join(MARKER).is_file())
}

fn built_on_chromium(path: &Path, path_dirs: &[PathBuf], depth: usize) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let dir = real.parent();
    if has_marker(dir) || has_marker(dir.and_then(Path::parent)) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let mut head = Vec::new();
    let Ok(file) = std::fs::File::open(&real) else {
        return false;
    };
    if file.take(READ).read_to_end(&mut head).is_err() {
        return false;
    }
    named_programs(&head)
        .into_iter()
        .filter_map(|name| resolve(&name, path_dirs))
        .filter(|p| *p != real && is_executable(p))
        .any(|p| built_on_chromium(&p, path_dirs, depth - 1))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The programs `launcher` may start: the absolute paths it names, and in a script the program named after `exec`. A
/// script's interpreter is not one of them.
fn named_programs(launcher: &[u8]) -> Vec<String> {
    let script = launcher.starts_with(b"#!");
    let launcher = match launcher.iter().position(|b| *b == b'\n') {
        Some(end) if script => &launcher[end..],
        _ => launcher,
    };
    let path_byte = |b: &u8| b.is_ascii_alphanumeric() || b"/._+-".contains(b);
    let words: Vec<&[u8]> = launcher
        .split(|b| !path_byte(b))
        .filter(|w| !w.is_empty())
        .collect();
    let mut named: Vec<String> = words
        .iter()
        .filter(|w| w.len() > 1 && w[0] == b'/')
        .take(PATHS)
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    if script {
        named.extend(
            words
                .windows(2)
                .filter(|pair| pair[0] == b"exec" && pair[1][0] != b'/' && pair[1][0] != b'-')
                .map(|pair| String::from_utf8_lossy(pair[1]).into_owned()),
        );
    }
    named
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn with(list: &[&str]) -> Vec<String> {
        let mut a = args(list);
        with_overlay_scroll_bars(&mut a);
        a
    }

    #[test]
    fn the_feature_goes_first_or_joins_the_features_already_enabled() {
        assert_eq!(with(&["%U"]), ["--enable-features=OverlayScrollbar", "%U"]);
        assert_eq!(
            with(&["--enable-features=WaylandWindowDecorations", "x"]),
            [
                "--enable-features=WaylandWindowDecorations,OverlayScrollbar",
                "x"
            ]
        );
        assert_eq!(
            with(&["--enable-features="]),
            ["--enable-features=OverlayScrollbar"]
        );
    }

    #[test]
    fn a_launch_that_already_names_the_feature_is_left_as_it_is() {
        for list in [
            &["--enable-features=OverlayScrollbar:mode/classic"][..],
            &["--disable-features=A,OverlayScrollbar"][..],
        ] {
            assert_eq!(with(list), args(list));
        }
    }

    fn executable(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_program_beside_chromium_s_resources_or_started_by_a_launcher_is_built_on_chromium() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let bin = r.join("bin");
        // a browser in its own folder, reached through a link on the search path
        executable(&r.join("lib/browser/browser"), "\x7fELF");
        std::fs::write(r.join("lib/browser").join(MARKER), "").unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(r.join("lib/browser/browser"), bin.join("browser")).unwrap();
        // an application whose script on the search path runs it by its full path
        executable(&r.join("lib/app/app"), "\x7fELF");
        std::fs::write(r.join("lib/app").join(MARKER), "").unwrap();
        let app = r.join("lib/app/app");
        executable(
            &bin.join("app"),
            &format!("#!/bin/sh\nexec {} \"$@\"\n", app.display()),
        );
        // a script that runs a runtime by name, whose own script runs it by its full path
        executable(&r.join("lib/runtime/runtime"), "\x7fELF");
        std::fs::write(r.join("lib/runtime").join(MARKER), "").unwrap();
        executable(
            &bin.join("runtime"),
            &format!(
                "#!/bin/sh\nexec {} \"$@\"\n",
                r.join("lib/runtime/runtime").display()
            ),
        );
        executable(
            &bin.join("editor"),
            "#!/bin/sh\nexec runtime /usr/lib/editor/main.js \"$@\"\n",
        );
        // an application whose executable sits one folder below its resources
        executable(
            &r.join("opt/code/bin/code"),
            "#!/bin/sh\nexec \"$DIR/code\" \"$@\"\n",
        );
        std::fs::write(r.join("opt/code").join(MARKER), "").unwrap();
        // programs that are not
        executable(&bin.join("terminal"), "\x7fELF /usr/lib/terminal/data");
        executable(&bin.join("wrapper"), "#!/bin/sh\nexec terminal \"$@\"\n");

        let dirs = [bin.clone()];
        for program in ["browser", "app", "editor"] {
            assert!(is_chromium(program, &dirs), "{program}");
        }
        assert!(is_chromium(
            &r.join("opt/code/bin/code").display().to_string(),
            &dirs
        ));
        for program in ["terminal", "wrapper", "missing"] {
            assert!(!is_chromium(program, &dirs), "{program}");
        }
    }
}
