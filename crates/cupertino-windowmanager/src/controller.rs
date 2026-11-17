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
    Place {
        id: u32,
        placement: Placement,
    },
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
        self.recency
            .iter()
            .map(|id| {
                (
                    WindowId(u64::from(*id)),
                    self.windows[id].app,
                    self.windows[id].frame,
                )
            })
            .collect()
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
                    let Some(group) = self.stage.group_of(WindowId(u64::from(*id))) else {
                        continue;
                    };
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
                self.windows.insert(
                    w.id,
                    Known {
                        app,
                        frame: w.frame,
                    },
                );
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
        let wanted: BTreeMap<u32, Placement> = self
            .stage
            .placements()
            .into_iter()
            .map(|(w, p)| (w.0 as u32, p))
            .filter(|(_, p)| *p != Placement::Stage)
            .collect();
        let mut requests = Vec::new();
        for (id, placement) in &wanted {
            if self.sent.get(id) != Some(placement) {
                requests.push(Request::Place {
                    id: *id,
                    placement: *placement,
                });
            }
        }
        for id in self.sent.keys().filter(|id| !wanted.contains_key(id)) {
            requests.push(Request::Place {
                id: *id,
                placement: Placement::Stage,
            });
        }
        self.sent = wanted;
        if !requests.is_empty() {
            requests.push(Request::Commit);
        }
        requests
    }
}
