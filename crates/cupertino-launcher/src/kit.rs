//! The launcher as a CupertinoKit application: the model around the navigation state ([`Nav`]), the search panel
//! ([`Panel`]), the application browser ([`Browser`]) and the panel's presentation ([`Motion`]), what each message does
//! to them, and the windows. What needs the session (starting commands, opening paths, building the file index and
//! searching it, the layer surface's size, timers, icons and what the preview shows of a file) goes through [`Host`],
//! so the model runs headless in tests.

use crate::browse::{self, App, Arrangement, BrowseLayout, Browser, Category, Zone};
use crate::engine::{
    self, ActionKind, Activation, ItemAction, Launcher, LauncherConfig, ResultItem,
};
use crate::exec::CommandSpec;
use crate::files::{FileIndex, FileSearchConfig};
use crate::motion::{Frame, Motion, Shape};
use crate::nav::{Focus as NavFocus, Intent, Nav, Nested, Step, View};
use crate::panel::{
    Focus, Key as PanelKey, Mods as PanelMods, Now, Panel, PanelCommand, PanelEvent,
};
use crate::preview::{self, Preview};
use crate::providers::{Cancel, Generation, Provider, Reply, Status, Tracker, Update};
use crate::view::{self, Input, KeyMode, PanelView, MAX_ROWS};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A launch waiting for its activation token.
struct Pending {
    spec: CommandSpec,
    /// The desktop file ID of the application it starts, when it is opened as its dock tile opens it.
    opens: Option<String>,
}

/// An application being opened, with its token: waiting for the server's answer, then for its D-Bus call.
struct Opening {
    desktop_id: String,
    spec: CommandSpec,
    token: Option<String>,
}

/// The most result icons kept; past it the cache starts again.
const MAX_ICONS: usize = 512;
/// How long the preview of a result may take before the pane says it is unavailable, in seconds.
pub const PREVIEW_TIMEOUT: f64 = 1.5;
/// Room kept free below the browser on a short output (the dock's), in points.
pub const BROWSE_BOTTOM_ROOM: f64 = 96.0;
/// How long the query must stand before what the panel says of it ("Searching", "No Results", that results may be
/// incomplete) is said aloud, in seconds: the providers' deadline, so a search that answers in time says nothing
/// before its results, and typing says nothing between keys.
pub const STATUS_SETTLE: f64 = crate::providers::DEADLINE;
/// How many queries the panel remembers having said a status of.
const MAX_SAID: usize = 32;
/// The most application icons the browser has being loaded at once, off the loop: those nearest the view are asked
/// first, and each one that arrives makes room for the next.
pub const MAX_APP_ICON_LOADS: usize = 14;

/// A file search for the host to run off the loop: [`engine::file_sections`] over `index`, answered as
/// [`Message::Provided`] with `generation`, given up once `cancel` is cancelled.
#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub generation: Generation,
    pub text: String,
    pub index: Arc<FileIndex>,
    pub config: LauncherConfig,
    pub cancel: Cancel,
    pub unix: f64,
}

impl SearchRequest {
    /// Run the search here and now: what a host does on its worker.
    pub fn run(&self) -> Update {
        let sections = engine::file_sections(
            &self.index,
            &self.text,
            self.unix,
            &self.config,
            Some(&self.cancel),
        );
        Update {
            generation: self.generation,
            provider: Provider::Files,
            reply: Reply::Sections(sections),
        }
    }
}

/// An `org.freedesktop.Application.Activate` call under way ([`Host::ask_for_window`]): whether it was answered.
pub type BusCall = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// What the server found for an application the launcher opens ([`Host::bring_forward`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    /// It showed a window and was brought forward, as a click on its dock tile brings it: nothing is started.
    Raised,
    /// A process of it runs but shows no window: it is asked for one, or started again.
    Resident,
    /// It does not run: it is started.
    NotRunning,
}

/// What the model needs the session for.
pub trait Host {
    /// Start `spec`, with the activation token the compositor gave for it.
    fn run(&mut self, spec: CommandSpec, token: Option<String>);
    /// Open a path or URL with the default handler.
    fn open(&mut self, target: &str);
    /// Build the file index of `config` off the loop; the commands deliver it as [`Message::Indexed`].
    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>>;
    /// The panel's window is now `size` points large.
    fn resize(&mut self, size: Size);
    /// The next animation frame, as [`Message::Frame`] with `frame`.
    fn frame(&mut self, frame: u64) -> Vec<Command<Message>>;
    /// The renditions of `item`'s icon, one per output scale it is loaded at; none to show its kind's symbol.
    fn icon(&mut self, item: &ResultItem) -> Vec<ImageSource>;
    /// Load and decode the renditions of an application's icon at the browser's sizes off the loop; the commands
    /// deliver them as [`Message::AppIcon`] with its desktop ID, none to draw a placeholder. The default delivers none
    /// at once.
    fn app_icon(&mut self, app: &App) -> Vec<Command<Message>> {
        let id = app.id.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::AppIcon(id, Vec::new())
        }))]
    }
    /// Find out what the preview shows of `item`, the selected result, off the loop; the commands deliver it as
    /// [`Message::Previewed`]. None when the preview shows only the result itself.
    fn preview(&mut self, item: &ResultItem) -> Vec<Command<Message>>;
    /// Run a file search; the commands deliver its [`Update`] as [`Message::Provided`]. The default runs it at once.
    fn search(&mut self, request: SearchRequest) -> Vec<Command<Message>> {
        let update = request.run();
        vec![Command::Perform(Box::pin(async move {
            Message::Provided(update)
        }))]
    }
    /// Deliver `message` after `seconds` (a deadline). The default never does: the model then waits for the answer.
    fn after(&mut self, _seconds: f64, _message: Message) -> Vec<Command<Message>> {
        Vec::new()
    }
    /// Both clocks now.
    fn now(&self) -> Now;
    /// Ask the server to bring the application of desktop file ID `desktop_id` forward as a click on its dock tile
    /// does; the answer comes back as [`Message::Opened`] with `launch`. False when the server cannot be asked (the
    /// default): the application is then started as before.
    fn bring_forward(&mut self, _desktop_id: &str, _launch: u64) -> bool {
        false
    }
    /// Ask the resident application of bus name `bus_name` (its desktop file ID without `.desktop`), whose desktop file
    /// is `source`, for a new window over D-Bus with `token`, when its entry is D-Bus activatable; the call ends in
    /// whether it was answered. `None` (the default) when it is not: the entry is then started again, and a running
    /// instance opens a window.
    fn ask_for_window(
        &mut self,
        _bus_name: &str,
        _source: Option<&std::path::Path>,
        _token: Option<String>,
    ) -> Option<BusCall> {
        None
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    /// `org.cupertino.Launcher1.Toggle`: the search field opens, or whatever is up closes.
    Toggle,
    /// `org.cupertino.Launcher1.ToggleApplications` (the dock's app-browser request): the browser opens, the search
    /// field unfolds into it, or the browser closes.
    Browse,
    /// The file index finished building.
    Indexed(Arc<FileIndex>),
    /// The activation token of the launch waiting as `launch` arrived (`None` when there is none).
    Launch(u64, Option<String>),
    /// The server's answer for the application opened as `launch` ([`Host::bring_forward`]).
    Opened(u64, Running),
    /// The resident application opened as `launch` answered the request for a window (`true`) or not.
    Asked(u64, bool),
    /// An animation frame.
    Frame(u64),
    /// The file index could not be built.
    IndexFailed,
    /// Nothing will send [`Message::Toggle`] any more: the process ends.
    Unheard,
    /// The session was locked: what is shown goes at once, and the launch history is written.
    Locked,
    /// The server closed the hidden window (its output went away): a new one is opened, on an output still there.
    Remap,
    /// What the preview shows of the result with this id.
    Previewed(String, Arc<Preview>),
    /// The preview of the result with this id is overdue.
    PreviewOverdue(String),
    /// A slow provider answered.
    Provided(Update),
    /// The renditions of the icon of the browser's application with this desktop ID arrived ([`Host::app_icon`]).
    AppIcon(String, Vec<ImageSource>),
    /// The slow providers of this generation are overdue.
    Overdue(Generation),
    /// The query of this change ([`Model::query_change`]) has stood for [`STATUS_SETTLE`]: what the panel says of it
    /// may be said aloud.
    Settled(u64),
    /// The browser's filter (its query and category) of this change ([`Model::browse_change`]) has stood for
    /// [`STATUS_SETTLE`]: "No Results" may be said aloud.
    BrowseSettled(u64),
    /// The output the panel is on is this large, in points.
    Output(Size),
    /// The session's Reduce Motion option.
    ReduceMotion(bool),
}

impl From<Input> for Message {
    fn from(input: Input) -> Self {
        Message::Input(input)
    }
}

/// Where the preview of the selected result is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PreviewState {
    /// Nothing asked (no selection, or the result shows itself).
    #[default]
    None,
    /// Asked for the result with this id; not arrived yet.
    Waiting(String),
    Ready(Arc<Preview>),
    /// It did not arrive in time.
    Unavailable,
}

pub struct Model {
    pub launcher: Launcher,
    pub panel: Panel,
    pub nav: Nav,
    /// The application browser, from its first opening on.
    pub browser: Option<Browser>,
    pub motion: Motion,
    /// The query's revision the model last took from the field.
    pub revision: u64,
    /// Images of the shown results' icons by result id.
    pub icons: HashMap<String, Vec<ImageSource>>,
    /// Images of the browser's applications by desktop ID.
    pub app_icons: HashMap<String, Vec<ImageSource>>,
    /// The applications whose icons are being loaded, at most [`MAX_APP_ICON_LOADS`].
    app_icons_loading: HashSet<String>,
    /// Where the file index is built from; `None` without file results.
    pub files: Option<FileSearchConfig>,
    /// Put every transition at its end at once (the server's settled test clock).
    pub settled: bool,
    host: Box<dyn Host>,
    indexing: bool,
    /// The file index, searched as a provider.
    file_index: Option<Arc<FileIndex>>,
    tracker: Tracker,
    /// The query the providers were last asked about.
    searched: Option<String>,
    /// How many times the query changed: what is said aloud belongs to one change.
    query_change: u64,
    /// The status said aloud and the query change it was said for. Said once per change at most, after the query
    /// stood for [`STATUS_SETTLE`].
    said: Option<(u64, Status)>,
    /// The queries and statuses said aloud since the panel opened, the latest [`MAX_SAID`]: the same status is not said
    /// again for the same query, even after it was edited away and typed again.
    said_before: Vec<(String, Status)>,
    /// The browser's filter last seen: its query and category.
    browse_filter: Option<(String, Option<Category>)>,
    /// How many times the browser's filter changed: its "No Results" belongs to one change.
    browse_change: u64,
    /// The browser's filter change "No Results" was said aloud for, once it stood for [`STATUS_SETTLE`].
    browse_said: Option<u64>,
    /// The browser's filters "No Results" was said for since the panel opened, the latest [`MAX_SAID`]: not said again
    /// for the same filter.
    browse_said_before: Vec<(String, Option<Category>)>,
    /// The panel window's size as last given.
    extent: Size,
    /// Launches waiting for their activation token.
    pending: HashMap<u64, Pending>,
    /// Applications opened with their token, waiting for the server's answer or for their D-Bus call.
    opening: HashMap<u64, Opening>,
    next_launch: u64,
    /// The animation frame that counts; an older one is ignored when it arrives.
    pub(crate) frame: u64,
    /// The hidden window's generation: each one is a new window, so one the server closed is not kept.
    pub hidden: u64,
    preview: PreviewState,
    /// The id of the result the preview was last asked for.
    previewed: Option<String>,
    /// The browser's arrangement, kept from one opening to the next.
    arrangement: Arrangement,
    /// The output's size, once known.
    output: Option<Size>,
}

impl Model {
    pub fn new(launcher: Launcher, host: Box<dyn Host>) -> Self {
        let mut panel = Panel::new();
        panel.max_rows = MAX_ROWS;
        Self {
            launcher,
            panel,
            nav: Nav::HIDDEN,
            browser: None,
            motion: Motion::new(view::compact_shape()),
            revision: 0,
            icons: HashMap::new(),
            app_icons: HashMap::new(),
            app_icons_loading: HashSet::new(),
            files: None,
            settled: false,
            host,
            indexing: false,
            file_index: None,
            tracker: Tracker::new(),
            searched: None,
            query_change: 0,
            said: None,
            said_before: Vec::new(),
            browse_filter: None,
            browse_change: 0,
            browse_said: None,
            browse_said_before: Vec::new(),
            extent: view::window_extent(view::compact_shape()),
            pending: HashMap::new(),
            opening: HashMap::new(),
            next_launch: 0,
            frame: 0,
            hidden: 0,
            preview: PreviewState::None,
            previewed: None,
            arrangement: Arrangement::Grid,
            output: None,
        }
    }

    fn now(&self) -> Now {
        self.host.now()
    }

    /// What is presented now.
    pub fn presented(&self) -> Frame {
        self.motion.frame(self.now().mono)
    }

    /// Whether the panel's window is up: open, or still fading out.
    pub fn presenting(&self) -> bool {
        self.nav.is_visible() || !self.motion.gone(self.now().mono)
    }

    /// The enclosure's opacity now.
    pub fn alpha(&self) -> f64 {
        self.presented().alpha
    }

    /// What the panel says when no result shows.
    pub fn status(&self) -> Status {
        self.tracker.status(
            !self.panel.query().trim().is_empty(),
            self.panel.results().len(),
        )
    }

    /// The browser's height on the output known.
    fn browse_height(&self) -> f64 {
        match self.output {
            Some(output) => BrowseLayout::height_on(
                output.height,
                (output.height * view::TOP_FRACTION).round(),
                BROWSE_BOTTOM_ROOM,
            ),
            None => BrowseLayout::HEIGHT,
        }
    }

    /// The shape the current view heads for.
    fn target_shape(&self) -> Shape {
        match self.nav.view {
            View::Browse => view::browse_shape(self.browse_height()),
            _ => view::search_shape(&self.panel, &self.status(), self.nav.nested),
        }
    }

    /// Open the browser (afresh) with the launcher's applications and recents, in the remembered arrangement.
    fn open_browser(&mut self) {
        let now = self.now();
        let recent = self.launcher.recent_apps(browse::COLUMNS, now.unix);
        let mut browser =
            Browser::from_entries(self.launcher.entries(), &recent, self.browse_height());
        browser.set_arrangement(self.arrangement);
        browser.set_query(self.panel.query());
        self.browser = Some(browser);
    }

    /// Carry out a navigation step.
    fn step(&mut self, step: Step) -> Vec<Command<Message>> {
        let now = self.now();
        let mut commands = Vec::new();
        match step {
            Step::Stay | Step::Noted => {}
            Step::Show(view) => {
                self.panel.show(&self.launcher, now);
                self.panel.settle_appear();
                self.searched = None;
                let browse = view == View::Browse;
                if browse {
                    self.panel.set_text_quiet("");
                    self.open_browser();
                }
                self.motion.show(self.target_shape(), browse, now.mono);
                if let (Some(config), false, None) = (&self.files, self.indexing, &self.file_index)
                {
                    self.indexing = true;
                    let config = config.clone();
                    commands.extend(self.host.index(config));
                }
            }
            Step::Hide => self.hide(false),
            Step::Fold => {
                self.panel.set_query("", &self.launcher, now);
                self.searched = None;
                self.motion.fold(self.target_shape(), now.mono);
            }
            Step::Unfold => {
                let query = self.panel.query().to_owned();
                self.panel.set_text_quiet(&query);
                self.tracker.cancel();
                self.preview = PreviewState::None;
                let height = self.browse_height();
                match self.browser.as_mut() {
                    Some(browser) => {
                        browser.set_height(height);
                        browser.set_query(&query);
                        browser.deselect();
                    }
                    None => self.open_browser(),
                }
                self.motion.unfold(self.target_shape(), now.mono);
            }
            Step::Leave(_, _) => {}
            Step::Enter(_) => {}
            Step::FocusField => self.panel.focus_field(),
            Step::ClearQuery => {
                match self.nav.view {
                    View::Browse => {
                        self.panel.set_text_quiet("");
                        if let Some(b) = self.browser.as_mut() {
                            b.set_query("");
                        }
                    }
                    _ => self.panel.set_query("", &self.launcher, now),
                }
                // the field shows the cleared query
                self.revision += 1;
            }
            Step::ClearCategory => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_category(None);
                }
            }
        }
        if self.settled {
            self.motion.settle(now.mono);
        }
        commands.extend(self.animate());
        commands
    }

    fn intent(&mut self, intent: Intent) -> Vec<Command<Message>> {
        let step = self.nav.apply(intent);
        self.step(step)
    }

    /// Close: the window fades out (at once when `now_gone`, for a session lock); the launch history is written.
    fn hide(&mut self, now_gone: bool) {
        let now = self.now();
        let browse = self.presented().header == crate::motion::Header::Browse;
        if self.nav.is_visible() {
            self.nav = Nav::HIDDEN;
        }
        self.panel.hide(now.mono);
        self.tracker.cancel();
        self.searched = None;
        self.said = None;
        self.said_before.clear();
        self.browse_filter = None;
        self.browse_said = None;
        self.browse_said_before.clear();
        self.preview = PreviewState::None;
        self.previewed = None;
        self.motion.hide(browse, now.mono);
        if now_gone || self.settled {
            self.motion.settle(now.mono);
        }
        if let Err(e) = self.launcher.frecency.save_if_dirty() {
            eprintln!("cupertino-launcher: launch history not saved: {e}");
        }
    }

    fn animate(&mut self) -> Vec<Command<Message>> {
        self.frame += 1;
        let now = self.now().mono;
        let scrolling = self.browser.as_ref().is_some_and(|b| b.animating(now));
        if self.motion.animating(now) || (self.nav.view == View::Browse && scrolling) {
            self.host.frame(self.frame)
        } else {
            Vec::new()
        }
    }

    /// What the preview shows of the selected result, once it has arrived.
    pub fn preview(&self) -> Option<&Preview> {
        match &self.preview {
            PreviewState::Ready(p) => Some(p),
            _ => None,
        }
    }

    pub fn preview_state(&self) -> &PreviewState {
        &self.preview
    }

    /// The query changed in search: ask the slow providers about it, cancelling what they still do for the old one.
    fn search(&mut self) -> Vec<Command<Message>> {
        let text = self.panel.query().to_owned();
        if self.searched.as_deref() == Some(text.as_str()) {
            return Vec::new();
        }
        self.searched = Some(text.clone());
        // nothing is said of the new query until it has stood a while
        self.query_change += 1;
        let mut commands = self
            .host
            .after(STATUS_SETTLE, Message::Settled(self.query_change));
        let now = self.now();
        let index = self.file_index.clone().filter(|_| !text.trim().is_empty());
        let Some(index) = index else {
            self.tracker.begin(&[], now.mono);
            return commands;
        };
        let generation = self.tracker.begin(&[Provider::Files], now.mono);
        let request = SearchRequest {
            generation,
            text,
            index,
            config: self.launcher.config.clone(),
            cancel: self.tracker.token(),
            unix: now.unix,
        };
        commands.extend(self.host.search(request));
        commands.extend(
            self.host
                .after(crate::providers::DEADLINE, Message::Overdue(generation)),
        );
        commands
    }

    /// The query of change `change` has stood for [`STATUS_SETTLE`]: say what the panel says of it, once. Providers past
    /// their deadline give up first, so what is said is final.
    fn settle(&mut self, change: u64) {
        if change != self.query_change || self.said.as_ref().is_some_and(|(c, _)| *c == change) {
            return;
        }
        self.tracker.expire(self.now().mono);
        let status = self.status();
        let said = (self.panel.query().to_owned(), status.clone());
        if view::status_text(&status).is_some() && !self.said_before.contains(&said) {
            self.said = Some((change, status));
            if self.said_before.len() == MAX_SAID {
                self.said_before.remove(0);
            }
            self.said_before.push(said);
        }
    }

    /// The status said aloud for the query now, while the panel still says it: `None` before the query settled, once
    /// results show, and after the query changed.
    pub fn said(&self) -> Option<Status> {
        let (change, said) = self.said.as_ref()?;
        let now = self.status();
        (*change == self.query_change && view::status_text(&now).is_some()).then(|| said.clone())
    }

    /// How many times the query changed ([`Message::Settled`]).
    pub fn query_change(&self) -> u64 {
        self.query_change
    }

    /// A browser whose filter (query or category) changed waits [`STATUS_SETTLE`] before "No Results" may be said of it.
    fn watch_browse_filter(&mut self) -> Vec<Command<Message>> {
        let Some(b) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        else {
            return Vec::new();
        };
        let filter = (b.query().to_owned(), b.category());
        if self.browse_filter.as_ref() == Some(&filter) {
            return Vec::new();
        }
        self.browse_filter = Some(filter);
        self.browse_change += 1;
        self.host
            .after(STATUS_SETTLE, Message::BrowseSettled(self.browse_change))
    }

    /// The browser's filter of change `change` has stood for [`STATUS_SETTLE`]: say "No Results" when nothing shows,
    /// once, and never again for the same filter while the panel is open.
    fn settle_browse(&mut self, change: u64) {
        if change != self.browse_change || self.browse_said == Some(change) {
            return;
        }
        let Some(filter) = self.browse_filter.clone() else {
            return;
        };
        let empty = self.browser.as_ref().is_some_and(|b| b.item_count() == 0);
        if empty && !self.browse_said_before.contains(&filter) {
            self.browse_said = Some(change);
            if self.browse_said_before.len() == MAX_SAID {
                self.browse_said_before.remove(0);
            }
            self.browse_said_before.push(filter);
        }
    }

    /// Whether the browser says "No Results" aloud now: its filter stood for [`STATUS_SETTLE`] with nothing to show,
    /// and it was not said for that filter before.
    pub fn browse_no_results_said(&self) -> bool {
        self.browse_said == Some(self.browse_change)
            && self.browser.as_ref().is_some_and(|b| b.item_count() == 0)
    }

    /// How many times the browser's filter changed ([`Message::BrowseSettled`]).
    pub fn browse_change(&self) -> u64 {
        self.browse_change
    }

    /// After every change: the providers, the shown icons, the selected result's preview, the window's size and the
    /// keyboard's place.
    fn changed(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        if !self.nav.is_visible() {
            return commands;
        }
        if self.nav.view == View::Search {
            commands.extend(self.search());
            let focus = match self.panel.focus() {
                Focus::Field => NavFocus::Field,
                Focus::List => NavFocus::Content,
            };
            self.nav
                .apply(Intent::Query(!self.panel.query().is_empty()));
            self.nav.apply(Intent::Focus(focus));
        }
        if let Some(b) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        {
            let focus = match b.zone() {
                Zone::Categories | Zone::Options => NavFocus::Categories,
                Zone::Content if b.selected().is_some() => NavFocus::Content,
                Zone::Content => NavFocus::Field,
            };
            let (query, category) = (!b.query().is_empty(), b.category().is_some());
            self.nav.apply(Intent::Query(query));
            self.nav.apply(Intent::Category(category));
            self.nav.apply(Intent::Focus(focus));
        }
        if self.icons.len() > MAX_ICONS {
            self.icons.clear();
        }
        if self.nav.view == View::Search {
            for row in view::visible_rows(&self.panel) {
                let view::Row::Item { flat, .. } = row else {
                    continue;
                };
                let Some(item) = self.panel.results().get(flat) else {
                    continue;
                };
                if !self.icons.contains_key(&item.id) {
                    let sources = self.host.icon(item);
                    if !sources.is_empty() {
                        self.icons.insert(item.id.clone(), sources);
                    }
                }
            }
            commands.extend(self.ask_preview());
        }
        commands.extend(self.watch_browse_filter());
        commands.extend(self.load_app_icons());
        let now = self.now().mono;
        let target = self.target_shape();
        if self.settled {
            self.motion.reshape(target, now);
            self.motion.settle(now);
        } else {
            // a change within the view shown (results, the output's size) reshapes it; a fold or unfold under way
            // keeps its course
            let header = self.motion.frame(now).header;
            let settled_in = match self.nav.view {
                View::Search => header == crate::motion::Header::Search,
                View::Browse => header == crate::motion::Header::Browse,
                View::Hidden => false,
            };
            if settled_in {
                self.motion.reshape(target, now);
            }
        }
        commands.extend(self.resize());
        commands
    }

    /// The window holds the shape presented and the one it heads for; it shrinks once the shape settled.
    fn resize(&mut self) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let target = self.motion.target();
        // settled, the window is the target's (what the motion presents may be a hair off it); while the shape
        // changes, it holds both and never shrinks
        let needed = if self.motion.animating(now) {
            let both = view::window_extent(target.union(self.motion.frame(now).shape));
            Size::new(
                both.width.max(self.extent.width),
                both.height.max(self.extent.height),
            )
        } else {
            view::window_extent(target)
        };
        if needed != self.extent {
            self.extent = needed;
            self.host.resize(needed);
        }
        Vec::new()
    }

    /// The window's size as last given.
    pub fn extent(&self) -> Size {
        self.extent
    }

    /// The selected result's preview: asked once per selection, with a deadline.
    fn ask_preview(&mut self) -> Vec<Command<Message>> {
        let selected = self.panel.selected_item().cloned();
        if selected.as_ref().map(|item| &item.id) == self.previewed.as_ref() {
            return Vec::new();
        }
        self.previewed = selected.as_ref().map(|item| item.id.clone());
        self.preview = PreviewState::None;
        let Some(item) = selected else {
            return Vec::new();
        };
        let mut commands = self.host.preview(&item);
        if !commands.is_empty() {
            self.preview = PreviewState::Waiting(item.id.clone());
            commands.extend(
                self.host
                    .after(PREVIEW_TIMEOUT, Message::PreviewOverdue(item.id.clone())),
            );
        }
        commands
    }

    /// Ask for the icons of the browser's applications near the view, nearest first, while fewer than
    /// [`MAX_APP_ICON_LOADS`] are being loaded. The host loads and decodes them off the loop; each that arrives
    /// ([`Message::AppIcon`]) makes room for the next.
    fn load_app_icons(&mut self) -> Vec<Command<Message>> {
        let Some(browser) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        else {
            return Vec::new();
        };
        let room = MAX_APP_ICON_LOADS.saturating_sub(self.app_icons_loading.len());
        let wanted: Vec<App> = browser
            .nearby()
            .into_iter()
            .filter_map(|i| browser.item(i))
            .filter(|a| {
                !self.app_icons.contains_key(&a.id) && !self.app_icons_loading.contains(&a.id)
            })
            .take(room)
            .cloned()
            .collect();
        let mut commands = Vec::new();
        for app in &wanted {
            self.app_icons_loading.insert(app.id.clone());
            commands.extend(self.host.app_icon(app));
        }
        commands
    }

    /// The applications whose icons are being loaded now.
    pub fn app_icons_loading(&self) -> usize {
        self.app_icons_loading.len()
    }

    /// Carry out what the panel asked for.
    fn outcome(&mut self, outcome: PanelEvent) -> Vec<Command<Message>> {
        match outcome {
            PanelEvent::None | PanelEvent::Changed | PanelEvent::Edit => Vec::new(),
            PanelEvent::Close => {
                self.hide(false);
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
            Activation::Run(spec) => self.ask_token(spec, None),
            Activation::OpenApp { desktop_id, spec } => self.ask_token(spec, Some(desktop_id)),
            Activation::Nothing => {
                eprintln!("cupertino-launcher: the entry has no usable command");
                Vec::new()
            }
        };
        self.hide(false);
        commands
    }

    /// Ask for the activation token of `spec` while the panel's window still exists. `opens` is the application it
    /// starts when that is to be brought forward instead when it runs.
    fn ask_token(&mut self, spec: CommandSpec, opens: Option<String>) -> Vec<Command<Message>> {
        let launch = self.next_launch;
        self.next_launch += 1;
        let app_id = spec.desktop_id.clone();
        self.pending.insert(launch, Pending { spec, opens });
        vec![Command::ActivationToken {
            window: WindowKey(Key::from("panel")),
            app_id,
            on_token: Box::new(move |token| Message::Launch(launch, token)),
        }]
    }

    /// The token of `launch` arrived: an application to open is first brought forward if it runs (the server is
    /// asked), anything else starts.
    fn launch(&mut self, launch: u64, token: Option<String>) {
        let Some(Pending { spec, opens }) = self.pending.remove(&launch) else {
            return;
        };
        if let Some(desktop_id) = opens {
            if self.host.bring_forward(&desktop_id, launch) {
                self.opening.insert(
                    launch,
                    Opening {
                        desktop_id,
                        spec,
                        token,
                    },
                );
                return;
            }
        }
        self.host.run(spec, token);
    }

    /// The server's answer for the application opened as `launch`, as a click on its dock tile goes on: nothing more
    /// when it came forward; a resident one is asked for a window over D-Bus when its entry allows, else started again;
    /// one that does not run is started.
    fn opened(&mut self, launch: u64, running: Running) -> Vec<Command<Message>> {
        let Some(opening) = self.opening.remove(&launch) else {
            return Vec::new();
        };
        match running {
            Running::Raised => Vec::new(),
            Running::NotRunning => {
                self.host.run(opening.spec, opening.token);
                Vec::new()
            }
            Running::Resident => {
                let source = self
                    .launcher
                    .entry(&opening.desktop_id)
                    .and_then(|e| e.source.clone());
                let bus_id = opening
                    .desktop_id
                    .strip_suffix(".desktop")
                    .unwrap_or(&opening.desktop_id)
                    .to_owned();
                match self
                    .host
                    .ask_for_window(&bus_id, source.as_deref(), opening.token.clone())
                {
                    Some(call) => {
                        self.opening.insert(launch, opening);
                        vec![Command::Perform(Box::pin(async move {
                            Message::Asked(launch, call.await)
                        }))]
                    }
                    None => {
                        self.host.run(opening.spec, opening.token);
                        Vec::new()
                    }
                }
            }
        }
    }

    /// Open the browser's application `id` as the search would.
    fn launch_app(&mut self, id: &str) -> Vec<Command<Message>> {
        let now = self.now();
        match self.launcher.app_result(id) {
            Some(item) => {
                let activation = self.launcher.activate(&item, "", now.unix);
                self.activated(activation)
            }
            None => Vec::new(),
        }
    }

    /// Shortcut commands of the panel that need the host.
    fn command(&mut self, command: PanelCommand) -> Vec<Command<Message>> {
        match command {
            PanelCommand::Reveal => {
                let reveal = self.panel.selected_item().and_then(|item| {
                    self.launcher
                        .actions(item)
                        .iter()
                        .position(|a: &ItemAction| a.kind == ActionKind::Reveal)
                });
                if let Some(index) = reveal {
                    let now = self.now();
                    let flat = self.panel.selected();
                    let outcome = self
                        .panel
                        .activate_action(flat, index, &mut self.launcher, now);
                    return self.outcome(outcome);
                }
            }
            PanelCommand::WebSearch => {
                match (&self.launcher.config.web_search_url, self.panel.query()) {
                    (Some(url), query) if !query.is_empty() => {
                        let target = url.replace("{}", &percent_encode(query));
                        self.hide(false);
                        self.host.open(&target);
                    }
                    _ => eprintln!(
                        "cupertino-launcher: no web search URL configured (`web_search_url`)"
                    ),
                }
            }
            // Space or Primary+Y: the selected result's preview, enlarged; again to go back
            PanelCommand::Preview => {
                if self.nav.nested == Some(Nested::Preview) {
                    return self.intent(Intent::Done);
                }
                if self.panel.selected_item().is_some() {
                    self.panel_focus_list();
                    return self.intent(Intent::Open(Nested::Preview));
                }
            }
            other => eprintln!("cupertino-launcher: {other:?} is not provided"),
        }
        Vec::new()
    }

    /// The keyboard is in the result list (a preview opens over it).
    fn panel_focus_list(&mut self) {
        self.nav.apply(Intent::Focus(NavFocus::Content));
    }

    /// How the panel's keys are split between the panel and the query field now.
    pub fn key_mode(&self) -> KeyMode {
        match (self.nav.view, self.nav.nested) {
            (_, Some(Nested::ViewOptions)) => KeyMode::Menu,
            (_, Some(Nested::Preview)) => KeyMode::Preview,
            (View::Browse, _) => KeyMode::Browse {
                selecting: self
                    .browser
                    .as_ref()
                    .is_some_and(|b| b.zone() != Zone::Content || b.selected().is_some()),
            },
            _ => KeyMode::Search(self.panel.focus()),
        }
    }

    /// A key of the panel.
    fn key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
        let now = self.now();
        if mods.only_primary() && key == PanelKey::Char('1') {
            return self.intent(Intent::Browse);
        }
        match self.nav.nested {
            Some(Nested::ViewOptions) => return self.menu_key(key),
            Some(Nested::Preview) => match key {
                PanelKey::Escape | PanelKey::Char(' ') => return self.intent(Intent::Escape),
                _ => {}
            },
            _ => {}
        }
        if key == PanelKey::Escape {
            return self.intent(Intent::Escape);
        }
        if self.nav.view == View::Browse {
            return self.browse_key(key, mods);
        }
        let outcome = self.panel.handle_key(key, mods, &mut self.launcher, now);
        let mut commands = self.outcome(outcome);
        // a shortcut that closes the panel (settings, help)
        if !self.panel.is_visible() && self.nav.is_visible() {
            self.hide(false);
        }
        if self.nav.nested == Some(Nested::Preview) && self.panel.selected_item().is_none() {
            commands.extend(self.intent(Intent::Done));
        }
        commands
    }

    fn browse_key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let key = match (key, mods.shift) {
            (PanelKey::Left, _) => browse::Key::Left,
            (PanelKey::Right, _) => browse::Key::Right,
            (PanelKey::Up, _) => browse::Key::Up,
            (PanelKey::Down, _) => browse::Key::Down,
            (PanelKey::Home, _) => browse::Key::Home,
            (PanelKey::End, _) => browse::Key::End,
            (PanelKey::PageUp, _) => browse::Key::PageUp,
            (PanelKey::PageDown, _) => browse::Key::PageDown,
            (PanelKey::Enter, _) => browse::Key::Return,
            (PanelKey::Char(' '), _) => browse::Key::Space,
            (PanelKey::Tab, false) => browse::Key::Tab,
            (PanelKey::Tab, true) => browse::Key::BackTab,
            _ => return Vec::new(),
        };
        let Some(browser) = self.browser.as_mut() else {
            return Vec::new();
        };
        match browser.key(key, now) {
            Some(browse::Outcome::Launch(id)) => self.launch_app(&id),
            Some(browse::Outcome::ViewOptions) => self.intent(Intent::Open(Nested::ViewOptions)),
            None => Vec::new(),
        }
    }

    /// A key that reaches the panel while the view options are open. Their menu holds the keyboard and moves between
    /// its items itself, so only a key the panel still hears matters: Escape closes the menu, everything else waits.
    fn menu_key(&mut self, key: PanelKey) -> Vec<Command<Message>> {
        match key {
            PanelKey::Escape => self.intent(Intent::Escape),
            _ => Vec::new(),
        }
    }

    /// Lay the browser out as `arrangement`; the view options close.
    fn arrange(&mut self, arrangement: Arrangement) -> Vec<Command<Message>> {
        self.arrangement = arrangement;
        if let Some(b) = self.browser.as_mut() {
            b.set_arrangement(arrangement);
        }
        if self.nav.nested == Some(Nested::ViewOptions) {
            self.intent(Intent::Done)
        } else {
            Vec::new()
        }
    }

    fn input(&mut self, input: Input) -> Vec<Command<Message>> {
        let now = self.now();
        match input {
            Input::Key(key, mods) => self.key(key, mods),
            Input::Query(change) => {
                self.revision = change.revision;
                if self.nav.nested == Some(Nested::Preview) && change.text != self.panel.query() {
                    self.nav.apply(Intent::Done);
                }
                if self.nav.view == View::Browse {
                    self.panel.set_text_quiet(&change.text);
                    if let Some(b) = self.browser.as_mut() {
                        b.set_query(&change.text);
                    }
                } else {
                    self.panel
                        .edit(&self.launcher, now, |field| field.set_text(&change.text));
                }
                Vec::new()
            }
            Input::Hover(flat) => {
                self.panel.set_selected(flat);
                Vec::new()
            }
            Input::Activate(flat) => {
                let outcome = self.panel.activate_index(flat, &mut self.launcher, now);
                self.outcome(outcome)
            }
            Input::RunAction(flat, index) => {
                let outcome = self
                    .panel
                    .activate_action(flat, index, &mut self.launcher, now);
                self.outcome(outcome)
            }
            Input::Dismiss => self.intent(Intent::Dismiss),
            // the view options menu, a popup of the panel's window, holds the keyboard while it is open; when the
            // compositor dismisses it because something else took the keyboard, the panel's leave comes after the
            // menu's close and closes the panel
            Input::KeyboardLeft if self.nav.nested == Some(Nested::ViewOptions) => Vec::new(),
            Input::KeyboardLeft => self.intent(Intent::Dismiss),
            Input::BrowseHover(index) => {
                if let Some(b) = self.browser.as_mut() {
                    b.hover(index);
                }
                Vec::new()
            }
            Input::BrowseActivate(index) => {
                let id = self
                    .browser
                    .as_ref()
                    .and_then(|b| b.item(index))
                    .map(|a| a.id.clone());
                match id {
                    Some(id) => self.launch_app(&id),
                    None => Vec::new(),
                }
            }
            Input::BrowseCategory(category) => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_zone(Zone::Categories);
                    b.toggle_category(category);
                }
                Vec::new()
            }
            Input::BrowseScroll(dy) => {
                if let Some(b) = self.browser.as_mut() {
                    b.scroll_by(dy, now.mono);
                }
                self.animate()
            }
            // the pointer (or assistive technology) pressed the button: the keys stay where they were, so the button
            // shows no focus ring once the menu closes
            Input::ViewOptions => {
                if self.nav.nested == Some(Nested::ViewOptions) {
                    self.intent(Intent::Done)
                } else {
                    self.intent(Intent::Open(Nested::ViewOptions))
                }
            }
            Input::Arrange(arrangement) => self.arrange(arrangement),
            Input::CloseMenu => {
                if self.nav.nested == Some(Nested::ViewOptions) {
                    self.intent(Intent::Done)
                } else {
                    Vec::new()
                }
            }
            Input::ClosePreview => self.intent(Intent::Done),
        }
    }
}

/// `preview` as the pane can show it ([`preview::bounded`] with the pane's picture limit).
pub fn bounded(preview: Preview) -> Preview {
    preview::bounded(preview, view::PREVIEW_IMAGE)
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
    let mut commands = match message {
        Message::Toggle => model.intent(Intent::Toggle),
        Message::Browse => model.intent(Intent::Browse),
        Message::Input(_) if !model.nav.is_visible() => Vec::new(),
        Message::Input(input) => model.input(input),
        Message::Indexed(index) => {
            model.indexing = false;
            model.file_index = Some(index);
            // the open query is asked again, now with the files
            model.searched = None;
            Vec::new()
        }
        Message::Launch(launch, token) => {
            model.launch(launch, token);
            Vec::new()
        }
        Message::Opened(launch, running) => model.opened(launch, running),
        Message::Asked(launch, answered) => {
            // an application that did not answer is started again with the token, as the dock does
            if let Some(opening) = model.opening.remove(&launch) {
                if !answered {
                    model.host.run(opening.spec, opening.token);
                }
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
        Message::Locked => {
            if model.presenting() {
                model.hide(true);
            }
            Vec::new()
        }
        Message::Remap => {
            model.hidden += 1;
            Vec::new()
        }
        Message::Previewed(id, preview) => {
            // only for the result last asked about; one that comes after its deadline still shows
            if model.previewed.as_deref() == Some(id.as_str()) {
                model.preview = PreviewState::Ready(Arc::new(bounded((*preview).clone())));
            }
            Vec::new()
        }
        Message::PreviewOverdue(id) => {
            let answered = matches!(model.preview, PreviewState::Ready(_));
            if model.previewed.as_deref() == Some(id.as_str()) && !answered {
                model.preview = PreviewState::Unavailable;
            }
            Vec::new()
        }
        Message::Provided(update) => {
            if model.tracker.apply(update) == crate::providers::Applied::Taken {
                let results =
                    model
                        .launcher
                        .compose(model.panel.query(), now.unix, model.tracker.sections());
                model.panel.set_results(results);
            }
            Vec::new()
        }
        Message::Overdue(generation) => {
            if generation == model.tracker.generation() {
                // the timer fired at the deadline
                let deadline = model.tracker.deadline().unwrap_or(now.mono);
                if model.tracker.expire(deadline) {
                    eprintln!("cupertino-launcher: the file search did not answer in time");
                }
            }
            Vec::new()
        }
        Message::AppIcon(id, sources) => {
            model.app_icons_loading.remove(&id);
            model.app_icons.insert(id, sources);
            Vec::new()
        }
        Message::Settled(change) => {
            model.settle(change);
            Vec::new()
        }
        Message::BrowseSettled(change) => {
            model.settle_browse(change);
            Vec::new()
        }
        Message::Output(size) => {
            model.output = Some(size);
            let height = model.browse_height();
            if let Some(b) = model.browser.as_mut() {
                b.set_height(height);
            }
            Vec::new()
        }
        Message::ReduceMotion(on) => {
            model.motion.set_reduce_motion(on);
            Vec::new()
        }
        Message::Unheard => {
            eprintln!("cupertino-launcher: the bus interface stopped");
            vec![Command::Exit]
        }
    };
    commands.extend(model.changed());
    if !model.presenting() {
        model.browser = None;
    }
    commands
}

/// The shown panel and its catcher, or the window kept mapped while it is hidden.
pub fn view(model: &Model) -> Vec<Window<Message>> {
    if !model.presenting() {
        return vec![view::hidden_window(model.hidden, Message::Remap)];
    }
    let actions = model
        .panel
        .selected_item()
        .map(|item| model.launcher.actions(item))
        .unwrap_or_default();
    let frame = model.presented();
    let browser = model
        .browser
        .as_ref()
        .filter(|_| model.nav.view == View::Browse || frame.content > 0.0);
    let panel = PanelView {
        panel: &model.panel,
        revision: model.revision,
        frame,
        extent: model.extent,
        open: model.nav.is_visible(),
        keys: model.key_mode(),
        nested: model.nav.nested,
        status: model.status(),
        said: model.said(),
        icons: &model.icons,
        actions: &actions,
        preview: model.preview(),
        preview_unavailable: model.preview == PreviewState::Unavailable,
        browser,
        app_icons: &model.app_icons,
        browse_no_results_said: model.browse_no_results_said(),
        now: model.now().mono,
    };
    let mut windows = vec![view::panel_window(&panel)];
    if model.nav.is_visible() {
        windows.push(view::catcher_window());
    }
    if model.nav.nested == Some(Nested::ViewOptions) && browser.is_some() {
        windows.extend(view::browse::options_menu(
            view::panel_window_key(),
            model.arrangement,
            view::options_menu_key(),
        ));
    }
    windows
}
