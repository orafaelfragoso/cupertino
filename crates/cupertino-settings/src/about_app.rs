//! The independent About window on CupertinoKit: the system mark, the name and version, the readings of
//! [`crate::about`] and More Info, which starts Settings.

use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cupertino_kit::controls::{
    Grid, ScrollView, Scroller, Spacer, TextRole, WindowDragArea, PANEL_WINDOW_BACKGROUND,
};
use cupertino_kit::core::{
    next_resource_id, Change, CommandRegistry, Event, EventCx, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Shortcut, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Constraints, ContainerKind, LayoutSpec, Measured, ScrollState, SizeSpec, Track,
};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowRequest};
use cupertino_kit::prelude::*;
use cupertino_kit::render::{ImageResource, PixelFormat, ResourceId, Sampling};
use cupertino_kit::text::{AttributedText, FontWeight, ParagraphStyle, TextStyle, TextSystem};
use cupertino_kit::theme::{ColorRole, StateVariant, ThemeTokens};

use crate::about::{metrics, About, Layout};
use crate::instance;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const APP_ID: &str = "cupertino-settings";
const TITLE: &str = "About This Computer";
const NAME: &str = "Cupertino";
const MORE_INFO: &str = "More Info…";
const ESCAPE: u32 = 0xff1b;
const KEYPAD_ENTER: u32 = 0xff8d;
const SPACE: u32 = 0x20;
const W: u32 = 0x77;

fn about_window() -> WindowKey {
    WindowKey("about".into())
}

/// What [`run`] hands the application before its first frame.
pub struct Start {
    pub about: About,
    /// From [`layout`], with the text system the window draws with.
    pub layout: Layout,
    /// Raises the window when it opens, from the process that started it.
    pub activation: String,
    /// The activation token of each later launch, empty when it carried none.
    pub shown: async_channel::Receiver<String>,
}

static START: Mutex<Option<Start>> = Mutex::new(None);

/// Open About, or hand the launch to the one already open, and serve it until it is closed.
pub fn run(activation: String) -> Result<()> {
    // A pending activation already represents all repeat launches; never queue an unbounded number of them.
    let (show, shown) = async_channel::bounded(1);
    let on_show = move |token| {
        let _ = show.try_send(token);
    };
    let Some(_instance) = instance::serve_or_hand_over(
        || instance::serve_about(zbus::connection::Builder::session()?, on_show.clone()),
        || instance::show_about(zbus::connection::Builder::session()?, &activation),
    )?
    else {
        return Ok(());
    };
    let mut about = About::read(Path::new("/"));
    let screen = about
        .read_displays()
        .map_err(|e| format!("cannot list the displays: {e}"))?;
    let options = cupertino_kit::app::options()?;
    let mut layout = layout(&about, &options.text, &options.theme);
    if let Some(height) = screen {
        layout.fit(height);
    }
    launch(Start {
        about,
        layout,
        activation,
        shown,
    });
    Ok(cupertino_kit::app::run::<AboutApp>()?)
}

/// The text role of the rows' labels and values.
const ROW_ROLE: TextRole = TextRole::Caption;

/// The rows wrapped as the window sets them, in the theme's caption type ([`ROW_ROLE`]).
pub fn layout(about: &About, text: &TextSystem, theme: &ThemeTokens) -> Layout {
    let caption = theme.type_scale.caption;
    about.layout(|line| {
        let style = TextStyle {
            weight: FontWeight(caption.weight),
            ..TextStyle::new(caption.size)
        };
        text.measure(
            &AttributedText::plain(line, style),
            &ParagraphStyle::default(),
            None,
        )
        .map_or(f64::INFINITY, |size| size.width)
    })
}

/// Hand `start` to the next [`AboutApp`] to start, as [`run`] does; for a test's window system.
pub fn launch(start: Start) {
    *START.lock().expect("no thread panics holding the start") = Some(start);
}

pub struct AboutApp;

pub struct AboutWindow {
    about: About,
    layout: Layout,
    shown: async_channel::Receiver<String>,
    /// A More Info process is being started or runs, so a held key or repeated clicks start one.
    more_info: bool,
    /// Where the rows are scrolled to when the screen is too short for them all.
    scroll: ScrollState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A later launch, with its activation token; `None` once no more can come.
    Show(Option<String>),
    MoreInfo,
    /// The token that raises More Info, `None` when the server gives none.
    Token(Option<String>),
    /// The More Info process ended.
    Ended,
    Scroll(ScrollState),
    Close,
}

fn wait(shown: &async_channel::Receiver<String>) -> Command<Message> {
    let shown = shown.clone();
    Command::Perform(Box::pin(
        async move { Message::Show(shown.recv().await.ok()) },
    ))
}

fn shortcuts() -> CommandRegistry<Message> {
    let key = |mods, keysym| Shortcut {
        mods,
        keysym: Keysym(keysym),
    };
    CommandRegistry::new()
        .with(key(Mods::NONE, ESCAPE), Message::Close)
        .with(key(Mods::PRIMARY, W), Message::Close)
        .with(key(Mods::NONE, KEYPAD_ENTER), Message::MoreInfo)
        .with(key(Mods::NONE, SPACE), Message::MoreInfo)
}

/// Start More Info with `token`; the command ends with [`Message::Ended`] when its process exits.
fn more_info(token: &str) -> Result<Command<Message>> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .env("XDG_ACTIVATION_TOKEN", token)
        .spawn()?;
    let (ended, exit) = async_channel::bounded(1);
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("cupertino-settings: More Info exited with {status}"),
            Err(e) => eprintln!("cupertino-settings: cannot wait for More Info: {e}"),
        }
        let _ = ended.try_send(());
    });
    Ok(Command::Perform(Box::pin(async move {
        let _ = exit.recv().await;
        Message::Ended
    })))
}

impl Application for AboutApp {
    type Model = AboutWindow;
    type Message = Message;

    fn init() -> (AboutWindow, Vec<Command<Message>>) {
        let Start {
            about,
            layout,
            activation,
            shown,
        } = START
            .lock()
            .expect("no thread panics holding the start")
            .take()
            .expect("About is launched with its start");
        let mut commands = vec![wait(&shown)];
        if !activation.is_empty() {
            commands.push(Command::Window(
                about_window(),
                WindowRequest::Activate(Some(activation)),
            ));
        }
        let window = AboutWindow {
            about,
            layout,
            shown,
            more_info: false,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
        };
        (window, commands)
    }

    fn update(window: &mut AboutWindow, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Show(None) => Vec::new(),
            Message::Show(Some(token)) => vec![
                Command::Window(
                    about_window(),
                    WindowRequest::Activate((!token.is_empty()).then_some(token)),
                ),
                wait(&window.shown),
            ],
            Message::MoreInfo if window.more_info => Vec::new(),
            Message::MoreInfo => {
                window.more_info = true;
                vec![Command::ActivationToken {
                    window: about_window(),
                    app_id: Some(APP_ID.into()),
                    on_token: Box::new(Message::Token),
                }]
            }
            Message::Token(token) => match more_info(&token.unwrap_or_default()) {
                Ok(ended) => vec![ended],
                Err(e) => {
                    eprintln!("cupertino-settings: cannot start More Info: {e}");
                    window.more_info = false;
                    Vec::new()
                }
            },
            Message::Ended => {
                window.more_info = false;
                Vec::new()
            }
            Message::Scroll(scroll) => {
                window.scroll = scroll;
                Vec::new()
            }
            Message::Close => vec![Command::Exit],
        }
    }

    fn view(window: &AboutWindow) -> Vec<Window<Message>> {
        let (width, height) = window.layout.size();
        let size = Size::new(f64::from(width), f64::from(height));
        vec![Window {
            key: about_window(),
            spec: WindowSpec {
                title: TITLE.into(),
                app_id: APP_ID.into(),
                size,
                min_size: Some(size),
                max_size: Some(size),
                decorations: DECORATIONS,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: shortcuts(),
            on_close: Some(Message::Close),
            background: BACKGROUND,
            // moved by a press anywhere the button does not take
            content: Element::from(WindowDragArea {
                content: content(&window.about, &window.layout, window.scroll),
            }),
            popup: None,
            menus: None,
        }]
    }
}

/// The window's title bar is only its control buttons, over the top of the content, which runs to every edge.
pub const DECORATIONS: DecorationMode = DecorationMode::Unified;
/// The glass of a panel window under the whole window, its corners the window's.
pub const BACKGROUND: WindowBackground = PANEL_WINDOW_BACKGROUND;

/// `content` set in `role`, which decides its size.
fn text(content: &str, role: TextRole, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

/// `children` stacked from the top of their cell, across it at `align`.
fn cell(align: Align, children: Vec<Element<Message>>) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align,
        children,
    })
}

/// The approved layout as a grid of fixed tracks, so every row sits where [`Layout`] put it and the window is its size.
fn content(about: &About, layout: &Layout, scroll: ScrollState) -> Element<Message> {
    use metrics::*;
    let whole = |children| cell(Align::Center, children).span(3, 1);
    let rows_block = layout.rows_view.unwrap_or(if layout.rows.is_empty() {
        layout.button.y - ROWS_Y - BUTTON_GAP
    } else {
        layout.rows_height()
    });
    let mut list = Element::new(Grid {
        columns: columns(),
        rows: row_tracks(layout),
        padding: Insets::ZERO,
        spacing: Size::ZERO,
        children: row_cells(about, layout),
    });
    if layout.rows_view.is_some() {
        // a screen too short for every row: they scroll in what it leaves
        list = Element::from(ScrollView {
            content: list,
            state: scroll,
            horizontal: false,
            vertical: true,
            scroller: Scroller::Overlay,
            on_scroll: Box::new(Message::Scroll),
        })
        .key("rows");
    }
    let rows = vec![
        Track::Fixed(NAME_Y - TOP),
        Track::Fixed(VERSION_Y - NAME_Y),
        Track::Fixed(ROWS_Y - VERSION_Y),
        Track::Fixed(rows_block),
        Track::Fixed(BUTTON_GAP),
        Track::Fixed(layout.button.h),
    ];
    let children = vec![
        whole(vec![Element::new(Mark)]),
        whole(vec![text(NAME, TextRole::LargeTitle, false)]),
        whole(vec![text(&about.version, TextRole::Caption, true)]),
        list.span(3, 1),
        Element::new(Spacer { min: 0.0 }).span(3, 1),
        whole(vec![Element::new(Button {
            label: MORE_INFO.into(),
            // a plain grey button: the window asks nothing, so it has no default action to colour
            role: ButtonRole::Normal,
            style: ButtonStyle::Bordered,
            enabled: true,
            busy: false,
            on_press: Some(Message::MoreInfo),
        })
        .width(layout.button.w)]),
    ];
    Element::new(Grid {
        columns: columns(),
        rows,
        padding: Insets::new(TOP, PADDING, BOTTOM, PADDING),
        spacing: Size::ZERO,
        children,
    })
}

/// The labels' column, the gap about the centre and the values' column.
fn columns() -> Vec<Track> {
    use metrics::*;
    vec![
        Track::Fixed(LABEL_COLUMN),
        Track::Fixed(LABEL_END + VALUE_START),
        Track::Fixed(VALUE_COLUMN),
    ]
}

/// Each row as tall as its wrapped lines put it, so every row sits where [`Layout`] put it.
fn row_tracks(layout: &Layout) -> Vec<Track> {
    let end = metrics::ROWS_Y + layout.rows_height();
    let mut tops = layout.rows.iter().map(|row| row.y).skip(1);
    layout
        .rows
        .iter()
        .map(|row| Track::Fixed(tops.next().unwrap_or(end) - row.y))
        .collect()
}

/// A repeated label is shown once, on the first of its rows.
fn row_cells(about: &About, layout: &Layout) -> Vec<Element<Message>> {
    let mut children = Vec::new();
    for (row, value) in layout.rows.iter().zip(&about.rows) {
        let label = if row.label.is_empty() {
            Element::new(Spacer { min: 0.0 })
        } else {
            cell(Align::End, vec![text(row.label, ROW_ROLE, true)])
        };
        children.push(label);
        children.push(Element::new(Spacer { min: 0.0 }));
        children.push(cell(
            Align::Start,
            vec![text(&value.value, ROW_ROLE, false)],
        ));
    }
    children
}

/// The system mark in the label colour; decorative, as the name beside it says what it is.
struct Mark;

/// The mark as last drawn: its device pixels and colour, and the image they made.
type Drawn = RefCell<Option<((u32, [u8; 4]), ResourceId, ImageResource)>>;

impl Widget<Message> for Mark {
    type State = Drawn;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(metrics::MARK),
            height: SizeSpec::Fixed(metrics::MARK),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Drawn {
        RefCell::new(None)
    }

    fn update(&self, _: &Self, _: &mut Drawn) -> Change {
        Change::None
    }

    fn measure(&self, _: &Drawn, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(metrics::MARK, metrics::MARK),
            baseline: None,
        }
    }

    fn event(&self, _: &mut Drawn, _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, drawn: &Drawn, cx: &mut PaintCx<'_>) {
        let Some(color) = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Text, StateVariant::NORMAL)
        else {
            return;
        };
        let rgba = [color.r, color.g, color.b, color.a];
        let px = (metrics::MARK * cx.env().scale).round() as u32;
        let key = (px, rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
        let mut drawn = drawn.borrow_mut();
        if drawn.as_ref().is_none_or(|(at, ..)| *at != key) {
            let mark = cupertino_icons::system_mark(px, rgba);
            let image = ImageResource {
                revision: 1,
                width: mark.width(),
                height: mark.height(),
                stride: mark.width() * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(mark.data()),
            };
            *drawn = Some((key, next_resource_id(), image));
        }
        let (_, id, image) = drawn.as_ref().expect("drawn above");
        let size = cx.size();
        cx.image(
            *id,
            image,
            Rect::new(0.0, 0.0, size.width, size.height),
            Sampling::Linear,
        );
    }

    fn semantics(&self, _: &Drawn, _: &mut SemanticsCx<'_>) {}
}
