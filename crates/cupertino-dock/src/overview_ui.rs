//! The overview as the dock process shows it: the layout of the windows the server lists (the
//! reference packing in `cupertino-overview`, which the dock answers with one `overview_frame` per
//! window), the desktops bar (names, opening on the pointer at the top to show the desktops' pictures, the add button,
//! the remove button after a rest on a picture, drag-reorder), the selection ring and name, dragging a window onto a
//! desktop, and what a click or key asks the server for. The server owns the open/close transition and the windows
//! themselves; this model is always at the final layout.

use cupertino_animation::{AnimatedValue, Curve, SpringParams};
use cupertino_overview::bar::{BarHit, BarLayout};
use cupertino_overview::{
    Highlight, Key, Overview, OverviewAction, OverviewConfig, OverviewMode, OverviewWindow,
    PlacedWindow,
};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::{AppId, WindowId, WorkspaceId};

/// Wayland layer-shell namespace of the overview's surface.
pub const NAMESPACE: &str = "cupertino-dock-overview";

/// Response of the spring the chrome leaves on when the overview closes: critically damped, 0.125 s.
pub const HIDE_RESPONSE: f64 = 0.125;

/// The chrome is gone once its opacity fell below this.
const GONE_ALPHA: f64 = 1e-3;

/// A press on a workspace slot or a window becomes a drag once the pointer moved this far (approximate).
pub const DRAG_SLACK: f64 = 4.0;

/// Diameter of the remove button on a desktop's picture (28 pt).
pub const REMOVE_BUTTON: f64 = 28.0;

/// Seconds the pointer rests on a desktop's picture before its remove button shows (1 s).
pub const REMOVE_HOVER_DELAY: f64 = 1.0;

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
    /// A window was dropped on another desktop of the bar.
    MoveWindow {
        window: u32,
        workspace: u32,
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

/// A press on a window: a release in place picks it, a release on a desktop of the bar after moving moves it there.
#[derive(Clone, Copy, Debug)]
struct WindowDrag {
    id: u32,
    /// Its frame in the layout when the press came.
    frame: Rect,
    origin: Point,
    pointer: Point,
    moved: bool,
}

pub struct OverviewUi {
    overview: Overview,
    mode: Mode,
    workspaces: Vec<Workspace>,
    /// The chrome's opacity: 1 while open, springing to 0 once the server said the overview leaves.
    alpha: AnimatedValue<f64>,
    closing: bool,
    drag: Option<Drag>,
    window_drag: Option<WindowDrag>,
    hovered_slot: Option<u32>,
    /// The desktop picture the pointer rests on, and since when.
    resting: Option<(u32, f64)>,
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
        _now: f64,
    ) -> Self {
        let cfg = OverviewConfig::default();
        let model_mode = match mode {
            Mode::All => OverviewMode::Overview,
            Mode::App => OverviewMode::AppOverview(AppId(0)),
        };
        let mut overview = Overview::new(cfg, area, model_mode, entries(windows));
        // the server drives the transition: here the layout is final from the first frame
        overview.transition.progress.set_immediately(1.0);
        let (list, current) = ids(&workspaces);
        overview.set_workspaces(&list, current);
        Self {
            overview,
            mode,
            workspaces,
            alpha: AnimatedValue::new(1.0),
            closing: false,
            drag: None,
            window_drag: None,
            hovered_slot: None,
            resting: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The server sent the window set again: windows reflow with the model's spring.
    pub fn set_windows(&mut self, windows: &[WindowEntry], now: f64) {
        self.overview.set_windows(entries(windows), now);
        if self
            .window_drag
            .is_some_and(|d| !windows.iter().any(|w| w.id == d.id))
        {
            self.window_drag = None;
        }
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

    /// Changes whenever [`Self::frames`] may have (the band opened or closed, the window set changed).
    pub fn layout_revision(&self) -> u64 {
        self.overview.layout_revision()
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

    /// The window being dragged and the frame it follows the pointer at.
    pub fn window_drag(&self) -> Option<(u32, Rect)> {
        self.window_drag.filter(|d| d.moved).map(|d| {
            (
                d.id,
                d.frame
                    .translate(d.pointer.x - d.origin.x, d.pointer.y - d.origin.y),
            )
        })
    }

    /// The desktop a dragged window would go to if it were dropped now: another one than the current, under the
    /// pointer in the bar.
    pub fn drop_target(&self, now: f64) -> Option<u32> {
        let d = self.window_drag.filter(|d| d.moved)?;
        match self.slot_hit(d.pointer, now) {
            Some(BarHit::Workspace(id)) => {
                let id = id.0 as u32;
                self.workspaces
                    .iter()
                    .any(|w| w.id == id && !w.current)
                    .then_some(id)
            }
            _ => None,
        }
    }

    /// The remove button of a desktop's picture: a circle on its top-left corner.
    pub fn remove_button(slot: &Rect) -> Rect {
        Rect::new(
            slot.x - REMOVE_BUTTON / 2.0,
            slot.y - REMOVE_BUTTON / 2.0,
            REMOVE_BUTTON,
            REMOVE_BUTTON,
        )
    }

    /// The desktop whose remove button shows: the pointer rested on its picture for [`REMOVE_HOVER_DELAY`] in the
    /// open band, and there is another desktop to keep.
    pub fn removable(&self, now: f64) -> Option<u32> {
        let (id, since) = self.resting?;
        (self.workspaces.len() > 1
            && self.drag.is_none()
            && self.window_drag.is_none()
            && now - since >= REMOVE_HOVER_DELAY)
            .then_some(id)
    }

    fn slot_hit(&self, p: Point, now: f64) -> Option<BarHit> {
        (self.mode == Mode::All)
            .then(|| self.overview.bar.hit_test(p, now))
            .flatten()
    }

    /// Pointer motion: hover selection, the hovered slot, the band opening, and a drag in progress.
    pub fn pointer_moved(&mut self, p: Option<Point>, now: f64) {
        if self.window_drag.is_none_or(|d| !d.moved) {
            self.overview.pointer_moved(p, now);
        } else if let Some(p) = p {
            // a dragged window still opens the band when it reaches the top
            let open = self.overview.bar.is_expanded_target()
                || p.y < self.overview.bar.layout(now).rect.bottom();
            self.overview.set_bar_open(open, now);
        }
        self.hovered_slot = p.and_then(|p| match self.slot_hit(p, now) {
            Some(BarHit::Workspace(id)) => Some(id.0 as u32),
            _ => None,
        });
        // the rest on a picture of the open band that shows the remove button
        let on_picture = p.and_then(|p| {
            let bar = self.bar(now)?;
            (bar.expansion > 0.5)
                .then(|| {
                    bar.slots
                        .iter()
                        .find(|s| s.thumb.inset(-REMOVE_BUTTON / 2.0).contains(p))
                })
                .flatten()
                .map(|s| s.id.0 as u32)
        });
        self.resting = match (on_picture, self.resting) {
            (Some(id), Some((was, since))) if id == was => Some((id, since)),
            (Some(id), _) => Some((id, now)),
            (None, _) => None,
        };
        if let (Some(d), Some(p)) = (self.drag.as_mut(), p) {
            d.pointer = p;
            if (p.x - d.origin.x).abs() > DRAG_SLACK || (p.y - d.origin.y).abs() > DRAG_SLACK {
                d.moved = true;
            }
        }
        if let (Some(d), Some(p)) = (self.window_drag.as_mut(), p) {
            d.pointer = p;
            if (p.x - d.origin.x).abs() > DRAG_SLACK || (p.y - d.origin.y).abs() > DRAG_SLACK {
                d.moved = true;
            }
        }
    }

    /// A left press: the remove button, the add button, a workspace slot (acted on at the release), a window (picked
    /// or dragged at the release) or the background (leaves).
    pub fn press(&mut self, p: Point, now: f64) -> UiAction {
        if self.is_closing() {
            return UiAction::None;
        }
        if let (Some(bar), Some(id)) = (self.bar(now), self.removable(now)) {
            if bar
                .slots
                .iter()
                .any(|s| s.id.0 as u32 == id && Self::remove_button(&s.thumb).contains(p))
            {
                return UiAction::RemoveWorkspace(id);
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
            OverviewAction::Select(w) => {
                let id = w.0 as u32;
                let frame = self
                    .frames()
                    .into_iter()
                    .find(|f| f.0 == id)
                    .map_or(Rect::default(), |f| f.1);
                self.window_drag = Some(WindowDrag {
                    id,
                    frame,
                    origin: p,
                    pointer: p,
                    moved: false,
                });
                UiAction::None
            }
            OverviewAction::Close => UiAction::Close,
            OverviewAction::None
            | OverviewAction::SwitchWorkspace(_)
            | OverviewAction::AddWorkspace => UiAction::None,
        }
    }

    /// A left release: ends a slot press as a switch (in place) or a reorder (after a drag, to the
    /// position of the slot under the pointer, or the end past the last one), and a window press as a pick (in
    /// place) or a move to the desktop it was dropped on.
    pub fn release(&mut self, p: Point, now: f64) -> UiAction {
        if let Some(d) = self.window_drag {
            self.pointer_moved(Some(p), now);
            let target = self.drop_target(now);
            self.window_drag = None;
            if !d.moved {
                return UiAction::Select(d.id);
            }
            return target.map_or(UiAction::None, |workspace| UiAction::MoveWindow {
                window: d.id,
                workspace,
            });
        }
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
            .position(|s| p.x < s.name.right())
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

    /// The server said the overview leaves: the chrome springs out from `now`.
    pub fn close(&mut self, now: f64) {
        if !self.closing {
            self.closing = true;
            self.drag = None;
            self.window_drag = None;
            self.alpha.animate_to(
                0.0,
                Curve::Spring(SpringParams::from_ratio(HIDE_RESPONSE, 1.0)),
                now,
            );
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// Opacity of the chrome: 1 while open, springing to 0 after `close`.
    pub fn alpha(&self, now: f64) -> f64 {
        self.alpha.value(now).clamp(0.0, 1.0)
    }

    pub fn is_gone(&self, now: f64) -> bool {
        self.closing && (self.alpha(now) < GONE_ALPHA || !self.alpha.is_animating(now))
    }

    /// Whether another frame is wanted: a reflow, the band, the highlight, a drag or the fade is moving.
    pub fn is_animating(&self, now: f64) -> bool {
        self.closing
            || self.overview.is_animating(now)
            || self.drag().is_some()
            || self.window_drag().is_some()
    }

    /// When the chrome next changes by itself without moving: the remove button showing after the rest.
    pub fn next_change(&self, now: f64) -> Option<f64> {
        let (_, since) = self.resting?;
        let at = since + REMOVE_HOVER_DELAY;
        (at > now && self.workspaces.len() > 1).then_some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
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
    fn windows_get_frames_apart_from_each_other_below_the_closed_band() {
        let ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let frames = ui.frames();
        assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1, 2]);
        assert!(!frames[0].1.intersects(&frames[1].1), "{frames:?}");
        let bar = ui.bar(0.0).unwrap();
        assert_eq!(bar.slots.len(), 2);
        assert_eq!(bar.rect.h, 72.0, "the band starts closed");
        for (_, f) in &frames {
            assert!(f.y >= 72.0 + 39.0 - 1e-9, "{f:?} below the band");
            assert!(f.right() <= AREA.right() && f.bottom() <= AREA.bottom());
        }
        let app = OverviewUi::new(Mode::App, AREA, &windows(), Vec::new(), 0.0);
        assert!(app.bar(0.0).is_none(), "the application mode has no bar");
        assert_eq!(app.frames().len(), 2);
    }

    #[test]
    fn the_pointer_at_the_top_opens_the_band_and_moves_the_windows_down() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let (rev, before) = (ui.layout_revision(), ui.frames());
        ui.pointer_moved(Some(Point::new(500.0, 30.0)), 1.0);
        assert_ne!(ui.layout_revision(), rev, "a new layout");
        let after = ui.frames();
        assert!(after
            .iter()
            .all(|(_, f)| f.y >= 165.0 + 39.0 - 1e-9 && before.iter().any(|b| b.1.y < f.y)));
        assert!((ui.bar(5.0).unwrap().rect.h - 165.0).abs() < 1e-3);
        ui.pointer_moved(Some(Point::new(500.0, 500.0)), 5.0);
        assert_eq!(ui.frames(), before, "closing the band brings them back");
    }

    #[test]
    fn a_press_and_release_on_a_window_selects_and_on_the_background_leaves() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let (id, frame) = ui.frames()[1];
        let c = Point::new(frame.cx(), frame.cy());
        assert_eq!(ui.press(c, 1.0), UiAction::None);
        assert_eq!(ui.release(c, 1.0), UiAction::Select(id));
        assert_eq!(
            ui.press(Point::new(AREA.x + 1.0, AREA.bottom() - 1.0), 1.0),
            UiAction::Close
        );
    }

    #[test]
    fn a_window_dropped_on_another_desktop_moves_there() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let (id, frame) = ui.frames()[0];
        let c = Point::new(frame.cx(), frame.cy());
        assert_eq!(ui.press(c, 1.0), UiAction::None);
        let other = ui.bar(1.0).unwrap().slots[1].name.center();
        ui.pointer_moved(Some(Point::new(c.x, c.y - 50.0)), 1.1);
        let (dragged, at) = ui
            .window_drag()
            .expect("past the slack the window is dragged");
        assert_eq!(dragged, id);
        assert!(
            (at.y - (frame.y - 50.0)).abs() < 1e-9,
            "it follows the pointer"
        );
        ui.pointer_moved(Some(other), 1.2);
        assert_eq!(ui.drop_target(1.2), Some(11));
        assert_eq!(
            ui.release(other, 1.3),
            UiAction::MoveWindow {
                window: id,
                workspace: 11
            }
        );
        assert!(ui.window_drag().is_none());
        // dropped on the current desktop or off the bar: nothing, the window goes back
        assert_eq!(ui.press(c, 2.0), UiAction::None);
        let current = ui.bar(2.0).unwrap().slots[0].name.center();
        ui.pointer_moved(Some(current), 2.1);
        assert_eq!(ui.release(current, 2.2), UiAction::None);
    }

    #[test]
    fn the_bar_switches_adds_removes_and_reorders_workspaces() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        let bar = ui.bar(1.0).unwrap();
        let second = bar.slots[1].name;
        let centre = Point::new(second.cx(), second.cy());
        // press and release in place: a switch
        assert_eq!(ui.press(centre, 1.0), UiAction::None);
        ui.pointer_moved(Some(centre), 1.0);
        assert_eq!(ui.release(centre, 1.0), UiAction::SwitchWorkspace(11));
        // the add button
        assert_eq!(ui.press(bar.add.center(), 1.0), UiAction::AddWorkspace);
        // the remove button shows on a picture of the open band after a second's rest
        let thumb = ui.bar(10.0).unwrap().slots[1].thumb;
        let on = Point::new(thumb.cx(), thumb.cy());
        ui.pointer_moved(Some(on), 10.0);
        assert_eq!(ui.hovered_slot(), Some(11));
        ui.pointer_moved(Some(Point::new(on.x + 1.0, on.y)), 10.2);
        assert_eq!(ui.removable(10.5), None, "not yet");
        assert_eq!(ui.next_change(10.5), Some(11.0));
        assert_eq!(ui.removable(11.0), Some(11));
        let button = OverviewUi::remove_button(&thumb);
        assert_eq!(button.w, 28.0);
        assert_eq!(
            ui.press(button.center(), 11.0),
            UiAction::RemoveWorkspace(11)
        );
        ui.pointer_moved(Some(Point::new(AREA.cx(), AREA.bottom() - 1.0)), 11.0);
        assert_eq!(ui.hovered_slot(), None);
        assert_eq!(ui.removable(20.0), None);
        // a drag of the second name to before the first reorders
        let bar = ui.bar(30.0).unwrap();
        let (first, second) = (bar.slots[0].name, bar.slots[1].name);
        let centre = Point::new(second.cx(), second.cy());
        ui.pointer_moved(Some(centre), 30.0);
        assert_eq!(ui.press(centre, 30.0), UiAction::None);
        let target = Point::new(first.x + 2.0, first.cy());
        ui.pointer_moved(Some(target), 30.1);
        assert_eq!(
            ui.drag().map(|d| d.0),
            Some(11),
            "past the slack the slot is dragged"
        );
        assert_eq!(
            ui.release(target, 30.2),
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
    fn closing_springs_the_chrome_out_quickly_and_ignores_input() {
        let mut ui = OverviewUi::new(Mode::All, AREA, &windows(), workspaces(), 0.0);
        assert_eq!(ui.alpha(5.0), 1.0);
        ui.close(5.0);
        assert!(ui.is_closing() && ui.is_animating(5.01));
        // critically damped, response 0.125 s: (1 + ωt)·e^(−ωt) with ω = 2π / 0.125
        let w = std::f64::consts::TAU / HIDE_RESPONSE;
        let t = 0.04;
        assert!((ui.alpha(5.0 + t) - (1.0 + w * t) * (-w * t).exp()).abs() < 1e-6);
        assert!(!ui.is_gone(5.05));
        assert!(ui.is_gone(5.3), "gone within a few tenths");
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
