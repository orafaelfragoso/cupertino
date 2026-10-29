//! Text and password fields: the field's own buffer, its keys, the input-method conversation, submit and the model
//! sync. A password lives only in a [`SecureBuffer`]: no undo, no clipboard export, no surrounding text for the input
//! method, and assistive technology learns only that there is a value.

use crate::look::{self, paint_focus_ring};
use crate::{SecureField, TextChange, TextField};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, TextSemantics, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{KeyEvent, Keysym, MouseButton, PointerEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{
    Affinity, ChangeCause, ContentHints, ContentPurpose, ImeBatch, ImeEvent, Keymap,
    ParagraphLayout, Response, SecureBuffer, Selection, Surrounding, TextBuffer, TextFieldKeys,
    TextInputSession, TextPosition, SURROUNDING_MAX_BYTES,
};
use cupertino_ui_theme::{ColorRole, ThemeTokens};
use std::cell::Cell;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);
/// Points between the baseline and the composition's one-pixel underline.
const COMPOSITION_DROP: f64 = 1.5;

fn session(purpose: ContentPurpose, protected: bool) -> TextInputSession {
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

fn position(byte: usize) -> TextPosition {
    TextPosition {
        byte,
        affinity: Affinity::Downstream,
    }
}

/// The chrome is the field's height, centred in a node padded out to the minimum target.
fn chrome(size: Size, theme: &ThemeTokens) -> Rect {
    let height = theme.metrics.field_height;
    Rect::new(0.0, (size.height - height) / 2.0, size.width, height)
}

/// Where text, selection, composition and caret show; they are clipped to it.
fn interior(size: Size, theme: &ThemeTokens) -> Rect {
    let inset = theme.metrics.field_inset;
    chrome(size, theme).inset(Insets {
        top: 0.0,
        left: inset,
        bottom: 0.0,
        right: inset,
    })
}

/// How far the text scrolls left so the caret stays inside `width`: unchanged while it is visible, else just enough
/// to show it, and never past the text's end.
fn follow(scroll: f64, caret_x: f64, text_width: f64, width: f64) -> f64 {
    let scroll = if caret_x < scroll {
        caret_x
    } else if caret_x > scroll + width {
        caret_x - width
    } else {
        scroll
    };
    scroll.min((text_width - width).max(0.0)).max(0.0)
}

/// The text laid out where the field paints it: the layout and its top-left corner in node coordinates.
fn placed(
    env: &Environment,
    text: &str,
    size: Size,
    scroll: f64,
) -> Option<(ParagraphLayout, Point)> {
    let layout = look::paragraph(env, text)?;
    let chrome = chrome(size, &env.theme);
    let origin = Point::new(
        interior(size, &env.theme).min_x() - scroll,
        look::top(&layout, chrome, env),
    );
    Some((layout, origin))
}

fn fill_rect(cx: &mut PaintCx<'_>, rect: Rect, color: Option<cupertino_ui_theme::Color>) {
    if let Some(color) = color {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(rect),
            paint: Paint::Solid(color),
        });
    }
}

fn layout() -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

fn measure(constraints: Constraints, cx: &MeasureCx<'_>) -> Measured {
    let metrics = cx.env().theme.metrics;
    Measured {
        size: Size::new(
            constraints.width.ideal(),
            metrics.field_height.max(metrics.min_target.height),
        ),
        baseline: None,
    }
}

fn paint_chrome(cx: &mut PaintCx<'_>, state: &ControlState) {
    let theme = cx.env().theme.clone();
    let rect = chrome(cx.size(), &theme);
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

fn control_state(enabled: bool, hovered: bool, cx: &PaintCx<'_>) -> ControlState {
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
fn pointer<M>(
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
}

impl FieldState {
    /// The text without what an input method is still composing: what the model holds.
    fn committed(&self) -> String {
        let text = self.buffer.text();
        match self.buffer.composition() {
            Some(range) => format!("{}{}", &text[..range.start], &text[range.end..]),
            None => text.to_string(),
        }
    }

    fn anchor(&self) -> usize {
        let selection = self.buffer.selection();
        if self.buffer.caret() == selection.start {
            selection.end
        } else {
            selection.start
        }
    }

    /// The caret in window coordinates, for the input method's candidate window.
    fn caret_rect<M>(&self, cx: &EventCx<'_, M>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        let (layout, origin) =
            placed(cx.env(), self.buffer.text(), bounds.size, self.scroll.get())?;
        let caret = layout.caret_rect(position(self.buffer.caret()));
        Some(caret.translate(bounds.min_x() + origin.x, bounds.min_y() + origin.y))
    }

    /// Puts the caret at the text position under `point`, in window coordinates.
    fn place_caret<M>(&mut self, point: Point, cx: &EventCx<'_, M>) {
        let Some(bounds) = cx.bounds() else {
            return;
        };
        let Some((layout, origin)) =
            placed(cx.env(), self.buffer.text(), bounds.size, self.scroll.get())
        else {
            return;
        };
        let local = point - bounds.origin - origin;
        let at = layout.index_at(local).byte;
        self.buffer.select(at..at);
    }

    /// Tells the input method where the field stands now.
    fn sync_input<M>(&mut self, cause: ChangeCause, cx: &mut EventCx<'_, M>) {
        self.session.select(Selection {
            anchor: position(self.anchor()),
            focus: position(self.buffer.caret()),
        });
        let surrounding = Surrounding::around(
            self.buffer.text(),
            self.buffer.caret(),
            self.anchor(),
            SURROUNDING_MAX_BYTES,
        );
        let caret = self.caret_rect(cx);
        let field = cx.bounds();
        cx.set_text_input(Some(self.session.state(
            Some(surrounding),
            cause,
            caret,
            field,
        )));
    }

    /// Applies one input-method batch: delete around the caret, commit, then show the preedit. A batch that answers
    /// an older state resets the composition instead.
    fn ime(&mut self, batch: &ImeBatch) {
        if !self.session.accepts(batch) {
            self.buffer.cancel_composition();
            return;
        }
        let (before, after) = batch.delete;
        if before > 0 || after > 0 {
            let caret = self.buffer.caret();
            let start = caret.saturating_sub(before as usize);
            let end = (caret + after as usize).min(self.buffer.text().len());
            self.buffer.select(start..end);
            self.buffer.delete_selection();
        }
        if let Some(commit) = &batch.commit {
            if self.buffer.composition().is_some() {
                self.buffer.set_composition(commit);
                self.buffer.commit_composition();
            } else {
                self.buffer.insert(commit);
            }
        }
        match &batch.preedit {
            Some(preedit) => self.buffer.set_composition(&preedit.text),
            None => self.buffer.cancel_composition(),
        }
    }
}

impl<M: Clone + 'static> TextField<M> {
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
        buffer.set_text(&self.value);
        FieldState {
            buffer,
            keymap: Keymap::standard(),
            session: session(self.purpose, false),
            reported: self.revision,
            hovered: false,
            scroll: Cell::new(0.0),
        }
    }

    /// Takes the model's text when the application changed it itself: a revision at or past the last one the field
    /// reported, with a different text. An older revision is the model catching up and is ignored.
    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        state.session.purpose = self.purpose;
        if self.revision >= state.reported && self.value != state.committed() {
            state.buffer.set_text(&self.value);
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
        measure(constraints, cx)
    }

    fn event(&self, state: &mut FieldState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::Pointer(
            PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed: true,
                ..
            },
            _,
        ) = event
        {
            if self.enabled && cx.node() == cx.target() && state.buffer.composition().is_none() {
                state.place_caret(*position, cx);
                if cx.is_focused() {
                    state.sync_input(ChangeCause::Other, cx);
                }
                cx.request_paint();
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
            Event::Focus { gained: false, .. } | Event::Cancel => {
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
                state.ime(batch);
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

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        let look = control_state(self.enabled, state.hovered, cx);
        paint_chrome(cx, &look);
        let env = cx.env().clone();
        let size = cx.size();
        let area = interior(size, &env.theme);
        let text = state.buffer.text();
        let Some(layout) = look::paragraph(&env, text) else {
            return;
        };
        let caret = layout.caret_rect(position(state.buffer.caret()));
        let scroll = follow(
            state.scroll.get(),
            caret.min_x(),
            layout.size().width,
            area.width(),
        );
        state.scroll.set(scroll);
        let origin = Point::new(
            area.min_x() - scroll,
            look::top(&layout, chrome(size, &env.theme), &env),
        );
        let color = |role| look::color(&env.theme, role, &look);
        cx.push(PaintOp::PushClip(Shape::Rect(area)));
        let focused = look.focused && self.enabled;
        let selection = state.buffer.selection();
        if focused && !selection.is_empty() {
            for rect in layout.selection_rects(selection) {
                fill_rect(
                    cx,
                    rect.translate(origin.x, origin.y),
                    color(ColorRole::SelectedTextBackground),
                );
            }
        }
        if text.is_empty() {
            if let Some(placeholder) = look::label(&env, &self.placeholder) {
                let at = Point::new(
                    area.min_x(),
                    look::top(&placeholder, chrome(size, &env.theme), &env),
                );
                look::glyphs(cx, &placeholder, at, color(ColorRole::PlaceholderText));
            }
        } else {
            look::glyphs(cx, &layout, origin, color(ColorRole::Text));
        }
        if let Some(range) = state.buffer.composition() {
            let start = layout.caret_rect(position(range.start)).min_x();
            let end = layout.caret_rect(position(range.end)).min_x();
            let y = look::baseline(chrome(size, &env.theme), &env) + COMPOSITION_DROP;
            fill_rect(
                cx,
                Rect::new(origin.x + start, y, end - start, 1.0 / env.scale),
                color(ColorRole::Text),
            );
        }
        if focused && look.window_active && state.buffer.selection().is_empty() {
            fill_rect(
                cx,
                caret.translate(origin.x, origin.y),
                color(ColorRole::Text),
            );
        }
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TextInput);
        cx.name(self.label.clone());
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
        }
    }
}

/// The node state of a [`SecureField`]. Opaque.
pub struct SecureState {
    buffer: SecureBuffer,
    session: TextInputSession,
    hovered: bool,
}

impl<M: 'static> SecureField<M> {
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
        match key.keysym.0 {
            0xff08 => buffer.delete_backward(),
            0xffff => buffer.delete_forward(),
            0xff51 if command => buffer.move_to_start(extend),
            0xff53 if command => buffer.move_to_end(extend),
            0xff51 => buffer.move_char(false, extend),
            0xff53 => buffer.move_char(true, extend),
            0xff50 => buffer.move_to_start(extend),
            0xff57 => buffer.move_to_end(extend),
            _ if command && key.text == Some('a') => buffer.select_all(),
            // copy, cut and every other command chord do nothing in a password field, and nothing reaches the window
            _ if command || key.mods.contains(Mods::CTRL) => {}
            _ => match key.text {
                Some(c) if !c.is_control() => buffer.insert(c.encode_utf8(&mut [0; 4])),
                _ => return Propagation::Continue,
            },
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
        SecureState {
            buffer: SecureBuffer::new(),
            session: session(ContentPurpose::Password, true),
            hovered: false,
        }
    }

    fn update(&self, old: &Self, _: &mut SecureState) -> Change {
        if self.enabled != old.enabled
            || self.label != old.label
            || self.placeholder != old.placeholder
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
        measure(constraints, cx)
    }

    fn event(
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
                    state.buffer.insert(commit);
                    cx.request_paint();
                }
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &SecureState, cx: &mut PaintCx<'_>) {
        let look = ControlState {
            protected: true,
            ..control_state(self.enabled, state.hovered, cx)
        };
        paint_chrome(cx, &look);
    }

    fn semantics(&self, _: &SecureState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PasswordInput);
        cx.name(self.label.clone());
        cx.protected();
        cx.value(AccessibleValue::Protected);
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
        }
    }
}
