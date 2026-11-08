//! The preview window on CupertinoKit: Space, or File › Quick Preview, shows the selected items one at a time in a
//! floating window of its own, without opening them.
//!
//! The window has its own toolbar: a close button and a full screen button at its leading end, the item's name, and an
//! "Open with" button naming the application that opens the item. Under it a picture shows whole, the start of a text
//! file in a fixed-pitch face, and anything else its large icon with its kind, size and dates
//! ([`cupertino_kit::controls::FilePreview`]). With several items selected the arrows go from one to the next; with
//! one, they move the window's selection, which the preview follows. Space or Escape closes it.
//!
//! The window grows out of its centre as it opens, fading in, and shrinks back as it closes
//! ([`crate::preview::Motion`]); with Reduce Motion it only fades.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cupertino_kit::controls::{FilePreview, PreviewStyle, Text, TextRole, WindowDragArea};
use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Shortcut, Widget, WindowBackground,
};
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

use super::columns::preview_content;
use super::toolbar::focus_ring;
use super::{BrowserWindow, Event as WindowEvent, Message, Session, NO_SCROLL};
use crate::preview::{self as file_preview, Browse, Motion, MotionFrame, PreviewKind};
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

const SPACE: Keysym = Keysym(0x20);
const ESCAPE: Keysym = Keysym(0xff1b);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const RETURN: Keysym = Keysym(0xff0d);

/// The start of the text files a window previews, by path, with the time each was changed when it was read.
#[derive(Default)]
pub(super) struct Texts {
    read: HashMap<PathBuf, (Option<SystemTime>, Option<String>)>,
}

impl Texts {
    /// The start of the text of the file at `path`, once read.
    pub fn get(&self, path: &Path) -> Option<String> {
        self.read.get(path).and_then(|(_, text)| text.clone())
    }

    /// Keep the texts of `entries` (the text files previewed now), reading those not read yet or changed since, and
    /// forget the others.
    pub fn follow<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>) {
        let wanted: Vec<&Entry> = entries
            .into_iter()
            .filter(|e| file_preview::kind(e) == PreviewKind::Text)
            .collect();
        self.read
            .retain(|path, _| wanted.iter().any(|e| &e.path == path));
        for entry in wanted {
            let fresh = self
                .read
                .get(&entry.path)
                .is_some_and(|(modified, _)| *modified == entry.modified);
            if fresh {
                continue;
            }
            let text = match file_preview::read_text(&entry.path) {
                Ok(text) => text.map(|t| t.text),
                Err(e) => {
                    eprintln!(
                        "cupertino-files: cannot read {} to preview it: {e}",
                        entry.path.display()
                    );
                    None
                }
            };
            self.read.insert(entry.path.clone(), (entry.modified, text));
        }
    }
}

/// The preview window of a browser window.
pub(super) struct Peek {
    pub browse: Browse,
    /// The window is closing: it shrinks away, then goes.
    pub closing: bool,
    /// When the motion's first frame came, in seconds, and the latest frame.
    started: Option<f64>,
    now: f64,
    /// How far into its motion the window starts at its next frame: a close while still opening starts part way.
    rewind: f64,
    reduce: bool,
    pub scroll: ScrollState,
    pub full_screen: bool,
    /// The name of the application that opens the item shown, with the item it was read for.
    app: Option<(PathBuf, Option<String>)>,
}

/// What the preview window asks of its browser window.
#[derive(Clone, Debug, PartialEq)]
pub enum PeekEvent {
    /// A frame of the window's motion at `now` seconds; `animate` is false with Reduce Motion.
    Tick {
        now: f64,
        animate: bool,
    },
    /// The previous item (-1) or the next (1).
    Step(isize),
    /// Close the window: it shrinks away first.
    Close,
    /// The window closed at once, as the compositor's close does.
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
            closing: false,
            started: None,
            now: 0.0,
            rewind: 0.0,
            reduce: false,
            scroll: NO_SCROLL,
            full_screen: false,
            app: None,
        }
    }

    fn motion(&self, size: (f64, f64)) -> Motion {
        Motion {
            closing: self.closing,
            reduce: self.reduce,
            size,
        }
    }

    fn elapsed(&self) -> f64 {
        self.started
            .map_or(self.rewind, |start| (self.now - start).max(0.0))
    }

    /// The window's motion moves.
    fn animating(&self) -> bool {
        self.started.is_none()
            || !self
                .motion(file_preview::WINDOW_SIZE)
                .finished(self.elapsed())
    }

    /// The motion's frame now.
    fn frame(&self, size: (f64, f64)) -> MotionFrame {
        self.motion(size).at(self.elapsed())
    }

    /// Start closing: the window shrinks away from where its opening got to.
    pub fn close(&mut self) {
        if self.closing {
            return;
        }
        // a window still opening closes from the size it reached
        let reached = self.frame(file_preview::WINDOW_SIZE);
        self.closing = true;
        let duration = self.motion(file_preview::WINDOW_SIZE).duration();
        self.rewind = duration * (1.0 - reached.alpha.clamp(0.0, 1.0));
        // the close runs from its first frame
        self.started = None;
    }

    /// A frame came at `now`: whether the window has finished closing.
    pub fn tick(&mut self, now: f64, animate: bool) -> bool {
        self.reduce = !animate;
        self.now = now;
        let started = *self.started.get_or_insert(now - self.rewind);
        self.closing
            && self
                .motion(file_preview::WINDOW_SIZE)
                .finished(now - started)
    }

    /// The item shown changed to `path`: its application's name is read again, from `session`'s associations.
    pub fn follow_app(&mut self, session: &Session, path: &Path) {
        if self.app.as_ref().is_some_and(|(p, _)| p == path) {
            return;
        }
        let name = (session.applications)().ok().and_then(|apps| {
            crate::open::open_with(&apps, path, path.is_dir())
                .into_iter()
                .find(|item| item.is_default)
                .map(|item| item.entry.name.clone())
        });
        self.app = Some((path.to_path_buf(), name));
        self.scroll = NO_SCROLL;
    }
}

/// The key of the preview window of browser window `id`.
pub(super) fn window_key(id: u64) -> cupertino_kit::core::WindowKey {
    cupertino_kit::core::WindowKey(Key::from(format!("preview/{id}")))
}

fn send(id: u64, event: PeekEvent) -> Message {
    Message::Window(id, WindowEvent::Peek(event))
}

/// The preview window of browser window `id`, when it has one and its item is listed.
pub(super) fn window(session: &Session, id: u64, win: &BrowserWindow) -> Option<Window<Message>> {
    let look = win.peek.as_ref()?;
    let entry = win
        .browser
        .entries()
        .iter()
        .find(|e| e.path == look.browse.current())?;
    let content = preview_content(session, &win.texts, entry, PICTURE, ICON);
    let size = match &content {
        cupertino_kit::controls::PreviewContent::Picture(source) => file_preview::picture_window(
            (source.image.width, source.image.height),
            2.0,
            TOOLBAR_HEIGHT,
        ),
        _ => file_preview::WINDOW_SIZE,
    };
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
    let body = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![toolbar(id, look, entry, app), preview],
    })
    .fill();
    let frame = look.frame(size);
    Some(Window {
        key: window_key(id),
        spec: WindowSpec {
            title: entry.name.clone(),
            app_id: super::APP_ID.into(),
            size: Size::new(size.0, size.1),
            min_size: Some(Size::new(320.0, 240.0)),
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: shortcuts(id),
        on_close: Some(send(id, PeekEvent::Closed)),
        background: WindowBackground::None,
        popup: None,
        menus: None,
        content: Element::new(Reveal {
            window: id,
            animating: look.animating(),
            size,
            children: vec![Element::new(Panel {
                alpha: frame.alpha,
                full: !look.animating() || look.reduce,
                size,
                inset: inset(size, frame.scale),
                children: vec![Element::new(Shift {
                    full: !look.animating() || look.reduce,
                    size,
                    inset: inset(size, frame.scale),
                    children: vec![body],
                })],
            })],
        })
        .key("reveal"),
    })
}

/// How far each side of the window's panel is in at `scale`.
fn inset(size: (f64, f64), scale: f64) -> (f64, f64) {
    let scale = scale.clamp(0.0, 1.0);
    ((1.0 - scale) * size.0 / 2.0, (1.0 - scale) * size.1 / 2.0)
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
    Element::from(WindowDragArea {
        content: Element::new(Bar {
            children: vec![row],
        }),
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
        let ink = colors.get(ColorRole::SecondaryText, StateVariant::NORMAL);
        let well = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL);
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
                if let Some(ink) = ink {
                    let reach = half * 0.8;
                    let head = half * 0.45;
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

/// The window's motion: the frames it asks for while it moves, and the panel it reveals.
struct Reveal {
    window: u64,
    animating: bool,
    size: (f64, f64),
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Reveal {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.animating && !old.animating {
            Change::Paint
        } else if self.size != old.size {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        if let Event::AnimationFrame(now) = *event {
            if self.animating {
                let animate = !cx.env().theme.motion.reduce;
                cx.emit(send(
                    self.window,
                    PeekEvent::Tick {
                        now: now.0 as f64 / 1e9,
                        animate,
                    },
                ));
                cx.request_animation_frame();
            }
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.animating {
            cx.request_animation_frame();
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The window's surface, `inset` in from each side while it grows or shrinks, faded to `alpha`; its content keeps the
/// window's full size meanwhile, so it is revealed rather than squeezed.
struct Panel {
    alpha: f64,
    /// The motion ended: the panel and its content follow the window's size.
    full: bool,
    size: (f64, f64),
    inset: (f64, f64),
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Panel {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (ix, iy) = self.inset;
        let (width, height) = match self.full {
            true => (SizeSpec::Fill, SizeSpec::Fill),
            false => (
                SizeSpec::Fixed((self.size.0 - 2.0 * ix).max(0.0)),
                SizeSpec::Fixed((self.size.1 - 2.0 * iy).max(0.0)),
            ),
        };
        LayoutSpec {
            width,
            height,
            position: if self.full {
                Point::ZERO
            } else {
                Point::new(ix, iy)
            },
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.full, self.size, self.inset) != (old.full, old.size, old.inset) {
            Change::Layout
        } else if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        if !cx.material(Surface::Popover, RADIUS, alpha) {
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
}

/// The panel's content at the window's full size, moved back by the panel's inset so it stays where it will be.
struct Shift {
    full: bool,
    size: (f64, f64),
    inset: (f64, f64),
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Shift {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height, position) = match self.full {
            true => (SizeSpec::Fill, SizeSpec::Fill, Point::ZERO),
            false => (
                SizeSpec::Fixed(self.size.0),
                SizeSpec::Fixed(self.size.1),
                Point::new(-self.inset.0, -self.inset.1),
            ),
        };
        LayoutSpec {
            width,
            height,
            position,
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
        if (self.full, self.size, self.inset) != (old.full, old.size, old.inset) {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The folder's items: Space there opens the preview, or closes it, before the scroll view under the items pages
/// with it. Without a message (while a name is edited) Space goes on, and so it does while a name is being typed to
/// select an item, where it is part of the name.
pub(super) struct SpaceKey {
    pub message: Option<Message>,
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
