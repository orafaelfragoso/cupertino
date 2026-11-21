//! The launcher's usage store, `$XDG_DATA_HOME/cupertino/launcher.json`: its file format, where it is, and its
//! ranking. Every launch adds 1 to an item's score after decaying the old value with a half-life
//! (`score·2^(−Δt/half_life)`); items rank by the decayed score. The `launcher` component (`cupertino-launcher`) writes the file;
//! other programs only read it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const FILE_VERSION: u32 = 1;

/// The file in the data home.
pub const STORE_NAME: &str = "cupertino/launcher.json";

/// Half-life of a launch's weight in seconds, the launcher's default. approximate.
pub const HALF_LIFE_SECS: f64 = 14.0 * 86400.0;

/// The key prefix of an application's entry: `app:<desktop file id>`; an action of it adds `#<action id>`.
const APP_KEY_PREFIX: &str = "app:";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub score: f64,
    pub last_used: f64,
    pub launches: u32,
}

/// The contents of the file: the items by key and the learned query associations.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub items: HashMap<String, ItemRecord>,
    #[serde(default)]
    pub queries: HashMap<String, HashMap<String, u32>>,
}

impl Store {
    /// A store with nothing recorded.
    pub fn empty() -> Self {
        Store {
            version: FILE_VERSION,
            ..Default::default()
        }
    }

    /// The store at `path`; a missing, corrupt, foreign (another version) or oversized file reads as [`Store::empty`].
    pub fn read(path: &Path) -> Self {
        crate::recent::read_capped(path, crate::recent::MAX_READ_BYTES)
            .ok()
            .and_then(|t| serde_json::from_str::<Store>(&t).ok())
            .filter(|d| d.version == FILE_VERSION)
            .unwrap_or_else(Store::empty)
    }
}

/// The store of the data home `data_home`.
pub fn store_path(data_home: &Path) -> PathBuf {
    data_home.join(STORE_NAME)
}

/// The store of the environment's data home (`$XDG_DATA_HOME`, else `~/.local/share`).
pub fn default_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(store_path(&base))
}

/// The score of `rec` decayed to `now` (Unix seconds).
pub fn decayed(rec: &ItemRecord, now: f64, half_life_secs: f64) -> f64 {
    let dt = (now - rec.last_used).max(0.0);
    rec.score * 0.5f64.powf(dt / half_life_secs)
}

/// Every item with its decayed score, highest first, equal scores by key.
pub fn ranked(
    items: &HashMap<String, ItemRecord>,
    now: f64,
    half_life_secs: f64,
) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = items
        .iter()
        .map(|(k, r)| (k.clone(), decayed(r, now, half_life_secs)))
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// An application of the store.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedApp {
    /// Desktop file id.
    pub id: String,
    /// Unix seconds of its last launch.
    pub last_used: f64,
}

/// The applications of the store (not their actions) in the launcher's order: by decayed score at `now`, highest
/// first.
pub fn ranked_apps(store: &Store, now: f64) -> Vec<RankedApp> {
    ranked(&store.items, now, HALF_LIFE_SECS)
        .into_iter()
        .filter_map(|(key, _)| {
            let id = key.strip_prefix(APP_KEY_PREFIX)?;
            (!id.contains('#')).then(|| RankedApp {
                id: id.to_owned(),
                last_used: store.items[&key].last_used,
            })
        })
        .collect()
}
