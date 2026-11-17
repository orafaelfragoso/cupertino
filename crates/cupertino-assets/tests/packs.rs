//! Reading packs, strict opening, and per-name resolution across packs, through the public API over temp dirs.

mod common;

use common::{manifest, write};
use cupertino_assets::{
    parse_credits, pictures_in, AssetError, Assets, CursorTheme, FontRole, FontSource, IconName,
    IconSource, PackReport, BUILTIN_UI_FAMILY,
};
use std::path::Path;

fn name(n: &str) -> IconName {
    IconName::new(n).unwrap()
}

/// A temp dir with `json` as its manifest and an empty file at each of `files`.
fn pack_with(json: &str, files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    manifest(dir.path(), json);
    for file in files {
        write(dir.path(), file, "");
    }
    dir
}

fn pack(json: &str) -> tempfile::TempDir {
    pack_with(json, &[])
}

fn load(dirs: &[&Path]) -> Assets {
    Assets::from_dirs(&dirs.iter().map(|d| d.to_path_buf()).collect::<Vec<_>>())
}

fn raster(assets: &Assets, icon: &str, px: u32) -> cupertino_assets::RasterIcon {
    match assets.icon_sources(&name(icon), px).remove(0).1 {
        IconSource::Raster(r) => r,
        other => panic!("{icon} at {px}: expected a raster, got {other:?}"),
    }
}

// ---- reading and opening ----

#[test]
fn a_pack_without_a_usable_manifest_is_refused_with_an_error_naming_the_manifest() {
    let empty = tempfile::tempdir().unwrap();
    match Assets::read_pack(empty.path()) {
        Err(AssetError::Read { path, .. }) => assert_eq!(path, empty.path().join("manifest.json")),
        other => panic!("no manifest: expected a read error, got {other:?}"),
    }
    for (row, json, reason) in [
        ("invalid json", "{ not json", None),
        (
            "unsupported version",
            r#"{"version": 3}"#,
            Some("not supported"),
        ),
        ("no version", r#"{"name": "x"}"#, None),
        ("not an object", "[1, 2]", None),
    ] {
        let dir = pack(json);
        for result in [
            Assets::read_pack(dir.path()).map(|_| ()),
            Assets::open(dir.path()).map(|_| ()),
        ] {
            let err = result.expect_err(row);
            let text = err.to_string();
            match err {
                AssetError::Manifest { path, reason: r } => {
                    assert_eq!(path, dir.path().join("manifest.json"), "{row}");
                    assert!(text.contains("manifest.json"), "{row}: {text}");
                    if let Some(reason) = reason {
                        assert!(r.contains(reason) && text.contains(reason), "{row}: {text}");
                    }
                }
                other => panic!("{row}: expected a manifest error, got {other:?}"),
            }
        }
    }
}

/// A row: its name, the manifest, the empty files the pack holds, the text its notes carry, and a check of what
/// the pack still provides.
type NoteRow = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    fn(&Assets) -> bool,
);

#[test]
fn read_pack_notes_each_problem_and_drops_only_the_bad_entry() {
    let rows: [NoteRow; 10] = [
        (
            "an unknown key",
            r#"{"version": 2, "name": "p", "surprise": 1}"#,
            &[],
            &["surprise"],
            |a| a.credits().next().unwrap().1 == Some("p"),
        ),
        (
            "files the manifest names but lacks",
            r#"{"version": 2,
                "fonts": {"ui": {"family": "F", "files": ["fonts/f.ttf"]}},
                "icons": {"folder": {"raster": {"32": "icons/folder-32.png"}}},
                "wallpapers": {"default": {"light": "w/day.jpg", "dark": "w/night.jpg"}}}"#,
            &[],
            &[
                "fonts/f.ttf",
                "icons/folder-32.png",
                "w/day.jpg",
                "w/night.jpg",
            ],
            |a| a.font_family(FontRole::Ui) == Some("F"),
        ),
        (
            "a path that escapes the pack",
            r#"{"version": 2, "credits": "../outside.md"}"#,
            &[],
            &["relative path"],
            |a| a.credits().next().unwrap().2.is_none(),
        ),
        (
            "an invalid icon name and an icon without a source",
            r#"{"version": 2, "icons": {"Not Valid": {"svg": "a.svg"}, "folder": {}}}"#,
            &["a.svg"],
            &["invalid semantic icon name", "provides no source"],
            |a| {
                a.icon_sources(&name("folder"), 32)
                    .iter()
                    .all(|(p, _)| p.as_os_str().is_empty())
            },
        ),
        (
            "a font entry without a family and an unknown role",
            r#"{"version": 2, "fonts": {"ui": {"files": ["f.ttf"]}, "bogus": {"family": "X"}}}"#,
            &["f.ttf"],
            &["needs a family name", "unknown role"],
            |a| a.font_family(FontRole::Ui).is_none(),
        ),
        (
            "a version 1 font listing that cannot be read costs the fonts, not the pack",
            r#"{"version": 1, "fonts": {"ui": "F"}}"#,
            &["fonts"],
            &["fonts are not used"],
            |a| a.fonts() == vec![FontSource::Builtin] && a.roots().count() == 1,
        ),
        (
            "version 1 font roles without font files",
            r#"{"version": 1, "fonts": {"ui": "F"}}"#,
            &[],
            &["a font file in fonts/"],
            |_| true,
        ),
        (
            "an unreadable icon theme map",
            r#"{"version": 2, "icon_theme": {"map": "missing.toml"}}"#,
            &[],
            &["icon_theme.map", "ignored"],
            |_| true,
        ),
        (
            "a cursor theme name with a path separator",
            r#"{"version": 2, "cursors": {"theme": "a/b"}}"#,
            &[],
            &["cursors.theme"],
            |a| a.cursor_theme().is_none(),
        ),
        (
            "a cursor theme that is not found means the built-in cursors",
            r#"{"version": 2, "cursors": {"theme": "absent"}}"#,
            &[],
            &["cursor theme"],
            |a| a.cursor_theme().is_none(),
        ),
    ];
    for (row, json, files, notes_contain, check) in rows {
        // "fonts" stands for a plain file in place of the fonts directory
        let dir = pack_with(json, files);
        let (assets, notes) = Assets::read_pack(dir.path()).expect(row);
        for needle in notes_contain {
            assert!(
                notes.iter().any(|n| n.contains(needle)),
                "{row}: no note with {needle:?}: {notes:?}"
            );
        }
        assert!(check(&assets), "{row}: {notes:?}");
    }
}

#[test]
fn open_accepts_only_a_pack_without_problems() {
    let complete = r#"{"version": 2, "name": "p",
        "fonts": {"ui": {"family": "F", "files": ["fonts/f.ttf"]}},
        "icons": {"document.pdf": {"svg": "pdf.svg"}}}"#;
    for (row, json, files, refused_for) in [
        ("complete", complete, &["fonts/f.ttf", "pdf.svg"][..], None),
        (
            "a named file is missing",
            complete,
            &["fonts/f.ttf"][..],
            Some("pdf.svg"),
        ),
        (
            "an unknown key",
            r#"{"version": 2, "mystery": true}"#,
            &[][..],
            Some("mystery"),
        ),
    ] {
        let dir = pack_with(json, files);
        match (Assets::open(dir.path()), refused_for) {
            (Ok(_), None) => {}
            (Err(AssetError::Manifest { reason, .. }), Some(needle)) => {
                assert!(reason.contains(needle), "{row}: {reason}")
            }
            (other, _) => panic!("{row}: got {other:?}"),
        }
    }
}

// ---- icons ----

#[test]
fn a_raster_icon_takes_the_smallest_size_that_covers_the_request_or_the_largest() {
    let dir = pack_with(
        r#"{"version": 2, "icons": {"folder": {"raster":
            {"16": "f16.png", "32": "f32.png", "64": "f64.png"}}}}"#,
        &["f16.png", "f32.png", "f64.png"],
    );
    let assets = load(&[dir.path()]);
    for (px, expected) in [(16, 16), (20, 32), (33, 64), (4096, 64)] {
        let icon = raster(&assets, "folder", px);
        assert_eq!(icon.px, expected, "request {px}");
        assert_eq!(
            icon.path,
            dir.path().join(format!("f{expected}.png")),
            "request {px}"
        );
    }
}

#[test]
fn icon_sources_come_in_trial_order_raster_svg_symbol_theme_without_missing_files() {
    let dir = pack_with(
        r#"{"version": 2, "icon_theme": {"name": "Adwaita"}, "icons": {
            "symbol.wifi": {"theme": ["network-wireless-symbolic"], "symbol": "s.svg", "svg": "i.svg",
                            "raster": {"32": "r.png"}},
            "document.pdf": {"svg": "gone.svg", "theme": ["x-office-document"]}}}"#,
        &["s.svg", "i.svg", "r.png"],
    );
    let assets = load(&[dir.path()]);
    let from_pack = |icon: &str| -> Vec<IconSource> {
        assets
            .icon_sources(&name(icon), 32)
            .into_iter()
            .filter(|(p, _)| *p == dir.path())
            .map(|(_, s)| s)
            .collect()
    };
    let theme = |n: &str| IconSource::Theme {
        names: vec![n.to_string()],
        theme: Some("Adwaita".to_string()),
    };
    let wifi = from_pack("symbol.wifi");
    assert!(matches!(wifi[0], IconSource::Raster(_)));
    assert_eq!(
        wifi[1..],
        [
            IconSource::Svg(dir.path().join("i.svg")),
            IconSource::Symbol(dir.path().join("s.svg")),
            theme("network-wireless-symbolic"),
        ]
    );
    assert_eq!(
        from_pack("document.pdf"),
        vec![theme("x-office-document")],
        "a missing svg is left out"
    );
}

#[test]
fn packs_are_tried_in_order_and_each_serves_only_the_names_it_lists() {
    let a = pack_with(
        r#"{"version": 2, "icons": {"document.pdf": {"svg": "a.svg"}, "folder": {"svg": "a.svg"}}}"#,
        &["a.svg"],
    );
    let b = pack_with(
        r#"{"version": 2, "icons": {"document.pdf": {"svg": "b.svg"}, "computer": {"svg": "b.svg"}}}"#,
        &["b.svg"],
    );
    let assets = load(&[a.path(), b.path()]);
    let pdf = assets.icon_sources(&name("document.pdf"), 32);
    assert_eq!(pdf[0], (a.path(), IconSource::Svg(a.path().join("a.svg"))));
    assert_eq!(pdf[1], (b.path(), IconSource::Svg(b.path().join("b.svg"))));
    let computer = assets.icon_sources(&name("computer"), 32);
    assert_eq!(computer[0].0, b.path());
    assert!(computer.iter().all(|(p, _)| *p != a.path()));
}

#[test]
fn the_built_in_icon_map_ends_the_list_and_an_unknown_name_has_no_source() {
    let assets = Assets::builtin();
    let sources = assets.icon_sources(&name("folder"), 32);
    let (pack, source) = sources.last().expect("the built-in map names folder");
    assert_eq!(*pack, Path::new(""));
    assert!(matches!(source, IconSource::Theme { names, theme: None } if !names.is_empty()));
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
    assert_eq!(
        assets.icon_sources(&name("no.such.icon"), 32),
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
fn the_application_icon_theme_is_the_first_one_that_asks_for_applications() {
    let a = pack(r#"{"version": 2, "icon_theme": {"name": "Plain"}}"#);
    let b = pack(r#"{"version": 2, "icon_theme": {"name": "Apps", "applications": true}}"#);
    assert_eq!(load(&[a.path()]).application_icon_theme(), None);
    assert_eq!(
        load(&[a.path(), b.path()]).application_icon_theme(),
        Some("Apps")
    );
}

#[test]
fn icon_names_are_dot_separated_lowercase_segments() {
    for ok in ["folder", "symbol.chevron.up.down", "a-b.c1"] {
        assert_eq!(IconName::new(ok).unwrap().as_str(), ok);
    }
    for bad in ["", "Folder", "a..b", ".a", "a.", "a b", "a_b", "a/b"] {
        assert!(IconName::new(bad).is_err(), "{bad:?} should be invalid");
    }
}

#[test]
fn the_logical_names_the_shell_looks_up_are_valid_and_unique() {
    use cupertino_assets::{RASTER_NAMES, SYMBOL_NAMES};
    let icons: Vec<&str> = RASTER_NAMES.iter().chain(&SYMBOL_NAMES).copied().collect();
    for n in &icons {
        assert!(IconName::new(n).is_ok(), "{n}");
    }
    let mut sorted = icons.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), icons.len());
    assert!(SYMBOL_NAMES.iter().all(|n| n.starts_with("symbol.")));
}

// ---- version 1 ----

#[test]
fn a_version_one_manifest_is_read_as_the_version_two_entries_it_means() {
    let dir = pack_with(
        r#"{"version": 1, "fonts": {"ui": "Old Family"},
            "icons": {"folder": {"raster": [32]}, "symbol.wifi": {"symbol": true}},
            "wallpaper": "wall.jpg", "cursors": "legacy"}"#,
        &[
            "icons/folder/32.png",
            "icons/symbol.wifi/symbol.svg",
            "wall.jpg",
            "cursors/legacy/index.theme",
            "fonts/b.otf",
            "fonts/a.ttf",
            "fonts/C.TTF",
            "fonts/readme.txt",
        ],
    );
    let assets = Assets::open(dir.path()).unwrap();
    assert_eq!(
        raster(&assets, "folder", 32).path,
        dir.path().join("icons/folder/32.png")
    );
    assert_eq!(
        assets.icon_sources(&name("symbol.wifi"), 16).remove(0).1,
        IconSource::Symbol(dir.path().join("icons/symbol.wifi/symbol.svg"))
    );
    assert_eq!(assets.font_family(FontRole::Ui), Some("Old Family"));
    assert_eq!(
        assets.wallpaper_path(false),
        Some(dir.path().join("wall.jpg"))
    );
    assert_eq!(
        assets.cursor_theme(),
        Some(CursorTheme {
            name: "legacy".to_string(),
            search_dir: dir.path().join("cursors"),
        })
    );
    // every font file of fonts/, sorted
    let fonts: Vec<_> = ["C.TTF", "a.ttf", "b.otf"]
        .iter()
        .map(|f| FontSource::File(dir.path().join("fonts").join(f)))
        .chain([FontSource::Builtin])
        .collect();
    assert_eq!(assets.fonts(), fonts);
}

// ---- fonts ----

#[test]
fn a_font_role_resolves_to_the_family_of_the_first_pack_that_assigns_it() {
    let a = pack(r#"{"version": 2, "fonts": {"ui": {"family": "A Sans"}}}"#);
    let b = pack(
        r#"{"version": 2, "fonts": {"ui": {"family": "B Sans"}, "mono": {"family": "B Mono"}}}"#,
    );
    let assets = load(&[a.path(), b.path()]);
    for (role, family) in [
        (FontRole::Ui, Some("A Sans")),
        (FontRole::Mono, Some("B Mono")),
        (FontRole::UiRounded, None),
        (FontRole::UiCompact, None),
        (FontRole::Serif, None),
    ] {
        assert_eq!(assets.font_family(role), family, "{role:?}");
    }
    assert_eq!(Assets::builtin().font_family(FontRole::Ui), None);
}

#[test]
fn fonts_list_each_packs_existing_files_once_in_lookup_order_then_the_built_in_font() {
    let a = pack_with(
        r#"{"version": 2,
            "fonts": {"ui": {"family": "U", "files": ["u.ttf", "shared.ttf", "gone.ttf"]},
                      "mono": {"family": "M", "files": ["m.ttf", "shared.ttf"]}},
            "fallback_fonts": ["fallback.ttf", "u.ttf"]}"#,
        &["u.ttf", "shared.ttf", "m.ttf", "fallback.ttf"],
    );
    let b = pack_with(
        r#"{"version": 2, "fonts": {"ui": {"family": "B", "files": ["b.ttf"]}}}"#,
        &["b.ttf"],
    );
    let expected: Vec<_> = ["u.ttf", "shared.ttf", "m.ttf", "fallback.ttf"]
        .iter()
        .map(|f| FontSource::File(a.path().join(f)))
        .chain([
            FontSource::File(b.path().join("b.ttf")),
            FontSource::Builtin,
        ])
        .collect();
    assert_eq!(load(&[a.path(), b.path()]).fonts(), expected);
    assert_eq!(Assets::builtin().fonts(), vec![FontSource::Builtin]);
}

// ---- cursors ----

#[test]
fn the_cursor_theme_is_the_first_one_found_in_its_packs_search_directories() {
    let elsewhere = tempfile::tempdir().unwrap();
    write(elsewhere.path(), "mine/index.theme", "");
    let absent = pack(r#"{"version": 2, "cursors": {"theme": "absent"}}"#);
    let relative = pack_with(
        r#"{"version": 2, "cursors": {"theme": "mine", "path": ["cursors"]}}"#,
        &["cursors/mine/index.theme"],
    );
    let absolute = pack(&format!(
        r#"{{"version": 2, "cursors": {{"theme": "mine", "path": ["nothing-here", {:?}]}}}}"#,
        elsewhere.path()
    ));
    let default_dir = pack_with(
        r#"{"version": 2, "cursors": {"theme": "present"}}"#,
        &["cursors/present/index.theme"],
    );
    let mine = |dir: &Path| CursorTheme {
        name: "mine".to_string(),
        search_dir: dir.to_path_buf(),
    };
    for (row, packs, expected) in [
        (
            "a pack-relative directory",
            vec![relative.path()],
            Some(mine(&relative.path().join("cursors"))),
        ),
        (
            "directories in order, absolute as given",
            vec![absolute.path()],
            Some(mine(elsewhere.path())),
        ),
        (
            "a later pack when the first one's theme is absent",
            vec![absent.path(), default_dir.path()],
            Some(CursorTheme {
                name: "present".to_string(),
                search_dir: default_dir.path().join("cursors"),
            }),
        ),
        ("none found", vec![absent.path()], None),
    ] {
        assert_eq!(load(&packs).cursor_theme(), expected, "{row}");
    }
}

// ---- wallpaper ----

#[test]
fn the_default_wallpaper_is_the_first_pack_picture_that_exists() {
    let both = pack_with(
        r#"{"version": 2, "wallpapers": {"default": {"light": "d.jpg", "dark": "n.jpg"}}}"#,
        &["d.jpg", "n.jpg"],
    );
    let light_only = pack_with(
        r#"{"version": 2, "wallpapers": {"default": {"light": "d.jpg"}}}"#,
        &["d.jpg"],
    );
    let gone = pack(r#"{"version": 2, "wallpapers": {"default": {"light": "gone.jpg"}}}"#);
    for (row, packs, dark, expected) in [
        (
            "light",
            vec![both.path()],
            false,
            Some(both.path().join("d.jpg")),
        ),
        (
            "dark",
            vec![both.path()],
            true,
            Some(both.path().join("n.jpg")),
        ),
        (
            "dark falls back to light",
            vec![light_only.path()],
            true,
            Some(light_only.path().join("d.jpg")),
        ),
        (
            "a missing file falls through",
            vec![gone.path(), light_only.path()],
            false,
            Some(light_only.path().join("d.jpg")),
        ),
        ("no picture", vec![gone.path()], false, None),
        ("the built-in fallback", vec![], false, None),
    ] {
        assert_eq!(load(&packs).wallpaper_path(dark), expected, "{row}");
    }
}

#[test]
fn wallpaper_files_pair_dark_pictures_and_list_each_name_once() {
    let a = pack_with(
        r#"{"version": 2}"#,
        &[
            "wallpapers/sunrise.jpg",
            "wallpapers/sunrise-dark.jpg",
            "wallpapers/plain.png",
            "wallpapers/.hidden.png",
            "wallpapers/notes.txt",
        ],
    );
    let b = pack_with(
        r#"{"version": 2}"#,
        &["wallpapers/plain.png", "wallpapers/other.jpg"],
    );
    let files = load(&[a.path(), b.path()]).wallpaper_files();
    let listed: Vec<_> = files
        .iter()
        .map(|f| (f.name.as_str(), f.light.clone(), f.dark.clone()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("plain", a.path().join("wallpapers/plain.png"), None),
            (
                "sunrise",
                a.path().join("wallpapers/sunrise.jpg"),
                Some(a.path().join("wallpapers/sunrise-dark.jpg"))
            ),
            ("other", b.path().join("wallpapers/other.jpg"), None),
        ]
    );

    let dir = tempfile::tempdir().unwrap();
    assert!(
        pictures_in(&dir.path().join("nowhere")).is_empty(),
        "an unreadable directory"
    );
    write(dir.path(), "lone-dark.png", "");
    let lone = pictures_in(dir.path());
    assert_eq!(lone.len(), 1);
    assert_eq!(
        lone[0].name, "lone-dark",
        "a dark picture without its light one stands alone"
    );
}

#[test]
fn wallpaper_credits_are_read_by_file_name() {
    let text = "wallpapers/a.jpg\n  \"Title A\" by Someone (https://x.test)\nwallpapers/b.jpg\n\t\"Title B\" by Other\nelse\n  \"Ignored\" by Nobody\n";
    let credits = parse_credits(text);
    assert_eq!(credits.len(), 2);
    assert_eq!(
        (
            credits["a.jpg"].title.as_str(),
            credits["a.jpg"].author.as_str()
        ),
        ("Title A", "Someone")
    );
    assert_eq!(
        (
            credits["b.jpg"].title.as_str(),
            credits["b.jpg"].author.as_str()
        ),
        ("Title B", "Other")
    );

    let dir = pack_with(r#"{"version": 2}"#, &["wallpapers/sunrise.jpg"]);
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

// ---- pack listing and the report ----

#[test]
fn roots_and_credits_list_each_pack_in_lookup_order() {
    let a = pack(r#"{"version": 2, "name": "alpha", "credits": "CREDITS.md"}"#);
    let b = pack(r#"{"version": 2}"#);
    let assets = load(&[a.path(), b.path()]);
    assert_eq!(assets.roots().collect::<Vec<_>>(), vec![a.path(), b.path()]);
    assert_eq!(
        assets.credits().collect::<Vec<_>>(),
        vec![
            (a.path(), Some("alpha"), Some(a.path().join("CREDITS.md"))),
            (b.path(), None, None)
        ]
    );
}

#[test]
fn the_pack_report_names_what_provides_each_name_and_what_falls_back() {
    let dir = pack_with(
        r#"{"version": 2, "name": "p", "fonts": {"ui": {"family": "Fam"}},
            "icons": {"folder": {"svg": "f.svg"}}}"#,
        &["f.svg"],
    );
    let (assets, notes) = Assets::read_pack(dir.path()).unwrap();
    let report = PackReport::new(&assets, &notes);
    assert_eq!(report.fonts[0].provider, "Fam");
    assert!(report
        .icons
        .iter()
        .any(|i| i.name == "folder" && i.provider.starts_with("svg ")));
    for fallback in ["cursor theme", "wallpaper light"] {
        assert!(
            report.fallbacks.contains(&fallback.to_string()),
            "{fallback}"
        );
    }
    assert!(!report.fallbacks.contains(&"font ui".to_string()));
    let text = report.to_string();
    assert!(
        text.contains("problems: 0") && text.contains("falls back:"),
        "{text}"
    );

    let built_in = PackReport::new(&Assets::builtin(), &[]);
    assert!(built_in.fallbacks.contains(&"font ui".to_string()));
    assert!(built_in.fonts[0].provider.contains(BUILTIN_UI_FAMILY));
}
