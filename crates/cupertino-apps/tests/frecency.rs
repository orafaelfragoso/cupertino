//! The launcher's usage store: reading its file and ranking its items.

mod common;

use common::write;
use cupertino_apps::frecency::{
    decayed, ranked, ranked_apps, store_path, ItemRecord, Store, FILE_VERSION, HALF_LIFE_SECS,
    STORE_NAME,
};
use cupertino_apps::recent::MAX_READ_BYTES;
use std::collections::HashMap;
use std::path::Path;

fn record(score: f64, last_used: f64) -> ItemRecord {
    ItemRecord {
        score,
        last_used,
        launches: 1,
    }
}

fn store_with(items: &[(&str, ItemRecord)]) -> Store {
    let mut store = Store::empty();
    store.items = items
        .iter()
        .map(|(k, r)| (k.to_string(), r.clone()))
        .collect();
    store
}

#[test]
fn the_store_is_a_file_in_the_data_home() {
    assert_eq!(
        store_path(Path::new("/data")),
        Path::new("/data").join(STORE_NAME)
    );
    assert_eq!(STORE_NAME, "cupertino/launcher.json");
}

#[test]
fn an_empty_store_has_the_current_version_and_nothing_recorded() {
    let store = Store::empty();
    assert_eq!(store.version, FILE_VERSION);
    assert!(store.items.is_empty() && store.queries.is_empty());
}

#[test]
fn a_store_file_is_read_with_its_items_and_queries() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "launcher.json",
        r#"{"version":1,
            "items":{"app:a.desktop":{"score":2.5,"last_used":100.0,"launches":3}},
            "queries":{"ed":{"app:a.desktop":2}}}"#,
    );
    let store = Store::read(&path);
    assert_eq!(store.items["app:a.desktop"], record_launches(2.5, 100.0, 3));
    assert_eq!(store.queries["ed"]["app:a.desktop"], 2);
}

fn record_launches(score: f64, last_used: f64, launches: u32) -> ItemRecord {
    ItemRecord {
        score,
        last_used,
        launches,
    }
}

#[test]
fn a_missing_store_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::read(&dir.path().join("absent.json"));
    assert!(store.items.is_empty());
    assert_eq!(store.version, FILE_VERSION);
}

#[test]
fn a_corrupt_store_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    for text in ["", "{", "not json", "[1,2]", r#"{"version":1,"items":3}"#] {
        let path = write(dir.path(), "launcher.json", text);
        let store = Store::read(&path);
        assert!(store.items.is_empty(), "{text:?}");
        assert_eq!(store.version, FILE_VERSION);
    }
}

#[test]
fn a_store_of_another_version_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let item = r#""items":{"app:a":{"score":1.0,"last_used":1.0,"launches":1}}"#;
    for version in ["0", "2", "99"] {
        let path = write(
            dir.path(),
            "launcher.json",
            &format!("{{\"version\":{version},{item}}}"),
        );
        assert!(Store::read(&path).items.is_empty(), "version {version}");
    }
    let unversioned = write(dir.path(), "launcher.json", &format!("{{{item}}}"));
    assert!(Store::read(&unversioned).items.is_empty());
}

#[test]
fn an_oversized_store_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("launcher.json");
    let mut text = String::from(r#"{"version":1,"items":{},"queries":{}}"#);
    text.push_str(&" ".repeat(MAX_READ_BYTES as usize));
    std::fs::write(&path, &text).unwrap();
    assert_eq!(Store::read(&path).version, FILE_VERSION);
    assert!(Store::read(&path).items.is_empty());
}

#[test]
fn a_launch_loses_half_its_weight_per_half_life() {
    let rec = record(4.0, 1000.0);
    assert_eq!(decayed(&rec, 1000.0, 10.0), 4.0);
    assert!((decayed(&rec, 1010.0, 10.0) - 2.0).abs() < 1e-12);
    assert!((decayed(&rec, 1020.0, 10.0) - 1.0).abs() < 1e-12);
}

#[test]
fn a_time_before_the_last_use_does_not_raise_the_score() {
    assert_eq!(decayed(&record(4.0, 1000.0), 0.0, 10.0), 4.0);
}

#[test]
fn the_default_half_life_is_two_weeks() {
    assert_eq!(HALF_LIFE_SECS, 14.0 * 86400.0);
}

#[test]
fn items_rank_by_decayed_score_highest_first() {
    let items: HashMap<String, ItemRecord> = [
        ("old".to_string(), record(8.0, 0.0)),
        ("new".to_string(), record(3.0, 100.0)),
        ("mid".to_string(), record(5.0, 50.0)),
    ]
    .into();
    // after one half-life of 100 seconds: old 8*2^-1.. at now=100: old 4.0, mid 5*2^-0.5=3.54, new 3.0
    let order: Vec<String> = ranked(&items, 100.0, 100.0)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(order, ["old", "mid", "new"]);
    // much later the order follows recency instead
    let later: Vec<String> = ranked(&items, 1000.0, 10.0)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(later, ["new", "mid", "old"]);
}

#[test]
fn equal_scores_rank_by_key() {
    let items: HashMap<String, ItemRecord> = [
        ("b".to_string(), record(1.0, 0.0)),
        ("a".to_string(), record(1.0, 0.0)),
        ("c".to_string(), record(1.0, 0.0)),
    ]
    .into();
    let order: Vec<String> = ranked(&items, 0.0, 10.0)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(order, ["a", "b", "c"]);
}

#[test]
fn ranking_nothing_gives_nothing() {
    assert!(ranked(&HashMap::new(), 0.0, 10.0).is_empty());
    assert!(ranked_apps(&Store::empty(), 0.0).is_empty());
}

#[test]
fn applications_are_the_app_keys_without_their_actions_in_ranked_order() {
    let store = store_with(&[
        ("app:low.desktop", record(1.0, 500.0)),
        ("app:high.desktop", record(9.0, 400.0)),
        ("app:high.desktop#new-window", record(20.0, 450.0)),
        ("file:/tmp/a", record(30.0, 450.0)),
        ("cmd:ls", record(40.0, 450.0)),
    ]);
    let apps = ranked_apps(&store, 500.0);
    let ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["high.desktop", "low.desktop"]);
    assert_eq!(apps[0].last_used, 400.0);
    assert_eq!(apps[1].last_used, 500.0);
}
