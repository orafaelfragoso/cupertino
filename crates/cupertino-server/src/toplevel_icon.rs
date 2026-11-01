//! The icon a window sets for itself with `xdg_toplevel_icon_v1`, as the server keeps it: copied once when the
//! surface commits a new icon, so nothing of the client (its `wl_buffer`s, its pool) is read after that commit.
//! The name is only ever a key into the icon theme: a name that could be read as a path is dropped. Of the first
//! [`MAX_ICON_BUFFERS`] buffers, the largest square `wl_shm` one of at most [`MAX_ICON_PIXELS`] a side is chosen by
//! its size and format alone, and only it is copied, read through smithay's shm access; larger ones are refused.

use std::sync::{Arc, Mutex};

use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shm::with_buffer_contents;
use smithay::wayland::xdg_toplevel_icon::ToplevelIconCachedState;

/// Longest icon name kept, in bytes.
pub const MAX_ICON_NAME: usize = 255;
/// Largest icon side kept, in pixels.
pub const MAX_ICON_PIXELS: i32 = 512;
/// Icon buffers looked at; the rest are ignored.
pub const MAX_ICON_BUFFERS: usize = 32;

/// A copied icon image: `size` × `size` pixels of `wl_shm` ARGB8888 (premultiplied, little-endian), rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    pub size: u32,
    pub argb: Vec<u8>,
}

/// What the server keeps of a window's own icon. Empty when the window set none, or nothing of it was kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowIcon {
    pub name: Option<String>,
    pub image: Option<IconImage>,
}

impl WindowIcon {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.image.is_none()
    }
}

/// The copy of a surface's icon, and whether `set_icon` gave it a new one its next commit applies.
#[derive(Default)]
struct Copied {
    pending: bool,
    icon: Arc<WindowIcon>,
}

type CopiedIcon = Mutex<Copied>;

fn with_copied<T>(
    surface: &WlSurface,
    f: impl FnOnce(&mut Copied, &ToplevelIconCachedState) -> T,
) -> T {
    with_states(surface, |s| {
        s.data_map.insert_if_missing_threadsafe(CopiedIcon::default);
        let mut copied = s
            .data_map
            .get::<CopiedIcon>()
            .expect("inserted above")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(
            &mut copied,
            s.cached_state.get::<ToplevelIconCachedState>().current(),
        )
    })
}

/// Whether `name` may be looked up in the icon theme: not empty, at most [`MAX_ICON_NAME`] bytes, and nothing
/// that makes it a path (`/`, `..`, NUL).
pub fn icon_name_allowed(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ICON_NAME
        && !name.contains(['/', '\0'])
        && !name.contains("..")
}

/// The side of a square `wl_shm` ARGB8888 or XRGB8888 buffer of at most [`MAX_ICON_PIXELS`] a side, read from its
/// metadata without touching its pixels.
fn icon_side(buffer: &WlBuffer) -> Option<i32> {
    with_buffer_contents(buffer, |_, _, data| {
        (data.width > 0
            && data.width <= MAX_ICON_PIXELS
            && data.height == data.width
            && matches!(data.format, Format::Argb8888 | Format::Xrgb8888))
        .then_some(data.width)
    })
    .ok()
    .flatten()
}

/// The pixels of a square `wl_shm` buffer of at most [`MAX_ICON_PIXELS`] a side, in ARGB8888.
fn copy_image(buffer: &WlBuffer) -> Option<IconImage> {
    with_buffer_contents(buffer, |ptr, len, data| {
        let side = data.width;
        if side <= 0 || side > MAX_ICON_PIXELS || data.height != side {
            return None;
        }
        let opaque = match data.format {
            Format::Argb8888 => false,
            Format::Xrgb8888 => true,
            _ => return None,
        };
        let (side, offset, stride) = (
            side as usize,
            usize::try_from(data.offset).ok()?,
            usize::try_from(data.stride).ok()?,
        );
        let row = side * 4;
        if stride < row || offset.checked_add(stride * (side - 1) + row)? > len {
            return None;
        }
        // SAFETY: smithay maps the pool for `len` bytes for the duration of this closure, and the range read is
        // inside it (checked above).
        let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
        let mut argb = Vec::with_capacity(row * side);
        for y in 0..side {
            let start = offset + y * stride;
            argb.extend_from_slice(&pool[start..start + row]);
        }
        if opaque {
            argb.as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|px| px[3] = 0xff);
        }
        Some(IconImage {
            size: side as u32,
            argb,
        })
    })
    .ok()
    .flatten()
}

fn copy(state: &ToplevelIconCachedState) -> WindowIcon {
    let name = state
        .icon_name()
        .filter(|n| icon_name_allowed(n))
        .map(str::to_owned);
    let image = state
        .buffers()
        .iter()
        .take(MAX_ICON_BUFFERS)
        .filter_map(|(buffer, _)| Some((icon_side(buffer)?, buffer)))
        .max_by_key(|(side, _)| *side)
        .and_then(|(_, buffer)| copy_image(buffer));
    WindowIcon { name, image }
}

/// `xdg_toplevel_icon_manager_v1.set_icon` made an icon (or none) pending on the surface.
pub fn icon_set(surface: &WlSurface) {
    with_copied(surface, |copied, _| copied.pending = true);
}

/// Copy the icon a commit applied, once per `set_icon`. Whether the copy changed.
pub fn icon_committed(surface: &WlSurface) -> bool {
    with_copied(surface, |copied, current| {
        if !std::mem::take(&mut copied.pending) {
            return false;
        }
        let icon = copy(current);
        if *copied.icon == icon {
            return false;
        }
        copied.icon = Arc::new(icon);
        true
    })
}

/// The surface's copied icon; the same `Arc` until a commit changes it.
pub fn window_icon(surface: &WlSurface) -> Arc<WindowIcon> {
    with_copied(surface, |copied, _| copied.icon.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_theme_key_is_an_allowed_icon_name() {
        for ok in [
            "org.example.painter",
            "utilities-terminal",
            "a",
            &"x".repeat(255),
        ] {
            assert!(icon_name_allowed(ok), "{ok}");
        }
        for bad in [
            "",
            "/usr/share/icons/x.png",
            "icons/x",
            "..",
            "x..y",
            "a\0b",
            &"x".repeat(256),
        ] {
            assert!(!icon_name_allowed(bad), "{bad:?}");
        }
    }
}
