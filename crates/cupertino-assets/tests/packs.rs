//! Reading packs, strict opening, and per-name resolution across packs, through the public API over temp dirs.

mod common;

use common::{manifest, write};
use cupertino_assets::{
    AssetError, Assets, CursorTheme, FontRole, IconName, IconSource, RasterIcon, BUILTIN_UI_FAMILY,
};
use std::path::{Path, PathBuf};

fn name(n: &str) -> IconName {
    IconName::new(n).unwrap()
}

fn pack(json: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    manifest(dir.path(), json);
    dir
}

fn load(dirs: &[&Path]) -> Assets {
    Assets::from_dirs(&dirs.iter().map(|d| d.to_path_buf()).collect::<Vec<_>>())
}

// ---- errors of reading and opening ----

#[test]
fn read_pack_without_a_manifest_is_a_read_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    match Assets::read_pack(dir.path()) {
        Err(AssetError::Read { path, .. }) => assert_eq!(path, dir.path().join("manifest.json")),
        other => panic!("expected a read error, got {other:?}"),
    }
}

#[test]
fn a_read_error_displays_its_path_and_exposes_the_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = Assets::read_pack(dir.path()).err().unwrap();
    assert!(err.to_string().contains("manifest.json"));
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn invalid_json_is_a_manifest_error() {
    let dir = pack("{ not json");
    match Assets::read_pack(dir.path()) {
        Err(AssetError::Manifest { path, .. }) => {
            assert_eq!(path, dir.path().join("manifest.json"))
        }
        other => panic!("expected a manifest error, got {other:?}"),
    }
}

#[test]
fn an_unsupported_version_is_a_manifest_error_saying_so() {
    let dir = pack(r#"{"version": 3}"#);
    match Assets::read_pack(dir.path()) {
        Err(AssetError::Manifest { reason, .. }) => assert!(reason.contains("not supported")),
        other => panic!("expected a manifest error, got {other:?}"),
    }
}

#[test]
fn a_manifest_without_a_version_is_unusable() {
    let dir = pack(r#"{"name": "x"}"#);
    assert!(matches!(
        Assets::read_pack(dir.path()),
        Err(AssetError::Manifest { .. })
    ));
}

#[test]
fn a_manifest_that_is_not_an_object_is_unusable() {
    let dir = pack("[1, 2]");
    assert!(matches!(
        Assets::open(dir.path()),
        Err(AssetError::Manifest { .. })
    ));
}

#[test]
fn a_manifest_error_displays_its_path_and_reason() {
    let dir = pack(r#"{"version": 3}"#);
    let text = Assets::open(dir.path()).err().unwrap().to_string();
    assert!(text.contains("manifest.json") && text.contains("not supported"));
}

#[test]
fn a_font_error_displays_its_reason() {
    let err = AssetError::Font {
        reason: "no usable face".to_string(),
    };
    assert_eq!(err.to_string(), "no usable face");
}

#[test]
fn read_pack_reports_unknown_keys_and_still_reads_the_pack() {
    let dir = pack(r#"{"version": 2, "name": "p", "surprise": 1}"#);
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("surprise")));
    assert_eq!(assets.roots().collect::<Vec<_>>(), vec![dir.path()]);
}

#[test]
fn read_pack_reports_files_the_manifest_names_but_lacks() {
    let dir = pack(
        r#"{"version": 2,
            "fonts": {"ui": {"family": "F", "files": ["fonts/f.ttf"]}},
            "icons": {"folder": {"raster": {"32": "icons/folder-32.png"}}},
            "wallpapers": {"default": {"light": "w/day.jpg", "dark": "w/night.jpg"}}}"#,
    );
    let (_, notes) = Assets::read_pack(dir.path()).unwrap();
    for gone in [
        "fonts/f.ttf",
        "icons/folder-32.png",
        "w/day.jpg",
        "w/night.jpg",
    ] {
        assert!(
            notes
                .iter()
                .any(|n| n.contains(gone) && n.contains("missing")),
            "no note about {gone}: {notes:?}"
        );
    }
}

#[test]
fn read_pack_drops_an_entry_with_an_escaping_path_and_says_so() {
    let dir = pack(r#"{"version": 2, "credits": "../outside.md"}"#);
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("relative path")));
    assert_eq!(assets.credits().next().unwrap().2, None);
}

#[test]
fn read_pack_drops_an_invalid_icon_name_and_an_icon_without_a_source() {
    let dir = pack(r#"{"version": 2, "icons": {"Not Valid": {"svg": "a.svg"}, "folder": {}}}"#);
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes
        .iter()
        .any(|n| n.contains("invalid semantic icon name")));
    assert!(notes.iter().any(|n| n.contains("provides no source")));
    assert!(assets
        .icon_sources(&name("folder"), 32)
        .iter()
        .all(|(p, _)| p.as_os_str().is_empty()));
}

#[test]
fn open_accepts_a_complete_pack() {
    let dir = pack(
        r#"{"version": 2, "name": "p",
            "fonts": {"ui": {"family": "F", "files": ["fonts/f.ttf"]}},
            "icons": {"document.pdf": {"svg": "pdf.svg"}}}"#,
    );
    write(dir.path(), "fonts/f.ttf", "x");
    write(dir.path(), "pdf.svg", "<svg/>");
    assert!(Assets::open(dir.path()).is_ok());
}

#[test]
fn open_rejects_a_pack_whose_named_file_is_missing() {
    let dir = pack(r#"{"version": 2, "icons": {"document.pdf": {"svg": "pdf.svg"}}}"#);
    match Assets::open(dir.path()) {
        Err(AssetError::Manifest { reason, .. }) => assert!(reason.contains("pdf.svg")),
        other => panic!("expected a manifest error, got {other:?}"),
    }
}

#[test]
fn open_rejects_a_pack_with_an_unknown_key() {
    let dir = pack(r#"{"version": 2, "mystery": true}"#);
    assert!(matches!(
        Assets::open(dir.path()),
        Err(AssetError::Manifest { .. })
    ));
}

#[test]
fn a_font_listing_that_cannot_be_read_costs_the_pack_its_fonts_not_the_pack() {
    // version 1 lists `fonts/` from the directory; a file in its place cannot be listed
    let dir = pack(r#"{"version": 1, "fonts": {"ui": "F"}}"#);
    write(dir.path(), "fonts", "not a directory");
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("fonts are not used")));
    assert_eq!(assets.fonts().len(), 1);
}

// ---- icons ----

#[test]
fn a_raster_icon_takes_the_smallest_size_that_covers_the_request() {
    let dir = pack(
        r#"{"version": 2, "icons": {"folder": {"raster":
            {"16": "f16.png", "32": "f32.png", "64": "f64.png"}}}}"#,
    );
    for f in ["f16.png", "f32.png", "f64.png"] {
        write(dir.path(), f, "png");
    }
    let assets = load(&[dir.path()]);
    let first = |px| match assets.icon_sources(&name("folder"), px).remove(0).1 {
        IconSource::Raster(r) => r,
        other => panic!("expected a raster, got {other:?}"),
    };
    assert_eq!(
        first(20),
        RasterIcon {
            path: dir.path().join("f32.png"),
            px: 32
        }
    );
    assert_eq!(first(16).px, 16);
    assert_eq!(first(33).px, 64);
}

#[test]
fn a_raster_request_above_every_size_takes_the_largest() {
    let dir =
        pack(r#"{"version": 2, "icons": {"folder": {"raster": {"16": "a.png", "64": "b.png"}}}}"#);
    write(dir.path(), "a.png", "");
    write(dir.path(), "b.png", "");
    let assets = load(&[dir.path()]);
    match assets.icon_sources(&name("folder"), 4096).remove(0).1 {
        IconSource::Raster(r) => assert_eq!(r.px, 64),
        other => panic!("expected a raster, got {other:?}"),
    }
}

#[test]
fn icon_sources_come_in_trial_order_raster_svg_symbol_theme() {
    let dir = pack(
        r#"{"version": 2, "icon_theme": {"name": "Adwaita"}, "icons": {"symbol.wifi": {
            "theme": ["network-wireless-symbolic"], "symbol": "s.svg", "svg": "i.svg",
            "raster": {"32": "r.png"}}}}"#,
    );
    for f in ["s.svg", "i.svg", "r.png"] {
        write(dir.path(), f, "");
    }
    let assets = load(&[dir.path()]);
    let sources: Vec<_> = assets
        .icon_sources(&name("symbol.wifi"), 32)
        .into_iter()
        .filter(|(p, _)| *p == dir.path())
        .map(|(_, s)| s)
        .collect();
    assert!(matches!(sources[0], IconSource::Raster(_)));
    assert_eq!(sources[1], IconSource::Svg(dir.path().join("i.svg")));
    assert_eq!(sources[2], IconSource::Symbol(dir.path().join("s.svg")));
    assert_eq!(
        sources[3],
        IconSource::Theme {
            names: vec!["network-wireless-symbolic".to_string()],
            theme: Some("Adwaita".to_string()),
        }
    );
    assert_eq!(sources.len(), 4);
}

#[test]
fn an_icon_source_whose_file_is_missing_is_left_out() {
    let dir = pack(
        r#"{"version": 2, "icons": {"document.pdf": {"svg": "gone.svg", "theme": ["x-office-document"]}}}"#,
    );
    let assets = load(&[dir.path()]);
    let sources = assets.icon_sources(&name("document.pdf"), 32);
    assert!(matches!(sources[0].1, IconSource::Theme { .. }));
    assert!(sources
        .iter()
        .all(|(_, s)| !matches!(s, IconSource::Svg(_))));
}

#[test]
fn the_first_pack_that_lists_an_icon_is_tried_first_and_later_packs_follow() {
    let a = pack(r#"{"version": 2, "icons": {"document.pdf": {"svg": "a.svg"}}}"#);
    let b = pack(r#"{"version": 2, "icons": {"document.pdf": {"svg": "b.svg"}}}"#);
    write(a.path(), "a.svg", "");
    write(b.path(), "b.svg", "");
    let assets = load(&[a.path(), b.path()]);
    let sources = assets.icon_sources(&name("document.pdf"), 32);
    assert_eq!(
        sources[0],
        (a.path(), IconSource::Svg(a.path().join("a.svg")))
    );
    assert_eq!(
        sources[1],
        (b.path(), IconSource::Svg(b.path().join("b.svg")))
    );
}

#[test]
fn a_partial_pack_leaves_the_other_names_to_the_next_pack() {
    let a = pack(r#"{"version": 2, "icons": {"folder": {"svg": "a.svg"}}}"#);
    let b = pack(r#"{"version": 2, "icons": {"computer": {"svg": "b.svg"}}}"#);
    write(a.path(), "a.svg", "");
    write(b.path(), "b.svg", "");
    let assets = load(&[a.path(), b.path()]);
    let computer = assets.icon_sources(&name("computer"), 32);
    assert_eq!(computer[0].0, b.path());
    assert!(computer.iter().all(|(p, _)| *p != a.path()));
}

#[test]
fn the_built_in_icon_map_ends_the_list_with_an_empty_pack_path() {
    let assets = Assets::builtin();
    let sources = assets.icon_sources(&name("folder"), 32);
    let (pack, source) = sources.last().expect("the built-in map names folder");
    assert_eq!(*pack, Path::new(""));
    assert!(matches!(source, IconSource::Theme { names, theme: None } if !names.is_empty()));
}

#[test]
fn a_name_in_no_pack_and_not_in_the_built_in_map_has_no_source() {
    let assets = Assets::builtin();
    assert!(assets.icon_sources(&name("no.such.icon"), 32).is_empty());
}

#[test]
fn the_packs_icon_theme_map_serves_the_names_it_does_not_list() {
    let dir = pack(r#"{"version": 2, "icon_theme": {"name": "Breeze", "map": "map.toml"}}"#);
    write(
        dir.path(),
        "map.toml",
        "\"no.such.icon\" = [\"first\", \"second\"]\n",
    );
    let assets = load(&[dir.path()]);
    let sources = assets.icon_sources(&name("no.such.icon"), 32);
    assert_eq!(
        sources,
        vec![(
            dir.path(),
            IconSource::Theme {
                names: vec!["first".to_string(), "second".to_string()],
                theme: Some("Breeze".to_string()),
            }
        )]
    );
}

#[test]
fn an_unreadable_icon_theme_map_is_reported_and_ignored() {
    let dir = pack(r#"{"version": 2, "icon_theme": {"map": "missing.toml"}}"#);
    let (_, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes
        .iter()
        .any(|n| n.contains("icon_theme.map") && n.contains("ignored")));
}

#[test]
fn the_application_icon_theme_is_the_first_one_that_asks_for_applications() {
    let a = pack(r#"{"version": 2, "icon_theme": {"name": "Plain"}}"#);
    let b = pack(r#"{"version": 2, "icon_theme": {"name": "Apps", "applications": true}}"#);
    assert_eq!(load(&[a.path()]).application_icon_theme(), None);
    assert_eq!(
        load(&[a.path(), b.path()]).application_icon_theme(),
        Some("Apps")
    );
}

// ---- version 1 ----

#[test]
fn a_version_one_manifest_is_read_as_the_version_two_entries_it_means() {
    let dir = pack(
        r#"{"version": 1, "fonts": {"ui": "Old Family"},
            "icons": {"folder": {"raster": [32]}, "symbol.wifi": {"symbol": true}},
            "wallpaper": "wall.jpg"}"#,
    );
    write(dir.path(), "icons/folder/32.png", "");
    write(dir.path(), "icons/symbol.wifi/symbol.svg", "");
    write(dir.path(), "wall.jpg", "");
    write(dir.path(), "fonts/old.ttf", "");
    let assets = Assets::open(dir.path()).unwrap();
    match assets.icon_sources(&name("folder"), 32).remove(0).1 {
        IconSource::Raster(r) => assert_eq!(r.path, dir.path().join("icons/folder/32.png")),
        other => panic!("expected a raster, got {other:?}"),
    }
    assert_eq!(
        assets.icon_sources(&name("symbol.wifi"), 16).remove(0).1,
        IconSource::Symbol(dir.path().join("icons/symbol.wifi/symbol.svg"))
    );
    assert_eq!(assets.font_family(FontRole::Ui), Some("Old Family"));
    assert_eq!(
        assets.wallpaper_path(false),
        Some(dir.path().join("wall.jpg"))
    );
}

#[test]
fn a_version_one_pack_loads_every_font_file_of_its_fonts_directory_sorted() {
    let dir = pack(r#"{"version": 1, "fonts": {"ui": "F"}}"#);
    write(dir.path(), "fonts/b.otf", "");
    write(dir.path(), "fonts/a.ttf", "");
    write(dir.path(), "fonts/C.TTF", "");
    write(dir.path(), "fonts/readme.txt", "");
    let fonts = load(&[dir.path()]).fonts();
    let files: Vec<_> = fonts
        .iter()
        .filter_map(|f| match f {
            cupertino_assets::FontSource::File(p) => p.file_name().map(|n| n.to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(files, vec!["C.TTF", "a.ttf", "b.otf"]);
}

#[test]
fn a_version_one_pack_with_font_roles_but_no_font_files_reports_it() {
    let dir = pack(r#"{"version": 1, "fonts": {"ui": "F"}}"#);
    let (_, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("a font file in fonts/")));
}

// ---- fonts ----

#[test]
fn a_font_role_resolves_to_the_family_of_the_first_pack_that_assigns_it() {
    let a = pack(r#"{"version": 2, "fonts": {"ui": {"family": "A Sans"}}}"#);
    let b = pack(
        r#"{"version": 2, "fonts": {"ui": {"family": "B Sans"}, "mono": {"family": "B Mono"}}}"#,
    );
    let assets = load(&[a.path(), b.path()]);
    assert_eq!(assets.font_family(FontRole::Ui), Some("A Sans"));
    assert_eq!(assets.font_family(FontRole::Mono), Some("B Mono"));
}

#[test]
fn a_role_no_pack_assigns_has_no_family() {
    let a = pack(r#"{"version": 2, "fonts": {"ui": {"family": "A Sans"}}}"#);
    let assets = load(&[a.path()]);
    for role in [
        FontRole::UiRounded,
        FontRole::UiCompact,
        FontRole::Mono,
        FontRole::Serif,
    ] {
        assert_eq!(assets.font_family(role), None, "{role:?}");
    }
    assert_eq!(Assets::builtin().font_family(FontRole::Ui), None);
}

#[test]
fn every_font_role_has_its_manifest_key() {
    let keys: Vec<_> = FontRole::ALL.iter().map(|r| r.key()).collect();
    assert_eq!(
        keys,
        vec!["ui", "ui-rounded", "ui-compact", "mono", "serif"]
    );
}

#[test]
fn a_font_entry_without_a_family_is_dropped() {
    let dir =
        pack(r#"{"version": 2, "fonts": {"ui": {"files": ["f.ttf"]}, "bogus": {"family": "X"}}}"#);
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("needs a family name")));
    assert!(notes.iter().any(|n| n.contains("unknown role")));
    assert_eq!(assets.font_family(FontRole::Ui), None);
}

#[test]
fn fonts_list_role_files_then_fallback_faces_without_repeats_and_end_with_the_built_in_font() {
    let dir = pack(
        r#"{"version": 2,
            "fonts": {"ui": {"family": "U", "files": ["u.ttf", "shared.ttf"]},
                      "mono": {"family": "M", "files": ["m.ttf", "shared.ttf"]}},
            "fallback_fonts": ["fallback.ttf", "u.ttf"]}"#,
    );
    for f in ["u.ttf", "shared.ttf", "m.ttf", "fallback.ttf"] {
        write(dir.path(), f, "");
    }
    let fonts = load(&[dir.path()]).fonts();
    let expected: Vec<_> = ["u.ttf", "shared.ttf", "m.ttf", "fallback.ttf"]
        .iter()
        .map(|f| cupertino_assets::FontSource::File(dir.path().join(f)))
        .chain([cupertino_assets::FontSource::Builtin])
        .collect();
    assert_eq!(fonts, expected);
}

#[test]
fn font_files_that_do_not_exist_are_not_listed() {
    let dir = pack(
        r#"{"version": 2, "fonts": {"ui": {"family": "U", "files": ["there.ttf", "gone.ttf"]}}}"#,
    );
    write(dir.path(), "there.ttf", "");
    let fonts = load(&[dir.path()]).fonts();
    assert_eq!(
        fonts,
        vec![
            cupertino_assets::FontSource::File(dir.path().join("there.ttf")),
            cupertino_assets::FontSource::Builtin
        ]
    );
}

#[test]
fn fonts_of_every_pack_come_in_lookup_order_before_the_built_in_font() {
    let a = pack(r#"{"version": 2, "fonts": {"ui": {"family": "A", "files": ["a.ttf"]}}}"#);
    let b = pack(r#"{"version": 2, "fonts": {"ui": {"family": "B", "files": ["b.ttf"]}}}"#);
    write(a.path(), "a.ttf", "");
    write(b.path(), "b.ttf", "");
    let fonts = load(&[a.path(), b.path()]).fonts();
    assert_eq!(fonts.len(), 3);
    assert_eq!(
        fonts[0],
        cupertino_assets::FontSource::File(a.path().join("a.ttf"))
    );
    assert_eq!(
        fonts[1],
        cupertino_assets::FontSource::File(b.path().join("b.ttf"))
    );
    assert_eq!(fonts[2], cupertino_assets::FontSource::Builtin);
}

#[test]
fn the_built_in_fallback_alone_loads_only_the_built_in_font() {
    assert_eq!(
        Assets::builtin().fonts(),
        vec![cupertino_assets::FontSource::Builtin]
    );
    assert_eq!(Assets::builtin().roots().count(), 0);
}

#[test]
fn a_font_source_labels_itself_for_messages() {
    let file = cupertino_assets::FontSource::File(PathBuf::from("/x/f.ttf"));
    assert_eq!(file.label(), "/x/f.ttf");
    assert!(cupertino_assets::FontSource::Builtin
        .label()
        .contains(BUILTIN_UI_FAMILY));
}

// ---- cursors ----

#[test]
fn a_cursor_theme_is_found_in_a_pack_relative_search_directory() {
    let dir = pack(r#"{"version": 2, "cursors": {"theme": "mine", "path": ["cursors"]}}"#);
    write(dir.path(), "cursors/mine/index.theme", "");
    let theme = load(&[dir.path()]).cursor_theme();
    assert_eq!(
        theme,
        Some(CursorTheme {
            name: "mine".to_string(),
            search_dir: dir.path().join("cursors"),
        })
    );
}

#[test]
fn the_cursor_search_directories_are_tried_in_order_and_absolute_ones_are_used_as_given() {
    let elsewhere = tempfile::tempdir().unwrap();
    write(elsewhere.path(), "mine/index.theme", "");
    let dir = pack(&format!(
        r#"{{"version": 2, "cursors": {{"theme": "mine", "path": ["nothing-here", {:?}]}}}}"#,
        elsewhere.path()
    ));
    let theme = load(&[dir.path()]).cursor_theme().unwrap();
    assert_eq!(theme.search_dir, elsewhere.path());
}

#[test]
fn a_version_one_cursor_theme_name_is_looked_up_in_the_packs_cursors_directory() {
    let dir = pack(r#"{"version": 1, "cursors": "legacy"}"#);
    write(dir.path(), "cursors/legacy/index.theme", "");
    let theme = load(&[dir.path()]).cursor_theme().unwrap();
    assert_eq!(
        (theme.name.as_str(), theme.search_dir),
        ("legacy", dir.path().join("cursors"))
    );
}

#[test]
fn a_cursor_theme_that_is_not_found_means_the_built_in_cursors_and_is_reported() {
    let dir = pack(r#"{"version": 2, "cursors": {"theme": "absent"}}"#);
    let assets = load(&[dir.path()]);
    assert_eq!(assets.cursor_theme(), None);
    let (_, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("cursor theme")));
}

#[test]
fn the_first_pack_whose_cursor_theme_is_found_wins() {
    let a = pack(r#"{"version": 2, "cursors": {"theme": "absent"}}"#);
    let b = pack(r#"{"version": 2, "cursors": {"theme": "present"}}"#);
    write(b.path(), "cursors/present/index.theme", "");
    let theme = load(&[a.path(), b.path()]).cursor_theme().unwrap();
    assert_eq!(theme.name, "present");
}

#[test]
fn a_cursor_theme_name_with_a_path_separator_is_rejected() {
    let dir = pack(r#"{"version": 2, "cursors": {"theme": "a/b"}}"#);
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    assert!(notes.iter().any(|n| n.contains("cursors.theme")));
    assert_eq!(assets.cursor_theme(), None);
}

// ---- wallpaper ----

#[test]
fn the_default_wallpaper_has_a_light_and_a_dark_picture() {
    let dir =
        pack(r#"{"version": 2, "wallpapers": {"default": {"light": "d.jpg", "dark": "n.jpg"}}}"#);
    write(dir.path(), "d.jpg", "");
    write(dir.path(), "n.jpg", "");
    let assets = load(&[dir.path()]);
    assert_eq!(assets.wallpaper_path(false), Some(dir.path().join("d.jpg")));
    assert_eq!(assets.wallpaper_path(true), Some(dir.path().join("n.jpg")));
}

#[test]
fn the_dark_wallpaper_is_the_light_one_when_the_pack_has_no_dark_picture() {
    let dir = pack(r#"{"version": 2, "wallpapers": {"default": {"light": "d.jpg"}}}"#);
    write(dir.path(), "d.jpg", "");
    assert_eq!(
        load(&[dir.path()]).wallpaper_path(true),
        Some(dir.path().join("d.jpg"))
    );
}

#[test]
fn a_wallpaper_whose_file_is_missing_falls_through_to_the_next_pack_then_to_none() {
    let a = pack(r#"{"version": 2, "wallpapers": {"default": {"light": "gone.jpg"}}}"#);
    let b = pack(r#"{"version": 2, "wallpapers": {"default": {"light": "b.jpg"}}}"#);
    write(b.path(), "b.jpg", "");
    assert_eq!(
        load(&[a.path(), b.path()]).wallpaper_path(false),
        Some(b.path().join("b.jpg"))
    );
    assert_eq!(load(&[a.path()]).wallpaper_path(false), None);
    assert_eq!(Assets::builtin().wallpaper_path(false), None);
}

#[test]
fn wallpaper_files_pair_dark_pictures_with_their_light_ones() {
    let dir = pack(r#"{"version": 2}"#);
    for f in [
        "sunrise.jpg",
        "sunrise-dark.jpg",
        "plain.png",
        ".hidden.png",
        "notes.txt",
    ] {
        write(dir.path(), &format!("wallpapers/{f}"), "");
    }
    let files = load(&[dir.path()]).wallpaper_files();
    let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["plain", "sunrise"]);
    assert_eq!(files[0].dark, None);
    assert_eq!(files[1].light, dir.path().join("wallpapers/sunrise.jpg"));
    assert_eq!(
        files[1].dark,
        Some(dir.path().join("wallpapers/sunrise-dark.jpg"))
    );
}

#[test]
fn wallpaper_files_carry_the_credit_the_packs_credits_file_names() {
    let dir = pack(r#"{"version": 2}"#);
    write(dir.path(), "wallpapers/sunrise.jpg", "");
    write(
        dir.path(),
        "wallpapers/credits.txt",
        "wallpapers/sunrise.jpg\n    \"Morning\" by Ada Lovelace (https://example.test)\n",
    );
    let credit = load(&[dir.path()])
        .wallpaper_files()
        .remove(0)
        .credit
        .unwrap();
    assert_eq!(
        (credit.title.as_str(), credit.author.as_str()),
        ("Morning", "Ada Lovelace")
    );
}

#[test]
fn a_wallpaper_name_an_earlier_pack_has_is_not_listed_again() {
    let a = pack(r#"{"version": 2}"#);
    let b = pack(r#"{"version": 2}"#);
    write(a.path(), "wallpapers/same.jpg", "");
    write(b.path(), "wallpapers/same.jpg", "");
    write(b.path(), "wallpapers/other.jpg", "");
    let files = load(&[a.path(), b.path()]).wallpaper_files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].light, a.path().join("wallpapers/same.jpg"));
}

#[test]
fn pictures_in_a_directory_that_cannot_be_read_are_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(cupertino_assets::pictures_in(&dir.path().join("nowhere")).is_empty());
}

#[test]
fn a_dark_picture_without_its_light_one_stands_alone() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lone-dark.png", "");
    let files = cupertino_assets::pictures_in(dir.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "lone-dark");
}

#[test]
fn parse_credits_reads_title_and_author_by_file_name() {
    let text = "wallpapers/a.jpg\n  \"Title A\" by Someone (https://x.test)\nwallpapers/b.jpg\n\t\"Title B\" by Other\nelse\n  \"Ignored\" by Nobody\n";
    let credits = cupertino_assets::parse_credits(text);
    assert_eq!(credits["a.jpg"].title, "Title A");
    assert_eq!(credits["a.jpg"].author, "Someone");
    assert_eq!(credits["b.jpg"].author, "Other");
    assert_eq!(credits.len(), 2);
}

// ---- pack listing ----

#[test]
fn roots_and_credits_list_each_pack_in_lookup_order() {
    let a = pack(r#"{"version": 2, "name": "alpha", "credits": "CREDITS.md"}"#);
    let b = pack(r#"{"version": 2}"#);
    let assets = load(&[a.path(), b.path()]);
    assert_eq!(assets.roots().collect::<Vec<_>>(), vec![a.path(), b.path()]);
    let credits: Vec<_> = assets.credits().collect();
    assert_eq!(
        credits[0],
        (a.path(), Some("alpha"), Some(a.path().join("CREDITS.md")))
    );
    assert_eq!(credits[1], (b.path(), None, None));
}

// ---- logical names ----

#[test]
fn icon_names_are_dot_separated_lowercase_segments() {
    for ok in ["folder", "symbol.chevron.up.down", "a-b.c1"] {
        assert_eq!(IconName::new(ok).unwrap().as_str(), ok);
        assert_eq!(IconName::new(ok).unwrap().to_string(), ok);
    }
    for bad in ["", "Folder", "a..b", ".a", "a.", "a b", "a_b", "a/b"] {
        assert!(IconName::new(bad).is_err(), "{bad:?} should be invalid");
    }
}

#[test]
fn the_logical_names_the_shell_looks_up_are_valid_and_unique() {
    use cupertino_assets::{CURSOR_NAMES, RASTER_NAMES, SYMBOL_NAMES};
    let icons: Vec<&str> = RASTER_NAMES.iter().chain(&SYMBOL_NAMES).copied().collect();
    for n in &icons {
        assert!(IconName::new(n).is_ok(), "{n}");
    }
    let mut sorted = icons.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), icons.len());
    assert!(SYMBOL_NAMES.iter().all(|n| n.starts_with("symbol.")));
    assert!(!CURSOR_NAMES.is_empty());
}

// ---- the report ----

#[test]
fn the_pack_report_names_what_provides_each_name_and_what_falls_back() {
    let dir = pack(
        r#"{"version": 2, "name": "p", "fonts": {"ui": {"family": "Fam"}},
            "icons": {"folder": {"svg": "f.svg"}}}"#,
    );
    write(dir.path(), "f.svg", "");
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    let report = cupertino_assets::PackReport::new(&assets, &notes);
    assert_eq!(report.fonts[0].provider, "Fam");
    assert!(report
        .icons
        .iter()
        .any(|i| i.name == "folder" && i.provider.starts_with("svg ")));
    assert!(report.fallbacks.contains(&"cursor theme".to_string()));
    assert!(report.fallbacks.contains(&"wallpaper light".to_string()));
    assert!(!report.fallbacks.contains(&"font ui".to_string()));
    let text = report.to_string();
    assert!(text.contains("problems: 0") && text.contains("falls back:"));
}

#[test]
fn the_pack_report_of_the_built_in_fallback_names_the_built_in_ui_font() {
    let report = cupertino_assets::PackReport::new(&Assets::builtin(), &[]);
    assert!(report.fallbacks.contains(&"font ui".to_string()));
    assert!(report.fonts[0].provider.contains(BUILTIN_UI_FAMILY));
}
