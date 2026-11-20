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

mod act;
mod flow;

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
/// How long past the end of its own motion the panel waits for the compositor to tell that an open it runs came to
/// rest ([`Message::OpenSettled`]) before it takes the open back, in seconds: well past the frame or two the compositor
/// starts it late.
pub const OPEN_REST_FALLBACK: f64 = 1.0;
/// The step, in seconds, the window's size looks ahead along the shape's motion by for the largest shape on its way.
const SHAPE_STEP: f64 = 1.0 / 240.0;
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
    /// Load and decode the renditions of `item`'s icon off the loop, one per output scale it is loaded at; the
    /// commands deliver them as [`Message::ResultIcon`] with its id. None to show its kind's symbol; the row is blank
    /// while they load.
    fn icon(&mut self, item: &ResultItem) -> Vec<Command<Message>>;
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
    /// [`Message::Previewed`], or [`Message::NoPreview`] once it turns out to show only the result itself. None when
    /// it is known at once to show only the result.
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
    /// The applications changed (entries installed, changed or removed while the launcher runs): all of them, as
    /// they are now.
    Installed(Vec<crate::DesktopEntry>),
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
    /// The server closed the hidden window (its output went away): a new one is opened, on an output still there,
    /// or once one comes back when none is left.
    Remap,
    /// Whether the server has an output the launcher's windows can go on.
    Outputs(bool),
    /// What the preview shows of the result with this id.
    Previewed(String, Arc<Preview>),
    /// The preview of the result with this id shows only the result itself.
    NoPreview(String),
    /// The renditions of the icon of the result with this id arrived ([`Host::icon`]).
    ResultIcon(String, Vec<ImageSource>),
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
    /// The session's option that keeps text carets steady instead of blinking.
    SteadyCaret(bool),
    /// The close the compositor runs ([`Model::compositor_layers`]) is over by now.
    CloseOver,
    /// The compositor did not tell that the open it runs came to rest ([`Message::OpenSettled`]) well past the open's
    /// end: the panel takes it back. For the open of this number (the count of opens handed over): a timer of an earlier open,
    /// still to come as the next one runs, ends nothing.
    OpenOver(u64),
    /// The panel's layers came to rest in the compositor: the open it runs is over, its last values shown.
    OpenSettled,
    /// The header of the fold the compositor runs switches now: the panel draws its query row again. For the fold of
    /// this number, as [`Message::OpenOver`].
    HeaderSwitch(u64),
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
    /// The host found the result shows only itself ([`Message::NoPreview`]).
    Plain,
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
    /// The server has no output: a window it closes is opened again only once one comes back, as the server closes
    /// every window put on none at once.
    pub no_output: bool,
    /// The hidden window was closed while the server had no output, and waits for one.
    remap_waiting: bool,
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
    /// An open from hidden hands itself to the compositor where the window system shows layers: a scope's
    /// ([`view::layers::open_changes`]), its glass the panel's own at every moment, its content the panel's pixels and
    /// its edge drawn by the compositor along the glass; and the search field's with its mode buttons
    /// ([`view::layers::field_open_changes`]), the glass of the field and the buttons the panel's own at every moment
    /// and the edge drawn by the compositor along it. On in the process; a test turns it off to compare the panel's own
    /// open.
    pub hand_opens: bool,
    /// The search field's open with its mode buttons is handed over too ([`view::layers::field_open_changes`]). On in
    /// the process; a test turns it off to compare the panel's own open.
    pub hand_field_opens: bool,
    /// The search field opens from hidden: its open is handed to the compositor once its mode buttons are placed.
    hand_field: bool,
    /// The close the compositor runs: the panel as it was when the close began, which the panel's layers keep showing
    /// until it is over, and the layers' changes not yet sent.
    handed: Option<Frame>,
    layer_changes: Vec<(u64, Vec<cupertino_kit::platform::LayerChange>)>,
    /// The open the compositor runs: the panel at rest, which its layers show from the open's first frame while the
    /// compositor moves them from where the open starts, until the compositor tells that they came to rest
    /// ([`Message::OpenSettled`]); and whether the timer that ends it all the same ([`OPEN_REST_FALLBACK`]) is asked
    /// for.
    open_rest: Option<Frame>,
    open_timer: bool,
    /// How many times the panel showed from hidden: the query field's caret blinks from the start of the open
    /// this numbers ([`view::PanelView::blink_start`]) until the open is over, whatever the keyboard's focus does.
    shows: u64,
    blink_start: Option<u64>,
    /// The blink of the query field's theme, as the field last told it; the theme's own before it told any.
    caret_blink: cupertino_kit::controls::CaretBlink,
    /// The query field's caret draws into a layer of its own whose opacity goes through this blink while the
    /// compositor runs the search field's open ([`view::layers::CARET_LAYER`]), until the caret shows again by itself.
    caret_layered: Option<cupertino_kit::controls::CaretBlink>,
    /// How many opens were handed to the compositor: the number of the one that runs.
    pub(crate) open_number: u64,
    /// When the panel's own motion of the open or fold the compositor runs ends, on the panel's clock, and how long
    /// an open's lasts.
    open_end: f64,
    open_length: f64,
    /// A scope's fold into the search field hands itself to the compositor where the panel rested as layers
    /// ([`view::layers::fold_changes`]): its glass the panel's own at every moment, the body fading on its own curve,
    /// the query row drawn again once, at the header's switch, and the edge drawn by the compositor along the glass,
    /// as the panel strokes its own. On in the process; a test turns it off to compare the panel's own fold.
    pub hand_folds: bool,
    /// The search field's unfold into a scope hands itself to the compositor where the field rested as layers, as a
    /// fold does ([`Model::hand_folds`]), the scope's body drawn at once at the scope's shape at rest and cut to the
    /// glass as it grows. It needs the window to keep its size through the unfold ([`Model::window_held`]). On in the
    /// process; a test turns it off to compare the panel's own unfold.
    pub hand_unfolds: bool,
    /// The panel's window keeps the largest size the panel takes ([`Model::largest_extent`]) for as long as the panel
    /// shows, so a fold or unfold the compositor runs never waits for the window to be configured again part way: on
    /// where the window system shows the panel's parts as layers of their own, never under Reduce Motion.
    pub window_held: bool,
    /// [`Model::largest_extent`] as last worked out, and the output it was worked out on.
    largest: Option<(Option<Size>, Size)>,
    /// A fold or unfold to hand to the compositor once the mode buttons are placed.
    hand_fold: bool,
    /// The fold or unfold the compositor runs: the panel as its layers show it before the header switches, when it
    /// switches, and after, which differs in the header alone.
    fold_shown: Option<(Frame, f64, Frame)>,
    /// The unfold the compositor runs: the scope's shape at rest, which its body is drawn at from the unfold's first
    /// frame, the glass cutting it as it grows.
    unfold_body: Option<Shape>,
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
            no_output: false,
            remap_waiting: false,
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
            hand_opens: true,
            hand_field_opens: true,
            hand_field: false,
            handed: None,
            layer_changes: Vec::new(),
            open_rest: None,
            shows: 0,
            blink_start: None,
            caret_blink: Default::default(),
            caret_layered: None,
            open_timer: false,
            open_number: 0,
            open_end: 0.0,
            open_length: 0.0,
            hand_folds: true,
            hand_unfolds: true,
            window_held: false,
            largest: None,
            hand_fold: false,
            fold_shown: None,
            unfold_body: None,
        }
    }

    fn now(&self) -> Now {
        self.host.now()
    }

    /// What is presented now: while the compositor runs the close, the panel as the close began, which its layers
    /// keep; while it runs a fold, the panel as the fold began, its header switched part way; while it runs an open,
    /// the panel at rest.
    pub fn presented(&self) -> Frame {
        let now = self.now().mono;
        self.handed
            .or(self
                .fold_shown
                .map(|(before, at, after)| if now >= at { after } else { before }))
            .or(self.open_rest)
            .unwrap_or_else(|| self.motion.frame(now))
    }

    /// Whether the compositor runs a fold or unfold now ([`Model::hand_folds`]).
    pub fn fold_handed(&self) -> bool {
        self.fold_shown.is_some()
    }

    /// Follow the session's steady caret option, which the query field's theme follows too: the caret's blink the
    /// compositor is handed through an open is steady from then on, before the field tells its own, so the first open
    /// keeps the caret in the query row's layer; a blinking one again is the theme's until the field tells. A caret
    /// that blinks in its own layer goes back into the row's at once.
    fn follow_steady_caret(&mut self, on: bool) {
        if on {
            self.caret_blink.phase_ms = 0;
            self.caret_layered = None;
        } else if self.caret_blink.phase_ms == 0 {
            self.caret_blink.phase_ms = cupertino_kit::theme::CARET_PHASE_MS;
        }
    }

    /// The open or fold the compositor runs is over, or taken back: the panel draws what it presents itself again.
    fn end_open(&mut self) {
        self.open_rest = None;
        self.caret_layered = None;
        self.fold_shown = None;
        self.unfold_body = None;
    }

    /// The shape of the scope the compositor unfolds the search field into, which the body is drawn at while it does.
    pub fn unfold_body(&self) -> Option<Shape> {
        self.unfold_body
    }

    /// Whether an unfold starting now leaves the search field drawn as layers at rest, which then unfolds on the
    /// compositor's clock ([`Model::hand_unfolds`]).
    fn unfolds_from_field(&self) -> bool {
        self.hand_unfolds
            && self.layered()
            && self.open_rest.is_none()
            && self.presented().header == Header::Search
    }

    /// Whether the panel's parts draw into compositor layers of their own now: while it rests, open, and while the
    /// compositor runs its close; never under Reduce Motion, where nothing is handed over.
    pub fn layered(&self) -> bool {
        self.compositor_layers
            && !self.motion.reduce_motion()
            && (self.handed.is_some()
                || self.open_rest.is_some()
                || (self.nav.is_visible() && !self.motion.animating(self.now().mono)))
    }

    /// Whether the compositor runs the panel's open now.
    pub fn open_handed(&self) -> bool {
        self.open_rest.is_some()
    }

    /// Whether the compositor runs the search field's open with its mode buttons now, drawing the edge along their glass.
    pub fn field_open_handed(&self) -> bool {
        self.fold_shown.is_none()
            && self
                .open_rest
                .is_some_and(|rest| rest.modes.count > 0 && rest.header == Header::Search)
    }

    /// The panel as it rests once what moves now comes to rest.
    fn resting(&self, now: f64) -> Frame {
        let mut resting = self.motion.clone();
        resting.settle(now);
        resting.frame(now)
    }

    /// The shape the enclosure moves to.
    pub fn motion_target(&self) -> Shape {
        self.motion.target()
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

    /// The applications are now `entries`. What is on screen follows in place: the results of the query keep the
    /// query, the selected result and what the file search found; the browser keeps its filter, its selection and
    /// its scroll. The icon and the preview of an application that changed or went are asked for again.
    fn set_applications(&mut self, entries: Vec<crate::DesktopEntry>) {
        let changed: Vec<String> = self
            .launcher
            .entries()
            .iter()
            .filter(|old| !entries.contains(old))
            .map(|old| old.id.clone())
            .collect();
        self.launcher.set_entries(entries);
        for id in &changed {
            self.app_icons.remove(id);
            // the application's result and those of its actions
            let (app, actions) = (format!("app:{id}"), format!("app:{id}#"));
            let of_app = |key: &str| key == app || key.starts_with(&actions);
            self.icons.retain(|key, _| !of_app(key));
            if self.previewed.as_deref().is_some_and(of_app) {
                self.previewed = None;
            }
        }
        if let Some(browser) = self.browser.as_mut() {
            browser.set_apps(self.launcher.entries().iter().map(App::of).collect());
        }
        // only a query that was searched has results; a kept one shows none until it is edited
        if self.nav.view == View::Search && self.searched.as_deref() == Some(self.panel.query()) {
            let results =
                self.launcher
                    .compose(self.panel.query(), self.now().unix, self.tracker.sections());
            self.panel.set_results(results);
        }
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
        Message::Installed(entries) => {
            model.set_applications(entries);
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
            if model.no_output {
                model.remap_waiting = true;
            } else {
                model.hidden += 1;
            }
            Vec::new()
        }
        Message::Outputs(any) => {
            model.no_output = !any;
            if any && std::mem::take(&mut model.remap_waiting) {
                model.hidden += 1;
            }
            Vec::new()
        }
        Message::Previewed(id, preview) => {
            // only for the result last asked about; one that comes after its deadline still shows
            if model.previewed.as_deref() == Some(id.as_str()) {
                model.preview = PreviewState::Ready(Arc::new(bounded((*preview).clone())));
            }
            Vec::new()
        }
        Message::NoPreview(id) => {
            if model.preview == PreviewState::Waiting(id) {
                model.preview = PreviewState::Plain;
            }
            Vec::new()
        }
        Message::PreviewOverdue(id) => {
            // an answer, or word that there is none to show, ends the wait
            let answered = matches!(model.preview, PreviewState::Ready(_) | PreviewState::Plain);
            if model.previewed.as_deref() == Some(id.as_str()) && !answered {
                model.preview = PreviewState::Unavailable;
            }
            Vec::new()
        }
        Message::ResultIcon(id, sources) => {
            // still shown blank (the icons are not cleared since)
            if let Some(icon) = model.icons.get_mut(&id).filter(|icon| icon.is_empty()) {
                *icon = sources;
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
        Message::SteadyCaret(on) => {
            model.follow_steady_caret(on);
            Vec::new()
        }
        Message::OpenOver(number) if number == model.open_number => {
            model.open_timer = false;
            if now.mono >= model.open_end {
                model.end_open();
            }
            model.animate()
        }
        Message::OpenOver(_) => Vec::new(),
        // what the fold's layers show is the panel with the header it switched to by now ([`Model::presented`])
        Message::HeaderSwitch(_) => Vec::new(),
        Message::OpenSettled => {
            // the compositor shows the open's last values: whatever the panel draws from now on comes after them. Where
            // it came to rest before the panel's own motion (it skipped to the end), the panel's motion ends first.
            let mut commands = Vec::new();
            if model.open_rest.is_some() {
                if now.mono < model.open_end {
                    let left = model.open_end - now.mono;
                    commands.extend(model.host.after(left, Message::OpenOver(model.open_number)));
                } else {
                    model.end_open();
                }
            }
            commands.extend(model.animate());
            commands
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
        if model.open_rest.is_none() {
            let left = model.motion.closing_left(model.now().mono);
            commands.extend(model.host.after(left, Message::CloseOver));
        }
    }
    // an open the compositor runs ends when the compositor tells its layers came to rest, which it does in the frame
    // that shows their last values: the compositor starts the open when it first shows its commit, a frame or more
    // after the panel's own motion started, so the panel's clock would end it early. A timer well past the open's end
    // ends it all the same. What changes where the panel comes to rest (a query that brings results, a scope's height)
    // takes the open back: the panel draws the rest of it itself.
    if let Some(rest) = model.open_rest {
        let now = model.now().mono;
        if !model.nav.is_visible() || model.resting(now) != rest {
            model.end_open();
        } else if !model.open_timer {
            model.open_timer = true;
            let left = (model.open_end - now).max(0.0) + OPEN_REST_FALLBACK;
            commands.extend(model.host.after(left, Message::OpenOver(model.open_number)));
        }
    }
    if model.handed.is_some() && model.motion.gone(model.now().mono) {
        model.handed = None;
    }
    // once the open is over the caret blinks from each focus gain again
    if model.blink_start.is_some()
        && model.open_rest.is_none()
        && !model.motion.animating(model.now().mono)
    {
        model.blink_start = None;
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
        handed: model.close_handed(),
        opening: model.open_handed(),
        moving: !model.layered() && model.motion.animating(model.now().mono),
        unfold_body: model.unfold_body(),
        blink_start: model.blink_start,
        caret_layer: model.caret_layered.is_some() && model.layered(),
    };
    let mut window = view::panel_window(&panel);
    // what moves by itself steps once per frame the compositor gives the panel's window, never on a timer of its own
    window.content = Element::from(FrameClock {
        content: window.content,
        on_frame: model.frame_waiting.then_some(Message::Frame(model.frame)),
        // the end of an open the compositor runs, told by the compositor
        on_layers_settled: model.open_rest.is_some().then_some(Message::OpenSettled),
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
