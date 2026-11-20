//! The component's work off the UI thread and what comes back of it: the session bus worker and the extras' task on
//! the component's connections, the settings files, the desktop entries, Recent Items, the icons, the application
//! associations and the X server of the X11 bridge on Kit's I/O pool, and the timers of the clock and the menus.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cupertino_apps::launch::{self, Launch, OPEN_COMMAND};
use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_defaults::appearance::AppearanceSettings;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::core::{Command as Do, WindowKey};
use cupertino_kit::task;
use cupertino_kit::text::Keymap;

use super::bars::{failure_line, launch_name};
use super::{fail, listen, ConfChange, Handed, Message, Model, X11Answer, PROCESS};
use crate::clock::{clock_text, local_time, next_tick, separators_off};
use crate::desktop::{app_name, AppName};
use crate::extras::Extras;
use crate::extras_bus::ExtrasBus;
use crate::feed::{status_icon_px, StatusState, X11Query};
use crate::icons::{Loader, Pixels, LOAD_DEADLINE};
use crate::layout::SlotKey;
use crate::recent::RecentSources;
use crate::tone::Backdrop as Wallpaper;
use crate::view::{self, PromptPhase};
use crate::worker::{self, Worker};

/// The scale status icons are loaded at: sharp on outputs up to it, scaled down on the others.
const ICON_SCALE: f64 = 2.0;

/// How long reading a settings file, a desktop entry or Recent Items should take: small files on a local disk.
const FILE_DEADLINE: Duration = Duration::from_secs(2);

/// How long the X server of the X11 bridge should take to answer a few questions about its windows.
const X11_DEADLINE: Duration = Duration::from_secs(2);

/// How long finding the default application of a document should take: the MIME database and the associations.
const OPEN_DEADLINE: Duration = Duration::from_secs(5);

/// Seconds since the Unix epoch (0 for a clock set before it).
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Where Recent Items is read from, from the session's directories; `None` (reported) when they are not known.
pub(super) fn recent_sources() -> Option<RecentSources> {
    match XdgDirs::from_env() {
        Ok(dirs) => Some(RecentSources::new(
            &dirs,
            Locale::from_env(),
            Visibility::from_env(),
        )),
        Err(e) => {
            eprintln!("{PROCESS}: no Recent Items: {e}");
            None
        }
    }
}

/// `menubar.conf` from the configuration directory (`CUPERTINO_MENUBAR_CONFIG` overrides the path); every key unset
/// when there is none. Lines that are not understood are reported; with `last_good` their keys keep its values.
fn load_conf(last_good: Option<&MenuBarSettings>) -> Result<MenuBarSettings, String> {
    let Some(path) = MenuBarSettings::path() else {
        return Ok(MenuBarSettings::UNSET);
    };
    let (settings, problems) = MenuBarSettings::load(&path, last_good)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for problem in problems {
        eprintln!(
            "{PROCESS}: {} line {}: {}",
            path.display(),
            problem.line,
            problem.message
        );
    }
    Ok(settings)
}

/// The first reading of `menubar.conf`, with the watch on it and on `appearance.conf`, whose schedule the bar
/// applies. A watch that cannot be set up is reported: the files then apply at the next start.
pub(super) fn first_conf() -> Do<Message> {
    task::io(
        FILE_DEADLINE,
        || {
            let conf = load_conf(None);
            let watch = MenuBarSettings::path().and_then(|path| {
                let files: Vec<PathBuf> = std::iter::once(path.clone())
                    .chain(AppearanceSettings::path())
                    .collect();
                ConfigWatcher::new(files)
                    .map_err(|e| {
                        eprintln!(
                            "{PROCESS}: cannot watch {}: {e}; changes apply at the next start",
                            path.display()
                        );
                    })
                    .ok()
            });
            (conf, watch)
        },
        |(conf, watch)| Message::Conf(conf, Handed::new(watch)),
    )
}

/// The editing chords of the Help menu's search field, from the keyboard profile and the binding files.
pub(super) fn keymap() -> Do<Message> {
    task::io(
        FILE_DEADLINE,
        || {
            Keymap::new(
                cupertino_defaults::keyboard::user_profile(),
                cupertino_kit::text::binding_file::load_default().0,
            )
        },
        |keymap| Message::Keymap(Handed::new(keymap)),
    )
}

/// Find the icon themes and the packs before the first icon is asked for.
pub(super) fn prepare_icons(loader: &Loader) -> Do<Message> {
    let loader = loader.clone();
    task::io(LOAD_DEADLINE, move || loader.prepare(), |()| Message::Done)
}

/// Store the session's look in `compositor.json`, which the server follows while it runs: dark or light, keeping a
/// high-contrast variant the file already holds.
fn write_appearance(dark: bool) -> std::io::Result<()> {
    use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
    let Some(path) = CompositorSettings::path() else {
        return Err(std::io::Error::other("there is no settings folder"));
    };
    let (mut file, _) = CompositorSettings::load(&path, None)?;
    let contrast = matches!(
        file.appearance,
        Some(AppearanceName::LightHighContrast | AppearanceName::DarkHighContrast)
    );
    file.appearance = Some(match (dark, contrast) {
        (false, false) => AppearanceName::Light,
        (true, false) => AppearanceName::Dark,
        (false, true) => AppearanceName::LightHighContrast,
        (true, true) => AppearanceName::DarkHighContrast,
    });
    file.store(&path)?;
    Ok(())
}

/// One step of the schedule of `appearance.conf` at `unix`, the session `dark` or not and `written` asked for last:
/// the look written to `compositor.json` when the schedule says it is time for the other one.
fn schedule_step(dark: bool, written: Option<bool>, unix: f64) -> Option<bool> {
    let schedule = AppearanceSettings::current();
    if !schedule.auto {
        return None;
    }
    let now = local_time(unix.floor() as i64);
    let wanted = schedule.dark_at(u32::from(now.hour) * 60 + u32::from(now.minute));
    if wanted == dark || written == Some(wanted) {
        return None;
    }
    match write_appearance(wanted) {
        Ok(()) => Some(wanted),
        Err(e) => {
            eprintln!("{PROCESS}: cannot switch the appearance: {e}");
            None
        }
    }
}

impl Model {
    /// The session connection is up: the worker serves the watcher on it, and the extras' task starts once the system
    /// connection is known too.
    pub(super) fn session(&mut self, connection: zbus::Connection) -> Vec<Do<Message>> {
        self.services.session = Some(connection.clone());
        let started = async move {
            let started = Worker::start(connection).await.map_err(|e| e.to_string());
            Message::WorkerStarted(Handed::new(started))
        };
        let mut commands = vec![Do::Perform(Box::pin(started))];
        commands.extend(self.start_extras());
        commands
    }

    pub(super) fn system(&mut self, system: Result<zbus::Connection, String>) -> Vec<Do<Message>> {
        self.services.system = Some(system);
        self.start_extras()
    }

    /// Start the extras' task, once both connections are known.
    fn start_extras(&mut self) -> Vec<Do<Message>> {
        if self.services.extras.is_some() {
            return Vec::new();
        }
        let (Some(session), Some(system)) = (&self.services.session, &self.services.system) else {
            return Vec::new();
        };
        let system = system.clone().map_err(zbus::Error::Failure);
        let (extras, events) = ExtrasBus::start(session.clone(), std::future::ready(system));
        if let Err(e) = extras.session_agent(self.services.agent) {
            return fail(e);
        }
        self.services.extras = Some(extras);
        let listening = listen(&events, Message::Extras);
        self.services.extras_events = Some(events);
        vec![listening]
    }

    pub(super) fn worker_started(
        &mut self,
        started: Result<(Worker, async_channel::Receiver<worker::Event>), String>,
    ) -> Vec<Do<Message>> {
        let (worker, events) = match started {
            Ok(started) => started,
            // another program serves the watcher, or the bus refused: the host starts the bar again
            Err(e) => return fail(e),
        };
        let mut commands = vec![listen(&events, Message::Worker)];
        self.services.worker_events = Some(events);
        for request in std::mem::take(&mut self.services.waiting) {
            if let Err(e) = worker.send(request) {
                return fail(e);
            }
        }
        self.services.worker = Some(worker);
        commands.extend(self.refresh());
        commands
    }

    pub(super) fn worker_event(&mut self, event: worker::Event) -> Vec<Do<Message>> {
        let mut commands = match &self.services.worker_events {
            Some(events) => vec![listen(events, Message::Worker)],
            None => Vec::new(),
        };
        match event {
            worker::Event::Layout { source, root } => self.shared.apply_layout(&source, *root),
            worker::Event::LayoutFailed { source } => self.shared.layout_failed(&source),
            worker::Event::StatusUpdated { item } => {
                commands.push(self.read_status_icon(*item));
                return commands;
            }
            worker::Event::StatusRemoved { id } => {
                self.status_reads.remove(&id);
                self.shared.remove_status(&id);
            }
            worker::Event::Registrations(list) => {
                self.shared.registrations = list;
                for i in 0..self.bars.len() {
                    let query = self.bars[i].feed.match_registered(&mut self.shared);
                    commands.extend(self.ask_x11(query));
                }
            }
        }
        commands.extend(self.refresh());
        commands
    }

    /// Load a status item's icon on the I/O pool; the item shows once it is loaded.
    fn read_status_icon(&mut self, item: crate::dbus::sni::StatusItem) -> Do<Message> {
        let read = self.next_status_read;
        self.next_status_read += 1;
        self.status_reads.insert(item.id.clone(), read);
        let loader = self.loader.clone();
        task::io(
            LOAD_DEADLINE,
            move || {
                let icon = loader.status_icon(&item, status_icon_px(ICON_SCALE));
                (item, icon)
            },
            move |(item, icon)| Message::StatusIcon(Box::new(item), read, icon),
        )
    }

    pub(super) fn status_icon(
        &mut self,
        item: crate::dbus::sni::StatusItem,
        read: u64,
        icon: Option<Pixels>,
    ) -> Vec<Do<Message>> {
        // an item that went, or was read again since
        if self.status_reads.get(&item.id) != Some(&read) {
            return Vec::new();
        }
        self.shared.set_status(StatusState {
            item,
            icon: icon.as_ref().map(view::icon_image),
            // its tree arrives as a layout; the one it has stays meanwhile
            menu: None,
        });
        self.refresh()
    }

    pub(super) fn extras(&mut self, extras: Extras) -> Vec<Do<Message>> {
        let mut commands = match &self.services.extras_events {
            Some(events) => vec![listen(events, Message::Extras)],
            None => Vec::new(),
        };
        // the session agent went away: the system menu's session items would do nothing, so it closes
        let agent_gone = self.shared.extras.loses_session(&extras);
        let (list_open, scanning) = (
            self.shared.extras.wifi_list_open,
            self.shared.extras.wifi_scanning,
        );
        self.shared.extras = extras;
        // what the panel keeps itself and what the host bus told outlive a reading; a list that arrived ends the scan
        self.shared.extras.wifi_list_open = list_open;
        self.keep_host_state();
        let listed = self
            .shared
            .extras
            .wifi
            .as_ref()
            .is_some_and(|w| !w.networks.is_empty());
        self.shared.extras.wifi_scanning = scanning && !listed;
        self.follow_join();
        if agent_gone {
            for i in 0..self.bars.len() {
                if let Some(outcome) = self.bars[i].controller.close_slot(&SlotKey::System) {
                    commands.extend(self.apply(i, outcome));
                }
            }
        }
        commands.extend(self.refresh());
        commands
    }

    /// `menubar.conf` was read: the first reading makes the bars; a later one applies its changes, and a new clock
    /// setting redraws the clock now. A file that cannot be read keeps the current values (reported).
    pub(super) fn conf_read(
        &mut self,
        conf: Result<MenuBarSettings, String>,
        watch: Option<ConfigWatcher>,
    ) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        if let Some(watch) = watch {
            let changes = self.conf_changes.0.clone();
            let follow = task::follow_settings(watch, move |event| {
                let change = match event {
                    WatchEvent::Changed(_) => ConfChange::Changed,
                    WatchEvent::Error(e) => ConfChange::Failed(e.to_string()),
                };
                // the model holds the receiver as long as the component lives
                let _ = changes.try_send(change);
            });
            commands.push(Do::Perform(Box::pin(follow)));
            commands.push(listen(&self.conf_changes.1, Message::ConfChanged));
        }
        let conf = match conf {
            Ok(conf) => conf,
            Err(e) if self.conf.is_none() => {
                eprintln!("{PROCESS}: {e}; the bar takes the default settings");
                MenuBarSettings::UNSET
            }
            Err(e) => {
                eprintln!("{PROCESS}: {e}; the clock keeps its settings");
                return commands;
            }
        };
        let first = self.conf.is_none();
        let old = self.conf.replace(conf.clone());
        if old
            .as_ref()
            .is_none_or(|old| old.clock != conf.clock || old.shown.clock != conf.shown.clock)
        {
            self.clock = conf.clock;
            self.next_clock = f64::NEG_INFINITY;
        }
        let background = old
            .as_ref()
            .is_some_and(|old| old.background != conf.background);
        self.shared.conf = conf;
        if background {
            let display = self.tone_display();
            for bar in &mut self.bars {
                bar.retone(display);
            }
        }
        if first {
            commands.extend(self.follow_outputs());
        }
        commands.extend(self.follow_schedule(unix_now()));
        commands.extend(self.refresh());
        commands
    }

    pub(super) fn conf_changed(&mut self, change: ConfChange) -> Vec<Do<Message>> {
        let mut commands = vec![listen(&self.conf_changes.1, Message::ConfChanged)];
        match change {
            ConfChange::Failed(e) => eprintln!("{PROCESS}: watching menubar.conf failed: {e}"),
            ConfChange::Changed => {
                let last_good = self.conf.clone();
                commands.push(task::io(
                    FILE_DEADLINE,
                    move || load_conf(last_good.as_ref()),
                    |conf| Message::Conf(conf, Handed::new(None)),
                ));
            }
        }
        commands
    }

    /// Switch the look when the schedule of `appearance.conf` says it is time: the session follows `compositor.json`.
    fn follow_schedule(&mut self, unix: f64) -> Option<Do<Message>> {
        if self.scheduling {
            return None;
        }
        self.scheduling = true;
        let (dark, written) = (self.dark, self.appearance_written);
        Some(task::io(
            FILE_DEADLINE,
            move || schedule_step(dark, written, unix),
            Message::Scheduled,
        ))
    }

    /// Read the name of `app_id` from its desktop entry, unless that is under way.
    pub(super) fn read_name(&mut self, app_id: &str) -> Option<Do<Message>> {
        if !self.reading_names.insert(app_id.to_owned()) {
            return None;
        }
        let (app_id, data_dirs) = (app_id.to_owned(), self.data_dirs.clone());
        Some(task::io(
            FILE_DEADLINE,
            move || {
                let name = app_name(&app_id, &data_dirs);
                (app_id, name)
            },
            |(app_id, name)| Message::AppName(app_id, name),
        ))
    }

    /// The name of `app_id` was read: the bars that wait for it go on, and one that shows the application takes it.
    pub(super) fn app_name(&mut self, app_id: String, name: AppName) -> Vec<Do<Message>> {
        self.reading_names.remove(&app_id);
        let changed = self.names.get(&app_id) != Some(&name);
        self.names.insert(app_id.clone(), name.clone());
        let mut commands = Vec::new();
        for i in 0..self.bars.len() {
            if changed && self.bars[i].feed.app_id() == app_id {
                self.bars[i].feed.set_app_name(&app_id, name.clone());
            }
            commands.extend(self.apply_held(i));
        }
        if changed {
            commands.extend(self.refresh());
        }
        commands
    }

    /// Ask the X server of the bridge `query`, when there is one: now if it is free, else once it answers; a newer
    /// question of the same bar takes the place of one that waits.
    pub(super) fn ask_x11(&mut self, query: Option<X11Query>) -> Vec<Do<Message>> {
        if let Some(query) = query {
            self.x11.waiting.retain(|q| q.key != query.key);
            self.x11.waiting.push(query);
        }
        let Some((mut server, mut seen)) = self.x11.idle.take() else {
            return Vec::new();
        };
        if self.x11.waiting.is_empty() {
            self.x11.idle = Some((server, seen));
            return Vec::new();
        }
        let query = self.x11.waiting.remove(0);
        vec![task::io(
            X11_DEADLINE,
            move || {
                let matched = query.resolve(&mut server, &mut seen);
                X11Answer {
                    server,
                    seen,
                    query,
                    matched,
                }
            },
            |answer| Message::X11(Handed::new(answer)),
        )]
    }

    /// The X server answered: the bar whose question it still is takes the menu found.
    pub(super) fn x11_answer(&mut self, answer: X11Answer) -> Vec<Do<Message>> {
        let X11Answer {
            server,
            seen,
            query,
            matched,
        } = answer;
        self.x11.idle = Some((server, seen));
        let current = self
            .bars
            .iter()
            .position(|bar| bar.feed.x11_query(&self.shared).as_ref() == Some(&query));
        let mut commands = Vec::new();
        if let Some(i) = current {
            self.bars[i].feed.registered_menu(&mut self.shared, matched);
            commands.extend(self.refresh());
        }
        commands.extend(self.ask_x11(None));
        commands
    }

    /// Load the icons the menus' rows asked for.
    pub(super) fn load_row_icons(&mut self) -> Vec<Do<Message>> {
        self.icons
            .take_wanted()
            .into_iter()
            .map(|request| {
                let loader = self.loader.clone();
                task::io(
                    LOAD_DEADLINE,
                    move || (request.key, loader.row_icon(&request)),
                    |(key, pixels)| Message::RowIcon(key, pixels),
                )
            })
            .collect()
    }

    /// Ask for an activation token for a launch from the bar `window`; it starts when the token arrives.
    pub(super) fn begin_launch(&mut self, window: &WindowKey, launch: Launch) -> Do<Message> {
        let number = self.next_launch;
        self.next_launch += 1;
        self.pending.insert(number, launch);
        Do::ActivationToken {
            window: window.clone(),
            app_id: None,
            on_token: Box::new(move |token| Message::Token {
                launch: number,
                token,
            }),
        }
    }

    /// Start the launch that waited for `token` (it uses the token to take the focus), as a child of the host.
    pub(super) fn complete_launch(&mut self, number: u64, token: Option<String>) {
        let Some(launch) = self.pending.remove(&number) else {
            return;
        };
        let Some(token) = token else {
            eprintln!(
                "{PROCESS}: no activation token for {}",
                launch_name(&launch)
            );
            return;
        };
        match launch.command(&token) {
            Some(spec) if spec.terminal => eprintln!(
                "{PROCESS}: {} runs in a terminal, and the menu bar has none",
                launch_name(&launch)
            ),
            Some(spec) => {
                if !cupertino_kit_shell::host::spawn(spec.to_command()) {
                    eprintln!("{PROCESS}: cannot start {}", launch_name(&launch));
                }
            }
            None => eprintln!("{PROCESS}: {} has no command", launch_name(&launch)),
        }
    }

    /// Find on the I/O pool the default application of the type of `file`, opened from the bar `window`. The
    /// associations are read at each open, so changed defaults apply at once.
    pub(super) fn open_document(&mut self, window: WindowKey, file: PathBuf) -> Do<Message> {
        task::io(
            OPEN_DEADLINE,
            move || {
                let apps = XdgDirs::from_env().and_then(|dirs| {
                    let db =
                        MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
                    MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env())
                        .map_err(|e| e.to_string())
                });
                let launch = apps.map(|apps| launch::open_with_default(&apps, &file));
                (file, launch)
            },
            move |(file, launch)| Message::Opened(window, file, launch),
        )
    }

    /// Open `file` with its default application, or through [`OPEN_COMMAND`] when no installed application handles
    /// it.
    pub(super) fn opened(
        &mut self,
        window: &WindowKey,
        file: &std::path::Path,
        launch: Result<Option<Launch>, String>,
    ) -> Vec<Do<Message>> {
        match launch {
            Ok(Some(launch)) => return vec![self.begin_launch(window, launch)],
            Ok(None) => {
                let mut command = std::process::Command::new(OPEN_COMMAND);
                command.arg(file);
                if !cupertino_kit_shell::host::spawn(command) {
                    eprintln!("{PROCESS}: cannot open {}", file.display());
                }
            }
            Err(e) => eprintln!("{PROCESS}: cannot open {}: {e}", file.display()),
        }
        Vec::new()
    }

    /// Read Recent Items for the system menu that is opening; the bars show it when it is read.
    pub(super) fn read_recent_items(&mut self) -> Option<Do<Message>> {
        let sources = self.services.recent.clone()?;
        let conf = self.conf();
        let (apps, documents) = (conf.recent_apps as usize, conf.recent_documents as usize);
        Some(task::io(
            FILE_DEADLINE,
            move || {
                let mut recent = sources.read(unix_now());
                recent.apps.truncate(apps);
                recent.documents.truncate(documents);
                recent
            },
            Message::Recent,
        ))
    }

    /// Clear Menu: Recent Items lists only what is used from now on.
    pub(super) fn clear_recent_items(&mut self) -> Option<Do<Message>> {
        let sources = self.services.recent.clone()?;
        self.shared.recent = Default::default();
        Some(task::io(
            FILE_DEADLINE,
            move || {
                sources
                    .clear(unix_now())
                    .map_err(|e| format!("cannot write {}: {e}", sources.cleared.display()))
            },
            Message::RecentCleared,
        ))
    }

    /// Update the clock, read the extras' services and follow the appearance's schedule once a minute, run due menu
    /// timers and schedule the timer for the next of them.
    pub(super) fn housekeeping(&mut self) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        let unix = unix_now();
        if unix >= self.next_clock {
            self.next_clock = next_tick(unix, &self.clock);
            self.shared.clock = clock_text(
                &local_time(unix.floor() as i64),
                &self.clock,
                separators_off(unix),
            );
            commands.extend(self.refresh());
        }
        // the services announce their changes; the duration rows change with the hour and announce nothing
        let minute = (unix / 60.0).floor() as i64;
        if self.extras_minute != Some(minute) {
            self.extras_minute = Some(minute);
            if self.conf.is_some() {
                commands.extend(self.follow_schedule(unix));
            }
            if let Some(extras) = &self.services.extras {
                if let Err(e) = extras.refresh() {
                    return fail(e);
                }
            }
        }
        let now = self.now();
        let mut next = Duration::from_secs_f64((self.next_clock - unix).max(0.0));
        if let Some(until) = self.wifi_scan_until {
            if Instant::now() >= until {
                self.wifi_scan_until = None;
                self.shared.extras.wifi_scanning = false;
                commands.extend(self.refresh());
            } else {
                next = next.min(until.saturating_duration_since(Instant::now()));
            }
        }
        if let Some(until) = self.join.as_ref().and_then(|j| j.deadline) {
            if Instant::now() >= until {
                if let Some(join) = self.join.as_mut() {
                    join.phase = PromptPhase::Failed(failure_line(""));
                    join.deadline = None;
                }
            } else {
                next = next.min(until.saturating_duration_since(Instant::now()));
            }
        }
        let per_item = self.conf().item_tone && !crate::tone::solid_bar(self.tone_display());
        for bar in &mut self.bars {
            match &bar.backdrop {
                Some(backdrop @ Wallpaper::Bands(_)) if per_item => {
                    let layout = bar.controller.layout();
                    bar.slot_tones
                        .update(&layout.slots, layout.bar, backdrop, now);
                    if let Some(due) = bar.slot_tones.deadline() {
                        next = next.min(Duration::from_secs_f64((due - now).max(0.0)));
                    }
                }
                _ => bar.slot_tones.clear(),
            }
        }
        for i in 0..self.bars.len() {
            if self.bars[i]
                .controller
                .next_deadline()
                .is_some_and(|d| d <= now)
            {
                let outcome = self.bars[i].controller.tick(now);
                commands.extend(self.apply(i, outcome));
            }
            if let Some(deadline) = self.bars[i].controller.next_deadline() {
                next = next.min(Duration::from_secs_f64((deadline - now).max(0.0)));
            }
        }
        let due = Instant::now() + next;
        if let Some(timer) = self.wakeup.request(due) {
            commands.push(Do::After {
                delay: next,
                message: Message::Timer(timer),
            });
        }
        commands
    }
}
