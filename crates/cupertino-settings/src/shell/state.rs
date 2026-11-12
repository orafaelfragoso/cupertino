//! What the Settings window shows and does, apart from how it shows it: the history, the settings and their files,
//! the live services, the sheets, and the work on other threads that answers through [`Background`].

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use crate::agent::{Agent, AgentEvent};
use crate::controls::{Control, LevelWriter, Value};
use crate::history::History;
use crate::inflight::Reads;
use crate::live::{display_name, Live, LoginScreen, Pictures, Reading};
use crate::pages::Page;
use crate::panes::{self, Call, ControlId, Effect, Section, Session};
use crate::services::{self, Services, SESSION_NAME, SESSION_PATH};
use crate::settings::{system_accessibility, File, Paths, Settings};
use crate::sheet::{Answer, Outcome, Sheet};
use crate::sidebar;
use crate::PaneId;
use cupertino_assets::Assets;
use cupertino_defaults::accessibility::{DisplayOptions, SYSTEM_ACCESSIBILITY};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_filepicker::Mode;

/// A level slider still this long, in seconds, has ended its drag: the live readings show again.
const LEVEL_SETTLE: f64 = 0.5;
/// While a slider without stops is dragged, its value is applied at most this often, in seconds, so what it sets
/// follows the knob; the value it is let go at is applied at once.
const SLIDE_APPLY_EVERY: f64 = 0.15;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Something for the window loop from another thread.
pub(crate) enum Background {
    ShowPane(PaneId, String),
    Agent(AgentEvent),
    /// A pairing ended.
    Paired(std::result::Result<(), String>),
    /// A settings file changed on disk.
    Files(WatchEvent),
    /// A listing of the Wallpaper pane's pictures: group 0 the packs', then one per added folder.
    Pictures {
        key: ListingKey,
        files: Vec<cupertino_assets::WallpaperFile>,
    },
    /// A folder the chooser asked for was read.
    Listing(cupertino_filepicker::Listing),
    /// The thumbnail of a picture of the Wallpaper pane is made.
    Thumbnail {
        image: std::path::PathBuf,
        thumbnail: std::path::PathBuf,
        /// The thumbnail's pixels, decoded on the thread that made it.
        pixels: Option<cupertino_kit::controls::ImageSource>,
    },
    /// Whether the Login Screen section shows, asked at the Accessibility pane's opening `opening`.
    LoginScreen {
        opening: u64,
        allowed: bool,
    },
    /// sessiond answered Use Mine.
    LoginScreenSaved(std::result::Result<(), String>),
    /// The graphics memory only the driver's tool reports was read.
    VideoMemory(Vec<(crate::about::PciAddress, u64)>),
    /// A service call made on a thread of its own ended; `id` is the control that asked for it.
    Called {
        id: Option<ControlId>,
        call: Call,
        result: std::result::Result<(), String>,
    },
}

/// Reads the graphics memory only the driver's tool reports, by PCI address.
pub(crate) type ReadVideoMemory =
    std::sync::Arc<dyn Fn() -> Vec<(crate::about::PciAddress, u64)> + Send + Sync>;

/// The driver's tool, given a short time to answer.
#[cfg(not(test))]
fn read_video_memory() -> Vec<(crate::about::PciAddress, u64)> {
    crate::about::nvidia_video_memory(crate::about::VIDEO_MEMORY_TIMEOUT)
}

/// Tests run no program of the machine's; one that reads the memory sets a reader of its own.
#[cfg(test)]
fn read_video_memory() -> Vec<(crate::about::PciAddress, u64)> {
    Vec::new()
}

/// The graphics memory only the driver's tool reports.
pub(crate) enum VideoMemory {
    /// Its thread is running.
    Reading,
    Read(Vec<(crate::about::PciAddress, u64)>),
}

/// The longest edge of a thumbnail the cache keeps, in pixels: the decoder keeps a picture of this size whole.
const THUMBNAIL_EDGE: u32 = 256;

/// The longest edge of the account picture kept for the sidebar's row, in pixels.
const ACCOUNT_PICTURE_EDGE: u32 = 128;
/// The largest account picture file read, in bytes.
const ACCOUNT_PICTURE_BYTES: u64 = 8 * 1024 * 1024;

/// The pixels of the thumbnail file `path`, as an image of its own for the window to draw; run on the thumbnail
/// thread, never the window's.
fn decode_thumbnail(
    path: &std::path::Path,
    edge: u32,
) -> Option<cupertino_kit::controls::ImageSource> {
    use cupertino_kit::render::{ImageResource, PixelFormat};
    let data = std::fs::read(path).ok()?;
    let icon = cupertino_icons::decode_png(&data, edge)?;
    Some(cupertino_kit::controls::ImageSource {
        id: cupertino_kit::core::next_resource_id(),
        image: ImageResource {
            revision: 1,
            width: icon.width,
            height: icon.height,
            stride: icon.width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: std::sync::Arc::from(icon.rgba),
        },
    })
}

/// What a listing thread reads: the packs' pictures, the pictures of an added folder, or a folder of the chooser.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ListingKey {
    Packs,
    Pictures(std::path::PathBuf),
    Chooser(std::path::PathBuf, bool, cupertino_filepicker::Mode),
}

/// Hands a [`Background`] to the window loop and wakes it.
pub(crate) type Notify = std::sync::Arc<dyn Fn(Background) + Send + Sync>;

/// Background events waiting for the window loop.
const BACKGROUND_QUEUE: usize = 64;
/// Where Choose Program… opens (approximate).
const PROGRAM_CHOOSER_START: &str = "/usr/bin";

/// The window loop owns the receiver until it ends; what arrives after that has no window. What arrives before the
/// window exists waits for the loop's first turn. A full queue makes a worker thread wait. What comes from a bus is
/// dropped instead, so a caller can neither grow the queue nor block a bus executor: a dropped pane request shows nothing,
/// a dropped agent prompt refuses the pairing.
pub(crate) fn background_channel(
    wake: impl Fn() + Send + Sync + 'static,
) -> (Notify, std::sync::mpsc::Receiver<Background>) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(BACKGROUND_QUEUE);
    let notify: Notify = std::sync::Arc::new(move |event| {
        let queued = match event {
            Background::ShowPane(..) | Background::Agent(_) => sender.try_send(event).is_ok(),
            event => sender.send(event).is_ok(),
        };
        if queued {
            wake();
        }
    });
    (notify, receiver)
}

/// The Wi-Fi pane scans this often while it is shown and the radio is on (approximate, §6.4).
const SCAN: f64 = 10.0;
/// How often the battery's recorded charge is read again while the Battery pane shows, in seconds.
const HISTORY_REFRESH: f64 = 60.0;

/// The level slider being dragged: its control, its writer, when it last moved and where its knob is.
pub(crate) struct LevelDrag {
    id: ControlId,
    writer: LevelWriter,
    moved: f64,
    value: f64,
}

/// A slider without stops being dragged: it commits every [`SLIDE_APPLY_EVERY`] while it moves, and its last value
/// when it is let go or has been still for [`LEVEL_SETTLE`].
pub(crate) struct SliderDrag {
    id: ControlId,
    value: f64,
    moved: f64,
    /// The value last committed and when.
    applied: Option<(f64, f64)>,
}

pub struct State {
    pub(crate) paths: Paths,
    pub(crate) settings: Settings,
    pub(crate) services: Services,
    pub(crate) live: Live,
    pub(crate) hidden: Vec<PaneId>,
    pub(crate) groups: Vec<Vec<PaneId>>,
    pub(crate) history: History<Page>,
    /// The pane whose services are started.
    pub(crate) content_pane: PaneId,
    /// The refusals shown under their rows.
    pub(crate) errors: Vec<(ControlId, String)>,
    /// Problems already logged.
    pub(crate) reported: std::collections::HashSet<String>,
    pub(crate) dark: bool,
    pub(crate) display: DisplayOptions,
    pub(crate) bus: Option<zbus::blocking::Connection>,
    pub(crate) sheet: Option<Sheet>,
    pub(crate) level: Option<LevelDrag>,
    pub(crate) slide: Option<SliderDrag>,
    pub(crate) start: std::time::Instant,
    pub(crate) refreshed: f64,
    pub(crate) scanned: f64,
    /// When the battery's charge history was last read.
    pub(crate) history_read: f64,
    /// The folder chooser that is open adds a folder the file search leaves out.
    pub(crate) picking_exclusion: bool,
    pub(crate) notify: Notify,
    pub(crate) assets: Assets,
    /// Each picture's thumbnail once made.
    pub(crate) thumbnails: HashMap<PathBuf, PathBuf>,
    /// Each thumbnail's decoded pixels, ready to draw.
    pub(crate) thumbnail_images: HashMap<PathBuf, cupertino_kit::controls::ImageSource>,
    /// The signed-in person, for the sidebar's account row, and their picture when they have one.
    pub(crate) account: crate::system::Account,
    pub(crate) account_picture: Option<cupertino_kit::controls::ImageSource>,
    pub(crate) thumbnails_asked: std::collections::HashSet<PathBuf>,
    pub(crate) listings: Reads<ListingKey, Option<cupertino_filepicker::Request>>,
    pub(crate) agent_answer: Option<(u64, async_channel::Sender<Answer>)>,
    pub(crate) agent_registered: bool,
    pub(crate) login_opening: u64,
    pub(crate) login_saving: bool,
    /// Where About's readings come from: `/`, or a test's tree.
    pub(crate) about_root: PathBuf,
    /// The graphics memory only the driver's tool reports, read once on a thread when a pane first shows the
    /// graphics; `None` until it is asked for.
    pub(crate) video_memory: Option<VideoMemory>,
    /// Reads that memory; on a thread, as it runs a program.
    pub(crate) read_video_memory: ReadVideoMemory,
    /// An activation token another start of Settings handed over, for the window to raise itself with.
    pub(crate) activation: Option<String>,
    _files: Option<ConfigWatcher>,
    /// Watches `desktop-control.conf`, which the compositor writes when an app first asks, on its own: a file that
    /// can't be read stops no other file's watch.
    _answers: Option<ConfigWatcher>,
}

impl State {
    /// The state of a window opening on `pane` (the first row when it is hidden); `display_height` is the height of the
    /// display the Dock sits on, which its default tile size depends on. Work on other threads answers through
    /// `notify`.
    pub(crate) fn new(
        paths: Paths,
        mut services: Services,
        pane: PaneId,
        display_height: f64,
        notify: Notify,
    ) -> Result<State> {
        let backends = services.detect();
        let hidden = backends.hidden();
        let pane = if hidden.contains(&pane) {
            sidebar::rows(&hidden)[0]
        } else {
            pane
        };
        let settings = Settings::load(&paths, display_height)?;
        // another writer's change shows at once (the dock's menu, the server, an editor); a commit re-reads the files
        // anyway
        let files: Vec<PathBuf> = [
            &paths.dock,
            &paths.compositor,
            &paths.menubar,
            &paths.keyboard,
            &paths.power,
            &paths.accessibility,
            &paths.appearance,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        let to_loop = notify.clone();
        let files = match ConfigWatcher::new(files, move |event| to_loop(Background::Files(event)))
        {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("cupertino-settings: cannot watch the settings files: {e}; the panes show changes of other writers at the next commit");
                None
            }
        };
        let answers = paths.desktop_control.clone().and_then(|path| {
            let to_loop = notify.clone();
            ConfigWatcher::new(vec![path], move |event| to_loop(Background::Files(event)))
                .inspect_err(|e| {
                    eprintln!("cupertino-settings: cannot watch desktop-control.conf: {e}; the Privacy pane shows its changes when it opens")
                })
                .ok()
        });
        let account = crate::system::Account::read();
        let display = settings.accessibility.display;
        let screen_reader_installed = paths.screen_reader.is_some();
        let mut state = State {
            paths,
            settings,
            services,
            live: Live {
                hardware: Some(backends.hardware),
                screen_reader_installed,
                ..Live::default()
            },
            groups: sidebar::groups(&hidden),
            hidden,
            history: History::new(Page::pane(pane)),
            content_pane: pane,
            errors: Vec::new(),
            reported: std::collections::HashSet::new(),
            dark: false,
            display,
            bus: None,
            sheet: None,
            level: None,
            slide: None,
            start: std::time::Instant::now(),
            refreshed: f64::NEG_INFINITY,
            scanned: f64::NEG_INFINITY,
            history_read: f64::NEG_INFINITY,
            picking_exclusion: false,
            notify,
            assets: Assets::discover(),
            thumbnails: HashMap::new(),
            thumbnail_images: HashMap::new(),
            account_picture: account
                .picture
                .as_deref()
                .filter(|path| {
                    std::fs::metadata(path).is_ok_and(|m| m.len() <= ACCOUNT_PICTURE_BYTES)
                })
                .and_then(|path| decode_thumbnail(path, ACCOUNT_PICTURE_EDGE)),
            account,
            thumbnails_asked: std::collections::HashSet::new(),
            listings: Reads::default(),
            agent_answer: None,
            agent_registered: false,
            login_opening: 0,
            about_root: PathBuf::from("/"),
            video_memory: None,
            read_video_memory: std::sync::Arc::new(read_video_memory),
            login_saving: false,
            activation: None,
            _files: files,
            _answers: answers,
        };
        state.sync_accessibility();
        state.enter_pane(None, pane)?;
        Ok(state)
    }

    pub(crate) fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    pub(crate) fn rows(&self) -> Vec<PaneId> {
        self.groups.iter().flatten().copied().collect()
    }

    /// Show `pane`, pushed on the history; a hidden pane shows the first row instead. The services follow.
    pub(crate) fn go(&mut self, pane: PaneId) -> Result<()> {
        self.show(Page::pane(pane))
    }

    /// Show `page`, pushed on the history; a page of a hidden pane shows the first row instead. The services follow.
    pub(crate) fn show(&mut self, page: Page) -> Result<()> {
        let page = if self.hidden.contains(&page.pane) {
            Page::pane(self.rows()[0])
        } else {
            page
        };
        self.history.push(page);
        self.follow()
    }

    /// The pane the history shows now.
    pub(crate) fn pane(&self) -> PaneId {
        self.history.current().pane
    }

    /// Start the services of the pane the history shows now and stop those of the one before.
    pub(crate) fn follow(&mut self) -> Result<()> {
        let to = self.pane();
        if self.content_pane != to {
            let from = std::mem::replace(&mut self.content_pane, to);
            self.level = None;
            self.enter_pane(Some(from), to)?;
        }
        Ok(())
    }

    /// Whether the input level meter listens and shows now.
    pub(crate) fn meter_running(&self) -> bool {
        self.content_pane == PaneId::Sound && self.live.tab == 1 && !self.live.inputs.is_empty()
    }

    /// Read how loud the input is now; true when it changed.
    pub(crate) fn refresh_meter(&mut self) -> bool {
        let level = self.services.meter_level();
        let changed = (level - self.live.input_level).abs() > 0.005;
        self.live.input_level = level;
        changed
    }

    /// Read the live services of the pane shown again; draw again when something changed. The battery is read on the
    /// first refresh too, whatever the pane: whether there is one names the Battery row.
    pub(crate) fn refresh(&mut self, now: f64) -> Result<()> {
        self.refreshed = now;
        let backends = self.services.detect();
        self.live.hardware = Some(backends.hardware);
        let hidden = backends.hidden();
        if hidden != self.hidden {
            self.groups = sidebar::groups(&hidden);
            self.hidden = hidden;
        }
        if self.hidden.contains(&self.pane()) {
            return self.go(self.rows()[0]);
        }
        if let Some(dark) = self.services.dark() {
            self.dark = dark;
        }
        self.display = self.settings.accessibility.display;
        if self.content_pane == PaneId::Battery
            || self.live.battery == crate::live::Reading::Unknown
        {
            self.live.battery = self.services.battery();
        }
        if self.content_pane == PaneId::Battery {
            let with_history = now - self.history_read >= HISTORY_REFRESH;
            if with_history {
                self.history_read = now;
            }
            self.live.battery_extras = self
                .services
                .battery_extras(&self.live.battery_extras, with_history);
        }
        match self.content_pane {
            PaneId::Focus => self.live.focus = self.services.focus(),
            PaneId::Notifications => self.live.notifying_apps = self.services.notifying_apps(),
            PaneId::Sound => {
                if self.level.is_none() {
                    self.live.volume = self.services.volume();
                    self.live.outputs = self.services.outputs();
                    self.live.balance = self.services.balance();
                    if self.live.tab == 1 {
                        self.live.inputs = self.services.inputs();
                        self.live.input_volume = self.services.input_volume();
                    }
                }
                if self.live.tab == 1 {
                    self.services.start_meter();
                    self.live.input_level = self.services.meter_level();
                } else {
                    self.services.stop_meter();
                    self.live.input_level = 0.0;
                }
            }
            PaneId::Keyboard => self.live.keyboard_backlight = self.services.keyboard_backlight(),
            PaneId::Vpn => self.live.vpns = self.services.vpns(),
            PaneId::Displays => {
                self.live.displays = self.services.displays();
                if self.level.is_none() {
                    self.live.brightness = self.services.brightness();
                }
            }
            PaneId::Wifi | PaneId::Network => {
                let keep = self.live.wifi.ready().cloned();
                self.live.wifi = self.services.wifi(keep.as_ref());
                self.live.interfaces = crate::netinfo::read();
                let radio_on = self
                    .live
                    .wifi
                    .ready()
                    .is_some_and(|w| w.enabled && w.has_wifi_device());
                if self.content_pane == PaneId::Wifi && radio_on && now - self.scanned >= SCAN {
                    self.scanned = now;
                    if let Err(e) = self.services.scan() {
                        eprintln!("cupertino-settings: Wi-Fi scan: {e}");
                    }
                }
            }
            PaneId::Mouse => self.live.mice = self.services.mice(),
            PaneId::Bluetooth => {
                let keep = self.live.bluetooth.ready().cloned();
                self.live.bluetooth = self.services.bluetooth(keep.as_ref());
            }
            _ => {}
        }
        Ok(())
    }

    /// The content now shows `to` (`from` before): start what the new pane needs and stop what the old one started.
    pub(crate) fn enter_pane(&mut self, from: Option<PaneId>, to: PaneId) -> Result<()> {
        if let Some(from) = from.filter(|f| *f != to) {
            self.leave_pane(from);
        }
        if from == Some(to) {
            return Ok(());
        }
        self.scanned = f64::NEG_INFINITY;
        self.history_read = f64::NEG_INFINITY;
        self.live.tab = 0;
        if to == PaneId::Users {
            self.live.users = crate::system::users();
        }
        if to == PaneId::Wallpaper {
            self.load_pictures();
        }
        if to == PaneId::Accessibility {
            self.ask_login_screen();
        }
        if matches!(to, PaneId::Account | PaneId::General) {
            let known = match &self.video_memory {
                Some(VideoMemory::Read(list)) => list.as_slice(),
                _ => &[],
            };
            self.live.system = Some(Box::new(crate::system::SystemInfo::read_with(
                &self.about_root,
                known,
            )));
            self.ask_video_memory();
        }
        if to == PaneId::General {
            self.live.login_items = crate::system::login_items();
        }
        if to == PaneId::DesktopDock {
            self.live.browsers = crate::system::browsers();
        }
        if to == PaneId::MenuBar {
            self.live.tray = self.services.tray_items();
        }
        if to == PaneId::Privacy {
            self.settings.reload_clipboard(&self.paths);
            self.settings.reload_screen_sharing(&self.paths);
            self.settings.reload_desktop_control(&self.paths);
        }
        if matches!(to, PaneId::Privacy | PaneId::Notifications) {
            let (locale, visibility) = (
                cupertino_apps::entry::Locale::from_env(),
                cupertino_apps::entry::Visibility::from_env(),
            );
            let entries = cupertino_apps::entry::index_dirs(
                &cupertino_apps::entry::xdg_application_dirs(),
                &locale,
                &visibility,
            );
            let path_dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default();
            self.live.apps = crate::clipboard::installed(entries, &path_dirs);
        }
        self.refresh(self.now())?;
        if to == PaneId::Bluetooth && self.live.bluetooth.ready().is_some_and(|b| b.available) {
            let notify = self.notify.clone();
            match self.services.register_agent(Agent {
                on_event: Box::new(move |event| notify(Background::Agent(event))),
            }) {
                Ok(ours) => {
                    self.agent_registered = ours;
                    if let crate::live::Reading::Ready(b) = &mut self.live.bluetooth {
                        b.agent_elsewhere = !ours;
                    }
                }
                Err(e) => eprintln!("cupertino-settings: Bluetooth pairing agent: {e}"),
            }
            // discovery needs a powered adapter; the pane shows why otherwise
            if self.live.bluetooth.ready().is_some_and(|b| b.powered) {
                if let Err(e) = self.services.discovery(true) {
                    eprintln!("cupertino-settings: Bluetooth discovery: {e}");
                }
            }
        }
        Ok(())
    }

    pub(crate) fn leave_pane(&mut self, pane: PaneId) {
        if pane == PaneId::Sound {
            self.services.stop_meter();
            self.live.input_level = 0.0;
        }
        if pane == PaneId::Bluetooth && self.live.bluetooth.ready().is_some_and(|b| b.discovering) {
            if let Err(e) = self.services.discovery(false) {
                eprintln!("cupertino-settings: Bluetooth discovery: {e}");
            }
        }
        if pane == PaneId::Bluetooth && self.agent_registered {
            self.agent_registered = false;
            if let Err(e) = self.services.unregister_agent() {
                eprintln!("cupertino-settings: Bluetooth pairing agent: {e}");
            }
        }
    }

    /// The icon of each installed application by the name its section shows: the notification apps' sections wear it
    /// before their titles.
    pub(crate) fn app_icons(&self) -> HashMap<String, String> {
        if self.pane() != PaneId::Notifications {
            return HashMap::new();
        }
        self.live
            .apps
            .iter()
            .map(|app| {
                (
                    app.entry.name.clone(),
                    app.entry.icon.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The current pane's sections with the values in effect and the errors of refused changes.
    pub(crate) fn sections(&self) -> Vec<Section> {
        let mut sections = panes::sections(
            self.pane(),
            &self.settings,
            Session {
                dark: self.dark,
                display: self.display,
            },
            &self.live,
        );
        for row in sections.iter_mut().flat_map(|s| &mut s.rows) {
            if let Some((_, e)) = self.errors.iter().find(|(id, _)| *id == row.id) {
                row.error = Some(e.clone());
            }
            let dragged = match &row.control {
                Control::Level { .. } => self.level.as_ref().map(|d| (d.id, d.value)),
                Control::Slider { .. } => self.slide.as_ref().map(|d| (d.id, d.value)),
                _ => None,
            };
            if let (Some((_, v)), Control::Level { value } | Control::Slider { value, .. }) =
                (dragged.filter(|(id, _)| *id == row.id), &mut row.control)
            {
                *value = v;
            }
        }
        sections
    }

    /// Commit `value` of control `id`: change the settings and do what that asks; a refusal restores the control and
    /// shows the error under its row.
    pub(crate) fn commit(&mut self, id: ControlId, value: Value) -> Result<()> {
        self.errors.retain(|(e, _)| *e != id);
        // read-modify-write: the files as they are now, so the store changes the user's key only and puts back no
        // value another writer changed since the last read
        if let Err(e) = self.settings.reload(&self.paths) {
            eprintln!("cupertino-settings: the settings could not be read: {e}");
            self.errors
                .push((id, "The settings could not be read.".to_string()));
            return Ok(());
        }
        self.sync_accessibility();
        let before = self.settings.clone();
        let effects = panes::change(id, value, &mut self.settings, &self.live);
        // restoring `before` on a failure undoes the whole commit only while a commit has one effect
        assert!(effects.len() <= 1, "{id:?} made {} effects", effects.len());
        for effect in effects {
            let outcome = match effect {
                Effect::Service(Call::Join {
                    ssid,
                    passphrase: None,
                }) => {
                    let name = self
                        .live
                        .wifi
                        .ready()
                        .and_then(|w| w.networks.iter().find(|n| n.ssid == ssid))
                        .map_or_else(|| display_name(&ssid), |n| n.name.clone());
                    self.sheet = Some(Sheet::passphrase(&ssid, &name));
                    Ok(())
                }
                Effect::Service(Call::Remove {
                    id,
                    confirmed: false,
                }) => {
                    let name = self
                        .live
                        .bluetooth
                        .ready()
                        .and_then(|b| b.devices.iter().find(|d| d.id == id))
                        .map_or(id.clone(), |d| d.title().to_string());
                    self.sheet = Some(Sheet::forget(&id, &name));
                    Ok(())
                }
                Effect::Service(call) => self.service_call(Some(id), &call),
                Effect::AskPicture { folder } => {
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                    let start = home
                        .as_ref()
                        .map(|h| h.join("Pictures"))
                        .filter(|p| p.is_dir())
                        .or(home)
                        .unwrap_or_else(|| std::path::PathBuf::from("/"));
                    let mode = if folder { Mode::Folder } else { Mode::Photo };
                    self.sheet = Some(Sheet::picker_pending(&start, mode));
                    self.drive_picker();
                    Ok(())
                }
                Effect::AskResolution { key, name } => {
                    self.sheet = Some(Sheet::resolution(&key, &name));
                    Ok(())
                }
                Effect::ConfirmUnallow { program, name } => {
                    self.sheet = Some(Sheet::unallow(&program, &name));
                    Ok(())
                }
                Effect::AskClipboardApp { program: true } => {
                    self.sheet = Some(Sheet::picker_pending(
                        std::path::Path::new(PROGRAM_CHOOSER_START),
                        Mode::Program,
                    ));
                    self.drive_picker();
                    Ok(())
                }
                Effect::AskClipboardApp { program: false } => {
                    let apps = crate::clipboard::apps(
                        &self.live.apps,
                        &self.settings.clipboard,
                        sidebar::standard_compare,
                    );
                    self.sheet = Some(Sheet::apps(apps));
                    Ok(())
                }
                Effect::ChartSpan(ten_days) => {
                    self.live.chart_days = ten_days;
                    Ok(())
                }
                Effect::Tab(tab) => {
                    self.live.tab = tab;
                    let now = self.now();
                    self.refresh(now).map_err(|e| e.to_string())
                }
                Effect::AskExclusion => {
                    let start = std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("/"));
                    self.picking_exclusion = true;
                    self.sheet = Some(Sheet::picker_pending(&start, Mode::Folder));
                    self.drive_picker();
                    Ok(())
                }
                Effect::Refuse(reason) => Err(reason),
                Effect::ForgetGrant(grant) => self.settings.forget_grant(&self.paths, &grant),
                Effect::DecideApp {
                    service,
                    app,
                    decision,
                } => self
                    .settings
                    .decide_app(&self.paths, service, &app, decision),
                Effect::ForgetApp { service, app } => {
                    self.settings.forget_app(&self.paths, service, &app)
                }
                Effect::Store(file) => {
                    let stored = self
                        .settings
                        .store(&self.paths, file)
                        .map_err(|e| e.to_string());
                    if file == File::DataControl && stored.is_ok() {
                        self.settings.reload_clipboard(&self.paths);
                    }
                    stored
                }
                Effect::SetAppearance { dark } => self.set_appearance(dark),
                Effect::SetTheme { dark } => {
                    // the file first, so the menu bar's schedule sees the new switch before the look changes
                    let stored = self
                        .settings
                        .store(&self.paths, File::Appearance)
                        .map_err(|e| e.to_string());
                    match (stored, dark) {
                        (Ok(()), Some(dark)) => self.set_appearance(dark),
                        (stored, _) => stored,
                    }
                }
                Effect::SetLoginItem { id, enable } => {
                    let done = crate::system::set_login_item(&id, enable)
                        .map_err(|e| format!("The login item could not be changed: {e}"));
                    self.live.login_items = crate::system::login_items();
                    done
                }
                Effect::SetDefaultBrowser { id } => {
                    let done = crate::system::set_default_browser(&id)
                        .map_err(|e| format!("The default browser could not be changed: {e}"));
                    self.live.browsers = crate::system::browsers();
                    done
                }
                Effect::SetAccessibility { key, value } => self.set_accessibility(key, &value),
                Effect::SetSystemAccessibility => self.save_login_screen(),
            };
            if let Err(e) = outcome {
                eprintln!("cupertino-settings: {e}");
                self.settings = before;
                self.errors.push((id, e));
                break;
            }
        }
        Ok(())
    }

    /// Make `call` on its service; a Join waits for the device until it connects or fails. The readings are refreshed
    /// after it, so the control shows the service's state, not the asked one.
    pub(crate) fn service_call(
        &mut self,
        id: Option<ControlId>,
        call: &Call,
    ) -> std::result::Result<(), String> {
        if let Call::Pair(id) = call {
            let notify = self.notify.clone();
            return self
                .services
                .pair(id, move |result| notify(Background::Paired(result)));
        }
        // a call that can take long runs on a thread: the window goes on, and its end refreshes the readings
        let notify = self.notify.clone();
        let asked = call.clone();
        if let Some(started) = self.services.call_in_background(call, move |result| {
            notify(Background::Called {
                id,
                call: asked,
                result,
            })
        }) {
            if let (Ok(()), Call::Join { ssid, .. }) = (&started, call) {
                if let crate::live::Reading::Ready(w) = &mut self.live.wifi {
                    w.joining = Some(ssid.clone());
                    w.failure = None;
                }
            }
            return started;
        }
        let result = self.services.call(call);
        if let (Ok(()), Call::Join { ssid, .. }) = (&result, call) {
            if let crate::live::Reading::Ready(w) = &mut self.live.wifi {
                w.joining = Some(ssid.clone());
                w.failure = None;
            }
        }
        let now = self.now();
        self.refresh(now).map_err(|e| e.to_string())?;
        result
    }

    /// Ask sessiond whether the Login Screen section shows at this opening of the Accessibility pane: it stays hidden
    /// until the answer, and the refusal shown under it at the last opening goes.
    /// Read the graphics memory only the driver's tool reports on a thread, once; the pane shows at once without
    /// it and gains it when [`Background::VideoMemory`] arrives.
    pub(crate) fn ask_video_memory(&mut self) {
        if self.video_memory.is_some() {
            return;
        }
        self.video_memory = Some(VideoMemory::Reading);
        let notify = self.notify.clone();
        let read = self.read_video_memory.clone();
        let spawned = std::thread::Builder::new()
            .name("settings-video-memory".into())
            .spawn(move || notify(Background::VideoMemory(read())));
        if let Err(e) = spawned {
            eprintln!("cupertino-settings: cannot read the graphics memory: {e}");
            self.video_memory = Some(VideoMemory::Read(Vec::new()));
        }
    }

    pub(crate) fn ask_login_screen(&mut self) {
        self.live.login_screen = None;
        self.errors.retain(|(id, _)| *id != ControlId::LoginScreen);
        self.login_opening += 1;
        let opening = self.login_opening;
        let notify = self.notify.clone();
        // a failure is logged and leaves the section hidden
        let _ = self.services.can_set_system_accessibility(move |allowed| {
            notify(Background::LoginScreen { opening, allowed })
        });
    }

    /// Use Mine: send the person's allow-listed options to sessiond; the row shows "Saving…" until the reply.
    pub(crate) fn save_login_screen(&mut self) -> std::result::Result<(), String> {
        let session = Session {
            dark: self.dark,
            display: self.display,
        };
        let options = panes::login_screen_options(session, &self.settings);
        let notify = self.notify.clone();
        self.services
            .set_system_accessibility(options, move |result| {
                notify(Background::LoginScreenSaved(result))
            })?;
        self.login_saving = true;
        if let Some(login) = &mut self.live.login_screen {
            login.saving = true;
        }
        Ok(())
    }

    /// What a sheet's key or press decided.
    pub(crate) fn sheet_outcome(&mut self, outcome: Outcome) -> Result<()> {
        match outcome {
            Outcome::Stay => {}
            Outcome::Cancel => {
                self.picking_exclusion = false;
                self.sheet = None;
                self.agent_answer = None;
            }
            Outcome::Answer(answer) => {
                self.sheet = None;
                if let Some((_, sender)) = self.agent_answer.take() {
                    // the daemon may have ended the pairing meanwhile: then nobody waits for the answer
                    let _ = sender.try_send(answer);
                }
            }
            Outcome::Resolution { key, dpi } => {
                self.sheet = None;
                if let Err(e) = self.set_mouse_dpi(&key, dpi) {
                    eprintln!("cupertino-settings: {e}");
                }
            }
            Outcome::Unallow(program) => {
                self.sheet = None;
                if let Err(e) = self.settings.unallow_clipboard(&self.paths, &program) {
                    eprintln!("cupertino-settings: {e}");
                    self.errors.push((ControlId::ClipboardAdd, e));
                }
            }
            // the sheet stays with the reason until the program is allowed
            Outcome::Program(program) => match self.settings.allow_clipboard(&self.paths, &program)
            {
                Ok(()) => self.sheet = None,
                Err(e) => {
                    if let Some(sheet) = &mut self.sheet {
                        sheet.failed(&e);
                    }
                }
            },
            Outcome::Picked { path, folder } if std::mem::take(&mut self.picking_exclusion) => {
                let _ = folder;
                self.sheet = None;
                let text = path.to_string_lossy().into_owned();
                if !self.settings.launcher.excluded.contains(&text) {
                    self.settings.launcher.excluded.push(text);
                    if let Err(e) = self.settings.store(&self.paths, File::Launcher) {
                        eprintln!("cupertino-settings: {e}");
                    }
                }
            }
            Outcome::Picked { path, folder } => {
                self.sheet = None;
                let added = if folder {
                    self.settings.add_wallpaper_folder(&self.paths, &path)
                } else {
                    self.settings.set_wallpaper(&self.paths, &path)
                };
                match added {
                    Ok(()) => self.load_pictures(),
                    Err(e) => eprintln!("cupertino-settings: {e}"),
                }
            }
            Outcome::Do(call) => match self.service_call(None, &call) {
                Ok(()) => self.sheet = None,
                // the passphrase sheet stays with the error (§7.2); a failed removal closes it and shows the error
                Err(e) => match &mut self.sheet {
                    Some(sheet) if matches!(call, Call::Join { .. }) => sheet.failed(&e),
                    _ => {
                        self.sheet = None;
                        eprintln!("cupertino-settings: {e}");
                        if let crate::live::Reading::Ready(b) = &mut self.live.bluetooth {
                            b.failure = Some(e);
                        }
                    }
                },
            },
        }
        Ok(())
    }

    /// List the pictures of the packs and of the added folders, and start the thumbnails of those without one.
    pub(crate) fn load_pictures(&mut self) {
        let folders: Vec<std::path::PathBuf> = self
            .settings
            .compositor
            .wallpaper_folders
            .iter()
            .flatten()
            .map(std::path::PathBuf::from)
            .collect();
        self.live.pictures = Pictures {
            bundled: Vec::new(),
            folders: folders.iter().map(|f| (f.clone(), Vec::new())).collect(),
            default: self.assets.wallpaper_path(false),
            loading: (0..=folders.len()).collect(),
        };
        let current: Vec<std::path::PathBuf> = self
            .settings
            .compositor
            .wallpaper
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        self.ask_thumbnails(current);
        // each listing on a thread of its own: a folder on a hung mount shows "Loading…" and holds nothing else up
        self.start_listing(ListingKey::Packs, None);
        for folder in folders {
            self.start_listing(ListingKey::Pictures(folder), None);
        }
    }

    /// Start the read of `key` on a thread unless one is running (a folder on a dead mount keeps one thread, however
    /// often it is visited); it is read once more when the running one is answered if it was asked for meanwhile.
    pub(crate) fn start_listing(
        &mut self,
        key: ListingKey,
        request: Option<cupertino_filepicker::Request>,
    ) {
        if let Some(request) = self.listings.ask(key.clone(), request) {
            self.run_listing(key, request);
        }
    }

    /// Run the read of `key` on a thread; its answer comes back as a [`Background`] event.
    pub(crate) fn run_listing(
        &mut self,
        key: ListingKey,
        request: Option<cupertino_filepicker::Request>,
    ) {
        let notify = self.notify.clone();
        let spawned_key = key.clone();
        let job: Box<dyn FnOnce() -> Background + Send> = match key.clone() {
            ListingKey::Packs => {
                let assets = self.assets.clone();
                Box::new(move || Background::Pictures {
                    files: assets.wallpaper_files(),
                    key,
                })
            }
            ListingKey::Pictures(folder) => Box::new(move || Background::Pictures {
                files: cupertino_assets::pictures_in(&folder),
                key,
            }),
            ListingKey::Chooser(..) => {
                let Some(request) = request else {
                    self.listings.abandon(&key);
                    return;
                };
                Box::new(move || Background::Listing(request.run()))
            }
        };
        let spawned = std::thread::Builder::new()
            .name("settings-listing".into())
            .spawn(move || notify(job()));
        if let Err(e) = spawned {
            eprintln!("cupertino-settings: cannot list a folder: {e}");
            self.listings.abandon(&spawned_key);
        }
    }

    /// Make the thumbnails of `images` that have none and were not asked for yet, one after the other on a thread.
    pub(crate) fn ask_thumbnails(&mut self, images: Vec<std::path::PathBuf>) {
        let wanted: Vec<std::path::PathBuf> = images
            .into_iter()
            .filter(|image| {
                !self.thumbnails.contains_key(image) && self.thumbnails_asked.insert(image.clone())
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let notify = self.notify.clone();
        let spawned = std::thread::Builder::new()
            .name("wallpaper-thumbnails".into())
            .spawn(move || {
                let cache = cupertino_thumbnails::Thumbnailer::from_env("cupertino-settings");
                for image in wanted {
                    match cache.get_or_create(&image, cupertino_thumbnails::ThumbSize::Large) {
                        Ok(thumbnail) => {
                            let pixels = decode_thumbnail(&thumbnail, THUMBNAIL_EDGE);
                            notify(Background::Thumbnail {
                                image,
                                thumbnail,
                                pixels,
                            })
                        }
                        Err(e) => {
                            eprintln!(
                                "cupertino-settings: no thumbnail of {}: {e}",
                                image.display()
                            )
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("cupertino-settings: cannot start the thumbnails: {e}");
        }
    }

    /// Hand the chooser's next listing to a thread; its answer comes back as [`Background::Listing`].
    pub(crate) fn drive_picker(&mut self) {
        let Some(request) = self.sheet.as_mut().and_then(Sheet::picker_request) else {
            return;
        };
        let (dir, hidden, mode) = request.key();
        let key = ListingKey::Chooser(dir, hidden, mode);
        self.start_listing(key, Some(request));
    }

    /// Store the typed resolution of the mouse `key`; the server applies the file.
    pub(crate) fn set_mouse_dpi(&mut self, key: &str, dpi: u32) -> std::result::Result<(), String> {
        self.settings.set_mouse_dpi(&self.paths, key, dpi)
    }

    /// Something from a bus thread: a pane to show, a question of the pairing agent, the end of a pairing.
    pub(crate) fn background_event(&mut self, event: Background) -> Result<()> {
        match event {
            Background::ShowPane(pane, token) => {
                self.go(pane)?;
                if !token.is_empty() {
                    self.activation = Some(token);
                }
            }
            Background::Agent(AgentEvent::Prompt {
                id,
                device,
                prompt,
                answer,
            }) => {
                // a new question replaces an open one: its pairing is over (the daemon pairs one device at a time)
                self.sheet = Some(Sheet::pairing(id, &device, prompt));
                self.agent_answer = answer.map(|a| (id, a));
            }
            Background::Agent(AgentEvent::Cancel(id)) => {
                if self.sheet.as_ref().and_then(Sheet::pairing_id) == Some(id) {
                    self.sheet = None;
                    self.agent_answer = None;
                }
            }
            Background::Pictures { key, files } => {
                if let Some(request) = self.listings.answered(&key) {
                    self.run_listing(key.clone(), request);
                }
                let images: Vec<std::path::PathBuf> =
                    files.iter().map(|f| f.light.clone()).collect();
                let pictures = &mut self.live.pictures;
                // the pane may list other folders by now: the answer goes to the one it is for, if still listed
                let group = match &key {
                    ListingKey::Packs => Some(0),
                    ListingKey::Pictures(folder) => pictures
                        .folders
                        .iter()
                        .position(|(f, _)| f == folder)
                        .map(|i| i + 1),
                    ListingKey::Chooser(..) => None,
                };
                if let Some(group) = group {
                    pictures.loading.remove(&group);
                    match group {
                        0 => pictures.bundled = files,
                        n => pictures.folders[n - 1].1 = files,
                    }
                    self.ask_thumbnails(images);
                }
            }
            Background::Listing(listing) => {
                let (dir, hidden, mode) = listing.key();
                let key = ListingKey::Chooser(dir, hidden, mode);
                if let Some(request) = self.listings.answered(&key) {
                    self.run_listing(key, request);
                }
                if let Some(sheet) = self.sheet.as_mut() {
                    sheet.apply_listing(listing);
                }
            }
            Background::Thumbnail {
                image,
                thumbnail,
                pixels,
            } => {
                if let Some(pixels) = pixels {
                    self.thumbnail_images.insert(image.clone(), pixels);
                }
                self.thumbnails.insert(image, thumbnail);
            }
            Background::LoginScreen { opening, allowed } => {
                if opening == self.login_opening {
                    self.live.login_screen = allowed.then(|| LoginScreen {
                        current: system_accessibility(std::path::Path::new(SYSTEM_ACCESSIBILITY)),
                        saving: self.login_saving,
                    });
                }
            }
            Background::VideoMemory(list) => {
                if !list.is_empty() {
                    if let Some(system) = &mut self.live.system {
                        system.about =
                            Some(crate::about::About::read_with(&self.about_root, &list));
                    }
                }
                self.video_memory = Some(VideoMemory::Read(list));
            }
            Background::LoginScreenSaved(result) => {
                self.login_saving = false;
                if let Some(login) = &mut self.live.login_screen {
                    login.saving = false;
                    login.current =
                        system_accessibility(std::path::Path::new(SYSTEM_ACCESSIBILITY));
                }
                if let Err(e) = result {
                    self.errors.push((ControlId::LoginScreen, e));
                }
            }
            Background::Files(WatchEvent::Changed(changed)) => {
                if let Err(e) = self.settings.reload(&self.paths) {
                    eprintln!("cupertino-settings: the settings could not be read again: {e}");
                }
                if self
                    .paths
                    .desktop_control
                    .as_ref()
                    .is_some_and(|p| changed.contains(p))
                {
                    self.settings.reload_desktop_control(&self.paths);
                }
                self.sync_accessibility();
            }
            Background::Files(WatchEvent::Error(e)) => {
                eprintln!("cupertino-settings: watching the settings files: {e}")
            }
            Background::Called { id, call, result } => {
                if let Err(e) = result {
                    eprintln!("cupertino-settings: {e}");
                    match (&call, &mut self.live.wifi, &mut self.live.bluetooth) {
                        (Call::Join { .. }, crate::live::Reading::Ready(w), _) => {
                            w.joining = None;
                            w.failure = Some(e);
                        }
                        (
                            Call::Connect(_) | Call::DisconnectDevice(_) | Call::Remove { .. },
                            _,
                            crate::live::Reading::Ready(b),
                        ) => b.failure = Some(e),
                        _ => {
                            if let Some(id) = id {
                                self.errors.retain(|(e, _)| *e != id);
                                self.errors.push((id, e));
                            }
                        }
                    }
                }
                let now = self.now();
                self.refresh(now)?;
            }
            Background::Paired(result) => {
                if let (Err(e), crate::live::Reading::Ready(b)) = (result, &mut self.live.bluetooth)
                {
                    b.failure = Some(e);
                }
                let now = self.now();
                self.refresh(now)?;
            }
        }
        Ok(())
    }

    /// The level slider `id` moved to `value` at `now`: the value to write now, if any.
    pub(crate) fn level_moved(&mut self, id: ControlId, value: f64, now: f64) -> Option<f64> {
        let drag = match &mut self.level {
            Some(drag) if drag.id == id => drag,
            level => level.insert(LevelDrag {
                id,
                writer: LevelWriter::default(),
                moved: now,
                value,
            }),
        };
        drag.moved = now;
        drag.value = value;
        drag.writer.moved(value, now)
    }

    /// The level slider's timer at `now`: its control, the waiting value to write if it is due, and whether the drag
    /// goes on. A slider still for [`LEVEL_SETTLE`] with nothing waiting ends its drag.
    pub(crate) fn level_tick(&mut self, now: f64) -> Option<(ControlId, Option<f64>, bool)> {
        let LevelDrag {
            id, writer, moved, ..
        } = self.level.as_mut()?;
        let id = *id;
        let write = writer
            .due()
            .filter(|due| *due <= now)
            .and_then(|_| writer.tick(now));
        let more = writer.due().is_some() || now - *moved < LEVEL_SETTLE;
        if !more {
            self.level = None;
        }
        Some((id, write, more))
    }

    /// The slider `id` moved to `value` at `now`: the drag of another slider that this one ends, to commit now.
    pub(crate) fn slide_moved(
        &mut self,
        id: ControlId,
        value: f64,
        now: f64,
    ) -> Option<(ControlId, f64)> {
        let ended = self
            .slide
            .take()
            .filter(|drag| drag.id != id)
            .map(|drag| (drag.id, drag.value));
        let applied = self
            .slide
            .as_ref()
            .filter(|drag| drag.id == id)
            .and_then(|drag| drag.applied);
        self.slide = Some(SliderDrag {
            id,
            value,
            moved: now,
            applied,
        });
        ended
    }

    /// The slider's timer at `now`: the value to commit (a moved knob every [`SLIDE_APPLY_EVERY`], the last value once
    /// it has settled), and whether a drag still waits.
    pub(crate) fn slide_tick(&mut self, now: f64) -> (Option<(ControlId, f64)>, bool) {
        let Some(mut drag) = self.slide.take() else {
            return (None, false);
        };
        let unapplied = drag.applied.is_none_or(|(v, _)| v != drag.value);
        if now - drag.moved >= LEVEL_SETTLE {
            return (unapplied.then_some((drag.id, drag.value)), false);
        }
        let due = drag
            .applied
            .is_none_or(|(_, at)| now - at >= SLIDE_APPLY_EVERY);
        let commit = (unapplied && due).then(|| {
            drag.applied = Some((drag.value, now));
            (drag.id, drag.value)
        });
        self.slide = Some(drag);
        (commit, true)
    }

    /// The slider `id` was let go at `value`: its drag ends, and the value to commit when it was not committed yet.
    pub(crate) fn slide_ended(&mut self, id: ControlId, value: f64) -> Option<(ControlId, f64)> {
        let drag = self.slide.take();
        match drag {
            Some(drag) if drag.id != id => {
                // another slider's drag, which this one ends; this one's value goes in after it
                self.slide = Some(SliderDrag {
                    id,
                    value,
                    moved: f64::NEG_INFINITY,
                    applied: None,
                });
                Some((drag.id, drag.value))
            }
            Some(SliderDrag {
                applied: Some((v, _)),
                ..
            }) if v == value => None,
            _ => Some((id, value)),
        }
    }

    /// Write the level slider `id` at `v`.
    pub(crate) fn level_write(&mut self, id: ControlId, v: f64) -> Result<()> {
        for effect in panes::change(id, Value::Fraction(v), &mut self.settings, &self.live) {
            if let Effect::Service(call) = effect {
                if let Err(e) = self.services.call(&call) {
                    eprintln!("cupertino-settings: {e}");
                    self.errors.retain(|(e, _)| *e != id);
                    self.errors.push((id, e));
                }
            }
        }
        Ok(())
    }

    /// Ask the server for the appearance (`org.cupertino.Session1.SetAppearance`); the control shows it at once and the
    /// session's report replaces it.
    pub(crate) fn set_appearance(&mut self, dark: bool) -> std::result::Result<(), String> {
        let name = if dark { "dark" } else { "light" };
        let bus = self.bus()?;
        bus.call_method(
            Some(SESSION_NAME),
            SESSION_PATH,
            Some(SESSION_NAME),
            "SetAppearance",
            &(name,),
        )
        .map_err(|e| {
            format!(
                "The appearance could not be changed: {}",
                services::failure(&e)
            )
        })?;
        self.dark = dark;
        Ok(())
    }

    /// The session bus, connected on first use.
    pub(crate) fn bus(&mut self) -> std::result::Result<&zbus::blocking::Connection, String> {
        if self.bus.is_none() {
            self.bus = Some(zbus::blocking::Connection::session().map_err(|e| {
                services::log_once(&format!("the session bus: {e}"));
                "The session is not available.".to_string()
            })?);
        }
        Ok(self.bus.as_ref().expect("connected above"))
    }

    /// Ask the server to set one key of `accessibility.conf` (`org.cupertino.Session1.SetAccessibility`) and show what
    /// the server now holds; the display options themselves arrive from the session.
    pub(crate) fn set_accessibility(
        &mut self,
        key: &str,
        value: &str,
    ) -> std::result::Result<(), String> {
        let bus = self.bus()?;
        bus.call_method(
            Some(SESSION_NAME),
            SESSION_PATH,
            Some(SESSION_NAME),
            "SetAccessibility",
            &(key, value),
        )
        .map_err(|e| {
            format!(
                "The setting could not be changed: {}",
                services::failure(&e)
            )
        })?;
        self.sync_accessibility();
        Ok(())
    }

    /// The accessibility settings as the server holds them (`org.cupertino.Session1.Accessibility`): the server is
    /// their only writer, and it keeps them in memory only when it has no file to store them in, so the file is not
    /// the truth. Without the property the file's values stay.
    pub(crate) fn sync_accessibility(&mut self) {
        let problems = match self.services.accessibility() {
            Reading::Ready(held) => self.settings.apply_session_accessibility(&held),
            Reading::Unavailable(reason) => {
                vec![format!(
                    "the session's accessibility settings cannot be read: {reason}"
                )]
            }
            Reading::Unknown => Vec::new(),
        };
        for problem in problems {
            if self.reported.insert(problem.clone()) {
                eprintln!("cupertino-settings: {problem}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_prompt_at_a_full_queue_is_refused_without_waiting() {
        let (notify, _background) = background_channel(|| {});
        for _ in 0..BACKGROUND_QUEUE {
            notify(Background::Paired(Ok(())));
        }
        let (answer, reply) = async_channel::bounded(1);
        let agent = notify.clone();
        let (done, returned) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            agent(Background::Agent(AgentEvent::Prompt {
                id: 1,
                device: "Keyboard".into(),
                prompt: crate::sheet::Prompt::Confirm {
                    passkey: Some(123456),
                },
                answer: Some(answer),
            }));
            done.send(()).unwrap();
        });
        returned
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a bus executor never waits for room");
        assert!(reply.recv_blocking().is_err(), "a dropped prompt refuses");
    }

    #[test]
    fn pane_requests_cannot_grow_the_background_queue_and_workers_wait_for_room() {
        let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = wakes.clone();
        let (notify, background) = background_channel(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        for _ in 0..BACKGROUND_QUEUE * 4 {
            notify(Background::ShowPane(PaneId::Wifi, String::new()));
        }
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            BACKGROUND_QUEUE
        );

        let worker = notify.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            worker(Background::Paired(Ok(())));
            done.send(()).unwrap();
        });
        assert!(finished
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());
        assert!(matches!(background.recv(), Ok(Background::ShowPane(..))));
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker's event is queued once there is room");
        let queued: Vec<_> = background.try_iter().collect();
        assert_eq!(queued.len(), BACKGROUND_QUEUE);
        assert!(matches!(queued.last(), Some(Background::Paired(Ok(())))));
    }
}
