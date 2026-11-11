//! The question a tile's menu asks before it does something that cannot be undone, as a popup of the bar over the tile:
//! a title, a line saying what happens and two buttons. It takes the keyboard, so Tab moves between the buttons,
//! Return chooses the one that is safe and Escape or a press outside closes it.

use cupertino_kit::controls::{Button, ButtonRole, ButtonStyle, Stack, Text, TextRole};
use cupertino_kit::core::{
    CommandRegistry, Element, Key, Popup, PopupAnchor, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::Surface;

use super::Input;
use crate::config::Orientation;

/// The panel's corner radius, the space around its parts and between them, and how far it stands off its tile.
const RADIUS: f64 = 14.5;
const PADDING: f64 = 18.0;
const SPACING: f64 = 10.0;
const BUTTON_SPACING: f64 = 8.0;
const GAP: f64 = 8.0;
/// The most the panel takes.
const MAX: Size = Size {
    width: 300.0,
    height: 220.0,
};

pub fn key() -> WindowKey {
    WindowKey(Key::from("confirm"))
}

/// What the question says.
pub struct ConfirmView<'a> {
    /// The bar's window, which the question is a popup of.
    pub parent: WindowKey,
    /// The tile it is about (`TileId.0`).
    pub tile: u64,
    pub orientation: Orientation,
    pub title: &'a str,
    pub message: &'a str,
    /// The labels of the button that closes the question and of the one that goes ahead.
    pub cancel: &'a str,
    pub confirm: &'a str,
}

fn text<M>(s: &str, role: TextRole, size: f32, secondary: bool, wrap: bool) -> Element<M>
where
    M: Clone + 'static,
{
    Element::new(Text {
        content: AttributedText::plain(s, TextStyle::new(size)),
        role,
        wrap,
        secondary,
        selected: false,
    })
}

/// The question over tile `view.tile`; `confirmed` is sent when the second button is pressed, `closed` when the first
/// is, and also on Escape and a press outside.
pub fn window<M: Clone + 'static>(view: ConfirmView<'_>, input: fn(Input) -> M) -> Window<M> {
    let anchor = Key::from(format!("tile-{}", view.tile));
    let anchor = match view.orientation {
        // over the tile: below has no room at the bottom of the output, so the popup flips
        Orientation::Bottom => PopupAnchor::Below {
            key: anchor,
            gap: GAP,
        },
        Orientation::Left | Orientation::Right => PopupAnchor::Beside {
            key: anchor,
            offset: Point::new(GAP, 0.0),
        },
    };
    let buttons = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: BUTTON_SPACING,
        align: Align::Center,
        children: vec![
            Element::new(Button {
                label: view.cancel.to_owned(),
                role: ButtonRole::Default,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(input(Input::ConfirmClosed)),
            })
            .key("cancel"),
            Element::new(Button {
                label: view.confirm.to_owned(),
                role: ButtonRole::Destructive,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(input(Input::ConfirmDone)),
            })
            .key("confirm"),
        ],
    });
    let content = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::uniform(PADDING),
        spacing: SPACING,
        align: Align::Start,
        children: vec![
            text(view.title, TextRole::Section, 13.0, false, false).key("title"),
            text(view.message, TextRole::Body, 12.0, true, true).key("message"),
            buttons.key("buttons"),
        ],
    });
    Window {
        key: key(),
        spec: WindowSpec {
            title: view.title.to_owned(),
            app_id: String::new(),
            size: MAX,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Surface {
            surface: Surface::Popover,
            radius: RADIUS,
            alpha: 1.0,
        },
        content,
        popup: Some(Popup {
            parent: view.parent,
            anchor,
            grab: true,
            on_dismiss: input(Input::ConfirmClosed),
        }),
        menus: None,
    }
}
