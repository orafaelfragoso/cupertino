//! What the dock's model needs the session for ([`Host`]), on the shell: the dock roles' requests, programs started
//! through the host, and the files and the bus as tasks off the UI thread (Kit's I/O pool, the session connection).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cupertino_apps::launch::OPEN_COMMAND;
use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::{Command, WindowKey};
use cupertino_kit::{shared, task};
use cupertino_kit_shell::{dock, host, ServerWindowId};

use super::{tasks, PROCESS};
use crate::desktop::DesktopIndex;
use crate::icons::{Icons, Job, WindowImage};
use crate::kit::{
    AppAction, BusCall, Emptying, FolderListing, Host, IconKey, IconLoad, Message, Request,
    Started, TrashLook, WindowIcon, ICON_SCALES,
};
use crate::session::launch_command;
use crate::view::IconSet;

/// The window id the slot of the next minimized window goes by: the server keeps one such slot per dock role, which
/// every request for a target sets, so it is no window's.
const NEXT_SLOT: ServerWindowId = ServerWindowId(0);
/// How long writing the pins should take: a few lines on a local disk.
const PINS_DEADLINE: Duration = Duration::from_secs(2);
/// How long loading a group of icons should take: a few theme lookups and decodes each.
const ICONS_DEADLINE: Duration = Duration::from_secs(5);
/// How long listing a folder for its stack should take: its entries and their file types.
const LISTING_DEADLINE: Duration = Duration::from_secs(5);
/// How long looking whether the home trash holds anything should take.
const TRASH_DEADLINE: Duration = Duration::from_secs(1);
/// How long emptying the trash should take: a big trash takes a while to delete, which is reported, not cut.
const EMPTYING_DEADLINE: Duration = Duration::from_secs(60);

/// What the host reads once the start's reading arrived, shared with the component, and the session connection.
pub struct Link {
    pub bus: Option<zbus::Connection>,
    pub terminal: Option<String>,
    pub folders: Vec<(String, String)>,
    pub home: Option<String>,
    pub trash: Option<Arc<cupertino_trash::Look>>,
    /// Where the pins are stored and `dock.conf` is (`None` when neither `XDG_CONFIG_HOME` nor `HOME` is set).
    pins: Option<PathBuf>,
    conf: Option<PathBuf>,
}

impl Link {
    pub fn new() -> Self {
        Self {
            bus: None,
            terminal: None,
            folders: Vec::new(),
            home: None,
            trash: None,
            pins: tasks::pins_path(),
            conf: DockSettings::path(),
        }
    }
}

/// The instance's icons, made on the pool by its first load and used by one load at a time there; the themes and the
/// asset packs they come from are the process's ([`shared`]).
type SharedIcons = Arc<Mutex<Option<Icons>>>;

pub struct ShellHost {
    link: Rc<RefCell<Link>>,
    start: Instant,
    /// The icons the windows set for themselves, by window id, with the keys of their applications.
    windows: HashMap<u32, ([String; 2], WindowImage)>,
    icons: SharedIcons,
}

impl ShellHost {
    pub fn new(link: Rc<RefCell<Link>>) -> Self {
        Self {
            link,
            start: Instant::now(),
            windows: HashMap::new(),
            icons: SharedIcons::default(),
        }
    }

    /// The icon of one of the windows of the application with key `app`, if one set its own.
    fn window_image(&self, app: &str) -> Option<WindowImage> {
        self.windows
            .values()
            .find(|(apps, _)| apps.iter().any(|a| a == app))
            .map(|(_, image)| image.clone())
    }

    /// Start `command` through the host, which reaps it.
    fn start(command: std::process::Command) -> bool {
        let program = command.get_program().to_string_lossy().into_owned();
        let started = host::spawn(command);
        if !started {
            eprintln!("{PROCESS}: cannot run {program}");
        }
        started
    }
}

fn kit_rect(r: &cupertino_wm::geometry::Rect) -> cupertino_kit::geometry::Rect {
    cupertino_kit::geometry::Rect::new(r.x, r.y, r.w, r.h)
}

/// The future of a task on the I/O pool: `task::io` makes a task of it.
fn pooled<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>> {
    match task::io(deadline, work, |value| value) {
        Command::Perform(future) => future,
        // `task::io` always makes a task
        _ => unreachable!("task::io makes a task"),
    }
}

impl Host for ShellHost {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn wake(&mut self, after: f64, generation: u64) -> Vec<Command<Message>> {
        let after = Duration::from_secs_f64(after.max(0.0));
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(after).await;
            Message::Wake(generation)
        }))]
    }

    fn request(&mut self, role: &WindowKey, request: Request) {
        let index = |i: usize| u32::try_from(i).ok();
        match request {
            Request::ActivateApp(app_id) => dock::activate_app(role, &app_id),
            Request::ActivateWindow { id, .. } => dock::activate_window(role, ServerWindowId(id)),
            // the window flies back out of its tile
            Request::RestoreWindow { id, tile } => {
                dock::set_minimize_target(role, ServerWindowId(id), kit_rect(&tile));
                dock::restore_window(role, ServerWindowId(id));
            }
            Request::MinimizeTarget(slot) => {
                dock::set_minimize_target(role, NEXT_SLOT, kit_rect(&slot))
            }
            Request::App(app_id, action) => dock::app_command(
                role,
                &app_id,
                match action {
                    AppAction::ShowWindows => dock::AppCommand::ShowWindows,
                    AppAction::Hide => dock::AppCommand::Hide,
                    AppAction::Quit => dock::AppCommand::Quit,
                },
            ),
            Request::ForceQuitApp(app_id) => dock::force_quit_app(role, &app_id),
            Request::RunAction(name) => dock::run_action(role, &name),
            Request::SwitcherSelect(i) => {
                if let Some(i) = index(i) {
                    dock::switcher_select(role, i);
                }
            }
            // the entry released on is selected, then committed
            Request::SwitcherActivate(i) => {
                if let Some(i) = index(i) {
                    dock::switcher_select(role, i);
                    dock::switcher_activate(role);
                }
            }
            Request::SwitcherCancel => dock::switcher_cancel(role),
        }
    }

    /// Started through the host, which reaps it and tells nothing of its exit: the guard against a second launch
    /// lasts its time.
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> Started {
        let Some(spec) = launch_command(argv, token.as_deref()) else {
            return Started::No;
        };
        if Self::start(spec.to_command()) {
            Started::Untracked
        } else {
            Started::No
        }
    }

    fn open(&mut self, target: &str) {
        let mut command = std::process::Command::new(OPEN_COMMAND);
        command.arg(target);
        Self::start(command);
    }

    fn shell(&mut self, command_line: &str) {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", command_line]);
        Self::start(command);
    }

    /// A failure goes to stderr: the dock keeps the order it shows, and the next change writes the file again.
    fn store_pins(&mut self, pins: &[String]) -> Vec<Command<Message>> {
        let Some(path) = self.link.borrow().pins.clone() else {
            return Vec::new();
        };
        let pins = pins.to_vec();
        vec![task::io(
            PINS_DEADLINE,
            move || {
                crate::conf::store_pins(&path, &pins)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))
            },
            |stored| {
                if let Err(e) = stored {
                    eprintln!("{PROCESS}: {e}");
                }
                Message::Done
            },
        )]
    }

    fn read_conf(&mut self, last_good: &DockSettings) -> Vec<Command<Message>> {
        let Some(path) = self.link.borrow().conf.clone() else {
            return Vec::new();
        };
        let last_good = last_good.clone();
        vec![task::io(
            tasks::CONF_DEADLINE,
            move || tasks::load_conf(&path, Some(&last_good)),
            |read| {
                if read.is_none() {
                    eprintln!("{PROCESS}: the dock keeps its settings");
                }
                Message::ConfRead(read.map(Box::new))
            },
        )]
    }

    fn terminal(&mut self) -> Option<String> {
        self.link.borrow().terminal.clone()
    }

    fn folders(&mut self) -> Vec<(String, String)> {
        self.link.borrow().folders.clone()
    }

    fn home(&mut self) -> Option<String> {
        self.link.borrow().home.clone()
    }

    /// Listed on the pool when the stack opens ([`crate::stack::list_folder`]).
    fn folder_entries(&mut self, folder: &str) -> FolderListing {
        let folder = PathBuf::from(folder);
        pooled(LISTING_DEADLINE, move || {
            let mime = shared::file_types();
            crate::stack::list_folder(&folder, |path, is_dir| {
                let ty = mime.file_type(path, is_dir);
                cupertino_icons::document::document_icon_name(mime, ty).to_owned()
            })
        })
    }

    /// The launcher's usage store, ranked now: the dock only reads it.
    fn recent_apps(&mut self) -> Vec<Command<Message>> {
        vec![task::io(
            tasks::CONF_DEADLINE,
            tasks::recent_apps,
            Message::RecentApps,
        )]
    }

    /// Asked on the component's session connection, given up after [`cupertino_apps::activate::ACTIVATE_TIMEOUT`].
    fn dbus_activate(&mut self, bus_name: &str, token: Option<String>) -> Option<BusCall> {
        let connection = self.link.borrow().bus.clone()?;
        let id = bus_name.to_owned();
        Some(Box::pin(async move {
            let call = std::pin::pin!(cupertino_apps::activate::activate(
                &connection,
                &id,
                token.as_deref()
            ));
            let timeout = std::pin::pin!(async_io::Timer::after(
                cupertino_apps::activate::ACTIVATE_TIMEOUT
            ));
            match futures_util::future::select(call, timeout).await {
                futures_util::future::Either::Left((Ok(()), _)) => true,
                futures_util::future::Either::Left((Err(e), _)) => {
                    eprintln!("{PROCESS}: {id} did not answer Activate: {e}");
                    false
                }
                futures_util::future::Either::Right(_) => {
                    eprintln!("{PROCESS}: {id} did not answer Activate in time");
                    false
                }
            }
        }))
    }

    /// Every icon loads on the pool: an application's from the theme name of its entry, else from the icon one of its
    /// windows set, else by its own name, else the generic application icon.
    fn preload(&mut self, desktop: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        let jobs: Vec<(IconKey, f64, Job)> = icons
            .into_iter()
            .map(|(key, side)| {
                let job = match &key {
                    IconKey::App(app) => Job::App {
                        app: app.clone(),
                        entry: desktop.entry_icon(app).map(str::to_owned),
                        window: self.window_image(app),
                    },
                    IconKey::Named(name) => Job::Named(name.clone()),
                    IconKey::Trash(full) => Job::Trash(*full),
                };
                (key, side, job)
            })
            .collect();
        let icons = Arc::clone(&self.icons);
        Some(pooled(ICONS_DEADLINE, move || {
            let mut slot = icons.lock().unwrap_or_else(PoisonError::into_inner);
            let icons = slot.get_or_insert_with(Icons::new);
            jobs.into_iter()
                .map(|(key, side, job)| {
                    let set: IconSet = ICON_SCALES
                        .iter()
                        .map(|scale| {
                            icons
                                .load(&job, (side * scale).ceil().max(1.0) as u32)
                                .image()
                        })
                        .collect();
                    (key, side.ceil().max(1.0) as u32, set)
                })
                .collect()
        }))
    }

    fn window_icon(&mut self, apps: [String; 2], icon: WindowIcon) {
        let image = WindowImage::new(
            icon.name,
            icon.image
                .as_ref()
                .map(|(side, argb)| (*side, argb.as_slice())),
        );
        match image {
            Some(image) => {
                self.windows.insert(icon.id, (apps, image));
            }
            None => {
                self.windows.remove(&icon.id);
            }
        }
    }

    fn window_closed(&mut self, id: u32) {
        self.windows.remove(&id);
    }

    fn trash_full(&mut self) -> Vec<Command<Message>> {
        let Some(look) = self.link.borrow().trash.clone() else {
            return Vec::new();
        };
        vec![task::io(
            TRASH_DEADLINE,
            move || look.full(),
            Message::TrashFull,
        )]
    }

    fn look_at_trash(&mut self) -> Option<TrashLook> {
        let look = self.link.borrow().trash.clone()?.next()?;
        Some(pooled(cupertino_trash::LOOK_DEADLINE, look))
    }

    fn empty_trash(&mut self) -> Emptying {
        let Some(look) = self.link.borrow().trash.clone() else {
            return Box::pin(async { None });
        };
        pooled(EMPTYING_DEADLINE, move || {
            let failed = look.trash().empty().failure_message();
            // what is left shows on the tile at the model's next look
            look.set_volumes_held(look.trash().volumes_hold_items());
            failed
        })
    }
}
