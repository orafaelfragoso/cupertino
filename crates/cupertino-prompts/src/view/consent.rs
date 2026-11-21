//! The consent alerts: whether an application may take a screenshot, and whether it may use keyboard shortcuts while
//! another application has the keyboard. Each is the prompts' alert with the application's icon where the lock symbol
//! goes, the question in bold, the shortcuts in a list that scrolls past its first lines, and the deny and allow
//! buttons. The keyboard focus starts on the deny button ([`Deny`]), and the allow button takes a press only once the
//! alert is armed ([`crate::consent::ARMING`]).

use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, Icon, IconDirection, Image, ImageFit, ImageSource, ScrollView,
    Scroller,
};
use cupertino_kit::core::{Element, Key, Window, WindowKey};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis, ScrollState};
use cupertino_kit::text::FontWeight;
use cupertino_kit::theme::ColorRole;

use super::widgets::{Deny, Sized};
use super::{
    alert, head_with, stack, text, BUTTON_GAP, BUTTON_HEIGHT, BUTTON_WIDTH, CAPTION_SIZE, PADDING,
    SYMBOL, TEXT_INSET, WIDTH,
};
use crate::consent::{Question, Row};
use crate::program::{ConsentInput, Message};

/// The alerts' titles, which assistive technology hears and the window is named by.
pub const SCREENSHOT_TITLE: &str = "Screenshot";
pub const SHORTCUTS_TITLE: &str = "Keyboard Shortcuts";
/// The buttons.
pub const CANCEL: &str = "Cancel";
pub const TAKE_SCREENSHOT: &str = "Take Screenshot";
pub const DONT_ALLOW: &str = "Don\u{2019}t Allow";
pub const ALLOW: &str = "Allow";
/// The symbol drawn for an application whose icon is not found.
pub const APPLICATION_SYMBOL: &str = "symbol.applications";
/// The surfaces the consent alerts are centred in: tall enough for the longest question and, for the shortcuts, the
/// list at its tallest; the rest is clear.
pub const SCREENSHOT_SIZE: Size = Size::new(WIDTH, 380.0);
pub const SHORTCUTS_SIZE: Size = Size::new(WIDTH, 620.0);
/// The shortcut list's text: the caption size, this many points a line and this far between rows.
const LIST_LINE: f64 = 14.0;
const ROW_GAP: f64 = 4.0;
/// Characters of a row that fit on one line of the list, an estimate the list's height is grown by.
const LIST_LINE_CHARS: usize = 38;
/// The list grows by the lines of its rows up to this many lines (a row counts at most three); the rest scroll.
const LIST_LINES: usize = 10;
/// Characters of a button's label that fit in a capsule of the side-by-side pair; with a longer label the buttons
/// stand one above the other across the alert's width, the allow button first.
const SIDE_BY_SIDE_CHARS: usize = 12;

/// What a consent alert shows.
#[derive(Clone, Debug)]
pub struct Consent {
    pub question: Question,
    /// The application's icon at the symbol's size and twice it; `None` draws [`APPLICATION_SYMBOL`].
    pub icon: Option<Vec<ImageSource>>,
    /// An answer that grants is taken now.
    pub armed: bool,
    /// Where the shortcut list is scrolled to.
    pub scroll: ScrollState,
}

/// What the screenshot alert asks: the app by its name, else an unnamed app.
pub fn screenshot_prompt(app_name: Option<&str>) -> String {
    match app_name {
        Some(name) => format!("\u{201c}{name}\u{201d} wants to take a screenshot of your screen."),
        None => "An application wants to take a screenshot of your screen.".to_owned(),
    }
}

/// What the shortcuts alert asks: the app by its name, else an unnamed app; more shortcuts when `allowed` were allowed
/// before.
pub fn shortcuts_prompt(app_name: Option<&str>, allowed: u32) -> String {
    let app = match app_name {
        Some(name) => format!("\u{201c}{name}\u{201d}"),
        None => "An application".to_owned(),
    };
    if allowed > 0 {
        format!("{app} wants to use more keyboard shortcuts, also while you work in other apps.")
    } else {
        format!("{app} wants to use these keyboard shortcuts, also while you work in other apps.")
    }
}

/// One row of the shortcut list: the keys, what the shortcut does, and the portal's note when it gets no key.
pub fn row_text(row: &Row) -> String {
    if row.note.is_empty() {
        format!("{}  {}", row.keys, row.description)
    } else {
        format!("{}  {} ({})", row.keys, row.description, row.note)
    }
}

/// The application's icon, or the applications symbol.
pub(super) fn app_icon(icon: Option<&Vec<ImageSource>>) -> Element<Message> {
    match icon {
        Some(sources) => Element::new(Image {
            sources: sources.clone(),
            size: Size::new(SYMBOL, SYMBOL),
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        }),
        None => Element::new(Icon {
            name: APPLICATION_SYMBOL.to_owned(),
            size: SYMBOL,
            direction: IconDirection::Fixed,
            label: None,
            tint: Some(ColorRole::SecondaryText),
        }),
    }
    .key("app-icon")
}

fn input(input: ConsentInput) -> Message {
    Message::Consent(input)
}

/// A capsule button `width` wide, keyed `key`.
fn pill(
    label: &str,
    width: f64,
    enabled: bool,
    press: Message,
    key: &'static str,
) -> Element<Message> {
    Element::new(Sized {
        size: Size::new(width, BUTTON_HEIGHT),
        initial: false,
        children: vec![Element::new(Button {
            label: label.to_owned(),
            role: ButtonRole::Normal,
            style: ButtonStyle::Pill,
            enabled,
            busy: false,
            on_press: Some(press),
        })],
    })
    .key(key)
}

/// The deny button, where the focus starts, and the allow button, which takes a press once the alert is armed: each
/// with its label and the message its press sends.
pub(super) fn buttons(
    (deny, denied): (&str, Message),
    (allow, allowed): (&str, Message),
    armed: bool,
) -> Element<Message> {
    let side_by_side = [deny, allow]
        .iter()
        .all(|label| label.chars().count() <= SIDE_BY_SIDE_CHARS);
    let width = if side_by_side {
        BUTTON_WIDTH
    } else {
        WIDTH - PADDING.left - PADDING.right
    };
    let deny = Element::new(Deny {
        armed,
        children: vec![pill(deny, width, true, denied, "deny")],
    });
    let allow = pill(allow, width, armed, allowed, "allow");
    if side_by_side {
        stack(
            Axis::Horizontal,
            BUTTON_GAP,
            Align::Center,
            Insets::ZERO,
            vec![deny, allow],
        )
    } else {
        stack(
            Axis::Vertical,
            BUTTON_GAP,
            Align::Center,
            Insets::ZERO,
            vec![allow, deny],
        )
    }
}

/// The height of the shortcut list for `texts`: their lines up to [`LIST_LINES`], a row that wraps by more.
fn list_height(texts: &[String]) -> f64 {
    let mut lines = 0;
    let mut rows: usize = 0;
    for text in texts {
        let wrapped = text.chars().count().div_ceil(LIST_LINE_CHARS).clamp(1, 3);
        if rows > 0 && lines + wrapped > LIST_LINES {
            break;
        }
        lines += wrapped;
        rows += 1;
    }
    lines as f64 * LIST_LINE + rows.saturating_sub(1) as f64 * ROW_GAP
}

/// Every shortcut, read before Allow: past the lines the list grows by, it scrolls.
fn shortcut_list(rows: &[Row], scroll: ScrollState) -> Element<Message> {
    let texts: Vec<String> = rows.iter().map(row_text).collect();
    let height = list_height(&texts);
    let children = texts
        .iter()
        .zip(rows)
        .enumerate()
        .map(|(i, (line, row))| {
            text(
                line,
                CAPTION_SIZE,
                FontWeight::REGULAR,
                !row.note.is_empty(),
            )
            .key(Key::from(format!("shortcut-{i}")))
        })
        .collect();
    let list = Element::from(ScrollView {
        content: stack(
            Axis::Vertical,
            ROW_GAP,
            Align::Stretch,
            Insets {
                top: 0.0,
                left: TEXT_INSET,
                bottom: 0.0,
                right: TEXT_INSET,
            },
            children,
        ),
        state: scroll,
        horizontal: false,
        vertical: true,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(|s| input(ConsentInput::Scroll(s))),
    })
    .key("shortcuts")
    .fill();
    Element::new(Sized {
        size: Size::new(WIDTH - PADDING.left - PADDING.right, height),
        initial: false,
        children: vec![list],
    })
}

/// The surface of the consent alert for `question`.
pub fn size(question: &Question) -> Size {
    match question {
        Question::Screenshot { .. } => SCREENSHOT_SIZE,
        Question::Shortcuts { .. } => SHORTCUTS_SIZE,
    }
}

/// The consent alert.
pub fn consent(key: WindowKey, prompt: &Consent) -> Window<Message> {
    let app = prompt.question.app();
    let icon = app_icon(prompt.icon.as_ref());
    let (title, heading, mut children) = match &prompt.question {
        Question::Screenshot { .. } => (
            SCREENSHOT_TITLE,
            screenshot_prompt(app.name.as_deref()),
            Vec::new(),
        ),
        Question::Shortcuts { rows, allowed, .. } => (
            SHORTCUTS_TITLE,
            shortcuts_prompt(app.name.as_deref(), *allowed),
            vec![shortcut_list(rows, prompt.scroll)],
        ),
    };
    let (deny, allow) = match &prompt.question {
        Question::Screenshot { .. } => (CANCEL, TAKE_SCREENSHOT),
        Question::Shortcuts { .. } => (DONT_ALLOW, ALLOW),
    };
    children.insert(0, head_with(icon, &heading, ""));
    children.push(buttons(
        (deny, input(ConsentInput::Deny)),
        (allow, input(ConsentInput::Allow)),
        prompt.armed,
    ));
    alert(key, title, &heading, size(&prompt.question), children)
}
