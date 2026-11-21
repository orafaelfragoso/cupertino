use super::*;

impl Cupertino {
    /// Where an absolute pointing device reports: the output under its pointer, and that output's mode turned as it
    /// is shown, in panel pixels. The device's range spans those pixels, so a position maps through the output's exact
    /// scale ([`Cupertino::output_pixels_to_global`]) and lands where drawing puts that pixel, up to the far edge.
    pub(super) fn absolute_device_space(
        &self,
    ) -> Option<(cupertino_wm::OutputId, smithay::utils::Size<i32, Logical>)> {
        let id = self.pointer_output()?;
        let geometry = self.output_geometry(id)?;
        let turned = if geometry.rotated {
            geometry.mode.swapped()
        } else {
            geometry.mode
        };
        // the size type of `position_transformed`; its values here are panel pixels
        let pixels = smithay::utils::Size::<i32, Logical>::from((
            i32::try_from(turned.width).ok()?,
            i32::try_from(turned.height).ok()?,
        ));
        Some((id, pixels))
    }

    /// A mouse is listed in `Mice` from its first motion, scroll or click on.
    fn pointer_device_used(&mut self, id: &str) {
        if self
            .pointer_devices
            .get_mut(id)
            .is_some_and(|device| !device.touchpad && device.note_use())
        {
            self.publish_session_settings();
            self.sync_scroll_bars();
        }
    }

    pub(super) fn on_pointer_motion<B: InputBackend>(&mut self, evt: B::PointerMotionEvent) {
        self.pointer_device_used(&evt.device().id());
        // Devices announced by the backend get the design curves; others keep the backend's delta.
        let delta = match self.pointer_devices.get_mut(&evt.device().id()) {
            Some(dev) => dev.accelerate(evt.delta_unaccel(), evt.time()),
            None => evt.delta(),
        };
        let pos = self.pointer.current_location() + delta;
        let rel = RelativeMotionEvent {
            delta,
            delta_unaccel: evt.delta_unaccel(),
            utime: evt.time(),
        };
        self.pointer_moved_to(pos, evt.time_msec(), Some(rel));
    }

    /// Feed a window move's motion to the top zone: true once the pointer, at its output's top edge, pushed up past it
    /// far enough to open the overview.
    fn window_drag_reached_top(
        &mut self,
        pos: Point<f64, Logical>,
        rel: Option<&RelativeMotionEvent>,
    ) -> bool {
        if !self.wm.is_moving() {
            return false;
        }
        let (Some(rel), Some(top)) = (rel, self.output_at(pos).map(|o| o.rect.y)) else {
            return false;
        };
        self.top_zone.motion(pos.y - top, rel.delta.x, rel.delta.y)
    }

    /// Clamp `pos` to the union of the outputs.
    fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if self.outputs.is_empty() {
            return pos;
        }
        let (x0, y0) = (
            self.outputs
                .iter()
                .map(|o| o.rect.x)
                .fold(f64::MAX, f64::min),
            self.outputs
                .iter()
                .map(|o| o.rect.y)
                .fold(f64::MAX, f64::min),
        );
        let (x1, y1) = (
            self.outputs
                .iter()
                .map(|o| o.rect.right())
                .fold(f64::MIN, f64::max),
            self.outputs
                .iter()
                .map(|o| o.rect.bottom())
                .fold(f64::MIN, f64::max),
        );
        Point::from((pos.x.clamp(x0, x1 - 1.0), pos.y.clamp(y0, y1 - 1.0)))
    }

    pub fn pointer_moved_to(
        &mut self,
        pos: Point<f64, Logical>,
        time: u32,
        rel: Option<RelativeMotionEvent>,
    ) {
        crate::frame_stats::note_pointer_motion(self);
        self.input_came();
        let pos = match self.constrain_motion(pos) {
            Some(Constrained::Locked) => {
                // the pointer stays; the client gets the motion as relative motion only
                if let Some(rel) = rel {
                    let pointer = self.pointer.clone();
                    let under = self.surface_under(pointer.current_location());
                    pointer.relative_motion(self, under, &rel);
                    pointer.frame(self);
                }
                return;
            }
            Some(Constrained::Confined(to)) => to,
            None => pos,
        };
        let pos = self.clamp_to_outputs(pos);
        self.shake_motion(pos);
        let now = self.now();
        let mp = MPoint::new(pos.x, pos.y);
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.pointer.clone();

        if self.session_lock.is_active() {
            let under = self.surface_under(pos);
            self.send_pointer_motion(under, pos, time, rel);
            return;
        }
        if self.pointer_mode == PointerMode::Normal {
            self.track_active_menu_bar(pos, now);
        }
        self.update_input_pointer(pos, now);
        if let Some(drag) = self.pending_drag.take_if(|d| {
            cupertino_input::drag_started(
                cupertino_input::Point::new(d.press.x, d.press.y),
                cupertino_input::Point::new(mp.x, mp.y),
            )
        }) {
            // anchored at the press, so the window catches up with the pointer at once
            self.begin_model_interaction(drag.command);
        }

        if self.pointer_mode == PointerMode::Model {
            let mods = self.current_mods();
            let m = WmModifiers {
                primary: mods.logo,
                option: mods.alt,
                shift: mods.shift,
                ctrl: mods.ctrl,
            };
            self.command(Command::PointerMotion {
                pointer: mp,
                modifiers: m,
            });
            if self.window_drag_reached_top(pos, rel.as_ref()) {
                // the window stays where it was taken and the overview component of its output opens
                self.command(Command::PointerRelease);
                if let Some(output) = self.output_at(pos).map(|o| o.id) {
                    self.ask_overview(
                        output,
                        cupertino_protocols::server::overview::cupertino_overview_v1::Mode::All,
                        "",
                    );
                }
            }
            if !self.wm.is_interacting() {
                self.pointer_mode = PointerMode::Normal;
                self.top_zone.reset();
            }
            pointer.motion(
                self,
                None,
                &MotionEvent {
                    location: pos,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
            self.request_redraw();
            return;
        }

        let target = self.window_target_on_top(pos);
        self.update_decor_hover(target.as_ref());
        let under = self.surface_under(pos);
        self.send_pointer_motion(under, pos, time, rel);
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_pointer_constraints(focus.as_ref());
    }

    /// Move the pointer to `pos`, focused on `under`.
    pub(crate) fn send_pointer_motion(
        &mut self,
        under: Option<(WlSurface, Point<f64, Logical>)>,
        pos: Point<f64, Logical>,
        time: u32,
        rel: Option<RelativeMotionEvent>,
    ) {
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            under.clone(),
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        if let Some(rel) = rel {
            pointer.relative_motion(self, under, &rel);
        }
        pointer.frame(self);
        self.request_redraw();
    }

    pub(super) fn on_pointer_button<B: InputBackend>(&mut self, evt: B::PointerButtonEvent) {
        self.pointer_device_used(&evt.device().id());
        self.pointer_button(evt.button_code(), evt.state(), evt.time_msec());
    }

    /// A pointer button event (also used by scripted input).
    ///
    /// A press the server consumes (decorations, a model move or resize) never reaches a client, so its release
    /// is consumed too; every other press and release goes to `pointer.button`, whose implicit grab keeps a client's
    /// release with the client that got the press.
    pub fn pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        crate::frame_stats::note_input(self);
        self.input_came();
        if !self.control_physical_transition(
            cupertino_control_model::Input::Button(button),
            state == ButtonState::Pressed,
        ) {
            return;
        }
        self.as_input(|s| s.handle_pointer_button(button, state, time));
        // a release that ended a drag without a drop sends its icon back
        self.drag_ended();
    }

    fn handle_pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        self.key_filters.super_tap.disturb();
        let serial = SERIAL_COUNTER.next_serial();
        let pos = self.pointer.current_location();
        let pointer = self.pointer.clone();
        self.control_user_input(serial.into(), pointer.current_focus());
        let now = self.now();
        if state == ButtonState::Pressed {
            self.last_user_event = Some(now);
            let focus = pointer.current_focus();
            self.note_input(focus.as_ref());
        }
        let release_consumed =
            state == ButtonState::Released && self.consumed_buttons.contains(&button);
        if release_consumed {
            self.consumed_buttons.retain(|b| *b != button);
        }
        if state == ButtonState::Released
            && self
                .pending_drag
                .as_ref()
                .is_some_and(|d| d.button == button)
        {
            // released inside the drag-start box: the press was a click
            self.pending_drag = None;
        }

        if self.session_lock.is_active() {
            if release_consumed {
                return;
            }
            if state == ButtonState::Pressed {
                self.focus_lock_surface_at(pos);
            }
            pointer.button(
                self,
                &ButtonEvent {
                    button,
                    state,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
            return;
        }
        if release_consumed {
            if self.pointer_mode == PointerMode::Model {
                self.end_model_interaction(pos, time);
            } else if self.decor.pressed.is_some() {
                self.decor_button_release(pos);
            } else if self.wm.switcher().is_some() {
                // a press off the switcher's panel: its release cancels the switch
                self.command(Command::SwitcherCancel);
            }
            return;
        }
        if state == ButtonState::Released && self.pointer_mode == PointerMode::Model {
            // a move or resize the client asked for after it got the press: the client still gets the release
            self.end_model_interaction(pos, time);
        }
        if state == ButtonState::Pressed && self.release_engaged_layer(pos, serial, time) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed && self.dismiss_popup_grab_for_press(pos, serial, time) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed {
            // a press off the picker's panel closes the picker and goes on
            self.accent_pointer_press(pos);
        }
        if state == ButtonState::Pressed && self.press_consumed(pos, button, now) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed {
            self.arm_by_press();
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }

    /// Hand the pointer back to the clients after a model move or resize.
    fn end_model_interaction(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.command(Command::PointerRelease);
        self.pointer_mode = PointerMode::Normal;
        self.pointer_moved_to(pos, time, None);
    }

    /// Run the server's own handling of a press at `pos`. True when the press was consumed and must not reach a client.
    fn press_consumed(&mut self, pos: Point<f64, Logical>, button: u32, now: f64) -> bool {
        // the dock's panel takes the pointer while a switch is open; anything else is the server's
        // and cancels the switch on release
        if self.wm.switcher().is_some() {
            return !self
                .surface_under(pos)
                .is_some_and(|(surface, _)| self.is_switcher_panel(&surface));
        }
        if self.decoration_press(pos, button, now) {
            return true;
        }
        // the content of a window a modal sheet or dialog blocks takes no clicks: the press goes
        // to the model, which brings the blocking window forward (the title bar still moves it)
        if let Some(id) = self
            .window_surface_on_top(pos)
            .filter(|id| self.wm.is_blocked(*id) && !self.current_mods().logo)
        {
            self.command(Command::FocusWindow { window: id });
            return true;
        }
        self.focus_at(pos);
        self.pending_drag
            .as_ref()
            .is_some_and(|d| d.button == button)
    }

    /// Arm `command` (a move or resize anchored at `press`) to start when the pointer leaves the
    /// drag-start box.
    fn defer_drag(&mut self, button: u32, press: MPoint, command: Command) {
        self.pending_drag = Some(PendingDrag {
            button,
            press,
            command,
        });
    }

    /// Hover state of the decorations under the pointer: button group glyphs, resize cursors.
    fn update_decor_hover(&mut self, target: Option<&WindowTarget>) {
        use cupertino_decorations::Hit;
        let (group, cursor) = match target {
            Some(WindowTarget::Decoration {
                id,
                style,
                hit,
                content,
                local,
            }) => {
                // a client-decorated window has no button group of ours, but in full screen
                let server_side = self
                    .entry(*id)
                    .is_some_and(|e| e.server_side || e.fullscreen);
                let group = server_side
                    && (matches!(hit, Hit::Button(_))
                        || self.decor.group_hovered(*style, *content, *local));
                let cursor = match hit {
                    Hit::Resize(edge) => self.resize_cursor(*id, *edge),
                    _ => None,
                };
                (group.then_some(*id), cursor)
            }
            _ => (None, None),
        };
        if self.decor.hover_group != group || self.decor.cursor != cursor {
            self.decor.hover_group = group;
            self.decor.cursor = cursor;
            self.request_redraw();
        }
    }

    /// A press on the decorations of a window: buttons arm, the title bar moves (double click:
    /// the configured action), the resize zones resize; moves and resizes start past the drag
    /// threshold ([`PendingDrag`]). True when the press was consumed.
    fn decoration_press(&mut self, pos: Point<f64, Logical>, button: u32, now: f64) -> bool {
        use cupertino_decorations::{DoubleClickAction, Hit};
        let Some(WindowTarget::Decoration { id, hit, .. }) = self.window_target_on_top(pos) else {
            return false;
        };
        if button != BTN_LEFT {
            return true;
        }
        let mp = MPoint::new(pos.x, pos.y);
        self.command(Command::FocusWindow { window: id });
        self.return_keyboard_to_key_window(id);
        match hit {
            Hit::Button(kind) if self.decor_button_enabled(id, kind) => {
                self.decor.pressed = Some((id, kind));
                self.request_redraw();
            }
            // a disabled button on the title bar revealed over a full-screen window is as the rest of that bar: nothing
            Hit::Button(_) if self.entry(id).is_some_and(|e| e.fullscreen) => {}
            // a disabled button is no control: the press is the title bar's, which moves the window past the drag
            // threshold, and fires nothing
            Hit::Button(_) | Hit::Titlebar => {
                if self.decor.register_click(id, now) {
                    match self.decor.double_click_action(Hit::Titlebar) {
                        DoubleClickAction::Zoom => self.command(Command::ToggleZoom { window: id }),
                        DoubleClickAction::Minimize => {
                            let target = self.minimize_target(id);
                            self.command(Command::Minimize { window: id, target });
                        }
                        DoubleClickAction::None => {}
                    }
                } else {
                    self.defer_drag(
                        button,
                        mp,
                        Command::BeginMove {
                            window: id,
                            pointer: mp,
                        },
                    );
                }
            }
            Hit::Resize(edge) => {
                let (left, right, top, bottom) = edge.sides();
                let edges = Edges {
                    left,
                    right,
                    top,
                    bottom,
                };
                if self.decor.register_click(id, now) {
                    // double click: the edges snap to the screen, Alt adds the opposite ones
                    self.command(Command::SnapEdgesToScreen {
                        window: id,
                        edges,
                        opposite: self.current_mods().alt,
                    });
                } else {
                    self.defer_drag(
                        button,
                        mp,
                        Command::BeginResize {
                            window: id,
                            edges,
                            pointer: mp,
                        },
                    );
                }
            }
            Hit::Content | Hit::None => {}
        }
        true
    }

    /// Whether window control button `kind` of window `id` takes clicks ([`crate::decor::button_enabled`]).
    pub(crate) fn decor_button_enabled(
        &self,
        id: WindowId,
        kind: cupertino_decorations::ButtonKind,
    ) -> bool {
        // a full-screen window does not minimize: its revealed title bar draws minimize disabled
        let fullscreen = self.entry(id).is_some_and(|e| e.fullscreen);
        self.wm.window(id).is_some_and(|w| {
            crate::decor::button_enabled(w.can_minimize() && !fullscreen, w.can_zoom())
                [kind.index()]
        })
    }

    /// Button released after a press on a window control button: run the action when the pointer
    /// is still over the same button.
    fn decor_button_release(&mut self, pos: Point<f64, Logical>) {
        use cupertino_decorations::Hit;
        let Some((id, kind)) = self.decor.pressed.take() else {
            return;
        };
        self.request_redraw();
        let still = matches!(self.window_target_on_top(pos), Some(WindowTarget::Decoration { id: w, hit: Hit::Button(k), .. }) if w == id && k == kind);
        if still {
            self.press_window_button(id, kind);
        }
    }

    /// Do what window control button `kind` of window `id` is for, as a click on it does: an assistive technology's
    /// press comes here too ([`crate::decor_access`]). Nothing when the button is disabled.
    pub fn press_window_button(&mut self, id: WindowId, kind: cupertino_decorations::ButtonKind) {
        use cupertino_decorations::ButtonKind;
        if !self.decor_button_enabled(id, kind) {
            return;
        }
        match kind {
            ButtonKind::Close => self.command(Command::RequestClose { window: id }),
            ButtonKind::Minimize => {
                let target = self.minimize_target(id);
                self.command(Command::Minimize { window: id, target });
            }
            ButtonKind::Zoom => self.command(Command::ToggleZoom { window: id }),
        }
    }

    /// Gives the keyboard back to `window` when it is still the key window but a layer surface that takes the keyboard
    /// on demand (the desktop) holds it: the window manager sees no change of key window then and moves nothing.
    fn return_keyboard_to_key_window(&mut self, window: cupertino_wm::WindowId) {
        if self.exclusive_layer.is_some() || self.wm.key_window() != Some(window) {
            return;
        }
        let Some(surface) = self.entry(window).map(|e| e.surface().clone()) else {
            return;
        };
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        if focus.as_ref() != Some(&surface) {
            self.set_keyboard_focus(Some(surface));
        }
    }

    /// The window whose own surface takes a press at `pos`: none when a surface drawn above the windows (a layer surface
    /// such as the dock, or a popup of one, such as its menu) lies there, so a press on it never focuses the window
    /// under it nor takes the keyboard from it.
    fn window_surface_on_top(&self, pos: Point<f64, Logical>) -> Option<WindowId> {
        match self.window_target_on_top(pos)? {
            WindowTarget::Surface { id, .. } => Some(id),
            WindowTarget::Decoration { .. } => None,
        }
    }

    /// Click-to-focus at `pos`. A modified click reaches the client unchanged: the design has no
    /// modifier drag that moves or resizes windows, so a primary-modifier click stays the client's.
    fn focus_at(&mut self, pos: Point<f64, Logical>) {
        if let Some(id) = self.window_surface_on_top(pos) {
            self.command(Command::FocusWindow { window: id });
            self.return_keyboard_to_key_window(id);
            return;
        }
        if let Some((surface, _)) = self.surface_under(pos) {
            // layer-shell surface that accepts keyboard input
            let wants = self.outputs.iter().any(|o| {
                let map = smithay::desktop::layer_map_for_output(&o.output);
                map.layer_for_surface(&surface, smithay::desktop::WindowSurfaceType::ALL)
                    .is_some_and(|l| l.can_receive_keyboard_focus())
            });
            if wants {
                self.set_keyboard_focus(Some(surface));
            }
        }
    }

    pub(super) fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        self.pointer_device_used(&evt.device().id());
        let both =
            |read: &dyn Fn(Axis) -> Option<f64>| [read(Axis::Horizontal), read(Axis::Vertical)];
        let input = AxisInput {
            device: evt.device().id(),
            source: evt.source(),
            usec: evt.time(),
            amount: both(&|axis| evt.amount(axis)),
            v120: both(&|axis| evt.amount_v120(axis)),
            relative_direction: [
                evt.relative_direction(Axis::Horizontal),
                evt.relative_direction(Axis::Vertical),
            ],
        };
        self.pointer_axis(&input);
    }

    /// A pointer axis event (also used by scripted input): the scroll goes to the focused client as one
    /// `wl_pointer.frame`, with the scroll count of a touchpad scroll before the frame.
    pub fn pointer_axis(&mut self, input: &AxisInput) {
        crate::frame_stats::note_input(self);
        self.input_came();
        self.key_filters.super_tap.disturb();
        // A wheel notch (120 v120 units) scrolls three lines. Finger scrolling of a
        // device with a scroll resolution (the touchpad's counts) is accelerated; other sources,
        // and finger scrolling of other devices, keep the backend's value.
        let usec = input.usec;
        let mut device = self.pointer_devices.get_mut(&input.device);
        let mut amount = |axis: Axis| match (
            input.source,
            input.v120[axis_index(axis)],
            input.amount[axis_index(axis)],
        ) {
            (AxisSource::Wheel, Some(v120), _) => cupertino_input::wheel_points(v120),
            (AxisSource::Finger, _, Some(counts)) => device
                .as_deref_mut()
                .and_then(|d| d.scroll_points(axis, counts, usec))
                .unwrap_or(counts),
            (_, v120, amount) => {
                amount.unwrap_or_else(|| cupertino_input::wheel_points(v120.unwrap_or(0.0)))
            }
        };
        let (h, v) = (amount(Axis::Horizontal), amount(Axis::Vertical));
        let count = (input.source == AxisSource::Finger)
            .then(|| device.and_then(|d| d.scroll_count((h, v), input.ends_gesture(), usec)))
            .flatten();
        let stops = input.stopped_axes();
        let mut frame = AxisFrame::new((usec / 1000) as u32).source(input.source);
        let mut carries_axis = false;
        for (axis, value) in [(Axis::Horizontal, h), (Axis::Vertical, v)] {
            let i = axis_index(axis);
            if value != 0.0 {
                carries_axis = true;
                let value = self.axis_carry.send(i, input.source, value);
                frame = frame
                    .relative_direction(axis, input.relative_direction[i])
                    .value(axis, value);
                if let Some(d) = input.v120[i] {
                    frame = frame.v120(axis, d as i32);
                }
            }
            if stops[i] {
                carries_axis = true;
                self.axis_carry.stop(i);
                frame = frame.stop(axis);
            }
        }
        // An event the scroll coalescing holds back has nothing to tell the client yet.
        if !carries_axis {
            return;
        }
        let pointer = self.pointer.clone();
        pointer.axis(self, frame);
        // The count goes to the `cupertino_scroll_v1` objects of the focused client, for each axis that moved.
        if let (Some(count), Some(client)) =
            (count, pointer.current_focus().and_then(|s| s.client()))
        {
            for (value, wl_axis) in [
                (h, wl_pointer::Axis::HorizontalScroll),
                (v, wl_pointer::Axis::VerticalScroll),
            ] {
                if value != 0.0 {
                    self.scroll_counts.send(&client.id(), wl_axis, count);
                }
            }
        }
        pointer.frame(self);
    }

    // ---- gestures ----------------------------------------------------------------------------

    fn pointer_output(&self) -> Option<cupertino_wm::OutputId> {
        self.output_at(self.pointer.current_location())
            .map(|o| o.id)
    }

    /// Feed the recognizer and act on its outputs. Events stay with the focused client until the
    /// recognizer claims the gesture; then the client's gesture is cancelled.
    pub fn gesture(&mut self, ev: GestureEvent) {
        if self.session_lock.is_active() {
            return;
        }
        for out in self.gestures.handle(ev) {
            self.gesture_output_event(out);
        }
    }

    fn gesture_output_event(&mut self, out: GestureOutput) {
        use cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture as TxGesture;
        let map_gesture = |kind: GestureKind| match kind {
            GestureKind::Overview => Some(TxGesture::SwipeUp),
            GestureKind::AppOverview => Some(TxGesture::SwipeDown),
            GestureKind::AppGrid => Some(TxGesture::Pinch),
            GestureKind::ShowDesktop => Some(TxGesture::Spread),
            GestureKind::WorkspaceSwitch => None,
        };
        match out {
            GestureOutput::Begin { kind, .. } => {
                self.cancel_forwarded_gesture();
                if let (Some(output), Some(wire)) = (self.pointer_output(), map_gesture(kind)) {
                    if self.transaction_gesture_begin(output, wire) {
                        self.gesture_output = Some(output);
                        return;
                    }
                }
                if kind == GestureKind::WorkspaceSwitch {
                    self.gesture_output = self.pointer_output();
                    if let Some(output) = self.gesture_output {
                        self.command(Command::WorkspaceGestureBegin { output });
                    }
                }
            }
            GestureOutput::Update { kind, progress, .. } => {
                if let Some(output) = self.gesture_output {
                    if self.transaction_gesture_update(output, progress.abs()) {
                        return;
                    }
                }
                if kind == GestureKind::WorkspaceSwitch {
                    if let Some(output) = self.gesture_output {
                        // progress is positive towards the next workspace; the model's fraction is the
                        // displacement of the content (positive = to the right)
                        self.command(Command::WorkspaceGestureUpdate {
                            output,
                            fraction: -progress,
                        });
                    }
                }
            }
            GestureOutput::End {
                kind,
                outcome,
                velocity,
                ..
            } => {
                let commit = outcome == GestureOutcome::Commit;
                if let Some(output) = self.gesture_output {
                    if self.transaction_gesture_end(output, commit, velocity) {
                        self.gesture_output = None;
                        return;
                    }
                }
                match kind {
                    GestureKind::WorkspaceSwitch => {
                        if let Some(output) = self.gesture_output.take() {
                            self.command(Command::WorkspaceGestureEnd { output });
                        }
                    }
                    // the dock of the output maps a gesture no transaction is bound to to its action
                    GestureKind::Overview
                    | GestureKind::AppOverview
                    | GestureKind::AppGrid
                    | GestureKind::ShowDesktop
                        if commit =>
                    {
                        let gesture = match kind {
                            GestureKind::Overview => DockGesture::SwipeUp,
                            GestureKind::AppOverview => DockGesture::SwipeDown,
                            GestureKind::AppGrid => DockGesture::Pinch,
                            _ => DockGesture::Spread,
                        };
                        if let Some(output) = self.pointer_output() {
                            self.docks.gesture(output, gesture);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Terminate a gesture the client already saw once the compositor takes it over.
    fn cancel_forwarded_gesture(&mut self) {
        if !std::mem::take(&mut self.gesture_forwarded) {
            return;
        }
        let pointer = self.pointer.clone();
        let time = (self.now() * 1000.0) as u32;
        let serial = SERIAL_COUNTER.next_serial();
        match self.gesture_source {
            GestureSource::Swipe => pointer.gesture_swipe_end(
                self,
                &GestureSwipeEndEvent {
                    serial,
                    time,
                    cancelled: true,
                },
            ),
            GestureSource::Pinch => pointer.gesture_pinch_end(
                self,
                &GesturePinchEndEvent {
                    serial,
                    time,
                    cancelled: true,
                },
            ),
        }
    }

    fn claimed(&self) -> bool {
        self.gestures.disposition() == Disposition::Claimed
    }

    pub(super) fn on_swipe_begin<B: InputBackend>(&mut self, evt: B::GestureSwipeBeginEvent) {
        let (time, fingers) = (evt.time_msec(), evt.fingers());
        self.gesture(GestureEvent::SwipeBegin {
            time: f64::from(time) / 1000.0,
            fingers,
        });
        if !self.claimed() {
            self.gesture_forwarded = true;
            self.gesture_source = GestureSource::Swipe;
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_begin(
                self,
                &GestureSwipeBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    fingers,
                },
            );
        }
    }

    pub(super) fn on_swipe_update<B: InputBackend>(&mut self, evt: B::GestureSwipeUpdateEvent) {
        let (time, d) = (evt.time_msec(), evt.delta());
        self.gesture(GestureEvent::SwipeUpdate {
            time: f64::from(time) / 1000.0,
            dx: d.x,
            dy: d.y,
        });
        if self.gesture_forwarded && !self.claimed() {
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_update(self, &SwipeUpdate { time, delta: d });
        }
    }

    pub(super) fn on_swipe_end<B: InputBackend>(&mut self, evt: B::GestureSwipeEndEvent) {
        let (time, cancelled) = (evt.time_msec(), evt.cancelled());
        let forwarded = self.gesture_forwarded && !self.claimed();
        self.gesture(GestureEvent::SwipeEnd {
            time: f64::from(time) / 1000.0,
            cancelled,
        });
        if forwarded {
            self.gesture_forwarded = false;
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_end(
                self,
                &GestureSwipeEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    cancelled,
                },
            );
        }
        self.gesture_forwarded = false;
    }

    pub(super) fn on_pinch_begin<B: InputBackend>(&mut self, evt: B::GesturePinchBeginEvent) {
        let (time, fingers) = (evt.time_msec(), evt.fingers());
        self.gesture(GestureEvent::PinchBegin {
            time: f64::from(time) / 1000.0,
            fingers,
        });
        if !self.claimed() {
            self.gesture_forwarded = true;
            self.gesture_source = GestureSource::Pinch;
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_begin(
                self,
                &GesturePinchBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    fingers,
                },
            );
        }
    }

    pub(super) fn on_pinch_update<B: InputBackend>(&mut self, evt: B::GesturePinchUpdateEvent) {
        let (time, d, scale, rotation) =
            (evt.time_msec(), evt.delta(), evt.scale(), evt.rotation());
        self.gesture(GestureEvent::PinchUpdate {
            time: f64::from(time) / 1000.0,
            dx: d.x,
            dy: d.y,
            scale,
            rotation,
        });
        if self.gesture_forwarded && !self.claimed() {
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_update(
                self,
                &PinchUpdate {
                    time,
                    delta: d,
                    scale,
                    rotation,
                },
            );
        }
    }

    pub(super) fn on_pinch_end<B: InputBackend>(&mut self, evt: B::GesturePinchEndEvent) {
        let (time, cancelled) = (evt.time_msec(), evt.cancelled());
        let forwarded = self.gesture_forwarded && !self.claimed();
        self.gesture(GestureEvent::PinchEnd {
            time: f64::from(time) / 1000.0,
            cancelled,
        });
        if forwarded {
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_end(
                self,
                &GesturePinchEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    cancelled,
                },
            );
        }
        self.gesture_forwarded = false;
    }
}
