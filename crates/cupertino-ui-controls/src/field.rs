//! Text and password fields: the field's own buffer, its keys, the input-method conversation, submit and the model
//! sync. A password lives only in a [`SecureBuffer`]: no undo, no clipboard export, no surrounding text for the input
//! method, and assistive technology learns only that there is a value.

use crate::look::{self, paint_focus_ring};
use crate::text_menu::{self, Can, Command, Opens};
use crate::{BareField, BareSecureField, SearchField, SecureField, TextChange, TextField};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::ui::{
    control_geometry, Color, ColorRole, ControlFamily, ControlSize, ThemeTokens,
};
use cupertino_ui_core::layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_core::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, OverlayCx, PaintCx, Pasted, Propagation, Role, SemanticsCx,
    SurfaceLayerMark, TextSemantics, Widget,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{
    Affinity, ChangeCause, ContentHints, ContentPurpose, ImeBatch, ImeEvent, Keymap,
    ParagraphLayout, Response, SecureBuffer, Selection, TextBuffer, TextFieldKeys,
    TextInputSession, TextPosition, CARET_WIDTH, SURROUNDING_MAX_BYTES,
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
            let opacity = blink.drawn_opacity(cx.env(), cx.now());
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
/// A search field's capsule on a material in a dark appearance: a quiet mid grey laid nearly opaque over the material,
/// so the field keeps its tone however light the material under it is, and a bright rim, white at this opacity.
const SEARCH_FILL_DARK_GREY: f32 = 0.33;
const SEARCH_FILL_DARK_ALPHA: f32 = 0.92;
const SEARCH_RIM_DARK: f32 = 0.28;
/// The same in a light appearance: the fill white and the rim black at these opacities. On the window's content the
/// capsule is a text field's recessed well instead.
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
    /// No chrome, the text at this many points and in this weight (the body type's when `None`), set as the third
    /// says: a [`BareField`] or a [`BareSecureField`].
    Bare(u16, Option<u16>, BareSetting),
}

/// Where a field without chrome sets its text: centred in it while it fits, and clear of a strip this many points
/// wide at its trailing end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BareSetting {
    pub(crate) centered: bool,
    pub(crate) trailing: u16,
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
            FieldLook::Bare(size, ..) => (f64::from(size) * BARE_LINE_SHARE).ceil(),
        }
    }

    /// The size of the field's text: the body type's, or a bare field's own.
    fn text_size(self, theme: &ThemeTokens) -> f32 {
        match self {
            FieldLook::Plain | FieldLook::Search(_) => theme.type_scale.body.size,
            FieldLook::Bare(size, ..) => f32::from(size),
        }
    }

    /// The weight of the field's text: the body type's, or a bare field's own.
    fn text_weight(self, theme: &ThemeTokens) -> u16 {
        match self {
            FieldLook::Bare(_, Some(weight), _) => weight,
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
        FieldLook::Bare(_, _, setting) => (0.0, f64::from(setting.trailing)),
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

/// The room between an empty centred field's caret and the start of its placeholder, in points.
const PLACEHOLDER_CARET_GAP: f64 = 1.0;

/// The field centres its text ([`BareSecureField::centered`]).
fn centred(field: FieldLook) -> bool {
    matches!(
        field,
        FieldLook::Bare(_, _, BareSetting { centered: true, .. })
    )
}

/// The left edge of text `width` wide before it scrolls: the interior's start, or in right-to-left as far right as
/// leaves the caret room after it; in a field that centres its text, centred while it fits (the caret's room
/// counted).
fn leading_x(env: &Environment, area: Rect, width: f64, field: FieldLook) -> f64 {
    if centred(field) {
        return area.min_x() + ((area.width() - CARET_WIDTH - width) / 2.0).max(0.0);
    }
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
        leading_x(env, area, layout.size().width, field) - scroll,
        field_top(&layout, chrome, env, field),
    );
    Some((layout, origin))
}

pub(crate) fn fill_rect(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    color: Option<cupertino_theme::ui::Color>,
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
        leading_x(&env, area, layout.size().width, field) - offset,
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
    // where the placeholder starts while it shows
    let mut placeholder_x = None;
    if shown.text.is_empty() {
        if let Some(placeholder) = field_label(&env, placeholder, field) {
            let at = Point::new(
                leading_x(&env, area, placeholder.size().width, field),
                field_top(&placeholder, chrome, &env, field),
            );
            placeholder_x = (placeholder.size().width > 0.0).then_some(at.x);
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
        let mut bar = caret.translate(origin.x, origin.y);
        // an empty field that centres its text centres the caret too, which would stand in the middle of the centred
        // placeholder and through its letters: it stands just before the placeholder's first glyph instead, and
        // follows the centred text once there is some (approximate)
        if let (true, Some(x)) = (centred(field), placeholder_x) {
            let x = (x - CARET_WIDTH - PLACEHOLDER_CARET_GAP).max(area.min_x());
            bar = bar.translate(x - bar.min_x(), 0.0);
        }
        marks.caret(caret_bar(bar, env.scale), color(ink));
    }
    marks.finish(cx);
}

/// After this long without an edit, move, click or focus, the caret stops blinking and stays on, so an idle window
/// settles.
const BLINK_LIMIT_NS: u64 = 10_000_000_000;

/// How the caret's blink cycle divides, as shares of its period: the caret fades out, stays hidden, fades in and
/// stays shown. The period is two blink phases ([`Motion::caret_phase_ms`](cupertino_theme::ui::Motion)), less a
/// fiftieth, so the default phase gives a period just under a second.
const BLINK_PERIOD_SHARE: f64 = 1.96;
const BLINK_FADE_OUT: f64 = 0.12 / 0.98;
const BLINK_FADE_IN: f64 = 0.14 / 0.98;
const BLINK_SHOWN: f64 = 0.50 / 0.98;
/// How often the caret asks to be drawn again while it fades, nanoseconds.
const BLINK_FRAME_NS: f64 = 16_000_000.0;

/// The caret's blink: shown for the first stretch after each edit, move, click or focus gain, then fading out and in
/// in a cycle just under a second long, steady with Reduce Motion and after [`BLINK_LIMIT_NS`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Blink {
    since: Cell<Timestamp>,
    /// The caret holds what it shows ([`BareField::hold_caret`]): a paint for any other reason draws it at the opacity
    /// it was last drawn with, not where its cycle has come to.
    holding: Cell<bool>,
    /// The opacity the caret was last drawn with; none since the blink restarted.
    drawn: Cell<Option<f32>>,
    /// The owner starts the blink ([`BareField::blink_start`]): the start it last asked for, taken by the field's
    /// first focus gain, window activation or frame after it, which started the blink.
    start: Cell<Option<u64>>,
    /// The owner asks to start the blink, taken or not yet.
    owned: Cell<bool>,
    /// The caret draws into a layer of its own ([`BareField::caret_layer`]), fully shown: the layer's opacity shows the
    /// blink.
    layered: Cell<bool>,
}

/// How a field's caret blinks, as its window's theme says ([`Motion::caret_phase_ms`](cupertino_theme::ui::Motion)):
/// for an owner that shows the blink itself, on the compositor's clock ([`BareField::caret_layer`]). Its default is
/// the theme's own blink, before a field told its own ([`BareField::on_blink`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretBlink {
    /// The phase of its cycle in milliseconds; 0 keeps the caret steady, as with Reduce Motion.
    pub phase_ms: u32,
    /// The blink started over as this was told: an edit, a caret move or a press, or a focus gain or the window
    /// becoming active while the owner starts no blink.
    pub restarted: bool,
}

impl Default for CaretBlink {
    fn default() -> CaretBlink {
        CaretBlink {
            phase_ms: cupertino_theme::ui::CARET_PHASE_MS,
            restarted: false,
        }
    }
}

impl CaretBlink {
    /// The blink of `env`'s theme.
    pub fn of(env: &Environment, restarted: bool) -> CaretBlink {
        CaretBlink {
            phase_ms: Blink::phase(env).map_or(0, |phase| (phase / 1_000_000) as u32),
            restarted,
        }
    }

    fn phase(&self) -> Option<u64> {
        (self.phase_ms > 0).then_some(u64::from(self.phase_ms) * 1_000_000)
    }

    /// The caret's opacity `elapsed` seconds after the blink started, from 0 to 1: as the field draws it.
    pub fn opacity(&self, elapsed: f64) -> f32 {
        let elapsed = (elapsed.max(0.0) * 1e9) as u64;
        match self.phase() {
            Some(phase) if elapsed < BLINK_LIMIT_NS => match Blink::cycle(phase, elapsed) {
                Cycle::Shown(_) => 1.0,
                Cycle::Hidden(_) => 0.0,
                Cycle::Fading(opacity) => opacity,
            },
            _ => 1.0,
        }
    }

    /// The first moment, in seconds after the blink started and at or after `elapsed`, from which the caret's opacity
    /// holds (shown or hidden) for at least `hold` seconds: `elapsed` itself unless it falls in a fade or too near
    /// one.
    pub fn steady_from(&self, elapsed: f64, hold: f64) -> f64 {
        let Some(phase) = self.phase() else {
            return elapsed.max(0.0);
        };
        let hold = hold.max(0.0) * 1e9;
        let mut at = elapsed.max(0.0) * 1e9;
        // a cycle has two steady stretches: past a few cycles none that is long enough is left to find
        for _ in 0..8 {
            if at >= BLINK_LIMIT_NS as f64 {
                break;
            }
            match Blink::cycle(phase, at as u64) {
                Cycle::Shown(left) | Cycle::Hidden(left) if left >= hold => break,
                // on to the next fade, and past it
                Cycle::Shown(left) | Cycle::Hidden(left) => at += left.max(1.0),
                Cycle::Fading(_) => at += self.fade_left(phase, at),
            }
        }
        at.min(BLINK_LIMIT_NS as f64).max(elapsed.max(0.0) * 1e9) / 1e9
    }

    /// How long the fade `at` nanoseconds after the start falls in has left, in nanoseconds.
    fn fade_left(&self, phase: u64, at: f64) -> f64 {
        let period = phase as f64 * BLINK_PERIOD_SHARE;
        let into = (at - BLINK_SHOWN * period).rem_euclid(period);
        // a fade out ends where the caret hides, a fade in where the shown stretch begins
        let out = BLINK_FADE_OUT * period;
        let left = if into < out {
            out - into
        } else {
            period - BLINK_SHOWN * period - into
        };
        left.max(1.0)
    }
}

/// The opacity of a caret whose blink restarted at `since`, at `now`: shown for the first stretch, then fading out and
/// in, steady with Reduce Motion and once it has blinked a while. For a caret a view draws itself.
pub fn caret_opacity(env: &Environment, since: Timestamp, now: Timestamp) -> f32 {
    Blink {
        since: Cell::new(since),
        ..Blink::default()
    }
    .opacity(env, now)
}

/// Whether a caret whose blink restarted at `since` may still change its opacity after `now`, so its view keeps
/// drawing frames.
pub fn caret_blinking(env: &Environment, since: Timestamp, now: Timestamp) -> bool {
    Blink {
        since: Cell::new(since),
        ..Blink::default()
    }
    .next_edge(env, now)
    .is_some()
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
        let elapsed = now.0.saturating_sub(self.since.get().0);
        match Self::phase(env) {
            Some(phase) if elapsed < BLINK_LIMIT_NS => match Self::cycle(phase, elapsed) {
                Cycle::Shown(_) => 1.0,
                Cycle::Hidden(_) => 0.0,
                Cycle::Fading(opacity) => opacity,
            },
            _ => 1.0,
        }
    }

    /// The opacity to draw the caret with at `now`: while it holds, the one it was last drawn with.
    fn drawn_opacity(&self, env: &Environment, now: Timestamp) -> f32 {
        if self.layered.get() {
            return 1.0;
        }
        match self.drawn.get() {
            Some(held) if self.holding.get() => held,
            _ => {
                let opacity = self.opacity(env, now);
                self.drawn.set(Some(opacity));
                opacity
            }
        }
    }

    /// The next time the caret's opacity needs drawing, or `None` once it is steady: a frame later while it fades,
    /// else when the next fade starts.
    fn next_edge(&self, env: &Environment, now: Timestamp) -> Option<Timestamp> {
        let phase = Self::phase(env)?;
        let elapsed = now.0.saturating_sub(self.since.get().0);
        if elapsed >= BLINK_LIMIT_NS {
            return None;
        }
        let wait = match Self::cycle(phase, elapsed) {
            Cycle::Fading(_) => BLINK_FRAME_NS,
            Cycle::Shown(left) | Cycle::Hidden(left) => left.max(1.0),
        };
        let at = (elapsed + wait as u64).min(BLINK_LIMIT_NS);
        Some(Timestamp(self.since.get().0 + at))
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

    /// Keeps the blink's cycle going on a wake without drawing it: the caret holds what it shows.
    fn hold<M>(&self, selecting: bool, cx: &mut EventCx<'_, M>) {
        self.arm(selecting, cx);
    }

    /// Starts the blink over at `now` when its owner asks for another start ([`BareField::blink_start`]), at the focus
    /// gain, window activation or frame that takes it; the next wake keeps it going from there.
    pub(crate) fn start_at(&self, start: Option<u64>, now: Timestamp) {
        if start.is_some() && start != self.start.get() {
            self.since.set(now);
            self.drawn.set(None);
        }
        self.start.set(start);
    }

    /// Whether the owner starts the blink, so gaining focus or the window becoming active does not.
    fn started_by_owner(&self, event: &Event) -> bool {
        self.owned.get()
            && matches!(
                event,
                Event::Focus { gained: true, .. } | Event::WindowActive(true)
            )
    }

    /// Restarts on the events that move the caret, and keeps blinking on a wake, after the field handled `event`.
    pub(crate) fn follow<M>(&mut self, event: &Event, selecting: bool, cx: &mut EventCx<'_, M>) {
        if Self::restarts(event) && self.started_by_owner(event) {
            // the caret shows, on the blink its owner started
            self.arm(selecting, cx);
            cx.request_paint();
        } else if Self::restarts(event) {
            self.since.set(cx.now());
            // shown again, even while it holds
            self.drawn.set(None);
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
            Color::new(
                SEARCH_FILL_DARK_GREY,
                SEARCH_FILL_DARK_GREY,
                SEARCH_FILL_DARK_GREY,
                SEARCH_FILL_DARK_ALPHA,
            ),
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

mod widgets;
pub(crate) use widgets::{apply_ime, committed, holds, sync_input, under};
pub use widgets::{FieldState, SecureState};
