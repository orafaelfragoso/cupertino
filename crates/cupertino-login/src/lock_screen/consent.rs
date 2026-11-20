//! The desktop-control consent panel, which the lock screen shows until `cupertino-prompts` takes it (W6.14): the
//! server gives the consent authority to the `session_lock` role. A request that comes while the session is locked is
//! dismissed, so nothing is remembered for an answer nobody gave.

use super::{Message, Model};
use crate::view::{ControlChoice, ControlRequest, RequestingApp};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Command, Key, WindowKey};
use cupertino_kit_shell::consent::{self, Answer, Event, Request, Run};
use cupertino_kit_shell::{outputs, Anchor, Keyboard, Layer, LayerSpec, Margin};
use std::time::Duration;

/// Device pixels of the app icon renditions the panel loads: its 64 pt icon at scales 1 and 2.
const ICON_PX: [u32; 2] = [64, 128];

/// How long loading an app's icon should take on the I/O pool: its desktop entry and a few icon files.
const ICON_DEADLINE: Duration = Duration::from_secs(5);

/// The layer namespace of the panel.
const NAMESPACE: &str = "desktop-control-consent";

/// The phase of a run that waits for the person's answer.
const ASKING: u32 = 1;

/// A request the panel asks about.
#[derive(Debug)]
pub struct Panel {
    pub run: Run,
    request: ControlRequest,
    phase: u32,
}

impl Panel {
    /// What the panel asks.
    pub fn request(&self) -> &ControlRequest {
        &self.request
    }

    /// Whether the panel shows: while the run waits for the answer.
    pub fn shown(&self) -> bool {
        self.phase == ASKING
    }

    /// The app's icon, once loaded.
    pub fn set_icons(&mut self, icons: Vec<ImageSource>) {
        if let Some(app) = &mut self.request.app {
            app.icon = icons;
        }
    }
}

/// The panel's window.
pub fn key() -> WindowKey {
    WindowKey(Key::from("desktop-control"))
}

/// The panel's layer surface: centred over everything on the output the run would control (else the first one), with
/// the keyboard while it asks.
pub fn layer(panel: Option<&Panel>) -> LayerSpec {
    let all = outputs::all();
    let output = panel
        .and_then(|p| {
            all.iter()
                .find(|o| o.name.as_deref() == Some(p.request.output.as_str()))
        })
        .or_else(|| all.iter().find(|o| o.geometry.is_some()))
        .map(|o| o.id);
    LayerSpec {
        namespace: NAMESPACE,
        output,
        layer: Layer::Overlay,
        anchor: Anchor::default(),
        size: crate::view::control_size(ASKING),
        margin: Margin::default(),
        exclusive_zone: -1,
        keyboard: Keyboard::Exclusive,
    }
}

/// What the consent authority said.
pub fn heard(model: &mut Model, event: Event) -> Vec<Command<Message>> {
    match event {
        Event::Requested(request) => requested(model, request),
        Event::Changed { run, phase } => {
            if model.consent.as_ref().is_some_and(|p| p.run == run) {
                if matches!(phase, 0 | 4) {
                    model.consent = None;
                } else if let Some(panel) = &mut model.consent {
                    panel.phase = phase;
                }
            }
            Vec::new()
        }
    }
}

fn requested(model: &mut Model, request: Request) -> Vec<Command<Message>> {
    if model.lock.is_some() {
        // nobody can answer now: this request is refused, nothing is remembered
        consent::answer(request.run, Answer::Dismiss);
        return Vec::new();
    }
    let run = request.run;
    let app = request.app.map(|app| RequestingApp {
        name: app.name,
        identity: app.identity,
        desktop: app.desktop_entry,
        services: app.services,
        remembered: app.remembered,
        icon: Vec::new(),
    });
    let icons = app
        .as_ref()
        .filter(|app| app.desktop)
        .map(|app| load_icons(run, app.identity.clone()));
    model.consent = Some(Panel {
        run,
        request: ControlRequest {
            output: request.output,
            capabilities: request.capabilities,
            app,
        },
        phase: ASKING,
    });
    consent::set_window(&key());
    icons.into_iter().collect()
}

/// The person's choice on the panel.
pub fn answer(model: &Model, choice: ControlChoice) {
    if let Some(panel) = &model.consent {
        consent::answer(
            panel.run,
            match choice {
                ControlChoice::Allow => Answer::Allow,
                ControlChoice::DontAllow => Answer::Deny,
                ControlChoice::Dismiss => Answer::Dismiss,
            },
        );
    }
}

/// The icon of the desktop entry `identity`, loaded on the I/O pool for the run `run`.
fn load_icons(run: Run, identity: String) -> Command<Message> {
    cupertino_kit::task::io(
        ICON_DEADLINE,
        move || {
            let mut icons = cupertino_icons::ThemeIcons::new(&cupertino_assets::Assets::discover());
            ICON_PX
                .iter()
                .filter_map(|&px| icons.load_for_entry(&identity, px))
                .map(|pixmap| ImageSource {
                    id: cupertino_kit::core::next_resource_id(),
                    image: cupertino_kit::render::ImageResource {
                        revision: 1,
                        width: pixmap.width(),
                        height: pixmap.height(),
                        stride: pixmap.width() * 4,
                        format: cupertino_kit::render::PixelFormat::Rgba8Premultiplied,
                        pixels: pixmap.take().into(),
                    },
                })
                .collect()
        },
        move |icons| Message::ConsentIcons { run, icons },
    )
}
