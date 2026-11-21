//! The dialogs as a component of the shell host (`cupertino-shell`): `org.cupertino.Dialogs1` on the component's own
//! session bus connection, the file choosers and app choosers it asks for as Kit windows, each a child of the app's
//! window the portal names, and the places, the item icons and every folder read as tasks.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::filepicker::Listing;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::places::{chooser_places, Place};
use cupertino_kit_shell::bus;
use cupertino_kit_shell::{Component, ComponentName, Event, Program, Role, Surface};

use crate::apps::{AppDialog, AppEvent};
use crate::file::{FileDialog, FileEvent};
use crate::service::{self, Request};
use crate::view;

/// How long reading the places and loading the icon themes should take.
const LOAD_DEADLINE: Duration = Duration::from_secs(5);
/// How long reading one folder should take; a folder on a hung mount is reported past it and keeps the dialog on the
/// folder it showed.
pub const LISTING_DEADLINE: Duration = Duration::from_secs(10);

/// The file and app choosers of the portal.
pub struct Dialogs;

/// The places and the item icons, read off the UI thread once.
pub struct Loaded {
    pub places: Vec<Place>,
    pub icons: FileIcons,
}

/// What the loading task made, taken once: the icons are no message's to copy.
#[derive(Clone)]
pub struct Taken(Arc<Mutex<Option<Loaded>>>);

/// What the component holds.
pub struct Model {
    session: Option<zbus::Connection>,
    lookup: Option<zbus::Connection>,
    serving: bool,
    /// Where `org.cupertino.Dialogs1` hands the calls it accepted, and where they come out.
    requests: (
        async_channel::Sender<Request>,
        async_channel::Receiver<Request>,
    ),
    home: PathBuf,
    places: Vec<Place>,
    icons: Option<Rc<ItemIcons>>,
    next: u64,
    files: Vec<FileDialog>,
    apps: Vec<AppDialog>,
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// The connection that asks who calls, or none.
    Lookup(Option<zbus::Connection>),
    LookupClosed,
    /// Whether `org.cupertino.Dialogs1` is served.
    Served(Result<(), String>),
    Request(Request),
    Loaded(Taken),
    File(u64, FileEvent),
    App(u64, AppEvent),
    /// The server closed a window.
    Closed(WindowKey),
}

/// The home folder: `HOME` when it is absolute, else the root.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The places of `home` and the icons of the items, both read from disk.
pub fn load(home: &std::path::Path) -> Loaded {
    let dirs = cupertino_defaults::user_dirs::read(home).unwrap_or_else(|e| {
        eprintln!("cupertino-shell-dialogs: cannot read the user directories: {e}");
        Vec::new()
    });
    let places = chooser_places(home, &dirs);
    let types = cupertino_apps::XdgDirs::from_env()
        .ok()
        .and_then(|dirs| cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).ok())
        .unwrap_or_default();
    let folders = places
        .iter()
        .filter_map(|p| Some((p.path.clone(), p.kind.folder_icon()?)))
        .collect();
    Loaded {
        icons: FileIcons::discover(types, folders),
        places,
    }
}

/// The item icons drawn from `icons`, without thumbnails.
pub fn item_icons(icons: FileIcons) -> ItemIcons {
    ItemIcons::new(icons, None, Arc::new(|| {}))
}

/// The next call `org.cupertino.Dialogs1` accepted.
fn listen(requests: &async_channel::Receiver<Request>) -> Command<Message> {
    let requests = requests.clone();
    Command::Perform(Box::pin(async move {
        match requests.recv().await {
            Ok(request) => Message::Request(request),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// Read the folder `request` asks for, for file chooser `id`.
pub fn read(id: u64, request: cupertino_kit::filepicker::Request) -> Command<Message> {
    cupertino_kit::task::io(
        LISTING_DEADLINE,
        move || request.run(),
        move |listing: Listing| Message::File(id, FileEvent::Listing(listing)),
    )
}

impl Model {
    /// Serve `org.cupertino.Dialogs1` once both connections are made.
    fn serve(&mut self) -> Vec<Command<Message>> {
        let (Some(session), Some(lookup)) = (&self.session, &self.lookup) else {
            return Vec::new();
        };
        if self.serving {
            return Vec::new();
        }
        self.serving = true;
        let (session, lookup) = (session.clone(), lookup.clone());
        let requests = self.requests.0.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::Served(
                service::serve(&session, lookup, move |request| {
                    requests.try_send(request).is_ok()
                })
                .await
                .map_err(|e| e.to_string()),
            )
        }))]
    }

    fn request(&mut self, request: Request) -> Vec<Command<Message>> {
        match request {
            Request::File { handle, ask, reply } => {
                let id = self.next;
                self.next += 1;
                let mut dialog = FileDialog::new(id, handle, *ask, reply, &self.home);
                let first = dialog.take_request();
                self.files.push(dialog);
                first.map(|r| read(id, r)).into_iter().collect()
            }
            Request::App { handle, ask, reply } => {
                let id = self.next;
                self.next += 1;
                self.apps.push(AppDialog::new(id, handle, *ask, reply));
                Vec::new()
            }
            Request::Update { handle, choices } => {
                if let Some(a) = self.apps.iter_mut().find(|a| a.handle() == handle) {
                    a.update_choices(choices);
                }
                Vec::new()
            }
            // a dialog dropped unanswered ends its call
            Request::Close { handle } => {
                self.files.retain(|d| d.handle() != handle);
                self.apps.retain(|a| a.handle() != handle);
                Vec::new()
            }
        }
    }

    fn file(&mut self, id: u64, event: FileEvent) -> Vec<Command<Message>> {
        let Some(index) = self.files.iter().position(|d| d.id == id) else {
            return Vec::new();
        };
        let (done, mut commands) = self.files[index].update(event, &self.places);
        if done {
            self.files.remove(index).finish();
        } else if let Some(request) = self.files[index].take_request() {
            commands.push(read(id, request));
        }
        commands
    }

    fn app(&mut self, id: u64, event: AppEvent) -> Vec<Command<Message>> {
        let Some(index) = self.apps.iter().position(|a| a.id == id) else {
            return Vec::new();
        };
        let (done, commands) = self.apps[index].update(event);
        if done {
            self.apps.remove(index).finish();
        }
        commands
    }

    fn buses(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Session(Some(connection)) => {
                self.session = Some(connection);
                self.serve()
            }
            Message::Lookup(Some(connection)) => {
                self.lookup = Some(connection);
                self.serve()
            }
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the component serves nobody: the host starts it again
                eprintln!("cupertino-shell-dialogs: no session bus connection");
                vec![Command::Exit]
            }
            Message::Lookup(None) | Message::LookupClosed => {
                eprintln!("cupertino-shell-dialogs: no session bus connection to ask who calls");
                vec![Command::Exit]
            }
            Message::Served(Ok(())) => Vec::new(),
            Message::Served(Err(e)) => {
                eprintln!(
                    "cupertino-shell-dialogs: {} is not served: {e}",
                    service::BUS_NAME
                );
                vec![Command::Exit]
            }
            _ => Vec::new(),
        }
    }
}

impl Application for Dialogs {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Dialogs shown now and then, mostly at rest: a resident GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let requests = async_channel::unbounded();
        let home = home();
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.extend(kit_bus::connect(
            Bus::Session,
            |connection| Message::Lookup(connection.ok()),
            Message::LookupClosed,
        ));
        commands.push(listen(&requests.1));
        let from = home.clone();
        commands.push(cupertino_kit::task::io(
            LOAD_DEADLINE,
            move || load(&from),
            |loaded| Message::Loaded(Taken(Arc::new(Mutex::new(Some(loaded))))),
        ));
        let model = Model {
            session: None,
            lookup: None,
            serving: false,
            requests,
            home,
            places: Vec::new(),
            icons: None,
            next: 0,
            files: Vec::new(),
            apps: Vec::new(),
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Request(request) => {
                let mut commands = model.request(request);
                commands.push(listen(&model.requests.1));
                commands
            }
            Message::Loaded(Taken(loaded)) => {
                if let Some(loaded) = loaded.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    model.places = loaded.places;
                    model.icons = Some(Rc::new(item_icons(loaded.icons)));
                }
                Vec::new()
            }
            Message::File(id, event) => model.file(id, event),
            Message::App(id, event) => model.app(id, event),
            Message::Closed(key) => {
                // the server closed a dialog's window: it ends as Cancel does
                if let Some(id) = model
                    .files
                    .iter()
                    .find(|d| d.window_key() == key)
                    .map(|d| d.id)
                {
                    return model.file(id, FileEvent::Cancel);
                }
                if let Some(id) = model
                    .apps
                    .iter()
                    .find(|a| a.window_key() == key)
                    .map(|a| a.id)
                {
                    return model.app(id, AppEvent::Cancel);
                }
                Vec::new()
            }
            message => model.buses(message),
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .files
            .iter()
            .map(|d| view::file_window(d, &model.places, model.icons.as_ref()))
            .chain(model.apps.iter().map(view::app_window))
            .collect()
    }
}

impl Program for Dialogs {
    /// Every dialog is Kit's own toplevel window, the child of the app's window the portal names.
    fn surface(_: &Model, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Window,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Closed(key) => Some(Message::Closed(key)),
            _ => None,
        }
    }
}

impl Component for Dialogs {
    const NAME: ComponentName = ComponentName::DIALOGS;
}
