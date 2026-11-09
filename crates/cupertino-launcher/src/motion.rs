//! How the panel moves between its views: one presentation of the enclosure (its shape, its opacity and scale), the
//! header (the search field's, or the browser's) and the content inside it, which the panel's paint, clip and input
//! region all read, so they always agree.
//!
//! The transitions are staged:
//!
//! * **Opening** the browser: the enclosure fades and grows into place first; the browser's filters and applications
//!   fade in a moment later, so the surface arrives and then fills.
//! * **Folding** the browser into the search field: the content fades out at once, the header becomes the search
//!   field's, then the enclosure contracts along a critically damped spring, its top edge fixed and its sides closing
//!   in evenly, its corners growing into the field's capsule.
//! * **Unfolding** the search field into the browser: the enclosure opens along the same spring and the content fades
//!   in as it opens.
//! * **Closing**: the browser fades and shrinks a little; the compact field only fades, quicker.
//! * The search panel growing or shrinking with its results moves along the same spring.
//!
//! Every value is retargeted from what is presented at the moment a new transition starts, with its velocity, so a
//! transition can be interrupted at any point (fold during an unfold, open during a close) without a jump. Under
//! Reduce Motion the shape changes at once and nothing scales; only the opacities still fade.
//!
//! The model is pure: every value is sampled at a time passed in.

use cupertino_animation::{AnimatedValue, CubicBezier, Curve, SpringParams};

/// The enclosure fading and growing in when the launcher opens, and the scale it grows from.
pub const OPEN_DURATION: f64 = 0.18;
pub const ENTER_SCALE: f64 = 0.96;
/// The browser's content fading in after the enclosure opened: its delay and its duration.
pub const CONTENT_DELAY: f64 = 0.08;
pub const CONTENT_FADE: f64 = 0.10;
/// The content fading out when the browser folds; the enclosure starts contracting after it.
pub const FOLD_FADE: f64 = 0.05;
/// The enclosure's spring between shapes: critically damped, with this response in seconds.
pub const SHAPE_RESPONSE: f64 = 0.24;
/// The content fading in as the browser unfolds: its delay and its duration.
pub const UNFOLD_CONTENT_DELAY: f64 = 0.06;
pub const UNFOLD_CONTENT_FADE: f64 = 0.12;
/// Closing: the browser's fade (it also shrinks back to [`ENTER_SCALE`]), and the compact field's quicker fade.
pub const CLOSE_DURATION: f64 = 0.2;
pub const CLOSE_COMPACT_DURATION: f64 = 0.1;

/// The enclosure's size and corner radius, in points. Its top edge stays where it is and it stays centred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub width: f64,
    pub height: f64,
    pub radius: f64,
}

impl Shape {
    pub const fn new(width: f64, height: f64, radius: f64) -> Self {
        Self {
            width,
            height,
            radius,
        }
    }

    fn array(self) -> [f64; 3] {
        [self.width, self.height, self.radius]
    }

    fn of(a: [f64; 3]) -> Self {
        Self::new(
            a[0].max(0.0),
            a[1].max(0.0),
            a[2].max(0.0).min(a[1].max(0.0) / 2.0),
        )
    }

    /// The smallest shape holding both.
    pub fn union(self, other: Shape) -> Shape {
        Shape::new(
            self.width.max(other.width),
            self.height.max(other.height),
            self.radius.max(other.radius),
        )
    }
}

/// Which header the panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    Search,
    Browse,
}

/// A value fading from where it is towards a target after a delay, along an ease.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
    ease: CubicBezier,
}

impl Fade {
    const fn at(value: f64) -> Self {
        Fade {
            from: value,
            to: value,
            start: 0.0,
            duration: 0.0,
            ease: CubicBezier::LINEAR,
        }
    }

    fn value(&self, now: f64) -> f64 {
        if self.duration <= 0.0 || now >= self.start + self.duration {
            return if now >= self.start {
                self.to
            } else {
                self.from
            };
        }
        let x = ((now - self.start) / self.duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * self.ease.eval(x)
    }

    /// Head for `to` from the value presented at `now`, starting `delay` later and taking `duration`.
    fn retarget(&mut self, to: f64, now: f64, delay: f64, duration: f64, ease: CubicBezier) {
        let from = self.value(now);
        *self = Fade {
            from,
            to,
            start: now + delay,
            duration,
            ease,
        };
    }

    fn animating(&self, now: f64) -> bool {
        self.from != self.to && now < self.start + self.duration
    }
}

/// What is presented at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub shape: Shape,
    /// The enclosure's opacity (shadow and content included) and its scale about its top centre.
    pub alpha: f64,
    pub scale: f64,
    /// The browser's content opacity (filters, applications, separators).
    pub content: f64,
    pub header: Header,
}

/// The panel's presentation.
#[derive(Clone, Debug)]
pub struct Motion {
    shape: AnimatedValue<[f64; 3]>,
    alpha: Fade,
    scale: Fade,
    content: Fade,
    /// The header shown before `switch_at`, and the one from then on.
    header: (Header, Header),
    switch_at: f64,
    reduce: bool,
    /// The shape the panel is heading for.
    target: Shape,
}

impl Motion {
    pub fn new(shape: Shape) -> Self {
        Self {
            shape: AnimatedValue::new(shape.array()),
            alpha: Fade::at(0.0),
            scale: Fade::at(1.0),
            content: Fade::at(0.0),
            header: (Header::Search, Header::Search),
            switch_at: 0.0,
            reduce: false,
            target: shape,
        }
    }

    /// Follow the session's Reduce Motion option from the next transition on.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce = on;
    }

    pub fn reduce_motion(&self) -> bool {
        self.reduce
    }

    fn spring() -> Curve {
        Curve::Spring(SpringParams::from_ratio(SHAPE_RESPONSE, 1.0))
    }

    /// Move the shape to `to`, starting `delay` from `now`; at once under Reduce Motion. A shape still moving turns
    /// at once, from where it is and with its velocity: waiting would hold it still mid-way.
    fn shape_to(&mut self, to: Shape, now: f64, delay: f64) {
        self.target = to;
        if self.reduce {
            self.shape.set_immediately(to.array());
        } else if self.shape.is_animating(now) {
            self.shape.animate_to(to.array(), Self::spring(), now);
        } else {
            self.shape.cancel(now);
            self.shape
                .animate_to(to.array(), Self::spring(), now + delay);
        }
    }

    fn switch_header(&mut self, to: Header, now: f64, at: f64) {
        let shown = self.header_at(now);
        self.header = (shown, to);
        self.switch_at = at;
    }

    fn header_at(&self, now: f64) -> Header {
        if now >= self.switch_at {
            self.header.1
        } else {
            self.header.0
        }
    }

    /// The panel appears at `now` with `shape`, showing the browser or the search field. A panel still closing is
    /// turned around from where it is.
    pub fn show(&mut self, shape: Shape, browse: bool, now: f64) {
        let closing = self.alpha.value(now) > 0.0;
        if !closing {
            self.shape.set_immediately(shape.array());
            self.target = shape;
        } else {
            self.shape_to(shape, now, 0.0);
        }
        self.alpha
            .retarget(1.0, now, 0.0, OPEN_DURATION, CubicBezier::EASE_OUT);
        let from_scale = if self.reduce || closing {
            self.scale.value(now)
        } else {
            ENTER_SCALE
        };
        self.scale = Fade::at(from_scale);
        self.scale
            .retarget(1.0, now, 0.0, OPEN_DURATION, CubicBezier::EASE_OUT);
        let header = if browse {
            Header::Browse
        } else {
            Header::Search
        };
        self.header = (header, header);
        if browse {
            if !closing {
                self.content = Fade::at(0.0);
            }
            self.content
                .retarget(1.0, now, CONTENT_DELAY, CONTENT_FADE, CubicBezier::EASE_OUT);
        } else {
            self.content
                .retarget(0.0, now, 0.0, 0.0, CubicBezier::LINEAR);
        }
    }

    /// The panel goes at `now`; `browse` while the browser is up (it also shrinks a little).
    pub fn hide(&mut self, browse: bool, now: f64) {
        let duration = if browse {
            CLOSE_DURATION
        } else {
            CLOSE_COMPACT_DURATION
        };
        self.alpha
            .retarget(0.0, now, 0.0, duration, CubicBezier::EASE_OUT);
        if browse && !self.reduce {
            self.scale
                .retarget(ENTER_SCALE, now, 0.0, duration, CubicBezier::EASE_OUT);
        }
    }

    /// The browser folds into the search field's `compact` shape at `now`.
    pub fn fold(&mut self, compact: Shape, now: f64) {
        self.content
            .retarget(0.0, now, 0.0, FOLD_FADE, CubicBezier::LINEAR);
        let wait = if self.reduce { 0.0 } else { FOLD_FADE };
        self.switch_header(Header::Search, now, now + wait);
        self.shape_to(compact, now, wait);
    }

    /// The search field unfolds into the browser's `open` shape at `now`.
    pub fn unfold(&mut self, open: Shape, now: f64) {
        self.switch_header(Header::Browse, now, now);
        self.shape_to(open, now, 0.0);
        let delay = if self.reduce {
            0.0
        } else {
            UNFOLD_CONTENT_DELAY
        };
        self.content
            .retarget(1.0, now, delay, UNFOLD_CONTENT_FADE, CubicBezier::EASE_OUT);
    }

    /// The search panel takes `shape` (its results changed) at `now`.
    pub fn reshape(&mut self, shape: Shape, now: f64) {
        if shape != self.target {
            self.shape_to(shape, now, 0.0);
        }
    }

    /// Put every transition at its end (the settled test clock).
    pub fn settle(&mut self, now: f64) {
        self.shape.set_immediately(self.target.array());
        for fade in [&mut self.alpha, &mut self.scale, &mut self.content] {
            *fade = Fade::at(fade.to);
        }
        self.header = (self.header.1, self.header.1);
        self.switch_at = now;
    }

    /// The shape the panel is heading for.
    pub fn target(&self) -> Shape {
        self.target
    }

    /// What is presented at `now`.
    pub fn frame(&self, now: f64) -> Frame {
        Frame {
            shape: Shape::of(self.shape.value(now)),
            alpha: self.alpha.value(now).clamp(0.0, 1.0),
            scale: if self.reduce {
                1.0
            } else {
                self.scale.value(now)
            },
            content: self.content.value(now).clamp(0.0, 1.0),
            header: self.header_at(now),
        }
    }

    /// Whether anything still moves at `now`.
    pub fn animating(&self, now: f64) -> bool {
        self.shape.is_animating(now)
            || self.alpha.animating(now)
            || self.scale.animating(now)
            || self.content.animating(now)
            || now < self.switch_at
    }

    /// Whether the panel is fully gone at `now` (closed and its fade over).
    pub fn gone(&self, now: f64) -> bool {
        self.alpha.to == 0.0 && self.alpha.value(now) <= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPACT: Shape = Shape::new(640.0, 56.0, 28.0);
    const OPEN: Shape = Shape::new(842.0, 576.0, 24.0);
    const RESULTS: Shape = Shape::new(640.0, 430.0, 28.0);

    fn settled(t: f64) -> f64 {
        t + 2.0
    }

    #[test]
    fn opening_the_browser_shows_the_enclosure_before_the_content() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        let f = m.frame(0.0);
        assert_eq!(
            f.shape, OPEN,
            "the shape is set; the enclosure grows by its scale"
        );
        assert_eq!((f.alpha, f.scale, f.content), (0.0, ENTER_SCALE, 0.0));
        assert_eq!(f.header, Header::Browse);
        // the enclosure is well on its way while the content has not started
        let f = m.frame(CONTENT_DELAY);
        assert!(f.alpha > 0.5 && f.scale > ENTER_SCALE, "{f:?}");
        assert_eq!(f.content, 0.0);
        let f = m.frame(CONTENT_DELAY + CONTENT_FADE / 2.0);
        assert!(f.content > 0.0 && f.content < 1.0, "{f:?}");
        let f = m.frame(OPEN_DURATION.max(CONTENT_DELAY + CONTENT_FADE));
        assert_eq!((f.alpha, f.scale, f.content), (1.0, 1.0, 1.0));
        assert!(!m.animating(OPEN_DURATION.max(CONTENT_DELAY + CONTENT_FADE)));
    }

    #[test]
    fn folding_withdraws_the_content_then_contracts_with_the_top_edge_fixed() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        m.fold(COMPACT, 1.0);
        // the content goes first while the enclosure keeps its size and the browser's header
        let f = m.frame(1.0 + FOLD_FADE / 2.0);
        assert!(f.content < 1.0 && f.content > 0.0);
        assert_eq!(f.shape, OPEN);
        assert_eq!(f.header, Header::Browse);
        let f = m.frame(1.0 + FOLD_FADE);
        assert_eq!(f.content, 0.0);
        assert_eq!(
            f.header,
            Header::Search,
            "the search field's header once the content is gone"
        );
        assert_eq!(f.shape, OPEN, "the contraction starts after the fade");
        // then the shape closes in, monotonically, and settles on the capsule
        let mut last = OPEN;
        for i in 1..=40 {
            let s = m.frame(1.0 + FOLD_FADE + i as f64 * 0.01).shape;
            assert!(s.width <= last.width + 1e-9 && s.height <= last.height + 1e-9);
            assert!(s.radius <= s.height / 2.0 + 1e-9);
            last = s;
        }
        let near = m.frame(1.0 + FOLD_FADE + 0.2).shape;
        assert!(
            near.height - COMPACT.height < 0.1 * (OPEN.height - COMPACT.height),
            "{near:?}"
        );
        let end = m.frame(settled(1.0)).shape;
        assert!(
            (end.width - COMPACT.width).abs() < 0.5 && (end.height - COMPACT.height).abs() < 0.5
        );
        assert!(!m.animating(settled(1.0)));
    }

    #[test]
    fn a_fold_interrupted_by_an_unfold_turns_around_from_where_it_is() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        m.fold(COMPACT, 1.0);
        let t = 1.0 + FOLD_FADE + 0.08;
        let before = m.frame(t - 0.001);
        let at = m.frame(t);
        assert!(at.shape.height < OPEN.height && at.shape.height > COMPACT.height);
        m.unfold(OPEN, t);
        let after = m.frame(t);
        // no jump in the shape or the content
        assert!((after.shape.height - at.shape.height).abs() < 1e-6);
        assert!((after.content - at.content).abs() < 1e-6);
        assert_eq!(after.header, Header::Browse);
        // the velocity carries: it is still closing for an instant, then opens
        let v_before = at.shape.height - before.shape.height;
        let next = m.frame(t + 0.001).shape.height - after.shape.height;
        assert!(v_before < 0.0);
        assert!(next < 1.0, "continuous velocity, no snap: {next}");
        let end = m.frame(settled(t));
        assert!((end.shape.height - OPEN.height).abs() < 0.5);
        assert_eq!(end.content, 1.0);
    }

    #[test]
    fn repeated_toggles_never_jump() {
        let mut m = Motion::new(COMPACT);
        let mut t = 0.0;
        let mut browse = true;
        m.show(OPEN, true, t);
        for _ in 0..20 {
            t += 0.037;
            let before = m.frame(t);
            if browse {
                m.fold(COMPACT, t);
            } else {
                m.unfold(OPEN, t);
            }
            browse = !browse;
            let after = m.frame(t);
            assert!((after.shape.width - before.shape.width).abs() < 1e-6);
            assert!((after.shape.height - before.shape.height).abs() < 1e-6);
            assert!((after.alpha - before.alpha).abs() < 1e-6);
            assert!((after.content - before.content).abs() < 1e-6);
        }
    }

    #[test]
    fn opening_while_closing_turns_around_from_the_presented_opacity() {
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.hide(false, 1.0);
        let mid = m.frame(1.0 + CLOSE_COMPACT_DURATION / 2.0);
        assert!(mid.alpha > 0.0 && mid.alpha < 1.0);
        m.show(COMPACT, false, 1.0 + CLOSE_COMPACT_DURATION / 2.0);
        let f = m.frame(1.0 + CLOSE_COMPACT_DURATION / 2.0);
        assert!((f.alpha - mid.alpha).abs() < 1e-9, "no flash to zero");
        assert!(!m.gone(1.0 + CLOSE_COMPACT_DURATION));
        m.hide(false, 5.0);
        assert!(m.gone(5.0 + CLOSE_COMPACT_DURATION));
    }

    #[test]
    fn closing_the_browser_fades_and_shrinks_and_the_compact_field_only_fades_quicker() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        m.hide(true, 1.0);
        let f = m.frame(1.0 + CLOSE_DURATION / 2.0);
        assert!(f.alpha < 1.0 && f.scale < 1.0);
        assert!(m.gone(1.0 + CLOSE_DURATION));
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.hide(false, 1.0);
        assert_eq!(m.frame(1.0 + CLOSE_COMPACT_DURATION / 2.0).scale, 1.0);
        assert!(m.gone(1.0 + CLOSE_COMPACT_DURATION));
    }

    #[test]
    fn under_reduce_motion_shapes_change_at_once_and_only_opacities_fade() {
        let mut m = Motion::new(COMPACT);
        m.set_reduce_motion(true);
        m.show(OPEN, true, 0.0);
        assert_eq!(m.frame(0.0).scale, 1.0);
        m.fold(COMPACT, 1.0);
        let f = m.frame(1.0);
        assert_eq!(f.shape, COMPACT);
        assert_eq!(f.header, Header::Search);
        assert!(f.content > 0.0, "the content still fades out");
        assert_eq!(m.frame(1.0 + FOLD_FADE).content, 0.0);
        m.unfold(OPEN, 2.0);
        assert_eq!(m.frame(2.0).shape, OPEN);
        m.hide(true, 3.0);
        assert_eq!(m.frame(3.0 + CLOSE_DURATION / 2.0).scale, 1.0);
    }

    #[test]
    fn the_search_panel_grows_with_its_results_along_the_spring() {
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.reshape(RESULTS, 1.0);
        let f = m.frame(1.05).shape;
        assert!(f.height > COMPACT.height && f.height < RESULTS.height);
        assert_eq!(m.target(), RESULTS);
        // the union is what a window must hold while the shape moves
        let u = COMPACT.union(OPEN);
        assert_eq!((u.width, u.height), (OPEN.width, OPEN.height));
        m.settle(1.05);
        assert_eq!(m.frame(1.05).shape, RESULTS);
        assert!(!m.animating(1.05));
    }
}
