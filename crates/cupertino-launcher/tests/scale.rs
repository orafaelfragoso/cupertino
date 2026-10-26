use cupertino_launcher::{DesktopEntry, Frecency, Launcher, LauncherConfig};
use std::time::Instant;

const WORDS: [&str; 60] = [
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo", "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango",
    "Uniform", "Victor", "Whiskey", "Xray", "Yankee", "Zulu", "Editor", "Viewer", "Player", "Manager", "Studio", "Console", "Monitor", "Designer", "Builder", "Reader", "Writer", "Painter", "Mixer", "Planner",
    "Notes", "Mail", "Chat", "Browser", "Terminal", "Settings", "Files", "Calendar", "Photos", "Music", "Video", "Radio", "Maps", "Clock", "Weather", "Books", "Archive", "Backup", "Sync", "Vault",
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
            let name: Vec<&str> = (0..words).map(|_| WORDS[next(WORDS.len())]).collect();
            let name = format!("{} {i}", name.join(" "));
            DesktopEntry {
                id: format!("app{i}.desktop"),
                name,
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

#[test]
fn five_thousand_entries_answer_quickly_and_sensibly() {
    let mut entries = synthetic(5000);
    entries[1234].name = "Firefox".into();
    entries[1234].id = "firefox.desktop".into();
    entries[1234].exec = "firefox %u".into();
    entries[77].name = "Visual Studio Code".into();
    let l = Launcher::new(LauncherConfig::default(), entries, Frecency::in_memory());
    let t = Instant::now();
    let mut total = 0;
    for q in ["fire", "vsc", "term", "mail", "libre wr", "zzzq", "e", "alpha bravo", "stu", "qxz"] {
        total += l.query(q, 1.0e6).len();
    }
    let per_query = t.elapsed().as_secs_f64() * 1000.0 / 10.0;
    assert!(total > 0);
    assert!(per_query < 250.0, "{per_query:.1} ms per query (debug build)");
    assert_eq!(l.query("fire", 1.0e6).get(0).unwrap().title, "Firefox");
    assert_eq!(l.query("vsc", 1.0e6).get(0).unwrap().title, "Visual Studio Code");
}
