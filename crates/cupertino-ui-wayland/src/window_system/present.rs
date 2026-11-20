use super::*;

impl WaylandWindowSystem {
    /// The window at `i` may draw again; whether the frame it asked for meanwhile is due now.
    pub(super) fn resume(&mut self, i: usize) -> bool {
        let w = &mut self.windows[i];
        w.stalled = None;
        w.busy_layers.clear();
        !w.frame_pending && !w.away && std::mem::take(&mut w.frame_wanted)
    }

    pub(super) fn send_size_limits(
        &mut self,
        window: WindowId,
    ) -> Result<Result<(), Error>, PlatformError> {
        let t = self.toplevel(window)?;
        let w = self.window(window)?;
        let (min, max) = (
            w.min_size.map(convert::whole),
            w.max_size.map(convert::whole),
        );
        Ok(self.conn.set_size_limits(t, min, max))
    }

    /// What every present of `window` does first: its backdrop shaped to its size, and its input and opaque regions
    /// set for the commit. Its surface and logical size.
    pub(super) fn before_present(
        &mut self,
        window: WindowId,
    ) -> Result<(SurfaceId, (u32, u32)), PlatformError> {
        let w = self.window_mut(window)?;
        let (surface, size) = (w.surface, w.size);
        if let Some(b) = w.backdrop.as_mut().filter(|b| b.shaped != Some(size)) {
            let bounds = Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
            if b.material.set_shape(&bounds, b.radius).is_ok() {
                b.shaped = Some(size);
            }
        }
        let opaque = w.opaque_region.take();
        if let Some(region) = w.input_region.take() {
            self.conn
                .set_input_region(surface, region.as_deref())
                .map_err(platform_error)?;
        }
        if let Some(region) = opaque {
            self.conn
                .set_opaque_region(surface, &region)
                .map_err(platform_error)?;
        }
        Ok((surface, size))
    }

    /// `window` committed a frame with a frame callback: its next frame waits for that callback. A frame queued as due
    /// at once before the commit (one the window asked for while handling the callback that let it draw) is folded
    /// into it, so the window draws once for each frame the compositor shows, not twice.
    pub(super) fn await_frame_callback(&mut self, window: WindowId) -> Result<(), PlatformError> {
        let folded = self.fold_due_frame(window);
        let w = self.window_mut(window)?;
        w.frame_pending = true;
        w.frame_wanted |= folded;
        Ok(())
    }

    /// `window`'s frame went to the render thread or the CPU pool, which commits it with a frame callback: the next
    /// frame waits for that callback, a frame queued as due at once before the submit and one the window asks for
    /// while the frame is away too, so the window is not handed frames it cannot draw until the frame is back and its
    /// commits are sent.
    pub(super) fn submitted(&mut self, window: WindowId) -> Result<Submitted, PlatformError> {
        let folded = self.fold_due_frame(window);
        let w = self.window_mut(window)?;
        w.away = true;
        w.frame_wanted |= folded;
        Ok(Submitted::Taken)
    }

    /// Takes the frames queued as due at once for `window` back; whether there was one.
    pub(super) fn fold_due_frame(&mut self, window: WindowId) -> bool {
        let before = self.queued.len();
        self.queued.retain(|event| {
            !matches!(event, PlatformEvent::Window { window: w, event: WindowEvent::Frame(_) } if *w == window)
        });
        self.queued.len() < before
    }

    /// `window` presented frame `id` of `size` pixels: its feedback awaited, its next frame paced, and the size and
    /// repaint rule told.
    pub(super) fn presented_frame(
        &mut self,
        window: WindowId,
        id: RenderFrameId,
        size: (u32, u32),
    ) -> Result<(), PlatformError> {
        let feedback = self.conn.capabilities().contains(Capability::Presentation);
        self.await_frame_callback(window)?;
        let w = self.window_mut(window)?;
        if feedback {
            w.in_flight.push_back(id);
        }
        w.shown = Some(size);
        let surface = w.surface;
        // a process of the size and repaint rule's class counts what its windows present
        let now = self.now().0;
        self.gpu.presented(&mut self.conn, surface, size, now);
        Ok(())
    }

    /// A submitted frame is back from the render thread or the CPU pool, before its commits go out: its surfaces'
    /// buffers and GPU state brought up to date, what its layers show recorded, its next frame paced, and how it was
    /// shown kept for the runtime ([`Self::sent`]).
    pub(super) fn finish(&mut self, finished: Finished) -> Result<(), PlatformError> {
        let Finished {
            window,
            backs,
            panic,
        } = finished;
        let mut presented = None;
        let mut failed = None;
        // the first error of the connection, returned once every buffer of the frame went back
        let mut broken = None;
        for back in backs {
            let drawn = match back.drew {
                Drew::Gpu(drawn) => {
                    let (shown, drawn) = match drawn {
                        Ok((id, from_texture)) => (Ok(id), Ok(from_texture)),
                        Err(Failure::Present(e)) => (Err(Shown::NotReady), Err(e)),
                        // the buffer prepared for it goes with the others
                        Err(Failure::Render(e)) => {
                            (Err(Shown::Failed(e)), Err(PresentError::Resized))
                        }
                    };
                    if let Err(e) = self.gpu.finish(&mut self.conn, back.surface, drawn) {
                        broken.get_or_insert(platform_error(e));
                    }
                    shown
                }
                Drew::Shm(buffer, Ok((id, damage))) => {
                    self.conn.buffer_shown(buffer, damage.as_deref());
                    Ok(id)
                }
                Drew::Shm(buffer, Err(e)) => {
                    self.conn.buffer_unused(buffer);
                    Err(match e {
                        Failure::Render(e) => Shown::Failed(e),
                        Failure::Present(_) => Shown::NotReady,
                    })
                }
            };
            match drawn {
                Ok(id) => match back.of {
                    Of::Window => presented = Some((id, back.size)),
                    Of::Raised => {
                        if let Some(layer) =
                            self.window_mut(window).ok().and_then(|w| w.layer.as_mut())
                        {
                            layer.shown = Some(back.size);
                        }
                    }
                    Of::Layer(key) => {
                        if let Some(layer) = self
                            .surface_layers
                            .shown_mut(window)
                            .iter_mut()
                            .find(|s| s.key == key)
                        {
                            layer.shown = Some(back.size);
                        }
                    }
                },
                // the renderer's error is the one the runtime hears
                Err(shown) => {
                    if !matches!(failed, Some(Shown::Failed(_))) {
                        failed = Some(shown);
                    }
                }
            }
        }
        // the frame's buffers went back, and the panic is the submitting instance's, as a task's is
        if let Some(panic) = panic {
            std::panic::resume_unwind(panic)
        }
        if let Some(e) = broken {
            return Err(e);
        }
        // a window destroyed meanwhile hears nothing
        if self.window_mut(window).is_err() {
            return Ok(());
        }
        let shown = match (failed, presented) {
            (Some(shown), _) => shown,
            (None, Some((id, size))) => {
                self.presented_frame(window, id, size)?;
                Shown::Presented(id)
            }
            (None, None) => Shown::NotReady,
        };
        self.window_mut(window)?.drawn = Some(shown);
        Ok(())
    }

    /// The commits of `window`'s frame are sent: the window may draw again, and the runtime hears how the frame was
    /// shown. Told only now, the runtime sends nothing of its next frame from the UI thread (a raised layer hidden, a
    /// solid colour) ahead of these commits. A commit without a buffer asked for meanwhile goes now: what it applies
    /// may have gone out after the frame's own commit.
    pub(super) fn sent(
        &mut self,
        window: WindowId,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        let Ok(w) = self.window_mut(window) else {
            return Ok(());
        };
        w.away = false;
        if let Some(shown) = w.drawn.take() {
            events.push(PlatformEvent::Window {
                window,
                event: WindowEvent::Shown(shown),
            });
        }
        if std::mem::take(&mut w.state_owed) {
            self.commit_state(window)?;
        }
        Ok(())
    }

    /// When a presented frame turned visible, `time_ns` on the compositor's presentation clock, on the window system's
    /// clock; now when that clock cannot be read.
    pub(super) fn presented_time(&self, time_ns: u64) -> Timestamp {
        let now = self.now().0;
        let clock_now = self
            .conn
            .presentation_clock()
            .and_then(|id| ClockId::try_from(id as i32).ok())
            .map(|id| {
                let t = rustix::time::clock_gettime(id);
                (t.tv_sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(t.tv_nsec as u64)
            });
        Timestamp(clock_now.map_or(now, |clock_now| {
            convert::on_window_clock(time_ns, clock_now, now)
        }))
    }

    pub(super) fn input_time(&mut self, millis: u32) -> Timestamp {
        let now = self.now().0;
        Timestamp(self.input_times.map(millis, now))
    }

    /// The window system's events for one transport event; none for a surface that is not a window here.
    pub(super) fn translate(&mut self, event: Event, out: &mut Vec<PlatformEvent>) {
        if matches!(&event, Event::Extension(e) if e.downcast_ref::<cupertino_wayland_client::app::SemanticWake>().is_some())
        {
            out.push(PlatformEvent::Wake);
            return;
        }
        let on = |windows: &[Window], surface: SurfaceId| {
            windows.iter().position(|w| w.surface == surface)
        };
        let mut due = false;
        let (i, event) = match event {
            Event::Configure { window, configure } => {
                let Some(i) = on(&self.windows, window.surface()) else {
                    return;
                };
                if let Some(size) = configure.size {
                    self.windows[i].size = size;
                }
                self.windows[i].free_size = configure.size.is_none();
                if !std::mem::replace(&mut self.windows[i].configured, true) {
                    let _ = self.publish_menus(self.windows[i].id);
                }
                due = self.resume(i);
                let configure = convert::configure(&configure);
                self.windows[i].activated = Some(configure.states.activated);
                (i, WindowEvent::Configure(configure))
            }
            Event::PopupConfigure { popup, configure } => {
                let Some(i) = on(&self.windows, popup.surface()) else {
                    return;
                };
                self.windows[i].size = (configure.width, configure.height);
                self.windows[i].free_size = false;
                due = self.resume(i);
                out.push(PlatformEvent::Window {
                    window: self.windows[i].id,
                    event: WindowEvent::PopupPlaced(convert::popup_placement(&configure)),
                });
                (
                    i,
                    WindowEvent::Configure(convert::popup_configure(&configure)),
                )
            }
            Event::Close(window) => match on(&self.windows, window.surface()) {
                Some(i) => (i, WindowEvent::CloseRequested),
                None => return,
            },
            Event::PopupDone(popup) => match on(&self.windows, popup.surface()) {
                Some(i) => (i, WindowEvent::PopupDone),
                None => return,
            },
            Event::Frame(callback) => {
                let Some(i) = on(&self.windows, callback.surface) else {
                    return;
                };
                self.windows[i].frame_pending = false;
                if let Some(roles) = self.roles.as_mut() {
                    roles.frame_done(callback.surface);
                }
                if !self.resume(i) {
                    return;
                }
                // animations step to the time of the frame the compositor showed, not to when it was read
                let time = Timestamp(convert::frame_time(callback.time, self.now().0));
                (i, WindowEvent::Frame(time))
            }
            Event::ScaleChanged { surface, scale } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Scale(scale.factor())),
                None => return,
            },
            Event::Pointer {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let serial = self.conn.input_serial().unwrap_or(0);
                let time = self.input_time(time);
                let mods = convert::mods(self.conn.modifiers(), &self.profile);
                (
                    i,
                    WindowEvent::Pointer(convert::pointer(event, serial, mods), time),
                )
            }
            Event::Touch {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let time = self.input_time(time);
                (i, WindowEvent::Touch(convert::touch(event), time))
            }
            Event::Keyboard {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                if let (KeyboardEvent::Enter | KeyboardEvent::Leave, Some(roles)) =
                    (&event, self.roles.as_mut())
                {
                    roles.keyboard_focus(surface, matches!(event, KeyboardEvent::Enter));
                }
                match event {
                    KeyboardEvent::Enter => self.windows[i].keyboard = true,
                    KeyboardEvent::Leave => self.windows[i].keyboard = false,
                    KeyboardEvent::Key { .. } => {}
                }
                let event = match event {
                    KeyboardEvent::Enter => WindowEvent::Focus(true),
                    KeyboardEvent::Leave => WindowEvent::Focus(false),
                    KeyboardEvent::Key {
                        code,
                        pressed,
                        repeat,
                        keysym,
                        character,
                        mods,
                    } => WindowEvent::Key(
                        KeyEvent {
                            code,
                            keysym: Keysym(keysym),
                            text: character,
                            pressed,
                            repeat,
                            mods: convert::mods(mods, &self.profile),
                        },
                        self.input_time(time),
                    ),
                };
                (i, event)
            }
            Event::TextInput { surface, event } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Ime(convert::ime(event))),
                None => return,
            },
            Event::Selection(offer) => {
                let mimes = offer.as_ref().map(|o| o.mime_types().to_vec());
                self.selection = offer;
                out.push(PlatformEvent::ClipboardOffered {
                    kind: ClipboardKind::Clipboard,
                    mimes,
                });
                return;
            }
            Event::SelectionLost => {
                out.push(PlatformEvent::ClipboardReplaced {
                    kind: ClipboardKind::Clipboard,
                });
                return;
            }
            Event::Received { id, data } => {
                if let Some(at) = self.reads.iter().position(|(r, _, _)| *r == id) {
                    let (_, token, mime) = self.reads.swap_remove(at);
                    let data = data.map(|bytes| ClipboardData { mime, bytes });
                    out.push(PlatformEvent::Clipboard { token, data });
                }
                return;
            }
            Event::ActivationToken { request, token } => {
                if let Some(at) = self.tokens.iter().position(|(r, _)| *r == request) {
                    let (_, request) = self.tokens.swap_remove(at);
                    out.push(PlatformEvent::ActivationToken {
                        request,
                        token: Some(token),
                    });
                }
                return;
            }
            Event::Extension(extension) => {
                if let Some(role) = self.roles.as_mut().and_then(|r| r.event(&extension)) {
                    let (surface, event) = match role {
                        RoleEvent::Configure { surface, size } => (surface, Some(size)),
                        RoleEvent::Closed(surface) => (surface, None),
                    };
                    let Some(i) = on(&self.windows, surface) else {
                        return;
                    };
                    match event {
                        Some(size) => {
                            self.role_configures = self.role_configures.wrapping_add(1);
                            let configure = convert::role_configure(
                                self.role_configures,
                                size,
                                holds_keyboard(&self.windows, i),
                            );
                            self.windows[i].size = size;
                            self.windows[i].free_size = false;
                            due = self.resume(i);
                            (i, WindowEvent::Configure(configure))
                        }
                        None => (i, WindowEvent::CloseRequested),
                    }
                } else {
                    match extension.downcast_ref::<AppEvent>() {
                        Some(AppEvent::SessionLock { locked }) => {
                            out.push(PlatformEvent::SessionLock(*locked));
                        }
                        Some(AppEvent::ScrollBars { always }) => {
                            out.push(PlatformEvent::ScrollBars(if *always {
                                ScrollBars::Always
                            } else {
                                ScrollBars::WhenScrolling
                            }));
                        }
                        Some(event) if self.surface_layers.heard(event) => {}
                        Some(event) if self.presentations.heard(event) => {}
                        Some(AppEvent::BuffersLost) => {
                            self.gpu.buffers_lost(&mut self.conn);
                            for w in &mut self.windows {
                                w.shown = None;
                            }
                            out.push(PlatformEvent::BuffersLost);
                        }
                        Some(event) if apply_appearance(&mut self.appearance, event) => {
                            out.push(PlatformEvent::Appearance(self.appearance));
                        }
                        Some(AppEvent::BackdropTone { backdrop, tone }) => {
                            if let Some((window, region)) = glass_region(&self.windows, *backdrop) {
                                out.push(PlatformEvent::Window {
                                    window,
                                    event: WindowEvent::GlassTone {
                                        region,
                                        tone: glass_tone(*tone),
                                    },
                                });
                            }
                        }
                        Some(event) => {
                            for w in &mut self.windows {
                                if let Some(ShownMenus {
                                    published: Published::Shown(menus),
                                    ..
                                }) = &mut w.menus
                                {
                                    if menus.handle(event) {
                                        break;
                                    }
                                }
                            }
                        }
                        None => {}
                    }
                    return;
                }
            }
            Event::Presented(presented) => {
                let Some(i) = on(&self.windows, presented.surface) else {
                    return;
                };
                let Some(frame) = self.windows[i].in_flight.pop_front() else {
                    return;
                };
                let time = self.presented_time(presented.time_ns);
                (i, WindowEvent::Presented { frame, time })
            }
            Event::Discarded(surface) => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let Some(frame) = self.windows[i].in_flight.pop_front() else {
                    return;
                };
                (i, WindowEvent::Discarded { frame })
            }
            Event::OutputsChanged => {
                if let Some(roles) = self.roles.as_mut() {
                    roles.outputs_changed(&self.conn);
                }
                return;
            }
            Event::DragSource { source, event } => {
                let Some((_, token)) = self.drag.filter(|(s, _)| *s == source) else {
                    return;
                };
                let progress = match event {
                    DragSourceEvent::Target(mime) => DragProgress::Target(mime),
                    DragSourceEvent::Action(action) => DragProgress::Action(drop_action(action)),
                    DragSourceEvent::Dropped => DragProgress::Dropped,
                    DragSourceEvent::Finished => DragProgress::Finished,
                    DragSourceEvent::Cancelled => DragProgress::Cancelled,
                };
                if matches!(progress, DragProgress::Finished | DragProgress::Cancelled) {
                    self.drag = None;
                }
                out.push(PlatformEvent::Drag { token, progress });
                return;
            }
            Event::Drag { surface, event } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let mods = convert::mods(self.conn.modifiers(), &self.profile);
                let event = match event {
                    DragEvent::Enter { offer, point } => {
                        self.next_drop += 1;
                        let token = DropToken(self.next_drop);
                        let event = DropEvent::Enter {
                            token,
                            position: point,
                            mimes: offer.mime_types().to_vec(),
                            actions: drop_actions(offer.source_actions()),
                            mods,
                        };
                        self.hovering = Some(IncomingDrop { token, offer });
                        event
                    }
                    DragEvent::Motion { point, .. } => DropEvent::Motion {
                        position: point,
                        mods,
                    },
                    // after the drop the choice is the window's own, told by finish_drop
                    DragEvent::Action(_) if self.hovering.is_none() => return,
                    DragEvent::Action(action) => DropEvent::Action(drop_action(action)),
                    DragEvent::Leave => {
                        self.hovering = None;
                        DropEvent::Leave
                    }
                    DragEvent::Drop => {
                        let Some(drop) = self.hovering.take() else {
                            return;
                        };
                        if self.landed.len() >= MAX_LANDED {
                            self.landed.remove(0);
                        }
                        self.landed.push(drop);
                        DropEvent::Drop
                    }
                };
                (i, WindowEvent::Drop(event))
            }
            Event::PopupRepositioned { .. } => return,
        };
        let window = self.windows[i].id;
        out.push(PlatformEvent::Window { window, event });
        if due {
            out.push(PlatformEvent::Window {
                window,
                event: WindowEvent::Frame(self.now()),
            });
        }
    }
}
