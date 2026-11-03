//! The launcher as a CupertinoKit application: the model around [`Panel`] and [`Launcher`], what each message does
//! to it, and its windows. What needs the session (starting commands, opening paths, building the file index, the
//! layer surface's size, the appear animation's timer) goes through [`Host`], so the model runs headless in tests.

use crate::engine::{Action, Activation, Launcher, ResultItem};
use crate::exec::CommandSpec;
use crate::files::{FileIndex, FileSearchConfig};
use crate::panel::{Now, Panel, PanelCommand, PanelEvent};
use crate::view::{self, Input, PanelView, MAX_ROWS};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Command, Key, Window, WindowKey};
use std::collections::HashMap;
use std::sync::Arc;

/// The most result icons kept; past it the cache starts again.
const MAX_ICONS: usize = 512;

/// What the model needs the session for.
pub trait Host {
    /// Start `spec`, with the activation token the compositor gave for it.
    fn run(&mut self, spec: CommandSpec, token: Option<String>);
    /// Open a path or URL with the default handler.
    fn open(&mut self, target: &str);
    /// Build the file index of `config` off the loop; the commands deliver it as [`Message::Indexed`].
    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>>;
    /// The shown panel is now `height` points tall.
    fn resize(&mut self, height: f64);
    /// The next frame of the appear animation, as [`Message::Frame`] with `frame`.
    fn frame(&mut self, frame: u64) -> Vec<Command<Message>>;
    /// The image of `item`'s icon, `None` to show its kind's symbol.
    fn icon(&mut self, item: &ResultItem) -> Option<ImageSource>;
    /// Both clocks now.
    fn now(&self) -> Now;
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    /// `org.cupertino.Launcher1.Toggle`.
    Toggle,
    /// The file index finished building.
    Indexed(Arc<FileIndex>),
    /// The activation token of the launch waiting as `launch` arrived (`None` when there is none).
    Launch(u64, Option<String>),
    /// A frame of the appear animation.
    Frame(u64),
    /// The file index could not be built.
    IndexFailed,
    /// Nothing will send [`Message::Toggle`] any more: the process ends.
    Unheard,
}

impl From<Input> for Message {
    fn from(input: Input) -> Self {
        Message::Input(input)
    }
}

pub struct Model {
    pub launcher: Launcher,
    pub panel: Panel,
    /// The query's revision the model last took from the field.
    pub revision: u64,
    /// Images of the shown results' icons by result id.
    pub icons: HashMap<String, ImageSource>,
    /// Where the file index is built from; `None` without file results.
    pub files: Option<FileSearchConfig>,
    /// Put the appear animation at its end at once (the server's settled test clock).
    pub settled: bool,
    host: Box<dyn Host>,
    indexing: bool,
    /// The height the shown panel was last given.
    height: f64,
    /// Launches waiting for their activation token.
    pending: HashMap<u64, CommandSpec>,
    next_launch: u64,
    /// The animation frame that counts; an older one is ignored when it arrives.
    frame: u64,
}

impl Model {
    pub fn new(launcher: Launcher, host: Box<dyn Host>) -> Self {
        let mut panel = Panel::new();
        panel.max_rows = MAX_ROWS;
        Self {
            launcher,
            panel,
            revision: 0,
            icons: HashMap::new(),
            files: None,
            settled: false,
            host,
            indexing: false,
            height: view::COLLAPSED_HEIGHT,
            pending: HashMap::new(),
            next_launch: 0,
            frame: 0,
        }
    }

    fn now(&self) -> Now {
        self.host.now()
    }

    /// The appear animation's alpha now.
    pub fn alpha(&self) -> f64 {
        if !self.panel.is_visible() {
            return 0.0;
        }
        self.panel.progress(self.now().mono).clamp(0.0, 1.0)
    }

    /// Open with an empty query on the output under the pointer; the file index starts on the first show.
    fn show(&mut self) -> Vec<Command<Message>> {
        let now = self.now();
        self.panel.show(&self.launcher, now);
        let mut commands = Vec::new();
        if self.settled {
            self.panel.settle_appear();
        } else {
            commands.extend(self.animate());
        }
        if let (Some(config), false, None) =
            (&self.files, self.indexing, self.launcher.file_index())
        {
            self.indexing = true;
            let config = config.clone();
            commands.extend(self.host.index(config));
        }
        commands
    }

    /// Close; the launch history is written.
    fn hide(&mut self) {
        let now = self.now();
        self.panel.hide(now.mono);
        if let Err(e) = self.launcher.frecency.save_if_dirty() {
            eprintln!("cupertino-launcher: launch history not saved: {e}");
        }
    }

    fn animate(&mut self) -> Vec<Command<Message>> {
        self.frame += 1;
        if self.panel.is_visible() && self.panel.is_animating(self.now().mono) {
            self.host.frame(self.frame)
        } else {
            Vec::new()
        }
    }

    /// After every change: the shown results' icons, and the panel's height.
    fn changed(&mut self) {
        if !self.panel.is_visible() {
            return;
        }
        if self.icons.len() > MAX_ICONS {
            self.icons.clear();
        }
        for row in view::visible_rows(&self.panel) {
            let view::Row::Item { flat, .. } = row else {
                continue;
            };
            let Some(item) = self.panel.results().get(flat) else {
                continue;
            };
            if !self.icons.contains_key(&item.id) {
                if let Some(image) = self.host.icon(item) {
                    self.icons.insert(item.id.clone(), image);
                }
            }
        }
        let height = view::panel_height(&self.panel);
        if height != self.height {
            self.height = height;
            self.host.resize(height);
        }
    }

    /// Carry out what the panel asked for.
    fn outcome(&mut self, outcome: PanelEvent) -> Vec<Command<Message>> {
        match outcome {
            PanelEvent::None | PanelEvent::Changed | PanelEvent::Edit => Vec::new(),
            PanelEvent::Close => {
                self.hide();
                Vec::new()
            }
            PanelEvent::Activate(activation) => self.activated(activation),
            PanelEvent::Command(command) => self.command(command),
        }
    }

    /// The panel closed itself for `activation`: the clipboard is set, or the token asked for, while its window
    /// still exists.
    fn activated(&mut self, activation: Activation) -> Vec<Command<Message>> {
        let commands = match activation {
            Activation::CopyToClipboard(text) => vec![Command::SetClipboard(text)],
            Activation::Run(spec) => {
                let launch = self.next_launch;
                self.next_launch += 1;
                let app_id = spec.desktop_id.clone();
                self.pending.insert(launch, spec);
                vec![Command::ActivationToken {
                    window: WindowKey(Key::from("panel")),
                    app_id,
                    on_token: Box::new(move |token| Message::Launch(launch, token)),
                }]
            }
            Activation::Nothing => {
                eprintln!("cupertino-launcher: the entry has no usable command");
                Vec::new()
            }
        };
        self.hide();
        commands
    }

    /// Shortcut commands of the panel that need the host.
    fn command(&mut self, command: PanelCommand) -> Vec<Command<Message>> {
        match command {
            PanelCommand::Reveal => {
                if let Some(Action::OpenPath(path)) =
                    self.panel.selected_item().map(|item| item.action.clone())
                {
                    let directory = if path.is_dir() {
                        path
                    } else {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                    };
                    self.hide();
                    self.host.open(&directory.to_string_lossy());
                }
            }
            PanelCommand::WebSearch => {
                match (&self.launcher.config.web_search_url, self.panel.query()) {
                    (Some(url), query) if !query.is_empty() => {
                        let target = url.replace("{}", &percent_encode(query));
                        self.hide();
                        self.host.open(&target);
                    }
                    _ => eprintln!(
                        "cupertino-launcher: no web search URL configured (`web_search_url`)"
                    ),
                }
            }
            other => eprintln!("cupertino-launcher: {other:?} is not provided"),
        }
        Vec::new()
    }
}

/// Percent-encode `text` for a URL query.
pub fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
    let now = model.now();
    let commands = match message {
        Message::Toggle if model.panel.is_visible() => {
            model.hide();
            Vec::new()
        }
        Message::Toggle => model.show(),
        Message::Input(_) if !model.panel.is_visible() => Vec::new(),
        Message::Input(Input::Key(key, mods)) => {
            let outcome = model.panel.handle_key(key, mods, &mut model.launcher, now);
            model.outcome(outcome)
        }
        Message::Input(Input::Query(change)) => {
            model.revision = change.revision;
            model
                .panel
                .edit(&model.launcher, now, |field| field.set_text(&change.text));
            Vec::new()
        }
        Message::Input(Input::Hover(flat)) => {
            model.panel.set_selected(flat);
            Vec::new()
        }
        Message::Input(Input::Activate(flat)) => {
            let outcome = model.panel.activate_index(flat, &mut model.launcher, now);
            model.outcome(outcome)
        }
        Message::Input(Input::Dismiss) => {
            model.hide();
            Vec::new()
        }
        Message::Indexed(index) => {
            model.indexing = false;
            model.launcher.set_file_index(Some(index));
            model.panel.refresh(&model.launcher, now);
            Vec::new()
        }
        Message::Launch(launch, token) => {
            if let Some(spec) = model.pending.remove(&launch) {
                model.host.run(spec, token);
            }
            Vec::new()
        }
        Message::Frame(frame) if frame == model.frame => model.animate(),
        Message::Frame(_) => Vec::new(),
        Message::IndexFailed => {
            model.indexing = false;
            eprintln!("cupertino-launcher: the file index was not built");
            Vec::new()
        }
        Message::Unheard => {
            eprintln!("cupertino-launcher: the bus interface stopped");
            vec![Command::Exit]
        }
    };
    model.changed();
    commands
}

/// The shown panel and its catcher, or the window kept mapped while it is hidden.
pub fn view(model: &Model) -> Vec<Window<Message>> {
    if !model.panel.is_visible() {
        return vec![view::hidden_window()];
    }
    let panel = PanelView {
        panel: &model.panel,
        revision: model.revision,
        alpha: model.alpha(),
        icons: &model.icons,
    };
    vec![view::panel_window(&panel), view::catcher_window()]
}
