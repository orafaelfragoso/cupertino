use super::*;

/// A buffer of a surface leased for drawing one frame; [`BufferLease::present`] commits it.
pub struct BufferLease<'a> {
    pub(super) connection: &'a mut Connection,
    pub(super) surface: SurfaceId,
    pub(super) slot: usize,
    pub(super) serial: u32,
    pub(super) logical: (u32, u32),
    pub(super) scale: Scale,
    pub(super) zoom: f64,
}

impl<'a> BufferLease<'a> {
    /// Show the frame magnified `zoom` times (at least 1) about its centre, scaled by the compositor: the viewport shows
    /// the middle `1 / zoom` of the buffer over the whole surface. `false` when the surface has no viewport (the
    /// compositor lacks `wp_viewporter` or fractional scaling), and the frame shows unmagnified.
    pub fn set_zoom(&mut self, zoom: f64) -> bool {
        self.zoom = zoom.max(1.0);
        self.connection
            .state
            .surface(self.surface)
            .is_ok_and(|s| s.viewport.is_some())
    }

    pub fn layout(&self) -> BufferLayout {
        self.slot_ref().layout
    }

    /// The size of the frame in logical points.
    pub fn logical_size(&self) -> (u32, u32) {
        self.logical
    }

    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// The configure this frame answers.
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// The rectangles in which this buffer differs from the frame shown last: what the frames shown in the other
    /// buffers since it last held one changed. Drawing the next frame needs only them and its own damage. `None` when
    /// its pixels are unknown, such as a new buffer, one of another size or one written but not presented: draw all of
    /// it.
    pub fn missed(&self) -> Option<&[PixelRect]> {
        self.slot_ref().missed.rects()
    }

    /// The pixels to draw the frame into. What [`BufferLease::missed`] says holds until the lease is presented.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        let (surface, slot) = (self.surface, self.slot);
        let slot = &mut self
            .connection
            .state
            .surface_mut(surface)
            .expect("a leased surface lives while the lease does")
            .slots[slot];
        slot.missed.forget();
        slot.pixels_mut()
    }

    pub(super) fn slot_ref(&self) -> &Slot {
        &self
            .connection
            .state
            .surface(self.surface)
            .expect("a leased surface lives while the lease does")
            .slots[self.slot]
    }

    /// Show the whole buffer.
    pub fn present(self) -> Result<(), Error> {
        self.present_damage(None)
    }

    /// Show the buffer, telling the compositor only `damage` changed since the frame before; `None` for all of it. The
    /// lease holds the connection, so no configure arrives between the lease and the present: the buffer answers the
    /// latest one, which is acknowledged with it.
    pub fn present_damage(self, damage: Option<&[PixelRect]>) -> Result<(), Error> {
        let (connection, buffer, commit) = self.split()?;
        commit.commit(damage)?;
        connection.buffer_shown(buffer, damage);
        Ok(())
    }

    /// The buffer taken out of the connection, to be drawn into and committed on other threads: its pixels (with
    /// what it missed, as [`BufferLease::missed`] says) and the [`Commit`] that shows it, which answers the configure
    /// of [`BufferLease::serial`]. The buffer counts as held by the compositor until it is given back, with
    /// [`Connection::buffer_shown`] once the commit is sent or [`Connection::buffer_unused`] when it is not; meanwhile
    /// the surface draws into its other buffers.
    pub fn detach(self) -> Result<(DetachedBuffer, Commit), Error> {
        let (_, buffer, commit) = self.split()?;
        Ok((buffer, commit))
    }

    pub(super) fn split(self) -> Result<(&'a mut Connection, DetachedBuffer, Commit), Error> {
        let qh = self.connection.queue.handle();
        let presentation = self.connection.state.presentation.clone();
        let conn = self.connection.conn.clone();
        // a commit without points on a surface with a syncobj object is a protocol error
        self.connection.state.drop_syncobj(self.surface)?;
        let surf = self.connection.state.surface_mut(self.surface)?;
        let ack = surf.answers()?.map(|(ack, _)| (ack, self.serial));
        let slot = &surf.slots[self.slot];
        let layout = slot.layout;
        let source = (self.zoom > 1.0 && surf.viewport.is_some()).then(|| {
            let (w, h) = (f64::from(layout.width()), f64::from(layout.height()));
            let (cw, ch) = (w / self.zoom, h / self.zoom);
            ((w - cw) / 2.0, (h - ch) / 2.0, cw, ch)
        });
        let scaling = surf.scaling(self.logical, self.scale, source)?;
        surf.presented_at = Some(Instant::now());
        let slot = &mut surf.slots[self.slot];
        let missed = slot.missed.rects().map(<[PixelRect]>::to_vec);
        slot.missed.forget();
        slot.busy = true;
        let map = slot.take_map();
        let commit = Commit {
            surface: self.surface,
            wl: surf.wl.clone(),
            shared: Arc::clone(&surf.shared),
            ack,
            buffer: slot.buffer.clone(),
            layout: (layout.width(), layout.height()),
            scaling,
            sync: None,
            qh,
            presentation,
            conn,
        };
        let buffer = DetachedBuffer {
            surface: self.surface,
            tag: slot.tag,
            layout,
            map,
            missed,
        };
        Ok((self.connection, buffer, commit))
    }
}

/// A buffer of a surface taken out of the connection to be drawn into on another thread ([`BufferLease::detach`]).
pub struct DetachedBuffer {
    surface: SurfaceId,
    tag: SlotTag,
    layout: BufferLayout,
    map: memmap2::MmapMut,
    missed: Option<Vec<PixelRect>>,
}

impl DetachedBuffer {
    /// The buffer's size and format, as its lease had them.
    pub fn layout(&self) -> BufferLayout {
        self.layout
    }

    /// What the buffer differs in from the frame shown last, as [`BufferLease::missed`] said.
    pub fn missed(&self) -> Option<&[PixelRect]> {
        self.missed.as_deref()
    }

    /// The pixels to draw the frame into, from any thread; [`Commit::commit`](crate::Commit::commit) shows them, and
    /// [`Connection::buffer_shown`] or [`Connection::buffer_unused`] gives the buffer back.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.map[..]
    }
}

impl Connection {
    /// `buffer` back after its commit was sent with `damage` (`None` for all of it): the other buffers of its size
    /// miss what it changed.
    pub fn buffer_shown(&mut self, buffer: DetachedBuffer, damage: Option<&[PixelRect]>) {
        let layout = buffer.layout;
        let Some(surf) = self.give_back(buffer) else {
            return;
        };
        let damage: Option<Vec<PixelRect>> =
            damage.map(|rects| rects.iter().filter_map(|r| r.clamp(layout)).collect());
        for (slot, given) in surf {
            if given {
                slot.missed.shown();
            } else if slot.layout == layout {
                slot.missed.passed(damage.as_deref());
            } else {
                slot.missed.forget();
            }
        }
    }

    /// `buffer` back unshown: its commit was never sent, and its pixels are unknown.
    pub fn buffer_unused(&mut self, buffer: DetachedBuffer) {
        if let Some(slots) = self.give_back(buffer) {
            for (slot, given) in slots {
                if given {
                    slot.busy = false;
                }
            }
        }
    }

    /// Puts `buffer`'s memory back in its slot: the surface's slots, each with whether it is the one given back.
    /// `None` when the surface or the slot is gone.
    pub(super) fn give_back(
        &mut self,
        buffer: DetachedBuffer,
    ) -> Option<impl Iterator<Item = (&mut Slot, bool)>> {
        let surf = self.state.surface_mut(buffer.surface).ok()?;
        let slot = surf.slots.iter_mut().find(|s| s.tag == buffer.tag)?;
        slot.put_map(buffer.map);
        let tag = buffer.tag;
        Some(surf.slots.iter_mut().map(move |s| {
            let given = s.tag == tag;
            (s, given)
        }))
    }
}

/// A positioner for `placement`; its anchor rectangle is rounded outwards to whole points. A reactive or resizing
/// placement names the parent's configure not yet acknowledged, from `parent`, which it answers.
pub(super) fn positioner(
    wm_base: &XdgWmBase,
    qh: &QueueHandle<State>,
    placement: &PopupPlacement,
    parent: Serials,
) -> Result<XdgPositioner, Error> {
    const REACTIVE_VERSION: u32 = 3;
    let r = placement.anchor_rect;
    if !(r.width() > 0.0 && r.height() > 0.0 && placement.size.0 > 0 && placement.size.1 > 0) {
        return Err(Error::Protocol(
            "a popup needs a non-empty anchor rectangle and size",
        ));
    }
    let positioner = wm_base.create_positioner(qh, ());
    positioner.set_size(clamp_i32(placement.size.0), clamp_i32(placement.size.1));
    let (x, y) = (r.x().floor() as i32, r.y().floor() as i32);
    positioner.set_anchor_rect(
        x,
        y,
        ((r.x() + r.width()).ceil() as i32 - x).max(1),
        ((r.y() + r.height()).ceil() as i32 - y).max(1),
    );
    positioner.set_anchor(placement.anchor);
    positioner.set_gravity(placement.gravity);
    positioner.set_constraint_adjustment(placement.constraint_adjustment);
    if placement.offset != (0, 0) {
        positioner.set_offset(placement.offset.0, placement.offset.1);
    }
    if wm_base.version() >= REACTIVE_VERSION {
        if placement.reactive {
            positioner.set_reactive();
        }
        if let Some((w, h)) = placement.parent_size {
            positioner.set_parent_size(clamp_i32(w), clamp_i32(h));
        }
        if placement.reactive || placement.parent_size.is_some() {
            if let Some(serial) = parent.pending() {
                positioner.set_parent_configure(serial);
            }
        }
    }
    Ok(positioner)
}

pub(super) fn decoration_mode(decorations: Decorations) -> zxdg_toplevel_decoration_v1::Mode {
    match decorations {
        Decorations::Server => zxdg_toplevel_decoration_v1::Mode::ServerSide,
        Decorations::Client => zxdg_toplevel_decoration_v1::Mode::ClientSide,
    }
}

/// The logical size a frame of a surface with `role` takes: the configured one, or `preferred` (points) where the
/// compositor leaves it to the client.
pub(super) fn logical_size(role: &Role, preferred: (u32, u32)) -> Result<(u32, u32), Error> {
    Ok(match role {
        Role::Toplevel { current, .. } => current.and_then(|c| c.size).unwrap_or(preferred),
        Role::Popup { current, .. } => current
            .map(|c| (c.width, c.height))
            .ok_or(Error::NotConfigured)?,
        Role::External { size: (w, h), .. } => (
            if *w == 0 { preferred.0 } else { *w },
            if *h == 0 { preferred.1 } else { *h },
        ),
        Role::Subsurface { size, .. } if size.0 > 0 && size.1 > 0 => *size,
        Role::Subsurface { .. } => return Err(Error::NotConfigured),
    })
}

/// `rgba` (straight sRGB components from 0 to 1) as a one-pixel buffer takes it: premultiplied by its alpha, each
/// component scaled to the whole `u32` range.
pub(super) fn single_pixel(rgba: [f32; 4]) -> [u32; 4] {
    let alpha = f64::from(rgba[3]).clamp(0.0, 1.0);
    let wide = |c: f64| (c.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32;
    [
        wide(f64::from(rgba[0]) * alpha),
        wide(f64::from(rgba[1]) * alpha),
        wide(f64::from(rgba[2]) * alpha),
        wide(alpha),
    ]
}

pub(super) fn clamp_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// `r` as whole points `(x, y, width, height)`, rounded outwards and saturated.
pub(crate) fn round_out(r: &Rect) -> (i32, i32, i32, i32) {
    let (left, top) = (r.min_x().floor() as i32, r.min_y().floor() as i32);
    let (right, bottom) = (r.max_x().ceil() as i32, r.max_y().ceil() as i32);
    (
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

/// The whole points inside `r`: its edges rounded inwards, as `(x, y, width, height)`.
pub(crate) fn round_in(r: &Rect) -> (i32, i32, i32, i32) {
    let (left, top) = (r.min_x().ceil() as i32, r.min_y().ceil() as i32);
    let (right, bottom) = (r.max_x().floor() as i32, r.max_y().floor() as i32);
    (
        left,
        top,
        right.saturating_sub(left).max(0),
        bottom.saturating_sub(top).max(0),
    )
}

impl State {
    /// Destroy the syncobj object of `surface`, if it has one, before it shows anything but a dmabuf with points.
    pub(super) fn drop_syncobj(&mut self, surface: SurfaceId) -> Result<(), Error> {
        let State { surfaces, gpu, .. } = self;
        let surf = surfaces
            .iter_mut()
            .find(|s| s.id == surface)
            .ok_or(Error::UnknownSurface)?;
        surf.drop_syncobj(gpu);
        Ok(())
    }

    /// Record a configure of an [`Connection::external_surface`]: its `serial`, acknowledged with the next present, and
    /// its logical `size`, where a 0 width or height leaves that dimension to the client. A serial that does not
    /// increase, or a surface that is not external, ends the next [`Connection::wait`] with the error.
    pub fn configure_external(&mut self, surface: SurfaceId, serial: u32, size: (u32, u32)) {
        let surf = match self.surface_mut(surface) {
            Ok(surf) => surf,
            // the application destroyed the surface while the configure was on its way
            Err(_) => return,
        };
        let Role::External { size: current, .. } = &mut surf.role else {
            return self.fail(Error::Protocol(
                "configure_external on a surface without an external role",
            ));
        };
        *current = size;
        let received = commit::lock(&surf.shared).serials.receive(serial);
        if let Err(e) = received {
            self.fail(e);
        }
    }

    /// The output an extension event names, as the application knows it; `None` for one this connection has not bound.
    pub fn output_id(&self, output: &WlOutput) -> Option<OutputId> {
        self.outputs
            .iter()
            .find(|o| o.wl == *output)
            .map(|o| o.info.current.id)
    }

    /// Queue `event` for the application as an [`Event::Extension`], after the events dispatched before it.
    pub fn emit(&mut self, event: impl Any + Send + Sync) {
        self.events
            .push(Event::Extension(ExtensionEvent(Arc::new(event))));
    }

    /// End the next [`Connection::wait`] with `error`, such as [`Error::Protocol`] for an event the extension cannot
    /// accept; the first error wins.
    pub fn fail(&mut self, error: Error) {
        self.fault.get_or_insert(error);
    }

    pub(super) fn next_id(&mut self) -> SurfaceId {
        self.next_surface += 1;
        SurfaceId(self.next_surface)
    }

    pub(super) fn add_surface(
        &mut self,
        id: SurfaceId,
        wl: WlSurface,
        role: Role,
        qh: &QueueHandle<State>,
    ) {
        let (viewport, fractional) = match &self.scaling {
            Some((manager, viewporter)) => (
                Some(viewporter.get_viewport(&wl, qh, ())),
                Some(manager.get_fractional_scale(&wl, qh, id)),
            ),
            None => (None, None),
        };
        let configured = match &role {
            Role::External { role, .. } => role.initial_commit(),
            Role::Subsurface { .. } => false,
            Role::Toplevel { .. } | Role::Popup { .. } => true,
        };
        // a role the compositor never configures presents at once
        let mut serials = Serials::default();
        if matches!(&role, Role::External { role, .. } if role.drawn_with().is_some()) {
            let _ = serials.receive(0);
        }
        if configured {
            // the initial commit without a buffer asks for the first configure
            wl.commit();
        }
        self.surfaces.push(Surface {
            id,
            wl,
            role,
            viewport,
            solid: None,
            fractional,
            fractional_scale: None,
            preferred_scale: None,
            entered: Vec::new(),
            scale: Scale::ONE,
            shared: Arc::new(std::sync::Mutex::new(commit::Shared {
                serials,
                gone: false,
                zoomed: false,
            })),
            slots: Vec::new(),
            next_slot: 0,
            spare: None,
            presented_at: None,
            syncobj: None,
        });
    }

    /// The scale of the surface `surface` is drawn with, when it is drawn at another's: a subsurface's parent's, or
    /// that of the surface an external role names ([`ExternalRole::drawn_with`]).
    pub(crate) fn drawn_scale(&self, surface: SurfaceId) -> Result<Option<Scale>, Error> {
        Ok(match &self.surface(surface)?.role {
            Role::Subsurface { parent, .. } => Some(self.surface(*parent)?.scale),
            Role::External { role, .. } => match role.drawn_with() {
                Some(other) => Some(self.surface(other)?.scale),
                None => None,
            },
            _ => None,
        })
    }

    pub(crate) fn has_surface(&self, id: SurfaceId) -> bool {
        self.surface(id).is_ok()
    }

    pub(super) fn surface(&self, id: SurfaceId) -> Result<&Surface, Error> {
        self.surfaces
            .iter()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownSurface)
    }

    pub(super) fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface, Error> {
        self.surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownSurface)
    }

    pub(super) fn toplevel(&self, window: Toplevel) -> Result<&XdgToplevel, Error> {
        match &self.surface(window.0)?.role {
            Role::Toplevel { toplevel, .. } => Ok(toplevel),
            Role::Popup { .. } | Role::External { .. } | Role::Subsurface { .. } => {
                unreachable!("a Toplevel handle names a toplevel")
            }
        }
    }

    pub(super) fn bind_output(
        &mut self,
        registry: &WlRegistry,
        qh: &QueueHandle<State>,
        name: u32,
        version: u32,
    ) {
        let id = OutputId(name);
        // version 4 adds the name and description
        let wl = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, id);
        let xdg = self
            .output_manager
            .as_ref()
            .map(|m| m.get_xdg_output(&wl, qh, id));
        self.outputs.push(Output {
            registry_name: name,
            wl,
            xdg,
            info: OutputBuilder::new(id),
        });
    }

    /// The whole scale a drawn cursor is made for: the next whole scale of the surface under the pointer, else 1.
    pub(super) fn cursor_scale(&self) -> u32 {
        self.pointer_tracker
            .focus()
            .and_then(|id| self.surfaces.iter().find(|surf| surf.id == id))
            .and_then(|surf| surf.scale.ceil().integer())
            .map_or(1, i32::unsigned_abs)
    }

    /// Report surfaces whose scale changed since they were last drawn.
    pub(super) fn update_scales(&mut self) {
        let fractional = self.scaling.is_some();
        for surf in &mut self.surfaces {
            let sources = ScaleSources {
                fractional: surf.fractional_scale,
                preferred: surf.preferred_scale,
                entered: surf
                    .entered
                    .iter()
                    .filter_map(|id| self.outputs.iter().find(|o| o.info.current.id == *id))
                    .map(|o| o.info.current.scale)
                    .collect(),
            };
            let scale = sources.resolve(fractional);
            // a subsurface is drawn at its parent's scale: the parent's change is the one reported
            if matches!(surf.role, Role::Subsurface { .. }) {
                surf.scale = scale;
                continue;
            }
            if scale != surf.scale {
                surf.scale = scale;
                self.events.push(Event::ScaleChanged {
                    surface: surf.id,
                    scale,
                });
            }
        }
    }

    pub(super) fn pump_reads(&mut self) {
        self.clipboard.pump(Instant::now(), &mut self.events);
    }

    pub(super) fn repeat_keys(&mut self, now: Instant) {
        let (Some(surface), Some(t)) = (self.key_focus, &self.translator) else {
            return;
        };
        if let Some((code, count)) = self.repeat.due(now) {
            let k = t.translate(code);
            let time = self.input_time(now);
            for _ in 0..count {
                self.events.push(Event::Keyboard {
                    surface,
                    time,
                    event: KeyboardEvent::Key {
                        code,
                        pressed: true,
                        repeat: true,
                        keysym: k.keysym,
                        character: k.character,
                        mods: k.mods,
                    },
                });
            }
        }
    }

    pub(super) fn note_time(&mut self, time: u32) {
        self.input_clock.note(time, Instant::now());
    }

    pub(super) fn input_time(&self, now: Instant) -> u32 {
        self.input_clock.at(now)
    }

    pub(crate) fn surface_of(&self, wl: &WlSurface) -> Option<SurfaceId> {
        wl.data::<SurfaceId>().copied()
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: ignore WlShmPool);
