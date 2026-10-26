//! Outputs of the server and their logical geometry (`zxdg_output_v1`).

/// Handle of an output: the registry name of its `wl_output` global.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutputId(pub(crate) u32);

impl OutputId {
    /// The registry name of the output's `wl_output` global, to bind the same output on another event queue of the
    /// connection.
    pub fn registry_name(self) -> u32 {
        self.0
    }
}

/// Position and size of an output in the compositor space, in logical points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// An output as the application sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// `None` until the server has described the output.
    pub geometry: Option<OutputGeometry>,
}

/// Collects the position and size events of an `xdg_output` until its `done` event. A group
/// that carries only one of the two keeps the other from the previous group.
#[derive(Debug, Default)]
pub(crate) struct GeometryBuilder {
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
    committed: Option<OutputGeometry>,
}

impl GeometryBuilder {
    pub(crate) fn position(&mut self, x: i32, y: i32) {
        self.position = Some((x, y));
    }

    /// Negative extents are a server bug and stay unreported; the caller turns `false` into a
    /// protocol error.
    pub(crate) fn size(&mut self, width: i32, height: i32) -> bool {
        match (u32::try_from(width), u32::try_from(height)) {
            (Ok(w), Ok(h)) => {
                self.size = Some((w, h));
                true
            }
            _ => false,
        }
    }

    /// The group ended. Returns the geometry when it is complete and different from the last one.
    pub(crate) fn done(&mut self) -> Option<OutputGeometry> {
        let ((x, y), (width, height)) = self.position.zip(self.size)?;
        let geometry = OutputGeometry {
            x,
            y,
            width,
            height,
        };
        (self.committed.replace(geometry) != Some(geometry)).then_some(geometry)
    }

    pub(crate) fn geometry(&self) -> Option<OutputGeometry> {
        self.committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_needs_position_and_size() {
        let mut b = GeometryBuilder::default();
        b.position(1920, 0);
        assert_eq!(b.done(), None);
        assert!(b.size(1280, 800));
        assert_eq!(
            b.done(),
            Some(OutputGeometry {
                x: 1920,
                y: 0,
                width: 1280,
                height: 800
            })
        );
    }

    #[test]
    fn a_partial_group_keeps_the_other_half_and_repeats_are_silent() {
        let mut b = GeometryBuilder::default();
        b.position(0, 0);
        b.size(1280, 800);
        b.done();
        assert_eq!(b.done(), None);
        b.size(1920, 1080);
        assert_eq!(
            b.done(),
            Some(OutputGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(
            b.geometry(),
            Some(OutputGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn negative_sizes_are_refused() {
        let mut b = GeometryBuilder::default();
        assert!(!b.size(-1, 800));
        assert!(!b.size(1280, -5));
    }
}
