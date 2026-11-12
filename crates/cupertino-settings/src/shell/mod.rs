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
    SearchField, SidebarWindow, SplitLayout, TileSidebar, TileSidebarItem, ToolbarTitle,
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
/// The window loop reads the live services again this often, as a safety net (the control panel's 1 s, approximate).
const REFRESH: Duration = Duration::from_secs(1);
/// How often the input level meter reads the level while it shows.
const METER_REFRESH: Duration = Duration::from_millis(100);
/// The level slider's waiting write is checked this often while it is dragged.
const LEVEL_TICK: Duration = Duration::from_millis(17);
/// How often a dragged slider without stops checks whether it has settled.
const SLIDE_TICK: Duration = Duration::from_millis(100);
/// The sidebar's width at the start and its least; the content's least.
const SIDEBAR: f64 = 232.0;
const MIN_SIDEBAR: f64 = 150.0;
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
pub const CHROME_SYMBOLS: [&str; 7] = [
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.search",
    "symbol.magnifyingglass",
    "symbol.xmark.circle",
    "symbol.chevron.up.down",
    "symbol.checkmark",
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
                while let Ok(event) = self.background.try_recv() {
                    self.state.background_event(event)?;
                }
                let mut commands = vec![wait(&self.woken)];
                if let Some(token) = self.state.activation.take() {
                    commands.push(Command::Window(
                        main_window(),
                        WindowRequest::Activate(Some(token)),
                    ));
                }
                return Ok(commands);
            }
            Message::Refresh => {
                let now = self.state.now();
                let listening = self.state.meter_running();
                if listening && now - self.state.refreshed < REFRESH.as_secs_f64() {
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
                let next = if listening { METER_REFRESH } else { REFRESH };
                return Ok(vec![after(next, Message::Refresh)]);
            }
        }
        Ok(Vec::new())
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
        let mut commands = vec![
            wait(&woken),
            Command::Perform(Box::pin(async { Message::Refresh })),
        ];
        if !activation.is_empty() {
            commands.push(Command::Window(
                main_window(),
                WindowRequest::Activate(Some(activation)),
            ));
        }
        let shell = Shell {
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
        };
        (shell, commands)
    }

    fn update(shell: &mut Shell, message: Message) -> Vec<Command<Message>> {
        shell.handle(message).unwrap_or_else(|e| {
            eprintln!("cupertino-settings: {e}");
            Vec::new()
        })
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

#[cfg(test)]
mod tests {
    mod general;
    mod layout;
    mod privacy;
    mod repaint;
    mod semantics;

    use std::num::NonZeroU32;
    use std::path::Path;
    use std::sync::Arc;

    use cupertino_assets::BUILTIN_FONT;
    use cupertino_kit::core::{AccessibilityUpdate, NodeId, Role};
    use cupertino_kit::core::{PlaceholderIcons, RuntimeOptions};
    use cupertino_kit::layout::LayoutDirection;
    use cupertino_kit::platform::{
        Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
        WindowId, WindowStates,
    };
    use cupertino_kit::render::{CpuConfig, CpuRenderer};
    use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
    use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
    use cupertino_theme::Theme;
    use cupertino_ui_test::{semantic_gaps, ManualClock, TestDriver};

    use super::*;
    use crate::mapping;
    use crate::services::Bus;
    use crate::settings::Settings;
    use crate::sheet::Sheet;

    const NOWHERE: &str = "unix:path=/nonexistent/cupertino-test-bus";
    const RETURN: u32 = 0xff0d;
    const BRACKET_LEFT: u32 = 0x5b;
    const HOME: u32 = 0xff50;
    const END: u32 = 0xff57;

    /// The back and forward buttons share one glass capsule over the page, as in the file manager: where the desktop
    /// draws materials, the pane's background is the compositor's too, below the capsule's glass, and the page fills
    /// the pane under the row: in the row's layer, the glass lies above the page and reads it.
    #[test]
    fn back_and_forward_share_a_glass_capsule_over_the_compositors_pane_background() {
        use cupertino_kit::platform::WindowRequest;
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // with a window system that draws the row in a layer over the page and with one that draws it in place
        for layered in [false, true] {
            let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
            let capabilities = Capabilities {
                app_effects: true,
                raised_layer: layered,
                ..Capabilities::default()
            };
            let (driver, window) = start_hosted(
                dir.path(),
                PaneId::Appearance,
                services,
                |_| {},
                capabilities,
            );
            let regions = driver
                .runtime()
                .window_system()
                .requests(window)
                .unwrap()
                .iter()
                .rev()
                .find_map(|request| match request {
                    WindowRequest::Materials(regions) => Some(regions.clone()),
                    _ => None,
                })
                .expect("the window asks for materials");
            let form = regions
                .iter()
                .position(|r| r.material == "WindowBackground")
                .unwrap_or_else(|| panic!("the form's background: {regions:?}"));
            let capsules: Vec<_> = regions
                .iter()
                .enumerate()
                .filter(|(_, r)| r.glass.is_some() && r.rect.height() == 36.0)
                .collect();
            assert_eq!(
                capsules.len(),
                1,
                "one capsule for both buttons: {regions:?}"
            );
            let (glass, capsule) = capsules[0];
            assert!(form < glass, "the glass lies over the pane: {regions:?}");
            assert_eq!(
                capsule.raised, layered,
                "above the page in the layer: {regions:?}"
            );
            assert_eq!(capsule.rect.width(), 73.0, "two 36.5 pt segments");
            assert_eq!(
                capsule.rect.center().y,
                26.0,
                "in the middle of the 52 pt row"
            );
            // both buttons lie in it
            let snapshot = driver.runtime().snapshot(window).unwrap();
            for name in ["Back", "Forward"] {
                let button = snapshot
                    .accessibility
                    .iter()
                    .find(|(_, n)| n.role == Role::Button && n.name.as_deref() == Some(name))
                    .unwrap_or_else(|| panic!("the {name} button"))
                    .1
                    .bounds;
                assert!(
                    capsule.rect.intersection(&button) == Some(button),
                    "{name} {button:?} in {:?}",
                    capsule.rect
                );
            }
        }
    }

    /// A pane's title shares the toolbar row after the back and forward buttons; a pane with a header card or a
    /// compact header also opens with it, which reads the pane's description.
    #[test]
    fn a_page_shows_its_title_in_the_toolbar_row_and_its_header_reads_the_description() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        for (pane, title, header) in [
            (PaneId::Appearance, "Appearance", false),
            (PaneId::DesktopDock, "Desktop & Dock", false),
            (PaneId::Accessibility, "Accessibility", true),
            (PaneId::Privacy, "Privacy & Security", true),
            (PaneId::General, "General", true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
            let (driver, window) = start_state(dir.path(), pane, services, |_| {});
            let snapshot = driver.runtime().snapshot(window).unwrap();
            let bounds = |name: &str, role: Option<Role>| {
                snapshot
                    .accessibility
                    .iter()
                    .filter(|(_, n)| {
                        n.name.as_deref() == Some(name) && role.is_none_or(|r| n.role == r)
                    })
                    .map(|(_, n)| n.bounds)
                    .collect::<Vec<_>>()
            };
            let forward = bounds("Forward", Some(Role::Button))[0];
            let titles = bounds(title, Some(Role::StaticText));
            let in_row: Vec<_> = titles
                .iter()
                .filter(|b| {
                    b.min_x() > forward.max_x() && (b.center().y - forward.center().y).abs() < 2.0
                })
                .collect();
            // the card reads the title described by the pane's description, a compact header reads the description
            // as a line of its own; nothing else reads it
            let text = pages::description(pane, None);
            let description: Vec<_> = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| {
                    n.description.as_deref() == Some(text)
                        || (n.role == Role::StaticText && n.name.as_deref() == Some(text))
                })
                .collect();
            assert_eq!(
                in_row.len(),
                1,
                "{pane:?}: the title after the buttons: {titles:?}"
            );
            if header {
                assert_eq!(description.len(), 1, "{pane:?}: its header");
            } else {
                assert!(description.is_empty(), "{pane:?}: no header");
            }
            // the first section's title starts at the page's inset below the row, as a card would
            if pane == PaneId::DesktopDock {
                let dock = bounds("Dock", Some(Role::StaticText))[0];
                let top = cupertino_kit::controls::TOOLBAR_ROW + PAGE_INSET.top;
                assert!((dock.min_y() - top).abs() < 1.0, "{dock:?}, not at {top}");
            }
        }
    }

    #[test]
    fn the_default_size_follows_the_width_factor() {
        let sizes: Vec<(f64, f64)> = [1.0, 1.1, 1.3]
            .iter()
            .map(|f| default_size(*f))
            .map(|s| (s.width, s.height))
            .collect();
        assert_eq!(sizes, [(740.0, 748.0), (815.0, 748.0), (962.0, 748.0)]);
    }

    #[test]
    fn a_missing_symbol_does_not_stop_the_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"version": 2, "name": "empty"}"#,
        )
        .unwrap();
        required_icons(&Assets::from_dirs(&[dir.path().to_path_buf()]));
    }

    fn paths(dir: &Path) -> Paths {
        Paths {
            dock: Some(dir.join("dock.conf")),
            compositor: Some(dir.join("compositor.json")),
            menubar: Some(dir.join("menubar.conf")),
            keyboard: Some(dir.join("keyboard.conf")),
            power: Some(dir.join("power.conf")),
            accessibility: Some(dir.join("accessibility.conf")),
            system_accessibility: None,
            data_control: Some(dir.join("data-control.conf")),
            screen_sharing: None,
            notifications: Some(dir.join("notifications.conf")),
            desktop_control: Some(dir.join("desktop-control.conf")),
            // shown as installed; Settings never starts it
            screen_reader: Some(std::path::PathBuf::from("/usr/bin/orca")),
            ..Paths::default()
        }
    }

    fn budget() -> NonZeroU32 {
        NonZeroU32::new(64).unwrap()
    }

    /// What a screen reader cannot announce in the window, besides the root's name: Kit's tree leaves the root unnamed
    /// and the AT-SPI bridge names it by the window's title.
    fn kit_gaps(tree: &AccessibilityUpdate) -> Vec<String> {
        const KIT_GAPS: [&str; 1] = ["Window #0: no name"];
        let mut gaps = semantic_gaps(tree);
        gaps.retain(|gap| !KIT_GAPS.contains(&gap.as_str()));
        gaps
    }

    /// The runtime a test window runs on: the workspace's light theme, the built-in font and a manual clock.
    fn runtime_options() -> RuntimeOptions {
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        };
        let theme = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 8000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        }
    }

    /// The start is the process's, so the tests that open a window take turns.
    static WINDOW: Mutex<()> = Mutex::new(());

    fn start(dir: &Path, pane: PaneId) -> (TestDriver<SettingsApp>, WindowId) {
        start_with(dir, pane, None)
    }

    fn start_with(
        dir: &Path,
        pane: PaneId,
        sheet: Option<Sheet>,
    ) -> (TestDriver<SettingsApp>, WindowId) {
        let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
        start_on(dir, pane, sheet, services)
    }

    /// [`start_with`] whose window reaches `services`.
    fn start_on(
        dir: &Path,
        pane: PaneId,
        sheet: Option<Sheet>,
        services: Services,
    ) -> (TestDriver<SettingsApp>, WindowId) {
        start_state(dir, pane, services, |state| state.sheet = sheet)
    }

    /// [`start_on`] with the state changed by `prepare` before the window opens.
    fn start_state(
        dir: &Path,
        pane: PaneId,
        services: Services,
        prepare: impl FnOnce(&mut State),
    ) -> (TestDriver<SettingsApp>, WindowId) {
        start_hosted(dir, pane, services, prepare, Capabilities::default())
    }

    /// [`start_state`] on a host with `capabilities`.
    fn start_hosted(
        dir: &Path,
        pane: PaneId,
        services: Services,
        prepare: impl FnOnce(&mut State),
        capabilities: Capabilities,
    ) -> (TestDriver<SettingsApp>, WindowId) {
        let (wake, woken) = async_channel::bounded(1);
        let (notify, background) = background_channel(move || {
            let _ = wake.try_send(());
        });
        let mut state = State::new(paths(dir), services, pane, 1080.0, notify).unwrap();
        prepare(&mut state);
        launch(Start {
            state,
            background,
            woken,
            activation: String::new(),
        });
        let options = runtime_options();
        let mut driver = TestDriver::new(capabilities, options, ManualClock::new(Timestamp(0)));
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        settle(&mut driver, window);
        (driver, window)
    }

    /// Runs until the window rests, then checks that a screen reader could read every window open.
    fn settle<A: Application>(driver: &mut TestDriver<A>, window: WindowId) {
        for _ in 0..8 {
            driver.run_until_idle(budget()).unwrap();
            let system = driver.window_system_mut();
            if !system.windows().contains(&window) || !system.frame_requested(window).unwrap() {
                break;
            }
            system.frame_ready(window, Timestamp(0)).unwrap();
        }
        driver.assert_accessible("Settings");
    }

    /// Makes `window` tall enough that a long pane's lower rows are on screen to be clicked.
    fn tall(driver: &mut TestDriver<SettingsApp>, window: WindowId) {
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(
                window,
                Some(Size::new(metrics::DEFAULT_WIDTH, 1600.0)),
                active,
            )
            .unwrap();
        settle(driver, window);
    }

    fn send(driver: &mut TestDriver<SettingsApp>, window: WindowId, event: WindowEvent) {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        settle(driver, window);
    }

    fn click(driver: &mut TestDriver<SettingsApp>, window: WindowId, key: &str) {
        let id = driver
            .runtime()
            .find(window, &Key::from(key.to_owned()))
            .unwrap();
        click_node(driver, window, id);
    }

    /// The accessibility node with `role` named `name`.
    fn node(driver: &TestDriver<SettingsApp>, window: WindowId, role: Role, name: &str) -> NodeId {
        driver
            .runtime()
            .snapshot(window)
            .unwrap()
            .accessibility
            .into_iter()
            .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
            .unwrap()
            .0
    }

    fn click_node(driver: &mut TestDriver<SettingsApp>, window: WindowId, id: NodeId) {
        let rect = driver
            .runtime()
            .snapshot(window)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect;
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        send(driver, window, pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            send(
                driver,
                window,
                pointer(PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: Mods::NONE,
                }),
            );
        }
    }

    fn key(
        driver: &mut TestDriver<SettingsApp>,
        window: WindowId,
        keysym: u32,
        text: Option<char>,
        mods: Mods,
    ) {
        for pressed in [true, false] {
            let event = KeyEvent {
                code: if keysym == RETURN { 28 } else { 0 },
                keysym: Keysym(keysym),
                text: text.filter(|_| pressed),
                pressed,
                repeat: false,
                mods,
            };
            send(driver, window, WindowEvent::Key(event, Timestamp(0)));
        }
    }

    fn title(driver: &mut TestDriver<SettingsApp>, window: WindowId) -> String {
        let system = driver.window_system_mut();
        let retitled = system
            .requests(window)
            .unwrap()
            .iter()
            .rev()
            .find_map(|request| match request {
                WindowRequest::SetTitle(title) => Some(title.clone()),
                _ => None,
            });
        retitled.unwrap_or_else(|| system.spec(window).unwrap().title.clone())
    }

    #[test]
    fn the_shell_navigates_searches_answers_its_menus_and_commits_a_control() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        assert_eq!(title(&mut driver, window), "Desktop & Dock");
        assert!(driver.window_system_mut().menus(window).unwrap().is_some());
        // the Keyboard row is low in the sidebar, past a window of the default height
        tall(&mut driver, window);

        click(&mut driver, window, "keyboard");
        assert_eq!(title(&mut driver, window), "Keyboard");
        key(&mut driver, window, BRACKET_LEFT, None, Mods::PRIMARY);
        assert_eq!(title(&mut driver, window), "Desktop & Dock");

        send(
            &mut driver,
            window,
            WindowEvent::MenuActivated(MenuAction::Find.name()),
        );
        let runtime = driver.runtime();
        assert_eq!(
            runtime.snapshot(window).unwrap().focus,
            Some(runtime.find(window, &Key::from("search")).unwrap()),
            "Find puts the keyboard in the search field"
        );
        for c in "keyboard".chars() {
            key(&mut driver, window, c as u32, Some(c), Mods::NONE);
        }
        key(&mut driver, window, RETURN, None, Mods::NONE);
        assert_eq!(title(&mut driver, window), "Keyboard");

        key(&mut driver, window, BRACKET_LEFT, None, Mods::PRIMARY);
        click(
            &mut driver,
            window,
            &format!("{:?}-control", ControlId::AutoHide),
        );
        let stored = Settings::load(&paths(dir.path()), 1080.0).unwrap();
        assert_eq!(stored.dock.autohide, Some(true));
    }

    #[test]
    fn the_rows_switches_are_small_and_their_rows_keep_their_height() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (driver, window) = start(dir.path(), PaneId::DesktopDock);
        let snapshot = driver.runtime().snapshot(window).unwrap();
        let mut switches: Vec<_> = snapshot
            .accessibility
            .iter()
            .filter(|(_, n)| n.role == Role::Switch)
            .filter_map(|(id, _)| Some(snapshot.nodes.iter().find(|m| m.id == *id)?.rect))
            .collect();
        assert!(switches.len() >= 2, "{switches:?}");
        for rect in &switches {
            assert_eq!(rect.size, Size::new(36.0, 16.0), "{rect:?}");
        }
        switches.sort_by(|a, b| a.origin.y.total_cmp(&b.origin.y));
        let step = switches
            .windows(2)
            .map(|pair| pair[1].origin.y - pair[0].origin.y)
            .fold(f64::INFINITY, f64::min);
        assert!(
            (step - (cupertino_kit::controls::FORM_ROW_HEIGHT + 1.0)).abs() < 0.01,
            "switches in neighbouring rows sit a row and its hairline apart: {step}"
        );
    }

    #[test]
    fn the_selected_panes_row_goes_back_from_a_sub_page_to_the_pane() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::Accessibility);
        click(&mut driver, window, &format!("open-{:?}", SubPage::Display));
        assert_eq!(title(&mut driver, window), SubPage::Display.title());
        click(&mut driver, window, "accessibility");
        assert_eq!(title(&mut driver, window), "Accessibility");
        let shell = driver.runtime().model();
        assert_eq!(
            shell.state.history.current(),
            Page::pane(PaneId::Accessibility)
        );
        assert!(
            shell.state.history.can_go_back(),
            "the sub-page stays in the history"
        );

        // on the pane itself the row changes nothing
        let entries = |driver: &TestDriver<SettingsApp>| {
            let history = &driver.runtime().model().state.history;
            (
                history.current(),
                history.can_go_back(),
                history.can_go_forward(),
            )
        };
        let before = entries(&driver);
        click(&mut driver, window, "accessibility");
        assert_eq!(entries(&driver), before);
    }

    #[test]
    fn a_stepped_slider_commits_its_stop() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::Keyboard);
        click(
            &mut driver,
            window,
            &format!("{:?}-control", ControlId::RepeatRate),
        );
        key(&mut driver, window, END, None, Mods::NONE);
        let stored = Settings::load(&paths(dir.path()), 1080.0).unwrap();
        assert_eq!(
            stored.keyboard.repeat_interval_ms,
            Some(mapping::rate_interval_ms(mapping::RATE_POSITIONS - 1))
        );
        key(&mut driver, window, HOME, None, Mods::NONE);
        let stored = Settings::load(&paths(dir.path()), 1080.0).unwrap();
        assert_eq!(
            stored.keyboard.repeat_delay_ms,
            Some(0),
            "the first stop is Off"
        );
    }

    #[test]
    fn a_slider_without_stops_moved_by_a_key_writes_at_once() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        click(
            &mut driver,
            window,
            &format!("{:?}-control", ControlId::DockSize),
        );
        key(&mut driver, window, END, None, Mods::NONE);
        let stored = || {
            Settings::load(&paths(dir.path()), 1080.0)
                .unwrap()
                .dock
                .tile_size
        };
        assert_eq!(
            stored(),
            Some(mapping::dock_px(1.0)),
            "a key ends the change: written without waiting for the knob to settle"
        );
        settle(&mut driver, window);
        assert_eq!(stored(), Some(mapping::dock_px(1.0)), "and once");
    }

    /// The dock size slider's value as the window shows it.
    fn dock_size_shown(driver: &TestDriver<SettingsApp>, window: WindowId) -> f64 {
        let key = Key::from(format!("{:?}-control", ControlId::DockSize));
        let id = driver.runtime().find(window, &key).unwrap();
        let snapshot = driver.runtime().snapshot(window).unwrap();
        match snapshot.accessibility.into_iter().find(|(n, _)| *n == id) {
            Some((_, node)) => match node.value {
                Some(cupertino_kit::core::AccessibleValue::Number { value, .. }) => value,
                other => panic!("a slider's value, not {other:?}"),
            },
            None => panic!("the slider is read"),
        }
    }

    /// A drag forward never shows the knob back where it was: not while it moves, not when it is let go in the same
    /// turn as its last move (before the next view carries that move), and not once its value is stored.
    #[test]
    fn a_dragged_slider_never_moves_back() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        let key = Key::from(format!("{:?}-control", ControlId::DockSize));
        let id = driver.runtime().find(window, &key).unwrap();
        let rect = driver
            .runtime()
            .snapshot(window)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect;
        let at = |share: f64| Point::new(rect.min_x() + rect.width() * share, rect.center().y);
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        let button = |share: f64, pressed: bool| {
            pointer(PointerEvent::Button {
                position: at(share),
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            })
        };
        let motion = |share: f64| {
            pointer(PointerEvent::Motion {
                position: at(share),
            })
        };
        // the stored size is whole points: the value read back may sit up to half a point under the one let go
        let store_step = 0.5 / mapping::DOCK_SPAN_PX;
        send(&mut driver, window, motion(0.2));
        send(&mut driver, window, button(0.2, true));
        let mut last = dock_size_shown(&driver, window);
        for share in [0.35, 0.5, 0.65] {
            send(&mut driver, window, motion(share));
            let shown = dock_size_shown(&driver, window);
            assert!(
                shown > last,
                "dragged forward to {share}: {shown} after {last}"
            );
            last = shown;
        }
        // the last move and the release in one turn, as a quick flick delivers them
        driver.window_system_mut().set_dispatch_one(false);
        for event in [motion(0.9), button(0.9, false)] {
            driver
                .window_system_mut()
                .enqueue_window(window, event)
                .unwrap();
        }
        settle(&mut driver, window);
        driver.window_system_mut().set_dispatch_one(true);
        let shown = dock_size_shown(&driver, window);
        assert!(
            shown > last + 0.1,
            "let go further on, the knob stays there: {shown} after {last}"
        );
        let stored = Settings::load(&paths(dir.path()), 1080.0)
            .unwrap()
            .dock
            .tile_size
            .unwrap();
        assert!(
            (mapping::dock_value(stored) - shown).abs() <= store_step,
            "the value let go at is the one stored"
        );
        // later turns (a timer, the files read again) keep it there
        settle(&mut driver, window);
        assert!(dock_size_shown(&driver, window) >= shown - store_step);
    }

    #[test]
    fn the_join_button_joins_with_the_typed_passphrase() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let sheet = Sheet::passphrase("home", "Home");
        let asking = sheet.message();
        let (mut driver, window) = start_with(dir.path(), PaneId::Wifi, Some(sheet));
        let described = |driver: &TestDriver<SettingsApp>| {
            format!(
                "{:?}",
                driver.runtime().snapshot(window).unwrap().accessibility
            )
        };
        assert!(described(&driver).contains(&asking));

        let field = node(&driver, window, Role::PasswordInput, "Password");
        click_node(&mut driver, window, field);
        for c in "hunter2".chars() {
            key(&mut driver, window, c as u32, Some(c), Mods::NONE);
        }
        let join = node(&driver, window, Role::Button, "Join");
        click_node(&mut driver, window, join);
        assert!(
            !described(&driver).contains(&asking),
            "Join called the service, which has no bus here, and the sheet shows why it failed"
        );
    }

    /// The server's session object as the Accessibility pane uses it: the keys it holds, and the calls that set them.
    struct FakeSession {
        held: std::collections::HashMap<String, String>,
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[zbus::interface(name = "org.cupertino.Session1")]
    impl FakeSession {
        #[zbus(property)]
        fn accessibility(&self) -> std::collections::HashMap<String, String> {
            self.held.clone()
        }

        fn set_accessibility(&mut self, key: String, value: String) {
            self.calls
                .lock()
                .unwrap()
                .push((key.clone(), value.clone()));
            self.held.insert(key, value);
        }
    }

    /// The state of the switch named `name`.
    fn switch_state(driver: &TestDriver<SettingsApp>, window: WindowId, name: &str) -> String {
        let snapshot = driver.runtime().snapshot(window).unwrap();
        snapshot
            .accessibility
            .iter()
            .find(|(_, n)| n.role == Role::Switch && n.name.as_deref() == Some(name))
            .map(|(_, n)| cupertino_ui_test::describe(n))
            .unwrap_or_else(|| panic!("a switch named {name:?}"))
    }

    /// The names of the nodes with `role`.
    fn named(driver: &TestDriver<SettingsApp>, window: WindowId, role: Role) -> Vec<String> {
        driver
            .runtime()
            .snapshot(window)
            .unwrap()
            .accessibility
            .into_iter()
            .filter(|(_, n)| n.role == role)
            .filter_map(|(_, n)| n.name)
            .collect()
    }

    #[test]
    fn a_search_keeps_the_sidebar_rows_of_the_panes_it_finds_and_one_of_them_opens_its_pane() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        // the sidebar's rows top to bottom
        let rows = |driver: &TestDriver<SettingsApp>| {
            let snapshot = driver.runtime().snapshot(window).unwrap();
            let mut rows: Vec<(f64, String)> = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| n.role == Role::ListItem)
                .filter_map(|(id, n)| {
                    let rect = snapshot.nodes.iter().find(|m| m.id == *id)?.rect;
                    Some((rect.origin.y, n.name.clone()?))
                })
                .collect();
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            rows.into_iter().map(|(_, name)| name).collect::<Vec<_>>()
        };
        let all = rows(&driver);
        click(&mut driver, window, "search");
        for c in "keyboard".chars() {
            key(&mut driver, window, c as u32, Some(c), Mods::NONE);
        }
        let shown = rows(&driver);
        let kept: Vec<String> = all.iter().filter(|r| shown.contains(r)).cloned().collect();
        assert_eq!(shown, kept, "the rows found, in the sidebar's order");
        assert!(
            shown.len() >= 2 && shown.contains(&"Keyboard".to_owned()),
            "{shown:?}"
        );
        assert!(!shown.contains(&"Desktop & Dock".to_owned()), "{shown:?}");
        // a row counts among the rows shown: the last of them is not the pane at its place among all the rows
        let last = shown.last().unwrap().clone();
        assert_ne!(all.iter().position(|r| *r == last), Some(shown.len() - 1));
        let row = node(&driver, window, Role::ListItem, &last);
        click_node(&mut driver, window, row);
        assert_eq!(title(&mut driver, window), last);
        assert_eq!(rows(&driver), all, "choosing a row ends the search");
    }

    #[test]
    fn a_navigation_row_opens_its_page_and_back_and_forward_go_through_the_history() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        // the row that opens Hot Corners is below a window of the default height
        tall(&mut driver, window);
        let corner = "Top Left".to_owned();
        let on_pane = |driver: &TestDriver<SettingsApp>| {
            let buttons = named(driver, window, Role::Button);
            assert!(buttons.contains(&"Hot Corners".to_owned()), "{buttons:?}");
            assert!(!named(driver, window, Role::PopUpButton).contains(&corner));
            assert!(named(driver, window, Role::StaticText).contains(&"Desktop & Dock".to_owned()));
        };
        let on_corners = |driver: &TestDriver<SettingsApp>| {
            assert!(named(driver, window, Role::PopUpButton).contains(&corner));
            assert!(named(driver, window, Role::StaticText).contains(&"Hot Corners".to_owned()));
            assert!(
                !named(driver, window, Role::Button).contains(&"Hot Corners".to_owned()),
                "the page no longer shows the row that opened it"
            );
        };
        on_pane(&driver);
        click(&mut driver, window, "open-HotCorners");
        on_corners(&driver);
        let back = node(&driver, window, Role::Button, "Back");
        click_node(&mut driver, window, back);
        on_pane(&driver);
        let forward = node(&driver, window, Role::Button, "Forward");
        click_node(&mut driver, window, forward);
        on_corners(&driver);
    }

    #[test]
    fn the_screen_reader_switch_asks_the_server_to_start_and_stop_the_reader() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return;
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let held = [("screen_reader".to_string(), "off".to_string())].into();
        let _session = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .serve_at(
                "/org/cupertino/Session1",
                FakeSession {
                    held,
                    calls: Arc::clone(&calls),
                },
            )
            .unwrap()
            .name("org.cupertino.Session1")
            .unwrap()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let services = Services::on(Bus::address(NOWHERE), Bus::at(bus.connect()));
        // the window's own session bus is the private one: no test reaches the real session
        let session = bus.connect();
        let (mut driver, window) =
            start_state(dir.path(), PaneId::Accessibility, services, |state| {
                state.bus = Some(session)
            });
        // the switch is on the pane's Screen Reader page
        click(&mut driver, window, "open-ScreenReader");
        assert_eq!(
            switch_state(&driver, window, "Screen reader"),
            "Switch \"Screen reader\" off focusable"
        );
        let control = format!("{:?}-control", ControlId::ScreenReader);
        click(&mut driver, window, &control);
        assert_eq!(
            switch_state(&driver, window, "Screen reader"),
            "Switch \"Screen reader\" on focusable",
            "the switch shows what the server now holds"
        );
        driver
            .look(window, "settings-accessibility-screen-reader-on")
            .unwrap();
        click(&mut driver, window, &control);
        assert_eq!(
            *calls.lock().unwrap(),
            [
                ("screen_reader".to_string(), "on".to_string()),
                ("screen_reader".to_string(), "off".to_string())
            ]
        );
        assert_eq!(
            switch_state(&driver, window, "Screen reader"),
            "Switch \"Screen reader\" off focusable"
        );
    }

    /// With the session asking for bars that always show, a pane longer than the window shows its scroll bar and
    /// one that fits does not. (Bars that show only while scrolling are not read as scroll bars.)
    #[test]
    fn a_pane_longer_than_the_window_shows_its_scroll_bar_and_one_that_fits_does_not() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::Keyboard);
        driver
            .window_system_mut()
            .enqueue(cupertino_kit::platform::PlatformEvent::ScrollBars(
                cupertino_kit::platform::ScrollBars::Always,
            ))
            .unwrap();
        settle(&mut driver, window);
        let bars = |driver: &TestDriver<SettingsApp>| {
            driver
                .runtime()
                .snapshot(window)
                .unwrap()
                .accessibility
                .into_iter()
                .filter(|(_, n)| n.role == Role::ScrollBar)
                .count()
        };
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(
                window,
                Some(Size::new(metrics::DEFAULT_WIDTH, 400.0)),
                active,
            )
            .unwrap();
        settle(&mut driver, window);
        settle(&mut driver, window);
        assert_eq!(
            bars(&driver),
            2,
            "the sidebar and the Keyboard pane go on below a short window"
        );
        driver
            .window_system_mut()
            .configure(
                window,
                Some(Size::new(metrics::DEFAULT_WIDTH, 3000.0)),
                active,
            )
            .unwrap();
        settle(&mut driver, window);
        settle(&mut driver, window);
        assert_eq!(bars(&driver), 0, "in a window tall enough it fits");
    }

    /// A sheet's long list scrolls inside a sheet that fits the window at its least height, its buttons in reach; a
    /// short list keeps its own height and does not scroll.
    #[test]
    fn a_sheets_long_list_scrolls_inside_the_window() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let apps = |n: usize| -> Vec<crate::clipboard::App> {
            (0..n)
                .map(|i| crate::clipboard::App {
                    name: format!("App {i:02}"),
                    icon: None,
                    program: Some(format!("/usr/bin/app{i:02}").into()),
                    reason: None,
                })
                .collect()
        };
        let bounds = |driver: &TestDriver<SettingsApp>, role: Role| {
            driver
                .runtime()
                .snapshot(driver.runtime().windows()[0].1)
                .unwrap()
                .accessibility
                .into_iter()
                .filter(|(_, n)| n.role == role)
                .map(|(_, n)| n.bounds)
                .collect::<Vec<_>>()
        };
        let (mut driver, window) =
            start_with(dir.path(), PaneId::Wifi, Some(Sheet::apps(apps(40))));
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(
                window,
                Some(Size::new(metrics::DEFAULT_WIDTH, metrics::MIN_HEIGHT)),
                active,
            )
            .unwrap();
        settle(&mut driver, window);
        settle(&mut driver, window);
        let sheet = bounds(&driver, Role::Dialog)[0];
        assert!(
            sheet.max_y() <= metrics::MIN_HEIGHT,
            "the sheet fits the window: {sheet:?}"
        );
        assert!(
            bounds(&driver, Role::ScrollView)
                .iter()
                .any(|list| sheet.contains(list.center()) && list.height() < 40.0 * 20.0),
            "its list scrolls"
        );

        let (driver, _) = start_with(dir.path(), PaneId::Wifi, Some(Sheet::apps(apps(3))));
        let sheet = bounds(&driver, Role::Dialog)[0];
        assert!(
            !bounds(&driver, Role::ScrollView)
                .iter()
                .any(|list| sheet.contains(list.center())),
            "a short list does not scroll"
        );
    }

    #[test]
    fn every_pane_leaves_nothing_a_screen_reader_cannot_announce() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        for pane in crate::sidebar::rows(&[]) {
            let dir = tempfile::tempdir().unwrap();
            let (mut driver, window) = start(dir.path(), pane);
            let name = format!("settings-{pane:?}").to_lowercase();
            driver.look(window, &name).unwrap();
            let snapshot = driver.runtime().snapshot(window).unwrap();
            let tree = AccessibilityUpdate {
                window,
                root: snapshot.accessibility[0].0,
                focus: snapshot.focus,
                nodes: snapshot.accessibility,
                removed: Vec::new(),
                full: true,
            };
            assert_eq!(kit_gaps(&tree), Vec::<String>::new(), "{pane:?}");
        }
    }

    #[test]
    fn the_toolbar_and_the_top_of_the_sidebar_move_the_window_on_a_drag_and_zoom_it_on_a_double_click(
    ) {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, window) = start(dir.path(), PaneId::DesktopDock);
        let rect = |driver: &TestDriver<SettingsApp>, key: &str| {
            let id = driver
                .runtime()
                .find(window, &Key::from(key.to_owned()))
                .unwrap();
            driver
                .runtime()
                .snapshot(window)
                .unwrap()
                .nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .rect
        };
        let field = rect(&driver, "search");
        let y = field.origin.y + field.size.height / 2.0;
        // in the toolbar row over the top of the page, after the back and forward buttons, where no toolbar item lies
        // each page's scroll view is keyed by its page
        let page = Page::pane(PaneId::DesktopDock);
        let content = rect(
            &driver,
            &format!("content-{}-{:?}", page.pane.key(), page.sub),
        );
        let gap = Point::new(
            content.origin.x + content.size.width - 60.0,
            content.origin.y + 26.0,
        );
        let at = |time: u64, event| WindowEvent::Pointer(event, Timestamp(time));
        let button = |position, pressed| PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        };
        let requests = |driver: &mut TestDriver<SettingsApp>| {
            driver
                .window_system_mut()
                .requests(window)
                .unwrap()
                .to_vec()
        };
        // a drag on the search field selects its text
        let inside = Point::new(field.origin.x + field.size.width / 2.0, y);
        for (time, event) in [
            (0, button(inside, true)),
            (
                1,
                PointerEvent::Motion {
                    position: Point::new(inside.x, y + 40.0),
                },
            ),
            (2, button(Point::new(inside.x, y + 40.0), false)),
        ] {
            send(&mut driver, window, at(time, event));
        }
        assert!(!requests(&mut driver).contains(&WindowRequest::Move));
        let to = Point::new(gap.x - 30.0, gap.y + 30.0);
        for (time, event) in [
            (10_000_000_000, PointerEvent::Motion { position: gap }),
            (10_000_000_000, button(gap, true)),
            (10_000_000_001, PointerEvent::Motion { position: to }),
            (10_000_000_002, button(to, false)),
            (10_000_000_003, PointerEvent::Leave),
        ] {
            send(&mut driver, window, at(time, event));
        }
        assert!(requests(&mut driver).contains(&WindowRequest::Move));
        for time in [20_000_000_000, 20_200_000_000] {
            send(&mut driver, window, at(time, button(gap, true)));
            send(&mut driver, window, at(time + 1, button(gap, false)));
        }
        assert!(requests(&mut driver).contains(&WindowRequest::Maximize(true)));
        // the top of the sidebar, beside the window control buttons, moves the window too
        let top = Point::new(field.max_x() - 20.0, 26.0);
        let moves = |driver: &mut TestDriver<SettingsApp>| {
            requests(driver)
                .iter()
                .filter(|r| **r == WindowRequest::Move)
                .count()
        };
        let before = moves(&mut driver);
        for (time, event) in [
            (30_000_000_000, PointerEvent::Motion { position: top }),
            (30_000_000_000, button(top, true)),
            (
                30_000_000_001,
                PointerEvent::Motion {
                    position: Point::new(top.x, top.y + 30.0),
                },
            ),
            (
                30_000_000_002,
                button(Point::new(top.x, top.y + 30.0), false),
            ),
            (30_000_000_003, PointerEvent::Leave),
        ] {
            send(&mut driver, window, at(time, event));
        }
        assert_eq!(moves(&mut driver), before + 1);
    }
}
