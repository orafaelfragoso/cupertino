//! The launcher's panel drawn to PNGs for look checks:
//! `cargo run -p cupertino-launcher --example launcher_capture -- launcher/panel [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! One surface, `panel`: the panel's window as it opens below the menu bar, its shadow's margin included. `--view`
//! names what it shows: `empty` (the default) the search field alone, `searching` a query the file search has not
//! answered yet, `results` a query's applications with the selected one previewed, `folders` the files scope
//! listing folders and documents, and `applications` the browser of every application. The applications, files and
//! clock are fixed sample data; the fonts, icons and theme are the system's, as an application's; nothing moves, so a
//! command gives the same pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::controls::{ImageSource, TextChange};
use cupertino_kit::core::{Application, Command, Icons, Window};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_launcher::browse::{App as BrowserApp, BrowseLayout};
use cupertino_launcher::engine::{ItemKind, Launcher, LauncherConfig, ResultItem};
use cupertino_launcher::exec::CommandSpec;
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::frecency::Frecency;
use cupertino_launcher::kit::{self, Host, Message, Model, SearchRequest};
use cupertino_launcher::panel::Now;
use cupertino_launcher::preview::{Preview, PreviewBody};
use cupertino_launcher::scope::{FileItem, ScopeAnswer, ScopeLayout, ScopeRequest};
use cupertino_launcher::view::{
    self, Input, DOCUMENT_ICON, FOLDER_ICON, GENERIC_APPLICATION_ICON, ICON_SIZE, PREVIEW_ICON_SIZE,
};
use cupertino_launcher::DesktopEntry;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 32;
/// The output the panel opens on, in points: the height the panel's placement is measured against.
const OUTPUT: Size = Size::new(1512.0, 982.0);
/// The wall clock the capture shows: 2026-11-14 10:00 UTC, in Unix seconds.
const UNIX_NOW: f64 = 1_794_650_400.0;
/// The home folder the sample files are in.
const HOME: &str = "/home/user";
/// The views a capture may show, by their flag values.
const VIEWS: [&str; 5] = ["empty", "searching", "results", "folders", "applications"];
/// What the `results` view types: it finds several applications.
const RESULTS_QUERY: &str = "te";
/// What the `searching` view types: no application has it, and the file search is still under way.
const SEARCHING_QUERY: &str = "quarterly";
/// The scales the icons are loaded at.
const SCALES: [u32; 3] = [1, 2, 3];

thread_local! {
    /// What the next capture's panel shows.
    static SHOWN: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

/// The sample applications: name, desktop file ID, icon and categories.
const APPLICATIONS: [(&str, &str, &str, &str); 12] = [
    (
        "Terminal",
        "org.example.Terminal",
        "utilities-terminal",
        "System;TerminalEmulator;",
    ),
    (
        "Text Editor",
        "org.example.TextEditor",
        "accessories-text-editor",
        "Utility;TextEditor;",
    ),
    (
        "Settings",
        "org.example.Settings",
        "preferences-system",
        "Settings;",
    ),
    (
        "Files",
        "org.example.Files",
        "system-file-manager",
        "System;FileManager;",
    ),
    (
        "Calculator",
        "org.example.Calculator",
        "accessories-calculator",
        "Utility;Calculator;",
    ),
    (
        "Calendar",
        "org.example.Calendar",
        "office-calendar",
        "Office;Calendar;",
    ),
    ("Mail", "org.example.Mail", "mail-client", "Network;Email;"),
    (
        "Music",
        "org.example.Music",
        "multimedia-audio-player",
        "AudioVideo;Audio;",
    ),
    (
        "Photos",
        "org.example.Photos",
        "image-viewer",
        "Graphics;Viewer;",
    ),
    ("Maps", "org.example.Maps", "maps", "Utility;Maps;"),
    ("Notes", "org.example.Notes", "notes", "Office;"),
    ("Weather", "org.example.Weather", "weather", "Utility;"),
];

/// The sample files of the files scope: name under [`HOME`], and whether it is a folder.
const FILES: [(&str, bool); 8] = [
    ("Documents", true),
    ("Downloads", true),
    ("Pictures", true),
    ("Projects", true),
    ("Music", true),
    ("Budget.ods", false),
    ("Notes.txt", false),
    ("Report.pdf", false),
];

/// The sample applications as desktop entries.
fn entries() -> Vec<DesktopEntry> {
    APPLICATIONS
        .iter()
        .map(|(name, id, icon, categories)| DesktopEntry {
            id: format!("{id}.desktop"),
            name: (*name).to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            keywords: Vec::new(),
            categories: categories
                .split(';')
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect(),
            icon: Some((*icon).to_owned()),
            exec: name.to_lowercase().replace(' ', "-"),
            try_exec: None,
            terminal: false,
            path: None,
            startup_wm_class: None,
            actions: Vec::new(),
            source: None,
        })
        .collect()
}

/// The sample files as the files scope lists them.
fn files() -> Vec<FileItem> {
    FILES
        .iter()
        .map(|(name, is_dir)| FileItem {
            path: PathBuf::from(HOME).join(name),
            name: (*name).to_owned(),
            is_dir: *is_dir,
        })
        .collect()
}

/// The icon `name` of the system's icons at `size` points, one rendition per scale.
fn named(icons: &dyn Icons, name: &str, size: f64) -> Vec<ImageSource> {
    SCALES
        .iter()
        .filter_map(|scale| icons.named(name, size as u32 * scale))
        .map(|icon| ImageSource {
            id: icon.id,
            image: icon.image,
        })
        .collect()
}

/// What the panel asks of the system, answered with the sample data at once; the file search is never answered
/// while `searching`.
struct Fixed {
    icons: Arc<dyn Icons>,
    searching: bool,
}

impl Host for Fixed {
    fn run(&mut self, _: CommandSpec, _: Option<String>) {}

    fn open(&mut self, _: &str) {}

    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>> {
        let index = Arc::new(FileIndex::empty(config));
        vec![Command::Perform(Box::pin(async move {
            Message::Indexed(index)
        }))]
    }

    fn resize(&mut self, _: Size) {}

    fn icon(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        let name = match item.kind {
            ItemKind::Application | ItemKind::ApplicationAction => item
                .icon
                .clone()
                .unwrap_or_else(|| GENERIC_APPLICATION_ICON.to_owned()),
            ItemKind::Folder => FOLDER_ICON.to_owned(),
            ItemKind::File => DOCUMENT_ICON.to_owned(),
            ItemKind::WebSearch | ItemKind::Calculator | ItemKind::Conversion => return Vec::new(),
        };
        let (id, icon) = (item.id.clone(), named(&*self.icons, &name, ICON_SIZE));
        vec![Command::Perform(Box::pin(async move {
            Message::ResultIcon(id, icon)
        }))]
    }

    fn app_icon(&mut self, app: &BrowserApp) -> Vec<Command<Message>> {
        let name = app.icon.as_deref().unwrap_or(GENERIC_APPLICATION_ICON);
        let mut icon = named(&*self.icons, name, BrowseLayout::LIST_ICON);
        icon.extend(named(&*self.icons, name, BrowseLayout::ICON));
        let id = app.id.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::AppIcon(id, icon)
        }))]
    }

    fn preview(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        let Some(name) = item.icon.as_deref() else {
            return Vec::new();
        };
        let preview = Preview {
            body: PreviewBody::Icon,
            icon: named(&*self.icons, name, PREVIEW_ICON_SIZE),
            kind: String::new(),
            details: Vec::new(),
        };
        let id = item.id.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::Previewed(id, Arc::new(preview))
        }))]
    }

    fn search(&mut self, request: SearchRequest) -> Vec<Command<Message>> {
        if self.searching {
            return Vec::new();
        }
        let update = request.run();
        vec![Command::Perform(Box::pin(async move {
            Message::Provided(update)
        }))]
    }

    fn scope_search(&mut self, request: ScopeRequest) -> Vec<Command<Message>> {
        let answer = ScopeAnswer {
            generation: request.generation,
            suggestions: Vec::new(),
            items: files(),
        };
        vec![Command::Perform(Box::pin(async move {
            Message::Scoped(answer)
        }))]
    }

    fn file_icon(&mut self, item: &FileItem) -> Vec<Command<Message>> {
        let name = if item.is_dir {
            FOLDER_ICON
        } else {
            DOCUMENT_ICON
        };
        let (path, icon) = (
            item.path.clone(),
            named(&*self.icons, name, ScopeLayout::THUMB),
        );
        vec![Command::Perform(Box::pin(async move {
            Message::FileIcon(path, icon)
        }))]
    }

    fn now(&self) -> Now {
        Now {
            mono: 0.0,
            unix: UNIX_NOW,
        }
    }
}

/// The panel showing [`SHOWN`].
struct Shown;

impl Application for Shown {
    type Model = Model;
    type Message = Message;

    // the capture draws with the system's icons, outside any session
    #[allow(clippy::disallowed_methods)]
    fn init() -> (Model, Vec<Command<Message>>) {
        let shown = SHOWN
            .with_borrow_mut(Option::take)
            .expect("a view for the capture");
        let icons = cupertino_kit::app::environment()
            .expect("the capture's environment")
            .icons;
        let host = Fixed {
            icons,
            searching: shown == "searching",
        };
        let launcher = Launcher::new(LauncherConfig::default(), entries(), Frecency::in_memory());
        let mut model = Model::new(launcher, Box::new(host));
        model.settled = true;
        model.files = Some(FileSearchConfig::new(HOME));
        let mut commands = kit::update(&mut model, Message::Output(OUTPUT));
        let opening = match shown {
            "folders" => Message::Files,
            "applications" => Message::Browse,
            _ => Message::Toggle,
        };
        commands.extend(kit::update(&mut model, opening));
        let query = match shown {
            "results" => Some(RESULTS_QUERY),
            "searching" => Some(SEARCHING_QUERY),
            _ => None,
        };
        if let Some(query) = query {
            let change = TextChange {
                text: query.to_owned(),
                revision: model.revision + 1,
            };
            commands.extend(kit::update(
                &mut model,
                Message::Input(Input::Query(change)),
            ));
        }
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        kit::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        kit::view(model)
    }
}

/// The value of state flag `flag`, or the first of `values`; an error unless it is one of them.
fn flag<'a>(request: &'a Request<'_>, flag: &str, values: &[&'a str]) -> Result<&'a str, String> {
    match request.state.iter().find(|(f, _)| f == flag) {
        None => Ok(values[0]),
        Some((_, value)) if values.contains(&value.as_str()) => Ok(value),
        Some((_, value)) => Err(format!("--{flag} {value}: one of {}", values.join(", "))),
    }
}

struct Panel;

impl App for Panel {
    const NAME: &'static str = "launcher";
    const STATE_FLAGS: &'static [&'static str] = &["view"];

    fn surfaces(&self) -> Vec<String> {
        vec!["panel".to_owned()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        VIEWS
            .iter()
            .map(|name| vec![("view".to_owned(), (*name).to_owned())])
            .collect()
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let shown = VIEWS
            .iter()
            .find(|v| **v == flag(request, "view", &VIEWS).unwrap_or_default())
            .copied();
        let shown = shown.ok_or_else(|| flag(request, "view", &VIEWS).unwrap_err())?;
        SHOWN.set(Some(shown));
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
    let mut driver = TestDriver::<Shown>::new(
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
    let windows: Vec<WindowId> = driver
        .runtime()
        .windows()
        .iter()
        .map(|(_, id)| *id)
        .collect();
    let panel: WindowId = driver
        .runtime()
        .windows()
        .iter()
        .find(|(key, _)| *key == view::panel_window_key())
        .ok_or("the launcher shows no panel")?
        .1;
    for &window in &windows {
        let system = driver.window_system_mut();
        system
            .enqueue_window(
                window,
                cupertino_kit::test::WindowEvent::Scale(f64::from(scale)),
            )
            .map_err(|e| fail(&e))?;
        system
            .configure(window, None, WindowStates::default())
            .map_err(|e| fail(&e))?;
    }
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(panel).map_err(|e| fail(&e))? {
            break;
        }
        system
            .frame_ready(panel, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    presented(driver.runtime().window_system(), panel)
}

fn main() -> ExitCode {
    cupertino_kit::test::capture::run(|| {
        cupertino_kit::test::capture::assets()?;
        Ok(Panel)
    })
}
