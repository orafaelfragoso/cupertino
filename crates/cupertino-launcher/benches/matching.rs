//! Dependency-free benchmark of query matching over 5 000 synthetic applications.
//!
//! `cargo bench -p cupertino-launcher`

use cupertino_launcher::matcher::{self, Candidate, CandidateField, Query, Weights};
use cupertino_launcher::{DesktopEntry, Frecency, Launcher, LauncherConfig};
use std::hint::black_box;
use std::time::Instant;

const WORDS: [&str; 48] = [
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo", "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango", "Uniform", "Victor",
    "Whiskey", "Xray", "Editor", "Viewer", "Player", "Manager", "Studio", "Console", "Monitor", "Designer", "Builder", "Reader", "Writer", "Painter", "Notes", "Mail", "Chat", "Browser", "Terminal", "Settings",
    "Files", "Calendar", "Photos", "Music", "Video", "Maps",
];

fn synthetic(n: usize) -> Vec<DesktopEntry> {
    let mut seed = 0x2545F491u64;
    let mut next = |m: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % m
    };
    (0..n)
        .map(|i| {
            let words = 1 + next(3);
            let name = (0..words).map(|_| WORDS[next(WORDS.len())]).collect::<Vec<_>>().join(" ");
            DesktopEntry {
                id: format!("app{i}.desktop"),
                name: format!("{name} {i}"),
                generic_name: WORDS[next(WORDS.len())].to_string(),
                comment: format!("{} {} utility", WORDS[next(WORDS.len())], WORDS[next(WORDS.len())]),
                keywords: vec![WORDS[next(WORDS.len())].to_lowercase()],
                categories: vec![],
                icon: None,
                exec: format!("app{i} %U"),
                try_exec: None,
                terminal: false,
                path: None,
                startup_wm_class: None,
                actions: vec![],
                source: None,
            }
        })
        .collect()
}

fn bench<F: FnMut() -> usize>(name: &str, iters: usize, mut f: F) {
    let t0 = Instant::now();
    let mut acc = 0;
    for _ in 0..iters {
        acc += f();
    }
    black_box(acc);
    let us = t0.elapsed().as_secs_f64() * 1e6 / iters as f64;
    println!("{name:<56} {us:>10.1} us/op");
}

fn main() {
    let entries = synthetic(5000);
    let t0 = Instant::now();
    let mut frecency = Frecency::in_memory();
    for i in (0..5000).step_by(25) {
        frecency.record(&format!("app:app{i}.desktop"), "", 1.0e6 - i as f64);
    }
    let launcher = Launcher::new(LauncherConfig::default(), entries.clone(), frecency);
    println!("index 5000 entries (fields, word starts): {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);

    for q in ["a", "ed", "mail", "terminal", "stu", "alpha bravo", "ab", "vcn", "libre wr", "zzzzq", "viewer 42"] {
        bench(&format!("launcher.query({q:?}) over 5000 apps"), 100, || launcher.query(q, 1.0e6).len());
    }
    bench("calculator 12*(3+4)^2/7", 100_000, || cupertino_launcher::calc::evaluate("12*(3+4)^2/7").is_some() as usize);

    let cands: Vec<Candidate> = entries.iter().map(|e| Candidate { fields: vec![CandidateField::new(&e.name, 1.0, true)], required_field: None }).collect();
    let w = Weights::default();
    for q in ["ed", "stu", "vcn"] {
        let query = Query::new(q);
        bench(&format!("matcher::score only, name field, {q:?}"), 100, || cands.iter().filter(|c| matcher::score(&query, c, &w).is_some()).count());
    }
}
