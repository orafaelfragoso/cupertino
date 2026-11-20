use super::*;

/// Scan the connectors of every card ([`scan_card`]).
pub(crate) fn scan_connectors(state: &mut Cupertino) {
    let nodes: Vec<DrmNode> =
        udev(state).map_or_else(Vec::new, |u| u.cards.keys().copied().collect());
    for node in nodes {
        scan_card(state, node);
    }
}

/// Bring the heads of a card in line with its connectors: every connected connector with a CRTC is driven, and nothing
/// else.
/// Smithay's scanner reports a connector once, when it connects; a connector that waited for a CRTC gets one later
/// without an event (another display let it go), and a display swapped on the same connector while the session was away
/// is no event either. So the heads are compared with the scanner's whole mapping, and a driven connector whose display
/// is another one (its EDID changed) is connected afresh.
pub(super) fn scan_card(state: &mut Cupertino, node: DrmNode) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    // a paused session cannot set a mode; its activation scans again. A session getting ready to be shown announces its
    // heads' outputs, and sets their modes when it is shown
    if u.paused && u.preparing.is_none() {
        return;
    }
    let paused = u.paused;
    let Some(u) = u.cards.get_mut(&node) else {
        return;
    };
    if let Err(e) = u.scanner.scan_connectors(u.manager.device()) {
        tracing::warn!(error = %e, "scanning connectors failed");
        return;
    }
    let mapped: Vec<(crtc::Handle, connector::Info)> = u
        .scanner
        .crtcs()
        .filter(|(info, _)| info.state() == connector::State::Connected)
        .map(|(info, crtc)| (crtc, info.clone()))
        .collect();
    let mut waiting = HashSet::new();
    for (handle, info) in u.scanner.connectors() {
        if info.state() == connector::State::Connected
            && u.scanner.crtc_for_connector(handle).is_none()
        {
            if !u.waiting.contains(handle) {
                tracing::warn!(connector = %format!("{}{}", u.prefix, connector_name(info)), "a display is connected but every CRTC is in use: it stays off until another display is unplugged");
            }
            waiting.insert(*handle);
        }
    }
    u.waiting = waiting;
    let device = u.manager.device();
    let driven: Vec<(crtc::Handle, connector::Handle)> = u
        .heads
        .iter()
        .filter(|(_, h)| {
            // an EDID that cannot be read now is no other display
            connector_edid(device, h.connector).is_none_or(|now| Some(now) == h.edid)
        })
        .map(|(crtc, h)| (*crtc, h.connector))
        .collect();
    let wanted: Vec<(crtc::Handle, connector::Handle)> = mapped
        .iter()
        .map(|(crtc, info)| (*crtc, info.handle()))
        .collect();
    let mut gone: Vec<crtc::Handle> = u
        .heads
        .keys()
        .filter(|crtc| !driven.iter().any(|(c, _)| c == *crtc))
        .copied()
        .collect();
    let (dropped, added) = reconcile(&driven, &wanted);
    gone.extend(dropped);
    for crtc in gone {
        let Some(u) = udev(state).and_then(|u| u.cards.get_mut(&node)) else {
            return;
        };
        u.surfaces.remove(&crtc);
        u.prepared.remove(&crtc);
        if let Some(head) = u.heads.remove(&crtc) {
            state.disconnect_head(head.head);
        }
    }
    for (crtc, connector) in added {
        if let Some((_, info)) = mapped
            .iter()
            .find(|(c, info)| *c == crtc && info.handle() == connector)
        {
            connector_connected(state, node, info.clone(), crtc);
        }
    }
    if !paused {
        if let Some(card) = udev(state).and_then(|u| u.cards.get_mut(&node)) {
            switch_off_strays(card);
        }
    }
}

/// Switch off every CRTC of `card` that shows something but drives no head of ours: what the session before (or the
/// console) left on a display this session does not drive, a display turned off here or one that waits for a CRTC. The
/// device is opened without blanking its displays, so this is what keeps them from showing an old picture.
fn switch_off_strays(card: &mut Card) {
    let device = card.manager.device();
    for crtc in device.crtcs().to_vec() {
        if card.surfaces.contains_key(&crtc) {
            continue;
        }
        let lit = device
            .get_crtc(crtc)
            .is_ok_and(|info| info.mode().is_some() || info.framebuffer().is_some());
        if !lit {
            continue;
        }
        match device.set_crtc(crtc, None, (0, 0), &[], None) {
            Ok(()) => tracing::info!(
                ?crtc,
                "switched off a CRTC that no display of this session uses"
            ),
            Err(e) => {
                tracing::warn!(?crtc, error = %e, "switching off a CRTC no display of this session uses failed")
            }
        }
    }
}

/// What a scan changes, from the (CRTC, connector) pairs driven and the pairs mapped now: the CRTCs whose head goes, then
/// the pairs to bring up.
fn reconcile<C: Copy + PartialEq, K: Copy + PartialEq>(
    driven: &[(C, K)],
    mapped: &[(C, K)],
) -> (Vec<C>, Vec<(C, K)>) {
    let gone = driven
        .iter()
        .filter(|pair| !mapped.contains(pair))
        .map(|(crtc, _)| *crtc)
        .collect();
    let new = mapped
        .iter()
        .filter(|pair| !driven.contains(pair))
        .copied()
        .collect();
    (gone, new)
}

fn connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    )
}

/// Refresh rate of a DRM mode in millihertz, from its pixel clock and totals (an interlaced mode
/// shows two fields per frame, a double-scanned one half; `vscan` repeats lines).
fn refresh_mhz(mode: &control::Mode) -> i32 {
    let (_, _, htotal) = mode.hsync();
    let (_, _, vtotal) = mode.vsync();
    if htotal == 0 || vtotal == 0 {
        return mode.vrefresh() as i32 * 1000;
    }
    let mut refresh = (u64::from(mode.clock()) * 1_000_000 / u64::from(htotal)
        + u64::from(vtotal) / 2)
        / u64::from(vtotal);
    let flags = mode.flags();
    if flags.contains(ModeFlags::INTERLACE) {
        refresh *= 2;
    }
    if flags.contains(ModeFlags::DBLSCAN) {
        refresh /= 2;
    }
    if mode.vscan() > 1 {
        refresh /= u64::from(mode.vscan());
    }
    refresh as i32
}

/// The EDID blob of a connector, when it has one.
fn connector_edid(device: &impl ControlDevice, connector: connector::Handle) -> Option<Vec<u8>> {
    let props = device.get_properties(connector).ok()?;
    let (handles, values) = props.as_props_and_values();
    handles.iter().zip(values).find_map(|(handle, value)| {
        let info = device.get_property(*handle).ok()?;
        if info.name().to_bytes() != b"EDID" {
            return None;
        }
        let blob = info
            .value_type()
            .convert_value(*value)
            .as_blob()
            .filter(|b| *b != 0)?;
        device.get_property_blob(blob).ok()
    })
}

fn connector_connected(
    state: &mut Cupertino,
    node: DrmNode,
    connector: connector::Info,
    crtc: crtc::Handle,
) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    let scale = u.scale;
    let Some(u) = u.cards.get_mut(&node) else {
        return;
    };
    let name = format!("{}{}", u.prefix, connector_name(&connector));
    let preferred = connector
        .modes()
        .iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    let mut modes: Vec<(HeadMode, control::Mode)> = Vec::new();
    for (i, m) in connector.modes().iter().enumerate() {
        let (w, h) = m.size();
        let mode = HeadMode {
            width: i32::from(w),
            height: i32::from(h),
            refresh_mhz: refresh_mhz(m),
        };
        if modes.iter().any(|(known, _)| *known == mode) {
            continue;
        }
        if i == preferred {
            modes.insert(0, (mode, *m));
        } else {
            modes.push((mode, *m));
        }
    }
    if modes.is_empty() {
        tracing::warn!(connector = %name, "no modes");
        return;
    }
    let edid = connector_edid(u.manager.device(), connector.handle());
    let identity = edid.as_deref().and_then(edid::parse);
    let (make, model, serial) = match identity {
        Some(id) => (id.make, id.model, id.serial),
        None => ("Unknown".to_string(), name.clone(), None),
    };
    let physical_mm = connector
        .size()
        .map_or((0, 0), |(w, h)| (w as i32, h as i32));
    let info = HeadInfo {
        connector: name,
        make,
        model,
        serial,
        physical_mm,
        modes: modes.iter().map(|(m, _)| *m).collect(),
        caps: HeadCaps {
            can_disable: true,
            custom_modes: false,
            transforms: true,
        },
        base_transform: Transform::Normal,
    };
    let (head, initial) = state.connect_head(info, scale);
    let Some(u) = udev(state).and_then(|u| u.cards.get_mut(&node)) else {
        return;
    };
    u.heads.insert(
        crtc,
        DrmHead {
            head,
            connector: connector.handle(),
            edid,
            modes,
        },
    );
    if initial.enabled {
        if let Err(e) = state.enable_head(head) {
            tracing::warn!(error = %e, "enabling the display failed");
        }
        state.heads_changed();
    }
}

/// Carry out `op` on a DRM head.
pub(crate) fn head_op(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    head: HeadId,
    op: HeadOp,
) -> Result<()> {
    let (node, crtc) = u.head_place(head).context("the head has no connector")?;
    let preparing = u.preparing.is_some();
    let u = u.cards.get_mut(&node).expect("the head's card");
    let drm_head = &u.heads[&crtc];
    match op {
        HeadOp::Enable => {
            let mode = state
                .displays
                .head(head)
                .context("the head being enabled")?
                .state
                .mode;
            let drm_mode = drm_head.drm_mode(mode)?;
            let connector = drm_head.connector;
            let output = state.prepare_head_output(head)?;
            if preparing {
                // the output is announced now, so the shell draws on it before the session is shown; its DRM output
                // waits for the activation ([`set_up_prepared`])
                let id = state.create_head_output(head, output.clone());
                tracing::info!(output = %output.name(), width = mode.width, height = mode.height, refresh_mhz = mode.refresh_mhz, "output announced; it is set up when the session is shown");
                u.prepared.insert(
                    crtc,
                    Prepared {
                        head,
                        output: id,
                        announced: output,
                        mode,
                        drm_mode,
                        connector,
                    },
                );
                return Ok(());
            }
            let drm_output = initialize_output(u, crtc, drm_mode, connector, &output)?;
            let id = state.create_head_output(head, output.clone());
            add_surface(state, u, crtc, id, drm_output, mode, &output);
            Ok(())
        }
        HeadOp::Disable => {
            // dropping the DRM output turns its CRTC off
            u.surfaces.remove(&crtc);
            u.prepared.remove(&crtc);
            state.drop_head_output(head);
            Ok(())
        }
        HeadOp::SetMode(mode) => {
            let drm_mode = drm_head.drm_mode(mode)?;
            if let Some(prepared) = u.prepared.get_mut(&crtc) {
                prepared.mode = mode;
                prepared.drm_mode = drm_mode;
                return Ok(());
            }
            let surface = u
                .surfaces
                .get_mut(&crtc)
                .context("the head is not enabled")?;
            // a frame waiting for its vblank would stay Smithay's pending frame across the mode change and hold every later
            // frame behind it, as after a pause; clearing the CRTC drops it, and the new mode starts with a full modeset
            if surface.pending {
                drop_pending_frame(surface, "the mode change");
            }
            let elements =
                DrmOutputRenderElements::<GlesRenderer, crate::render::CupertinoElement>::default();
            surface
                .drm_output
                .use_mode(drm_mode, &mut u.renderer, &elements)
                .map_err(|e| anyhow!("switching the mode: {e}"))?;
            surface.refresh = Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
            surface.pending = false;
            reset_frames(state, surface.output);
            Ok(())
        }
        // a display that is not shown yet has nothing to blank
        HeadOp::Power(_) if u.prepared.contains_key(&crtc) => Ok(()),
        HeadOp::Power(on) => {
            let surface = u
                .surfaces
                .get_mut(&crtc)
                .context("the head is not enabled")?;
            if !on {
                // blanks the CRTC; the next queued frame turns it on again
                surface
                    .drm_output
                    .with_compositor(|c| c.clear())
                    .map_err(|e| anyhow!("blanking the output: {e}"))?;
                surface.pending = false;
                reset_frames(state, surface.output);
            }
            Ok(())
        }
    }
}

/// Set up the DRM output of a display of `card` on `crtc` in `drm_mode`, for `output`. Nothing is committed: the first
/// frame queued sets the mode, which needs no modeset when the CRTC has it already.
fn initialize_output(
    card: &mut Card,
    crtc: crtc::Handle,
    drm_mode: control::Mode,
    connector: connector::Handle,
    output: &smithay::output::Output,
) -> Result<DrmSurfaceOutput> {
    let elements =
        DrmOutputRenderElements::<GlesRenderer, crate::render::CupertinoElement>::default();
    let mut planes = card.manager.device().planes(&crtc).ok();
    if let Some(p) = planes.as_mut() {
        // overlay planes misbehave on the proprietary NVIDIA driver
        if card
            .manager
            .device()
            .get_driver()
            .map(|d| d.name().to_string_lossy().to_lowercase().contains("nvidia"))
            .unwrap_or(false)
        {
            p.overlay = vec![];
        }
    }
    card.manager
        .initialize_output(
            crtc,
            drm_mode,
            &[connector],
            output,
            planes,
            &mut card.renderer,
            &elements,
        )
        .map_err(|e| {
            // a paused session is busy for every mode, as a device another program holds
            let paused = matches!(
                e,
                DrmOutputManagerError::Drm(DrmError::DeviceInactive)
                    | DrmOutputManagerError::Frame(FrameError::DrmError(DrmError::DeviceInactive))
            );
            let e = if paused {
                anyhow::Error::new(std::io::Error::new(ErrorKind::ResourceBusy, e))
            } else {
                anyhow::Error::new(e)
            };
            e.context(format!("initialising {}", output.name()))
        })
}

/// Drive the display on `crtc` of `card` with `drm_output`, the DRM output of the registered output `id`.
fn add_surface(
    state: &mut Cupertino,
    card: &mut Card,
    crtc: crtc::Handle,
    id: OutputId,
    drm_output: DrmSurfaceOutput,
    mode: HeadMode,
    output: &smithay::output::Output,
) {
    let refresh = Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
    let format = drm_output.format();
    card.surfaces.insert(
        crtc,
        Surface {
            output: id,
            drm_output,
            copy: None,
            late_buffers: Vec::new(),
            pending: false,
            refresh,
            latch: LateLatch::default(),
            target: None,
            in_flight: None,
        },
    );
    // a frame shrunk to the display's pixels is dithered at the display's depth
    if let Some(entry) = state.outputs.iter_mut().find(|o| o.id == id) {
        entry.cache.supersample.depth = crate::output_depth::format_bits(format);
    }
    tracing::info!(output = %output.name(), width = mode.width, height = mode.height, refresh_mhz = mode.refresh_mhz, format = %format, bits = crate::output_depth::format_bits(format), "output initialised");
}

/// The session is shown for the first time: the displays enabled while it got ready get their DRM outputs, and their first
/// frames follow at once. One that cannot be set up in its mode is enabled again the ordinary way, which tries smaller
/// modes.
pub(super) fn set_up_prepared(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    let mut again = Vec::new();
    if let Backend::Udev(u) = &mut backend {
        if let Some(preparing) = u.preparing.take() {
            tracing::info!(
                after = ?preparing.since.elapsed(),
                told = preparing.told,
                "the session is shown"
            );
        }
        for card in u.cards.values_mut() {
            for (crtc, prepared) in std::mem::take(&mut card.prepared) {
                match initialize_output(
                    card,
                    crtc,
                    prepared.drm_mode,
                    prepared.connector,
                    &prepared.announced,
                ) {
                    Ok(drm_output) => add_surface(
                        state,
                        card,
                        crtc,
                        prepared.output,
                        drm_output,
                        prepared.mode,
                        &prepared.announced,
                    ),
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "the display prepared in the background cannot be set up; enabling it again");
                        again.push(prepared.head);
                    }
                }
            }
        }
    }
    state.backend = Some(backend);
    if again.is_empty() {
        return;
    }
    for head in again {
        state.drop_head_output(head);
        if let Err(e) = state.enable_head(head) {
            tracing::warn!(error = %format!("{e:#}"), "enabling the display failed");
        }
    }
    state.heads_changed();
}

/// How long a session in the background waits for its wallpaper before it says it can be shown all the same (approximate,
/// ours: an image is read in a fraction of a second; the session manager stops waiting after 5 s).
const PREPARE_LIMIT: Duration = Duration::from_secs(3);
/// How often the loop of a session in the background looks whether the wallpaper is in, since no frame does.
pub(super) const PREPARE_POLL: Duration = Duration::from_millis(10);

/// A session in the background gets its first frame ready: its displays' scenes are composed until the wallpaper of each
/// is read, then each is drawn once off screen (its programs, the wallpaper's texture and the glass made), and the session
/// manager is told that the session can be shown. After [`PREPARE_LIMIT`] it is told all the same.
pub(super) fn prepare_first_frame(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    if let Backend::Udev(u) = &mut backend {
        if let Some(since) = u
            .preparing
            .as_ref()
            .filter(|preparing| !preparing.told)
            .map(|preparing| preparing.since)
        {
            let outputs: Vec<OutputId> = u
                .cards
                .values()
                .flat_map(|card| card.prepared.values().map(|p| p.output))
                .collect();
            let indices: Vec<usize> = outputs
                .iter()
                .filter_map(|id| state.outputs.iter().position(|o| o.id == *id))
                .collect();
            let renderer = u.renderer();
            // composing asks for the wallpaper and takes in what the reader has
            for idx in &indices {
                let _ = state.compose_output(renderer, *idx, false);
            }
            let read = !state.overlays.wallpaper.pending();
            let late = since.elapsed() >= PREPARE_LIMIT;
            if read || late {
                if read {
                    for idx in &indices {
                        if let Err(e) = state.draw_output_image(renderer, *idx, false, false) {
                            tracing::warn!(error = %format!("{e:#}"), "drawing the first frame off screen failed");
                        }
                    }
                } else {
                    tracing::warn!(limit = ?PREPARE_LIMIT, "the wallpaper is not read yet; the session can be shown all the same");
                }
                if let Some(preparing) = u.preparing.as_mut() {
                    preparing.told = true;
                }
                tracing::info!(after = ?since.elapsed(), "the first frame is ready");
                tell_prepared();
            }
        }
    }
    state.backend = Some(backend);
}

/// Tell the session manager that this session can be shown (`SessionPrepared` of `org.cupertino.LoginManager1` on the
/// system bus, the manager's interface), on a thread of its own, since the call waits for the manager. A manager that did
/// not start this session, or none at all (a session started from a text console), answers with an error, which is only
/// logged: such a session is shown by whatever switches to its terminal.
fn tell_prepared() {
    const MANAGER: &str = "org.cupertino.LoginManager1";
    const MANAGER_PATH: &str = "/org/cupertino/LoginManager1";
    let told = std::thread::Builder::new()
        .name("cupertino-prepared".to_owned())
        .spawn(|| {
            let answer = zbus::blocking::Connection::system().and_then(|bus| {
                bus.call_method(
                    Some(MANAGER),
                    MANAGER_PATH,
                    Some(MANAGER),
                    "SessionPrepared",
                    &(),
                )
                .map(drop)
            });
            match answer {
                Ok(()) => tracing::info!("the session manager was told that the session can be shown"),
                Err(e) => tracing::info!(error = %e, "the session manager was not told that the session can be shown"),
            }
        });
    if let Err(e) = told {
        tracing::warn!(error = %e, "cannot tell the session manager that the session can be shown");
    }
}

/// Let go of the frame `surface` queued before the pause, whose vblank went to the session that had the device: it counts
/// as shown, so the next frame is not held behind it, and the display keeps what it shows until that frame replaces it.
pub(super) fn retire_pending_frame(surface: &mut Surface) {
    surface.target = None;
    surface.in_flight = None;
    match surface.drm_output.frame_submitted() {
        Ok(_) => tracing::info!("let go of the frame queued before the pause"),
        Err(e) => {
            tracing::warn!(error = %e, "letting go of the frame queued before the pause failed")
        }
    }
}

/// Drop the frame waiting for its vblank on `surface` by clearing its CRTC, before `what`. The commit is blocking: a
/// driver that lost the flip may hold it until its own timeout (up to 3 s on nvidia-drm), so the time it took is
/// logged. The driver's timeout is the only deadline: a commit that does not block would need the flip's completion
/// handled as an event, which Smithay's DRM compositor does not offer for a clear and which needs testing on the drivers
/// that lose flips, so the wait stays and is bounded by the driver. Returns whether the frame is gone.
fn drop_pending_frame(surface: &mut Surface, what: &str) -> bool {
    let started = Instant::now();
    let cleared = surface.drm_output.with_compositor(|c| c.clear());
    let took = started.elapsed();
    match cleared {
        Ok(()) => {
            tracing::info!(?took, "dropped the frame queued before {what}");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, ?took, "dropping the frame queued before {what} failed");
            false
        }
    }
}

/// The flip in flight on `output` ended without its vblank: a late event for it is stale, and the next frame is full.
fn reset_frames(state: &mut Cupertino, output: OutputId) {
    if let Some(o) = state.outputs.iter_mut().find(|o| o.id == output) {
        o.scheduler.reset();
    }
}

pub(super) fn render_outputs(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    let mut unsupported = Vec::new();
    if let Backend::Udev(u) = &mut backend {
        if !(u.paused || u.asleep) {
            let loss = crate::gpu_reset::status(u.renderer())
                .or_else(|| state.gpu_reset.take().map(Loss::Reset))
                .or_else(|| {
                    u.renderer_lost
                        .then(|| Loss::Reset("the rendering context was lost".to_owned()))
                });
            if let Some(loss) = loss {
                if !recover_gpu(state, u, &loss) {
                    state.backend = Some(backend);
                    return;
                }
            }
        }
        let places: Vec<(DrmNode, crtc::Handle)> = u
            .cards
            .iter()
            .flat_map(|(node, card)| card.surfaces.keys().map(|crtc| (*node, *crtc)))
            .collect();
        let now = state.frame_now();
        for (node, crtc) in places {
            let Some(s) = u.cards.get(&node).and_then(|c| c.surfaces.get(&crtc)) else {
                continue;
            };
            let Some(idx) = state.outputs.iter().position(|o| o.id == s.output) else {
                continue;
            };
            if s.pending && !(u.paused || u.asleep) {
                let overdue = state.outputs[idx].scheduler.overdue(now, s.refresh);
                if let Some(overdue) = overdue {
                    flip_overdue(state, u, node, crtc, idx, overdue);
                }
                continue;
            }
            if !state.displays.powered(s.output)
                || !frame_due(
                    u.paused || u.asleep || u.renderer_lost,
                    s.pending,
                    state.frame_due(idx, now),
                )
            {
                continue;
            }
            unsupported.extend(render_surface(state, u, node, crtc, idx));
        }
    }
    state.backend = Some(backend);
    for head in unsupported {
        state.head_unsupported(head);
    }
}

/// The rendering context was lost (`loss`): every card gets a new context and renderer on its EGL display, its displays'
/// buffers and copies are made again, and so is every GPU object of the scene; client buffers are imported again by the
/// next frame. Whether the session goes on: too many resets in a row, or a renderer that cannot be made, end it.
fn recover_gpu(state: &mut Cupertino, u: &mut UdevBackend, loss: &Loss) -> bool {
    let reason = loss.reason();
    let gpu = u
        .render_node
        .dev_path()
        .map_or_else(|| u.render_node.to_string(), |p| p.display().to_string());
    let recovery = if state.gpu_resets.allow(loss, Instant::now()) {
        new_renderers(state, u).map_err(|e| format!("{e:#}"))
    } else {
        Err("the GPU was reset too often within a minute".to_owned())
    };
    crate::gpu_reset::record(&gpu, reason, &recovery);
    match recovery {
        Ok(()) => {
            u.renderer_lost = false;
            state.gpu_reset = None;
            true
        }
        Err(e) => {
            u.renderer_lost = true;
            state.end_for_lost_context(&format!("{reason}; {e}"));
            false
        }
    }
}

fn new_renderers(state: &mut Cupertino, u: &mut UdevBackend) -> Result<()> {
    for card in u.cards.values_mut() {
        let display = card.renderer.egl_context().display().clone();
        let context =
            EGLContext::new_with_reset_notification(&display, Some(ContextPriority::High))
                .map_err(|e| anyhow!("EGL context: {e}"))?;
        card.renderer = unsafe { super::super::gles_renderer(context) }
            .map_err(|e| anyhow!("GLES renderer: {e}"))?;
        for s in card.surfaces.values_mut() {
            s.copy = None;
            s.drm_output.reset_buffers();
        }
    }
    state.rebuild_render(u.renderer())
}

/// A flip whose vblank is overdue: its clients are called back once per refresh, so none waits on a dead head, and after
/// a second without the vblank the flip is dropped and the head starts over with a full frame. A head whose flip cannot
/// be dropped stays stuck with paced callbacks until a sleep, a resume, a mode change or an unplug.
fn flip_overdue(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
    overdue: Overdue,
) {
    let Some(surface) = u
        .cards
        .get_mut(&node)
        .and_then(|c| c.surfaces.get_mut(&crtc))
    else {
        return;
    };
    let name = state.outputs[idx].output.name();
    match overdue {
        Overdue::TimedOut => {
            tracing::warn!(output = %name, "the display has not shown the queued frame; calling its clients back once per refresh")
        }
        Overdue::CallBack => {}
        Overdue::GiveUp => {
            tracing::warn!(output = %name, "the display has not shown the queued frame for a second; dropping it");
            if drop_pending_frame(surface, "the head was given up") {
                surface.pending = false;
                state.outputs[idx].scheduler.reset();
                return;
            }
            tracing::error!(output = %name, "the head is stuck until it sleeps, resumes, changes mode or is unplugged");
        }
    }
    let time = state.frame_now();
    state.send_frames(idx, time);
}

/// Whether the frame already on the screen is what this frame would show, so it may be reported as shown: only when a
/// scene was composed and drawn and nothing differed from it. Without a scene nothing says what the screen shows, and
/// the session lock must not be confirmed by it.
fn already_shown(scene: bool, rendered: bool) -> bool {
    scene && !rendered
}

/// What drawing a frame of a display came to.
struct Drawn {
    /// A scene was composed and drawn, so the screen's content is known ([`already_shown`]).
    scene: bool,
    /// Something differs from the frame before: it waits to be queued.
    rendered: bool,
    failed: Option<RenderError>,
    /// The client buffers of the surfaces the frame was drawn from ([`crate::frame::drawn_buffers`]).
    buffers: Vec<Buffer>,
    /// What tells when the GPU finished the frame.
    fence: RenderFence,
    /// How the frame of a display of another card stands to the scene.
    copy: CopyFrame,
}

/// How the frame of a display of another card stands to the scene ([`copy::Brought`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyFrame {
    /// Drawn from the scene as it is, or not a copy.
    Current,
    /// The render GPU has not finished it: nothing is queued, and it is brought at the next refresh.
    Late,
    /// The frame that was late, now shown: the scene may have changed since it was composed, so it is drawn again.
    Stale,
}

impl Drawn {
    const NOTHING: Drawn = Drawn {
        scene: false,
        rendered: false,
        failed: None,
        buffers: Vec::new(),
        fence: RenderFence::WithinCpu,
        copy: CopyFrame::Current,
    };

    /// The outcome of a render of the scene of output `idx` from `buffers`.
    fn of(
        state: &mut Cupertino,
        idx: usize,
        result: Result<bool, RenderError>,
        buffers: Vec<Buffer>,
        lost: &mut bool,
    ) -> Self {
        state.outputs[idx].scheduler.rendered();
        match result {
            Ok(rendered) => {
                state.frame_drawn(idx);
                if !rendered {
                    state.outputs[idx].scheduler.empty();
                }
                Drawn {
                    scene: true,
                    rendered,
                    failed: None,
                    buffers,
                    fence: RenderFence::WithinCpu,
                    copy: CopyFrame::Current,
                }
            }
            Err(e) => {
                *lost |= e.failure() == Failure::ContextLost;
                Drawn {
                    failed: Some(e),
                    ..Drawn::NOTHING
                }
            }
        }
    }
}

/// Draw and queue a frame of output `idx` on the display at `crtc` of card `node`. Returns the head when its display
/// turned out unsupported: it cannot show the render GPU's frames.
fn render_surface(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<HeadId> {
    // the frame's render time runs from here to its render fence signalling (its CPU time, to its flip being queued),
    // and the vblank it is drawn for is the first after this moment
    let started = Instant::now();
    let started_at = Duration::from(state.clock.now());
    let _frame = tracing::trace_span!("server.output_frame", output = idx).entered();
    let target = {
        let refresh = state.refresh_of(idx);
        let now = state.frame_now();
        state.outputs[idx].scheduler.presentation_at(now, refresh)
    };
    let held = u.late_latch
        && u.cards
            .get(&node)
            .and_then(|c| c.surfaces.get(&crtc))
            .is_some_and(|s| !s.latch.delay(s.refresh).is_zero());
    // the wait from the vblank to this start, on the timeline beside the frames
    if held && state.fixed_time.is_none() {
        if let Some(vblank) = state.outputs[idx].scheduler.last_vblank() {
            let nanos = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
            cupertino_diagnostic_events::profile::interval(
                "server.latch_wait",
                nanos(vblank),
                nanos(started_at),
            );
        }
    }
    // without a scene (no render resources or no mode yet) or a drawn frame, nothing is queued, but the tail still runs
    let drawn = if node == u.render_node {
        draw_direct(state, u, node, crtc, idx)?
    } else {
        match draw_copy(state, u, node, crtc, idx)? {
            Ok(drawn) => drawn,
            Err(Unsupported) => {
                return u.cards[&node].heads.get(&crtc).map(|h| h.head);
            }
        }
    };
    let surface = u.cards.get_mut(&node)?.surfaces.get_mut(&crtc)?;
    let time = Duration::from(state.clock.now());
    if drawn.rendered {
        let feedback = state.collect_feedback(idx);
        let id = state.outputs[idx].scheduler.next_presentation();
        let name = state
            .drm_trace
            .is_some()
            .then(|| state.outputs[idx].output.name());
        let trace = state
            .drm_trace
            .as_mut()
            .zip(name)
            .map(|(t, name)| t.queued(&name));
        let frame = Frame {
            id,
            feedback,
            buffers: drawn.buffers,
            trace,
        };
        let queued = tracing::trace_span!("server.submit")
            .in_scope(|| surface.drm_output.queue_frame(frame));
        match queued {
            Ok(()) => {
                surface.pending = true;
                surface.in_flight = Some(InFlight {
                    started: started_at,
                    cpu: started.elapsed(),
                    fence: drawn.fence,
                    held,
                });
                surface.target = target;
                let locked = state.scene_built_locked(idx);
                let now = state.frame_now();
                state.outputs[idx].scheduler.flip_queued(now, locked);
            }
            // logind drops DRM master before it sends the pause, so a flip in between is refused; the pause follows
            Err(FrameError::DrmError(DrmError::Access(e)))
                if e.source.kind() == ErrorKind::PermissionDenied =>
            {
                tracing::debug!(error = %e, "the page flip was refused: the session is being paused")
            }
            Err(e) => {
                tracing::warn!(output = %state.outputs[idx].output.name(), error = %e, "queueing the frame failed")
            }
        }
    }
    let refresh = surface.refresh;
    if already_shown(drawn.scene, drawn.rendered) {
        state.note_frame_shown(idx);
    }
    state.process_screenshots(u.renderer(), idx);
    match drawn.failed {
        Some(e) => state.frame_failed(idx, time, refresh, &e),
        None => state.send_frames(idx, time),
    }
    match drawn.copy {
        CopyFrame::Current => {}
        CopyFrame::Late => state.outputs[idx].scheduler.not_ready(time, refresh),
        CopyFrame::Stale => state.outputs[idx].scheduler.damage(),
    }
    None
}

/// Draw a frame of a display of the render GPU: its DRM compositor draws the scene's elements.
fn draw_direct(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<Drawn> {
    let card = u.cards.get_mut(&node)?;
    let Some(composed) = state.compose_for_present(&mut card.renderer, idx) else {
        return Some(Drawn::NOTHING);
    };
    let surface = card.surfaces.get_mut(&crtc)?;
    let buffers = crate::frame::frame_buffers(&composed.resources);
    let (res, cache, _) = state.gles_parts(idx)?;
    let mut resources = GlesResources::new(res, cache, composed.resources);
    let result = Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Drm {
            renderer: &mut card.renderer,
            output: &mut surface.drm_output,
            clear: CLEAR,
        },
    );
    drop(resources);
    if let (Ok(GlesCompletion::Direct { planes, .. }), Some(trace)) =
        (&result, state.drm_trace.as_mut())
    {
        let name = state.outputs[idx].output.name();
        if let Some(line) = trace.planes(&name, *planes) {
            crate::drm_trace::log(&line);
        }
    }
    let fence = match &result {
        Ok(GlesCompletion::Direct { sync, .. }) => RenderFence::of(sync.as_ref()),
        _ => RenderFence::WithinCpu,
    };
    let result = result.map(|completion| completion.status() == RenderStatus::Rendered);
    let mut drawn = Drawn::of(state, idx, result, buffers, &mut u.renderer_lost);
    drawn.fence = fence;
    Some(drawn)
}

/// Draw a frame of a display of another card: the render GPU draws the scene into a buffer of the display's copy, and
/// the display's card draws that buffer ([`copy`]).
fn draw_copy(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<Result<Drawn, Unsupported>> {
    let [Some(render), Some(card)] = u.cards.get_disjoint_mut([&u.render_node, &node]) else {
        return None;
    };
    let output = &state.outputs[idx].output;
    let mode = output.current_mode()?;
    let size = output.current_transform().transform_size(mode.size);
    let scale = output.current_scale().fractional_scale();
    let surface = card.surfaces.get_mut(&crtc)?;
    let copy = match &mut surface.copy {
        Some(copy) if copy.fits(size, scale) => copy,
        slot => {
            let via = match slot {
                Some(copy) => copy.via(),
                None => copy::first_via(&render.renderer, &card.renderer),
            };
            let allocator = GbmAllocator::new(render.gbm.clone(), GbmBufferFlags::RENDERING);
            slot.insert(FrameCopy::new(allocator, size, scale, via))
        }
    };
    // a frame the render GPU had not finished goes before another is drawn
    if let Some(brought) = copy.bring_late(&mut render.renderer, &mut card.renderer) {
        let Ok(brought) = brought else {
            return Some(Err(Unsupported));
        };
        let buffers = std::mem::take(&mut surface.late_buffers);
        return Some(Ok(show_copy(
            state,
            idx,
            surface,
            &mut card.renderer,
            brought,
            buffers,
            &mut u.renderer_lost,
            CopyFrame::Stale,
        )));
    }
    surface.late_buffers.clear();
    let copy = surface.copy.as_mut()?;
    let mut target = match copy.acquire(&mut render.renderer) {
        Ok(acquired) => acquired,
        Err(_) if copy.via() == Via::Unsupported => return Some(Err(Unsupported)),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "no buffer to draw the display's frame into");
            return Some(Ok(Drawn::NOTHING));
        }
    };
    let Some(composed) = state.compose_for_present(&mut render.renderer, idx) else {
        return Some(Ok(Drawn::NOTHING));
    };
    let buffers = crate::frame::frame_buffers(&composed.resources);
    let (res, cache, _) = state.gles_parts(idx)?;
    let mut resources = GlesResources::new(res, cache, composed.resources);
    let result = match &mut target {
        copy::Target::Dmabuf { slot, dmabuf } => Gles.render(
            &composed.frame,
            &mut resources,
            GlesTarget::Dmabuf {
                renderer: &mut render.renderer,
                dmabuf,
                tracker: &mut copy.tracker,
                age: usize::from(slot.age()),
                clear: CLEAR,
            },
        ),
        copy::Target::Texture { texture, age } => Gles.render(
            &composed.frame,
            &mut resources,
            GlesTarget::Texture {
                renderer: &mut render.renderer,
                texture,
                tracker: &mut copy.tracker,
                age: *age,
                clear: CLEAR,
            },
        ),
    };
    let drawn = match result {
        Ok(GlesCompletion::Offscreen {
            damage: Some(damage),
            sync,
        }) => Ok(Some((damage, sync))),
        Ok(GlesCompletion::Drawn {
            damage: Some(damage),
        }) => Ok(Some((damage, SyncPoint::signaled()))),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    };
    let result = match drawn {
        Ok(Some((damage, sync))) => {
            let brought = match copy.bring(
                &mut render.renderer,
                &mut card.renderer,
                target,
                sync,
                &damage,
            ) {
                Ok(brought) => brought,
                Err(Unsupported) => return Some(Err(Unsupported)),
            };
            return Some(Ok(show_copy(
                state,
                idx,
                surface,
                &mut card.renderer,
                brought,
                buffers,
                &mut u.renderer_lost,
                CopyFrame::Current,
            )));
        }
        Ok(_) => Ok(false),
        // the render GPU cannot draw into the copy's buffer: the next frame tries another kind, or the display goes
        Err(RenderError::Target(e)) => {
            copy.cannot_draw(&e);
            if copy.via() == Via::Unsupported {
                return Some(Err(Unsupported));
            }
            Err(RenderError::Target(e))
        }
        Err(e) => Err(e),
    };
    Some(Ok(Drawn::of(
        state,
        idx,
        result,
        buffers,
        &mut u.renderer_lost,
    )))
}

/// Queue on `surface` the frame `brought` from the render GPU, drawn by its card's `renderer` from the client
/// `buffers`; a late frame keeps them until it is brought again. `copy` is how the frame stands to the scene.
#[allow(clippy::too_many_arguments)]
fn show_copy(
    state: &mut Cupertino,
    idx: usize,
    surface: &mut Surface,
    renderer: &mut GlesRenderer,
    brought: copy::Brought,
    buffers: Vec<Buffer>,
    lost: &mut bool,
    copy: CopyFrame,
) -> Drawn {
    let frame = match brought {
        copy::Brought::Frame(frame) => frame,
        copy::Brought::Late => {
            surface.late_buffers = buffers;
            return Drawn {
                copy: CopyFrame::Late,
                ..Drawn::NOTHING
            };
        }
    };
    let mut fence = RenderFence::WithinCpu;
    let result = surface
        .drm_output
        .render_frame(renderer, &[frame], CLEAR, FrameFlags::DEFAULT)
        .map(|r| {
            fence = RenderFence::of(super::super::primary_sync(&r.primary_element).as_ref());
            !r.is_empty
        })
        .map_err(|e| match e {
            RenderFrameError::PrepareFrame(e) => RenderError::Rendering(e.into()),
            RenderFrameError::RenderFrame(e) => RenderError::Target(format!("{e:?}")),
        });
    let mut drawn = Drawn::of(state, idx, result, buffers, lost);
    drawn.fence = fence;
    drawn.copy = copy;
    drawn
}

pub(super) fn frame_finish(
    state: &mut Cupertino,
    node: DrmNode,
    crtc: crtc::Handle,
    time: Option<DrmEventTime>,
    seq: Option<u32>,
) {
    let hardware = match time {
        Some(DrmEventTime::Monotonic(t)) if !t.is_zero() => Some(t),
        _ => None,
    };
    let flags = if hardware.is_some() {
        wp_presentation_feedback::Kind::Vsync
            | wp_presentation_feedback::Kind::HwClock
            | wp_presentation_feedback::Kind::HwCompletion
    } else {
        wp_presentation_feedback::Kind::Vsync
    };
    let clock = hardware.map_or_else(|| state.clock.now(), Into::into);
    let Some(u) = udev(state) else { return };
    let Some(surface) = u
        .cards
        .get_mut(&node)
        .and_then(|c| c.surfaces.get_mut(&crtc))
    else {
        return;
    };
    surface.pending = false;
    let (output, refresh) = (surface.output, surface.refresh);
    let target = surface.target.take();
    let in_flight = surface.in_flight.take();
    // `Ok(None)`: no frame of ours waited (it was dropped with its CRTC's state). An error comes after Smithay took the
    // shown frame, whose id goes with it, and failed to submit the frame parked behind it (queued while a dropped frame
    // was still pending): that is the flip in flight, and it never reaches the screen
    let (shown, failed) = match surface.drm_output.frame_submitted() {
        Ok(shown) => (shown, false),
        Err(e) => {
            tracing::warn!(error = %e, "frame submission failed");
            (None, true)
        }
    };
    let Some(idx) = state.outputs.iter().position(|o| o.id == output) else {
        return;
    };
    let scheduler = &mut state.outputs[idx].scheduler;
    if failed {
        scheduler.reset();
    }
    // only the flip in flight is shown: a late event of a flip dropped by a reset confirms no frame, a lock frame least
    // of all, and the refresh grid does not follow it; its feedback is discarded
    let presented = shown.as_ref().and_then(|f| scheduler.presented(f.id));
    // the late start needs the driver's timestamps on the presentation clock, which a test's fixed time is not
    let stamp = hardware.filter(|_| state.fixed_time.is_none());
    let late_latch = udev(state).is_some_and(|u| u.late_latch);
    // the fence has signalled by now, the flip having waited for it, so reading its time does not wait
    let frame = in_flight.map(|f| FrameTimes {
        started: f.started,
        cpu: f.cpu,
        gpu: f.fence.done(f.started, stamp),
        held: f.held,
    });
    let flipped = match (presented.is_some(), udev(state)) {
        (true, Some(u)) => u
            .cards
            .get_mut(&node)
            .and_then(|c| c.surfaces.get_mut(&crtc))
            .map(|s| s.latch.flipped(frame, stamp, target, refresh, late_latch)),
        _ => None,
    };
    if let (Some(presented), Some(trace)) = (presented.as_ref(), state.drm_trace.as_mut()) {
        let now = std::time::Instant::now();
        let flip = hardware.map_or(now, |t| {
            crate::drm_trace::instant_of(t, now, Duration::from(state.clock.now()))
        });
        let branch = if hardware.is_some() {
            crate::drm_trace::Branch::FlipTimestamp
        } else {
            crate::drm_trace::Branch::OwnClock
        };
        let frame = shown.as_ref().and_then(|f| f.trace).unwrap_or(0);
        let name = state.outputs[idx].output.name();
        let late = presented.late_periods.max(flipped.map_or(0, |f| f.late));
        for line in trace.flipped(&name, branch, late, frame, flip) {
            crate::drm_trace::log(&line);
        }
        let delay = flipped
            .and_then(|f| f.start_at)
            .zip(stamp)
            .map_or(Duration::ZERO, |(at, vblank)| at - vblank);
        if let Some(line) = trace.start_delay(&name, delay) {
            crate::drm_trace::log(&line);
        }
    }
    if let Some(presented) = presented {
        if let Some(Frame { mut feedback, .. }) = shown {
            feedback.presented(
                clock,
                Refresh::fixed(refresh),
                seq.unwrap_or(0) as u64,
                flags,
            );
        }
        if presented.late_periods > 0 {
            tracing::info!(
                late_periods = presented.late_periods,
                "the display showed the queued frame late"
            );
        }
        let vblank = match hardware {
            Some(t) if state.fixed_time.is_none() => t,
            _ => state.frame_now(),
        };
        state.outputs[idx].scheduler.vblank(vblank, refresh);
        state.outputs[idx]
            .scheduler
            .hold_until(flipped.and_then(|f| f.start_at));
        state.note_frame_presented(idx, presented.locked);
    }
    state.outputs[idx].scheduler.damage();
}
