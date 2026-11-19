//! Compositor wiring for [`cupertino_server_input::hung`].

use smithay::wayland::shell::xdg::ShellClient;

pub use cupertino_server_input::hung::*;

impl crate::state::Cupertino {
    /// Input came for the clients (a key, the pointer, a scroll; the person's, a script's or a controlling agent's):
    /// they are pinged again.
    pub fn input_came(&mut self) {
        let now = self.now();
        self.hung.input(now);
    }

    /// Ping the clients that own a mapped toplevel and note those that do not answer (the model clock `now`).
    pub fn tick_hung(&mut self, now: f64) {
        let clients: Vec<ShellClient> = self
            .windows
            .iter()
            .filter(|e| e.mapped)
            .map(|e| e.toplevel.client())
            .collect();
        let changed = self.hung.tick(now, clients, || {
            smithay::utils::SERIAL_COUNTER.next_serial()
        });
        if changed.0 {
            tracing::warn!("an application does not answer its pings");
            self.request_redraw();
        }
        self.announce_responsiveness();
    }

    /// Whether the client of window `id` does not answer its pings.
    pub fn window_is_unresponsive(&self, id: cupertino_wm::WindowId) -> bool {
        self.entry(id)
            .is_some_and(|e| self.hung.is_unresponsive(&e.toplevel.client()))
    }

    /// Whether the pointer is over the content of a window of an unresponsive client: the busy cursor shows there.
    pub fn pointer_over_unresponsive(&self) -> bool {
        if !self.hung.any_unresponsive() {
            return false;
        }
        match self.window_target_on_top(self.pointer.current_location()) {
            Some(crate::state::WindowTarget::Surface { id, .. }) => self.window_is_unresponsive(id),
            _ => false,
        }
    }

    /// The application ids whose clients do not answer their pings.
    pub fn unresponsive_apps(&self) -> std::collections::BTreeSet<String> {
        if !self.hung.any_unresponsive() {
            return Default::default();
        }
        self.windows
            .iter()
            .filter(|e| e.mapped && self.hung.is_unresponsive(&e.toplevel.client()))
            .filter_map(|e| self.wm.window(e.id))
            .filter_map(|w| self.app_identity_of(w.app))
            .collect()
    }

    /// Tell the docks which applications started or stopped answering since they were last told.
    pub(crate) fn announce_responsiveness(&mut self) {
        let now_unresponsive = self.unresponsive_apps();
        if now_unresponsive == self.hung_announced {
            return;
        }
        for app_id in now_unresponsive.difference(&self.hung_announced) {
            self.docks.unresponsive(app_id, true);
        }
        for app_id in self.hung_announced.difference(&now_unresponsive) {
            self.docks.unresponsive(app_id, false);
        }
        self.hung_announced = now_unresponsive;
    }
}
