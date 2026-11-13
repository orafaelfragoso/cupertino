//! `cupertino_app_effects_v1`: the appearance observer, backdrops, the title bar style, the menu announcement and the
//! disabled window controls.

use crate::backdrop::BackdropData;
use crate::{Backdrop, BackdropId, BackdropTone, Quiet};
use cupertino_app_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle, WindowControl,
};
use cupertino_app_protocols::client::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1,
};
use cupertino_app_protocols::client::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1, DisplayOption,
};
use cupertino_app_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_wayland_client::{Connection, Error, State, SurfaceId};
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle};

macro_rules! quiet {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, Quiet> for State {
            fn event(
                _: &mut State,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &Quiet,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
            }
        }
    )*};
}

quiet!(CupertinoAppEffectsV1, CupertinoScrollV1);

/// What an object of the public protocols received, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// The session's appearance; also sent once when the observer is created.
    Appearance { dark: bool },
    /// The session's display options; also sent once when the observer is created.
    DisplayOptions(DisplayOptions),
    /// Whether scroll views keep tracks beside their content that stay shown (`always`) or show thin bars over it
    /// while it scrolls; sent after every [`AppEvent::DisplayOptions`] of an appearance observer from version 8.
    ScrollBars { always: bool },
    /// Whether text carets stay shown without blinking (`on`) or blink; sent after every [`AppEvent::ScrollBars`] of an
    /// appearance observer from version 9.
    SteadyCaret { on: bool },
    /// Whether the session is locked; also sent once when the observer is created, from version 5. While it is, the
    /// application announces nothing to assistive technology.
    SessionLock { locked: bool },
    /// The scroll count of one axis of the pointer frame being received; the frame's scroll follows.
    ScrollCount { vertical: bool, count: u32 },
    /// Whether the desktop's menu bar shows the menus `surface` announced; also sent once when its [`AppMenu`] is
    /// created. From version 3 it is true only while [`AppEvent::MenuReader`] names a connection, which arrives first.
    MenuBar { surface: SurfaceId, shown: bool },
    /// The unique bus name of the menu bar connection the server verified, the only peer that may use the menus
    /// `surface` announced (`Exporter::set_reader` of `cupertino-ui-menus`); `None` while there is none. Sent with
    /// [`AppEvent::MenuBar`] from version 3.
    MenuReader {
        surface: SurfaceId,
        bus_name: Option<String>,
    },
    /// The text and symbols over a glass backdrop take `tone`: the face behind a small shape turned dark or light, or
    /// the shape grew too large to follow it. From version 7; before it, and before the first measurement, the tone is
    /// [`BackdropTone::Appearance`].
    BackdropTone {
        backdrop: BackdropId,
        tone: BackdropTone,
    },
}

/// The app effects global (`cupertino_app_effects_v1`).
pub struct AppEffects(CupertinoAppEffectsV1);

impl AppEffects {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<AppEffects> {
        conn.bind_extension(1..=20, Quiet).map(AppEffects)
    }

    /// Observe the session's appearance and display options: [`AppEvent::Appearance`] and
    /// [`AppEvent::DisplayOptions`] arrive now and on every change while the observer lives, and from version 5
    /// [`AppEvent::SessionLock`] after them.
    pub fn appearance(&self, conn: &Connection) -> Appearance {
        Appearance(self.0.get_appearance(&conn.queue_handle(), Quiet))
    }

    /// A backdrop below the content of `surface`. On a subsurface it draws only from version 11
    /// ([`Self::draws_subsurface_backdrops`]): then directly below the subsurface, above the rest of its window.
    pub fn backdrop(&self, conn: &Connection, surface: SurfaceId) -> Result<Backdrop, Error> {
        let wl = conn.wl_surface(surface)?;
        Ok(Backdrop::new(self.0.get_backdrop(
            &wl,
            &conn.queue_handle(),
            BackdropData::new(),
        )))
    }

    /// Give the window of `surface` the unified title bar. Call it before the surface's first buffer, which the server
    /// reads the style with.
    pub fn unify_titlebar(&self, conn: &Connection, surface: SurfaceId) -> Result<(), Error> {
        self.set_window_style(conn, surface, WindowStyle::Unified)
    }

    /// Give the window of `surface` a window style. Call it before the surface's first buffer, which the server reads
    /// the style with. A server before version 6 knows the titled and the unified styles only: the compact unified
    /// style becomes the unified one there, and the sidebar style stays titled.
    pub fn set_window_style(
        &self,
        conn: &Connection,
        surface: SurfaceId,
        style: WindowStyle,
    ) -> Result<(), Error> {
        let styles = self.0.version() >= STYLES_VERSION;
        let wire = match style {
            WindowStyle::Titled => TitlebarStyle::Standard,
            WindowStyle::Unified => TitlebarStyle::Unified,
            WindowStyle::UnifiedCompact if styles => TitlebarStyle::UnifiedCompact,
            WindowStyle::UnifiedCompact => TitlebarStyle::Unified,
            WindowStyle::Sidebar if styles => TitlebarStyle::Sidebar,
            WindowStyle::Sidebar => TitlebarStyle::Standard,
        };
        self.0.set_titlebar_style(&conn.wl_surface(surface)?, wire);
        Ok(())
    }

    /// The window style a server gives a window that asks for `style`: the styles of version 6 fall back on older
    /// servers ([`Self::set_window_style`]).
    pub fn granted_style(&self, style: WindowStyle) -> WindowStyle {
        match style {
            _ if self.0.version() >= STYLES_VERSION => style,
            WindowStyle::UnifiedCompact => WindowStyle::Unified,
            WindowStyle::Sidebar => WindowStyle::Titled,
            other => other,
        }
    }

    /// The menu announcement of the window of `surface`, one per surface; `None` when the server predates it (version
    /// 1), so the window keeps its menus. [`AppEvent::MenuBar`] tells whether a menu bar shows them.
    pub fn menu(&self, conn: &Connection, surface: SurfaceId) -> Result<Option<AppMenu>, Error> {
        if self.0.version() < 2 {
            return Ok(None);
        }
        let wl = conn.wl_surface(surface)?;
        Ok(Some(AppMenu(self.0.get_menu(
            &wl,
            &conn.queue_handle(),
            MenuSurface(surface),
        ))))
    }

    /// Disable controls of the window of `surface`, replacing those it disabled before; they apply with the surface's
    /// next commit and may change at any time. The close button can't be disabled. Not sent to a server that predates
    /// it (below version 4): the window keeps every control.
    pub fn set_disabled_controls(
        &self,
        conn: &Connection,
        surface: SurfaceId,
        disabled: WindowControls,
    ) -> Result<(), Error> {
        if !self.can_set_disabled_controls() {
            return Ok(());
        }
        let mut bits = WindowControl::empty();
        bits.set(WindowControl::Minimize, disabled.minimize);
        bits.set(WindowControl::Zoom, disabled.zoom);
        self.0
            .set_disabled_controls(&conn.wl_surface(surface)?, bits);
        Ok(())
    }

    /// Whether observers tell [`AppEvent::SessionLock`] (version 5).
    pub fn tells_session_lock(&self) -> bool {
        self.0.version() >= SESSION_LOCK_VERSION
    }

    /// Whether the server takes [`Self::set_disabled_controls`] (version 4).
    pub fn can_set_disabled_controls(&self) -> bool {
        self.0.version() >= DISABLED_CONTROLS_VERSION
    }

    /// Whether the backdrops of a subsurface draw (version 11): directly below the subsurface, above the rest of its
    /// window, reading what of the window lies below them. Before it they draw nothing.
    pub fn draws_subsurface_backdrops(&self) -> bool {
        self.0.version() >= SUBSURFACE_BACKDROPS_VERSION
    }
}

/// The version that adds the compact unified and the sidebar title bar styles.
const STYLES_VERSION: u32 = 6;
/// The version that adds `set_disabled_controls`.
const DISABLED_CONTROLS_VERSION: u32 = 4;
/// The version whose appearance observer tells `session_lock`.
const SESSION_LOCK_VERSION: u32 = 5;
/// The version whose display options say whether scroll bars stay shown.
const SCROLL_BARS_VERSION: u32 = 8;
/// The version whose display options say whether text carets keep from blinking.
const STEADY_CARET_VERSION: u32 = 9;
/// The version whose backdrops of a subsurface draw above the rest of its window.
const SUBSURFACE_BACKDROPS_VERSION: u32 = 11;

/// How the server decorates an application window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowStyle {
    /// A title bar above the surface with the window's title.
    #[default]
    Titled,
    /// The surface fills the window; the server draws the window control buttons in its toolbar row.
    Unified,
    /// [`WindowStyle::Unified`] with the compact toolbar row.
    UnifiedCompact,
    /// The surface fills the window; the server draws the window control buttons at the top of its full-height sidebar.
    Sidebar,
}

/// Window controls, each `true` when it is in the set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowControls {
    /// The minimize button, the minimize shortcut and the Window menu's Minimize.
    pub minimize: bool,
    /// The zoom button, the zoom shortcut, the Window menu's Zoom and the title bar's double click.
    pub zoom: bool,
}

/// Where a window's menus are on the session bus: its unique bus name and the object paths of the application's
/// actions, the window's actions, its menu bar and the application menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuAddress {
    pub bus_name: String,
    pub application_object_path: String,
    pub window_object_path: String,
    pub menubar_object_path: String,
    pub app_menu_object_path: Option<String>,
}

/// The menu announcement of a window; dropping it withdraws the menus from the menu bar.
pub struct AppMenu(CupertinoAppMenuV1);

/// User data of a menu announcement: the surface its events name.
pub(crate) struct MenuSurface(SurfaceId);

impl AppMenu {
    /// Announce (or replace) where the window's menus are. A malformed bus name or object path fails the connection.
    pub fn set_address(&self, address: &MenuAddress) {
        self.0.set_address(
            address.bus_name.clone(),
            address.application_object_path.clone(),
            address.window_object_path.clone(),
            address.menubar_object_path.clone(),
            address.app_menu_object_path.clone().unwrap_or_default(),
        );
    }
}

impl Drop for AppMenu {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// An appearance observer; dropping it stops the events.
pub struct Appearance(CupertinoAppearanceV1);

impl Drop for Appearance {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// Report an appearance value of the wire (0 light, 1 dark) as [`AppEvent::Appearance`]; any other value fails the
/// connection.
pub fn report_appearance(state: &mut State, dark: u32) {
    if dark > 1 {
        return state.fail(Error::Protocol("appearance other than 0 or 1"));
    }
    state.emit(AppEvent::Appearance { dark: dark == 1 });
}

/// Report display option bits of the wire, without reserved bits, as [`AppEvent::DisplayOptions`].
pub fn report_display_options(state: &mut State, bits: u32) {
    let bits = DisplayOption::from_bits_truncate(bits);
    state.emit(AppEvent::DisplayOptions(DisplayOptions {
        reduce_motion: bits.contains(DisplayOption::ReduceMotion),
        reduce_transparency: bits.contains(DisplayOption::ReduceTransparency),
        increase_contrast: bits.contains(DisplayOption::IncreaseContrast),
        differentiate_without_color: bits.contains(DisplayOption::DifferentiateWithoutColor),
    }));
}

impl Dispatch<CupertinoAppMenuV1, MenuSurface> for State {
    fn event(
        state: &mut State,
        _: &CupertinoAppMenuV1,
        event: cupertino_app_menu_v1::Event,
        surface: &MenuSurface,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_app_menu_v1::Event::MenuBar { shown } => {
                if shown > 1 {
                    return state.fail(Error::Protocol("menu bar shown other than 0 or 1"));
                }
                state.emit(AppEvent::MenuBar {
                    surface: surface.0,
                    shown: shown == 1,
                });
            }
            cupertino_app_menu_v1::Event::MenuReader { bus_name } => {
                if !(bus_name.is_empty() || bus_name.starts_with(':')) {
                    return state
                        .fail(Error::Protocol("menu reader that is not a unique bus name"));
                }
                state.emit(AppEvent::MenuReader {
                    surface: surface.0,
                    bus_name: (!bus_name.is_empty()).then_some(bus_name),
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoAppearanceV1, Quiet> for State {
    fn event(
        state: &mut State,
        observer: &CupertinoAppearanceV1,
        event: cupertino_appearance_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_appearance_v1::Event::Appearance { dark } => report_appearance(state, dark),
            cupertino_appearance_v1::Event::DisplayOptions { options } => {
                match options.into_result() {
                    Ok(bits) => {
                        report_display_options(state, bits.bits());
                        // an older server does not know the setting: the views keep their own bars
                        if observer.version() >= SCROLL_BARS_VERSION {
                            state.emit(AppEvent::ScrollBars {
                                always: bits.contains(DisplayOption::AlwaysShowScrollBars),
                            });
                        }
                        if observer.version() >= STEADY_CARET_VERSION {
                            state.emit(AppEvent::SteadyCaret {
                                on: bits.contains(DisplayOption::SteadyCaret),
                            });
                        }
                    }
                    Err(_) => state.fail(Error::Protocol("display options with reserved bits")),
                }
            }
            cupertino_appearance_v1::Event::SessionLock { locked } => {
                if locked > 1 {
                    return state.fail(Error::Protocol("session lock other than 0 or 1"));
                }
                state.emit(AppEvent::SessionLock {
                    locked: locked == 1,
                });
            }
            _ => {}
        }
    }
}
