//! Server side of the session observer (`cupertino_shell_v1.get_session`, `cupertino_session_v1`):
//! tells the shell processes the session's appearance, accessibility display options (version 2)
//! and lock state, on bind and on every change. The appearance changes only through
//! [`Cupertino::set_appearance`], the display options through `accessibility.rs`, which calls
//! [`SessionObservers::display_options`]; the lock state follows `session_lock.rs`, which calls
//! [`SessionObservers::lock`] where the lock begins and ends.

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::server::cupertino_session_v1::{self, CupertinoSessionV1, DisplayOption};
use cupertino_theme::Appearance;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, New, Resource};

use crate::state::Cupertino;

/// The session observers of every client, in creation order.
#[derive(Default)]
pub struct SessionObservers {
    observers: Vec<CupertinoSessionV1>,
}

impl SessionObservers {
    /// Tell every observer the appearance changed.
    fn appearance(&self, dark: bool) {
        for observer in &self.observers {
            observer.appearance(u32::from(dark));
        }
    }

    /// Tell every observer of version 2 or later the display options changed.
    pub fn display_options(&self, options: DisplayOptions) {
        for observer in &self.observers {
            send_display_options(observer, options);
        }
    }

    /// Tell every observer the session was locked or unlocked.
    pub fn lock(&self, locked: bool) {
        for observer in &self.observers {
            send_lock(observer, locked);
        }
    }
}

/// `display_options` with the bits of `options`; observers created at version 1 know no such event.
fn send_display_options(observer: &CupertinoSessionV1, options: DisplayOptions) {
    if observer.version() < 2 {
        return;
    }
    let mut bits = DisplayOption::empty();
    bits.set(DisplayOption::ReduceMotion, options.reduce_motion);
    bits.set(DisplayOption::ReduceTransparency, options.reduce_transparency);
    bits.set(DisplayOption::IncreaseContrast, options.increase_contrast);
    bits.set(DisplayOption::DifferentiateWithoutColor, options.differentiate_without_color);
    observer.display_options(bits);
}

fn send_lock(observer: &CupertinoSessionV1, locked: bool) {
    if locked {
        observer.locked();
    } else {
        observer.unlocked();
    }
}

impl Cupertino {
    /// `cupertino_shell_v1.get_session`: register the observer and send it the current state.
    pub fn create_session_observer(&mut self, id: New<CupertinoSessionV1>, init: &mut DataInit<'_, Self>) {
        let observer = init.init(id, ());
        observer.appearance(u32::from(self.config.is_dark()));
        send_display_options(&observer, self.display_options());
        send_lock(&observer, self.session_lock.is_active());
        self.sessions.observers.push(observer);
    }

    /// Switch the session's appearance: the server's own drawing follows on the next frame and the
    /// shell processes are told. Setting the appearance it already has changes nothing.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if self.config.appearance == appearance {
            return;
        }
        self.config.appearance = appearance;
        self.decor.set_appearance(self.appearance());
        self.sessions.appearance(appearance.is_dark());
        self.publish_session_settings();
        self.request_redraw();
    }

    /// Light becomes dark and dark becomes light.
    pub fn toggle_appearance(&mut self) {
        self.set_appearance(if self.config.is_dark() { Appearance::Light } else { Appearance::Dark });
    }
}

impl Dispatch<CupertinoSessionV1, ()> for Cupertino {
    fn request(_: &mut Self, _: &Client, _: &CupertinoSessionV1, _: cupertino_session_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        // `Destroy` is handled in `destroyed`
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoSessionV1, _: &()) {
        state.sessions.observers.retain(|o| o != resource);
    }
}
