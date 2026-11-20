//! The consent panel of desktop control: the alert the lock screen shows when an app asks to control or capture the
//! desktop, its request and the person's choice, and the texts it says.

use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, Image, ImageFit, ImageSource, Stack, TextRole,
};
use cupertino_kit::core::{Element, Role, Window, WindowBackground, WindowKey};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::text::FontWeight;

use super::widgets::{AlertBody, Backdrop, Place, ESCAPE};
use super::{full_screen, icon_disc, panel_text, Input};

pub const CONTROL_APP_ID: &str = "org.cupertino.login.desktop-control";
/// The consent panel's surface: the alert's width, and a height that holds the alert with the longest name, path and
/// list of rights, the alert centred in it and the rest clear.
pub const CONTROL_WIDTH: f64 = 260.0;
pub const CONTROL_HEIGHT: f64 = 520.0;
/// The alert: its corner radius and its padding; the icon and the text sit further in than the buttons.
const CONTROL_RADIUS: f64 = 26.0;
const CONTROL_PADDING: Insets = Insets {
    top: 22.0,
    left: 16.0,
    bottom: 16.0,
    right: 16.0,
};
const CONTROL_TEXT_INSET: f64 = 6.0;
/// The app's icon, and the room under it, under the title and above the buttons.
const CONTROL_ICON: f64 = 64.0;
const CONTROL_ICON_GAP: f64 = 14.0;
const CONTROL_TITLE_GAP: f64 = 10.0;
const CONTROL_BUTTONS_GAP: f64 = 16.0;
/// The two capsule buttons side by side, this far apart, filling the alert's width between its insets.
const CONTROL_BUTTON_GAP: f64 = 8.0;
const CONTROL_BUTTON_HEIGHT: f64 = 28.0;
const CONTROL_BUTTON_WIDTH: f64 = (CONTROL_WIDTH - 16.0 - 16.0 - CONTROL_BUTTON_GAP) / 2.0;
/// The size of the surface: the consent alert while a request waits, the status panel with Stop while a run is on.
pub fn control_size(phase: u32) -> Size {
    if phase == 1 {
        Size::new(CONTROL_WIDTH, CONTROL_HEIGHT)
    } else {
        Size::new(CONTROL_STATUS_WIDTH, CONTROL_STATUS_HEIGHT)
    }
}
/// The status panel's size while a run is on: the session agent's interrupt panel's, 413 by 128.
const CONTROL_STATUS_WIDTH: f64 = 413.0;
const CONTROL_STATUS_HEIGHT: f64 = 128.0;
/// Characters of an app's name the panel shows; a longer name is cut with an ellipsis.
const CONTROL_NAME_CHARS: usize = 48;
/// Characters of an executable's path the panel shows; a longer path keeps its end, which names the program.
const CONTROL_PATH_CHARS: usize = 120;
/// Characters of an output's name the panel shows.
const CONTROL_OUTPUT_CHARS: usize = 64;
/// Service bits of an app's request: desktop control and screen capture.
const SERVICE_CONTROL: u32 = 1;
const SERVICE_CAPTURE: u32 = 2;

/// What the consent panel asks the person.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlRequest {
    /// The output the run would control.
    pub output: String,
    /// The capability bits the run asks for: observe 1, capture 2, pointer 4, keyboard 8, native controls 16.
    pub capabilities: u32,
    /// The app asking; `None` for a controller that does not name it, whose answer holds for this run only.
    pub app: Option<RequestingApp>,
}

/// The app a consent request comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestingApp {
    /// The desktop entry's name, or the executable's file name.
    pub name: String,
    /// The desktop entry id, or the executable's absolute path.
    pub identity: String,
    /// Whether `identity` is a desktop entry id.
    pub desktop: bool,
    /// The services asked: desktop control 1, screen capture 2.
    pub services: u32,
    /// Whether the answer is remembered for the app.
    pub remembered: bool,
    /// The app's icon at the sizes there are; none draws the plain disc.
    pub icon: Vec<ImageSource>,
}

/// The person's choice on the consent panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlChoice {
    /// Allow, remembered for the app.
    Allow,
    /// Don't Allow, remembered for the app.
    DontAllow,
    /// The panel was closed without a choice (Escape): this request only.
    Dismiss,
}

/// `text` without control or bidirectional formatting characters, cut at `chars` characters with an ellipsis; `keep_end`
/// keeps the end instead of the start.
fn clean(text: &str, chars: usize, keep_end: bool) -> String {
    let kept: Vec<char> = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    if kept.len() <= chars {
        return kept.into_iter().collect();
    }
    if keep_end {
        std::iter::once('\u{2026}')
            .chain(kept[kept.len() + 1 - chars..].iter().copied())
            .collect()
    } else {
        kept[..chars - 1]
            .iter()
            .copied()
            .chain(std::iter::once('\u{2026}'))
            .collect()
    }
}

/// The title and the information text of the consent panel for `request`.
pub fn control_text(request: &ControlRequest) -> (String, String) {
    let services = request.app.as_ref().map_or_else(
        || {
            let mut services = 0;
            if request.capabilities & !2 != 0 {
                services |= SERVICE_CONTROL;
            }
            if request.capabilities & 2 != 0 {
                services |= SERVICE_CAPTURE;
            }
            services
        },
        |app| app.services,
    );
    let rights: Vec<_> = [
        (1, SERVICE_CONTROL, "inspect the display"),
        (2, SERVICE_CAPTURE, "capture the display"),
        (4, SERVICE_CONTROL, "use the pointer"),
        (8, SERVICE_CONTROL, "use the keyboard"),
        (16, SERVICE_CONTROL, "use native controls"),
    ]
    .into_iter()
    .filter(|(bit, service, _)| request.capabilities & bit != 0 && services & service != 0)
    .map(|(_, _, label)| label)
    .collect();
    let display = clean(&request.output, CONTROL_OUTPUT_CHARS, false);
    let Some(app) = &request.app else {
        return (
            "Allow desktop control?".to_owned(),
            format!(
                "Allow the desktop controller to {} on {display}. This choice applies to this request only. \
                 Ctrl+Alt+Escape stops it.",
                rights.join(", ")
            ),
        );
    };
    let name = clean(&app.name, CONTROL_NAME_CHARS, false);
    let wants = match (
        services & SERVICE_CONTROL != 0,
        services & SERVICE_CAPTURE != 0,
    ) {
        (true, true) => "control and capture the desktop",
        (false, true) => "capture the screen",
        _ => "control the desktop",
    };
    let title = format!("Allow \u{201c}{name}\u{201d} to {wants}?");
    let mut info = format!("It will be able to {} on {display}.", rights.join(", "));
    if !app.desktop {
        info.push_str(&format!(
            " The program is {}.",
            clean(&app.identity, CONTROL_PATH_CHARS, true)
        ));
    }
    info.push_str(if app.remembered {
        " Your choice is remembered for this app; change it any time in Settings \u{203a} Privacy."
    } else {
        " This choice applies to this request only."
    });
    info.push_str(" Ctrl+Alt+Escape stops control.");
    (title, info)
}

/// The native consent panel, an alert on glass: the app's icon, the question, the rights and the display, and Don't
/// Allow beside Allow. Neither button has a default-key binding (Return does nothing); Don't Allow initially has the
/// focus, and Escape closes the panel without a choice.
pub fn desktop_control<M: Clone + 'static>(
    key: WindowKey,
    request: &ControlRequest,
    map: fn(Input) -> M,
) -> Window<M> {
    let (title, info) = control_text(request);
    let icon_rect = Rect::new(0.0, 0.0, CONTROL_ICON, CONTROL_ICON);
    let icon = match request.app.as_ref().filter(|a| !a.icon.is_empty()) {
        Some(app) => Element::new(Place::new(
            icon_rect,
            vec![Element::new(Image {
                sources: app.icon.clone(),
                size: icon_rect.size,
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            })],
        )),
        None => icon_disc(icon_rect),
    };
    let stack = |axis, spacing, align, padding, children| {
        Element::new(Stack {
            axis,
            padding,
            spacing,
            align,
            children,
        })
    };
    let text = stack(
        Axis::Vertical,
        CONTROL_TITLE_GAP,
        Align::Stretch,
        Insets::ZERO,
        vec![
            panel_text(&title, TextRole::Body, FontWeight::BOLD, false),
            panel_text(&info, TextRole::Body, FontWeight::REGULAR, false),
        ],
    );
    let head = stack(
        Axis::Vertical,
        CONTROL_ICON_GAP,
        Align::Stretch,
        Insets {
            top: 0.0,
            left: CONTROL_TEXT_INSET,
            bottom: 0.0,
            right: CONTROL_TEXT_INSET,
        },
        vec![
            stack(
                Axis::Horizontal,
                0.0,
                Align::Start,
                Insets::ZERO,
                vec![icon],
            ),
            text,
        ],
    );
    let buttons = [
        ("Don't Allow", ControlChoice::DontAllow),
        ("Allow", ControlChoice::Allow),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (label, choice))| {
        Element::new(Place {
            initial: index == 0,
            ..Place::filled(
                Rect::new(0.0, 0.0, CONTROL_BUTTON_WIDTH, CONTROL_BUTTON_HEIGHT),
                vec![Element::new(Button {
                    label: label.to_owned(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Pill,
                    enabled: true,
                    busy: false,
                    on_press: Some(map(Input::DesktopControl(choice))),
                })],
            )
        })
        .key(label)
    })
    .collect();
    let body = stack(
        Axis::Vertical,
        CONTROL_BUTTONS_GAP,
        Align::Stretch,
        Insets::ZERO,
        vec![
            head,
            stack(
                Axis::Horizontal,
                CONTROL_BUTTON_GAP,
                Align::Center,
                Insets::ZERO,
                buttons,
            ),
        ],
    );
    let cancel = map(Input::DesktopControl(ControlChoice::Dismiss));
    let close = cancel.clone();
    let size = Size::new(CONTROL_WIDTH, CONTROL_HEIGHT);
    let content = Element::new(Backdrop {
        ground: None,
        hidden: false,
        on_activity: None,
        keys: Box::new(move |sym, _| (sym == ESCAPE).then(|| cancel.clone())),
        label: title.clone(),
        description: None,
        role: Role::AlertDialog,
        modal: true,
        children: vec![Element::new(Place::new(
            Rect::new(0.0, 0.0, size.width, size.height),
            vec![Element::new(AlertBody {
                width: CONTROL_WIDTH,
                height: None,
                radius: CONTROL_RADIUS,
                padding: CONTROL_PADDING,
                children: vec![body],
            })],
        ))],
    });
    Window {
        background: WindowBackground::None,
        ..full_screen(
            key,
            CONTROL_APP_ID.to_owned(),
            &title,
            size,
            content,
            Some(close),
        )
    }
}
