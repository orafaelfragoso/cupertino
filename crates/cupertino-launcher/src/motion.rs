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

use cupertino_kit::motion::{Animated, SpringPreset, Timing};

/// The enclosure fading in when the launcher opens.
pub const OPEN_FADE: Timing = Timing::LAUNCHER_OPEN_FADE;
/// The scale the enclosure starts at when the launcher opens; the catalogue's `SpringPreset::LAUNCHER_OPEN`, whose
/// damping ratio is under 1 so it undershoots a little, settles it to 1 about its centre.
pub const ENTER_SCALE: f64 = 1.1;
/// The content fading out when a scope folds; the enclosure starts contracting after it.
pub const FOLD_FADE: Timing = Timing::LAUNCHER_FOLD_FADE;
/// The enclosure's spring between shapes when it grows and when it shrinks. Both overshoot a few points.
pub const GROW: SpringPreset = SpringPreset::LAUNCHER_GROW;
pub const SHRINK: SpringPreset = SpringPreset::LAUNCHER_SHRINK;
/// The spring's tail: once every edge is this many points from its target, and still is a frame ([`REST_LOOKAHEAD`]
/// seconds) later, the shape is at its target. The tail moves nothing a display shows, and presenting it would
/// reshape the glass under the panel on every repaint long after the motion ended. A spring past its settling time
/// presents its target too.
pub const SHAPE_REST: f64 = 1.0 / 64.0;
pub const REST_LOOKAHEAD: f64 = 1.0 / 60.0;
/// The content fading in as a scope unfolds: its delay in seconds and its fade, and the same for the files scope,
/// whose grid waits for the enclosure to have grown.
pub const UNFOLD_CONTENT_DELAY: f64 = 0.06;
pub const UNFOLD_CONTENT_FADE: Timing = Timing::LAUNCHER_UNFOLD_CONTENT;
pub const FILES_CONTENT_DELAY: f64 = 0.16;
pub const FILES_CONTENT_FADE: Timing = Timing::LAUNCHER_FILES_CONTENT;
/// Into the files scope from the search field, the header switches and the enclosure starts growing this long after
/// the buttons merged into the field and the width began its dip.
pub const FILES_GROW_DELAY: f64 = 0.09;
/// Closing: its motion, how much the enclosure grows, and how far the content blurs, in points.
pub const CLOSE: Timing = Timing::LAUNCHER_CLOSE;
pub const CLOSE_SCALE: f64 = 1.02;
pub const CLOSE_BLUR: f64 = 8.0;
/// The mode buttons budding out of the field: the field's end springs back while the buttons slide out on a slower
/// spring that carries them a little past their places.
pub const FIELD: SpringPreset = SpringPreset::LAUNCHER_FIELD;
pub const SPLIT: SpringPreset = SpringPreset::LAUNCHER_SPLIT;
/// The buttons' symbols sharpening after they bud: their delay in seconds and their motion.
pub const GLYPH_DELAY: f64 = 0.25;
pub const GLYPH_SHARPEN: Timing = Timing::LAUNCHER_GLYPH_SHARPEN;
/// The buttons merging back into the field when a scope opens.
pub const MERGE: Timing = Timing::LAUNCHER_MERGE;
/// How long the buttons wait after a scope folds before they bud out of the field again: the field settles into its
/// capsule and rests a while first.
pub const RESPLIT_DELAY: f64 = 1.1;
/// The width the enclosure dips by while the files scope opens, in points, the motion that takes it to its depth, and
/// the spring that brings the width back.
pub const DIP_DEPTH: f64 = 123.0;
pub const DIP_IN: Timing = Timing::LAUNCHER_DIP;
pub const DIP_BACK: SpringPreset = SpringPreset::LAUNCHER_DIP_BACK;
/// How long after the width starts coming back the dip is over, in seconds: the spring's tail is far below a point by
/// then.
const DIP_BACK_END: f64 = 1.2;

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

/// A value fading from where it is towards a target after a delay, along a timed motion of the catalogue; none to
/// jump.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    from: f64,
    to: f64,
    start: f64,
    timing: Option<Timing>,
}

impl Fade {
    const fn at(value: f64) -> Self {
        Fade {
            from: value,
            to: value,
            start: 0.0,
            timing: None,
        }
    }

    /// How long the fade takes, in seconds.
    fn duration(&self) -> f64 {
        self.timing.map_or(0.0, Timing::duration)
    }

    fn value(&self, now: f64) -> f64 {
        let duration = self.duration();
        let Some(timing) = self
            .timing
            .filter(|_| duration > 0.0 && now < self.start + duration)
        else {
            return if now >= self.start {
                self.to
            } else {
                self.from
            };
        };
        let elapsed = (now - self.start).clamp(0.0, duration);
        self.from + (self.to - self.from) * timing.value(elapsed)
    }

    /// Head for `to` from the value presented at `now`, starting `delay` later and moving along `timing`.
    fn retarget(&mut self, to: f64, now: f64, delay: f64, timing: Timing) {
        let from = self.value(now);
        *self = Fade {
            from,
            to,
            start: now + delay,
            timing: Some(timing),
        };
    }

    /// Jump to `to` at `now`.
    fn jump(&mut self, to: f64, now: f64) {
        let from = self.value(now);
        *self = Fade {
            from,
            to,
            start: now,
            timing: None,
        };
    }

    fn animating(&self, now: f64) -> bool {
        self.from != self.to && now < self.start + self.duration()
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
    shape: Animated<[f64; 3]>,
    alpha: Fade,
    scale: Animated<f64>,
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
    split: Animated<f64>,
    field: Animated<f64>,
    sharp: Fade,
    /// When the width's dip began, while the files scope opens.
    dip: Option<f64>,
    /// The enclosure scales across only: the search field opening.
    widen_only: bool,
}

impl Motion {
    pub fn new(shape: Shape) -> Self {
        Self {
            shape: Animated::new(shape.array()),
            alpha: Fade::at(0.0),
            scale: Animated::new(1.0),
            content: Fade::at(0.0),
            blur: Fade::at(0.0),
            header: (Header::Search, Header::Search),
            switch_at: 0.0,
            reduce: false,
            target: shape,
            modes_wanted: 0,
            modes_laid: 0,
            split: Animated::new(0.0),
            field: Animated::new(0.0),
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

    fn spring(grow: bool) -> Timing {
        Timing::from(if grow { GROW } else { SHRINK })
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

    /// When the header switches after `now`, and to which, while a switch waits.
    pub fn header_switch(&self, now: f64) -> Option<(f64, Header)> {
        (now < self.switch_at && self.header.0 != self.header.1)
            .then_some((self.switch_at, self.header.1))
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
        self.alpha.retarget(1.0, now, 0.0, OPEN_FADE);
        self.blur.jump(0.0, now);
        if self.reduce {
            self.scale.set_immediately(1.0);
        } else {
            if !closing {
                self.scale.set_immediately(ENTER_SCALE);
            }
            self.scale
                .animate_to(1.0, Timing::from(SpringPreset::LAUNCHER_OPEN), now);
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
                self.content.retarget(1.0, now, 0.0, OPEN_FADE);
            } else {
                self.content = Fade::at(1.0);
            }
        } else {
            self.content.jump(0.0, now);
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
        self.alpha.retarget(0.0, now, 0.0, CLOSE);
        if !self.reduce {
            self.scale.animate_to(CLOSE_SCALE, CLOSE, now);
            self.blur.retarget(1.0, now, 0.0, CLOSE);
        }
    }

    /// A scope folds into the search field's `compact` shape at `now`.
    pub fn fold(&mut self, compact: Shape, now: f64) {
        self.content.retarget(0.0, now, 0.0, FOLD_FADE);
        let wait = if self.reduce {
            0.0
        } else {
            FOLD_FADE.duration()
        };
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
        self.content.retarget(1.0, now, delay, fade);
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
        self.field.animate_to(1.0, Timing::from(FIELD), now + delay);
        self.split.animate_to(1.0, Timing::from(SPLIT), now + delay);
        self.sharp = Fade::at(0.0);
        self.sharp
            .retarget(1.0, now, delay + GLYPH_DELAY, GLYPH_SHARPEN);
    }

    /// The buttons merge back into the field.
    fn merge(&mut self, now: f64) {
        if self.reduce {
            self.split.set_immediately(0.0);
            self.field.set_immediately(0.0);
            self.sharp = Fade::at(0.0);
            return;
        }
        self.field.animate_to(0.0, MERGE, now);
        self.split.animate_to(0.0, MERGE, now);
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
        let dip_in = DIP_IN.duration();
        if t <= 0.0 || t >= dip_in + DIP_BACK_END {
            return 0.0;
        }
        if t < dip_in {
            return -DIP_DEPTH * DIP_IN.value(t);
        }
        let back = Timing::from(DIP_BACK).value(t - dip_in);
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
            || self.dip.is_some_and(|start| now < start + dip_end())
            || now < self.switch_at
    }

    /// Whether the enclosure still changes at `now`, its mode buttons aside: its shape, opacity, scale, content, blur,
    /// the width's dip or the header's switch.
    fn shaping(&self, now: f64) -> bool {
        (self.shape.is_animating(now) && !self.shape_rests(now))
            || self.alpha.animating(now)
            || self.scale.is_animating(now)
            || self.content.animating(now)
            || self.blur.animating(now)
            || self.dip.is_some_and(|start| now < start + dip_end())
            || now < self.switch_at
    }

    /// How long from `now` until the enclosure stops changing, its mode buttons aside, to the next [`MOVING_STEP`]:
    /// a fold's own length, after which the buttons bud out of the field again on their own delay.
    pub fn shaping_left(&self, now: f64) -> f64 {
        let steps = (MOVING_MOST / MOVING_STEP) as usize;
        (0..=steps)
            .map(|i| i as f64 * MOVING_STEP)
            .find(|&t| !self.shaping(now + t))
            .unwrap_or(MOVING_MOST)
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
        (self.alpha.start + self.alpha.duration() - now).max(0.0)
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

/// Past this long after it starts, a dip is over.
fn dip_end() -> f64 {
    DIP_IN.duration() + DIP_BACK_END
}

/// The step and the bound of [`Motion::moving_left`]'s search, in seconds.
const MOVING_STEP: f64 = 1.0 / 120.0;
const MOVING_MOST: f64 = 4.0;

/// What `value` presents at `now`: its target once it no longer moves. A spring past its settling time is a hair off
/// its target for ever; presenting that hair would leave the panel's place and the text in it a little off the
/// device pixels they settle on.
fn shown(value: &Animated<f64>, now: f64) -> f64 {
    if value.is_animating(now) {
        value.value(now)
    } else {
        value.target()
    }
}
