//! The toolkit's surfaces for look checks, each a window drawn on the headless window system with the CPU renderer:
//! `kit/controls`, the sheet of the choice and value controls of Kit's row 167a capture (check boxes off, on, mixed and
//! disabled, radio buttons, a segmented control, the regular, ticked, level and disabled sliders, and progress), and
//! `kit/icons`, a grid of system symbols at the toolkit's sizes and tints with the chevrons along the reading direction
//! and the placeholder of a missing symbol, `kit/sidebar`, a sidebar's sections running to the window's edges with
//! its second row selected, `kit/tooltip`, a short tip, one cut with an ellipsis at the text width and a one-line label
//! cut at its row's width. `kit/counter`, `kit/forms`, `kit/virtual-list` and `kit/editor` are the examples' windows
//! at their own sizes, as they open (the Editor with a short document typed in). `kit/scroll` is a list in a scroll
//! view with overlay bars at a moment of a scroll, `--phase`: `rest`, `stretch` (a touchpad finger holding the list
//! pulled down past its start), `shown` (just after a wheel scroll), `fading` (half way through the bar's fade) or
//! `hidden` (after it). `kit/under-toolbar` is a list on a page under a toolbar row, the row drawn in a layer over the
//! page as a desktop with app effects version 11 draws it, at `--phase` `rest`, `shown` or `hidden`: what passes under
//! the row fades out under the scroll edge. `--direction ltr|rtl` (default ltr) lays a surface out in that direction;
//! the sheet shows both.
//!
//! `kit/gallery` is every control family side by side: buttons (ordinary, default, destructive, disabled and glass),
//! switches, check boxes, radio buttons, a segmented control, a pop-up button, a stepper, a text and a search field,
//! a pull-down button, a combo box, a date picker, a colour well, sliders and progress. `--state rest|held-button|held-switch|held-slider|held-segment|disabled` holds the pointer
//! down on one control (the switch dragged a little, the segmented control dragged from its first segment onto its
//! last) or disables them all; `--control-size mini|small|regular|large|extra-large` and `--compact on|off` size
//! them; `--backdrop flat|busy` draws them over the window's background or over hard-edged colour bands. Its sheet
//! shows each state.
//!
//! `ci/capture.sh kit/controls --scale 2` (the flags are `cupertino-capture`'s)
//!
//! The fonts, symbols and theme are the system's, as an application's ([`cupertino_kit::app::options`]). The clock
//! stands still, so every control shows its resting look, but for the gallery's held states.

#[path = "apps/counter.rs"]
mod counter;
#[path = "apps/editor.rs"]
mod editor;
#[path = "apps/forms.rs"]
mod forms;
#[path = "apps/virtual-list.rs"]
mod virtual_list;

use std::cell::Cell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_capture::{App, Look, Request, State};
use cupertino_kit::controls::{
    unobscured_insets, CalendarDate, Canvas, CheckBox, ColorWell, ComboBox, DatePicker, Icon,
    IconDirection, NavigationStack, PopUpButton, Progress, PullDownButton, RadioGroup, ScrollView,
    Scroller, SearchField, SegmentedControl, Sidebar, SidebarItem, SidebarSection, Slider,
    SliderStyle, Stepper, TextRole, ToolbarTitle, Tooltip, TooltipState, WINDOW_CONTENT_INSET,
};
use cupertino_kit::core::{Checked, CommandRegistry, Key, PaintCx};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{LayoutDirection, ScrollState};
use cupertino_kit::platform::{
    Capabilities, DecorationMode, MouseButton, PointerEvent, ScrollSource, SystemAppearance,
    Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::prelude::*;
use cupertino_kit::render::{GradientStop, Paint, PaintOp, PixelFormat, Shape};
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_kit::theme::{Color, ColorRole, ControlSize, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use tiny_skia::{IntSize, Pixmap};

/// Between the controls of a row, an approximate design value.
const ROW_GAP: f64 = 16.0;
/// Between the rows of a window, an approximate design value.
const LINE_GAP: f64 = 12.0;
/// The rows of the icon grid: the toolbar, sidebar and status symbols the apps draw, the chevrons again along the
/// reading direction (mirrored under `--direction rtl`), and a name no pack provides (the placeholder).
const SYMBOLS: [(&str, IconDirection); 15] = [
    ("symbol.chevron.left", IconDirection::Fixed),
    ("symbol.chevron.right", IconDirection::Fixed),
    ("symbol.magnifyingglass", IconDirection::Fixed),
    ("symbol.house", IconDirection::Fixed),
    ("symbol.desktop", IconDirection::Fixed),
    ("symbol.document", IconDirection::Fixed),
    ("symbol.download", IconDirection::Fixed),
    ("symbol.trash", IconDirection::Fixed),
    ("symbol.wifi", IconDirection::Fixed),
    ("symbol.bluetooth", IconDirection::Fixed),
    ("symbol.speaker.high", IconDirection::Fixed),
    ("symbol.battery", IconDirection::Fixed),
    ("symbol.chevron.left", IconDirection::Semantic),
    ("symbol.chevron.right", IconDirection::Semantic),
    ("symbol.does-not-exist", IconDirection::Fixed),
];
/// The icon sizes of the grid in points: small, regular and large symbols.
const ICON_SIZES: [f64; 3] = [16.0, 20.0, 24.0];
/// The gallery's text and search fields' width.
const FIELD_WIDTH: f64 = 150.0;
/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 8;
/// The time between the frames of a held control, in nanoseconds: long enough for any press animation to finish.
const SETTLE_STEP: u64 = 500_000_000;

#[derive(Clone, Debug, PartialEq)]
enum Message {}

fn text(content: &str, role: TextRole) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary: false,
        selected: false,
    })
}

fn stack<M: 'static>(axis: Axis, spacing: f64, children: Vec<Element<M>>) -> Element<M> {
    Element::new(Stack {
        axis,
        padding: Insets::ZERO,
        spacing,
        align: if axis == Axis::Vertical {
            Align::Stretch
        } else {
            Align::Center
        },
        children,
    })
}

fn window<M: Clone + 'static>(
    title: &'static str,
    size: Size,
    content: Vec<Element<M>>,
) -> Vec<Window<M>> {
    vec![Window {
        key: WindowKey(title.into()),
        spec: WindowSpec {
            title: title.into(),
            app_id: "org.cupertino.examples.Capture".into(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        popup: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: WINDOW_CONTENT_INSET,
            spacing: LINE_GAP,
            align: Align::Stretch,
            children: content,
        }),
        menus: None,
    }]
}

struct Controls;

impl Application for Controls {
    type Model = ();
    type Message = Message;

    fn init() -> ((), Vec<Command<Message>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), message: Message) -> Vec<Command<Message>> {
        match message {}
    }

    fn view(_: &()) -> Vec<Window<Message>> {
        let check = |label: &str, checked, enabled| {
            Element::new(CheckBox {
                label: label.into(),
                checked,
                enabled,
                on_toggle: None,
            })
        };
        let slider = |label: &str, value, ticks, style, enabled| {
            Element::new(Slider {
                label: label.into(),
                value,
                min: 0.0,
                max: 1.0,
                step: None,
                style,
                ticks,
                enabled,
                on_change: None,
                on_end: None,
            })
        };
        window(
            "Controls",
            Size::new(440.0, 330.0),
            vec![
                stack(
                    Axis::Horizontal,
                    ROW_GAP,
                    vec![
                        check("Off", Checked::False, true),
                        check("On", Checked::True, true),
                        check("Mixed", Checked::Mixed, true),
                        check("Disabled", Checked::True, false),
                    ],
                ),
                Element::new(RadioGroup {
                    label: "Size".into(),
                    options: vec!["Small".into(), "Medium".into(), "Large".into()],
                    selected: Some(1),
                    axis: Axis::Horizontal,
                    spacing: ROW_GAP,
                    enabled: true,
                    on_select: None,
                }),
                stack(
                    Axis::Horizontal,
                    0.0,
                    vec![Element::new(SegmentedControl {
                        label: "View".into(),
                        segments: vec!["Icons".into(), "List".into(), "Columns".into()],
                        selected: Some(1),
                        enabled: true,
                        on_select: None,
                    })],
                ),
                slider("Regular", 0.4, 0, SliderStyle::Regular, true),
                slider("Ticks", 0.7, 5, SliderStyle::Regular, true),
                slider("Level", 0.6, 0, SliderStyle::Level, true),
                slider("Disabled", 0.3, 0, SliderStyle::Regular, false),
                Element::new(Progress {
                    label: "Progress".into(),
                    value: Some(0.6),
                }),
            ],
        )
    }
}

struct Icons;

impl Application for Icons {
    type Model = ();
    type Message = Message;

    fn init() -> ((), Vec<Command<Message>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), message: Message) -> Vec<Command<Message>> {
        match message {}
    }

    fn view(_: &()) -> Vec<Window<Message>> {
        let rows = SYMBOLS
            .iter()
            .map(|&(name, direction)| {
                let icon = |size, tint| {
                    Element::new(Icon {
                        name: name.into(),
                        size,
                        direction,
                        label: None,
                        tint,
                    })
                };
                let mut row: Vec<Element<Message>> =
                    ICON_SIZES.iter().map(|&s| icon(s, None)).collect();
                row.push(icon(20.0, Some(ColorRole::SecondaryText)));
                row.push(icon(20.0, Some(ColorRole::KeyboardFocusIndicator)));
                let mut caption = name.trim_start_matches("symbol.").to_string();
                if direction == IconDirection::Semantic {
                    caption.push_str(", semantic");
                }
                row.push(text(&caption, TextRole::Caption));
                stack(Axis::Horizontal, ROW_GAP, row)
            })
            .collect();
        window(
            "Icons",
            Size::new(360.0, 540.0),
            vec![stack(Axis::Vertical, 8.0, rows)],
        )
    }
}

/// A short tip, a tip past the tooltip's text width, and a one-line label past its row's width.
struct Tips;

impl Application for Tips {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let bubble = |text: &str| {
            let tip = Tooltip {
                text: text.into(),
                anchor: Element::new(Spacer { min: 0.0 }),
                state: TooltipState::default(),
                on_change: Box::new(|_| ()),
            }
            .window(
                WindowKey("tip".into()),
                WindowKey("Tips".into()),
                "anchor".into(),
            )
            .content;
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::default(),
                spacing: 0.0,
                align: Align::Start,
                children: vec![tip],
            })
        };
        let label = Element::new(Text {
            content: AttributedText::plain(
                "Quarterly report, final revision with the reviewers' comments.pdf",
                TextStyle::new(13.0),
            ),
            role: TextRole::Body,
            wrap: false,
            secondary: false,
            selected: false,
        });
        window(
            "Tips",
            Size::new(360.0, 140.0),
            vec![
                bubble("Move to the Trash"),
                bubble("Move the selected items to the Trash, where they stay until it is emptied or they are put back"),
                label,
            ],
        )
    }
}

struct Places;

impl Application for Places {
    type Model = ScrollState;
    type Message = ScrollState;

    fn init() -> (ScrollState, Vec<Command<ScrollState>>) {
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        (scroll, Vec::new())
    }

    fn update(scroll: &mut ScrollState, message: ScrollState) -> Vec<Command<ScrollState>> {
        *scroll = message;
        Vec::new()
    }

    fn view(scroll: &ScrollState) -> Vec<Window<ScrollState>> {
        let item = |label: &str, icon: &str| SidebarItem {
            key: Key::from(label.to_owned()),
            label: label.into(),
            icon: Some(icon.into()),
            description: None,
            accessory: None,
            busy: false,
        };
        let mut windows = window("Sidebar", Size::new(220.0, 300.0), Vec::new());
        windows[0].content = Element::from(Sidebar {
            label: "Places".into(),
            sections: vec![
                SidebarSection {
                    title: Some("Favorites".into()),
                    items: vec![
                        item("Home", "symbol.house"),
                        item("Desktop", "symbol.desktop"),
                        item("Documents", "symbol.document"),
                        item("Downloads", "symbol.download"),
                    ],
                },
                SidebarSection {
                    title: Some("Locations".into()),
                    items: vec![item("Trash", "symbol.trash")],
                },
            ],
            selected: Some(1),
            scroll: *scroll,
            on_scroll: Box::new(|scroll| scroll),
            on_select: None,
            on_accessory: None,
        });
        windows
    }
}

/// Thirty rows in a scroll view with overlay bars.
struct Scrolled;

impl Application for Scrolled {
    type Model = ScrollState;
    type Message = ScrollState;

    fn init() -> (ScrollState, Vec<Command<ScrollState>>) {
        Places::init()
    }

    fn update(scroll: &mut ScrollState, message: ScrollState) -> Vec<Command<ScrollState>> {
        *scroll = message;
        Vec::new()
    }

    fn view(scroll: &ScrollState) -> Vec<Window<ScrollState>> {
        let rows = (1..=30)
            .map(|i| {
                Element::new(Text {
                    content: AttributedText::plain(format!("Row {i}"), TextStyle::new(13.0)),
                    role: TextRole::Body,
                    wrap: false,
                    secondary: false,
                    selected: false,
                })
            })
            .collect();
        let mut windows = window("Scroll", Size::new(260.0, 220.0), Vec::new());
        windows[0].content = Element::from(ScrollView {
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::uniform(12.0),
                spacing: 6.0,
                align: Align::Stretch,
                children: rows,
            }),
            state: *scroll,
            horizontal: false,
            vertical: true,
            scroller: Scroller::Overlay,
            on_scroll: Box::new(|scroll| scroll),
        });
        windows
    }
}

/// Thirty rows on a page under a toolbar row: the page fills the window, starts below the row and scrolls behind it,
/// on a window system that draws the row in a layer over it.
struct UnderToolbar;

impl Application for UnderToolbar {
    type Model = ScrollState;
    type Message = ScrollState;

    fn init() -> (ScrollState, Vec<Command<ScrollState>>) {
        Places::init()
    }

    fn update(scroll: &mut ScrollState, message: ScrollState) -> Vec<Command<ScrollState>> {
        *scroll = message;
        Vec::new()
    }

    fn view(scroll: &ScrollState) -> Vec<Window<ScrollState>> {
        let rows = (1..=30)
            .map(|i| {
                Element::new(Text {
                    content: AttributedText::plain(format!("Row {i}"), TextStyle::new(13.0)),
                    role: TextRole::Body,
                    wrap: false,
                    secondary: false,
                    selected: false,
                })
            })
            .collect();
        let row = unobscured_insets(DecorationMode::Unified);
        let mut windows = window("Under the toolbar", Size::new(320.0, 240.0), Vec::new());
        windows[0].spec.decorations = DecorationMode::Unified;
        windows[0].content = Element::from(NavigationStack {
            content: Element::from(ScrollView {
                content: Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: Insets {
                        top: row.top + 12.0,
                        ..Insets::uniform(12.0)
                    },
                    spacing: 6.0,
                    align: Align::Stretch,
                    children: rows,
                }),
                state: *scroll,
                horizontal: false,
                vertical: true,
                scroller: Scroller::Overlay,
                on_scroll: Box::new(|scroll| scroll),
            }),
            toolbar: vec![Element::new(ToolbarTitle {
                text: "Rows".into(),
            })],
            can_go_back: true,
            can_go_forward: false,
            on_navigate: None,
            under_toolbar: true,
        });
        windows
    }
}

/// What the gallery shows: the control under the pointer, the controls' size, the compact setting and what lies
/// under the controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GalleryOptions {
    held: Option<Held>,
    disabled: bool,
    size: ControlSize,
    compact: bool,
    busy: bool,
}

impl Default for GalleryOptions {
    fn default() -> Self {
        Self {
            held: None,
            disabled: false,
            size: ControlSize::Regular,
            compact: false,
            busy: false,
        }
    }
}

/// The control the gallery holds the pointer down on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    /// The ordinary button, pressed.
    Button,
    /// The off switch, pressed and dragged a little along its track.
    Switch,
    /// The regular slider, its knob pressed.
    Slider,
    /// The segmented control, pressed on its first segment and dragged onto its last.
    Segment,
}

thread_local! {
    static GALLERY: Cell<GalleryOptions> = Cell::new(GalleryOptions::default());
    /// The window system draws a window's raised nodes in a layer over its content, as a desktop with app effects
    /// version 11 does.
    static LAYERED: Cell<bool> = const { Cell::new(false) };
}

/// A backdrop of hard-edged diagonal bands of light and dark colours, for how the controls read over a busy picture.
fn busy_backdrop(cx: &mut PaintCx<'_>) {
    const BANDS: [[u8; 3]; 8] = [
        [242, 160, 48],
        [24, 52, 96],
        [250, 246, 236],
        [40, 150, 140],
        [12, 14, 20],
        [214, 70, 120],
        [250, 220, 90],
        [70, 90, 200],
    ];
    let size = cx.size();
    let rect = Rect::new(0.0, 0.0, size.width, size.height);
    let count = BANDS.len() * 3;
    let stops = (0..count)
        .flat_map(|i| {
            let [r, g, b] = BANDS[i % BANDS.len()];
            let color = Color::from_rgba8([r, g, b, 255]);
            let from = i as f32 / count as f32;
            let to = (i + 1) as f32 / count as f32;
            [
                GradientStop {
                    offset: from,
                    color,
                },
                GradientStop { offset: to, color },
            ]
        })
        .collect();
    cx.push(PaintOp::Fill {
        shape: Shape::Rect(rect),
        paint: Paint::Linear {
            start: Point::ZERO,
            end: Point::new(size.width, size.height),
            stops,
        },
    });
}

/// Every control family side by side, in the gallery's options.
struct Gallery;

impl Application for Gallery {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let options = GALLERY.with(Cell::get);
        let enabled = !options.disabled;
        let button = |label: &str, role, style, on| {
            Element::new(Button {
                label: label.into(),
                role,
                style,
                enabled: on,
                busy: false,
                on_press: Some(()),
            })
        };
        let toggle = |label: &str, on, enabled| {
            Element::new(Toggle {
                label: label.into(),
                on,
                enabled,
                on_toggle: Some(Box::new(|_| ())),
            })
        };
        let check = |label: &str, checked, enabled| {
            Element::new(CheckBox {
                label: label.into(),
                checked,
                enabled,
                on_toggle: Some(Box::new(|_| ())),
            })
        };
        let field = |label: &str, placeholder: &str, value: &str| TextField {
            label: label.into(),
            placeholder: placeholder.into(),
            value: value.into(),
            revision: 0,
            purpose: ContentPurpose::Normal,
            enabled,
            on_change: Some(Box::new(|_| ())),
            on_submit: None,
            selection: None,
            initial: false,
        };
        let slider = |label: &str, value, ticks| {
            Element::new(Slider {
                label: label.into(),
                value,
                min: 0.0,
                max: 1.0,
                step: None,
                style: SliderStyle::Regular,
                ticks,
                enabled,
                on_change: Some(Box::new(|_| ())),
                on_end: None,
            })
        };
        let row = |children| stack(Axis::Horizontal, ROW_GAP, children);
        let rows = vec![
            row(vec![
                button("Cancel", ButtonRole::Normal, ButtonStyle::Bordered, enabled).key("button"),
                button("Save", ButtonRole::Default, ButtonStyle::Bordered, enabled),
                button(
                    "Delete",
                    ButtonRole::Destructive,
                    ButtonStyle::Bordered,
                    enabled,
                ),
                button("Disabled", ButtonRole::Normal, ButtonStyle::Bordered, false),
                button("Glass", ButtonRole::Normal, ButtonStyle::Glass, enabled),
            ]),
            row(vec![
                toggle("Off", false, enabled).key("switch"),
                toggle("On", true, enabled),
                toggle("Disabled", true, false),
                check("Off", Checked::False, enabled),
                check("On", Checked::True, enabled),
                check("Mixed", Checked::Mixed, enabled),
            ]),
            row(vec![
                Element::new(RadioGroup {
                    label: "Size".into(),
                    options: vec!["Small".into(), "Medium".into(), "Large".into()],
                    selected: Some(1),
                    axis: Axis::Horizontal,
                    spacing: ROW_GAP,
                    enabled,
                    on_select: Some(Box::new(|_| ())),
                }),
                Element::new(SegmentedControl {
                    label: "View".into(),
                    segments: vec!["Icons".into(), "List".into(), "Columns".into()],
                    selected: Some(1),
                    enabled,
                    on_select: Some(Box::new(|_| ())),
                })
                .key("segments"),
            ]),
            row(vec![
                Element::new(PopUpButton {
                    label: "Sort".into(),
                    options: vec!["Name".into(), "Date Modified".into(), "Size".into()],
                    selected: 0,
                    open: false,
                    enabled,
                    on_open: Some(()),
                }),
                Element::new(Stepper {
                    label: "Copies".into(),
                    value: 2.0,
                    min: 1.0,
                    max: 9.0,
                    step: 1.0,
                    enabled,
                    on_change: Some(Box::new(|_| ())),
                }),
                Element::new(field("Name", "Your name", "")).width(FIELD_WIDTH),
                Element::new(SearchField {
                    field: field("Search", "Search", ""),
                    size: None,
                })
                .width(FIELD_WIDTH),
            ]),
            row(vec![
                Element::new(PullDownButton {
                    label: "Actions".into(),
                    actions: vec!["Duplicate".into(), "Rename".into()],
                    open: false,
                    enabled,
                    on_open: Some(()),
                }),
                Element::from(ComboBox {
                    field: field("Choice", "", "Alpha"),
                    options: vec!["Alpha".into(), "Beta".into()],
                    open: false,
                    on_open: Some(()),
                    text_size: 13.0,
                })
                .width(FIELD_WIDTH),
                Element::new(DatePicker {
                    label: "Date".into(),
                    date: CalendarDate {
                        year: 2026,
                        month: 7,
                        day: 11,
                    },
                    enabled,
                    on_change: Some(Box::new(|_| ())),
                }),
                Element::new(ColorWell {
                    label: "Colour".into(),
                    color: Color::from_rgba8([72, 196, 214, 255]),
                    enabled,
                    on_open: Some(()),
                }),
            ]),
            slider("Volume", 0.4, 0).key("slider"),
            slider("Ticks", 0.7, 5),
            Element::new(Progress {
                label: "Progress".into(),
                value: Some(0.6),
            }),
        ];
        let mut windows = window("Gallery", Size::new(640.0, 340.0), Vec::new());
        let content = Element::new(Stack {
            axis: Axis::Vertical,
            padding: WINDOW_CONTENT_INSET,
            spacing: LINE_GAP,
            align: Align::Stretch,
            children: rows,
        });
        windows[0].content = Element::new(Canvas {
            size: None,
            label: "Backdrop".into(),
            paint: Box::new(move |cx| {
                if options.busy {
                    busy_backdrop(cx);
                }
            }),
            children: vec![content],
        });
        windows
    }
}

/// A moment of a scroll the `kit/scroll` surface shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Rest,
    Stretch,
    Shown,
    Fading,
    Hidden,
}

const MS: u64 = 1_000_000;

/// The rectangle of the node with `key` in `window`, in window points.
fn rect_of<A: Application>(
    driver: &TestDriver<A>,
    window: WindowId,
    key: &'static str,
) -> Option<Rect> {
    let snapshot = driver.runtime().snapshot(window)?;
    let key = Key::from(key);
    snapshot
        .nodes
        .iter()
        .find(|n| n.key.as_ref() == Some(&key))
        .map(|n| n.rect)
}

/// The pointer events that hold `held` down, from the gallery's layout.
fn hold(driver: &TestDriver<Gallery>, window: WindowId, held: Held) -> Option<Vec<PointerEvent>> {
    let key = match held {
        Held::Button => "button",
        Held::Switch => "switch",
        Held::Slider => "slider",
        Held::Segment => "segments",
    };
    let rect = rect_of(driver, window, key)?;
    let middle = rect.center();
    let at = |x: f64| Point::new(x, middle.y);
    let (press, moves) = match held {
        Held::Button => (middle, vec![]),
        Held::Switch => (middle, vec![at(middle.x + 4.0), at(middle.x + 7.0)]),
        // the knob of the regular slider at 0.4
        Held::Slider => {
            let x = rect.min_x() + 10.0 + 0.4 * (rect.width() - 20.0);
            (at(x), vec![at(x + 1.0)])
        }
        Held::Segment => {
            let third = rect.width() / 3.0;
            (
                at(rect.min_x() + third / 2.0),
                vec![at(rect.max_x() - third / 2.0)],
            )
        }
    };
    let button = |pressed| PointerEvent::Button {
        position: press,
        button: MouseButton::Left,
        pressed,
        serial: 1,
        mods: Default::default(),
    };
    let mut events = vec![PointerEvent::Motion { position: press }, button(true)];
    events.extend(
        moves
            .into_iter()
            .map(|position| PointerEvent::Motion { position }),
    );
    Some(events)
}

/// The window of `A` drawn at rest in `look` at `scale` and `direction`, `size` points when given.
fn draw<A: Application>(
    look: Look,
    scale: f32,
    direction: LayoutDirection,
    size: Option<Size>,
) -> Result<Pixmap, String> {
    draw_with::<A>(
        look,
        scale,
        direction,
        size,
        Phase::Rest,
        |_| {},
        |_, _| None,
    )
}

/// [`draw`], the theme tuned by `tune`, then the scroll of `phase` and the frames up to its moment, then the
/// pointer events `act` gives sent to the window, and the window drawn again as they leave it.
fn draw_with<A: Application>(
    look: Look,
    scale: f32,
    direction: LayoutDirection,
    size: Option<Size>,
    phase: Phase,
    tune: impl FnOnce(&mut ThemeTokens),
    act: impl FnOnce(&TestDriver<A>, WindowId) -> Option<Vec<PointerEvent>>,
) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    options.direction = direction;
    let preferences = cupertino_kit::app::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: false,
    });
    let theme = Theme::load().map_err(|e| e.to_string())?;
    let mut tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    tune(&mut tokens);
    options.theme = Arc::new(tokens);
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<A>::new(
        Capabilities {
            raised_layer: LAYERED.with(Cell::get),
            ..Capabilities::default()
        },
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
    let window = driver.runtime().windows()[0].1;
    let system = driver.window_system_mut();
    system
        .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system
        .configure(window, size, active)
        .map_err(|e| fail(&e))?;
    let mut rested = false;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            rested = true;
            break;
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    if !rested {
        return Err("the window keeps asking for frames".into());
    }
    let scroll = |dy: f64, source: ScrollSource| {
        WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: Point::new(120.0, 100.0),
                delta: Point::new(0.0, dy),
                value120: None,
                source,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(0),
        )
    };
    // the moment to show, after the events that lead to it
    let (events, at): (Vec<(u64, WindowEvent)>, u64) = match phase {
        Phase::Rest => (Vec::new(), 0),
        Phase::Stretch => (
            (1..=8)
                .map(|i| (i * 10, scroll(-30.0, ScrollSource::Finger)))
                .collect(),
            100,
        ),
        Phase::Shown => (vec![(10, scroll(120.0, ScrollSource::Wheel))], 300),
        Phase::Fading => (vec![(10, scroll(120.0, ScrollSource::Wheel))], 1_160),
        Phase::Hidden => (vec![(10, scroll(120.0, ScrollSource::Wheel))], 1_600),
    };
    let mut last = 0;
    for (ms, event) in events {
        last = ms;
        driver
            .advance_to(Timestamp(ms * MS))
            .map_err(|e| fail(&e))?;
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .map_err(|e| fail(&e))?;
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if system.frame_requested(window).map_err(|e| fail(&e))? {
            system
                .frame_ready(window, Timestamp(ms * MS))
                .map_err(|e| fail(&e))?;
            driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        }
    }
    if phase != Phase::Rest {
        // frames up to the moment, one every 16 ms after the last event
        let mut ms = last + 16;
        while ms <= at {
            driver
                .advance_to(Timestamp(ms * MS))
                .map_err(|e| fail(&e))?;
            driver.run_until_idle(turns).map_err(|e| fail(&e))?;
            let system = driver.window_system_mut();
            if system.frame_requested(window).map_err(|e| fail(&e))? {
                system
                    .frame_ready(window, Timestamp(ms * MS))
                    .map_err(|e| fail(&e))?;
                driver.run_until_idle(turns).map_err(|e| fail(&e))?;
            }
            ms += 16;
        }
    }
    if let Some(events) = act(&driver, window) {
        let system = driver.window_system_mut();
        for event in events {
            system
                .enqueue_window(window, WindowEvent::Pointer(event, Timestamp(0)))
                .map_err(|e| fail(&e))?;
        }
        // a held control may animate towards its held look: let the clock run until it settles
        for frame in 1..=FRAMES as u64 {
            driver.run_until_idle(turns).map_err(|e| fail(&e))?;
            let at = Timestamp(frame * SETTLE_STEP);
            driver.advance_to(at).map_err(|e| fail(&e))?;
            let system = driver.window_system_mut();
            if !system.frame_requested(window).map_err(|e| fail(&e))? {
                break;
            }
            system.frame_ready(window, at).map_err(|e| fail(&e))?;
        }
    }
    let system = driver.runtime().window_system();
    let frame = system
        .frames(window)
        .map_err(|e| fail(&e))?
        .last()
        .ok_or("the window presented no frame")?;
    // the raised layer over the content, as the compositor shows them
    let mut pixels = match system.composited(window) {
        Ok(Some(composited)) => composited,
        _ => frame.pixels.clone(),
    };
    if frame.format == PixelFormat::Rgba8 {
        for px in pixels.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    let size = IntSize::from_wh(frame.width, frame.height).ok_or("the frame is empty")?;
    Pixmap::from_vec(pixels, size).ok_or_else(|| "the frame's pixels do not fill it".to_string())
}

/// The Editor example with a short document typed into it, since it opens empty.
struct SampleEditor;

impl Application for SampleEditor {
    type Model = editor::Document;
    type Message = editor::Message;

    fn init() -> (editor::Document, Vec<Command<editor::Message>>) {
        let (mut document, commands) = editor::Editor::init();
        let text = "Meeting notes\n\nThe editor scrolls, selects, copies and undoes.\nThis line was typed second.";
        editor::Editor::update(
            &mut document,
            editor::Message::Edited(TextChange {
                text: text.into(),
                revision: 1,
            }),
        );
        (document, commands)
    }

    fn update(
        document: &mut editor::Document,
        message: editor::Message,
    ) -> Vec<Command<editor::Message>> {
        editor::Editor::update(document, message)
    }

    fn view(document: &editor::Document) -> Vec<Window<editor::Message>> {
        editor::Editor::view(document)
    }
}

struct Capture;

impl App for Capture {
    const NAME: &'static str = "kit";
    const STATE_FLAGS: &'static [&'static str] = &[
        "direction",
        "phase",
        "state",
        "control-size",
        "compact",
        "backdrop",
    ];

    fn surfaces(&self) -> Vec<String> {
        [
            "controls",
            "gallery",
            "icons",
            "sidebar",
            "tooltip",
            "counter",
            "forms",
            "virtual-list",
            "editor",
            "scroll",
            "under-toolbar",
        ]
        .map(String::from)
        .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        let state = |key: &str, value: &str| vec![(key.to_string(), value.to_string())];
        if surface == "under-toolbar" {
            return ["rest", "shown", "hidden"]
                .map(|p| state("phase", p))
                .to_vec();
        }
        if surface == "scroll" {
            return ["rest", "stretch", "shown", "fading", "hidden"]
                .map(|p| state("phase", p))
                .to_vec();
        }
        if surface == "gallery" {
            return vec![
                State::new(),
                state("state", "held-button"),
                state("state", "held-switch"),
                state("state", "held-slider"),
                state("state", "held-segment"),
                state("state", "disabled"),
            ];
        }
        vec![State::new(), state("direction", "rtl")]
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        let size = r.size.map(|(w, h)| Size::new(w, h));
        let mut direction = LayoutDirection::Ltr;
        let mut phase = Phase::Rest;
        let mut gallery = GalleryOptions::default();
        for (flag, value) in r.state {
            let wrong = || {
                let choices = match flag.as_str() {
                    "direction" => "ltr or rtl",
                    "state" => {
                        "rest, held-button, held-switch, held-slider, held-segment or disabled"
                    }
                    "control-size" => "mini, small, regular, large or extra-large",
                    "compact" => "on or off",
                    "phase" => "rest, stretch, shown, fading or hidden",
                    _ => "flat or busy",
                };
                Err(format!("--{flag} {value}: {choices}"))
            };
            match (flag.as_str(), value.as_str()) {
                ("direction", "ltr") => direction = LayoutDirection::Ltr,
                ("direction", "rtl") => direction = LayoutDirection::Rtl,
                ("state", "rest") => {}
                ("state", "held-button") => gallery.held = Some(Held::Button),
                ("state", "held-switch") => gallery.held = Some(Held::Switch),
                ("state", "held-slider") => gallery.held = Some(Held::Slider),
                ("state", "held-segment") => gallery.held = Some(Held::Segment),
                ("state", "disabled") => gallery.disabled = true,
                ("control-size", "mini") => gallery.size = ControlSize::Mini,
                ("control-size", "small") => gallery.size = ControlSize::Small,
                ("control-size", "regular") => gallery.size = ControlSize::Regular,
                ("control-size", "large") => gallery.size = ControlSize::Large,
                ("control-size", "extra-large") => gallery.size = ControlSize::ExtraLarge,
                ("compact", "on") => gallery.compact = true,
                ("compact", "off") => gallery.compact = false,
                ("backdrop", "flat") => gallery.busy = false,
                ("backdrop", "busy") => gallery.busy = true,
                ("phase", "rest") => phase = Phase::Rest,
                ("phase", "stretch") => phase = Phase::Stretch,
                ("phase", "shown") => phase = Phase::Shown,
                ("phase", "fading") => phase = Phase::Fading,
                ("phase", "hidden") => phase = Phase::Hidden,
                _ => return wrong(),
            }
        }
        match r.surface {
            "controls" => draw::<Controls>(r.look, r.scale, direction, size),
            "gallery" => {
                GALLERY.with(|g| g.set(gallery));
                draw_with::<Gallery>(
                    r.look,
                    r.scale,
                    direction,
                    size,
                    Phase::Rest,
                    |theme| theme.metrics = theme.metrics.at_size(gallery.size, gallery.compact),
                    |driver, window| hold(driver, window, gallery.held?),
                )
            }
            "counter" => draw::<counter::Counter>(r.look, r.scale, direction, size),
            "forms" => draw::<forms::Forms>(r.look, r.scale, direction, size),
            "sidebar" => draw::<Places>(r.look, r.scale, direction, size),
            "tooltip" => draw::<Tips>(r.look, r.scale, direction, size),
            "virtual-list" => {
                draw::<virtual_list::VirtualList<50_000>>(r.look, r.scale, direction, size)
            }
            "editor" => draw::<SampleEditor>(r.look, r.scale, direction, size),
            "scroll" => {
                draw_with::<Scrolled>(r.look, r.scale, direction, size, phase, |_| {}, |_, _| None)
            }
            "under-toolbar" => {
                LAYERED.with(|l| l.set(true));
                let drawn = draw_with::<UnderToolbar>(
                    r.look,
                    r.scale,
                    direction,
                    size,
                    phase,
                    |_| {},
                    |_, _| None,
                );
                LAYERED.with(|l| l.set(false));
                drawn
            }
            _ => draw::<Icons>(r.look, r.scale, direction, size),
        }
    }
}

fn main() -> ExitCode {
    cupertino_capture::run(|| cupertino_capture::assets().map(|_| Capture))
}
