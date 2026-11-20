use super::*;

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
    /// What opens the field's menu.
    menu: text_menu::Opener,
    /// Whether the field has a menu and takes a paste: every field but a [`BareField`] that turns them off.
    pastes: bool,
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

/// Where a menu opened from the keyboard stands: under `caret` (window coordinates), or at the field's corner before
/// its first layout.
pub(crate) fn under<M>(caret: Option<Rect>, cx: &EventCx<'_, M>) -> Point {
    match caret {
        Some(caret) => Point::new(caret.min_x(), caret.max_y()),
        None => cx.bounds().map_or(Point::ZERO, |b| b.origin),
    }
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
        if state.pastes {
            if let Some(opens) = state.menu.opens(event, cx) {
                self.open_menu(state, opens, cx);
                return Propagation::Stop;
            }
        }
        match event {
            Event::PopupAnswer(answer) if state.pastes => {
                if let Some(command) = text_menu::command(*answer) {
                    self.command(state, command, cx);
                }
                return Propagation::Stop;
            }
            Event::Paste(Pasted(text)) if self.enabled && state.pastes => {
                state.buffer.paste(text);
                self.report(state, cx);
                self.shown(state, cx);
                return Propagation::Stop;
            }
            _ => {}
        }
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

    /// After the menu or a paste changed the text or the selection: the input method learns of it and the field
    /// paints.
    fn shown(&self, state: &mut FieldState, cx: &mut EventCx<'_, M>) {
        if cx.is_focused() {
            state.sync_input(ChangeCause::Other, cx);
        }
        cx.request_paint();
    }

    /// Opens the field's menu: at the pointer, where a press outside the selection first puts the caret (one inside
    /// keeps the selection), or from the keyboard under the caret. The field takes the keyboard as on any press, and
    /// what the input method composes stays as typed. A disabled field is read only: it has no selection, and its
    /// menu copies all its text.
    fn open_menu(&self, state: &mut FieldState, opens: Opens, cx: &mut EventCx<'_, M>) {
        if self.enabled {
            if state.buffer.composition().is_some() {
                state.buffer.commit_composition();
                state.reset_input = true;
                self.report(state, cx);
            }
            cx.request_focus();
        }
        let at = match opens {
            Opens::At(point) => {
                if let Some(at) = state.index_at(point, cx).filter(|_| self.enabled) {
                    let selection = state.buffer.selection();
                    if selection.is_empty() || !(selection.start..=selection.end).contains(&at.byte)
                    {
                        state.buffer.select(at.byte..at.byte);
                    }
                }
                point
            }
            Opens::Keyboard => under(state.caret_rect(cx), cx),
        };
        let selected = !state.buffer.selection().is_empty();
        let holds = !state.buffer.is_empty();
        let can = Can {
            cut: self.enabled && selected,
            copy: selected || (!self.enabled && holds),
            paste: self.enabled && cx.clipboard_has_text(),
            select_all: self.enabled && holds,
        };
        text_menu::open(at, can, cx);
        self.shown(state, cx);
    }

    /// A row of the field's menu was chosen: what the row's keys do.
    fn command(&self, state: &mut FieldState, command: Command, cx: &mut EventCx<'_, M>) {
        match command {
            Command::Cut if self.enabled => {
                cx.set_clipboard(state.buffer.cut());
                self.report(state, cx);
            }
            Command::Copy if self.enabled => {
                cx.set_clipboard(state.buffer.selected_text().to_owned())
            }
            Command::Copy => cx.set_clipboard(state.committed()),
            Command::Paste if self.enabled => cx.request_paste(),
            Command::SelectAll if self.enabled => state.buffer.select_all(),
            _ => {}
        }
        self.shown(state, cx);
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
            Response::Paste if state.pastes => cx.request_paste(),
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
            menu: text_menu::Opener::default(),
            pastes: true,
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
        if state.pastes {
            state.menu.arm(cx);
        }
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
    /// What opens the field's menu.
    menu: text_menu::Opener,
    /// Whether the field has a menu and takes a paste: a [`BareSecureField`], which stands on the surfaces that ask
    /// for the session's password, has neither.
    pastes: bool,
}

impl SecureState {
    /// The caret in window coordinates.
    fn caret_rect<M>(&self, cx: &EventCx<'_, M>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        let mask = self.buffer.mask(BULLET);
        let (layout, origin) = placed(cx.env(), &mask, bounds.size, self.scroll.get(), self.look)?;
        let (caret, _) = self.buffer.masked_selection();
        let caret = layout.caret_rect(position(caret * BULLET.len_utf8()));
        Some(caret.translate(bounds.min_x() + origin.x, bounds.min_y() + origin.y))
    }

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

impl<M: Clone + 'static> SecureField<M> {
    /// Opens the field's menu, as a text field's opens. A password never leaves its field, so Cut and Copy are always
    /// disabled; a disabled field's menu has nothing to do.
    fn open_menu(&self, state: &mut SecureState, opens: Opens, cx: &mut EventCx<'_, M>) {
        if self.enabled {
            cx.request_focus();
        }
        let at = match opens {
            Opens::At(point) => {
                if let Some(at) = state.mask_index(point, cx).filter(|_| self.enabled) {
                    let (_, selection) = state.buffer.masked_selection();
                    if selection.is_empty() || !(selection.start..=selection.end).contains(&at) {
                        state.buffer.select_masked(at, at);
                    }
                }
                point
            }
            Opens::Keyboard => under(state.caret_rect(cx), cx),
        };
        let can = Can {
            cut: false,
            copy: false,
            paste: self.enabled && cx.clipboard_has_text(),
            select_all: self.enabled && !state.buffer.is_empty(),
        };
        text_menu::open(at, can, cx);
        cx.request_paint();
    }

    fn handle(
        &self,
        state: &mut SecureState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if state.pastes {
            if let Some(opens) = state.menu.opens(event, cx) {
                self.open_menu(state, opens, cx);
                return Propagation::Stop;
            }
        }
        match event {
            Event::PopupAnswer(answer) if self.enabled && state.pastes => {
                match text_menu::command(*answer) {
                    Some(Command::Paste) => cx.request_paste(),
                    Some(Command::SelectAll) => state.buffer.select_all(),
                    _ => {}
                }
                cx.request_paint();
                return Propagation::Stop;
            }
            Event::Paste(Pasted(text)) if self.enabled && state.pastes => {
                if insert(&mut state.buffer, text) {
                    self.changed(&state.buffer, cx);
                }
                cx.request_paint();
                return Propagation::Stop;
            }
            _ => {}
        }
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
        use cupertino_ui_core::platform::Mods;
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
            _ if command && key.text == Some('v') && state.pastes => {
                cx.request_paste();
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

impl<M: Clone + 'static> Widget<M> for SecureField<M> {
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
            menu: text_menu::Opener::default(),
            pastes: true,
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
        if state.pastes {
            state.menu.arm(cx);
        }
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
impl<M: Clone + 'static> Widget<M> for BareSecureField<M> {
    type State = SecureState;

    fn layout(&self) -> LayoutSpec {
        layout()
    }

    fn state(&self) -> SecureState {
        SecureState {
            look: bare_secure_look(self),
            placeholder_color: self.placeholder_color,
            pastes: false,
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
        let look = bare_secure_look(self);
        if look != state.look {
            state.look = look;
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
            if self.field.enabled
                && cx.phase() != cupertino_ui_core::Phase::Capture
                && self.in_submit_strip(*position, cx)
            {
                if let Some(on_submit) = &self.field.on_submit {
                    cx.emit(on_submit(&state.buffer));
                }
                return Propagation::Stop;
            }
        }
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

impl<M> BareSecureField<M> {
    /// Whether `point` (window coordinates) lies in the strip at the field's trailing end where a press submits.
    fn in_submit_strip(&self, point: Point, cx: &EventCx<'_, M>) -> bool {
        let width = self.submit_width;
        let Some(bounds) = cx.bounds().filter(|_| width.is_finite() && width > 0.0) else {
            return false;
        };
        let x = point.x - bounds.min_x();
        let inside = (0.0..=bounds.width()).contains(&x)
            && (bounds.min_y()..=bounds.max_y()).contains(&point.y);
        inside
            && match cx.env().direction {
                LayoutDirection::Ltr => x >= bounds.width() - width,
                LayoutDirection::Rtl => x <= width,
            }
    }
}

/// The look of a bare secure field: its text `size` points large (whole points from 1 to 200) in `weight`, set as
/// `centered` and `submit_width` say.
fn bare_secure_look<M>(field: &BareSecureField<M>) -> FieldLook {
    let width = field.submit_width;
    let trailing = if width.is_finite() { width } else { 0.0 };
    match bare_look(field.size) {
        FieldLook::Bare(size, ..) => FieldLook::Bare(
            size,
            field.weight,
            BareSetting {
                centered: field.centered,
                trailing: trailing.ceil().clamp(0.0, f64::from(u16::MAX)) as u16,
            },
        ),
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
            pastes: self.menu,
            ..self.field.state()
        }
    }

    fn update(&self, old: &Self, state: &mut FieldState) -> Change {
        state.pastes = self.menu;
        let change = self.field.update(&old.field, state);
        if self.size != old.size {
            state.look = bare_look(self.size);
            return Change::Layout;
        }
        // a new start takes the next frame's time; none takes nothing, and needs no frame
        if self.blink_start.is_none() {
            state.blink.start.set(None);
        }
        state.blink.owned.set(self.blink_start.is_some());
        let started = self.blink_start.is_some() && self.blink_start != old.blink_start;
        if change == Change::None && (started || self.caret_layer != old.caret_layer) {
            return Change::Paint;
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
        // a held caret, or one whose layer shows its blink, draws nothing as it blinks
        if self.hold_caret || self.caret_layer.is_some() {
            if let Event::Wake(_) = event {
                let selecting = !state.buffer.selection().is_empty();
                state.blink.hold(selecting, cx);
                return Propagation::Continue;
            }
        }
        state.blink.owned.set(self.blink_start.is_some());
        // a new start is taken by the focus or the window's activation that shows the caret, or else by the next frame
        if state.blink.started_by_owner(event) {
            state.blink.start_at(self.blink_start, cx.now());
        }
        let since = state.blink.since.get();
        let shows = Blink::restarts(event);
        let propagation = self.field.event(state, event, cx);
        if let Some(on_blink) = &self.on_blink {
            let restarted = state.blink.since.get() != since;
            if restarted || shows {
                cx.emit(on_blink(CaretBlink::of(cx.env(), restarted)));
            }
        }
        propagation
    }

    fn paint(&self, state: &FieldState, cx: &mut PaintCx<'_>) {
        state.blink.start_at(self.blink_start, cx.now());
        state.blink.layered.set(self.caret_layer.is_some());
        self.field.paint(state, cx)
    }

    fn overlay(&self, state: &FieldState, cx: &mut OverlayCx<'_>) {
        state.blink.holding.set(self.hold_caret);
        state.blink.layered.set(self.caret_layer.is_some());
        self.field.overlay(state, cx)
    }

    fn semantics(&self, state: &FieldState, cx: &mut SemanticsCx<'_>) {
        self.field.semantics(state, cx)
    }

    fn overlay_layer(&self, _: &FieldState) -> Option<SurfaceLayerMark> {
        self.caret_layer.map(|key| SurfaceLayerMark {
            key,
            look: cupertino_ui_core::platform::SurfaceLayerLook::default(),
        })
    }
}

/// The look of a bare field of text `size` points large, in whole points from 1 to 200.
fn bare_look(size: f64) -> FieldLook {
    let size = if size.is_finite() { size } else { 0.0 };
    FieldLook::Bare(
        size.round().clamp(1.0, 200.0) as u16,
        None,
        BareSetting::default(),
    )
}
