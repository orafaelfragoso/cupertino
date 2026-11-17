//! Instruction counts of the launcher's matching over 5 000 synthetic applications: building the index, a query typed
//! as a person types it (one letter, a word start, a whole word, two words, initials, nothing that matches), and the
//! scorer alone over every name.
//!
//! The wall-clock bench beside it (`cargo bench -p cupertino-launcher
//! --bench matching`) times the same queries.

use cupertino_launcher::matcher::{self, Candidate, CandidateField, Query, Weights};
use cupertino_launcher::{DesktopEntry, Frecency, Launcher, LauncherConfig};
use gungraun::prelude::*;
use std::hint::black_box;

const APPS: usize = 5000;
/// The time the queries are made at, in the frecency's seconds.
const NOW: f64 = 1.0e6;

const WORDS: [&str; 48] = [
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet",
    "Kilo", "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango",
    "Uniform", "Victor", "Whiskey", "Xray", "Editor", "Viewer", "Player", "Manager", "Studio",
    "Console", "Monitor", "Designer", "Builder", "Reader", "Writer", "Painter", "Notes", "Mail",
    "Chat", "Browser", "Terminal", "Settings", "Files", "Calendar", "Photos", "Music", "Video",
    "Maps",
];

/// `n` applications of one to three words each, from a fixed seed.
fn synthetic(n: usize) -> Vec<DesktopEntry> {
    let mut seed = 0x2545F491u64;
    let mut next = |m: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % m
    };
    (0..n)
        .map(|i| {
            let words = 1 + next(3);
            let name = (0..words)
                .map(|_| WORDS[next(WORDS.len())])
                .collect::<Vec<_>>()
                .join(" ");
            DesktopEntry {
                id: format!("app{i}.desktop"),
                name: format!("{name} {i}"),
                generic_name: WORDS[next(WORDS.len())].to_string(),
                comment: format!(
                    "{} {} utility",
                    WORDS[next(WORDS.len())],
                    WORDS[next(WORDS.len())]
                ),
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

/// Every 25th application launched before, the earlier ones longer ago.
fn frecency() -> Frecency {
    let mut frecency = Frecency::in_memory();
    for i in (0..APPS).step_by(25) {
        frecency.record(&format!("app:app{i}.desktop"), "", NOW - i as f64);
    }
    frecency
}

fn entries() -> (Vec<DesktopEntry>, Frecency) {
    (synthetic(APPS), frecency())
}

fn launcher(query: &'static str) -> (Launcher, &'static str) {
    let (entries, frecency) = entries();
    (
        Launcher::new(LauncherConfig::default(), entries, frecency),
        query,
    )
}

fn names(query: &'static str) -> (Vec<Candidate>, Query) {
    let candidates = synthetic(APPS)
        .iter()
        .map(|e| Candidate {
            fields: vec![CandidateField::new(&e.name, 1.0, true)],
            required_field: None,
        })
        .collect();
    (candidates, Query::new(query))
}

#[library_benchmark]
#[bench::apps_5000(setup = entries)]
fn index((entries, frecency): (Vec<DesktopEntry>, Frecency)) -> Launcher {
    black_box(Launcher::new(LauncherConfig::default(), entries, frecency))
}

#[library_benchmark]
#[bench::letter(setup = launcher, args = ("a"))]
#[bench::word_start(setup = launcher, args = ("ed"))]
#[bench::word(setup = launcher, args = ("terminal"))]
#[bench::two_words(setup = launcher, args = ("alpha bravo"))]
#[bench::initials(setup = launcher, args = ("vcn"))]
#[bench::with_number(setup = launcher, args = ("viewer 42"))]
#[bench::no_match(setup = launcher, args = ("zzzzq"))]
fn query((launcher, query): (Launcher, &'static str)) -> usize {
    black_box(launcher.query(black_box(query), NOW).len())
}

#[library_benchmark]
#[bench::word_start(setup = names, args = ("ed"))]
#[bench::initials(setup = names, args = ("vcn"))]
fn score((candidates, query): (Vec<Candidate>, Query)) -> usize {
    let weights = Weights::default();
    black_box(
        candidates
            .iter()
            .filter(|c| matcher::score(&query, black_box(c), &weights).is_some())
            .count(),
    )
}

library_benchmark_group!(name = matching, benchmarks = [index, query, score]);
main!(library_benchmark_groups = matching);
