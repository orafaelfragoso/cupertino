//! Dependency-free benchmark: enumerate and sort 50 000 entries, collation keys, layouts.
//!
//! `cargo bench -p cupertino-files --bench directory`

use cupertino_files::entry::Entry;
use cupertino_files::layout::icon::{self, FontMetrics, IconParams, SectionSpec};
use cupertino_files::model::{enumerate_blocking, DirectoryModel, ModelOptions};
use cupertino_files::natural::NameKey;
use cupertino_files::sort::{sort_entries, SortKey, SortSpec};
use std::hint::black_box;
use std::time::Instant;

fn time<T>(name: &str, mut f: impl FnMut() -> T) -> T {
    let t0 = Instant::now();
    let r = f();
    println!("{name:<56} {:>10.2} ms", t0.elapsed().as_secs_f64() * 1e3);
    r
}

fn main() {
    const N: usize = 50_000;
    // synthetic names with numbers, mixed case and accents
    let names: Vec<String> = (0..N)
        .map(|i| {
            let h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let stem =
                ["IMG", "photo", "Résumé", "report", "Notes", "scan"][(h >> 60) as usize % 6];
            format!(
                "{stem} {} ({}).{}",
                (h >> 20) % 5000,
                i % 7,
                ["png", "txt", "pdf", "jpg"][(h >> 8) as usize % 4]
            )
        })
        .collect();

    let keys = time("collation keys of 50k names", || {
        names.iter().map(|n| NameKey::new(n)).collect::<Vec<_>>()
    });
    black_box(keys.len());

    let mut entries: Vec<Entry> = time("build 50k synthetic entries", || {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| Entry::synthetic(n, i % 9 == 0, (i * 37) as u64 % 1_000_000))
            .collect()
    });
    time("sort 50k by name (precomputed keys)", || {
        sort_entries(&mut entries, &SortSpec::default())
    });
    time("sort 50k by size", || {
        sort_entries(
            &mut entries,
            &SortSpec {
                key: SortKey::Size,
                ascending: false,
                folders_first: false,
            },
        )
    });
    time("sort 50k by name, folders first", || {
        sort_entries(
            &mut entries,
            &SortSpec {
                folders_first: true,
                ..SortSpec::default()
            },
        )
    });

    let p = IconParams::new(64, 54, 12, true, false, FontMetrics::estimate(12.0));
    let sec = [SectionSpec {
        title: None,
        count: N,
    }];
    let l = time("icon layout of 50k items (1440 pt wide)", || {
        icon::layout(&p, 1440.0, &sec, icon::Flow::Leading)
    });
    println!(
        "    columns {} cell {}x{} content height {:.0}",
        l.columns, l.cell.w, l.cell.h, l.content.h
    );
    time("1000 rubber-band hit queries over 50k cells", || {
        let mut n = 0;
        for i in 0..1000 {
            let r = cupertino_files::Rect::new(i as f64, i as f64 * 20.0, 300.0, 200.0);
            n += l.hits_in_rect(&r).len();
        }
        n
    });

    // real files
    let dir = std::env::temp_dir().join(format!("cupertino-files-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    time("create 50k files", || {
        for n in names.iter().enumerate().map(|(i, n)| format!("{i:05}-{n}")) {
            std::fs::write(dir.join(n), b"").unwrap();
        }
    });
    let v = time("enumerate_blocking 50k (stat + sort)", || {
        enumerate_blocking(
            &dir,
            &ModelOptions {
                watch: false,
                ..Default::default()
            },
        )
        .unwrap()
    });
    println!("    {} entries", v.len());
    time("DirectoryModel batched load 50k (no watch)", || {
        let mut m = DirectoryModel::open(
            &dir,
            ModelOptions {
                watch: false,
                ..Default::default()
            },
        )
        .unwrap();
        m.wait_loaded();
        m.len()
    });
    std::fs::remove_dir_all(&dir).unwrap();
}
