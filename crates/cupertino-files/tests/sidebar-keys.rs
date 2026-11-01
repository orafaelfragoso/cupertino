//! The sidebar's keys on a sidebar of Home, Downloads, a mounted, an unmounted and a busy removable file system.

use std::path::{Path, PathBuf};

use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_files::sidebar_keys::{entry_row, sidebar_key, SidebarAction, SidebarKey};

const HOME: usize = 0;
const DOWNLOADS: usize = 1;
const BACKUP: usize = 2;
const KINGSTON: usize = 3;
const CAMERA: usize = 4;

fn places() -> Vec<Place> {
    let folder = |name: &str, path: &str, kind| Place {
        name: name.into(),
        path: PathBuf::from(path),
        group: PlaceGroup::Favorites,
        kind,
    };
    vec![
        folder("rafael", "/home/rafael", PlaceKind::Home),
        folder("Downloads", "/home/rafael/Downloads", PlaceKind::Downloads),
        Place::removable(
            "Backup Disk",
            Some(Path::new("/run/media/rafael/Backup")),
            false,
        ),
        Place::removable("KINGSTON", None, false),
        Place::removable(
            "Camera Card",
            Some(Path::new("/run/media/rafael/CARD")),
            true,
        ),
    ]
}

#[test]
fn the_sidebar_takes_the_keyboard_on_the_row_of_the_folder_shown_else_the_first() {
    let places = places();
    assert_eq!(
        entry_row(&places, Path::new("/home/rafael/Downloads")),
        DOWNLOADS
    );
    assert_eq!(
        entry_row(&places, Path::new("/run/media/rafael/Backup")),
        BACKUP
    );
    assert_eq!(entry_row(&places, Path::new("/tmp")), HOME);
}

#[test]
fn up_down_home_and_end_move_the_focus_and_stop_at_the_ends() {
    let places = places();
    let key = |focus, key| sidebar_key(&places, focus, key);
    assert_eq!(
        key(HOME, SidebarKey::Down),
        Some(SidebarAction::Focus(DOWNLOADS))
    );
    assert_eq!(
        key(DOWNLOADS, SidebarKey::Up),
        Some(SidebarAction::Focus(HOME))
    );
    assert_eq!(key(HOME, SidebarKey::Up), None);
    assert_eq!(key(CAMERA, SidebarKey::Down), None);
    assert_eq!(
        key(BACKUP, SidebarKey::Home),
        Some(SidebarAction::Focus(HOME))
    );
    assert_eq!(
        key(BACKUP, SidebarKey::End),
        Some(SidebarAction::Focus(CAMERA))
    );
    assert_eq!(key(HOME, SidebarKey::Home), None);
}

#[test]
fn return_opens_the_row_as_a_click_does() {
    let places = places();
    let open = |focus| sidebar_key(&places, focus, SidebarKey::Open);
    assert_eq!(open(DOWNLOADS), Some(SidebarAction::Go(DOWNLOADS)));
    assert_eq!(open(BACKUP), Some(SidebarAction::Go(BACKUP)));
    assert_eq!(open(KINGSTON), Some(SidebarAction::Mount(KINGSTON)));
    assert_eq!(open(CAMERA), None);
}

#[test]
fn primary_e_ejects_only_a_mounted_removable_row() {
    let places = places();
    let eject = |focus| sidebar_key(&places, focus, SidebarKey::Eject);
    assert_eq!(eject(BACKUP), Some(SidebarAction::Eject(BACKUP)));
    assert_eq!(eject(KINGSTON), None);
    assert_eq!(eject(CAMERA), None);
    assert_eq!(eject(DOWNLOADS), None);
}

#[test]
fn a_focus_left_past_the_rows_by_a_removed_device_acts_on_the_last_row() {
    let mut places = places();
    places.truncate(BACKUP + 1);
    assert_eq!(
        sidebar_key(&places, CAMERA, SidebarKey::Eject),
        Some(SidebarAction::Eject(BACKUP))
    );
    assert_eq!(sidebar_key(&[], 0, SidebarKey::Down), None);
}
