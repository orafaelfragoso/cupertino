//! `IconLookup` and `ThemeIcons`: where an application's icon file is found by name, size and scale.

#[allow(dead_code)]
mod support;

use cupertino_assets::Assets;
use cupertino_icons::{IconLookup, ThemeIcons};

const NO_PACK: &str = r#"{"version": 2}"#;

fn bases(dir: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    vec![dir.path().join("icons")]
}

#[test]
fn an_empty_name_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), bases(&dir));
    assert_eq!(lookup.find("", 32, 1), None);
}

#[test]
fn an_absolute_path_is_found_as_it_is_when_it_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mine.png");
    support::write_png(&file, 8, 8, [1, 1, 1, 255]);
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), bases(&dir));
    assert_eq!(lookup.find(file.to_str().unwrap(), 64, 2), Some(file));
    assert_eq!(
        lookup.find(dir.path().join("none.png").to_str().unwrap(), 32, 1),
        None
    );
    assert_eq!(
        lookup.find(dir.path().to_str().unwrap(), 32, 1),
        None,
        "a directory is no icon"
    );
}

#[test]
fn hicolor_is_searched_for_the_nearest_size() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    let root = support::theme(
        &icons,
        "hicolor",
        None,
        &[
            ("16x16/apps", 16, "Fixed"),
            ("48x48/apps", 48, "Fixed"),
            ("256x256/apps", 256, "Fixed"),
        ],
    );
    for px in [16, 48, 256] {
        support::write_png(
            &root.join(format!("{px}x{px}/apps/mail.png")),
            px,
            px,
            [0, 0, 0, 255],
        );
    }
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons]);
    let pick = |lookup: &mut IconLookup, size, scale| {
        lookup
            .find("mail", size, scale)
            .unwrap()
            .strip_prefix(&root)
            .unwrap()
            .to_path_buf()
    };
    assert_eq!(
        pick(&mut lookup, 16, 1),
        std::path::Path::new("16x16/apps/mail.png")
    );
    assert_eq!(
        pick(&mut lookup, 40, 1),
        std::path::Path::new("48x48/apps/mail.png")
    );
    assert_eq!(
        pick(&mut lookup, 200, 1),
        std::path::Path::new("256x256/apps/mail.png")
    );
    assert_eq!(
        pick(&mut lookup, 24, 2),
        std::path::Path::new("48x48/apps/mail.png"),
        "size times scale"
    );
    assert_eq!(
        pick(&mut lookup, 1000, 1),
        std::path::Path::new("256x256/apps/mail.png")
    );
}

#[test]
fn a_scalable_directory_serves_every_size_in_its_range() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    let root = icons.join("hicolor");
    support::write(
        &root.join("index.theme"),
        b"[Icon Theme]\nDirectories=32x32/apps,scalable/apps\n\n[32x32/apps]\nSize=32\nType=Fixed\n\n[scalable/apps]\nSize=64\nMinSize=8\nMaxSize=512\nType=Scalable\n",
    );
    support::write_png(&root.join("32x32/apps/x.png"), 32, 32, [0, 0, 0, 255]);
    support::write(
        &root.join("scalable/apps/x.svg"),
        support::BLACK_SVG.as_bytes(),
    );
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons]);
    assert!(
        lookup
            .find("x", 32, 1)
            .unwrap()
            .ends_with("32x32/apps/x.png"),
        "exact fixed size"
    );
    assert!(lookup
        .find("x", 100, 1)
        .unwrap()
        .ends_with("scalable/apps/x.svg"));
}

#[test]
fn a_png_wins_over_an_svg_of_the_same_directory() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    support::write_png(&root.join("48x48/apps/a.png"), 48, 48, [0, 0, 0, 255]);
    support::write(
        &root.join("48x48/apps/a.svg"),
        support::BLACK_SVG.as_bytes(),
    );
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons]);
    assert!(lookup.find("a", 48, 1).unwrap().ends_with("a.png"));
}

#[test]
fn a_name_no_theme_has_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons]);
    assert_eq!(lookup.find("missing", 48, 1), None);
}

#[test]
fn other_themes_than_hicolor_are_not_searched_without_an_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "Other", None, &[("48x48/apps", 48, "Fixed")]);
    support::write_png(&root.join("48x48/apps/a.png"), 48, 48, [0, 0, 0, 255]);
    support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons]);
    assert_eq!(lookup.find("a", 48, 1), None);
}

#[test]
fn flat_directories_are_searched_after_the_themes() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("icons");
    support::write_png(&icons.join("flat.png"), 8, 8, [0, 0, 0, 255]);
    support::write(&icons.join("vector.svg"), support::BLACK_SVG.as_bytes());
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons.clone()]);
    assert_eq!(lookup.find("flat", 32, 1), Some(icons.join("flat.png")));
    assert_eq!(lookup.find("vector", 32, 1), Some(icons.join("vector.svg")));
}

#[test]
fn a_theme_is_found_in_the_first_base_that_has_it_and_files_in_every_base() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (dir.path().join("one"), dir.path().join("two"));
    let dirs = [("48x48/apps", 48, "Fixed")];
    support::theme(&one, "hicolor", None, &dirs);
    let second = support::theme(&two, "hicolor", None, &dirs);
    support::write_png(
        &second.join("48x48/apps/only-second.png"),
        48,
        48,
        [0, 0, 0, 255],
    );
    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![one, two]);
    assert!(lookup
        .find("only-second", 48, 1)
        .unwrap()
        .starts_with(&second));
}

#[test]
fn an_application_name_comes_from_the_pack_before_the_themes() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("pack");
    support::write_png(&pack.join("icons/files-48.png"), 48, 48, [5, 5, 5, 255]);
    let assets = support::pack(
        &pack,
        r#"{"version": 2, "icons": {"application.files": {"raster": {"48": "icons/files-48.png"}}}}"#,
    );
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    support::write_png(
        &root.join("48x48/apps/application.files.png"),
        48,
        48,
        [0, 0, 0, 255],
    );
    let mut lookup = IconLookup::with_roots(&assets, vec![icons]);
    assert_eq!(
        lookup.find("application.files", 48, 1),
        Some(pack.join("icons/files-48.png"))
    );
}

#[test]
fn an_application_name_the_pack_lacks_falls_through_to_the_themes() {
    let dir = tempfile::tempdir().unwrap();
    let assets = support::pack(&dir.path().join("pack"), NO_PACK);
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    support::write_png(
        &root.join("48x48/apps/application.other.png"),
        48,
        48,
        [0, 0, 0, 255],
    );
    let mut lookup = IconLookup::with_roots(&assets, vec![icons]);
    assert!(lookup
        .find("application.other", 48, 1)
        .unwrap()
        .starts_with(&root));
}

#[test]
fn a_pack_that_opts_in_adds_its_theme_and_the_themes_it_inherits_before_hicolor() {
    let dir = tempfile::tempdir().unwrap();
    let assets = support::pack(
        &dir.path().join("pack"),
        r#"{"version": 2, "icon_theme": {"name": "Fancy", "applications": true}}"#,
    );
    assert_eq!(assets.application_icon_theme(), Some("Fancy"));
    let icons = dir.path().join("icons");
    let dirs = [("48x48/apps", 48, "Fixed")];
    let fancy = support::theme(&icons, "Fancy", Some("Parent"), &dirs);
    let parent = support::theme(&icons, "Parent", None, &dirs);
    let hicolor = support::theme(&icons, "hicolor", None, &dirs);
    support::write_png(&fancy.join("48x48/apps/a.png"), 48, 48, [0, 0, 0, 255]);
    support::write_png(&parent.join("48x48/apps/a.png"), 48, 48, [0, 0, 0, 255]);
    support::write_png(&parent.join("48x48/apps/b.png"), 48, 48, [0, 0, 0, 255]);
    support::write_png(&hicolor.join("48x48/apps/a.png"), 48, 48, [0, 0, 0, 255]);
    support::write_png(&hicolor.join("48x48/apps/c.png"), 48, 48, [0, 0, 0, 255]);
    let mut lookup = IconLookup::with_roots(&assets, vec![icons]);
    assert!(lookup.find("a", 48, 1).unwrap().starts_with(&fancy));
    assert!(lookup.find("b", 48, 1).unwrap().starts_with(&parent));
    assert!(lookup.find("c", 48, 1).unwrap().starts_with(&hicolor));
}

#[test]
fn a_pack_theme_without_the_opt_in_is_not_an_application_theme() {
    let dir = tempfile::tempdir().unwrap();
    let assets = support::pack(
        &dir.path().join("pack"),
        r#"{"version": 2, "icon_theme": {"name": "Fancy"}}"#,
    );
    assert_eq!(assets.application_icon_theme(), None);
}

#[test]
fn the_application_icon_of_a_desktop_entry_is_loaded_by_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("applications");
    support::write(
        &apps.join("org.example.Mail.desktop"),
        b"[Desktop Entry]\nType=Application\nName=Mail\nExec=mail\nIcon=mail-icon\n",
    );
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "hicolor", None, &[("48x48/apps", 48, "Fixed")]);
    support::write_png(
        &root.join("48x48/apps/mail-icon.png"),
        48,
        48,
        [255, 0, 0, 255],
    );
    let mut theme_icons = ThemeIcons::with_roots(&Assets::builtin(), vec![icons], vec![apps]);
    let pixmap = theme_icons.load_for_entry("org.example.Mail", 48).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (48, 48));
    assert_eq!(pixmap.pixels()[0].red(), 255);
    let with_suffix = theme_icons
        .load_for_entry("org.example.Mail.desktop", 48)
        .unwrap();
    assert_eq!(
        with_suffix.data(),
        pixmap.data(),
        "a trailing .desktop is accepted"
    );
}

#[test]
fn an_entry_that_is_not_installed_or_names_no_icon_has_no_icon() {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("applications");
    support::write(
        &apps.join("plain.desktop"),
        b"[Desktop Entry]\nType=Application\nName=Plain\nExec=plain\n",
    );
    let mut theme_icons = ThemeIcons::with_roots(
        &Assets::builtin(),
        vec![dir.path().join("icons")],
        vec![apps],
    );
    assert!(theme_icons.load_for_entry("plain", 32).is_none());
    assert!(theme_icons.load_for_entry("absent", 32).is_none());
}

#[test]
fn an_entry_whose_icon_is_a_file_path_loads_that_file() {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("applications");
    let png = dir.path().join("own.png");
    support::write_png(&png, 16, 16, [0, 255, 0, 255]);
    support::write(
        &apps.join("own.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Own\nExec=own\nIcon={}\n",
            png.display()
        )
        .as_bytes(),
    );
    let mut theme_icons = ThemeIcons::with_roots(&Assets::builtin(), vec![], vec![apps]);
    let pixmap = theme_icons.load_for_entry("own", 16).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (16, 16));
}

#[test]
fn a_name_loads_as_a_pixmap_and_a_file_uri_is_a_path() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("p.png");
    support::write_png(&png, 8, 8, [0, 0, 255, 255]);
    let mut theme_icons = ThemeIcons::with_roots(&Assets::builtin(), vec![], vec![]);
    let by_path = theme_icons.load(png.to_str().unwrap(), 8).unwrap();
    let by_uri = theme_icons
        .load(&format!("file://{}", png.display()), 8)
        .unwrap();
    assert_eq!(by_path.data(), by_uri.data());
    assert_eq!(by_path.pixels()[0].blue(), 255);
    assert!(theme_icons.load("no-such-icon", 8).is_none());
}
