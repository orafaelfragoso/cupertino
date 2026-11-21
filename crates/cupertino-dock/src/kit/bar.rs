//! The bar: the pointer on it, a press, a drag and a drop, what a tile stands for when it is activated, the folders'
//! stacks, and the launches with their activation tokens.

use cupertino_kit::core::Command;
use cupertino_kit::platform::KeyEvent;
use cupertino_wm::geometry::{Point, Rect};

use super::{
    dropped_app, fan_basis, Closing, Message, Model, OpenStack, Opening, Pending, Request, Started,
    DOWN, END, ESCAPE, HOME, KP_ENTER, NO_WINDOWS, OPEN_GUARD, RETURN, SPACE, TOKEN_TIMEOUT,
    TRASH_URI, UP,
};
use crate::conf::FILE_MANAGER;
use crate::config::Orientation;
use crate::fan::{self, FanLayout};
use crate::session::Activation;
use crate::stack::{self, StackEntry, StackHit, StackLayout};
use crate::tile_drag::{Poof, TilePress};
use crate::tiles::{TileId, TileKind};
use crate::view::{self, IconSet, Pointer};
use crate::DragOutcome;

impl Model {
    /// The pointer on the bar. A left press on an application tile waits for the release or the first move (a press
    /// that moves on drags the tile); any other tile is activated at once.
    pub(super) fn bar_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let now = self.now();
        let Some(screen) = self.screen() else {
            return Vec::new();
        };
        let at = |p: cupertino_kit::geometry::Point| Point::new(p.x, p.y);
        if matches!(pointer, Pointer::Move(_)) {
            self.leaving_label = None;
        }
        match pointer {
            Pointer::Move(p) => {
                let p = at(p);
                self.pointer = Some(p);
                if let Some(press) = self.press.as_mut() {
                    let (began, id, origin) = (press.moved(p), press.id, press.origin);
                    if began {
                        self.held = None;
                        self.session.dock.begin_drag(id, origin, p, screen, now);
                    } else if self.session.dock.is_dragging() {
                        self.session.dock.drag_move(p, screen, now);
                    }
                }
                // the darkening goes when the pointer leaves the tile it is held on
                if let Some((id, _)) = &self.held {
                    let over = self.session.dock.hit_test(p, screen, now).map(|h| h.id);
                    if over != Some(*id) {
                        self.held = None;
                    }
                }
                Vec::new()
            }
            Pointer::Leave => {
                self.leaving_label = if self.session.dock.cfg.autohide {
                    self.label(&self.session.dock.layout(screen, now))
                } else {
                    None
                };
                self.pointer = None;
                self.held = None;
                Vec::new()
            }
            Pointer::Press(p, _) => {
                let p = at(p);
                // a release was lost (the server ended the grab): the drag of that press is over
                self.session.dock.cancel_drag();
                let hit = self.session.dock.hit_test(p, screen, now);
                self.held = hit
                    .as_ref()
                    .filter(|h| h.kind != TileKind::Separator)
                    .map(|h| (h.id, self.darkened_icon(h.id)));
                match hit {
                    Some(hit) if matches!(hit.kind, TileKind::App | TileKind::Recent) => {
                        self.press = Some(TilePress::new(hit.id, p, now));
                        Vec::new()
                    }
                    Some(hit) => {
                        self.press = None;
                        self.activate(hit.id)
                    }
                    None => {
                        self.press = None;
                        Vec::new()
                    }
                }
            }
            Pointer::Release(p) => self.release(at(p)),
            Pointer::Secondary(p) => {
                if let Some(hit) = self.session.dock.hit_test(at(p), screen, now) {
                    self.open_menu(hit.id);
                }
                Vec::new()
            }
            Pointer::Scroll(..) => Vec::new(),
        }
    }

    /// A left release: a drag drops the tile, a press that stayed in place is a click.
    pub(super) fn release(&mut self, position: Point) -> Vec<Command<Message>> {
        self.held = None;
        let Some(press) = self.press.take() else {
            return Vec::new();
        };
        if !press.is_dragging() {
            return self.activate(press.id);
        }
        let now = self.now();
        let Some(screen) = self.screen() else {
            return Vec::new();
        };
        match self.session.dock.end_drag(screen, now) {
            DragOutcome::Cancelled => {}
            DragOutcome::Reordered { .. } => self.store_pins(),
            DragOutcome::Removed { kind, .. } => {
                self.poof = Some(Poof {
                    centre: position,
                    started: now,
                });
                if kind == TileKind::App {
                    self.store_pins();
                }
            }
        }
        Vec::new()
    }

    pub(super) fn store_pins(&mut self) {
        let pins = self.session.dock.pinned_keys();
        let stored = self.host.store_pins(&pins);
        self.later.extend(stored);
    }

    /// A drop on the bar: the first application desktop file of a `text/uri-list` the session knows is kept in the
    /// dock at the slot the drop landed on, among the applications; anything else is refused, so its source keeps it.
    pub(super) fn dropped(
        &mut self,
        dropped: cupertino_kit::core::Dropped,
    ) -> Vec<Command<Message>> {
        let refuse = vec![Command::FinishDrop(dropped.token, None)];
        let text = String::from_utf8_lossy(&dropped.data.bytes);
        let Some(key) = dropped_app(&text).filter(|key| {
            self.session
                .desktop()
                .find(key)
                .is_some_and(|e| !e.no_display)
        }) else {
            return refuse;
        };
        let Some(screen) = self.screen() else {
            return refuse;
        };
        let now = self.now();
        let orientation = self.session.dock.cfg.orientation;
        let layout = self.session.dock.layout(screen, now);
        // the slot: how many application tiles stand before the point along the bar
        let along = |p: Point| match orientation {
            Orientation::Bottom => p.x,
            _ => p.y,
        };
        let at = along(Point::new(dropped.position.x, dropped.position.y));
        let index = layout
            .tiles
            .iter()
            .filter(|t| t.kind == TileKind::App)
            .filter(|t| along(t.rect.center()) < at)
            .count();
        self.learn_names([key.as_str()]);
        self.session.dock.pin(&key, Some(index), now);
        self.store_pins();
        vec![Command::FinishDrop(
            dropped.token,
            Some(cupertino_kit::platform::DropAction::Copy),
        )]
    }

    /// Look the desktop entry's name of every key in `keys` up once.
    pub(super) fn learn_names<'k>(&mut self, keys: impl IntoIterator<Item = &'k str>) {
        for key in keys {
            if !self.names.contains_key(key) {
                let name = self.session.desktop().find(key).map(|e| e.name.clone());
                self.names.insert(key.to_owned(), name);
            }
        }
    }

    /// The name of the desktop entry of the application `key`, as [`Self::learn_names`] looked it up.
    pub(super) fn entry_name(&self, key: &str) -> Option<&str> {
        self.names.get(key).and_then(|name| name.as_deref())
    }

    /// Whether the application of tile `id` runs without windows.
    pub(super) fn windowless(&self, id: TileId) -> bool {
        self.session.dock.tile(id).is_some_and(|t| {
            t.kind == TileKind::App && t.running && !self.session.has_windows(&t.key)
        })
    }

    /// How much of its colour a pressed tile's icon keeps.
    const PRESSED_ICON: f32 = 0.405;

    /// The icon of tile `id` darkened as a pressed tile shows it.
    pub(super) fn darkened_icon(&self, id: TileId) -> IconSet {
        let icon = self
            .tile_icon(id)
            .map(|key| self.icon(&key, &self.drawn_sides()))
            .unwrap_or_default();
        view::paint::darkened(&icon, Self::PRESSED_ICON)
    }

    /// The tile's rectangle in the bar's window, as it is laid out now.
    pub(super) fn tile_rect(&self, id: TileId) -> Option<Rect> {
        let screen = self.screen()?;
        self.session
            .dock
            .layout(screen, self.now())
            .tiles
            .into_iter()
            .find(|t| t.id == id)
            .map(|t| t.rect)
    }

    /// Carry out what the tile stands for; the bar gives the keyboard back.
    pub(super) fn activate(&mut self, id: TileId) -> Vec<Command<Message>> {
        self.engaged = false;
        match self.session.activation(id) {
            Some(Activation::ActivateApp(app_id)) => {
                self.bar_request(Request::ActivateApp(app_id));
                Vec::new()
            }
            Some(Activation::RestoreWindow(window)) => {
                if let Some(tile) = self.tile_rect(id) {
                    self.bar_request(Request::RestoreWindow { id: window, tile });
                }
                Vec::new()
            }
            Some(Activation::Launch(key)) => self.begin_launch(key),
            Some(Activation::Reopen { key, .. }) => self.reopen(key),
            Some(Activation::OpenTrash) => self.open_in_file_manager(TRASH_URI),
            Some(Activation::ToggleStack(id)) => self.toggle_stack(id),
            None => Vec::new(),
        }
    }

    /// Open the stack of folder tile `id` over it, or close it when it is the one open (or still being listed).
    /// Opening it closes a menu; it shows once its folder is listed ([`Model::folder_listed`]).
    pub(super) fn toggle_stack(&mut self, id: TileId) -> Vec<Command<Message>> {
        if self.stack.as_ref().is_some_and(|s| s.tile == id) {
            self.close_stack();
            return Vec::new();
        }
        self.stack = None;
        let Some(tile) = self.session.dock.tile(id) else {
            return Vec::new();
        };
        let folder = tile.key.clone();
        self.menu = None;
        self.press = None;
        let listing = self.next_listing;
        self.next_listing += 1;
        let listed = self.host.folder_entries(&folder);
        self.stack = Some(OpenStack {
            tile: id,
            name: stack::folder_name(&folder),
            folder,
            listing,
            entries: None,
            opened: self.now(),
            shape: fan::Shape::Grid,
            closing: None,
            hover: None,
            selected: None,
        });
        vec![Command::Perform(Box::pin(async move {
            Message::FolderListed {
                listing,
                entries: listed.await,
            }
        }))]
    }

    /// The folder of the stack waiting for `listing` was listed: the stack shows, coming in from now. A listing for a
    /// stack that closed, or was opened again, meanwhile is dropped.
    pub(super) fn folder_listed(&mut self, listing: u64, mut entries: Vec<StackEntry>) {
        let now = self.now();
        let Some(tile) = self
            .stack
            .as_ref()
            .filter(|s| s.listing == listing && s.entries.is_none())
            .map(|s| s.tile)
        else {
            return;
        };
        entries.truncate(stack::MAX_ENTRIES);
        let shape = self.stack_shape(tile, entries.len());
        if let Some(open) = self.stack.as_mut() {
            open.entries = Some(entries);
            open.opened = now;
            open.shape = shape;
        }
    }

    /// Fan or grid for the stack of `tile` with `count` entries: the setting's choice (`dock.conf`'s `stack`), a grid
    /// for a folder with more entries than the fan holds and where the fan has no room.
    pub(super) fn stack_shape(&self, tile: TileId, count: usize) -> fan::Shape {
        let bottom = self.session.dock.cfg.orientation == Orientation::Bottom;
        // the fan rises over the whole output, not only over the bar's own window
        let output = self.output.and_then(|key| self.output_info(key));
        let room = self.screen().zip(output).map_or(0, |(screen, output)| {
            let layout = self.session.dock.layout(screen, self.now());
            fan_basis(&layout, screen, tile).map_or(0, |b| fan::room(output.height, b.above_screen))
        });
        fan::shape(self.conf.stack.unwrap_or_default(), count, room, bottom)
    }

    /// The layout of the fan of the open stack, as the bar lays out now.
    pub(super) fn fan_layout(&self) -> Option<FanLayout> {
        let open = self.stack.as_ref()?;
        let entries = open.entries.as_ref()?;
        let screen = self.screen()?;
        let layout = self.session.dock.layout(screen, self.now());
        let basis = fan_basis(&layout, screen, open.tile)?;
        Some(FanLayout::new(
            &fan::names(entries),
            stack::OPEN_FOLDER,
            basis.lowest,
        ))
    }

    /// Close the open stack: a fan folds back into its tile over [`fan::CLOSE_S`], a grid goes at once.
    pub(super) fn close_stack(&mut self) {
        let now = self.now();
        let motion = !self.reduce_motion;
        let Some(open) = self.stack.as_mut() else {
            return;
        };
        if open.closing.is_some() {
            return;
        }
        if open.shape == fan::Shape::Fan && open.entries.is_some() && motion {
            let from = fan::progress(now, open.opened, None, false);
            open.closing = Some(Closing { start: now, from });
        } else {
            self.stack = None;
        }
    }

    /// Whether a stack, still moving in or out, wants the next frame.
    pub(super) fn stack_moving(&self, now: f64) -> bool {
        self.stack.as_ref().is_some_and(|s| {
            s.entries.is_some()
                && s.shape == fan::Shape::Fan
                && fan::moving(now, s.opened, s.closing.map(|c| c.start))
        })
    }

    /// The tile whose name shows over it, and the name: the tile under the pointer, while no menu, stack, press or drag
    /// is on the bar. A separator has none.
    pub(super) fn label(
        &self,
        layout: &crate::dock::DockLayout,
    ) -> Option<(TileId, String, Option<&'static str>)> {
        // an ordinary click keeps the name; a drag, a menu or an open stack takes it down
        if self.menu.is_some()
            || self.confirm.is_some()
            || self.stack.is_some()
            || self.press.as_ref().is_some_and(|p| p.is_dragging())
            || layout.dragging.is_some()
            || self.locked
        {
            return None;
        }
        let id = self.session.dock.hovered()?;
        let tile = layout.tiles.iter().find(|t| t.id == id)?;
        if tile.kind == TileKind::Separator {
            return None;
        }
        let name = self.tile_name(id);
        let status = self.windowless(id).then_some(NO_WINDOWS);
        (!name.is_empty()).then_some((id, name, status))
    }

    /// The pointer on the open stack. A grid takes a click on its release: an entry opens and the stack closes, the
    /// button opens the folder. A fan highlights the row under the pointer and takes the same click; a click on none
    /// of its rows closes it.
    pub(super) fn stack_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let Some(open) = self.stack.as_ref().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        let Some(entries) = open.entries.as_ref() else {
            return Vec::new();
        };
        if open.shape == fan::Shape::Fan {
            return self.fan_pointer(pointer);
        }
        let Pointer::Release(p) = pointer else {
            return Vec::new();
        };
        match StackLayout::new(entries.len()).hit(Point::new(p.x, p.y), entries.len()) {
            Some(StackHit::Entry(i)) => self.open_stack_entry(i),
            Some(StackHit::OpenFolder) => self.open_stack_folder(),
            None => Vec::new(),
        }
    }

    /// The pointer on the open fan.
    pub(super) fn fan_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let Some(layout) = self.fan_layout() else {
            return Vec::new();
        };
        let row_at = |model: &Self, p: cupertino_kit::geometry::Point| {
            let entries = model.stack.as_ref()?.entries.as_ref()?;
            layout.hit(
                Point::new(p.x, p.y),
                &fan::names(entries),
                stack::OPEN_FOLDER,
            )
        };
        match pointer {
            Pointer::Move(p) => {
                let row = row_at(self, p);
                if let Some(open) = self.stack.as_mut() {
                    open.hover = row;
                    open.selected = None;
                }
                Vec::new()
            }
            Pointer::Leave => {
                if let Some(open) = self.stack.as_mut() {
                    open.hover = None;
                }
                Vec::new()
            }
            Pointer::Release(p) => match row_at(self, p) {
                Some(row) if row < layout.entries => self.open_stack_entry(row),
                Some(_) => self.open_stack_folder(),
                None => {
                    self.close_stack();
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    /// A key on the open stack that none of its parts took: Escape closes it, and on a fan the arrows, Home and End
    /// move the selection (the lowest row first), Return and Space choose the selected row.
    pub(super) fn stack_key(&mut self, key: KeyEvent) -> Vec<Command<Message>> {
        if !key.pressed {
            return Vec::new();
        }
        let Some(open) = self.stack.as_mut().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        if key.keysym == ESCAPE {
            self.close_stack();
            return Vec::new();
        }
        let Some(entries) = open.entries.as_ref().map(Vec::len) else {
            return Vec::new();
        };
        if open.shape != fan::Shape::Fan {
            return Vec::new();
        }
        // the button is the row after the last entry
        let top = entries;
        let current = open.selected.or(open.hover);
        let moved = match key.keysym {
            UP => Some(current.map_or(0, |i| (i + 1).min(top))),
            DOWN => Some(current.map_or(0, |i| i.saturating_sub(1))),
            HOME => Some(0),
            END => Some(top),
            RETURN | KP_ENTER | SPACE => {
                return match current {
                    Some(row) if row < top => self.open_stack_entry(row),
                    Some(_) => self.open_stack_folder(),
                    None => Vec::new(),
                };
            }
            _ => None,
        };
        if let Some(row) = moved {
            open.selected = Some(row);
            open.hover = None;
        }
        Vec::new()
    }

    pub(super) fn open_stack_entry(&mut self, index: usize) -> Vec<Command<Message>> {
        let Some(open) = self.stack.as_ref().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        let path = open
            .entries
            .as_ref()
            .and_then(|e| e.get(index))
            .map(|entry| entry.path.to_string_lossy().into_owned());
        if let Some(path) = path {
            self.host.open(&path);
        }
        self.close_stack();
        Vec::new()
    }

    pub(super) fn open_stack_folder(&mut self) -> Vec<Command<Message>> {
        let Some(folder) = self
            .stack
            .as_ref()
            .filter(|s| s.closing.is_none())
            .map(|s| s.folder.clone())
        else {
            return Vec::new();
        };
        self.close_stack();
        self.open_in_file_manager(&folder)
    }

    /// The folders the host shows as stacks, asked for once.
    pub(super) fn ask_folders(&mut self) {
        if std::mem::replace(&mut self.folders_asked, true) {
            return;
        }
        let folders = self.host.folders();
        let paths: Vec<&str> = folders.iter().map(|(p, _)| p.as_str()).collect();
        self.session.dock.set_folders(&paths);
        self.folder_icons = folders.into_iter().collect();
    }

    /// Open the application of tile `key` as a click on its tile does, also when it has no tile: raise its windows,
    /// ask its resident process for a window, or start it.
    pub(super) fn open_app(&mut self, key: String) -> Vec<Command<Message>> {
        match self.session.opening(&key) {
            Activation::ActivateApp(app_id) => {
                self.bar_request(Request::ActivateApp(app_id));
                Vec::new()
            }
            Activation::Reopen { key, .. } => self.reopen(key),
            Activation::Launch(key) => self.begin_launch(key),
            Activation::RestoreWindow(_) | Activation::OpenTrash | Activation::ToggleStack(_) => {
                Vec::new()
            }
        }
    }

    /// Whether the application of tile `key` was asked to open a moment ago and shows no window yet: a second ask
    /// would open it twice.
    pub(super) fn opening_now(&self, key: &str) -> bool {
        let now = self.now();
        self.pending
            .values()
            .any(|p| p.key == key && now - p.asked < TOKEN_TIMEOUT)
            || self
                .opening
                .get(key)
                .is_some_and(|&at| now - at < OPEN_GUARD)
    }

    /// Ask the resident process of tile `key` for a new window: through `org.freedesktop.Application.Activate` when its
    /// desktop entry is D-Bus activatable, else by starting the entry again (a running instance opens a window). The
    /// tile keeps its running dot and does not bounce.
    pub(super) fn reopen(&mut self, key: String) -> Vec<Command<Message>> {
        if self.opening_now(&key) {
            return Vec::new();
        }
        match self.session.bus_name(&key) {
            Some(name) => {
                self.opening.insert(key.clone(), self.now());
                self.ask_token(key, Opening::Activate(name))
            }
            None => self.start(key, false),
        }
    }

    /// Open `target` (a folder or the trash) in the session's file manager like a launch of its tile (an activation
    /// token, so its window takes the focus, and a bounce); without it through the default handler.
    pub(super) fn open_in_file_manager(&mut self, target: &str) -> Vec<Command<Message>> {
        match self.session.file_manager_command(target) {
            Some(argv) => self.ask_token(
                FILE_MANAGER.to_owned(),
                Opening::Spawn { argv, bounce: true },
            ),
            None => {
                self.host.open(target);
                Vec::new()
            }
        }
    }

    /// Fetch an activation token for the launch of the tile with `key`; the program starts when it arrives. A launch
    /// asked for again before the first one showed a window is dropped.
    pub(super) fn begin_launch(&mut self, key: String) -> Vec<Command<Message>> {
        if self.opening_now(&key) {
            return Vec::new();
        }
        self.start(key, true)
    }

    /// Start the entry of tile `key` with an activation token, bouncing its tile when `bounce` is set.
    pub(super) fn start(&mut self, key: String, bounce: bool) -> Vec<Command<Message>> {
        let launch = self.session.launch(&key);
        let mut argv = launch.argv;
        if launch.terminal {
            let Some(terminal) = self.host.terminal() else {
                eprintln!("cupertino-dock: {key} runs in a terminal and none is installed");
                return Vec::new();
            };
            argv.splice(0..0, [terminal, "-e".to_owned()]);
        }
        if argv.is_empty() {
            eprintln!("cupertino-dock: {key} has no command");
            return Vec::new();
        }
        self.opening.insert(key.clone(), self.now());
        self.ask_token(key, Opening::Spawn { argv, bounce })
    }

    pub(super) fn ask_token(&mut self, key: String, opening: Opening) -> Vec<Command<Message>> {
        let launch = self.next_launch;
        self.next_launch += 1;
        let asked = self.now();
        self.pending.insert(
            launch,
            Pending {
                key,
                opening,
                asked,
            },
        );
        let window = self
            .bar_key()
            .unwrap_or_else(|| view::bar_key(0, self.bar_generation));
        vec![Command::ActivationToken {
            window,
            app_id: None,
            on_token: Box::new(move |token| Message::Token(launch, token)),
        }]
    }

    /// Start the program that waited for `token` and bounce its tile, or ask the application for a window; an
    /// application that cannot be asked is started again with the token.
    pub(super) fn complete(&mut self, launch: u64, token: Option<String>) -> Vec<Command<Message>> {
        let Some(Pending { key, opening, .. }) = self.pending.remove(&launch) else {
            return Vec::new();
        };
        match opening {
            Opening::Spawn { argv, bounce } => {
                let started = self.host.spawn(argv, token);
                if matches!(started, Started::No) {
                    self.opening.remove(&key);
                    return Vec::new();
                }
                if bounce {
                    let now = self.now();
                    self.session.dock.app_launching(&key, now);
                }
                // the guard against a second launch ends with a process that fails: a program that crashes at once
                // can be opened again straight away
                let (Started::Watched(exited), Some(&asked)) = (started, self.opening.get(&key))
                else {
                    return Vec::new();
                };
                vec![Command::Perform(Box::pin(async move {
                    let failed = exited.await;
                    Message::Exited { key, asked, failed }
                }))]
            }
            Opening::Activate(name) => match self.host.dbus_activate(&name, token) {
                Some(call) => vec![Command::Perform(Box::pin(async move {
                    Message::BusActivated {
                        key,
                        ok: call.await,
                    }
                }))],
                None => {
                    self.opening.remove(&key);
                    self.start(key, false)
                }
            },
        }
    }

    /// Launches whose activation token did not come within [`TOKEN_TIMEOUT`] go ahead without one.
    pub(super) fn expire_tokens(&mut self, now: f64) -> Vec<Command<Message>> {
        let mut late: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now - p.asked >= TOKEN_TIMEOUT)
            .map(|(launch, _)| *launch)
            .collect();
        late.sort_unstable();
        late.into_iter()
            .flat_map(|launch| self.complete(launch, None))
            .collect()
    }

    /// The program started for tile `key` when it was asked to open at `asked` exited: with no window of it shown,
    /// the guard against a second launch goes, so asking again starts it again. A later ask has a guard of its own.
    ///
    /// Only a failed process ends it: a launcher that hands the application on and exits successfully (a wrapper that
    /// starts it in the background, or one that passes the open to an instance already running) is followed by a
    /// window that has not come yet, and a second ask meanwhile would open the application twice.
    pub(super) fn exited(&mut self, key: &str, asked: f64, failed: bool) {
        if failed && self.opening.get(key) == Some(&asked) && !self.session.has_windows(key) {
            self.opening.remove(key);
        }
    }

    /// The bus call for a window of tile `key` ended: when it failed and no window came meanwhile, start the entry
    /// again instead (a running instance opens a window).
    pub(super) fn bus_activated(&mut self, key: String, ok: bool) -> Vec<Command<Message>> {
        if ok || self.session.has_windows(&key) {
            return Vec::new();
        }
        self.opening.remove(&key);
        self.start(key, false)
    }
}
