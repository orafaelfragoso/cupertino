//! Basic 2D value types. Coordinates are `f64`, y grows downwards (screen convention).

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, o: Point) -> f64 {
        (self - o).length()
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, o: Point) -> f64 {
        self.x * o.x + self.y * o.y
    }

    pub fn lerp(self, o: Point, t: f64) -> Point {
        Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }

    pub fn to_f32(self) -> [f32; 2] {
        [self.x as f32, self.y as f32]
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Point {
    type Output = Point;
    fn mul(self, s: f64) -> Point {
        Point::new(self.x * s, self.y * s)
    }
}

impl Neg for Point {
    type Output = Point;
    fn neg(self) -> Point {
        Point::new(-self.x, -self.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size { width: 0.0, height: 0.0 };

    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn scale(self, s: f64) -> Size {
        Size::new(self.width * s, self.height * s)
    }
}

/// Edge insets (distance from each edge towards the interior).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

impl Insets {
    pub const ZERO: Insets = Insets { top: 0.0, left: 0.0, bottom: 0.0, right: 0.0 };

    pub const fn new(top: f64, left: f64, bottom: f64, right: f64) -> Self {
        Self { top, left, bottom, right }
    }

    pub const fn uniform(v: f64) -> Self {
        Self::new(v, v, v, v)
    }

    pub const fn symmetric(horizontal: f64, vertical: f64) -> Self {
        Self::new(vertical, horizontal, vertical, horizontal)
    }

    pub fn horizontal(self) -> f64 {
        self.left + self.right
    }

    pub fn vertical(self) -> f64 {
        self.top + self.bottom
    }

    pub fn scale(self, s: f64) -> Insets {
        Insets::new(self.top * s, self.left * s, self.bottom * s, self.right * s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const ZERO: Rect = Rect { origin: Point::ZERO, size: Size::ZERO };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { origin: Point::new(x, y), size: Size::new(width, height) }
    }

    pub fn from_ltrb(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self::new(left, top, right - left, bottom - top)
    }

    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    pub fn x(&self) -> f64 {
        self.origin.x
    }
    pub fn y(&self) -> f64 {
        self.origin.y
    }
    pub fn width(&self) -> f64 {
        self.size.width
    }
    pub fn height(&self) -> f64 {
        self.size.height
    }
    pub fn min_x(&self) -> f64 {
        self.origin.x
    }
    pub fn min_y(&self) -> f64 {
        self.origin.y
    }
    pub fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }
    pub fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }

    pub fn center(&self) -> Point {
        Point::new(self.origin.x + self.size.width * 0.5, self.origin.y + self.size.height * 0.5)
    }

    pub fn is_empty(&self) -> bool {
        self.size.is_empty()
    }

    pub fn area(&self) -> f64 {
        self.size.width.max(0.0) * self.size.height.max(0.0)
    }

    /// Half-open containment: `min <= p < max`.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.min_x() < o.max_x() && o.min_x() < self.max_x() && self.min_y() < o.max_y() && o.min_y() < self.max_y()
    }

    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let l = self.min_x().max(o.min_x());
        let t = self.min_y().max(o.min_y());
        let r = self.max_x().min(o.max_x());
        let b = self.max_y().min(o.max_y());
        (r > l && b > t).then(|| Rect::from_ltrb(l, t, r, b))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_ltrb(self.min_x().min(o.min_x()), self.min_y().min(o.min_y()), self.max_x().max(o.max_x()), self.max_y().max(o.max_y()))
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.origin.x + dx, self.origin.y + dy, self.size.width, self.size.height)
    }

    /// Shrink by the insets (negative insets grow the rect). Size is clamped at zero.
    pub fn inset(&self, i: Insets) -> Rect {
        let w = (self.size.width - i.horizontal()).max(0.0);
        let h = (self.size.height - i.vertical()).max(0.0);
        Rect::new(self.origin.x + i.left, self.origin.y + i.top, w, h)
    }

    pub fn outset(&self, d: f64) -> Rect {
        Rect::new(self.origin.x - d, self.origin.y - d, self.size.width + 2.0 * d, self.size.height + 2.0 * d)
    }

    /// Scale about the origin of the coordinate system.
    pub fn scale(&self, s: f64) -> Rect {
        Rect::new(self.origin.x * s, self.origin.y * s, self.size.width * s, self.size.height * s)
    }

    /// Smallest integer-aligned rect containing this one.
    pub fn round_out(&self) -> Rect {
        Rect::from_ltrb(self.min_x().floor(), self.min_y().floor(), self.max_x().ceil(), self.max_y().ceil())
    }

    pub fn to_f32(&self) -> [f32; 4] {
        [self.origin.x as f32, self.origin.y as f32, self.size.width as f32, self.size.height as f32]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_basics() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.max_x(), 110.0);
        assert_eq!(r.center(), Point::new(60.0, 45.0));
        assert!(r.contains(Point::new(10.0, 20.0)));
        assert!(!r.contains(Point::new(110.0, 20.0)));
        assert_eq!(r.area(), 5000.0);
    }

    #[test]
    fn intersection_union() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert_eq!(a.intersection(&Rect::new(10.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(Rect::ZERO.union(&b), b);
    }

    #[test]
    fn inset_and_outset() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(r.inset(Insets::new(1.0, 2.0, 3.0, 4.0)), Rect::new(2.0, 1.0, 94.0, 46.0));
        assert_eq!(r.inset(Insets::uniform(60.0)).size, Size::ZERO);
        assert_eq!(r.outset(5.0), Rect::new(-5.0, -5.0, 110.0, 60.0));
    }

    #[test]
    fn round_out_contains_original() {
        let r = Rect::new(0.3, 0.7, 10.2, 5.5);
        let o = r.round_out();
        assert!(o.min_x() <= r.min_x() && o.max_x() >= r.max_x() && o.min_y() <= r.min_y() && o.max_y() >= r.max_y());
        assert_eq!(o, Rect::new(0.0, 0.0, 11.0, 7.0));
    }

    #[test]
    fn point_ops() {
        let p = Point::new(3.0, 4.0);
        assert_eq!(p.length(), 5.0);
        assert_eq!(p.lerp(Point::ZERO, 0.5), Point::new(1.5, 2.0));
        assert_eq!((p + p) * 0.5, p);
    }
}
