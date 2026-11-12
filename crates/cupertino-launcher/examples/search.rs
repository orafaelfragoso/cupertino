//! Command line front end: runs a query against the real desktop entries (and optionally the file
//! index of `$HOME`) and prints the sections. Nothing is launched or persisted.
//!
//! `cargo run -p cupertino-launcher --release --example search -- [--files] <query>`

use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::frecency::{unix_now, Frecency};
use cupertino_launcher::{Launcher, LauncherConfig};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let files = args.first().is_some_and(|a| a == "--files");
    if files {
        args.remove(0);
    }
    let query = args.join(" ");
    let t = Instant::now();
    let mut launcher = Launcher::new(
        LauncherConfig::default(),
        cupertino_launcher::desktop::index_applications(
            &cupertino_launcher::desktop::xdg_application_dirs(),
            &cupertino_launcher::desktop::Locale::from_env(),
            &cupertino_launcher::desktop::Visibility::from_env(),
        ),
        Frecency::in_memory(),
    );
    println!(
        "indexed {} applications in {:.1} ms",
        launcher.entries().len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    if files {
        if let Some(cfg) = FileSearchConfig::home() {
            let t = Instant::now();
            let idx = FileIndex::build(cfg);
            println!(
                "indexed {} files ({} directories{}) in {:.0} ms",
                idx.len(),
                idx.dirs_read,
                if idx.truncated { ", truncated" } else { "" },
                t.elapsed().as_secs_f64() * 1e3
            );
            launcher.set_file_index(Some(Arc::new(idx)));
        }
    }
    let t = Instant::now();
    let results = launcher.query(&query, unix_now());
    println!(
        "query {query:?}: {} results in {:.2} ms",
        results.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    for s in &results.sections {
        println!("\n{}", s.title);
        for i in &s.items {
            println!(
                "  {:<40} {:<40} {:>7.0}",
                i.title,
                i.subtitle,
                i.score.min(99999.0)
            );
        }
    }
}
