//! Server side of the session observers: `cupertino_shell_v1.get_session` (`cupertino_session_v1`, shell processes
//! with the Session role) tells the session's appearance, accessibility display options (version 2) and lock state,
//! on bind and on every change; `cupertino_app_effects_v1.get_appearance` (`cupertino_appearance_v1`, any client) tells
//! the appearance, display options and (version 5) whether the session is locked, so an application keeps quiet for
//! assistive technology behind the lock. The appearance changes only through
//! [`Cupertino::set_appearance`], the display options through `accessibility.rs`, which calls
//! [`SessionObservers::display_options`]; the lock state follows `session_lock.rs`, which calls
//! [`SessionObservers::lock`] where the lock begins and ends. From shell version 20 a session observer also takes
//! `activate_app` (the launcher opening an application, [`Cupertino::activate_desktop_app`]), checked for the Session role
//! on every request, and answers it with `app_activation`.

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::compositor::ScrollBarsName;
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

use crate::authorization::{authorized, refuse, require, role, Permit, ShellRole};
use crate::state::Cupertino;

/// The session observers of every client, in creation order.
#[derive(Default)]
pub struct SessionObservers {
    observers: Vec<CupertinoSessionV1>,
    appearance_observers: Vec<CupertinoAppearanceV1>,
    /// What the appearance observers were last told of the scroll bars: always shown.
    always_show_scroll_bars: bool,
    /// What the appearance observers were last told of the text caret: kept from blinking.
    steady_caret: bool,
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
            send_appearance_options(observer, options, self.extras());
        }
    }

    fn extras(&self) -> Extras {
        Extras {
            always_show_scroll_bars: self.always_show_scroll_bars,
            steady_caret: self.steady_caret,
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
        for observer in &self.appearance_observers {
            send_appearance_lock(observer, locked);
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

/// The version of `cupertino_appearance_v1` that knows the `always_show_scroll_bars` display option; an older
/// observer takes an unknown bit as a protocol error, so it is never sent one.
const SCROLL_BARS_VERSION: u32 = 8;

/// The version of `cupertino_appearance_v1` that knows the `steady_caret` display option.
const STEADY_CARET_VERSION: u32 = 9;

/// The display options of the appearance observers that are not the session's display options.
#[derive(Clone, Copy)]
struct Extras {
    always_show_scroll_bars: bool,
    steady_caret: bool,
}

/// `display_options` with the bits of `options`, and each of `extras` that is on when the observer knows its bit.
fn send_appearance_options(
    observer: &CupertinoAppearanceV1,
    options: DisplayOptions,
    extras: Extras,
) {
    let mut bits = AppearanceOption::empty();
    bits.set(
        AppearanceOption::AlwaysShowScrollBars,
        extras.always_show_scroll_bars && observer.version() >= SCROLL_BARS_VERSION,
    );
    bits.set(
        AppearanceOption::SteadyCaret,
        extras.steady_caret && observer.version() >= STEADY_CARET_VERSION,
    );
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

/// `session_lock`; observers created below version 5 know no such event.
fn send_appearance_lock(observer: &CupertinoAppearanceV1, locked: bool) {
    if observer.version() >= 5 {
        observer.session_lock(u32::from(locked));
    }
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
        self.sync_scroll_bars();
        send_appearance_options(&observer, self.display_options(), self.sessions.extras());
        send_appearance_lock(&observer, self.session_lock.is_active());
        self.sessions.appearance_observers.push(observer);
    }

    /// Whether applications show their scroll bars beside the content at all times: by the setting, and for an
    /// automatic one while a mouse is connected (a pointing device other than a touchpad that has moved, scrolled
    /// or clicked; a keyboard's unused pointer interface does not count).
    pub fn always_show_scroll_bars(&self) -> bool {
        match self.config.show_scroll_bars {
            ScrollBarsName::Always => true,
            ScrollBarsName::WhenScrolling => false,
            ScrollBarsName::Automatic => self.pointer_devices.values().any(|d| d.is_used_mouse()),
        }
    }

    /// Tell the appearance observers when [`Self::always_show_scroll_bars`] changed: after the setting changes and
    /// when a mouse is first used or goes away.
    pub fn sync_scroll_bars(&mut self) {
        let always = self.always_show_scroll_bars();
        if always == self.sessions.always_show_scroll_bars {
            return;
        }
        self.sessions.always_show_scroll_bars = always;
        let (options, extras) = (self.display_options(), self.sessions.extras());
        for observer in &self.sessions.appearance_observers {
            // an older observer would be told the same options again
            if observer.version() >= SCROLL_BARS_VERSION {
                send_appearance_options(observer, options, extras);
            }
        }
    }

    /// Tell the observers the display options changed when they differ from `before`, and the appearance observers
    /// that know it when only the setting that keeps the text caret from blinking changed.
    pub(crate) fn tell_display_options(&mut self, before: DisplayOptions) {
        let steady = self.accessibility.settings.steady_caret;
        let steady_changed = steady != self.sessions.steady_caret;
        self.sessions.steady_caret = steady;
        let options = self.display_options();
        if options != before {
            self.sessions.display_options(options);
        } else if steady_changed {
            let extras = self.sessions.extras();
            for observer in &self.sessions.appearance_observers {
                // an older observer would be told the same options again
                if observer.version() >= STEADY_CARET_VERSION {
                    send_appearance_options(observer, options, extras);
                }
            }
        }
    }

    /// Switch the session's appearance: the server's own drawing follows on the next frame and the
    /// shell processes are told. Setting the appearance it already has changes nothing.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if self.config.appearance == appearance {
            return;
        }
        self.snapshot_appearance_fade();
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
        state: &mut Self,
        client: &Client,
        observer: &CupertinoSessionV1,
        request: cupertino_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // `Destroy` is handled in `destroyed`
        if let cupertino_session_v1::Request::ActivateApp { serial, desktop_id } = request {
            // the role is checked again here: the holder may have exited since `get_session`
            if !require(client, ShellRole::Session, observer) {
                return;
            }
            let result = state.activate_desktop_app(&desktop_id);
            observer.app_activation(serial, result);
        }
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
