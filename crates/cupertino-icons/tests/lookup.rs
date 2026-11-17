//! `IconLookup` and `ThemeIcons`: where an application's icon file is found by name, size and scale.

#[allow(dead_code)]
mod support;

use std::path::{Path, PathBuf};

use cupertino_assets::Assets;
use cupertino_icons::{IconLookup, ThemeIcons};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const APPS_48: [(&str, u32, &str); 1] = [("48x48/apps", 48, "Fixed")];

#[test]
fn a_name_is_found_in_hicolor_at_the_nearest_size_then_in_the_flat_directories() {
    let dir = tempfile::tempdir().unwrap();
    let (icons, second_base) = (dir.path().join("icons"), dir.path().join("second"));
    let hicolor = icons.join("hicolor");
    support::write(
        &hicolor.join("index.theme"),
        b"[Icon Theme]\nDirectories=16x16/apps,32x32/apps,48x48/apps,256x256/apps,scalable/apps\n\n\
          [16x16/apps]\nSize=16\nType=Fixed\n\n[32x32/apps]\nSize=32\nType=Fixed\n\n\
          [48x48/apps]\nSize=48\nType=Fixed\n\n[256x256/apps]\nSize=256\nType=Fixed\n\n\
          [scalable/apps]\nSize=64\nMinSize=8\nMaxSize=512\nType=Scalable\n",
    );
    for px in [16, 48, 256] {
        support::write_png(
            &hicolor.join(format!("{px}x{px}/apps/mail.png")),
            px,
            px,
            BLACK,
        );
    }
    support::write_png(&hicolor.join("32x32/apps/x.png"), 32, 32, BLACK);
    support::write(
        &hicolor.join("scalable/apps/x.svg"),
        support::BLACK_SVG.as_bytes(),
    );
    support::write_png(&hicolor.join("48x48/apps/both.png"), 48, 48, BLACK);
    support::write(
        &hicolor.join("48x48/apps/both.svg"),
        support::BLACK_SVG.as_bytes(),
    );
    support::write_png(&icons.join("flat.png"), 8, 8, BLACK);
    support::write(&icons.join("vector.svg"), support::BLACK_SVG.as_bytes());
    let other = support::theme(&icons, "Other", None, &APPS_48);
    support::write_png(&other.join("48x48/apps/other-only.png"), 48, 48, BLACK);
    let second = support::theme(&second_base, "hicolor", None, &APPS_48);
    support::write_png(&second.join("48x48/apps/only-second.png"), 48, 48, BLACK);
    let file = dir.path().join("mine.png");
    support::write_png(&file, 8, 8, BLACK);

    let mut lookup = IconLookup::with_roots(&Assets::builtin(), vec![icons.clone(), second_base]);
    let path = |p: &Path| p.to_str().unwrap().to_string();
    for (row, name, size, scale, expected) in [
        (
            "exact size",
            "mail".to_string(),
            16,
            1,
            Some(hicolor.join("16x16/apps/mail.png")),
        ),
        (
            "nearest size",
            "mail".to_string(),
            40,
            1,
            Some(hicolor.join("48x48/apps/mail.png")),
        ),
        (
            "size times scale",
            "mail".to_string(),
            24,
            2,
            Some(hicolor.join("48x48/apps/mail.png")),
        ),
        (
            "above every size",
            "mail".to_string(),
            1000,
            1,
            Some(hicolor.join("256x256/apps/mail.png")),
        ),
        (
            "an exact fixed size before scalable",
            "x".to_string(),
            32,
            1,
            Some(hicolor.join("32x32/apps/x.png")),
        ),
        (
            "scalable within its range",
            "x".to_string(),
            100,
            1,
            Some(hicolor.join("scalable/apps/x.svg")),
        ),
        (
            "a png before an svg",
            "both".to_string(),
            48,
            1,
            Some(hicolor.join("48x48/apps/both.png")),
        ),
        (
            "a flat png",
            "flat".to_string(),
            32,
            1,
            Some(icons.join("flat.png")),
        ),
        (
            "a flat svg",
            "vector".to_string(),
            32,
            1,
            Some(icons.join("vector.svg")),
        ),
        (
            "hicolor of a later base",
            "only-second".to_string(),
            48,
            1,
            Some(second.join("48x48/apps/only-second.png")),
        ),
        (
            "other themes need an opt-in",
            "other-only".to_string(),
            48,
            1,
            None,
        ),
        ("unknown", "missing".to_string(), 48, 1, None),
        ("empty", String::new(), 32, 1, None),
        ("an absolute file", path(&file), 64, 2, Some(file.clone())),
        (
            "an absolute path that is missing",
            path(&dir.path().join("none.png")),
            32,
            1,
            None,
        ),
        ("a directory is no icon", path(dir.path()), 32, 1, None),
    ] {
        assert_eq!(lookup.find(&name, size, scale), expected, "{row}");
    }
}

#[test]
fn the_pack_comes_before_the_themes_and_an_opted_in_pack_theme_before_hicolor() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("pack");
    support::write_png(&pack.join("icons/files-48.png"), 48, 48, [5, 5, 5, 255]);
    let assets = support::pack(
        &pack,
        r#"{"version": 2, "icon_theme": {"name": "Fancy", "applications": true},
            "icons": {"application.files": {"raster": {"48": "icons/files-48.png"}}}}"#,
    );
    let icons = dir.path().join("icons");
    let fancy = support::theme(&icons, "Fancy", Some("Parent"), &APPS_48);
    let parent = support::theme(&icons, "Parent", None, &APPS_48);
    let hicolor = support::theme(&icons, "hicolor", None, &APPS_48);
    for (root, names) in [
        (&fancy, &["a"][..]),
        (&parent, &["a", "b"]),
        (
            &hicolor,
            &["a", "c", "application.files", "application.other"],
        ),
    ] {
        for name in names {
            support::write_png(&root.join(format!("48x48/apps/{name}.png")), 48, 48, BLACK);
        }
    }
    let mut lookup = IconLookup::with_roots(&assets, vec![icons]);
    for (name, expected) in [
        ("application.files", pack.join("icons/files-48.png")),
        (
            "application.other",
            hicolor.join("48x48/apps/application.other.png"),
        ),
        ("a", fancy.join("48x48/apps/a.png")),
        ("b", parent.join("48x48/apps/b.png")),
        ("c", hicolor.join("48x48/apps/c.png")),
    ] {
        assert_eq!(lookup.find(name, 48, 1), Some(expected), "{name}");
    }
}

#[test]
fn the_icon_of_a_desktop_entry_is_loaded_by_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("applications");
    let own = dir.path().join("own.png");
    support::write_png(&own, 16, 16, [0, 255, 0, 255]);
    let entry = |id: &str, icon: &str| {
        support::write(
            &apps.join(format!("{id}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName={id}\nExec={id}\n{icon}").as_bytes(),
        );
    };
    entry("org.example.Mail", "Icon=mail-icon\n");
    entry("own", &format!("Icon={}\n", own.display()));
    entry("plain", "");
    let icons = dir.path().join("icons");
    let root = support::theme(&icons, "hicolor", None, &APPS_48);
    support::write_png(
        &root.join("48x48/apps/mail-icon.png"),
        48,
        48,
        [255, 0, 0, 255],
    );
    let mut theme_icons = ThemeIcons::with_roots(&Assets::builtin(), vec![icons], vec![apps]);
    for (id, size, expected) in [
        ("org.example.Mail", 48, Some((48, [255, 0, 0]))),
        ("org.example.Mail.desktop", 48, Some((48, [255, 0, 0]))),
        ("own", 16, Some((16, [0, 255, 0]))),
        ("plain", 32, None),
        ("absent", 32, None),
    ] {
        let got = theme_icons.load_for_entry(id, size).map(|p| {
            let c = p.pixels()[0];
            (p.width(), [c.red(), c.green(), c.blue()])
        });
        assert_eq!(got, expected, "{id}");
    }
}

#[test]
fn a_name_path_or_file_uri_loads_as_a_pixmap() {
    let dir = tempfile::tempdir().unwrap();
    let png: PathBuf = dir.path().join("p.png");
    support::write_png(&png, 8, 8, [0, 0, 255, 255]);
    let mut theme_icons = ThemeIcons::with_roots(&Assets::builtin(), vec![], vec![]);
    let by_path = theme_icons.load(png.to_str().unwrap(), 8).unwrap();
    assert_eq!(by_path.pixels()[0].blue(), 255);
    let by_uri = theme_icons
        .load(&format!("file://{}", png.display()), 8)
        .unwrap();
    assert_eq!(by_path.data(), by_uri.data());
    assert!(theme_icons.load("no-such-icon", 8).is_none());
}
