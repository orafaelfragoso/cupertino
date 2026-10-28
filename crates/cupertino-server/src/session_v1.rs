//! Server side of the session observers: `cupertino_shell_v1.get_session` (`cupertino_session_v1`, shell processes
//! with the Session role) tells the session's appearance, accessibility display options (version 2) and lock state,
//! on bind and on every change; `cupertino_app_effects_v1.get_appearance` (`cupertino_appearance_v1`, any client) tells
//! the appearance and display options only, so the lock state stays with the shell. The appearance changes only through
//! [`Cupertino::set_appearance`], the display options through `accessibility.rs`, which calls
//! [`SessionObservers::display_options`]; the lock state follows `session_lock.rs`, which calls
//! [`SessionObservers::lock`] where the lock begins and ends.

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::server::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1, DisplayOption as AppearanceOption,
};
use cupertino_protocols::server::cupertino_session_v1::{self, CupertinoSessionV1, DisplayOption};
use cupertino_protocols::server::cupertino_shell_v1::CupertinoShellV1;
use cupertino_theme::Appearance;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource,
};

use crate::authorization::{authorized, refuse, role, Permit, ShellRole};
use crate::state::Cupertino;

/// The session observers of every client, in creation order.
#[derive(Default)]
pub struct SessionObservers {
    observers: Vec<CupertinoSessionV1>,
    appearance_observers: Vec<CupertinoAppearanceV1>,
}

/// Whether the connection of `observer` still holds the Session role: an observer whose holder exited is not told.
fn permitted(observer: &CupertinoSessionV1) -> bool {
    observer
        .client()
        .is_some_and(|c| authorized(&c, ShellRole::Session))
}

impl SessionObservers {
    /// Tell every observer the appearance changed.
    fn appearance(&self, dark: bool) {
        for observer in self.observers.iter().filter(|o| permitted(o)) {
            observer.appearance(u32::from(dark));
        }
        for observer in &self.appearance_observers {
            observer.appearance(u32::from(dark));
        }
    }

    /// Tell every observer of version 2 or later the display options changed.
    pub fn display_options(&self, options: DisplayOptions) {
        for observer in self.observers.iter().filter(|o| permitted(o)) {
            send_display_options(observer, options);
        }
        for observer in &self.appearance_observers {
            send_appearance_options(observer, options);
        }
    }

    /// Ask the lock screen to lock the session: only an observer of the connection that holds the session lock role is
    /// told. False when there is none, so the caller falls back to the session bus.
    pub fn lock_requested(&self, explicit: bool) -> bool {
        let mut told = false;
        for observer in self.observers.iter().filter(|o| {
            o.version() >= 7
                && o.client()
                    .is_some_and(|c| authorized(&c, ShellRole::SessionLock))
        }) {
            observer.lock_requested(u32::from(explicit));
            told = true;
        }
        told
    }

    /// Ask the login process to ask whether to log out, the way [`Self::lock_requested`] asks for the lock.
    pub fn log_out_requested(&self) -> bool {
        let mut told = false;
        for observer in self.observers.iter().filter(|o| {
            o.version() >= 9
                && o.client()
                    .is_some_and(|c| authorized(&c, ShellRole::SessionLock))
        }) {
            observer.log_out_requested();
            told = true;
        }
        told
    }

    /// Tell every observer the session was locked or unlocked.
    pub fn lock(&self, locked: bool) {
        for observer in self.observers.iter().filter(|o| permitted(o)) {
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
    bits.set(
        DisplayOption::ReduceTransparency,
        options.reduce_transparency,
    );
    bits.set(DisplayOption::IncreaseContrast, options.increase_contrast);
    bits.set(
        DisplayOption::DifferentiateWithoutColor,
        options.differentiate_without_color,
    );
    observer.display_options(bits);
}

fn send_appearance_options(observer: &CupertinoAppearanceV1, options: DisplayOptions) {
    let mut bits = AppearanceOption::empty();
    bits.set(AppearanceOption::ReduceMotion, options.reduce_motion);
    bits.set(
        AppearanceOption::ReduceTransparency,
        options.reduce_transparency,
    );
    bits.set(
        AppearanceOption::IncreaseContrast,
        options.increase_contrast,
    );
    bits.set(
        AppearanceOption::DifferentiateWithoutColor,
        options.differentiate_without_color,
    );
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
    pub fn create_session_observer(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::Session>>,
        id: New<CupertinoSessionV1>,
        init: &mut DataInit<'_, Self>,
    ) {
        let observer = init.init(id, ());
        if permit.is_none() {
            refuse(shell, ShellRole::Session);
            return;
        }
        observer.appearance(u32::from(self.config.is_dark()));
        send_display_options(&observer, self.display_options());
        send_lock(&observer, self.session_lock.is_active());
        self.sessions.observers.push(observer);
    }

    /// `cupertino_app_effects_v1.get_appearance`: register the observer and send it the current state.
    pub fn create_appearance_observer(
        &mut self,
        id: New<CupertinoAppearanceV1>,
        init: &mut DataInit<'_, Self>,
    ) {
        let observer = init.init(id, ());
        observer.appearance(u32::from(self.config.is_dark()));
        send_appearance_options(&observer, self.display_options());
        self.sessions.appearance_observers.push(observer);
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
        self.set_appearance(if self.config.is_dark() {
            Appearance::Light
        } else {
            Appearance::Dark
        });
    }
}

impl Dispatch<CupertinoSessionV1, ()> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &CupertinoSessionV1,
        _: cupertino_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // `Destroy` is handled in `destroyed`
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoSessionV1, _: &()) {
        state.sessions.observers.retain(|o| o != resource);
    }
}

impl Dispatch<CupertinoAppearanceV1, ()> for Cupertino {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &CupertinoAppearanceV1,
        _: cupertino_appearance_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // `Destroy` is handled in `destroyed`
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoAppearanceV1, _: &()) {
        state
            .sessions
            .appearance_observers
            .retain(|o| o != resource);
    }
}
