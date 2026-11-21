//! The portal's dialogs drawn to PNGs for look checks:
//! `cargo run -p cupertino-shell-dialogs --example dialogs_capture -- dialogs/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! Surfaces: `open` (a file chooser opening a text file, one selected, under its filters), `save` (saving
//! `Notes.txt` with an encoding choice; `--state replace` asks whether to replace the file of that name) and `apps`
//! (an app chooser for `Notes.txt`, the last choice selected). The folders are example data under the temporary
//! directory, and the capture runs with a home of its own under `target/`, never the person's; the fonts, symbols,
//! icons and theme are the system's, as an application's.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use cupertino_kit::controls::Selection;
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::panel::PanelEvent;
use cupertino_kit::files::places::{chooser_places, Place};
use cupertino_kit::files::ViewEvent;
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_shell_dialogs::apps::AppDialog;
use cupertino_shell_dialogs::ask::{AppAsk, AppChoice, Choice, FileAsk, Filter, Kind, Pattern};
use cupertino_shell_dialogs::file::{FileDialog, FileEvent};
use cupertino_shell_dialogs::{item_icons, view, Message};

/// The runtime turns a frame may take, and the frames a window may ask for before it rests.
const TURNS: u32 = 10_000;
const FRAMES: usize = 16;
/// The example home folder, under the temporary directory.
const ROOT: &str = "cupertino-dialogs-capture";
/// The user folders of the example home, and the files of its Documents folder.
const USER_DIRS: [(&str, &str); 5] = [
    ("DESKTOP", "Desktop"),
    ("DOCUMENTS", "Documents"),
    ("DOWNLOAD", "Downloads"),
    ("MUSIC", "Music"),
    ("PICTURES", "Pictures"),
];
const FOLDERS: [&str; 2] = ["Projects", "drafts"];
const FILES: [&str; 7] = [
    "Budget 2026.txt",
    "Notes.txt",
    "notes 9.txt",
    "notes 10.txt",
    "Report.pdf",
    "Photo.png",
    "Archive.zip",
];
/// The file the open chooser shows selected, and the name a save suggests.
const NOTES: &str = "Notes.txt";

/// What the next capture's window shows.
enum Shown {
    File {
        dialog: Box<FileDialog>,
        places: Vec<Place>,
        icons: Rc<ItemIcons>,
    },
    App(Box<AppDialog>),
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// The dialog's window showing [`SHOWN`].
struct Capture;

impl Application for Capture {
    type Model = Option<Shown>;
    type Message = Message;

    fn init() -> (Option<Shown>, Vec<Command<Message>>) {
        (SHOWN.with(|s| s.borrow_mut().take()), Vec::new())
    }

    /// The window's own events (its list's scroll, above all, which builds its rows) change the dialog as the
    /// component's do; a dialog that ends stays drawn.
    fn update(shown: &mut Option<Shown>, message: Message) -> Vec<Command<Message>> {
        match (shown, message) {
            (Some(Shown::File { dialog, places, .. }), Message::File(_, event)) => {
                dialog.update(event, places);
            }
            (Some(Shown::App(dialog)), Message::App(_, event)) => {
                dialog.update(event);
            }
            _ => {}
        }
        Vec::new()
    }

    fn view(shown: &Option<Shown>) -> Vec<Window<Message>> {
        match shown {
            Some(Shown::File {
                dialog,
                places,
                icons,
            }) => vec![view::file_window(dialog, places, Some(icons))],
            Some(Shown::App(dialog)) => vec![view::app_window(dialog)],
            None => Vec::new(),
        }
    }
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// The example home under the temporary directory: the user folders, and folders and files in Documents.
// the example writes its own data, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn home() -> Result<PathBuf, String> {
    let home = std::env::temp_dir().join(ROOT).join("avery");
    for (_, name) in USER_DIRS {
        std::fs::create_dir_all(home.join(name)).map_err(io)?;
    }
    let documents = home.join("Documents");
    for folder in FOLDERS {
        std::fs::create_dir_all(documents.join(folder)).map_err(io)?;
    }
    for file in FILES {
        std::fs::write(documents.join(file), b"example").map_err(io)?;
    }
    Ok(home)
}

fn filters() -> Vec<Filter> {
    vec![
        Filter {
            name: "Text".into(),
            patterns: vec![Pattern::Glob("*.txt".into())],
        },
        Filter {
            name: "All Files".into(),
            patterns: vec![Pattern::Glob("*".into())],
        },
    ]
}

fn file_ask(kind: Kind, home: &Path) -> FileAsk {
    let save = kind == Kind::Save;
    FileAsk {
        kind,
        app_id: "org.example.Editor".into(),
        parent_window: String::new(),
        title: if save {
            "Save Document"
        } else {
            "Open Document"
        }
        .into(),
        accept_label: None,
        modal: true,
        multiple: false,
        directory: false,
        filters: filters(),
        current_filter: None,
        choices: save
            .then(|| Choice {
                id: "encoding".into(),
                label: "Encoding".into(),
                options: vec![
                    ("utf8".into(), "UTF-8".into()),
                    ("latin1".into(), "Latin-1".into()),
                ],
                initial: "utf8".into(),
            })
            .into_iter()
            .collect(),
        current_name: save.then(|| NOTES.into()),
        current_folder: Some(home.join("Documents")),
        current_file: None,
        files: Vec::new(),
    }
}

/// A file chooser of `kind` with its folder read, in `state`.
// the example reads its own folders and the system's icons, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn file_dialog(kind: Kind, replace: bool) -> Result<Shown, String> {
    let home = home()?;
    let dirs: Vec<(String, PathBuf)> = USER_DIRS
        .iter()
        .map(|(key, name)| ((*key).to_owned(), home.join(name)))
        .collect();
    let places = chooser_places(&home, &dirs);
    let (reply, _) = async_channel::bounded(1);
    let mut dialog = FileDialog::new(0, "/capture".into(), file_ask(kind, &home), reply, &home);
    if let Some(request) = dialog.take_request() {
        dialog.apply(request.run());
    }
    let row = dialog.items().iter().position(|e| e.name == NOTES);
    if let (Kind::Open, Some(row)) = (kind, row) {
        let mut rows = Selection::default();
        rows.select_only(row);
        dialog.update(FileEvent::List(ViewEvent::Select(rows)), &places);
    }
    if replace {
        dialog.update(FileEvent::Panel(PanelEvent::Accept), &places);
    }
    let types = cupertino_apps::XdgDirs::from_env()
        .ok()
        .and_then(|dirs| cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).ok())
        .unwrap_or_default();
    let folders = places
        .iter()
        .filter_map(|p| Some((p.path.clone(), p.kind.folder_icon()?)))
        .collect();
    let icons = Rc::new(item_icons(FileIcons::discover(types, folders)));
    Ok(Shown::File {
        dialog: Box::new(dialog),
        places,
        icons,
    })
}

fn app_dialog() -> Shown {
    let (reply, _) = async_channel::bounded(1);
    let choice = |id: &str, name: &str, icon: Option<&str>| AppChoice {
        id: id.into(),
        name: name.into(),
        icon: icon.map(Into::into),
    };
    let ask = AppAsk {
        app_id: "org.example.Mail".into(),
        parent_window: String::new(),
        target: Some(NOTES.into()),
        choices: vec![
            choice(
                "org.example.TextEdit",
                "TextEdit",
                Some("accessories-text-editor"),
            ),
            choice("org.example.Writer", "Writer", Some("org.example.Writer")),
            choice("org.example.Viewer", "Viewer", None),
        ],
        last_choice: Some("org.example.Writer".into()),
        wants_token: false,
    };
    Shown::App(Box::new(AppDialog::new(0, "/capture".into(), ask, reply)))
}

struct Dialogs;

impl App for Dialogs {
    const NAME: &'static str = "dialogs";
    const STATE_FLAGS: &'static [&'static str] = &["state"];

    fn surfaces(&self) -> Vec<String> {
        ["open", "save", "apps"].map(str::to_owned).to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            "save" => vec![Vec::new(), vec![("state".into(), "replace".into())]],
            _ => vec![Vec::new()],
        }
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let replace = match request.state.iter().find(|(f, _)| f == "state") {
            None => false,
            Some((_, value)) if value == "rest" => false,
            Some((_, value)) if value == "replace" && request.surface == "save" => true,
            Some((_, value)) => return Err(format!("--state {value}: rest, or replace for save")),
        };
        let shown = match request.surface {
            "open" => file_dialog(Kind::Open, false)?,
            "save" => file_dialog(Kind::Save, replace)?,
            "apps" => app_dialog(),
            other => return Err(format!("no surface {other}")),
        };
        SHOWN.with(|s| *s.borrow_mut() = Some(shown));
        draw(request.look, request.scale)
    }
}

// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(look: Look, scale: f32) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: true,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    let tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    options.theme = Arc::new(tokens);
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Capture>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver
        .runtime_mut()
        .set_theme_for(Box::new(move |appearance: &SystemAppearance| {
            let preferences = cupertino_kit::test::preferences(appearance);
            Arc::new(ThemeTokens::cupertino(&theme, preferences).expect("the theme loaded once"))
        }));
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
    let window: WindowId = driver.runtime().windows()[0].1;
    let system = driver.window_system_mut();
    system
        .enqueue_window(
            window,
            cupertino_kit::test::WindowEvent::Scale(f64::from(scale)),
        )
        .map_err(|e| fail(&e))?;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system
        .configure(window, None, active)
        .map_err(|e| fail(&e))?;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            break;
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    presented(driver.runtime().window_system(), window)
}

/// The home a capture runs with, never the person's own: their settings and asset packs would change the pixels.
fn private_home() {
    let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/capture-home");
    std::env::set_var("HOME", &home);
    for (name, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_STATE_HOME", ".local/state"),
    ] {
        std::env::set_var(name, home.join(dir));
    }
}

fn main() -> ExitCode {
    private_home();
    cupertino_kit::test::capture::run(|| {
        cupertino_kit::test::capture::assets()?;
        Ok(Dialogs)
    })
}
