//! Text and password fields: the field's own buffer, its keys, the input-method conversation, submit and the model
//! sync. A password lives only in a [`SecureBuffer`]: no undo, no clipboard export, no surrounding text for the input
//! method, and assistive technology learns only that there is a value.

use crate::look::{self, paint_focus_ring};
use crate::{BareField, BareSecureField, SearchField, SecureField, TextChange, TextField};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, OverlayCx, PaintCx, Propagation, Role, SemanticsCx, TextSemantics,
    Widget,
};
use cupertino_ui_layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{
    Affinity, ChangeCause, ContentHints, ContentPurpose, ImeBatch, ImeEvent, Keymap,
    ParagraphLayout, Response, SecureBuffer, Selection, TextBuffer, TextFieldKeys,
    TextInputSession, TextPosition, CARET_WIDTH, SURROUNDING_MAX_BYTES,
};
use cupertino_ui_theme::{
    control_geometry, Color, ColorRole, ControlFamily, ControlSize, ThemeTokens,
};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::time::Duration;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);
const BULLET: char = '\u{2022}';
/// Points between a line's baseline and the top of the composition's underline.
pub(crate) const COMPOSITION_DROP: f64 = 1.5;
/// How thick the composition's underline is, in points.
const COMPOSITION_LINE: f64 = 1.0;
/// How thick the underline of the clause the input method is converting is, in points.
const CLAUSE_LINE: f64 = 2.0;
/// How often a drag held past a field's or an editor's edge scrolls on by itself, in nanoseconds; each step scrolls
/// as far as the pointer is past the edge.
pub(crate) const AUTOSCROLL_NS: u64 = 50_000_000;

/// A stroke `points` thick at `scale` in whole device pixels, at least one.
pub(crate) fn device_thickness(points: f64, scale: f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (points * scale).round().max(1.0) / scale
}

/// The bar a caret `caret` (a layout's caret rectangle, in node points) draws as at `scale`: [`CARET_WIDTH`] in whole
/// device pixels (at least one). The overlay that draws it puts its edges on the window's device pixel edges.
pub(crate) fn caret_bar(caret: Rect, scale: f64) -> Rect {
    Rect::new(
        caret.min_x(),
        caret.min_y(),
        device_thickness(CARET_WIDTH, scale),
        caret.height(),
    )
}

/// What a field or an editor draws over its paint ([`Widget::overlay`]), as its last paint placed it in its own
/// coordinates: the caret and the composition's underlines. The overlay puts them on whole device pixels of the
/// window wherever the field lies, and a blink draws only the overlay again.
#[derive(Debug, Default)]
pub(crate) struct Marks {
    /// The caret and its colour, while it can show: the field has the keyboard in an active window, nothing is
    /// selected and the input method shows its cursor. The blink decides when it does.
    caret: Cell<Option<(Rect, Color)>>,
    /// The composition's underlines and their colour.
    underlines: RefCell<Vec<(Rect, Color)>>,
    /// What they are clipped to, the field's interior; none for an editor, whose scroll view clips it.
    clip: Cell<Option<Rect>>,
}

impl Marks {
    /// Forgets the last paint's marks, before a paint places them again.
    pub(crate) fn clear(&self) {
        self.caret.set(None);
        self.underlines.borrow_mut().clear();
        self.clip.set(None);
    }

    pub(crate) fn caret(&self, bar: Rect, color: Option<Color>) {
        self.caret.set(color.map(|color| (bar, color)));
    }

    fn underline(&self, rect: Rect, color: Option<Color>) {
        if let Some(color) = color {
            self.underlines.borrow_mut().push((rect, color));
        }
    }

    fn clip(&self, rect: Rect) {
        self.clip.set(Some(rect));
    }

    /// Asks for the overlay when there is something to draw in it.
    pub(crate) fn finish(&self, cx: &mut PaintCx<'_>) {
        if self.caret.get().is_some() || !self.underlines.borrow().is_empty() {
            cx.overlay();
        }
    }

    /// Draws the underlines, and the caret in the on phase of `blink`.
    pub(crate) fn draw(&self, blink: &Blink, cx: &mut OverlayCx<'_>) {
        if let Some(clip) = self.clip.get() {
            cx.clip(clip);
        }
        for &(rect, color) in self.underlines.borrow().iter() {
            cx.fill_pixels(rect, color);
        }
        if let Some((bar, color)) = self.caret.get() {
            let opacity = blink.opacity(cx.env(), cx.now());
            if opacity > 0.0 {
                cx.fill_pixels(
                    bar,
                    Color::new(color.r, color.g, color.b, color.a * opacity),
                );
            }
        }
    }
}

/// The composition's underline under `composition` (bytes of `layout`'s text) on each line it covers,
/// [`COMPOSITION_DROP`] below the baseline, and a heavier one under `clause`, the part the input method is converting,
/// as marks for the overlay. The layout's origin is at `origin`, in node points.
pub(crate) fn mark_composition(
    marks: &Marks,
    scale: f64,
    layout: &ParagraphLayout,
    origin: Point,
    composition: Range<usize>,
    clause: Option<Range<usize>>,
    color: Option<Color>,
) {
    let underline = |range: Range<usize>, points: f64| {
        for rect in layout.selection_rects(range) {
            let Some(line) = layout.lines().iter().find(|l| l.rect.y() == rect.y()) else {
                continue;
            };
            marks.underline(
                Rect::new(
                    origin.x + rect.min_x(),
                    origin.y + line.baseline + COMPOSITION_DROP,
                    rect.width(),
                    device_thickness(points, scale),
                ),
                color,
            );
        }
    };
    underline(composition, COMPOSITION_LINE);
    if let Some(clause) = clause.filter(|c| !c.is_empty()) {
        underline(clause, CLAUSE_LINE);
    }
}

/// Where a buffer's caret draws: the input method's cursor while it composes (`None` when it hides it or converts a
/// clause), else the buffer's caret.
pub(crate) fn drawn_caret(buffer: &TextBuffer) -> Option<TextPosition> {
    match buffer.composition() {
        Some(_) => buffer
            .composition_cursor()
            .filter(Range::is_empty)
            .map(|c| position(c.start)),
        None => Some(buffer.caret_position()),
    }
}

/// The position the input method's candidate window anchors at and a field scrolls to show: the input method's
/// cursor (its clause's start) while it composes, else the caret.
pub(crate) fn input_anchor(buffer: &TextBuffer) -> TextPosition {
    match (buffer.composition(), buffer.composition_cursor()) {
        (Some(_), Some(cursor)) => position(cursor.start),
        (Some(range), None) => position(range.start),
        (None, _) => buffer.caret_position(),
    }
}

pub(crate) fn session(purpose: ContentPurpose, protected: bool) -> TextInputSession {
    let caret = TextPosition {
        byte: 0,
        affinity: Affinity::Downstream,
    };
    TextInputSession {
        revision: 0,
        purpose,
        hints: ContentHints::default(),
        selection: Selection {
            anchor: caret,
            focus: caret,
        },
        preedit: None,
        protected,
    }
}

pub(crate) fn position(byte: usize) -> TextPosition {
    TextPosition {
        byte,
        affinity: Affinity::Downstream,
    }
}

/// A search field's magnifying glass: the size of the symbol's box (its strokes span about 12.5 points of it, from
/// about 10.5 points in) and the box's leading edge, and the text's leading edge. Its height is the size table's for
/// a search field ([`ControlFamily::SearchField`]): 24 points at the regular size, 28 at the large size a sidebar's
/// search uses.
const SEARCH_GLYPH: f64 = 16.0;
const SEARCH_GLYPH_X: f64 = 8.25;
const SEARCH_TEXT_X: f64 = 29.5;
/// A search field's capsule on a material: its fill and rim, white at these opacities in a dark appearance, the fill
/// white and the rim black in a light one. On the window's content it is a text field's recessed well instead.
const SEARCH_FILL_DARK: f32 = 0.23;
const SEARCH_RIM_DARK: f32 = 0.12;
const SEARCH_FILL_LIGHT: f32 = 0.45;
const SEARCH_RIM_LIGHT: f32 = 0.09;

/// A text field's well in a dark appearance, black at this opacity over what it sits on, and its edge: white at this
/// opacity in a dark appearance, black at this one in a light one.
const WELL_DARK: f32 = 0.25;
const WELL_EDGE_DARK: f32 = 0.05;
const WELL_EDGE_LIGHT: f32 = 0.08;

/// How a text field draws its chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FieldLook {
    #[default]
    Plain,
    /// A capsule with a magnifying glass before the text: a [`SearchField`], at its own control size or the window's.
    Search(Option<ControlSize>),
    /// No chrome, the text at this many points and in this weight (the body type's when `None`): a [`BareField`] or a
    /// [`BareSecureField`].
    Bare(u16, Option<u16>),
}

/// A bare field's height as a share of its text size: one line with room for its ascenders and descenders.
const BARE_LINE_SHARE: f64 = 1.3;

impl FieldLook {
    fn height(self, theme: &ThemeTokens) -> f64 {
        match self {
            FieldLook::Plain => theme.metrics.field_height,
            FieldLook::Search(size) => {
                let metrics = &theme.metrics;
                control_geometry(
                    ControlFamily::SearchField,
                    size.unwrap_or(metrics.size),
                    metrics.compact,
                )
                .height
            }
            FieldLook::Bare(size, _) => (f64::from(size) * BARE_LINE_SHARE).ceil(),
        }
    }

    /// The size of the field's text: the body type's, or a bare field's own.
    fn text_size(self, theme: &ThemeTokens) -> f32 {
        match self {
            FieldLook::Plain | FieldLook::Search(_) => theme.type_scale.body.size,
            FieldLook::Bare(size, _) => f32::from(size),
        }
    }

    /// The weight of the field's text: the body type's, or a bare field's own.
    fn text_weight(self, theme: &ThemeTokens) -> u16 {
        match self {
            FieldLook::Bare(_, Some(weight)) => weight,
            _ => theme.type_scale.body.weight,
        }
    }
}

/// `text` on one line in the field's type.
fn field_paragraph(env: &Environment, text: &str, field: FieldLook) -> Option<ParagraphLayout> {
    look::paragraph_weighted(
        env,
        text,
        Some(look::direction(env)),
        field.text_size(&env.theme),
        field.text_weight(&env.theme),
    )
}

/// The placeholder on one line in the field's type; none when it is empty.
fn field_label(env: &Environment, text: &str, field: FieldLook) -> Option<ParagraphLayout> {
    if text.is_empty() {
        return None;
    }
    look::paragraph_weighted(
        env,
        text,
        None,
        field.text_size(&env.theme),
        field.text_weight(&env.theme),
    )
}

/// The y of `layout`'s top that puts its first baseline where the field's text sits in `chrome`.
fn field_top(layout: &ParagraphLayout, chrome: Rect, env: &Environment, field: FieldLook) -> f64 {
    let first = layout.lines().first().map_or(0.0, |line| line.baseline);
    look::baseline_sized(chrome, field.text_size(&env.theme)) - first
}

/// The chrome is the field's height, centred in a node padded out to the minimum target.
fn chrome(size: Size, theme: &ThemeTokens, look: FieldLook) -> Rect {
    let height = look.height(theme);
    Rect::new(0.0, (size.height - height) / 2.0, size.width, height)
}

/// Where text, selection, composition and caret show; they are clipped to it. A search field's text starts after its
/// magnifying glass, at the reading direction's start.
fn interior(size: Size, theme: &ThemeTokens, look: FieldLook, direction: LayoutDirection) -> Rect {
    let inset = theme.metrics.field_inset;
    let (start, end) = match look {
        FieldLook::Plain => (inset, inset),
        FieldLook::Search(_) => (SEARCH_TEXT_X, inset),
        FieldLook::Bare(..) => (0.0, 0.0),
    };
    let (left, right) = match direction {
        LayoutDirection::Ltr => (start, end),
        LayoutDirection::Rtl => (end, start),
    };
    chrome(size, theme, look).inset(Insets {
        top: 0.0,
        left,
        bottom: 0.0,
        right,
    })
}

/// How far the text scrolls left so a caret `caret_width` wide stays inside `width`: unchanged while it is visible,
/// else just enough to show all of it, and never further than leaves room for it after the text's end.
fn follow(scroll: f64, caret_x: f64, caret_width: f64, text_width: f64, width: f64) -> f64 {
    let scroll = if caret_x < scroll {
        caret_x
    } else if caret_x + caret_width > scroll + width {
        caret_x + caret_width - width
    } else {
        scroll
    };
    scroll
        .min((text_width + caret_width - width).max(0.0))
        .max(0.0)
}

/// The left edge of text `width` wide before it scrolls: the interior's start, or in right-to-left as far right as
/// leaves the caret room after it.
fn leading_x(env: &Environment, area: Rect, width: f64) -> f64 {
    match env.direction {
        LayoutDirection::Ltr => area.min_x(),
        LayoutDirection::Rtl => area.min_x() + (area.width() - CARET_WIDTH - width).max(0.0),
    }
}

/// The text laid out where the field paints it: the layout and its top-left corner in node coordinates.
fn placed(
    env: &Environment,
    text: &str,
    size: Size,
    scroll: f64,
    field: FieldLook,
) -> Option<(ParagraphLayout, Point)> {
    let layout = field_paragraph(env, text, field)?;
    let chrome = chrome(size, &env.theme, field);
    let area = interior(size, &env.theme, field, env.direction);
    let origin = Point::new(
        leading_x(env, area, layout.size().width) - scroll,
        field_top(&layout, chrome, env, field),
    );
    Some((layout, origin))
}

pub(crate) fn fill_rect(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    color: Option<cupertino_ui_theme::Color>,
) {
    if let Some(color) = color {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(rect),
            paint: Paint::Solid(color),
        });
    }
}

/// What a field paints of its buffer, in byte offsets of `text`.
struct Shown<'a> {
    text: &'a str,
    /// Where the field scrolls to keep in view: the caret, or the input method's cursor.
    caret: TextPosition,
    /// The caret draws (an input method can hide it, or convert a clause in its place).
    caret_drawn: bool,
    selection: Range<usize>,
    composition: Option<Range<usize>>,
    /// The clause of the composition the input method is converting, drawn with a heavier underline.
    clause: Option<Range<usize>>,
    /// The placeholder's colour in place of the look's ([`BareSecureField::placeholder_color`]).
    placeholder_color: Option<Color>,
}

/// Paints a field's text, or its placeholder while empty, with the selection while focused, all clipped to the
/// interior, and places the caret and the composition's underline as `marks` for the overlay. Scrolls so the caret
/// stays inside while focused, and to the text's start otherwise.
#[allow(clippy::too_many_arguments)]
fn paint_text(
    cx: &mut PaintCx<'_>,
    look: &ControlState,
    enabled: bool,
    shown: &Shown<'_>,
    placeholder: &str,
    scroll: &Cell<f64>,
    marks: &Marks,
    field: FieldLook,
) {
    marks.clear();
    let env = cx.env().clone();
    let size = cx.size();
    let area = interior(size, &env.theme, field, env.direction);
    let chrome = chrome(size, &env.theme, field);
    let Some(layout) = field_paragraph(&env, shown.text, field) else {
        return;
    };
    let caret = layout.caret_rect(shown.caret);
    let focused = look.focused && enabled;
    let (shown_x, shown_width) = if focused {
        (caret.min_x(), device_thickness(CARET_WIDTH, env.scale))
    } else {
        (layout.caret_rect(position(0)).min_x(), 0.0)
    };
    let offset = follow(
        scroll.get(),
        shown_x,
        shown_width,
        layout.size().width,
        area.width(),
    );
    scroll.set(offset);
    let origin = Point::new(
        leading_x(&env, area, layout.size().width) - offset,
        field_top(&layout, chrome, &env, field),
    );
    let color = |role| look::color(&env.theme, role, look);
    cx.clip_pixels(area);
    if let Some(selected) =
        color(ColorRole::SelectedTextBackground).filter(|_| focused && !shown.selection.is_empty())
    {
        for rect in layout.selection_rects(shown.selection.clone()) {
            cx.fill_pixels(rect.translate(origin.x, origin.y), selected);
        }
    }
    if shown.text.is_empty() {
        if let Some(placeholder) = field_label(&env, placeholder, field) {
            let at = Point::new(
                leading_x(&env, area, placeholder.size().width),
                field_top(&placeholder, chrome, &env, field),
            );
            // a search field's placeholder, and a bare field's, read as its label, so they are stronger than a plain
            // field's
            let role = match field {
                FieldLook::Plain => ColorRole::PlaceholderText,
                FieldLook::Search(_) | FieldLook::Bare(..) => ColorRole::SecondaryText,
            };
            look::glyphs(
                cx,
                &placeholder,
                at,
                shown.placeholder_color.or(color(role)),
            );
        }
    } else {
        look::glyphs(cx, &layout, origin, color(ColorRole::Text));
    }
    cx.push(PaintOp::PopClip);
    marks.clip(area);
    if let Some(range) = &shown.composition {
        mark_composition(
            marks,
            env.scale,
            &layout,
            origin,
            range.clone(),
            shown.clause.clone(),
            color(ColorRole::Text),
        );
    }
    if focused && look.window_active && shown.selection.is_empty() && shown.caret_drawn {
        // a bare field's caret is drawn in its text's colour, as it stands alone in a larger surface; a field with
        // chrome draws it in the accent colour
        let ink = match field {
            FieldLook::Bare(..) => ColorRole::Text,
            FieldLook::Plain | FieldLook::Search(_) => ColorRole::DefaultButtonFill,
        };
        marks.caret(
            caret_bar(caret.translate(origin.x, origin.y), env.scale),
            color(ink),
        );
    }
    marks.finish(cx);
}

/// After this long without an edit, move, click or focus, the caret stops blinking and stays on, so an idle window
/// settles.
const BLINK_LIMIT_NS: u64 = 10_000_000_000;

/// How the caret's blink cycle divides, as shares of its period: the caret fades out, stays hidden, fades in and
/// stays shown. The period is two blink phases ([`Motion::caret_phase_ms`](cupertino_ui_theme::Motion)), less a
/// fiftieth, so the default phase gives a period just under a second.
const BLINK_PERIOD_SHARE: f64 = 1.96;
const BLINK_FADE_OUT: f64 = 0.12 / 0.98;
const BLINK_FADE_IN: f64 = 0.14 / 0.98;
const BLINK_SHOWN: f64 = 0.50 / 0.98;
/// How often the caret asks to be drawn again while it fades, nanoseconds.
const BLINK_FRAME_NS: f64 = 16_000_000.0;

/// The caret's blink: shown for the first stretch after each edit, move, click or focus gain, then fading out and in
/// in a cycle just under a second long, steady with Reduce Motion and after [`BLINK_LIMIT_NS`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Blink {
    since: Timestamp,
}

/// The opacity of a caret whose blink restarted at `since`, at `now`: shown for the first stretch, then fading out and
/// in, steady with Reduce Motion and once it has blinked a while. For a caret a view draws itself.
pub fn caret_opacity(env: &Environment, since: Timestamp, now: Timestamp) -> f32 {
    Blink { since }.opacity(env, now)
}

/// Whether a caret whose blink restarted at `since` may still change its opacity after `now`, so its view keeps
/// drawing frames.
pub fn caret_blinking(env: &Environment, since: Timestamp, now: Timestamp) -> bool {
    Blink { since }.next_edge(env, now).is_some()
}

/// Where `elapsed` nanoseconds after a restart lies in the blink cycle of a phase of `phase` nanoseconds.
enum Cycle {
    /// Fully shown for this many more nanoseconds.
    Shown(f64),
    /// Fully hidden for this many more nanoseconds.
    Hidden(f64),
    /// Fading, at this opacity.
    Fading(f32),
}

impl Blink {
    fn phase(env: &Environment) -> Option<u64> {
        let motion = env.theme.motion;
        let phase = u64::from(motion.caret_phase_ms) * 1_000_000;
        (!motion.reduce && phase > 0).then_some(phase)
    }

    fn cycle(phase: u64, elapsed: u64) -> Cycle {
        let period = phase as f64 * BLINK_PERIOD_SHARE;
        let first = BLINK_SHOWN * period;
        let t = elapsed as f64;
        if t < first {
            return Cycle::Shown(first - t);
        }
        let into = (t - first) % period;
        let out = BLINK_FADE_OUT * period;
        let fade_in = BLINK_FADE_IN * period;
        let hidden = period - BLINK_SHOWN * period - out - fade_in;
        if into < out {
            Cycle::Fading((1.0 - into / out) as f32)
        } else if into < out + hidden {
            Cycle::Hidden(out + hidden - into)
        } else if into < out + hidden + fade_in {
            Cycle::Fading(((into - out - hidden) / fade_in) as f32)
        } else {
            Cycle::Shown(period - into)
        }
    }

    /// The caret's opacity at `now`, from 0 to 1.
    pub(crate) fn opacity(&self, env: &Environment, now: Timestamp) -> f32 {
        let elapsed = now.0.saturating_sub(self.since.0);
        match Self::phase(env) {
            Some(phase) if elapsed < BLINK_LIMIT_NS => match Self::cycle(phase, elapsed) {
                Cycle::Shown(_) => 1.0,
                Cycle::Hidden(_) => 0.0,
                Cycle::Fading(opacity) => opacity,
            },
            _ => 1.0,
        }
    }

    /// The next time the caret's opacity needs drawing, or `None` once it is steady: a frame later while it fades,
    /// else when the next fade starts.
    fn next_edge(&self, env: &Environment, now: Timestamp) -> Option<Timestamp> {
        let phase = Self::phase(env)?;
        let elapsed = now.0.saturating_sub(self.since.0);
        if elapsed >= BLINK_LIMIT_NS {
            return None;
        }
        let wait = match Self::cycle(phase, elapsed) {
            Cycle::Fading(_) => BLINK_FRAME_NS,
            Cycle::Shown(left) | Cycle::Hidden(left) => left.max(1.0),
        };
        let at = (elapsed + wait as u64).min(BLINK_LIMIT_NS);
        Some(Timestamp(self.since.0 + at))
    }

    /// Asks to be woken at the next edge while the caret shows in a focused field of an active window.
    fn arm<M>(&self, selecting: bool, cx: &mut EventCx<'_, M>) {
        if selecting || !cx.is_focused() || !cx.window_active() {
            return;
        }
        if let Some(at) = self.next_edge(cx.env(), cx.now()) {
            cx.wake_at(at);
        }
    }

    /// Whether `event` restarts the blink with the caret showing.
    fn restarts(event: &Event) -> bool {
        matches!(
            event,
            Event::Key(..)
                | Event::Ime(ImeEvent::Done(_))
                | Event::Focus { gained: true, .. }
                | Event::WindowActive(true)
                | Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
        )
    }

    /// Restarts on the events that move the caret, and keeps blinking on a wake, after the field handled `event`.
    pub(crate) fn follow<M>(&mut self, event: &Event, selecting: bool, cx: &mut EventCx<'_, M>) {
        if Self::restarts(event) {
            self.since = cx.now();
            self.arm(selecting, cx);
            cx.request_paint();
        } else if let Event::Wake(_) = event {
            // the blink changes only the overlay
            self.arm(selecting, cx);
            cx.request_overlay();
        }
    }
}

fn layout() -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

fn measure(constraints: Constraints, cx: &MeasureCx<'_>, field: FieldLook) -> Measured {
    Measured {
        size: Size::new(constraints.width.ideal(), field.height(&cx.env().theme)),
        baseline: None,
    }
}

fn paint_chrome(cx: &mut PaintCx<'_>, state: &ControlState, field: FieldLook) {
    let theme = cx.env().theme.clone();
    let rect = chrome(cx.size(), &theme, field);
    match field {
        FieldLook::Search(_) => {
            paint_search_chrome(cx, rect, state);
            return;
        }
        // the surface it sits on is its chrome
        FieldLook::Bare(..) => return,
        FieldLook::Plain => {}
    }
    let radius = theme.metrics.field_radius;
    let (fill, edge) = well(&theme, state);
    look::fill(cx, rect, radius, fill);
    look::stroke(
        cx,
        rect.inset(cupertino_geometry::Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        edge,
    );
    paint_focus_ring(cx, rect, radius);
}

/// A field's recessed well: darker than what it sits on in a dark appearance, the text background in a light one,
/// with a faint edge; the theme's stronger border under increased contrast. Disabled, the dark well is fainter.
pub(crate) fn well(theme: &ThemeTokens, state: &ControlState) -> (Option<Color>, Option<Color>) {
    let appearance = theme.preferences.appearance;
    let fill = if appearance.is_dark() {
        let share = if state.enabled {
            WELL_DARK
        } else {
            WELL_DARK / 2.0
        };
        Some(Color::new(0.0, 0.0, 0.0, share))
    } else {
        look::color(theme, ColorRole::TextBackground, state)
    };
    let edge = if appearance.is_high_contrast() {
        look::color(theme, ColorRole::TextFieldBorder, state)
    } else if appearance.is_dark() {
        Some(Color::new(1.0, 1.0, 1.0, WELL_EDGE_DARK))
    } else {
        Some(Color::new(0.0, 0.0, 0.0, WELL_EDGE_LIGHT))
    };
    (fill, edge)
}

/// A search field's capsule and its magnifying glass at the reading direction's start. On a material (a sidebar) the
/// capsule is a light fill with a rim; on the window's content it is a text field's recessed well with round ends.
fn paint_search_chrome(cx: &mut PaintCx<'_>, rect: Rect, state: &ControlState) {
    let theme = cx.env().theme.clone();
    let radius = rect.height() / 2.0;
    let (fill, rim) = if cx.on_material() {
        search_capsule(&theme, state)
    } else {
        well(&theme, state)
    };
    look::fill(cx, rect, radius, fill);
    look::stroke(
        cx,
        rect.inset(cupertino_geometry::Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        rim,
    );
    let glyph = match cx.env().direction {
        LayoutDirection::Ltr => SEARCH_GLYPH_X,
        LayoutDirection::Rtl => rect.width() - SEARCH_GLYPH_X - SEARCH_GLYPH,
    };
    let area = Rect::new(
        rect.min_x() + glyph,
        rect.min_y(),
        SEARCH_GLYPH,
        rect.height(),
    );
    if let Some(color) = look::color(&theme, ColorRole::SecondaryText, state) {
        crate::icon::symbol(
            cx,
            "symbol.magnifyingglass",
            SEARCH_GLYPH,
            color,
            false,
            area,
        );
    }
    // a focused search field is ringed all round in the accent, softly, whether or not the keyboard moved the focus
    if state.focused && state.enabled && state.window_active {
        let accent = look::color(
            &theme,
            ColorRole::ControlFill,
            &ControlState {
                selected: true,
                ..*state
            },
        );
        look::stroke(
            cx,
            rect.outset(SEARCH_RING / 2.0),
            radius + SEARCH_RING / 2.0,
            SEARCH_RING,
            accent.map(|c| c.with_alpha(SEARCH_RING_ALPHA)),
        );
    }
}

/// A focused search field's ring: its width, outside the capsule, and its opacity.
const SEARCH_RING: f64 = 3.5;
const SEARCH_RING_ALPHA: f32 = 0.6;

/// A search field's capsule on a material: a light fill and its rim. Under increased contrast the rim is the theme's
/// stronger field border.
fn search_capsule(theme: &ThemeTokens, state: &ControlState) -> (Option<Color>, Option<Color>) {
    let appearance = theme.preferences.appearance;
    let white = Color::new(1.0, 1.0, 1.0, 1.0);
    let black = Color::new(0.0, 0.0, 0.0, 1.0);
    let (fill, rim) = if appearance.is_dark() {
        (
            white.with_alpha(SEARCH_FILL_DARK),
            white.with_alpha(SEARCH_RIM_DARK),
        )
    } else {
        (
            white.with_alpha(SEARCH_FILL_LIGHT),
            black.with_alpha(SEARCH_RIM_LIGHT),
        )
    };
    let rim = if appearance.is_high_contrast() {
        look::color(theme, ColorRole::TextFieldBorder, state)
    } else {
        Some(rim)
    };
    (Some(fill), rim)
}

pub(crate) fn control_state(enabled: bool, hovered: bool, cx: &PaintCx<'_>) -> ControlState {
    ControlState {
        enabled,
        hovered,
        focused: cx.is_focused(),
        focus_visible: cx.focus_visible(),
        window_active: cx.window_active(),
        ..ControlState::default()
    }
}

/// Hover, a press that focuses the field, and the keys a field never takes (Tab moves the focus).
pub(crate) fn pointer<M>(
    event: &Event,
    enabled: bool,
    hovered: &mut bool,
    cx: &mut EventCx<'_, M>,
) -> Option<Propagation> {
    match event {
        Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
            *hovered = true;
            cx.request_paint();
            Some(Propagation::Continue)
        }
        Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
            *hovered = false;
            cx.request_paint();
            Some(Propagation::Continue)
        }
        Event::Pointer(
            PointerEvent::Button {
                button: MouseButton::Left,
                pressed: true,
                ..
            },
            _,
        ) if enabled => {
            cx.request_focus();
            Some(Propagation::Stop)
        }
        Event::Action(request) if request.action == Action::Focus && enabled => {
            cx.request_focus();
            Some(Propagation::Stop)
        }
        Event::Key(key, _) if key.keysym == TAB => Some(Propagation::Continue),
        _ => None,
    }
}

/// The node state of a [`TextField`]. Opaque.
pub struct FieldState {
    buffer: TextBuffer,
    keymap: Keymap,
    session: TextInputSession,
    /// The revision of the last change the field reported.
    reported: u64,
    hovered: bool,
    /// Set by paint, where the field's width is known, and read by hit testing.
    scroll: Cell<f64>,
    blink: Blink,
    /// The caret and underlines the last paint placed, for the overlay.
    marks: Marks,
    look: FieldLook,
    /// Where the pointer is while a press drags, in window coordinates, so the selection scrolls on past the edge.
    drag_point: Option<Point>,
    /// The next change tells the input method to start afresh.
    reset_input: bool,
}

/// The text without what an input method is still composing: what the model holds.
pub(crate) fn committed(buffer: &TextBuffer) -> String {
    buffer.committed()
}

/// Whether a model's `value` is the text the buffer holds, once the buffer's own clean-up is applied to it, so a value
/// with a line break the buffer converted is not taken again on every rebuild.
pub(crate) fn holds(buffer: &TextBuffer, value: &str) -> bool {
    let held = committed(buffer);
    value == held || buffer.cleaned(value) == held
}

fn anchor(buffer: &TextBuffer) -> usize {
    let selection = buffer.selection();
    if buffer.caret() == selection.start {
        selection.end
    } else {
        selection.start
    }
}

/// Tells the input method where an editable text stands now: the committed text around the caret (never what it is
/// still composing), and `caret`, the rectangle its candidate window anchors at, in window coordinates. With `reset`
/// the input method starts afresh, as after the field kept or dropped a composition it still holds.
pub(crate) fn sync_input<M>(
    buffer: &TextBuffer,
    session: &mut TextInputSession,
    caret: Option<Rect>,
    cause: ChangeCause,
    reset: bool,
    cx: &mut EventCx<'_, M>,
) {
    session.select(Selection {
        anchor: position(anchor(buffer)),
        focus: position(buffer.caret()),
    });
    let surrounding = buffer.surrounding(SURROUNDING_MAX_BYTES);
    let field = cx.bounds();
    let mut state = session.state(Some(surrounding), cause, caret, field);
    state.reset = reset;
    cx.set_text_input(Some(state));
}

/// Applies one input-method batch ([`TextBuffer::apply_ime`]).
pub(crate) fn apply_ime(buffer: &mut TextBuffer, batch: &ImeBatch) {
    buffer.apply_ime(batch);
}

impl FieldState {
    fn committed(&self) -> String {
        committed(&self.buffer)
    }

    /// The caret (the input method's cursor while it composes) in window coordinates, for the input method's
    /// candidate window, at the scroll the next paint takes.
    fn caret_rect<M>(&self, cx: &EventCx<'_, M>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        let (layout, origin) = placed(cx.env(), self.buffer.text(), bounds.size, 0.0, self.look)?;
        let caret = layout.caret_rect(input_anchor(&self.buffer));
        let width = interior(bounds.size, &cx.env().theme, self.look, cx.env().direction).width();
        let scroll = follow(
            self.scroll.get(),
            caret.min_x(),
            device_thickness(CARET_WIDTH, cx.env().scale),
            layout.size().width,
            width,
        );
        Some(caret.translate(
            bounds.min_x() + origin.x - scroll,
            bounds.min_y() + origin.y,
        ))
    }

    /// The text position under `point`, in window coordinates.
    fn index_at<M>(&self, point: Point, cx: &EventCx<'_, M>) -> Option<TextPosition> {
        let bounds = cx.bounds()?;
        let (layout, origin) = placed(
            cx.env(),
            self.buffer.text(),
            bounds.size,
            self.scroll.get(),
            self.look,
        )?;
        Some(layout.index_at(point - bounds.origin - origin))
    }

    /// A press, a drag with the button held and its release: the caret, then a word or the whole text on a second
    /// and third press, extended to where the pointer goes; with Shift the press extends the selection. The press
    /// captures the pointer, so a drag past the field still extends the selection, and one held past its start or end
    /// scrolls on by itself. A press while the input method composes keeps the composed text as typed (reported as
    /// an edit) and has the input method start afresh.
    fn select<M>(
        &mut self,
        event: &Event,
        on_commit: &mut dyn FnMut(&mut Self, &mut EventCx<'_, M>),
        cx: &mut EventCx<'_, M>,
    ) -> Option<Propagation> {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) if cx.node() == cx.target() => {
                if self.buffer.composition().is_some() {
                    self.buffer.commit_composition();
                    self.reset_input = true;
                    on_commit(self, cx);
                }
                let at = self.index_at(position, cx)?;
                let now = Duration::from_nanos(cx.now().0);
                self.buffer.press_at(at, mods.contains(Mods::SHIFT), now);
                self.drag_point = Some(position);
                cx.capture_pointer();
                None
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if self.buffer.is_dragging() => {
                self.drag_point = Some(position);
                let at = self.index_at(position, cx)?;
                self.buffer.drag_to(at);
                Some(Propagation::Stop)
            }
            Event::Wake(_) if self.buffer.is_dragging() => {
                if let Some(point) = self.drag_point.filter(|p| self.past_edge(*p, cx)) {
                    let at = self.index_at(point, cx)?;
                    self.buffer.drag_to(at);
                }
                None
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if self.buffer.is_dragging() => {
                self.buffer.release();
                self.drag_point = None;
                cx.release_pointer();
                Some(Propagation::Stop)
            }
            // the capture was lost: the drag ends where it is
            Event::Cancel if self.buffer.is_dragging() => {
                self.buffer.release();
                self.drag_point = None;
                Some(Propagation::Continue)
            }
            _ => None,
        }
    }

    /// Whether `point` (window coordinates) lies before the start or past the end of the text area, where a held drag
    /// scrolls.
    fn past_edge<M>(&self, point: Point, cx: &EventCx<'_, M>) -> bool {
        let Some(bounds) = cx.bounds() else {
            return false;
        };
        let area = interior(bounds.size, &cx.env().theme, self.look, cx.env().direction);
        let x = point.x - bounds.min_x();
        x < area.min_x() || x > area.max_x()
    }

    /// Asks for the next autoscroll step while a drag is held past the edge; after the blink, whose wake it replaces
    /// with an earlier one.
    fn arm_autoscroll<M>(&self, cx: &mut EventCx<'_, M>) {
        if self.buffer.is_dragging() && self.drag_point.is_some_and(|p| self.past_edge(p, cx)) {
            cx.wake_at(Timestamp(cx.now().0 + AUTOSCROLL_NS));
        }
    }

    /// Tells the input method where the field stands now.
    fn sync_input<M>(&mut self, cause: ChangeCause, cx: &mut EventCx<'_, M>) {
        let caret = self.caret_rect(cx);
        let reset = std::mem::take(&mut self.reset_input);
        sync_input(&self.buffer, &mut self.session, caret, cause, reset, cx);
    }
}

impl<M: Clone + 'static> TextField<M> {
    fn fill(&self, buffer: &mut TextBuffer) {
        buffer.set_text(&self.value);
        if let Some(range) = &self.selection {
            buffer.select(range.clone());
        }
    }

    fn handle(
        &self,
        state: &mut FieldState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if self.enabled {
            let selection = state.buffer.selection();
            let ended = state.select(event, &mut |state, cx| self.report(state, cx), cx);
            if state.buffer.selection() != selection || state.buffer.is_dragging() {
                if cx.is_focused() {
                    state.sync_input(ChangeCause::Other, cx);
                }
                cx.request_paint();
            }
            if let Some(propagation) = ended {
                return propagation;
            }
        }
        if let Some(propagation) = pointer(event, self.enabled, &mut state.hovered, cx) {
            return propagation;
        }
        match event {
            Event::Focus { gained: true, .. } if self.enabled => {
                state.sync_input(ChangeCause::Other, cx);
                cx.request_paint();
                Propagation::Continue
            }
            // what the input method composed stays as typed; the window system turns the input method off
            Event::Focus { gained: false, .. } => {
                let composing = state.buffer.composition().is_some();
                state.buffer.blur();
                state.drag_point = None;
                if composing {
                    self.report(state, cx);
                }
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && cx.node() == cx.target() => {
                self.key(state, key, cx)
            }
            Event::Ime(ImeEvent::Done(batch)) if self.enabled => {
                let before = state.committed();
                apply_ime(&mut state.buffer, batch);
                if state.committed() != before {
                    self.report(state, cx);
                }
                // a stale batch is answered by the input method's next one, not by this state
                if state.session.accepts(batch) {
                    state.sync_input(ChangeCause::InputMethod, cx);
                }
                cx.request_paint();
                Propagation::Stop
            }
            // the window lost the keyboard: the input method dropped its composition, which stays as typed
            Event::Ime(ImeEvent::Leave) => {
                if state.buffer.composition().is_some() {
                    state.buffer.commit_composition();
                    self.report(state, cx);
                }
                cx.request_paint();
                Propagation::Continue
            }
            Event::Action(request) if self.enabled => {
                match (request.action, &request.data) {
                    (Action::SetValue, Some(ActionData::Text(text))) => {
                        state.buffer.set_text(text);
                        self.report(state, cx);
                    }
                    (Action::SetSelection, Some(ActionData::Selection(range))) => {
                        state.buffer.select(range.clone());
                    }
                    _ => return Propagation::Continue,
                }
                cx.request_paint();
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn report(&self, state: &mut FieldState, cx: &mut EventCx<'_, M>) {
        state.reported += 1;
        if let Some(on_change) = &self.on_change {
            cx.emit(on_change(TextChange {
                text: state.committed(),
                revision: state.reported,
            }));
        }
    }

    fn key(&self, state: &mut FieldState, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        if !key.pressed {
            return Propagation::Continue;
        }
        let composing = state.buffer.composition().is_some();
        match key.keysym {
            RETURN | KP_ENTER if !composing => {
                if let Some(message) = &self.on_submit {
                    cx.emit(message.clone());
                }
                return Propagation::Stop;
            }
            ESCAPE if composing => {
                state.buffer.cancel_composition();
                state.reset_input = true;
                state.sync_input(ChangeCause::Other, cx);
                cx.request_paint();
                return Propagation::Stop;
            }
            ESCAPE => return Propagation::Continue,
            _ => {}
        }
        let response = state
            .buffer
            .logical_key(&mut state.keymap, key.code, key.mods, key.text);
        match response {
            Response::Ignored => return Propagation::Continue,
            Response::Moved => {}
            Response::Edited => self.report(state, cx),
            Response::Copy(text) => cx.set_clipboard(text),
            Response::Cut(text) => {
                cx.set_clipboard(text);
                self.report(state, cx);
            }
            Response::Paste => {}
        }
        state.sync_input(ChangeCause::Other, cx);
        cx.request_paint();
        Propagation::Stop
    }
}

impl<M: Clone + 'static> Widget<M> for TextField<M> {
    type State = FieldState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> FieldState {
        let mut buffer = TextBuffer::new();
        self.fill(&mut buffer);
        FieldState {
            buffer,
            keymap: Keymap::standard(),
            session: session(self.purpose, false),
            reported: self.revision,
            hovered: false,
            scroll: Cell::new(0.0),
            blink: Blink::default(),
            marks: Marks::default(),
            look: FieldLook::Plain,
            drag_point: None,
            reset_input: false,
        }
    }

    /// Takes the model's text when the application changed it itself: a revision at or past the last one the field
    /// reported, with a different text. An older revision is the model catching up and is ignored.
    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        state.session.purpose = self.purpose;
        if self.revision >= state.reported && !holds(&state.buffer, &self.value) {
            self.fill(&mut state.buffer);
            state.reported = self.revision;
            return Change::Paint;
        }
        if self.enabled != old.enabled
            || self.label != old.label
            || self.placeholder != old.placeholder
            || self.purpose != old.purpose
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(
        &self,
        _: &FieldState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, FieldLook::Plain)
    }

    fn target(&self, _: &FieldState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(&self, state: &mut FieldState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        state.buffer.set_direction(Some(look::direction(cx.env())));
        let propagation = self.handle(state, event, cx);
        let selecting = !state.buffer.selection().is_empty();
        state.blink.follow(event, selecting, cx);
        state.arm_autoscroll(cx);
        propagation
    }

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        let look = control_state(self.enabled, state.hovered, cx);
        paint_chrome(cx, &look, state.look);
        let drawn = drawn_caret(&state.buffer);
        let shown = Shown {
            text: state.buffer.text(),
            caret: drawn.unwrap_or_else(|| input_anchor(&state.buffer)),
            caret_drawn: drawn.is_some(),
            selection: state.buffer.selection(),
            composition: state.buffer.composition(),
            clause: state.buffer.composition_cursor(),
            placeholder_color: None,
        };
        paint_text(
            cx,
            &look,
            self.enabled,
            &shown,
            &self.placeholder,
            &state.scroll,
            &state.marks,
            state.look,
        );
    }

    fn overlay(&self, state: &FieldState, cx: &mut OverlayCx<'_>) {
        state.marks.draw(&state.blink, cx);
    }

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TextInput);
        cx.name(self.label.clone());
        if state.buffer.composition().is_some() {
            cx.composing();
        }
        // what an input method is still composing stays out: the tree reaches adapters, the inspector and test dumps
        let (caret, selection) = match state.buffer.composition() {
            Some(range) => (range.start, range.start..range.start),
            None => (state.buffer.caret(), state.buffer.selection()),
        };
        cx.value(AccessibleValue::Text {
            value: state.committed(),
            editing: Some(TextSemantics { caret, selection }),
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
            cx.action(Action::SetValue);
            cx.action(Action::SetSelection);
            if self.initial {
                cx.initial_focus();
            }
        }
    }
}

/// The node state of a [`SecureField`]. Opaque.
pub struct SecureState {
    buffer: SecureBuffer,
    session: TextInputSession,
    hovered: bool,
    scroll: Cell<f64>,
    blink: Blink,
    /// The caret the last paint placed, for the overlay.
    marks: Marks,
    /// The bullet a held press started from, while it drags.
    drag: Option<usize>,
    /// When and at which bullet the last press went down, for a double click.
    last_press: Option<(Timestamp, usize)>,
    /// The field's chrome and type: a [`BareSecureField`]'s has none.
    look: FieldLook,
    /// The placeholder's colour in place of the look's: a [`BareSecureField`]'s own.
    placeholder_color: Option<Color>,
}

impl SecureState {
    /// The bullet boundary under `point` (window coordinates): a grapheme index of the password.
    fn mask_index<M>(&self, point: Point, cx: &EventCx<'_, M>) -> Option<usize> {
        let bounds = cx.bounds()?;
        let mask = self.buffer.mask(BULLET);
        let (layout, origin) = placed(cx.env(), &mask, bounds.size, self.scroll.get(), self.look)?;
        Some(layout.index_at(point - bounds.origin - origin).byte / BULLET.len_utf8())
    }

    /// A press puts the caret between two bullets (with Shift it extends the selection), a drag selects to the
    /// pointer, and a second press at the same place selects the whole password: it has no words to show.
    fn select<M>(&mut self, event: &Event, cx: &mut EventCx<'_, M>) -> bool {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) if cx.node() == cx.target() => {
                let Some(at) = self.mask_index(position, cx) else {
                    return false;
                };
                let now = cx.now();
                let again = self.last_press.is_some_and(|(then, index)| {
                    index == at
                        && now.0.saturating_sub(then.0)
                            <= cupertino_ui_text::MULTI_CLICK_INTERVAL.as_nanos() as u64
                });
                self.last_press = (!again).then_some((now, at));
                let (caret, selection) = self.buffer.masked_selection();
                let anchor = if caret == selection.start {
                    selection.end
                } else {
                    selection.start
                };
                if again {
                    self.buffer.select_all();
                    self.drag = None;
                } else {
                    let from = if mods.contains(Mods::SHIFT) {
                        anchor
                    } else {
                        at
                    };
                    self.buffer.select_masked(from, at);
                    self.drag = Some(from);
                    cx.capture_pointer();
                }
                true
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let (Some(from), Some(at)) = (self.drag, self.mask_index(position, cx)) else {
                    return false;
                };
                self.buffer.select_masked(from, at);
                true
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if self.drag.is_some() => {
                self.drag = None;
                cx.release_pointer();
                true
            }
            Event::Cancel => {
                self.drag = None;
                false
            }
            _ => false,
        }
    }
}

/// Inserts `text` in place of the selection, and whether that changed the buffer: not when [`SecureField::max_len`]
/// left no room and nothing was selected.
fn insert(buffer: &mut SecureBuffer, text: &str) -> bool {
    let before = (buffer.with_secret(str::len), buffer.selection());
    buffer.insert(text);
    (buffer.with_secret(str::len), buffer.selection()) != before
}

impl<M: 'static> SecureField<M> {
    fn handle(
        &self,
        state: &mut SecureState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if self.enabled && state.select(event, cx) {
            cx.request_paint();
            if state.drag.is_some()
                && !matches!(event, Event::Pointer(PointerEvent::Button { .. }, _))
            {
                return Propagation::Stop;
            }
        }
        if let Some(propagation) = pointer(event, self.enabled, &mut state.hovered, cx) {
            return propagation;
        }
        match event {
            // the input method learns the field's place for its window, its purpose and hints, and nothing of its text
            Event::Focus { gained: true, .. } if self.enabled => {
                cx.set_text_input(Some(state.session.state(
                    None,
                    ChangeCause::Other,
                    None,
                    cx.bounds(),
                )));
                cx.request_paint();
                Propagation::Continue
            }
            Event::Focus { gained: false, .. } => {
                state.drag = None;
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && cx.node() == cx.target() => {
                self.key(state, key, cx)
            }
            // a password takes committed text only; a composition is never held anywhere, and with no surrounding
            // text there is nothing to delete
            Event::Ime(ImeEvent::Done(batch)) if self.enabled => {
                if let Some(commit) = &batch.commit {
                    if insert(&mut state.buffer, commit) {
                        self.changed(&state.buffer, cx);
                    }
                    cx.request_paint();
                }
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn changed(&self, buffer: &SecureBuffer, cx: &mut EventCx<'_, M>) {
        if let Some(on_change) = &self.on_change {
            cx.emit(on_change(buffer));
        }
    }

    fn key(&self, state: &mut SecureState, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        use cupertino_ui_platform::Mods;
        if !key.pressed {
            return Propagation::Continue;
        }
        let buffer = &mut state.buffer;
        let extend = key.mods.contains(Mods::SHIFT);
        let command = key.mods.contains(Mods::PRIMARY);
        match key.keysym {
            RETURN | KP_ENTER => {
                if let Some(on_submit) = &self.on_submit {
                    cx.emit(on_submit(buffer));
                }
                return Propagation::Stop;
            }
            ESCAPE => return Propagation::Continue,
            _ => {}
        }
        let edited = match key.keysym.0 {
            0xff08 => {
                buffer.delete_backward();
                true
            }
            0xffff => {
                buffer.delete_forward();
                true
            }
            0xff51 if command => {
                buffer.move_to_start(extend);
                false
            }
            0xff53 if command => {
                buffer.move_to_end(extend);
                false
            }
            0xff51 => {
                buffer.move_char(false, extend);
                false
            }
            0xff53 => {
                buffer.move_char(true, extend);
                false
            }
            0xff50 => {
                buffer.move_to_start(extend);
                false
            }
            0xff57 => {
                buffer.move_to_end(extend);
                false
            }
            _ if command && key.text == Some('a') => {
                buffer.select_all();
                false
            }
            // copy, cut and every other command chord do nothing in a password field, and nothing reaches the window
            _ if command || key.mods.contains(Mods::CTRL) => false,
            _ => match key.text {
                Some(c) if !c.is_control() => insert(buffer, c.encode_utf8(&mut [0; 4])),
                _ => return Propagation::Continue,
            },
        };
        if edited {
            self.changed(buffer, cx);
        }
        cx.request_paint();
        Propagation::Stop
    }
}

impl<M: 'static> Widget<M> for SecureField<M> {
    type State = SecureState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> SecureState {
        let mut buffer = SecureBuffer::new();
        buffer.set_max_len(self.max_len);
        let mut session = session(ContentPurpose::Password, true);
        session.hints = TextInputSession::protected_hints();
        SecureState {
            buffer,
            session,
            hovered: false,
            scroll: Cell::new(0.0),
            blink: Blink::default(),
            marks: Marks::default(),
            drag: None,
            last_press: None,
            look: FieldLook::Plain,
            placeholder_color: None,
        }
    }

    fn update(&self, old: &Self, state: &mut SecureState) -> Change {
        state.buffer.set_max_len(self.max_len);
        if self.enabled != old.enabled
            || self.label != old.label
            || self.placeholder != old.placeholder
            || self.busy != old.busy
            || self.invalid != old.invalid
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(
        &self,
        state: &SecureState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, state.look)
    }

    fn target(&self, _: &SecureState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(
        &self,
        state: &mut SecureState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let propagation = self.handle(state, event, cx);
        let selecting = !state.buffer.selection().is_empty();
        state.blink.follow(event, selecting, cx);
        propagation
    }

    fn paint(&self, state: &SecureState, cx: &mut PaintCx<'_>) {
        let look = ControlState {
            protected: true,
            ..control_state(self.enabled, state.hovered, cx)
        };
        paint_chrome(cx, &look, state.look);
        // the text system shapes only bullets: one per grapheme, with the caret and selection counted in graphemes
        let (caret, selection) = state.buffer.masked_selection();
        let width = BULLET.len_utf8();
        let mask = state.buffer.mask(BULLET);
        let shown = Shown {
            text: &mask,
            caret: position(caret * width),
            caret_drawn: true,
            selection: selection.start * width..selection.end * width,
            composition: None,
            clause: None,
            placeholder_color: state.placeholder_color,
        };
        paint_text(
            cx,
            &look,
            self.enabled,
            &shown,
            &self.placeholder,
            &state.scroll,
            &state.marks,
            state.look,
        );
    }

    fn overlay(&self, state: &SecureState, cx: &mut OverlayCx<'_>) {
        state.marks.draw(&state.blink, cx);
    }

    fn semantics(&self, _: &SecureState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PasswordInput);
        cx.name(self.label.clone());
        cx.protected();
        cx.value(AccessibleValue::Protected);
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            busy: self.busy,
            invalid: self.invalid,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
            if self.initial {
                cx.initial_focus();
            }
        }
    }
}

/// A bare secure field is its secure field without chrome: the same buffer, keys, input method and semantics.
impl<M: 'static> Widget<M> for BareSecureField<M> {
    type State = SecureState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> SecureState {
        SecureState {
            look: bare_secure_look(self.size, self.weight),
            placeholder_color: self.placeholder_color,
            ..self.field.state()
        }
    }

    fn update(&self, old: &Self, state: &mut SecureState) -> Change {
        let change = self.field.update(&old.field, state);
        if self.placeholder_color != old.placeholder_color {
            state.placeholder_color = self.placeholder_color;
            if change == Change::None {
                return Change::Paint;
            }
        }
        if self.size != old.size || self.weight != old.weight {
            state.look = bare_secure_look(self.size, self.weight);
            return Change::Layout;
        }
        change
    }

    fn measure(
        &self,
        state: &SecureState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        self.field.measure(state, constraints, cx)
    }

    fn target(&self, state: &SecureState, env: &Environment) -> Size {
        self.field.target(state, env)
    }

    fn event(
        &self,
        state: &mut SecureState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        self.field.event(state, event, cx)
    }

    fn paint(&self, state: &SecureState, cx: &mut PaintCx<'_>) {
        self.field.paint(state, cx)
    }

    fn overlay(&self, state: &SecureState, cx: &mut OverlayCx<'_>) {
        self.field.overlay(state, cx)
    }

    fn semantics(&self, state: &SecureState, cx: &mut SemanticsCx<'_>) {
        self.field.semantics(state, cx)
    }
}

/// The look of a bare secure field of text `size` points large (whole points from 1 to 200) in `weight`.
fn bare_secure_look(size: f64, weight: Option<u16>) -> FieldLook {
    match bare_look(size) {
        FieldLook::Bare(size, _) => FieldLook::Bare(size, weight),
        look => look,
    }
}

/// A search field is its text field with the search look: the same state, keys, input method and semantics.
impl<M: Clone + 'static> Widget<M> for SearchField<M> {
    type State = FieldState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> FieldState {
        FieldState {
            look: FieldLook::Search(self.size),
            ..self.field.state()
        }
    }

    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        let change = self.field.update(&old.field, state);
        if self.size != old.size {
            state.look = FieldLook::Search(self.size);
            return Change::Layout;
        }
        change
    }

    fn measure(
        &self,
        _: &FieldState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, FieldLook::Search(self.size))
    }

    fn target(&self, state: &FieldState, env: &Environment) -> Size {
        self.field.target(state, env)
    }

    fn event(&self, state: &mut FieldState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.field.event(state, event, cx)
    }

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        self.field.paint(state, cx)
    }

    fn overlay(&self, state: &FieldState, cx: &mut OverlayCx<'_>) {
        self.field.overlay(state, cx)
    }

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        self.field.semantics(state, cx)
    }
}

/// A bare field is its text field without chrome, its text at its own size: the same state, keys, input method and
/// semantics.
impl<M: Clone + 'static> Widget<M> for BareField<M> {
    type State = FieldState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> FieldState {
        FieldState {
            look: bare_look(self.size),
            ..self.field.state()
        }
    }

    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        let change = self.field.update(&old.field, state);
        if self.size != old.size {
            state.look = bare_look(self.size);
            return Change::Layout;
        }
        change
    }

    fn measure(
        &self,
        _: &FieldState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, bare_look(self.size))
    }

    fn target(&self, state: &FieldState, env: &Environment) -> Size {
        self.field.target(state, env)
    }

    fn event(&self, state: &mut FieldState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.field.event(state, event, cx)
    }

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        self.field.paint(state, cx)
    }

    fn overlay(&self, state: &FieldState, cx: &mut OverlayCx<'_>) {
        self.field.overlay(state, cx)
    }

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        self.field.semantics(state, cx)
    }
}

/// The look of a bare field of text `size` points large, in whole points from 1 to 200.
fn bare_look(size: f64) -> FieldLook {
    let size = if size.is_finite() { size } else { 0.0 };
    FieldLook::Bare(size.round().clamp(1.0, 200.0) as u16, None)
}
