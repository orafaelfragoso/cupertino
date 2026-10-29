//! Text and password fields: the field's own buffer, its keys, the input-method conversation, submit and the model
//! sync. A password lives only in a [`SecureBuffer`]: no undo, no clipboard export, no surrounding text for the input
//! method, and assistive technology learns only that there is a value.

use crate::look::{self, paint_focus_ring};
use crate::{SecureField, TextChange, TextField};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, TextSemantics, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{KeyEvent, Keysym, MouseButton, PointerEvent};
use cupertino_ui_text::{
    Affinity, ChangeCause, ContentHints, ContentPurpose, ImeBatch, ImeEvent, Keymap, Response,
    SecureBuffer, Selection, Surrounding, TextBuffer, TextFieldKeys, TextInputSession,
    TextPosition, SURROUNDING_MAX_BYTES,
};
use cupertino_ui_theme::{ColorRole, ThemeTokens};

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);

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
        cx.set_text_input(Some(self.session.state(
            Some(surrounding),
            cause,
            None,
            None,
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
