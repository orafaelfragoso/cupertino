//! The Settings window on CupertinoKit: the full-height sidebar of panes with the search field at its top, beside the
//! page under a floating back and forward toolbar with the page's title; a hub pane's header card and the sections as Kit controls, its sheets as
//! dialogs and its menus on the desktop's menu bar. What the window shows and does lives in [`state`]; this module only
//! shows it.

mod form;
mod sheets;
pub mod state;

use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::Duration;

use cupertino_kit::controls::{
    unobscured_insets, AccountLook, Modal, Navigation, NavigationStack, ScrollView, Scroller,
    SearchField, SidebarWindow, SplitLayout, SplitPane, TileSidebar, TileSidebarItem, ToolbarTitle,
};
use cupertino_kit::core::{CommandRegistry, Menus, Shortcut};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowRequest};
use cupertino_kit::prelude::*;
use cupertino_kit::text::ContentPurpose;
use cupertino_kit::theme::ControlSize;
use cupertino_menu_model::ActionName;

use crate::controls::Value;
use crate::instance;
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::pages::{self, Page, SubPage};
use crate::panes::ControlId;
use crate::search::{self, Hit};
use crate::services::Services;
use crate::settings::Paths;
use crate::PaneId;
use cupertino_assets::Assets;
use state::{background_channel, Background, Result, State};

/// The window's identifier for the server and the name of its desktop entry.
pub const APP_ID: &str = "cupertino-settings";
/// The level slider's waiting write is checked this often while it is dragged.
const LEVEL_TICK: Duration = Duration::from_millis(17);
/// How often a dragged slider without stops checks whether it has settled.
const SLIDE_TICK: Duration = Duration::from_millis(100);
/// The sidebar's width at the start, its least and the most a drag on the divider gives it (the content's least wins
/// over that in a narrow window); the content's least.
const SIDEBAR: f64 = 232.0;
const MIN_SIDEBAR: f64 = 150.0;
const MAX_SIDEBAR: f64 = 400.0;
const MIN_CONTENT: f64 = 320.0;
/// The page's padding inside the content, below the toolbar row.
const PAGE_INSET: Insets = Insets {
    top: 12.0,
    left: 20.0,
    bottom: 20.0,
    right: 20.0,
};
/// The interface width factor: 1 for English (approximate).
const WIDTH_FACTOR: f64 = 1.0;
/// How much wider the sidebar is when the session asks for scroll bars that always show: the list keeps its width
/// beside the track.
const ALWAYS_SHOWN_SIDEBAR_EXTRA: f64 = 17.0;

/// The room the sidebar gains for the scroll bar's track: nothing while bars show only when scrolling.
fn sidebar_extra() -> f64 {
    match cupertino_kit::core::session_scroll_bars() {
        Some(cupertino_kit::platform::ScrollBars::Always) => ALWAYS_SHOWN_SIDEBAR_EXTRA,
        _ => 0.0,
    }
}

/// Window metrics (§4.1).
pub mod metrics {
    /// The default content width before the scale factor, and the height.
    pub const DEFAULT_WIDTH: f64 = 740.0;
    pub const DEFAULT_HEIGHT: f64 = 748.0;
    /// Smallest window height: approximate (the standard panel's minimum content height, by analogy; O6).
    pub const MIN_HEIGHT: f64 = 400.0;
}

/// The default window size at interface width factor `f`: `(ceil(740 f), 748)`.
pub fn default_size(f: f64) -> Size {
    Size::new((metrics::DEFAULT_WIDTH * f).ceil(), metrics::DEFAULT_HEIGHT)
}

/// The template symbols of the window's chrome; with the panes' ([`crate::pane::symbols`]) the process requires them.
pub const CHROME_SYMBOLS: [&str; 8] = [
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.search",
    "symbol.magnifyingglass",
    "symbol.xmark.circle",
    "symbol.chevron.up.down",
    "symbol.checkmark",
    form::NO_PICTURE_ICON,
];

/// Every template symbol the window draws.
pub fn required_symbols() -> Vec<&'static str> {
    let mut all = crate::pane::symbols();
    for symbol in pages::symbols().into_iter().chain(CHROME_SYMBOLS) {
        if !all.contains(&symbol) {
            all.push(symbol);
        }
    }
    all
}

/// The icon of an application whose icon theme has none.
const GENERIC_APPLICATION_ICON: &str = "application.generic";

/// The icons the window draws; the ones no asset pack provides are reported once on stderr.
pub fn required_icons(assets: &Assets) -> cupertino_icons::SystemIcons {
    cupertino_icons::SystemIcons::required(assets, &[GENERIC_APPLICATION_ICON], &required_symbols())
}

const UNSCROLLED: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

fn main_window() -> WindowKey {
    WindowKey("settings".into())
}

/// What [`run`] hands the application before its first frame.
pub(crate) struct Start {
    pub state: State,
    pub background: Receiver<Background>,
    /// Woken each time `background` has something.
    pub woken: async_channel::Receiver<()>,
    /// Raises the window when it opens, from the process that started it.
    pub activation: String,
}

static START: Mutex<Option<Start>> = Mutex::new(None);

/// Open the window on `pane` and serve it until it is closed or Quit is chosen. A second start hands its pane to the
/// running one and ends, before it reaches the display.
pub fn run(pane: PaneId, activation: String) -> Result<()> {
    let (wake, woken) = async_channel::bounded(1);
    let (notify, background) = background_channel(move || {
        let _ = wake.try_send(());
    });
    let to_loop = notify.clone();
    let on_show = move |pane, token| to_loop(Background::ShowPane(pane, token));
    let Some(_instance) = instance::serve_or_hand_over(
        || instance::serve(zbus::connection::Builder::session()?, on_show.clone()),
        || {
            instance::show(
                zbus::connection::Builder::session()?,
                pane.key(),
                &activation,
            )
        },
    )
    .map_err(|e| format!("cannot claim or reach {}: {e}", instance::BUS_NAME))?
    else {
        return Ok(());
    };
    // missing icons never stop the start: they are named here and drawn as placeholders
    required_icons(&Assets::discover());
    let state = State::new(
        Paths::from_env(),
        Services::new(),
        pane,
        display_height()?,
        notify,
    )?;
    launch(Start {
        state,
        background,
        woken,
        activation,
    });
    Ok(cupertino_kit::app::run::<SettingsApp>()?)
}

/// The height of the display, for the Dock's default tile size: the window system reports no outputs, so one short
/// connection asks the server.
fn display_height() -> Result<f64> {
    let mut conn = cupertino_wayland_client::Connection::connect()
        .map_err(|e| format!("cannot connect to the display server: {e}"))?;
    conn.roundtrip()?;
    Ok(conn
        .outputs()
        .into_iter()
        .find_map(|o| o.geometry.map(|g| f64::from(g.height)))
        .ok_or("the server has no output")?)
}

/// Hand `start` to the next [`SettingsApp`] to start, as [`run`] does; for a test's window system.
pub(crate) fn launch(start: Start) {
    *START.lock().expect("no thread panics holding the start") = Some(start);
}

pub struct SettingsApp;

pub struct Shell {
    pub state: State,
    background: Receiver<Background>,
    woken: async_channel::Receiver<()>,
    split: SplitLayout,
    sidebar_scroll: ScrollState,
    content_scroll: ScrollState,
    /// How far each strip of thumbnails is scrolled, by control.
    /// Where the sheet's list is scrolled, and the rows it was scrolled on: another list starts at its top.
    sheet_scroll: (Vec<String>, ScrollState),
    query: String,
    query_revision: u64,
    /// The pop-up button whose menu is open.
    open: Option<ControlId>,
    full_screen: bool,
    /// Whether a [`Message::Refresh`] is on its way: one is, while the pane shown polls ([`State::poll`]).
    refresh_armed: bool,
    /// Whether a [`Message::Rewatch`] is on its way.
    rewatch_armed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Select(usize),
    Navigate(Navigation),
    Split(SplitLayout),
    SidebarScroll(ScrollState),
    ContentScroll(ScrollState),
    /// The sheet's list scrolled.
    SheetScroll(ScrollState),
    /// The strip of thumbnails of this control scrolled.
    Query(TextChange),
    /// Return in the search field: the first result.
    Search,
    Found(usize),
    Commit(ControlId, Value),
    /// A row of the page opens this sub-page.
    OpenPage(SubPage),
    /// The level slider of `ControlId` moved, written while it moves.
    Level(ControlId, f64),
    Open(Option<ControlId>),
    Sheet(sheets::Action),
    Menu(ActionName),
    /// Another thread has something for the window.
    Woken,
    /// Try again to watch a bus whose watch ended ([`State::rewatch_after`]).
    Rewatch,
    Refresh,
    LevelTick,
    /// The slider without stops of `ControlId` moved, committed once it settles.
    Slide(ControlId, f64),
    /// A slider without stops was let go, or moved by a key.
    SlideEnd(ControlId, f64),
    SlideTick,
}

/// Wait for the next wake of the background queue.
fn wait(woken: &async_channel::Receiver<()>) -> Command<Message> {
    let woken = woken.clone();
    Command::Perform(Box::pin(async move {
        // a closed channel has no more wakes: the message then reads the queue one last time
        let _ = woken.recv().await;
        Message::Woken
    }))
}

/// Back and forward from anywhere in the window, the sidebar included; the page's own stack takes them first.
fn shortcuts() -> CommandRegistry<Message> {
    let primary = |keysym| Shortcut {
        mods: Mods::PRIMARY,
        keysym: Keysym(keysym),
    };
    CommandRegistry::new()
        .with(primary(0x5b), Message::Navigate(Navigation::Back))
        .with(primary(0x5d), Message::Navigate(Navigation::Forward))
}

fn after(delay: Duration, message: Message) -> Command<Message> {
    Command::Perform(Box::pin(async move {
        async_io::Timer::after(delay).await;
        message
    }))
}

impl Shell {
    fn hits(&self) -> Vec<Hit> {
        if self.query.trim().is_empty() {
            return Vec::new();
        }
        search::search(&self.query, &self.state.hidden)
    }

    fn menu_state(&self) -> MenuState {
        MenuState {
            search_focused: false,
            has_selection: false,
            sidebar: self.split.collapsed.is_none(),
            full_screen: self.full_screen,
        }
    }

    /// Show `page` from the sidebar, a row or a search; the content goes back to its top.
    fn show(&mut self, page: Page) -> Result<()> {
        self.open = None;
        self.content_scroll.offset = Default::default();
        self.state.show(page)
    }

    /// The sidebar's groups: every pane's row, or while a search is typed only the rows of the panes it finds.
    fn shown_groups(&self) -> Vec<Vec<PaneId>> {
        let hits = self.hits();
        if self.query.trim().is_empty() {
            return self.state.groups.clone();
        }
        self.state
            .groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .copied()
                    .filter(|pane| hits.iter().any(|hit| hit.pane == *pane))
                    .collect::<Vec<_>>()
            })
            .filter(|group| !group.is_empty())
            .collect()
    }

    /// Ends a search: the field clears.
    fn end_search(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.query_revision += 1;
        }
    }

    fn handle(&mut self, message: Message) -> Result<Vec<Command<Message>>> {
        match message {
            Message::Select(i) => {
                let rows: Vec<PaneId> = self.shown_groups().into_iter().flatten().collect();
                if let Some(&pane) = rows.get(i) {
                    // the row of the pane already shown leaves its sub-page or a search; on the pane itself it does
                    // nothing, and the pane keeps its scroll position
                    let shown =
                        self.query.is_empty() && self.state.history.current() == Page::pane(pane);
                    if !shown {
                        self.end_search();
                        self.show(Page::pane(pane))?;
                    }
                }
            }
            Message::OpenPage(sub) => self.show(Page {
                pane: sub.pane(),
                sub: Some(sub),
            })?,
            Message::Navigate(to) => {
                let moved = match to {
                    Navigation::Back => self.state.history.back(),
                    Navigation::Forward => self.state.history.forward(),
                };
                if moved.is_some() {
                    self.open = None;
                    self.content_scroll.offset = Default::default();
                    self.state.follow()?;
                }
            }
            Message::Split(split) => {
                self.split = SplitLayout {
                    position: (split.position - sidebar_extra()).max(0.0),
                    ..split
                }
            }
            Message::SidebarScroll(scroll) => self.sidebar_scroll = scroll,
            Message::ContentScroll(scroll) => self.content_scroll = scroll,
            Message::SheetScroll(scroll) => {
                let rows = self.state.sheet.as_ref().and_then(|s| s.rows());
                self.sheet_scroll = (rows.map(|(r, _)| r).unwrap_or_default(), scroll);
            }
            Message::Query(change) => {
                self.query = change.text;
                self.query_revision = change.revision;
            }
            Message::Search => return self.found(0),
            Message::Found(i) => return self.found(i),
            Message::Commit(id, value) => {
                self.open = None;
                self.state.commit(id, value)?;
                return Ok(self.sheet_started());
            }
            Message::Level(id, value) => {
                let now = self.state.now();
                let started = self.state.level.is_none();
                if let Some(write) = self.state.level_moved(id, value, now) {
                    self.state.level_write(id, write)?;
                }
                if started {
                    return Ok(vec![after(LEVEL_TICK, Message::LevelTick)]);
                }
            }
            Message::LevelTick => {
                let now = self.state.now();
                if let Some((id, write, more)) = self.state.level_tick(now) {
                    if let Some(write) = write {
                        self.state.level_write(id, write)?;
                    }
                    if more {
                        return Ok(vec![after(LEVEL_TICK, Message::LevelTick)]);
                    }
                }
            }
            Message::Slide(id, value) => {
                let now = self.state.now();
                let started = self.state.slide.is_none();
                if let Some((ended, v)) = self.state.slide_moved(id, value, now) {
                    self.state.commit(ended, Value::Fraction(v))?;
                }
                if started {
                    return Ok(vec![after(SLIDE_TICK, Message::SlideTick)]);
                }
            }
            Message::SlideEnd(id, value) => {
                if let Some((id, v)) = self.state.slide_ended(id, value) {
                    self.state.commit(id, Value::Fraction(v))?;
                }
                // another slider's drag that this one ended leaves this one's value for the timer
                if self.state.slide.is_some() {
                    return Ok(vec![after(Duration::ZERO, Message::SlideTick)]);
                }
            }
            Message::SlideTick => {
                let now = self.state.now();
                let (settled, more) = self.state.slide_tick(now);
                if let Some((id, v)) = settled {
                    self.state.commit(id, Value::Fraction(v))?;
                }
                if more {
                    return Ok(vec![after(SLIDE_TICK, Message::SlideTick)]);
                }
            }
            Message::Open(id) => self.open = id,
            Message::Sheet(action) => {
                if let Some(outcome) = sheets::apply(&mut self.state, action) {
                    self.state.sheet_outcome(outcome)?;
                }
                return Ok(self.sheet_started());
            }
            Message::Menu(action) => {
                return Ok(match MenuAction::from_name(&action) {
                    Some(MenuAction::Quit) => vec![Command::Exit],
                    Some(MenuAction::Find) => vec![Command::Focus(Key::from("search"))],
                    Some(MenuAction::ToggleSidebar) => {
                        self.split.collapsed = match self.split.collapsed {
                            Some(_) => None,
                            None => Some(SplitPane::Leading),
                        };
                        Vec::new()
                    }
                    Some(MenuAction::FullScreen) => {
                        self.full_screen = !self.full_screen;
                        vec![Command::Window(
                            main_window(),
                            WindowRequest::Fullscreen(self.full_screen),
                        )]
                    }
                    // the field edits through its own keys
                    Some(_) | None => Vec::new(),
                });
            }
            Message::Woken => {
                // the wait is armed again before anything that can fail: a failure ends no background work
                let mut commands = vec![wait(&self.woken)];
                // a burst of change signals reads the pane again once
                let mut changes = Vec::new();
                while let Ok(event) = self.background.try_recv() {
                    match event {
                        Background::Changed(changed) => {
                            if !changes.contains(&changed) {
                                changes.push(changed);
                            }
                        }
                        event => {
                            if let Err(e) = self.state.background_event(event) {
                                eprintln!("cupertino-settings: {e}");
                            }
                        }
                    }
                }
                if let Err(e) = self.state.changed(&changes) {
                    eprintln!("cupertino-settings: {e}");
                }
                if let Some(token) = self.state.activation.take() {
                    commands.push(Command::Window(
                        main_window(),
                        WindowRequest::Activate(Some(token)),
                    ));
                }
                return Ok(commands);
            }
            Message::Rewatch => {
                self.rewatch_armed = false;
                self.state.rewatch();
            }
            Message::Refresh => {
                self.refresh_armed = false;
                // the pane that asked for it may have given way to one that waits for change signals
                if self.state.poll().is_none() {
                    return Ok(Vec::new());
                }
                let now = self.state.now();
                let listening = self.state.meter_running();
                if listening && now - self.state.refreshed < state::POLL {
                    self.state.refresh_meter();
                } else if self.state.level.is_none() {
                    let apps = |state: &state::State| {
                        crate::panes::notification_apps(&state.settings, &state.live)
                    };
                    let before = apps(&self.state);
                    self.state.refresh(now)?;
                    if self.open.is_some_and(|open| {
                        !crate::panes::menu_survives(open, &before, &apps(&self.state))
                    }) {
                        self.open = None;
                    }
                }
            }
        }
        Ok(Vec::new())
    }

    /// The next [`Message::Refresh`], when the pane shown polls and none is on its way.
    fn arm_refresh(&mut self) -> Option<Command<Message>> {
        let every = self.state.poll().filter(|_| !self.refresh_armed)?;
        self.refresh_armed = true;
        Some(after(Duration::from_secs_f64(every), Message::Refresh))
    }

    /// The next [`Message::Rewatch`], while a bus whose watch ended is not watched again and none is on its way.
    fn arm_rewatch(&mut self) -> Option<Command<Message>> {
        let delay = self.state.rewatch_after().filter(|_| !self.rewatch_armed)?;
        self.rewatch_armed = true;
        Some(after(Duration::from_secs_f64(delay), Message::Rewatch))
    }

    /// Choose search result `i`: its pane shows and the search ends.
    fn found(&mut self, i: usize) -> Result<Vec<Command<Message>>> {
        if let Some(hit) = self.hits().get(i) {
            let page = pages::of_hit(hit);
            self.end_search();
            self.show(page)?;
        }
        Ok(Vec::new())
    }

    /// A sheet a commit opened may need a listing from another thread first.
    fn sheet_started(&mut self) -> Vec<Command<Message>> {
        self.state.drive_picker();
        Vec::new()
    }

    /// The search field over the rows of the panes, each with its tile; the current pane's row selected.
    fn sidebar(&self) -> Element<Message> {
        let has_battery = self.state.live.has_battery();
        let current = self.state.history.current().pane;
        let groups = self.shown_groups();
        let selected = groups.iter().flatten().position(|p| *p == current);
        let groups = groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|pane| {
                        let account = &self.state.account;
                        let mine = *pane == PaneId::Account && !account.full_name.is_empty();
                        TileSidebarItem {
                            key: Key::from(pane.key()),
                            label: if mine {
                                account.full_name.clone()
                            } else {
                                pane.title(has_battery).into()
                            },
                            symbol: pane.pane().symbol.into(),
                            tile: form::tile_color(pane.pane().enclosure),
                            account: mine.then(|| AccountLook {
                                subtitle: PaneId::Account.title(None).into(),
                                initials: account.initials.clone(),
                                picture: self.state.account_picture.clone(),
                            }),
                        }
                    })
                    .collect()
            })
            .collect();
        let search = Element::new(SearchField {
            field: TextField {
                label: "Search".into(),
                placeholder: "Search".into(),
                value: self.query.clone(),
                revision: self.query_revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Query)),
                on_submit: Some(Message::Search),
                selection: None,
                initial: false,
            },
            // the sidebar's search is the large size
            size: Some(ControlSize::Large),
        })
        .key("search");
        // the sidebar window keeps the room for the window control buttons above it
        Element::from(TileSidebar {
            label: "Settings".into(),
            header: Some(search),
            groups,
            selected,
            scroll: self.sidebar_scroll,
            on_scroll: Box::new(Message::SidebarScroll),
            on_select: Some(Box::new(Message::Select)),
        })
        .key("sidebar")
    }

    /// The page's title after the back and forward buttons, sharing the toolbar row.
    fn toolbar(&self) -> Vec<Element<Message>> {
        let page = self.state.history.current();
        vec![Element::new(ToolbarTitle {
            text: page.title(self.state.live.has_battery()).into(),
        })
        .key("title")]
    }

    fn content(&self) -> Element<Message> {
        let body = if self.query.trim().is_empty() {
            let page = self.state.history.current();
            let sections = self.state.sections();
            let has_battery = self.state.live.has_battery();
            let mut blocks = pages::blocks(page, sections.clone());
            let has_card = page.sub.is_none() && pages::has_card(page.pane);
            let has_header = page.sub.is_none() && pages::has_header(page.pane);
            let master = if has_header {
                form::take_master(page.pane, &mut blocks)
            } else {
                None
            };
            let header = if has_card {
                Some(form::pane_card(
                    page.pane,
                    page.pane.title(has_battery),
                    pages::description(page.pane, has_battery),
                ))
            } else {
                has_header.then(|| {
                    form::pane_header(
                        page.pane,
                        page.pane.title(has_battery),
                        pages::description(page.pane, has_battery),
                        master.as_ref(),
                    )
                })
            };
            let icons = self.state.app_icons();
            let view = form::View {
                open: self.open,
                icons: &icons,
                thumbnails: &self.state.thumbnail_images,
            };
            form::page(header, &blocks, &sections, &view)
        } else {
            form::results(&self.hits())
        };
        // the page fills the pane, under the toolbar row, and starts below it
        let row = unobscured_insets(DecorationMode::Sidebar);
        let page = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: PAGE_INSET.top + row.top,
                ..PAGE_INSET
            },
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![body],
        });
        // overlay bars, as everywhere; the session may ask for bars that always show
        let scroller = Scroller::Overlay;
        let current = self.state.history.current();
        let stack = Element::from(NavigationStack {
            content: Element::from(ScrollView {
                content: page,
                state: self.content_scroll,
                horizontal: false,
                vertical: true,
                scroller,
                on_scroll: Box::new(Message::ContentScroll),
            })
            // each page is a view of its own, so its scroll bar shows when the page appears
            .key(format!("content-{}-{:?}", current.pane.key(), current.sub)),
            toolbar: self.toolbar(),
            can_go_back: self.state.history.can_go_back(),
            can_go_forward: self.state.history.can_go_forward(),
            on_navigate: Some(Box::new(Message::Navigate)),
            under_toolbar: true,
        });
        // the pane's background is the window's, which the sidebar window draws
        stack
    }
}

impl Application for SettingsApp {
    type Model = Shell;
    type Message = Message;
    /// A large window whose panes scroll and change whole: its repaints are the GPU's.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Gpu;

    fn init() -> (Shell, Vec<Command<Message>>) {
        let Start {
            state,
            background,
            woken,
            activation,
        } = START
            .lock()
            .expect("no thread panics holding the start")
            .take()
            .expect("the Settings window is launched with its start");
        let mut commands = vec![wait(&woken)];
        if !activation.is_empty() {
            commands.push(Command::Window(
                main_window(),
                WindowRequest::Activate(Some(activation)),
            ));
        }
        let mut shell = Shell {
            state,
            background,
            woken,
            split: SplitLayout {
                position: SIDEBAR,
                collapsed: None,
            },
            sidebar_scroll: UNSCROLLED,
            content_scroll: UNSCROLLED,
            sheet_scroll: (Vec::new(), UNSCROLLED),
            query: String::new(),
            query_revision: 0,
            open: None,
            full_screen: false,
            refresh_armed: false,
            rewatch_armed: false,
        };
        commands.extend(shell.arm_refresh());
        (shell, commands)
    }

    fn update(shell: &mut Shell, message: Message) -> Vec<Command<Message>> {
        let mut commands = shell.handle(message).unwrap_or_else(|e| {
            eprintln!("cupertino-settings: {e}");
            Vec::new()
        });
        // a pane that polls is read again on a timer; the others wait for their change signals
        commands.extend(shell.arm_refresh());
        commands.extend(shell.arm_rewatch());
        commands
    }

    fn view(shell: &Shell) -> Vec<Window<Message>> {
        let state = shell.menu_state();
        let disabled = menu_bar::enabled(&state)
            .into_iter()
            .filter(|(_, on)| !on)
            .map(|(action, _)| action.name())
            .collect();
        let split = Element::from(SidebarWindow {
            label: "Sidebar".into(),
            layout: SplitLayout {
                position: shell.split.position + sidebar_extra(),
                ..shell.split
            },
            min_sidebar: MIN_SIDEBAR,
            max_sidebar: MAX_SIDEBAR + sidebar_extra(),
            min_content: MIN_CONTENT,
            on_change: Some(Box::new(Message::Split)),
            sidebar_header: None,
            sidebar: shell.sidebar(),
            content: shell.content(),
        });
        let size = default_size(WIDTH_FACTOR);
        let mut windows = vec![Window {
            key: main_window(),
            spec: WindowSpec {
                title: shell
                    .state
                    .history
                    .current()
                    .title(shell.state.live.has_battery())
                    .into(),
                app_id: APP_ID.into(),
                size,
                min_size: Some(Size::new(MIN_SIDEBAR + MIN_CONTENT, metrics::MIN_HEIGHT)),
                max_size: None,
                decorations: DecorationMode::Sidebar,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: shortcuts(),
            on_close: None,
            // the sidebar window paints the sidebar's glass and the pane's background itself
            background: WindowBackground::None,
            content: Element::from(Modal {
                content: split,
                dialog: shell.state.sheet.as_ref().map(|sheet| {
                    let scroll = match sheet.rows() {
                        Some((rows, _)) if rows == shell.sheet_scroll.0 => shell.sheet_scroll.1,
                        _ => UNSCROLLED,
                    };
                    sheets::dialog(sheet, scroll)
                }),
            }),
            popup: None,
            menus: Some(Menus {
                bar: menu_bar::menu_bar(&state),
                disabled,
                on_activate: Box::new(Message::Menu),
            }),
        }];
        if let Some(menu) = shell
            .open
            .and_then(|id| form::menu(&shell.state.sections(), id, main_window()))
        {
            windows.extend(menu);
        }
        windows
    }
}
