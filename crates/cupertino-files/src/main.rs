//! The `cupertino-files` process: one browser window on a folder (the home folder when none is
//! given) as a client of the server. It exits when the window is closed. See the README.

mod app;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cupertino_files::trash::Trash;
use percent_encoding::percent_decode_str;

const USAGE: &str =
    "usage: cupertino-files [--terminal PROGRAM] [FOLDER | file:///FOLDER | trash:///]";

/// What the command line asks for.
struct Options {
    folder: PathBuf,
    /// The argv prefix that runs applications with `Terminal=true` (`PROGRAM -e`), as the launcher's.
    terminal: Option<Vec<String>>,
}

/// The folder an argument names: a path, a local `file://` URI (the desktop entry's `%U`) or the trash
/// (`trash:///`, the scheme the session's file manager handles), whose folder is `<home_trash>/files`.
fn folder_of(arg: &str, home_trash: &Path) -> Result<PathBuf, String> {
    if let Some(rest) = arg.strip_prefix("trash:") {
        return match rest.trim_start_matches('/') {
            "" => Ok(home_trash.join("files")),
            _ => Err(format!(
                "{arg}: only the trash itself (trash:///) can be opened"
            )),
        };
    }
    if let Some(rest) = arg.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        // the query and fragment are not part of the path (RFC 8089, RFC 3986 §3)
        let path = path.split(['?', '#']).next().unwrap_or(path);
        if !path.starts_with('/') {
            return Err(format!("{arg}: not a local file URI"));
        }
        let bytes: Vec<u8> = percent_decode_str(path).collect();
        return Ok(PathBuf::from(OsString::from_vec(bytes)));
    }
    Ok(PathBuf::from(arg))
}

/// `[--terminal PROGRAM] [FOLDER]`; the folder defaults to `$HOME`. Opening the trash creates it when
/// nothing was trashed yet (the freedesktop Trash specification lets the implementation create it).
fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut folder = None;
    let mut terminal = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--terminal" => {
                terminal = Some(vec![
                    args.next().ok_or("--terminal needs a program")?,
                    "-e".to_string(),
                ])
            }
            a if a.starts_with('-') => return Err(format!("unexpected argument {a}")),
            _ if folder.is_some() => return Err(format!("unexpected argument {arg}")),
            _ => folder = Some(arg),
        }
    }
    let trash = Trash::new();
    let folder = folder
        .map(|arg| folder_of(&arg, trash.home_trash()))
        .transpose()?;
    if folder.as_ref() == Some(&trash.home_trash().join("files")) {
        for sub in ["files", "info"] {
            let dir = trash.home_trash().join(sub);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
    }
    let folder = match folder {
        Some(folder) => folder,
        None => PathBuf::from(
            std::env::var_os("HOME").ok_or("HOME is not set; name the folder to open")?,
        ),
    };
    let folder = std::path::absolute(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    Ok(Options { folder, terminal })
}

fn main() -> ExitCode {
    let options = match options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-files: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match app::run(options.folder, options.terminal) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cupertino-files: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::folder_of;
    use std::path::{Path, PathBuf};

    #[test]
    fn arguments_name_a_path_a_file_uri_or_the_trash() {
        let trash = Path::new("/home/u/.local/share/Trash");
        assert_eq!(
            folder_of("trash:///", trash),
            Ok(PathBuf::from("/home/u/.local/share/Trash/files"))
        );
        assert_eq!(
            folder_of("trash:", trash),
            Ok(PathBuf::from("/home/u/.local/share/Trash/files"))
        );
        assert!(
            folder_of("trash:///old/report.txt", trash).is_err(),
            "only the trash folder itself"
        );
        assert_eq!(
            folder_of("file:///home/u/My%20Files", trash),
            Ok(PathBuf::from("/home/u/My Files"))
        );
        assert_eq!(
            folder_of("file://localhost/tmp", trash),
            Ok(PathBuf::from("/tmp"))
        );
        assert_eq!(
            folder_of("file:///tmp/a%3Fb?x=1#top", trash),
            Ok(PathBuf::from("/tmp/a?b"))
        );
        assert!(
            folder_of("file://server/share", trash).is_err(),
            "a remote host"
        );
        assert_eq!(
            folder_of("Documents", trash),
            Ok(PathBuf::from("Documents"))
        );
    }
}
