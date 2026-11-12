//! Items dropped on a window from any application, this window included. A `text/uri-list` of local files lands on a
//! folder of the list or the grid, on a place of the sidebar, or anywhere else in the folder area on the folder shown;
//! the Trash place, and the trash's own window, move them to the trash. The place is highlighted while the drag is
//! over it. With neither Option nor the primary modifier held the items move within a device and are copied across
//! devices; Option copies, the primary modifier moves. For a drag from this application the device is known while the
//! drag moves; for one from another application the files are known only at the drop, so the drag shows a move and
//! the drop copies what lies on another device. A folder is not dropped on itself, and a drag of this application's
//! items over the folder holding them all lands nowhere. A folder of the path bar takes drops as the folder it names.
//! The targets tell the window which folder the drag is over, so a folder held under it springs open ([`super::spring`]),
//! and the folder area's target tells it when the drag leaves.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cupertino_kit::core::{DropOver, DropPlace, DropTarget, Dropped, Key};
use cupertino_kit::platform::{DropAction as Offered, Mods};
use cupertino_kit::prelude::*;
use percent_encoding::percent_decode_str;

use super::{view, BrowserWindow, Event, Message, Session};
use crate::operations::{self, DropAction};
use crate::places::PlaceKind;

/// The type dropped files are read in.
pub(super) const URI_LIST: &str = "text/uri-list";

/// Where dropped items go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Folder(PathBuf),
    Trash,
}

impl Destination {
    fn folder(&self) -> Option<&Path> {
        match self {
            Destination::Folder(path) => Some(path),
            Destination::Trash => None,
        }
    }
}

/// The folder area `content` of window `id`, taking drops on its folders and on the folder shown.
pub(super) fn folder(
    session: &Session,
    id: u64,
    win: &BrowserWindow,
    content: Element<Message>,
) -> Element<Message> {
    // the column view's other columns: their folders' rows, and the columns themselves for their folders
    let columns = match win.view {
        crate::window::ViewMode::Columns => win.columns.row_folders(id),
        _ => Vec::new(),
    };
    let count = win.browser.entries().len();
    let folders: Rc<[Option<PathBuf>]> = win
        .browser
        .entries()
        .iter()
        .map(|e| e.is_dir.then(|| e.path.clone()))
        .chain(columns.iter().map(|(_, folder)| folder.clone()))
        .collect();
    let here = shown(session, win.browser.dir());
    let dragged: Rc<[PathBuf]> = session.dragged.clone().into();
    // the list keys its rows by path, so their focus follows them while folders open above; the grid by index
    let rows: Rc<HashMap<Key, u64>> = Rc::new(
        win.browser
            .entries()
            .iter()
            .enumerate()
            .map(|(i, e)| (view::row_key(id, &e.path), i as u64))
            .chain(
                columns
                    .into_iter()
                    .enumerate()
                    .map(|(j, (key, _))| (key, (count + j) as u64)),
            )
            .collect(),
    );
    let places = Rc::clone(&folders);
    let target = Rc::clone(&folders);
    let hovered = Rc::clone(&folders);
    let row_places = Rc::clone(&rows);
    let row_hovers = Rc::clone(&rows);
    let into = here.clone();
    let read_only = read_only(session);
    Element::from(DropTarget {
        content,
        mimes: vec![URI_LIST.into()],
        actions: [Offered::Copy, Offered::Move].into(),
        place: Box::new(move |over| {
            let item = item_under(over.keys, &row_places);
            let folder = item
                .and_then(|(key, i)| Some((key, places.get(usize::try_from(i).ok()?)?.clone()?)))
                .filter(|_| here != Some(Destination::Trash));
            match folder {
                Some((key, path)) => offer(
                    over,
                    Some(key),
                    &Destination::Folder(path),
                    &dragged,
                    &*read_only,
                ),
                None => offer(over, None, here.as_ref()?, &dragged, &*read_only),
            }
        }),
        // the folder under the drag, which springs open when it is held there
        on_hover: Some(Box::new(move |place| {
            let folder = place
                .and_then(|place| place.key)
                .and_then(|key| item_under(std::slice::from_ref(&key), &row_hovers))
                .and_then(|(_, i)| hovered.get(usize::try_from(i).ok()?)?.clone());
            Message::Window(id, Event::SpringOver(folder))
        })),
        on_leave: Some(Message::Window(id, Event::DragLeft)),
        on_drop: Box::new(move |dropped| {
            let item = dropped
                .place
                .as_ref()
                .and_then(|key| item_under(std::slice::from_ref(key), &rows));
            let to = match item {
                Some((_, i)) => usize::try_from(i)
                    .ok()
                    .and_then(|i| target.get(i)?.clone())
                    .map(Destination::Folder),
                None => into.clone(),
            };
            dropped_on(id, to, dropped)
        }),
    })
    // a folder the drag opened lays out a new target, which the drag resting there is asked of anew
    .key(format!("drops/{}", win.springing.opened()))
}

/// The item under a drag among `keys`, with the key that names it: a list row by its key in `rows`, else a grid cell
/// by the index its key holds. A row is looked for first, as the cells inside it are keyed by their column.
fn item_under(keys: &[Key], rows: &HashMap<Key, u64>) -> Option<(Key, u64)> {
    let row = keys
        .iter()
        .find_map(|key| Some((key.clone(), *rows.get(key)?)));
    row.or_else(|| {
        keys.iter()
            .find_map(|key| Some((key.clone(), super::grid::cell_index(key)?)))
    })
}

/// The sidebar `content` of window `id`, taking drops on the places that are folders and on the Trash.
pub(super) fn sidebar(session: &Session, id: u64, content: Element<Message>) -> Element<Message> {
    let places: Rc<[(Key, Destination)]> = (0..session.places.len())
        .filter_map(|i| {
            let place = &session.places[i];
            let to = match place.kind {
                PlaceKind::Trash => Destination::Trash,
                PlaceKind::Recents | PlaceKind::Network => return None,
                _ => Destination::Folder(place.folder()?.to_path_buf()),
            };
            Some((super::view::place_key(session, i), to))
        })
        .collect();
    let dragged: Rc<[PathBuf]> = session.dragged.clone().into();
    let target = Rc::clone(&places);
    let hovered = Rc::clone(&places);
    let read_only = read_only(session);
    Element::from(DropTarget {
        content,
        mimes: vec![URI_LIST.into()],
        actions: [Offered::Copy, Offered::Move].into(),
        place: Box::new(move |over| {
            let (key, to) = over
                .keys
                .iter()
                .find_map(|key| places.iter().find(|(place, _)| place == key))?;
            offer(over, Some(key.clone()), to, &dragged, &*read_only)
        }),
        on_hover: Some(Box::new(move |place| {
            let folder = place.and_then(|place| {
                let key = place.key?;
                let (_, to) = hovered.iter().find(|(k, _)| *k == key)?;
                Some(to.folder()?.to_path_buf())
            });
            Message::Window(id, Event::SpringOver(folder))
        })),
        on_leave: None,
        on_drop: Box::new(move |dropped| {
            let to = dropped.place.as_ref().and_then(|key| {
                let (_, to) = target.iter().find(|(place, _)| place == key)?;
                Some(to.clone())
            });
            dropped_on(id, to, dropped)
        }),
    })
}

/// The path bar `content` of window `id`, whose folders, `chain` from the top down and each keyed by its place in it,
/// take drops as the folder area does when it shows them: the trash's own folder trashes, Recents takes nothing.
pub(super) fn path_bar(
    session: &Session,
    id: u64,
    chain: &[PathBuf],
    content: Element<Message>,
) -> Element<Message> {
    let folders: Rc<[Option<Destination>]> = chain.iter().map(|dir| shown(session, dir)).collect();
    let dragged: Rc<[PathBuf]> = session.dragged.clone().into();
    let target = Rc::clone(&folders);
    let hovered = Rc::clone(&folders);
    let read_only = read_only(session);
    Element::from(DropTarget {
        content,
        mimes: vec![URI_LIST.into()],
        actions: [Offered::Copy, Offered::Move].into(),
        place: Box::new(move |over| {
            let (key, to) = over
                .keys
                .iter()
                .find_map(|key| Some((key.clone(), segment(&folders, key)?)))?;
            offer(over, Some(key), &to, &dragged, &*read_only)
        }),
        on_hover: Some(Box::new(move |place| {
            let folder = place
                .and_then(|place| place.key)
                .and_then(|key| segment(&hovered, &key)?.folder().map(Path::to_path_buf));
            Message::Window(id, Event::SpringOver(folder))
        })),
        on_leave: None,
        on_drop: Box::new(move |dropped| {
            let to = dropped.place.as_ref().and_then(|key| segment(&target, key));
            dropped_on(id, to, dropped)
        }),
    })
}

/// Where the path bar's folder keyed `key` takes drops.
fn segment(folders: &[Option<Destination>], key: &Key) -> Option<Destination> {
    let Key::Int(i) = key else { return None };
    folders.get(usize::try_from(*i).ok()?)?.clone()
}

/// Where a drop on the folder shown at `dir` goes: the trash's own folder trashes, Recents takes nothing.
fn shown(session: &Session, dir: &Path) -> Option<Destination> {
    if dir == session.trash.home_trash().join("files") {
        return Some(Destination::Trash);
    }
    (dir != crate::recents::location()).then(|| Destination::Folder(dir.to_path_buf()))
}

/// The place a drag over `to` lands on, and the action it shows: none on one of this application's dragged folders,
/// or on the folder holding all of them; the trash only when the source lets the items go.
fn offer(
    over: &DropOver<'_>,
    key: Option<Key>,
    to: &Destination,
    dragged: &[PathBuf],
    read_only: &ReadOnly,
) -> Option<DropPlace> {
    let action = match to {
        Destination::Trash => over
            .actions
            .contains(&Offered::Move)
            .then_some(Offered::Move)?,
        Destination::Folder(dir) => {
            if read_only(dir)
                || dragged.iter().any(|p| p == dir)
                || (!dragged.is_empty() && dragged.iter().all(|p| p.parent() == Some(dir)))
            {
                return None;
            }
            // another application's files are known only at the drop: show a move, and decide there
            let same = dragged.is_empty() || all_on(dragged, dir).unwrap_or(false);
            offered(action(same, over.mods))
        }
    };
    Some(DropPlace { key, action })
}

/// Whether a folder takes nothing new, so no drop lands on it.
type ReadOnly = dyn Fn(&Path) -> bool;

/// [`ReadOnly`] for the session's media: a folder on a medium that can't be written, or on a file system mounted
/// read-only.
fn read_only(session: &Session) -> Rc<ReadOnly> {
    let media: Vec<(PathBuf, bool)> = (0..)
        .map_while(|i| session.media.device(i))
        .filter_map(|(device, _)| Some((device.mount.clone()?, device.read_only)))
        .collect();
    Rc::new(move |dir: &Path| {
        // the medium the folder is on is the one mounted deepest above it
        let medium = media
            .iter()
            .filter(|(mount, _)| dir.starts_with(mount))
            .max_by_key(|(mount, _)| mount.components().count())
            .is_some_and(|&(_, read_only)| read_only);
        medium || crate::browser::mounted_read_only(dir).unwrap_or(false)
    })
}

/// What a drop does, by the modifiers held: Option copies, the primary modifier moves, with neither by device.
fn action(same_device: bool, mods: Mods) -> DropAction {
    operations::drop_action(
        same_device,
        mods.contains(Mods::ALT),
        mods.contains(Mods::PRIMARY),
    )
}

fn offered(action: DropAction) -> Offered {
    match action {
        DropAction::Copy => Offered::Copy,
        DropAction::Move => Offered::Move,
    }
}

/// Whether every one of `paths` is on the device of `dir`.
fn all_on(paths: &[PathBuf], dir: &Path) -> std::io::Result<bool> {
    for path in paths {
        if !operations::same_volume(path, dir)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The message of `dropped` on window `id`, to go `to`; a list Files cannot read in full goes nowhere.
fn dropped_on(id: u64, to: Option<Destination>, dropped: Dropped) -> Message {
    let paths = uri_paths(&dropped.data.bytes);
    Message::Window(
        id,
        Event::Dropped {
            token: dropped.token,
            to: to.filter(|_| paths.is_some()),
            paths: paths.unwrap_or_default(),
            action: dropped.action,
            mods: dropped.mods,
        },
    )
}

/// What a drop of `paths` on `to` does, given the action agreed with the source and the modifiers held: a copy when
/// the source wanted one, else the modifiers' choice, else a move within a device and a copy across devices.
pub(super) fn chosen(
    paths: &[PathBuf],
    to: &Destination,
    agreed: Offered,
    mods: Mods,
) -> std::io::Result<DropAction> {
    let Some(dir) = to.folder() else {
        return Ok(DropAction::Move);
    };
    if agreed != Offered::Move {
        return Ok(DropAction::Copy);
    }
    let held = mods.contains(Mods::ALT) || mods.contains(Mods::PRIMARY);
    let same = held || all_on(paths, dir)?;
    Ok(action(same, mods))
}

/// The local files of a `text/uri-list`: one URI a line, `#` lines being comments. The list comes from any
/// application, so it is taken whole or not at all: `None` when it is not text, or a URI is of another scheme or host,
/// or its path is not absolute, steps through `.` or `..`, or holds a NUL. A source told its items moved removes them
/// all, so none may be left out.
pub(super) fn uri_paths(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|uri| {
            let rest = uri.strip_prefix("file://")?;
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            // the query and fragment are not part of the path
            let path = path.split(['?', '#']).next()?;
            if !path.starts_with('/') {
                return None;
            }
            let bytes: Vec<u8> = percent_decode_str(path).collect();
            let plain = bytes
                .split(|&b| b == b'/')
                .all(|step| step != b"." && step != b"..");
            (plain && !bytes.contains(&0)).then(|| PathBuf::from(OsString::from_vec(bytes)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_takes_nothing_new_takes_no_drop_and_the_trash_still_does() {
        let d = tempfile::tempdir().unwrap();
        let (disc, home) = (d.path().join("DISC"), d.path().join("home"));
        let actions = [Offered::Copy, Offered::Move].into();
        let over = DropOver {
            position: cupertino_kit::geometry::Point::ZERO,
            keys: &[],
            mimes: &[],
            actions: &actions,
            mods: Mods::NONE,
        };
        let read_only = disc.clone();
        let on_disc = move |dir: &Path| dir.starts_with(&read_only);
        let dragged = [home.join("x.txt")];
        assert_eq!(
            offer(
                &over,
                None,
                &Destination::Folder(disc.join("inner")),
                &dragged,
                &on_disc
            ),
            None,
            "nothing lands on read-only media"
        );
        assert!(offer(
            &over,
            None,
            &Destination::Folder(home.join("b")),
            &dragged,
            &on_disc
        )
        .is_some());
        assert!(offer(&over, None, &Destination::Trash, &dragged, &on_disc).is_some());
    }

    #[test]
    fn a_uri_list_gives_its_local_files_decoded() {
        let list = b"# dragged\r\nfile:///home/a/My%20Notes.txt\r\nfile://localhost/tmp/b\r\n\r\nfile:///e%23f?x=1\n";
        assert_eq!(
            uri_paths(list),
            Some(vec![
                PathBuf::from("/home/a/My Notes.txt"),
                PathBuf::from("/tmp/b"),
                PathBuf::from("/e#f"),
            ])
        );
        assert_eq!(uri_paths(b"# nothing\r\n"), Some(Vec::new()));
    }

    #[test]
    fn a_uri_list_with_anything_but_plain_local_absolute_paths_is_refused_whole() {
        for odd in [
            "https://example.org/c",
            "file://elsewhere/d",
            "file://localhostx/d",
            "file:relative/d",
            "file:/d",
            "/d",
            "file://",
            "file:///a/../etc/passwd",
            "file:///a/%2E%2E/etc",
            "file:///a/./b",
            "file:///a/.",
            "file:///a/b%00c",
        ] {
            let list = format!("file:///ok\r\n{odd}\r\n");
            assert_eq!(uri_paths(list.as_bytes()), None, "{odd}");
        }
        assert_eq!(uri_paths(b"file:///a\xff\r\n"), None, "not text");
        assert_eq!(
            uri_paths(b"file:///a/%FF"),
            Some(vec![PathBuf::from(OsString::from_vec(b"/a/\xff".to_vec()))]),
            "a name that is not UTF-8 is percent-encoded"
        );
    }

    #[test]
    fn a_drop_copies_when_the_source_wanted_a_copy_and_follows_the_modifiers_and_the_device_otherwise(
    ) {
        let dir = std::env::temp_dir();
        let here = Destination::Folder(dir.clone());
        let file = vec![dir.join("absent")];
        assert_eq!(
            chosen(&file, &here, Offered::Copy, Mods::NONE).unwrap(),
            DropAction::Copy
        );
        assert_eq!(
            chosen(&file, &here, Offered::Move, Mods::ALT).unwrap(),
            DropAction::Copy
        );
        assert_eq!(
            chosen(&file, &here, Offered::Move, Mods::PRIMARY).unwrap(),
            DropAction::Move
        );
        // a file that is gone has no device to compare
        assert!(chosen(&file, &here, Offered::Move, Mods::NONE).is_err());
        assert_eq!(
            chosen(&file, &Destination::Trash, Offered::Copy, Mods::NONE).unwrap(),
            DropAction::Move
        );
    }
}
