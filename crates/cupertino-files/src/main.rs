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
use cupertino_files::instance::{self, Request, Target};
use cupertino_files::kit::{self, Buses, Ended, Start};
use cupertino_files::trash::Trash;
use cupertino_files::window_prefs;
use cupertino_icons::thumbnail::Thumbnailer;
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::files::places::{session_places, SessionPlaces};
use cupertino_kit::files::recents;

const USAGE: &str =
    "usage: cupertino-files [--terminal PROGRAM] [FOLDER | file:///FOLDER | trash:///] | \
     --gapplication-service";

/// What the command line asks for.
struct Options {
    /// The folder named, made absolute; `None` for the home folder (or the running Files' window).
    target: Option<Target>,
    /// The argv prefix that runs applications with `Terminal=true` (`PROGRAM -e`), as the launcher's.
    terminal: Option<Vec<String>>,
    /// Started by the session bus for a call: serve the names and open only the windows the calls ask for.
    service: bool,
}

/// `[--terminal PROGRAM] [FOLDER]`.
fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut folder = None;
    let mut terminal = None;
    let mut service = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--terminal" => {
                terminal = Some(vec![
                    args.next().ok_or("--terminal needs a program")?,
                    "-e".to_string(),
                ])
            }
            "--gapplication-service" => service = true,
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
    if service && target.is_some() {
        return Err("--gapplication-service takes no folder".into());
    }
    Ok(Options {
        target,
        terminal,
        service,
    })
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
    let buses = match (&options.service, &options.target) {
        // the bus started Files for a call to one of its names: the running Files already takes those calls
        (true, _) => Buses::Service,
        (false, Some(target)) => Buses::Launch(Request::Open(vec![target.clone()])),
        (false, None) => Buses::Launch(Request::Activate),
    };
    run(start(options, token).map(|start| start.with_buses(buses)))
}

/// Run Files from `start`, or say why it cannot start.
fn run(start: io::Result<Start>) -> ExitCode {
    let start = match start {
        Ok(start) => start,
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-files: {e}");
            return ExitCode::FAILURE;
        }
    };
    match kit::run(start) {
        Ok(Ended::Closed) => ExitCode::SUCCESS,
        Ok(Ended::Refused(why)) => {
            diag::exit_error(&why);
            eprintln!("cupertino-files: {why}");
            ExitCode::FAILURE
        }
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
/// folder, the icons of the asset packs and the icon theme, and pictures' thumbnails in the session's thumbnail cache.
fn start(options: Options, token: Option<String>) -> io::Result<Start> {
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
        Arc::new(move || session_places(&source_home, &data_dirs).map(|p| p.places));
    // one database for the Kind column, the icons and what opens an item, so all name the same type
    let types = Arc::new(MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?);
    let icons = FileIcons::discover((*types).clone(), folder_icons.into_iter().collect());
    let assets = cupertino_assets::Assets::discover();
    let recents = dirs.data_home.join(recents::XBEL_NAME);
    let start = match options.service {
        // started for a call, which says what to open
        true => Start::idle(
            places,
            Some(types),
            recents,
            Box::new(applications),
            options.terminal,
        ),
        false => Start::open(
            folder.clone(),
            places,
            Some(types),
            recents,
            Box::new(applications),
            options.terminal,
        ),
    };
    let start = start
        .with_file_icons(icons)
        .with_app_icons(
            cupertino_icons::ThemeIcons::new(&assets),
            cupertino_icons::SystemIcons::new(&assets),
        )
        .with_thumbnails(Thumbnailer::from_env(kit::APP_ID))
        .with_places_source(source, watched)
        .with_activation_token(token);
    Ok(match window_prefs::location() {
        Some(file) => start.with_prefs_file(file),
        None => start,
    })
}

/// The default-application resolver of the session's directories and desktops, read again at each open.
fn applications() -> io::Result<MimeApps> {
    let dirs = XdgDirs::from_env().map_err(other)?;
    let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?;
    MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env()).map_err(other)
}
