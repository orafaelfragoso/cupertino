//! Workspaces bar model: thumbnail slots, expansion on hover, add-workspace slot.
//!
//! The collapsed bar band is 90 pt tall (the spaces bar height). Its
//! split into padding and thumbnail height, the expanded size and the expansion curve are
//! approximate.

use cupertino_animation::{AnimatedValue, Curve, SpringParams};
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::WorkspaceId;

#[derive(Clone, Debug)]
pub struct BarConfig {
    /// Gap between the top of the area and the bar.
    pub top_margin: f64,
    /// Thumbnail height when collapsed / expanded.
    pub collapsed_thumb_height: f64,
    pub expanded_thumb_height: f64,
    /// Padding between the bar edge and the thumbnails.
    pub padding: f64,
    pub gap: f64,
    /// Extra pointer margin around the bar that keeps it expanded.
    pub hover_margin: f64,
    /// Expand while the pointer is over the bar.
    pub expand_on_hover: bool,
    pub expand_curve: Curve,
    /// Space kept between the bottom edge of the strip's slots, at their tallest, and the top of
    /// the window thumbnails (20 pt).
    pub layout_gap: f64,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            top_margin: 0.0,
            collapsed_thumb_height: 66.0,
            expanded_thumb_height: 120.0,
            padding: 12.0,
            gap: 12.0,
            hover_margin: 8.0,
            expand_on_hover: true,
            expand_curve: Curve::Spring(SpringParams::from_perceptual(0.3, 0.0)),
            layout_gap: 20.0,
        }
    }
}

impl BarConfig {
    /// Vertical space the bar takes at the top of the area: collapsed when it expands only on hover, expanded when
    /// it is pinned.
    pub fn reserved_height(&self) -> f64 {
        let thumb = if self.expand_on_hover {
            self.collapsed_thumb_height
        } else {
            self.expanded_thumb_height
        };
        self.top_margin + thumb + 2.0 * self.padding
    }

    /// Distance from the top of the area to the bottom edge of the slots when the strip is
    /// expanded, the tallest it gets. A bar that expands on hover reaches it too, so windows are
    /// laid out below it either way and never sit under a slot.
    pub fn strip_bottom(&self) -> f64 {
        self.top_margin + self.padding + self.expanded_thumb_height
    }

    /// Distance from the top of the area to the highest point a window thumbnail may reach:
    /// the strip's bottom plus [`layout_gap`](Self::layout_gap).
    pub fn thumbnails_top(&self) -> f64 {
        self.strip_bottom() + self.layout_gap
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
    pub rect: Rect,
    pub current: bool,
    pub hovered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub rect: Rect,
    pub slots: Vec<BarSlot>,
    /// The add-workspace slot; its width grows with the expansion (0 when collapsed).
    pub add: Option<Rect>,
    pub expansion: f64,
}

pub struct WorkspaceBar {
    pub cfg: BarConfig,
    area: Rect,
    /// Width / height of an output (thumbnail aspect ratio).
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

    pub fn layout(&self, now: f64) -> BarLayout {
        let e = self.expansion(now);
        let c = &self.cfg;
        let th =
            c.collapsed_thumb_height + (c.expanded_thumb_height - c.collapsed_thumb_height) * e;
        let tw = th * self.aspect;
        let n = self.workspaces.len();
        let add_w = tw * e;
        let slots_w = n as f64 * tw + n.saturating_sub(1) as f64 * c.gap;
        let content_w = slots_w + if add_w > 0.0 { c.gap * e + add_w } else { 0.0 };
        let bar_w = content_w + 2.0 * c.padding;
        let bar_h = th + 2.0 * c.padding;
        let rect = Rect::new(
            self.area.cx() - bar_w / 2.0,
            self.area.y + c.top_margin,
            bar_w,
            bar_h,
        );
        let mut x = rect.x + c.padding;
        let y = rect.y + c.padding;
        let slots = self
            .workspaces
            .iter()
            .map(|id| {
                let r = Rect::new(x, y, tw, th);
                x += tw + c.gap;
                BarSlot {
                    id: *id,
                    rect: r,
                    current: self.current == Some(*id),
                    hovered: self.hovered == Some(BarHit::Workspace(*id)),
                }
            })
            .collect();
        let add = (add_w > 0.0).then(|| Rect::new(x - c.gap + c.gap * e, y, add_w, th));
        BarLayout {
            rect,
            slots,
            add,
            expansion: e,
        }
    }

    /// Pointer region that keeps the bar expanded.
    fn hover_region(&self, now: f64) -> Rect {
        let r = self.layout(now).rect;
        let m = self.cfg.hover_margin;
        Rect::new(
            r.x - m,
            r.y - m - self.cfg.top_margin,
            r.w + 2.0 * m,
            r.h + 2.0 * m + self.cfg.top_margin,
        )
    }

    pub fn hit_test(&self, p: Point, now: f64) -> Option<BarHit> {
        let l = self.layout(now);
        if let Some(s) = l.slots.iter().find(|s| s.rect.contains(p)) {
            return Some(BarHit::Workspace(s.id));
        }
        l.add.filter(|a| a.contains(p)).map(|_| BarHit::Add)
    }

    /// Track the pointer: updates the hovered slot and expands/collapses the bar.
    pub fn pointer_moved(&mut self, pointer: Option<Point>, now: f64) {
        let over = pointer.is_some_and(|p| self.hover_region(now).contains(p));
        if self.cfg.expand_on_hover {
            self.set_expanded(over, now);
        }
        self.hovered = pointer.and_then(|p| self.hit_test(p, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    fn bar(n: u64) -> WorkspaceBar {
        let mut b = WorkspaceBar::new(BarConfig::default(), AREA, AREA);
        let ids: Vec<WorkspaceId> = (1..=n).map(WorkspaceId).collect();
        b.set_workspaces(&ids, ids.first().copied());
        b
    }

    #[test]
    fn collapsed_layout_is_centred_at_the_top() {
        let b = bar(3);
        let l = b.layout(0.0);
        assert!((l.rect.cx() - 960.0).abs() < 1e-9);
        assert_eq!(l.rect.y, b.cfg.top_margin);
        assert_eq!(l.slots.len(), 3);
        assert!(l.add.is_none());
        assert!(l.slots[0].current && !l.slots[1].current);
        let th = b.cfg.collapsed_thumb_height;
        assert!((l.slots[0].rect.h - th).abs() < 1e-9);
        assert!(
            (l.slots[0].rect.w / l.slots[0].rect.h - 1920.0 / 1080.0).abs() < 1e-9,
            "output aspect"
        );
        assert!(l
            .slots
            .windows(2)
            .all(|p| (p[1].rect.x - p[0].rect.right() - b.cfg.gap).abs() < 1e-9));
        assert_eq!(b.cfg.reserved_height(), l.rect.bottom());
    }

    #[test]
    fn hover_expands_and_leaving_collapses_smoothly() {
        let mut b = bar(2);
        let c = b.layout(0.0).rect.center();
        b.pointer_moved(Some(c), 0.0);
        assert!(b.is_expanded_target());
        assert_eq!(b.expansion(0.0), 0.0);
        let mid = b.expansion(0.1);
        assert!(mid > 0.0 && mid < 1.0);
        let l = b.layout(3.0);
        assert!((l.expansion - 1.0).abs() < 1e-3);
        assert!((l.slots[0].rect.h - b.cfg.expanded_thumb_height).abs() < 0.2);
        assert!(l.add.is_some());
        b.pointer_moved(Some(Point::new(960.0, 900.0)), 3.0);
        assert!(!b.is_expanded_target());
        let before = b.expansion(3.05);
        assert!(before < 1.0 && before > 0.0);
        b.pointer_moved(Some(c), 3.05);
        assert!(
            (before - b.expansion(3.05)).abs() < 1e-9,
            "re-expanding keeps the current value"
        );
        assert!(b.is_expanded_target() && b.is_animating(3.05));
    }

    #[test]
    fn hover_keeps_expanded_within_margin_and_can_be_disabled() {
        let mut b = bar(2);
        let r = b.layout(0.0).rect;
        b.pointer_moved(
            Some(Point::new(r.cx(), r.bottom() + b.cfg.hover_margin - 1.0)),
            0.0,
        );
        assert!(b.is_expanded_target());
        let cfg = BarConfig {
            expand_on_hover: false,
            ..Default::default()
        };
        let mut b = WorkspaceBar::new(cfg, AREA, AREA);
        b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
        b.pointer_moved(Some(Point::new(960.0, 40.0)), 0.0);
        assert!(!b.is_expanded_target());
    }

    #[test]
    fn hit_testing_slots_and_add() {
        let mut b = bar(3);
        b.set_expanded(true, 0.0);
        let l = b.layout(5.0);
        for s in &l.slots {
            assert_eq!(
                b.hit_test(s.rect.center(), 5.0),
                Some(BarHit::Workspace(s.id))
            );
        }
        let add = l.add.unwrap();
        assert_eq!(b.hit_test(add.center(), 5.0), Some(BarHit::Add));
        assert_eq!(b.hit_test(Point::new(10.0, 500.0), 5.0), None);
        b.pointer_moved(Some(l.slots[1].rect.center()), 5.0);
        assert_eq!(b.hovered(), Some(BarHit::Workspace(WorkspaceId(2))));
        assert!(b.layout(5.0).slots[1].hovered);
        b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
        assert_eq!(b.hovered(), None, "stale hover is dropped");
    }

    #[test]
    fn growth_is_centred_with_add_slot_and_stays_inside_area() {
        let mut b = bar(6);
        b.set_expanded(true, 0.0);
        let l = b.layout(5.0);
        assert!((l.rect.cx() - 960.0).abs() < 1e-9);
        assert!(l.rect.x >= 0.0 && l.rect.right() <= 1920.0);
        let add = l.add.unwrap();
        assert!(add.x >= l.slots.last().unwrap().rect.right());
        assert!(add.right() <= l.rect.right());
    }

    #[test]
    fn empty_bar_is_valid() {
        let b = WorkspaceBar::new(BarConfig::default(), AREA, AREA);
        let l = b.layout(0.0);
        assert!(l.slots.is_empty());
        assert_eq!(b.hit_test(Point::new(960.0, 20.0), 0.0), None);
    }
}
