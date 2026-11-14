//! How the panel moves between its views: one presentation of the enclosure (its shape, its opacity and scale), the
//! header (the search field's, or the browser's, or the file scope's), the content inside it and the mode buttons
//! beside the search field, which the panel's paint, clip and input region all read, so they always agree.
//!
//! The transitions are staged:
//!
//! * **Opening**: the enclosure starts a little larger than it settles (about 1.1) and shrinks to its size about its
//!   centre on an underdamped spring, so it undershoots a hair, while it fades in over a tenth of a second. A scope's
//!   content is part of it from the first frame: header, filters and grid fade in with the enclosure. With the mode buttons, the bar starts as one wide capsule that contracts while the
//!   buttons bud off its end as one shape and pinch apart, going a little past their places on a slower spring.
//! * **Folding** a scope into the search field: the content fades out at once, the header becomes the search
//!   field's, then the enclosure contracts along a spring, its top edge fixed and its sides closing in evenly, its
//!   corners growing into the field's capsule; the buttons bud out again once it has.
//! * **Unfolding** the search field into a scope: the buttons merge into the field at once, the enclosure opens along
//!   the spring and the content fades in as it opens. Into the files scope the enclosure's width also dips and comes
//!   back while its height grows.
//! * **Closing**: a tenth of a second in which the enclosure grows two percent, its content blurs and everything
//!   fades out.
//! * The search panel growing or shrinking with its results moves along the same spring.
//!
//! Every value is retargeted from what is presented at the moment a new transition starts, with its velocity, so a
//! transition can be interrupted at any point (fold during an unfold, open during a close) without a jump. Under
//! Reduce Motion the shape changes at once, nothing scales, blurs or buds and only the opacities still fade.
//!
//! The model is pure: every value is sampled at a time passed in.

use cupertino_animation::{AnimatedValue, CubicBezier, Curve, Spring, SpringParams};

/// The enclosure fading in when the launcher opens, in seconds.
pub const OPEN_FADE: f64 = 0.10;
/// The scale the enclosure starts at when the launcher opens, and the spring that settles it to 1 about its centre:
/// its response in seconds and its damping ratio (under 1: it undershoots a little).
pub const ENTER_SCALE: f64 = 1.1;
pub const OPEN_RESPONSE: f64 = 0.30;
pub const OPEN_DAMPING: f64 = 0.74;
/// The content fading out when a scope folds; the enclosure starts contracting after it.
pub const FOLD_FADE: f64 = 0.05;
/// The enclosure's spring between shapes: its response and damping ratio when it grows and when it shrinks. Both
/// overshoot a few points.
pub const GROW_RESPONSE: f64 = 0.27;
pub const GROW_DAMPING: f64 = 0.83;
pub const SHRINK_RESPONSE: f64 = 0.295;
pub const SHRINK_DAMPING: f64 = 0.86;
/// The spring's tail: once every edge is this many points from its target, and still is a frame ([`REST_LOOKAHEAD`]
/// seconds) later, the shape is at its target. The tail moves nothing a display shows, and presenting it would
/// reshape the glass under the panel on every repaint long after the motion ended. A spring past its settling time
/// presents its target too.
pub const SHAPE_REST: f64 = 1.0 / 64.0;
pub const REST_LOOKAHEAD: f64 = 1.0 / 60.0;
/// The content fading in as a scope unfolds: its delay and its duration, and the same for the files scope, whose
/// grid waits for the enclosure to have grown.
pub const UNFOLD_CONTENT_DELAY: f64 = 0.06;
pub const UNFOLD_CONTENT_FADE: f64 = 0.12;
pub const FILES_CONTENT_DELAY: f64 = 0.16;
pub const FILES_CONTENT_FADE: f64 = 0.10;
/// Into the files scope from the search field, the header switches and the enclosure starts growing this long after
/// the buttons merged into the field and the width began its dip.
pub const FILES_GROW_DELAY: f64 = 0.09;
/// Closing: how long it takes, how much the enclosure grows, and how far the content blurs, in points.
pub const CLOSE_DURATION: f64 = 0.10;
pub const CLOSE_SCALE: f64 = 1.02;
pub const CLOSE_BLUR: f64 = 8.0;
/// The mode buttons budding out of the field: the field's end springs back (response, damping ratio) while the
/// buttons slide out on a slower spring that carries them a little past their places.
pub const FIELD_RESPONSE: f64 = 0.305;
pub const FIELD_DAMPING: f64 = 0.68;
pub const SPLIT_RESPONSE: f64 = 0.575;
pub const SPLIT_DAMPING: f64 = 0.69;
/// The buttons' symbols sharpening after they bud: their delay and duration.
pub const GLYPH_DELAY: f64 = 0.25;
pub const GLYPH_SHARPEN: f64 = 0.2;
/// The buttons merging back into the field when a scope opens.
pub const MERGE_DURATION: f64 = 0.05;
/// How long the buttons wait after a scope folds before they bud out of the field again: the field settles into its
/// capsule and rests a while first.
pub const RESPLIT_DELAY: f64 = 1.1;
/// The width the enclosure dips by while the files scope opens, how long the dip takes to reach its depth, and the
/// response of the spring that brings the width back.
pub const DIP_DEPTH: f64 = 123.0;
pub const DIP_IN: f64 = 0.14;
pub const DIP_RESPONSE: f64 = 0.25;
/// Past this long after it starts, a dip is over.
const DIP_END: f64 = DIP_IN + 1.2;

/// The widest the mode buttons and the field can be together: the width of the field without the buttons.
pub const GROUP_WIDTH: f64 = 640.0;

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
    Files,
}

impl Header {
    /// Whether the header belongs to a scope panel (not the search field's).
    pub fn is_scope(self) -> bool {
        self != Header::Search
    }
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

/// The mode buttons as presented: how many, how far they are out of the field and how sharp their symbols are.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modes {
    /// How many buttons show; none once they merged into the field.
    pub count: usize,
    /// How far they are from the field's end to their places: 0 inside the field, 1 in place, a little past 1 while
    /// they overshoot.
    pub spread: f64,
    /// How sharp their symbols are, from 0 (blurred) to 1.
    pub sharp: f64,
}

/// What is presented at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The enclosure as it is drawn: while the mode buttons show, only the search field's part of the group.
    pub shape: Shape,
    /// The width the enclosure and the buttons together are placed by: the left edge of the group is half of it left
    /// of the middle. It is the shape's own width, or the search field's whatever the buttons make of the shape.
    pub anchor: f64,
    /// The enclosure's opacity (shadow and content included) and its scale about its centre: `scale` across, and
    /// `scale_y` down, which stays 1 while the search field opens (it widens, it does not grow taller).
    pub alpha: f64,
    pub scale: f64,
    pub scale_y: f64,
    /// The scope's content opacity (filters, items, separators).
    pub content: f64,
    /// How blurred the content is while the panel closes, from 0 to 1.
    pub blur: f64,
    pub header: Header,
    pub modes: Modes,
}

/// The panel's presentation.
#[derive(Clone, Debug)]
pub struct Motion {
    shape: AnimatedValue<[f64; 3]>,
    alpha: Fade,
    scale: AnimatedValue<f64>,
    content: Fade,
    blur: Fade,
    /// The header shown before `switch_at`, and the one from then on.
    header: (Header, Header),
    switch_at: f64,
    reduce: bool,
    /// The shape the panel is heading for.
    target: Shape,
    /// How many mode buttons the panel wants, and how many it lays out while they are out of the field.
    modes_wanted: usize,
    modes_laid: usize,
    /// How far the buttons are out of the field, and how far the field's end has drawn back for them.
    split: AnimatedValue<f64>,
    field: AnimatedValue<f64>,
    sharp: Fade,
    /// When the width's dip began, while the files scope opens.
    dip: Option<f64>,
    /// The enclosure scales across only: the search field opening.
    widen_only: bool,
}

impl Motion {
    pub fn new(shape: Shape) -> Self {
        Self {
            shape: AnimatedValue::new(shape.array()),
            alpha: Fade::at(0.0),
            scale: AnimatedValue::new(1.0),
            content: Fade::at(0.0),
            blur: Fade::at(0.0),
            header: (Header::Search, Header::Search),
            switch_at: 0.0,
            reduce: false,
            target: shape,
            modes_wanted: 0,
            modes_laid: 0,
            split: AnimatedValue::new(0.0),
            field: AnimatedValue::new(0.0),
            sharp: Fade::at(0.0),
            dip: None,
            widen_only: false,
        }
    }

    /// Follow the session's Reduce Motion option from the next transition on.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce = on;
    }

    pub fn reduce_motion(&self) -> bool {
        self.reduce
    }

    fn spring(grow: bool) -> Curve {
        Curve::Spring(if grow {
            SpringParams::from_ratio(GROW_RESPONSE, GROW_DAMPING)
        } else {
            SpringParams::from_ratio(SHRINK_RESPONSE, SHRINK_DAMPING)
        })
    }

    /// Move the shape to `to`, starting `delay` from `now`; at once under Reduce Motion. A shape still moving turns
    /// at once, from where it is and with its velocity: waiting would hold it still mid-way.
    fn shape_to(&mut self, to: Shape, now: f64, delay: f64) {
        self.target = to;
        if self.reduce {
            self.shape.set_immediately(to.array());
            return;
        }
        let grow = to.height >= self.shape.value(now)[1];
        if self.shape.is_animating(now) {
            self.shape.animate_to(to.array(), Self::spring(grow), now);
        } else {
            self.shape.cancel(now);
            self.shape
                .animate_to(to.array(), Self::spring(grow), now + delay);
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
        self.show_in(
            shape,
            if browse {
                Header::Browse
            } else {
                Header::Search
            },
            now,
        );
    }

    /// The panel appears at `now` with `shape`, showing the view of `header`.
    pub fn show_in(&mut self, shape: Shape, header: Header, now: f64) {
        let closing = self.alpha.value(now) > 0.0;
        if !closing {
            self.shape.set_immediately(shape.array());
            self.target = shape;
        } else {
            self.shape_to(shape, now, 0.0);
        }
        self.alpha
            .retarget(1.0, now, 0.0, OPEN_FADE, CubicBezier::LINEAR);
        self.blur.retarget(0.0, now, 0.0, 0.0, CubicBezier::LINEAR);
        if self.reduce {
            self.scale.set_immediately(1.0);
        } else {
            if !closing {
                self.scale.set_immediately(ENTER_SCALE);
            }
            self.scale.animate_to(
                1.0,
                Curve::Spring(SpringParams::from_ratio(OPEN_RESPONSE, OPEN_DAMPING)),
                now,
            );
        }
        self.header = (header, header);
        self.dip = None;
        // the search field opens wider than it settles, never taller; a scope panel grows both ways
        if !closing {
            self.widen_only = header == Header::Search;
        }
        if header.is_scope() {
            // the content fades in with the enclosure, not after it: shown at once, the enclosure's fade carries it;
            // reopened while closing, it comes back alongside the enclosure
            if closing {
                self.content
                    .retarget(1.0, now, 0.0, OPEN_FADE, CubicBezier::LINEAR);
            } else {
                self.content = Fade::at(1.0);
            }
        } else {
            self.content
                .retarget(0.0, now, 0.0, 0.0, CubicBezier::LINEAR);
        }
        // the buttons start inside the field and bud out as it opens
        if !closing {
            self.split.set_immediately(0.0);
            self.field.set_immediately(0.0);
            self.sharp = Fade::at(0.0);
            self.modes_laid = 0;
            self.modes_wanted = 0;
        }
        self.bud(now, 0.0);
    }

    /// The panel goes at `now`: it grows a little, its content blurs and it fades out.
    pub fn hide(&mut self, now: f64) {
        self.alpha
            .retarget(0.0, now, 0.0, CLOSE_DURATION, CubicBezier::EASE_OUT);
        if !self.reduce {
            self.scale.animate_to(
                CLOSE_SCALE,
                Curve::Timed {
                    duration: CLOSE_DURATION,
                    bezier: CubicBezier::EASE_OUT,
                },
                now,
            );
            self.blur
                .retarget(1.0, now, 0.0, CLOSE_DURATION, CubicBezier::EASE_OUT);
        }
    }

    /// A scope folds into the search field's `compact` shape at `now`.
    pub fn fold(&mut self, compact: Shape, now: f64) {
        self.content
            .retarget(0.0, now, 0.0, FOLD_FADE, CubicBezier::LINEAR);
        let wait = if self.reduce { 0.0 } else { FOLD_FADE };
        self.switch_header(Header::Search, now, now + wait);
        self.shape_to(compact, now, wait);
        self.dip = None;
    }

    /// The search field unfolds into the browser's `open` shape at `now`.
    pub fn unfold(&mut self, open: Shape, now: f64) {
        self.unfold_to(open, Header::Browse, now);
    }

    /// The search field, or another scope, unfolds into the shape `open` of the scope of `header` at `now`.
    pub fn unfold_to(&mut self, open: Shape, header: Header, now: f64) {
        let from_field = self.header_at(now) == Header::Search;
        // into the files scope the width dips at once while the header and the growth wait a moment
        let grow = if header == Header::Files && from_field && !self.reduce {
            FILES_GROW_DELAY
        } else {
            0.0
        };
        self.switch_header(header, now, now + grow);
        self.shape_to(open, now, grow);
        let (delay, fade) = if self.reduce {
            (0.0, UNFOLD_CONTENT_FADE)
        } else if header == Header::Files {
            (FILES_CONTENT_DELAY, FILES_CONTENT_FADE)
        } else {
            (UNFOLD_CONTENT_DELAY, UNFOLD_CONTENT_FADE)
        };
        self.content
            .retarget(1.0, now, delay, fade, CubicBezier::EASE_OUT);
        self.dip = (header == Header::Files && from_field && !self.reduce).then_some(now);
    }

    /// The search panel takes `shape` (its results changed) at `now`.
    pub fn reshape(&mut self, shape: Shape, now: f64) {
        if shape != self.target {
            self.shape_to(shape, now, 0.0);
        }
    }

    /// The panel wants `count` mode buttons beside the field from `now` on (none: they merge into it). `delay` is how
    /// long the buttons wait before they bud out.
    pub fn set_modes(&mut self, count: usize, now: f64, delay: f64) {
        if count == self.modes_wanted {
            return;
        }
        self.modes_wanted = count;
        if count == 0 {
            self.merge(now);
        } else {
            self.bud(now, delay);
        }
    }

    /// The buttons bud out of the field's end after `delay`.
    fn bud(&mut self, now: f64, delay: f64) {
        if self.modes_wanted == 0 {
            return;
        }
        self.modes_laid = self.modes_wanted;
        if self.reduce {
            self.split.set_immediately(1.0);
            self.field.set_immediately(1.0);
            self.sharp = Fade::at(1.0);
            return;
        }
        let spring = |response, damping| Curve::Spring(SpringParams::from_ratio(response, damping));
        self.field
            .animate_to(1.0, spring(FIELD_RESPONSE, FIELD_DAMPING), now + delay);
        self.split
            .animate_to(1.0, spring(SPLIT_RESPONSE, SPLIT_DAMPING), now + delay);
        self.sharp = Fade::at(0.0);
        self.sharp.retarget(
            1.0,
            now,
            delay + GLYPH_DELAY,
            GLYPH_SHARPEN,
            CubicBezier::EASE_OUT,
        );
    }

    /// The buttons merge back into the field.
    fn merge(&mut self, now: f64) {
        if self.reduce {
            self.split.set_immediately(0.0);
            self.field.set_immediately(0.0);
            self.sharp = Fade::at(0.0);
            return;
        }
        let timed = Curve::Timed {
            duration: MERGE_DURATION,
            bezier: CubicBezier::EASE_OUT,
        };
        self.field.animate_to(0.0, timed, now);
        self.split.animate_to(0.0, timed, now);
    }

    /// Put every transition at its end (the settled test clock).
    pub fn settle(&mut self, now: f64) {
        self.shape.set_immediately(self.target.array());
        for fade in [
            &mut self.alpha,
            &mut self.content,
            &mut self.blur,
            &mut self.sharp,
        ] {
            *fade = Fade::at(fade.to);
        }
        let scale = if self.alpha.to > 0.0 {
            1.0
        } else {
            CLOSE_SCALE
        };
        self.scale.set_immediately(scale);
        let out = if self.modes_wanted > 0 { 1.0 } else { 0.0 };
        self.split.set_immediately(out);
        self.field.set_immediately(out);
        if self.modes_wanted > 0 {
            self.modes_laid = self.modes_wanted;
        }
        self.dip = None;
        self.header = (self.header.1, self.header.1);
        self.switch_at = now;
    }

    /// The shape the panel is heading for.
    pub fn target(&self) -> Shape {
        self.target
    }

    /// How many mode buttons the panel wants.
    pub fn modes_wanted(&self) -> usize {
        self.modes_wanted
    }

    /// Whether the shape is within [`SHAPE_REST`] of its target at `now` and a frame later.
    fn shape_rests(&self, now: f64) -> bool {
        let target = self.target.array();
        let near = |at: f64| {
            let value = self.shape.value(at);
            (0..3).all(|i| (value[i] - target[i]).abs() <= SHAPE_REST)
        };
        near(now) && near(now + REST_LOOKAHEAD)
    }

    /// How far the dip draws the width in at `now`, in points (zero or negative).
    fn dip_at(&self, now: f64) -> f64 {
        let Some(start) = self.dip else {
            return 0.0;
        };
        let t = now - start;
        if t <= 0.0 || t >= DIP_END {
            return 0.0;
        }
        if t < DIP_IN {
            let x = t / DIP_IN;
            return -DIP_DEPTH * CubicBezier::EASE_IN_EASE_OUT.eval(x);
        }
        let back = Spring::new(SpringParams::from_ratio(DIP_RESPONSE, 1.0)).value(t - DIP_IN);
        -DIP_DEPTH * (1.0 - back)
    }

    /// What is presented at `now`.
    pub fn frame(&self, now: f64) -> Frame {
        let mut shape = if !self.shape.is_animating(now) || self.shape_rests(now) {
            self.target
        } else {
            Shape::of(self.shape.value(now))
        };
        let anchor = shape.width.max(GROUP_WIDTH);
        shape.width = (shape.width + self.dip_at(now)).max(0.0);
        let header = self.header_at(now);
        let spread = shown(&self.split, now);
        let drawn = shown(&self.field, now);
        let out = spread.abs() > 1e-3 || drawn.abs() > 1e-3;
        let count = if out && header == Header::Search {
            self.modes_laid
        } else {
            0
        };
        if count > 0 {
            // the field's end draws back from the whole group's width to the field's own
            let field = crate::modes::field_width(count);
            shape.width = (shape.width - (shape.width - field) * drawn).max(0.0);
        }
        Frame {
            shape,
            anchor,
            alpha: self.alpha.value(now).clamp(0.0, 1.0),
            scale: if self.reduce {
                1.0
            } else {
                shown(&self.scale, now)
            },
            scale_y: if self.reduce || self.widen_only {
                1.0
            } else {
                shown(&self.scale, now)
            },
            content: self.content.value(now).clamp(0.0, 1.0),
            blur: self.blur.value(now).clamp(0.0, 1.0),
            header,
            modes: Modes {
                count,
                spread,
                sharp: self.sharp.value(now).clamp(0.0, 1.0),
            },
        }
    }

    /// Whether anything still moves at `now`.
    pub fn animating(&self, now: f64) -> bool {
        (self.shape.is_animating(now) && !self.shape_rests(now))
            || self.alpha.animating(now)
            || self.scale.is_animating(now)
            || self.content.animating(now)
            || self.blur.animating(now)
            || self.split.is_animating(now)
            || self.field.is_animating(now)
            || self.sharp.animating(now)
            || self.dip.is_some_and(|start| now < start + DIP_END)
            || now < self.switch_at
    }

    /// Whether the panel is fully gone at `now` (closed and its fade over).
    pub fn gone(&self, now: f64) -> bool {
        self.alpha.to == 0.0 && self.alpha.value(now) <= 0.0
    }

    /// How long the panel's fade out still runs at `now`, in seconds; 0 once it is gone or while it is not closing.
    pub fn closing_left(&self, now: f64) -> f64 {
        if self.alpha.to != 0.0 {
            return 0.0;
        }
        (self.alpha.start + self.alpha.duration - now).max(0.0)
    }

    /// How long from `now` until nothing moves, to the next [`MOVING_STEP`]; 0 when nothing does, and at most
    /// [`MOVING_MOST`].
    pub fn moving_left(&self, now: f64) -> f64 {
        let steps = (MOVING_MOST / MOVING_STEP) as usize;
        (0..=steps)
            .map(|i| i as f64 * MOVING_STEP)
            .find(|&t| !self.animating(now + t))
            .unwrap_or(MOVING_MOST)
    }
}

/// The step and the bound of [`Motion::moving_left`]'s search, in seconds.
const MOVING_STEP: f64 = 1.0 / 120.0;
const MOVING_MOST: f64 = 4.0;

/// What `value` presents at `now`: its target once it no longer moves. A spring past its settling time is a hair off
/// its target for ever; presenting that hair would leave the panel's place and the text in it a little off the
/// device pixels they settle on.
fn shown(value: &AnimatedValue<f64>, now: f64) -> f64 {
    if value.is_animating(now) {
        value.value(now)
    } else {
        value.target()
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
    fn opening_the_browser_fades_its_content_in_with_the_enclosure() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        let f = m.frame(0.0);
        assert_eq!(
            f.shape, OPEN,
            "the shape is set; the enclosure grows by its scale"
        );
        assert_eq!((f.alpha, f.scale, f.content), (0.0, ENTER_SCALE, 1.0));
        assert_eq!(f.header, Header::Browse);
        // half way through the enclosure's fade the content shows as much as the enclosure does: no stagger
        let f = m.frame(OPEN_FADE / 2.0);
        assert!(
            f.alpha > 0.3 && f.alpha < 0.7 && f.scale < ENTER_SCALE,
            "{f:?}"
        );
        assert_eq!(f.content, 1.0);
        let f = m.frame(settled(0.0));
        assert_eq!((f.alpha, f.content), (1.0, 1.0));
        assert!((f.scale - 1.0).abs() < 1e-9, "{f:?}");
        assert!(!m.animating(settled(0.0)));
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
        // then the shape closes in, going past the capsule by a few points at most, and settles on it
        for i in 1..=40 {
            let s = m.frame(1.0 + FOLD_FADE + i as f64 * 0.01).shape;
            assert!(s.width <= OPEN.width + 1e-9 && s.height <= OPEN.height + 1e-9);
            assert!(
                s.width >= COMPACT.width - 8.0 && s.height >= COMPACT.height - 8.0,
                "{s:?}"
            );
            assert!(s.radius <= s.height / 2.0 + 1e-9);
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
        m.hide(1.0);
        let mid = m.frame(1.0 + CLOSE_DURATION / 2.0);
        assert!(mid.alpha > 0.0 && mid.alpha < 1.0);
        m.show(COMPACT, false, 1.0 + CLOSE_DURATION / 2.0);
        let f = m.frame(1.0 + CLOSE_DURATION / 2.0);
        assert!((f.alpha - mid.alpha).abs() < 1e-9, "no flash to zero");
        assert!(!m.gone(1.0 + CLOSE_DURATION));
        m.hide(5.0);
        assert!(m.gone(5.0 + CLOSE_DURATION));
    }

    #[test]
    fn closing_fades_grows_a_little_and_blurs_whatever_the_panel_shows() {
        for (shape, browse) in [(OPEN, true), (COMPACT, false)] {
            let mut m = Motion::new(COMPACT);
            m.show(shape, browse, 0.0);
            m.hide(settled(0.0));
            let f = m.frame(settled(0.0) + CLOSE_DURATION / 2.0);
            assert!(f.alpha > 0.0 && f.alpha < 1.0, "{f:?}");
            assert!(f.scale > 1.0 && f.scale < CLOSE_SCALE, "{f:?}");
            assert!(f.blur > 0.0 && f.blur < 1.0, "{f:?}");
            assert!(m.gone(settled(0.0) + CLOSE_DURATION));
        }
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
        m.hide(3.0);
        let f = m.frame(3.0 + CLOSE_DURATION / 2.0);
        assert_eq!((f.scale, f.blur), (1.0, 0.0));
    }

    #[test]
    fn the_spring_s_tail_presents_the_target_and_ends_the_motion() {
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.shape_to(RESULTS, 0.0, 0.0);
        // mid-way it moves
        assert!(m.animating(0.1));
        assert_ne!(m.frame(0.1).shape, RESULTS);
        let rest = (1..400)
            .map(|i| f64::from(i) / 120.0)
            .find(|&t| !m.animating(t))
            .unwrap();
        // a frame not yet at the target is never taken for the end of the motion
        for i in 1..(rest * 120.0).round() as u32 {
            let t = f64::from(i) / 120.0;
            if m.frame(t).shape != RESULTS {
                assert!(m.animating(t), "at {t}");
            }
        }
        for i in 0..120 {
            assert_eq!(m.frame(rest + f64::from(i) / 60.0).shape, RESULTS);
        }
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

    #[test]
    fn the_search_field_opens_wider_but_never_taller() {
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        let f = m.frame(0.0);
        assert_eq!((f.scale, f.scale_y), (ENTER_SCALE, 1.0));
        for i in 1..40 {
            let f = m.frame(f64::from(i) / 120.0);
            assert_eq!(f.scale_y, 1.0, "{f:?}");
        }
        // a scope panel grows both ways
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        let f = m.frame(0.0);
        assert_eq!((f.scale, f.scale_y), (ENTER_SCALE, ENTER_SCALE));
    }

    #[test]
    fn once_its_springs_settle_the_panel_rests_exactly_at_its_place() {
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.set_modes(2, 0.0, 0.0);
        let rest = (1..400)
            .map(|i| f64::from(i) / 120.0)
            .find(|&t| !m.animating(t))
            .unwrap();
        let f = m.frame(rest);
        assert_eq!((f.scale, f.scale_y), (1.0, 1.0));
        assert_eq!(f.modes.spread, 1.0);
        assert_eq!(f.shape.width, crate::modes::field_width(2));
    }

    #[test]
    fn the_opening_spring_undershoots_by_about_half_a_percent() {
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        let least = (1..120)
            .map(|i| m.frame(f64::from(i) / 240.0).scale)
            .fold(f64::INFINITY, f64::min);
        assert!(least < 1.0 && least > 0.99, "{least}");
    }

    #[test]
    fn into_the_files_scope_the_width_dips_before_the_header_switches_and_the_panel_grows() {
        let files = Shape::new(640.0, 547.0, 28.5);
        let mut m = Motion::new(COMPACT);
        m.show(COMPACT, false, 0.0);
        m.settle(0.0);
        m.unfold_to(files, Header::Files, 1.0);
        let early = m.frame(1.0 + FILES_GROW_DELAY / 2.0);
        assert_eq!(early.header, Header::Search);
        assert_eq!(early.shape.height, COMPACT.height);
        assert!(
            early.shape.width < COMPACT.width,
            "the dip has begun: {early:?}"
        );
        let later = m.frame(1.0 + FILES_GROW_DELAY + 0.05);
        assert_eq!(later.header, Header::Files);
        assert!(later.shape.height > COMPACT.height, "{later:?}");
        // out of the browser the files scope switches at once
        let mut m = Motion::new(COMPACT);
        m.show(OPEN, true, 0.0);
        m.settle(0.0);
        m.unfold_to(files, Header::Files, 1.0);
        assert_eq!(m.frame(1.0).header, Header::Files);
    }
}
