//! The launcher's usage store: reading its file and ranking its items.

mod common;

use common::write;
use cupertino_apps::frecency::{
    decayed, ranked, ranked_apps, store_path, ItemRecord, Store, FILE_VERSION, STORE_NAME,
};
use cupertino_apps::recent::MAX_READ_BYTES;
use std::collections::HashMap;

fn record(score: f64, last_used: f64) -> ItemRecord {
    ItemRecord {
        score,
        last_used,
        launches: 1,
    }
}

fn keys(items: Vec<(String, f64)>) -> Vec<String> {
    items.into_iter().map(|(k, _)| k).collect()
}

#[test]
fn the_store_file_in_the_data_home_is_read_with_its_items_and_queries() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(store_path(dir.path()), dir.path().join(STORE_NAME));
    let path = write(
        dir.path(),
        STORE_NAME,
        r#"{"version":1,
            "items":{"app:a.desktop":{"score":2.5,"last_used":100.0,"launches":3}},
            "queries":{"ed":{"app:a.desktop":2}}}"#,
    );
    let store = Store::read(&path);
    assert_eq!(
        store.items["app:a.desktop"],
        ItemRecord {
            score: 2.5,
            last_used: 100.0,
            launches: 3
        }
    );
    assert_eq!(store.queries["ed"]["app:a.desktop"], 2);
}

#[test]
fn a_missing_corrupt_oversized_or_other_version_store_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let item = r#""items":{"app:a":{"score":1.0,"last_used":1.0,"launches":1}}"#;
    let mut oversized = String::from(r#"{"version":1,"items":{},"queries":{}}"#);
    oversized.push_str(&" ".repeat(MAX_READ_BYTES as usize));
    let cases = [
        String::new(),
        "{".into(),
        "not json".into(),
        "[1,2]".into(),
        r#"{"version":1,"items":3}"#.into(),
        format!("{{\"version\":0,{item}}}"),
        format!("{{\"version\":2,{item}}}"),
        format!("{{\"version\":99,{item}}}"),
        format!("{{{item}}}"),
        oversized,
    ];
    for text in &cases {
        let path = write(dir.path(), "launcher.json", text);
        let store = Store::read(&path);
        assert!(
            store.items.is_empty() && store.queries.is_empty(),
            "{:.60?}",
            text
        );
        assert_eq!(store.version, FILE_VERSION);
    }
    let absent = Store::read(&dir.path().join("absent.json"));
    assert!(absent.items.is_empty());
    assert_eq!(absent.version, FILE_VERSION);
}

#[test]
fn a_launch_loses_half_its_weight_per_half_life_and_never_gains() {
    let rec = record(4.0, 1000.0);
    for (now, expected) in [(1000.0, 4.0), (1010.0, 2.0), (1020.0, 1.0), (0.0, 4.0)] {
        assert!(
            (decayed(&rec, now, 10.0) - expected).abs() < 1e-12,
            "now {now}"
        );
    }
}

#[test]
fn items_rank_by_decayed_score_highest_first_then_by_key() {
    let items: HashMap<String, ItemRecord> = [
        ("old".to_string(), record(8.0, 0.0)),
        ("new".to_string(), record(3.0, 100.0)),
        ("mid".to_string(), record(5.0, 50.0)),
    ]
    .into();
    // at 100 with a half-life of 100: old 4.0, mid 3.54, new 3.0; much later recency decides
    assert_eq!(keys(ranked(&items, 100.0, 100.0)), ["old", "mid", "new"]);
    assert_eq!(keys(ranked(&items, 1000.0, 10.0)), ["new", "mid", "old"]);

    let ties: HashMap<String, ItemRecord> = ["b", "a", "c"]
        .map(|k| (k.to_string(), record(1.0, 0.0)))
        .into();
    assert_eq!(keys(ranked(&ties, 0.0, 10.0)), ["a", "b", "c"]);
    assert!(ranked(&HashMap::new(), 0.0, 10.0).is_empty());
}

#[test]
fn applications_are_the_app_keys_without_their_actions_in_ranked_order() {
    let mut store = Store::empty();
    assert!(ranked_apps(&store, 0.0).is_empty());
    store.items = [
        ("app:low.desktop", record(1.0, 500.0)),
        ("app:high.desktop", record(9.0, 400.0)),
        ("app:high.desktop#new-window", record(20.0, 450.0)),
        ("file:/tmp/a", record(30.0, 450.0)),
        ("cmd:ls", record(40.0, 450.0)),
    ]
    .map(|(k, r)| (k.to_string(), r))
    .into();
    let apps = ranked_apps(&store, 500.0);
    let ids: Vec<(&str, f64)> = apps.iter().map(|a| (a.id.as_str(), a.last_used)).collect();
    assert_eq!(ids, [("high.desktop", 400.0), ("low.desktop", 500.0)]);
}
