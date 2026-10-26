//! The overview as the dock process shows it: the layout of the windows the server lists (the
//! reference packing in `cupertino-overview`, which the dock answers with one `overview_frame` per
//! window), the workspaces bar with its add slot, remove button and drag-reorder, the selection
//! highlight, the titles, and what a click or key asks the server for. The server owns the
//! open/close transition and the windows themselves; this model is always at the final layout.

use cupertino_overview::bar::{BarHit, BarLayout};
use cupertino_overview::{
    Highlight, Key, Overview, OverviewAction, OverviewConfig, OverviewMode, OverviewWindow,
    PlacedWindow,
};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::{AppId, WindowId, WorkspaceId};

/// Wayland layer-shell namespace of the overview's surface.
pub const NAMESPACE: &str = "cupertino-dock-overview";

/// The chrome fades out over the transition's duration when the overview leaves (0.25 s,
/// the same value the server's transition uses).
pub const HIDE_DURATION: f64 = 0.25;

/// A press on a workspace slot becomes a reorder drag once the pointer moved this far (approximate).
pub const DRAG_SLACK: f64 = 4.0;

/// Diameter of the remove button on the hovered workspace slot (approximate).
pub const REMOVE_BUTTON: f64 = 18.0;

/// Which windows the server put in the overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every window of the current workspace, with the workspaces bar.
    All,
    /// One application's windows, minimized ones in a lower row; no bar.
    App,
}

/// A window of the overview as the dock knows it: the server's id and desktop frame (output
/// coordinates), the title and minimized state from the window feed.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowEntry {
    pub id: u32,
    pub frame: Rect,
    pub title: String,
    pub minimized: bool,
}

/// A workspace of the bar, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: u32,
    pub current: bool,
}

/// What the pointer or a key asked the server for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiAction {
    None,
    Select(u32),
    Close,
    SwitchWorkspace(u32),
    AddWorkspace,
    RemoveWorkspace(u32),
    /// Move the workspace to this position in the bar.
    MoveWorkspace {
        id: u32,
        index: usize,
    },
}

/// A press on a workspace slot: a release in place switches, a release after moving reorders.
#[derive(Clone, Copy, Debug)]
struct Drag {
    id: u32,
    origin: Point,
    pointer: Point,
    moved: bool,
}

pub struct OverviewUi {
    overview: Overview,
    mode: Mode,
    workspaces: Vec<Workspace>,
    /// When the server said the overview leaves: the chrome fades from then.
    closed_at: Option<f64>,
    drag: Option<Drag>,
    hovered_slot: Option<u32>,
}

fn entries(windows: &[WindowEntry]) -> Vec<OverviewWindow> {
    windows
        .iter()
        .map(|w| {
            // `from` is the server's business (it animates); the layout only needs the size
            if w.minimized {
                OverviewWindow::minimized(
                    WindowId(u64::from(w.id)),
                    AppId(0),
                    &w.title,
                    w.frame,
                    w.frame,
                )
            } else {
                OverviewWindow::new(WindowId(u64::from(w.id)), AppId(0), &w.title, w.frame)
            }
        })
        .collect()
}

fn ids(workspaces: &[Workspace]) -> (Vec<WorkspaceId>, Option<WorkspaceId>) {
    let ids: Vec<WorkspaceId> = workspaces
        .iter()
        .map(|w| WorkspaceId(u64::from(w.id)))
        .collect();
    let current = workspaces
        .iter()
        .find(|w| w.current)
        .map(|w| WorkspaceId(u64::from(w.id)));
    (ids, current)
}

impl OverviewUi {
    /// The overview the server opened over `area` (output coordinates).
    pub fn new(
        mode: Mode,
        area: Rect,
        windows: &[WindowEntry],
        workspaces: Vec<Workspace>,
        now: f64,
    ) -> Self {
        let mut cfg = OverviewConfig::default();
        // the bar stays expanded (thumbnail slots with the add slot) for the whole overview; the
        // reference collapses it to names until the pointer reaches it (approximate)
        cfg.bar.expand_on_hover = false;
        let model_mode = match mode {
            Mode::All => OverviewMode::Overview,
            Mode::App => OverviewMode::AppOverview(AppId(0)),
        };
        let mut overview = Overview::new(cfg, area, model_mode, entries(windows));
        // the server drives the transition: here the layout is final from the first frame
        overview.transition.progress.set_immediately(1.0);
        overview.bar.set_expanded(true, now - 10.0);
        let (list, current) = ids(&workspaces);
        overview.set_workspaces(&list, current);
        Self {
            overview,
            mode,
            workspaces,
            closed_at: None,
            drag: None,
            hovered_slot: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The server sent the window set again: windows reflow with the model's spring.
    pub fn set_windows(&mut self, windows: &[WindowEntry], now: f64) {
        self.overview.set_windows(entries(windows), now);
    }

    pub fn set_workspaces(&mut self, workspaces: Vec<Workspace>) {
        let (list, current) = ids(&workspaces);
        self.overview.set_workspaces(&list, current);
        self.workspaces = workspaces;
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    /// The final frame of every window: what the server is told with `overview_frame`.
    pub fn frames(&self) -> Vec<(u32, Rect)> {
        self.overview
            .shown_windows()
            .iter()
            .zip(self.overview.layout_rects())
            .map(|(w, r)| (w.id.0 as u32, *r))
            .collect()
    }

    /// The windows at `now` (the reflow spring may still be moving them), with titles and selection.
    pub fn placed(&self, now: f64) -> Vec<PlacedWindow> {
        self.overview.placed(now)
    }

    /// The title of window `id`, from the window feed.
    pub fn title(&self, id: u32) -> Option<&str> {
        self.overview
            .shown_windows()
            .into_iter()
            .find(|w| w.id.0 as u32 == id)
            .map(|w| w.title.as_str())
    }

    /// The selected window (hover or keyboard), if any.
    pub fn selection(&self) -> Option<u32> {
        self.overview.selection().map(|w| w.0 as u32)
    }

    pub fn highlight(&self, now: f64) -> Option<Highlight> {
        self.overview.highlight(now)
    }

    /// The bar's layout; empty in the application mode.
    pub fn bar(&self, now: f64) -> Option<BarLayout> {
        (self.mode == Mode::All).then(|| self.overview.bar.layout(now))
    }

    pub fn hovered_slot(&self) -> Option<u32> {
        self.hovered_slot
    }

    /// The slot being dragged and the pointer's offset from where the drag started.
    pub fn drag(&self) -> Option<(u32, Point)> {
        self.drag.filter(|d| d.moved).map(|d| {
            (
                d.id,
                Point::new(d.pointer.x - d.origin.x, d.pointer.y - d.origin.y),
            )
        })
    }

    /// The remove button of a slot: a circle on its top-left corner.
    pub fn remove_button(slot: &Rect) -> Rect {
        Rect::new(
            slot.x - REMOVE_BUTTON / 2.0,
            slot.y - REMOVE_BUTTON / 2.0,
            REMOVE_BUTTON,
            REMOVE_BUTTON,
        )
    }

    fn slot_hit(&self, p: Point, now: f64) -> Option<BarHit> {
        (self.mode == Mode::All)
            .then(|| self.overview.bar.hit_test(p, now))
            .flatten()
    }

    /// Pointer motion: hover selection, the hovered slot, and a drag in progress.
    pub fn pointer_moved(&mut self, p: Option<Point>, now: f64) {
        self.overview.pointer_moved(p, now);
        self.hovered_slot = p.and_then(|p| match self.slot_hit(p, now) {
            Some(BarHit::Workspace(id)) => Some(id.0 as u32),
            _ => None,
        });
        if let (Some(d), Some(p)) = (self.drag.as_mut(), p) {
            d.pointer = p;
            if (p.x - d.origin.x).abs() > DRAG_SLACK || (p.y - d.origin.y).abs() > DRAG_SLACK {
                d.moved = true;
            }
        }
    }

    /// A left press: the remove button, the add slot, a workspace slot (acted on at the release) or
    /// a window (selected) or the background (leaves).
    pub fn press(&mut self, p: Point, now: f64) -> UiAction {
        if self.is_closing() {
            return UiAction::None;
        }
        if let Some(bar) = self.bar(now) {
            if let Some(slot) = bar.slots.iter().find(|s| {
                Some(s.id.0 as u32) == self.hovered_slot && Self::remove_button(&s.rect).contains(p)
            }) {
                return UiAction::RemoveWorkspace(slot.id.0 as u32);
            }
        }
        match self.slot_hit(p, now) {
            Some(BarHit::Add) => return UiAction::AddWorkspace,
            Some(BarHit::Workspace(id)) => {
                self.drag = Some(Drag {
                    id: id.0 as u32,
                    origin: p,
                    pointer: p,
                    moved: false,
                });
                return UiAction::None;
            }
            None => {}
        }
        match self.overview.click(p, now) {
            OverviewAction::Select(w) => UiAction::Select(w.0 as u32),
            OverviewAction::Close => UiAction::Close,
            OverviewAction::None
            | OverviewAction::SwitchWorkspace(_)
            | OverviewAction::AddWorkspace => UiAction::None,
        }
    }

    /// A left release: ends a slot press as a switch (in place) or a reorder (after a drag, to the
    /// position of the slot under the pointer, or the end past the last one).
    pub fn release(&mut self, p: Point, now: f64) -> UiAction {
        let Some(d) = self.drag.take() else {
            return UiAction::None;
        };
        if !d.moved {
            return UiAction::SwitchWorkspace(d.id);
        }
        let Some(bar) = self.bar(now) else {
            return UiAction::None;
        };
        let index = bar
            .slots
            .iter()
            .position(|s| p.x < s.rect.right())
            .unwrap_or(bar.slots.len().saturating_sub(1));
        let from = self.workspaces.iter().position(|w| w.id == d.id);
        if from == Some(index) {
            return UiAction::None;
        }
        UiAction::MoveWorkspace { id: d.id, index }
    }

    /// A key: navigation moves the selection, Return or Space selects, Escape leaves.
    pub fn key(&mut self, key: Key, now: f64) -> UiAction {
        if self.is_closing() {
            return UiAction::None;
        }
        match self.overview.key(key, now) {
            OverviewAction::Select(w) => UiAction::Select(w.0 as u32),
            OverviewAction::Close => UiAction::Close,
            OverviewAction::None
            | OverviewAction::SwitchWorkspace(_)
            | OverviewAction::AddWorkspace => UiAction::None,
        }
    }

    /// The server said the overview leaves: the chrome fades from `now`.
    pub fn close(&mut self, now: f64) {
        if self.closed_at.is_none() {
            self.closed_at = Some(now);
            self.drag = None;
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closed_at.is_some()
    }

    /// Opacity of the chrome: 1 while open, fading to 0 over [`HIDE_DURATION`] after `close`.
    pub fn alpha(&self, now: f64) -> f64 {
        match self.closed_at {
            None => 1.0,
            Some(t) => (1.0 - (now - t) / HIDE_DURATION).clamp(0.0, 1.0),
        }
    }

    pub fn is_gone(&self, now: f64) -> bool {
        self.closed_at.is_some_and(|t| now - t >= HIDE_DURATION)
    }

    /// Whether another frame is wanted: a reflow, the highlight or the fade is moving.
    pub fn is_animating(&self, now: f64) -> bool {
        self.closed_at.is_some() || self.overview.is_animating(now) || self.drag().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 24.0,
        w: 1000.0,
        h: 600.0,
    };

    fn windows() -> Vec<WindowEntry> {
        vec![
            WindowEntry {
                id: 1,
                frame: Rect::new(100.0, 100.0, 600.0, 400.0),
                title: "one".into(),
                minimized: false,
            },
            WindowEntry {
                id: 2,
                frame: Rect::new(200.0, 150.0, 600.0, 400.0),
                title: "two".into(),
                minimized: false,
            },
        ]
    }

    fn workspaces() -> Vec<Workspace> {
        vec![
            Workspace {
                id: 10,
                current: true,
            },
            Workspace {
                id: 11,
                current: false,
            },
        ]
    }

    #[test]
    fn windows_get_frames_apart_from_each_other_below_the_bar() {
        let ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let frames = ui.frames();
        assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1, 2]);
        assert!(!frames[0].1.intersects(&frames[1].1), "{frames:?}");
        let bar = ui.bar(0.0).unwrap();
        assert_eq!(bar.slots.len(), 2);
        assert!(bar.add.is_some(), "the bar is expanded with its add slot");
        for (_, f) in &frames {
            assert!(
                f.y >= bar.rect.bottom(),
                "windows are laid out below the bar: {f:?} vs {:?}",
                bar.rect
            );
            assert!(f.right() <= AREA.right() && f.bottom() <= AREA.bottom());
        }
        let app = OverviewUi::new(Mode::App, AREA, &windows(), Vec::new(), 0.0);
        assert!(app.bar(0.0).is_none(), "the application mode has no bar");
        assert_eq!(app.frames().len(), 2);
    }

    #[test]
    fn a_press_on_a_window_selects_and_on_the_background_leaves() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let (id, frame) = ui.frames()[1];
        assert_eq!(
            ui.press(Point::new(frame.cx(), frame.cy()), 1.0),
            UiAction::Select(id)
        );
        assert_eq!(
            ui.press(Point::new(AREA.x + 1.0, AREA.bottom() - 1.0), 1.0),
            UiAction::Close
        );
    }

    #[test]
    fn the_bar_switches_adds_removes_and_reorders_workspaces() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let bar = ui.bar(1.0).unwrap();
        let second = bar.slots[1].rect;
        let centre = Point::new(second.cx(), second.cy());
        // press and release in place: a switch
        assert_eq!(ui.press(centre, 1.0), UiAction::None);
        ui.pointer_moved(Some(centre), 1.0);
        assert_eq!(ui.release(centre, 1.0), UiAction::SwitchWorkspace(11));
        // the add slot
        let add = bar.add.unwrap();
        assert_eq!(
            ui.press(Point::new(add.cx(), add.cy()), 1.0),
            UiAction::AddWorkspace
        );
        // the remove button shows on the hovered slot only
        ui.pointer_moved(Some(centre), 1.0);
        assert_eq!(ui.hovered_slot(), Some(11));
        let button = OverviewUi::remove_button(&second);
        assert_eq!(
            ui.press(Point::new(button.cx(), button.cy()), 1.0),
            UiAction::RemoveWorkspace(11)
        );
        ui.pointer_moved(Some(Point::new(AREA.cx(), AREA.bottom() - 1.0)), 1.0);
        assert_eq!(ui.hovered_slot(), None);
        // a drag of the second slot to before the first reorders
        ui.pointer_moved(Some(centre), 1.0);
        assert_eq!(ui.press(centre, 1.0), UiAction::None);
        let first = bar.slots[0].rect;
        let target = Point::new(first.x + 2.0, first.cy());
        ui.pointer_moved(Some(target), 1.1);
        assert_eq!(
            ui.drag().map(|d| d.0),
            Some(11),
            "past the slack the slot is dragged"
        );
        assert_eq!(
            ui.release(target, 1.2),
            UiAction::MoveWorkspace { id: 11, index: 0 }
        );
        assert!(ui.drag().is_none());
    }

    #[test]
    fn keys_walk_the_windows_and_return_selects() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        assert_eq!(
            ui.key(Key::Activate, 1.0),
            UiAction::None,
            "nothing selected yet"
        );
        assert_eq!(ui.key(Key::Next, 1.0), UiAction::None);
        let first = ui.selection().expect("the first window is selected");
        assert_eq!(
            ui.highlight(1.5).map(|h| h.window.0 as u32),
            Some(first),
            "and highlighted once the fade ran"
        );
        assert_eq!(ui.key(Key::Right, 1.5), UiAction::None);
        let second = ui.selection().unwrap();
        assert_ne!(first, second);
        assert_eq!(ui.key(Key::Activate, 1.5), UiAction::Select(second));
        assert_eq!(ui.key(Key::Cancel, 1.5), UiAction::Close);
    }

    #[test]
    fn closing_fades_the_chrome_and_ignores_input() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        assert_eq!(ui.alpha(5.0), 1.0);
        ui.close(5.0);
        assert!(ui.is_closing() && ui.is_animating(5.1));
        assert!((ui.alpha(5.0 + HIDE_DURATION / 2.0) - 0.5).abs() < 1e-9);
        assert!(!ui.is_gone(5.2));
        assert!(ui.is_gone(5.0 + HIDE_DURATION));
        assert_eq!(ui.alpha(6.0), 0.0);
        let (_, frame) = ui.frames()[0];
        assert_eq!(
            ui.press(Point::new(frame.cx(), frame.cy()), 5.1),
            UiAction::None
        );
        assert_eq!(ui.key(Key::Cancel, 5.1), UiAction::None);
    }

    #[test]
    fn a_new_window_set_reflows_and_keeps_the_ids_in_order() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let mut more = windows();
        more.push(WindowEntry {
            id: 3,
            frame: Rect::new(0.0, 24.0, 300.0, 200.0),
            title: "three".into(),
            minimized: false,
        });
        ui.set_windows(&more, 1.0);
        assert_eq!(
            ui.frames().iter().map(|f| f.0).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(ui.is_animating(1.01), "the reflow spring runs");
        ui.set_workspaces(vec![
            Workspace {
                id: 10,
                current: false,
            },
            Workspace {
                id: 11,
                current: true,
            },
            Workspace {
                id: 12,
                current: false,
            },
        ]);
        let bar = ui.bar(2.0).unwrap();
        assert_eq!(bar.slots.len(), 3);
        assert!(bar.slots[1].current);
    }
}
