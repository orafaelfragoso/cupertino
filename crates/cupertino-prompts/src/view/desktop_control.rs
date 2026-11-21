//! The desktop-control consent alert: the app's icon where the lock symbol goes (the applications symbol until it is
//! read, or when the app names no desktop entry), the question in bold, the rights and the display under it, and Don't
//! Allow beside Allow. The keyboard focus starts on Don't Allow, and Allow takes a press only once the alert is armed
//! ([`crate::consent::ARMING`]); Escape closes it without a choice.

use cupertino_kit::core::{Window, WindowKey};
use cupertino_kit::geometry::Size;

use super::consent::{app_icon, buttons, ALLOW, DONT_ALLOW};
use super::{alert, head_with, WIDTH};
use crate::desktop_control::{control_text, ControlRequest};
use crate::program::{DesktopControlInput, Message};

/// The surface the alert is centred in: tall enough for the alert with the longest name, path and list of rights; the
/// rest is clear.
pub const SIZE: Size = Size::new(WIDTH, 520.0);

/// What the desktop-control alert shows.
#[derive(Clone, Debug)]
pub struct DesktopControl {
    pub request: ControlRequest,
    /// Allow is taken now.
    pub armed: bool,
}

fn input(input: DesktopControlInput) -> Message {
    Message::DesktopControl(input)
}

/// The desktop-control consent alert.
pub fn desktop_control(key: WindowKey, prompt: &DesktopControl) -> Window<Message> {
    let (title, info) = control_text(&prompt.request);
    let icon = prompt
        .request
        .app
        .as_ref()
        .filter(|app| !app.icon.is_empty())
        .map(|app| &app.icon);
    let children = vec![
        head_with(app_icon(icon), &title, &info),
        buttons(
            (DONT_ALLOW, input(DesktopControlInput::DontAllow)),
            (ALLOW, input(DesktopControlInput::Allow)),
            prompt.armed,
        ),
    ];
    alert(key, &title, &info, SIZE, children)
}
