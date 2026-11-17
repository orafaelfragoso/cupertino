//! The file-chooser, app-chooser, screen-sharing and screenshot windows: one for each open request, drawn with CupertinoKit. The D-Bus side
//! hands its events to the [`inbox`]; [`Dialogs`] is resident and keeps running while no dialog is open.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cupertino_assets::Assets;
use cupertino_icons::ThemeIcons;
use cupertino_kit::controls::{
    CheckBox, Image, ImageFit, ImageSource, ListView, ScrollView, Scroller, SegmentedControl,
    Selection, SplitLayout, SplitView, TextRole, WindowDragArea, PANEL_WINDOW_BACKGROUND,
    SIDEBAR_TOP,
};
use cupertino_kit::core::{
    next_resource_id, Change, Checked, CommandRegistry, Event as CoreEvent, EventCx, Key,
    MeasureCx, PaintCx, Phase, Propagation, SemanticsCx, Shortcut, Widget,
};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::filepicker::places::{session_places, Place};
use cupertino_kit::filepicker::{Listing, Request};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, ScrollState};
use cupertino_kit::platform::{DecorationMode, ForeignParent, Keysym, Mods};
use cupertino_kit::prelude::*;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use zbus::zvariant::OwnedObjectPath;

use crate::app_chooser;
use crate::chooser::{Accept, Chooser};
use crate::consent::Arming;
use crate::file_chooser::{self, Answer, Kind, Reply};
use crate::global_shortcuts::{self, Refusal};
use crate::screen_cast::{self, Source};
use crate::screenshot;

const ROW_HEIGHT: f64 = 24.0;
const ROW_ICON: f64 = 16.0;
/// Device pixels an app's own icon is loaded at: the row icon at 1× and 2×.
const APP_ICON_PX: [u32; 2] = [16, 32];
const SIDEBAR_WIDTH: f64 = 180.0;
const SAVE_AS_WIDTH: f64 = 260.0;
const GAP: f64 = 8.0;
/// Around the bottom bar's controls, which makes it 52 pt tall with a regular control; an approximate design value.
const BAR_INSETS: Insets = Insets {
    top: 12.0,
    left: 16.0,
    bottom: 12.0,
    right: 16.0,
};
const SIZE: Size = Size::new(720.0, 460.0);
/// The room above the first row of the chooser's sidebar and of its file pane: the chooser has no title bar, and its
/// first rows clear the window's round corners; an approximate design value.
const TOP_INSET: f64 = 10.0;

/// The room above the chooser's panes: none under the Save As row, which is the window's top then.
fn top_inset(d: &Dialog) -> f64 {
    if d.chooser.ask().kind == Kind::Save {
        0.0
    } else {
        TOP_INSET
    }
}
/// The prompt windows: the row of the window's buttons ([`SIDEBAR_TOP`]) over their content.
const PICKER_SIZE: Size = Size::new(480.0, 360.0 + SIDEBAR_TOP);
const PICKER_MIN_SIZE: Size = Size::new(360.0, 240.0 + SIDEBAR_TOP);
const CONSENT_SIZE: Size = Size::new(420.0, 180.0 + SIDEBAR_TOP);
/// The shortcut window grows by one line of this height for each line of the first [`SHORTCUT_ROWS`] shortcuts it
/// lists (two for a row that wraps); the rest scroll in the list.
const SHORTCUT_ROW: f64 = 20.0;
const SHORTCUT_ROWS: usize = 8;
/// Characters of a shortcut's description its row shows before it ends it with an ellipsis: two lines of the window.
const DESCRIPTION_CHARS: usize = 80;
/// Characters of a shortcut's row that fit on one line of the window, an estimate the window's height is grown by.
const SHORTCUT_LINE_CHARS: usize = 56;
/// A prompt window's title bar is only its control buttons, over the top of its glass panel.
const PROMPT_DECORATIONS: DecorationMode = DecorationMode::Unified;
/// Side of the requesting app's icon in the screen-sharing and screenshot windows.
const APP_ICON: f64 = 32.0;
const RETURN: Keysym = Keysym(0xff0d);
const ESCAPE: Keysym = Keysym(0xff1b);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
/// Characters of a file name or URI the app chooser's prompt shows before it ends it with an ellipsis.
const TARGET_CHARS: usize = 80;

/// What the D-Bus side asks of the dialogs.
pub enum Event {
    FileChooser(file_chooser::Event),
    ScreenCast(screen_cast::Event),
    AppChooser(app_chooser::Event),
    Screenshot(screenshot::Event),
    GlobalShortcuts(global_shortcuts::Event),
}

impl From<global_shortcuts::Event> for Event {
    fn from(event: global_shortcuts::Event) -> Self {
        Event::GlobalShortcuts(event)
    }
}

impl From<file_chooser::Event> for Event {
    fn from(event: file_chooser::Event) -> Self {
        Event::FileChooser(event)
    }
}

impl From<screen_cast::Event> for Event {
    fn from(event: screen_cast::Event) -> Self {
        Event::ScreenCast(event)
    }
}

impl From<app_chooser::Event> for Event {
    fn from(event: app_chooser::Event) -> Self {
        Event::AppChooser(event)
    }
}

impl From<screenshot::Event> for Event {
    fn from(event: screenshot::Event) -> Self {
        Event::Screenshot(event)
    }
}

/// Events from the D-Bus side waiting for a run of [`Dialogs`].
pub struct Inbox {
    events: Mutex<VecDeque<Event>>,
    wake: (async_channel::Sender<()>, async_channel::Receiver<()>),
}

/// The process's inbox.
pub fn inbox() -> &'static Inbox {
    static INBOX: OnceLock<Inbox> = OnceLock::new();
    INBOX.get_or_init(|| Inbox {
        events: Mutex::new(VecDeque::new()),
        wake: async_channel::bounded(1),
    })
}

impl Inbox {
    pub fn push(&self, event: impl Into<Event>) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event.into());
        // a token already waiting wakes for this event too
        let _ = self.wake.0.try_send(());
    }

    fn take(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

/// Start the arming delay of consent window `id`, whose window is `window`, once the window has presented its first
/// frame: a window the server is slow to show takes no answer before the person could see it for the whole delay.
fn arm_when_shown(id: u64, window: WindowKey) -> Command<Message> {
    Command::OnPresented {
        window,
        message: Message::Shown(id),
    }
}

/// Arm consent window `id` once its arming delay has passed, on a thread of its own.
fn arm(id: u64, arming: Arming) -> Command<Message> {
    let (sender, armed) = async_channel::bounded(1);
    let wait = arming.remaining(Instant::now());
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        let _ = sender.send_blocking(());
    });
    Command::Perform(Box::pin(async move {
        let _ = armed.recv().await;
        Message::Armed(id)
    }))
}

fn woken() -> Command<Message> {
    Command::Perform(Box::pin(async {
        let _ = inbox().wake.1.recv().await;
        Message::Woken
    }))
}

/// Read `request` on a thread of its own: a folder on a hung mount may never answer.
fn read(id: u64, request: Request) -> Command<Message> {
    let (sender, listing) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = sender.send_blocking(request.run());
    });
    Command::Perform(Box::pin(async move {
        listing
            .recv()
            .await
            .map_or(Message::Ignore, |l| Message::Listing(id, l))
    }))
}

pub struct Dialogs;

pub struct Model {
    home: PathBuf,
    places: Vec<Place>,
    next: u64,
    open: Vec<Dialog>,
    pickers: Vec<Picker>,
    apps: Vec<AppPicker>,
    app_icons: AppIcons,
    /// The icons of the file chooser's rows, by the items' types.
    file_icons: RefCell<FileIcons>,
    consents: Vec<Consent>,
    shortcut_consents: Vec<ShortcutConsent>,
}

/// The apps' own icons by the name of their `Icon=`, each name loaded once.
struct AppIcons {
    theme: ThemeIcons,
    /// The renditions of a name; none when it resolves to no icon.
    loaded: HashMap<String, Vec<ImageSource>>,
}

impl AppIcons {
    fn load(&mut self, choices: &[app_chooser::Choice]) {
        self.load_names(choices.iter().filter_map(|c| c.icon.as_deref()));
    }

    fn load_names<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            let theme = &mut self.theme;
            self.loaded.entry(name.to_owned()).or_insert_with(|| {
                APP_ICON_PX
                    .iter()
                    .filter_map(|&px| theme.load(name, px))
                    .map(|pixmap| ImageSource {
                        id: next_resource_id(),
                        image: ImageResource {
                            revision: 1,
                            width: pixmap.width(),
                            height: pixmap.height(),
                            stride: pixmap.width() * 4,
                            format: PixelFormat::Rgba8Premultiplied,
                            pixels: pixmap.take().into(),
                        },
                    })
                    .collect()
            });
        }
    }

    /// The renditions of the icon of `choice`; `None` when it has none.
    fn of(&self, choice: &app_chooser::Choice) -> Option<Vec<ImageSource>> {
        self.named(choice.icon.as_deref()?)
    }

    /// The renditions of the icon `name`, loaded before; `None` when it resolves to none.
    fn named(&self, name: &str) -> Option<Vec<ImageSource>> {
        let sources = self.loaded.get(name)?;
        (!sources.is_empty()).then(|| sources.clone())
    }
}

struct Dialog {
    id: u64,
    handle: OwnedObjectPath,
    reply: Reply,
    chooser: Chooser,
    rows: Selection,
    scroll: ScrollState,
    places_scroll: ScrollState,
    split: SplitLayout,
    name_revision: u64,
    /// The file a save would replace, while the person is asked.
    replace: Option<PathBuf>,
}

/// The chooser of what a screen-sharing request shares.
struct Picker {
    id: u64,
    handle: OwnedObjectPath,
    reply: screen_cast::Reply,
    ask: screen_cast::Ask,
    rows: Selection,
    scroll: ScrollState,
    /// The person ticked to let the app restore the share.
    remember: bool,
    /// The arming delay has passed: a row can be picked and shared.
    armed: bool,
    /// From the window's first presented frame; `None` before it.
    arming: Option<Arming>,
}

/// The question whether an app may take a screenshot.
struct Consent {
    id: u64,
    handle: OwnedObjectPath,
    reply: screenshot::Reply,
    ask: screenshot::Ask,
    /// The arming delay has passed: Take Screenshot can be pressed.
    armed: bool,
    /// From the window's first presented frame; `None` before it.
    arming: Option<Arming>,
}

/// The question whether an app may use keyboard shortcuts while another app has the keyboard.
struct ShortcutConsent {
    id: u64,
    handle: OwnedObjectPath,
    reply: global_shortcuts::Reply,
    ask: global_shortcuts::Ask,
    /// The list of the shortcuts.
    scroll: ScrollState,
    /// The arming delay has passed: Allow can be pressed.
    armed: bool,
    arming: Option<Arming>,
}

/// Whether a window armed by its message, whose delay started at `arming`, takes an answer now: the delay has passed
/// by the clock too.
fn ready(armed: bool, arming: Option<Arming>) -> bool {
    armed && arming.is_some_and(|a| a.armed(Instant::now()))
}

impl Picker {
    fn chosen(&self) -> Option<&Source> {
        self.rows.cursor().and_then(|i| self.ask.sources.get(i))
    }

    /// Whether the window takes a pick: armed by its message, and the delay has passed by the clock too.
    fn ready(&self) -> bool {
        ready(self.armed, self.arming)
    }
}

/// The chooser of the app that opens a link or a file.
struct AppPicker {
    id: u64,
    handle: OwnedObjectPath,
    reply: app_chooser::Reply,
    ask: app_chooser::Ask,
    rows: Selection,
    scroll: ScrollState,
    /// The app chosen while its activation token is asked for; the window answers when the token arrives.
    opening: Option<String>,
}

impl AppPicker {
    fn chosen(&self) -> Option<&app_chooser::Choice> {
        self.rows.cursor().and_then(|i| self.ask.choices.get(i))
    }

    /// The rows with the choice `id` selected, or none when it is not offered.
    fn select(&mut self, id: Option<&str>) {
        self.rows = Selection::default();
        if let Some(i) = id.and_then(|id| self.ask.choices.iter().position(|c| c.id == id)) {
            self.rows.select_only(i);
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Woken,
    Ignore,
    Listing(u64, Listing),
    Place(u64, usize),
    PlacesScroll(u64, ScrollState),
    Scroll(u64, ScrollState),
    Select(u64, Selection),
    Open(u64, usize),
    Parent(u64),
    Split(u64, SplitLayout),
    Name(u64, TextChange),
    Filter(u64, usize),
    Choice(u64, usize, String),
    Accept(u64),
    Replace(u64),
    KeepEditing(u64),
    Cancel(u64),
    PickerScroll(u64, ScrollState),
    PickerSelect(u64, Selection),
    Share(u64),
    AppScroll(u64, ScrollState),
    AppSelect(u64, Selection),
    OpenWith(u64),
    /// The activation token the server made for app chooser `0`'s choice `1`.
    Opened(u64, String, Option<String>),
    TakeScreenshot(u64),
    /// Allow shortcut window `0`'s shortcuts, remembered for its app.
    AllowShortcuts(u64),
    /// Don't Allow them, remembered for its app.
    DenyShortcuts(u64),
    /// The list of shortcut window `0` moved.
    ShortcutsScroll(u64, ScrollState),
    /// Consent window `0` has presented its first frame: its arming delay starts.
    Shown(u64),
    /// The arming delay of consent window `0` has passed.
    Armed(u64),
    /// The person ticked or unticked remembering the share of chooser `0`.
    Remember(u64, bool),
}

const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// The session's MIME database, which types the file chooser's rows for their icons; none when it cannot be read.
fn types() -> cupertino_apps::MimeDatabase {
    cupertino_apps::XdgDirs::from_env()
        .ok()
        .and_then(|dirs| cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).ok())
        .unwrap_or_default()
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl Model {
    fn dialog(&mut self, id: u64) -> Option<&mut Dialog> {
        self.open.iter_mut().find(|d| d.id == id)
    }

    fn picker(&mut self, id: u64) -> Option<&mut Picker> {
        self.pickers.iter_mut().find(|p| p.id == id)
    }

    fn app_picker(&mut self, id: u64) -> Option<&mut AppPicker> {
        self.apps.iter_mut().find(|a| a.id == id)
    }

    fn take(&mut self, events: Vec<Event>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::FileChooser(file_chooser::Event::Ask { handle, ask, reply }) => {
                    let id = self.next;
                    self.next += 1;
                    let mut chooser = Chooser::new(*ask, &self.home);
                    commands.extend(chooser.take_request().map(|r| read(id, r)));
                    self.open.push(Dialog {
                        id,
                        handle,
                        reply,
                        chooser,
                        rows: Selection::default(),
                        scroll: NO_SCROLL,
                        places_scroll: NO_SCROLL,
                        split: SplitLayout {
                            position: SIDEBAR_WIDTH,
                            collapsed: None,
                        },
                        name_revision: 0,
                        replace: None,
                    });
                }
                Event::FileChooser(file_chooser::Event::Close { handle }) => {
                    self.open.retain(|d| d.handle != handle)
                }
                Event::ScreenCast(screen_cast::Event::Ask { handle, ask, reply }) => {
                    let id = self.next;
                    self.next += 1;
                    self.app_icons.load_names(ask.app_icon.as_deref());
                    // nothing is chosen until the person picks a row after the arming delay, so keys meant for the
                    // app cannot share
                    commands.push(arm_when_shown(id, picker_window_key(id)));
                    self.pickers.push(Picker {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        rows: Selection::default(),
                        scroll: NO_SCROLL,
                        remember: false,
                        armed: false,
                        arming: None,
                    });
                }
                Event::ScreenCast(screen_cast::Event::Close { handle }) => {
                    self.pickers.retain(|p| p.handle != handle)
                }
                Event::AppChooser(app_chooser::Event::Ask { handle, ask, reply }) => {
                    self.app_icons.load(&ask.choices);
                    let id = self.next;
                    self.next += 1;
                    let mut picker = AppPicker {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        rows: Selection::default(),
                        scroll: NO_SCROLL,
                        opening: None,
                    };
                    let last = picker.ask.last_choice.clone();
                    picker.select(last.as_deref());
                    self.apps.push(picker);
                }
                Event::AppChooser(app_chooser::Event::Update { handle, choices }) => {
                    self.app_icons.load(&choices);
                    if let Some(a) = self.apps.iter_mut().find(|a| a.handle == handle) {
                        let chosen = a.chosen().map(|c| c.id.clone());
                        a.ask.choices = choices;
                        a.select(chosen.as_deref());
                    }
                }
                Event::AppChooser(app_chooser::Event::Close { handle }) => {
                    self.apps.retain(|a| a.handle != handle)
                }
                Event::Screenshot(screenshot::Event::Ask { handle, ask, reply }) => {
                    let id = self.next;
                    self.next += 1;
                    self.app_icons.load_names(ask.app_icon.as_deref());
                    commands.push(arm_when_shown(id, consent_window_key(id)));
                    self.consents.push(Consent {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        armed: false,
                        arming: None,
                    });
                }
                Event::Screenshot(screenshot::Event::Close { handle }) => {
                    self.consents.retain(|c| c.handle != handle)
                }
                Event::GlobalShortcuts(global_shortcuts::Event::Ask { handle, ask, reply }) => {
                    let id = self.next;
                    self.next += 1;
                    self.app_icons.load_names(ask.app_icon.as_deref());
                    commands.push(arm_when_shown(id, shortcuts_window_key(id)));
                    self.shortcut_consents.push(ShortcutConsent {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        scroll: NO_SCROLL,
                        armed: false,
                        arming: None,
                    });
                }
                Event::GlobalShortcuts(global_shortcuts::Event::Close { handle }) => {
                    self.shortcut_consents.retain(|c| c.handle != handle)
                }
            }
        }
        commands
    }

    /// End dialog `id` with `answer`.
    fn finish(&mut self, id: u64, answer: Answer) {
        if let Some(i) = self.open.iter().position(|d| d.id == id) {
            self.open.remove(i).reply.send(answer);
        }
    }

    /// End chooser `id` with `answer`.
    fn finish_picker(&mut self, id: u64, answer: screen_cast::Answer) {
        if let Some(i) = self.pickers.iter().position(|p| p.id == id) {
            self.pickers.remove(i).reply.send(answer);
        }
    }

    /// End app chooser `id` with `answer`.
    fn finish_app(&mut self, id: u64, answer: app_chooser::Answer) {
        if let Some(i) = self.apps.iter().position(|a| a.id == id) {
            self.apps.remove(i).reply.send(answer);
        }
    }

    /// End consent window `id` with `answer`.
    fn finish_consent(&mut self, id: u64, answer: screenshot::Answer) {
        if let Some(i) = self.consents.iter().position(|c| c.id == id) {
            self.consents.remove(i).reply.send(answer);
        }
    }

    /// End shortcut window `id` with `answer`.
    fn finish_shortcuts(&mut self, id: u64, answer: global_shortcuts::Answer) {
        if let Some(i) = self.shortcut_consents.iter().position(|c| c.id == id) {
            self.shortcut_consents.remove(i).reply.send(answer);
        }
    }
}

impl Dialog {
    /// The list's selection after the rows changed: the chooser's selected paths where they are shown now.
    fn shown_selection(&self) -> Selection {
        let rows: Vec<usize> = self
            .chooser
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| self.chooser.selection().contains(&e.path))
            .map(|(i, _)| i)
            .collect();
        let cursor = rows.last().copied();
        Selection::new(rows.into_iter().map(|i| i..i + 1), cursor, cursor)
    }
}

impl Application for Dialogs {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The file chooser and the portal's dialogs are shown now and then, mostly at rest: a resident GPU context would
    /// only cost memory.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let home = home();
        let mut model = Model {
            places: session_places(&home),
            home,
            next: 0,
            open: Vec::new(),
            pickers: Vec::new(),
            apps: Vec::new(),
            app_icons: AppIcons {
                theme: ThemeIcons::new(&Assets::discover()),
                loaded: HashMap::new(),
            },
            file_icons: RefCell::new(FileIcons::discover(types(), HashMap::new())),
            consents: Vec::new(),
            shortcut_consents: Vec::new(),
        };
        let mut commands = model.take(inbox().take());
        commands.push(woken());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        let read_next = |d: &mut Dialog| d.chooser.take_request().map(|r| read(d.id, r));
        match message {
            Message::Woken => {
                commands = model.take(inbox().take());
                commands.push(woken());
            }
            Message::Ignore => {}
            Message::Listing(id, listing) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser.apply(listing);
                    d.rows = d.shown_selection();
                    commands.extend(read_next(d));
                }
            }
            Message::Place(id, row) => {
                let place = model.places.get(row).map(|p| p.path.clone());
                if let (Some(d), Some(path)) = (model.dialog(id), place) {
                    d.chooser.go(&path);
                    commands.extend(read_next(d));
                }
            }
            Message::PlacesScroll(id, scroll) => {
                if let Some(d) = model.dialog(id) {
                    d.places_scroll = scroll;
                }
            }
            Message::Scroll(id, scroll) => {
                if let Some(d) = model.dialog(id) {
                    d.scroll = scroll;
                }
            }
            Message::Select(id, rows) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser
                        .select(rows.ranges().iter().flat_map(Clone::clone));
                    d.rows = rows;
                    if d.chooser.ask().kind == Kind::Save {
                        if let Some(file) = d
                            .chooser
                            .entries()
                            .iter()
                            .find(|e| !e.is_dir && d.chooser.selection().contains(&e.path))
                        {
                            let name = file.name.clone();
                            d.chooser.set_name(name);
                            d.name_revision += 1;
                        }
                    }
                }
            }
            Message::Open(id, row) => {
                let accept = match model.dialog(id) {
                    Some(d) => {
                        let accept = d.chooser.open(row);
                        commands.extend(read_next(d));
                        accept
                    }
                    None => Accept::Nothing,
                };
                commands.extend(Self::accepted(model, id, accept));
            }
            Message::Parent(id) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser.parent();
                    commands.extend(read_next(d));
                }
            }
            Message::Split(id, split) => {
                if let Some(d) = model.dialog(id) {
                    d.split = split;
                }
            }
            Message::Name(id, change) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser.set_name(change.text);
                    d.name_revision = change.revision;
                    d.replace = None;
                }
            }
            Message::Filter(id, index) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser.set_filter(index);
                    d.rows = d.shown_selection();
                }
            }
            Message::Choice(id, index, value) => {
                if let Some(d) = model.dialog(id) {
                    d.chooser.set_choice(index, &value);
                }
            }
            Message::Accept(id) => {
                let accept = model
                    .dialog(id)
                    .map_or(Accept::Nothing, |d| d.chooser.accept());
                commands.extend(Self::accepted(model, id, accept));
            }
            Message::Replace(id) => {
                let answer = model.dialog(id).and_then(|d| {
                    let path = d.replace.take()?;
                    Some(d.chooser.replace(path))
                });
                if let Some(answer) = answer {
                    model.finish(id, answer);
                }
            }
            Message::KeepEditing(id) => {
                if let Some(d) = model.dialog(id) {
                    d.replace = None;
                }
            }
            Message::Cancel(id) => {
                model.finish(id, Answer::Cancelled);
                model.finish_picker(id, screen_cast::Answer::Cancelled);
                model.finish_app(id, app_chooser::Answer::Cancelled);
                model.finish_consent(id, screenshot::Answer::Cancelled);
                model.finish_shortcuts(id, global_shortcuts::Answer::Dismissed);
            }
            Message::DenyShortcuts(id) => {
                model.finish_shortcuts(id, global_shortcuts::Answer::Denied)
            }
            Message::ShortcutsScroll(id, scroll) => {
                if let Some(c) = model.shortcut_consents.iter_mut().find(|c| c.id == id) {
                    c.scroll = scroll;
                }
            }
            Message::AllowShortcuts(id) => {
                if model
                    .shortcut_consents
                    .iter()
                    .any(|c| c.id == id && ready(c.armed, c.arming))
                {
                    model.finish_shortcuts(id, global_shortcuts::Answer::Allowed)
                }
            }
            Message::PickerScroll(id, scroll) => {
                if let Some(p) = model.picker(id) {
                    p.scroll = scroll;
                }
            }
            Message::PickerSelect(id, rows) => {
                // a row is not picked before the window is armed: a key typed for the app moves nothing
                if let Some(p) = model.picker(id).filter(|p| p.ready()) {
                    p.rows = rows;
                }
            }
            Message::Share(id) => {
                let chosen = model
                    .picker(id)
                    .filter(|p| p.ready())
                    .and_then(|p| Some((p.chosen()?.clone(), p.remember && p.ask.offer_remember)));
                if let Some((source, remember)) = chosen {
                    model.finish_picker(id, screen_cast::Answer::Chosen { source, remember });
                }
            }
            Message::Remember(id, on) => {
                if let Some(p) = model.picker(id).filter(|p| p.ready()) {
                    p.remember = on;
                }
            }
            Message::Shown(id) => {
                let arming = Arming::new(Instant::now());
                let picker = model.picker(id).filter(|p| p.arming.is_none());
                let shown = match picker {
                    Some(p) => {
                        p.arming = Some(arming);
                        true
                    }
                    None => model
                        .consents
                        .iter_mut()
                        .find(|c| c.id == id && c.arming.is_none())
                        .map(|c| c.arming = Some(arming))
                        .or_else(|| {
                            model
                                .shortcut_consents
                                .iter_mut()
                                .find(|c| c.id == id && c.arming.is_none())
                                .map(|c| c.arming = Some(arming))
                        })
                        .is_some(),
                };
                if shown {
                    commands.push(arm(id, arming));
                }
            }
            Message::Armed(id) => {
                if let Some(p) = model.picker(id) {
                    p.armed = true;
                }
                if let Some(c) = model.consents.iter_mut().find(|c| c.id == id) {
                    c.armed = true;
                }
                if let Some(c) = model.shortcut_consents.iter_mut().find(|c| c.id == id) {
                    c.armed = true;
                }
            }
            Message::AppScroll(id, scroll) => {
                if let Some(a) = model.app_picker(id) {
                    a.scroll = scroll;
                }
            }
            Message::AppSelect(id, rows) => {
                if let Some(a) = model.app_picker(id) {
                    a.rows = rows;
                }
            }
            Message::OpenWith(id) => {
                let Some(a) = model.app_picker(id).filter(|a| a.opening.is_none()) else {
                    return commands;
                };
                let Some(choice) = a.chosen().map(|c| c.id.clone()) else {
                    return commands;
                };
                if !a.ask.wants_token {
                    model.finish_app(
                        id,
                        app_chooser::Answer::Chosen {
                            id: choice,
                            token: None,
                        },
                    );
                    return commands;
                }
                // made for the key or click that chose, on this window, so the server lets the app take the focus
                a.opening = Some(choice.clone());
                let chosen = choice.clone();
                commands.push(Command::ActivationToken {
                    window: app_window_key(id),
                    app_id: Some(choice),
                    on_token: Box::new(move |token| Message::Opened(id, chosen, token)),
                });
            }
            Message::Opened(id, choice, token) => {
                if model
                    .app_picker(id)
                    .is_some_and(|a| a.opening.as_deref() == Some(choice.as_str()))
                {
                    model.finish_app(id, app_chooser::Answer::Chosen { id: choice, token });
                }
            }
            Message::TakeScreenshot(id) => {
                if model
                    .consents
                    .iter()
                    .any(|c| c.id == id && ready(c.armed, c.arming))
                {
                    model.finish_consent(id, screenshot::Answer::Granted)
                }
            }
        }
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .open
            .iter()
            .map(|d| window(d, &model.places, &model.file_icons))
            .chain(
                model
                    .pickers
                    .iter()
                    .map(|p| picker_window(p, &model.app_icons)),
            )
            .chain(model.apps.iter().map(|a| app_window(a, &model.app_icons)))
            .chain(
                model
                    .consents
                    .iter()
                    .map(|c| consent_window(c, &model.app_icons)),
            )
            .chain(
                model
                    .shortcut_consents
                    .iter()
                    .map(|c| shortcuts_window(c, &model.app_icons)),
            )
            .collect()
    }
}

impl Dialogs {
    fn accepted(model: &mut Model, id: u64, accept: Accept) -> Vec<Command<Message>> {
        match accept {
            Accept::Answer(answer) => model.finish(id, answer),
            Accept::Replace(path) => {
                if let Some(d) = model.dialog(id) {
                    d.replace = Some(path);
                    return vec![Command::Focus(Key::from("replace"))];
                }
            }
            Accept::Nothing => {}
        }
        Vec::new()
    }
}

fn text(content: String, role: TextRole, secondary: bool, selected: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary,
        selected,
    })
}

fn symbol(name: &str) -> Element<Message> {
    Element::new(Icon {
        name: name.to_owned(),
        size: ROW_ICON,
        direction: Default::default(),
        label: None,
        tint: None,
    })
}

fn row(icon: Element<Message>, name: String, selected: bool) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: GAP,
            bottom: 0.0,
            right: GAP,
        },
        spacing: 6.0,
        align: Align::Center,
        children: vec![icon, text(name, TextRole::Body, false, selected)],
    })
}

fn button(label: String, role: ButtonRole, enabled: bool, on_press: Message) -> Element<Message> {
    Element::new(Button {
        label,
        role,
        style: ButtonStyle::Bordered,
        enabled,
        busy: false,
        on_press: Some(on_press),
    })
}

/// The Cancel button of a consent window: the window's keyboard focus starts on it, so the answer a stray key reaches
/// is the one that denies. Until the window is `armed` the keys that press a button (Return, Enter and Space) are taken
/// before they reach it, so a key typed for the app does nothing at all; Tab and Escape still work, and a click on
/// Cancel denies at once.
fn deny_button(id: u64, armed: bool) -> Element<Message> {
    labelled_deny_button("Cancel", Message::Cancel(id), armed)
}

/// [`deny_button`] with its own label and message.
fn labelled_deny_button(label: &str, on_press: Message, armed: bool) -> Element<Message> {
    Element::new(Deny {
        armed,
        children: vec![button(label.to_owned(), ButtonRole::Normal, true, on_press).key("cancel")],
    })
}

/// Holds a consent window's Cancel button; see [`deny_button`].
struct Deny {
    armed: bool,
    children: Vec<Element<Message>>,
}

/// The keys that press a focused button.
fn presses(keysym: Keysym) -> bool {
    matches!(keysym, RETURN | KP_ENTER | SPACE)
}

impl Widget<Message> for Deny {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.armed == old.armed {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &CoreEvent, cx: &mut EventCx<'_, Message>) -> Propagation {
        match event {
            CoreEvent::Key(key, _)
                if !self.armed && cx.phase() == Phase::Capture && presses(key.keysym) =>
            {
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.initial_focus();
    }
}

/// A choice among several, in the bottom bar.
fn pick(
    label: String,
    options: Vec<String>,
    selected: Option<usize>,
    on_select: impl Fn(usize) -> Message + 'static,
) -> Element<Message> {
    Element::new(SegmentedControl {
        label,
        segments: options,
        selected,
        enabled: true,
        on_select: Some(Box::new(on_select)),
    })
}

fn sidebar(d: &Dialog, places: &[Place]) -> Element<Message> {
    let id = d.id;
    let open = d.chooser.picker().dir().to_path_buf();
    let mut selection = Selection::default();
    if let Some(i) = places.iter().position(|p| p.path == open) {
        selection.select_only(i);
    }
    let rows: Vec<(&'static str, String)> = places
        .iter()
        .map(|p| (p.symbol, p.name.to_owned()))
        .collect();
    let names: Vec<String> = rows.iter().map(|(_, n)| n.clone()).collect();
    let keys: Vec<&'static str> = places.iter().map(|p| p.name).collect();
    let list = Element::from(ListView {
        label: "Places".into(),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: d.places_scroll,
        selection,
        multiple: false,
        initial: false,
        row: Box::new(move |i, selected| row(symbol(rows[i].0), rows[i].1.clone(), selected)),
        key: Some(Box::new(move |i| Key::from(keys[i]))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::PlacesScroll(id, s)),
        on_select: Some(Box::new(move |s: Selection| {
            s.cursor()
                .map_or(Message::Ignore, |i| Message::Place(id, i))
        })),
        double_click_activates: true,
        on_activate: Some(Box::new(move |i| Message::Place(id, i))),
    })
    .key("places");
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: top_inset(d),
            ..Insets::ZERO
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![list],
    })
}

fn files(d: &Dialog, icons: &RefCell<FileIcons>) -> Element<Message> {
    let id = d.id;
    let mut icons = icons.borrow_mut();
    let rows: Vec<(Vec<ImageSource>, String)> = d
        .chooser
        .entries()
        .iter()
        .map(|e| {
            let mime = icons.database().file_type(&e.path, e.is_dir).to_owned();
            let icon = icons.images(&e.path, &mime, false, ROW_ICON);
            (icon, e.name.clone())
        })
        .collect();
    let names: Vec<String> = rows.iter().map(|(_, n)| n.clone()).collect();
    let keys = names.clone();
    let label = d
        .chooser
        .picker()
        .dir()
        .file_name()
        .map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned());
    let list = Element::from(ListView {
        label,
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: d.scroll,
        selection: d.rows.clone(),
        multiple: d.chooser.ask().multiple,
        initial: false,
        row: Box::new(move |i, selected| {
            let (icon, name) = &rows[i];
            let icon = Element::new(Image {
                sources: icon.clone(),
                size: Size::new(ROW_ICON, ROW_ICON),
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            });
            row(icon, name.clone(), selected)
        }),
        key: Some(Box::new(move |i| Key::from(keys[i].clone()))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::Scroll(id, s)),
        on_select: Some(Box::new(move |s| Message::Select(id, s))),
        double_click_activates: true,
        on_activate: Some(Box::new(move |i| Message::Open(id, i))),
    })
    .key("files");
    let unreadable = d.chooser.picker().unreadable().then(|| {
        text(
            "This folder can't be opened.".to_owned(),
            TextRole::Caption,
            true,
            false,
        )
        .key("unreadable")
    });
    // the row at the top of the pane moves the window, which has no title bar
    let path = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 6.0 + top_inset(d),
            left: GAP,
            bottom: 6.0,
            right: GAP,
        },
        spacing: GAP,
        align: Align::Center,
        children: vec![
            button(
                "Enclosing Folder".to_owned(),
                ButtonRole::Normal,
                d.chooser.picker().dir().parent().is_some(),
                Message::Parent(id),
            )
            .key("parent"),
            text(
                d.chooser.picker().dir().display().to_string(),
                TextRole::Caption,
                true,
                false,
            )
            .key("path"),
        ],
    });
    let path = Element::from(WindowDragArea { content: path });
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: [Some(path), unreadable, Some(list)]
            .into_iter()
            .flatten()
            .collect(),
    })
}

fn save_as(d: &Dialog) -> Element<Message> {
    let id = d.id;
    // the window's top row, which moves the window: it has no title bar
    let row = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Center,
        children: vec![
            Element::new(Spacer { min: 0.0 }),
            text("Save As:".to_owned(), TextRole::Body, false, false),
            Element::new(TextField {
                label: "Save As".into(),
                placeholder: String::new(),
                value: d.chooser.name().to_owned(),
                revision: d.name_revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(move |c| Message::Name(id, c))),
                on_submit: Some(Message::Accept(id)),
                selection: Some(0..Chooser::stem_len(d.chooser.name())),
                initial: true,
            })
            .key("name")
            .width(SAVE_AS_WIDTH),
            Element::new(Spacer { min: 0.0 }),
        ],
    });
    Element::from(WindowDragArea { content: row })
}

fn bar(d: &Dialog) -> Element<Message> {
    let id = d.id;
    let ask = d.chooser.ask();
    let mut leading = Vec::new();
    if let Some(path) = &d.replace {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        leading.push(
            text(
                format!("“{name}” already exists. Do you want to replace it?"),
                TextRole::Body,
                false,
                false,
            )
            .key("replace-question"),
        );
        return bar_row(
            leading,
            vec![
                button(
                    "Cancel".to_owned(),
                    ButtonRole::Normal,
                    true,
                    Message::KeepEditing(id),
                )
                .key("keep"),
                button(
                    "Replace".to_owned(),
                    ButtonRole::Destructive,
                    true,
                    Message::Replace(id),
                )
                .key("replace"),
            ],
        );
    }
    if !ask.filters.is_empty() {
        leading.push(
            pick(
                "Format".to_owned(),
                ask.filters.iter().map(|f| f.name.clone()).collect(),
                d.chooser.filter(),
                move |i| Message::Filter(id, i),
            )
            .key("filters"),
        );
    }
    for (i, choice) in ask.choices.iter().enumerate() {
        let value = d.chooser.choice(i).unwrap_or_default().to_owned();
        let element = if choice.options.is_empty() {
            Element::new(CheckBox {
                label: choice.label.clone(),
                checked: if value == "true" {
                    Checked::True
                } else {
                    Checked::False
                },
                enabled: true,
                on_toggle: Some(Box::new(move |on: bool| {
                    Message::Choice(id, i, on.to_string())
                })),
            })
        } else {
            let ids: Vec<String> = choice.options.iter().map(|(o, _)| o.clone()).collect();
            pick(
                choice.label.clone(),
                choice.options.iter().map(|(_, l)| l.clone()).collect(),
                ids.iter().position(|o| *o == value),
                move |o| Message::Choice(id, i, ids[o].clone()),
            )
        };
        leading.push(element.key(Key::Int(i as u64)));
    }
    bar_row(
        leading,
        vec![
            button(
                "Cancel".to_owned(),
                ButtonRole::Normal,
                true,
                Message::Cancel(id),
            )
            .key("cancel"),
            button(
                d.chooser.accept_label(),
                ButtonRole::Default,
                d.chooser.accept() != Accept::Nothing,
                Message::Accept(id),
            )
            .key("accept"),
        ],
    )
}

/// A prompt window's content from the top of its panel down, under the row of the window's buttons; a press its
/// controls do not take moves the window.
fn prompt_content(children: Vec<Element<Message>>) -> Element<Message> {
    Element::from(WindowDragArea {
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: SIDEBAR_TOP,
                ..Insets::ZERO
            },
            spacing: 0.0,
            align: Align::Stretch,
            children,
        }),
    })
}

fn bar_row(
    mut leading: Vec<Element<Message>>,
    trailing: Vec<Element<Message>>,
) -> Element<Message> {
    leading.push(Element::new(Spacer { min: GAP }));
    leading.extend(trailing);
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Center,
        children: leading,
    })
}

fn window(d: &Dialog, places: &[Place], icons: &RefCell<FileIcons>) -> Window<Message> {
    let id = d.id;
    let ask = d.chooser.ask();
    let browser = Element::from(SplitView {
        label: "Sidebar".into(),
        axis: Axis::Horizontal,
        leading: sidebar(d, places),
        trailing: files(d, icons),
        layout: d.split,
        min_leading: 120.0,
        max_leading: f64::INFINITY,
        min_trailing: 320.0,
        on_change: Some(Box::new(move |l| Message::Split(id, l))),
    })
    .key("browser");
    let mut children = Vec::new();
    if ask.kind == Kind::Save {
        children.push(save_as(d));
        children.push(Element::new(Separator {
            axis: Axis::Horizontal,
        }));
    }
    children.push(browser);
    children.push(Element::new(Separator {
        axis: Axis::Horizontal,
    }));
    children.push(bar(d));
    let title = if ask.title.is_empty() {
        d.chooser.accept_label()
    } else {
        ask.title.clone()
    };
    Window {
        key: WindowKey(Key::Str(format!("chooser-{id}").into())),
        spec: WindowSpec {
            title,
            app_id: "org.cupertino.Portal".into(),
            size: SIZE,
            min_size: Some(Size::new(560.0, 340.0)),
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: foreign_parent(&ask.parent_window, ask.modal),
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(Message::Cancel(id)),
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children,
        }),
    }
}

/// The window of another application a request names as its parent, for the dialog's window: the dialog opens as its
/// child, and as its modal dialog when `modal`.
fn foreign_parent(parent_window: &str, modal: bool) -> Option<ForeignParent> {
    crate::wayland_parent(parent_window).map(|handle| ForeignParent {
        handle: handle.to_owned(),
        modal,
    })
}

/// A source as its row shows it: the display's description or connector name, the window's title and app.
fn source_row(source: &Source) -> (&'static str, String) {
    match source {
        Source::Display {
            name, description, ..
        } => (
            "symbol.desktop",
            if description.is_empty() {
                name.clone()
            } else {
                description.clone()
            },
        ),
        Source::Window { title, app_id, .. } => (
            "symbol.applications",
            match (title.is_empty(), app_id.is_empty()) {
                (false, false) => format!("{title} ({app_id})"),
                (false, true) => title.clone(),
                (true, _) => app_id.clone(),
            },
        ),
    }
}

/// The requesting app's icon at [`APP_ICON`], else the applications symbol.
fn app_icon(icon: Option<&str>, app_icons: &AppIcons) -> Element<Message> {
    match icon.and_then(|name| app_icons.named(name)) {
        Some(sources) => Element::new(Image {
            sources,
            size: Size::new(APP_ICON, APP_ICON),
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        }),
        None => Element::new(Icon {
            name: "symbol.applications".to_owned(),
            size: APP_ICON,
            direction: Default::default(),
            label: None,
            tint: None,
        }),
    }
    .key("app-icon")
}

/// A window's header: the requesting app's icon above `prompt`, which wraps inside the window.
fn app_header(icon: Option<&str>, app_icons: &AppIcons, prompt: String) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Stretch,
        children: vec![
            // at the leading edge, not stretched across the header
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Center,
                children: vec![app_icon(icon, app_icons), Element::new(Spacer { min: 0.0 })],
            }),
            Element::new(Text {
                content: AttributedText::plain(prompt, TextStyle::new(13.0)),
                role: TextRole::Body,
                wrap: true,
                secondary: false,
                selected: false,
            })
            .key("prompt"),
        ],
    })
}

/// What the screen-sharing window asks: the app by its name, else an unnamed app.
fn share_prompt(ask: &screen_cast::Ask) -> String {
    let app = match &ask.app_name {
        Some(name) => format!("“{name}” wants to share your screen."),
        None => "An application wants to share your screen.".to_owned(),
    };
    if ask.sources.is_empty() {
        format!("{app} There is no display or window to share.")
    } else {
        format!("{app} Choose a display or a window.")
    }
}

fn picker_window(p: &Picker, app_icons: &AppIcons) -> Window<Message> {
    let id = p.id;
    let rows: Vec<(&'static str, String)> = p.ask.sources.iter().map(source_row).collect();
    let names: Vec<String> = rows.iter().map(|(_, n)| n.clone()).collect();
    let list = Element::from(ListView {
        label: "Displays and Windows".into(),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: p.scroll,
        selection: p.rows.clone(),
        multiple: false,
        initial: false,
        row: Box::new(move |i, selected| row(symbol(rows[i].0), rows[i].1.clone(), selected)),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::PickerScroll(id, s)),
        on_select: Some(Box::new(move |s| Message::PickerSelect(id, s))),
        // Return reaches the window's shortcut, which a held key's repeats do not send
        double_click_activates: true,
        on_activate: None,
    })
    .key("sources");
    let header = app_header(p.ask.app_icon.as_deref(), app_icons, share_prompt(&p.ask));
    let remember = p.ask.offer_remember.then(|| {
        let app = p.ask.app_name.as_deref().unwrap_or("the app");
        Element::new(CheckBox {
            label: format!("Let “{app}” share it again without asking"),
            checked: if p.remember {
                Checked::True
            } else {
                Checked::False
            },
            enabled: p.armed,
            on_toggle: Some(Box::new(move |on: bool| Message::Remember(id, on))),
        })
        .key("remember")
    });
    let bar = bar_row(
        remember.into_iter().collect(),
        vec![
            deny_button(id, p.armed),
            button(
                "Share".to_owned(),
                ButtonRole::Default,
                p.armed && p.chosen().is_some(),
                Message::Share(id),
            )
            .key("share"),
        ],
    );
    Window {
        key: picker_window_key(id),
        spec: WindowSpec {
            title: "Share Screen".to_owned(),
            app_id: crate::PROMPT_APP_ID.into(),
            size: PICKER_SIZE,
            min_size: Some(PICKER_MIN_SIZE),
            max_size: None,
            decorations: PROMPT_DECORATIONS,
            foreign_parent: foreign_parent(&p.ask.parent_window, false),
            input_region: None,
        },
        // Return presses the focused button, Cancel when the window opens; with the focus in the list it is the default
        // button once a row is picked and the window is armed. Only a fresh press of Return shares, never a key held
        // since before. Escape cancels wherever the focus is
        shortcuts: CommandRegistry::new()
            .with_fresh(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: RETURN,
                },
                Message::Share(id),
            )
            .with(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: ESCAPE,
                },
                Message::Cancel(id),
            ),
        on_close: Some(Message::Cancel(id)),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content: prompt_content(vec![
            header,
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            list,
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            bar,
        ]),
    }
}

/// `target` as the app chooser's prompt names it: at most [`TARGET_CHARS`] characters, the rest left out.
fn shortened(target: &str) -> String {
    match target.char_indices().nth(TARGET_CHARS) {
        Some((end, _)) => format!("{}…", &target[..end]),
        None => target.to_owned(),
    }
}

fn app_window_key(id: u64) -> WindowKey {
    WindowKey(Key::Str(format!("apps-{id}").into()))
}

fn picker_window_key(id: u64) -> WindowKey {
    WindowKey(Key::Str(format!("picker-{id}").into()))
}

fn consent_window_key(id: u64) -> WindowKey {
    WindowKey(Key::Str(format!("consent-{id}").into()))
}

fn shortcuts_window_key(id: u64) -> WindowKey {
    WindowKey(Key::Str(format!("shortcuts-{id}").into()))
}

fn app_window(a: &AppPicker, app_icons: &AppIcons) -> Window<Message> {
    let id = a.id;
    let names: Vec<String> = a.ask.choices.iter().map(|c| c.name.clone()).collect();
    let shown = names.clone();
    let icons: Vec<Option<Vec<ImageSource>>> =
        a.ask.choices.iter().map(|c| app_icons.of(c)).collect();
    let list = Element::from(ListView {
        label: "Apps".into(),
        count: names.len(),
        row_height: ROW_HEIGHT,
        scroll: a.scroll,
        selection: a.rows.clone(),
        multiple: false,
        initial: true,
        row: Box::new(move |i, selected| {
            let icon = match &icons[i] {
                Some(sources) => Element::new(Image {
                    sources: sources.clone(),
                    size: Size::new(ROW_ICON, ROW_ICON),
                    fit: ImageFit::Contain,
                    radius: 0.0,
                    label: None,
                }),
                None => symbol("symbol.applications"),
            };
            row(icon, shown[i].clone(), selected)
        }),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::AppScroll(id, s)),
        on_select: Some(Box::new(move |s| Message::AppSelect(id, s))),
        double_click_activates: true,
        on_activate: Some(Box::new(move |_| Message::OpenWith(id))),
    })
    .key("apps");
    let target = a.ask.target.as_deref().map(shortened);
    let prompt = match (&target, a.ask.choices.is_empty()) {
        (Some(target), false) => format!("Open “{target}” with:"),
        (Some(target), true) => format!("There is no app to open “{target}” with."),
        (None, false) => "Choose an app to open it with.".to_owned(),
        (None, true) => "There is no app to open it with.".to_owned(),
    };
    let header = Element::new(Stack {
        axis: Axis::Vertical,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Stretch,
        children: vec![Element::new(Text {
            content: AttributedText::plain(prompt, TextStyle::new(13.0)),
            role: TextRole::Body,
            wrap: true,
            secondary: false,
            selected: false,
        })
        .key("prompt")],
    });
    let bar = bar_row(
        Vec::new(),
        vec![
            button(
                "Cancel".to_owned(),
                ButtonRole::Normal,
                true,
                Message::Cancel(id),
            )
            .key("cancel"),
            button(
                "Open".to_owned(),
                ButtonRole::Default,
                a.chosen().is_some(),
                Message::OpenWith(id),
            )
            .key("open"),
        ],
    );
    Window {
        key: app_window_key(id),
        spec: WindowSpec {
            title: "Open With".to_owned(),
            app_id: "org.cupertino.Portal".into(),
            size: PICKER_SIZE,
            min_size: Some(PICKER_MIN_SIZE),
            max_size: None,
            decorations: PROMPT_DECORATIONS,
            foreign_parent: foreign_parent(&a.ask.parent_window, false),
            input_region: None,
        },
        // Return opens the selected app and Escape cancels, wherever the focus is
        shortcuts: CommandRegistry::new()
            .with(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: RETURN,
                },
                Message::OpenWith(id),
            )
            .with(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: ESCAPE,
                },
                Message::Cancel(id),
            ),
        on_close: Some(Message::Cancel(id)),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content: prompt_content(vec![
            header,
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            list,
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            bar,
        ]),
    }
}

/// What the consent window asks: the app by its name, else an unnamed app.
fn consent_prompt(ask: &screenshot::Ask) -> String {
    match &ask.app_name {
        Some(name) => format!("“{name}” wants to take a screenshot of your screen."),
        None => "An application wants to take a screenshot of your screen.".to_owned(),
    }
}

fn consent_window(c: &Consent, app_icons: &AppIcons) -> Window<Message> {
    let id = c.id;
    let prompt = app_header(c.ask.app_icon.as_deref(), app_icons, consent_prompt(&c.ask));
    let bar = bar_row(
        Vec::new(),
        vec![
            deny_button(id, c.armed),
            button(
                "Take Screenshot".to_owned(),
                ButtonRole::Normal,
                c.armed,
                Message::TakeScreenshot(id),
            )
            .key("take"),
        ],
    );
    Window {
        key: consent_window_key(id),
        spec: WindowSpec {
            title: "Screenshot".to_owned(),
            app_id: crate::PROMPT_APP_ID.into(),
            size: CONSENT_SIZE,
            min_size: Some(CONSENT_SIZE),
            max_size: None,
            decorations: PROMPT_DECORATIONS,
            foreign_parent: foreign_parent(&c.ask.parent_window, false),
            input_region: None,
        },
        // Escape cancels wherever the focus is; Return has no shortcut and presses only the focused button, which is
        // Cancel when the window opens, so a key meant for the app never consents
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            Message::Cancel(id),
        ),
        on_close: Some(Message::Cancel(id)),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content: prompt_content(vec![
            Element::new(Spacer { min: 0.0 }),
            prompt,
            Element::new(Spacer { min: 0.0 }),
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            bar,
        ]),
    }
}

/// What the shortcut window asks: the app by its name, else an unnamed app.
fn shortcuts_prompt(ask: &global_shortcuts::Ask) -> String {
    let app = match &ask.app_name {
        Some(name) => format!("“{name}”"),
        None => "An application".to_owned(),
    };
    if ask.allowed > 0 {
        format!("{app} wants to use more keyboard shortcuts, also while you work in other apps.")
    } else {
        format!("{app} wants to use these keyboard shortcuts, also while you work in other apps.")
    }
}

/// `text` as a shortcut's row shows it: at most [`DESCRIPTION_CHARS`] characters, the rest left out.
fn description(text: &str) -> String {
    match text.char_indices().nth(DESCRIPTION_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// One row of the shortcut window: the keys, what the shortcut does, and why it gets no key when it does not.
fn shortcut_row(offered: &global_shortcuts::Offered, holders: &HashMap<String, String>) -> String {
    let keys = offered
        .trigger
        .map_or_else(|| "No key".to_owned(), |t| t.describe());
    let what = description(if offered.description.is_empty() {
        &offered.id
    } else {
        &offered.description
    });
    let why = match &offered.refusal {
        None => return format!("{keys}  {what}"),
        Some(Refusal::NoKey) => "it names no key".to_owned(),
        Some(Refusal::Modifier) => "its key is a modifier key".to_owned(),
        Some(Refusal::Typing) => "it needs ⌃, ⌥ or ⌘".to_owned(),
        Some(Refusal::System) => "the system uses it".to_owned(),
        Some(Refusal::Taken(app)) => {
            let name = holders.get(app).map_or(app.as_str(), String::as_str);
            format!("“{name}” uses it")
        }
        Some(Refusal::Twice) => "another shortcut here uses it".to_owned(),
    };
    format!("{keys}  {what} (not available: {why})")
}

fn shortcuts_window(c: &ShortcutConsent, app_icons: &AppIcons) -> Window<Message> {
    let id = c.id;
    let prompt = app_header(
        c.ask.app_icon.as_deref(),
        app_icons,
        shortcuts_prompt(&c.ask),
    );
    let texts: Vec<String> = c
        .ask
        .shortcuts
        .iter()
        .map(|offered| plain_row(&shortcut_row(offered, &c.ask.holders)))
        .collect();
    // the window grows by the lines of its first rows, a row that wraps by two
    let lines: usize = texts
        .iter()
        .take(SHORTCUT_ROWS)
        .map(|t| t.chars().count().div_ceil(SHORTCUT_LINE_CHARS).clamp(1, 2))
        .sum();
    let rows: Vec<Element<Message>> = texts
        .into_iter()
        .zip(&c.ask.shortcuts)
        .enumerate()
        .map(|(i, (text, offered))| {
            Element::new(Text {
                content: AttributedText::plain(text, TextStyle::new(13.0)),
                role: TextRole::Body,
                wrap: true,
                secondary: offered.refusal.is_some(),
                selected: false,
            })
            .key(Key::Str(format!("shortcut-{i}").into()))
        })
        .collect();
    // every shortcut can be read before Allow: past the rows the window grows by, the list scrolls
    let list = Element::from(ScrollView {
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: BAR_INSETS,
            spacing: 4.0,
            align: Align::Stretch,
            children: rows,
        }),
        state: c.scroll,
        horizontal: false,
        vertical: true,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(move |s| Message::ShortcutsScroll(id, s)),
    })
    .key("shortcuts")
    .fill();
    let bar = bar_row(
        Vec::new(),
        vec![
            labelled_deny_button("Don’t Allow", Message::DenyShortcuts(id), c.armed),
            button(
                "Allow".to_owned(),
                ButtonRole::Normal,
                c.armed,
                Message::AllowShortcuts(id),
            )
            .key("allow"),
        ],
    );
    let size = Size::new(
        CONSENT_SIZE.width,
        CONSENT_SIZE.height + SHORTCUT_ROW * lines as f64,
    );
    Window {
        key: shortcuts_window_key(id),
        spec: WindowSpec {
            title: "Keyboard Shortcuts".to_owned(),
            app_id: crate::PROMPT_APP_ID.into(),
            size,
            min_size: Some(size),
            max_size: None,
            decorations: PROMPT_DECORATIONS,
            foreign_parent: foreign_parent(&c.ask.parent_window, false),
            input_region: None,
        },
        // Escape and closing the window answer nothing that is remembered; Return presses only the focused button,
        // Don't Allow when the window opens
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            Message::Cancel(id),
        ),
        on_close: Some(Message::Cancel(id)),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content: prompt_content(vec![
            prompt,
            list,
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            bar,
        ]),
    }
}

/// A row's text without control or bidirectional-override characters.
fn plain_row(text: &str) -> String {
    crate::file_chooser::plain(text)
}
