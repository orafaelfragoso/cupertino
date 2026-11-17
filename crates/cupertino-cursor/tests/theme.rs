//! Building a theme: the files a session finds, the hotspots and sizes of the cursors, and what the builder will not
//! overwrite or accept.

use cupertino_cursor::names::{all_names, NAMES, REQUIRED_NAMES};
use cupertino_cursor::procedural::Kind;
use cupertino_cursor::theme::{procedural_cursor, MARKER_FILE};
use cupertino_cursor::{build_theme, xcursor, BuildOptions, ShadowMode};
use std::path::PathBuf;

/// A fresh output directory under the target directory.
fn output(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cursor-theme")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn small(output: PathBuf) -> BuildOptions {
    let mut o = BuildOptions::new(output);
    o.sizes = vec![24, 48];
    o
}

#[test]
fn a_theme_has_every_required_name_and_each_file_parses() {
    let mut o = small(output("complete"));
    o.inherits = Some("Adwaita".into());
    o.hyprcursor = true;
    let report = build_theme(&o).unwrap();
    let dir = report.theme_dir;

    let index = std::fs::read_to_string(dir.join("index.theme")).unwrap();
    assert!(index.contains("Name=Cupertino") && index.contains("Inherits=Adwaita"));
    assert!(dir.join(MARKER_FILE).is_file());
    for name in REQUIRED_NAMES {
        let bytes = std::fs::read(dir.join("cursors").join(name))
            .unwrap_or_else(|e| panic!("cursor {name}: {e}"));
        let images = xcursor::parse(&bytes).unwrap();
        let nominal: Vec<u32> = images.iter().map(|i| i.nominal_size).collect();
        assert!(nominal.contains(&24) && nominal.contains(&48), "{name}");
    }
    for mapping in NAMES {
        for alias in all_names(mapping) {
            assert_eq!(
                std::fs::read(dir.join("cursors").join(alias)).unwrap(),
                std::fs::read(dir.join("cursors").join(mapping.primary)).unwrap(),
                "{alias} is {}",
                mapping.primary
            );
        }
    }
    let hypr = report.hypr_dir.unwrap();
    assert!(hypr.join("manifest.hl").is_file());
    assert!(hypr.join("hyprcursors/default/meta.hl").is_file());
}

#[test]
fn cursors_keep_their_hotspot_and_frames_at_every_scale() {
    let mut o = small(output("unused"));
    o.sizes = vec![24, 48, 96];
    for kind in Kind::ALL {
        let cursor = procedural_cursor(kind, &o, Vec::new());
        assert_eq!(cursor.name, kind.name());
        let (hx, hy) = kind.hotspot();
        let mut previous_width = 0;
        for variant in &cursor.sizes {
            let scale = variant.nominal as f64 / 24.0;
            assert_eq!(variant.frames.len(), kind.frames(), "{}", kind.name());
            for frame in &variant.frames {
                let (w, h) = (frame.image.width(), frame.image.height());
                assert!(
                    frame.hotspot.0 < w && frame.hotspot.1 < h,
                    "{} hotspot inside",
                    kind.name()
                );
                assert_eq!(
                    frame.hotspot.0 as f64,
                    (hx * scale).round().min(w as f64 - 1.0)
                );
                assert_eq!(
                    frame.hotspot.1 as f64,
                    (hy * scale).round().min(h as f64 - 1.0)
                );
                assert_eq!(frame.delay_ms, kind.delay_ms());
            }
            let width = variant.frames[0].image.width();
            assert!(
                width > previous_width,
                "{} grows with the size",
                kind.name()
            );
            previous_width = width;
        }
    }
}

#[test]
fn the_shadow_mode_decides_whether_a_shadow_is_baked() {
    let mut with = small(output("unused"));
    with.sizes = vec![48];
    let mut without = with.clone();
    without.shadow = ShadowMode::Never;
    let ink = |o: &BuildOptions| -> u64 {
        let c = procedural_cursor(Kind::Default, o, Vec::new());
        c.sizes[0].frames[0]
            .image
            .pixels()
            .iter()
            .map(|p| p.alpha() as u64)
            .sum()
    };
    assert!(ink(&with) > ink(&without));
}

#[test]
fn a_rebuild_replaces_a_generated_theme_and_leaves_a_foreign_directory_alone() {
    let o = small(output("rebuild"));
    build_theme(&o).unwrap();
    build_theme(&o).expect("the marker lets the builder replace its own theme");

    let foreign = output("foreign");
    std::fs::create_dir_all(foreign.join("Cupertino")).unwrap();
    std::fs::write(foreign.join("Cupertino/mine.txt"), "keep").unwrap();
    assert!(build_theme(&small(foreign.clone())).is_err());
    assert!(foreign.join("Cupertino/mine.txt").is_file());
}

#[test]
fn options_that_cannot_make_a_theme_are_refused() {
    let out = output("refused");
    type Mutation = fn(&mut BuildOptions);
    let mutations: [(&str, Mutation); 5] = [
        ("no sizes", |o| o.sizes.clear()),
        ("size zero", |o| o.sizes = vec![0]),
        ("size over 512", |o| o.sizes = vec![513]),
        ("base size zero", |o| o.base_size = 0),
        ("name with a separator", |o| o.name = "a/b".into()),
    ];
    for (what, mutate) in mutations {
        let mut o = small(out.clone());
        mutate(&mut o);
        assert!(build_theme(&o).is_err(), "{what}");
    }
    assert!(!out.exists(), "nothing is written for refused options");
}
