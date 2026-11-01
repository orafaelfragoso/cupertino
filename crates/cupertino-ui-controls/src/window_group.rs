use crate::WindowGroup;
use cupertino_ui_core::{Key, Shortcut, Window, WindowKey};
use cupertino_ui_platform::{Keysym, Mods};

const N: Keysym = Keysym(0x6e);
const W: Keysym = Keysym(0x77);

impl<M: Clone + 'static> WindowGroup<M> {
    /// The key of the group's window `id`.
    pub fn window_key(&self, id: u64) -> WindowKey {
        WindowKey(Key::from(format!("{}/{id}", self.key)))
    }

    /// The group's windows, in the order of `windows`.
    pub fn windows(self) -> Vec<Window<M>> {
        let keys: Vec<WindowKey> = self.windows.iter().map(|w| self.window_key(w.id)).collect();
        let WindowGroup {
            windows,
            on_new,
            on_close,
            ..
        } = self;
        windows
            .into_iter()
            .zip(keys)
            .map(|(window, key)| {
                let close = on_close(window.id);
                let mut shortcuts = window.shortcuts;
                shortcuts.insert(primary(N), on_new.clone());
                shortcuts.insert(primary(W), close.clone());
                Window {
                    key,
                    spec: window.spec,
                    shortcuts,
                    on_close: Some(close),
                    background: window.background,
                    content: window.content,
                    popup: None,
                }
            })
            .collect()
    }
}

fn primary(keysym: Keysym) -> Shortcut {
    Shortcut {
        mods: Mods::PRIMARY,
        keysym,
    }
}
