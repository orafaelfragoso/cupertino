//! The preview window on CupertinoKit: Space, or File › Quick Preview, shows the selected items one at a time in a
//! floating panel of their window, without opening them.
//!
//! The panel has its own toolbar: a close button and a full screen button at its leading end, the item's name, and an
//! "Open with" button naming the application that opens the item. Under it a picture shows whole, the start of a text
//! file in a fixed-pitch face, and anything else its large icon with its kind, size and dates
//! ([`cupertino_kit::controls::FilePreview`]). With several items selected the arrows go from one to the next; with
//! one, they move the window's selection, which the preview follows. Space or Escape closes it.
//!
//! The panel is a popup of its browser window, so the window keeps the keyboard and the desktop's menu bar keeps the
//! window's menus while it is up. It lies centred over the folder, never above the window's top edge, on the window's
//! display; it grows out of the item it shows as it opens and shrinks back into it as it closes, wherever that item
//! is by then (out of the middle of the folder when the item is scrolled out of view). With Reduce Motion it shows and
//! goes at once. It opens once what it shows is ready; a picture or a text that takes longer than
//! [`file_preview::LOADING_DELAY`] opens it with the item's icon over a loading bar, and so does an item stepped to,
//! which never shows the one before it. In full screen it is a window of its own that shows the browser window's menus.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

use cupertino_apps::MimeApps;
use cupertino_kit::controls::{
    FilePreview, PreviewContent, PreviewStyle, Progress, Text, TextRole,
};
use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Shortcut, Widget, WindowBackground,
};
use cupertino_kit::core::{Popup, PopupAnchor};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, Overflow,
    ScrollState, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_kit::prelude::{Button, ButtonRole, ButtonStyle, Spacer, Stack, Window, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle};
use cupertino_kit::theme::{ColorRole, StateVariant, Surface};

use super::toolbar::focus_ring;
use super::{BrowserWindow, Event as WindowEvent, Message, Session, Wake, NO_SCROLL};
use crate::preview::{self as file_preview, Browse, PreviewKind};
use crate::window::ViewMode;
use crate::Entry;

/// The height of the window's toolbar.
const TOOLBAR_HEIGHT: f64 = 36.0;
/// From the window's leading edge to its first button, and between the toolbar's items.
const TOOLBAR_INSET: f64 = 12.0;
const TOOLBAR_GAP: f64 = 8.0;
/// The side of a toolbar button drawn here, and of the glyph in it.
const BUTTON: f64 = 22.0;
const GLYPH: f64 = 14.0;
/// The window's corner radius.
const RADIUS: f64 = 12.0;
/// The side of the pictures previewed, here and in the preview column, which draws them smaller.
pub(super) const PICTURE: f64 = 512.0;
/// The side of an icon in the window.
const ICON: f64 = 256.0;
/// The size of the window's title.
const TITLE_SIZE: f32 = 13.0;
/// The room around the loading bar of a preview whose item is still being made ready.
const LOADING_INSET: f64 = 24.0;

const SPACE: Keysym = Keysym(0x20);
const ESCAPE: Keysym = Keysym(0xff1b);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const RETURN: Keysym = Keysym(0xff0d);

/// The start of the text files a window previews, by path, with the time each was changed when it was read. A file is
/// read on a thread of its own, which wakes the window when it is done; a result for a file no longer previewed, or
/// changed since, is dropped.
pub(super) struct Texts {
    read: HashMap<PathBuf, (Option<SystemTime>, Option<String>)>,
    /// The files being read, with the time each was changed when its read started.
    reading: HashMap<PathBuf, Option<SystemTime>>,
    done: (Sender<TextRead>, Receiver<TextRead>),
}

/// A file read: its path, the time it was changed when the read started, and its text (`None` when it is not text).
type TextRead = (PathBuf, Option<SystemTime>, Option<String>);

impl Default for Texts {
    fn default() -> Self {
        Texts {
            read: HashMap::new(),
            reading: HashMap::new(),
            done: mpsc::channel(),
        }
    }
}

impl Texts {
    /// The start of the text of the file at `path`, once read.
    pub fn get(&self, path: &Path) -> Option<String> {
        self.read.get(path).and_then(|(_, text)| text.clone())
    }

    /// Keep the texts of `entries` (the text files previewed now), reading those not read yet or changed since, away
    /// from the window's thread, and forget the others; `wake` is called as each read ends.
    pub fn follow<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>, wake: &Wake) {
        let wanted: Vec<&Entry> = entries
            .into_iter()
            .filter(|e| file_preview::kind(e) == PreviewKind::Text)
            .collect();
        self.read
            .retain(|path, _| wanted.iter().any(|e| &e.path == path));
        self.reading
            .retain(|path, _| wanted.iter().any(|e| &e.path == path));
        for entry in wanted {
            let fresh = self
                .read
                .get(&entry.path)
                .is_some_and(|(modified, _)| *modified == entry.modified);
            if fresh || self.reading.get(&entry.path) == Some(&entry.modified) {
                continue;
            }
            self.reading.insert(entry.path.clone(), entry.modified);
            let (path, modified) = (entry.path.clone(), entry.modified);
            let (done, wake) = (self.done.0.clone(), Arc::clone(wake));
            std::thread::spawn(move || {
                let text = match file_preview::read_text(&path) {
                    Ok(text) => text.map(|t| t.text),
                    Err(e) => {
                        eprintln!(
                            "cupertino-files: cannot read {} to preview it: {e}",
                            path.display()
                        );
                        None
                    }
                };
                // the window may be gone, and nobody is left to tell
                if done.send((path, modified, text)).is_ok() {
                    wake();
                }
            });
        }
    }

    /// Whether the file at `path` was read: its text, or that it is not text, is known.
    pub fn read(&self, path: &Path) -> bool {
        self.read.contains_key(path)
    }

    /// Take in the reads that ended: whether a text previewed now arrived.
    pub fn poll(&mut self) -> bool {
        let mut arrived = false;
        while let Ok((path, modified, text)) = self.done.1.try_recv() {
            // a read for a file no longer previewed, or since changed again, is stale
            if self.reading.get(&path) != Some(&modified) {
                continue;
            }
            self.reading.remove(&path);
            self.read.insert(path, (modified, text));
            arrived = true;
        }
        arrived
    }
}

/// The preview window of a browser window.
pub(super) struct Peek {
    pub browse: Browse,
    pub scroll: ScrollState,
    pub full_screen: bool,
    /// The name of the application that opens the item shown, with the item it was read for.
    app: Option<(PathBuf, Option<String>)>,
    /// The session's associations, read once while the window is open: stepping through items reads none again.
    apps: Option<Option<MimeApps>>,
    /// The window has shown something: it stays up, in its loading state, while the next item it shows is made ready.
    shown: bool,
    /// The item the window waits for, with the number of that wait; and whether it has waited past
    /// [`file_preview::LOADING_DELAY`].
    waiting: Option<(PathBuf, u64)>,
    waits: u64,
    slow: bool,
    /// The size the window last showed an item at, which it keeps while it waits for the next.
    size: (f64, f64),
}

/// What the preview window asks of its browser window.
#[derive(Clone, Debug, PartialEq)]
pub enum PeekEvent {
    /// The wait with this number ran past [`file_preview::LOADING_DELAY`].
    Waited(u64),
    /// The previous item (-1) or the next (1).
    Step(isize),
    /// Close the window: it shrinks back into its item.
    Close,
    /// The window closed at once, as the compositor's dismissal does.
    Closed,
    FullScreen,
    /// Open the item shown with its application.
    Open,
    Scroll(ScrollState),
}

impl Peek {
    pub fn new(browse: Browse) -> Peek {
        Peek {
            browse,
            scroll: NO_SCROLL,
            full_screen: false,
            app: None,
            apps: None,
            shown: false,
            waiting: None,
            waits: 0,
            slow: false,
            size: file_preview::WINDOW_SIZE,
        }
    }

    /// The item shown is ready to show at `size` (`Some`), or is still being made ready: the number of a wait that
    /// starts for it, which [`PeekEvent::Waited`] ends.
    pub fn follow_ready(&mut self, size: Option<(f64, f64)>) -> Option<u64> {
        if let Some(size) = size {
            self.shown = true;
            self.size = size;
            self.slow = false;
            self.waiting = None;
            return None;
        }
        let current = self.browse.current();
        if self
            .waiting
            .as_ref()
            .is_some_and(|(path, _)| path == current)
        {
            return None;
        }
        self.waits += 1;
        self.slow = false;
        self.waiting = Some((current.to_path_buf(), self.waits));
        Some(self.waits)
    }

    /// The wait numbered `wait` ran long: the window shows that it is loading, if it still waits for that item.
    pub fn waited(&mut self, wait: u64) {
        if self.waiting.as_ref().is_some_and(|(_, n)| *n == wait) {
            self.slow = true;
        }
    }

    /// The item shown changed to `path`: its application's name is read again, from `session`'s associations.
    pub fn follow_app(&mut self, session: &Session, path: &Path) {
        if self.app.as_ref().is_some_and(|(p, _)| p == path) {
            return;
        }
        let apps = self
            .apps
            .get_or_insert_with(|| (session.applications)().ok());
        // an application opens itself: its button reads Open
        let name = apps.as_ref().and_then(|apps| {
            if crate::open::application_at(apps, path).is_some() {
                return None;
            }
            crate::open::open_with(apps, path, path.is_dir())
                .into_iter()
                .find(|item| item.is_default)
                .map(|item| item.entry.name.clone())
        });
        self.app = Some((path.to_path_buf(), name));
        self.scroll = NO_SCROLL;
    }
}

/// The key of the preview window of browser window `id`, a popup of it.
pub(super) fn window_key(id: u64) -> cupertino_kit::core::WindowKey {
    cupertino_kit::core::WindowKey(Key::from(format!("preview/{id}")))
}

/// The key of the preview window of browser window `id` in full screen, a window of its own.
pub(super) fn full_screen_key(id: u64) -> cupertino_kit::core::WindowKey {
    cupertino_kit::core::WindowKey(Key::from(format!("preview-full-screen/{id}")))
}

fn send(id: u64, event: PeekEvent) -> Message {
    Message::Window(id, WindowEvent::Peek(event))
}

/// What the preview shows of `entry` once it is ready, and the size of the window then: `None` while its picture is
/// made or its text read.
pub(super) fn ready(
    session: &Session,
    texts: &Texts,
    entry: &Entry,
) -> Option<(PreviewContent, (f64, f64))> {
    let icon = || PreviewContent::Icon(session.icons.icon(entry, ICON));
    match file_preview::kind(entry) {
        PreviewKind::Picture => {
            let Some(picture) = session.icons.preview_picture(entry, PICTURE)? else {
                return Some((icon(), file_preview::WINDOW_SIZE));
            };
            let size = file_preview::picture_window(
                (picture.image.width, picture.image.height),
                2.0,
                TOOLBAR_HEIGHT,
            );
            Some((PreviewContent::Picture(picture), size))
        }
        PreviewKind::Text if !texts.read(&entry.path) => None,
        PreviewKind::Text => {
            let content = texts
                .get(&entry.path)
                .map_or_else(icon, PreviewContent::Text);
            Some((content, file_preview::WINDOW_SIZE))
        }
        PreviewKind::Icon => Some((icon(), file_preview::WINDOW_SIZE)),
    }
}

/// The key of the node of browser window `id` that shows `entry`, which the preview grows out of.
fn item_key(id: u64, win: &BrowserWindow, entry: &Entry) -> Key {
    match win.view {
        ViewMode::Icons => {
            let index = win
                .browser
                .entries()
                .iter()
                .position(|e| e.path == entry.path);
            super::view::item_key(id, index.unwrap_or(usize::MAX))
        }
        _ => super::view::row_key(id, &entry.path),
    }
}

/// The preview window of browser window `id`, when it has one, its item is listed and it has something to show: what
/// the item shows once it is ready, or, once the wait for it ran long or the window has shown, its icon.
pub(super) fn window(session: &Session, id: u64, win: &BrowserWindow) -> Option<Window<Message>> {
    let look = win.peek.as_ref()?;
    let entry = win
        .browser
        .entries()
        .iter()
        .find(|e| e.path == look.browse.current())?;
    let ready = ready(session, &win.texts, entry);
    if ready.is_none() && !look.shown && !look.slow {
        return None;
    }
    let loading = ready.is_none() && look.slow;
    let (content, size) = ready.unwrap_or_else(|| {
        (
            PreviewContent::Icon(session.icons.icon(entry, ICON)),
            look.size,
        )
    });
    let preview = Element::from(FilePreview {
        content,
        text_family: session.text_family.clone(),
        name: entry.name.clone(),
        summary: file_preview::summary(entry),
        details: file_preview::details(entry, win.now)
            .into_iter()
            .map(|(label, value)| (label.to_owned(), value))
            .collect(),
        footer: None,
        style: PreviewStyle::Window,
        scroll: look.scroll,
        on_scroll: Box::new(move |s| send(id, PeekEvent::Scroll(s))),
    })
    .key("preview")
    .fill();
    let app = look.app.as_ref().and_then(|(_, name)| name.clone());
    let mut children = vec![toolbar(id, look, entry, app), preview];
    if loading {
        children.push(loading_bar());
    }
    let body = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
    .fill();
    let content = Element::new(Sheet {
        size,
        children: vec![body],
    })
    .key("sheet");
    let spec = WindowSpec {
        title: entry.name.clone(),
        app_id: super::APP_ID.into(),
        size: Size::new(size.0, size.1),
        min_size: Some(Size::new(320.0, 240.0)),
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    };
    if look.full_screen {
        // a window of its own, which the menu bar follows: it shows the browser window's menus
        return Some(Window {
            key: full_screen_key(id),
            spec,
            shortcuts: shortcuts(id),
            on_close: Some(send(id, PeekEvent::Closed)),
            background: WindowBackground::None,
            popup: None,
            menus: Some(super::view::menus(session, id, win)),
            content,
        });
    }
    Some(Window {
        key: window_key(id),
        spec,
        // the browser window keeps the keyboard, and takes the preview's keys
        shortcuts: CommandRegistry::new(),
        on_close: Some(send(id, PeekEvent::Closed)),
        background: WindowBackground::None,
        popup: Some(Popup {
            parent: super::window_key(id),
            anchor: PopupAnchor::Panel {
                over: Key::from("folder"),
                from: item_key(id, win, entry),
            },
            grab: false,
            on_dismiss: send(id, PeekEvent::Closed),
        }),
        menus: None,
        content,
    })
}

/// The bar under a preview whose item is still being made ready.
fn loading_bar() -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: LOADING_INSET,
            bottom: LOADING_INSET,
            right: LOADING_INSET,
        },
        spacing: 0.0,
        align: Align::Center,
        children: vec![Element::new(Progress {
            label: "Loading".into(),
            value: None,
        })
        .key("loading")],
    })
}

/// Space and Escape close the window; the arrows go from one item to the next.
fn shortcuts(id: u64) -> CommandRegistry<Message> {
    let mut registry = CommandRegistry::new();
    let key = |keysym| Shortcut {
        mods: Mods::NONE,
        keysym,
    };
    registry.insert(key(SPACE), send(id, PeekEvent::Close));
    registry.insert(key(ESCAPE), send(id, PeekEvent::Close));
    for (keysym, by) in [(LEFT, -1), (UP, -1), (RIGHT, 1), (DOWN, 1)] {
        registry.insert(key(keysym), send(id, PeekEvent::Step(by)));
    }
    registry
}

/// The window's toolbar: close and full screen, the item's name, and the button that opens it.
fn toolbar(id: u64, look: &Peek, entry: &Entry, app: Option<String>) -> Element<Message> {
    let mut title = vec![Element::new(Text {
        content: AttributedText::plain(
            entry.name.clone(),
            TextStyle {
                weight: FontWeight::BOLD,
                ..TextStyle::new(TITLE_SIZE)
            },
        ),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: false,
    })];
    if look.browse.len() > 1 {
        title.push(Element::new(Text {
            content: AttributedText::plain(
                format!("{} of {}", look.browse.index() + 1, look.browse.len()),
                TextStyle::new(TITLE_SIZE),
            ),
            role: TextRole::Body,
            wrap: false,
            secondary: true,
            selected: false,
        }));
    }
    let open = Element::new(Button {
        label: match app {
            Some(app) => format!("Open with {app}"),
            None => "Open".into(),
        },
        role: ButtonRole::Normal,
        style: ButtonStyle::Pill,
        enabled: true,
        busy: false,
        on_press: Some(send(id, PeekEvent::Open)),
    })
    .key("open");
    let row = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: TOOLBAR_INSET,
            bottom: 0.0,
            right: TOOLBAR_INSET,
        },
        spacing: TOOLBAR_GAP,
        align: Align::Center,
        children: vec![
            Element::new(GlyphButton {
                glyph: Glyph::Close,
                label: "Close".into(),
                size: BUTTON,
                on_press: send(id, PeekEvent::Close),
            })
            .key("close"),
            Element::new(GlyphButton {
                glyph: Glyph::FullScreen,
                size: BUTTON,
                label: if look.full_screen {
                    "Exit Full Screen".into()
                } else {
                    "Full Screen".into()
                },
                on_press: send(id, PeekEvent::FullScreen),
            })
            .key("full-screen"),
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: TOOLBAR_GAP,
                align: Align::Center,
                children: title,
            }),
            Element::new(Spacer { min: 0.0 }),
            open,
        ],
    })
    .fill();
    Element::new(Bar {
        children: vec![row],
    })
}

/// The toolbar's row, its height fixed.
struct Bar {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Bar {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TOOLBAR_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let line = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL);
        if let Some(line) = line {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, size.height - 1.0, size.width, 1.0)),
                paint: Paint::Solid(line),
            });
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Toolbar);
    }
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// What a [`GlyphButton`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glyph {
    /// A cross in a filled circle.
    Close,
    /// Two arrows pointing out to opposite corners.
    FullScreen,
    /// A plus in a filled circle the size of the button.
    Plus,
}

/// A round button whose glyph is drawn here, `size` points square.
pub(super) struct GlyphButton {
    pub glyph: Glyph,
    pub label: String,
    pub size: f64,
    pub on_press: Message,
}

impl Widget<Message> for GlyphButton {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size),
            height: SizeSpec::Fixed(self.size),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.glyph, &self.label, self.size) != (old.glyph, &old.label, old.size) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.size, self.size),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed && key.mods.is_empty() && matches!(key.keysym, RETURN) =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let colors = &cx.env().theme.colors;
        // the window's own buttons are solid discs with their glyph cut out; the + of a bar is a faint well
        let (ink, well) = match self.glyph {
            Glyph::Close | Glyph::FullScreen => (
                colors.get(ColorRole::WindowBackground, StateVariant::NORMAL),
                colors.get(ColorRole::SecondaryText, StateVariant::NORMAL),
            ),
            Glyph::Plus => (
                colors.get(ColorRole::SecondaryText, StateVariant::NORMAL),
                colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL),
            ),
        };
        let (cx0, cy0) = (size.width / 2.0, size.height / 2.0);
        let half = GLYPH / 2.0;
        match self.glyph {
            Glyph::Close => {
                if let Some(well) = well {
                    cx.push(PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: Rect::new(cx0 - half, cy0 - half, GLYPH, GLYPH),
                            radius: half,
                        },
                        paint: Paint::Solid(well),
                    });
                }
                if let Some(ink) = ink {
                    let arm = GLYPH * 0.2;
                    cx.push(PaintOp::Stroke {
                        shape: Shape::Path {
                            verbs: vec![
                                PathVerb::MoveTo(Point::new(cx0 - arm, cy0 - arm)),
                                PathVerb::LineTo(Point::new(cx0 + arm, cy0 + arm)),
                                PathVerb::MoveTo(Point::new(cx0 + arm, cy0 - arm)),
                                PathVerb::LineTo(Point::new(cx0 - arm, cy0 + arm)),
                            ],
                            fill_rule: Default::default(),
                        },
                        paint: Paint::Solid(ink),
                        width: 1.5,
                    });
                }
            }
            Glyph::Plus => {
                let r = size.width.min(size.height) / 2.0;
                if let Some(well) = well {
                    cx.push(PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: Rect::new(cx0 - r, cy0 - r, 2.0 * r, 2.0 * r),
                            radius: r,
                        },
                        paint: Paint::Solid(well),
                    });
                }
                if let Some(ink) = ink {
                    let arm = r * 0.42;
                    cx.push(PaintOp::Stroke {
                        shape: Shape::Path {
                            verbs: vec![
                                PathVerb::MoveTo(Point::new(cx0 - arm, cy0)),
                                PathVerb::LineTo(Point::new(cx0 + arm, cy0)),
                                PathVerb::MoveTo(Point::new(cx0, cy0 - arm)),
                                PathVerb::LineTo(Point::new(cx0, cy0 + arm)),
                            ],
                            fill_rule: Default::default(),
                        },
                        paint: Paint::Solid(ink),
                        width: 1.5,
                    });
                }
            }
            Glyph::FullScreen => {
                if let Some(well) = well {
                    cx.push(PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: Rect::new(cx0 - half, cy0 - half, GLYPH, GLYPH),
                            radius: half,
                        },
                        paint: Paint::Solid(well),
                    });
                }
                if let Some(ink) = ink {
                    let reach = half * 0.45;
                    let head = half * 0.3;
                    let (l, t, r, b) = (cx0 - reach, cy0 - reach, cx0 + reach, cy0 + reach);
                    cx.push(PaintOp::Stroke {
                        shape: Shape::Path {
                            verbs: vec![
                                // the arrow to the top trailing corner
                                PathVerb::MoveTo(Point::new(cx0 + 1.0, cy0 - 1.0)),
                                PathVerb::LineTo(Point::new(r, t)),
                                PathVerb::MoveTo(Point::new(r - head, t)),
                                PathVerb::LineTo(Point::new(r, t)),
                                PathVerb::LineTo(Point::new(r, t + head)),
                                // the arrow to the bottom leading corner
                                PathVerb::MoveTo(Point::new(cx0 - 1.0, cy0 + 1.0)),
                                PathVerb::LineTo(Point::new(l, b)),
                                PathVerb::MoveTo(Point::new(l + head, b)),
                                PathVerb::LineTo(Point::new(l, b)),
                                PathVerb::LineTo(Point::new(l, b - head)),
                            ],
                            fill_rule: Default::default(),
                        },
                        paint: Paint::Solid(ink),
                        width: 1.5,
                    });
                }
            }
        }
        if cx.is_focused() && cx.focus_visible() {
            focus_ring(cx, 0.0);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.action(Action::Click);
        cx.focusable();
    }
}

/// The preview's panel: `size` points, the popover material under its content, its corners rounded. As the root of the
/// popup it grows out of the item shown and shrinks back into it.
struct Sheet {
    size: (f64, f64),
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Sheet {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.0),
            height: SizeSpec::Fixed(self.size.1),
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.size.0, self.size.1),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !cx.material(Surface::Popover, RADIUS, 1.0) {
            let size = cx.size();
            if let Some(fill) = cx.env().theme.surface(Surface::Popover).fill {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(0.0, 0.0, size.width, size.height),
                        radius: RADIUS,
                    },
                    paint: Paint::Solid(fill),
                });
            }
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn grows_from_anchor(&self) -> bool {
        true
    }
}

/// The folder's items: Space there opens the preview, or closes it, before the scroll view under the items pages
/// with it. Without a message (while a name is edited) Space goes on, and so it does while a name is being typed to
/// select an item, where it is part of the name.
pub(super) struct SpaceKey {
    pub message: Option<Message>,
    /// The browser window whose preview steps through several items: the arrows go from one to the next there,
    /// rather than moving the selection.
    pub steps: Option<u64>,
    pub children: Vec<Element<Message>>,
}

/// How long after a key typed on the items Space still goes on to the name being typed, as the list's search waits.
const TYPING_NS: u64 = 1_000_000_000;

/// Until when Space belongs to a name being typed on the items.
#[derive(Default)]
pub(super) struct Typing {
    until: Option<Timestamp>,
}

impl Widget<Message> for SpaceKey {
    type State = Typing;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Typing {
        Typing::default()
    }

    fn update(&self, _: &Self, _: &mut Typing) -> Change {
        Change::None
    }

    fn measure(&self, _: &Typing, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        typing: &mut Typing,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        let (Some(message), Event::Key(key, _)) = (&self.message, event) else {
            return Propagation::Continue;
        };
        if cx.phase() != Phase::Capture || !key.pressed {
            return Propagation::Continue;
        }
        if let Some(id) = self.steps.filter(|_| key.mods.is_empty()) {
            let by = match key.keysym {
                LEFT | UP => Some(-1),
                RIGHT | DOWN => Some(1),
                _ => None,
            };
            if let Some(by) = by {
                cx.emit(send(id, PeekEvent::Step(by)));
                return Propagation::Stop;
            }
        }
        let now = cx.now();
        let typed = typing.until.is_some_and(|until| now < until);
        let plain = !key.mods.contains(Mods::PRIMARY) && !key.mods.contains(Mods::CTRL);
        let printable = key.text.is_some_and(|c| !c.is_control() && c != ' ');
        if plain && (printable || (typed && key.keysym == SPACE)) {
            typing.until = Some(Timestamp(now.0 + TYPING_NS));
            return Propagation::Continue;
        }
        if key.keysym != SPACE || !key.mods.is_empty() {
            return Propagation::Continue;
        }
        if !key.repeat {
            cx.emit(message.clone());
        }
        Propagation::Stop
    }

    fn paint(&self, _: &Typing, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Typing, _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peek(items: &[&str]) -> Peek {
        let items = items.iter().map(PathBuf::from).collect();
        Peek::new(Browse::new(items, 0).unwrap())
    }

    #[test]
    fn a_preview_waits_for_its_item_out_of_sight_then_says_it_is_loading() {
        let mut look = peek(&["a.png", "b.png"]);
        // the first item is still being made ready: a wait starts, once, and the window stays out of sight
        let wait = look.follow_ready(None).expect("a wait starts");
        assert_eq!(look.follow_ready(None), None, "the same wait goes on");
        assert!(!look.shown && !look.slow);
        // a wait that ran long shows the window in its loading state
        look.waited(wait);
        assert!(look.slow);
        // ready: the loading state ends, and the window keeps the item's size
        assert_eq!(look.follow_ready(Some((300.0, 200.0))), None);
        assert!(look.shown && !look.slow);
        assert_eq!(look.size, (300.0, 200.0));

        // the next item waits in the window as it was, a wait of its own; the end of an older one changes nothing
        look.browse.step(1);
        let next = look.follow_ready(None).expect("a wait for the next item");
        assert_ne!(next, wait);
        look.waited(wait);
        assert!(!look.slow, "a wait for another item");
        look.waited(next);
        assert!(look.slow);
        assert_eq!(look.size, (300.0, 200.0), "the size it last showed");
    }
}
