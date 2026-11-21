//! The desktop-control side of the program: the server's consent authority (`cupertino_kit_shell::consent`, bound
//! with the `consent` role), the request it asks, and what the person does in its alert. The server asks one run at a
//! time and keeps the answer; the alert shows while that run waits for it and goes when the server says the run is
//! no longer waiting. The app's icon is read on Kit's I/O pool, and the arming delay is a timer future of the loop that
//! starts when the alert presents its first frame. While the session is away nothing is asked: a request is dismissed,
//! so nothing is remembered for an answer nobody gave.

use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_icons::ThemeIcons;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Command, WindowKey};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::task;
use cupertino_kit_shell::consent::{self, Answer, Event, Request, Run};

use crate::consent::Arming;
use crate::desktop_control::{ControlChoice, ControlRequest, RequestingApp};
use crate::program::{DesktopControlInput, Message};
use crate::view;

/// Device pixels of the app icon renditions the alert loads: its icon at scales 1 and 2.
const ICON_PX: [u32; 2] = [view::SYMBOL as u32, 2 * view::SYMBOL as u32];

/// How long loading an app's icon should take on the I/O pool: its desktop entry and a few icon files.
const ICON_DEADLINE: Duration = Duration::from_secs(5);

/// The phase of a run that waits for the person's answer.
const ASKING: u32 = 1;

/// The request the alert asks about.
#[derive(Debug)]
struct Panel {
    run: Run,
    request: ControlRequest,
    /// From the alert's first presented frame; `None` before it.
    arming: Option<Arming>,
    /// The program heard the arming delay pass.
    armed: bool,
}

impl Panel {
    fn ready(&self, now: Instant) -> bool {
        self.armed && self.arming.is_some_and(|a| a.armed(now))
    }
}

#[derive(Debug, Default)]
pub struct DesktopControl {
    panel: Option<Panel>,
    away: bool,
}

/// The icon of the desktop entry `identity`, loaded on the I/O pool for the run `run`.
fn load_icons(run: Run, identity: String) -> Command<Message> {
    task::io(
        ICON_DEADLINE,
        move || {
            let mut icons = ThemeIcons::new(&Assets::discover());
            ICON_PX
                .iter()
                .filter_map(|&px| icons.load_for_entry(&identity, px))
                .map(|pixmap| ImageSource {
                    id: next_resource_id(),
                    image: ImageResource {
                        revision: 1,
                        width: pixmap.width(),
                        height: pixmap.height(),
                        stride: pixmap.width() * 4,
                        format: PixelFormat::Rgba8Premultiplied,
                        pixels: pixmap.take().into(),
                    },
                })
                .collect()
        },
        move |icons| Message::DesktopControl(DesktopControlInput::Icons { run, icons }),
    )
}

impl DesktopControl {
    /// Become the session's consent authority: the server sends its requests from now on.
    pub fn bind() -> DesktopControl {
        consent::bind();
        DesktopControl::default()
    }

    /// What the consent authority said.
    pub fn heard(&mut self, event: Event) -> Vec<Command<Message>> {
        match event {
            Event::Requested(request) => self.requested(request),
            Event::Changed { run, phase } => {
                // the alert asks only while the run waits for the answer
                if phase != ASKING && self.panel.as_ref().is_some_and(|p| p.run == run) {
                    self.panel = None;
                }
                Vec::new()
            }
        }
    }

    fn requested(&mut self, request: Request) -> Vec<Command<Message>> {
        if self.away {
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
        self.panel = Some(Panel {
            run,
            request: ControlRequest {
                output: request.output,
                capabilities: request.capabilities,
                app,
            },
            arming: None,
            armed: false,
        });
        icons.into_iter().collect()
    }

    /// The session went away or came back. Going away dismisses the request asked.
    pub fn set_away(&mut self, away: bool) {
        self.away = away;
        if away {
            if let Some(panel) = self.panel.take() {
                consent::answer(panel.run, Answer::Dismiss);
            }
        }
    }

    /// Answer the request asked with `choice`. Allow is taken only once the alert is armed; the alert stays until the
    /// server says the run no longer waits, so an answer the server does not take can be given again.
    fn answer(&mut self, choice: ControlChoice) {
        let Some(panel) = &self.panel else {
            return;
        };
        let answer = match choice {
            ControlChoice::Allow if !panel.ready(Instant::now()) => return,
            ControlChoice::Allow => Answer::Allow,
            ControlChoice::DontAllow => Answer::Deny,
            ControlChoice::Dismiss => Answer::Dismiss,
        };
        consent::answer(panel.run, answer);
    }

    /// Escape, or the server closed the alert.
    pub fn dismiss(&mut self) {
        self.answer(ControlChoice::Dismiss);
    }

    /// The alert of the request asked opened as `window`: the person answers on its surface, and its arming starts
    /// with its first presented frame.
    pub fn opened(&mut self, window: WindowKey) -> Vec<Command<Message>> {
        let Some(panel) = &mut self.panel else {
            return Vec::new();
        };
        panel.arming = None;
        panel.armed = false;
        consent::set_window(&window);
        vec![Command::OnPresented {
            window,
            message: Message::DesktopControl(DesktopControlInput::Presented(panel.run)),
        }]
    }

    pub fn input(&mut self, input: DesktopControlInput) -> Vec<Command<Message>> {
        match input {
            DesktopControlInput::Allow => self.answer(ControlChoice::Allow),
            DesktopControlInput::DontAllow => self.answer(ControlChoice::DontAllow),
            DesktopControlInput::Presented(run) => {
                let now = Instant::now();
                if let Some(panel) = self
                    .panel
                    .as_mut()
                    .filter(|p| p.run == run && p.arming.is_none())
                {
                    let arming = Arming::new(now);
                    panel.arming = Some(arming);
                    let wait = arming.remaining(now);
                    return vec![Command::Perform(Box::pin(async move {
                        async_io::Timer::after(wait).await;
                        Message::DesktopControl(DesktopControlInput::Armed(run))
                    }))];
                }
            }
            DesktopControlInput::Armed(run) => {
                if let Some(panel) = self.panel.as_mut().filter(|p| p.run == run) {
                    panel.armed = true;
                }
            }
            DesktopControlInput::Icons { run, icons } => {
                if let Some(app) = self
                    .panel
                    .as_mut()
                    .filter(|p| p.run == run)
                    .and_then(|p| p.request.app.as_mut())
                {
                    app.icon = icons;
                }
            }
        }
        Vec::new()
    }

    /// The run of the request to show, none while the session is away.
    pub fn shown_about(&self) -> Option<String> {
        self.panel
            .as_ref()
            .filter(|_| !self.away)
            .map(|panel| format!("desktop control {:?}", panel.run))
    }

    /// The output the run would control, by its name.
    pub fn output(&self) -> Option<&str> {
        self.panel.as_ref().map(|p| p.request.output.as_str())
    }

    /// What the alert on screen shows.
    pub fn view(&self) -> Option<view::desktop_control::DesktopControl> {
        let panel = self.panel.as_ref().filter(|_| !self.away)?;
        Some(view::desktop_control::DesktopControl {
            request: panel.request.clone(),
            armed: panel.ready(Instant::now()),
        })
    }
}
