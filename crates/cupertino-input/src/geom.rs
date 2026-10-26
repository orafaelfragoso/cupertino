//! Minimal geometry and modifier types shared by the input modules.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Half-open containment, `[x, x + w) × [y, y + h)`.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn inflate(&self, d: f64) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
}

/// Travel from the press point, per axis, before a press becomes a drag: 5 pt (for the
/// table and path controls and the window-edge hit test, which use 5 × 5; the table's 3 pt
/// vertical and the browser's and tab strip's larger boxes are not modelled).
pub const DRAG_START_THRESHOLD: f64 = 5.0;

/// Whether `pointer` has left the box of [`DRAG_START_THRESHOLD`] around `press`, so the press
/// becomes a drag (the strictness of the comparison is).
pub fn drag_started(press: Point, pointer: Point) -> bool {
    (pointer.x - press.x).abs() > DRAG_START_THRESHOLD || (pointer.y - press.y).abs() > DRAG_START_THRESHOLD
}

/// Keyboard modifiers. `super_key` is the compositor's primary shortcut modifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, super_key: false };

    pub const fn new(shift: bool, ctrl: bool, alt: bool, super_key: bool) -> Self {
        Self { shift, ctrl, alt, super_key }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }

    /// Every modifier set in `required` is also set in `self`.
    pub fn contains(&self, required: Modifiers) -> bool {
        (!required.shift || self.shift) && (!required.ctrl || self.ctrl) && (!required.alt || self.alt) && (!required.super_key || self.super_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
        assert!(r.inflate(1.0).contains(Point::new(-0.5, -0.5)));
    }

    #[test]
    fn a_press_becomes_a_drag_only_after_leaving_the_5_pt_box() {
        let press = Point::new(100.0, 100.0);
        assert!(!drag_started(press, Point::new(105.0, 95.0)));
        assert!(drag_started(press, Point::new(105.1, 100.0)));
        assert!(drag_started(press, Point::new(100.0, 94.9)));
    }

    #[test]
    fn modifier_subset() {
        let held = Modifiers { ctrl: true, alt: true, ..Default::default() };
        assert!(held.contains(Modifiers { ctrl: true, ..Default::default() }));
        assert!(held.contains(Modifiers::NONE));
        assert!(!held.contains(Modifiers { shift: true, ..Default::default() }));
        assert!(Modifiers::NONE.is_empty() && !held.is_empty());
    }
}
