//! Renders sample views into `output/`.
//!
//! `cargo run -p cupertino-files --release --example files_samples`
//!
//! Needs `CUPERTINO_ASSET_DIR` (the fonts come from the asset directory). The theme data is used
//! for colours when `CUPERTINO_SPEC_DIR` (or the theme crate's default `data/theme`) resolves;
//! otherwise the built-in palette is used.

use cupertino_files::entry::Entry;
use cupertino_files::group::{group_entries, GroupBy};
use cupertino_files::layout::list::RowSpec;
use cupertino_files::render::{Palette, Renderer};
use cupertino_files::settings::{IconViewSettings, ListViewSettings};
use cupertino_files::sort::{sort_entries, SortSpec};
use cupertino_files::Size;
use cupertino_theme::{Appearance, Theme};
use std::time::{Duration, SystemTime};

fn sample() -> Vec<Entry> {
    let now = SystemTime::now();
    let items: [(&str, bool, u64, u64); 16] = [
        ("Applications", true, 0, 2),
        ("Desktop", true, 0, 40),
        ("Documents", true, 0, 3),
        ("Downloads", true, 0, 1),
        ("IMG_0042.png", false, 3_400_000, 5),
        ("IMG_0100.png", false, 2_800_000, 5),
        ("IMG_0099.png", false, 2_700_000, 5),
        ("Résumé 2026.pdf", false, 184_000, 12),
        ("Résumé 2025.pdf", false, 171_000, 400),
        ("notes.txt", false, 2_100, 0),
        (
            "Quarterly report with a very long name that must wrap and truncate in the middle.xlsx",
            false,
            912_000,
            30,
        ),
        ("holiday-video.mp4", false, 840_000_000, 90),
        ("song.mp3", false, 5_400_000, 20),
        ("archive.tar.gz", false, 48_000_000, 7),
        ("main.rs", false, 4_200, 0),
        ("Screenshot 2026-10-22 at 09.41.12.png", false, 1_200_000, 0),
    ];
    let mut v: Vec<Entry> = items
        .iter()
        .map(|(n, d, s, days)| {
            let mut e = Entry::synthetic(n, *d, *s);
            e.modified = Some(now - Duration::from_secs(days * 86_400 + 3_600));
            e
        })
        .collect();
    sort_entries(&mut v, &SortSpec::default());
    v
}

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("output");
    std::fs::create_dir_all(&out).unwrap();
    let theme = Theme::load().ok();
    let entries = sample();
    let now = SystemTime::now();
    let assets = cupertino_assets::Assets::discover();
    let dirs = cupertino_apps::XdgDirs::from_env().unwrap_or_else(|e| panic!("{e}"));
    let database =
        cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).unwrap_or_else(|e| panic!("{e}"));
    let icons = cupertino_icons::document::FileIcons::new(
        &assets,
        database,
        std::collections::HashMap::new(),
    );
    let mut r = Renderer::new(icons, &assets).unwrap_or_else(|e| panic!("{e}"));
    for (dark, name) in [(false, "light"), (true, "dark")] {
        let appearance = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let pal = match &theme {
            Some(t) => Palette::from_theme(t, appearance),
            None => Palette::builtin(dark),
        };
        let icons = IconViewSettings::default();
        let pm = r.icon_view(
            &entries,
            None,
            &[4, 9],
            true,
            &icons,
            Size { w: 720.0, h: 520.0 },
            0.0,
            2.0,
            &pal,
        );
        pm.save_png(out.join(format!("icon-view-{name}.png")))
            .unwrap();
        let groups = group_entries(&entries, GroupBy::Kind, now);
        let pm = r.icon_view(
            &entries,
            Some(&groups),
            &[4],
            true,
            &IconViewSettings {
                icon_size: 48,
                ..icons.clone()
            },
            Size { w: 720.0, h: 560.0 },
            0.0,
            2.0,
            &pal,
        );
        pm.save_png(out.join(format!("icon-view-grouped-{name}.png")))
            .unwrap();
        let pm = r.icon_view(
            &entries,
            None,
            &[4],
            true,
            &IconViewSettings {
                icon_size: 32,
                label_on_bottom: false,
                ..icons.clone()
            },
            Size { w: 720.0, h: 420.0 },
            0.0,
            2.0,
            &pal,
        );
        pm.save_png(out.join(format!("icon-view-label-right-{name}.png")))
            .unwrap();
        let outline: Vec<RowSpec> = entries
            .iter()
            .map(|e| RowSpec {
                level: 0,
                expandable: e.is_dir,
                expanded: false,
            })
            .collect();
        let pm = r.list_view(
            &entries,
            &outline,
            &[2, 3],
            true,
            &ListViewSettings::default(),
            SortSpec::default(),
            Size { w: 760.0, h: 400.0 },
            0.0,
            2.0,
            &pal,
            now,
        );
        pm.save_png(out.join(format!("list-view-{name}.png")))
            .unwrap();
    }
    println!("wrote {}", out.display());
}
