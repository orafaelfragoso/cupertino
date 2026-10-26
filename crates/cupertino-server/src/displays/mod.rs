//! Displays: the heads (connected display devices) the backends found, the state each one is in,
//! and the arrangement the user gave them, stored in `displays.conf` (`cupertino_defaults::displays`).
//! An enabled head has an output; a disabled head stays listed for output-management clients.
//!
//! Backends report heads with [`Cupertino::connect_head`] and [`Cupertino::disconnect_head`] and
//! carry out the [`HeadOp`]s asked of them. The output-management protocol (`management`) changes
//! heads through [`Cupertino::configure_heads`], which validates, applies, reverts on failure and
//! stores the result. A new head starts in its stored state, else at its preferred mode, the scale
//! of `cupertino_displays::default_choice` and to the right of the other heads.

pub mod edid;
pub mod management;
pub mod power;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use cupertino_defaults::displays::{display_key, DisplaySettings, DisplaysConfig, ModeSetting, Transform as StoredTransform};
use cupertino_wm::OutputId;
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Transform;

use crate::backend::Backend;
use crate::state::{output_rect, Cupertino};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HeadId(pub u64);

/// A mode by its pixel size and refresh rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadMode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

impl HeadMode {
    pub fn output_mode(self) -> OutputMode {
        OutputMode { size: (self.width, self.height).into(), refresh: self.refresh_mhz }
    }
}

/// What a head is set to. A disabled head keeps the rest, so enabling it restores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadState {
    pub enabled: bool,
    pub mode: HeadMode,
    /// Top-left corner in the global logical space.
    pub position: (i32, i32),
    /// Physical pixels per logical point.
    pub scale: f64,
    pub transform: Transform,
}

/// What a backend can change on a head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadCaps {
    /// The head can be turned off (not the nested window, which is the only output).
    pub can_disable: bool,
    /// Any pixel size is a mode (offscreen outputs); otherwise only the listed modes.
    pub custom_modes: bool,
    /// Transforms other than normal can be set.
    pub transforms: bool,
}

/// A display as its backend found it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadInfo {
    /// Connector name (`DP-1`, `headless-1`, `winit`); unique among the connected heads.
    pub connector: String,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    /// Size of the picture in millimetres; zero when unknown.
    pub physical_mm: (i32, i32),
    /// Modes the head can show, the preferred one first.
    pub modes: Vec<HeadMode>,
    pub caps: HeadCaps,
    /// Transform the backend applies under the user's (the nested window is drawn upside down);
    /// normal everywhere else.
    pub base_transform: Transform,
}

/// A connected display.
pub struct Head {
    pub id: HeadId,
    pub info: HeadInfo,
    /// Key of its stored settings (`cupertino_defaults::displays::display_key`).
    pub key: String,
    pub state: HeadState,
    /// The output while the head is enabled.
    pub output: Option<OutputId>,
    /// Powered on (output power management); a powered-off output is not drawn.
    pub powered: bool,
    /// The state and modes output-management clients were last told, `None` before the first time.
    advertised: Option<(HeadState, Vec<HeadMode>)>,
}

impl Head {
    /// The transform the output is drawn with: the user's, under the backend's own.
    pub fn output_transform(&self) -> Transform {
        match self.info.base_transform {
            Transform::Normal => self.state.transform,
            base => base,
        }
    }

    pub fn preferred_mode(&self) -> HeadMode {
        self.info.modes[0]
    }
}

/// What a backend is asked to do with a head.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeadOp {
    /// Create the head's output in its current state (`Cupertino::create_head_output`) and start
    /// driving it.
    Enable,
    /// Stop driving the head and drop its output (`Cupertino::drop_head_output`).
    Disable,
    /// Switch the enabled head to another mode; the output itself is updated by the caller.
    SetMode(HeadMode),
    /// Power the enabled head's display on or off.
    Power(bool),
}

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
    pub(crate) managers: Vec<management::Manager>,
    pub(crate) powers: Vec<power::PowerObject>,
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
            managers: Vec::new(),
            powers: Vec::new(),
            _management: management::create_global(dh),
            _power: power::create_global(dh),
        }
    }

    /// Store the settings in `path` and read what it holds. Problems in the file are logged and
    /// the sections they are in are ignored; a file that cannot be read is an error.
    pub fn use_store(&mut self, path: PathBuf) -> Result<()> {
        let (saved, problems) = DisplaysConfig::load(&path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
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
            .map(|h| h.state.position.0 + output_rect(h.state.mode.output_mode(), h.state.scale, h.state.transform, (0, 0)).w as i32)
            .max()
            .unwrap_or(0)
    }
}

/// The scale a display without stored settings starts at: the anchor of its "looks like" list
/// (`cupertino_displays::default_choice`), or 1 when its physical size is unknown or its height is
/// at or below the list's floor.
pub fn default_scale(mode: HeadMode, physical_mm: (i32, i32)) -> f64 {
    let pixels = (mode.width.max(0) as u32, mode.height.max(0) as u32);
    let mm = (physical_mm.0.max(0) as u32, physical_mm.1.max(0) as u32);
    cupertino_displays::Panel::from_physical(pixels, mm, false)
        .and_then(|panel| cupertino_displays::default_choice(&panel, mode.refresh_mhz.max(0) as u32))
        .map_or(1.0, |choice| choice.scale)
}

fn stored_transform(t: Transform) -> StoredTransform {
    match t {
        Transform::Normal => StoredTransform::Normal,
        Transform::_90 => StoredTransform::Rotate90,
        Transform::_180 => StoredTransform::Rotate180,
        Transform::_270 => StoredTransform::Rotate270,
        Transform::Flipped => StoredTransform::Flipped,
        Transform::Flipped90 => StoredTransform::Flipped90,
        Transform::Flipped180 => StoredTransform::Flipped180,
        Transform::Flipped270 => StoredTransform::Flipped270,
    }
}

fn from_stored_transform(t: StoredTransform) -> Transform {
    match t {
        StoredTransform::Normal => Transform::Normal,
        StoredTransform::Rotate90 => Transform::_90,
        StoredTransform::Rotate180 => Transform::_180,
        StoredTransform::Rotate270 => Transform::_270,
        StoredTransform::Flipped => Transform::Flipped,
        StoredTransform::Flipped90 => Transform::Flipped90,
        StoredTransform::Flipped180 => Transform::Flipped180,
        StoredTransform::Flipped270 => Transform::Flipped270,
    }
}

fn stored_settings(state: &HeadState) -> DisplaySettings {
    DisplaySettings {
        enabled: state.enabled,
        mode: ModeSetting { width: state.mode.width as u32, height: state.mode.height as u32, refresh_mhz: state.mode.refresh_mhz as u32 },
        position: state.position,
        scale: state.scale,
        transform: stored_transform(state.transform),
    }
}

/// Why `wanted` cannot be applied to `head`, if it cannot.
fn check_head(head: &Head, wanted: &HeadState) -> Result<(), String> {
    let name = &head.info.connector;
    let caps = head.info.caps;
    if !wanted.enabled {
        return if caps.can_disable { Ok(()) } else { Err(format!("{name} cannot be disabled")) };
    }
    if !(wanted.scale.is_finite() && wanted.scale > 0.0) {
        return Err(format!("{name}: scale {} is not above zero", wanted.scale));
    }
    let mode = wanted.mode;
    let listed = head.info.modes.contains(&mode);
    if !listed && !(caps.custom_modes && mode.width > 0 && mode.height > 0 && mode.refresh_mhz >= 0) {
        return Err(format!("{name} has no mode {}x{}@{}", mode.width, mode.height, mode.refresh_mhz));
    }
    if wanted.transform != Transform::Normal && !caps.transforms {
        return Err(format!("{name} cannot be rotated or flipped"));
    }
    let rect = output_rect(mode.output_mode(), wanted.scale, wanted.transform, wanted.position);
    if rect.w < 1.0 || rect.h < 1.0 {
        return Err(format!("{name}: scale {} leaves no logical size", wanted.scale));
    }
    Ok(())
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
        let key = display_key(&info.connector, &info.make, &info.model, info.serial.as_deref());
        let preferred = info.modes[0];
        let mut state = match self.displays.saved.get(&key) {
            Some(s) => {
                let stored = HeadMode { width: s.mode.width as i32, height: s.mode.height as i32, refresh_mhz: s.mode.refresh_mhz as i32 };
                let mode = if info.modes.contains(&stored) || info.caps.custom_modes {
                    stored
                } else {
                    tracing::info!(%key, "the stored mode is not offered any more, using the preferred mode");
                    preferred
                };
                let transform = if info.caps.transforms { from_stored_transform(s.transform) } else { Transform::Normal };
                HeadState { enabled: s.enabled || !info.caps.can_disable, mode, position: s.position, scale: s.scale, transform }
            }
            None => HeadState { enabled: true, mode: preferred, position: (self.displays.right_edge(id), 0), scale: default_scale(preferred, info.physical_mm), transform: Transform::Normal },
        };
        if let Some(scale) = scale {
            state.scale = scale;
        }
        if !state.enabled && !self.displays.heads.iter().any(|h| h.state.enabled) {
            tracing::info!(%key, "enabling a display stored as disabled: no other display is on");
            state.enabled = true;
        }
        tracing::info!(connector = %info.connector, %key, ?state, "display connected");
        self.displays.heads.push(Head { id, info, key, state, output: None, powered: true, advertised: None });
        self.heads_changed();
        (id, state)
    }

    /// A backend lost a display; it has already stopped driving it. Its output goes away, and when no
    /// display is left on, the first disabled one is turned on.
    pub fn disconnect_head(&mut self, id: HeadId) {
        let Some(idx) = self.displays.heads.iter().position(|h| h.id == id) else { return };
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
        let mut backend = self.backend.take().ok_or_else(|| anyhow!("no backend is running"))?;
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

    /// Create the output of an enabled head in its state. Backends call this for [`HeadOp::Enable`].
    pub fn create_head_output(&mut self, id: HeadId) -> OutputId {
        let head = self.displays.head(id).expect("the head being enabled");
        let physical = PhysicalProperties {
            size: head.info.physical_mm.into(),
            subpixel: Subpixel::Unknown,
            make: head.info.make.clone(),
            model: head.info.model.clone(),
        };
        let (name, s, transform) = (head.info.connector.clone(), head.state, head.output_transform());
        let output = self.add_output(&name, physical, s.mode.output_mode(), s.scale, transform, s.position);
        let head = self.displays.head_mut(id).expect("the head being enabled");
        head.output = Some(output);
        head.powered = true;
        self.refresh_surface_scales();
        output
    }

    /// Drop the output of a head: its layer surfaces are closed and its power objects fail.
    /// Backends call this for [`HeadOp::Disable`] and when a display goes away.
    pub fn drop_head_output(&mut self, id: HeadId) {
        let Some(head) = self.displays.head_mut(id) else { return };
        let Some(output) = head.output.take() else { return };
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
    pub fn configure_heads(&mut self, wanted: &[(HeadId, HeadState)], test_only: bool) -> Result<(), String> {
        let mut after: Vec<(HeadId, HeadState)> = self.displays.heads.iter().map(|h| (h.id, h.state)).collect();
        for (id, state) in wanted {
            let head = self.displays.head(*id).ok_or_else(|| format!("display {id:?} went away"))?;
            check_head(head, state)?;
            let slot = after.iter_mut().find(|(h, _)| h == id).expect("every head is listed");
            slot.1 = *state;
        }
        if !after.iter().any(|(_, s)| s.enabled) {
            return Err("at least one display must stay on".into());
        }
        if test_only {
            return Ok(());
        }
        let before: Vec<(HeadId, HeadState)> = self.displays.heads.iter().map(|h| (h.id, h.state)).collect();
        let result = self.set_head_states(&after);
        if let Err(e) = &result {
            tracing::warn!(error = %e, "applying the display configuration failed, restoring the previous one");
            if let Err(e) = self.set_head_states(&before) {
                tracing::error!(error = %e, "restoring the previous display configuration failed");
            }
        } else {
            self.store_displays();
        }
        result
    }

    /// Bring every head to its wanted state: the ones turned off first, so their resources are
    /// free for the ones turned on.
    fn set_head_states(&mut self, wanted: &[(HeadId, HeadState)]) -> Result<(), String> {
        for (id, state) in wanted.iter().filter(|(_, s)| !s.enabled) {
            if self.displays.head(*id).is_some_and(|h| h.output.is_some()) {
                self.head_op(*id, HeadOp::Disable).map_err(|e| format!("{e:#}"))?;
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
                        self.head_op(*id, HeadOp::SetMode(state.mode)).map_err(|e| format!("{e:#}"))?;
                    }
                    let head = self.displays.head_mut(*id).expect("checked");
                    head.state = *state;
                    let transform = head.output_transform();
                    self.configure_output(output, state.mode.output_mode(), state.scale, transform, state.position);
                }
            }
        }
        self.refresh_surface_scales();
        Ok(())
    }

    /// The backend's only mode of a head changed (the nested window was resized): the head takes it.
    pub fn head_mode_replaced(&mut self, id: HeadId, mode: HeadMode) {
        let Some(head) = self.displays.head_mut(id) else { return };
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
        let head = self.displays.head_of_output(output).map(|h| h.id).ok_or_else(|| anyhow!("the output has no display"))?;
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
            self.displays.saved.set(&head.key, stored_settings(&head.state));
        }
        let Some(path) = &self.displays.store else { return };
        match self.displays.saved.store(path) {
            Ok(()) => tracing::info!(path = %path.display(), "display configuration stored"),
            Err(e) => tracing::error!(path = %path.display(), error = %e, "storing the display configuration failed"),
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
        }
    }

    /// Tell every surface the scale it should draw at: a layer surface its output's, a window the
    /// first output's (windows are not tracked per output yet).
    pub fn refresh_surface_scales(&mut self) {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::fractional_scale::with_fractional_scale;
        let set = |surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, scale: f64| {
            with_states(surface, |states| with_fractional_scale(states, |fs| fs.set_preferred_scale(scale)));
        };
        for o in &self.outputs {
            let map = smithay::desktop::layer_map_for_output(&o.output);
            for layer in map.layers() {
                set(layer.wl_surface(), o.scale);
            }
        }
        let scale = self.preferred_scale();
        for w in &self.windows {
            set(w.surface(), scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(caps: HeadCaps) -> Head {
        let mode = HeadMode { width: 1920, height: 1080, refresh_mhz: 60_000 };
        let info = HeadInfo {
            connector: "DP-1".into(),
            make: "GSM".into(),
            model: "Panel".into(),
            serial: None,
            physical_mm: (527, 296),
            modes: vec![mode],
            caps,
            base_transform: Transform::Normal,
        };
        let state = HeadState { enabled: true, mode, position: (0, 0), scale: 1.0, transform: Transform::Normal };
        Head { id: HeadId(1), info, key: "connector DP-1".into(), state, output: None, powered: true, advertised: None }
    }

    #[test]
    fn a_head_accepts_only_what_its_backend_can_do() {
        let fixed = head(HeadCaps { can_disable: false, custom_modes: false, transforms: false });
        let ok = fixed.state;
        assert_eq!(check_head(&fixed, &ok), Ok(()));
        assert!(check_head(&fixed, &HeadState { enabled: false, ..ok }).is_err());
        assert!(check_head(&fixed, &HeadState { transform: Transform::_90, ..ok }).is_err());
        assert!(check_head(&fixed, &HeadState { mode: HeadMode { width: 800, height: 600, refresh_mhz: 60_000 }, ..ok }).is_err());
        assert!(check_head(&fixed, &HeadState { scale: 0.0, ..ok }).is_err());
        let free = head(HeadCaps { can_disable: true, custom_modes: true, transforms: true });
        assert_eq!(check_head(&free, &HeadState { mode: HeadMode { width: 800, height: 600, refresh_mhz: 0 }, transform: Transform::Flipped270, ..ok }), Ok(()));
        assert!(check_head(&free, &HeadState { scale: 4000.0, ..ok }).is_err(), "no logical size left");
    }

    #[test]
    fn a_new_display_starts_at_its_default_scale() {
        // 27-inch 4K: looks like 2560x1440; 24-inch 1080p: unscaled; no physical size: unscaled.
        assert_eq!(default_scale(HeadMode { width: 3840, height: 2160, refresh_mhz: 60_000 }, (597, 336)), 1.5);
        assert_eq!(default_scale(HeadMode { width: 1920, height: 1080, refresh_mhz: 60_000 }, (527, 296)), 1.0);
        assert_eq!(default_scale(HeadMode { width: 3840, height: 2160, refresh_mhz: 60_000 }, (0, 0)), 1.0);
    }

    #[test]
    fn the_winit_transform_stays_under_the_users() {
        let mut h = head(HeadCaps { can_disable: false, custom_modes: false, transforms: false });
        h.info.base_transform = Transform::Flipped180;
        assert_eq!(h.output_transform(), Transform::Flipped180);
    }
}
