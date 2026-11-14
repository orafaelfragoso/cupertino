//! Hung applications: every client that owns a mapped toplevel is pinged (`xdg_wm_base.ping`) once a second while the
//! person uses the desktop, and one that has not answered a ping for two seconds is unresponsive until it answers. A ping
//! goes out only after input that came since the last one: on a desktop nobody touches, no event waits for an application
//! to handle it, so nothing shows whether it hangs, and neither the server nor the applications wake for pings.
//! The first input after a rest pings again at once, and pinging goes on until a ping has gone out a second after the
//! last input. The timing is [`Liveness`], pure and driven by the
//! model clock; [`Pings`] holds one per client and does the sending. What the person sees of it, the busy cursor over the
//! windows and "Not Responding" in the dock, is built on [`Pings::unresponsive`].

use smithay::utils::Serial;
use smithay::wayland::shell::xdg::ShellClient;

/// Time between a pong (or the first sight of a client) and the next ping, in seconds.
pub const PING_INTERVAL: f64 = 1.0;
/// A ping unanswered for this long marks the client unresponsive, in seconds.
pub const PONG_TIMEOUT: f64 = 2.0;

/// Whether one client answers its pings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Liveness {
    /// When the outstanding ping was sent.
    outstanding: Option<f64>,
    /// When the last ping was sent.
    last_ping: Option<f64>,
    unresponsive: bool,
}

impl Liveness {
    /// A ping is due: none is outstanding, the interval has passed since the last and the last input still wants one
    /// (`input`: the last input's time).
    pub fn ping_due(&self, now: f64, input: Option<f64>) -> bool {
        self.outstanding.is_none()
            && self.last_ping.is_none_or(|t| now - t >= PING_INTERVAL)
            && self.input_since_ping(input)
    }

    /// Whether the last input still wants a ping: there was input and no ping has gone out [`PING_INTERVAL`] or more
    /// after it. The ping sent in the loop pass that handled the input reads the clock a moment after the input did,
    /// and the client may answer it before it handles the input; so pinging goes on until a ping has gone out an
    /// interval after the last input, the one that shows whether handling the input hung the client: one more ping per
    /// rest.
    fn input_since_ping(&self, input: Option<f64>) -> bool {
        input.is_some_and(|i| self.last_ping.is_none_or(|t| t < i + PING_INTERVAL))
    }

    pub fn pinged(&mut self, now: f64) {
        self.outstanding = Some(now);
        self.last_ping = Some(now);
    }

    /// The pong came: true when the client was unresponsive until now.
    pub fn ponged(&mut self) -> bool {
        self.outstanding = None;
        std::mem::take(&mut self.unresponsive)
    }

    /// Look at the clock: true when the client just became unresponsive.
    pub fn check(&mut self, now: f64) -> bool {
        match self.outstanding {
            Some(sent) if now - sent >= PONG_TIMEOUT && !self.unresponsive => {
                self.unresponsive = true;
                true
            }
            _ => false,
        }
    }

    pub fn is_unresponsive(&self) -> bool {
        self.unresponsive
    }

    /// When the clock next matters to this client: its next ping, or the moment an outstanding one makes it
    /// unresponsive. `None` for a client never pinged (the next tick pings it), one the last input wants no more pings
    /// for (input wakes the loop and pings it) or one already unresponsive.
    pub fn next_deadline(&self, input: Option<f64>) -> Option<f64> {
        match (self.outstanding, self.last_ping) {
            (Some(sent), _) => (!self.unresponsive).then_some(sent + PONG_TIMEOUT),
            (None, last) => last
                .filter(|_| self.input_since_ping(input))
                .map(|t| t + PING_INTERVAL),
        }
    }

    /// A ping went out and has not been answered yet.
    pub fn awaiting_pong(&self) -> bool {
        self.outstanding.is_some()
    }
}

struct Watched {
    client: ShellClient,
    liveness: Liveness,
}

/// What a tick or a pong changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed(pub bool);

/// The clients being pinged.
#[derive(Default)]
pub struct Pings {
    watched: Vec<Watched>,
    /// When the last input came (the model clock): pings go out only after it.
    input: Option<f64>,
}

impl Pings {
    /// Input came at `now` (any key, pointer or touch event): the clients are pinged again.
    pub fn input(&mut self, now: f64) {
        self.input = Some(now);
    }

    /// Ping what is due among `clients` (those that own a mapped toplevel; duplicates are one) and forget the others.
    /// True when a client became unresponsive.
    pub fn tick(
        &mut self,
        now: f64,
        clients: impl IntoIterator<Item = ShellClient>,
        mut next_serial: impl FnMut() -> Serial,
    ) -> Changed {
        let mut wanted: Vec<ShellClient> = Vec::new();
        for client in clients {
            if client.alive() && !wanted.contains(&client) {
                wanted.push(client);
            }
        }
        self.watched.retain(|w| wanted.contains(&w.client));
        for client in wanted {
            if !self.watched.iter().any(|w| w.client == client) {
                self.watched.push(Watched {
                    client,
                    liveness: Liveness::default(),
                });
            }
        }
        let mut changed = false;
        for w in &mut self.watched {
            if w.liveness.ping_due(now, self.input) && w.client.send_ping(next_serial()).is_ok() {
                w.liveness.pinged(now);
            }
            changed |= w.liveness.check(now);
        }
        Changed(changed)
    }

    /// The client answered. True when it was unresponsive.
    pub fn pong(&mut self, client: &ShellClient) -> Changed {
        Changed(
            self.watched
                .iter_mut()
                .find(|w| &w.client == client)
                .is_some_and(|w| w.liveness.ponged()),
        )
    }

    pub fn is_unresponsive(&self, client: &ShellClient) -> bool {
        self.watched
            .iter()
            .any(|w| &w.client == client && w.liveness.is_unresponsive())
    }

    /// Whether any client has a ping it has not answered yet. A model clock that jumps while this holds counts the jump
    /// as time the client took to answer.
    pub fn awaiting_pong(&self) -> bool {
        self.watched.iter().any(|w| w.liveness.awaiting_pong())
    }

    /// The earliest moment the clock next matters to a watched client.
    pub fn next_deadline(&self) -> Option<f64> {
        self.watched
            .iter()
            .filter_map(|w| w.liveness.next_deadline(self.input))
            .reduce(f64::min)
    }

    /// Whether any client is unresponsive.
    pub fn any_unresponsive(&self) -> bool {
        self.watched.iter().any(|w| w.liveness.is_unresponsive())
    }
}

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
}

impl crate::state::Cupertino {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_next_matters_at_the_next_ping_or_the_pong_timeout() {
        let mut l = Liveness::default();
        let input = Some(10.0);
        assert_eq!(
            l.next_deadline(input),
            None,
            "the next tick pings a new client"
        );
        l.pinged(10.0);
        assert_eq!(l.next_deadline(input), Some(10.0 + PONG_TIMEOUT));
        l.ponged();
        assert_eq!(l.next_deadline(input), Some(10.0 + PING_INTERVAL));
        l.pinged(11.0);
        assert!(l.check(11.0 + PONG_TIMEOUT));
        assert_eq!(
            l.next_deadline(input),
            None,
            "an unresponsive client waits for its pong"
        );
    }

    #[test]
    fn a_client_that_answers_is_pinged_every_second_and_never_unresponsive() {
        let mut l = Liveness::default();
        // the person keeps using the desktop
        let input = |now: f64| Some(now);
        assert!(l.ping_due(0.0, input(0.0)), "the first ping is due at once");
        assert!(!l.awaiting_pong());
        l.pinged(0.0);
        assert!(l.awaiting_pong());
        assert!(!l.ping_due(0.5, input(0.5)), "one is outstanding");
        assert!(!l.check(1.9));
        assert!(!l.ponged());
        assert!(!l.awaiting_pong());
        assert!(!l.ping_due(0.9, input(0.9)), "the interval has not passed");
        assert!(l.ping_due(1.0, input(1.0)));
        l.pinged(1.0);
        assert!(!l.check(2.9) && !l.is_unresponsive());
        assert!(!l.ponged());
    }

    #[test]
    fn two_seconds_without_a_pong_make_it_unresponsive_once_and_a_pong_clears_it() {
        let mut l = Liveness::default();
        l.pinged(10.0);
        assert!(!l.check(11.99));
        assert!(l.check(12.0), "2 s after the ping");
        assert!(!l.check(13.0), "reported once");
        assert!(l.is_unresponsive());
        assert!(
            !l.ping_due(20.0, Some(20.0)),
            "the ping is still outstanding: no second one"
        );
        assert!(l.ponged(), "it answered at last");
        assert!(!l.is_unresponsive());
        assert!(!l.ponged());
        assert!(l.ping_due(20.0, Some(20.0)), "pinging goes on");
    }

    #[test]
    fn a_desktop_nobody_touches_pings_no_one_and_the_first_input_pings_at_once() {
        let mut l = Liveness::default();
        assert!(!l.ping_due(5.0, None), "no input yet");
        assert!(l.ping_due(5.0, Some(4.0)));
        l.pinged(5.0);
        l.ponged();
        // the last input came before the ping: the desktop rests
        for now in [6.0, 10.0, 600.0] {
            assert!(!l.ping_due(now, Some(4.0)), "{now}");
        }
        assert_eq!(l.next_deadline(Some(4.0)), None, "nothing to wake for");
        // the person comes back
        assert!(l.ping_due(600.0, Some(600.0)));
        assert_eq!(l.next_deadline(Some(600.0)), Some(5.0 + PING_INTERVAL));
        l.pinged(600.0);
        assert_eq!(l.next_deadline(Some(600.0)), Some(600.0 + PONG_TIMEOUT));
        // an outstanding ping times out whether or not input comes
        assert!(l.check(600.0 + PONG_TIMEOUT));
    }

    #[test]
    fn a_client_that_hangs_on_the_first_input_after_a_rest_is_found_unresponsive() {
        let mut l = Liveness::default();
        // the input is read a hair before the ping its loop pass sends, and the client answers that ping at once
        let input = Some(600.0);
        let sent = 600.0 + 1e-6;
        assert!(l.ping_due(sent, input));
        l.pinged(sent);
        l.ponged();
        // then it blocks handling the input: one more ping goes out, and its silence shows
        assert_eq!(l.next_deadline(input), Some(sent + PING_INTERVAL));
        assert!(
            !l.ping_due(sent + 0.5, input),
            "the interval has not passed"
        );
        assert!(l.ping_due(sent + PING_INTERVAL, input));
        l.pinged(sent + PING_INTERVAL);
        assert!(l.check(sent + PING_INTERVAL + PONG_TIMEOUT));
        assert!(l.is_unresponsive());
        // a client that answers that ping too is left alone until the next input
        let mut l = Liveness::default();
        l.pinged(sent);
        l.ponged();
        l.pinged(sent + PING_INTERVAL);
        l.ponged();
        assert!(!l.ping_due(sent + 10.0, input));
        assert_eq!(l.next_deadline(input), None, "the desktop rests");
    }
}
