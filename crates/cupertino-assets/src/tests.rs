use super::*;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = r#"{
  "version": 1,
  "fonts": {"ui": "Fixture Sans", "mono": "Fixture Mono"},
  "icons": {
    "folder": {"raster": [16, 32, 128]},
    "symbol.house": {"symbol": true},
    "document.generic": {"raster": [64], "symbol": false}
  },
  "cursors": "fixture",
  "wallpaper": "wallpapers/default.png"
}"#;

/// The files of `MANIFEST` other than the fonts.
const FILES: [&str; 7] = [
    "cursors/fixture/index.theme",
    "wallpapers/default.png",
    "icons/folder/16.png",
    "icons/folder/32.png",
    "icons/folder/128.png",
    "icons/symbol.house/symbol.svg",
    "icons/document.generic/64.png",
];

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

/// A pack directory with `manifest` and the pack-relative `files`.
fn pack(manifest: &str, files: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("manifest.json"), manifest).unwrap();
    for rel in files {
        touch(d.path(), rel);
    }
    d
}

/// The version-1 fixture with every file it names.
fn fixture() -> tempfile::TempDir {
    let d = pack(MANIFEST, &FILES);
    for rel in ["fonts/a.ttf", "fonts/b.otf", "fonts/notes.txt"] {
        touch(d.path(), rel);
    }
    d
}

fn name(s: &str) -> IconName {
    IconName::new(s).unwrap()
}

#[test]
fn the_original_pack_explicitly_provides_every_contract_icon() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/assets/cupertino");
    let assets = Assets::open(&root).expect("the original pack has no problems");
    for n in RASTER_NAMES {
        assert!(
            sources(&assets, n, 32)
                .iter()
                .any(|s| matches!(s, IconSource::Raster(_) | IconSource::Svg(_))),
            "{n}"
        );
    }
    for n in SYMBOL_NAMES {
        assert!(
            sources(&assets, n, 24)
                .iter()
                .any(|s| matches!(s, IconSource::Symbol(_))),
            "{n}"
        );
    }
}

fn sources(a: &Assets, n: &str, px: u32) -> Vec<IconSource> {
    // the packs' own sources: the built-in host-theme names that end every list have an empty pack path
    a.icon_sources(&name(n), px)
        .into_iter()
        .filter(|(root, _)| !root.as_os_str().is_empty())
        .map(|(_, s)| s)
        .collect()
}

fn raster(a: &Assets, n: &str, px: u32) -> Option<(PathBuf, u32)> {
    sources(a, n, px).into_iter().find_map(|s| match s {
        IconSource::Raster(r) => Some((r.path, r.px)),
        _ => None,
    })
}

#[test]
fn a_version_1_raster_resolves_to_the_smallest_size_at_least_requested_else_the_largest() {
    let d = fixture();
    let a = Assets::open(d.path()).unwrap();
    let pick = |px| raster(&a, "folder", px).map(|(_, s)| s);
    assert_eq!(
        [pick(1), pick(16), pick(17), pick(128), pick(512)],
        [Some(16), Some(16), Some(32), Some(128), Some(128)]
    );
    assert_eq!(
        raster(&a, "folder", 20).unwrap().0,
        d.path().join("icons/folder/32.png")
    );
}

#[test]
fn version_1_symbols_cursors_wallpaper_and_fonts_resolve() {
    let d = fixture();
    let a = Assets::open(d.path()).unwrap();
    assert_eq!(
        sources(&a, "symbol.house", 16),
        [IconSource::Symbol(
            d.path().join("icons/symbol.house/symbol.svg")
        )]
    );
    assert!(sources(&a, "computer", 16).is_empty());
    assert_eq!(
        a.cursor_theme(),
        Some(CursorTheme {
            name: "fixture".into(),
            search_dir: d.path().join("cursors")
        })
    );
    assert_eq!(
        a.wallpaper_path(false),
        Some(d.path().join("wallpapers/default.png"))
    );
    assert_eq!(
        a.wallpaper_path(true),
        Some(d.path().join("wallpapers/default.png")),
        "no dark picture: the light one"
    );
    assert_eq!(a.font_family(FontRole::Ui), Some("Fixture Sans"));
    assert_eq!(a.font_family(FontRole::Serif), None);
    let f = |rel: &str| FontSource::File(d.path().join(rel));
    assert_eq!(
        a.fonts(),
        [f("fonts/a.ttf"), f("fonts/b.otf"), FontSource::Builtin]
    );
}

const MANIFEST_2: &str = r#"{
  "version": 2,
  "name": "fixture-pack",
  "credits": "CREDITS.md",
  "fonts": {"ui": {"family": "Fixture Sans", "files": ["fonts/sans.ttf"]}, "mono": {"family": "Fixture Mono", "files": ["fonts/mono.ttf"]}},
  "fallback_fonts": ["fonts/arabic.ttf", "fonts/sans.ttf"],
  "icons": {
    "folder.documents": {"raster": {"32": "icons/documents-32.png", "64": "icons/documents-64.png"}, "theme": ["folder-documents"]},
    "document.pdf": {"svg": "icons/pdf.svg"},
    "symbol.wifi": {"symbol": "symbols/wifi.svg"},
    "symbol.battery": {"theme": ["battery-symbolic", "battery-full-symbolic"]}
  },
  "icon_theme": {"name": "Fixture", "map": "icon-map.toml"},
  "cursors": {"theme": "pointers", "path": ["cursors", "/nonexistent/icons"]},
  "wallpapers": {"default": {"light": "wallpapers/day.jpg", "dark": "wallpapers/night.jpg"}}
}"#;

fn fixture_2() -> tempfile::TempDir {
    let d = pack(
        MANIFEST_2,
        &[
            "CREDITS.md",
            "fonts/sans.ttf",
            "fonts/mono.ttf",
            "fonts/arabic.ttf",
            "icons/documents-32.png",
            "icons/documents-64.png",
            "icons/pdf.svg",
            "symbols/wifi.svg",
            "cursors/pointers/index.theme",
            "wallpapers/day.jpg",
            "wallpapers/night.jpg",
        ],
    );
    fs::write(
        d.path().join("icon-map.toml"),
        "\"folder\" = [\"folder\", \"inode-directory\"]\n",
    )
    .unwrap();
    d
}

#[test]
fn a_version_2_pack_resolves_every_kind_of_source() {
    let d = fixture_2();
    let a = Assets::open(d.path()).unwrap();
    let theme = |names: &[&str]| IconSource::Theme {
        names: names.iter().map(|n| n.to_string()).collect(),
        theme: Some("Fixture".into()),
    };
    assert_eq!(
        sources(&a, "folder.documents", 40),
        [
            IconSource::Raster(RasterIcon {
                path: d.path().join("icons/documents-64.png"),
                px: 64
            }),
            theme(&["folder-documents"])
        ]
    );
    assert_eq!(
        sources(&a, "document.pdf", 16),
        [IconSource::Svg(d.path().join("icons/pdf.svg"))]
    );
    assert_eq!(
        sources(&a, "symbol.wifi", 16),
        [IconSource::Symbol(d.path().join("symbols/wifi.svg"))]
    );
    assert_eq!(
        sources(&a, "symbol.battery", 16),
        [theme(&["battery-symbolic", "battery-full-symbolic"])]
    );
    assert_eq!(
        sources(&a, "folder", 16),
        [theme(&["folder", "inode-directory"])],
        "an unlisted name takes the default map"
    );
    assert!(sources(&a, "computer", 16).is_empty());
    assert_eq!(
        a.cursor_theme(),
        Some(CursorTheme {
            name: "pointers".into(),
            search_dir: d.path().join("cursors")
        })
    );
    assert_eq!(
        a.wallpaper_path(true),
        Some(d.path().join("wallpapers/night.jpg"))
    );
    assert_eq!(a.font_family(FontRole::Mono), Some("Fixture Mono"));
    let f = |rel: &str| FontSource::File(d.path().join(rel));
    assert_eq!(
        a.fonts(),
        [
            f("fonts/sans.ttf"),
            f("fonts/mono.ttf"),
            f("fonts/arabic.ttf"),
            FontSource::Builtin
        ],
        "roles, then fallbacks, once each"
    );
    assert_eq!(
        a.credits().next(),
        Some((
            d.path(),
            Some("fixture-pack"),
            Some(d.path().join("CREDITS.md"))
        ))
    );
}

/// An environment holding `vars`, for [`search_path_from`].
fn environment<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<std::ffi::OsString> + 'a {
    move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.into())
}

#[test]
fn the_search_path_is_the_variable_then_the_user_then_the_system_pack() {
    assert_eq!(
        search_path_from(environment(&[
            ("CUPERTINO_ASSET_DIR", "/packs/dev"),
            ("XDG_DATA_HOME", "/data"),
            ("HOME", "/home/u")
        ])),
        [
            PathBuf::from("/packs/dev"),
            "/data/cupertino/assets".into(),
            PathBuf::from(SYSTEM_PACK).join("cupertino"),
            SYSTEM_PACK.into()
        ]
    );
    assert_eq!(
        search_path_from(environment(&[
            ("CUPERTINO_ASSET_DIR", ""),
            ("HOME", "/home/u")
        ])),
        [
            PathBuf::from("/home/u/.local/share/cupertino/assets"),
            PathBuf::from(SYSTEM_PACK).join("cupertino"),
            SYSTEM_PACK.into()
        ]
    );
    assert_eq!(
        search_path_from(environment(&[])),
        [
            PathBuf::from(SYSTEM_PACK).join("cupertino"),
            PathBuf::from(SYSTEM_PACK)
        ]
    );
}

#[test]
fn the_search_path_variable_replaces_the_whole_list() {
    let list = |search: &str| {
        search_path_from(environment(&[
            ("CUPERTINO_ASSET_SEARCH_PATH", search),
            ("CUPERTINO_ASSET_DIR", "/packs/dev"),
            ("HOME", "/home/u"),
        ]))
    };
    assert_eq!(list("/a:/b"), [PathBuf::from("/a"), "/b".into()]);
    assert_eq!(list(""), Vec::<PathBuf>::new(), "empty: built-in only");
}

#[test]
fn a_name_one_pack_lacks_comes_from_the_next_pack() {
    let first = fixture();
    let second = fixture();
    fs::remove_file(first.path().join("icons/folder/32.png")).unwrap();
    fs::remove_dir_all(first.path().join("cursors/fixture")).unwrap();
    fs::remove_file(first.path().join("icons/symbol.house/symbol.svg")).unwrap();
    let a = Assets::from_dirs(&[first.path().to_path_buf(), second.path().to_path_buf()]);
    assert_eq!(
        raster(&a, "folder", 20).unwrap().0,
        second.path().join("icons/folder/32.png")
    );
    assert_eq!(
        raster(&a, "folder", 16).unwrap().0,
        first.path().join("icons/folder/16.png")
    );
    assert_eq!(
        sources(&a, "symbol.house", 16),
        [IconSource::Symbol(
            second.path().join("icons/symbol.house/symbol.svg")
        )]
    );
    assert_eq!(
        a.cursor_theme().unwrap().search_dir,
        second.path().join("cursors")
    );
    assert_eq!(a.roots().collect::<Vec<_>>(), [first.path(), second.path()]);
}

#[test]
fn absent_and_unusable_packs_are_skipped_and_bad_entries_dropped() {
    let good = fixture();
    let invalid = pack("{", &[]);
    let future = pack(r#"{"version": 3}"#, &[]);
    let partial = pack(
        r#"{"version": 2, "icons": {"Folder": {"svg": "x.svg"}, "computer": {"svg": "../x.svg"}}, "future_key": 1}"#,
        &[],
    );
    let absent = good.path().join("no-such-pack");
    let dirs = [
        absent,
        invalid.path().to_path_buf(),
        future.path().to_path_buf(),
        partial.path().to_path_buf(),
        good.path().to_path_buf(),
    ];
    let a = Assets::from_dirs(&dirs);
    assert_eq!(a.roots().collect::<Vec<_>>(), [partial.path(), good.path()]);
    assert_eq!(a.font_family(FontRole::Ui), Some("Fixture Sans"));
    assert!(sources(&a, "computer", 16).is_empty());
}

#[test]
fn a_fonts_directory_that_cannot_be_read_gives_no_fonts_and_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let d = fixture();
    fs::set_permissions(d.path().join("fonts"), fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = fs::read_dir(d.path().join("fonts")).is_err();
    let read = Assets::read_pack(d.path());
    fs::set_permissions(d.path().join("fonts"), fs::Permissions::from_mode(0o755)).unwrap();
    if !unreadable {
        return; // running as root: every directory is readable
    }
    let (a, notes) = read.unwrap();
    assert_eq!(a.fonts(), [FontSource::Builtin]);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("the pack's fonts are not used")),
        "{notes:?}"
    );
}

#[test]
fn the_built_in_fallback_is_the_embedded_font_and_nothing_else() {
    let a = Assets::builtin();
    assert!(sources(&a, "folder", 16).is_empty() && sources(&a, "symbol.house", 16).is_empty());
    assert_eq!(
        (
            a.cursor_theme(),
            a.wallpaper_path(false),
            a.font_family(FontRole::Ui)
        ),
        (None, None, None)
    );
    assert_eq!(a.fonts(), [FontSource::Builtin]);
    assert_eq!(&BUILTIN_FONT[..4], &[0, 1, 0, 0], "a TrueType font");
}

#[test]
fn the_strict_check_rejects_with_a_reason() {
    let cases = [
        (
            MANIFEST.replace("\"version\": 1", "\"version\": 3"),
            "contract version 3",
        ),
        (MANIFEST.replace("\"ui\"", "\"display\""), "display"),
        (
            MANIFEST.replace("\"folder\"", "\"Folder\""),
            "invalid semantic icon name",
        ),
        (
            MANIFEST.replace("\"wallpapers/default.png\"", "\"../x.png\""),
            "wallpaper",
        ),
        (MANIFEST.replace("\"fixture\"", "\"a/b\""), "theme name"),
        (MANIFEST.replace("[16, 32, 128]", "[]"), "icons.folder"),
        (
            MANIFEST.replace("\"version\": 1,", "\"version\": 1, \"colour\": 1,"),
            "unknown key \"colour\"",
        ),
        ("{".to_string(), "EOF"),
    ];
    for (text, needle) in cases {
        let d = pack(&text, &FILES);
        touch(d.path(), "fonts/a.ttf");
        let msg = Assets::open(d.path()).unwrap_err().to_string();
        assert!(msg.contains(needle), "{needle}: {msg}");
    }
    let empty = tempfile::tempdir().unwrap();
    assert!(Assets::open(empty.path())
        .unwrap_err()
        .to_string()
        .contains("manifest.json"));
}

#[test]
fn the_strict_check_names_a_file_the_manifest_names_but_lacks() {
    for rel in [
        "icons/folder/32.png",
        "icons/symbol.house/symbol.svg",
        "wallpapers/default.png",
        "cursors/fixture",
        "fonts/a.ttf",
    ] {
        let d = fixture();
        let p = d.path().join(rel);
        if p.is_dir() {
            fs::remove_dir_all(&p).unwrap()
        } else {
            fs::remove_file(&p).unwrap()
        }
        if rel == "fonts/a.ttf" {
            fs::remove_file(d.path().join("fonts/b.otf")).unwrap();
        }
        let msg = Assets::open(d.path()).unwrap_err().to_string();
        assert!(msg.contains("missing"), "{rel}: {msg}");
    }
}

#[test]
fn the_builtin_icon_map_covers_every_logical_name_and_parses() {
    let map = builtin_icon_map();
    for name in RASTER_NAMES.iter().chain(&SYMBOL_NAMES) {
        assert!(
            map.get(*name).is_some_and(|n| !n.is_empty()),
            "no host-theme names for {name}"
        );
    }
    assert_eq!(
        map["folder.downloads"],
        ["folder-download", "folder-downloads"]
    );
    assert!(map.keys().all(|k| IconName::new(k).is_ok()));
}

#[test]
fn with_no_pack_every_icon_resolves_to_the_host_theme() {
    let none = Assets::builtin();
    let name = IconName::new("symbol.wifi").unwrap();
    let sources = none.icon_sources(&name, 16);
    assert_eq!(sources.len(), 1);
    assert!(
        matches!(&sources[0].1, IconSource::Theme { names, theme: None } if names[0] == "network-wireless-symbolic")
    );
}

#[test]
fn a_pack_source_comes_before_the_builtin_host_names() {
    let d = pack(MANIFEST, &FILES);
    let a = Assets::from_dirs(&[d.path().to_path_buf()]);
    let sources = a.icon_sources(&IconName::new("folder").unwrap(), 32);
    assert!(matches!(sources.first(), Some((_, IconSource::Raster(_)))));
    assert!(matches!(
        sources.last(),
        Some((_, IconSource::Theme { theme: None, .. }))
    ));
}

#[test]
fn the_pictures_of_a_directory_pair_light_and_dark_files_and_skip_the_rest() {
    let d = tempfile::tempdir().unwrap();
    for rel in [
        "sunrise.jpg",
        "sunrise-dark.jpg",
        "ridge.PNG",
        "night-dark.png",
        ".hidden.png",
        "notes.txt",
        "photo.jpeg",
    ] {
        touch(d.path(), rel);
    }
    fs::create_dir(d.path().join("folder.png")).unwrap();
    let list = pictures_in(d.path());
    let summary: Vec<(&str, bool)> = list
        .iter()
        .map(|f| (f.name.as_str(), f.dark.is_some()))
        .collect();
    assert_eq!(
        summary,
        [
            ("night-dark", false),
            ("photo", false),
            ("ridge", false),
            ("sunrise", true)
        ],
        "a dark file without a light one stands alone; hidden files, other types and folders are skipped"
    );
    assert_eq!(
        list[3].dark.as_deref(),
        Some(d.path().join("sunrise-dark.jpg").as_path())
    );
    assert!(pictures_in(&d.path().join("missing")).is_empty());
}

#[test]
fn the_packs_list_their_wallpapers_and_the_first_pack_wins_a_name() {
    let first = pack(
        MANIFEST,
        &["wallpapers/shared.png", "wallpapers/only-first.jpg"],
    );
    let second = pack(
        MANIFEST,
        &["wallpapers/shared.jpg", "wallpapers/only-second.png"],
    );
    let a = Assets::from_dirs(&[first.path().to_path_buf(), second.path().to_path_buf()]);
    let names: Vec<(String, PathBuf)> = a
        .wallpaper_files()
        .into_iter()
        .map(|f| (f.name, f.light))
        .collect();
    assert_eq!(
        names,
        [
            (
                "only-first".to_string(),
                first.path().join("wallpapers/only-first.jpg")
            ),
            (
                "shared".to_string(),
                first.path().join("wallpapers/shared.png")
            ),
            (
                "only-second".to_string(),
                second.path().join("wallpapers/only-second.png")
            ),
        ]
    );
}

#[test]
fn the_credits_file_names_the_title_and_photographer_of_each_picture() {
    let text = "Wallpapers: photographs from Unsplash.\n\nwallpapers/snow-W3V.jpg\n  \"Trees covering snow\" by Ryan Pohanic (https://unsplash.com/@elevatedtv)\n  Source: https://unsplash.com/photos/snow\n  License: Unsplash License\n\nwallpapers/other.jpg\n  no quotes here\n";
    let credits = parse_credits(text);
    assert_eq!(
        credits.get("snow-W3V.jpg"),
        Some(&Credit {
            title: "Trees covering snow".into(),
            author: "Ryan Pohanic".into()
        })
    );
    assert_eq!(credits.len(), 1);
    let pack_dir = pack(
        MANIFEST,
        &["wallpapers/snow-W3V.jpg", "wallpapers/plain.png"],
    );
    fs::write(pack_dir.path().join("wallpapers/credits.txt"), text).unwrap();
    let a = Assets::from_dirs(&[pack_dir.path().to_path_buf()]);
    let files = a.wallpaper_files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "plain");
    assert_eq!(files[0].credit, None);
    assert_eq!(
        files[1].credit.as_ref().map(|c| c.author.as_str()),
        Some("Ryan Pohanic")
    );
}

#[test]
fn only_the_first_megabyte_of_a_credits_file_is_read() {
    let pack_dir = pack(MANIFEST, &["wallpapers/a.jpg", "wallpapers/z.jpg"]);
    let mut text = String::from("wallpapers/a.jpg\n  \"First\" by Early (x)\n");
    text.push_str(&" ".repeat(2 << 20));
    text.push_str("\nwallpapers/z.jpg\n  \"Last\" by Late (y)\n");
    fs::write(pack_dir.path().join("wallpapers/credits.txt"), text).unwrap();
    let files = Assets::from_dirs(&[pack_dir.path().to_path_buf()]).wallpaper_files();
    assert_eq!(
        files[0].credit.as_ref().map(|c| c.author.as_str()),
        Some("Early")
    );
    assert_eq!(files[1].credit, None, "a credit past the cap is not read");
}
