//! The shipped `data/applications` (the desktop entry and `cupertino-mimeapps.list`) make the file manager
//! the session's handler of folders and the trash, ahead of a host default, only in the Cupertino session.

use std::path::{Path, PathBuf};

use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};

const NAUTILUS: &str = "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus --new-window %U\nMimeType=inode/directory;\n";

/// A host with a GNOME file manager as the system default of folders, the crate's data directory
/// installed ahead of it, and the user's own list `user_list` (none when `None`).
fn resolver(root: &Path, desktops: &[&str], user_list: Option<&str>) -> MimeApps {
    let host = root.join("usr");
    std::fs::create_dir_all(host.join("applications")).unwrap();
    std::fs::write(host.join("applications/org.gnome.Nautilus.desktop"), NAUTILUS).unwrap();
    std::fs::write(host.join("applications/mimeapps.list"), "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n").unwrap();
    let config_home = root.join("config");
    std::fs::create_dir_all(&config_home).unwrap();
    if let Some(list) = user_list {
        std::fs::write(config_home.join("mimeapps.list"), list).unwrap();
    }
    let session = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let dirs = XdgDirs {
        config_home,
        config_dirs: vec![root.join("etc")],
        data_home: root.join("local"),
        data_dirs: vec![session, host],
        current_desktops: desktops.iter().map(|d| d.to_string()).collect(),
    };
    let db = MimeDatabase::load(&dirs.all_data_dirs()).unwrap();
    let vis = Visibility { current_desktops: dirs.current_desktops.clone(), path_dirs: vec![], skip_try_exec: true };
    MimeApps::load(&dirs, db, &Locale::default(), &vis).unwrap()
}

fn default_id(apps: &MimeApps, mime: &str) -> Option<String> {
    apps.default_for(mime).map(|e| e.id.clone())
}

#[test]
fn the_session_opens_folders_and_the_trash_in_its_file_manager() {
    let root = tempfile::tempdir().unwrap();
    let apps = resolver(root.path(), &["Cupertino"], None);
    assert_eq!(default_id(&apps, "inode/directory").as_deref(), Some("cupertino-files.desktop"));
    assert_eq!(default_id(&apps, "x-scheme-handler/trash").as_deref(), Some("cupertino-files.desktop"));
}

#[test]
fn other_desktops_keep_the_host_default() {
    let root = tempfile::tempdir().unwrap();
    let apps = resolver(root.path(), &["GNOME"], None);
    assert_eq!(default_id(&apps, "inode/directory").as_deref(), Some("org.gnome.Nautilus.desktop"));
}

#[test]
fn a_default_the_user_chose_still_wins_in_the_session() {
    let root = tempfile::tempdir().unwrap();
    let apps = resolver(root.path(), &["Cupertino"], Some("[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n"));
    assert_eq!(default_id(&apps, "inode/directory").as_deref(), Some("org.gnome.Nautilus.desktop"));
    assert_eq!(default_id(&apps, "x-scheme-handler/trash").as_deref(), Some("cupertino-files.desktop"), "only the type the user set");
}
