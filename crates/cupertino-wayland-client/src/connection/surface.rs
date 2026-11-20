use super::*;

impl Connection {
    /// Connect to the compositor named by `WAYLAND_DISPLAY` (or the socket in `WAYLAND_SOCKET`).
    pub fn connect() -> Result<Connection, Error> {
        Self::from_wayland(
            WlConnection::connect_to_env()
                .map_err(|e| Error::Disconnected(Disconnect::Connect(e)))?,
        )
    }

    /// Connect over an established socket, such as one end of a socket pair.
    pub fn from_stream(stream: UnixStream) -> Result<Connection, Error> {
        Self::from_wayland(
            WlConnection::from_socket(stream)
                .map_err(|e| Error::Disconnected(Disconnect::Connect(e)))?,
        )
    }

    /// Take over an established connection, for a process that runs event queues of its own on it next to this one.
    pub fn from_wayland(conn: WlConnection) -> Result<Connection, Error> {
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| Error::Disconnected(Disconnect::Registry(e)))?;
        let qh = queue.handle();
        let capabilities = CapabilitySet::from_globals(
            globals
                .contents()
                .clone_list()
                .iter()
                .map(|g| (g.interface.as_str(), g.version))
                .collect::<Vec<_>>(),
        )?;
        // version 8 adds `axis_value120`, version 9 `axis_relative_direction`
        let seat: Option<WlSeat> = bind(&globals, &qh, 1..=9);
        let data_device_manager = bind(&globals, &qh, 1..=3);
        let clipboard = Clipboard::new(data_device_manager.as_ref(), seat.as_ref(), &qh);
        let text_input_manager: Option<ZwpTextInputManagerV3> = bind(&globals, &qh, 1..=1);
        let text_input = TextInput::new(text_input_manager.as_ref(), seat.as_ref(), &qh);
        // version 6 adds `preferred_buffer_scale`
        let compositor: WlCompositor = required(bind(&globals, &qh, 4..=6), "wl_compositor")?;
        let shm: WlShm = required(bind(&globals, &qh, 1..=1), "wl_shm")?;
        let fractional: Option<WpFractionalScaleManagerV1> = bind(&globals, &qh, 1..=1);
        let viewporter: Option<WpViewporter> = bind(&globals, &qh, 1..=1);
        let mut state = State {
            capabilities,
            themed_cursor: ThemedCursor::new(compositor.clone(), shm.clone()),
            compositor,
            shm,
            shm_formats: Vec::new(),
            // version 4 adds `configure_bounds`, version 6 the suspended state
            wm_base: required(bind(&globals, &qh, 1..=6), "xdg_wm_base")?,
            subcompositor: bind(&globals, &qh, 1..=1),
            seat,
            decoration_manager: bind(&globals, &qh, 1..=1),
            importer: bind(&globals, &qh, 1..=1),
            dialogs: bind(&globals, &qh, 1..=1),
            scaling: fractional.zip(viewporter.clone()),
            viewporter,
            single_pixel: bind(&globals, &qh, 1..=1),
            activation: bind(&globals, &qh, 1..=1),
            cursor_shape: bind(&globals, &qh, 1..=1),
            // version 2 allows a refresh rate picked for a variable one
            presentation: bind(&globals, &qh, 1..=2),
            presentation_clock: None,
            // version 3 sends the logical geometry before `wl_output.done` instead of a `done` of its own
            output_manager: bind(&globals, &qh, 1..=3),
            data_device_manager,
            clipboard,
            text_input,
            pointer: None,
            cursor_device: None,
            pointer_enter_serial: None,
            held_buttons: Vec::new(),
            press_serial: None,
            touch: None,
            held_touches: Vec::new(),
            touch_press_serial: None,
            latest_press: PressDevice::Pointer,
            keyboard: None,
            outputs: Vec::new(),
            surfaces: Vec::new(),
            next_surface: 0,
            next_reposition: 0,
            next_token: 0,
            pointer_tracker: PointerTracker::default(),
            touch_tracker: TouchTracker::default(),
            key_focus: None,
            translator: None,
            swallowed: None,
            repeat: Repeat::default(),
            last_serial: None,
            input_clock: InputClock::default(),
            events: Vec::new(),
            fault: None,
            gpu: gpu::GpuState::default(),
        };
        let registry = globals.registry();
        for g in globals
            .contents()
            .clone_list()
            .iter()
            .filter(|g| g.interface == "wl_output")
        {
            state.bind_output(registry, &qh, g.name, g.version);
        }
        // outputs and seat capabilities report before the first surface exists
        queue.roundtrip(&mut state)?;
        state.events.clear();
        let (wake, writer) = UnixStream::pair().map_err(Error::Io)?;
        wake.set_nonblocking(true).map_err(Error::Io)?;
        writer.set_nonblocking(true).map_err(Error::Io)?;
        Ok(Connection {
            conn,
            globals,
            queue,
            state,
            wake,
            waker: Waker(Arc::new(writer)),
        })
    }

    /// The optional protocols the compositor offers.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.state.capabilities
    }

    /// Whether the compositor takes buffers of `format`: the two every compositor takes, and those it lists.
    pub fn offers(&self, format: BufferFormat) -> bool {
        format.required() || self.state.shm_formats.contains(&format.wire())
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// The clock of [`Presented::time_ns`]: a `clockid_t` such as `CLOCK_MONOTONIC`, to read with `clock_gettime`.
    /// `None` without [`Capability::Presentation`].
    pub fn presentation_clock(&self) -> Option<u32> {
        self.state.presentation_clock
    }

    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state
            .outputs
            .iter()
            .map(|o| o.info.current.clone())
            .collect()
    }

    /// The buffer scale of a surface: what [`Connection::lease`] draws at.
    pub fn scale(&self, surface: SurfaceId) -> Result<Scale, Error> {
        Ok(self.state.surface(surface)?.scale)
    }

    /// Create an application window. It maps with its first presented buffer, after its first
    /// [`Event::Configure`].
    pub fn toplevel(&mut self, config: &ToplevelConfig) -> Result<Toplevel, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let xdg = s.wm_base.get_xdg_surface(&wl, &qh, id);
        let toplevel = xdg.get_toplevel(&qh, id);
        toplevel.set_app_id(config.app_id.clone());
        toplevel.set_title(config.title.clone());
        if let Some((w, h)) = config.min_size {
            toplevel.set_min_size(clamp_i32(w), clamp_i32(h));
        }
        if let Some((w, h)) = config.max_size {
            toplevel.set_max_size(clamp_i32(w), clamp_i32(h));
        }
        let decoration = s.decoration_manager.as_ref().map(|m| {
            let d = m.get_toplevel_decoration(&toplevel, &qh, id);
            d.set_mode(decoration_mode(config.decorations));
            d
        });
        let imported =
            config
                .foreign_parent
                .as_ref()
                .zip(s.importer.as_ref())
                .map(|(parent, importer)| {
                    let imported = importer.import_toplevel(parent.handle.clone(), &qh, id);
                    imported.set_parent_of(&wl);
                    imported
                });
        // before the first commit, so the compositor places the window as its parent's modal dialog from the start
        let dialog = config
            .foreign_parent
            .as_ref()
            .filter(|parent| parent.modal)
            .zip(s.dialogs.as_ref())
            .map(|(_, dialogs)| {
                let dialog = dialogs.get_xdg_dialog(&toplevel, &qh, ());
                dialog.set_modal();
                Box::new(dialog)
            });
        s.add_surface(
            id,
            wl,
            Role::Toplevel {
                xdg,
                toplevel,
                decoration,
                decorations: Decorations::Client,
                imported,
                dialog,
                pending: PendingToplevel::default(),
                current: None,
            },
            &qh,
        );
        Ok(Toplevel(id))
    }

    /// Create a popup of `parent` (a window or popup). It maps with its first presented buffer, after its first
    /// [`Event::PopupConfigure`]. A grab needs an input event to answer: before one ([`Connection::input_serial`] is
    /// `None`) the popup is [`Error::PermissionDenied`] and nothing is made, and the connection goes on.
    pub fn popup(&mut self, parent: SurfaceId, placement: &PopupPlacement) -> Result<Popup, Error> {
        self.make_popup(parent, placement, None)
    }

    /// [`Connection::popup`] whose grab, when `placement.grab` asks for one, answers `serial`: a serial the compositor
    /// handed out for a grab of this connection's (as with an action it routed here) rather than the latest input
    /// event's, which may be none yet or long past.
    pub fn popup_with_grab_serial(
        &mut self,
        parent: SurfaceId,
        placement: &PopupPlacement,
        serial: u32,
    ) -> Result<Popup, Error> {
        self.make_popup(parent, placement, Some(serial))
    }

    pub(super) fn make_popup(
        &mut self,
        parent: SurfaceId,
        placement: &PopupPlacement,
        serial: Option<u32>,
    ) -> Result<Popup, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let parent_xdg = match &s.surface(parent)?.role {
            Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => Some(xdg.clone()),
            Role::External { .. } => None,
            Role::Subsurface { .. } => {
                return Err(Error::Protocol("a subsurface takes no popups"));
            }
        };
        // refused before any object is made, so a refusal leaves nothing behind
        let grab_serial = if placement.grab {
            Some(
                serial
                    .or(s.last_serial)
                    .ok_or(Error::PermissionDenied("a popup grab"))?,
            )
        } else {
            None
        };
        let positioner = positioner(&s.wm_base, &qh, placement, s.surface(parent)?.serials())?;
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let xdg = s.wm_base.get_xdg_surface(&wl, &qh, id);
        let popup = xdg.get_popup(parent_xdg.as_ref(), &positioner, &qh, id);
        positioner.destroy();
        if let Role::External { role, .. } = &s.surface(parent)?.role {
            if let Err(e) = role.adopt_popup(&popup) {
                popup.destroy();
                xdg.destroy();
                wl.destroy();
                return Err(e);
            }
        }
        if let (Some(serial), Some(seat)) = (grab_serial, s.seat.as_ref()) {
            popup.grab(seat, serial);
        }
        s.add_surface(
            id,
            wl,
            Role::Popup {
                xdg,
                popup,
                parent,
                pending: None,
                current: None,
            },
            &qh,
        );
        Ok(Popup(id))
    }

    /// Move or resize `popup` to `placement` (its grab and `placement.grab` are left as they are); the compositor answers
    /// with [`Event::PopupRepositioned`] carrying the returned token, then a new [`Event::PopupConfigure`]. Needs
    /// `xdg_wm_base` version 3.
    pub fn reposition_popup(
        &mut self,
        popup: Popup,
        placement: &PopupPlacement,
    ) -> Result<u32, Error> {
        const REPOSITION_VERSION: u32 = 3;
        let qh = self.queue.handle();
        let s = &mut self.state;
        if s.wm_base.version() < REPOSITION_VERSION {
            return Err(Error::MissingRequiredProtocol {
                interface: "xdg_wm_base",
                minimum_version: REPOSITION_VERSION,
            });
        }
        let Role::Popup { popup, parent, .. } = &s.surface(popup.0)?.role else {
            unreachable!("a Popup handle names a popup");
        };
        let (popup, parent) = (popup.clone(), *parent);
        let positioner = positioner(&s.wm_base, &qh, placement, s.surface(parent)?.serials())?;
        s.next_reposition += 1;
        popup.reposition(&positioner, s.next_reposition);
        positioner.destroy();
        Ok(s.next_reposition)
    }

    /// Destroy a surface, with its popups first.
    pub fn destroy(&mut self, surface: SurfaceId) -> Result<(), Error> {
        self.state.surface(surface)?;
        let children: Vec<SurfaceId> = self
            .state
            .surfaces
            .iter()
            .filter_map(|s| match s.role {
                Role::Popup { parent, .. } | Role::Subsurface { parent, .. }
                    if parent == surface =>
                {
                    Some(s.id)
                }
                _ => None,
            })
            .collect();
        for child in children {
            self.destroy(child)?;
        }
        let i = self
            .state
            .surfaces
            .iter()
            .position(|s| s.id == surface)
            .ok_or(Error::UnknownSurface)?;
        let s = self.state.surfaces.remove(i);
        // a commit made for it on another thread sends nothing from now on
        commit::lock(&s.shared).gone = true;
        self.state.clipboard.forget_surface(surface);
        match &s.role {
            Role::Toplevel {
                xdg,
                toplevel,
                decoration,
                imported,
                dialog,
                ..
            } => {
                if let Some(d) = decoration {
                    d.destroy();
                }
                if let Some(d) = dialog {
                    d.destroy();
                }
                toplevel.destroy();
                xdg.destroy();
                // after the window: giving the handle up first would end the parent while the window is still open,
                // and the compositor could not hand the keyboard back to the parent as the window closes
                if let Some(i) = imported {
                    i.destroy();
                }
            }
            Role::Popup { xdg, popup, .. } => {
                popup.destroy();
                xdg.destroy();
            }
            Role::External { role, .. } => role.destroy(),
            Role::Subsurface { subsurface, .. } => subsurface.destroy(),
        }
        if let Some(o) = &s.syncobj {
            o.destroy();
            self.state.gpu.syncobj_surfaces.1 += 1;
        }
        if let Some(v) = &s.viewport {
            v.destroy();
        }
        if let Some(f) = &s.fractional {
            f.destroy();
        }
        s.wl.destroy();
        if let Some((buffer, _)) = &s.solid {
            buffer.destroy();
        }
        self.state.pointer_tracker.forget(surface);
        self.state.touch_tracker.forget(surface);
        self.state.text_input.forget(surface);
        if self.state.key_focus == Some(surface) {
            self.state.key_focus = None;
            self.state.repeat.clear();
        }
        Ok(())
    }

    pub fn set_title(&mut self, window: Toplevel, title: &str) -> Result<(), Error> {
        self.state.toplevel(window)?.set_title(title.to_string());
        Ok(())
    }

    /// The smallest and largest size, in logical points; `None` for no limit. Applies with the next present.
    pub fn set_size_limits(
        &mut self,
        window: Toplevel,
        min: Option<(u32, u32)>,
        max: Option<(u32, u32)>,
    ) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        let (w, h) = min.unwrap_or((0, 0));
        t.set_min_size(clamp_i32(w), clamp_i32(h));
        let (w, h) = max.unwrap_or((0, 0));
        t.set_max_size(clamp_i32(w), clamp_i32(h));
        Ok(())
    }

    pub fn set_maximized(&mut self, window: Toplevel, maximized: bool) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        if maximized {
            t.set_maximized();
        } else {
            t.unset_maximized();
        }
        Ok(())
    }

    pub fn set_fullscreen(&mut self, window: Toplevel, fullscreen: bool) -> Result<(), Error> {
        let t = self.state.toplevel(window)?;
        if fullscreen {
            t.set_fullscreen(None);
        } else {
            t.unset_fullscreen();
        }
        Ok(())
    }

    pub fn minimize(&mut self, window: Toplevel) -> Result<(), Error> {
        self.state.toplevel(window)?.set_minimized();
        Ok(())
    }

    /// Start moving the window with the pointer or finger, in answer to the press still held ([`Connection::held_press`]),
    /// whatever pointer motion or keys came since (a client-drawn title bar). With no press held nothing is sent and the
    /// move is [`Error::PermissionDenied`].
    pub fn start_move(&mut self, window: Toplevel) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "moving a window without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?._move(&seat, serial);
        Ok(())
    }

    /// Show the compositor's window menu at `position` (window-local logical points), in answer to the press still held
    /// ([`Connection::held_press`]: a right click on a client-drawn title bar). With no press held nothing is sent and
    /// the menu is [`Error::PermissionDenied`].
    pub fn show_window_menu(&mut self, window: Toplevel, position: Point) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "a window menu without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?.show_window_menu(
            &seat,
            serial,
            position.x.round() as i32,
            position.y.round() as i32,
        );
        Ok(())
    }

    /// Ask again who draws the frame; the answer arrives with the next [`Event::Configure`]. Without the decoration
    /// protocol the client always does, so asking the compositor is [`Error::Unsupported`].
    pub fn set_decorations(
        &mut self,
        window: Toplevel,
        decorations: Decorations,
    ) -> Result<(), Error> {
        let Role::Toplevel { decoration, .. } = &self.state.surface(window.0)?.role else {
            unreachable!("a Toplevel handle names a toplevel");
        };
        match (decoration, decorations) {
            (Some(d), _) => d.set_mode(decoration_mode(decorations)),
            (None, Decorations::Client) => {}
            (None, Decorations::Server) => {
                return Err(Error::Unsupported(Capability::ServerDecoration))
            }
        }
        Ok(())
    }

    /// Start resizing the window from `edge`, in answer to the press still held ([`Connection::held_press`]: a
    /// client-drawn border). With no press held nothing is sent and the resize is [`Error::PermissionDenied`].
    pub fn start_resize(&mut self, window: Toplevel, edge: ResizeEdge) -> Result<(), Error> {
        let serial = self
            .state
            .held_press()
            .ok_or(Error::PermissionDenied(
                "resizing a window without a press held",
            ))?
            .serial;
        let seat = self
            .state
            .seat
            .clone()
            .ok_or(Error::Unsupported(Capability::Seat))?;
        self.state.toplevel(window)?.resize(&seat, serial, edge);
        Ok(())
    }

    /// Ask the compositor to focus the window (`xdg_activation_v1`), with the latest input event as the reason.
    pub fn activate(&mut self, window: Toplevel) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let qh = self.queue.handle();
        let s = &self.state;
        let surface = s.surface(window.0)?;
        let activation = s.activation.as_ref().expect("bound with the capability");
        let token = activation.get_activation_token(&qh, TokenUse::Activate(window.0));
        if let (Some(serial), Some(seat)) = (s.last_serial, s.seat.as_ref()) {
            token.set_serial(serial, seat);
        }
        token.set_surface(&surface.wl);
        token.commit();
        Ok(())
    }

    /// Activate an existing window with a token forwarded by its launcher or a second instance.
    /// The compositor validates and consumes the token; this does not mint an input grant.
    pub fn activate_with_token(&mut self, window: Toplevel, token: &str) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let surface = self.state.surface(window.0)?;
        self.state
            .activation
            .as_ref()
            .expect("bound with the capability")
            .activate(token.to_owned(), &surface.wl);
        Ok(())
    }

    /// Take pointer input only inside `rects` (surface-local logical points, rounded outwards), or over the whole surface
    /// with `None`; input elsewhere reaches the surfaces below, and an empty slice lets all of it through. Applies with
    /// the next present.
    pub fn set_input_region(
        &mut self,
        surface: SurfaceId,
        rects: Option<&[Rect]>,
    ) -> Result<(), Error> {
        let wl = &self.state.surface(surface)?.wl;
        let Some(rects) = rects else {
            wl.set_input_region(None);
            return Ok(());
        };
        let region = self
            .state
            .compositor
            .create_region(&self.queue.handle(), ());
        for r in rects {
            let (x, y, width, height) = round_out(r);
            region.add(x, y, width, height);
        }
        wl.set_input_region(Some(&region));
        region.destroy();
        Ok(())
    }

    /// Say the parts of `surface` its pixels cover whole, `rects` in surface-local logical points rounded inwards, so
    /// the compositor may leave out what lies under them and tell a window they cover that it is not seen; an empty
    /// slice says none is. Applies with the next present.
    pub fn set_opaque_region(&mut self, surface: SurfaceId, rects: &[Rect]) -> Result<(), Error> {
        let wl = &self.state.surface(surface)?.wl;
        if rects.is_empty() {
            wl.set_opaque_region(None);
            return Ok(());
        }
        let region = self
            .state
            .compositor
            .create_region(&self.queue.handle(), ());
        for r in rects {
            let (x, y, width, height) = round_in(r);
            if width > 0 && height > 0 {
                region.add(x, y, width, height);
            }
        }
        wl.set_opaque_region(Some(&region));
        region.destroy();
        Ok(())
    }

    /// Ask for an activation token to hand to another client, such as an application this one launches (in
    /// `XDG_ACTIVATION_TOKEN`), with the latest input event on `surface` as the reason; `app_id` names that application
    /// when known. The token arrives as [`Event::ActivationToken`].
    pub fn request_activation_token(
        &mut self,
        surface: SurfaceId,
        app_id: Option<&str>,
    ) -> Result<TokenRequest, Error> {
        self.state.capabilities.require(Capability::Activation)?;
        let qh = self.queue.handle();
        let s = &mut self.state;
        let wl = s.surface(surface)?.wl.clone();
        s.next_token += 1;
        let request = TokenRequest(s.next_token);
        let activation = s.activation.as_ref().expect("bound with the capability");
        let token = activation.get_activation_token(&qh, TokenUse::Hand(request));
        if let (Some(serial), Some(seat)) = (s.last_serial, s.seat.as_ref()) {
            token.set_serial(serial, seat);
        }
        if let Some(app_id) = app_id {
            token.set_app_id(app_id.to_owned());
        }
        token.set_surface(&wl);
        token.commit();
        Ok(request)
    }

    /// Hide the cursor over the surface the pointer is on, for an application that draws its own.
    pub fn hide_cursor(&mut self) -> Result<(), Error> {
        let (Some(pointer), Some(serial)) = (&self.state.pointer, self.state.pointer_enter_serial)
        else {
            return Err(Error::PermissionDenied(
                "a cursor before the pointer entered",
            ));
        };
        pointer.set_cursor(serial, None, 0, 0);
        self.state.themed_cursor.stop();
        Ok(())
    }

    /// The modifiers held now, as of the last `wl_keyboard.modifiers`; none before the keymap arrives.
    pub fn modifiers(&self) -> Modifiers {
        self.state
            .translator
            .as_ref()
            .map_or(Modifiers::default(), Translator::modifiers)
    }

    /// The serial of the latest input event, for an extension request that answers one; `None` before any input.
    pub fn input_serial(&self) -> Option<u32> {
        self.state.last_serial
    }

    /// The press the seat still holds, for an extension request that must answer it: of the pointer buttons and the
    /// fingers down, whichever started its implicit grab last. `None` once all of them are up, or after the pointer
    /// left or the compositor cancelled the touches.
    pub fn held_press(&self) -> Option<HeldPress> {
        self.state.held_press()
    }

    /// The outputs `surface` is on, in the order it entered them.
    pub fn surface_outputs(&self, surface: SurfaceId) -> Result<Vec<OutputId>, Error> {
        Ok(self.state.surface(surface)?.entered.clone())
    }

    /// Draw the cursors of [`Connection::set_cursor`] from `theme` when the compositor has no cursor shapes, instead of
    /// the user's ([`CursorTheme::from_env`], read when first needed).
    pub fn set_cursor_theme(&mut self, theme: CursorTheme) {
        self.state.themed_cursor.set_theme(theme);
    }

    /// Show the cursor `shape` over the surface the pointer is on: the compositor's with [`Capability::CursorShape`],
    /// else drawn from the cursor theme at the scale of that surface.
    pub fn set_cursor(&mut self, shape: wp_cursor_shape_device_v1::Shape) -> Result<(), Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let (Some(pointer), Some(serial)) = (&s.pointer, s.pointer_enter_serial) else {
            return Err(Error::PermissionDenied(
                "a cursor before the pointer entered",
            ));
        };
        if let Some(device) = &s.cursor_device {
            device.set_shape(serial, shape);
            s.themed_cursor.stop();
            return Ok(());
        }
        let scale = s.cursor_scale();
        s.themed_cursor.show(&qh, pointer, serial, shape, scale)?;
        self.flush()
    }

    /// Tell the input method about the text field focused on `surface`, or with `None` that no field there takes text.
    /// It is sent while the surface has the text-input focus and again after each [`TextInputEvent::Enter`]; an
    /// unchanged state sends nothing. A password or PIN purpose sends no surrounding text and adds the sensitive-data
    /// hint. Answers arrive as [`Event::TextInput`].
    pub fn set_text_input(
        &mut self,
        surface: SurfaceId,
        state: Option<&TextInputState>,
    ) -> Result<(), Error> {
        if state.is_some() {
            self.state.capabilities.require(Capability::TextInput)?;
        }
        self.state.surface(surface)?;
        self.state.text_input.set(surface, state);
        Ok(())
    }

    /// Set the selection to `contents`, one byte string per MIME type, in answer to the latest input event.
    pub fn set_selection(&mut self, contents: Vec<(String, Vec<u8>)>) -> Result<(), Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        let serial = self
            .state
            .last_serial
            .ok_or(Error::PermissionDenied("setting the selection"))?;
        let qh = self.queue.handle();
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("bound with the capability");
        self.state.clipboard.set(&manager, contents, serial, &qh);
        Ok(())
    }

    /// Read the selection `offer` in `mime`, one of its [`ClipboardOffer::mime_types`]. Blocks for up to one second;
    /// [`Connection::receive_async`] doesn't.
    pub fn receive(&mut self, offer: &ClipboardOffer, mime: &str) -> Result<Vec<u8>, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        self.state.clipboard.receive(&self.conn, offer, mime)
    }

    /// Start reading the selection `offer` in `mime` while [`Connection::wait`] goes on; the data arrives as
    /// [`Event::Received`] with the returned id, at most 16 MiB, or `None` when the offer was replaced, its client sent
    /// nothing for a second, or the read took longer than ten seconds. At most eight reads run at once.
    pub fn receive_async(&mut self, offer: &ClipboardOffer, mime: &str) -> Result<ReadId, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        self.state.clipboard.receive_async(&self.conn, offer, mime)
    }

    /// Start dragging `contents`, offered in each of their MIME types, from `origin` while a pointer button is held,
    /// the target allowed the `actions`; [`Event::DragSource`] tells how it goes. The drag answers that button's
    /// press, whatever keys were pressed since; with no button held it is refused. A drag started before replaces the
    /// last.
    pub fn start_drag(
        &mut self,
        origin: SurfaceId,
        contents: Vec<(String, Vec<u8>)>,
        actions: DragActions,
    ) -> Result<DragSource, Error> {
        self.start_drag_with_icon(origin, contents, actions, None)
    }

    /// [`Connection::start_drag`] with `icon` following the pointer, its hotspot under it. The icon's surface and
    /// buffer live until the drag drops, is cancelled or is replaced by another; a drag the compositor ignores (one
    /// that came after the button went up) ends as [`DragSourceEvent::Cancelled`] once the release arrives, and frees
    /// them too.
    pub fn start_drag_with_icon(
        &mut self,
        origin: SurfaceId,
        contents: Vec<(String, Vec<u8>)>,
        actions: DragActions,
        icon: Option<&DragIcon>,
    ) -> Result<DragSource, Error> {
        self.state.capabilities.require(Capability::Clipboard)?;
        let serial = self
            .state
            .press_serial
            .ok_or(Error::PermissionDenied("a drag without a button held"))?;
        let qh = self.queue.handle();
        let origin = self.state.surface(origin)?.wl.clone();
        let manager = self
            .state
            .data_device_manager
            .clone()
            .expect("bound with the capability");
        let source = self.state.clipboard.start_drag(
            &manager,
            (&self.state.compositor, &self.state.shm),
            &origin,
            contents,
            actions,
            icon,
            serial,
            &qh,
        )?;
        self.flush()?;
        Ok(source)
    }

    /// Accept the drag `offer` in `mime`, one of its [`DragOffer::mime_types`], or refuse it with `None`; it may change
    /// until the drop, and only an accepted drop can be read.
    pub fn accept_drag(&mut self, offer: &DragOffer, mime: Option<&str>) -> Result<(), Error> {
        self.state.clipboard.accept_drag(offer, mime)
    }

    /// The `actions` this client takes for the drag `offer`, and the one it prefers; the compositor's choice arrives as
    /// [`DragEvent::Action`].
    pub fn set_drag_actions(
        &mut self,
        offer: &DragOffer,
        actions: DragActions,
        preferred: DragActions,
    ) -> Result<(), Error> {
        self.state
            .clipboard
            .set_drag_actions(offer, actions, preferred)
    }

    /// Start reading the drop of `offer` in `mime`, the type it was accepted in; the data arrives as
    /// [`Event::Received`], with the limits of [`Connection::receive_async`], except that a drop past the size limit
    /// gives `None` rather than its start. Before the drop, or in another type, it is refused.
    pub fn receive_drop(&mut self, offer: &DragOffer, mime: &str) -> Result<ReadId, Error> {
        self.state.clipboard.receive_drop(&self.conn, offer, mime)
    }

    /// Tell the dragging client the drop of `offer` was taken, after reading it.
    pub fn finish_drag(&mut self, offer: &DragOffer) -> Result<(), Error> {
        self.state.clipboard.finish_drag(offer)?;
        self.flush()
    }

    /// Let the drop of `offer` go without taking it, as when its data could not be read or used: the dragging client
    /// hears a cancel (from `wl_data_offer` version 3) and keeps its data. Before the drop it is refused.
    pub fn refuse_drop(&mut self, offer: &DragOffer) -> Result<(), Error> {
        self.state.clipboard.refuse_drop(offer)?;
        self.flush()
    }

    /// The queue extensions create their objects on.
    pub fn queue_handle(&self) -> QueueHandle<State> {
        self.queue.handle()
    }

    /// Bind the global of an extension's protocol at the highest version in `versions` the compositor offers, with
    /// `data` as user data, and list it in [`Connection::capabilities`]; `None` when it offers none in the range, so the
    /// application falls back. Binding through here is what makes an extension visible in the capabilities.
    pub fn bind_extension<I, U>(&mut self, versions: RangeInclusive<u32>, data: U) -> Option<I>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        State: Dispatch<I, U>,
    {
        let proxy: I = self
            .globals
            .bind(&self.queue.handle(), versions, data)
            .ok()?;
        self.state.capabilities.add_extension(Extension {
            interface: I::interface().name,
            version: proxy.version(),
        });
        Some(proxy)
    }

    /// The `wl_surface` of `surface`, for an extension request that names it.
    pub fn wl_surface(&self, surface: SurfaceId) -> Result<WlSurface, Error> {
        Ok(self.state.surface(surface)?.wl.clone())
    }

    /// The seat, when the compositor offers one.
    pub fn seat(&self) -> Option<WlSeat> {
        self.state.seat.clone()
    }

    /// The seat's `wl_pointer` once it has one, for an extension that follows the pointer.
    pub fn pointer(&self) -> Option<WlPointer> {
        self.state.pointer.clone()
    }

    /// The `wl_output` of `output`, for an extension request that names it.
    pub fn wl_output(&self, output: OutputId) -> Option<WlOutput> {
        self.state
            .outputs
            .iter()
            .find(|o| o.info.current.id == output)
            .map(|o| o.wl.clone())
    }

    /// Create a surface whose role an extension gives it: `assign` creates the role's objects for the new `wl_surface`
    /// (with the [`SurfaceId`] as user data where the protocol reports per surface) before its first commit. The
    /// extension reports each configure with [`State::configure_external`]; the surface then draws through
    /// [`Connection::lease`] like any other, with scale, frame callbacks and input.
    pub fn external_surface<R: ExternalRole + 'static>(
        &mut self,
        assign: impl FnOnce(&WlSurface, &QueueHandle<State>, SurfaceId) -> R,
    ) -> SurfaceId {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let role = Arc::new(assign(&wl, &qh, id));
        s.add_surface(id, wl, Role::External { role, size: (0, 0) }, &qh);
        id
    }

    /// Create a surface drawn as part of `parent` (a window, popup or external surface), above it, at its top left
    /// corner until [`Connection::place_subsurface`] moves it. It is synchronized: what it presents shows with the
    /// parent's next commit, so the two always change together. It takes no input (its input region is empty), so
    /// the pointer and touches over it reach the parent below, and it is drawn at the parent's scale; it gets no
    /// frame callbacks of its own. It shows nothing until it has a size and presents a buffer, and
    /// [`Connection::unmap_subsurface`] hides it again with the parent's next commit. Needs
    /// [`Capability::Subsurface`].
    pub fn subsurface(&mut self, parent: SurfaceId) -> Result<SurfaceId, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let subcompositor = s
            .subcompositor
            .clone()
            .ok_or(Error::Unsupported(Capability::Subsurface))?;
        let parent_wl = s.surface(parent)?.wl.clone();
        let id = s.next_id();
        let wl = s.compositor.create_surface(&qh, id);
        let subsurface = subcompositor.get_subsurface(&wl, &parent_wl, &qh, ());
        subsurface.set_sync();
        let region = s.compositor.create_region(&qh, ());
        wl.set_input_region(Some(&region));
        region.destroy();
        s.add_surface(
            id,
            wl,
            Role::Subsurface {
                subsurface,
                parent,
                size: (0, 0),
                position: (0, 0),
            },
            &qh,
        );
        Ok(id)
    }

    /// Move `surface`, a subsurface, to `position` in its parent's logical coordinates (with the parent's next commit)
    /// and draw its next buffers `size` logical points large.
    pub fn place_subsurface(
        &mut self,
        surface: SurfaceId,
        position: (i32, i32),
        size: (u32, u32),
    ) -> Result<(), Error> {
        let surf = self.state.surface_mut(surface)?;
        let Role::Subsurface {
            subsurface,
            size: current,
            position: at,
            ..
        } = &mut surf.role
        else {
            return Err(Error::Protocol(
                "place_subsurface on a surface that is not a subsurface",
            ));
        };
        if *at != position {
            subsurface.set_position(position.0, position.1);
            *at = position;
        }
        *current = size;
        Ok(())
    }

    /// Commit `surface`'s pending state without a new buffer: what an extension asked of it (a layer's animation)
    /// applies, and the buffer it shows stays. Nothing goes before the surface has shown a buffer.
    pub fn commit_state(&mut self, surface: SurfaceId) -> Result<(), Error> {
        let surf = self.state.surface_mut(surface)?;
        // after a commit another thread is sending, not in the middle of it
        let shared = commit::lock(&surf.shared);
        surf.wl.commit();
        drop(shared);
        self.flush()
    }

    /// Hide `surface`, a subsurface, with its parent's next commit; presenting a buffer shows it again.
    pub fn unmap_subsurface(&mut self, surface: SurfaceId) -> Result<(), Error> {
        let surf = self.state.surface_mut(surface)?;
        if !matches!(surf.role, Role::Subsurface { .. }) {
            return Err(Error::Protocol(
                "unmap_subsurface on a surface that is not a subsurface",
            ));
        }
        let shared = commit::lock(&surf.shared);
        surf.wl.attach(None, 0, 0);
        surf.wl.commit();
        drop(shared);
        // the buffers it showed are the compositor's until it releases them; what they hold is still known
        self.flush()
    }

    /// Send the queued requests and dispatch every event the compositor sent in answer, keeping the events for the
    /// next [`Connection::wait`]; for extensions that read state at setup.
    ///
    /// Each read is prepared before the sync goes out: `EventQueue::roundtrip` sends first, so another thread reading
    /// the connection for a queue of its own could take the answer in between and leave this one waiting on a socket
    /// that stays quiet.
    pub fn roundtrip(&mut self) -> Result<(), Error> {
        self.roundtrip_once().map_err(|e| self.explained(e))
    }

    pub(super) fn roundtrip_once(&mut self) -> Result<(), Error> {
        let done = Synced::default();
        self.conn.display().sync(&self.queue.handle(), done.clone());
        let mut woken = false;
        while !done.0.load(Ordering::SeqCst) {
            woken |= self.block(None)?.1;
            self.queue.dispatch_pending(&mut self.state)?;
        }
        // a wake read meanwhile is kept for the next wait
        if woken {
            self.waker.wake();
        }
        Ok(())
    }

    /// Whether [`Connection::lease`] has a buffer for `surface`: `false` while the compositor holds all of them, until
    /// it releases one.
    pub fn buffer_free(&self, surface: SurfaceId) -> Result<bool, Error> {
        let busy = self
            .state
            .surface(surface)?
            .slots
            .iter()
            .filter(|s| s.busy)
            .count();
        Ok(busy < MAX_SLOTS)
    }

    /// A buffer to draw the next frame of `surface` into, at the surface's scale. Its logical size is the configured
    /// one; `preferred` (logical points) applies when the compositor leaves the size to the client. The pixels are
    /// those of an earlier frame or zero; [`BufferLease::missed`] tells which differ from the frame shown last. Dropping
    /// the lease without presenting gives it back.
    pub fn lease(
        &mut self,
        surface: SurfaceId,
        preferred: (u32, u32),
        format: BufferFormat,
    ) -> Result<BufferLease<'_>, Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let shm = s.shm.clone();
        // a subsurface is drawn at its parent's scale and answers no configure
        let parent_scale = s.drawn_scale(surface)?;
        let surf = s.surface_mut(surface)?;
        let serial = match surf.role {
            Role::Subsurface { .. } => 0,
            _ => surf.serials().latest().ok_or(Error::NotConfigured)?,
        };
        let logical = logical_size(&surf.role, preferred)?;
        let scale = parent_scale.unwrap_or(surf.scale);
        let layout = scale
            .to_physical(logical.0)
            .zip(scale.to_physical(logical.1))
            .and_then(|(w, h)| BufferLayout::new(w, h, format))
            .ok_or(Error::InvalidScale)?;
        let (slot, created) =
            match pick_slot(surf.slots.iter().map(|s| (!s.free(), s.layout)), layout) {
                SlotChoice::Reuse(i) => (i, false),
                SlotChoice::AllBusy => return Err(Error::NoFreeBuffer),
                SlotChoice::Create => {
                    surf.slots.retain(|s| !s.free());
                    surf.next_slot += 1;
                    let tag = SlotTag {
                        surface: surface.0,
                        slot: surf.next_slot,
                    };
                    let memory = surf
                        .spare
                        .take()
                        .filter(|spare| spare.fits(layout))
                        .and_then(Spare::take);
                    surf.slots.push(Slot::new(&shm, &qh, tag, layout, memory)?);
                    (surf.slots.len() - 1, true)
                }
            };
        let animating = surf
            .presented_at
            .is_some_and(|at| at.elapsed() < ANIMATION_GAP);
        let of_layout = surf.slots.iter().filter(|s| s.layout == layout).count();
        let free = surf
            .slots
            .iter()
            .enumerate()
            .any(|(i, s)| i != slot && s.free() && s.layout == layout);
        let fitting = surf.spare.as_ref().is_some_and(|s| s.fits(layout));
        match spare_wanted(animating, created, of_layout, free, fitting) {
            Some(true) => surf.spare = Spare::start(layout),
            Some(false) => surf.spare = None,
            None => {}
        }
        Ok(BufferLease {
            connection: self,
            surface,
            slot,
            serial,
            logical,
            scale,
            zoom: 1.0,
        })
    }

    /// Show `surface` as one colour all over without drawing a buffer of its size: one pixel of `rgba` (straight, not
    /// premultiplied, sRGB components from 0 to 1; a transparent one for a surface that only takes input), which the
    /// compositor stretches over the surface's logical size: the configured one, or `preferred` (points) where the
    /// compositor leaves it to the client. Like a present it acknowledges the latest configure and asks for a frame
    /// callback; a pixel of the colour shown last is attached again rather than made anew. The surface's input region,
    /// outputs and scale are untouched, and a new scale needs no new present, the pixel being stretched in points. A
    /// buffer leased afterwards is drawn whole ([`BufferLease::missed`] is `None`). [`Error::Unsupported`] when the
    /// compositor lacks [`Capability::SolidColor`]: draw the colour into a leased buffer instead.
    pub fn present_solid(
        &mut self,
        surface: SurfaceId,
        preferred: (u32, u32),
        rgba: [f32; 4],
    ) -> Result<(), Error> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let (Some(pixels), Some(viewporter)) = (s.single_pixel.clone(), s.viewporter.clone())
        else {
            return Err(Error::Unsupported(Capability::SolidColor));
        };
        let surf = s.surface_mut(surface)?;
        // a subsurface is drawn into buffers at its parent's scale, with the parent's commit; it has no one-colour path
        if matches!(surf.role, Role::Subsurface { .. }) {
            return Err(Error::Protocol("present_solid on a subsurface"));
        }
        surf.serials().latest().ok_or(Error::NotConfigured)?;
        let logical = logical_size(&surf.role, preferred)?;
        if logical.0 == 0 || logical.1 == 0 {
            return Err(Error::InvalidScale);
        }
        s.drop_syncobj(surface)?;
        let surf = s.surface_mut(surface)?;
        // held until the commit is sent, as a commit from another thread holds it
        let shared = Arc::clone(&surf.shared);
        let mut shared = commit::lock(&shared);
        let acked = shared.serials.present()?;
        if let Some(serial) = acked {
            match &surf.role {
                Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => xdg.ack_configure(serial),
                Role::External { role, .. } => role.ack_configure(serial),
                Role::Subsurface { .. } => unreachable!("refused above"),
            }
        }
        let color = single_pixel(rgba);
        let replaced = match &surf.solid {
            Some((_, shown)) if *shown == color => None,
            _ => {
                let [r, g, b, a] = color;
                let buffer = pixels.create_u32_rgba_buffer(r, g, b, a, &qh, ());
                surf.solid.replace((buffer, color)).map(|(old, _)| old)
            }
        };
        let (buffer, _) = surf.solid.as_ref().expect("made above");
        surf.wl.attach(Some(buffer), 0, 0);
        let wl = &surf.wl;
        let viewport = surf
            .viewport
            .get_or_insert_with(|| viewporter.get_viewport(wl, &qh, ()));
        if shared.zoomed {
            viewport.set_source(-1.0, -1.0, -1.0, -1.0);
            shared.zoomed = false;
        }
        viewport.set_destination(clamp_i32(logical.0), clamp_i32(logical.1));
        surf.wl.set_buffer_scale(1);
        surf.wl.damage_buffer(0, 0, 1, 1);
        // the slots no longer hold what is shown
        for slot in &mut surf.slots {
            slot.missed.forget();
        }
        surf.wl.frame(&qh, surface);
        surf.wl.commit();
        drop(shared);
        // the pixel it replaced is no longer attached
        if let Some(old) = replaced {
            old.destroy();
        }
        self.flush()
    }

    /// Wait for events, at most `timeout` (`None` waits until one arrives or a [`Waker`] wakes the loop). It returns
    /// after each read from the compositor, maybe with no event, once everything read is dispatched, so extensions
    /// find what their objects received.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, Error> {
        self.wait_once(timeout).map_err(|e| self.explained(e))
    }

    pub(super) fn wait_once(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, Error> {
        // read what arrived while the caller was busy first: a key released meanwhile ends its repeat before the
        // repeats due since are counted
        let (read, woken) = self.block(Some(Duration::ZERO))?;
        self.queue.dispatch_pending(&mut self.state)?;
        self.state.repeat_keys(Instant::now());
        self.animate_cursor()?;
        self.state.pump_reads();
        self.update_scales()?;
        if self.state.events.is_empty() && self.state.fault.is_none() && !read && !woken {
            // a key repeat, the next image of an animated cursor or a clipboard read giving up earlier than the
            // caller's timeout ends the wait first
            let now = Instant::now();
            let timers = [
                self.state.repeat.deadline(),
                self.state.themed_cursor.deadline(),
                self.state.clipboard.read_deadline(),
            ];
            let until_timer = timers
                .into_iter()
                .flatten()
                .map(|at| at.saturating_duration_since(now));
            self.block(timeout.into_iter().chain(until_timer).min())?;
            self.queue.dispatch_pending(&mut self.state)?;
            self.state.repeat_keys(Instant::now());
            self.animate_cursor()?;
            self.state.pump_reads();
            self.update_scales()?;
        }
        match self.state.fault.take() {
            Some(e) => Err(e),
            None => Ok(std::mem::take(&mut self.state.events)),
        }
    }

    /// Update the scales of the surfaces; a drawn cursor follows the scale of the surface it is over.
    pub(super) fn update_scales(&mut self) -> Result<(), Error> {
        self.state.update_scales();
        let qh = self.queue.handle();
        let scale = self.state.cursor_scale();
        if self.state.themed_cursor.rescale(&qh, scale)? {
            self.flush()?;
        }
        Ok(())
    }

    pub(super) fn animate_cursor(&mut self) -> Result<(), Error> {
        if self.state.themed_cursor.advance(Instant::now()) {
            self.flush()?;
        }
        Ok(())
    }

    /// Send queued requests now; [`Connection::wait`] also does.
    pub fn flush(&self) -> Result<(), Error> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            // the socket is full; the next call flushes again
            Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(self.explained(e.into())),
        }
    }

    /// `error`, or the protocol error the compositor ended the connection with, when the socket failed before the
    /// client library read that error: a write into a socket the compositor reset fails before the error is read
    /// ([`crate::last_word`]).
    pub(super) fn explained(&self, error: Error) -> Error {
        let socket_failed = match &error {
            Error::Disconnected(
                Disconnect::Backend(WaylandError::Io(e))
                | Disconnect::Dispatch(DispatchError::Backend(WaylandError::Io(e))),
            ) => e.kind() != ErrorKind::WouldBlock,
            _ => false,
        };
        // only a dead connection's bytes are read here: the client library reads nothing more from it
        if !socket_failed || self.conn.backend().last_error().is_none() {
            return error;
        }
        match crate::last_word::unread_error(self.conn.backend().poll_fd()) {
            Some(protocol) => {
                Error::Disconnected(Disconnect::Backend(WaylandError::Protocol(protocol)))
            }
            None => error,
        }
    }

    /// Poll once; report dispatched or read events separately from a [`Waker`] waking the loop.
    pub(super) fn block(&mut self, timeout: Option<Duration>) -> Result<(bool, bool), Error> {
        let mut dispatched = 0;
        let guard = loop {
            match self.queue.prepare_read() {
                Some(guard) => break guard,
                None => {
                    dispatched += self.queue.dispatch_pending(&mut self.state)?;
                }
            }
        };
        // another thread reading the connection for a queue of its own may have read events for this queue since
        // the last dispatch; with the read prepared it reads no more without this one, so what is dispatched now is
        // all that arrived, and the wait does not block when it found something
        dispatched += self.queue.dispatch_pending(&mut self.state)?;
        let timeout = if dispatched == 0 {
            timeout
        } else {
            Some(Duration::ZERO)
        };
        self.flush()?;
        let timespec = timeout.map(|d| rustix::event::Timespec {
            tv_sec: d.as_secs() as i64,
            tv_nsec: i64::from(d.subsec_nanos()),
        });
        let (connection_ready, wake_ready) = {
            let connection_fd = guard.connection_fd();
            // a clipboard read's pipe ends the wait too, to be read with the next pump
            let reads: Vec<_> = self.state.clipboard.read_fds().collect();
            let mut fds: Vec<_> = [
                PollFd::new(&connection_fd, PollFlags::IN),
                PollFd::new(&self.wake, PollFlags::IN),
            ]
            .into_iter()
            .chain(reads.iter().map(|fd| PollFd::new(fd, PollFlags::IN)))
            .collect();
            match poll(&mut fds, timespec.as_ref()) {
                Ok(_) => {}
                // a signal interrupted the wait: report an empty wake-up
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(Error::Io(e.into())),
            }
            (
                fds[0]
                    .revents()
                    .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR),
                fds[1].revents().contains(PollFlags::IN),
            )
        };
        if connection_ready {
            match guard.read() {
                Ok(_) => {}
                // another reader consumed the data
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            drop(guard);
        }
        if wake_ready {
            let mut buf = [0u8; 64];
            loop {
                match (&self.wake).read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => return Err(Error::Io(e)),
                }
            }
        }
        Ok((connection_ready || dispatched > 0, wake_ready))
    }
}
