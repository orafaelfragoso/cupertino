//! Which output new and transient things appear on, and the scale each surface is told.
//!
//! The **active output** is the output the person is working on: the one the pointer last moved onto, or the output of
//! the window that last took the keyboard, whichever happened last. Its menu bar is the bright one. Everything that
//! opens without an output of its own opens there: a new window, a layer surface created without an output (the
//! launcher, notifications, the control panel), the overview, the application switcher's panel, the dock the keyboard
//! goes to and workspace shortcuts. A dock and a menu bar are per output and stay on their own.
//!
//! Each output keeps its own work area and scale. A window is told the scale of the output its frame overlaps most, held
//! near a seam (`surface_output`), and a layer surface its own output's, so a panel on one output never draws at the
//! scale of another.

use cupertino_wm::{OutputId, WindowId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::Cupertino;

impl Cupertino {
    /// The output new and transient things appear on (see the module documentation): the active menu bar's output,
    /// else the one under the pointer, else the first.
    pub fn active_output(&self) -> Option<OutputId> {
        self.menu_bars
            .set
            .active_output()
            .filter(|o| self.output_entry(*o).is_some())
            .or_else(|| {
                self.output_at(self.pointer.current_location())
                    .map(|o| o.id)
            })
            .or_else(|| self.outputs.first().map(|o| o.id))
    }

    /// The keyboard went to `window`: its output becomes the active one.
    pub(crate) fn follow_focus_to_output(&mut self, window: WindowId) {
        let Some(output) = self.wm.window(window).map(|w| w.output) else {
            return;
        };
        if self.output_entry(output).is_none() || self.menu_bars.set.active_output() == Some(output)
        {
            return;
        }
        let now = self.now();
        self.menu_bars.set.set_active_output(Some(output), now);
        self.request_redraw();
    }

    /// The scale of output `id`, the active output's when it is unknown.
    pub(crate) fn output_scale(&self, id: Option<OutputId>) -> f64 {
        id.and_then(|id| self.output_entry(id))
            .or_else(|| self.active_output().and_then(|o| self.output_entry(o)))
            .map_or(1.0, |o| o.scale)
    }

    /// The scale window `window` draws at: its scale output's (`surface_output`).
    pub fn window_scale(&self, window: WindowId) -> f64 {
        self.output_scale(self.window_scale_output(window))
    }

    /// The scale a surface that asks for its preferred scale starts at: a window's (a popup's or subsurface's root
    /// window's), a layer surface's or a lock surface's output's, else the active output's.
    pub fn surface_scale(&self, surface: &WlSurface) -> f64 {
        if let Some(window) = self
            .root_window_surface(surface)
            .and_then(|root| self.window_id_for_surface(&root))
        {
            return self.window_scale(window);
        }
        let layer_output = self.outputs.iter().find(|o| {
            o.lock.wl_surface() == Some(surface)
                || smithay::desktop::layer_map_for_output(&o.output)
                    .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::ALL)
                    .is_some()
        });
        match layer_output {
            Some(o) => o.scale,
            None => self.output_scale(None),
        }
    }

    /// Tell every window the outputs it is on and the scale of the one it draws at.
    pub fn refresh_window_scales(&mut self) {
        self.refresh_window_outputs(true);
    }
}
