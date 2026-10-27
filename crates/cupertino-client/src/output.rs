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

impl From<cupertino_wayland_client::OutputInfo> for OutputInfo {
    fn from(info: cupertino_wayland_client::OutputInfo) -> Self {
        OutputInfo {
            id: OutputId(info.id.registry_name()),
            geometry: info.geometry.map(|g| OutputGeometry {
                x: g.x,
                y: g.y,
                width: g.width,
                height: g.height,
            }),
        }
    }
}
