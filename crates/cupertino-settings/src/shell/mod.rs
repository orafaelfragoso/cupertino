//! The Settings window on CupertinoKit: the sidebar beside the pane under a back and forward toolbar with the search
//! field, the pane's sections as Kit controls, its sheets as dialogs and its menus on the desktop's menu bar. What the
//! window shows and does lives in [`state`]; this module only shows it.

mod form;
mod sheets;
pub mod state;

use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::Duration;

use cupertino_kit::controls::{
    Modal, Navigation, NavigationStack, ScrollView, Scroller, Sidebar, SidebarItem, SidebarSection,
    SplitLayout, SplitView, TextRole, WINDOW_CONTENT_INSET,
};
use cupertino_kit::core::{CommandRegistry, Menus, Shortcut};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowRequest};
use cupertino_kit::prelude::*;
use cupertino_kit::text::ContentPurpose;
use cupertino_menu_model::ActionName;

use crate::controls::Value;
use crate::instance;
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::panes::ControlId;
use crate::search::{self, Hit};
use crate::services::Services;
use crate::settings::Paths;
use crate::window::metrics;
use crate::PaneId;
use cupertino_assets::Assets;
use state::{background_channel, Background, Result, State};

/// The window's identifier for the server and the name of its desktop entry.
pub const APP_ID: &str = "cupertino-settings";
/// The window loop reads the live services again this often, as a safety net (the control panel's 1 s, approximate).
const REFRESH: Duration = Duration::from_secs(1);
/// The level slider's waiting write is checked this often while it is dragged.
const LEVEL_TICK: Duration = Duration::from_millis(17);
/// The sidebar's width at the start and its least; the content's least.
const SIDEBAR: f64 = 215.0;
const MIN_SIDEBAR: f64 = 150.0;
const MIN_CONTENT: f64 = 320.0;
/// The toolbar's search field, as wide as other toolbars' (approximate).
const SEARCH_WIDTH: f64 = 112.0;
/// The interface width factor: 1 for English (approximate).
const WIDTH_FACTOR: f64 = 1.0;

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
    crate::paint::required_icons(&Assets::discover());
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
    Query(TextChange),
    /// Return in the search field: the first result.
    Search,
    Found(usize),
    Commit(ControlId, Value),
    /// The level slider of `ControlId` moved, written while it moves.
    Level(ControlId, f64),
    Open(Option<ControlId>),
    Sheet(sheets::Action),
    Menu(ActionName),
    /// Another thread has something for the window.
    Woken,
    Refresh,
    LevelTick,
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

    /// Show `pane` from the sidebar or a search; the content goes back to its top.
    fn go(&mut self, pane: PaneId) -> Result<()> {
        self.open = None;
        self.content_scroll.offset = Default::default();
        self.state.go(pane)
    }

    fn handle(&mut self, message: Message) -> Result<Vec<Command<Message>>> {
        match message {
            Message::Select(i) => {
                if let Some(&pane) = self.state.rows().get(i) {
                    self.go(pane)?;
                }
            }
            Message::Navigate(to) => {
                let moved = match to {
                    Navigation::Back => self.state.history.back(),
                    Navigation::Forward => self.state.history.forward(),
                };
                if moved.is_some() {
                    self.open = None;
                    self.state.follow()?;
                }
            }
            Message::Split(split) => self.split = split,
            Message::SidebarScroll(scroll) => self.sidebar_scroll = scroll,
            Message::ContentScroll(scroll) => self.content_scroll = scroll,
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
                if self.state.level.is_none() {
                    self.state.refresh(now)?;
                }
                return Ok(vec![after(REFRESH, Message::Refresh)]);
            }
        }
        Ok(Vec::new())
    }

    /// Choose search result `i`: its pane shows and the search ends.
    fn found(&mut self, i: usize) -> Result<Vec<Command<Message>>> {
        if let Some(hit) = self.hits().get(i) {
            let pane = hit.pane;
            self.query.clear();
            self.query_revision += 1;
            self.go(pane)?;
        }
        Ok(Vec::new())
    }

    /// A sheet a commit opened may need a listing from another thread first.
    fn sheet_started(&mut self) -> Vec<Command<Message>> {
        self.state.drive_picker();
        Vec::new()
    }

    fn sidebar(&self) -> Element<Message> {
        let has_battery = self.state.live.has_battery();
        let current = self.state.history.current();
        let sections = self
            .state
            .groups
            .iter()
            .map(|group| SidebarSection {
                title: None,
                items: group
                    .iter()
                    .map(|pane| SidebarItem {
                        key: Key::from(pane.key()),
                        label: pane.title(has_battery).into(),
                        icon: Some(pane.pane().symbol.into()),
                        description: None,
                        accessory: None,
                        busy: false,
                    })
                    .collect(),
            })
            .collect();
        Element::from(Sidebar {
            label: "Settings".into(),
            sections,
            selected: self.state.rows().iter().position(|p| *p == current),
            scroll: self.sidebar_scroll,
            on_scroll: Box::new(Message::SidebarScroll),
            on_select: Some(Box::new(Message::Select)),
            on_accessory: None,
        })
        .key("sidebar")
    }

    /// The pane's title and the search field, after the back and forward buttons.
    fn toolbar(&self) -> Vec<Element<Message>> {
        let title = self
            .state
            .history
            .current()
            .title(self.state.live.has_battery());
        let mut items: Vec<Element<Message>> =
            form::text(title, TextRole::Title).into_iter().collect();
        items.extend([
            Element::new(Spacer::default()),
            Element::new(TextField {
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
            })
            .width(SEARCH_WIDTH)
            .key("search"),
        ]);
        items
    }

    fn content(&self) -> Element<Message> {
        let body = if self.query.trim().is_empty() {
            form::pane(&self.state.sections(), self.open)
        } else {
            form::results(&self.hits())
        };
        let page = Element::new(Stack {
            axis: Axis::Vertical,
            padding: WINDOW_CONTENT_INSET,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![body],
        });
        Element::from(NavigationStack {
            content: Element::from(ScrollView {
                content: page,
                state: self.content_scroll,
                horizontal: false,
                vertical: true,
                scroller: Scroller::Overlay,
                on_scroll: Box::new(Message::ContentScroll),
            })
            .key("content"),
            toolbar: self.toolbar(),
            can_go_back: self.state.history.can_go_back(),
            can_go_forward: self.state.history.can_go_forward(),
            on_navigate: Some(Box::new(Message::Navigate)),
        })
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
        let split = Element::from(SplitView {
            label: "Sidebar".into(),
            axis: Axis::Horizontal,
            leading: shell.sidebar(),
            trailing: shell.content(),
            layout: shell.split,
            min_leading: MIN_SIDEBAR,
            min_trailing: MIN_CONTENT,
            on_change: Some(Box::new(Message::Split)),
        });
        let size = crate::window::default_size(WIDTH_FACTOR);
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
                size: Size::new(size.w, size.h),
                min_size: Some(Size::new(MIN_SIDEBAR + MIN_CONTENT, metrics::MIN_HEIGHT)),
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
            },
            shortcuts: shortcuts(),
            on_close: None,
            background: WindowBackground::Theme,
            content: Element::from(Modal {
                content: split,
                dialog: shell.state.sheet.as_ref().map(sheets::dialog),
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
        }
    }

    fn budget() -> NonZeroU32 {
        NonZeroU32::new(64).unwrap()
    }

    /// Gaps of every Kit window, not of Settings: Kit names neither the window's root nor a navigation toolbar.
    const KIT_GAPS: [&str; 2] = ["Window #0: no name", "Window #0 > Toolbar #1: no name"];

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
        let (wake, woken) = async_channel::bounded(1);
        let (notify, background) = background_channel(move || {
            let _ = wake.try_send(());
        });
        let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
        let mut state = State::new(paths(dir), services, pane, 1080.0, notify).unwrap();
        state.sheet = sheet;
        launch(Start {
            state,
            background,
            woken,
            activation: String::new(),
        });
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
        let options = RuntimeOptions {
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
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
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

    fn settle(driver: &mut TestDriver<SettingsApp>, window: WindowId) {
        for _ in 0..8 {
            driver.run_until_idle(budget()).unwrap();
            let system = driver.window_system_mut();
            if !system.windows().contains(&window) || !system.frame_requested(window).unwrap() {
                return;
            }
            system.frame_ready(window, Timestamp(0)).unwrap();
        }
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

    #[test]
    fn every_pane_leaves_nothing_a_screen_reader_cannot_announce() {
        let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        for pane in crate::sidebar::rows(&[]) {
            let dir = tempfile::tempdir().unwrap();
            let (driver, window) = start(dir.path(), pane);
            let snapshot = driver.runtime().snapshot(window).unwrap();
            let tree = AccessibilityUpdate {
                window,
                root: snapshot.accessibility[0].0,
                focus: snapshot.focus,
                nodes: snapshot.accessibility,
                removed: Vec::new(),
                full: true,
            };
            let mut gaps = semantic_gaps(&tree);
            gaps.retain(|gap| !KIT_GAPS.contains(&gap.as_str()));
            assert_eq!(gaps, Vec::<String>::new(), "{pane:?}");
        }
    }
}
