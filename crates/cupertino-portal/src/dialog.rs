//! The file-chooser, app-chooser, screen-sharing and screenshot windows: one for each open request, drawn with CupertinoKit. The D-Bus side
//! hands its events to the [`inbox`]; [`Dialogs`] is resident and keeps running while no dialog is open.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use cupertino_filepicker::places::{session_places, Place};
use cupertino_filepicker::{Listing, Request};
use cupertino_kit::controls::{
    CheckBox, ListView, SegmentedControl, Selection, SplitLayout, SplitView, TextRole,
};
use cupertino_kit::core::{Checked, CommandRegistry, Key, Shortcut};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{DecorationMode, Keysym, Mods};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use zbus::zvariant::OwnedObjectPath;

use crate::app_chooser;
use crate::chooser::{Accept, Chooser};
use crate::file_chooser::{self, Answer, Kind, Reply};
use crate::screen_cast::{self, Source};
use crate::screenshot;

const ROW_HEIGHT: f64 = 24.0;
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
const PICKER_SIZE: Size = Size::new(480.0, 360.0);
const CONSENT_SIZE: Size = Size::new(420.0, 140.0);
const RETURN: Keysym = Keysym(0xff0d);
const ESCAPE: Keysym = Keysym(0xff1b);
/// Characters of a file name or URI the app chooser's prompt shows before it ends it with an ellipsis.
const TARGET_CHARS: usize = 80;

/// What the D-Bus side asks of the dialogs.
pub enum Event {
    FileChooser(file_chooser::Event),
    ScreenCast(screen_cast::Event),
    AppChooser(app_chooser::Event),
    Screenshot(screenshot::Event),
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
    consents: Vec<Consent>,
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
}

/// The question whether an app may take a screenshot.
struct Consent {
    id: u64,
    handle: OwnedObjectPath,
    reply: screenshot::Reply,
    ask: screenshot::Ask,
}

impl Picker {
    fn chosen(&self) -> Option<&Source> {
        self.rows.cursor().and_then(|i| self.ask.sources.get(i))
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
    TakeScreenshot(u64),
}

const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

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
                    // nothing is chosen until the person picks a row, so keys meant for the app cannot share
                    self.pickers.push(Picker {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        rows: Selection::default(),
                        scroll: NO_SCROLL,
                    });
                }
                Event::ScreenCast(screen_cast::Event::Close { handle }) => {
                    self.pickers.retain(|p| p.handle != handle)
                }
                Event::AppChooser(app_chooser::Event::Ask { handle, ask, reply }) => {
                    let id = self.next;
                    self.next += 1;
                    let mut picker = AppPicker {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                        rows: Selection::default(),
                        scroll: NO_SCROLL,
                    };
                    let last = picker.ask.last_choice.clone();
                    picker.select(last.as_deref());
                    self.apps.push(picker);
                }
                Event::AppChooser(app_chooser::Event::Update { handle, choices }) => {
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
                    self.consents.push(Consent {
                        id,
                        handle,
                        reply,
                        ask: *ask,
                    });
                }
                Event::Screenshot(screenshot::Event::Close { handle }) => {
                    self.consents.retain(|c| c.handle != handle)
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

    fn init() -> (Model, Vec<Command<Message>>) {
        let home = home();
        let mut model = Model {
            places: session_places(&home),
            home,
            next: 0,
            open: Vec::new(),
            pickers: Vec::new(),
            apps: Vec::new(),
            consents: Vec::new(),
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
            }
            Message::PickerScroll(id, scroll) => {
                if let Some(p) = model.picker(id) {
                    p.scroll = scroll;
                }
            }
            Message::PickerSelect(id, rows) => {
                if let Some(p) = model.picker(id) {
                    p.rows = rows;
                }
            }
            Message::Share(id) => {
                let chosen = model.picker(id).and_then(|p| p.chosen().cloned());
                if let Some(source) = chosen {
                    model.finish_picker(id, screen_cast::Answer::Chosen(source));
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
                let chosen = model.app_picker(id).and_then(|a| a.chosen().cloned());
                if let Some(choice) = chosen {
                    model.finish_app(id, app_chooser::Answer::Chosen(choice.id));
                }
            }
            Message::TakeScreenshot(id) => model.finish_consent(id, screenshot::Answer::Granted),
        }
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .open
            .iter()
            .map(|d| window(d, &model.places))
            .chain(model.pickers.iter().map(picker_window))
            .chain(model.apps.iter().map(app_window))
            .chain(model.consents.iter().map(consent_window))
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

fn row(symbol: &str, name: String, selected: bool) -> Element<Message> {
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
        children: vec![
            Element::new(Icon {
                name: symbol.to_owned(),
                size: 16.0,
                direction: Default::default(),
                label: None,
                tint: None,
            }),
            text(name, TextRole::Body, false, selected),
        ],
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
    Element::from(ListView {
        label: "Places".into(),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: d.places_scroll,
        selection,
        multiple: false,
        initial: false,
        row: Box::new(move |i, selected| row(rows[i].0, rows[i].1.clone(), selected)),
        key: Some(Box::new(move |i| Key::from(keys[i]))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::PlacesScroll(id, s)),
        on_select: Some(Box::new(move |s: Selection| {
            s.cursor()
                .map_or(Message::Ignore, |i| Message::Place(id, i))
        })),
        on_activate: Some(Box::new(move |i| Message::Place(id, i))),
    })
    .key("places")
}

fn files(d: &Dialog) -> Element<Message> {
    let id = d.id;
    let rows: Vec<(bool, String)> = d
        .chooser
        .entries()
        .iter()
        .map(|e| (e.is_dir, e.name.clone()))
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
            let (is_dir, name) = &rows[i];
            row(
                if *is_dir {
                    "symbol.folder"
                } else {
                    "symbol.doc"
                },
                name.clone(),
                selected,
            )
        }),
        key: Some(Box::new(move |i| Key::from(keys[i].clone()))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::Scroll(id, s)),
        on_select: Some(Box::new(move |s| Message::Select(id, s))),
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
    let path = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 6.0,
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
    Element::new(Stack {
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
    })
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

fn window(d: &Dialog, places: &[Place]) -> Window<Message> {
    let id = d.id;
    let ask = d.chooser.ask();
    let browser = Element::from(SplitView {
        label: "Sidebar".into(),
        axis: Axis::Horizontal,
        leading: sidebar(d, places),
        trailing: files(d),
        layout: d.split,
        min_leading: 120.0,
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
            decorations: DecorationMode::Preferred,
            foreign_parent: crate::wayland_parent(&ask.parent_window).map(str::to_owned),
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

fn picker_window(p: &Picker) -> Window<Message> {
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
        row: Box::new(move |i, selected| row(rows[i].0, rows[i].1.clone(), selected)),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::PickerScroll(id, s)),
        on_select: Some(Box::new(move |s| Message::PickerSelect(id, s))),
        on_activate: Some(Box::new(move |_| Message::Share(id))),
    })
    .key("sources");
    let prompt = if p.ask.sources.is_empty() {
        "There is no display or window to share.".to_owned()
    } else {
        "Choose a display or a window to share.".to_owned()
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
                "Share".to_owned(),
                ButtonRole::Default,
                p.chosen().is_some(),
                Message::Share(id),
            )
            .key("share"),
        ],
    );
    Window {
        key: WindowKey(Key::Str(format!("picker-{id}").into())),
        spec: WindowSpec {
            title: "Share Screen".to_owned(),
            app_id: "org.cupertino.Portal".into(),
            size: PICKER_SIZE,
            min_size: Some(Size::new(360.0, 240.0)),
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: crate::wayland_parent(&p.ask.parent_window).map(str::to_owned),
        },
        // Return is the default button once a row is picked, and Escape cancels, wherever the focus is
        shortcuts: CommandRegistry::new()
            .with(
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
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                header,
                Element::new(Separator {
                    axis: Axis::Horizontal,
                }),
                list,
                Element::new(Separator {
                    axis: Axis::Horizontal,
                }),
                bar,
            ],
        }),
    }
}

/// `target` as the app chooser's prompt names it: at most [`TARGET_CHARS`] characters, the rest left out.
fn shortened(target: &str) -> String {
    match target.char_indices().nth(TARGET_CHARS) {
        Some((end, _)) => format!("{}…", &target[..end]),
        None => target.to_owned(),
    }
}

fn app_window(a: &AppPicker) -> Window<Message> {
    let id = a.id;
    let names: Vec<String> = a.ask.choices.iter().map(|c| c.name.clone()).collect();
    let shown = names.clone();
    let list = Element::from(ListView {
        label: "Apps".into(),
        count: names.len(),
        row_height: ROW_HEIGHT,
        scroll: a.scroll,
        selection: a.rows.clone(),
        multiple: false,
        initial: true,
        row: Box::new(move |i, selected| row("symbol.applications", shown[i].clone(), selected)),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Message::AppScroll(id, s)),
        on_select: Some(Box::new(move |s| Message::AppSelect(id, s))),
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
        key: WindowKey(Key::Str(format!("apps-{id}").into())),
        spec: WindowSpec {
            title: "Open With".to_owned(),
            app_id: "org.cupertino.Portal".into(),
            size: PICKER_SIZE,
            min_size: Some(Size::new(360.0, 240.0)),
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: crate::wayland_parent(&a.ask.parent_window).map(str::to_owned),
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
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                header,
                Element::new(Separator {
                    axis: Axis::Horizontal,
                }),
                list,
                Element::new(Separator {
                    axis: Axis::Horizontal,
                }),
                bar,
            ],
        }),
    }
}

/// What the consent window asks: the app by its name, else an unnamed app.
fn consent_prompt(ask: &screenshot::Ask) -> String {
    match &ask.app_name {
        Some(name) => format!("“{name}” wants to take a screenshot of your screen."),
        None => "An application wants to take a screenshot of your screen.".to_owned(),
    }
}

fn consent_window(c: &Consent) -> Window<Message> {
    let id = c.id;
    let prompt = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Center,
        children: vec![Element::new(Text {
            content: AttributedText::plain(consent_prompt(&c.ask), TextStyle::new(13.0)),
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
                "Take Screenshot".to_owned(),
                ButtonRole::Normal,
                true,
                Message::TakeScreenshot(id),
            )
            .key("take"),
        ],
    );
    Window {
        key: WindowKey(Key::Str(format!("consent-{id}").into())),
        spec: WindowSpec {
            title: "Screenshot".to_owned(),
            app_id: "org.cupertino.Portal".into(),
            size: CONSENT_SIZE,
            min_size: Some(CONSENT_SIZE),
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: crate::wayland_parent(&c.ask.parent_window).map(str::to_owned),
        },
        // Escape cancels wherever the focus is; Return has no shortcut, so a key meant for the app never consents
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            Message::Cancel(id),
        ),
        on_close: Some(Message::Cancel(id)),
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                Element::new(Spacer { min: 0.0 }),
                prompt,
                Element::new(Spacer { min: 0.0 }),
                Element::new(Separator {
                    axis: Axis::Horizontal,
                }),
                bar,
            ],
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use cupertino_assets::BUILTIN_FONT;
    use cupertino_kit::core::{PlaceholderIcons, RuntimeOptions};
    use cupertino_kit::geometry::{Point, Rect};
    use cupertino_kit::layout::LayoutDirection;
    use cupertino_kit::platform::{
        Capabilities, KeyEvent, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowId,
        WindowStates,
    };
    use cupertino_kit::render::{CpuConfig, CpuRenderer};
    use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
    use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
    use cupertino_theme::Theme;
    use cupertino_ui_test::{ManualClock, TestDriver};

    use super::*;
    use crate::file_chooser::{Ask, Event};

    fn ask(kind: Kind, folder: &std::path::Path) -> Ask {
        Ask {
            kind,
            app_id: "org.example.Editor".to_owned(),
            parent_window: String::new(),
            title: "Export".to_owned(),
            accept_label: None,
            modal: true,
            multiple: false,
            directory: false,
            filters: Vec::new(),
            current_filter: None,
            choices: Vec::new(),
            current_name: Some("new.txt".to_owned()),
            current_folder: Some(folder.to_path_buf()),
            current_file: None,
            files: Vec::new(),
        }
    }

    fn handle(n: u32) -> OwnedObjectPath {
        OwnedObjectPath::try_from(format!("/org/freedesktop/portal/desktop/request/1_7/t{n}"))
            .unwrap()
    }

    fn post(n: u32, ask: Ask) -> async_channel::Receiver<Answer> {
        let (sender, answer) = async_channel::bounded(1);
        inbox().push(Event::Ask {
            handle: handle(n),
            ask: Box::new(ask),
            reply: Reply(sender),
        });
        answer
    }

    fn budget() -> NonZeroU32 {
        NonZeroU32::new(64).unwrap()
    }

    fn start() -> (TestDriver<Dialogs>, WindowId) {
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        };
        let theme = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 4000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        let window = show(&mut driver);
        (driver, window)
    }

    /// Configures the window of the request that waits.
    fn show(driver: &mut TestDriver<Dialogs>) -> WindowId {
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        settle(driver, window);
        window
    }

    fn settle(driver: &mut TestDriver<Dialogs>, window: WindowId) {
        for _ in 0..8 {
            driver.run_until_idle(budget()).unwrap();
            let system = driver.window_system_mut();
            if !system.windows().contains(&window) || !system.frame_requested(window).unwrap() {
                return;
            }
            system.frame_ready(window, Timestamp(0)).unwrap();
        }
    }

    fn send(driver: &mut TestDriver<Dialogs>, window: WindowId, event: WindowEvent) {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        settle(driver, window);
    }

    fn type_key(
        driver: &mut TestDriver<Dialogs>,
        window: WindowId,
        keysym: Keysym,
        text: Option<char>,
    ) {
        let event = KeyEvent {
            code: 0,
            keysym,
            text,
            pressed: true,
            repeat: false,
            mods: Mods::NONE,
        };
        send(driver, window, WindowEvent::Key(event, Timestamp(0)));
    }

    fn click(driver: &mut TestDriver<Dialogs>, window: WindowId, key: &'static str) {
        click_at(driver, window, Key::from(key));
    }

    fn rect_of(driver: &TestDriver<Dialogs>, window: WindowId, key: &Key) -> Rect {
        let runtime = driver.runtime();
        let id = runtime.find(window, key).unwrap();
        runtime
            .snapshot(window)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect
    }

    fn click_at(driver: &mut TestDriver<Dialogs>, window: WindowId, key: Key) {
        let rect = rect_of(driver, window, &key);
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        send(driver, window, pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            send(
                driver,
                window,
                pointer(PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: Mods::NONE,
                }),
            );
        }
    }

    // one test: the inbox is the process's
    #[test]
    fn a_request_opens_a_window_that_answers_it_and_the_portal_waits_with_none_open() {
        let folder = tempfile::tempdir().unwrap();

        let answer = post(1, ask(Kind::Save, folder.path()));
        let (mut driver, window) = start();
        let spec = driver.window_system_mut().spec(window).unwrap().clone();
        assert_eq!(spec.title, "Export");
        let runtime = driver.runtime();
        assert_eq!(
            runtime.snapshot(window).unwrap().focus,
            Some(runtime.find(window, &Key::from("name")).unwrap()),
            "Save As opens with its name field focused"
        );
        click(&mut driver, window, "accept");
        assert_eq!(
            answer.try_recv().unwrap(),
            Answer::Chosen {
                paths: vec![folder.path().join("new.txt")],
                filter: None,
                choices: Vec::new(),
            }
        );
        assert!(driver.runtime().windows().is_empty());
        assert!(!driver.runtime().exited());

        let answer = post(2, ask(Kind::Open, folder.path()));
        let window = show(&mut driver);
        send(&mut driver, window, WindowEvent::CloseRequested);
        assert_eq!(answer.try_recv().unwrap(), Answer::Cancelled);

        let answer = post(3, ask(Kind::Open, folder.path()));
        let window = show(&mut driver);
        inbox().push(Event::Close { handle: handle(3) });
        settle(&mut driver, window);
        assert!(answer.is_closed(), "a closed request is dropped unanswered");
        assert!(driver.runtime().windows().is_empty());
        assert!(!driver.runtime().exited());

        let display = Source::Display {
            output: 4,
            name: "DP-1".to_owned(),
            description: "Studio Display".to_owned(),
        };
        let share = |n: u32| {
            let (sender, answer) = async_channel::bounded(1);
            inbox().push(screen_cast::Event::Ask {
                handle: handle(n),
                ask: Box::new(screen_cast::Ask {
                    app_id: "org.example.Meeting".to_owned(),
                    parent_window: String::new(),
                    sources: vec![
                        display.clone(),
                        Source::Window {
                            identifier: "a1".to_owned(),
                            title: "Notes".to_owned(),
                            app_id: "org.example.Notes".to_owned(),
                        },
                    ],
                }),
                reply: screen_cast::Reply(sender),
            });
            answer
        };
        let answer = share(4);
        let window = show(&mut driver);
        let spec = driver.window_system_mut().spec(window).unwrap().clone();
        assert_eq!(spec.title, "Share Screen");
        type_key(&mut driver, window, Keysym('n' as u32), Some('n'));
        type_key(&mut driver, window, RETURN, None);
        click(&mut driver, window, "share");
        assert!(
            answer.try_recv().is_err(),
            "keys typed for the app and Share with no row picked share nothing"
        );
        click_at(&mut driver, window, Key::Int(0));
        type_key(&mut driver, window, RETURN, None);
        assert_eq!(
            answer.try_recv().unwrap(),
            screen_cast::Answer::Chosen(display.clone())
        );
        assert!(driver.runtime().windows().is_empty());

        let answer = share(5);
        let window = show(&mut driver);
        click(&mut driver, window, "cancel");
        assert_eq!(answer.try_recv().unwrap(), screen_cast::Answer::Cancelled);
        assert!(driver.runtime().windows().is_empty());
        assert!(!driver.runtime().exited());

        let choice = |id: &str, name: &str| app_chooser::Choice {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        let open_with = |n: u32| {
            let (sender, answer) = async_channel::bounded(1);
            inbox().push(app_chooser::Event::Ask {
                handle: handle(n),
                ask: Box::new(app_chooser::Ask {
                    app_id: "org.example.Mail".to_owned(),
                    parent_window: String::new(),
                    target: Some("report.pdf".to_owned()),
                    choices: vec![
                        choice("org.example.Viewer", "Document Viewer"),
                        choice("org.example.Draw", "Drawing"),
                    ],
                    last_choice: Some("org.example.Draw".to_owned()),
                }),
                reply: app_chooser::Reply(sender),
            });
            answer
        };
        let answer = open_with(6);
        let window = show(&mut driver);
        let spec = driver.window_system_mut().spec(window).unwrap().clone();
        assert_eq!(spec.title, "Open With");
        let snapshot = driver.runtime().snapshot(window).unwrap();
        let names: Vec<&str> = snapshot
            .accessibility
            .iter()
            .filter_map(|(_, n)| n.name.as_deref())
            .collect();
        for shown in ["Open “report.pdf” with:", "Document Viewer", "Drawing"] {
            assert!(names.contains(&shown), "{shown:?} in {names:?}");
        }
        type_key(&mut driver, window, Keysym(0xff52), None);
        type_key(&mut driver, window, RETURN, None);
        assert_eq!(
            answer.try_recv().unwrap(),
            app_chooser::Answer::Chosen("org.example.Viewer".to_owned()),
            "the last choice opens selected with the focus in the list, so Up moves to the app above it"
        );
        assert!(driver.runtime().windows().is_empty());

        let answer = open_with(7);
        let window = show(&mut driver);
        inbox().push(app_chooser::Event::Update {
            handle: handle(7),
            choices: vec![choice("org.example.Viewer", "Document Viewer")],
        });
        settle(&mut driver, window);
        type_key(&mut driver, window, RETURN, None);
        assert!(
            answer.try_recv().is_err(),
            "a selected choice that is no longer offered opens nothing"
        );
        type_key(&mut driver, window, ESCAPE, None);
        assert_eq!(answer.try_recv().unwrap(), app_chooser::Answer::Cancelled);
        assert!(driver.runtime().windows().is_empty());

        let consent = |n: u32, app_name: Option<&str>| {
            let (sender, answer) = async_channel::bounded(1);
            inbox().push(screenshot::Event::Ask {
                handle: handle(n),
                ask: Box::new(screenshot::Ask {
                    app_id: "org.example.Draw".to_owned(),
                    app_name: app_name.map(str::to_owned),
                    parent_window: String::new(),
                }),
                reply: screenshot::Reply(sender),
            });
            answer
        };
        let shown = |driver: &TestDriver<Dialogs>, window: WindowId| -> Vec<String> {
            let snapshot = driver.runtime().snapshot(window).unwrap();
            snapshot
                .accessibility
                .iter()
                .filter_map(|(_, n)| n.name.clone())
                .collect()
        };
        let answer = consent(6, Some("Drawing"));
        let window = show(&mut driver);
        let spec = driver.window_system_mut().spec(window).unwrap().clone();
        assert_eq!(spec.title, "Screenshot");
        let names = shown(&driver, window);
        for text in [
            "“Drawing” wants to take a screenshot of your screen.",
            "Cancel",
            "Take Screenshot",
        ] {
            assert!(names.iter().any(|n| n == text), "{text:?} in {names:?}");
        }
        let prompt = rect_of(&driver, window, &Key::from("prompt"));
        let take = rect_of(&driver, window, &Key::from("take"));
        let separator = take.origin.y - BAR_INSETS.top - 1.0;
        assert!(
            (prompt.origin.y - (separator - prompt.origin.y - prompt.size.height)).abs() < 0.5,
            "the prompt is centred in the body above the separator: {prompt:?}, separator at {separator}"
        );
        type_key(&mut driver, window, RETURN, None);
        type_key(&mut driver, window, Keysym(' ' as u32), Some(' '));
        assert!(
            answer.try_recv().is_err(),
            "nothing is preselected, so no key consents"
        );
        click(&mut driver, window, "take");
        assert_eq!(answer.try_recv().unwrap(), screenshot::Answer::Granted);
        assert!(driver.runtime().windows().is_empty());

        let answer = consent(7, None);
        let window = show(&mut driver);
        let names = shown(&driver, window);
        let unnamed = "An application wants to take a screenshot of your screen.";
        assert!(names.iter().any(|n| n == unnamed), "{names:?}");
        type_key(&mut driver, window, ESCAPE, None);
        assert_eq!(answer.try_recv().unwrap(), screenshot::Answer::Cancelled);
        assert!(driver.runtime().windows().is_empty());
        assert!(!driver.runtime().exited());
    }
}
