//! Frecency (frequency × recency) and learned query associations, persisted as JSON.
//!
//! Every launch adds 1 to the item's score after decaying the old value with a half-life
//! (`score·2^(−Δt/half_life)); the ranking bonus grows with the logarithm of the decayed score and
//! is capped, so frequently and recently used items win ties and near-ties but never outrank a
//! clearly better text match. A second table remembers which item was launched after which query:
//! typing a prefix of a remembered query boosts that item. All constants are authored
//! (approximate) and configurable through [`FrecencyConfig`].
//!
//! The default location is `$XDG_DATA_HOME/cupertino/launcher.json`
//! (`~/.local/share/cupertino/launcher.json`). A missing or corrupt file starts an empty store; saves
//! are atomic (temporary file and rename). The file format, its location and the ranking are
//! `cupertino_apps::frecency`, which other processes read the store with.

use cupertino_apps::frecency::{self as store, Store};
pub use cupertino_apps::frecency::{default_path, ItemRecord, FILE_VERSION};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct FrecencyConfig {
    pub half_life_secs: f64,
    pub max_bonus: f64,
    pub bonus_scale: f64,
    pub query_max_bonus: f64,
    pub query_bonus_scale: f64,
    pub max_items: usize,
    pub max_queries: usize,
}

impl Default for FrecencyConfig {
    fn default() -> Self {
        Self {
            half_life_secs: store::HALF_LIFE_SECS,
            max_bonus: 120.0,
            bonus_scale: 45.0,
            query_max_bonus: 80.0,
            query_bonus_scale: 30.0,
            max_items: 2000,
            max_queries: 500,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frecency {
    pub config: FrecencyConfig,
    data: Store,
    path: Option<PathBuf>,
    dirty: bool,
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Default for Frecency {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl Frecency {
    pub fn in_memory() -> Self {
        Self {
            config: FrecencyConfig::default(),
            data: Store::empty(),
            path: None,
            dirty: false,
        }
    }

    /// Load from `path`; the store remembers the path for [`Self::save`].
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let data = Store::read(&path);
        Self {
            config: FrecencyConfig::default(),
            data,
            path: Some(path),
            dirty: false,
        }
    }

    pub fn load_default() -> Self {
        match default_path() {
            Some(p) => Self::load(p),
            None => Self::in_memory(),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len(&self) -> usize {
        self.data.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.items.is_empty()
    }

    fn decay(&self, rec: &ItemRecord, now: f64) -> f64 {
        store::decayed(rec, now, self.config.half_life_secs)
    }

    /// Decayed score of an item.
    pub fn score(&self, key: &str, now: f64) -> f64 {
        self.data.items.get(key).map_or(0.0, |r| self.decay(r, now))
    }

    pub fn launches(&self, key: &str) -> u32 {
        self.data.items.get(key).map_or(0, |r| r.launches)
    }

    /// Ranking bonus of `key` for the (normalised) query typed so far.
    pub fn bonus(&self, key: &str, normalized_query: &str, now: f64) -> f64 {
        let c = &self.config;
        let mut b = (c.bonus_scale * self.score(key, now).ln_1p()).min(c.max_bonus);
        if !normalized_query.is_empty() {
            let n: u32 = self
                .data
                .queries
                .iter()
                .filter(|(q, _)| q.starts_with(normalized_query))
                .filter_map(|(_, m)| m.get(key))
                .sum();
            if n > 0 {
                b += (c.query_bonus_scale * (n as f64).ln_1p()).min(c.query_max_bonus);
            }
        }
        b
    }

    /// Items with the highest decayed score, best first.
    pub fn top(&self, n: usize, now: f64) -> Vec<(String, f64)> {
        let mut v = store::ranked(&self.data.items, now, self.config.half_life_secs);
        v.truncate(n);
        v
    }

    /// Record a launch of `key` after the user typed `normalized_query` (may be empty).
    pub fn record(&mut self, key: &str, normalized_query: &str, now: f64) {
        let decayed = self.score(key, now);
        let r = self.data.items.entry(key.to_string()).or_default();
        r.score = decayed + 1.0;
        r.last_used = now;
        r.launches += 1;
        let q = normalized_query.trim();
        if !q.is_empty() && q.chars().count() <= 40 {
            *self
                .data
                .queries
                .entry(q.to_string())
                .or_default()
                .entry(key.to_string())
                .or_insert(0) += 1;
        }
        self.prune(now);
        self.dirty = true;
    }

    pub fn forget(&mut self, key: &str) {
        self.data.items.remove(key);
        for m in self.data.queries.values_mut() {
            m.remove(key);
        }
        self.data.queries.retain(|_, m| !m.is_empty());
        self.dirty = true;
    }

    fn prune(&mut self, now: f64) {
        if self.data.items.len() > self.config.max_items {
            let mut v: Vec<(String, f64)> = self
                .data
                .items
                .iter()
                .map(|(k, r)| (k.clone(), self.decay(r, now)))
                .collect();
            v.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            let excess = v.len() - self.config.max_items;
            for (k, _) in v.into_iter().take(excess) {
                self.data.items.remove(&k);
            }
        }
        if self.data.queries.len() > self.config.max_queries {
            let mut v: Vec<(String, u32)> = self
                .data
                .queries
                .iter()
                .map(|(k, m)| (k.clone(), m.values().sum()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            let excess = v.len() - self.config.max_queries;
            for (k, _) in v.into_iter().take(excess) {
                self.data.queries.remove(&k);
            }
        }
    }

    /// Write to the remembered path (no-op for an in-memory store). Atomic.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&self.data).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)?;
        self.dirty = false;
        Ok(())
    }

    pub fn save_if_dirty(&mut self) -> std::io::Result<()> {
        if self.dirty {
            self.save()
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 86400.0;

    #[test]
    fn repeated_launches_raise_the_bonus_with_a_cap() {
        let mut f = Frecency::in_memory();
        assert_eq!(f.bonus("a", "", 0.0), 0.0);
        f.record("a", "", 0.0);
        let one = f.bonus("a", "", 0.0);
        for _ in 0..50 {
            f.record("a", "", 0.0);
        }
        let many = f.bonus("a", "", 0.0);
        assert!(one > 0.0 && many > one);
        assert!(many <= f.config.max_bonus + 1e-9);
        assert_eq!(f.launches("a"), 51);
    }

    #[test]
    fn score_halves_per_half_life() {
        let mut f = Frecency::in_memory();
        f.record("a", "", 0.0);
        let hl = f.config.half_life_secs;
        assert!((f.score("a", 0.0) - 1.0).abs() < 1e-12);
        assert!((f.score("a", hl) - 0.5).abs() < 1e-9);
        assert!((f.score("a", 2.0 * hl) - 0.25).abs() < 1e-9);
        f.record("a", "", hl);
        assert!((f.score("a", hl) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn recent_beats_stale_with_equal_counts() {
        let mut f = Frecency::in_memory();
        for _ in 0..3 {
            f.record("old", "", 0.0);
            f.record("new", "", 60.0 * DAY);
        }
        assert!(f.bonus("new", "", 61.0 * DAY) > f.bonus("old", "", 61.0 * DAY));
        assert_eq!(f.top(1, 61.0 * DAY)[0].0, "new");
    }

    #[test]
    fn query_association_boosts_on_prefix() {
        let mut f = Frecency::in_memory();
        f.record("browser", "ff", 0.0);
        let plain = f.bonus("browser", "", 0.0);
        let with_q = f.bonus("browser", "f", 0.0);
        let exact = f.bonus("browser", "ff", 0.0);
        let other = f.bonus("browser", "xx", 0.0);
        assert!(with_q > plain && exact > plain);
        assert_eq!(other, plain);
        assert_eq!(f.bonus("editor", "ff", 0.0), 0.0);
    }

    #[test]
    fn persistence_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/launcher.json");
        let mut f = Frecency::load(&path);
        assert!(f.is_empty());
        f.record("app:x", "x", 100.0);
        assert!(f.is_dirty());
        f.save().unwrap();
        assert!(!f.is_dirty());
        assert!(path.is_file());
        let g = Frecency::load(&path);
        assert_eq!(g.len(), 1);
        assert_eq!(g.launches("app:x"), 1);
        assert!((g.score("app:x", 100.0) - 1.0).abs() < 1e-12);
        assert!(g.bonus("app:x", "x", 100.0) > g.bonus("app:x", "", 100.0));
        assert!(!dir.path().join("nested/launcher.json.tmp").exists());
    }

    #[test]
    fn stores_are_bounded() {
        let mut f = Frecency::in_memory();
        f.config.max_items = 5;
        f.config.max_queries = 3;
        for i in 0..20 {
            f.record(&format!("k{i}"), &format!("q{i}"), i as f64);
        }
        assert_eq!(f.len(), 5);
        assert!(f.data.queries.len() <= 3);
        assert!(f.score("k19", 19.0) > 0.0, "newest kept");
        assert_eq!(f.score("k0", 19.0), 0.0, "oldest dropped");
    }

    #[test]
    fn forget_removes_item_and_associations() {
        let mut f = Frecency::in_memory();
        f.record("a", "q", 0.0);
        f.forget("a");
        assert_eq!(f.bonus("a", "q", 0.0), 0.0);
        assert!(f.is_empty());
    }

    #[test]
    fn save_in_memory_is_a_no_op() {
        let mut f = Frecency::in_memory();
        f.record("a", "", 0.0);
        f.save().unwrap();
        assert!(f.path().is_none());
    }
}
