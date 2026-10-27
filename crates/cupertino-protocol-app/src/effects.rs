//! `cupertino_app_effects_v1`: the appearance observer, backdrops and the title bar style.

use crate::{Backdrop, Quiet};
use cupertino_app_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle,
};
use cupertino_app_protocols::client::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1, DisplayOption,
};
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_app_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_wayland_client::{Connection, Error, State, SurfaceId};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// The session's appearance; also sent once when the observer is created.
    Appearance { dark: bool },
    /// The session's display options; also sent once when the observer is created.
    DisplayOptions(DisplayOptions),
    /// The scroll count of one axis of the pointer frame being received; the frame's scroll follows.
    ScrollCount { vertical: bool, count: u32 },
}

/// The app effects global (`cupertino_app_effects_v1`).
pub struct AppEffects(CupertinoAppEffectsV1);

impl AppEffects {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<AppEffects> {
        conn.bind_extension(1..=1, Quiet).map(AppEffects)
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
