//! Compositor wiring for [`cupertino_server_render::decor_access`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use cupertino_geometry::{Rect, Size};
use cupertino_server_render::decorations::ButtonKind;
use cupertino_wm::WindowId;
use smithay::reexports::calloop::channel;

use crate::decor::button_enabled;
use crate::state::Cupertino;

pub use cupertino_server_render::decor_access::*;

impl Cupertino {
    /// Publish the window control buttons on the accessibility bus from now on, and take the presses an assistive
    /// technology asks for on the loop: a person's session on the hardware.
    pub fn start_button_access(&mut self) {
        let (presses, received) = channel::channel::<(WindowId, ButtonKind)>();
        let inserted = self.loop_handle.insert_source(received, |event, _, state| {
            if let channel::Event::Msg((window, kind)) = event {
                state.press_published_button(window, kind);
            }
        });
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "the window buttons are not published to assistive technology");
            return;
        }
        let presses = Mutex::new(presses);
        self.button_access.set_sink(Box::new(AtspiSink::new(
            "Cupertino",
            Arc::new(move |window, kind| {
                // the loop is gone only while the server shuts down
                let _ = presses
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((window, kind));
            }),
        )));
    }

    /// Press button `kind` of `window` as an assistive technology asked: only a button published enabled, of a window
    /// whose buttons are published now, while the session is unlocked; then it does what a click does
    /// ([`Cupertino::press_window_button`], which checks the button again).
    pub fn press_published_button(&mut self, window: WindowId, kind: ButtonKind) {
        let published = self
            .button_access
            .published()
            .get(&window)
            .is_some_and(|b| b.buttons[kind.index()].enabled);
        if published && !self.session_lock.is_active() {
            self.press_window_button(window, kind);
        }
    }

    /// The buttons of every window whose buttons the server draws and shows, while the session is unlocked: a mapped
    /// window with server-side decorations, neither full screen nor minimized (nor on its way there or out); and a
    /// full-screen window while its title bar shows, revealed with the menu bar.
    pub fn window_buttons(&self) -> HashMap<WindowId, WindowButtons> {
        use cupertino_wm::LifecycleState::*;
        if self.session_lock.is_active() || !self.config.window.decorations {
            return HashMap::new();
        }
        let now = self.now();
        self.windows
            .iter()
            .filter(|e| e.mapped)
            .filter_map(|e| {
                let w = self.wm.window(e.id)?;
                if e.fullscreen {
                    return self.revealed_buttons(e, w, now);
                }
                if !e.server_side || !(e.insets().top > 0.0 || e.titlebar.client_drawn()) {
                    return None;
                }
                if matches!(
                    w.state(),
                    Created
                        | Minimizing
                        | Minimized
                        | EnterFullScreen
                        | FullScreen
                        | ExitFullScreen
                        | Closing
                        | Destroyed
                ) {
                    return None;
                }
                let geo = e.window.geometry();
                let content = Size::new(f64::from(geo.size.w), f64::from(geo.size.h));
                let layout = self.decor.for_style(e.titlebar).layout(content);
                let enabled = button_enabled(w.can_minimize(), w.can_zoom());
                let frame = w.geometry();
                Some((
                    e.id,
                    WindowButtons {
                        title: w.title.clone(),
                        frame: Rect::new(frame.x, frame.y, frame.w, frame.h),
                        buttons: ButtonKind::ALL.map(|kind| ButtonAccess {
                            kind,
                            enabled: enabled[kind.index()],
                            bounds: layout.button(kind).art,
                        }),
                    },
                ))
            })
            .collect()
    }

    /// The buttons of full-screen window `e` while its title bar shows, revealed with the menu bar over it, as the scene
    /// draws them: close and zoom (which leaves full screen) enabled, minimize disabled.
    fn revealed_buttons(
        &self,
        e: &crate::state::WindowEntry,
        w: &cupertino_wm::Window,
        now: f64,
    ) -> Option<(WindowId, WindowButtons)> {
        use crate::state::WindowEntry;
        if w.state() != cupertino_wm::LifecycleState::FullScreen {
            return None;
        }
        let strip = e.revealed_titlebar(&self.menu_bars, &self.decor, true, w.output, now)?;
        let layout = self
            .decor
            .for_style(WindowEntry::REVEALED_TITLEBAR)
            .layout(Size::new(strip.w, strip.h));
        let enabled = button_enabled(false, true);
        Some((
            e.id,
            WindowButtons {
                title: w.title.clone(),
                frame: Rect::new(strip.x, strip.y, strip.w, strip.h),
                buttons: ButtonKind::ALL.map(|kind| ButtonAccess {
                    kind,
                    enabled: enabled[kind.index()],
                    bounds: layout.button(kind).art,
                }),
            },
        ))
    }

    /// Bring what assistive technology reads of the window buttons up to date; nothing when they are not published.
    pub(crate) fn sync_button_access(&mut self) {
        if !self.button_access.enabled() {
            return;
        }
        let animating = self.animating();
        if self
            .button_access
            .due(animating, self.session_lock.is_active())
        {
            let now = self.window_buttons();
            self.button_access.sync(now);
        }
    }
}
