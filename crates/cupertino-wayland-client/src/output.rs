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
}

/// Where an output sits in the compositor's global space, in logical points (`zxdg_output_v1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What `wl_output` reported about an output, as of its last `done` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// The connector name (version 4), such as `DP-1`.
    pub name: Option<String>,
    /// A human-readable description (version 4).
    pub description: Option<String>,
    /// The integer scale the compositor renders this output at.
    pub scale: Scale,
    /// The current mode in device pixels.
    pub mode: Option<(u32, u32)>,
    /// The logical position and size; `None` without [`crate::Capability::OutputLayout`] or before the compositor
    /// reported both.
    pub geometry: Option<OutputGeometry>,
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
