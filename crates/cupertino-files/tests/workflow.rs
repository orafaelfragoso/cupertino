//! End-to-end: a folder is opened, listed, grouped and laid out; a selection survives changes;
//! settings persist; an item goes to the trash and comes back.

use cupertino_files::group::{group_entries, GroupBy};
use cupertino_files::layout::icon::{self, FontMetrics, IconParams, SectionSpec};
use cupertino_files::selection::{Modifiers, Selection};
use cupertino_files::settings::{FolderKey, SettingsStore, ViewSettings, ViewStyle};
use cupertino_files::trash::Trash;
use cupertino_files::{DirectoryModel, ModelDelta, ModelOptions};
use std::time::{Duration, Instant, SystemTime};

fn pump(m: &mut DirectoryModel, mut done: impl FnMut(&DirectoryModel) -> bool) -> Vec<ModelDelta> {
    let t = Instant::now();
    let mut out = Vec::new();
    while !done(m) && t.elapsed() < Duration::from_secs(5) {
        out.extend(m.poll_timeout(Duration::from_millis(50)));
    }
    out
}

#[test]
fn open_group_layout_select_and_survive_changes() {
    let d = tempfile::tempdir().unwrap();
    for n in ["a.png", "b.png", "c.pdf", "notes.txt"] {
        std::fs::write(d.path().join(n), b"x").unwrap();
    }
    std::fs::create_dir(d.path().join("sub")).unwrap();
    let mut m = DirectoryModel::open(d.path(), ModelOptions::default()).unwrap();
    m.wait_loaded();
    assert_eq!(m.len(), 5);

    // group by kind and lay the sections out
    let groups = group_entries(m.entries(), GroupBy::Kind, SystemTime::now());
    let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["Folders", "Images", "PDF Documents", "Text"]);
    let sections: Vec<SectionSpec> = groups.iter().map(|g| SectionSpec { title: Some(g.title.clone()), count: g.items.len() }).collect();
    let p = IconParams::new(64, 54, 12, true, false, FontMetrics::estimate(12.0));
    let l = icon::layout(&p, 600.0, &sections, icon::Flow::Leading);
    assert_eq!(l.cells.len(), 5);
    assert_eq!(l.headers.len(), 4);

    // select "b.png" and keep it selected while an item sorting before it appears and one is removed
    let mut sel = Selection::new();
    let b = m.index_of_name("b.png").unwrap();
    sel.mouse_down(b, Modifiers::NONE);
    std::fs::write(d.path().join("0first"), b"x").unwrap();
    let deltas = pump(&mut m, |m| m.len() == 6);
    for dl in deltas {
        match dl {
            ModelDelta::Inserted { index } => sel.on_insert(index),
            ModelDelta::Removed { index } => sel.on_remove(index),
            ModelDelta::Moved { from, to } => sel.on_move(from, to),
            _ => {}
        }
    }
    let now_b = m.index_of_name("b.png").unwrap();
    assert!(sel.contains(now_b), "selection follows the item: {:?} vs {}", sel.indices().collect::<Vec<_>>(), now_b);
    assert_eq!(sel.len(), 1);
}

#[test]
fn settings_and_trash_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("x.jpg"), b"x").unwrap();

    let key = FolderKey::of(&folder).unwrap();
    let mut store = SettingsStore::open(d.path().join("s/folders.json")).unwrap();
    let mut s = ViewSettings { style: ViewStyle::Gallery, ..Default::default() };
    s.icon.grid_spacing = 80;
    store.set(&key, s);
    store.save().unwrap();
    let reopened = SettingsStore::open(d.path().join("s/folders.json")).unwrap();
    assert_eq!(reopened.get(&key).style, ViewStyle::Gallery);
    assert_eq!(reopened.get(&key).icon.grid_spacing, 80);

    let trash = Trash::with_home(d.path().join("Trash"));
    let item = trash.trash(&folder.join("x.jpg")).unwrap();
    assert!(!folder.join("x.jpg").exists());
    trash.restore(&item).unwrap();
    assert!(folder.join("x.jpg").exists());
}
