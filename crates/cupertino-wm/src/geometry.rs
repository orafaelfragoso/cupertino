//! Minimal logical-coordinate geometry used by the model (y grows downwards).

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
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
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
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            w: size.w,
            h: size.h,
        }
    }
    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
    pub fn center(&self) -> Point {
        Point::new(self.cx(), self.cy())
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, r - x, b - y))
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }
    pub fn inset(&self, d: f64) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn with_origin(&self, p: Point) -> Rect {
        Rect::new(p.x, p.y, self.w, self.h)
    }
    pub fn lerp(a: Rect, b: Rect, t: f64) -> Rect {
        Rect::new(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
            a.w + (b.w - a.w) * t,
            a.h + (b.h - a.h) * t,
        )
    }
    pub fn to_array(self) -> [f64; 4] {
        [self.x, self.y, self.w, self.h]
    }
    pub fn from_array(a: [f64; 4]) -> Rect {
        Rect::new(a[0], a[1], a[2], a[3])
    }
    /// Translate (never resize) so that the rectangle lies inside `bounds` where possible; when it
    /// is larger than `bounds` its leading edge is aligned instead.
    pub fn clamp_into(&self, bounds: &Rect) -> Rect {
        let x = self.x.min(bounds.right() - self.w).max(bounds.x);
        let y = self.y.min(bounds.bottom() - self.h).max(bounds.y);
        Rect::new(x, y, self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert!(a.contains(Point::new(0.0, 0.0)) && !a.contains(Point::new(10.0, 5.0)));
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 5.0, 5.0)));
        assert_eq!(
            Rect::new(-5.0, 95.0, 10.0, 10.0).clamp_into(&Rect::new(0.0, 0.0, 100.0, 100.0)),
            Rect::new(0.0, 90.0, 10.0, 10.0)
        );
        assert_eq!(Rect::from_array(a.to_array()), a);
    }
}

/// A set of rectangle edges (resize handles, tiled sides).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub const NONE: Edges = Edges {
        left: false,
        right: false,
        top: false,
        bottom: false,
    };
    pub const ALL: Edges = Edges {
        left: true,
        right: true,
        top: true,
        bottom: true,
    };
    pub fn any(&self) -> bool {
        self.left || self.right || self.top || self.bottom
    }
}
