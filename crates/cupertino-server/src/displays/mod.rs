//! Displays: the heads (connected display devices) the backends found, the state each one is in,
//! and the arrangement the user gave them, stored in `displays.conf` (`cupertino_defaults::displays`).
//! An enabled head has an output; a disabled head stays listed for output-management clients.
//!
//! Backends report heads with [`Cupertino::connect_head`] and [`Cupertino::disconnect_head`] and
//! carry out the [`HeadOp`]s asked of them. The output-management protocol (`management`) changes
//! heads through [`Cupertino::configure_heads`], which validates, applies, reverts on failure and
//! stores the result. A new head starts in its stored state, else at its preferred mode, the scale
//! of `cupertino_displays::default_choice` and to the right of the other heads.

pub use cupertino_server_backend::displays::{
    another_mode_may_help, check_head, default_scale, edid, exact_choice_scale, fallback_modes,
    from_stored_transform, panel_of, state_area, stored_settings, stored_transform, Head, HeadCaps,
    HeadId, HeadInfo, HeadMode, HeadOp, HeadState,
};

pub mod management;
pub mod placement;
pub mod power;
pub mod recovery;
pub mod service;
pub mod surface_output;

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use cupertino_defaults::displays::{display_key, DisplaysConfig};
use cupertino_displays::arrangement::{self, Area, ArrangementError};
use cupertino_wm::OutputId;
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Transform;

use crate::backend::Backend;
use crate::state::{new_output, output_rect, Cupertino};

/// The heads and the globals of the output-management protocols.
pub struct Displays {
    pub heads: Vec<Head>,
    next_head: u64,
    /// Configuration serial: changes whenever a head appears, goes away or changes.
    pub serial: u32,
    /// Where the settings are stored; `None` keeps them in memory only (offscreen runs and tests).
    pub store: Option<PathBuf>,
    pub saved: DisplaysConfig,
    /// Something changed that output-management clients have not been sent `done` for.
    dirty: bool,
    /// Windows that moved off a display that went away, until it comes back (`recovery`).
    pub departed: Vec<recovery::Departure>,
    pub(crate) managers: Vec<management::Manager>,
    pub(crate) powers: Vec<power::PowerObject>,
    pub(crate) bus_signals: Option<async_channel::Sender<service::DisplaysBusOut>>,
    _management: GlobalId,
    _power: GlobalId,
}

impl Displays {
    pub fn new(dh: &DisplayHandle) -> Displays {
        Displays {
            heads: Vec::new(),
            next_head: 1,
            serial: 1,
            store: None,
            saved: DisplaysConfig::default(),
            dirty: false,
            departed: Vec::new(),
            managers: Vec::new(),
            powers: Vec::new(),
            bus_signals: None,
            _management: management::create_global(dh),
            _power: power::create_global(dh),
        }
    }

    /// Store the settings in `path` and read what it holds. Problems in the file are logged and
    /// the sections they are in are ignored; a file that cannot be read is an error.
    pub fn use_store(&mut self, path: PathBuf) -> Result<()> {
        let (saved, problems) =
            DisplaysConfig::load(&path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
        for p in problems {
            tracing::warn!(path = %path.display(), line = p.line, problem = %p.problem, "ignoring display setting");
        }
        self.saved = saved;
        self.store = Some(path);
        Ok(())
    }

    pub fn head(&self, id: HeadId) -> Option<&Head> {
        self.heads.iter().find(|h| h.id == id)
    }

    fn head_mut(&mut self, id: HeadId) -> Option<&mut Head> {
        self.heads.iter_mut().find(|h| h.id == id)
    }

    pub fn head_of_output(&self, output: OutputId) -> Option<&Head> {
        self.heads.iter().find(|h| h.output == Some(output))
    }

    /// Whether `output` is drawn (not powered off).
    pub fn powered(&self, output: OutputId) -> bool {
        self.head_of_output(output).is_none_or(|h| h.powered)
    }

    /// The x coordinate right of every enabled head other than `except`.
    fn right_edge(&self, except: HeadId) -> i32 {
        self.heads
            .iter()
            .filter(|h| h.id != except && h.state.enabled)
            .map(|h| {
                h.state.position.0
                    + output_rect(
                        h.state.mode.output_mode(),
                        h.state.scale,
                        h.state.transform,
                        (0, 0),
                    )
                    .w as i32
            })
            .max()
            .unwrap_or(0)
    }
}

impl Cupertino {
    /// A backend found a display. Its head starts in the stored state, else the default one, and
    /// is announced; the caller then enables it ([`HeadOp::Enable`]) if that state says so. `scale`
    /// is a scale the backend was told to use (the command line's): it beats the stored and the
    /// default one.
    pub fn connect_head(&mut self, info: HeadInfo, scale: Option<f64>) -> (HeadId, HeadState) {
        assert!(!info.modes.is_empty(), "a head has at least one mode");
        let id = HeadId(self.displays.next_head);
        self.displays.next_head += 1;
        let key = display_key(
            &info.connector,
            &info.make,
            &info.model,
            info.serial.as_deref(),
        );
        let preferred = info.modes[0];
        let mut state = match self.displays.saved.get(&key) {
            Some(s) => {
                let stored = HeadMode {
                    width: s.mode.width as i32,
                    height: s.mode.height as i32,
                    refresh_mhz: s.mode.refresh_mhz as i32,
                };
                let mode = if info.modes.contains(&stored) || info.caps.custom_modes {
                    stored
                } else {
                    tracing::info!(%key, "the stored mode is not offered any more, using the preferred mode");
                    preferred
                };
                let transform = if info.caps.transforms {
                    from_stored_transform(s.transform)
                } else {
                    Transform::Normal
                };
                HeadState {
                    enabled: s.enabled || !info.caps.can_disable,
                    mode,
                    position: s.position,
                    scale: s.scale,
                    transform,
                }
            }
            None => HeadState {
                enabled: true,
                mode: preferred,
                position: (self.displays.right_edge(id), 0),
                scale: default_scale(
                    preferred,
                    info.physical_mm,
                    cupertino_displays::is_built_in(&info.connector),
                ),
                transform: Transform::Normal,
            },
        };
        if let Some(scale) = scale {
            state.scale = scale;
        }
        // a display that comes back to a place another one took meanwhile goes beside the others
        let others: Vec<Area> = self
            .displays
            .heads
            .iter()
            .filter(|h| h.state.enabled)
            .map(|h| state_area(&h.state))
            .collect();
        let (x, y) = arrangement::free_place(&others, state_area(&state));
        if (x, y) != (f64::from(state.position.0), f64::from(state.position.1)) {
            tracing::info!(%key, from = ?state.position, to = ?(x, y), "the stored place is taken; the display goes beside the others");
            state.position = (x as i32, y as i32);
        }
        if !state.enabled && !self.displays.heads.iter().any(|h| h.state.enabled) {
            tracing::info!(%key, "enabling a display stored as disabled: no other display is on");
            state.enabled = true;
        }
        tracing::info!(connector = %info.connector, %key, ?state, "display connected");
        self.displays.heads.push(Head {
            id,
            info,
            key,
            state,
            output: None,
            powered: true,
            wanted_mode: None,
            unsupported: false,
            advertised: None,
        });
        self.heads_changed();
        (id, state)
    }

    /// A backend lost a display; it has already stopped driving it. Its output goes away, and when no
    /// display is left on, the first disabled one is turned on.
    pub fn disconnect_head(&mut self, id: HeadId) {
        let Some(idx) = self.displays.heads.iter().position(|h| h.id == id) else {
            return;
        };
        if self.displays.heads[idx].output.is_some() {
            self.drop_head_output(id);
        }
        let head = self.displays.heads.remove(idx);
        tracing::info!(connector = %head.info.connector, "display disconnected");
        management::head_gone(self, id);
        self.displays.serial = self.displays.serial.wrapping_add(1);
        self.displays.dirty = true;
        if !self.displays.heads.iter().any(|h| h.state.enabled) {
            if let Some(next) = self.displays.heads.first().map(|h| h.id) {
                let mut state = self.displays.heads[0].state;
                state.enabled = true;
                if let Err(e) = self.configure_heads(&[(next, state)], false) {
                    tracing::error!(error = %e, "no display is on and the remaining one could not be enabled");
                }
            }
        }
        self.heads_changed();
    }

    /// Ask the backend to carry out `op` on `head`.
    pub(crate) fn head_op(&mut self, head: HeadId, op: HeadOp) -> Result<()> {
        let mut backend = self
            .backend
            .take()
            .ok_or_else(|| anyhow!("no backend is running"))?;
        let result = match &mut backend {
            #[cfg(feature = "winit")]
            Backend::Winit(w) => crate::backend::winit::head_op(self, w, head, op),
            Backend::Headless(h) => crate::backend::headless::head_op(self, h, head, op),
            #[cfg(feature = "udev")]
            Backend::Udev(u) => crate::backend::udev::head_op(self, u, head, op),
        };
        self.backend = Some(backend);
        result
    }

    /// Turn on a head the backend found, in its state, or else in the first of its [`fallback_modes`] the display can
    /// show. A fallback is shown and announced but not stored. Fails, with the error for its own mode, when none works.
    pub fn enable_head(&mut self, id: HeadId) -> Result<()> {
        let Err(first) = self.head_op(id, HeadOp::Enable) else {
            return Ok(());
        };
        let head = self.displays.head(id).context("the head being enabled")?;
        let wanted = head.state.mode;
        tracing::warn!(connector = %head.info.connector, ?wanted, error = %format!("{first:#}"), "the display cannot show its mode; trying smaller ones");
        if !another_mode_may_help(&first) {
            return Err(first);
        }
        for mode in fallback_modes(wanted, &head.info.modes.clone()) {
            self.displays.head_mut(id).expect("checked").state.mode = mode;
            match self.head_op(id, HeadOp::Enable) {
                Ok(()) => {
                    let head = self.displays.head_mut(id).expect("checked");
                    head.wanted_mode = Some(wanted);
                    tracing::warn!(connector = %head.info.connector, ?mode, "the display shows a fallback mode");
                    self.heads_changed();
                    return Ok(());
                }
                Err(e) => {
                    tracing::info!(?mode, error = %format!("{e:#}"), "the fallback mode failed too")
                }
            }
        }
        self.displays.head_mut(id).expect("checked").state.mode = wanted;
        Err(first)
    }

    /// The backend cannot show `id` at all (it cannot show the render GPU's frames): its output goes, and it is listed as
    /// off while it stays connected. Its stored state is kept, so plugged in again it is tried again.
    pub fn head_unsupported(&mut self, id: HeadId) {
        if self.displays.head(id).is_some_and(|h| h.output.is_some()) {
            if let Err(e) = self.head_op(id, HeadOp::Disable) {
                tracing::warn!(error = %format!("{e:#}"), "turning off an unsupported display failed");
            }
        }
        if let Some(head) = self.displays.head_mut(id) {
            head.state.enabled = false;
            head.unsupported = true;
        }
        self.heads_changed();
    }

    /// The output of a head being enabled, in its state, for the backend to set up. Nothing sees it until the backend
    /// shows it and hands it to [`Cupertino::create_head_output`]. Backends call this for [`HeadOp::Enable`].
    pub fn prepare_head_output(&self, id: HeadId) -> Result<Output> {
        let head = self.displays.head(id).context("the head being enabled")?;
        let physical = PhysicalProperties {
            size: head.info.physical_mm.into(),
            subpixel: Subpixel::Unknown,
            make: head.info.make.clone(),
            model: head.info.model.clone(),
        };
        Ok(new_output(
            &head.info.connector,
            physical,
            head.state.mode.output_mode(),
            head.state.scale,
            head.output_transform(),
            head.state.position,
        ))
    }

    /// Register and announce the output of [`Cupertino::prepare_head_output`] once the backend shows it.
    pub fn create_head_output(&mut self, id: HeadId, output: Output) -> OutputId {
        let output = self.add_output(output);
        let head = self.displays.head_mut(id).expect("the head being enabled");
        head.output = Some(output);
        head.powered = true;
        let key = head.key.clone();
        if let Some(entry) = self.outputs.iter_mut().find(|o| o.id == output) {
            entry.identity = Some(key.clone());
        }
        self.restore_departed(output, &key);
        self.refresh_render_scales();
        self.refresh_surface_scales();
        output
    }

    /// Drop the output of a head: its layer surfaces are closed and its power objects fail.
    /// Backends call this for [`HeadOp::Disable`] and when a display goes away.
    pub fn drop_head_output(&mut self, id: HeadId) {
        let Some(head) = self.displays.head_mut(id) else {
            return;
        };
        let Some(output) = head.output.take() else {
            return;
        };
        if let Some(entry) = self.output_entry(output) {
            let map = smithay::desktop::layer_map_for_output(&entry.output);
            for layer in map.layers() {
                layer.layer_surface().send_close();
            }
        }
        power::output_gone(self, output);
        self.remove_output(output);
        self.refresh_surface_scales();
    }

    /// Apply (or with `test_only`, only check) a configuration of heads. Heads not named keep their
    /// state. On a failure part way, the previous state is restored. An applied configuration is stored.
    /// The caller then reports the change with [`Cupertino::heads_changed`] (after answering the
    /// configuration, which the protocol orders first).
    pub fn configure_heads(
        &mut self,
        wanted: &[(HeadId, HeadState)],
        test_only: bool,
    ) -> Result<(), String> {
        let mut after: Vec<(HeadId, HeadState)> = self
            .displays
            .heads
            .iter()
            .map(|h| (h.id, h.state))
            .collect();
        for (id, state) in wanted {
            let head = self
                .displays
                .head(*id)
                .ok_or_else(|| format!("display {id:?} went away"))?;
            check_head(head, state)?;
            let slot = after
                .iter_mut()
                .find(|(h, _)| h == id)
                .expect("every head is listed");
            slot.1 = *state;
        }
        self.check_arrangement(&after)?;
        if test_only {
            return Ok(());
        }
        let before: Vec<(HeadId, HeadState)> = self
            .displays
            .heads
            .iter()
            .map(|h| (h.id, h.state))
            .collect();
        let offered: Vec<Vec<HeadMode>> = self
            .displays
            .heads
            .iter()
            .map(|h| h.info.modes.clone())
            .collect();
        let result = self.set_head_states(&after);
        if let Err(e) = &result {
            tracing::warn!(error = %e, "applying the display configuration failed, restoring the previous one");
            // a mode the failed configuration added is not offered afterwards
            for (head, modes) in self.displays.heads.iter_mut().zip(offered) {
                head.info.modes = modes;
            }
            if let Err(e) = self.set_head_states(&before) {
                tracing::error!(error = %e, "restoring the previous display configuration failed");
            }
        } else {
            // what the person changed is their choice; a fallback or an unsupported head they left alone keeps its own
            for (id, state) in wanted {
                let was = before.iter().find(|(h, _)| h == id).map(|(_, s)| *s);
                if let Some(head) = self.displays.head_mut(*id) {
                    if was.map(|s| s.mode) != Some(state.mode) {
                        head.wanted_mode = None;
                    }
                    if was.map(|s| s.enabled) != Some(state.enabled) {
                        head.unsupported = false;
                    }
                }
            }
            self.store_displays();
        }
        result
    }

    /// Check the arrangement `states` would give as a whole (`cupertino_displays::arrangement::check`): a display on,
    /// each inside the coordinate range, none overlapping another.
    fn check_arrangement(&self, states: &[(HeadId, HeadState)]) -> Result<(), String> {
        let on: Vec<(&str, Area)> = states
            .iter()
            .filter(|(_, s)| s.enabled)
            .filter_map(|(id, s)| {
                let head = self.displays.head(*id)?;
                Some((head.info.connector.as_str(), state_area(s)))
            })
            .collect();
        let areas: Vec<Area> = on.iter().map(|(_, a)| *a).collect();
        arrangement::check(&areas).map_err(|e| match e {
            ArrangementError::Nothing => e.to_string(),
            ArrangementError::Empty(i) => format!("{} would have no logical size", on[i].0),
            ArrangementError::OutOfRange(i) => {
                format!("{} would lie beyond the coordinate range", on[i].0)
            }
            ArrangementError::Overlap(a, b) => {
                format!("{} and {} would overlap", on[a].0, on[b].0)
            }
        })
    }

    /// Bring every head to its wanted state: the ones turned off first, so their resources are
    /// free for the ones turned on.
    fn set_head_states(&mut self, wanted: &[(HeadId, HeadState)]) -> Result<(), String> {
        for (id, state) in wanted.iter().filter(|(_, s)| !s.enabled) {
            if self.displays.head(*id).is_some_and(|h| h.output.is_some()) {
                self.head_op(*id, HeadOp::Disable)
                    .map_err(|e| format!("{e:#}"))?;
            }
            self.displays.head_mut(*id).expect("checked").state = *state;
        }
        for (id, state) in wanted.iter().filter(|(_, s)| s.enabled) {
            let head = self.displays.head_mut(*id).expect("checked");
            let previous = head.state;
            if !head.info.modes.contains(&state.mode) {
                head.info.modes.push(state.mode);
            }
            match head.output {
                None => {
                    head.state = *state;
                    if let Err(e) = self.head_op(*id, HeadOp::Enable) {
                        self.displays.head_mut(*id).expect("checked").state = previous;
                        return Err(format!("{e:#}"));
                    }
                }
                Some(output) => {
                    if state.mode != previous.mode {
                        self.head_op(*id, HeadOp::SetMode(state.mode))
                            .map_err(|e| format!("{e:#}"))?;
                    }
                    let head = self.displays.head_mut(*id).expect("checked");
                    head.state = *state;
                    let transform = head.output_transform();
                    self.configure_output(
                        output,
                        state.mode.output_mode(),
                        state.scale,
                        transform,
                        state.position,
                    );
                }
            }
        }
        self.refresh_surface_scales();
        Ok(())
    }

    /// The backend's only mode of a head changed (the nested window was resized): the head takes it.
    pub fn head_mode_replaced(&mut self, id: HeadId, mode: HeadMode) {
        let Some(head) = self.displays.head_mut(id) else {
            return;
        };
        head.info.modes = vec![mode];
        head.state.mode = mode;
        let (output, s, transform) = (head.output, head.state, head.output_transform());
        if let Some(output) = output {
            self.configure_output(output, mode.output_mode(), s.scale, transform, s.position);
        }
        self.heads_changed();
    }

    /// Power `output` on or off. Fails for an output without a head or when the backend cannot.
    pub fn set_output_power(&mut self, output: OutputId, on: bool) -> Result<()> {
        let head = self
            .displays
            .head_of_output(output)
            .map(|h| h.id)
            .ok_or_else(|| anyhow!("the output has no display"))?;
        self.head_op(head, HeadOp::Power(on))?;
        self.displays.head_mut(head).expect("checked").powered = on;
        if on {
            self.request_redraw();
        }
        power::mode_changed(self, output, on);
        Ok(())
    }

    /// Write every connected head's state to the store, keeping the entries of absent displays.
    fn store_displays(&mut self) {
        for head in &self.displays.heads {
            self.displays.saved.set(&head.key, stored_settings(head));
        }
        let Some(path) = &self.displays.store else {
            return;
        };
        match self.displays.saved.store(path) {
            Ok(()) => tracing::info!(path = %path.display(), "display configuration stored"),
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "storing the display configuration failed")
            }
        }
    }

    /// Tell output-management clients what changed since they were last told; the serial moves on
    /// when anything did.
    pub fn heads_changed(&mut self) {
        let mut changed = false;
        for idx in 0..self.displays.heads.len() {
            let head = &self.displays.heads[idx];
            let now = (head.state, head.info.modes.clone());
            if head.advertised.as_ref() == Some(&now) {
                continue;
            }
            changed = true;
            management::head_changed(self, idx);
            self.displays.heads[idx].advertised = Some(now);
        }
        if changed {
            self.displays.serial = self.displays.serial.wrapping_add(1);
            self.displays.dirty = true;
        }
        if std::mem::take(&mut self.displays.dirty) {
            management::done(self);
            self.notify_displays_bus_changed();
        }
    }

    /// Tell every surface which outputs it is on and the scale it should draw at: a layer surface its output's, a window
    /// that of the output it overlaps most (`surface_output`).
    pub fn refresh_surface_scales(&mut self) {
        self.refresh_surface_outputs();
    }
}
