//! The stage policy of one output: the server's groups of window events in, placement and
//! activation requests out. The `cupertino-stage` model decides; the controller keeps what the
//! server was told so that each group yields only the placements that changed.

use std::collections::{BTreeMap, HashMap};

use cupertino_stage::{Placement, Stage, StageConfig};
use cupertino_wm::geometry::Rect;
use cupertino_wm::ids::{AppId, WindowId};

/// An arranged window as the server describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// The server's id: stable for the window's lifetime.
    pub id: u32,
    /// Application id; windows of one application share a group.
    pub app_id: String,
    /// Frame in global logical coordinates.
    pub frame: Rect,
    /// The key window of the session.
    pub active: bool,
}

/// One event of a group, in the order the server sent it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    UsableArea(Rect),
    /// A window joined the arranged set or changed.
    Window(Window),
    /// A window left the arranged set.
    WindowClosed(u32),
    /// The stage shortcut was pressed.
    ToggleStage,
    /// The pointer pressed the strip thumbnail of this window.
    StripPressed(u32),
}

/// What to send the server, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Place { id: u32, placement: Placement },
    /// Apply the placements sent since the last commit together.
    Commit,
    /// Focus and raise the window.
    Activate(u32),
}

struct Known {
    app: AppId,
    frame: Rect,
}

pub struct Controller {
    stage: Stage,
    /// Whether the stage is on: the setting at start, then the shortcut.
    enabled: bool,
    /// The first group has been applied.
    started: bool,
    windows: BTreeMap<u32, Known>,
    /// Most recently active first.
    recency: Vec<u32>,
    active: Option<u32>,
    apps: HashMap<String, AppId>,
    /// Placements the server holds that are not the stage; a window missing here is on the stage.
    sent: BTreeMap<u32, Placement>,
}

impl Controller {
    /// `enabled` is the stage setting at start (`windowmanager.conf`).
    pub fn new(cfg: StageConfig, enabled: bool) -> Self {
        Self {
            stage: Stage::new(cfg, Rect::new(0.0, 0.0, 0.0, 0.0)),
            enabled,
            started: false,
            windows: BTreeMap::new(),
            recency: Vec::new(),
            active: None,
            apps: HashMap::new(),
            sent: BTreeMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn app(&mut self, app_id: &str) -> AppId {
        let next = AppId(self.apps.len() as u64 + 1);
        *self.apps.entry(app_id.to_string()).or_insert(next)
    }

    /// Known windows, most recently active first, for building the stage.
    fn by_recency(&self) -> Vec<(WindowId, AppId, Rect)> {
        self.recency.iter().map(|id| (WindowId(u64::from(*id)), self.windows[id].app, self.windows[id].frame)).collect()
    }

    /// Apply one group (`now` in seconds of a monotonic clock) and return the requests it calls for.
    pub fn apply(&mut self, group: &[Change], now: f64) -> Vec<Request> {
        let mut activate = None;
        for change in group {
            match change {
                Change::UsableArea(area) => self.stage.set_usable(*area, now),
                Change::Window(w) => self.window(w, now),
                Change::WindowClosed(id) => {
                    self.windows.remove(id);
                    self.recency.retain(|x| x != id);
                    self.sent.remove(id);
                    if self.active == Some(*id) {
                        self.active = None;
                    }
                    self.stage.window_closed(WindowId(u64::from(*id)), now);
                }
                Change::ToggleStage => {
                    self.enabled = !self.enabled;
                    if self.started {
                        let windows = self.by_recency();
                        self.stage.set_enabled(self.enabled, &windows, now);
                    }
                }
                Change::StripPressed(id) => {
                    let Some(group) = self.stage.group_of(WindowId(u64::from(*id))) else { continue };
                    self.stage.activate_group(group, now);
                    activate = self.stage.group_windows(group).first().map(|w| w.0 as u32);
                }
            }
        }
        if !self.started {
            self.started = true;
            if self.enabled {
                let windows = self.by_recency();
                self.stage.set_enabled(true, &windows, now);
            }
        }
        let mut requests = self.placement_requests();
        requests.extend(activate.map(Request::Activate));
        requests
    }

    fn window(&mut self, w: &Window, now: f64) {
        let id = WindowId(u64::from(w.id));
        let app = self.app(&w.app_id);
        match self.windows.get_mut(&w.id) {
            Some(known) => {
                if known.frame != w.frame {
                    known.frame = w.frame;
                    self.stage.set_window_frame(id, w.frame);
                }
            }
            None => {
                self.windows.insert(w.id, Known { app, frame: w.frame });
                self.recency.insert(0, w.id);
                // the first group builds the stage at its end, from the whole list
                if self.started {
                    self.stage.window_opened(id, app, w.frame, now);
                }
            }
        }
        if w.active && self.active != Some(w.id) {
            self.active = Some(w.id);
            self.recency.retain(|x| *x != w.id);
            self.recency.insert(0, w.id);
            if self.started {
                self.stage.window_activated(id, now);
            }
        } else if !w.active && self.active == Some(w.id) {
            self.active = None;
        }
    }

    /// The placements that differ from what the server holds, then a commit; nothing when none
    /// changed.
    fn placement_requests(&mut self) -> Vec<Request> {
        let wanted: BTreeMap<u32, Placement> = self.stage.placements().into_iter().map(|(w, p)| (w.0 as u32, p)).filter(|(_, p)| *p != Placement::Stage).collect();
        let mut requests = Vec::new();
        for (id, placement) in &wanted {
            if self.sent.get(id) != Some(placement) {
                requests.push(Request::Place { id: *id, placement: *placement });
            }
        }
        for id in self.sent.keys().filter(|id| !wanted.contains_key(id)) {
            requests.push(Request::Place { id: *id, placement: Placement::Stage });
        }
        self.sent = wanted;
        if !requests.is_empty() {
            requests.push(Request::Commit);
        }
        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USABLE: Rect = Rect::new(0.0, 24.0, 1280.0, 776.0);

    fn window(id: u32, app: &str, active: bool) -> Change {
        Change::Window(Window { id, app_id: app.into(), frame: Rect::new(400.0, 100.0 + f64::from(id), 600.0, 400.0), active })
    }

    fn placed(requests: &[Request]) -> BTreeMap<u32, Placement> {
        requests.iter().filter_map(|r| if let Request::Place { id, placement } = r { Some((*id, *placement)) } else { None }).collect()
    }

    fn strip(p: Option<&Placement>) -> bool {
        matches!(p, Some(Placement::Strip(_)))
    }

    #[test]
    fn with_the_stage_off_nothing_is_sent() {
        let mut c = Controller::new(StageConfig::default(), false);
        assert!(c.apply(&[Change::UsableArea(USABLE), window(1, "a", false), window(2, "b", true)], 0.0).is_empty());
        assert!(c.apply(&[window(3, "c", true)], 1.0).is_empty());
        assert!(c.apply(&[Change::StripPressed(1)], 2.0).is_empty(), "no strip while off");
    }

    #[test]
    fn turning_the_stage_on_puts_the_other_apps_in_the_strip_and_off_brings_them_back() {
        let mut c = Controller::new(StageConfig::default(), false);
        c.apply(&[Change::UsableArea(USABLE), window(1, "a", false), window(2, "b", true)], 0.0);
        let on = c.apply(&[Change::ToggleStage], 1.0);
        assert_eq!(on.last(), Some(&Request::Commit));
        let p = placed(&on);
        assert!(strip(p.get(&1)), "the inactive window goes to the strip: {on:?}");
        assert!(!p.contains_key(&2), "the active window stays on the stage");
        let off = c.apply(&[Change::ToggleStage], 2.0);
        assert_eq!(off, vec![Request::Place { id: 1, placement: Placement::Stage }, Request::Commit]);
    }

    #[test]
    fn a_strip_press_swaps_the_groups_and_activates_the_window() {
        let mut c = Controller::new(StageConfig::default(), true);
        let first = c.apply(&[Change::UsableArea(USABLE), window(1, "a", false), window(2, "b", true)], 0.0);
        assert!(strip(placed(&first).get(&1)));
        let swap = c.apply(&[Change::StripPressed(1)], 1.0);
        let p = placed(&swap);
        assert_eq!(p.get(&1), Some(&Placement::Stage), "window 1 leaves the strip");
        assert!(strip(p.get(&2)), "window 2 takes its place");
        assert_eq!(swap.last(), Some(&Request::Activate(1)));
        // the server reports the focus change it made: nothing more to do
        assert!(c.apply(&[window(1, "a", true), window(2, "b", false)], 1.1).is_empty());
    }

    #[test]
    fn focus_from_elsewhere_and_new_windows_take_the_stage() {
        let mut c = Controller::new(StageConfig::default(), true);
        c.apply(&[Change::UsableArea(USABLE), window(1, "a", false), window(2, "b", true)], 0.0);
        // the switcher focused window 1
        let p = placed(&c.apply(&[window(1, "a", true), window(2, "b", false)], 1.0));
        assert_eq!(p.get(&1), Some(&Placement::Stage));
        assert!(strip(p.get(&2)));
        // a window of a third application opens and takes the stage
        let p = placed(&c.apply(&[window(3, "c", true), window(1, "a", false)], 2.0));
        assert!(strip(p.get(&1)) && !p.contains_key(&3));
        // a second window of the stage's application joins its group without a swap
        assert!(placed(&c.apply(&[window(4, "c", true), window(3, "c", false)], 3.0)).is_empty());
    }

    #[test]
    fn a_closed_window_is_forgotten_without_a_request() {
        let mut c = Controller::new(StageConfig::default(), true);
        c.apply(&[Change::UsableArea(USABLE), window(1, "a", false), window(2, "b", false), window(3, "c", true)], 0.0);
        // window 1 had the lower strip slot: closing window 2 moves it up
        let closed = c.apply(&[Change::WindowClosed(2)], 1.0);
        let p = placed(&closed);
        assert!(!p.contains_key(&2), "the server drops a closed window's placement itself");
        assert!(strip(p.get(&1)));
        assert!(c.apply(&[Change::StripPressed(2)], 2.0).is_empty(), "a press on a window that just closed");
    }
}
