//! The `cupertino-files` process: browser windows on CupertinoKit, the first on a folder (the home folder when none is
//! given). New windows open in the same process; it exits once the last window is closed and no file operation runs.
//! One process serves the session: a launch while Files runs hands its folder (or its wish for a window) to it over the
//! session bus and exits. See the README.

use cupertino_diagnostic_events as diag;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_files::instance::{self, Launch, LaunchError, Request, Service, Target};
use cupertino_files::kit::{self, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{session_places, SessionPlaces};
use cupertino_files::recents;
use cupertino_files::thumbnail::Thumbnailer;
use cupertino_files::trash::Trash;
use cupertino_files::window_prefs;
use cupertino_kit::file_icons::FileIcons;

const USAGE: &str =
    "usage: cupertino-files [--terminal PROGRAM] [FOLDER | file:///FOLDER | trash:///]";

/// What the command line asks for.
struct Options {
    /// The folder named, made absolute; `None` for the home folder (or the running Files' window).
    target: Option<Target>,
    /// The argv prefix that runs applications with `Terminal=true` (`PROGRAM -e`), as the launcher's.
    terminal: Option<Vec<String>>,
}

/// `[--terminal PROGRAM] [FOLDER]`.
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
    let target = folder
        .map(|arg| {
            Target::of_argument(&arg)?
                .absolute()
                .map_err(|e| format!("{arg}: {e}"))
        })
        .transpose()?;
    Ok(Options { target, terminal })
}

fn main() -> ExitCode {
    diag::init(diag::component!(), "files", diag::Sink::UserStore);
    let options = match options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            diag::exit_error(&problem);
            eprintln!("cupertino-files: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // before any thread starts, so no program Files starts inherits the launch's spent token
    let token = instance::take_activation_token();
    let request = match &options.target {
        Some(target) => Request::Open(vec![target.clone()]),
        None => Request::Activate,
    };
    let service = match instance::serve_or_forward(
        zbus::connection::Builder::session,
        &request,
        token.as_deref(),
        instance::FORWARD_TIMEOUT,
    ) {
        Ok(Launch::Serve(service)) => Some(service),
        Ok(Launch::Forwarded) => return ExitCode::SUCCESS,
        Err(LaunchError::NoBus(e)) => {
            eprintln!(
                "cupertino-files: a later launch cannot reach this one: cannot claim {}: {e}",
                instance::BUS_NAME
            );
            None
        }
        Err(e @ LaunchError::Refused(_)) => {
            diag::exit_error(&e);
            eprintln!("cupertino-files: {e}");
            return ExitCode::FAILURE;
        }
    };
    let start = match start(options, service, token) {
        Ok(start) => start,
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-files: {e}");
            return ExitCode::FAILURE;
        }
    };
    match kit::run(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-files: {e}");
            ExitCode::FAILURE
        }
    }
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// The first window of the session: its folder listed, the session's places and file types, the Recents of the data
/// folder, notices on the session bus, removable media on the system bus, the icons of the asset packs and the icon
/// theme, and pictures' thumbnails in the session's thumbnail cache.
fn start(options: Options, service: Option<Service>, token: Option<String>) -> io::Result<Start> {
    let dirs = XdgDirs::from_env().map_err(other)?;
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .ok_or_else(|| io::Error::other("HOME is not set; name the folder to open"))?,
    );
    let folder = match &options.target {
        Some(target) => target.folder(&Trash::new())?,
        None => home.clone(),
    };
    let SessionPlaces {
        places,
        folder_icons,
    } = session_places(&home, &dirs.data_dirs)?;
    // the places follow the user directories, their configuration and the trash
    let watched = [Some(home.clone()), cupertino_defaults::config_home()]
        .into_iter()
        .flatten()
        .collect();
    let (source_home, data_dirs) = (home.clone(), dirs.data_dirs.clone());
    let source: kit::PlacesSource =
        Box::new(move || session_places(&source_home, &data_dirs).map(|p| p.places));
    // one database for the Kind column, the icons and what opens an item, so all name the same type
    let types = Arc::new(MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?);
    let icons = FileIcons::discover((*types).clone(), folder_icons.into_iter().collect());
    let notices = Notices::start(zbus::blocking::Connection::session().ok());
    let system = zbus::blocking::Connection::system()
        .map_err(|e| eprintln!("cupertino-files: no removable media: {e}"))
        .ok();
    Start::open(
        folder.clone(),
        places,
        Some(types),
        dirs.data_home.join(recents::XBEL_NAME),
        notices,
        Box::new(applications),
        options.terminal,
        system,
    )
    .map(|start| {
        let start = start
            .with_file_icons(icons)
            .with_thumbnails(Thumbnailer::from_env(kit::APP_ID))
            .with_places_source(source, watched)
            .with_instance(service)
            .with_activation_token(token);
        match window_prefs::location() {
            Some(file) => start.with_prefs_file(file),
            None => start,
        }
    })
    .map_err(|e| io::Error::new(e.kind(), format!("cannot open {}: {e}", folder.display())))
}

/// The default-application resolver of the session's directories and desktops, read again at each open.
fn applications() -> io::Result<MimeApps> {
    let dirs = XdgDirs::from_env().map_err(other)?;
    let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?;
    MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env()).map_err(other)
}
