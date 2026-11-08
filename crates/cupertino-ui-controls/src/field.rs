//! Text and password fields: the field's own buffer, its keys, the input-method conversation, submit and the model
//! sync. A password lives only in a [`SecureBuffer`]: no undo, no clipboard export, no surrounding text for the input
//! method, and assistive technology learns only that there is a value.

use crate::look::{self, paint_focus_ring};
use crate::{SearchField, SecureField, TextChange, TextField};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, TextSemantics, Widget,
};
use cupertino_ui_layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_platform::{KeyEvent, Keysym, MouseButton, PointerEvent, Timestamp};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{
    Affinity, ChangeCause, ContentHints, ContentPurpose, ImeBatch, ImeEvent, Keymap,
    ParagraphLayout, Response, SecureBuffer, Selection, Surrounding, TextBuffer, TextFieldKeys,
    TextInputSession, TextPosition, CARET_WIDTH, SURROUNDING_MAX_BYTES,
};
use cupertino_ui_theme::{ColorRole, ThemeTokens};
use std::cell::Cell;
use std::ops::Range;
use std::time::Duration;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);
const BULLET: char = '\u{2022}';
/// Points between the baseline and the composition's one-pixel underline.
const COMPOSITION_DROP: f64 = 1.5;

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

/// A search field's height, the magnifying glass's size and its leading edge, the text's leading edge, and its fill
/// and rim: white at these opacities in a dark appearance, the fill white and the rim black in a light one.
const SEARCH_HEIGHT: f64 = 28.0;
const SEARCH_GLYPH: f64 = 12.5;
const SEARCH_GLYPH_X: f64 = 9.5;
const SEARCH_TEXT_X: f64 = 28.0;
const SEARCH_FILL_DARK: f32 = 0.2;
const SEARCH_RIM_DARK: f32 = 0.12;
const SEARCH_FILL_LIGHT: f32 = 0.45;
const SEARCH_RIM_LIGHT: f32 = 0.09;

/// How a text field draws its chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FieldLook {
    #[default]
    Plain,
    /// A capsule with a magnifying glass before the text: a [`SearchField`].
    Search,
}

impl FieldLook {
    fn height(self, theme: &ThemeTokens) -> f64 {
        match self {
            FieldLook::Plain => theme.metrics.field_height,
            FieldLook::Search => SEARCH_HEIGHT,
        }
    }
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
        FieldLook::Search => (SEARCH_TEXT_X, inset),
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
    let layout = look::paragraph(env, text, Some(look::direction(env)))?;
    let chrome = chrome(size, &env.theme, field);
    let area = interior(size, &env.theme, field, env.direction);
    let origin = Point::new(
        leading_x(env, area, layout.size().width) - scroll,
        look::top(&layout, chrome, env),
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
    caret: TextPosition,
    selection: Range<usize>,
    composition: Option<Range<usize>>,
    /// The caret is in the on phase of its blink.
    caret_on: bool,
}

/// Paints a field's text, or its placeholder while empty, with the selection and caret while focused and the
/// composition's underline, all clipped to the interior. Scrolls so the caret stays inside while focused, and to the
/// text's start otherwise.
fn paint_text(
    cx: &mut PaintCx<'_>,
    look: &ControlState,
    enabled: bool,
    shown: &Shown<'_>,
    placeholder: &str,
    scroll: &Cell<f64>,
    field: FieldLook,
) {
    let env = cx.env().clone();
    let size = cx.size();
    let area = interior(size, &env.theme, field, env.direction);
    let chrome = chrome(size, &env.theme, field);
    let Some(layout) = look::paragraph(&env, shown.text, Some(look::direction(&env))) else {
        return;
    };
    let caret = layout.caret_rect(shown.caret);
    let focused = look.focused && enabled;
    let (shown_x, shown_width) = if focused {
        (caret.min_x(), CARET_WIDTH)
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
        look::top(&layout, chrome, &env),
    );
    let color = |role| look::color(&env.theme, role, look);
    cx.push(PaintOp::PushClip(Shape::Rect(area)));
    if focused && !shown.selection.is_empty() {
        for rect in layout.selection_rects(shown.selection.clone()) {
            fill_rect(
                cx,
                rect.translate(origin.x, origin.y),
                color(ColorRole::SelectedTextBackground),
            );
        }
    }
    if shown.text.is_empty() {
        if let Some(placeholder) = look::label(&env, placeholder) {
            let at = Point::new(
                leading_x(&env, area, placeholder.size().width),
                look::top(&placeholder, chrome, &env),
            );
            // a search field's placeholder reads as its label, so it is stronger than a plain field's
            let role = match field {
                FieldLook::Plain => ColorRole::PlaceholderText,
                FieldLook::Search => ColorRole::SecondaryText,
            };
            look::glyphs(cx, &placeholder, at, color(role));
        }
    } else {
        look::glyphs(cx, &layout, origin, color(ColorRole::Text));
    }
    if let Some(range) = &shown.composition {
        let start = layout.caret_rect(position(range.start)).min_x();
        let end = layout.caret_rect(position(range.end)).min_x();
        let y = look::baseline(chrome, &env) + COMPOSITION_DROP;
        fill_rect(
            cx,
            Rect::new(origin.x + start, y, end - start, 1.0 / env.scale),
            color(ColorRole::Text),
        );
    }
    if focused && look.window_active && shown.selection.is_empty() && shown.caret_on {
        fill_rect(
            cx,
            caret.translate(origin.x, origin.y),
            color(ColorRole::Text),
        );
    }
    cx.push(PaintOp::PopClip);
}

/// After this long without an edit, move, click or focus, the caret stops blinking and stays on, so an idle window
/// settles.
const BLINK_LIMIT_NS: u64 = 10_000_000_000;

/// The caret's blink: on for the first phase after each edit, move, click or focus gain, then alternating, steady with
/// Reduce Motion and after [`BLINK_LIMIT_NS`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Blink {
    since: Timestamp,
}

impl Blink {
    fn phase(env: &Environment) -> Option<u64> {
        let motion = env.theme.motion;
        let phase = u64::from(motion.caret_phase_ms) * 1_000_000;
        (!motion.reduce && phase > 0).then_some(phase)
    }

    pub(crate) fn on(&self, env: &Environment, now: Timestamp) -> bool {
        let elapsed = now.0.saturating_sub(self.since.0);
        match Self::phase(env) {
            Some(phase) if elapsed < BLINK_LIMIT_NS => (elapsed / phase).is_multiple_of(2),
            _ => true,
        }
    }

    /// The next time the caret shows or hides, or `None` once it is steady.
    fn next_edge(&self, env: &Environment, now: Timestamp) -> Option<Timestamp> {
        let phase = Self::phase(env)?;
        let elapsed = now.0.saturating_sub(self.since.0);
        (elapsed < BLINK_LIMIT_NS).then(|| {
            let next = (elapsed / phase + 1) * phase;
            Timestamp(self.since.0 + next.min(BLINK_LIMIT_NS))
        })
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
            self.arm(selecting, cx);
            cx.request_paint();
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
    if field == FieldLook::Search {
        paint_search_chrome(cx, rect, state);
        return;
    }
    let radius = theme.metrics.field_radius;
    look::fill(
        cx,
        rect,
        radius,
        look::color(&theme, ColorRole::TextBackground, state),
    );
    look::stroke(
        cx,
        rect.inset(cupertino_geometry::Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        look::color(&theme, ColorRole::TextFieldBorder, state),
    );
    paint_focus_ring(cx, rect, radius);
}

/// A search field's capsule, its rim and its magnifying glass at the reading direction's start.
fn paint_search_chrome(cx: &mut PaintCx<'_>, rect: Rect, state: &ControlState) {
    let theme = cx.env().theme.clone();
    let radius = rect.height() / 2.0;
    let white = cupertino_ui_theme::Color::new(1.0, 1.0, 1.0, 1.0);
    let black = cupertino_ui_theme::Color::new(0.0, 0.0, 0.0, 1.0);
    let (fill, rim) = if theme.preferences.appearance.is_dark() {
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
    look::fill(cx, rect, radius, Some(fill));
    look::stroke(
        cx,
        rect.inset(cupertino_geometry::Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        Some(rim),
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
    paint_focus_ring(cx, rect, radius);
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
    look: FieldLook,
}

/// The text without what an input method is still composing: what the model holds.
pub(crate) fn committed(buffer: &TextBuffer) -> String {
    let text = buffer.text();
    match buffer.composition() {
        Some(range) => format!("{}{}", &text[..range.start], &text[range.end..]),
        None => text.to_string(),
    }
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

/// Tells the input method where an editable text stands now, with its caret at `caret` in window coordinates.
pub(crate) fn sync_input<M>(
    buffer: &TextBuffer,
    session: &mut TextInputSession,
    caret: Option<Rect>,
    cause: ChangeCause,
    cx: &mut EventCx<'_, M>,
) {
    session.select(Selection {
        anchor: position(anchor(buffer)),
        focus: position(buffer.caret()),
    });
    let surrounding = Surrounding::around(
        buffer.text(),
        buffer.caret(),
        anchor(buffer),
        SURROUNDING_MAX_BYTES,
    );
    let field = cx.bounds();
    cx.set_text_input(Some(session.state(Some(surrounding), cause, caret, field)));
}

/// Applies one input-method batch: delete around the caret, commit, then show the preedit. A batch that answers an
/// older state resets the composition instead.
pub(crate) fn apply_ime(buffer: &mut TextBuffer, session: &TextInputSession, batch: &ImeBatch) {
    if !session.accepts(batch) {
        buffer.cancel_composition();
        return;
    }
    let (before, after) = batch.delete;
    if before > 0 || after > 0 {
        let caret = buffer.caret();
        let start = caret.saturating_sub(before as usize);
        let end = (caret + after as usize).min(buffer.text().len());
        buffer.select(start..end);
        buffer.delete_selection();
    }
    if let Some(commit) = &batch.commit {
        if buffer.composition().is_some() {
            buffer.set_composition(commit);
            buffer.commit_composition();
        } else {
            buffer.insert(commit);
        }
    }
    match &batch.preedit {
        Some(preedit) => buffer.set_composition(&preedit.text),
        None => buffer.cancel_composition(),
    }
}

impl FieldState {
    fn committed(&self) -> String {
        committed(&self.buffer)
    }

    /// The caret in window coordinates, for the input method's candidate window, at the scroll the next paint takes.
    fn caret_rect<M>(&self, cx: &EventCx<'_, M>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        let (layout, origin) = placed(cx.env(), self.buffer.text(), bounds.size, 0.0, self.look)?;
        let caret = layout.caret_rect(self.buffer.caret_position());
        let width = interior(bounds.size, &cx.env().theme, self.look, cx.env().direction).width();
        let scroll = follow(
            self.scroll.get(),
            caret.min_x(),
            CARET_WIDTH,
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
    /// and third press, extended to where the pointer goes. The press captures the pointer, so a drag past the field
    /// still extends the selection.
    fn select<M>(&mut self, event: &Event, cx: &mut EventCx<'_, M>) -> Option<Propagation> {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.node() == cx.target() && self.buffer.composition().is_none() => {
                let at = self.index_at(position, cx)?;
                let now = Duration::from_nanos(cx.now().0);
                self.buffer.press_at(at, false, now);
                cx.capture_pointer();
                None
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if self.buffer.is_dragging() => {
                let at = self.index_at(position, cx)?;
                self.buffer.drag_to(at);
                Some(Propagation::Stop)
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
                cx.release_pointer();
                Some(Propagation::Stop)
            }
            // the capture was lost: the drag ends where it is
            Event::Cancel if self.buffer.is_dragging() => {
                self.buffer.release();
                Some(Propagation::Continue)
            }
            _ => None,
        }
    }

    /// Tells the input method where the field stands now.
    fn sync_input<M>(&mut self, cause: ChangeCause, cx: &mut EventCx<'_, M>) {
        let caret = self.caret_rect(cx);
        sync_input(&self.buffer, &mut self.session, caret, cause, cx);
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
            let ended = state.select(event, cx);
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
            Event::Focus { gained: false, .. } => {
                state.buffer.cancel_composition();
                state.buffer.blur();
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && cx.node() == cx.target() => {
                self.key(state, key, cx)
            }
            Event::Ime(ImeEvent::Done(batch)) if self.enabled => {
                let before = state.committed();
                apply_ime(&mut state.buffer, &state.session, batch);
                if state.committed() != before {
                    self.report(state, cx);
                }
                state.sync_input(ChangeCause::InputMethod, cx);
                cx.request_paint();
                Propagation::Stop
            }
            Event::Ime(ImeEvent::Leave) => {
                state.buffer.cancel_composition();
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
            look: FieldLook::Plain,
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
        propagation
    }

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        let look = control_state(self.enabled, state.hovered, cx);
        paint_chrome(cx, &look, state.look);
        let shown = Shown {
            text: state.buffer.text(),
            caret: state.buffer.caret_position(),
            selection: state.buffer.selection(),
            composition: state.buffer.composition(),
            caret_on: state.blink.on(cx.env(), cx.now()),
        };
        paint_text(
            cx,
            &look,
            self.enabled,
            &shown,
            &self.placeholder,
            &state.scroll,
            state.look,
        );
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
        if let Some(propagation) = pointer(event, self.enabled, &mut state.hovered, cx) {
            return propagation;
        }
        match event {
            Event::Focus { gained: true, .. } if self.enabled => {
                cx.set_text_input(Some(state.session.state(
                    None,
                    ChangeCause::Other,
                    None,
                    None,
                )));
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && cx.node() == cx.target() => {
                self.key(state, key, cx)
            }
            // a password takes committed text only; a composition is never held anywhere
            Event::Ime(ImeEvent::Done(batch)) if self.enabled && state.session.accepts(batch) => {
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
        SecureState {
            buffer,
            session: session(ContentPurpose::Password, true),
            hovered: false,
            scroll: Cell::new(0.0),
            blink: Blink::default(),
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
        _: &SecureState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, FieldLook::Plain)
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
        paint_chrome(cx, &look, FieldLook::Plain);
        // the text system shapes only bullets: one per grapheme, with the caret and selection counted in graphemes
        let (caret, selection) = state.buffer.masked_selection();
        let width = BULLET.len_utf8();
        let mask = state.buffer.mask(BULLET);
        let shown = Shown {
            text: &mask,
            caret: position(caret * width),
            selection: selection.start * width..selection.end * width,
            composition: None,
            caret_on: state.blink.on(cx.env(), cx.now()),
        };
        paint_text(
            cx,
            &look,
            self.enabled,
            &shown,
            &self.placeholder,
            &state.scroll,
            FieldLook::Plain,
        );
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
        }
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
            look: FieldLook::Search,
            ..self.field.state()
        }
    }

    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        self.field.update(&old.field, state)
    }

    fn measure(
        &self,
        _: &FieldState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        measure(constraints, cx, FieldLook::Search)
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

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        self.field.semantics(state, cx)
    }
}
