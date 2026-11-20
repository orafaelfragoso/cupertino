use super::*;

impl Dispatch<WlShm, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: WEnum::Value(format),
        } = event
        {
            state.shm_formats.push(format);
        }
    }
}
delegate_noop!(State: XdgPositioner);
delegate_noop!(State: WpFractionalScaleManagerV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: WpSinglePixelBufferManagerV1);
delegate_noop!(State: ZxdgDecorationManagerV1);
delegate_noop!(State: ZxdgImporterV2);
delegate_noop!(State: XdgWmDialogV1);
delegate_noop!(State: XdgDialogV1);
delegate_noop!(State: XdgActivationV1);
delegate_noop!(State: WpCursorShapeManagerV1);
delegate_noop!(State: WpCursorShapeDeviceV1);
// the themed cursor's surface and buffers
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ZxdgOutputManagerV1);

impl Dispatch<ZxdgOutputV1, OutputId> for State {
    fn event(
        state: &mut Self,
        xdg: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        id: &OutputId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.info.current.id == *id) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.info.set_position(x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.info.set_size(width, height)
            }
            zxdg_output_v1::Event::Done if xdg.version() < 3 && output.info.done() => {
                state.events.push(Event::OutputsChanged)
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                state.bind_output(registry, qh, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.registry_name == name) {
                    let output = state.outputs.remove(i);
                    if let Some(xdg) = &output.xdg {
                        xdg.destroy();
                    }
                    if output.wl.version() >= 3 {
                        output.wl.release();
                    }
                    for s in &mut state.surfaces {
                        s.entered.retain(|id| *id != OutputId(name));
                    }
                    state.events.push(Event::OutputsChanged);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, OutputId> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        id: &OutputId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.info.current.id == *id) else {
            return;
        };
        let pending = output.info.pending();
        match event {
            wl_output::Event::Scale { factor } => match Scale::from_integer(factor) {
                Some(scale) => pending.scale = scale,
                None => state.fail(Error::InvalidScale),
            },
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                refresh,
            } if flags.contains(wl_output::Mode::Current) => {
                pending.mode = u32::try_from(width).ok().zip(u32::try_from(height).ok());
                pending.refresh_mhz = u32::try_from(refresh).ok().filter(|r| *r > 0);
            }
            wl_output::Event::Geometry {
                physical_width,
                physical_height,
                make,
                model,
                transform,
                ..
            } => {
                pending.make = identification(make);
                pending.model = identification(model);
                pending.physical_size_mm = physical_size(physical_width, physical_height);
                pending.transform = match transform {
                    WEnum::Value(wl_output::Transform::_90) => OutputTransform::Rotated90,
                    WEnum::Value(wl_output::Transform::_180) => OutputTransform::Rotated180,
                    WEnum::Value(wl_output::Transform::_270) => OutputTransform::Rotated270,
                    WEnum::Value(wl_output::Transform::Flipped) => OutputTransform::Flipped,
                    WEnum::Value(wl_output::Transform::Flipped90) => OutputTransform::Flipped90,
                    WEnum::Value(wl_output::Transform::Flipped180) => OutputTransform::Flipped180,
                    WEnum::Value(wl_output::Transform::Flipped270) => OutputTransform::Flipped270,
                    _ => OutputTransform::Normal,
                };
            }
            wl_output::Event::Name { name } => pending.name = Some(name),
            wl_output::Event::Description { description } => {
                pending.description = Some(description)
            }
            wl_output::Event::Done if output.info.done() => {
                state.events.push(Event::OutputsChanged)
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let output_id = |o: &WlOutput| o.data::<OutputId>().copied();
        let Ok(surf) = state.surface_mut(*id) else {
            return;
        };
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some(o) = output_id(&output).filter(|o| !surf.entered.contains(o)) {
                    surf.entered.push(o);
                }
            }
            wl_surface::Event::Leave { output } => {
                if let Some(o) = output_id(&output) {
                    surf.entered.retain(|e| *e != o);
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => match Scale::from_integer(factor)
            {
                Some(scale) => surf.preferred_scale = Some(scale),
                None => state.fail(Error::InvalidScale),
            },
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        match (Scale::from_120ths(scale), state.surface_mut(*id)) {
            (Some(scale), Ok(surf)) => surf.fractional_scale = Some(scale),
            (None, _) => state.fail(Error::InvalidScale),
            (_, Err(_)) => {}
        }
    }
}

/// Set when the compositor answered the `wl_display.sync` of [`Connection::roundtrip`].
#[derive(Clone, Default)]
pub(super) struct Synced(pub(super) Arc<AtomicBool>);

impl Dispatch<WlCallback, Synced> for State {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        done: &Synced,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            done.0.store(true, Ordering::SeqCst);
        }
    }
}

impl Dispatch<WlCallback, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wl_callback::Event::Done { callback_data } = event else {
            return;
        };
        if state.surface(*id).is_ok() {
            state.events.push(Event::Frame(FrameCallback {
                surface: *id,
                time: callback_data,
            }));
        }
    }
}

impl Dispatch<WlBuffer, SlotTag> for State {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        tag: &SlotTag,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let wl_buffer::Event::Release = event else {
            return;
        };
        if let Some(slot) = state
            .surface_mut(SurfaceId(tag.surface))
            .ok()
            .and_then(|surf| surf.slots.iter_mut().find(|s| s.tag == *tag))
        {
            slot.busy = false;
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        let Ok(surf) = state.surface_mut(*id) else {
            return;
        };
        let received = commit::lock(&surf.shared).serials.receive(serial);
        if let Err(e) = received {
            return state.fail(e);
        }
        let event = match &mut surf.role {
            Role::Toplevel {
                pending,
                decorations,
                current,
                ..
            } => {
                let configure = WindowConfigure {
                    serial,
                    size: pending.size,
                    bounds: pending.bounds,
                    states: pending.states,
                    decorations: *decorations,
                    capabilities: pending.capabilities,
                };
                *current = Some(configure);
                Event::Configure {
                    window: Toplevel(*id),
                    configure,
                }
            }
            Role::Popup {
                pending, current, ..
            } => {
                let Some((x, y, width, height)) = *pending else {
                    return state.fail(Error::Protocol("popup configured without a placement"));
                };
                let configure = PopupConfigure {
                    serial,
                    x,
                    y,
                    width,
                    height,
                };
                *current = Some(configure);
                Event::PopupConfigure {
                    popup: Popup(*id),
                    configure,
                }
            }
            Role::External { .. } | Role::Subsurface { .. } => {
                return state.fail(Error::Protocol(
                    "an xdg_surface configure for a surface with an external role",
                ))
            }
        };
        state.events.push(event);
    }
}

impl Dispatch<XdgToplevel, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Ok(Surface {
            role: Role::Toplevel { pending, .. },
            ..
        }) = state.surface_mut(*id)
        else {
            return;
        };
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => match wire_size(width, height) {
                Ok(size) => {
                    pending.size = size;
                    pending.states = WindowStates::from_wire(&states);
                }
                Err(e) => state.fail(e),
            },
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                match wire_size(width, height) {
                    Ok(bounds) => pending.bounds = bounds,
                    Err(e) => state.fail(e),
                }
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                pending.capabilities = Some(WmCapabilities::from_wire(&capabilities));
            }
            xdg_toplevel::Event::Close => state.events.push(Event::Close(Toplevel(*id))),
            _ => {}
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event else {
            return;
        };
        if let Ok(Surface {
            role: Role::Toplevel { decorations, .. },
            ..
        }) = state.surface_mut(*id)
        {
            *decorations = match mode {
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => Decorations::Server,
                _ => Decorations::Client,
            };
        }
    }
}

/// The compositor ended an import: the handle is unknown or no longer exported, or the exported window is gone. The
/// window stays, without the parent.
impl Dispatch<ZxdgImportedV2, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_imported_v2::Event::Destroyed = event else {
            return;
        };
        if let Ok(Surface {
            role: Role::Toplevel { imported, .. },
            ..
        }) = state.surface_mut(*id)
        {
            if let Some(i) = imported.take() {
                i.destroy();
            }
        }
    }
}

impl Dispatch<XdgPopup, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgPopup,
        event: xdg_popup::Event,
        id: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                let size = match wire_size(width, height) {
                    Ok(Some(size)) => size,
                    Ok(None) => {
                        return state.fail(Error::Protocol("popup configured with an empty size"))
                    }
                    Err(e) => return state.fail(e),
                };
                if let Ok(Surface {
                    role: Role::Popup { pending, .. },
                    ..
                }) = state.surface_mut(*id)
                {
                    *pending = Some((x, y, size.0, size.1));
                }
            }
            xdg_popup::Event::PopupDone => state.events.push(Event::PopupDone(Popup(*id))),
            xdg_popup::Event::Repositioned { token } => {
                state.events.push(Event::PopupRepositioned {
                    popup: Popup(*id),
                    token,
                })
            }
            _ => {}
        }
    }
}

/// What an activation token is for.
pub(super) enum TokenUse {
    /// Activate this surface with it.
    Activate(SurfaceId),
    /// Hand it to the application.
    Hand(TokenRequest),
}

impl Dispatch<XdgActivationTokenV1, TokenUse> for State {
    fn event(
        state: &mut Self,
        token: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        used: &TokenUse,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_activation_token_v1::Event::Done { token: value } = event else {
            return;
        };
        match used {
            TokenUse::Activate(id) => {
                if let (Some(activation), Ok(surf)) = (&state.activation, state.surface(*id)) {
                    activation.activate(value, &surf.wl);
                }
            }
            TokenUse::Hand(request) => state.events.push(Event::ActivationToken {
                request: *request,
                token: value,
            }),
        }
        token.destroy();
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        let has_pointer = caps.contains(wl_seat::Capability::Pointer);
        if has_pointer && state.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            state.cursor_device = state
                .cursor_shape
                .as_ref()
                .map(|m| m.get_pointer(&pointer, qh, ()));
            state.pointer = Some(pointer);
        } else if let Some(pointer) = state.pointer.take_if(|_| !has_pointer) {
            state.themed_cursor.stop();
            state.held_buttons.clear();
            state.press_serial = None;
            if let Some(device) = state.cursor_device.take() {
                device.destroy();
            }
            if pointer.version() >= 3 {
                pointer.release();
            }
            if let Some((surface, event)) = state.pointer_tracker.leave() {
                let time = state.input_time(Instant::now());
                state.events.push(Event::Pointer {
                    surface,
                    time,
                    event,
                });
            }
        }
        let has_touch = caps.contains(wl_seat::Capability::Touch);
        if has_touch && state.touch.is_none() {
            state.touch = Some(seat.get_touch(qh, ()));
        } else if let Some(touch) = state.touch.take_if(|_| !has_touch) {
            if touch.version() >= 3 {
                touch.release();
            }
            state.cancel_touches();
        }
        let has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
        if has_keyboard && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        } else if let Some(keyboard) = state.keyboard.take_if(|_| !has_keyboard) {
            if keyboard.version() >= 3 {
                keyboard.release();
            }
            state.repeat.clear();
            if let Some(surface) = state.key_focus.take() {
                let time = state.input_time(Instant::now());
                state.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Leave,
                });
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        // a frame's scroll takes the time of its last axis event
        let own_time = match &event {
            wl_pointer::Event::Motion { time, .. }
            | wl_pointer::Event::Button { time, .. }
            | wl_pointer::Event::Axis { time, .. } => Some(*time),
            wl_pointer::Event::Frame => state.input_clock.latest(),
            _ => None,
        };
        let mut released_all = false;
        let found = match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.pointer_enter_serial = Some(serial);
                state.surface_of(&surface).map(|s| {
                    state
                        .pointer_tracker
                        .enter(s, Point::new(surface_x, surface_y))
                })
            }
            wl_pointer::Event::Leave { .. } => {
                state.themed_cursor.stop();
                state.held_buttons.clear();
                state.press_serial = None;
                state.clipboard.pointer_left();
                state.pointer_tracker.leave()
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => {
                state.note_time(time);
                state
                    .pointer_tracker
                    .motion(Point::new(surface_x, surface_y))
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(button_state),
            } => {
                state.note_time(time);
                state.last_serial = Some(serial);
                let pressed = button_state == ButtonState::Pressed;
                if pressed {
                    if state.held_buttons.is_empty() {
                        state.press_serial = Some(serial);
                        state.latest_press = PressDevice::Pointer;
                    }
                    state.held_buttons.push(button);
                } else {
                    state.held_buttons.retain(|b| *b != button);
                    if state.held_buttons.is_empty() {
                        state.press_serial = None;
                        released_all = true;
                    }
                }
                state.pointer_tracker.button(button, pressed)
            }
            wl_pointer::Event::Axis {
                time,
                axis: WEnum::Value(axis),
                value,
            } => {
                state.note_time(time);
                state
                    .pointer_tracker
                    .axis(axis == Axis::VerticalScroll, value);
                // before version 5 there is no `frame`, so each axis is a scroll of its own
                if pointer.version() < 5 {
                    state.pointer_tracker.frame()
                } else {
                    None
                }
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => {
                state.pointer_tracker.axis_source(source);
                None
            }
            wl_pointer::Event::AxisStop {
                time,
                axis: WEnum::Value(_),
            } => {
                state.note_time(time);
                state.pointer_tracker.axis_stop();
                None
            }
            // before version 8 a wheel reports whole notches
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                state
                    .pointer_tracker
                    .axis_value120(axis == Axis::VerticalScroll, discrete.saturating_mul(120));
                None
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                state
                    .pointer_tracker
                    .axis_value120(axis == Axis::VerticalScroll, value120);
                None
            }
            wl_pointer::Event::AxisRelativeDirection {
                axis: WEnum::Value(axis),
                direction: WEnum::Value(direction),
            } => {
                state.pointer_tracker.axis_relative_direction(
                    axis == Axis::VerticalScroll,
                    direction == wl_pointer::AxisRelativeDirection::Inverted,
                );
                None
            }
            wl_pointer::Event::Frame => state.pointer_tracker.frame(),
            _ => None,
        };
        if let Some((surface, event)) = found {
            let time = own_time.unwrap_or_else(|| state.input_time(Instant::now()));
            state.events.push(Event::Pointer {
                surface,
                time,
                event,
            });
        }
        if released_all {
            state.clipboard.buttons_released(&mut state.events);
        }
    }
}

impl State {
    pub(super) fn held_press(&self) -> Option<HeldPress> {
        let pointer = self.press_serial.map(|serial| HeldPress {
            serial,
            device: PressDevice::Pointer,
        });
        let touch = self.touch_press_serial.map(|serial| HeldPress {
            serial,
            device: PressDevice::Touch,
        });
        match self.latest_press {
            PressDevice::Pointer => pointer.or(touch),
            PressDevice::Touch => touch.or(pointer),
        }
    }

    fn cancel_touches(&mut self) {
        self.held_touches.clear();
        self.touch_press_serial = None;
        let time = self.input_time(Instant::now());
        for (surface, event) in self.touch_tracker.cancel() {
            self.events.push(Event::Touch {
                surface,
                time,
                event,
            });
        }
    }
}

impl Dispatch<WlTouch, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let (found, time) = match event {
            wl_touch::Event::Down {
                serial,
                time,
                surface,
                id,
                x,
                y,
            } => {
                state.last_serial = Some(serial);
                if state.held_touches.is_empty() {
                    state.touch_press_serial = Some(serial);
                    state.latest_press = PressDevice::Touch;
                }
                state.held_touches.push(id);
                let found = state
                    .surface_of(&surface)
                    .map(|s| (s, state.touch_tracker.down(id, s, Point::new(x, y))));
                (found, time)
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                (state.touch_tracker.motion(id, Point::new(x, y)), time)
            }
            wl_touch::Event::Up { serial, time, id } => {
                state.last_serial = Some(serial);
                state.held_touches.retain(|t| *t != id);
                if state.held_touches.is_empty() {
                    state.touch_press_serial = None;
                }
                (state.touch_tracker.up(id), time)
            }
            wl_touch::Event::Cancel => {
                state.cancel_touches();
                return;
            }
            _ => return,
        };
        state.note_time(time);
        if let Some((surface, event)) = found {
            state.events.push(Event::Touch {
                surface,
                time,
                event,
            });
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => match Translator::from_fd(fd, size as usize) {
                Ok(t) => state.translator = Some(t),
                Err(e) => state.fail(e),
            },
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                state.last_serial = Some(serial);
                state.key_focus = state.surface_of(&surface);
                if let Some(surface) = state.key_focus {
                    let time = state.input_time(Instant::now());
                    state.events.push(Event::Keyboard {
                        surface,
                        time,
                        event: KeyboardEvent::Enter,
                    });
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                state.repeat.clear();
                let shown = state.translator.as_mut().is_some_and(|t| t.reset_compose());
                if let (true, Some(surface)) = (shown, state.key_focus) {
                    state.events.push(preview_event(surface, None, None));
                }
                if let Some(surface) = state.key_focus.take() {
                    let time = state.input_time(Instant::now());
                    state.events.push(Event::Keyboard {
                        surface,
                        time,
                        event: KeyboardEvent::Leave,
                    });
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: WEnum::Value(key_state),
            } => {
                state.note_time(time);
                state.last_serial = Some(serial);
                let pressed = key_state == KeyState::Pressed;
                let (Some(surface), Some(t)) = (state.key_focus, state.translator.as_mut()) else {
                    return;
                };
                let k = if pressed {
                    t.press(key, &mut state.repeat, Instant::now())
                } else {
                    state.repeat.release(key);
                    t.translate(key)
                };
                if let Some(preview) = t.take_preview() {
                    state
                        .events
                        .push(preview_event(surface, preview.preedit, preview.commit));
                    if preview.swallow {
                        state.swallowed = Some(key);
                        return;
                    }
                }
                if !pressed && state.swallowed == Some(key) {
                    state.swallowed = None;
                    return;
                }
                state.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Key {
                        code: key,
                        pressed,
                        repeat: false,
                        keysym: k.keysym,
                        character: k.character,
                        mods: k.mods,
                    },
                });
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(t) = state.translator.as_mut() {
                    t.update_modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => state.repeat.set_info(rate, delay),
            _ => {}
        }
    }
}

/// The marks of a pending dead key sequence as an input method batch for `surface`: `preedit` shown highlighted,
/// `commit` typed. Text fields take it as they take one from an input method.
fn preview_event(surface: SurfaceId, preedit: Option<String>, commit: Option<String>) -> Event {
    Event::TextInput {
        surface,
        event: TextInputEvent::Done(TextInputDone {
            preedit: preedit.map(|text| Preedit {
                cursor: Some((0, text.len())),
                text,
            }),
            commit,
            delete: (0, 0),
            current: true,
        }),
    }
}
