//! Pixels the compositor's own CPU drawing sent to the GPU, by what drew them: title bars and rims, and the keyboard
//! HUDs (the input-source HUD, the shortcut notice, the accent popup). The frame statistics read the totals before and
//! after each frame ([`crate::frame_stats`]).
//!
//! An upload is told by the renderer's own count of memory uploads, read around the import of each CPU-drawn image:
//! an image the GPU already holds costs nothing and counts nothing.

use std::cell::Cell;

use cupertino_scene::SceneKind;

/// Uploads and their bytes so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uploads {
    pub decoration: u64,
    pub decoration_bytes: u64,
    pub hud: u64,
    pub hud_bytes: u64,
}

/// What drew a CPU image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawer {
    Decoration,
    Hud,
}

/// The drawer of an image of `kind`; `None` for the images counted elsewhere or not at all (wallpaper, cursor).
pub fn drawer(kind: SceneKind) -> Option<Drawer> {
    match kind {
        SceneKind::TitleBar | SceneKind::Rim => Some(Drawer::Decoration),
        SceneKind::InputHud | SceneKind::ShortcutNotice | SceneKind::AccentPopup => {
            Some(Drawer::Hud)
        }
        _ => None,
    }
}

thread_local! {
    static TOTAL: Cell<Uploads> = Cell::new(Uploads::default());
}

/// `uploads` uploads of `bytes` bytes for an image of `kind`.
pub fn note(kind: SceneKind, uploads: u64, bytes: u64) {
    let Some(drawer) = drawer(kind) else {
        return;
    };
    TOTAL.with(|t| {
        let mut u = t.get();
        match drawer {
            Drawer::Decoration => {
                u.decoration += uploads;
                u.decoration_bytes += bytes;
            }
            Drawer::Hud => {
                u.hud += uploads;
                u.hud_bytes += bytes;
            }
        }
        t.set(u);
    });
}

/// The uploads of this thread so far.
pub fn total() -> Uploads {
    TOTAL.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_bars_and_rims_count_as_decorations_and_keyboard_overlays_as_huds() {
        assert_eq!(drawer(SceneKind::TitleBar), Some(Drawer::Decoration));
        assert_eq!(drawer(SceneKind::Rim), Some(Drawer::Decoration));
        for kind in [
            SceneKind::InputHud,
            SceneKind::ShortcutNotice,
            SceneKind::AccentPopup,
        ] {
            assert_eq!(drawer(kind), Some(Drawer::Hud));
        }
        assert_eq!(drawer(SceneKind::WallpaperImage), None);
        assert_eq!(drawer(SceneKind::CursorImage), None);
    }

    #[test]
    fn uploads_add_up_by_drawer_and_others_are_not_counted() {
        let before = total();
        note(SceneKind::TitleBar, 1, 400);
        note(SceneKind::Rim, 2, 100);
        note(SceneKind::InputHud, 1, 64);
        note(SceneKind::WallpaperImage, 1, 1 << 20);
        let after = total();
        assert_eq!(after.decoration - before.decoration, 3);
        assert_eq!(after.decoration_bytes - before.decoration_bytes, 500);
        assert_eq!(after.hud - before.hud, 1);
        assert_eq!(after.hud_bytes - before.hud_bytes, 64);
    }
}
