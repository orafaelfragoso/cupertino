//! Display head data model, EDID parsing, mode fallback selection, scale choice, and head
//! validation decoupled from compositor state.

pub mod edid;

use cupertino_defaults::displays::{DisplaySettings, ModeSetting, Transform as StoredTransform};
use cupertino_displays::arrangement::Area;
use cupertino_wm::{OutputId, Rect};
use smithay::output::Mode as OutputMode;
use smithay::utils::Transform;

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
        OutputMode {
            size: (self.width, self.height).into(),
            refresh: self.refresh_mhz,
        }
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

/// A connected display head.
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
    /// The mode the head was to show when the display could not show it and fell back to `state.mode`.
    pub wanted_mode: Option<HeadMode>,
    /// The backend could not show the head, so it is off; stays stored as on to retry on replug.
    pub unsupported: bool,
    /// The state and modes output-management clients were last told, `None` before the first time.
    pub advertised: Option<(HeadState, Vec<HeadMode>)>,
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
    /// Create the head's output in its current state and start driving it.
    Enable,
    /// Stop driving the head and drop its output.
    Disable,
    /// Switch the enabled head to another mode; the output itself is updated by the caller.
    SetMode(HeadMode),
    /// Power the enabled head's display on or off.
    Power(bool),
}

/// The rectangle of an output in the global logical space: its mode turned by `transform`, divided
/// by `scale`, at `position`.
pub fn output_rect(
    mode: OutputMode,
    scale: f64,
    transform: Transform,
    position: (i32, i32),
) -> Rect {
    let size = transform.transform_size(mode.size);
    Rect::new(
        f64::from(position.0),
        f64::from(position.1),
        (f64::from(size.w) / scale).round(),
        (f64::from(size.h) / scale).round(),
    )
}

/// The area of the global logical space a head in `state` covers.
pub fn state_area(state: &HeadState) -> Area {
    let r = output_rect(
        state.mode.output_mode(),
        state.scale,
        state.transform,
        state.position,
    );
    Area::new(r.x, r.y, r.w, r.h)
}

/// The display model's panel for a head showing `mode`: `None` when its physical size is unknown.
pub fn panel_of(
    mode: HeadMode,
    physical_mm: (i32, i32),
    built_in: bool,
) -> Option<cupertino_displays::Panel> {
    let pixels = (mode.width.max(0) as u32, mode.height.max(0) as u32);
    let mm = (physical_mm.0.max(0) as u32, physical_mm.1.max(0) as u32);
    cupertino_displays::Panel::from_physical(pixels, mm, false).map(|p| p.built_in(built_in))
}

/// The scale a display without stored settings starts at.
pub fn default_scale(mode: HeadMode, physical_mm: (i32, i32), built_in: bool) -> f64 {
    panel_of(mode, physical_mm, built_in)
        .and_then(|panel| {
            cupertino_displays::default_choice(&panel, mode.refresh_mhz.max(0) as u32)
        })
        .map_or(1.0, |choice| choice.scale)
}

/// Snap a 1/256-quantised output-management scale back to an exact display choice when within 1/256.
pub fn exact_choice_scale(head: &Head, mode: HeadMode, scale: f64) -> f64 {
    const FIXED_STEP: f64 = 1.0 / 256.0;
    panel_of(
        mode,
        head.info.physical_mm,
        cupertino_displays::is_built_in(&head.info.connector),
    )
    .map(|panel| cupertino_displays::catalogue(&panel, mode.refresh_mhz.max(0) as u32))
    .and_then(|list| {
        list.choices
            .iter()
            .map(|c| c.scale)
            .find(|exact| (exact - scale).abs() < FIXED_STEP)
    })
    .unwrap_or(scale)
}

pub fn stored_transform(t: Transform) -> StoredTransform {
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

pub fn from_stored_transform(t: StoredTransform) -> Transform {
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

pub fn stored_settings(head: &Head) -> DisplaySettings {
    let (state, mode) = (&head.state, head.wanted_mode.unwrap_or(head.state.mode));
    DisplaySettings {
        enabled: state.enabled || head.unsupported,
        mode: ModeSetting {
            width: mode.width as u32,
            height: mode.height as u32,
            refresh_mhz: mode.refresh_mhz as u32,
        },
        position: state.position,
        scale: state.scale,
        transform: stored_transform(state.transform),
    }
}

/// The modes to try, in order, when a display cannot show `wanted`.
pub fn fallback_modes(wanted: HeadMode, modes: &[HeadMode]) -> Vec<HeadMode> {
    let area = |m: &HeadMode| i64::from(m.width) * i64::from(m.height);
    let same_size = |m: &HeadMode| m.width == wanted.width && m.height == wanted.height;
    let mut fallbacks: Vec<HeadMode> = modes
        .iter()
        .copied()
        .filter(|m| {
            if same_size(m) {
                m.refresh_mhz < wanted.refresh_mhz
            } else {
                m.width <= wanted.width && m.height <= wanted.height && area(m) < area(&wanted)
            }
        })
        .collect();
    fallbacks.sort_by_key(|m| (std::cmp::Reverse(area(m)), std::cmp::Reverse(m.refresh_mhz)));
    fallbacks.dedup();
    fallbacks
}

/// Whether a head that failed to turn on with `error` may turn on in another mode.
pub fn another_mode_may_help(error: &anyhow::Error) -> bool {
    !error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                std::io::ErrorKind::ResourceBusy | std::io::ErrorKind::PermissionDenied
            )
        })
    })
}

/// Why `wanted` cannot be applied to `head`, if it cannot.
pub fn check_head(head: &Head, wanted: &HeadState) -> Result<(), String> {
    let name = &head.info.connector;
    let caps = head.info.caps;
    if !wanted.enabled {
        return if caps.can_disable {
            Ok(())
        } else {
            Err(format!("{name} cannot be disabled"))
        };
    }
    if !(wanted.scale.is_finite() && wanted.scale > 0.0) {
        return Err(format!("{name}: scale {} is not above zero", wanted.scale));
    }
    let mode = wanted.mode;
    let listed = head.info.modes.contains(&mode);
    if !listed && !(caps.custom_modes && mode.width > 0 && mode.height > 0 && mode.refresh_mhz >= 0)
    {
        return Err(format!(
            "{name} has no mode {}x{}@{}",
            mode.width, mode.height, mode.refresh_mhz
        ));
    }
    if wanted.transform != Transform::Normal && !caps.transforms {
        return Err(format!("{name} cannot be rotated or flipped"));
    }
    let rect = output_rect(
        mode.output_mode(),
        wanted.scale,
        wanted.transform,
        wanted.position,
    );
    if rect.w < 1.0 || rect.h < 1.0 {
        return Err(format!(
            "{name}: scale {} leaves no logical size",
            wanted.scale
        ));
    }
    Ok(())
}
