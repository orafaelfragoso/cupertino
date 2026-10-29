//! The cursor drawn by the client from the XCursor theme (`XCURSOR_THEME`, `XCURSOR_SIZE`), for a compositor without
//! `wp_cursor_shape_v1`. An animated cursor shows its first image.

use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use wayland_cursor::CursorTheme;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

use crate::connection::State;
use crate::Error;

/// The size in pixels at scale 1 when `XCURSOR_SIZE` doesn't say.
const DEFAULT_SIZE: u32 = 24;

/// The theme loaded at one scale and the surface the cursor is drawn on.
pub(crate) struct ThemedCursor {
    compositor: WlCompositor,
    shm: WlShm,
    theme: Option<(u32, CursorTheme)>,
    surface: Option<WlSurface>,
}

impl ThemedCursor {
    pub(crate) fn new(compositor: WlCompositor, shm: WlShm) -> ThemedCursor {
        ThemedCursor {
            compositor,
            shm,
            theme: None,
            surface: None,
        }
    }

    /// Show `shape` drawn for `scale` as the cursor of the pointer entered with `serial`.
    pub(crate) fn show(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<State>,
        pointer: &WlPointer,
        serial: u32,
        shape: Shape,
        scale: u32,
    ) -> Result<(), Error> {
        let scale = scale.max(1);
        if self.theme.as_ref().is_none_or(|(s, _)| *s != scale) {
            let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
            let size = std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(DEFAULT_SIZE);
            let theme = CursorTheme::load_from_name(
                conn,
                self.shm.clone(),
                &name,
                size.saturating_mul(scale),
            )
            .map_err(|_| Error::Protocol("the cursor theme's buffers"))?;
            self.theme = Some((scale, theme));
        }
        let (_, theme) = self.theme.as_mut().expect("loaded above");
        let cursor = names(shape)
            .iter()
            .chain(&["default", "left_ptr"])
            .find_map(|name| theme.get_cursor(name).map(|_| *name))
            .and_then(|name| theme.get_cursor(name))
            .ok_or(Error::Protocol("the cursor theme has no default cursor"))?;
        let image = &cursor[0];
        let (width, height) = image.dimensions();
        let (hx, hy) = image.hotspot();
        // a buffer scale must divide the buffer's size; a theme without a size for this scale draws it at 1
        let buffer_scale = if width.is_multiple_of(scale) && height.is_multiple_of(scale) {
            scale
        } else {
            1
        };
        let surface = self
            .surface
            .get_or_insert_with(|| self.compositor.create_surface(qh, ()));
        surface.set_buffer_scale(buffer_scale as i32);
        surface.attach(Some(&**image), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        pointer.set_cursor(
            serial,
            Some(surface),
            (hx / buffer_scale) as i32,
            (hy / buffer_scale) as i32,
        );
        Ok(())
    }
}

/// The theme's names for `shape`: the CSS name of the cursor specification, then older X11 names themes still use.
fn names(shape: Shape) -> &'static [&'static str] {
    match shape {
        Shape::Default => &["default", "left_ptr"],
        Shape::ContextMenu => &["context-menu"],
        Shape::Help => &["help", "question_arrow"],
        Shape::Pointer => &["pointer", "hand2"],
        Shape::Progress => &["progress", "left_ptr_watch"],
        Shape::Wait => &["wait", "watch"],
        Shape::Cell => &["cell", "plus"],
        Shape::Crosshair => &["crosshair", "cross"],
        Shape::Text => &["text", "xterm"],
        Shape::VerticalText => &["vertical-text"],
        Shape::Alias => &["alias", "dnd-link"],
        Shape::Copy => &["copy", "dnd-copy"],
        Shape::Move => &["move", "dnd-move"],
        Shape::NoDrop => &["no-drop", "dnd-none"],
        Shape::NotAllowed => &["not-allowed", "crossed_circle"],
        Shape::Grab => &["grab", "openhand"],
        Shape::Grabbing => &["grabbing", "closedhand"],
        Shape::EResize => &["e-resize", "right_side"],
        Shape::NResize => &["n-resize", "top_side"],
        Shape::NeResize => &["ne-resize", "top_right_corner"],
        Shape::NwResize => &["nw-resize", "top_left_corner"],
        Shape::SResize => &["s-resize", "bottom_side"],
        Shape::SeResize => &["se-resize", "bottom_right_corner"],
        Shape::SwResize => &["sw-resize", "bottom_left_corner"],
        Shape::WResize => &["w-resize", "left_side"],
        Shape::EwResize => &["ew-resize", "sb_h_double_arrow"],
        Shape::NsResize => &["ns-resize", "sb_v_double_arrow"],
        Shape::NeswResize => &["nesw-resize", "fd_double_arrow"],
        Shape::NwseResize => &["nwse-resize", "bd_double_arrow"],
        Shape::ColResize => &["col-resize", "sb_h_double_arrow"],
        Shape::RowResize => &["row-resize", "sb_v_double_arrow"],
        Shape::AllScroll => &["all-scroll", "fleur"],
        Shape::ZoomIn => &["zoom-in"],
        Shape::ZoomOut => &["zoom-out"],
        Shape::DndAsk => &["dnd-ask", "copy"],
        Shape::AllResize => &["all-resize", "fleur"],
        _ => &[],
    }
}
