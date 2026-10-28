//! `cupertino_app_effects_v1`: the appearance observer, backdrops, the title bar style, the menu announcement and the
//! disabled window controls.

use crate::{Backdrop, Quiet};
use cupertino_app_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle, WindowControl,
};
use cupertino_app_protocols::client::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1,
};
use cupertino_app_protocols::client::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1, DisplayOption,
};
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
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

quiet!(
    CupertinoAppEffectsV1,
    CupertinoBackdropV1,
    CupertinoScrollV1
);

/// What an object of the public protocols received, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// The session's appearance; also sent once when the observer is created.
    Appearance { dark: bool },
    /// The session's display options; also sent once when the observer is created.
    DisplayOptions(DisplayOptions),
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
}

/// The app effects global (`cupertino_app_effects_v1`).
pub struct AppEffects(CupertinoAppEffectsV1);

impl AppEffects {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<AppEffects> {
        conn.bind_extension(1..=4, Quiet).map(AppEffects)
    }

    /// Observe the session's appearance and display options: [`AppEvent::Appearance`] and
    /// [`AppEvent::DisplayOptions`] arrive now and on every change while the observer lives.
    pub fn appearance(&self, conn: &Connection) -> Appearance {
        Appearance(self.0.get_appearance(&conn.queue_handle(), Quiet))
    }

    /// A backdrop below the content of `surface`.
    pub fn backdrop(&self, conn: &Connection, surface: SurfaceId) -> Result<Backdrop, Error> {
        let wl = conn.wl_surface(surface)?;
        Ok(Backdrop::new(self.0.get_backdrop(
            &wl,
            &conn.queue_handle(),
            Quiet,
        )))
    }

    /// Give the window of `surface` the unified title bar. Call it before the surface's first buffer, which the server
    /// reads the style with.
    pub fn unify_titlebar(&self, conn: &Connection, surface: SurfaceId) -> Result<(), Error> {
        self.0
            .set_titlebar_style(&conn.wl_surface(surface)?, TitlebarStyle::Unified);
        Ok(())
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

    /// Whether the server takes [`Self::set_disabled_controls`] (version 4).
    pub fn can_set_disabled_controls(&self) -> bool {
        self.0.version() >= DISABLED_CONTROLS_VERSION
    }
}

/// The version that adds `set_disabled_controls`.
const DISABLED_CONTROLS_VERSION: u32 = 4;

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
        _: &CupertinoAppearanceV1,
        event: cupertino_appearance_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_appearance_v1::Event::Appearance { dark } => report_appearance(state, dark),
            cupertino_appearance_v1::Event::DisplayOptions { options } => {
                match options.into_result() {
                    Ok(bits) => report_display_options(state, bits.bits()),
                    Err(_) => state.fail(Error::Protocol("display options with reserved bits")),
                }
            }
            _ => {}
        }
    }
}
