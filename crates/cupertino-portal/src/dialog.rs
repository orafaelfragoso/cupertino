//! The file-chooser windows: one for each open request, drawn with CupertinoKit. The D-Bus side hands its events to
//! the [`inbox`]; a run of [`Dialogs`] lasts while any dialog is open, so the process starts one when an event waits.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use cupertino_filepicker::places::{session_places, Place};
use cupertino_filepicker::{Listing, Request};
use cupertino_kit::controls::{
    CheckBox, ListView, SegmentedControl, Selection, SplitLayout, SplitView, TextRole,
};
use cupertino_kit::core::{Checked, CommandRegistry};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use zbus::zvariant::OwnedObjectPath;

use crate::chooser::{Accept, Chooser};
use crate::file_chooser::{Answer, Event, Kind, Reply};

const ROW_HEIGHT: f64 = 24.0;
const SIDEBAR_WIDTH: f64 = 180.0;
const GAP: f64 = 8.0;
/// Around the bottom bar's controls, which makes it 52 pt tall with a regular control; an approximate design value.
const BAR_INSETS: Insets = Insets {
    top: 12.0,
    left: 16.0,
    bottom: 12.0,
    right: 16.0,
};
const SIZE: Size = Size::new(720.0, 460.0);

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
    pub fn push(&self, event: Event) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
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

    /// Block until an event waits. Events stay in the inbox until a run takes them, so none is lost when a run ends.
    pub fn wait(&self) {
        while self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            let _ = self.wake.1.recv_blocking();
        }
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

    fn take(&mut self, events: Vec<Event>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::Ask { handle, ask, reply } => {
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
                Event::Close { handle } => self.open.retain(|d| d.handle != handle),
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

    fn init() -> (Model, Vec<Command<Message>>) {
        let home = home();
        let mut model = Model {
            places: session_places(&home),
            home,
            next: 0,
            open: Vec::new(),
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
            Message::Cancel(id) => model.finish(id, Answer::Cancelled),
        }
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .open
            .iter()
            .map(|d| window(d, &model.places))
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
    Element::from(ListView {
        label: "Places".into(),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: d.places_scroll,
        selection,
        multiple: false,
        row: Box::new(move |i, selected| row(rows[i].0, rows[i].1.clone(), selected)),
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
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Center,
                children: vec![Element::new(TextField {
                    label: "Save As".into(),
                    placeholder: String::new(),
                    value: d.chooser.name().to_owned(),
                    revision: d.name_revision,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(move |c| Message::Name(id, c))),
                    on_submit: Some(Message::Accept(id)),
                })
                .key("name")],
            }),
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
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(Message::Cancel(id)),
        background: WindowBackground::Theme,
        popup: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children,
        }),
    }
}
