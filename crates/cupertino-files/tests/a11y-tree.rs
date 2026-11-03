//! Every node of a Files window's accessibility tree has a name a screen reader can announce. The tree holds the window
//! and its sidebar; the folder's items are not in it yet (Shared/cupertino/probe/a11y-gaps.md).

use std::fs;
use std::path::Path;

use cupertino_files::access::{window_tree, PlaceKeys, WindowView};
use cupertino_files::geom::Size;
use cupertino_files::places::{standard_places, Place};
use cupertino_files::window::{Bars, WindowLayout};
use cupertino_ui_test::{semantic_gaps, semantic_outline};

#[test]
fn every_node_of_a_files_window_is_named() {
    let home = tempfile::tempdir().unwrap();
    for dir in [
        "Desktop",
        "Documents",
        "Downloads",
        ".local/share/Trash/files",
    ] {
        fs::create_dir_all(home.path().join(dir)).unwrap();
    }
    let mut places = standard_places(
        home.path(),
        &[],
        &home.path().join(".local/share/Trash/files"),
        Some(Path::new("/usr/share/applications")),
        "Workstation",
    );
    places.extend([
        Place::removable("Backup Disk", Some(Path::new("/run/media/u/Backup")), false),
        Place::removable("32 GB Volume", None, false),
        Place::removable("Camera Card", Some(Path::new("/run/media/u/CARD")), true),
    ]);
    let mut keys = PlaceKeys::default();
    let keys: Vec<u32> = places.iter().map(|p| keys.key(&p.name)).collect();
    let downloads = home.path().join("Downloads");
    for bars in [
        Bars::HIDDEN,
        Bars {
            path: true,
            status: true,
        },
    ] {
        let layout = WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places, bars);
        let tree = window_tree(&WindowView {
            title: "Downloads",
            layout: &layout,
            places: &places,
            keys: &keys,
            dir: &downloads,
            sidebar_focus: Some(places.len() - 3),
        });
        assert!(
            tree.nodes.len() > places.len(),
            "{bars:?}: {} nodes",
            tree.nodes.len()
        );
        assert_eq!(semantic_gaps(&tree), Vec::<String>::new(), "{bars:?}");
    }
}

#[test]
fn the_sidebar_reads_its_places_by_role_name_and_state() {
    let media = tempfile::tempdir().unwrap();
    let backup = media.path().join("Backup");
    fs::create_dir_all(&backup).unwrap();
    let places = vec![
        Place::removable("Backup Disk", Some(&backup), false),
        Place::removable("32 GB Volume", None, false),
        Place::removable("Camera Card", Some(Path::new("/run/media/u/CARD")), true),
    ];
    let mut keys = PlaceKeys::default();
    let keys: Vec<u32> = places.iter().map(|p| keys.key(&p.name)).collect();
    let layout = WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places, Bars::HIDDEN);
    let tree = window_tree(&WindowView {
        title: "Backup",
        layout: &layout,
        places: &places,
        keys: &keys,
        dir: &backup,
        sidebar_focus: Some(2),
    });
    assert_eq!(
        semantic_outline(&tree),
        [
            "Window \"Backup\"",
            "  List \"Sidebar\" 3 items",
            "    Group \"Locations\"",
            "      ListItem \"Backup Disk\" focusable selected",
            "        Button \"Eject Backup Disk\"",
            "      ListItem \"32 GB Volume\" focusable",
            "      ListItem \"Camera Card\" focusable focused busy",
        ],
        "the folder shown is selected, the focused row focused, a busy file system busy, and only a mounted one ejects"
    );
}
