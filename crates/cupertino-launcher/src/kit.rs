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
use crate::modes::{self, Mode};
use crate::motion::{Frame, Header, Motion, Shape, RESPLIT_DELAY};
use crate::nav::{Focus as NavFocus, Intent, Nav, Nested, Step, View};
use crate::panel::{
    Focus, Key as PanelKey, Mods as PanelMods, Now, Panel, PanelCommand, PanelEvent,
};
use crate::preview::{self, Preview};
use crate::providers::{Cancel, Generation, Provider, Reply, Status, Tracker, Update};
use crate::scope::{self, FileItem, FilesScope, Filter, ScopeAnswer, ScopeLayout, ScopeRequest};
use crate::view::{self, Input, KeyMode, PanelView, MAX_ROWS};
use cupertino_defaults::launcher::LauncherSettings;
use cupertino_kit::controls::{FrameClock, ImageSource};
use cupertino_kit::core::{Command, Element, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
/// The most file thumbnails the files scope has being loaded at once, and the most pictures it keeps; past it the
/// cache starts again.
pub const MAX_FILE_THUMB_LOADS: usize = 12;
const MAX_FILE_IMAGES: usize = 400;

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
    /// Search the file index for the files scope; the commands deliver the answer as [`Message::Scoped`]. The default
    /// runs it at once.
    fn scope_search(&mut self, request: ScopeRequest) -> Vec<Command<Message>> {
        let answer = request.run();
        vec![Command::Perform(Box::pin(async move {
            Message::Scoped(answer)
        }))]
    }
    /// The renditions of the icon of `item`'s type, at once: what the files scope shows until its thumbnail loads.
    /// None to draw a placeholder (the default).
    fn file_icon(&mut self, _item: &FileItem) -> Vec<ImageSource> {
        Vec::new()
    }
    /// Load and decode the thumbnail of `item` off the loop; the commands deliver it as [`Message::FileThumb`], none
    /// when the file has none. The default delivers none at once.
    fn file_thumb(&mut self, item: &FileItem) -> Vec<Command<Message>> {
        let path = item.path.clone();
        vec![Command::Perform(Box::pin(async move {
            Message::FileThumb(path, Vec::new())
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
    /// How the drag of an application out of the browser goes.
    Dragged(cupertino_kit::platform::DragProgress),
    /// `org.cupertino.Launcher1.Toggle`: the search field opens, or whatever is up closes.
    Toggle,
    /// `org.cupertino.Launcher1.ToggleApplications` (the dock's app-browser request): the browser opens, the search
    /// field unfolds into it, or the browser closes.
    Browse,
    /// The files scope opens, the search field unfolds into it, or it closes.
    Files,
    /// The files scope's search finished.
    Scoped(ScopeAnswer),
    /// The thumbnail of the file at this path arrived ([`Host::file_thumb`]); none when it has none.
    FileThumb(PathBuf, Vec<ImageSource>),
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
    /// The close the compositor runs ([`Model::compositor_layers`]) is over by now.
    CloseOver,
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
    /// An application is being dragged out of the browser: the catcher steps aside so the dock and the desktop under
    /// the pointer take the drag.
    pub dragging: bool,
    /// Where the file index is built from; `None` without file results.
    pub files: Option<FileSearchConfig>,
    /// Put every transition at its end at once (the server's settled test clock).
    pub settled: bool,
    host: Box<dyn Host>,
    indexing: bool,
    /// What `launcher.conf` held when the panel last opened.
    launcher_settings: LauncherSettings,
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
    /// The animation frame [`Model::frame`] names was asked of the host and has not come yet.
    frame_waiting: bool,
    /// The hidden window's generation: each one is a new window, so one the server closed is not kept.
    pub hidden: u64,
    preview: PreviewState,
    /// The id of the result the preview was last asked for.
    previewed: Option<String>,
    /// The browser's arrangement, kept from one opening to the next.
    arrangement: Arrangement,
    /// The output's size, once known.
    output: Option<Size>,
    /// The files scope, from its first opening on.
    pub scope: Option<FilesScope>,
    /// Pictures of the scope's files by path: their type's icon, then their thumbnail.
    pub file_images: HashMap<PathBuf, Vec<ImageSource>>,
    /// The files whose thumbnails were asked for, and those still being loaded.
    file_thumbed: HashSet<PathBuf>,
    file_thumbs_loading: usize,
    /// The number of the scope's last search, and what it asked.
    scope_generation: u64,
    scope_asked: Option<(String, Option<Filter>)>,
    /// The scope shown last, which a fold keeps fading out.
    last_scope: Option<View>,
    /// The mode button the pointer is over, and where the keys are among the buttons.
    pub mode_hover: Option<usize>,
    pub mode_focus: modes::Focus,
    /// The query was kept from the last opening: it is shown selected and nothing is searched or filtered by it until
    /// it is edited.
    held: bool,
    /// The query is shown selected (the kept one, until the first edit).
    select_query: bool,
    /// The panel just opened: its buttons bud out at once.
    just_shown: bool,
    /// The primary modifier is held while the panel has the keyboard.
    pub primary_held: bool,
    /// The window system shows the panel's parts as compositor layers ([`view::layers`]): while the panel rests it
    /// draws them so, and the compositor runs its close.
    pub compositor_layers: bool,
    /// The close the compositor runs: the panel as it was when the close began, which the panel's layers keep showing
    /// until it is over, and the layers' changes not yet sent.
    handed: Option<Frame>,
    layer_changes: Vec<(u64, Vec<cupertino_kit::platform::LayerChange>)>,
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
            dragging: false,
            files: None,
            settled: false,
            host,
            indexing: false,
            launcher_settings: LauncherSettings::UNSET,
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
            frame_waiting: false,
            hidden: 0,
            preview: PreviewState::None,
            previewed: None,
            arrangement: Arrangement::Grid,
            output: None,
            scope: None,
            file_images: HashMap::new(),
            file_thumbed: HashSet::new(),
            file_thumbs_loading: 0,
            scope_generation: 0,
            scope_asked: None,
            last_scope: None,
            mode_hover: None,
            mode_focus: modes::Focus::default(),
            held: false,
            select_query: false,
            just_shown: false,
            primary_held: false,
            compositor_layers: false,
            handed: None,
            layer_changes: Vec::new(),
        }
    }

    fn now(&self) -> Now {
        self.host.now()
    }

    /// What is presented now: while the compositor runs the close, the panel as the close began, which its layers
    /// keep.
    pub fn presented(&self) -> Frame {
        self.handed
            .unwrap_or_else(|| self.motion.frame(self.now().mono))
    }

    /// Whether the panel's parts draw into compositor layers of their own now: while it rests, open, and while the
    /// compositor runs its close; never under Reduce Motion, where nothing is handed over.
    pub fn layered(&self) -> bool {
        self.compositor_layers
            && !self.motion.reduce_motion()
            && (self.handed.is_some()
                || (self.nav.is_visible() && !self.motion.animating(self.now().mono)))
    }

    /// Whether the compositor runs the panel's close now.
    pub fn close_handed(&self) -> bool {
        self.handed.is_some()
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
        if self.held {
            return Status::Idle;
        }
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

    /// The files scope's height on the output known.
    fn scope_height(&self) -> f64 {
        match self.output {
            Some(output) => ScopeLayout::height_on(
                output.height,
                (output.height * view::TOP_FRACTION).round(),
                BROWSE_BOTTOM_ROOM,
            ),
            None => ScopeLayout::HEIGHT,
        }
    }

    /// The scopes the mode buttons lead to, in order: the applications, and the files while they are searched.
    pub fn modes(&self) -> Vec<Mode> {
        let mut modes = vec![Mode::Applications];
        if self.files.is_some() {
            modes.push(Mode::Files);
        }
        modes
    }

    /// Whether the mode buttons stand beside the search field: it is the plain bar, with nothing below it.
    fn wants_modes(&self) -> bool {
        self.nav.view == View::Search
            && self.nav.nested.is_none()
            && view::search_shape(&self.panel, &self.status(), self.nav.nested).height
                <= view::COLLAPSED_HEIGHT
    }

    /// The text the scopes filter by: the query, unless it was kept from the last opening and not yet edited.
    fn filter_text(&self) -> String {
        if self.held {
            String::new()
        } else {
            self.panel.query().to_owned()
        }
    }

    /// The shape the current view heads for.
    fn target_shape(&self) -> Shape {
        match self.nav.view {
            View::Files => view::scope_shape(self.scope_height()),
            View::Browse => view::browse_shape(self.browse_height()),
            _ => view::search_shape(&self.panel, &self.status(), self.nav.nested),
        }
    }

    /// Read `launcher.conf` again and apply it: the kinds of result listed, the top hit, and the folders the file
    /// search leaves out (a change of those builds the index again).
    fn apply_launcher_settings(&mut self) {
        let Some(path) = LauncherSettings::path() else {
            return;
        };
        let (settings, _) = match LauncherSettings::load(&path, Some(&self.launcher_settings)) {
            Ok(loaded) => loaded,
            Err(_) => return,
        };
        if settings == self.launcher_settings {
            return;
        }
        let config = &mut self.launcher.config;
        // a kind that is off lists no rows: its section's limit is zero
        let defaults = LauncherConfig::default();
        config.max_apps = if settings.applications {
            defaults.max_apps
        } else {
            0
        };
        config.max_folders = if settings.folders {
            defaults.max_folders
        } else {
            0
        };
        config.max_files = if settings.files {
            defaults.max_files
        } else {
            0
        };
        config.show_calculator = settings.calculator;
        config.top_hit = settings.top_hit;
        if config.include_actions != settings.actions {
            config.include_actions = settings.actions;
            let entries = self.launcher.entries().to_vec();
            self.launcher.set_entries(entries);
        }
        if settings.excluded != self.launcher_settings.excluded {
            if let Some(files) = self.files.as_mut() {
                files.extra_patterns = settings.exclusion_patterns(&files.root);
                // the index is built again from the new rules the next time the panel needs it
                self.file_index = None;
                self.launcher.set_file_index(None);
                self.indexing = false;
            }
        }
        self.launcher_settings = settings;
    }

    /// Open the browser (afresh) with the launcher's applications and recents, in the remembered arrangement.
    fn open_browser(&mut self) {
        let now = self.now();
        let recent = self.launcher.recent_apps(browse::COLUMNS, now.unix);
        let mut browser =
            Browser::from_entries(self.launcher.entries(), &recent, self.browse_height());
        browser.set_arrangement(self.arrangement);
        browser.set_query(&self.filter_text());
        // the scroll indicator shows as the browser comes into view; on the settled clock it has already faded
        if !self.settled {
            browser.reveal_scroller(now.mono);
        }
        self.browser = Some(browser);
    }

    /// Open the files scope (afresh), for the query it filters by.
    fn open_scope(&mut self) {
        let mut files = FilesScope::new(self.scope_height());
        files.set_query(&self.filter_text());
        self.scope = Some(files);
        self.scope_asked = None;
    }

    /// Carry out a navigation step.
    fn step(&mut self, step: Step) -> Vec<Command<Message>> {
        let now = self.now();
        let mut commands = Vec::new();
        match step {
            Step::Stay | Step::Noted => {}
            Step::Show(view) => {
                // the previous query is kept and shown selected, so typing replaces it; nothing filters by it
                let kept = self.panel.query().to_owned();
                self.apply_launcher_settings();
                self.panel.show(&self.launcher, now);
                self.panel.settle_appear();
                self.searched = None;
                self.held = !kept.is_empty();
                self.select_query = self.held;
                self.mode_hover = None;
                self.mode_focus = modes::Focus::default();
                self.just_shown = true;
                // a close the compositor ran is turned around by the panel itself, which draws its opening
                self.handed = None;
                self.layer_changes.clear();
                match view {
                    View::Browse => {
                        self.panel.set_text_quiet(&kept);
                        self.open_browser();
                        self.last_scope = Some(View::Browse);
                    }
                    View::Files => {
                        self.panel.set_text_quiet(&kept);
                        self.open_scope();
                        self.last_scope = Some(View::Files);
                    }
                    _ => {}
                }
                self.motion
                    .show_in(self.target_shape(), header_of(view), now.mono);
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
                self.held = false;
                self.select_query = false;
                self.revision += 1;
                self.motion.fold(self.target_shape(), now.mono);
            }
            Step::Unfold => {
                let query = self.filter_text();
                let text = self.panel.query().to_owned();
                self.panel.set_text_quiet(&text);
                self.last_scope = Some(View::Browse);
                self.mode_hover = None;
                self.tracker.cancel();
                self.preview = PreviewState::None;
                let height = self.browse_height();
                match self.browser.as_mut() {
                    Some(browser) => {
                        browser.set_height(height);
                        browser.set_query(&query);
                        browser.deselect();
                        if !self.settled {
                            browser.reveal_scroller(now.mono);
                        }
                    }
                    None => self.open_browser(),
                }
                self.motion.unfold(self.target_shape(), now.mono);
            }
            Step::UnfoldFiles => {
                let text = self.panel.query().to_owned();
                self.panel.set_text_quiet(&text);
                self.tracker.cancel();
                self.preview = PreviewState::None;
                self.last_scope = Some(View::Files);
                self.mode_hover = None;
                let height = self.scope_height();
                match self.scope.as_mut() {
                    Some(files) => files.set_height(height),
                    None => self.open_scope(),
                }
                let query = self.filter_text();
                if let Some(files) = self.scope.as_mut() {
                    files.set_query(&query);
                    files.deselect();
                }
                self.motion
                    .unfold_to(self.target_shape(), Header::Files, now.mono);
            }
            Step::Leave(_, _) => {}
            Step::Enter(_) => {}
            Step::FocusField => self.panel.focus_field(),
            Step::ClearQuery => {
                self.held = false;
                self.select_query = false;
                match self.nav.view {
                    View::Browse => {
                        self.panel.set_text_quiet("");
                        if let Some(b) = self.browser.as_mut() {
                            b.set_query("");
                        }
                    }
                    View::Files => {
                        self.panel.set_text_quiet("");
                        if let Some(files) = self.scope.as_mut() {
                            files.set_query("");
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
        // the navigation has already left the panel: it was drawn as layers if it rested, shown
        let layered = self.compositor_layers
            && !self.motion.reduce_motion()
            && self.handed.is_none()
            && !self.motion.animating(now.mono)
            && !self.motion.gone(now.mono);
        self.dragging = false;
        if self.nav.is_visible() {
            self.nav = Nav::HIDDEN;
        }
        self.mode_hover = None;
        self.mode_focus = modes::Focus::default();
        self.primary_held = false;
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
        // a panel drawn as layers at rest closes on the compositor's clock: its layers keep what they show and the
        // compositor grows, blurs and fades them on the curves the panel draws them on itself
        let rest = (layered && !now_gone && !self.settled).then(|| self.motion.frame(now.mono));
        self.motion.hide(now.mono);
        if now_gone || self.settled {
            self.motion.settle(now.mono);
        }
        if let Some(from) = rest {
            let to = self.motion.frame(now.mono + crate::motion::CLOSE_DURATION);
            self.layer_changes = view::layers::close_changes(
                self.extent,
                &from,
                &to,
                self.modes().len(),
                &self.motion,
                now.mono,
            );
            self.handed = Some(from);
        }
        if let Err(e) = self.launcher.frecency.save_if_dirty() {
            eprintln!("cupertino-launcher: launch history not saved: {e}");
        }
    }

    fn animate(&mut self) -> Vec<Command<Message>> {
        self.frame += 1;
        let now = self.now().mono;
        let scrolling = match self.nav.view {
            View::Browse => self.browser.as_ref().is_some_and(|b| b.animating(now)),
            View::Files => self.scope.as_ref().is_some_and(|s| s.animating(now)),
            _ => false,
        };
        // the panel's window sends the frame's message at its next frame callback ([`view`]); a close the compositor
        // runs asks for none
        self.frame_waiting = (self.motion.animating(now) && self.handed.is_none()) || scrolling;
        Vec::new()
    }

    /// Whether the panel moves by itself and its window asks the compositor for frames.
    pub fn animating(&self) -> bool {
        self.frame_waiting
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
            if !self.held {
                commands.extend(self.search());
            }
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
            let (query, category) = (!self.panel.query().is_empty(), b.category().is_some());
            self.nav.apply(Intent::Query(query));
            self.nav.apply(Intent::Category(category));
            self.nav.apply(Intent::Focus(focus));
        }
        if self.nav.view == View::Files {
            let text = self.filter_text();
            if let Some(files) = self.scope.as_mut() {
                files.set_query(&text);
                let focus = match files.zone() {
                    scope::Zone::Filters => NavFocus::Categories,
                    scope::Zone::Content if files.selected().is_some() => NavFocus::Content,
                    scope::Zone::Content => NavFocus::Field,
                };
                let (query, category) = (!self.panel.query().is_empty(), files.filter().is_some());
                self.nav.apply(Intent::Query(query));
                self.nav.apply(Intent::Category(category));
                self.nav.apply(Intent::Focus(focus));
            }
            commands.extend(self.ask_scope());
            commands.extend(self.load_file_images());
        }
        commands.extend(self.place_modes());
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
                View::Files => header == crate::motion::Header::Files,
                View::Hidden => false,
            };
            if settled_in {
                self.motion.reshape(target, now);
            }
        }
        commands.extend(self.resize());
        commands
    }

    /// The buttons stand beside the plain bar, and merge into the field when anything opens below it.
    fn place_modes(&mut self) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let want = if self.wants_modes() {
            self.modes().len()
        } else {
            0
        };
        let first = std::mem::take(&mut self.just_shown);
        let mut commands = Vec::new();
        if want != self.motion.modes_wanted() || (first && want > 0) {
            let delay = if first { 0.0 } else { RESPLIT_DELAY };
            self.motion.set_modes(want, now, delay);
            if self.settled {
                self.motion.settle(now);
            }
            // the buttons' own motion needs frames
            commands.extend(self.animate());
        }
        if want == 0 {
            self.mode_hover = None;
            self.mode_focus = modes::Focus::default();
        }
        commands
    }

    /// Ask the host for the files the scope shows, when its query or filter changed since it last asked. The answer
    /// comes back as [`Message::Scoped`]; the scope shows no spinner while it waits.
    fn ask_scope(&mut self) -> Vec<Command<Message>> {
        let (Some(files), Some(index)) = (self.scope.as_ref(), self.file_index.clone()) else {
            return Vec::new();
        };
        let asked = (files.query().to_owned(), files.filter());
        if self.scope_asked.as_ref() == Some(&asked) {
            return Vec::new();
        }
        self.scope_asked = Some(asked.clone());
        self.scope_generation += 1;
        let request = ScopeRequest {
            generation: self.scope_generation,
            text: asked.0,
            filter: asked.1,
            index,
            config: self.launcher.config.clone(),
            unix: self.now().unix,
        };
        self.host.scope_search(request)
    }

    /// Give the scope's files near the view their type's icon, and ask for the thumbnails of those that have one, a
    /// few at a time. A thumbnail that arrives ([`Message::FileThumb`]) takes the icon's place and makes room for the
    /// next.
    fn load_file_images(&mut self) -> Vec<Command<Message>> {
        let Some(files) = self.scope.as_ref() else {
            return Vec::new();
        };
        if self.file_images.len() > MAX_FILE_IMAGES {
            self.file_images.clear();
            self.file_thumbed.clear();
        }
        let near: Vec<FileItem> = files
            .nearby()
            .into_iter()
            .filter_map(|i| files.item(i).cloned())
            .collect();
        let mut commands = Vec::new();
        for item in near {
            if !self.file_images.contains_key(&item.path) {
                let icon = self.host.file_icon(&item);
                if !icon.is_empty() {
                    self.file_images.insert(item.path.clone(), icon);
                }
            }
            if !item.is_dir
                && self.file_thumbs_loading < MAX_FILE_THUMB_LOADS
                && !self.file_thumbed.contains(&item.path)
            {
                self.file_thumbed.insert(item.path.clone());
                self.file_thumbs_loading += 1;
                commands.extend(self.host.file_thumb(&item));
            }
        }
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
    /// The application at `index` of the browser dragged out as its desktop file, for the dock to keep or the desktop
    /// to hold; its icon follows the pointer, held at `grab`.
    fn drag_app(
        &mut self,
        index: usize,
        grab: cupertino_kit::geometry::Point,
    ) -> Vec<Command<Message>> {
        use cupertino_kit::controls::{Image, ImageFit};
        use cupertino_kit::core::{Drag, DragPreview};
        use cupertino_kit::platform::{ClipboardData, DropAction};
        let Some(id) = self
            .browser
            .as_ref()
            .and_then(|b| b.item(index))
            .map(|a| a.id.clone())
        else {
            return Vec::new();
        };
        let Some(path) = self.launcher.entry(&id).and_then(|e| e.source.clone()) else {
            return Vec::new();
        };
        let icon = BrowseLayout::ICON;
        let image = DragPreview {
            content: Element::new(Image {
                sources: self.app_icons.get(&id).cloned().unwrap_or_default(),
                size: Size::new(icon, icon),
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            }),
            size: Size::new(icon, icon),
            hotspot: grab,
        };
        self.dragging = true;
        vec![Command::StartDrag(Drag {
            window: view::panel_window_key(),
            data: vec![ClipboardData {
                mime: "text/uri-list".into(),
                bytes: format!("{}\r\n", file_uri(&path)).into_bytes(),
            }],
            actions: [DropAction::Copy].into(),
            image: Some(image),
            on_progress: Box::new(Message::Dragged),
        })]
    }

    /// The drag of an application went on: once it ends the catcher comes back, and a drop something took closes
    /// the panel, as a launch does.
    fn dragged(
        &mut self,
        progress: cupertino_kit::platform::DragProgress,
    ) -> Vec<Command<Message>> {
        use cupertino_kit::platform::DragProgress;
        match progress {
            DragProgress::Finished => {
                self.dragging = false;
                self.intent(Intent::Dismiss)
            }
            DragProgress::Cancelled => {
                self.dragging = false;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

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
            (View::Search, _) if self.mode_focus.on.is_some() => KeyMode::Modes,
            (View::Files, _) => KeyMode::Files {
                selecting: self
                    .scope
                    .as_ref()
                    .is_some_and(|s| s.zone() != scope::Zone::Content || s.selected().is_some()),
            },
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
        if mods.only_primary() {
            if let PanelKey::Char(c) = key {
                if let Some(index) = modes::index_of_digit(c) {
                    return match self.modes().get(index) {
                        Some(&mode) => self.press_mode(mode),
                        None => Vec::new(),
                    };
                }
            }
        }
        if let Some(commands) = self.mode_key(key, mods) {
            return commands;
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
        if self.nav.view == View::Files {
            return self.scope_key(key, mods);
        }
        let before = self.panel.query().to_owned();
        let outcome = self.panel.handle_key(key, mods, &mut self.launcher, now);
        if self.panel.query() != before {
            // a recalled search shows selected, so typing replaces it; a kind's token goes in with the caret after it
            self.held = false;
            self.select_query = key == PanelKey::Up;
            self.revision += 1;
        }
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

    /// Switch to the scope of `mode`, as its button or its shortcut asks.
    fn press_mode(&mut self, mode: Mode) -> Vec<Command<Message>> {
        self.mode_hover = None;
        self.mode_focus = modes::Focus::default();
        match mode {
            Mode::Applications => self.intent(Intent::Browse),
            Mode::Files => self.intent(Intent::Files),
        }
    }

    /// A key while the buttons stand beside the plain bar: Tab goes to them, along them and back to the field; Left and
    /// Right move between them, Return and Space press the one the keys are on, Escape gives the keys back to the field.
    /// `None` when the key is not the buttons'.
    fn mode_key(&mut self, key: PanelKey, mods: PanelMods) -> Option<Vec<Command<Message>>> {
        let count = self.modes().len();
        let shown = self.nav.view == View::Search
            && self.nav.nested.is_none()
            && self.motion.modes_wanted() > 0
            && {
                let m = self.presented().modes;
                m.count > 0 && m.spread > 0.5
            };
        if !shown {
            self.mode_focus = modes::Focus::default();
            return None;
        }
        match key {
            PanelKey::Tab => {
                match self.mode_focus.tab(mods.shift, count) {
                    modes::Move::To(i) => self.mode_focus.on = Some(i),
                    _ => self.mode_focus.on = None,
                }
                Some(Vec::new())
            }
            _ if self.mode_focus.on.is_none() => None,
            PanelKey::Left | PanelKey::Right => {
                if let Some(modes::Move::To(i)) =
                    self.mode_focus.step(key == PanelKey::Right, count)
                {
                    self.mode_focus.on = Some(i);
                }
                Some(Vec::new())
            }
            PanelKey::Enter | PanelKey::Char(' ') => {
                let mode = self
                    .mode_focus
                    .on
                    .and_then(|i| self.modes().get(i).copied());
                Some(match mode {
                    Some(mode) => self.press_mode(mode),
                    None => Vec::new(),
                })
            }
            PanelKey::Escape => {
                self.mode_focus.on = None;
                Some(Vec::new())
            }
            // anything else is typing: the keys go back to the field
            _ => {
                self.mode_focus.on = None;
                None
            }
        }
    }

    /// A key in the files scope: the arrows, Home, End and the pages move along the files, Return opens the selected
    /// one, Tab moves between the files and the filters.
    fn scope_key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
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
        let Some(files) = self.scope.as_mut() else {
            return Vec::new();
        };
        match files.key(key, now) {
            Some(scope::Outcome::Open(path)) => self.open_file(&path),
            None => Vec::new(),
        }
    }

    /// Open the scope's file or folder with its default handler; the launcher closes.
    fn open_file(&mut self, path: &std::path::Path) -> Vec<Command<Message>> {
        self.hide(false);
        self.host.open(&path.to_string_lossy());
        Vec::new()
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
                // an edit ends the kept query: from here on it filters and is searched
                self.held = false;
                self.select_query = false;
                self.mode_focus = modes::Focus::default();
                if self.nav.view == View::Browse {
                    self.panel.set_text_quiet(&change.text);
                    if let Some(b) = self.browser.as_mut() {
                        b.set_query(&change.text);
                    }
                } else if self.nav.view == View::Files {
                    self.panel.set_text_quiet(&change.text);
                    if let Some(files) = self.scope.as_mut() {
                        files.set_query(&change.text);
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
            Input::BrowseDrag(index, grab) => self.drag_app(index, grab),
            Input::BrowseCategory(category) => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_zone(Zone::Categories);
                    b.toggle_category(category);
                }
                Vec::new()
            }
            Input::BrowseScroll(dy) => {
                if self.nav.view == View::Files {
                    if let Some(files) = self.scope.as_mut() {
                        files.scroll_by(dy, now.mono);
                    }
                } else if let Some(b) = self.browser.as_mut() {
                    b.scroll_by(dy, now.mono);
                }
                self.animate()
            }
            Input::ModeHover(index) => {
                self.mode_hover = index;
                Vec::new()
            }
            Input::ModePress(index) => match self.modes().get(index) {
                Some(&mode) => self.press_mode(mode),
                None => Vec::new(),
            },
            Input::ScopeHover(index) => {
                if let Some(files) = self.scope.as_mut() {
                    files.hover(index);
                }
                Vec::new()
            }
            Input::ScopeActivate(index) => {
                let path = self
                    .scope
                    .as_ref()
                    .and_then(|s| s.item(index))
                    .map(|f| f.path.clone());
                match path {
                    Some(path) => self.open_file(&path),
                    None => Vec::new(),
                }
            }
            Input::Primary(held) => {
                self.primary_held = held;
                Vec::new()
            }
            Input::ScopeFilter(filter) => {
                if let Some(files) = self.scope.as_mut() {
                    files.set_zone(scope::Zone::Filters);
                    files.toggle_filter(filter);
                }
                Vec::new()
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
        Message::Files => model.intent(Intent::Files),
        Message::Scoped(answer) => {
            if answer.generation == model.scope_generation {
                if let Some(files) = model.scope.as_mut() {
                    files.answer(answer);
                }
            }
            Vec::new()
        }
        Message::FileThumb(path, sources) => {
            model.file_thumbs_loading = model.file_thumbs_loading.saturating_sub(1);
            if !sources.is_empty() {
                model.file_images.insert(path, sources);
            }
            Vec::new()
        }
        Message::Input(_) if !model.nav.is_visible() => Vec::new(),
        Message::Input(input) => model.input(input),
        Message::Indexed(index) => {
            model.indexing = false;
            model.file_index = Some(index);
            // the open query is asked again, now with the files
            model.searched = None;
            model.scope_asked = None;
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
        Message::Dragged(progress) => model.dragged(progress),
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
        Message::CloseOver => {
            // a timer a little early waits for the rest of the close
            let left = model.handed.map(|_| model.motion.closing_left(now.mono));
            match left {
                Some(left) if left > 0.0 => model.host.after(left, Message::CloseOver),
                _ => Vec::new(),
            }
        }
        Message::Unheard => {
            eprintln!("cupertino-launcher: the bus interface stopped");
            vec![Command::Exit]
        }
    };
    commands.extend(model.changed());
    // a close handed to the compositor: the layers' changes, and a timer for its end, when the window goes
    if !model.layer_changes.is_empty() {
        for (key, changes) in std::mem::take(&mut model.layer_changes) {
            commands.push(Command::Window(
                view::panel_window_key(),
                cupertino_kit::platform::WindowRequest::Layer { key, changes },
            ));
        }
        let left = model.motion.closing_left(model.now().mono);
        commands.extend(model.host.after(left, Message::CloseOver));
    }
    if model.handed.is_some() && model.motion.gone(model.now().mono) {
        model.handed = None;
    }
    // whatever started a transition (a launch or an opened file hides the panel as Escape does), its frames follow
    if !model.frame_waiting && model.motion.animating(model.now().mono) {
        commands.extend(model.animate());
    }
    if !model.presenting() {
        model.browser = None;
        model.scope = None;
        model.file_images.clear();
        model.file_thumbed.clear();
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
    // which scope the content shows: the one of the header, or the last while a fold fades it out
    let shown = match frame.header {
        Header::Browse => Some(View::Browse),
        Header::Files => Some(View::Files),
        Header::Search => model.last_scope,
    };
    let browser = model.browser.as_ref().filter(|_| {
        shown == Some(View::Browse) && (model.nav.view == View::Browse || frame.content > 0.0)
    });
    let files = model.scope.as_ref().filter(|_| {
        shown == Some(View::Files) && (model.nav.view == View::Files || frame.content > 0.0)
    });
    let modes = model.modes();
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
        modes: &modes,
        mode_hover: model.mode_hover,
        mode_focus: model.mode_focus.on,
        scope: files,
        file_images: &model.file_images,
        select_query: model.select_query,
        show_location: model.primary_held,
        layered: model.layered(),
    };
    let mut window = view::panel_window(&panel);
    // what moves by itself steps once per frame the compositor gives the panel's window, never on a timer of its own
    window.content = Element::from(FrameClock {
        content: window.content,
        on_frame: model.frame_waiting.then_some(Message::Frame(model.frame)),
    });
    let mut windows = vec![window];
    if model.nav.is_visible() && !model.dragging {
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

/// The header of the panel's view.
fn header_of(view: View) -> Header {
    match view {
        View::Browse => Header::Browse,
        View::Files => Header::Files,
        View::Search | View::Hidden => Header::Search,
    }
}

/// A `file://` URI of `path`: every byte but the unreserved ones and `/` percent-encoded.
pub fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}
