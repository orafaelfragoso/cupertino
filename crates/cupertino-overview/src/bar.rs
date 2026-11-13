//! Desktops bar model: a band across the top of the output. Closed it shows the desktops' names alone, the current
//! one on a pill; open (the pointer at the top) it grows and shows a live picture of every desktop with its name
//! below. The add button is a bare plus at the band's top-right corner in both states.

use cupertino_animation::{AnimatedValue, Curve, SpringParams};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::WorkspaceId;

#[derive(Clone, Debug)]
pub struct BarConfig {
    /// Height of the band while it shows the names alone (72 pt).
    pub closed_height: f64,
    /// Height of the band while it shows the desktops' pictures (165 pt).
    pub open_height: f64,
    /// Height of a desktop's picture in the open band; its width follows the output's aspect (95 pt).
    pub thumb_height: f64,
    /// Space between the band's top and the pictures in the open band (20 pt).
    pub thumb_top: f64,
    /// Space between two pictures; the names of the closed band keep the same centres (24 pt, approximate).
    pub gap: f64,
    /// Space kept at the band's left and right ends, clear of the add button (64 pt, approximate).
    pub side_margin: f64,
    /// Height of a name's pill (24 pt).
    pub name_height: f64,
    /// Centre of the names below the band's top in the closed band (52 pt).
    pub closed_name_cy: f64,
    /// Centre of a name below the bottom of its picture in the open band (20 pt).
    pub open_name_gap: f64,
    /// Side of the add button's plus (20 pt).
    pub add_size: f64,
    /// Side of the square round the plus that takes a press (32 pt, approximate).
    pub add_hit: f64,
    /// Centre of the plus from the band's right edge and from its top (28 pt, 35 pt).
    pub add_right: f64,
    pub add_cy: f64,
    /// Open while the pointer is over the band.
    pub expand_on_hover: bool,
    /// Spring of the band's opening and closing: critically damped, response 0.25 s.
    pub expand_curve: Curve,
    /// Space between the band's bottom and the highest point a window may reach (39 pt).
    pub layout_gap: f64,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            closed_height: 72.0,
            open_height: 165.0,
            thumb_height: 95.0,
            thumb_top: 20.0,
            gap: 24.0,
            side_margin: 64.0,
            name_height: 24.0,
            closed_name_cy: 52.0,
            open_name_gap: 20.0,
            add_size: 20.0,
            add_hit: 32.0,
            add_right: 28.0,
            add_cy: 35.0,
            expand_on_hover: true,
            expand_curve: Curve::Spring(SpringParams::from_ratio(0.25, 1.0)),
            layout_gap: 39.0,
        }
    }
}

impl BarConfig {
    /// Height of the band, open or closed.
    pub fn band_height(&self, open: bool) -> f64 {
        if open {
            self.open_height
        } else {
            self.closed_height
        }
    }

    /// Distance from the top of the area to the highest point a window thumbnail may reach: the band's bottom plus
    /// [`layout_gap`](Self::layout_gap).
    pub fn windows_top(&self, open: bool) -> f64 {
        self.band_height(open) + self.layout_gap
    }

    /// Centre of a name below the band's top in the open band.
    pub fn open_name_cy(&self) -> f64 {
        self.thumb_top + self.thumb_height + self.open_name_gap
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Workspace(WorkspaceId),
    Add,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarSlot {
    pub id: WorkspaceId,
    /// The desktop's picture: full size in the open band, shrunk to nothing towards its top centre as it closes.
    pub thumb: Rect,
    /// Where the name is: as wide as the picture, [`BarConfig::name_height`] tall, centred on the name.
    pub name: Rect,
    pub current: bool,
    pub hovered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    /// The band, across the whole area.
    pub rect: Rect,
    pub slots: Vec<BarSlot>,
    /// The square the add button's plus is centred in ([`BarConfig::add_hit`]).
    pub add: Rect,
    /// 0 closed, 1 open.
    pub expansion: f64,
}

pub struct WorkspaceBar {
    pub cfg: BarConfig,
    area: Rect,
    /// Width / height of an output (picture aspect ratio).
    aspect: f64,
    workspaces: Vec<WorkspaceId>,
    current: Option<WorkspaceId>,
    expanded: AnimatedValue<f64>,
    target_expanded: bool,
    hovered: Option<BarHit>,
}

impl WorkspaceBar {
    pub fn new(cfg: BarConfig, area: Rect, output: Rect) -> Self {
        Self {
            cfg,
            area,
            aspect: output.w / output.h.max(1.0),
            workspaces: Vec::new(),
            current: None,
            expanded: AnimatedValue::new(0.0),
            target_expanded: false,
            hovered: None,
        }
    }

    pub fn set_workspaces(&mut self, ids: &[WorkspaceId], current: Option<WorkspaceId>) {
        self.workspaces = ids.to_vec();
        self.current = current;
        if self
            .hovered
            .is_some_and(|h| matches!(h, BarHit::Workspace(w) if !ids.contains(&w)))
        {
            self.hovered = None;
        }
    }

    pub fn expansion(&self, now: f64) -> f64 {
        self.expanded.value(now).clamp(0.0, 1.0)
    }

    pub fn is_expanded_target(&self) -> bool {
        self.target_expanded
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.expanded.is_animating(now)
    }

    pub fn hovered(&self) -> Option<BarHit> {
        self.hovered
    }

    /// Request expansion (or collapse); interruptible.
    pub fn set_expanded(&mut self, expanded: bool, now: f64) {
        if expanded != self.target_expanded {
            self.target_expanded = expanded;
            self.expanded
                .animate_to(if expanded { 1.0 } else { 0.0 }, self.cfg.expand_curve, now);
        }
    }

    /// Size of a picture in the open band: [`BarConfig::thumb_height`] at the output's aspect, narrowed so every
    /// desktop fits between the side margins.
    fn thumb_size(&self) -> (f64, f64) {
        let c = &self.cfg;
        let n = self.workspaces.len().max(1) as f64;
        let room = (self.area.w - 2.0 * c.side_margin - (n - 1.0) * c.gap).max(n);
        let w = (c.thumb_height * self.aspect).min(room / n);
        (w, w / self.aspect)
    }

    pub fn layout(&self, now: f64) -> BarLayout {
        let e = self.expansion(now);
        let c = &self.cfg;
        let (tw, th) = self.thumb_size();
        let n = self.workspaces.len();
        let row_w = n as f64 * tw + n.saturating_sub(1) as f64 * c.gap;
        let top = self.area.y;
        let rect = Rect::new(
            self.area.x,
            top,
            self.area.w,
            c.closed_height + (c.open_height - c.closed_height) * e,
        );
        let name_cy = top + c.closed_name_cy + (c.open_name_cy() - c.closed_name_cy) * e;
        let mut x = self.area.cx() - row_w / 2.0;
        let slots = self
            .workspaces
            .iter()
            .map(|id| {
                let cx = x + tw / 2.0;
                x += tw + c.gap;
                BarSlot {
                    id: *id,
                    thumb: Rect::new(cx - tw * e / 2.0, top + c.thumb_top * e, tw * e, th * e),
                    name: Rect::new(
                        cx - tw / 2.0,
                        name_cy - c.name_height / 2.0,
                        tw,
                        c.name_height,
                    ),
                    current: self.current == Some(*id),
                    hovered: self.hovered == Some(BarHit::Workspace(*id)),
                }
            })
            .collect();
        let add = Rect::new(
            self.area.right() - c.add_right - c.add_hit / 2.0,
            top + c.add_cy - c.add_hit / 2.0,
            c.add_hit,
            c.add_hit,
        );
        BarLayout {
            rect,
            slots,
            add,
            expansion: e,
        }
    }

    pub fn hit_test(&self, p: Point, now: f64) -> Option<BarHit> {
        let l = self.layout(now);
        if l.add.contains(p) {
            return Some(BarHit::Add);
        }
        l.slots
            .iter()
            .find(|s| s.name.contains(p) || (l.expansion > 0.5 && s.thumb.contains(p)))
            .map(|s| BarHit::Workspace(s.id))
    }

    /// Whether `p` is over the band at the height it is heading for.
    fn over_band(&self, p: Point) -> bool {
        let h = self.cfg.band_height(self.target_expanded);
        p.x >= self.area.x && p.x < self.area.right() && p.y >= self.area.y && p.y < self.area.y + h
    }

    /// Track the pointer: updates the hovered slot and opens or closes the band.
    pub fn pointer_moved(&mut self, pointer: Option<Point>, now: f64) {
        let over = pointer.is_some_and(|p| self.over_band(p));
        if self.cfg.expand_on_hover {
            self.set_expanded(over, now);
        }
        self.hovered = pointer.and_then(|p| self.hit_test(p, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 1512.0, 982.0);

    fn bar(n: u64) -> WorkspaceBar {
        let mut b = WorkspaceBar::new(BarConfig::default(), AREA, AREA);
        let ids: Vec<WorkspaceId> = (1..=n).map(WorkspaceId).collect();
        b.set_workspaces(&ids, ids.first().copied());
        b
    }

    #[test]
    fn the_closed_band_spans_the_top_with_the_names_alone() {
        let b = bar(1);
        let l = b.layout(0.0);
        assert_eq!(l.rect, Rect::new(0.0, 0.0, 1512.0, 72.0));
        assert!(l.slots[0].current);
        assert_eq!(l.slots[0].thumb.w, 0.0, "no picture while closed");
        let name = l.slots[0].name;
        assert!(
            (name.cx() - 756.0).abs() < 1e-9,
            "one desktop's name is centred"
        );
        assert!((name.cy() - 52.0).abs() < 1e-9);
        assert_eq!(name.h, 24.0);
        assert!((l.add.cx() - (1512.0 - 28.0)).abs() < 1e-9);
        assert!((l.add.cy() - 35.0).abs() < 1e-9);
    }

    #[test]
    fn the_open_band_shows_pictures_at_the_output_aspect_with_names_below() {
        let mut b = bar(3);
        b.set_expanded(true, 0.0);
        let l = b.layout(5.0);
        assert!((l.rect.h - 165.0).abs() < 1e-6);
        let s = l.slots[0];
        assert!((s.thumb.h - 95.0).abs() < 1e-6);
        assert!((s.thumb.w / s.thumb.h - 1512.0 / 982.0).abs() < 1e-6);
        assert!((s.thumb.y - 20.0).abs() < 1e-6);
        assert!(s.name.y > s.thumb.bottom(), "the name is below its picture");
        assert!(s.name.bottom() <= l.rect.bottom());
        assert!(l
            .slots
            .windows(2)
            .all(|p| (p[1].thumb.x - p[0].thumb.right() - b.cfg.gap).abs() < 1e-6));
        let row = l.slots[0].thumb.union(&l.slots[2].thumb);
        assert!((row.cx() - 756.0).abs() < 1e-6, "the row is centred");
    }

    #[test]
    fn names_keep_their_centres_as_the_band_opens() {
        let mut b = bar(2);
        let closed: Vec<f64> = b.layout(0.0).slots.iter().map(|s| s.name.cx()).collect();
        b.set_expanded(true, 0.0);
        let open: Vec<f64> = b.layout(5.0).slots.iter().map(|s| s.name.cx()).collect();
        assert_eq!(closed, open);
    }

    #[test]
    fn many_desktops_shrink_to_fit_between_the_margins() {
        let mut b = bar(12);
        b.set_expanded(true, 0.0);
        let l = b.layout(5.0);
        assert!(l.slots[0].thumb.x >= b.cfg.side_margin - 1e-6);
        assert!(l.slots[11].thumb.right() <= 1512.0 - b.cfg.side_margin + 1e-6);
    }

    #[test]
    fn the_pointer_at_the_top_opens_the_band_and_leaving_it_closes() {
        let mut b = bar(2);
        b.pointer_moved(Some(Point::new(300.0, 60.0)), 0.0);
        assert!(b.is_expanded_target());
        let mid = b.expansion(0.1);
        assert!(mid > 0.0 && mid < 1.0);
        // within the open band it stays open
        b.pointer_moved(Some(Point::new(300.0, 150.0)), 2.0);
        assert!(b.is_expanded_target());
        b.pointer_moved(Some(Point::new(300.0, 400.0)), 2.0);
        assert!(!b.is_expanded_target());
        let cfg = BarConfig {
            expand_on_hover: false,
            ..Default::default()
        };
        let mut b = WorkspaceBar::new(cfg, AREA, AREA);
        b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
        b.pointer_moved(Some(Point::new(700.0, 40.0)), 0.0);
        assert!(!b.is_expanded_target());
    }

    #[test]
    fn hit_testing_names_pictures_and_add() {
        let mut b = bar(3);
        let l = b.layout(0.0);
        for s in &l.slots {
            assert_eq!(
                b.hit_test(s.name.center(), 0.0),
                Some(BarHit::Workspace(s.id))
            );
        }
        assert_eq!(b.hit_test(l.add.center(), 0.0), Some(BarHit::Add));
        b.set_expanded(true, 0.0);
        let l = b.layout(5.0);
        for s in &l.slots {
            assert_eq!(
                b.hit_test(s.thumb.center(), 5.0),
                Some(BarHit::Workspace(s.id))
            );
        }
        assert_eq!(b.hit_test(Point::new(10.0, 500.0), 5.0), None);
        b.pointer_moved(Some(l.slots[1].thumb.center()), 5.0);
        assert_eq!(b.hovered(), Some(BarHit::Workspace(WorkspaceId(2))));
        assert!(b.layout(5.0).slots[1].hovered);
        b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
        assert_eq!(b.hovered(), None, "stale hover is dropped");
    }

    #[test]
    fn windows_start_below_the_band_by_the_layout_gap() {
        let c = BarConfig::default();
        assert_eq!(c.windows_top(false), 111.0);
        assert_eq!(c.windows_top(true), 204.0);
    }

    #[test]
    fn empty_bar_is_valid() {
        let b = WorkspaceBar::new(BarConfig::default(), AREA, AREA);
        let l = b.layout(0.0);
        assert!(l.slots.is_empty());
        assert_eq!(b.hit_test(Point::new(700.0, 20.0), 0.0), None);
    }
}
