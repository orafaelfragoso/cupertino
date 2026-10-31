//! A multi-line editor: its buffer laid out one paragraph at a time, so an edit lays out only the paragraphs it touched,
//! and painted only where it shows. It grows with its text; a vertical scroll view around it scrolls it.

use crate::field::{self, Blink};
use crate::look;
use crate::{TextChange, TextEditor};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, Environment, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, TextSemantics, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{KeyEvent, Keysym, MouseButton, PointerEvent};
use cupertino_ui_text::{
    AttributedText, ChangeCause, ContentPurpose, FontWeight, ImeEvent, Keymap, LineLayout,
    ParagraphLayout, ParagraphStyle, Response, TextBuffer, TextEditorKeys, TextInputSession,
    TextPosition, TextStyle, TextSystem, CARET_WIDTH,
};
use cupertino_ui_theme::ColorRole;
use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

const ESCAPE: Keysym = Keysym(0xff1b);
/// Points between a line's baseline and the composition's one-pixel underline.
const COMPOSITION_DROP: f64 = 1.5;

/// One paragraph (the text between two line breaks) laid out.
struct Laid {
    start: usize,
    len: usize,
    top: f64,
    /// The index of its first visual line among all the editor's.
    first_line: usize,
    layout: ParagraphLayout,
}

/// What the paragraphs were laid out for; a change lays them all out again.
struct Basis {
    text: Arc<TextSystem>,
    style: TextStyle,
    width: Option<f32>,
}

impl Basis {
    fn of(env: &Environment, width: Option<f32>) -> Self {
        let body = env.theme.type_scale.body;
        Basis {
            text: env.text.clone(),
            style: TextStyle {
                weight: FontWeight(body.weight),
                ..TextStyle::new(body.size)
            },
            width,
        }
    }

    fn same(&self, other: &Basis) -> bool {
        Arc::ptr_eq(&self.text, &other.text)
            && self.style == other.style
            && self.width == other.width
    }

    fn lay_out(&self, text: &str) -> Option<ParagraphLayout> {
        let paragraph = self
            .text
            .paragraph(
                &AttributedText::plain(text, self.style.clone()),
                &ParagraphStyle::default(),
            )
            .ok()?;
        Some(paragraph.layout(self.width))
    }
}

/// The editor's text laid out, in coordinates with the origin at the text's top-left corner.
#[derive(Default)]
struct Lines {
    basis: Option<Basis>,
    text: String,
    paragraphs: Vec<Laid>,
    /// Visual lines a page moves, from the part of the editor that last showed.
    page_lines: usize,
}

impl Lines {
    /// Lays `text` out at `width`: after an edit, only the paragraphs it touched. Empty when the fonts cannot shape it.
    fn sync(&mut self, env: &Environment, text: &str, width: Option<f32>) {
        let basis = Basis::of(env, width);
        if !self.basis.as_ref().is_some_and(|b| b.same(&basis)) {
            self.basis = Some(basis);
            self.paragraphs.clear();
        }
        if self.paragraphs.is_empty() {
            self.splice(0..0, text, 0);
        } else if self.text != text {
            let old = self.text.as_bytes();
            let new = text.as_bytes();
            let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
            let suffix = old[prefix..]
                .iter()
                .rev()
                .zip(new[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let first = self.index_at(prefix);
            let last = self.index_at(old.len() - suffix);
            let from = self.paragraphs[first].start;
            let to = self.paragraphs[last].start + self.paragraphs[last].len;
            let end = to + text.len() - self.text.len();
            self.splice(first..last + 1, &text[from..end], from);
        }
        self.text.clear();
        self.text.push_str(text);
    }

    /// Replaces the paragraphs `replaced` with `text`'s, laid out, starting at byte `from`; moves the ones after.
    fn splice(&mut self, replaced: Range<usize>, text: &str, from: usize) {
        let Some(basis) = &self.basis else {
            return;
        };
        let mut laid = Vec::new();
        let mut start = from;
        for paragraph in text.split('\n') {
            let Some(layout) = basis.lay_out(paragraph) else {
                self.paragraphs.clear();
                return;
            };
            laid.push(Laid {
                start,
                len: paragraph.len(),
                top: 0.0,
                first_line: 0,
                layout,
            });
            start += paragraph.len() + 1;
        }
        let first = replaced.start;
        let old_end = self
            .paragraphs
            .get(replaced.end.saturating_sub(1))
            .filter(|_| !replaced.is_empty())
            .map_or(from, |p| p.start + p.len);
        let shift = (from + text.len()) as isize - old_end as isize;
        self.paragraphs.splice(replaced, laid);
        let mut top = 0.0;
        let mut line = 0;
        if let Some(before) = first.checked_sub(1).map(|i| &self.paragraphs[i]) {
            top = before.top + before.layout.size().height;
            line = before.first_line + before.layout.lines().len();
        }
        let moved = first + text.split('\n').count();
        for (i, p) in self.paragraphs.iter_mut().enumerate().skip(first) {
            if i >= moved {
                p.start = p.start.saturating_add_signed(shift);
            }
            p.top = top;
            p.first_line = line;
            top += p.layout.size().height;
            line += p.layout.lines().len();
        }
    }

    fn height(&self) -> f64 {
        self.paragraphs
            .last()
            .map_or(0.0, |p| p.top + p.layout.size().height)
    }

    /// The paragraph that holds byte `at`.
    fn index_at(&self, at: usize) -> usize {
        self.paragraphs
            .partition_point(|p| p.start <= at)
            .saturating_sub(1)
    }

    /// The paragraph at `y`, the first or last past the text's ends.
    fn index_at_y(&self, y: f64) -> usize {
        self.paragraphs
            .partition_point(|p| p.top <= y)
            .saturating_sub(1)
    }

    fn line(&self, line: usize) -> Option<(&Laid, usize)> {
        let index = self
            .paragraphs
            .partition_point(|p| p.first_line <= line)
            .checked_sub(1)?;
        let laid = &self.paragraphs[index];
        Some((laid, line - laid.first_line))
    }

    fn caret_rect(&self, at: TextPosition) -> Option<Rect> {
        let laid = self.paragraphs.get(self.index_at(at.byte))?;
        Some(
            laid.layout
                .caret_rect(local(at, laid.start))
                .translate(0.0, laid.top),
        )
    }

    fn index_at_point(&self, point: Point) -> Option<usize> {
        let laid = self.paragraphs.get(self.index_at_y(point.y))?;
        let at = laid
            .layout
            .index_at(Point::new(point.x, point.y - laid.top));
        Some(laid.start + at.byte)
    }

    /// The paragraphs that `top..bottom` shows.
    fn shown(&self, top: f64, bottom: f64) -> &[Laid] {
        let first = self.index_at_y(top);
        let end = self.paragraphs.partition_point(|p| p.top < bottom);
        &self.paragraphs[first..end.max(first)]
    }
}

fn local(at: TextPosition, start: usize) -> TextPosition {
    TextPosition {
        byte: at.byte.saturating_sub(start),
        ..at
    }
}

fn global(at: TextPosition, start: usize) -> TextPosition {
    TextPosition {
        byte: at.byte + start,
        ..at
    }
}

impl LineLayout for Lines {
    fn line_count(&self) -> usize {
        self.paragraphs
            .last()
            .map_or(0, |p| p.first_line + p.layout.lines().len())
    }

    fn locate(&self, at: TextPosition) -> (usize, f64) {
        let Some(laid) = self.paragraphs.get(self.index_at(at.byte)) else {
            return (0, 0.0);
        };
        let caret = laid.layout.caret_rect(local(at, laid.start));
        let line = laid
            .layout
            .lines()
            .partition_point(|line| line.rect.y() < caret.y());
        (laid.first_line + line, caret.min_x())
    }

    fn position_on(&self, line: usize, x: f64) -> TextPosition {
        let Some((laid, line)) = self.line(line) else {
            return field::position(0);
        };
        let rect = laid.layout.lines()[line].rect;
        let at = laid
            .layout
            .index_at(Point::new(x, rect.y() + rect.height() / 2.0));
        global(at, laid.start)
    }

    fn line_edges(&self, line: usize) -> (TextPosition, TextPosition) {
        let Some((laid, index)) = self.line(line) else {
            return (field::position(0), field::position(0));
        };
        let lines = laid.layout.lines();
        let range = &lines[index].range.0;
        let end = TextPosition {
            byte: laid.start + range.end,
            affinity: if index + 1 < lines.len() {
                cupertino_ui_text::Affinity::Upstream
            } else {
                cupertino_ui_text::Affinity::Downstream
            },
        };
        (field::position(laid.start + range.start), end)
    }

    fn page_lines(&self) -> usize {
        self.page_lines
    }
}

fn inset(env: &Environment) -> f64 {
    env.theme.metrics.field_inset
}

/// The width the text wraps at in an editor `width` wide, with room for the caret after a line's last character.
fn wrap_width(env: &Environment, width: f64) -> Option<f32> {
    let width = width - 2.0 * inset(env) - CARET_WIDTH;
    width.is_finite().then_some(width.max(0.0) as f32)
}

/// The node state of a [`TextEditor`]. Opaque.
pub struct EditorState {
    buffer: TextBuffer,
    keymap: Keymap,
    session: TextInputSession,
    /// The revision of the last change the editor reported.
    reported: u64,
    hovered: bool,
    blink: Blink,
    lines: RefCell<Lines>,
}

impl EditorState {
    /// Lays the text out at the editor's width; `false` before its first layout.
    fn sync<M>(&self, cx: &EventCx<'_, M>) -> bool {
        let Some(bounds) = cx.bounds() else {
            return false;
        };
        let env = cx.env();
        self.lines
            .borrow_mut()
            .sync(env, self.buffer.text(), wrap_width(env, bounds.width()));
        true
    }

    /// The caret in node coordinates.
    fn caret_rect(&self, env: &Environment) -> Option<Rect> {
        let inset = inset(env);
        self.lines
            .borrow()
            .caret_rect(self.buffer.caret_position())
            .map(|caret| caret.translate(inset, inset))
    }

    /// The text position under `point`, in window coordinates.
    fn index_at<M>(&self, point: Point, cx: &EventCx<'_, M>) -> Option<usize> {
        let bounds = cx.bounds()?;
        let inset = inset(cx.env());
        let local = point - bounds.origin;
        self.lines
            .borrow()
            .index_at_point(Point::new(local.x - inset, local.y - inset))
    }

    /// After the text, the caret or the selection changed: lays out what changed, grows or shrinks the editor with
    /// its text, shows the caret and tells the input method.
    fn changed<M>(&mut self, cause: ChangeCause, cx: &mut EventCx<'_, M>) {
        let height = self.lines.borrow().height();
        if self.sync(cx) && self.lines.borrow().height() != height {
            cx.request_layout();
        }
        cx.request_paint();
        if !cx.is_focused() {
            return;
        }
        let caret = self.caret_rect(cx.env());
        if let Some(caret) = caret {
            // with the inset around it, so the first and last lines show their margin
            let margin = inset(cx.env());
            cx.reveal(Rect::new(
                caret.min_x(),
                caret.min_y() - margin,
                caret.width(),
                caret.height() + 2.0 * margin,
            ));
        }
        let window = caret
            .zip(cx.bounds())
            .map(|(caret, bounds)| caret.translate(bounds.min_x(), bounds.min_y()));
        field::sync_input(&self.buffer, &mut self.session, window, cause, cx);
    }

    /// A press, a drag with the button held and its release, as in a text field.
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
                self.buffer
                    .press_at(at, false, Duration::from_nanos(cx.now().0));
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
            Event::Cancel if self.buffer.is_dragging() => {
                self.buffer.release();
                Some(Propagation::Continue)
            }
            _ => None,
        }
    }
}

impl<M: Clone + 'static> TextEditor<M> {
    fn handle(
        &self,
        state: &mut EditorState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if self.enabled {
            let selection = state.buffer.selection();
            let caret = state.buffer.caret_position();
            let ended = state.select(event, cx);
            if state.buffer.selection() != selection || state.buffer.caret_position() != caret {
                state.changed(ChangeCause::Other, cx);
            }
            if let Some(propagation) = ended {
                return propagation;
            }
        }
        if let Some(propagation) = field::pointer(event, self.enabled, &mut state.hovered, cx) {
            return propagation;
        }
        match event {
            Event::Focus { gained: true, .. } if self.enabled => {
                state.changed(ChangeCause::Other, cx);
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
                let before = field::committed(&state.buffer);
                field::apply_ime(&mut state.buffer, &state.session, batch);
                if field::committed(&state.buffer) != before {
                    self.report(state, cx);
                }
                state.changed(ChangeCause::InputMethod, cx);
                Propagation::Stop
            }
            Event::Ime(ImeEvent::Leave) => {
                state.buffer.cancel_composition();
                state.changed(ChangeCause::Other, cx);
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
                state.changed(ChangeCause::Other, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn report(&self, state: &mut EditorState, cx: &mut EventCx<'_, M>) {
        state.reported += 1;
        if let Some(on_change) = &self.on_change {
            cx.emit(on_change(TextChange {
                text: field::committed(&state.buffer),
                revision: state.reported,
            }));
        }
    }

    fn key(&self, state: &mut EditorState, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        if !key.pressed {
            return Propagation::Continue;
        }
        if key.keysym == ESCAPE {
            if state.buffer.composition().is_none() {
                return Propagation::Continue;
            }
            state.buffer.cancel_composition();
            state.changed(ChangeCause::Other, cx);
            return Propagation::Stop;
        }
        state.sync(cx);
        let response = {
            let lines = state.lines.borrow();
            state
                .buffer
                .editor_key(&mut state.keymap, key.code, key.mods, key.text, &*lines)
        };
        match response {
            Response::Ignored => return Propagation::Continue,
            Response::Moved | Response::Paste => {}
            Response::Edited => self.report(state, cx),
            Response::Copy(text) => cx.set_clipboard(text),
            Response::Cut(text) => {
                cx.set_clipboard(text);
                self.report(state, cx);
            }
        }
        state.changed(ChangeCause::Other, cx);
        Propagation::Stop
    }
}

impl<M: Clone + 'static> Widget<M> for TextEditor<M> {
    type State = EditorState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> EditorState {
        let mut buffer = TextBuffer::multiline();
        buffer.set_text(&self.value);
        let mut session = field::session(ContentPurpose::Normal, false);
        session.hints.multiline = true;
        EditorState {
            buffer,
            keymap: Keymap::standard(),
            session,
            reported: self.revision,
            hovered: false,
            blink: Blink::default(),
            lines: RefCell::default(),
        }
    }

    /// Takes the model's text when the application changed it itself, as a text field does.
    fn update(&self, old: &Self, state: &mut EditorState) -> Change {
        if self.revision >= state.reported && self.value != field::committed(&state.buffer) {
            state.buffer.set_text(&self.value);
            state.reported = self.revision;
            return Change::Layout;
        }
        if self.enabled != old.enabled
            || self.label != old.label
            || self.placeholder != old.placeholder
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    /// As wide as offered and as tall as its text, at least the height offered, so a press below the text lands on
    /// it.
    fn measure(
        &self,
        state: &EditorState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        let env = cx.env();
        let width = constraints.width.max();
        let mut lines = state.lines.borrow_mut();
        lines.sync(env, state.buffer.text(), wrap_width(env, width));
        let width = if width.is_finite() {
            width
        } else {
            constraints.width.ideal()
        };
        Measured {
            size: Size::new(
                width,
                (lines.height() + 2.0 * inset(env)).max(constraints.height.min()),
            ),
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut EditorState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let propagation = self.handle(state, event, cx);
        let selecting = !state.buffer.selection().is_empty();
        state.blink.follow(event, selecting, cx);
        propagation
    }

    fn paint(&self, state: &EditorState, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let look = field::control_state(self.enabled, state.hovered, cx);
        let color = |role| look::color(&env.theme, role, &look);
        let visible = cx.visible();
        field::fill_rect(cx, visible, color(ColorRole::TextBackground));
        let inset = inset(&env);
        let mut lines = state.lines.borrow_mut();
        lines.sync(&env, state.buffer.text(), wrap_width(&env, cx.size().width));
        if let Some(first) = lines.paragraphs.first() {
            let line_height = first.layout.size().height / first.layout.lines().len().max(1) as f64;
            lines.page_lines = ((visible.height() - 2.0 * inset) / line_height)
                .floor()
                .max(1.0) as usize;
        }
        let focused = look.focused && self.enabled;
        let selection = state.buffer.selection();
        let composition = state.buffer.composition();
        if state.buffer.is_empty() {
            if let Some(placeholder) = look::label(&env, &self.placeholder) {
                let at = Point::new(inset, inset);
                look::glyphs(cx, &placeholder, at, color(ColorRole::PlaceholderText));
            }
        }
        for laid in lines.shown(visible.min_y() - inset, visible.max_y() - inset) {
            let origin = Point::new(inset, inset + laid.top);
            let span = laid.start..laid.start + laid.len + 1;
            let part = |range: &Range<usize>| {
                let start = range.start.max(span.start);
                let end = range.end.min(span.end);
                (start < end).then(|| start - laid.start..(end - laid.start).min(laid.len))
            };
            if focused {
                if let Some(range) = part(&selection) {
                    for rect in laid.layout.selection_rects(range) {
                        field::fill_rect(
                            cx,
                            rect.translate(origin.x, origin.y),
                            color(ColorRole::SelectedTextBackground),
                        );
                    }
                }
            }
            look::glyphs(cx, &laid.layout, origin, color(ColorRole::Text));
            if let Some(range) = composition.as_ref().and_then(part) {
                for rect in laid.layout.selection_rects(range) {
                    let Some(line) = laid.layout.lines().iter().find(|l| l.rect.y() == rect.y())
                    else {
                        continue;
                    };
                    field::fill_rect(
                        cx,
                        Rect::new(
                            origin.x + rect.min_x(),
                            origin.y + line.baseline + COMPOSITION_DROP,
                            rect.width(),
                            1.0 / env.scale,
                        ),
                        color(ColorRole::Text),
                    );
                }
            }
        }
        if focused && look.window_active && selection.is_empty() && state.blink.on(&env, cx.now()) {
            if let Some(caret) = lines.caret_rect(state.buffer.caret_position()) {
                field::fill_rect(cx, caret.translate(inset, inset), color(ColorRole::Text));
            }
        }
    }

    fn semantics(&self, state: &EditorState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::MultilineTextInput);
        cx.name(self.label.clone());
        let (caret, selection) = match state.buffer.composition() {
            Some(range) => (range.start, range.start..range.start),
            None => (state.buffer.caret(), state.buffer.selection()),
        };
        cx.value(AccessibleValue::Text {
            value: field::committed(&state.buffer),
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
        }
    }
}
