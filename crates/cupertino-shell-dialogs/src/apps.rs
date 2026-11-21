//! One app chooser: the applications the portal offers, the one selected, and the activation token asked for the
//! chosen one when the app wants it.

use cupertino_kit::controls::Selection;
use cupertino_kit::core::{Command, Key, WindowKey};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;

use crate::ask::{AppAnswer, AppAsk, AppChoice};
use crate::component::Message;

const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// What happens in an app chooser's window.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Scroll(ScrollState),
    Select(Selection),
    /// Open the selected application: the Open button, Return or a double click.
    Open,
    /// The activation token the server made for the application chosen.
    Opened(String, Option<String>),
    /// Cancel, Escape, or the server closed the window.
    Cancel,
}

/// One open app chooser.
pub struct AppDialog {
    pub id: u64,
    handle: String,
    reply: async_channel::Sender<AppAnswer>,
    ask: AppAsk,
    rows: Selection,
    scroll: ScrollState,
    /// The application chosen while its activation token is asked for; the dialog answers when the token arrives.
    opening: Option<String>,
    answer: Option<AppAnswer>,
}

impl AppDialog {
    /// The dialog for `ask`, from the portal's request `handle`, which answers on `reply`; the app's last choice is
    /// selected when it is offered.
    pub fn new(
        id: u64,
        handle: String,
        ask: AppAsk,
        reply: async_channel::Sender<AppAnswer>,
    ) -> AppDialog {
        let last = ask.last_choice.clone();
        let mut dialog = AppDialog {
            id,
            handle,
            reply,
            ask,
            rows: Selection::default(),
            scroll: NO_SCROLL,
            opening: None,
            answer: None,
        };
        dialog.select(last.as_deref());
        dialog
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    pub fn window_key(&self) -> WindowKey {
        WindowKey(Key::from(format!("apps-{}", self.id)))
    }

    pub fn ask(&self) -> &AppAsk {
        &self.ask
    }

    pub fn rows(&self) -> &Selection {
        &self.rows
    }

    pub fn scroll(&self) -> ScrollState {
        self.scroll
    }

    /// The application selected.
    pub fn chosen(&self) -> Option<&AppChoice> {
        self.rows.cursor().and_then(|i| self.ask.choices.get(i))
    }

    /// The rows with the application `id` selected, or none when it is not offered.
    fn select(&mut self, id: Option<&str>) {
        self.rows = Selection::default();
        if let Some(i) = id.and_then(|id| self.ask.choices.iter().position(|c| c.id == id)) {
            self.rows.select_only(i);
        }
    }

    /// The applications offered changed; the one selected stays selected when it is still offered.
    pub fn update_choices(&mut self, choices: Vec<AppChoice>) {
        let chosen = self.chosen().map(|c| c.id.clone());
        self.ask.choices = choices;
        self.select(chosen.as_deref());
    }

    /// Carry out `event`; true when the dialog is done.
    pub fn update(&mut self, event: AppEvent) -> (bool, Vec<Command<Message>>) {
        match event {
            AppEvent::Scroll(scroll) => self.scroll = scroll,
            AppEvent::Select(rows) => self.rows = rows,
            AppEvent::Open => return self.open(),
            AppEvent::Opened(choice, token) => {
                if self.opening.as_deref() == Some(choice.as_str()) {
                    self.answer = Some(AppAnswer::Chosen { id: choice, token });
                    return (true, Vec::new());
                }
            }
            AppEvent::Cancel => {
                self.answer = Some(AppAnswer::Cancelled);
                return (true, Vec::new());
            }
        }
        (false, Vec::new())
    }

    fn open(&mut self) -> (bool, Vec<Command<Message>>) {
        if self.opening.is_some() {
            return (false, Vec::new());
        }
        let Some(choice) = self.chosen().map(|c| c.id.clone()) else {
            return (false, Vec::new());
        };
        if !self.ask.wants_token {
            self.answer = Some(AppAnswer::Chosen {
                id: choice,
                token: None,
            });
            return (true, Vec::new());
        }
        // made for the key or click that chose, on this window, so the server lets the application take the focus
        self.opening = Some(choice.clone());
        let (id, chosen) = (self.id, choice.clone());
        let command = Command::ActivationToken {
            window: self.window_key(),
            app_id: Some(choice),
            on_token: Box::new(move |token| Message::App(id, AppEvent::Opened(chosen, token))),
        };
        (false, vec![command])
    }

    /// Send the dialog's answer to its call; a dialog dropped without one ends its call.
    pub fn finish(self) {
        if let Some(answer) = self.answer {
            let _ = self.reply.try_send(answer);
        }
    }
}
