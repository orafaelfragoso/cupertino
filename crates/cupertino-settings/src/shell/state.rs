//! What the Settings window shows and does, apart from how it shows it: the history, the settings and their files,
//! the live services (read and called off the window's thread: [`super::reading`], [`super::calls`]), the files and
//! programs the panes read ([`super::files`]), the sheets, and what the buses and watches tell through [`Background`].

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use super::calls::Asker;
use super::desk::Desk;
use super::reading::{Buses, Watches};
use crate::agent::AgentEvent;
use crate::controls::{Control, LevelWriter, Value};
use crate::history::History;
use crate::inflight::Reads;
use crate::live::{display_name, DeviceAction, Live};
use crate::meter::Meter;
use crate::pages::Page;
use crate::panes::{self, Call, ControlId, Effect, Section, Session};
use crate::services::Services;
use crate::settings::{File, Paths, Settings};
use crate::sheet::{Answer, Outcome, Sheet};
use crate::sidebar;
use crate::watch::{Changed, Interest, Which};
use crate::PaneId;
use cupertino_assets::Assets;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::filepicker::Mode;

/// A level slider still this long, in seconds, has ended its drag: the live readings show again.
const LEVEL_SETTLE: f64 = 0.5;
/// While a slider without stops is dragged, its value is applied at most this often, in seconds, so what it sets
/// follows the knob; the value it is let go at is applied at once.
const SLIDE_APPLY_EVERY: f64 = 0.15;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Something for the window loop from a bus or a watch.
pub(crate) enum Background {
    ShowPane(PaneId, String),
    Agent(AgentEvent),
    /// The watch of this bus has the bus daemon's rules ([`crate::watch`]).
    Watching(Which),
    /// A settings file changed on disk.
    Files(WatchEvent),
    /// A service the panes read sent a change signal ([`crate::watch`]).
    Changed(Changed),
}

/// Reads the graphics memory only the driver's tool reports, by PCI address; on the I/O pool.
pub(crate) type ReadVideoMemory =
    std::sync::Arc<dyn Fn() -> Vec<(crate::about::PciAddress, u64)> + Send + Sync>;

/// The driver's tool, given a short time to answer.
fn read_video_memory() -> Vec<(crate::about::PciAddress, u64)> {
    crate::about::nvidia_video_memory(crate::about::VIDEO_MEMORY_TIMEOUT)
}

/// The graphics memory only the driver's tool reports.
pub(crate) enum VideoMemory {
    /// Its task is running.
    Reading,
    Read(Vec<(crate::about::PciAddress, u64)>),
}

/// What a listing task reads: the packs' pictures, the pictures of an added folder, or a folder of the chooser.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ListingKey {
    Packs,
    Pictures(std::path::PathBuf),
    Chooser(std::path::PathBuf, bool, cupertino_kit::filepicker::Mode),
}

/// What the open panel chooses: a picture or a folder of them for the Wallpaper pane, a program to allow to use the
/// clipboard, or a folder the file search leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choosing {
    Wallpaper,
    WallpaperFolder,
    Program,
    Exclusion,
}

impl Choosing {
    /// What the panel lists and chooses.
    pub(crate) fn mode(self) -> Mode {
        match self {
            Choosing::Wallpaper => Mode::Photo,
            Choosing::WallpaperFolder | Choosing::Exclusion => Mode::Folder,
            Choosing::Program => Mode::Program,
        }
    }
}

/// Hands a [`Background`] to the window loop and wakes it.
pub(crate) type Notify = std::sync::Arc<dyn Fn(Background) + Send + Sync>;

/// Background events waiting for the window loop.
const BACKGROUND_QUEUE: usize = 64;

/// The window loop owns the receiver until it ends; what arrives after that has no window. What arrives before the
/// window exists waits for the loop's first turn. What finds the queue full is dropped, so a caller can neither grow the
/// queue nor block a bus executor or the window's loop: a dropped pane request shows nothing, a dropped agent prompt
/// refuses the pairing, a dropped change signal is not needed (the changes queued read everything again), a dropped
/// report of the settings files' watch is followed by a later one that reads the files again, and a watch whose
/// report of its rules is dropped is no less watched.
pub(crate) fn background_channel(
    wake: impl Fn() + Send + Sync + 'static,
) -> (Notify, std::sync::mpsc::Receiver<Background>) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(BACKGROUND_QUEUE);
    let notify: Notify = std::sync::Arc::new(move |event| {
        if sender.try_send(event).is_ok() {
            wake();
        }
    });
    (notify, receiver)
}

/// The Wi-Fi pane scans this often while it is shown and the radio is on (approximate, §6.4).
pub(crate) const SCAN: f64 = 10.0;
/// How often the battery's recorded charge is read again while the Battery pane shows, in seconds.
pub(crate) const HISTORY_REFRESH: f64 = 60.0;
/// How often a pane reads again the readings that have no change signal, in seconds.
pub(crate) const POLL: f64 = 1.0;
/// How often the input level meter reads the level while it shows, in seconds.
pub(crate) const METER_POLL: f64 = 0.1;
/// How often the Focus pane reads again the durations it offers, which follow the time of day, in seconds.
const LIFETIMES_POLL: f64 = 60.0;

/// How soon a bus whose watch ended and could not start again is tried again, in seconds; each failed try doubles it
/// up to [`REWATCH_MAX`].
pub(crate) const REWATCH_FIRST: f64 = 1.0;
pub(crate) const REWATCH_MAX: f64 = 60.0;

/// The change signals `pane` reads, besides those every pane reads ([`crate::watch::Interest`]).
pub(crate) fn reads(pane: PaneId) -> &'static [Changed] {
    match pane {
        PaneId::Focus => &[Changed::Centre],
        PaneId::Wifi | PaneId::Network | PaneId::Vpn => &[Changed::Network],
        PaneId::Bluetooth => &[Changed::Bluetooth],
        PaneId::Battery => &[Changed::Battery],
        PaneId::Displays => &[Changed::Brightness],
        PaneId::Keyboard => &[Changed::KeyboardBacklight],
        _ => &[],
    }
}

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
    /// The services, called on the I/O pool, and the other tasks the window starts.
    pub(crate) desk: Desk,
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
    /// The accessibility settings as the server held them at its last reading, put over the files' after each read of
    /// them ([`State::sync_accessibility`]).
    pub(crate) held_accessibility: Option<HashMap<String, String>>,
    pub(crate) sheet: Option<Sheet>,
    /// The open panel, a window of its own, and what it chooses.
    pub(crate) chooser: Option<(Choosing, crate::chooser::Chooser)>,
    /// The icons of the panel's items once read, for the window to take, and whether they were asked for.
    pub(crate) file_icons: Option<cupertino_kit::file_icons::FileIcons>,
    pub(crate) file_icons_asked: bool,
    pub(crate) level: Option<LevelDrag>,
    pub(crate) slide: Option<SliderDrag>,
    pub(crate) start: std::time::Instant,
    pub(crate) refreshed: f64,
    pub(super) audio: super::audio::Audio,
    /// The input level meter, while the Sound pane shows the input.
    pub(crate) meter: Option<Meter>,
    /// Whether a reading of the shown pane is on its way, and whether another was asked for meanwhile.
    pub(crate) reading: bool,
    pub(crate) read_again: bool,
    /// The Bluetooth pane waits for its reading to show an adapter before it serves its pairing agent.
    pub(crate) agent_wanted: bool,
    pub(crate) scanned: f64,
    /// When the battery's charge history was last read.
    pub(crate) history_read: f64,
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
    /// The cache folder the thumbnails are kept under; the standard one (`XDG_CACHE_HOME`) when `None`.
    pub(crate) thumbnail_cache: Option<PathBuf>,
    pub(crate) listings: Reads<ListingKey, Option<cupertino_kit::filepicker::Request>>,
    pub(crate) agent_answer: Option<(u64, async_channel::Sender<Answer>)>,
    pub(crate) agent_registered: bool,
    pub(crate) login_opening: u64,
    pub(crate) login_saving: bool,
    /// Where About's readings come from: `/`, or a test's tree.
    pub(crate) about_root: PathBuf,
    /// The graphics memory only the driver's tool reports, read once by a task when a pane first shows the
    /// graphics; `None` until it is asked for.
    pub(crate) video_memory: Option<VideoMemory>,
    /// Reads that memory; on the I/O pool, as it runs a program.
    pub(crate) read_video_memory: ReadVideoMemory,
    /// An activation token another start of Settings handed over, for the window to raise itself with.
    pub(crate) activation: Option<String>,
    /// The buses the watches and the Sound pane's client reach, the watches, and the change signals the shown pane
    /// reads, which they share.
    pub(crate) buses: Buses,
    pub(crate) watches: Watches,
    pub(crate) interest: Interest,
    /// Watches the settings files, until [`State::watching`] hands it to the loop.
    files: Option<ConfigWatcher>,
    /// Watches `desktop-control.conf`, which the compositor writes when an app first asks, on its own: a file that
    /// can't be read stops no other file's watch.
    answers: Option<ConfigWatcher>,
}

impl State {
    /// The state of a window opening on `pane` (the first row when it is hidden); `display_height` is the height of the
    /// display the Dock sits on, which its default tile size depends on. The buses and the watches answer through
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
        let files = match ConfigWatcher::new(files) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("cupertino-settings: cannot watch the settings files: {e}; the panes show changes of other writers at the next commit");
                None
            }
        };
        let answers = paths.desktop_control.clone().and_then(|path| {
            ConfigWatcher::new(vec![path])
                .inspect_err(|e| {
                    eprintln!("cupertino-settings: cannot watch desktop-control.conf: {e}; the Privacy pane shows its changes when it opens")
                })
                .ok()
        });
        let account = crate::system::Account::read();
        let display = settings.accessibility.display;
        let screen_reader_installed = paths.screen_reader.is_some();
        let devices = services.reads_hardware();
        let buses = Buses {
            session: services.session.reach(),
            system: services.system.reach(),
        };
        let mut state = State {
            paths,
            settings,
            desk: Desk::new(services),
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
            held_accessibility: None,
            sheet: None,
            chooser: None,
            file_icons: None,
            file_icons_asked: false,
            level: None,
            slide: None,
            start: std::time::Instant::now(),
            refreshed: f64::NEG_INFINITY,
            audio: Default::default(),
            meter: None,
            reading: false,
            read_again: false,
            agent_wanted: false,
            scanned: f64::NEG_INFINITY,
            history_read: f64::NEG_INFINITY,
            notify,
            assets: Assets::discover(),
            thumbnails: HashMap::new(),
            thumbnail_images: HashMap::new(),
            account_picture: None,
            account,
            thumbnails_asked: std::collections::HashSet::new(),
            thumbnail_cache: None,
            listings: Reads::default(),
            agent_answer: None,
            agent_registered: false,
            login_opening: 0,
            about_root: PathBuf::from("/"),
            video_memory: None,
            read_video_memory: std::sync::Arc::new(read_video_memory),
            login_saving: false,
            activation: None,
            buses,
            watches: Watches::default(),
            interest: Interest::default(),
            files,
            answers,
        };
        state.read_account_picture();
        state.sync_accessibility();
        state.start_watching(devices);
        state.read_appearance();
        state.enter_pane(None, pane)?;
        Ok(state)
    }

    /// The tasks of the window loop that follow the settings files, once: they never end, and each change goes to the
    /// loop as [`Background::Files`].
    pub(crate) fn watching<T: 'static>(
        &mut self,
    ) -> Vec<std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>> {
        [self.files.take(), self.answers.take()]
            .into_iter()
            .flatten()
            .map(|watch| {
                let to_loop = self.notify.clone();
                let follow = cupertino_kit::task::follow_settings(watch, move |event| {
                    to_loop(Background::Files(event))
                });
                Box::pin(follow) as std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>
            })
            .collect()
    }

    /// How soon the shown pane reads its readings again when no change signal comes, in seconds; `None` when every
    /// reading it shows has a change signal ([`crate::watch`]) or it shows none. Only these panes wake the window on
    /// a timer:
    /// * Sound: the volume and the outputs are read from the sound server, which sends no change signal; the input
    ///   level meter reads the level ten times a second while it shows.
    /// * Displays: the displays come from the server's `Displays1`, whose change signal the window does not watch.
    /// * Wi-Fi and Network: the interfaces' addresses are read from the kernel, which sends no change signal; the
    ///   Wi-Fi pane also scans every ten seconds.
    /// * Notifications: a new notification's application has no change signal.
    /// * Focus: the durations offered follow the time of day (once a minute).
    /// * Battery: the charge history (once a minute; the rest has the power service's signals).
    pub(crate) fn poll(&self) -> Option<f64> {
        match self.content_pane {
            PaneId::Sound if self.meter_running() => Some(METER_POLL),
            PaneId::Sound
            | PaneId::Displays
            | PaneId::Wifi
            | PaneId::Network
            | PaneId::Notifications => Some(POLL),
            PaneId::Focus => Some(LIFETIMES_POLL),
            PaneId::Battery => Some(HISTORY_REFRESH),
            _ => None,
        }
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

    /// The content now shows `to` (`from` before): start what the new pane needs and stop what the old one started.
    pub(crate) fn enter_pane(&mut self, from: Option<PaneId>, to: PaneId) -> Result<()> {
        self.interest.set(reads(to));
        if let Some(from) = from.filter(|f| *f != to) {
            self.leave_pane(from);
        }
        if from == Some(to) {
            return Ok(());
        }
        self.scanned = f64::NEG_INFINITY;
        self.history_read = f64::NEG_INFINITY;
        self.live.tab = 0;
        if to == PaneId::Sound {
            self.refresh_sound(self.now());
            return Ok(());
        }
        self.read_files(to);
        if to == PaneId::Wallpaper {
            self.load_pictures();
        }
        if to == PaneId::Accessibility {
            self.ask_login_screen();
        }
        if to == PaneId::MenuBar {
            self.desk
                .serve(Services::tray_items, |state, tray| state.live.tray = tray);
        }
        if to == PaneId::Privacy {
            self.settings.reload_clipboard(&self.paths);
            self.settings.reload_screen_sharing(&self.paths);
            self.settings.reload_desktop_control(&self.paths);
        }
        // the pairing agent waits for the reading to show an adapter
        self.agent_wanted = to == PaneId::Bluetooth;
        self.refresh(self.now())?;
        Ok(())
    }

    pub(crate) fn leave_pane(&mut self, pane: PaneId) {
        if pane == PaneId::Sound {
            self.stop_meter();
        }
        if pane == PaneId::Bluetooth {
            let discovering = self.live.bluetooth.ready().is_some_and(|b| b.discovering);
            self.stop_bluetooth(discovering);
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
        // Sound controls change only the session mixer; no file or accessibility bus read is needed.
        if self.content_pane != PaneId::Sound {
            if let Err(e) = self.settings.reload(&self.paths) {
                eprintln!("cupertino-settings: the settings could not be read: {e}");
                self.errors
                    .push((id, "The settings could not be read.".to_string()));
                return Ok(());
            }
            self.apply_held_accessibility();
            self.sync_accessibility();
        }
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
                Effect::Service(call) => {
                    self.service_call(Asker::Row(Some(id)), &call);
                    Ok(())
                }
                Effect::Audio(change) => {
                    self.audio.write(id, change);
                    Ok(())
                }
                Effect::AskPicture { folder } => {
                    self.open_chooser(if folder {
                        Choosing::WallpaperFolder
                    } else {
                        Choosing::Wallpaper
                    });
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
                    self.open_chooser(Choosing::Program);
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
                    if self.content_pane == PaneId::Sound {
                        self.refresh_sound(now);
                        Ok(())
                    } else {
                        self.refresh(now).map_err(|e| e.to_string())
                    }
                }
                Effect::AskExclusion => {
                    self.open_chooser(Choosing::Exclusion);
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
                        self.read_clipboard_states();
                    }
                    stored
                }
                Effect::SetAppearance { dark } => {
                    self.set_appearance(id, dark);
                    Ok(())
                }
                Effect::SetTheme { dark } => {
                    // the file first, so the menu bar's schedule sees the new switch before the look changes
                    let stored = self
                        .settings
                        .store(&self.paths, File::Appearance)
                        .map_err(|e| e.to_string());
                    if let (Ok(()), Some(dark)) = (&stored, dark) {
                        self.set_appearance(id, dark);
                    }
                    stored
                }
                Effect::SetLoginItem { id: item, enable } => {
                    self.set_login_item(id, item, enable);
                    Ok(())
                }
                Effect::SetDefaultBrowser { id: browser } => {
                    self.set_default_browser(id, browser);
                    Ok(())
                }
                Effect::SetAccessibility { key, value } => {
                    self.set_accessibility(id, key, &value);
                    Ok(())
                }
                Effect::SetSystemAccessibility => {
                    self.save_login_screen();
                    Ok(())
                }
            };
            if let Err(e) = outcome {
                eprintln!("cupertino-settings: {e}");
                self.settings = before;
                // a device action that could not start says so on its device's row already, which follows the device
                // when the list changes; the index of the row would not
                let on_device = matches!(id, ControlId::BluetoothDevice(_))
                    && self
                        .live
                        .bluetooth
                        .ready()
                        .is_some_and(|b| b.device_failure.is_some());
                if !on_device {
                    self.errors.push((id, e));
                }
                break;
            }
        }
        Ok(())
    }

    /// What a sheet's key or press decided.
    pub(crate) fn sheet_outcome(&mut self, outcome: Outcome) -> Result<()> {
        match outcome {
            Outcome::Stay => {}
            Outcome::Cancel => {
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
                self.read_clipboard_states();
            }
            // the sheet stays with the reason until the program is allowed
            Outcome::Program(program) => self.allow_clipboard(program),
            // the passphrase sheet waits for the join and stays with its error (§7.2); a removal's sheet closes, and the
            // device's row shows its error
            Outcome::Do(call) => {
                if !matches!(call, Call::Join { .. }) {
                    self.sheet = None;
                }
                self.service_call(Asker::Sheet, &call);
            }
        }
        Ok(())
    }

    /// Store the typed resolution of the mouse `key`; the server applies the file.
    pub(crate) fn set_mouse_dpi(&mut self, key: &str, dpi: u32) -> std::result::Result<(), String> {
        self.settings.set_mouse_dpi(&self.paths, key, dpi)
    }

    /// Something from a bus or a watch: a pane to show, a question of the pairing agent, a change.
    pub(crate) fn background_event(&mut self, event: Background) -> Result<()> {
        match event {
            Background::Changed(changed) => self.changed(&[changed])?,
            Background::Watching(bus) => self.watch_started(bus),
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
                self.apply_held_accessibility();
                self.sync_accessibility();
            }
            Background::Files(WatchEvent::Error(e)) => {
                eprintln!("cupertino-settings: watching the settings files: {e}")
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
            match effect {
                Effect::Audio(change) => self.audio.write(id, change),
                Effect::Service(call) => self.level_call(id, call),
                _ => {}
            }
        }
        Ok(())
    }
}

/// The device a Bluetooth device call acts on, and the action; `None` for any other call (and for a removal not yet
/// confirmed, which only asks).
pub(crate) fn device_call(call: &Call) -> Option<(&str, DeviceAction)> {
    match call {
        Call::Connect(id) => Some((id, DeviceAction::Connect)),
        Call::DisconnectDevice(id) => Some((id, DeviceAction::Disconnect)),
        Call::Remove {
            id,
            confirmed: true,
        } => Some((id, DeviceAction::Forget)),
        Call::Pair(id) => Some((id, DeviceAction::Pair)),
        _ => None,
    }
}
