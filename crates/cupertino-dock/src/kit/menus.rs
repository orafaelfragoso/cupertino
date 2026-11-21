//! The tiles' context menus and the Force Quit dialog.

use cupertino_kit::core::Command;
use cupertino_kit::platform::{KeyEvent, Mods};

use super::{
    AppAction, ForceQuit, Message, Model, OpenMenu, Request, DOWN, END, ESCAPE, HOME, KP_ENTER,
    RETURN, TRASH_URI, UP,
};
use crate::force_quit::{self, Button, Dialog, Entry, Extend};
use crate::menu::{self, MenuCommand, TileFacts, TileWindow};
use crate::tiles::{TileId, TileKind};
use crate::view::{self, Pointer};

impl Model {
    /// The menu of the tile or the separator.
    pub(super) fn open_menu(&mut self, id: TileId) {
        self.menu = None;
        self.confirm = None;
        self.stack = None;
        self.press = None;
        let Some(tile) = self.session.dock.tile(id) else {
            return;
        };
        let items = if tile.kind == TileKind::Separator {
            menu::separator_menu(&self.session.dock.cfg)
        } else {
            let entry_path = self
                .session
                .desktop()
                .find(&tile.key)
                .map(|e| e.path.as_path());
            let running_app_id = self.session.running_app_id(id);
            let windows = self
                .session
                .windows_of(&tile.key)
                .into_iter()
                .map(|(id, w)| TileWindow {
                    id,
                    title: w.title.clone(),
                })
                .collect();
            menu::tile_menu(&TileFacts {
                kind: tile.kind,
                key: &tile.key,
                pinned: tile.pinned,
                built_in: tile.built_in,
                running_app_id,
                unresponsive: running_app_id.is_some_and(|app| self.unresponsive.contains(app)),
                entry_path,
                trash_full: self.trash_full,
                windows,
            })
        };
        if items.is_empty() {
            return;
        }
        let label = self.tile_name(id);
        let dark = self.darkened_icon(id);
        self.menu_fade = None;
        self.menu = Some(OpenMenu {
            tile: id,
            items,
            label,
            dark,
        });
        self.menu_takes_keyboard = self.engaged;
    }

    /// Bring one window forward, as its row in a tile's menu asks: a minimized one flies out of its own tile, any other
    /// is raised and given the keyboard. A window that closed meanwhile is left alone.
    pub(super) fn show_window(&mut self, id: u32) {
        let Some(window) = self.session.window_info(id) else {
            return;
        };
        let app_id = window.app_id.clone();
        let tile = window
            .minimized
            .then(|| {
                self.session
                    .dock
                    .tiles()
                    .into_iter()
                    .find(|t| {
                        t.kind == TileKind::MinimizedWindow
                            && t.window.is_some_and(|w| w.0 == u64::from(id))
                    })
                    .map(|t| t.id)
            })
            .flatten()
            .and_then(|tile| self.tile_rect(tile));
        match tile {
            Some(tile) => self.bar_request(Request::RestoreWindow { id, tile }),
            None => self.bar_request(Request::ActivateWindow { id, app_id }),
        }
    }

    /// Another window of the file manager, the tile with `key`: on the person's home folder, or as a click on a tile
    /// of an application without windows asks where the host names no folder.
    pub(super) fn new_window(&mut self, key: String) -> Vec<Command<Message>> {
        match self.host.home() {
            Some(home) => self.open_in_file_manager(&home),
            None => self.reopen(key),
        }
    }

    pub(super) fn run_menu_command(&mut self, command: MenuCommand) -> Vec<Command<Message>> {
        let now = self.now();
        self.engaged = false;
        match command {
            MenuCommand::KeepInDock(key) => {
                self.session.dock.pin(&key, None, now);
                self.store_pins();
            }
            MenuCommand::RemoveFromDock(key) => {
                self.session.dock.unpin(&key, now);
                self.store_pins();
            }
            MenuCommand::ShowInFileManager(dir) => {
                return self.open_in_file_manager(&dir.to_string_lossy());
            }
            MenuCommand::ShowAllWindows(app) => {
                self.bar_request(Request::App(app, AppAction::ShowWindows))
            }
            MenuCommand::ShowWindow(id) => self.show_window(id),
            MenuCommand::NewWindow(key) => return self.new_window(key),
            MenuCommand::Hide(app) => self.bar_request(Request::App(app, AppAction::Hide)),
            MenuCommand::Quit(app) => self.bar_request(Request::App(app, AppAction::Quit)),
            MenuCommand::ForceQuit(app) => self.open_force_quit(Some(&app)),
            MenuCommand::Open(key) => return self.open_app(key),
            MenuCommand::OpenTrash => return self.open_in_file_manager(TRASH_URI),
            // asked for before it gets here ([`Input::MenuItem`])
            MenuCommand::EmptyTrash => {}
            MenuCommand::ToggleAutoHide => self.session.dock.toggle_autohide(now),
            MenuCommand::ToggleMagnification => {
                self.session.dock.cfg.magnification = !self.session.dock.cfg.magnification
            }
            // the items are disabled: the position is the file's
            MenuCommand::Position(_) => {}
        }
        Vec::new()
    }

    // ---- the Force Quit dialog -------------------------------------------------------------------------------

    /// The running applications for the dialog: the dock's running application tiles, the unresponsive ones marked.
    pub(super) fn force_quit_entries(&self) -> Vec<Entry> {
        self.session
            .dock
            .tiles()
            .into_iter()
            .filter(|t| t.kind == TileKind::App && t.running)
            .map(|t| Entry {
                app_id: t.key.clone(),
                name: self
                    .entry_name(&t.key)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        if t.title.is_empty() {
                            t.key.clone()
                        } else {
                            t.title.clone()
                        }
                    }),
                unresponsive: self.unresponsive.contains(&t.key),
                built_in: t.built_in,
            })
            .collect()
    }

    pub(super) fn toggle_force_quit(&mut self) {
        if self.force_quit.take().is_none() {
            self.open_force_quit(None);
        }
    }

    /// Open the dialog on the bar's output, on the second step for `ask_about` when that is given.
    pub(super) fn open_force_quit(&mut self, ask_about: Option<&str>) {
        let Some(output) = self.output.or(self.outputs.first().map(|o| o.key)) else {
            return;
        };
        let entries = self.force_quit_entries();
        let dialog = match ask_about {
            Some(app_id) => Dialog::asking_about(entries, app_id),
            None => Dialog::new(entries),
        };
        let layout = force_quit::Layout::for_dialog(&dialog);
        self.force_quit = Some(ForceQuit {
            output,
            dialog,
            layout,
            hover_row: None,
            hover_button: None,
        });
    }

    /// The applications or their answers changed: the open dialog follows.
    pub(super) fn refresh_force_quit(&mut self) {
        if self.force_quit.is_none() {
            return;
        }
        let entries = self.force_quit_entries();
        if let Some(open) = self.force_quit.as_mut() {
            if open.dialog.entries() != entries.as_slice() {
                open.dialog.set_entries(entries);
                open.layout = force_quit::Layout::for_dialog(&open.dialog);
            }
        }
    }

    pub(super) fn force_quit_outcome(&mut self, outcome: force_quit::Outcome) {
        match outcome {
            force_quit::Outcome::Nothing => {}
            // the second step has another size and other parts
            force_quit::Outcome::Redraw => {
                if let Some(open) = self.force_quit.as_mut() {
                    open.layout = force_quit::Layout::for_dialog(&open.dialog);
                }
            }
            force_quit::Outcome::Close => self.force_quit = None,
            force_quit::Outcome::Force(app_ids) => {
                for app_id in app_ids {
                    self.bar_request(Request::ForceQuitApp(app_id));
                }
                self.force_quit = None;
            }
        }
    }

    pub(super) fn force_quit_button(&mut self, button: Button) {
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        let confirming = open.dialog.confirming().is_some();
        let outcome = match (button, confirming) {
            (Button::Cancel, true) => open.dialog.cancel_pressed(),
            (Button::ForceQuit, true) => open.dialog.confirm_pressed(),
            (Button::ForceQuit, false) => open.dialog.force_quit_pressed(),
            (Button::Cancel, false) => force_quit::Outcome::Nothing,
        };
        self.force_quit_outcome(outcome);
    }

    pub(super) fn force_quit_pointer(&mut self, pointer: Pointer) {
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        match pointer {
            Pointer::Move(p) => {
                (open.hover_row, open.hover_button) = view::force_quit::hit(&open.layout, p);
            }
            Pointer::Leave => (open.hover_row, open.hover_button) = (None, None),
            Pointer::Press(p, mods) => {
                let (row, button) = view::force_quit::hit(&open.layout, p);
                if let Some(button) = button {
                    return self.force_quit_button(button);
                }
                if let Some(i) = row {
                    let how = if mods.contains(Mods::SHIFT) {
                        Extend::Range
                    } else if mods.contains(Mods::CTRL) || mods.contains(Mods::PRIMARY) {
                        Extend::Toggle
                    } else {
                        Extend::Replace
                    };
                    let outcome = open.dialog.select(i, how);
                    self.force_quit_outcome(outcome);
                }
            }
            Pointer::Scroll(_, delta) => {
                let dy = delta.y;
                let rows = (dy / force_quit::ROW_HEIGHT).round() as isize;
                let rows = if rows == 0 {
                    dy.signum() as isize
                } else {
                    rows
                };
                let outcome = open.dialog.scroll(rows);
                self.force_quit_outcome(outcome);
            }
            Pointer::Release(_) | Pointer::Secondary(_) => {}
        }
    }

    pub(super) fn force_quit_key(&mut self, key: KeyEvent) {
        if !key.pressed {
            return;
        }
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        let mapped = match key.keysym {
            ESCAPE => force_quit::Key::Escape,
            RETURN | KP_ENTER => force_quit::Key::Return,
            UP => force_quit::Key::Up,
            DOWN => force_quit::Key::Down,
            HOME => force_quit::Key::Home,
            END => force_quit::Key::End,
            _ => return,
        };
        let outcome = open.dialog.key(mapped, key.mods.contains(Mods::SHIFT));
        self.force_quit_outcome(outcome);
    }
}
