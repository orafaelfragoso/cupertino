//! The consent side of the program: `org.cupertino.Prompts1` on the main session bus connection, the questions the
//! portal asks through it, and what the person does in their alert. A call comes as news; the model
//! ([`crate::consent::model`]) decides, and each answer goes back to its call. The application's icon is read on Kit's
//! I/O pool, and the arming delay is a timer future of the loop that starts when the alert presents its first frame.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_icons::ThemeIcons;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Command, WindowKey};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::task;

use crate::consent::model::{Answered, Consents};
use crate::consent::{service, Choice, Request};
use crate::program::{ConsentInput, Message, News};
use crate::view;

/// How long reading an application's icon should take: a few files of an icon theme on a local disk.
const ICON_DEADLINE: Duration = Duration::from_secs(2);
/// Device pixels an application's icon is read at: the symbol's size at 1x and 2x.
const ICON_PX: [u32; 2] = [view::SYMBOL as u32, 2 * view::SYMBOL as u32];

const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

pub struct Consent {
    model: Consents,
    /// The reply of each call that waits.
    replies: HashMap<u64, async_channel::Sender<u32>>,
    calls: u64,
    /// The interface is served, or being served.
    served: bool,
    /// The applications' icons by `Icon=` name: none while it is read or when it resolves to none.
    icons: HashMap<String, Option<Vec<ImageSource>>>,
    scroll: ScrollState,
}

/// The renditions of the icon `name` from the host's icon themes. Reads files: run it on the I/O pool.
fn read_icon(name: &str) -> Option<Vec<ImageSource>> {
    let mut theme = ThemeIcons::new(&Assets::discover());
    let sources: Vec<ImageSource> = ICON_PX
        .iter()
        .filter_map(|&px| theme.load(name, px))
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
        .collect();
    (!sources.is_empty()).then_some(sources)
}

impl Default for Consent {
    fn default() -> Consent {
        Consent::new()
    }
}

impl Consent {
    pub fn new() -> Consent {
        Consent {
            model: Consents::new(),
            replies: HashMap::new(),
            calls: 0,
            served: false,
            icons: HashMap::new(),
            scroll: NO_SCROLL,
        }
    }

    /// Serve the interface on `session`, the main session bus connection, asking who calls on `lookup`, once both
    /// are there.
    pub fn serve(
        &mut self,
        session: Option<&zbus::Connection>,
        lookup: Option<&zbus::Connection>,
        heard: &async_channel::Sender<News>,
    ) -> Vec<Command<Message>> {
        let (false, Some(session), Some(lookup)) = (self.served, session, lookup) else {
            return Vec::new();
        };
        self.served = true;
        let (session, lookup, heard) = (session.clone(), lookup.clone(), heard.clone());
        vec![Command::Perform(Box::pin(async move {
            let served = service::serve(&session, lookup, move |request| {
                heard.try_send(News::Consent(request)).is_ok()
            })
            .await;
            match served {
                Ok(()) => Message::Stopped(format!("serving {}", service::BUS_NAME)),
                Err(error) => Message::Stopped(format!("serving {}: {error}", service::BUS_NAME)),
            }
        }))]
    }

    fn apply(&mut self, answered: Vec<Answered>) {
        for Answered { call, response } in answered {
            if let Some(reply) = self.replies.remove(&call) {
                let _ = reply.try_send(response);
            }
        }
    }

    /// A call of the portal, and the reading of its application's icon.
    pub fn request(&mut self, request: Request) -> Vec<Command<Message>> {
        match request {
            Request::Ask {
                handle,
                question,
                reply,
            } => {
                if self.model.is_empty() {
                    // the icons of questions answered before are read again when asked again
                    self.icons.clear();
                }
                self.calls += 1;
                let call = self.calls;
                let icon = question.app().icon.clone();
                self.replies.insert(call, reply);
                let answered = self.model.ask(call, handle, question);
                let asked = answered.is_empty();
                self.apply(answered);
                match icon {
                    Some(name) if asked && !self.icons.contains_key(&name) => {
                        self.icons.insert(name.clone(), None);
                        vec![task::io(
                            ICON_DEADLINE,
                            {
                                let name = name.clone();
                                move || read_icon(&name)
                            },
                            move |sources| Message::Consent(ConsentInput::Icon { name, sources }),
                        )]
                    }
                    _ => Vec::new(),
                }
            }
            Request::Close { handle } => {
                let answered = self.model.close(&handle);
                self.apply(answered);
                Vec::new()
            }
        }
    }

    pub fn set_away(&mut self, away: bool) {
        if self.model.away() != away {
            let answered = self.model.set_away(away);
            self.apply(answered);
        }
    }

    /// Escape, or the server closed the alert.
    pub fn dismiss(&mut self) {
        let answered = self.model.answer(Choice::Dismiss, Instant::now());
        self.apply(answered);
    }

    /// The alert of the question on screen opened as `window`: its arming starts with its first presented frame.
    pub fn opened(&mut self, window: WindowKey) -> Vec<Command<Message>> {
        self.scroll = NO_SCROLL;
        match self.model.shown(Instant::now()) {
            Some(shown) => vec![Command::OnPresented {
                window,
                message: Message::Consent(ConsentInput::Presented(shown.call)),
            }],
            None => Vec::new(),
        }
    }

    pub fn input(&mut self, input: ConsentInput) -> Vec<Command<Message>> {
        let now = Instant::now();
        match input {
            ConsentInput::Allow => {
                let answered = self.model.answer(Choice::Allow, now);
                self.apply(answered);
            }
            ConsentInput::Deny => {
                let answered = self.model.answer(Choice::Deny, now);
                self.apply(answered);
            }
            ConsentInput::Scroll(scroll) => self.scroll = scroll,
            ConsentInput::Presented(call) => {
                if let Some(wait) = self.model.presented(call, now) {
                    return vec![Command::Perform(Box::pin(async move {
                        async_io::Timer::after(wait).await;
                        Message::Consent(ConsentInput::Armed(call))
                    }))];
                }
            }
            ConsentInput::Armed(call) => self.model.arm(call),
            ConsentInput::Icon { name, sources } => {
                self.icons.insert(name, sources);
            }
        }
        Vec::new()
    }

    /// The call of the question to show.
    pub fn shown_about(&self) -> Option<String> {
        self.model
            .shown(Instant::now())
            .map(|shown| format!("consent {}", shown.call))
    }

    /// What the alert on screen shows.
    pub fn view(&self) -> Option<view::consent::Consent> {
        let shown = self.model.shown(Instant::now())?;
        let icon = shown
            .question
            .app()
            .icon
            .as_ref()
            .and_then(|name| self.icons.get(name).cloned().flatten());
        Some(view::consent::Consent {
            question: shown.question.clone(),
            icon,
            armed: shown.armed,
            scroll: self.scroll,
        })
    }

    /// The size of the alert's surface.
    pub fn alert_size(&self) -> Size {
        self.model
            .shown(Instant::now())
            .map_or(view::consent::SCREENSHOT_SIZE, |shown| {
                view::consent::size(shown.question)
            })
    }
}
