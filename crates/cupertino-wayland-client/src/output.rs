//! Outputs as `wl_output` describes them.

use crate::Scale;

/// An output, by its registry name; stable while it is plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputId(pub(crate) u32);

impl OutputId {
    /// The `wl_registry` name of the output's global, which other connections to the compositor know it by.
    pub fn registry_name(self) -> u32 {
        self.0
    }

    /// The output whose `wl_output` global has the registry name `name`, as another connection reported it.
    pub fn from_registry_name(name: u32) -> OutputId {
        OutputId(name)
    }
}

/// Where an output sits in the compositor's global space, in logical points (`zxdg_output_v1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How the compositor rotates and flips an output's content (`wl_output.transform`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputTransform {
    #[default]
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl OutputTransform {
    /// Whether the mode's width runs along the logical height.
    pub fn swaps_sides(self) -> bool {
        matches!(
            self,
            Self::Rotated90 | Self::Rotated270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

/// What `wl_output` reported about an output, as of its last `done` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// The connector name (version 4), such as `DP-1`.
    pub name: Option<String>,
    /// A human-readable description (version 4).
    pub description: Option<String>,
    /// The manufacturer, as the display reports it (often a three-letter vendor code); `None` when unknown.
    pub make: Option<String>,
    /// The model name; `None` when unknown.
    pub model: Option<String>,
    /// The physical width and height in millimetres; `None` when the display has no size, such as a projector.
    pub physical_size_mm: Option<(u32, u32)>,
    pub transform: OutputTransform,
    /// The integer scale the compositor renders this output at.
    pub scale: Scale,
    /// The current mode in device pixels.
    pub mode: Option<(u32, u32)>,
    /// The logical position and size; `None` without [`crate::Capability::OutputLayout`] or before the compositor
    /// reported both.
    pub geometry: Option<OutputGeometry>,
}

impl OutputInfo {
    /// Device pixels per logical point: the current mode over the logical size, such as 1.5 for a fractional scale.
    /// `None` without a mode or a logical size.
    pub fn effective_scale(&self) -> Option<f64> {
        let (width, height) = self.mode?;
        let mode_width = if self.transform.swaps_sides() {
            height
        } else {
            width
        };
        Some(f64::from(mode_width) / f64::from(self.geometry?.width))
    }
}

/// A make or model as `wl_output.geometry` sends it: `None` for an empty one or the placeholder of a display without
/// identification.
pub(crate) fn identification(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "Unknown").then(|| value.to_owned())
}

/// A physical size as `wl_output.geometry` sends it, in millimetres: `None` when a side is not positive.
pub(crate) fn physical_size(width: i32, height: i32) -> Option<(u32, u32)> {
    u32::try_from(width)
        .ok()
        .zip(u32::try_from(height).ok())
        .filter(|(w, h)| *w > 0 && *h > 0)
}

/// An output's properties as they arrive; [`OutputBuilder::done`] applies them at once.
#[derive(Clone, Debug)]
pub(crate) struct OutputBuilder {
    pub current: OutputInfo,
    pending: OutputInfo,
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
}

impl OutputBuilder {
    pub(crate) fn new(id: OutputId) -> Self {
        let info = OutputInfo {
            id,
            name: None,
            description: None,
            make: None,
            model: None,
            physical_size_mm: None,
            transform: OutputTransform::Normal,
            scale: Scale::ONE,
            mode: None,
            geometry: None,
        };
        Self {
            current: info.clone(),
            pending: info,
            position: None,
            size: None,
        }
    }

    /// The logical position from `zxdg_output_v1`, applied with the next `done`.
    pub(crate) fn set_position(&mut self, x: i32, y: i32) {
        self.position = Some((x, y));
    }

    /// The logical size from `zxdg_output_v1`, applied with the next `done`; an empty one is ignored.
    pub(crate) fn set_size(&mut self, width: i32, height: i32) {
        self.size = u32::try_from(width)
            .ok()
            .zip(u32::try_from(height).ok())
            .filter(|(w, h)| *w > 0 && *h > 0);
    }

    pub(crate) fn pending(&mut self) -> &mut OutputInfo {
        &mut self.pending
    }

    /// Apply what arrived since the last `done`; whether anything changed.
    pub(crate) fn done(&mut self) -> bool {
        self.pending.geometry = self
            .position
            .zip(self.size)
            .map(|((x, y), (width, height))| OutputGeometry {
                x,
                y,
                width,
                height,
            });
        let changed = self.current != self.pending;
        self.current = self.pending.clone();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_apply_together_at_done() {
        let mut b = OutputBuilder::new(OutputId(7));
        b.pending().scale = Scale::from_integer(2).unwrap();
        b.pending().name = Some("DP-1".into());
        assert_eq!(b.current.scale, Scale::ONE);
        assert!(b.done());
        assert_eq!(
            (b.current.scale, b.current.name.as_deref()),
            (Scale::from_integer(2).unwrap(), Some("DP-1"))
        );
        assert!(!b.done());
    }

    #[test]
    fn the_effective_scale_is_the_mode_over_the_logical_size_along_the_same_side() {
        let mut b = OutputBuilder::new(OutputId(7));
        b.pending().mode = Some((2560, 1600));
        b.set_position(0, 0);
        b.set_size(1280, 800);
        b.done();
        assert_eq!(b.current.effective_scale(), Some(2.0));
        b.pending().transform = OutputTransform::Rotated90;
        b.set_size(1067, 1707);
        b.done();
        assert_eq!(b.current.effective_scale(), Some(1600.0 / 1067.0));
        b.pending().mode = None;
        b.done();
        assert_eq!(b.current.effective_scale(), None);
    }

    #[test]
    fn an_empty_or_placeholder_identification_is_unknown() {
        assert_eq!(identification("Unknown".into()), None);
        assert_eq!(identification(" ".into()), None);
        assert_eq!(identification(" GSM ".into()).as_deref(), Some("GSM"));
    }

    #[test]
    fn a_physical_size_without_a_side_is_unknown() {
        assert_eq!(physical_size(0, 0), None);
        assert_eq!(physical_size(600, 0), None);
        assert_eq!(physical_size(-1, 340), None);
        assert_eq!(physical_size(600, 340), Some((600, 340)));
    }

    #[test]
    fn the_geometry_needs_both_position_and_size() {
        let mut b = OutputBuilder::new(OutputId(7));
        b.set_position(-1920, 0);
        assert!(!b.done());
        b.set_size(1920, 0);
        assert!(!b.done());
        b.set_size(1920, 1080);
        assert!(b.done());
        assert_eq!(
            b.current.geometry,
            Some(OutputGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
    }
}
