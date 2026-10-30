//! A list of `COUNT` two-line rows in a scrolling list with multiple selection, a status line counting the selected
//! rows, and a button that reverses the order and keeps the same items selected.

use cupertino_kit::controls::{ListView, Selection, TextRole};
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

/// Fits a body line over a caption line; an approximate design value.
pub const ROW_HEIGHT: f64 = 36.0;
/// Around the status line; an approximate design value.
const STATUS_INSETS: Insets = Insets {
    top: 6.0,
    left: 12.0,
    bottom: 6.0,
    right: 12.0,
};

pub struct VirtualList<const COUNT: usize>;

pub struct Items {
    pub scroll: ScrollState,
    pub selection: Selection,
    pub descending: bool,
    pub opened: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Scroll(ScrollState),
    Select(Selection),
    Open(usize),
    Reverse,
}

/// `n` with its thousands grouped by commas.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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

impl<const COUNT: usize> VirtualList<COUNT> {
    /// The item shown at `index`, numbered from 1.
    fn item(descending: bool, index: usize) -> usize {
        if descending {
            COUNT - index
        } else {
            index + 1
        }
    }

    /// The same items' indices once the order is reversed.
    fn reversed(selection: &Selection) -> Selection {
        let flip = |i: usize| COUNT - 1 - i;
        Selection::new(
            selection
                .ranges()
                .iter()
                .map(|r| COUNT - r.end..COUNT - r.start),
            selection.cursor().map(flip),
            selection.anchor().map(flip),
        )
    }
}

impl<const COUNT: usize> Application for VirtualList<COUNT> {
    type Model = Items;
    type Message = Message;

    fn init() -> (Items, Vec<Command<Message>>) {
        let items = Items {
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            selection: Selection::default(),
            descending: false,
            opened: None,
        };
        (items, Vec::new())
    }

    fn update(items: &mut Items, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => items.scroll = scroll,
            Message::Select(selection) => items.selection = selection,
            Message::Open(index) => items.opened = Some(Self::item(items.descending, index)),
            Message::Reverse => {
                items.selection = Self::reversed(&items.selection);
                items.descending = !items.descending;
            }
        }
        Vec::new()
    }

    fn view(items: &Items) -> Vec<Window<Message>> {
        let descending = items.descending;
        let list = Element::from(ListView {
            label: "Items".into(),
            count: COUNT,
            row_height: ROW_HEIGHT,
            scroll: items.scroll,
            selection: items.selection.clone(),
            multiple: true,
            row: Box::new(move |index, selected| {
                let item = Self::item(descending, index);
                Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: Insets::ZERO,
                    spacing: 0.0,
                    align: Align::Start,
                    children: vec![
                        text(
                            format!("Item {}", grouped(item)),
                            TextRole::Body,
                            false,
                            selected,
                        ),
                        text(
                            format!("Row {} of {}", grouped(index + 1), grouped(COUNT)),
                            TextRole::Caption,
                            true,
                            selected,
                        ),
                    ],
                })
            }),
            type_select: Some(Box::new(move |index| {
                format!("Item {}", grouped(Self::item(descending, index)))
            })),
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
            on_activate: Some(Box::new(Message::Open)),
        })
        .key("list");
        let selected = match items.selection.len() {
            0 => "No selection".to_owned(),
            n => format!("{} of {} selected", grouped(n), grouped(COUNT)),
        };
        let status = match items.opened {
            Some(item) => format!("{selected}. Opened Item {}", grouped(item)),
            None => selected,
        };
        let bar = Element::new(Stack {
            axis: Axis::Horizontal,
            padding: STATUS_INSETS,
            spacing: 8.0,
            align: Align::Center,
            children: vec![
                text(status, TextRole::Caption, true, false).key("status"),
                Element::new(Spacer { min: 0.0 }),
                Element::new(Button {
                    label: "Reverse Order".into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(Message::Reverse),
                })
                .key("reverse"),
            ],
        });
        vec![Window {
            key: WindowKey("virtual-list".into()),
            spec: WindowSpec {
                title: "Virtual List".into(),
                app_id: "org.cupertino.examples.VirtualList".into(),
                size: Size::new(480.0, 600.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![
                    list,
                    Element::new(Separator {
                        axis: Axis::Horizontal,
                    }),
                    bar,
                ],
            }),
        }]
    }
}
