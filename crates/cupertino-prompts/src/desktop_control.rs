//! The desktop-control consent question: whether an app may control or capture the desktop, which the server asks
//! the program that holds its consent authority (`cupertino_control_authority_v1`, through
//! `cupertino_kit_shell::consent`). The server decides when to ask, one run at a time, and keeps the answer; this
//! program shows the question and says what the person answered. The texts the alert says are made here from the
//! request alone.

use cupertino_kit::controls::ImageSource;

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
    /// The app's icon at the sizes there are; none draws the applications symbol.
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
