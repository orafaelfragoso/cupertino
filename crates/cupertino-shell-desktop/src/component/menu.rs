//! What the desktop's menus, keys and launches do: the context menu's rows, the shortcuts, the commands they send and
//! the results of the work those start.

use std::collections::BTreeSet;
use std::path::PathBuf;

use cupertino_kit::controls::{MenuItem as KitItem, TextField};
use cupertino_kit::core::{Command, CommandRegistry, Element, Shortcut};
use cupertino_kit::files::geom::Direction;
use cupertino_kit::files::notice::Action;
use cupertino_kit::files::operations::{DropAction, Transfer};
use cupertino_kit::files::sort::SortKey;
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{Keysym, Mods};
use cupertino_kit::text::ContentPurpose;

use super::work::{self, Launch};
use super::{desk_key, name_field_key, DeskCommand, Message, Model, OpenMenu, Running};
use crate::arrangement::{self, Arrangement};
use crate::metrics as m;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const BACKSPACE: Keysym = Keysym(0xff08);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);

impl Model {
    /// Open `paths` with the application of desktop entry `with`, else each one's own: what opens each is read on
    /// the I/O pool, then each asks for an activation token naming that application, so the dock shows it starting.
    pub(super) fn open(&self, paths: Vec<PathBuf>, with: Option<String>) -> Vec<Command<Message>> {
        if paths.is_empty() {
            return Vec::new();
        }
        let items = paths
            .into_iter()
            .map(|path| {
                let is_dir = self.is_dir(&path);
                (path, is_dir)
            })
            .collect();
        vec![work::resolve(items, with)]
    }

    /// Ask for the activation token of `launch`, naming `app_id`; without a screen it starts with none.
    fn launch(&self, app_id: Option<String>, launch: Launch) -> Command<Message> {
        match self.main() {
            Some(main) => Command::ActivationToken {
                window: desk_key(main.number),
                app_id,
                on_token: Box::new(move |token| Message::Launch(launch.clone(), token)),
            },
            None => Command::Perform(Box::pin(async move { Message::Launch(launch, None) })),
        }
    }

    /// Start a copy or a move of `sources`, its first slice on the I/O pool.
    pub(super) fn transfer(
        &mut self,
        action: Action,
        sources: Vec<PathBuf>,
        transfer: Transfer,
    ) -> Vec<Command<Message>> {
        if sources.is_empty() {
            return Vec::new();
        }
        self.next_transfer += 1;
        let number = self.next_transfer;
        self.transfers.push(Running {
            number,
            action,
            sources,
            transfer,
        });
        self.next_slice(number)
    }

    /// The transfer `number` after a slice ran: its next slice, or its failures reported once it has ended.
    fn next_slice(&mut self, number: u64) -> Vec<Command<Message>> {
        let Some(at) = self.transfers.iter().position(|r| r.number == number) else {
            return Vec::new();
        };
        if let Some(results) = self.transfers[at].transfer.results() {
            let running = self.transfers.remove(at);
            let results: Vec<_> = running.sources.into_iter().zip(results).collect();
            self.failed_some(running.action, &results);
            return Vec::new();
        }
        work::slice(&mut self.transfers[at]).into_iter().collect()
    }

    /// The context menu of the selection, or of the desktop, on screen `output` at `at`; the applications of Open
    /// With are read on the I/O pool.
    pub(super) fn context_menu(&mut self, output: u64, at: Point) -> Vec<Command<Message>> {
        self.menus += 1;
        let on_items = !self.selection.is_empty();
        self.menu = Some(OpenMenu {
            number: self.menus,
            output,
            at,
            on_items,
            open: Vec::new(),
            choices: None,
        });
        match self.selected().into_iter().next() {
            Some(first) if on_items => {
                let is_dir = self.is_dir(&first);
                vec![work::choices(self.menus, first, is_dir)]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn command(&mut self, command: DeskCommand) -> Vec<Command<Message>> {
        let at = self.menu.as_ref().map(|menu| menu.at);
        self.menu = None;
        match command {
            DeskCommand::Open => return self.open(self.selected(), None),
            DeskCommand::OpenWith(id) => return self.open(self.selected(), Some(id)),
            DeskCommand::MoveToTrash => {
                let paths = self.selected();
                self.selection.clear();
                if !paths.is_empty() {
                    return vec![work::trash(paths)];
                }
            }
            DeskCommand::Rename => {
                if let [only] = self.selected().as_slice() {
                    return self.begin_rename(only.clone());
                }
            }
            DeskCommand::Duplicate => {
                let paths = self.selected();
                let transfer = Transfer::duplicate(paths.clone());
                return self.transfer(Action::Duplicate, paths, transfer);
            }
            DeskCommand::MakeAlias => {
                let paths = self.selected();
                if !paths.is_empty() {
                    return vec![work::aliases(paths)];
                }
            }
            DeskCommand::Copy => {
                self.pasteboard = self.selected();
                let text = self
                    .pasteboard
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    return vec![Command::SetClipboard(text)];
                }
            }
            DeskCommand::Paste => {
                if let Some(dest) = self.folder.clone() {
                    let sources = self.pasteboard.clone();
                    let transfer = Transfer::into_folder(sources.clone(), dest, DropAction::Copy);
                    return self.transfer(Action::Paste, sources, transfer);
                }
            }
            DeskCommand::NewFolder => {
                if let Some(folder) = self.folder.clone() {
                    return vec![work::new_folder(folder, at)];
                }
            }
            DeskCommand::ChangeWallpaper => {
                let (app_id, launch) = Launch::wallpaper();
                return vec![self.launch(app_id, launch)];
            }
            DeskCommand::SortBy(key) => {
                self.prefs.arrangement = key.map_or(Arrangement::Free, Arrangement::Sorted);
                self.save();
                self.relist();
            }
            DeskCommand::CleanUp => {
                if let Some(grid) = self.grid() {
                    self.slots = arrangement::cleaned(&self.slots, &grid);
                    self.keep_slots();
                }
            }
            DeskCommand::SelectAll => {
                self.selection = self.entries.iter().map(|e| e.path.clone()).collect();
            }
            DeskCommand::ClearSelection => self.selection.clear(),
            DeskCommand::Step(direction) => {
                let cursor = self
                    .entries
                    .iter()
                    .rposition(|e| self.selection.contains(&e.path));
                let next = match cursor {
                    Some(from) => arrangement::neighbour(&self.slots, from, direction),
                    None => (!self.entries.is_empty()).then_some(0),
                };
                if let Some(next) = next {
                    self.selection = BTreeSet::from([self.entries[next].path.clone()]);
                }
            }
        }
        Vec::new()
    }

    /// What the work the desktop started brought back.
    pub(super) fn worked(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Resolved(openings) => {
                let mut commands = Vec::new();
                for opening in openings {
                    match opening.launch {
                        Ok(launch) => commands.push(self.launch(opening.app_id, launch)),
                        Err(e) => self.failed(Action::Open, &[&opening.path], &e),
                    }
                }
                return commands;
            }
            Message::Launch(launch, token) => {
                if !cupertino_kit_shell::host::spawn(launch.command(token.as_deref())) {
                    eprintln!("cupertino-shell-desktop: cannot start what opens an item");
                }
            }
            Message::Renamed { from, to } => match to {
                Ok(to) => {
                    self.renamed = Some(to);
                    self.relist();
                }
                Err(e) => self.failed(Action::Rename, &[&from], &e),
            },
            Message::Made(made, at) => match made {
                Ok(path) => {
                    self.place_made(&path, at);
                    self.made = Some(path);
                    self.relist();
                    return self.rename_made();
                }
                Err(e) => self.failed(Action::NewFolder, &[], &e),
            },
            Message::Trashed(moved, sizes) => {
                self.failed_some(Action::Trash, &moved);
                if !sizes.is_empty() {
                    return vec![work::record_sizes(sizes)];
                }
            }
            Message::Aliased(made) => self.failed_some(Action::MakeAlias, &made),
            Message::Sliced(number) => return self.next_slice(number),
            Message::Saved(error) => {
                self.saving = false;
                if let Some(e) = error {
                    eprintln!(
                        "cupertino-shell-desktop: cannot keep the desktop's arrangement: {e}"
                    );
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// A folder just made at `path` keeps the free cell nearest to `at`, where the menu that made it was opened.
    fn place_made(&mut self, path: &std::path::Path, at: Option<Point>) {
        let (Some(at), Some(grid), Arrangement::Free) = (at, self.grid(), self.prefs.arrangement)
        else {
            return;
        };
        let taken = self.slots.iter().copied().collect();
        let near = grid.nearest(at);
        if let (Some(slot), Some(name)) = (
            grid.nearest_free(near, &taken),
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
        ) {
            self.prefs.positions.insert(name, slot);
            self.save();
        }
    }

    pub(super) fn menu_items(&self, menu: &OpenMenu) -> Vec<KitItem<Message>> {
        let item =
            |label: &str, enabled: bool, checked: bool, command: DeskCommand| KitItem::Item {
                label: label.into(),
                enabled,
                checked,
                on_select: Message::Command(command),
            };
        if menu.on_items {
            let one = self.selected().len() == 1;
            let apps = menu.choices.clone().unwrap_or_default();
            return vec![
                item("Open", true, false, DeskCommand::Open),
                KitItem::Submenu {
                    label: "Open With".into(),
                    enabled: !apps.is_empty(),
                    items: apps
                        .into_iter()
                        .map(|(id, name)| item(&name, true, false, DeskCommand::OpenWith(id)))
                        .collect(),
                },
                KitItem::Separator,
                item("Move to Trash", true, false, DeskCommand::MoveToTrash),
                KitItem::Separator,
                item("Rename", one, false, DeskCommand::Rename),
                item("Duplicate", true, false, DeskCommand::Duplicate),
                item("Make Alias", one, false, DeskCommand::MakeAlias),
                KitItem::Separator,
                item("Copy", true, false, DeskCommand::Copy),
            ];
        }
        let sorted = |key: Option<SortKey>| {
            self.prefs.arrangement == key.map_or(Arrangement::Free, Arrangement::Sorted)
        };
        let sort = |label: &str, key: Option<SortKey>| {
            item(label, true, sorted(key), DeskCommand::SortBy(key))
        };
        vec![
            item("New Folder", true, false, DeskCommand::NewFolder),
            KitItem::Separator,
            item(
                "Paste",
                !self.pasteboard.is_empty(),
                false,
                DeskCommand::Paste,
            ),
            KitItem::Separator,
            item(
                "Change Wallpaper\u{2026}",
                true,
                false,
                DeskCommand::ChangeWallpaper,
            ),
            KitItem::Separator,
            KitItem::Submenu {
                label: "Sort By".into(),
                enabled: true,
                items: vec![
                    sort("None", None),
                    KitItem::Separator,
                    sort("Name", Some(SortKey::Name)),
                    sort("Kind", Some(SortKey::Kind)),
                    sort("Date Modified", Some(SortKey::DateModified)),
                    sort("Size", Some(SortKey::Size)),
                ],
            },
            item(
                "Clean Up",
                self.prefs.arrangement == Arrangement::Free && !self.entries.is_empty(),
                false,
                DeskCommand::CleanUp,
            ),
        ]
    }

    pub(super) fn shortcuts(&self) -> CommandRegistry<Message> {
        let mut registry = CommandRegistry::new();
        let key = |mods: Mods, keysym: Keysym| Shortcut { mods, keysym };
        let command = |c: DeskCommand| Message::Command(c);
        if self.rename.is_some() {
            registry.insert(key(Mods::NONE, ESCAPE), Message::NameCancelled);
            return registry;
        }
        let some = !self.selection.is_empty();
        if self.selection.len() == 1 {
            registry.insert(key(Mods::NONE, RETURN), command(DeskCommand::Rename));
            registry.insert(key(Mods::NONE, KP_ENTER), command(DeskCommand::Rename));
        }
        if some {
            for open in [Keysym(u32::from(b'o')), DOWN] {
                registry.insert(key(Mods::PRIMARY, open), command(DeskCommand::Open));
            }
            registry.insert(
                key(Mods::PRIMARY, BACKSPACE),
                command(DeskCommand::MoveToTrash),
            );
            registry.insert(
                key(Mods::PRIMARY, Keysym(u32::from(b'd'))),
                command(DeskCommand::Duplicate),
            );
            registry.insert(
                key(Mods::PRIMARY, Keysym(u32::from(b'c'))),
                command(DeskCommand::Copy),
            );
        }
        registry.insert(
            key(Mods::PRIMARY, Keysym(u32::from(b'v'))),
            command(DeskCommand::Paste),
        );
        registry.insert(
            key(Mods::PRIMARY, Keysym(u32::from(b'a'))),
            command(DeskCommand::SelectAll),
        );
        for n in *b"nN" {
            registry.insert(
                key(Mods::PRIMARY | Mods::SHIFT, Keysym(u32::from(n))),
                command(DeskCommand::NewFolder),
            );
        }
        for (keysym, direction) in [
            (LEFT, Direction::Left),
            (RIGHT, Direction::Right),
            (UP, Direction::Up),
            (DOWN, Direction::Down),
        ] {
            registry.insert(
                key(Mods::NONE, keysym),
                command(DeskCommand::Step(direction)),
            );
        }
        registry.insert(
            key(Mods::NONE, ESCAPE),
            command(DeskCommand::ClearSelection),
        );
        registry
    }

    pub(super) fn name_field(&self) -> Option<(usize, Element<Message>)> {
        let rename = self.rename.as_ref()?;
        let index = self.entries.iter().position(|e| e.path == rename.path)?;
        let field = rename.edit.field();
        let field = Element::new(TextField {
            label: "Name".into(),
            placeholder: String::new(),
            value: field.text().to_string(),
            revision: rename.revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(Message::NameChanged)),
            on_submit: Some(Message::NameSubmitted),
            selection: Some(field.selection()),
            initial: false,
        })
        .key(name_field_key())
        .width(m::LABEL_WIDTH + 2.0 * m::LABEL_PAD.0);
        Some((index, field))
    }
}
